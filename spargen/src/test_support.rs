//! Helpers the in-crate tests of every subsystem share, compiled only into the test build. Not a
//! subsystem and not facade plumbing: `spargen/tests/layering/` admits it by name in its
//! `TEST_SUPPORT` list, so a subsystem's tests reach it without a `layer-deps:` edge.

use std::path::{Path, PathBuf};

/// The workspace root, or `None` when this crate is tested from a packaged `.crate`, which
/// carries neither the workspace manifest nor the documents, corpus and test suites the callers
/// read. Gating on the *workspace marker* rather than on the file under inspection is deliberate:
/// inside the repository a missing file must fail the test, not silently skip it.
pub(crate) fn repo_root() -> Option<PathBuf> {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).to_path_buf();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    manifest.contains("[workspace]").then_some(root)
}

/// Read a file the repository must carry, failing — never skipping — when it is absent.
/// Only call this under [`repo_root`], which is what decides whether the file must exist.
pub(crate) fn read_repo_file(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|error| {
        panic!("{relative} must exist in the repository, and reading it failed: {error}")
    })
}

/// Every `.rs` file under `dir`, recursively, in a stable order.
pub(crate) fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                into.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files
}

mod tests {
    use super::read_repo_file;

    /// Once the workspace gate has passed, a missing input is a failure: a skip keyed on the file
    /// itself turned a renamed `corpus/manifest.toml` or `tests/diff.rs` into a silent pass (#409).
    #[test]
    #[should_panic(expected = "must exist in the repository")]
    fn a_missing_repository_file_fails_rather_than_skips() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        read_repo_file(root, "no-such-dir/no-such-manifest.toml");
    }
}
