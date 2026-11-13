//! The network-only vendor step behind `spargen lock`.
//!
//! It fetches through the injected [`RemoteFetch`] seam, whose real implementation is
//! [`ReqwestFetcher`], so tests exercise the walk with a stub instead of real HTTP. It walks every
//! remote `$ref` reachable from the spec (recursing through local sub-files and through fetched
//! remote documents), fetches each once, writes the bytes under `.spargen/vendor/`, and records a
//! hash pin in `spargen.lock`.

use std::collections::{HashMap, HashSet, VecDeque};

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};

use super::bundle::{local_identity, parse_by_extension, working_dir};
use super::lock::{vendor_path_for_url, Lock, RemoteEntry, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{
    classify_ref, classify_self, collect_refs, enters_extension, split_fragment, RefTarget,
    SelfIdentity,
};
use super::sha256::sha256_hex;
use super::SpannedValue;

/// The network seam used by [`vendor`]. Real fetching lives in [`ReqwestFetcher`]; tests supply a
/// stub so the vendor logic is exercised without HTTP.
pub(crate) trait RemoteFetch {
    /// Fetch the document at an absolute `http`/`https` `url`, following redirects, or an error
    /// message.
    fn fetch(&self, url: &str) -> Result<Fetched, String>;
}

/// The file-writing seam used by [`vendor`]: write every `(path, bytes)` pair in order, or report
/// the path whose write failed.
pub(crate) type WriteAll =
    dyn Fn(&[(Utf8PathBuf, Vec<u8>)]) -> Result<(), (Utf8PathBuf, std::io::Error)>;

/// A fetched remote document.
pub(crate) struct Fetched {
    /// The raw bytes.
    pub(crate) bytes: Vec<u8>,
    /// The URL the bytes were retrieved from: the requested URL, or where its redirects ended.
    /// It is the document's retrieval URI (RFC 3986 §5.1.3): its base when it carries no `$self`,
    /// and what a `$self` is resolved against when it does.
    pub(crate) url: String,
}

/// One vendored remote document, reported back from [`crate::vendor`].
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct VendoredRef {
    /// The absolute URL that was pinned.
    pub url: String,
    /// The vendor-relative path the bytes were written to.
    pub path: String,
    /// The recorded SHA-256.
    pub sha256: String,
}

/// The result of a successful [`crate::vendor`] run.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct VendorReport {
    /// Every remote document that was fetched and pinned, in URL order.
    pub refs: Vec<VendoredRef>,
    /// Where the lock was written.
    #[serde(serialize_with = "serialize_path")]
    pub lock_path: Utf8PathBuf,
    /// The vendor directory the copies were written under.
    #[serde(serialize_with = "serialize_path")]
    pub vendor_dir: Utf8PathBuf,
}

/// The human rendering `spargen lock` prints on success. It lives with the report rather than in
/// the facade so the success and failure halves of `vendor` both render themselves.
impl std::fmt::Display for VendorReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.refs.is_empty() {
            return write!(formatter, "no remote $refs found; wrote {}", self.lock_path);
        }
        writeln!(
            formatter,
            "vendored {} remote document(s) under {}:",
            self.refs.len(),
            self.vendor_dir
        )?;
        for vendored in &self.refs {
            writeln!(formatter, "  {} -> {}", vendored.url, vendored.path)?;
        }
        write!(formatter, "wrote {}", self.lock_path)
    }
}

/// `camino` does not implement `serde` without its optional feature, and the runtime dependency
/// set is closed, so paths serialize through their `Display` form.
fn serialize_path<S: serde::Serializer>(
    path: &Utf8PathBuf,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(path.as_str())
}

#[derive(Clone)]
enum Base {
    Local(Utf8PathBuf),
    Remote(String),
}

struct ScanDoc {
    value: SpannedValue,
    base: Base,
}

