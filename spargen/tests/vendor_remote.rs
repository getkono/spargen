//! `spargen lock`'s real, networked fetcher driven over a local HTTP socket.
//!
//! Every in-module `vendor` test uses an in-memory stub fetcher, so without this suite the
//! reqwest-backed fetcher — the generator's only networked path — would be reached by no test at
//! all. Here a plain-HTTP server on `127.0.0.1` serves the remote documents, and the public
//! [`spargen::vendor`] facade (and, under `cli`, the `spargen lock` binary) fetches them for real:
//! request, status handling, redirect following, and connection failure are all the real ones.
//!
//! What this does **not** reach is a completed TLS handshake: the fetcher trusts only the bundled
//! web roots, so a local HTTPS server would need a certificate-trust seam the fetcher does not
//! have. `tests/tls_strictness.rs` drives the handshake up to the point it is refused, which
//! needs no trust.

#![cfg(feature = "remote-fetch")]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use camino::Utf8PathBuf;
use sha2::{Digest, Sha256};
use spargen::{CargoIntegration, Code, Outcome, Report, Spec};

/// What the mock server answers for one path.
#[derive(Clone)]
enum Reply {
    /// `200 OK` with this body.
    Body(&'static str),
    /// `301 Moved Permanently` to this server-relative location.
    Redirect(&'static str),
    /// A bodyless response with this status line (e.g. `404 Not Found`).
    Status(&'static str),
}

/// A minimal HTTP/1.1 server: one request per connection, answered from a fixed route table
/// (unknown paths get `404`). Every requested path is recorded, so a test can assert what the
/// fetcher actually asked for and how often.
struct MockServer {
    base: String,
    hits: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    fn start(routes: &[(&'static str, Reply)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: BTreeMap<&'static str, Reply> = routes.iter().cloned().collect();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&hits);
        // The thread outlives the test only as long as the test binary; it is never joined.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                answer(stream, &routes, &recorded);
            }
        });
        Self { base, hits }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

/// Read one request head, record its path, then write the routed reply.
///
/// The path is recorded *before* the reply is written. Once `write_all` returns, the client may
/// already have its response and the test may already be reading the hit list, so a path
/// recorded after the write could be missing from that read.
fn answer(
    mut stream: TcpStream,
    routes: &BTreeMap<&'static str, Reply>,
    recorded: &Mutex<Vec<String>>,
) -> Option<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).ok()?;
    let path = head.split_whitespace().nth(1)?.to_owned();
    let (status, headers, body) = match routes.get(path.as_str()) {
        Some(Reply::Body(body)) => ("200 OK", String::new(), *body),
        Some(Reply::Redirect(location)) => (
            "301 Moved Permanently",
            format!("Location: {location}\r\n"),
            "",
        ),
        Some(Reply::Status(status)) => (*status, String::new(), ""),
        None => ("404 Not Found", String::new(), ""),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    recorded.lock().unwrap().push(path);
    stream.write_all(response.as_bytes()).ok()
}

/// A remote schema whose own `$ref` is relative, so it must resolve against the fetched URL.
const PET_YAML: &str = "type: object\n\
                        required: [id]\n\
                        properties:\n  \
                        id: { type: integer, format: int64 }\n  \
                        tag: { $ref: 'tag.yaml' }\n";

const TAG_YAML: &str = "type: object\n\
                        required: [label]\n\
                        properties:\n  \
                        label: { type: string }\n";

/// A root description whose two operations both `$ref` the same remote document.
fn root_spec(remote: &str) -> String {
    root_spec_naming(remote, remote)
}

/// A root description whose first operation `$ref`s `first` and whose second `$ref`s `second`,
/// in that document order.
fn root_spec_naming(first: &str, second: &str) -> String {
    format!(
        "openapi: 3.1.0\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         servers: [{{ url: 'https://e.com' }}]\n\
         paths:\n  \
         /pet:\n    \
         get:\n      \
         operationId: getPet\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '{first}' }}\n  \
         /pet/again:\n    \
         get:\n      \
         operationId: getPetAgain\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '{second}' }}\n"
    )
}

/// Write `root_spec(remote)` into a fresh tempdir and return the dir and the spec path.
fn workspace(remote: &str) -> (tempfile::TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec = dir.join("openapi.yaml");
    std::fs::write(&spec, root_spec(remote)).unwrap();
    (temp, spec)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The single diagnostic a failed fetch reports — `E025`, and never `E003` "not pinned", which
/// would blame the document for a network failure — with its message.
fn fetch_failure(report: &Report) -> String {
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{report:#?}");
    assert_eq!(diagnostics[0].code, Code::RemoteFetchFailed, "{report:#?}");
    diagnostics[0].message.clone()
}

#[test]
fn vendors_remote_refs_over_http_and_then_resolves_them_without_the_network() {
    let server = MockServer::start(&[
        ("/schemas/pet.yaml", Reply::Body(PET_YAML)),
        ("/schemas/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let pet_url = server.url("/schemas/pet.yaml");
    let tag_url = server.url("/schemas/tag.yaml");
    let (_temp, spec_path) = workspace(&pet_url);
    let spec = Spec::new(spec_path.clone());

    let report = spargen::vendor(&spec).unwrap_or_else(|report| panic!("{report:#?}"));

    // Both documents were fetched exactly once: the root names `pet.yaml` twice, and `tag.yaml`
    // was found only by resolving the fetched document's relative `$ref` against its own URL.
    let mut hits = server.hits();
    hits.sort();
    assert_eq!(hits, ["/schemas/pet.yaml", "/schemas/tag.yaml"]);

    let urls: Vec<&str> = report.refs.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(urls, [pet_url.as_str(), tag_url.as_str()]);
    for (vendored, served) in report.refs.iter().zip([PET_YAML, TAG_YAML]) {
        assert_eq!(
            vendored.sha256,
            sha256_hex(served.as_bytes()),
            "{vendored:?}"
        );
        let on_disk = std::fs::read(report.vendor_dir.join(&vendored.path)).unwrap();
        assert_eq!(on_disk, served.as_bytes(), "{vendored:?}");
    }
    assert_eq!(report.lock_path, spec_path.with_file_name("spargen.lock"));
    let lock = std::fs::read_to_string(&report.lock_path).unwrap();
    for vendored in &report.refs {
        assert!(
            lock.contains(&format!("url = \"{}\"", vendored.url)),
            "{lock}"
        );
        assert!(lock.contains(&vendored.sha256), "{lock}");
    }

    // The pinned copies are what generation reads: the remote schemas lower to typed fields, and
    // neither `generate` nor `check` asks the server for anything.
    let out = spec_path.with_file_name("client.rs");
    let generated =
        spargen::generate(&spec.clone().build(out.clone()).cargo(CargoIntegration::Off));
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    assert!(code.contains("pub label"), "{code}");
    assert!(code.contains("pub tag"), "{code}");
    let checked = spargen::check(&spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert_eq!(server.hits().len(), 2, "generation must not fetch");
}

#[test]
fn an_http_error_status_is_e025_naming_the_url_and_the_status() {
    let server = MockServer::start(&[("/gone.yaml", Reply::Status("410 Gone"))]);
    let url = server.url("/gone.yaml");
    let (_temp, spec_path) = workspace(&url);

    let report = spargen::vendor(&Spec::new(spec_path.clone())).unwrap_err();

    let message = fetch_failure(&report);
    assert!(message.contains(&url), "{message}");
    assert!(message.contains("410"), "{message}");
    assert!(
        !spec_path.with_file_name(".spargen").exists(),
        "a failed fetch must vendor nothing"
    );
}

#[test]
fn a_redirect_is_followed_and_pinned_under_the_url_the_spec_names() {
    let server = MockServer::start(&[
        ("/moved.yaml", Reply::Redirect("/schemas/tag.yaml")),
        ("/schemas/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let url = server.url("/moved.yaml");
    let (_temp, spec_path) = workspace(&url);

    let report = spargen::vendor(&Spec::new(spec_path)).unwrap_or_else(|r| panic!("{r:#?}"));

    assert_eq!(server.hits(), ["/moved.yaml", "/schemas/tag.yaml"]);
    assert_eq!(report.refs.len(), 1, "{report:?}");
    assert_eq!(report.refs[0].url, url);
    assert_eq!(report.refs[0].sha256, sha256_hex(TAG_YAML.as_bytes()));
}

/// A redirected document's relative `$ref`s resolve against the URL it was retrieved from, not the
/// one the spec names (RFC 3986 §5.1.3, #405): `tag.yaml` inside `/old/pet.yaml`, which redirects
/// to `/new/pet.yaml`, is `/new/tag.yaml`. `/old/tag.yaml` does not exist, so resolving against
/// the requested URL fails the lock with `E025`. The lock records the retrieval URL beside the
/// pin, and generation resolves the vendored copy's references against it, offline — the
/// relative one and the in-document one, which is made absolute against that same base.
#[test]
fn a_redirected_documents_relative_refs_resolve_against_the_url_it_was_retrieved_from() {
    const MOVED_PET_YAML: &str = "type: object\n\
                                  required: [id]\n\
                                  properties:\n  \
                                  id: { type: integer, format: int64 }\n  \
                                  tag: { $ref: 'tag.yaml' }\n  \
                                  owner: { $ref: '#/$defs/Owner' }\n\
                                  $defs:\n  \
                                  Owner:\n    \
                                  type: object\n    \
                                  required: [name]\n    \
                                  properties:\n      \
                                  name: { type: string }\n";
    let server = MockServer::start(&[
        ("/old/pet.yaml", Reply::Redirect("/new/pet.yaml")),
        ("/new/pet.yaml", Reply::Body(MOVED_PET_YAML)),
        ("/new/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let requested = server.url("/old/pet.yaml");
    let retrieved = server.url("/new/pet.yaml");
    let tag_url = server.url("/new/tag.yaml");
    let (_temp, spec_path) = workspace(&requested);
    let spec = Spec::new(spec_path.clone());

    let report = spargen::vendor(&spec).unwrap_or_else(|r| panic!("{r:#?}"));

    assert_eq!(
        server.hits(),
        ["/old/pet.yaml", "/new/pet.yaml", "/new/tag.yaml"]
    );
    let urls: Vec<&str> = report.refs.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(urls, [tag_url.as_str(), requested.as_str()]);
    let lock = std::fs::read_to_string(&report.lock_path).unwrap();
    assert!(
        lock.contains(&format!(
            "url = \"{requested}\"\nretrieval_url = \"{retrieved}\"\n"
        )),
        "{lock}"
    );
    assert!(
        !lock.contains(&format!("url = \"{tag_url}\"\nretrieval_url")),
        "an unredirected pin records no retrieval URL: {lock}"
    );

    let out = spec_path.with_file_name("client.rs");
    let generated =
        spargen::generate(&spec.clone().build(out.clone()).cargo(CargoIntegration::Off));
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    assert!(code.contains("pub label"), "{code}");
    assert!(code.contains("pub tag"), "{code}");
    assert!(code.contains("pub name"), "{code}");
    assert_eq!(server.hits().len(), 3, "generation must not fetch");
}

/// Generate `spec_path` offline into a sibling `client.rs`, require it not be rejected, and return
/// the emitted source.
fn generate_offline(spec_path: &Utf8PathBuf) -> String {
    let out = spec_path.with_file_name("client.rs");
    let generated = spargen::generate(
        &Spec::new(spec_path.clone())
            .build(out.clone())
            .cargo(CargoIntegration::Off),
    );
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    std::fs::read_to_string(&out).unwrap()
}

/// The URL each `[[remote]]` pin in the lock at `report.lock_path` is keyed by, in lock order.
fn pinned_urls(report: &spargen::VendorReport) -> Vec<String> {
    std::fs::read_to_string(&report.lock_path)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("url = \""))
        .map(|rest| rest.trim_end_matches('"').to_owned())
        .collect()
}

/// A redirected document that names itself by its retrieval URL (`pet.yaml#/$defs/Owner` inside
/// `/new/pet.yaml`, reached through `/old/pet.yaml`) is the document already fetched, as the build
/// treats it: the build registers the vendored copy under its retrieval URL too, so it never reads
/// a pin for `/new/pet.yaml`, and `spargen lock` must neither fetch that URL again nor pin it.
#[test]
fn a_redirected_document_naming_itself_by_its_retrieval_url_is_not_fetched_again() {
    const SELF_NAMING_PET_YAML: &str = "type: object\n\
                                        required: [id]\n\
                                        properties:\n  \
                                        id: { type: integer, format: int64 }\n  \
                                        owner: { $ref: 'pet.yaml#/$defs/Owner' }\n\
                                        $defs:\n  \
                                        Owner:\n    \
                                        type: object\n    \
                                        required: [name]\n    \
                                        properties:\n      \
                                        name: { type: string }\n";
    let server = MockServer::start(&[
        ("/old/pet.yaml", Reply::Redirect("/new/pet.yaml")),
        ("/new/pet.yaml", Reply::Body(SELF_NAMING_PET_YAML)),
    ]);
    let requested = server.url("/old/pet.yaml");
    let (_temp, spec_path) = workspace(&requested);

    let report =
        spargen::vendor(&Spec::new(spec_path.clone())).unwrap_or_else(|r| panic!("{r:#?}"));

    assert_eq!(server.hits(), ["/old/pet.yaml", "/new/pet.yaml"]);
    assert_eq!(pinned_urls(&report), [requested]);
    let code = generate_offline(&spec_path);
    assert!(code.contains("pub owner"), "{code}");
    assert!(code.contains("pub name"), "{code}");
    assert_eq!(server.hits().len(), 2, "generation must not fetch");
}

/// A spec that names a redirect's target before the redirecting URL pins both: the target is met
/// first, so it is fetched under its own name and the build loads it from its own pin, and the
/// redirecting URL is pinned beside it with `retrieval_url` naming the target. The build keeps the
/// target's own pin under the target URL rather than re-registering the redirected copy there.
/// Named the other way round, the target is the document the redirect already fetched, and it is
/// pinned once.
#[test]
fn a_spec_naming_a_redirect_and_its_target_pins_what_the_build_reads_in_either_order() {
    let server = MockServer::start(&[
        ("/old/pet.yaml", Reply::Redirect("/new/pet.yaml")),
        ("/new/pet.yaml", Reply::Body(PET_YAML)),
        ("/new/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let requested = server.url("/old/pet.yaml");
    let retrieved = server.url("/new/pet.yaml");
    let tag_url = server.url("/new/tag.yaml");

    // Target first: every named URL is fetched once, and both are pinned.
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec_path = dir.join("openapi.yaml");
    std::fs::write(&spec_path, root_spec_naming(&retrieved, &requested)).unwrap();
    let report =
        spargen::vendor(&Spec::new(spec_path.clone())).unwrap_or_else(|r| panic!("{r:#?}"));
    assert_eq!(
        server.hits(),
        [
            "/new/pet.yaml",
            "/old/pet.yaml",
            "/new/pet.yaml",
            "/new/tag.yaml"
        ]
    );
    assert_eq!(
        pinned_urls(&report),
        [retrieved.clone(), tag_url.clone(), requested.clone()]
    );
    let lock = std::fs::read_to_string(&report.lock_path).unwrap();
    assert!(
        lock.contains(&format!(
            "url = \"{requested}\"\nretrieval_url = \"{retrieved}\"\n"
        )),
        "{lock}"
    );
    let code = generate_offline(&spec_path);
    assert!(code.contains("pub label"), "{code}");
    assert_eq!(server.hits().len(), 4, "generation must not fetch");

    // Redirect first: the target is the document already retrieved, so it is not pinned again.
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec_path = dir.join("openapi.yaml");
    std::fs::write(&spec_path, root_spec_naming(&requested, &retrieved)).unwrap();
    let report =
        spargen::vendor(&Spec::new(spec_path.clone())).unwrap_or_else(|r| panic!("{r:#?}"));
    assert_eq!(
        server.hits()[4..],
        ["/old/pet.yaml", "/new/pet.yaml", "/new/tag.yaml"]
    );
    assert_eq!(pinned_urls(&report), [tag_url, requested]);
    let code = generate_offline(&spec_path);
    assert!(code.contains("pub label"), "{code}");
    assert_eq!(server.hits().len(), 7, "generation must not fetch");
}

/// A relative `$self` in a redirected document resolves against the URL the document was
/// retrieved from, like its relative `$ref`s: `$self: pet-canonical.yaml` inside `/new/pet.yaml`,
/// reached through `/old/pet.yaml`, is `/new/pet-canonical.yaml`, so `tag.yaml` is the pinned
/// `/new/tag.yaml`. Resolved against the requested URL instead, the identity would be
/// `/old/pet-canonical.yaml` and `tag.yaml` the unpinned `/old/tag.yaml` (`E003`). The in-document
/// reference is made absolute against that identity, under which the document is registered.
#[test]
fn a_redirected_documents_relative_self_resolves_against_the_url_it_was_retrieved_from() {
    const SELF_PET_YAML: &str = "$self: pet-canonical.yaml\n\
                                 type: object\n\
                                 required: [id]\n\
                                 properties:\n  \
                                 id: { type: integer, format: int64 }\n  \
                                 tag: { $ref: 'tag.yaml' }\n  \
                                 owner: { $ref: '#/$defs/Owner' }\n\
                                 $defs:\n  \
                                 Owner:\n    \
                                 type: object\n    \
                                 required: [name]\n    \
                                 properties:\n      \
                                 name: { type: string }\n";
    let server = MockServer::start(&[
        ("/old/pet.yaml", Reply::Redirect("/new/pet.yaml")),
        ("/new/pet.yaml", Reply::Body(SELF_PET_YAML)),
        ("/new/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let (_temp, spec_path) = workspace(&server.url("/old/pet.yaml"));

    spargen::vendor(&Spec::new(spec_path.clone())).unwrap_or_else(|r| panic!("{r:#?}"));

    let code = generate_offline(&spec_path);
    assert!(code.contains("pub label"), "{code}");
    assert!(code.contains("pub name"), "{code}");
    assert_eq!(server.hits().len(), 3, "generation must not fetch");
}

/// A fetched document whose `$self` names another directory resolves its relative `$ref`s from
/// that identity, in `spargen lock` as in the build (#426): `$self: ../b/pet.yaml` inside
/// `/a/pet.yaml` makes `tag.yaml` the document at `/b/tag.yaml`. `/a/tag.yaml` exists too, with
/// a different shape, so a lock that resolved from the retrieval URL would succeed, pin the wrong
/// document, and leave the build rejecting `/b/tag.yaml` as unpinned (`E003`).
#[test]
fn a_remote_documents_self_in_another_directory_is_the_base_lock_resolves_from() {
    const SELF_ELSEWHERE_PET_YAML: &str = "$self: ../b/pet.yaml\n\
                                           type: object\n\
                                           required: [id]\n\
                                           properties:\n  \
                                           id: { type: integer, format: int64 }\n  \
                                           tag: { $ref: 'tag.yaml' }\n";
    const DECOY_TAG_YAML: &str = "type: object\n\
                                  required: [decoy]\n\
                                  properties:\n  \
                                  decoy: { type: string }\n";
    let server = MockServer::start(&[
        ("/a/pet.yaml", Reply::Body(SELF_ELSEWHERE_PET_YAML)),
        ("/a/tag.yaml", Reply::Body(DECOY_TAG_YAML)),
        ("/b/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let pet_url = server.url("/a/pet.yaml");
    let (_temp, spec_path) = workspace(&pet_url);

    let report =
        spargen::vendor(&Spec::new(spec_path.clone())).unwrap_or_else(|r| panic!("{r:#?}"));

    assert_eq!(server.hits(), ["/a/pet.yaml", "/b/tag.yaml"]);
    assert_eq!(pinned_urls(&report), [pet_url, server.url("/b/tag.yaml")]);
    let code = generate_offline(&spec_path);
    assert!(code.contains("pub label"), "{code}");
    assert!(!code.contains("pub decoy"), "{code}");
    let checked = spargen::check(&Spec::new(spec_path));
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert_eq!(server.hits().len(), 2, "generation must not fetch");
}

#[test]
fn a_refused_connection_is_e025_naming_the_url() {
    // Bind and release a port so nothing is listening on it.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/pet.yaml");
    let (_temp, spec_path) = workspace(&url);

    let report = spargen::vendor(&Spec::new(spec_path)).unwrap_err();

    let message = fetch_failure(&report);
    assert!(message.contains(&url), "{message}");
    // The cause, not only reqwest's "error sending request": the refused connect's own I/O
    // error, whose std rendering always carries the OS error number.
    assert!(message.contains("(os error "), "{message}");
}

/// The same fetch through the installed binary, so `spargen lock`'s own wiring is reached too.
#[cfg(feature = "cli")]
#[test]
fn spargen_lock_vendors_over_http_and_reports_what_it_pinned() {
    let server = MockServer::start(&[
        ("/schemas/pet.yaml", Reply::Body(PET_YAML)),
        ("/schemas/tag.yaml", Reply::Body(TAG_YAML)),
    ]);
    let (_temp, spec_path) = workspace(&server.url("/schemas/pet.yaml"));

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_spargen"))
        .arg("lock")
        .arg(spec_path.as_str())
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("vendored 2 remote document(s)"), "{stdout}");
    assert!(
        stdout.contains(&server.url("/schemas/tag.yaml")),
        "{stdout}"
    );
    assert!(spec_path.with_file_name("spargen.lock").exists());
}
