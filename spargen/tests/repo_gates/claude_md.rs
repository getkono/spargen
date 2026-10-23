//! CLAUDE.md's claims about the gates: its Quality list quotes each task's command verbatim, and
//! its testing-strategy table names every test of the suites this repository's gate tests live in.

use std::collections::BTreeSet;

use crate::repo::{mise_commands, mise_tasks, read, workspace_root};

#[test]
fn the_quality_list_quotes_its_tasks_verbatim() {
    // CLAUDE.md's Quality block lists the tasks, and glosses some with the command they run. The
    // listed tasks are exactly mise.toml's, so a task added or removed on one side fails here. A
    // gloss that is a command is a claim about the task, so it must be the task's command exactly.
    let tasks = mise_tasks();
    let claude = read("CLAUDE.md");
    let block = quality_block(&claude);
    let mut listed = BTreeSet::new();
    let mut quoted = 0usize;
    for line in block {
        let rest = line.strip_prefix("mise run ").unwrap_or_else(|| {
            panic!("CLAUDE.md's Quality block carries `{line}`, which is no `mise run <task>` line")
        });
        let (task, gloss) = rest.split_once('#').unwrap_or((rest, ""));
        let (task, gloss) = (task.trim(), gloss.trim());
        assert!(
            listed.insert(task.to_owned()),
            "CLAUDE.md's Quality block lists `mise run {task}` twice"
        );
        // `cargo hack: every feature combination…` is prose about a command, not a command.
        if !gloss.starts_with("cargo ") || gloss.contains(':') {
            continue;
        }
        assert_eq!(
            mise_commands(&tasks, task),
            [gloss],
            "CLAUDE.md glosses `mise run {task}` as `{gloss}`, which is not what it runs"
        );
        quoted += 1;
    }
    assert!(
        quoted > 0,
        "CLAUDE.md's Quality block glosses no task with its command; this test reads nothing"
    );
    let defined: BTreeSet<String> = tasks.keys().cloned().collect();
    assert_eq!(
        listed, defined,
        "CLAUDE.md's Quality block (left) must list exactly the tasks mise.toml defines (right)"
    );
}

/// The lines of the fenced block that opens CLAUDE.md's `## Quality` section, the fence lines
/// excluded.
fn quality_block(claude: &str) -> Vec<&str> {
    let mut lines = claude
        .lines()
        .skip_while(|line| *line != "## Quality")
        .skip(1)
        .skip_while(|line| !line.starts_with("```"));
    assert!(
        lines.next().is_some(),
        "CLAUDE.md carries no `## Quality` section opening a fenced block"
    );
    let block: Vec<&str> = lines
        .by_ref()
        .take_while(|line| !line.starts_with("```"))
        .collect();
    assert!(
        !block.is_empty(),
        "CLAUDE.md's Quality block is empty or unterminated; the scan has stopped reading it"
    );
    block
}

/// Does a backticked span in CLAUDE.md spell a test function's name? Every test here is a
/// snake_case sentence, and the table's other spans (`sha256`, `expect`, file paths) are not.
fn spells_test_name(span: &str) -> bool {
    span.matches('_').count() >= 3
        && span
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// The backticked spans of a table cell that cite a test: every span naming a test in `defined`
/// exactly, whatever its shape, plus every span shaped like a test name, so a citation of a test
/// that was renamed or removed is still collected and reported stale. The exact match comes
/// first because `spells_test_name`'s underscore floor would otherwise drop a defined test with
/// a short name, which the row could then never satisfy.
fn cited_test_names<'a>(cell: &'a str, defined: &BTreeSet<String>) -> Vec<&'a str> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| defined.contains(*span) || spells_test_name(span))
        .collect()
}

