//! Generating a description into a throwaway directory and reading the module back, shared by the
//! suites that inspect what a description generates (`snapshot.rs`, `recipes.rs`,
//! `determinism.rs`). Each integration test is its own crate, so each declares this file with
//! `#[path = "shared/generate.rs"]`.

use std::io::ErrorKind;

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Report, Spec};

/// Run `generate` on `spec`, its module written into a fresh temporary directory outside any Cargo
/// build, and return the report plus the module text.
///
/// The text is `Some` exactly when the outcome is `Generated`: a run that generates must have
/// written its module, and any other run must have written none. Each call writes to a directory
/// of its own, so two calls on one `spec` differ only in where the module went.
pub fn generate_to_string(spec: Spec) -> (Report, Option<String>) {
    let temp = tempfile::tempdir().unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let report = spargen::generate(&spec.build(out.clone()).cargo(CargoIntegration::Off));
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
