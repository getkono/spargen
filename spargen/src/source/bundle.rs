use std::collections::VecDeque;
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};

use super::lock::{Lock, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{classify_ref, collect_refs, resolve_ref_url, split_fragment, RefTarget};
use super::sha256::sha256_hex;
use super::{parse_json, parse_yaml, SpannedValue};

/// A single loaded source file.
#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    /// Path as loaded (relative to the bundle root, or the vendored path for a remote document).
    pub(crate) path: Utf8PathBuf,
    /// Exact bytes parsed from disk, retained for build-input fingerprinting.
    pub(crate) bytes: Arc<[u8]>,
}

/// Where a loaded file came from — used to resolve refs that appear *inside* it. A relative ref in a
/// local file resolves against its directory; a relative ref in a vendored remote document resolves
/// against that document's URL.
#[derive(Debug, Clone)]
enum Origin {
    Local(Utf8PathBuf),
    Remote(String),
}

/// An input bundle: the root document plus every document reachable through `$ref`s — local files
/// loaded on demand, and remote (`http`/`https`) documents resolved *only* from their locally
/// vendored, hash-pinned copies. Builds are hermetic: the bundle never touches the network. An
/// unpinned remote ref is rejected with a narrowed `E003` (run `spargen lock`), and a vendored copy
/// whose bytes disagree with the lock is refused as drift (`E021`) rather than silently used.
#[derive(Debug, Default)]
pub(crate) struct InputBundle {
    root: Option<FileId>,
    files: IndexMap<FileId, SourceFile>,
    values: IndexMap<FileId, SpannedValue>,
    origins: IndexMap<FileId, Origin>,
    /// Absolute base URL → the vendored file loaded for it, so a remote doc is loaded once.
    url_to_file: IndexMap<String, FileId>,
    /// The lock pinning remote documents, discovered next to the root spec (absent ⇒ no remote
    /// resolution, so any remote ref is unpinned).
    lock: Option<Lock>,
    /// Directory holding vendored remote documents (`.spargen/vendor/` next to the spec).
    vendor_dir: Utf8PathBuf,
}

impl InputBundle {
    /// Load the root document at `root`, pulling in referenced local files on demand and resolving
    /// remote `$ref`s from the vendored, hash-pinned copies recorded in `spargen.lock` (looked up
    /// next to `root`). No network access occurs. The parse format is chosen by extension
    /// (`.json` vs `.yaml`/`.yml`). Diagnostics flow through `diags`.
    pub(crate) fn load(root: &Utf8Path, diags: &mut Diagnostics) -> Result<InputBundle, Aborted> {
        let mut bundle = InputBundle::default();

        let spec_dir = root.parent().unwrap_or_else(|| Utf8Path::new(""));
        bundle.vendor_dir = spec_dir.join(VENDOR_DIR);
        bundle.lock = load_lock(&spec_dir.join(LOCK_FILE_NAME), diags)?;

        let root_id = bundle.load_file(root.to_path_buf(), diags)?;
        bundle.root = Some(root_id);

        let mut queue = VecDeque::from([root_id]);
        while let Some(file) = queue.pop_front() {
            let remote_base = match bundle.origins.get(&file) {
                Some(Origin::Remote(url)) => Some(url.clone()),
                _ => None,
            };
            let refs = collect_refs(bundle.value_at(file));
            for reference in refs {
                match classify_ref(&reference, remote_base.as_deref()) {
                    RefTarget::InDocument => {}
                    RefTarget::LocalRelative(path) => {
                        // Only a *local* document produces a local-relative target: a relative ref
                        // inside a vendored remote doc is classified `Remote` against its URL.
                        let resolved = bundle.resolve_path(file, &path);
                        if bundle.file_id_by_path(&resolved).is_none() {
                            let loaded = bundle.load_file(resolved, diags)?;
                            queue.push_back(loaded);
                        }
                    }
                    RefTarget::UnsupportedRemote(url) if bundle.url_to_file.contains_key(&url) => {}
                    RefTarget::UnsupportedRemote(url) => bundle.reject_unpinned(&url, file, diags),
                    RefTarget::Remote(url) => {
                        if bundle.url_to_file.contains_key(&url) {
                            continue;
                        }
                        if let Some(loaded) = bundle.load_remote(&url, file, diags)? {
                            queue.push_back(loaded);
                        }
                    }
                }
            }
        }

        diags.result(bundle)
    }

    /// The root document's value tree.
    pub(crate) fn root(&self) -> &SpannedValue {
        self.value_at(self.root.expect("input bundle root is loaded"))
    }

    /// The value tree of a loaded `file`.
    pub(crate) fn value_at(&self, file: FileId) -> &SpannedValue {
        self.values
            .get(&file)
            .expect("file id came from this input bundle")
    }

    /// Mutable value tree of a loaded `file`.
    pub(crate) fn value_at_mut(&mut self, file: FileId) -> &mut SpannedValue {
        self.values
            .get_mut(&file)
            .expect("file id came from this input bundle")
    }

    /// The loaded record for `file`, if present.
    pub(crate) fn file(&self, file: FileId) -> Option<&SourceFile> {
        self.files.get(&file)
    }