/// The identity `value`'s `$self` gives a document reached at `base`, read by the build's own
/// [`classify_self`], or `None` where it carries none (or an empty or fragment-only one).
fn self_identity(value: &SpannedValue, base: &Base) -> Option<SelfIdentity> {
    let self_uri = value.get("$self")?.as_str()?;
    match base {
        Base::Remote(retrieved) => classify_self(self_uri, Some(retrieved), Utf8Path::new("")),
        Base::Local(path) => classify_self(self_uri, None, path),
    }
}

/// Every document the walk has scanned, and the names a reference reaches each one by.
#[derive(Default)]
struct Scanned {
    /// Every scanned document, kept because a reference into a specification extension is walked
    /// at its target (`enters_extension`), which may lie in a document scanned earlier.
    docs: Vec<ScanDoc>,
    /// The [`local_identity`] of each local path a scanned local document is reached by.
    local_docs: HashMap<Utf8PathBuf, usize>,
    /// The URL (or opaque `$self` URI) each scanned document is reached by.
    remote_docs: HashMap<String, usize>,
    /// The [`local_identity`] of every local path already read, or attempted, or named by a
    /// `$self`.
    seen_local: HashSet<Utf8PathBuf>,
    /// Every URL already fetched, or attempted, or named by a `$self`.
    seen_remote: HashSet<String>,
    /// The working directory local identities are made absolute against, captured once as the
    /// build captures it.
    working_dir: Option<Utf8PathBuf>,
}

impl Scanned {
    /// The identity the build gives the local document at `path` (#451): `lib.yaml`,
    /// `./lib.yaml`, `sub/../lib.yaml` and its absolute path are one document, so a reference
    /// under any of them reaches the document scanned under another instead of reading it again.
    fn identity(&self, path: &Utf8Path) -> Utf8PathBuf {
        local_identity(self.working_dir.as_deref(), path)
    }

    /// Keep `value`, reached at `base`, and return its index. Its `$self`, if any, replaces `base`
    /// as the base its relative references resolve against and is registered as a name it is
    /// reached by, as the build registers it: a reference to that name reaches this document
    /// rather than reading (or fetching and pinning) one of its own. The caller registers the
    /// names the document was reached by first, as the build does before reading its `$self`.
    fn add(&mut self, value: SpannedValue, base: Base) -> usize {
        let index = self.docs.len();
        let base = match self_identity(&value, &base) {
            Some(SelfIdentity::Remote(url)) => {
                self.seen_remote.insert(url.clone());
                self.remote_docs.insert(url.clone(), index);
                Base::Remote(url)
            }
            Some(SelfIdentity::Local(path)) => {
                // The build matches a document's stored path before any `$self` identity.
                let identity = self.identity(&path);
                self.seen_local.insert(identity.clone());
                self.local_docs.entry(identity).or_insert(index);
                Base::Local(path)
            }
            Some(SelfIdentity::Opaque(uri)) => {
                self.remote_docs.insert(uri, index);
                base
            }
            None => base,
        };
        self.docs.push(ScanDoc { value, base });
        index
    }
}

