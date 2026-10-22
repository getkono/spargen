//! No tracked `.rs` or `.md` file outgrows the length limit unless the shrinking baseline names
//! it, and no baselined file grows past its ceiling there.

use std::collections::BTreeMap;

use crate::workspace_root;

/// The most lines a tracked `.rs` or `.md` file may have: the tree's p90 when the gate landed.
const MAX_FILE_LINES: usize = 1666;

/// The files already over [`MAX_FILE_LINES`], each as `(path, ceiling)`: the file may not grow
/// past its ceiling, which starts at the file's length when it is listed and only lowers. The
/// list only shrinks: an entry back under the limit, or no longer tracked, fails the gate until it
/// is removed, and a new file over the limit is not added.
const OVER_LENGTH_BASELINE: &[(&str, usize)] = &[];

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
    let violations = length_violations(&lengths, OVER_LENGTH_BASELINE);
    assert!(
        violations.over.is_empty(),
        "these files exceed {MAX_FILE_LINES} lines; split them rather than adding them to \
         OVER_LENGTH_BASELINE: {:#?}",
        violations.over
    );
    assert!(
        violations.grown.is_empty(),
        "these baselined files have grown past their OVER_LENGTH_BASELINE ceiling, which only \
         lowers; shrink them rather than raising it: {:#?}",
        violations.grown
    );
    assert!(
        violations.stale.is_empty(),
        "remove these entries from OVER_LENGTH_BASELINE, which only shrinks: {:#?}",
        violations.stale
    );
}

/// What the length gate rejects, each as `path: detail` lines.
#[derive(Debug, Default, PartialEq)]
struct LengthViolations {
    /// Over the limit, not vendored, and not baselined.
    over: Vec<String>,
    /// Baselined, and longer than the entry's ceiling.
    grown: Vec<String>,
    /// Baselined, but no longer tracked or back at or under the limit.
    stale: Vec<String>,
}

fn length_violations(
    lengths: &BTreeMap<String, usize>,
    baseline: &[(&str, usize)],
) -> LengthViolations {
    let ceiling = |path: &str| {
        baseline
            .iter()
            .find(|(entry, _)| *entry == path)
            .map(|(_, ceiling)| *ceiling)
    };
    let over = lengths
        .iter()
        .filter(|(_, lines)| **lines > MAX_FILE_LINES)
        .filter(|(path, _)| !VENDORED.iter().any(|prefix| path.starts_with(prefix)))
        .filter(|(path, _)| ceiling(path).is_none())
        .map(|(path, lines)| format!("{path}: {lines} lines"))
        .collect();
    let grown = baseline
        .iter()
        .filter_map(|(path, ceiling)| match lengths.get(*path) {
            Some(&lines) if lines > *ceiling => {
                Some(format!("{path}: {lines} lines, ceiling {ceiling}"))
            }
            _ => None,
        })
        .collect();
    let stale = baseline
        .iter()
        .filter_map(|(path, _)| match lengths.get(*path) {
            None => Some(format!("{path}: no longer a tracked file")),
            Some(&lines) if lines <= MAX_FILE_LINES => Some(format!("{path}: {lines} lines")),
            Some(_) => None,
        })
        .collect();
    LengthViolations { over, grown, stale }
}

/// The gate's three verdicts on a fixed tree: an unlisted file over the limit, a baselined file
/// past its ceiling, and baselined entries back under the limit or gone; a baselined file at or
/// under its ceiling and a vendored file pass.
#[test]
fn the_length_gate_holds_each_baselined_file_to_its_ceiling() {
    let over = MAX_FILE_LINES + 1;
    let lengths: BTreeMap<String, usize> = [
        ("new.rs", over),
        ("references/spec.md", over + 500),
        ("held.rs", over + 10),
        ("grown.rs", over + 11),
        ("shrunk.rs", MAX_FILE_LINES),
    ]
    .into_iter()
    .map(|(path, lines)| (path.to_owned(), lines))
    .collect();
    let baseline = [
        ("held.rs", over + 10),
        ("grown.rs", over + 10),
        ("shrunk.rs", over),
        ("gone.rs", over),
    ];
    assert_eq!(
        length_violations(&lengths, &baseline),
        LengthViolations {
            over: vec![format!("new.rs: {over} lines")],
            grown: vec![format!(
                "grown.rs: {} lines, ceiling {}",
                over + 11,
                over + 10
            )],
            stale: vec![
                format!("shrunk.rs: {MAX_FILE_LINES} lines"),
                "gone.rs: no longer a tracked file".to_owned(),
            ],
        }
    );
}
