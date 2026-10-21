//! `corpus/manifest.toml` through one typed reader, shared by `corpus_manifest.rs`, which holds
//! the repository to the manifest, and `determinism.rs`, which drives its generating cases. Each
//! integration test is its own crate, so each declares this file with
//! `#[path = "shared/corpus.rs"]`.

// Each test crate that declares this module reads the fields its checks need.
#![allow(dead_code)]

use camino::Utf8PathBuf;

#[derive(serde::Deserialize)]
pub struct Manifest {
    #[serde(rename = "case")]
    pub cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
pub struct Case {
    pub id: String,
    /// The spec's path, relative to `corpus/`.
    pub path: String,
    /// The pinned content hash of `path`. Not optional: a case that omits it would vendor a spec
    /// nothing pins, which is the drift this field exists to prevent.
    pub sha256: String,
    /// `generate` or `reject:E###`.
    pub expect: String,
}

impl Case {
    /// The diagnostic code a `reject:E###` expectation names, or `None` for `generate`.
    pub fn rejection_code(&self) -> Option<&str> {
        self.expect.strip_prefix("reject:")
    }
}

/// The corpus directory, `corpus/` at the workspace root one level above this crate.
pub fn corpus_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .join("corpus")
}

/// The parsed manifest. Every test that loops over its cases reads it here, so an empty case list
/// fails each of them instead of letting every loop pass by checking nothing.
pub fn manifest() -> Manifest {
    let path = corpus_dir().join("manifest.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path} must be readable: {error}"));
    let manifest: Manifest = toml::from_str(&text).expect("corpus/manifest.toml must parse");
    assert!(
        !manifest.cases.is_empty(),
        "corpus/manifest.toml declares no `[[case]]`, so every corpus check would pass vacuously"
    );
    manifest
}