/// Fetch and hash-pin every remote `$ref` reachable from `spec`, writing the vendored copies under
/// `.spargen/vendor/` and (re)writing `spargen.lock` next to the spec. Every fetch goes through the
/// injected `fetcher`; the `vendor` facade in `lib.rs` passes [`ReqwestFetcher`].
///
/// The walk recurses through relative-file refs (to catch remote refs nested in local sub-files)
/// and through fetched remote documents (whose relative refs resolve against each one's `$self`,
/// itself resolved against the URL it was retrieved from after redirects, or against that URL
/// where it has none). Every document's `$self` sets its base and names it, as in the build. It
/// follows exactly the references the build's bundle loader does: none inside a specification
/// extension, unless a followed reference addresses that extension's contents, so `spargen lock`
/// pins every document a build reads and fetches nothing a build ignores. Recursion parsing is
/// best-effort: a fetched document that does not parse is still vendored and pinned; the next
/// `generate` rejects it as `E011`.
///
/// A fetch that fails is `E025`, and the lock is still written: every document fetched is pinned
/// at what it served, and every pin of the earlier lock for a URL this run left unpinned is kept.
///
/// Nothing is written until the walk ends. Then every vendored copy, and `spargen.lock` last, go
/// to `write_all` in one batch; the facade passes `output::atomic_write_all`, which stages every
/// file before it replaces any, so a file that cannot be written leaves the earlier lock and
/// copies in place.
pub(crate) fn vendor(
    spec: &Utf8Path,
    fetcher: &dyn RemoteFetch,
    write_all: &WriteAll,
    diags: &mut Diagnostics,
) -> Result<VendorReport, Aborted> {
    let spec_dir = spec.parent().unwrap_or_else(|| Utf8Path::new(""));
    let vendor_dir = spec_dir.join(VENDOR_DIR);

    let root_text = std::fs::read_to_string(spec).map_err(|error| {
        input_error(diags, format!("failed to read `{spec}`: {error}"));
        Aborted
    })?;
    let Some(root_value) = parse_scratch_local(spec.as_str(), &root_text) else {
        input_error(
            diags,
            format!("failed to parse `{spec}` while scanning for remote $refs"),
        );
        return Err(Aborted);
    };
    // Only the root's spans are kept (`parse_scratch` parses every document as `FileId(0)`), so
    // the root is the one file a vendoring diagnostic's span can name.
    diags.register_file(crate::diag::FileId(0), spec.to_path_buf());

    let mut lock = Lock::default();
    let mut files: Vec<(Utf8PathBuf, Vec<u8>)> = Vec::new();
    let mut fetch_failed = false;
    let mut refs: Vec<VendoredRef> = Vec::new();
    let mut scanned = Scanned {
        working_dir: working_dir(),
        ..Scanned::default()
    };
    let spec_identity = scanned.identity(spec);
    scanned.seen_local.insert(spec_identity.clone());
    scanned.local_docs.insert(spec_identity, 0);
    let root = scanned.add(root_value, Base::Local(spec.to_path_buf()));

    // The queue holds a document index and the pointer to walk from: a document's root, or the
    // target of a reference into a specification extension.
    let mut walked_targets: HashSet<(usize, JsonPointer)> = HashSet::new();
    let mut queue: VecDeque<(usize, JsonPointer)> = VecDeque::from([(root, JsonPointer::root())]);

    while let Some((index, pointer)) = queue.pop_front() {
        let base = scanned.docs[index].base.clone();
        let remote_base = match &base {
            Base::Local(_) => None,
            Base::Remote(url) => Some(url.clone()),
        };
        let Some(value) = scanned.docs[index].value.pointer(&pointer) else {
            continue;
        };
        let collected_refs = collect_refs(value, &pointer);
        for collected in &collected_refs {
            let reference = &collected.reference;
            match classify_ref(reference, remote_base.as_deref()) {
                RefTarget::InDocument => {}
                RefTarget::LocalRelative(path) => {
                    if let Base::Local(base_path) = &base {
                        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
                        // Read at the spelling the build reads, keyed by the identity it compares.
                        let target = parent.join(&path);
                        let identity = scanned.identity(&target);
                        if scanned.seen_local.insert(identity.clone()) {
                            if let Ok(text) = std::fs::read_to_string(&target) {
                                if let Some(value) = parse_scratch_local(target.as_str(), &text) {
                                    scanned.local_docs.insert(identity, scanned.docs.len());
                                    let loaded = scanned.add(value, Base::Local(target));
                                    queue.push_back((loaded, JsonPointer::root()));
                                }
                            }
                        }
                    }
                }
                // A document's opaque `$self` names it, as the build resolves such a reference.
                RefTarget::UnsupportedRemote(url) if scanned.remote_docs.contains_key(&url) => {}
                RefTarget::UnsupportedRemote(url) => {
                    // Located at the reference, in its document's terms. Every scanned document
                    // is parsed as `FileId(0)` (`parse_scratch`), so only the root's spans name
                    // the file they lie in; any other document's reference keeps its pointer.
                    let span = (index == root).then_some(collected.span);
                    Diagnostic::error(
                        Code::AbsoluteRefUnsupported,
                        Provenance::new(collected.pointer.clone(), span),
                    )
                    .message(format!("cannot vendor non-http(s) $ref `{url}`"))
                    .remedy("vendor the referenced document locally and use a relative $ref")
                    .emit(diags);
                }
                RefTarget::Remote(url) => {
                    if !scanned.seen_remote.insert(url.clone()) {
                        continue;
                    }
                    let Fetched {
                        bytes,
                        url: retrieved,
                    } = match fetcher.fetch(&url) {
                        Ok(fetched) => fetched,
                        Err(error) => {
                            // A fetch failure is a fact about the network, not the document:
                            // the ref is well-formed and exactly what this step exists to pin.
                            // Located at the reference, as `E003` above is: only the root's
                            // references carry a span, for the reason given there.
                            let span = (index == root).then_some(collected.span);
                            Diagnostic::error(
                                Code::RemoteFetchFailed,
                                Provenance::new(collected.pointer.clone(), span),
                            )
                            .message(format!("failed to fetch remote $ref `{url}`: {error}"))
                            .remedy(
                                "check the URL and the reported error, then re-run \
                                 `spargen lock`; or vendor the document by hand and use a \
                                 relative $ref",
                            )
                            .emit(diags);
                            fetch_failed = true;
                            continue;
                        }
                    };
                    // A redirect's `Location` may carry a fragment; a base URI carries none
                    // (RFC 3986 §5.1), and the build drops one from a lock it reads.
                    let retrieved = split_fragment(&retrieved).0.to_owned();
                    // The build registers a redirected copy under its retrieval URL as well, so a
                    // later reference to that URL reaches this document and reads no pin of its
                    // own: it is seen here too, or the lock would fetch and pin what the build
                    // never reads. A retrieval URL already seen stays the document it named.
                    scanned.seen_remote.insert(retrieved.clone());
                    let sha256 = sha256_hex(&bytes);
                    let rel_path = vendor_path_for_url(&url);
                    let target = vendor_dir.join(&rel_path);
                    // The pin stays keyed by the URL the spec names, which is what a build looks
                    // up; a redirect's end is recorded beside it, because it is the base the
                    // document's own relative references resolve against.
                    lock.upsert(RemoteEntry {
                        url: url.clone(),
                        sha256: sha256.clone(),
                        path: rel_path.clone(),
                        retrieval_url: (retrieved != url).then(|| retrieved.clone()),
                    });
                    refs.push(VendoredRef {
                        url: url.clone(),
                        path: rel_path,
                        sha256,
                    });
                    let parsed = std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|text| parse_scratch_remote(&url, text));
                    // Written with the lock once the walk ends, never before it.
                    files.push((target, bytes));
                    if let Some(value) = parsed {
                        let next = scanned.docs.len();
                        scanned.remote_docs.insert(url.clone(), next);
                        scanned.remote_docs.entry(retrieved.clone()).or_insert(next);
                        // Its relative references resolve from its `$self`, resolved against
                        // where it was retrieved, or from that retrieval URL where it has none.
                        let loaded = scanned.add(value, Base::Remote(retrieved));
                        queue.push_back((loaded, JsonPointer::root()));
                    }
                }
            }
        }
        // Every document a reference names has been scanned by now, so its target can be found.
        for reference in collected_refs
            .iter()
            .map(|collected| &collected.reference)
            .filter(|reference| enters_extension(reference))
        {
            let target = match classify_ref(reference, remote_base.as_deref()) {
                RefTarget::InDocument => Some(index),
                RefTarget::LocalRelative(path) => match &base {
                    Base::Local(base_path) => {
                        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
                        let identity = scanned.identity(&parent.join(&path));
                        scanned.local_docs.get(&identity).copied()
                    }
                    Base::Remote(_) => None,
                },
                RefTarget::Remote(url) | RefTarget::UnsupportedRemote(url) => {
                    scanned.remote_docs.get(&url).copied()
                }
            };
            let (_, fragment) = split_fragment(reference);
            if let Some(target) = target {
                let target = (target, JsonPointer::from(fragment.to_owned()));
                if walked_targets.insert(target.clone()) {
                    queue.push_back(target);
                }
            }
        }
    }

    let lock_path = spec_dir.join(LOCK_FILE_NAME);
    if fetch_failed {
        // A failed fetch leaves the run's walk incomplete: the document it could not fetch, and
        // every document reachable only through it, went unpinned. Their earlier pins are kept,
        // beside the vendored copies this run did not touch, so a transient network failure leaves
        // the lock the build reads as usable as it was (#714). A document this run did fetch keeps
        // its new pin. A lock that cannot be read keeps nothing.
        let earlier = std::fs::read_to_string(&lock_path)
            .ok()
            .and_then(|text| Lock::parse(&text).ok())
            .unwrap_or_default();
        lock.keep_unpinned_from(earlier);
    }
    // The lock goes last: it pins the copies, so every copy is in place before it names them.
    files.push((lock_path.clone(), lock.to_toml().into_bytes()));
    write_all(&files).map_err(|(path, error)| {
        input_error(diags, format!("failed to write `{path}`: {error}"));
        Aborted
    })?;

    refs.sort_by(|a, b| a.url.cmp(&b.url));
    diags.result(VendorReport {
        refs,
        lock_path,
        vendor_dir,
    })
}