    /// The root document id.
    pub(crate) fn root_id(&self) -> FileId {
        self.root.expect("input bundle root is loaded")
    }

    /// The vendored document loaded for an absolute base `url`, if it was pinned and loaded.
    pub(crate) fn remote_file(&self, url: &str) -> Option<FileId> {
        self.url_to_file.get(url).copied()
    }

    /// Resolve a `$ref` from `from` to a loaded document and JSON Pointer. All reachable files were
    /// loaded during bundle construction, so this performs no I/O and never reaches the network.
    pub(crate) fn reference_target(
        &self,
        reference: &str,
        from: FileId,
    ) -> Option<(FileId, JsonPointer)> {
        let (_, fragment) = split_fragment(reference);
        let remote_base = match self.origins.get(&from) {
            Some(Origin::Remote(url)) => Some(url.as_str()),
            _ => None,
        };
        let file = match classify_ref(reference, remote_base) {
            RefTarget::InDocument => from,
            RefTarget::LocalRelative(path) => {
                let path = self.resolve_path(from, &path);
                self.file_id_by_path(&path)?
            }
            RefTarget::Remote(url) => self.remote_file(&url)?,
            RefTarget::UnsupportedRemote(uri) => self.url_to_file.get(&uri).copied()?,
        };
        let pointer = if fragment.is_empty() {
            JsonPointer::root()
        } else if fragment.starts_with('/') {
            JsonPointer::from(fragment.to_owned())
        } else {
            // A non-empty fragment that is not a JSON Pointer is a named JSON Schema anchor
            // (`$anchor`), which spargen rejects rather than resolves (`E004`). Do not mistake one
            // for a pointer.
            return None;
        };
        Some((file, pointer))
    }

    /// The retrieval URL for a vendored remote document.
    pub(crate) fn remote_origin(&self, file: FileId) -> Option<&str> {
        match self.origins.get(&file) {
            Some(Origin::Remote(url)) => Some(url),
            _ => None,
        }
    }

    /// The filesystem paths of every loaded document: the root spec, each relative-file `$ref`
    /// target reachable from it, and each vendored remote copy. Used for build invalidation and
    /// macro dependency tracking. Order follows load order (root first).
    pub(crate) fn source_paths(&self) -> impl Iterator<Item = &Utf8Path> {
        self.files.values().map(|file| file.path.as_path())
    }

    /// Every loaded document and the exact bytes used to parse it.
    pub(crate) fn source_inputs(&self) -> impl Iterator<Item = (&Utf8Path, &[u8])> {
        self.files
            .values()
            .map(|file| (file.path.as_path(), file.bytes.as_ref()))
    }

    /// Find a loaded file by its stored path, exact first and suffix second for ergonomic
    /// file-local omit rules.
    pub(crate) fn file_id_for_path(&self, path: &str) -> Option<FileId> {
        self.files
            .iter()
            .find_map(|(id, file)| (file.path.as_str() == path).then_some(*id))
            .or_else(|| {
                self.files
                    .iter()
                    .find_map(|(id, file)| file.path.as_str().ends_with(path).then_some(*id))
            })
    }
}