#[test]
fn a_cited_test_counts_whatever_its_name_is_shaped_like() {
    let defined: BTreeSet<String> = ["short_name", "a_long_test_name"].map(str::to_owned).into();
    assert_eq!(
        cited_test_names(
            "`short_name`, `a_long_test_name`, `a_renamed_test_name`, `sha256`, `x_y`",
            &defined
        ),
        ["short_name", "a_long_test_name", "a_renamed_test_name"],
        "a defined test is cited by its exact name, and a test-shaped span by its shape; \
         an undefined short span is not a citation"
    );
}

#[test]
fn the_testing_strategy_table_names_every_test_in_its_suite() {
    // CLAUDE.md's testing-strategy table is where a change learns which suite its guard belongs
    // in. Two assertions over CI configuration sat in the corpus suite for several commits while
    // the only row naming it described the corpus manifest (#230), so a broken gate was reported
    // by a suite that row gave nobody debugging it a reason to read, and the next such assertion
    // had no documented home. For each suite below, the rows whose suite it is name each of its
    // tests, and each name they cite is a test it defines, so neither side drifts from the other
    // unseen. A directory suite is every `.rs` file in it, so a test moved between its modules
    // stays counted.
    const SUITES: [&str; 2] = [
        "spargen/tests/corpus_manifest.rs",
        "spargen/tests/repo_gates/",
    ];
    let claude = read("CLAUDE.md");
    for suite_path in SUITES {
        let defined = defined_tests(suite_path);
        assert!(
            defined.len() > 1,
            "found {defined:?} in {suite_path}; the scan is not finding its tests"
        );

        let mut rows = 0usize;
        let mut named = BTreeSet::new();
        for line in claude.lines() {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // `| subsystem | suite | what to cover |` splits into five cells, the outer two empty.
            let [_, _, suite, cover, _] = cells.as_slice() else {
                continue;
            };
            if !suite.contains(&format!("`{suite_path}`")) {
                continue;
            }
            rows += 1;
            named.extend(
                cited_test_names(cover, &defined)
                    .into_iter()
                    .map(str::to_owned),
            );
        }
        assert!(
            rows > 0,
            "no row of CLAUDE.md's testing-strategy table names `{suite_path}` as its suite"
        );
        let unnamed: Vec<_> = defined.difference(&named).collect();
        assert!(
            unnamed.is_empty(),
            "{suite_path} defines {unnamed:?}, which no testing-strategy row naming it as the \
             suite mentions: add each to the row whose subject it tests, or give it a row of its \
             own"
        );
        let stale: Vec<_> = named.difference(&defined).collect();
        assert!(
            stale.is_empty(),
            "CLAUDE.md's testing-strategy rows for {suite_path} cite {stale:?}, which it does not \
             define"
        );
    }
}

/// The `#[test]` functions `suite` defines: the file it names, or, for a path ending in `/`,
/// every `.rs` file in that directory.
fn defined_tests(suite: &str) -> BTreeSet<String> {
    let sources: Vec<String> = if suite.ends_with('/') {
        let dir = workspace_root().join(suite);
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("cannot list `{dir}`: {error}"))
            .map(|entry| entry.expect("a directory entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".rs"))
            .collect();
        files.sort();
        files
            .iter()
            .map(|name| read(&format!("{suite}{name}")))
            .collect()
    } else {
        vec![read(suite)]
    };
    let mut defined = BTreeSet::new();
    for source in &sources {
        let mut lines = source.lines().map(str::trim);
        while let Some(line) = lines.next() {
            if line != "#[test]" {
                continue;
            }
            let signature = lines
                .by_ref()
                .find(|line| !line.starts_with("#[") && !line.starts_with("//"))
                .expect("a `#[test]` attribute is followed by its function");
            let name = signature
                .strip_prefix("fn ")
                .and_then(|rest| rest.split_once('('))
                .map(|(name, _)| name)
                .unwrap_or_else(|| panic!("`#[test]` is followed by `{signature}`, not a `fn`"));
            assert!(
                defined.insert(name.to_owned()),
                "{suite} defines `{name}` twice"
            );
        }
    }
    defined
}