/// Parse a local document's text into a value tree for ref-scanning only, discarding parse
/// diagnostics. Chooses the format the way the build does ([`parse_by_extension`]), from the
/// extension `Utf8Path::extension` reads from `path`, the reading the build's `parse_by_name`
/// makes: a name that is only a dot and an extension, such as `.json`, has none (#917). A `?` or
/// `#` in a local path is part of the file's name (a relative `$ref` such as `api.yaml?v=1` keeps
/// its query in the path it names), so it is read through rather than cut off: `api.json?v=1`
/// names no recognised extension (#916).
pub(super) fn parse_scratch_local(path: &str, text: &str) -> Option<SpannedValue> {
    parse_scratch(Utf8Path::new(path).extension(), text)
}

/// Parse a fetched document's text into a value tree for ref-scanning only, discarding parse
/// diagnostics. Chooses the format the way the build does ([`parse_by_extension`]), from the
/// extension `Utf8Path::extension` reads from [`vendor_path_for_url`]'s path for `url`, which is
/// the name the build parses the vendored copy by. That path drops a `?` query, a `#` fragment and
/// any empty segment, so `api.json/` is read as JSON, as the build reads it (#918); a name that is
/// only a dot and an extension, such as `.json`, has none, as it has none to the build (#917).
pub(super) fn parse_scratch_remote(url: &str, text: &str) -> Option<SpannedValue> {
    parse_scratch(Utf8Path::new(&vendor_path_for_url(url)).extension(), text)
}

