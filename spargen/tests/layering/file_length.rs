//! No tracked `.rs` or `.md` file outgrows the length limit unless the shrinking baseline names
//! it.

use std::collections::BTreeMap;

use crate::workspace_root;

/// The most lines a tracked `.rs` or `.md` file may have: the tree's p90 when the gate landed.
const MAX_FILE_LINES: usize = 1666;

/// The files already over [`MAX_FILE_LINES`]. It only shrinks: an entry back under the limit, or
/// no longer tracked, fails the gate until it is removed, and a new file over it is not added.
const OVER_LENGTH_BASELINE: &[&str] = &[];

/// Directories of vendored text, copied verbatim from upstream rather than written here.
const VENDORED: &[&str] = &["references/"];

fn line_count(bytes: &[u8]) -> usize {
    let unterminated = bytes.last().is_some_and(|&byte| byte != b'\n');
    bytes.iter().filter(|&&byte| byte == b'\n').count() + usize::from(unterminated)
}

#[test]
fn no_tracked_file_outgrows_the_length_limit_unless_baselined() {
    assert_eq!(
        (line_count(b""), line_count(b"a\nb\n"), line_count(b"a\nb")),
        (0, 2, 2)
    );
    let root = workspace_root();
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z", "--", "*.rs", "*.md"])
        .current_dir(&root)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "`git ls-files` failed");
    let mut lengths = BTreeMap::new();
    for path in String::from_utf8(output.stdout)
        .expect("UTF-8 paths")
        .split_terminator('\0')
    {
        let full = root.join(path);
        // A symlink (`spargen/src/support/runtime/`, `CLAUDE.md`) is measured at its target's own
        // tracked path; a tracked file deleted from the working tree is not part of the tree.
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if !meta.file_type().is_symlink() => {}
            _ => continue,
        }
        let bytes = std::fs::read(&full).unwrap_or_else(|error| panic!("{path}: {error}"));
        lengths.insert(path.to_owned(), line_count(&bytes));
    }
    for prefix in VENDORED {
        assert!(
            lengths.keys().any(|path| path.starts_with(prefix)),
            "the vendored directory {prefix} has no tracked file left: drop it from VENDORED"
        );
    }
    let over: Vec<String> = lengths
        .iter()
        .filter(|(_, lines)| **lines > MAX_FILE_LINES)
        .filter(|(path, _)| !VENDORED.iter().any(|prefix| path.starts_with(prefix)))
        .filter(|(path, _)| !OVER_LENGTH_BASELINE.contains(&path.as_str()))
        .map(|(path, lines)| format!("{path}: {lines} lines"))
        .collect();
    assert!(
        over.is_empty(),
        "these files exceed {MAX_FILE_LINES} lines; split them rather than adding them to \
         OVER_LENGTH_BASELINE: {over:#?}"
    );
    let stale: Vec<String> = OVER_LENGTH_BASELINE
        .iter()
        .filter_map(|path| match lengths.get(*path) {
            None => Some(format!("{path}: no longer a tracked file")),
            Some(&lines) if lines <= MAX_FILE_LINES => Some(format!("{path}: {lines} lines")),
            Some(_) => None,
        })
        .collect();
    assert!(
        stale.is_empty(),
        "remove these entries from OVER_LENGTH_BASELINE, which only shrinks: {stale:#?}"
    );
}
