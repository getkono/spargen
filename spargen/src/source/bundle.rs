use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};

use super::lock::{Lock, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{
    classify_ref, classify_self, collect_refs, enters_extension, split_fragment, CollectedRef,
    RefTarget, SelfIdentity,
};
use super::sha256::sha256_hex;
use super::{parse_json, parse_yaml, SpannedValue};

/// A single loaded source file.
#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    /// Path as loaded: the root spec's path as given to [`InputBundle::load`]; for a relative-file
    /// `$ref` target, the referring document's directory (or its local `$self` base) joined with
    /// the reference; for a remote document, its pinned `path` joined onto the vendor directory.
    /// [`InputBundle::root_relative_path`] derives the root-relative spelling from it.
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

/// Why [`InputBundle::file_id_for_path`] found no single file for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileLookupError {
    /// No loaded file is named by the path.
    Unloaded,
    /// Two or more loaded files end in the path's components: each one's
    /// [`InputBundle::root_relative_path`], in load order.
    Ambiguous(Vec<String>),
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
    /// Every file the load read or tried to read, in order: what
    /// [`load_recording`](Self::load_recording) hands back.
    read: Vec<Utf8PathBuf>,
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
        Self::load_recording(root, diags, &mut Vec::new())
    }

    /// [`Self::load`], appending to `read` every file the load read or tried to read, whether or
    /// not it succeeds: `spargen.lock` beside `root` when it exists, each local document in the
    /// order it was reached (the root, then each relative-file `$ref` target, the one whose read
    /// or parse failed included), and each vendored copy of a pinned remote document. A build
    /// names them to Cargo when the load fails, so fixing the file that failed it reruns the build.
    pub(crate) fn load_recording(
        root: &Utf8Path,
        diags: &mut Diagnostics,
        read: &mut Vec<Utf8PathBuf>,
    ) -> Result<InputBundle, Aborted> {
        let mut bundle = InputBundle {
            working_dir: working_dir(),
            ..InputBundle::default()
        };
        let walked = bundle.walk(root, diags);
        read.append(&mut bundle.read);
        walked?;
        diags.result(bundle)
    }

    /// Load the root document at `root` into this empty bundle, then every document reachable
    /// from it: the body of [`Self::load_recording`].
    fn walk(&mut self, root: &Utf8Path, diags: &mut Diagnostics) -> Result<(), Aborted> {
        let spec_dir = root.parent().unwrap_or_else(|| Utf8Path::new(""));
        self.vendor_dir = spec_dir.join(VENDOR_DIR);
        let lock_path = spec_dir.join(LOCK_FILE_NAME);
        if lock_path.exists() {
            self.read.push(lock_path.clone());
        }
        self.lock = load_lock(&lock_path, diags)?;

        // The root document is read before any reference is walked, so a failure to read it is
        // about the input as a whole and is located at its root.
        let root_site = Provenance::new(JsonPointer::root(), None);
        let root_id = self.load_file(root.to_path_buf(), root_site, diags)?;
        self.root = Some(root_id);

        // Each entry is a value to walk: a loaded document's root, or a reference target inside a
        // specification extension, which the walk of its document skipped (`enters_extension`).
        let mut queue = VecDeque::from([(root_id, JsonPointer::root())]);
        let mut walked_targets = HashSet::new();
        while let Some((file, pointer)) = queue.pop_front() {
            let remote_base = match self.origins.get(&file) {
                Some(Origin::Remote(url)) => Some(url.clone()),
                _ => None,
            };
            // A dangling target is the resolver's to report, where a site interprets it (`E004`).
            let Some(value) = self.value_at(file).pointer(&pointer) else {
                continue;
            };
            let refs = collect_refs(value, &pointer);
            for collected in &refs {
                match classify_ref(&collected.reference, remote_base.as_deref()) {
                    RefTarget::InDocument => {}
                    RefTarget::LocalRelative(path) => {
                        // Only a *local* document produces a local-relative target: a relative ref
                        // inside a vendored remote doc is classified `Remote` against its URL.
                        let resolved = self.resolve_path(file, &path);
                        if self.file_id_by_path(&resolved).is_none() {
                            let site = referring_site(collected);
                            let loaded = self.load_file(resolved, site, diags)?;
                            queue.push_back((loaded, JsonPointer::root()));
                        }
                    }
                    RefTarget::UnsupportedRemote(url) if self.url_to_file.contains_key(&url) => {}
                    RefTarget::UnsupportedRemote(url) => {
                        reject_unpinned(&url, referring_site(collected), diags)
                    }
                    RefTarget::Remote(url) => {
                        if self.url_to_file.contains_key(&url) {
                            continue;
                        }
                        let site = referring_site(collected);
                        if let Some(loaded) = self.load_remote(&url, site, diags)? {
                            queue.push_back((loaded, JsonPointer::root()));
                        }
                    }
                }
            }
            // Every document a reference names is loaded by now, so its target can be found.
            for collected in refs
                .iter()
                .filter(|collected| enters_extension(&collected.reference))
            {
                if let Some(target) = self.reference_target(&collected.reference, file) {
                    if walked_targets.insert(target.clone()) {
                        queue.push_back(target);
                    }
                }
            }
        }

        Ok(())
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

    /// The `http`/`https` base URL of `file`, which its relative references resolve against. For a
    /// vendored remote document that is its `http`/`https` (or relative) `$self`, resolved against
    /// the retrieval URL, where it has one, and otherwise the retrieval URL itself (the pin's
    /// `retrieval_url`, or its `url` when the fetch was not redirected). For a local document it is
    /// its `$self` when that is an absolute `http`/`https` URL. `None` otherwise. See
    /// [`Self::register_self_identity`].
    pub(crate) fn remote_origin(&self, file: FileId) -> Option<&str> {
        match self.origins.get(&file) {
            Some(Origin::Remote(url)) => Some(url),
            _ => None,
        }
    }

    /// Every loaded document and the exact bytes used to parse it: the root spec, each
    /// relative-file `$ref` target reachable from it, and each vendored remote copy. The build
    /// cache's input fingerprint and the macro's dependency tracking read it. Order follows load
    /// order (root first).
    pub(crate) fn source_inputs(&self) -> impl Iterator<Item = (&Utf8Path, &[u8])> {
        self.files
            .values()
            .map(|file| (file.path.as_path(), file.bytes.as_ref()))
    }

    /// Find a loaded file by the path a file-scoped omit rule names it by: its stored path exactly,
    /// then its path relative to the root document's directory, and last any loaded file whose
    /// stored path ends in `path`'s whole components, for ergonomic file-local omit rules.
    ///
    /// The last step compares path components, never strings, so `pet.yaml` names
    /// `schemas/pet.yaml` and not `schemas/legacy-pet.yaml`. It is the only step two files can
    /// meet at (`a/pet.yaml` and `b/pet.yaml` both end in `pet.yaml`, and so does a vendored
    /// `.spargen/vendor/<host>/…/pet.yaml`); then the lookup is [`FileLookupError::Ambiguous`],
    /// never the first in load order. A path [`Self::root_relative_path`] produced always resolves
    /// at the relative step, to its own file.
    pub(crate) fn file_id_for_path(&self, path: &str) -> Result<FileId, FileLookupError> {
        let relative = self.root_dir().join(path);
        if let Some(id) = self
            .files
            .iter()
            .find_map(|(id, file)| (file.path.as_str() == path).then_some(*id))
            .or_else(|| {
                self.files
                    .iter()
                    .find_map(|(id, file)| (file.path == relative).then_some(*id))
            })
        {
            return Ok(id);
        }
        let suffix = Utf8Path::new(path);
        // An empty path ends every path; it names no file.
        if suffix.components().next().is_none() {
            return Err(FileLookupError::Unloaded);
        }
        let matches: Vec<FileId> = self
            .files
            .iter()
            .filter(|(_, file)| file.path.ends_with(suffix))
            .map(|(id, _)| *id)
            .collect();
        match matches.as_slice() {
            [] => Err(FileLookupError::Unloaded),
            [id] => Ok(*id),
            _ => Err(FileLookupError::Ambiguous(
                matches
                    .iter()
                    .filter_map(|id| self.root_relative_path(*id))
                    .map(str::to_owned)
                    .collect(),
            )),
        }
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
    /// Load the local document at `path`, unless it is already loaded. A failure to read it is
    /// reported (`E011`) at `site`: the reference that named `path`, or the root for the root
    /// document itself.
    fn load_file(
        &mut self,
        path: Utf8PathBuf,
        site: Provenance,
        diags: &mut Diagnostics,
    ) -> Result<FileId, Aborted> {
        if let Some(id) = self.file_id_by_path(&path) {
            return Ok(id);
        }
        self.read.push(path.clone());
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Diagnostic::error(Code::InvalidInput, site)
                .message(format!("failed to read `{path}`: {error}"))
                .emit(diags);
            Aborted
        })?;
        let id = FileId(self.files.len() as u32);
        diags.register_file(id, path.clone());
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
    /// file id, or `None` when the ref is unpinned (`E003`), the vendored bytes drift from the
    /// lock (`E021`), or they are not UTF-8 (`E011`) — in each case a diagnostic is emitted at
    /// `site`, the reference that named `url`, and the load ultimately aborts. Never performs
    /// network I/O.
    fn load_remote(
        &mut self,
        url: &str,
        site: Provenance,
        diags: &mut Diagnostics,
    ) -> Result<Option<FileId>, Aborted> {
        let Some(entry) = self.lock.as_ref().and_then(|lock| lock.get(url)).cloned() else {
            reject_unpinned(url, site, diags);
            return Ok(None);
        };
        let vendored = self.vendor_dir.join(&entry.path);
        self.read.push(vendored.clone());
        let bytes = match std::fs::read(&vendored) {
            Ok(bytes) => bytes,
            Err(error) => {
                reject_drift(
                    site,
                    format!("vendored file for `{url}` is missing or unreadable at `{vendored}`: {error}"),
                    diags,
                );
                return Ok(None);
            }
        };
        let actual = sha256_hex(&bytes);
        if actual != entry.sha256 {
            reject_drift(
                site,
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
                Diagnostic::error(Code::InvalidInput, site)
                    .message(format!("vendored file for `{url}` is not valid UTF-8"))
                    .emit(diags);
                return Ok(None);
            }
        };
        let id = FileId(self.files.len() as u32);
        diags.register_file(id, vendored.clone());
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
        let parent = self
            .base_path(base)
            .parent()
            .unwrap_or_else(|| Utf8Path::new(""));
        parent.join(path)
    }

    /// The local path `file`'s relative paths resolve against: its local `$self` identity, or the
    /// path it was loaded from.
    fn base_path(&self, file: FileId) -> &Utf8Path {
        match self.origins.get(&file) {
            Some(Origin::Local(path)) => path,
            _ => &self.file(file).expect("base file exists").path,
        }
    }

    /// Register an OpenAPI 3.2 `$self` identity, a name a reference reaches `file` by. An
    /// `http`/`https` identity, or a relative one, also becomes the base its relative references
    /// resolve against: a relative identity is resolved from the document's own path when it is
    /// local, and from the URL it was retrieved from when it is remote. Any other identity with a
    /// scheme (`urn:x:y`) is registered as written, in a remote document as in a local one, and
    /// supplies no hierarchical base, so the document keeps the one it was reached at.
    fn register_self_identity(&mut self, file: FileId, retrieval_url: Option<&str>) {
        let Some(self_uri) = self
            .value_at(file)
            .get("$self")
            .and_then(SpannedValue::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let identity = classify_self(&self_uri, retrieval_url, self.base_path(file));
        match identity {
            None => {}
            Some(SelfIdentity::Remote(canonical)) => {
                self.origins.insert(file, Origin::Remote(canonical.clone()));
                self.url_to_file.insert(canonical, file);
            }
            Some(SelfIdentity::Local(canonical)) => {
                self.origins.insert(file, Origin::Local(canonical));
            }
            Some(SelfIdentity::Opaque(uri)) => {
                self.url_to_file.insert(uri, file);
            }
        }
    }
}

/// Where a diagnostic about `collected`'s target is raised: at the reference that names it, in
/// the document holding it, rather than at that document's root (#534).
fn referring_site(collected: &CollectedRef) -> Provenance {
    Provenance::new(collected.pointer.clone(), Some(collected.span))
}

/// Reject a remote reference to `url` that `spargen.lock` does not pin (`E003`), at `site`.
fn reject_unpinned(url: &str, site: Provenance, diags: &mut Diagnostics) {
    Diagnostic::error(Code::AbsoluteRefUnsupported, site)
        .message(format!(
            "remote $ref `{url}` is not pinned in {LOCK_FILE_NAME}"
        ))
        .remedy(format!(
            "run `spargen lock <spec>` to fetch, vendor, and pin `{url}`"
        ))
        .emit(diags);
}

/// Reject a pinned remote reference whose vendored copy is missing or drifted (`E021`), at `site`.
fn reject_drift(site: Provenance, message: String, diags: &mut Diagnostics) {
    Diagnostic::error(Code::VendoredRefDrift, site)
        .message(message)
        .remedy(format!(
            "re-run `spargen lock <spec>` to re-vendor, or restore the vendored file under {VENDOR_DIR}"
        ))
        .emit(diags);
}

/// Parse `text` into a value tree, choosing the format from `name`'s extension
/// ([`parse_by_extension`]).
pub(super) fn parse_by_name(
    id: FileId,
    name: &str,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    parse_by_extension(id, Utf8Path::new(name).extension(), text, diags)
}

/// The UTF-8 byte-order mark [`parse_by_extension`] skips at the start of a document.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// Parse `text` in the format `extension` names, compared ASCII case-insensitively: `json` as
/// JSON, `yaml` or `yml` as YAML, and any other extension, or none, as YAML then JSON. The build
/// ([`parse_by_name`]) and `spargen lock` (`vendor::parse_scratch_local` and
/// `vendor::parse_scratch_remote`) each read the extension from a document's name and both choose
/// the format here, so an upper- or mixed-case extension such as `.JSON` names the same format to
/// both (#915).
///
/// One leading UTF-8 byte-order mark (U+FEFF), which some Windows tooling writes, is skipped
/// before either parser runs, as RFC 8259 §8.1 allows (#1425): it is an encoding signature, not
/// the document's first character, so spans count from the character after it. Only the parse
/// skips it; the bytes a lock pins and a build fingerprints are the file's own, mark included.
pub(super) fn parse_by_extension(
    id: FileId,
    extension: Option<&str>,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    let text = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(text);
    match extension {
        Some(ext) if ext.eq_ignore_ascii_case("json") => parse_json(id, text, diags),
        Some(ext) if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") => {
            parse_yaml(id, text, diags)
        }
        _ => parse_yaml_then_json(id, text, diags),
    }
}

