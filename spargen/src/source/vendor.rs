//! The network-only vendor step behind `spargen lock`.
//!
//! This is the ONLY place spargen performs network I/O, and only through the injected
//! [`RemoteFetch`] seam — so `generate`/`check` stay hermetic and tests exercise the walk with a
//! stub instead of real HTTP. It walks every remote `$ref` reachable from the spec (recursing
//! through local sub-files and through fetched remote documents), fetches each once, writes the
//! bytes under `.spargen/vendor/`, and records a hash pin in `spargen.lock`.

use std::collections::{HashSet, VecDeque};

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};

use super::lock::{vendor_path_for_url, Lock, RemoteEntry, LOCK_FILE_NAME, VENDOR_DIR};
use super::remote::{classify_ref, collect_refs, RefTarget};
use super::sha256::sha256_hex;
use super::{parse_json, parse_yaml, SpannedValue};

/// The network seam used by [`vendor`]. Real fetching lives in [`ReqwestFetcher`]; tests supply a
/// stub so the vendor logic is exercised without HTTP.
pub trait RemoteFetch {
    /// Fetch the raw bytes at an absolute `http`/`https` `url`, or an error message.
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// One vendored remote document, reported back from [`vendor`].
#[derive(Debug, Clone)]
pub struct VendoredRef {
    /// The absolute URL that was pinned.
    pub url: String,
    /// The vendor-relative path the bytes were written to.
    pub path: String,
    /// The recorded SHA-256.
    pub sha256: String,
}

/// The result of a successful [`vendor`] run.
#[derive(Debug, Clone)]
pub struct VendorReport {
    /// Every remote document that was fetched and pinned, in URL order.
    pub refs: Vec<VendoredRef>,
    /// Where the lock was written.
    pub lock_path: Utf8PathBuf,
    /// The vendor directory the copies were written under.
    pub vendor_dir: Utf8PathBuf,
}

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
/// and through fetched remote documents (whose relative refs resolve against their own URL).
/// Recursion parsing is best-effort: a fetched doc that does not parse is still vendored, and any
/// remote ref it hides surfaces later as an actionable `E003` on the next `generate`.
pub fn vendor(
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

    let mut queue: VecDeque<ScanDoc> = VecDeque::new();
    queue.push_back(ScanDoc {
        value: root_value,
        base: Base::Local(spec.to_path_buf()),
    });

    while let Some(doc) = queue.pop_front() {
        let remote_base = match &doc.base {
            Base::Local(_) => None,
            Base::Remote(url) => Some(url.clone()),
        };
        for reference in collect_refs(&doc.value) {
            match classify_ref(&reference, remote_base.as_deref()) {
                RefTarget::InDocument => {}
                RefTarget::LocalRelative(path) => {
                    if let Base::Local(base_path) = &doc.base {
                        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
                        let target = parent.join(&path);
                        if seen_local.insert(target.clone()) {
                            if let Ok(text) = std::fs::read_to_string(&target) {
                                if let Some(value) = parse_scratch(target.as_str(), &text) {
                                    queue.push_back(ScanDoc {
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
                    let bytes = match fetcher.fetch(&url) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            Diagnostic::error(
                                Code::AbsoluteRefUnsupported,
                                Provenance::new(JsonPointer::root(), None),
                            )
                            .message(format!("failed to fetch remote $ref `{url}`: {error}"))
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
                    lock.upsert(RemoteEntry {
                        url: url.clone(),
                        sha256: sha256.clone(),
                        path: rel_path.clone(),
                    });
                    refs.push(VendoredRef {
                        url: url.clone(),
                        path: rel_path,
                        sha256,
                    });
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some(value) = parse_scratch(&url, &text) {
                            queue.push_back(ScanDoc {
                                value,
                                base: Base::Remote(url),
                            });
                        }
                    }
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
    diags.into_result(VendorReport {
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
pub struct ReqwestFetcher;

#[cfg(feature = "remote-fetch")]
impl RemoteFetch for ReqwestFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = reqwest::blocking::get(url)
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| error.to_string())?;
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stubbed fetcher backed by an in-memory URL → bytes map; no network.
    struct StubFetcher {
        docs: std::collections::HashMap<String, Vec<u8>>,
    }

    impl RemoteFetch for StubFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.docs
                .get(url)
                .cloned()
                .ok_or_else(|| format!("404 {url}"))
        }
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
}