fn parse_scratch(extension: Option<&str>, text: &str) -> Option<SpannedValue> {
    parse_by_extension(
        crate::diag::FileId(0),
        extension,
        text,
        &mut Diagnostics::default(),
    )
    .ok()
}

fn input_error(diags: &mut Diagnostics, message: String) {
    Diagnostic::error(
        Code::InvalidInput,
        Provenance::new(JsonPointer::root(), None),
    )
    .message(message)
    .emit(diags);
}

/// The real, reqwest-backed fetcher used by `spargen lock`.
pub(crate) struct ReqwestFetcher;

impl RemoteFetch for ReqwestFetcher {
    fn fetch(&self, url: &str) -> Result<Fetched, String> {
        // What `reqwest::blocking::get` does: a default client per fetch.
        fetch_with(reqwest::blocking::Client::builder(), url)
    }
}

/// Fetch `url` with a client built from `builder`: the whole of [`ReqwestFetcher`]'s fetch but the
/// builder. The builder is the trust seam — the TLS test in this module adds its self-signed root
/// to it and otherwise runs exactly the path `spargen lock` runs, so the handshake, the root
/// store, and ALPN under the linked reqwest/rustls stack are reached by a test.
///
/// It follows at most [`MAX_REDIRECTS`] redirects and refuses one that downgrades `https` to
/// `http` ([`refuses_downgrade`]), and it reads at most [`MAX_REMOTE_BODY`] bytes.
fn fetch_with(builder: reqwest::blocking::ClientBuilder, url: &str) -> Result<Fetched, String> {
    let mut response = builder
        .redirect(redirect_policy())
        .build()
        .and_then(|client| client.get(url).send())
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| with_causes(&error))?;
    // Where the client's redirects ended; read before `bytes` consumes the response. The client
    // normalizes what it requests (`HTTPS://Host` is `https://host/`), so an unredirected fetch
    // is reported under the spelling it was asked for, not the normalized one.
    let retrieved = if reqwest::Url::parse(url).ok().as_ref() == Some(response.url()) {
        url.to_owned()
    } else {
        response.url().to_string()
    };
    let bytes = read_bounded(&mut response, MAX_REMOTE_BODY)?;
    Ok(Fetched {
        bytes,
        url: retrieved,
    })
}

