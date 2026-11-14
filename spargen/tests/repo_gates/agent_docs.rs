//! The agent instructions stay short and scoped: `AGENTS.md` (which `CLAUDE.md` links to) holds
//! only the rules that apply everywhere, and each `.claude/rules/*.md` file the rules for the paths
//! its `paths:` frontmatter names. `AGENTS.md` names each rule file, so an agent that reads only it
//! still finds them.

use crate::repo::{read, workspace_root};

/// The most lines `AGENTS.md` may have. It is loaded into every session; a rule that applies to
/// one area belongs in a scoped `.claude/rules/` file instead. Raising this is a reviewed edit.
const MAX_AGENTS_LINES: usize = 80;

/// The most lines one `.claude/rules/*.md` file may have, frontmatter included.
const MAX_RULE_LINES: usize = 60;

/// The `.claude/rules/*.md` files, by path relative to the workspace root, sorted.
fn rule_files() -> Vec<String> {
    let dir = workspace_root().join(".claude/rules");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot list `{dir}`: {error}"))
        .map(|entry| entry.expect("a directory entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .map(|name| format!(".claude/rules/{name}"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "found no rule file in `{dir}`");
    files
}

#[test]
fn agent_instructions_stay_short() {
    let agents = read("AGENTS.md").lines().count();
    assert!(
        agents <= MAX_AGENTS_LINES,
        "AGENTS.md has {agents} lines, over {MAX_AGENTS_LINES}: move what applies to one area \
         into a scoped `.claude/rules/` file, and cut what the code already says"
    );
    for file in rule_files() {
        let lines = read(&file).lines().count();
        assert!(
            lines <= MAX_RULE_LINES,
            "{file} has {lines} lines, over {MAX_RULE_LINES}: cut what the code already says, \
             or split it by the paths each part governs"
        );
    }
}

#[test]
fn agents_md_names_every_rule_file() {
    let agents = read("AGENTS.md");
    for file in rule_files() {
        let name = file.trim_start_matches(".claude/rules/");
        assert!(
            agents.contains(&format!("`{name}`")),
            "AGENTS.md does not name `{name}`; list it with the area it covers"
        );
    }
}

/// The `paths:` globs of a rule file's frontmatter, which must open the file.
fn rule_paths(file: &str, text: &str) -> Vec<String> {
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{file} opens with no frontmatter"
    );
    let frontmatter: Vec<&str> = lines.by_ref().take_while(|line| *line != "---").collect();
    let mut entries = frontmatter.iter();
    assert_eq!(
        entries.next().copied(),
        Some("paths:"),
        "{file}'s frontmatter must be a `paths:` list, so the rule loads only for those paths"
    );
    let globs: Vec<String> = entries
        .map(|line| {
            line.trim()
                .strip_prefix("- ")
                .map(|glob| glob.trim_matches('"').to_owned())
                .unwrap_or_else(|| panic!("{file}'s `paths:` carries `{line}`, not a list entry"))
        })
        .collect();
    assert!(!globs.is_empty(), "{file}'s `paths:` list is empty");
    globs
}

/// Does `path` match `glob`, where `**` is any number of whole segments and `*` any run of
/// characters within one segment?
fn glob_matches(glob: &str, path: &str) -> bool {
    fn segments(glob: &[&str], path: &[&str]) -> bool {
        match glob.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
            Some((head, rest)) => path.split_first().is_some_and(|(segment, tail)| {
                segment_matches(head, segment) && segments(rest, tail)
            }),
        }
    }
    fn segment_matches(pattern: &str, segment: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == segment,
            Some((prefix, rest)) => segment.strip_prefix(prefix).is_some_and(|tail| {
                (0..=tail.len())
                    .filter(|at| tail.is_char_boundary(*at))
                    .any(|at| segment_matches(rest, &tail[at..]))
            }),
        }
    }
    let glob: Vec<&str> = glob.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&glob, &path)
}

#[test]
fn the_glob_matcher_reads_star_and_double_star() {
    for (glob, path, expected) in [
        ("mise.toml", "mise.toml", true),
        ("mise.toml", "spargen/mise.toml", false),
        ("**/Cargo.toml", "Cargo.toml", true),
        ("**/Cargo.toml", "examples/petstore/Cargo.toml", true),
        ("corpus/**", "corpus/manifest.toml", true),
        ("corpus/**", "corpus/recipes/a/b.json", true),
        ("corpus/**", "corpusx/manifest.toml", false),
        ("spargen/tests/e2e.rs", "spargen/tests/e2e/specs.rs", false),
        ("docs/*.md", "docs/errors.md", true),
        ("docs/*.md", "docs/book/src/cli.md", false),
    ] {
        assert_eq!(
            glob_matches(glob, path),
            expected,
            "`{glob}` against `{path}`"
        );
    }
}

#[test]
fn every_rule_file_is_scoped_to_files_that_exist() {
    // A rule whose glob matches nothing never loads, so a rename that orphans it would silently
    // drop the rule. Each glob must match a tracked file.
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(workspace_root())
        .output()
        .expect("git runs");
    assert!(output.status.success(), "`git ls-files` failed");
    let tracked = String::from_utf8(output.stdout).expect("UTF-8 paths");
    let tracked: Vec<&str> = tracked.split_terminator('\0').collect();
    for file in rule_files() {
        assert!(
            tracked.contains(&file.as_str()),
            "{file} is not tracked, so no other checkout carries it"
        );
        for glob in rule_paths(&file, &read(&file)) {
            assert!(
                tracked.iter().any(|path| glob_matches(&glob, path)),
                "{file} is scoped to `{glob}`, which matches no tracked file"
            );
        }
    }
}