impl InputBundle {
    fn load_file(&mut self, path: Utf8PathBuf, diags: &mut Diagnostics) -> Result<FileId, Aborted> {
        if let Some(id) = self.file_id_by_path(&path) {
            return Ok(id);
        }
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(format!("failed to read `{path}`: {error}"))
            .emit(diags);
            Aborted
        })?;
        let id = FileId(self.files.len() as u32);
        let parsed = parse_by_name(id, path.as_str(), &text, diags)?;
        self.files.insert(
            id,
            SourceFile {
                path: path.clone(),
                bytes: Arc::<[u8]>::from(text.as_bytes()),
            },
        );
        self.values.insert(id, parsed);
        self.origins.insert(id, Origin::Local(path));
        self.register_self_identity(id, None);
        Ok(id)
    }

    /// Resolve a remote `$ref` base `url` from its vendored, hash-pinned copy. Returns the loaded
    /// file id, or `None` when the ref is unpinned (`E003`) or the vendored bytes drift from the
    /// lock (`E021`) — in either case a diagnostic is emitted and the load ultimately aborts. Never
    /// performs network I/O.
    fn load_remote(
        &mut self,
        url: &str,
        referrer: FileId,
        diags: &mut Diagnostics,
    ) -> Result<Option<FileId>, Aborted> {
        let Some(entry) = self.lock.as_ref().and_then(|lock| lock.get(url)).cloned() else {
            self.reject_unpinned(url, referrer, diags);
            return Ok(None);
        };
        let vendored = self.vendor_dir.join(&entry.path);
        let bytes = match std::fs::read(&vendored) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.reject_drift(
                    referrer,
                    format!("vendored file for `{url}` is missing or unreadable at `{vendored}`: {error}"),
                    diags,
                );
                return Ok(None);
            }
        };
        let actual = sha256_hex(&bytes);
        if actual != entry.sha256 {
            self.reject_drift(
                referrer,
                format!(
                    "vendored content for `{url}` does not match the pinned sha256 \
                     (lock `{}`, on-disk `{actual}`)",
                    entry.sha256
                ),
                diags,
            );
            return Ok(None);
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                Diagnostic::error(
                    Code::InvalidInput,
                    Provenance::new(JsonPointer::root(), None),
                )
                .message(format!("vendored file for `{url}` is not valid UTF-8"))
                .emit(diags);
                return Ok(None);
            }
        };
        let id = FileId(self.files.len() as u32);
        let parsed = parse_by_name(id, &entry.path, &text, diags)?;
        self.files.insert(
            id,
            SourceFile {
                path: vendored,
                bytes: Arc::<[u8]>::from(text.as_bytes()),
            },
        );
        self.values.insert(id, parsed);
        self.origins.insert(id, Origin::Remote(url.to_owned()));
        self.url_to_file.insert(url.to_owned(), id);
        self.register_self_identity(id, Some(url));
        Ok(Some(id))
    }

    fn reject_unpinned(&self, url: &str, referrer: FileId, diags: &mut Diagnostics) {
        Diagnostic::error(
            Code::AbsoluteRefUnsupported,
            Provenance::new(JsonPointer::root(), Some(self.value_at(referrer).span())),
        )
        .message(format!(
            "remote $ref `{url}` is not pinned in {LOCK_FILE_NAME}"
        ))
        .remedy(format!(
            "run `spargen lock <spec>` to fetch, vendor, and pin `{url}`"
        ))
        .emit(diags);
    }

    fn reject_drift(&self, referrer: FileId, message: String, diags: &mut Diagnostics) {
        Diagnostic::error(
            Code::VendoredRefDrift,
            Provenance::new(JsonPointer::root(), Some(self.value_at(referrer).span())),
        )
        .message(message)
        .remedy(format!(
            "re-run `spargen lock <spec>` to re-vendor, or restore the vendored file under {VENDOR_DIR}"
        ))
        .emit(diags);
    }

    fn file_id_by_path(&self, path: &Utf8Path) -> Option<FileId> {
        self.files
            .iter()
            .find_map(|(id, file)| (file.path == path).then_some(*id))
            .or_else(|| {
                self.origins.iter().find_map(|(id, origin)| match origin {
                    Origin::Local(identity) if identity == path => Some(*id),
                    Origin::Local(_) | Origin::Remote(_) => None,
                })
            })
    }

    fn resolve_path(&self, base: FileId, path: &str) -> Utf8PathBuf {
        let base_path = match self.origins.get(&base) {
            Some(Origin::Local(path)) => path,
            _ => &self.file(base).expect("base file exists").path,
        };
        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
        parent.join(path)
    }

    /// Register an OpenAPI 3.2 `$self` identity and make it the base for subsequent references.
    /// Relative identities in local documents are resolved from the retrieval path; identities in
    /// remote documents are resolved from the pinned retrieval URL.
    fn register_self_identity(&mut self, file: FileId, retrieval_url: Option<&str>) {
        let Some(self_uri) = self
            .value_at(file)
            .get("$self")
            .and_then(SpannedValue::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let (base, _) = split_fragment(&self_uri);
        if base.is_empty() {
            return;
        }
        if let Some(retrieval_url) = retrieval_url {
            let canonical = resolve_ref_url(retrieval_url, base);
            self.origins.insert(file, Origin::Remote(canonical.clone()));
            self.url_to_file.insert(canonical, file);
        } else if base.starts_with("http://") || base.starts_with("https://") {
            self.origins.insert(file, Origin::Remote(base.to_owned()));
            self.url_to_file.insert(base.to_owned(), file);
        } else if !base.contains(':') {
            let canonical = self.resolve_path(file, base);
            self.origins.insert(file, Origin::Local(canonical));
        } else {
            // Opaque canonical identities can still resolve exact absolute references back to this
            // document. They cannot supply a hierarchical base for relative reference resolution.
            self.url_to_file.insert(base.to_owned(), file);
        }
    }
}

/// Parse `text` into a value tree, choosing the format from `name`'s `.json`/`.yaml`/`.yml`
/// extension (falling back to YAML-then-JSON).
fn parse_by_name(
    id: FileId,
    name: &str,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    match Utf8Path::new(name).extension() {
        Some("json") => parse_json(id, text, diags),
        Some("yaml" | "yml") => parse_yaml(id, text, diags),
        _ => parse_yaml(id, text, diags).or_else(|_| parse_json(id, text, diags)),
    }
}

/// Read and parse the lock next to the spec, if present. A missing lock is fine (no remote refs, or
/// they will be reported as unpinned); a malformed lock is a hard error.
fn load_lock(path: &Utf8Path, diags: &mut Diagnostics) -> Result<Option<Lock>, Aborted> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(format!("failed to read `{path}`: {error}"))
            .emit(diags);
            return Err(Aborted);
        }
    };
    match Lock::parse(&text) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(format!("invalid {LOCK_FILE_NAME}: {error}"))
            .emit(diags);
            Err(Aborted)
        }
    }
}