/// Parse a document whose name carries no recognised extension: as YAML, then as JSON (#717).
/// Each attempt collects into scratch diagnostics, and only the kept attempt's are replayed into
/// `diags`: the parse that succeeded, or YAML's when both fail. A failed YAML attempt therefore
/// does not abort a document JSON accepts, and a document neither accepts reports only the YAML
/// attempt's diagnostic rather than one per attempt.
/// The build ([`parse_by_name`]) and `spargen lock` both take this fallback.
pub(super) fn parse_yaml_then_json(
    id: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    let mut yaml = Diagnostics::default();
    let parsed = parse_yaml(id, text, &mut yaml);
    let (parsed, kept) = match parsed {
        Ok(value) => (Ok(value), yaml),
        Err(_) => {
            let mut json = Diagnostics::default();
            match parse_json(id, text, &mut json) {
                Ok(value) => (Ok(value), json),
                Err(aborted) => (Err(aborted), yaml),
            }
        }
    };
    for diagnostic in kept.items() {
        diags.emit(diagnostic.clone());
    }
    parsed
}

/// The working directory a relative local path is made absolute against by [`local_identity`], or
/// `None` when it is unreadable or not UTF-8 (relative paths are then compared relative). The build
/// and `spargen lock` each capture it once, so identity is a pure function of the path for a run.
pub(crate) fn working_dir() -> Option<Utf8PathBuf> {
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
pub(crate) fn local_identity(working_dir: Option<&Utf8Path>, path: &Utf8Path) -> Utf8PathBuf {
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

    /// Every format `parse_by_extension` selects, the build's and `spargen lock`'s alike, skips
    /// one leading byte-order mark and reads the same tree as without it, spans included, and a
    /// second mark is the document's own first character (#1425).
    #[test]
    fn parse_by_extension_skips_one_leading_byte_order_mark_in_every_format() {
        let yaml = "a: 1\n";
        let json = r#"{"a": 1}"#;
        for (extension, body) in [
            (Some("yaml"), yaml),
            (Some("YML"), yaml),
            (Some("json"), json),
            (None, yaml),
            (Some("txt"), json),
        ] {
            let parse = |text: &str| {
                let mut diags = Diagnostics::default();
                parse_by_extension(FileId(0), extension, text, &mut diags).ok()
            };
            let plain = parse(body).expect("parses without a mark");
            let marked = parse(&format!("{BYTE_ORDER_MARK}{body}"));
            assert_eq!(marked, Some(plain), "{extension:?}");
            let twice = parse(&format!("{BYTE_ORDER_MARK}{BYTE_ORDER_MARK}{body}"));
            assert_ne!(twice, marked, "{extension:?}: a second mark is not skipped");
        }
    }

    /// The bytes a build fingerprints are the file's own, mark included: only the parse skips it.
    #[test]
    fn a_loaded_document_keeps_its_byte_order_mark_in_its_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let text = format!(
            "{BYTE_ORDER_MARK}openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n"
        );
        std::fs::write(dir.join("openapi.yaml"), &text).unwrap();
        let bundle = load(&dir.join("openapi.yaml"));
        let root = bundle.file(bundle.root_id()).expect("the root is loaded");
        assert_eq!(&*root.bytes, text.as_bytes());
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
                    bundle
                        .source_inputs()
                        .map(|(path, _)| path)
                        .collect::<Vec<_>>()
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
                assert_eq!(
                    bundle.source_inputs().next().map(|(path, _)| path),
                    Some(root.as_path())
                );
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
            bundle
                .source_inputs()
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
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

    /// A failed load records every file it read or tried to read, so a build that names them to
    /// Cargo reruns once the broken one is fixed (#1350): the lock, the root, a sub-file that
    /// loaded, and the `$ref` target that failed, whether it failed to parse or to be read.
    #[test]
    fn a_failed_load_records_every_file_it_read_and_the_failing_target() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::create_dir(dir.join("schemas")).unwrap();
        std::fs::write(dir.join(LOCK_FILE_NAME), "version = 1\n").unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
             schemas:\n    A: { $ref: 'schemas/a.yaml' }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("schemas/a.yaml"),
            "type: object\nproperties:\n  pet: { $ref: 'pet.yaml' }\n",
        )
        .unwrap();
        let expected = [
            dir.join(LOCK_FILE_NAME),
            dir.join("openapi.yaml"),
            dir.join("schemas/a.yaml"),
            dir.join("schemas/pet.yaml"),
        ];

        // Missing, then present and malformed: each fails the load, and each names the target.
        for pet in [None, Some("type: [unclosed\n")] {
            if let Some(contents) = pet {
                std::fs::write(dir.join("schemas/pet.yaml"), contents).unwrap();
            }
            let mut diags = Diagnostics::default();
            let mut read = Vec::new();
            let loaded =
                InputBundle::load_recording(&dir.join("openapi.yaml"), &mut diags, &mut read);
            assert!(loaded.is_err(), "{pet:?}: {:?}", diags.items());
            assert_eq!(read, expected, "{pet:?}");
        }

        // A load that succeeds records the same files.
        std::fs::write(dir.join("schemas/pet.yaml"), "type: string\n").unwrap();
        let mut diags = Diagnostics::default();
        let mut read = Vec::new();
        InputBundle::load_recording(&dir.join("openapi.yaml"), &mut diags, &mut read)
            .expect("the bundle loads");
        assert_eq!(read, expected);
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

    /// A hand-edited `retrieval_url` holding a fragment is read without it: the pinned document
    /// is keyed by, and resolves its relative `c.yaml` against, `https://h.example/new/b.yaml`,
    /// so the reference reaches `https://h.example/new/c.yaml` rather than
    /// `https://h.example/new/b.yaml#/x/c.yaml`, which no pin names.
    #[test]
    fn a_fragment_in_a_pinned_retrieval_url_is_not_part_of_the_base() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let redirecting = "https://h.example/old/b.yaml";
        let sibling = "https://h.example/new/c.yaml";
        let moved = "$ref: c.yaml\n";
        let target = "type: string\n";
        let vendor = dir.join(VENDOR_DIR);
        std::fs::create_dir_all(vendor.join("h.example/old")).unwrap();
        std::fs::create_dir_all(vendor.join("h.example/new")).unwrap();
        std::fs::write(vendor.join("h.example/old/b.yaml"), moved).unwrap();
        std::fs::write(vendor.join("h.example/new/c.yaml"), target).unwrap();
        std::fs::write(
            dir.join(LOCK_FILE_NAME),
            format!(
                "version = 1\n\n[[remote]]\nurl = \"{sibling}\"\nsha256 = \"{}\"\n\
                 path = \"h.example/new/c.yaml\"\n\n[[remote]]\nurl = \"{redirecting}\"\n\
                 retrieval_url = \"https://h.example/new/b.yaml#/x/y\"\nsha256 = \"{}\"\n\
                 path = \"h.example/old/b.yaml\"\n",
                sha256_hex(target.as_bytes()),
                sha256_hex(moved.as_bytes()),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            format!(
                "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
                 schemas:\n    B: {{ $ref: '{redirecting}' }}\n"
            ),
        )
        .unwrap();

        let bundle = load(&dir.join("openapi.yaml"));

        let moved_id = bundle.remote_file(redirecting).expect("the pin is loaded");
        assert_eq!(
            bundle.remote_file("https://h.example/new/b.yaml"),
            Some(moved_id)
        );
        let sibling_id = bundle.remote_file(sibling).expect("the sibling is loaded");
        assert_eq!(
            bundle.reference_target("c.yaml", moved_id),
            Some((sibling_id, JsonPointer::root()))
        );
        assert_eq!(
            bundle.remote_file("https://h.example/new/b.yaml#/x/c.yaml"),
            None
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

    /// A root in `dir` whose schemas `$ref` each of `files` (relative or absolute) in order, so
    /// they load in that order after the root.
    fn root_referring_to(dir: &Utf8Path, files: &[&str]) -> InputBundle {
        let mut root = String::from(
            "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
             schemas:\n",
        );
        for (index, file) in files.iter().enumerate() {
            root.push_str(&format!("    S{index}: {{ $ref: '{file}' }}\n"));
        }
        std::fs::write(dir.join("openapi.yaml"), root).unwrap();
        load(&dir.join("openapi.yaml"))
    }

    /// The loaded file whose root-relative path is `relative`.
    fn file_named(bundle: &InputBundle, relative: &str) -> FileId {
        bundle
            .file_ids()
            .find(|id| bundle.root_relative_path(*id) == Some(relative))
            .unwrap_or_else(|| panic!("`{relative}` is not loaded"))
    }

    /// A bare file name matches whole path components, never a string suffix: with
    /// `schemas/legacy-pet.yaml` loaded first, `pet.yaml` names `schemas/pet.yaml`, which a string
    /// suffix match took the first file in load order for.
    #[test]
    fn a_file_name_matches_whole_path_components_not_a_string_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::create_dir(dir.join("schemas")).unwrap();
        std::fs::write(dir.join("schemas/legacy-pet.yaml"), "type: string\n").unwrap();
        std::fs::write(dir.join("schemas/pet.yaml"), "type: integer\n").unwrap();
        let bundle = root_referring_to(&dir, &["schemas/legacy-pet.yaml", "schemas/pet.yaml"]);
        let pet = file_named(&bundle, "schemas/pet.yaml");
        assert_eq!(bundle.file_id_for_path("pet.yaml"), Ok(pet));
        assert_eq!(bundle.file_id_for_path("schemas/pet.yaml"), Ok(pet));
        assert_eq!(
            bundle.file_id_for_path("legacy-pet.yaml"),
            Ok(file_named(&bundle, "schemas/legacy-pet.yaml"))
        );
        assert_eq!(
            bundle.file_id_for_path("t.yaml"),
            Err(FileLookupError::Unloaded)
        );
        assert_eq!(bundle.file_id_for_path(""), Err(FileLookupError::Unloaded));
    }

    /// A name two loaded files end in is ambiguous, and the error names both in load order; a
    /// longer suffix, or the path relative to the root document, names one of them.
    #[test]
    fn a_file_name_two_loaded_files_share_is_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        for sub in ["a", "b"] {
            std::fs::create_dir(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("pet.yaml"), "type: string\n").unwrap();
        }
        let bundle = root_referring_to(&dir, &["b/pet.yaml", "a/pet.yaml"]);
        assert_eq!(
            bundle.file_id_for_path("pet.yaml"),
            Err(FileLookupError::Ambiguous(vec![
                "b/pet.yaml".to_owned(),
                "a/pet.yaml".to_owned()
            ]))
        );
        assert_eq!(
            bundle.file_id_for_path("a/pet.yaml"),
            Ok(file_named(&bundle, "a/pet.yaml"))
        );
        let absolute = dir.join("b/pet.yaml");
        assert_eq!(
            bundle.file_id_for_path(absolute.as_str()),
            Ok(file_named(&bundle, "b/pet.yaml"))
        );
    }

    /// A vendored remote copy is a loaded file like any other: a bare name it shares with a local
    /// file is ambiguous, and each is named by its path relative to the root document.
    #[test]
    fn a_file_name_a_vendored_copy_shares_is_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let url = "https://h.example/v1/pet.yaml";
        let remote = "type: string\n";
        std::fs::create_dir_all(dir.join(VENDOR_DIR).join("h.example/v1")).unwrap();
        std::fs::write(dir.join(VENDOR_DIR).join("h.example/v1/pet.yaml"), remote).unwrap();
        std::fs::write(
            dir.join(LOCK_FILE_NAME),
            format!(
                "version = 1\n\n[[remote]]\nurl = \"{url}\"\nsha256 = \"{}\"\n\
                 path = \"h.example/v1/pet.yaml\"\n",
                sha256_hex(remote.as_bytes()),
            ),
        )
        .unwrap();
        std::fs::create_dir(dir.join("schemas")).unwrap();
        std::fs::write(dir.join("schemas/pet.yaml"), "type: integer\n").unwrap();
        let bundle = root_referring_to(&dir, &["schemas/pet.yaml", url]);
        let vendored = format!("{VENDOR_DIR}/h.example/v1/pet.yaml");
        assert_eq!(
            bundle.file_id_for_path("pet.yaml"),
            Err(FileLookupError::Ambiguous(vec![
                "schemas/pet.yaml".to_owned(),
                vendored.clone()
            ]))
        );
        assert_eq!(
            bundle.file_id_for_path("schemas/pet.yaml"),
            Ok(file_named(&bundle, "schemas/pet.yaml"))
        );
        assert_eq!(
            bundle.file_id_for_path(&vendored),
            Ok(file_named(&bundle, &vendored))
        );
        assert_eq!(
            bundle.file_id_for_path("v1/pet.yaml"),
            Ok(file_named(&bundle, &vendored))
        );
    }
}
