//! Writing a description to disk, generating it into a throwaway directory, and reading the module
//! back, shared by the integration suites that write or inspect what a description generates. Each
//! integration test is its own crate, so each declares this file with
//! `#[path = "shared/generate.rs"]`; a suite that calls only some of these helpers allows
//! `dead_code` on that declaration, naming what it uses.

use std::io::ErrorKind;
use std::path::Path;

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Report, Spec};

/// Write `contents` to `name` in `dir`, returning the file's path.
pub fn write_spec(dir: &Path, name: &str, contents: &str) -> Utf8PathBuf {
    let path = Utf8PathBuf::from_path_buf(dir.join(name)).expect("the directory path is UTF-8");
    std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{path} is writable: {error}"));
    path
}

/// The module `contents` generates, written as `openapi.yaml` into a fresh directory, with
/// `configure` applied to its [`Spec`]. The run must reach `Generated`.
pub fn generate_text(contents: &str, configure: impl FnOnce(Spec) -> Spec) -> String {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(dir.path(), "openapi.yaml", contents);
    let (report, text) = generate_to_string(configure(Spec::new(spec)));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    text.expect("a `Generated` run wrote its module")
}

/// Run `generate` on `spec`, its module written into a fresh temporary directory outside any Cargo
/// build, and return the report plus the module text.
///
/// The text is `Some` exactly when the outcome is `Generated`: a run that generates must have
/// written its module, and any other run must have written none. Each call writes to a directory
/// of its own, so two calls on one `spec` differ only in where the module went.
pub fn generate_to_string(spec: Spec) -> (Report, Option<String>) {
    let temp = tempfile::tempdir().unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let report = spargen::generate(&spec.build(out.clone()).with_cargo(CargoIntegration::Off));
    let text = match std::fs::read_to_string(&out) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => panic!("the module at {out} is unreadable: {error}"),
    };
    assert_eq!(
        text.is_some(),
        report.outcome() == Outcome::Generated,
        "a `{:?}` run {} its module: {report:#?}",
        report.outcome(),
        if text.is_some() {
            "wrote"
        } else {
            "did not write"
        },
    );
    (report, text)
}
