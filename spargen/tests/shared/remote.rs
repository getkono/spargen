//! The root description `spargen lock`'s real fetcher is pointed at, and what a failed fetch must
//! report, shared by `vendor_remote.rs` (a local plain-HTTP server) and `tls_strictness.rs` (a
//! non-conformant TLS peer). Each integration test is its own crate, so each declares this file
//! with `#[path = "shared/remote.rs"]`.

use camino::Utf8PathBuf;
use spargen::{Code, Outcome, Report};

/// The operations [`root_spec`] declares, one per remote reference, in document order.
const OPERATIONS: [(&str, &str); 2] = [("/pet", "getPet"), ("/pet/again", "getPetAgain")];

/// A root description whose `n`th operation's `200` response schema is `refs[n]`, for one or two
/// references.
pub fn root_spec(refs: &[&str]) -> String {
    assert!(
        (1..=OPERATIONS.len()).contains(&refs.len()),
        "a root description names one or two remote references, not {}",
        refs.len()
    );
    let mut spec = String::from(
        "openapi: 3.1.0\n\
         info: { title: T, version: 1.0.0 }\n\
         servers: [{ url: 'https://e.com' }]\n\
         paths:\n",
    );
    for ((path, operation_id), url) in OPERATIONS.iter().zip(refs) {
        spec.push_str(&format!(
            "  {path}:\n    \
             get:\n      \
             operationId: {operation_id}\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json:\n              \
             schema: {{ $ref: '{url}' }}\n"
        ));
    }
    spec
}

/// Write [`root_spec`]`(refs)` as `openapi.yaml` into a fresh tempdir and return the dir and the
/// spec path.
pub fn workspace(refs: &[&str]) -> (tempfile::TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec = dir.join("openapi.yaml");
    std::fs::write(&spec, root_spec(refs)).unwrap();
    (temp, spec)
}

/// The message of the single diagnostic a failed fetch of `url` for the root at `spec` reports —
/// `E025`, and never `E003` "not pinned", which would blame the document for a network failure.
/// The message names `url`, and nothing was vendored beside `spec`.
pub fn fetch_failure(report: &Report, spec: &Utf8PathBuf, url: &str) -> String {
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{report:#?}");
    assert_eq!(diagnostics[0].code, Code::RemoteFetchFailed, "{report:#?}");
    let message = diagnostics[0].message.clone();
    assert!(message.contains(url), "{message}");
    assert!(
        !spec.with_file_name(".spargen").exists(),
        "a failed fetch must vendor nothing"
    );
    message
}
