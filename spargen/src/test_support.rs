//! Helpers the in-crate tests of every subsystem share, compiled only into the test build.

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

/// Whether `source` declares `fn {name}(` immediately preceded by `#[test]`, with only whitespace
/// between.
///
/// The one predicate behind every check that a fixture cited for an explain clause is a `#[test]`
/// in the module named — `EXPLAIN_CLAUSES_OWNED_ELSEWHERE` in `diag/code/tests.rs` and the `E023`
/// byte-for-byte test in `runtime_contract/tests/explain.rs` — so a fix to it reaches both.
pub(crate) fn is_test_fn(source: &str, name: &str) -> bool {
    source.match_indices(&format!("fn {name}(")).any(|(at, _)| {
        source[..at]
            .rsplit_once("#[test]")
            .is_some_and(|(_, between)| between.trim().is_empty())
    })
}

mod tests {
    use super::{is_test_fn, read_repo_file};

    #[test]
    fn is_test_fn_accepts_a_function_its_test_attribute_immediately_precedes() {
        assert!(is_test_fn("#[test]\nfn pinned() {}\n", "pinned"));
        assert!(is_test_fn(
            "mod tests {\n    #[test]\n    fn pinned() {}\n}\n",
            "pinned"
        ));
    }

    #[test]
    fn is_test_fn_rejects_every_name_that_is_not_a_test() {
        // Every caller only ever asks about names it expects to be tests, so without these cases
        // the predicate could accept anything and no fixture citation would notice (#202).
        // A helper with no attribute at all.
        assert!(!is_test_fn("fn helper() {}\n", "helper"));
        // A helper that follows a test: the nearest `#[test]` above it belongs to another item.
        assert!(!is_test_fn(
            "#[test]\nfn pinned() {}\n\nfn helper() {}\n",
            "helper"
        ));
        // A name that is only a prefix of the test's.
        assert!(!is_test_fn("#[test]\nfn pinned_more() {}\n", "pinned"));
        // A name the source never declares.
        assert!(!is_test_fn("#[test]\nfn pinned() {}\n", "absent"));
    }

    /// Once the workspace gate has passed, a missing input is a failure, so a renamed
    /// `corpus/manifest.toml` or `tests/diff.rs` fails rather than passes silently (#409).
    #[test]
    #[should_panic(expected = "must exist in the repository")]
    fn a_missing_repository_file_fails_rather_than_skips() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        read_repo_file(root, "no-such-dir/no-such-manifest.toml");
    }
}