/// The most bytes [`fetch_with`] reads of one remote document: 64 MiB, about five times the 13 MB
/// GitHub description in the corpus, so a hostile or misconfigured server cannot stream unbounded
/// data into memory.
const MAX_REMOTE_BODY: u64 = 64 * 1024 * 1024;

/// The most redirects [`fetch_with`] follows for one fetch, reqwest's own default.
const MAX_REDIRECTS: usize = 10;

/// The body of `response`, refused once it declares or delivers more than `limit` bytes.
fn read_bounded(response: &mut reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let over = || format!("the response body is larger than the {limit}-byte limit");
    if response
        .content_length()
        .is_some_and(|declared| declared > limit)
    {
        return Err(over());
    }
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| with_causes(&error))?;
    if bytes.len() as u64 > limit {
        return Err(over());
    }
    Ok(bytes)
}

/// Follow up to [`MAX_REDIRECTS`] redirects, refusing any [`refuses_downgrade`] names.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            let error = RedirectRefused(format!("more than {MAX_REDIRECTS} redirects"));
            attempt.error(error)
        } else if refuses_downgrade(attempt.previous(), attempt.url()) {
            let error = RedirectRefused(format!(
                "refused a redirect from https to `{}`: a document fetched over https is never \
                 fetched over plaintext, where it could be substituted before it is pinned",
                attempt.url()
            ));
            attempt.error(error)
        } else {
            attempt.follow()
        }
    })
}

/// Whether a redirect to `next` downgrades the fetch: once any URL of the chain so far
/// (`previous`, the requested URL first) was `https`, every later hop must be too.
fn refuses_downgrade(previous: &[reqwest::Url], next: &reqwest::Url) -> bool {
    next.scheme() != "https" && previous.iter().any(|url| url.scheme() == "https")
}

/// Why [`redirect_policy`] stopped following a redirect.
#[derive(Debug)]
struct RedirectRefused(String);

impl std::fmt::Display for RedirectRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RedirectRefused {}

/// `error` followed by each error in its `source()` chain, `: `-separated.
///
/// A reqwest transport error displays only "error sending request for url (…)"; what actually
/// failed — the refused connection, the DNS lookup, the TLS alert and its reason — is carried by
/// its sources, and E025 promises to report it.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        // Some layers repeat their source's text in their own; print each distinct text once.
        if !rendered.ends_with(&cause_text) {
            rendered.push_str(": ");
            rendered.push_str(&cause_text);
        }
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests;
