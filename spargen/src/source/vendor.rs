//! The network-only vendor step behind `spargen lock`.
//!
//! This is the ONLY place spargen performs network I/O, and only through the injected
//! [`RemoteFetch`] seam — so `generate`/`check` stay hermetic and tests exercise the walk with a
//! stub instead of real HTTP. It walks every remote `$ref` reachable from the spec (recursing
//! through local sub-files and through fetched remote documents), fetches each once, writes the
//! bytes under `.spargen/vendor/`, and records a hash pin in `spargen.lock`.

use std::collections::{HashMap, HashSet, VecDeque};

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};

use super::lock::{vendor_path_for_url, Lock, RemoteEntry, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{classify_ref, collect_refs, enters_extension, split_fragment, RefTarget};
use super::sha256::sha256_hex;
use super::{parse_json, parse_yaml, SpannedValue};

/// The network seam used by [`vendor`]. Real fetching lives in [`ReqwestFetcher`]; tests supply a
/// stub so the vendor logic is exercised without HTTP.
pub(crate) trait RemoteFetch {
    /// Fetch the document at an absolute `http`/`https` `url`, following redirects, or an error
    /// message.
    fn fetch(&self, url: &str) -> Result<Fetched, String>;
}

/// A fetched remote document.
pub(crate) struct Fetched {
    /// The raw bytes.
    pub(crate) bytes: Vec<u8>,
    /// The URL the bytes were retrieved from: the requested URL, or where its redirects ended.
    /// It is the document's base URI (RFC 3986 §5.1.3), so its relative `$ref`s resolve against
    /// it.
    pub(crate) url: String,
}

/// One vendored remote document, reported back from [`crate::vendor`].
#[derive(Debug, Clone, serde::Serialize)]
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

/// Fetch and hash-pin every remote `$ref` reachable from `spec`, writing the vendored copies under
/// `.spargen/vendor/` and (re)writing `spargen.lock` next to the spec. This is the ONLY function
/// that performs network I/O, and only through the injected `fetcher`.
///
/// The walk recurses through relative-file refs (to catch remote refs nested in local sub-files)
/// and through fetched remote documents (whose relative refs resolve against the URL each was
/// retrieved from, after redirects). It
/// follows exactly the references the build's bundle loader does: none inside a specification
/// extension, unless a followed reference addresses that extension's contents, so `spargen lock`
/// pins every document a build reads and fetches nothing a build ignores. Recursion parsing is best-effort: a fetched doc that does not parse is still vendored, and any
/// remote ref it hides surfaces later as an actionable `E003` on the next `generate`.
pub(crate) fn vendor(
    spec: &Utf8Path,
    fetcher: &dyn RemoteFetch,
    diags: &mut Diagnostics,
) -> Result<VendorReport, Aborted> {
    let spec_dir = spec.parent().unwrap_or_else(|| Utf8Path::new(""));
    let vendor_dir = spec_dir.join(VENDOR_DIR);

    let root_text = std::fs::read_to_string(spec).map_err(|error| {
        input_error(diags, format!("failed to read `{spec}`: {error}"));
        Aborted
    })?;
    let Some(root_value) = parse_scratch(spec.as_str(), &root_text) else {
        input_error(
            diags,
            format!("failed to parse `{spec}` while scanning for remote $refs"),
        );
        return Err(Aborted);
    };

    let mut lock = Lock::default();
    let mut refs: Vec<VendoredRef> = Vec::new();
    let mut seen_remote: HashSet<String> = HashSet::new();
    let mut seen_local: HashSet<Utf8PathBuf> = HashSet::new();
    seen_local.insert(spec.to_path_buf());

    // Every scanned document is kept, because a reference into a specification extension is walked
    // at its target (`enters_extension`), which may lie in a document scanned earlier. The queue
    // holds a document index and the pointer to walk from: the root, or such a target.
    let mut docs: Vec<ScanDoc> = vec![ScanDoc {
        value: root_value,
        base: Base::Local(spec.to_path_buf()),
    }];
    let mut local_docs: HashMap<Utf8PathBuf, usize> = HashMap::from([(spec.to_path_buf(), 0)]);
    let mut remote_docs: HashMap<String, usize> = HashMap::new();
    let mut walked_targets: HashSet<(usize, JsonPointer)> = HashSet::new();
    let mut queue: VecDeque<(usize, JsonPointer)> = VecDeque::from([(0, JsonPointer::root())]);

    while let Some((index, pointer)) = queue.pop_front() {
        let base = docs[index].base.clone();
        let remote_base = match &base {
            Base::Local(_) => None,
            Base::Remote(url) => Some(url.clone()),
        };
        let Some(value) = docs[index].value.pointer(&pointer) else {
            continue;
        };
        let doc_refs = collect_refs(value);
        for reference in &doc_refs {
            match classify_ref(reference, remote_base.as_deref()) {
                RefTarget::InDocument => {}
                RefTarget::LocalRelative(path) => {
                    if let Base::Local(base_path) = &base {
                        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
                        let target = parent.join(&path);
                        if seen_local.insert(target.clone()) {
                            if let Ok(text) = std::fs::read_to_string(&target) {
                                if let Some(value) = parse_scratch(target.as_str(), &text) {
                                    local_docs.insert(target.clone(), docs.len());
                                    queue.push_back((docs.len(), JsonPointer::root()));
                                    docs.push(ScanDoc {
                                        value,
                                        base: Base::Local(target),
                                    });
                                }
                            }
                        }
                    }
                }
                RefTarget::UnsupportedRemote(url) => {
                    Diagnostic::error(
                        Code::AbsoluteRefUnsupported,
                        Provenance::new(JsonPointer::root(), None),
                    )
                    .message(format!("cannot vendor non-http(s) $ref `{url}`"))
                    .remedy("vendor the referenced document locally and use a relative $ref")
                    .emit(diags);
                }
                RefTarget::Remote(url) => {
                    if !seen_remote.insert(url.clone()) {
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
                            Diagnostic::error(
                                Code::RemoteFetchFailed,
                                Provenance::new(JsonPointer::root(), None),
                            )
                            .message(format!("failed to fetch remote $ref `{url}`: {error}"))
                            .remedy(
                                "check the URL and the reported error, then re-run \
                                 `spargen lock`; or vendor the document by hand and use a \
                                 relative $ref",
                            )
                            .emit(diags);
                            continue;
                        }
                    };
                    let sha256 = sha256_hex(&bytes);
                    let rel_path = vendor_path_for_url(&url);
                    let target = vendor_dir.join(&rel_path);
                    if let Some(parent) = target.parent() {
                        if let Err(error) = std::fs::create_dir_all(parent) {
                            input_error(
                                diags,
                                format!("failed to create vendor directory `{parent}`: {error}"),
                            );
                            continue;
                        }
                    }
                    if let Err(error) = std::fs::write(&target, &bytes) {
                        input_error(
                            diags,
                            format!("failed to write vendored file `{target}`: {error}"),
                        );
                        continue;
                    }
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
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some(value) = parse_scratch(&url, &text) {
                            remote_docs.insert(url.clone(), docs.len());
                            queue.push_back((docs.len(), JsonPointer::root()));
                            docs.push(ScanDoc {
                                value,
                                base: Base::Remote(retrieved),
                            });
                        }
                    }
                }
            }
        }
        // Every document a reference names has been scanned by now, so its target can be found.
        for reference in doc_refs
            .iter()
            .filter(|reference| enters_extension(reference))
        {
            let target = match classify_ref(reference, remote_base.as_deref()) {
                RefTarget::InDocument => Some(index),
                RefTarget::LocalRelative(path) => match &base {
                    Base::Local(base_path) => {
                        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
                        local_docs.get(&parent.join(&path)).copied()
                    }
                    Base::Remote(_) => None,
                },
                RefTarget::Remote(url) => remote_docs.get(&url).copied(),
                RefTarget::UnsupportedRemote(_) => None,
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
    std::fs::write(&lock_path, lock.to_toml()).map_err(|error| {
        input_error(diags, format!("failed to write `{lock_path}`: {error}"));
        Aborted
    })?;

    refs.sort_by(|a, b| a.url.cmp(&b.url));
    diags.result(VendorReport {
        refs,
        lock_path,
        vendor_dir,
    })
}

/// Parse text into a value tree for ref-scanning only, discarding parse diagnostics. Chooses the
/// format by the `.json`/`.yaml`/`.yml` suffix of `name` (a path or URL), else tries YAML then JSON.
fn parse_scratch(name: &str, text: &str) -> Option<SpannedValue> {
    let id = crate::diag::FileId(0);
    let lowered = name
        .split(['?', '#'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if lowered.ends_with(".json") {
        parse_json(id, text, &mut Diagnostics::default()).ok()
    } else if lowered.ends_with(".yaml") || lowered.ends_with(".yml") {
        parse_yaml(id, text, &mut Diagnostics::default()).ok()
    } else {
        parse_yaml(id, text, &mut Diagnostics::default())
            .ok()
            .or_else(|| parse_json(id, text, &mut Diagnostics::default()).ok())
    }
}

fn input_error(diags: &mut Diagnostics, message: String) {
    Diagnostic::error(
        Code::InvalidInput,
        Provenance::new(JsonPointer::root(), None),
    )
    .message(message)
    .emit(diags);
}

/// The real, reqwest-backed fetcher used by `spargen lock`. Present only under the `remote-fetch`
/// feature so a library-only build carries no HTTP stack.
#[cfg(feature = "remote-fetch")]
pub(crate) struct ReqwestFetcher;

#[cfg(feature = "remote-fetch")]
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
#[cfg(feature = "remote-fetch")]
fn fetch_with(builder: reqwest::blocking::ClientBuilder, url: &str) -> Result<Fetched, String> {
    let response = builder
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
    let bytes = response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|error| with_causes(&error))?;
    Ok(Fetched {
        bytes,
        url: retrieved,
    })
}

/// `error` followed by each error in its `source()` chain, `: `-separated.
///
/// A reqwest transport error displays only "error sending request for url (…)"; what actually
/// failed — the refused connection, the DNS lookup, the TLS alert and its reason — is carried by
/// its sources, and E025 promises to report it.
#[cfg(feature = "remote-fetch")]
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
mod tests {
    use super::*;

    /// A stubbed fetcher backed by an in-memory URL → bytes map; no network.
    struct StubFetcher {
        docs: std::collections::HashMap<String, Vec<u8>>,
    }

    impl RemoteFetch for StubFetcher {
        fn fetch(&self, url: &str) -> Result<Fetched, String> {
            self.docs
                .get(url)
                .cloned()
                .map(|bytes| Fetched {
                    bytes,
                    url: url.to_owned(),
                })
                .ok_or_else(|| format!("404 {url}"))
        }
    }

    /// A failed fetch is `E025`, never `E003` "not pinned": the document is fine, the network
    /// is not. The message carries the URL and the fetcher's own error, nothing is vendored for
    /// that URL, and the unrelated ref beside it is still fetched and pinned.
    #[test]
    fn a_fetch_failure_is_e025_naming_the_url_and_the_error() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let spec = dir.join("openapi.yaml");
        std::fs::write(
            &spec,
            "openapi: 3.1.0\n\
             components:\n\
             \x20 schemas:\n\
             \x20   Pet:\n\
             \x20     $ref: \"https://api.example.com/missing.yaml\"\n\
             \x20   Tag:\n\
             \x20     $ref: \"https://api.example.com/tag.yaml\"\n",
        )
        .unwrap();
        let mut docs = std::collections::HashMap::new();
        docs.insert(
            "https://api.example.com/tag.yaml".to_owned(),
            b"type: string\n".to_vec(),
        );
        let fetcher = StubFetcher { docs };

        let mut diags = Diagnostics::default();
        assert!(vendor(&spec, &fetcher, &mut diags).is_err());

        let codes: Vec<Code> = diags.items().iter().map(|diag| diag.code).collect();
        assert_eq!(codes, [Code::RemoteFetchFailed], "{:?}", diags.items());
        let message = &diags.items()[0].message;
        assert!(
            message.contains("https://api.example.com/missing.yaml"),
            "{message}"
        );
        assert!(
            message.contains("404 https://api.example.com/missing.yaml"),
            "the fetcher's own error must be carried: {message}"
        );

        let vendor_dir = dir.join(VENDOR_DIR);
        assert!(!vendor_dir
            .join(vendor_path_for_url("https://api.example.com/missing.yaml"))
            .exists());
        assert!(vendor_dir
            .join(vendor_path_for_url("https://api.example.com/tag.yaml"))
            .exists());
    }

    /// `spargen lock` follows the references a build follows (#239): none inside a specification
    /// extension, so a remote document one names is never fetched (the stub would fail it with
    /// `E025`), but every one inside an extension a followed reference addresses — in the root
    /// document or in a fetched one — and every one under an author-chosen `x-` name.
    #[test]
    fn vendors_what_a_build_reads_and_nothing_inside_an_unaddressed_extension() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let spec = dir.join("openapi.yaml");
        std::fs::write(
            &spec,
            "openapi: 3.1.0\n\
             x-note: { $ref: \"https://api.example.com/unused-root.yaml\" }\n\
             x-defs:\n\
             \x20 Pet: { $ref: \"https://api.example.com/pet.yaml\" }\n\
             components:\n\
             \x20 x-note: { $ref: \"https://api.example.com/unused-components.yaml\" }\n\
             \x20 schemas:\n\
             \x20   Pet: { $ref: \"#/x-defs/Pet\" }\n\
             \x20   Tag: { $ref: \"https://api.example.com/lib.yaml#/x-defs/Tag\" }\n",
        )
        .unwrap();
        let mut docs = std::collections::HashMap::new();
        docs.insert(
            "https://api.example.com/pet.yaml".to_owned(),
            b"type: object\n\
              x-meta: { $ref: \"./unused-schema.yaml\" }\n\
              properties:\n  x-owner: { $ref: \"./owner.yaml\" }\n"
                .to_vec(),
        );
        docs.insert(
            "https://api.example.com/owner.yaml".to_owned(),
            b"type: string\n".to_vec(),
        );
        docs.insert(
            "https://api.example.com/lib.yaml".to_owned(),
            b"x-defs:\n  Tag: { $ref: \"./tag.yaml\" }\n".to_vec(),
        );
        docs.insert(
            "https://api.example.com/tag.yaml".to_owned(),
            b"type: string\n".to_vec(),
        );
        let fetcher = StubFetcher { docs };

        let mut diags = Diagnostics::default();
        let report = vendor(&spec, &fetcher, &mut diags).expect("vendor succeeds");
        assert!(diags.items().is_empty(), "{:?}", diags.items());
        let urls: Vec<&str> = report.refs.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://api.example.com/lib.yaml",
                "https://api.example.com/owner.yaml",
                "https://api.example.com/pet.yaml",
                "https://api.example.com/tag.yaml",
            ]
        );
    }

    #[test]
    fn vendors_and_pins_recursively_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let spec = dir.join("openapi.yaml");
        std::fs::write(
            &spec,
            "openapi: 3.1.0\n\
             components:\n\
             \x20 schemas:\n\
             \x20   Pet:\n\
             \x20     $ref: \"https://api.example.com/schemas/pet.yaml\"\n",
        )
        .unwrap();

        // The remote pet doc itself references a sibling remote doc via a relative ref.
        let mut docs = std::collections::HashMap::new();
        docs.insert(
            "https://api.example.com/schemas/pet.yaml".to_owned(),
            b"type: object\nproperties:\n  tag:\n    $ref: \"./tag.yaml\"\n".to_vec(),
        );
        docs.insert(
            "https://api.example.com/schemas/tag.yaml".to_owned(),
            b"type: string\n".to_vec(),
        );
        let fetcher = StubFetcher { docs };

        let mut diags = Diagnostics::default();
        let report = vendor(&spec, &fetcher, &mut diags).expect("vendor succeeds");
        assert!(!diags.has_errors(), "no diagnostics: {:?}", diags.items());

        // Both the directly-referenced doc and the transitively-referenced one were fetched.
        let urls: Vec<&str> = report.refs.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://api.example.com/schemas/pet.yaml",
                "https://api.example.com/schemas/tag.yaml",
            ]
        );

        // The lock is written and re-parses to the same pins.
        let lock_text = std::fs::read_to_string(&report.lock_path).unwrap();
        let lock = Lock::parse(&lock_text).unwrap();
        assert!(lock
            .get("https://api.example.com/schemas/tag.yaml")
            .is_some());

        // Vendored bytes exist on disk and match the pinned sha256.
        for vendored in &report.refs {
            let on_disk = std::fs::read(report.vendor_dir.join(&vendored.path)).unwrap();
            assert_eq!(sha256_hex(&on_disk), vendored.sha256);
        }
    }

    /// The linked reqwest/rustls stack itself (#292): `spargen lock`'s fetch path completing a TLS
    /// handshake against a local HTTPS server. `tests/vendor_remote.rs` drives the binary over
    /// plain HTTP only, because the fetcher trusts only the bundled `webpki-roots` and a local
    /// server's certificate is self-signed; here the certificate's root is added to the client
    /// builder through `fetch_with`, and nothing else differs from `ReqwestFetcher`.
    #[cfg(feature = "remote-fetch")]
    mod tls {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::thread::JoinHandle;
        use std::time::{Duration, Instant};

        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        use super::*;

        const DOCUMENT: &[u8] = b"type: string\n";
        /// Bounds every wait on the peer, so a client that never connects fails the test
        /// instead of hanging it.
        const TIMEOUT: Duration = Duration::from_secs(30);

        /// `ReqwestFetcher` with one extra trusted root.
        struct TrustingFetcher(reqwest::Certificate);

        impl RemoteFetch for TrustingFetcher {
            fn fetch(&self, url: &str) -> Result<Fetched, String> {
                fetch_with(
                    reqwest::blocking::Client::builder().add_root_certificate(self.0.clone()),
                    url,
                )
            }
        }

        /// What the server saw of the one connection it accepted.
        struct Served {
            request_line: String,
            alpn: Option<Vec<u8>>,
        }

        /// A self-signed P-256 end-entity certificate for `IP:127.0.0.1` (`CA:FALSE`, `serverAuth`),
        /// valid until 2126-09-05, and its key: a test-only fixture, trusted by nothing but the
        /// test that adds it. Embedded rather than minted so the test needs no dependency beyond
        /// the rustls `remote-fetch` already links (a certificate-minting crate would bring
        /// `rustls-pki-types` into the default-features graph). Made with
        /// `openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 36500
        /// -subj /CN=spargen-test -addext subjectAltName=IP:127.0.0.1
        /// -addext basicConstraints=critical,CA:FALSE -addext keyUsage=critical,digitalSignature
        /// -addext extendedKeyUsage=serverAuth`.
        const CERTIFICATE_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBuTCCAWCgAwIBAgIUMQ5PDmfQIGeB2CWkWzraOMdmoS8wCgYIKoZIzj0EAwIw
FzEVMBMGA1UEAwwMc3Bhcmdlbi10ZXN0MCAXDTI2MDkyOTIyMzczN1oYDzIxMjYw
OTA1MjIzNzM3WjAXMRUwEwYDVQQDDAxzcGFyZ2VuLXRlc3QwWTATBgcqhkjOPQIB
BggqhkjOPQMBBwNCAATeM/pv9KGwcjCeD708X0y6GI4V08wLz/A/Lwh8zf2sORpx
5dzRI7oSp+ICMvIiTbGx8VlQAm0vsSPk0M/34yZjo4GHMIGEMB0GA1UdDgQWBBT/
IuqSf4eS+2sbcUsuvja1+KxwnTAfBgNVHSMEGDAWgBT/IuqSf4eS+2sbcUsuvja1
+KxwnTAPBgNVHREECDAGhwR/AAABMAwGA1UdEwEB/wQCMAAwDgYDVR0PAQH/BAQD
AgeAMBMGA1UdJQQMMAoGCCsGAQUFBwMBMAoGCCqGSM49BAMCA0cAMEQCIHdk3YK1
ld3StLgbIWF95gZ1PexX3NwsadyTcn8xGrScAiBLo8+2dtpd3P+v7PB9CKu7CbWm
u11UFaEGgB1ZbDcTng==
-----END CERTIFICATE-----
";
        const PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgW2trw08Yt5NxD9IA
dwZUQ3cTNCm3zN7X+n8ukYTTly6hRANCAATeM/pv9KGwcjCeD708X0y6GI4V08wL
z/A/Lwh8zf2sORpx5dzRI7oSp+ICMvIiTbGx8VlQAm0vsSPk0M/34yZj
-----END PRIVATE KEY-----
";

        /// The fixture certificate as DER, and its PKCS#8 key.
        fn self_signed() -> (CertificateDer<'static>, PrivatePkcs8KeyDer<'static>) {
            (
                CertificateDer::from_pem_slice(CERTIFICATE_PEM.as_bytes())
                    .expect("the fixture certificate parses"),
                PrivatePkcs8KeyDer::from_pem_slice(PRIVATE_KEY_PEM.as_bytes())
                    .expect("the fixture key parses"),
            )
        }

        /// Accept one connection on `listener`, complete a TLS handshake offering `h2` and
        /// `http/1.1`, and answer its request with `DOCUMENT`. The error is the handshake's.
        fn serve_one(
            listener: TcpListener,
            cert: CertificateDer<'static>,
            key: PrivatePkcs8KeyDer<'static>,
        ) -> JoinHandle<Result<Served, String>> {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("ring supports the default protocol versions")
                .with_no_client_auth()
                .with_single_cert(vec![cert], PrivateKeyDer::Pkcs8(key))
                .expect("the minted key matches its certificate");
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            let config = Arc::new(config);

            std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = Instant::now() + TIMEOUT;
                let tcp = loop {
                    match listener.accept() {
                        Ok((tcp, _)) => break tcp,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() > deadline {
                                return Err("no client connected".to_owned());
                            }
                            std::thread::yield_now();
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                };
                tcp.set_nonblocking(false).unwrap();
                tcp.set_read_timeout(Some(TIMEOUT)).unwrap();
                tcp.set_write_timeout(Some(TIMEOUT)).unwrap();
                let connection = rustls::ServerConnection::new(config).unwrap();
                let mut tls = rustls::StreamOwned::new(connection, tcp);

                // Reading drives the handshake; its failure surfaces here.
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = tls.read(&mut chunk).map_err(|error| error.to_string())?;
                    if read == 0 {
                        return Err("the client closed before sending a request".to_owned());
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                let alpn = tls.conn.alpn_protocol().map(<[u8]>::to_vec);
                let request = String::from_utf8_lossy(&request);
                let request_line = request.lines().next().unwrap_or_default().to_owned();

                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    DOCUMENT.len()
                );
                tls.write_all(head.as_bytes())
                    .and_then(|()| tls.write_all(DOCUMENT))
                    .and_then(|()| tls.flush())
                    .map_err(|error| error.to_string())?;
                tls.conn.send_close_notify();
                // Best effort: the client may already have closed once it read the body.
                let _ = tls.flush();
                Ok(Served { request_line, alpn })
            })
        }

        /// A spec whose one remote `$ref` is `url`.
        fn spec_referencing(url: &str) -> (tempfile::TempDir, Utf8PathBuf) {
            let temp = tempfile::tempdir().unwrap();
            let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
            let spec = dir.join("openapi.yaml");
            std::fs::write(
                &spec,
                format!(
                    "openapi: 3.1.0\n\
                     components:\n\
                     \x20 schemas:\n\
                     \x20   Pet:\n\
                     \x20     $ref: \"{url}\"\n"
                ),
            )
            .unwrap();
            (temp, spec)
        }

        /// With the server's root trusted, the handshake completes, ALPN settles on HTTP/1.1
        /// (the only protocol reqwest is built to speak here), and the document is vendored and
        /// pinned byte for byte.
        #[test]
        fn vendors_a_document_over_a_completed_tls_handshake() {
            let (cert, key) = self_signed();
            let root = reqwest::Certificate::from_der(&cert).expect("a DER certificate");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("https://{}/pet.yaml", listener.local_addr().unwrap());
            let server = serve_one(listener, cert, key);
            let (_temp, spec) = spec_referencing(&url);

            let mut diags = Diagnostics::default();
            let report = vendor(&spec, &TrustingFetcher(root), &mut diags);
            let served = server.join().expect("the server thread does not panic");
            let report = report.unwrap_or_else(|_| panic!("vendor failed: {:?}", diags.items()));
            assert!(!diags.has_errors(), "{:?}", diags.items());
            let served = served.expect("the server completes the handshake");

            assert_eq!(served.request_line, "GET /pet.yaml HTTP/1.1");
            assert_eq!(served.alpn.as_deref(), Some(&b"http/1.1"[..]));
            let urls: Vec<&str> = report.refs.iter().map(|r| r.url.as_str()).collect();
            assert_eq!(urls, [url.as_str()]);
            let vendored = &report.refs[0];
            let on_disk = std::fs::read(report.vendor_dir.join(&vendored.path)).unwrap();
            assert_eq!(on_disk, DOCUMENT);
            assert_eq!(vendored.sha256, sha256_hex(DOCUMENT));
        }

        /// The production fetcher does not trust that root: the same server fails certificate
        /// verification, reported as `E025` carrying the TLS cause, and nothing is vendored. So
        /// the root the passing test adds is what makes it pass, and the fetcher's own root store
        /// holds only what it bundles.
        #[test]
        fn the_production_fetcher_rejects_a_self_signed_server_with_e025() {
            let (cert, key) = self_signed();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("https://{}/pet.yaml", listener.local_addr().unwrap());
            let server = serve_one(listener, cert, key);
            let (_temp, spec) = spec_referencing(&url);

            let mut diags = Diagnostics::default();
            let outcome = vendor(&spec, &ReqwestFetcher, &mut diags);
            let served = server.join().expect("the server thread does not panic");
            assert!(outcome.is_err());
            assert!(served.is_err(), "the handshake must fail on the server too");

            let codes: Vec<Code> = diags.items().iter().map(|diag| diag.code).collect();
            assert_eq!(codes, [Code::RemoteFetchFailed], "{:?}", diags.items());
            let message = &diags.items()[0].message;
            assert!(message.contains(&url), "{message}");
            assert!(
                message.contains("UnknownIssuer"),
                "the certificate-verification cause must be carried: {message}"
            );
        }
    }
}
