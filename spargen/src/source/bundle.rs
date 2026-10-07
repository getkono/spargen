use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};

use super::lock::{Lock, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{
    classify_ref, collect_refs, enters_extension, resolve_ref_url, split_fragment, RefTarget,
};
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
    /// The working directory at load time, against which a relative local path is made absolute
    /// for [`local_identity`](Self::local_identity). Captured once, so identity is a pure function
    /// of the path for the bundle's whole life. `None` when it is unreadable or not UTF-8, and then
    /// relative paths are compared relative.
    working_dir: Option<Utf8PathBuf>,
}

impl InputBundle {
    /// Load the root document at `root`, pulling in referenced local files on demand and resolving
    /// remote `$ref`s from the vendored, hash-pinned copies recorded in `spargen.lock` (looked up
    /// next to `root`). No network access occurs. The parse format is chosen by extension
    /// (`.json` vs `.yaml`/`.yml`). Diagnostics flow through `diags`.
    ///
    /// A reference inside a specification extension is not followed, so a file it names is not
    /// read, unless a reference that is followed addresses the extension's contents: then that
    /// target is walked like a document of its own (see [`collect_refs`]).
    pub(crate) fn load(root: &Utf8Path, diags: &mut Diagnostics) -> Result<InputBundle, Aborted> {
        let mut bundle = InputBundle {
            working_dir: working_dir(),
            ..InputBundle::default()
        };

        let spec_dir = root.parent().unwrap_or_else(|| Utf8Path::new(""));
        bundle.vendor_dir = spec_dir.join(VENDOR_DIR);
        bundle.lock = load_lock(&spec_dir.join(LOCK_FILE_NAME), diags)?;

        let root_id = bundle.load_file(root.to_path_buf(), diags)?;
        bundle.root = Some(root_id);

        // Each entry is a value to walk: a loaded document's root, or a reference target inside a
        // specification extension, which the walk of its document skipped (`enters_extension`).
        let mut queue = VecDeque::from([(root_id, JsonPointer::root())]);
        let mut walked_targets = HashSet::new();
        while let Some((file, pointer)) = queue.pop_front() {
            let remote_base = match bundle.origins.get(&file) {
                Some(Origin::Remote(url)) => Some(url.clone()),
                _ => None,
            };
            // A dangling target is the resolver's to report, where a site interprets it (`E004`).
            let Some(value) = bundle.value_at(file).pointer(&pointer) else {
                continue;
            };
            let refs = collect_refs(value);
            for reference in &refs {
                match classify_ref(reference, remote_base.as_deref()) {
                    RefTarget::InDocument => {}
                    RefTarget::LocalRelative(path) => {
                        // Only a *local* document produces a local-relative target: a relative ref
                        // inside a vendored remote doc is classified `Remote` against its URL.
                        let resolved = bundle.resolve_path(file, &path);
                        if bundle.file_id_by_path(&resolved).is_none() {
                            let loaded = bundle.load_file(resolved, diags)?;
                            queue.push_back((loaded, JsonPointer::root()));
                        }
                    }
                    RefTarget::UnsupportedRemote(url) if bundle.url_to_file.contains_key(&url) => {}
                    RefTarget::UnsupportedRemote(url) => bundle.reject_unpinned(&url, file, diags),
                    RefTarget::Remote(url) => {
                        if bundle.url_to_file.contains_key(&url) {
                            continue;
                        }
                        if let Some(loaded) = bundle.load_remote(&url, file, diags)? {
                            queue.push_back((loaded, JsonPointer::root()));
                        }
                    }
                }
            }
            // Every document a reference names is loaded by now, so its target can be found.
            for reference in refs.iter().filter(|reference| enters_extension(reference)) {
                if let Some(target) = bundle.reference_target(reference, file) {
                    if walked_targets.insert(target.clone()) {
                        queue.push_back(target);
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

    /// Find a loaded file by its stored path: exact first, then relative to the root document's
    /// directory, and suffix last for ergonomic file-local omit rules.
    ///
    /// The relative step comes before the suffix one because a suffix is ambiguous — `lib.yaml` is
    /// a suffix of `xlib.yaml` — and the first match in load order would win. A path
    /// [`Self::root_relative_path`] produced always resolves at the relative step, to its own file.
    pub(crate) fn file_id_for_path(&self, path: &str) -> Option<FileId> {
        let relative = self.root_dir().join(path);
        self.files
            .iter()
            .find_map(|(id, file)| (file.path.as_str() == path).then_some(*id))
            .or_else(|| {
                self.files
                    .iter()
                    .find_map(|(id, file)| (file.path == relative).then_some(*id))
            })
            .or_else(|| {
                self.files
                    .iter()
                    .find_map(|(id, file)| file.path.as_str().ends_with(path).then_some(*id))
            })
    }

    /// The path a file-scoped omit rule names `file` by: its stored path relative to the root
    /// document's directory, or the stored path itself when it lies outside that directory. Either
    /// resolves back to `file` through [`Self::file_id_for_path`].
    ///
    /// Relative, so a rule auto-carve derives — and the omit fingerprint stamped into generated
    /// output — does not depend on where the checkout lives. The one exception is a file reached by
    /// an absolute-path `$ref`, whose location the description itself fixes.
    pub(crate) fn root_relative_path(&self, file: FileId) -> Option<&str> {
        let path = &self.file(file)?.path;
        Some(
            path.strip_prefix(self.root_dir())
                .map_or(path.as_str(), Utf8Path::as_str),
        )
    }

    /// Every loaded document's id, in load order (root first).
    pub(crate) fn file_ids(&self) -> impl Iterator<Item = FileId> + '_ {
        self.values.keys().copied()
    }

    /// The directory the root document was loaded from, against which relative paths resolve.
    fn root_dir(&self) -> &Utf8Path {
        self.file(self.root_id())
            .and_then(|root| root.path.parent())
            .unwrap_or_else(|| Utf8Path::new(""))
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
        // The document's base URI is where its fetch was retrieved from (RFC 3986 §5.1.3): after a
        // redirect, that and not `url` is what its relative references resolve against (#405).
        // A reference that reaches the document by its retrieval URL — its own fragment-only
        // references do, once made absolute — finds it there, unless that URL was loaded first.
        let base = entry.base_url();
        self.origins.insert(id, Origin::Remote(base.to_owned()));
        self.url_to_file.insert(url.to_owned(), id);
        self.url_to_file.entry(base.to_owned()).or_insert(id);
        self.register_self_identity(id, Some(base));
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

    /// The loaded local document `path` denotes, compared by [`Self::local_identity`] rather than
    /// by spelling: `openapi.yaml`, `./openapi.yaml`, `sub/../openapi.yaml` and the absolute path
    /// are one document, so a `$ref` naming the root by any of them reaches the root instead of
    /// loading it a second time (#220). The stored path is matched first, then a local `$self`
    /// identity.
    fn file_id_by_path(&self, path: &Utf8Path) -> Option<FileId> {
        let wanted = self.local_identity(path);
        self.files
            .iter()
            .find_map(|(id, file)| (self.local_identity(&file.path) == wanted).then_some(*id))
            .or_else(|| {
                self.origins.iter().find_map(|(id, origin)| match origin {
                    Origin::Local(identity) if self.local_identity(identity) == wanted => Some(*id),
                    Origin::Local(_) | Origin::Remote(_) => None,
                })
            })
    }

    /// The document identity of a local `path`, against the load-time working directory: see
    /// [`local_identity`].
    fn local_identity(&self, path: &Utf8Path) -> Utf8PathBuf {
        local_identity(self.working_dir.as_deref(), path)
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

/// The working directory a relative local path is made absolute against by [`local_identity`], or
/// `None` when it is unreadable or not UTF-8 (relative paths are then compared relative). The build
/// and `spargen lock` each capture it once, so identity is a pure function of the path for a run.
pub(super) fn working_dir() -> Option<Utf8PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|dir| Utf8PathBuf::from_path_buf(dir).ok())
}

/// The document identity of a local `path`: absolute against `working_dir` (see [`working_dir`]),
/// with `.` and `..` segments removed lexically — what RFC 3986 §5.2.4 does to a relative
/// reference resolved against its base URI, which is how a `$ref` names a document. Lexical
/// rather than `canonicalize`: it reads nothing from the filesystem, so it is deterministic and
/// usable where the bundle promises no I/O, and it identifies documents the way references do,
/// by URI, so a symlink is not followed. The build and `spargen lock` both key local documents by
/// it, so the lock reads exactly the local documents the build reads (#451).
pub(super) fn local_identity(working_dir: Option<&Utf8Path>, path: &Utf8Path) -> Utf8PathBuf {
    match working_dir {
        Some(dir) if path.is_relative() => normalize_lexically(&dir.join(path)),
        _ => normalize_lexically(path),
    }
}

/// `path` with every `.` segment dropped and every `..` segment cancelling the normal segment
/// before it. A `..` with nothing to cancel is kept on a relative path (`../a.yaml` names a file
/// outside the base) and dropped at a root (`/..` is `/`), as RFC 3986 §5.2.4 does.
fn normalize_lexically(path: &Utf8Path) -> Utf8PathBuf {
    use camino::Utf8Component;

    let mut normal: Vec<Utf8Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => match normal.last() {
                Some(Utf8Component::Normal(_)) => {
                    normal.pop();
                }
                Some(Utf8Component::RootDir | Utf8Component::Prefix(_)) => {}
                Some(Utf8Component::ParentDir | Utf8Component::CurDir) | None => {
                    normal.push(component);
                }
            },
            Utf8Component::Prefix(_) | Utf8Component::RootDir | Utf8Component::Normal(_) => {
                normal.push(component);
            }
        }
    }
    normal.into_iter().collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A root whose one schema reaches itself through the file, spelled `reference`.
    fn self_referring_root(reference: &str) -> String {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    Node:\n      oneOf:\n        - $ref: '{reference}#/components/schemas/Node'\n        \
             - type: 'null'\n"
        )
    }

    fn load(root: &Utf8Path) -> InputBundle {
        let mut diags = Diagnostics::default();
        InputBundle::load(root, &mut diags).expect("the bundle loads")
    }

    /// Every spelling of the root's own path is the root: one document, one `FileId`, and the
    /// reference resolves to it. A second load is what duplicated every component and made the
    /// shadow check report the root as shadowing itself (#220).
    #[test]
    fn a_reference_to_the_root_by_any_spelling_resolves_to_the_root() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let absolute = dir.join("openapi.yaml");
        for reference in [
            "./openapi.yaml".to_owned(),
            "openapi.yaml".to_owned(),
            "sub/../openapi.yaml".to_owned(),
            "./sub/.././openapi.yaml".to_owned(),
            absolute.to_string(),
        ] {
            std::fs::write(&absolute, self_referring_root(&reference)).unwrap();
            for root in [
                absolute.clone(),
                dir.join("sub/../openapi.yaml"),
                dir.join("./openapi.yaml"),
            ] {
                let bundle = load(&root);
                assert_eq!(
                    bundle.file_ids().count(),
                    1,
                    "root `{root}`, ref `{reference}`: the root was loaded again: {:?}",
                    bundle.source_paths().collect::<Vec<_>>()
                );
                let target = bundle.reference_target(
                    &format!("{reference}#/components/schemas/Node"),
                    bundle.root_id(),
                );
                assert_eq!(
                    target.map(|(file, _)| file),
                    Some(bundle.root_id()),
                    "root `{root}`, ref `{reference}`"
                );
                // The root keeps the spelling it was given: build invalidation and the paths omit
                // rules name files by read it.
                assert_eq!(bundle.source_paths().next(), Some(root.as_path()));
            }
        }
    }

    /// Normalisation joins spellings of one file; it never joins two files. Two spellings of a
    /// sub-file load it once, and a sibling of the same name in another directory stays its own
    /// document.
    #[test]
    fn spellings_of_one_sub_file_load_it_once_and_distinct_files_stay_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let schema = "components:\n  schemas:\n    Leaf: { type: string }\n";
        std::fs::write(dir.join("lib.yaml"), schema).unwrap();
        std::fs::write(dir.join("sub/lib.yaml"), schema).unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
             schemas:\n    A: { $ref: './lib.yaml#/components/schemas/Leaf' }\n    \
             B: { $ref: 'sub/../lib.yaml#/components/schemas/Leaf' }\n    \
             C: { $ref: 'sub/lib.yaml#/components/schemas/Leaf' }\n    \
             D: { $ref: './sub/./lib.yaml#/components/schemas/Leaf' }\n",
        )
        .unwrap();
        let bundle = load(&dir.join("openapi.yaml"));
        assert_eq!(
            bundle.file_ids().count(),
            3,
            "{:?}",
            bundle.source_paths().collect::<Vec<_>>()
        );
        let file = |reference: &str| {
            bundle
                .reference_target(reference, bundle.root_id())
                .map(|(file, _)| file)
        };
        assert_eq!(file("./lib.yaml#/x"), file("sub/../lib.yaml#/x"));
        assert_eq!(file("sub/lib.yaml#/x"), file("./sub/./lib.yaml#/x"));
        assert_ne!(file("./lib.yaml#/x"), file("sub/lib.yaml#/x"));
        assert_ne!(file("./lib.yaml#/x"), Some(bundle.root_id()));
    }

    /// A spec that names a redirect's target before the redirecting URL loads the target from its
    /// own pin, and the redirected pin, loaded second, leaves the target URL naming that document:
    /// the two pins hold different bytes here (the document changed between the fetches), and a
    /// reference by the target URL reaches the target's own pin, not the redirected copy.
    #[test]
    fn a_separately_pinned_retrieval_url_keeps_its_own_document() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let target = "https://h.example/new/pet.yaml";
        let redirecting = "https://h.example/old/pet.yaml";
        let own = "type: string\n";
        let redirected = "type: integer\n";
        let vendor = dir.join(VENDOR_DIR);
        std::fs::create_dir_all(vendor.join("h.example/new")).unwrap();
        std::fs::create_dir_all(vendor.join("h.example/old")).unwrap();
        std::fs::write(vendor.join("h.example/new/pet.yaml"), own).unwrap();
        std::fs::write(vendor.join("h.example/old/pet.yaml"), redirected).unwrap();
        std::fs::write(
            dir.join(LOCK_FILE_NAME),
            format!(
                "version = 1\n\n[[remote]]\nurl = \"{target}\"\nsha256 = \"{}\"\n\
                 path = \"h.example/new/pet.yaml\"\n\n[[remote]]\nurl = \"{redirecting}\"\n\
                 retrieval_url = \"{target}\"\nsha256 = \"{}\"\npath = \"h.example/old/pet.yaml\"\n",
                sha256_hex(own.as_bytes()),
                sha256_hex(redirected.as_bytes()),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            format!(
                "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
                 schemas:\n    A: {{ $ref: '{target}' }}\n    B: {{ $ref: '{redirecting}' }}\n"
            ),
        )
        .unwrap();

        let bundle = load(&dir.join("openapi.yaml"));

        let own_id = bundle.remote_file(target).expect("the target is loaded");
        let redirected_id = bundle
            .remote_file(redirecting)
            .expect("the redirecting URL is loaded");
        assert_ne!(own_id, redirected_id);
        assert_eq!(
            bundle
                .value_at(own_id)
                .get("type")
                .and_then(SpannedValue::as_str),
            Some("string")
        );
        assert_eq!(
            bundle
                .value_at(redirected_id)
                .get("type")
                .and_then(SpannedValue::as_str),
            Some("integer")
        );
    }

    #[test]
    fn lexical_normalisation_removes_dot_segments_and_keeps_leading_parents() {
        for (path, normal) in [
            ("a/./b/../c.yaml", "a/c.yaml"),
            ("./a.yaml", "a.yaml"),
            ("../../a.yaml", "../../a.yaml"),
            ("a/../../b.yaml", "../b.yaml"),
            ("/a/../../b.yaml", "/b.yaml"),
            ("/a/b/./../c.yaml", "/a/c.yaml"),
            (".", ""),
        ] {
            assert_eq!(
                normalize_lexically(Utf8Path::new(path)),
                Utf8PathBuf::from(normal),
                "{path}"
            );
        }
    }
}
