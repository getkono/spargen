//! `corpus/manifest.toml` as the single source of corpus expectations.
//!
//! CLAUDE.md names the manifest as where a pinned spec's expected outcome lives, and says
//! expectations change "only with a reviewed reason". Nothing read it: the same expectations were
//! restated by hand in the `corpus-smoke` mise task, again in the CI job, and again in
//! `tests/snapshot.rs`. They had already drifted — `openai-openapi` was in the manifest and the
//! snapshot suite but in neither smoke copy.
//!
//! This suite drives the manifest itself, and holds the other copies to it. The repository's
//! assertions over its own gate configuration live in `tests/repo_gates/`, which reads `mise.toml`
//! and `.github/workflows/` through the same `shared/repo.rs` readers this suite does. CLAUDE.md's
//! corpus row names this file and every test here
//! (`the_testing_strategy_table_names_every_test_in_its_suite`).
//!
//! One manifest field is held only to its `corpus/README.md` mirror, never to the files it hashes:
//! `tree_sha256`, carried by `openapi-boilerplate` alone. How
//! it was constructed is recorded nowhere, and no natural definition over that directory
//! reproduces it, so verifying it would mean inventing a rule and calling the result a guarantee.
//! Every case's `sha256` — the per-file pin the support documents cite — is verified below.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use spargen::{Outcome, Severity, Spec};

#[path = "shared/corpus.rs"]
mod corpus;

use corpus::{manifest, Case};

#[path = "shared/repo.rs"]
mod repo;

use repo::{ci_workflow, mise_commands, mise_tasks, read, workspace_root};

#[test]
fn every_declared_expectation_is_a_shape_the_suite_understands() {
    // A typo such as `expect = "rejects:E001"` would otherwise make a case silently unchecked.
    for case in manifest().cases {
        assert!(
            case.expect == "generate" || case.rejection_code().is_some_and(|code| code.len() == 4),
            "`{}` declares `expect = {:?}`, which is neither `generate` nor `reject:E###`",
            case.id,
            case.expect
        );
    }
}

#[test]
fn every_pinned_spec_is_present_and_is_not_an_unfetched_lfs_pointer() {
    // The corpus is Git-LFS. Without smudging, each file is a ~130-byte pointer that parses as
    // neither JSON nor YAML, and every corpus assertion below would fail for the wrong reason.
    for case in manifest().cases {
        let path = workspace_root().join("corpus").join(&case.path);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("`{}` is missing at {path}: {error}", case.id));
        assert!(
            !bytes.starts_with(b"version https://git-lfs.github.com/spec/"),
            "`{}` is an unfetched Git-LFS pointer — run `git lfs pull`",
            case.id
        );
    }
}

#[test]
fn every_pinned_spec_matches_the_hash_the_manifest_declares() {
    // The manifest records where each spec came from and what it hashed to. Nothing read the hash,
    // so a re-fetched, hand-edited, or silently-updated corpus file would have changed the meaning
    // of every expectation below it while still reporting a pass.
    for case in manifest().cases {
        let path = workspace_root().join("corpus").join(&case.path);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("`{}` is missing at {path}: {error}", case.id));

        let actual = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            actual, case.sha256,
            "`{}` no longer matches its pinned hash. The vendored file at {path} changed; \
             re-pin it in corpus/manifest.toml only with a reviewed reason.",
            case.id
        );
    }
}

#[test]
fn every_case_meets_its_declared_expectation() {
    for case in manifest().cases {
        let spec = Spec::new(workspace_root().join("corpus").join(&case.path))
            // The big descriptions produce more than the default 100 diagnostics, and a truncated
            // batch can hide the terminal rejection code the manifest names.
            .batch_cap(usize::MAX);
        let report = spargen::check(&spec);
        // Every diagnostic's declared claim must be one the run's outcome admits (#413). This
        // checks the declared claim only, not the message prose: a message that states an outcome
        // its claim does not declare (#174's "is generated", which was built `Independent`) passes
        // here. `frontend/main.rs`'s `claim_violations` reads the prose and is what catches that shape.
        let contradicted: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| !report.outcome().admits(diagnostic.claim))
            .collect();
        assert!(
            contradicted.is_empty(),
            "`{}` reported diagnostics whose claim its `{}` outcome contradicts: {contradicted:#?}",
            case.id,
            report.outcome()
        );

        match case.rejection_code() {
            None => assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{}` is declared `generate` but was rejected: {:?}",
                case.id,
                report
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.code.as_str().starts_with('E'))
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect::<BTreeSet<_>>()
            ),
            Some(expected) => {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{}` is declared `{}` but was not rejected",
                    case.id,
                    case.expect
                );
                // The whole set of error codes, not just the presence of the named one: a second
                // rejection appearing beside it changes what the case pins.
                let codes: BTreeSet<&str> = report
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == Severity::Error)
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect();
                assert_eq!(
                    codes,
                    BTreeSet::from([expected]),
                    "`{}` is declared `{}` but its rejection codes are {codes:?}",
                    case.id,
                    case.expect
                );
            }
        }
    }
}

/// Does `haystack` name `id` as a whole token? A bare `contains` would let `github-api-3-1` stand
/// in for `github-api-3-0`'s coverage and vice versa.
fn names_case(haystack: &str, id: &str) -> bool {
    haystack.match_indices(id).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + id.len()..].chars().next();
        let boundary = |ch: Option<char>| {
            ch.is_none_or(|ch| !ch.is_alphanumeric() && ch != '-' && ch != '_' && ch != '.')
        };
        boundary(before) && boundary(after)
    })
}

#[test]
fn the_corpus_smoke_gate_covers_every_manifest_case() {
    // CLAUDE.md points at `mise run corpus-smoke` for the corpus row, so a manifest case the task
    // never runs is a case that gate does not actually check. The task and the CI job restate the
    // same list, so both are held to the manifest: each is read as its own commands, not as the
    // whole file a path might appear anywhere in, and each case's command must state the
    // manifest's expectation.
    assert_smoke_expectation_reads_the_commands_a_case_needs();
    let mise = mise_commands(&mise_tasks(), "corpus-smoke");
    let workflow = ci_workflow();
    let ci: Vec<String> = workflow["jobs"]["corpus-smoke"]["steps"]
        .as_vec()
        .expect("ci.yml carries a `corpus-smoke` job with steps")
        .iter()
        .filter_map(|step| step["run"].as_str().map(str::to_owned))
        .collect();

    for case in manifest().cases {
        for (gate, commands) in [
            ("`mise run corpus-smoke`", &mise),
            ("the CI corpus-smoke job", &ci),
        ] {
            if let Err(problem) = smoke_expectation(commands, &case) {
                panic!(
                    "`{}` is declared `{}`, but {gate} {problem}",
                    case.id, case.expect
                );
            }
        }
    }
}

/// Whether the smoke `commands` check `case` the way its expectation says: exactly one command
/// runs `check` on it; for `generate` that command must succeed; for `reject:E###` it must accept
/// exit status 1 into a `target/corpus-smoke/` report, and the next command must find the code in
/// that report.
fn smoke_expectation(commands: &[String], case: &Case) -> Result<(), String> {
    const REJECTED: &str = " || test $? -eq 1";
    let path = format!("corpus/{}", case.path);
    let runs: Vec<usize> = commands
        .iter()
        .enumerate()
        .filter(|(_, command)| command.contains(" check ") && names_case(command, &path))
        .map(|(at, _)| at)
        .collect();
    let [at] = runs.as_slice() else {
        return Err(format!(
            "runs `check` on `{path}` {} times, not once",
            runs.len()
        ));
    };
    let command = &commands[*at];
    match case.rejection_code() {
        None if command.contains(REJECTED) => Err(format!("accepts a rejection from `{command}`")),
        None => Ok(()),
        Some(code) => {
            let report = command
                .strip_suffix(REJECTED)
                .and_then(|command| command.rsplit_once(" > "))
                .map(|(_, report)| report)
                .filter(|report| report.starts_with("target/corpus-smoke/"))
                .ok_or_else(|| {
                    format!(
                        "does not run `{command}` as `… > target/corpus-smoke/<name>.json{REJECTED}`"
                    )
                })?;
            let grep = format!("grep -q {code} {report}");
            match commands.get(at + 1) {
                Some(next) if *next == grep => Ok(()),
                next => Err(format!("follows `{command}` with {next:?}, not `{grep}`")),
            }
        }
    }
}

/// [`smoke_expectation`] pinned on fixtures rather than on whatever the gate says today. Called by
/// the gate test, so the reader is proven before its verdict on the real commands is trusted.
fn assert_smoke_expectation_reads_the_commands_a_case_needs() {
    let case = |expect: &str| Case {
        id: "pet".to_owned(),
        path: "pet/openapi.yaml".to_owned(),
        sha256: String::new(),
        upstream: String::new(),
        revision: String::new(),
        upstream_path: String::new(),
        tree_sha256: None,
        expect: expect.to_owned(),
    };
    let commands = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| (*line).to_owned())
            .collect::<Vec<_>>()
    };
    let check = "cargo run -- check corpus/pet/openapi.yaml --format json";
    let rejected = format!("{check} > target/corpus-smoke/pet.json || test $? -eq 1");

    assert_eq!(
        smoke_expectation(&commands(&[check]), &case("generate")),
        Ok(())
    );
    assert_eq!(
        smoke_expectation(
            &commands(&[&rejected, "grep -q E001 target/corpus-smoke/pet.json"]),
            &case("reject:E001")
        ),
        Ok(())
    );
    for (lines, expect) in [
        (vec![], "generate"),
        (vec![check, check], "generate"),
        (
            vec!["cargo run -- check corpus/pet/openapi.yaml.bak"],
            "generate",
        ),
        (vec![rejected.as_str()], "generate"),
        (vec![check], "reject:E001"),
        (vec![rejected.as_str()], "reject:E001"),
        (
            vec![
                rejected.as_str(),
                "grep -q E009 target/corpus-smoke/pet.json",
            ],
            "reject:E001",
        ),
        (
            vec![
                "cargo run -- check corpus/pet/openapi.yaml > /tmp/pet.json || test $? -eq 1",
                "grep -q E001 /tmp/pet.json",
            ],
            "reject:E001",
        ),
    ] {
        assert!(
            smoke_expectation(&commands(&lines), &case(expect)).is_err(),
            "{lines:?} must not satisfy `{expect}`"
        );
    }
}

#[test]
fn the_snapshot_suite_covers_every_manifest_case() {
    // "Per-corpus outcome plus a sorted diagnostic histogram" — five of nine cases had one, so
    // four real-world specs could change what they produce with no reviewable diff anywhere.
    // Read as code, not text: a case counts only when a `#[test]` passes its path to
    // `generate_corpus`, and that test asserts the outcome the manifest declares in an
    // `assert_eq!`.
    assert_snapshot_tests_count_only_asserted_outcomes();
    let tests = snapshot_tests(&read("spargen/tests/snapshot.rs"));
    let cases = manifest().cases;
    for case in &cases {
        let expected = if case.rejection_code().is_some() {
            "Rejected"
        } else {
            "Generated"
        };
        let covering: Vec<&(String, BTreeSet<String>)> = tests
            .iter()
            .filter(|(path, _)| *path == case.path)
            .collect();
        let [(_, outcomes)] = covering.as_slice() else {
            panic!(
                "`{}` is snapshotted by {} tests in spargen/tests/snapshot.rs, not one",
                case.id,
                covering.len()
            );
        };
        assert_eq!(
            outcomes,
            &BTreeSet::from([expected.to_owned()]),
            "`{}` is declared `{}`, so its snapshot test must assert `Outcome::{expected}` alone",
            case.id,
            case.expect
        );
    }
    for (path, _) in &tests {
        assert!(
            cases.iter().any(|case| case.path == *path),
            "spargen/tests/snapshot.rs snapshots `{path}`, which is no manifest case"
        );
    }
}

/// Each `#[test]` in `source` that calls `generate_corpus("<path>", …)`, as the path and the
/// `Outcome::` variants its body names inside an `assert_eq!`'s arguments, at any depth of the
/// body. A variant named anywhere else (a `match` arm, a `let`, an `assert_ne!`) asserts nothing
/// the manifest declares, so it is not counted.
fn snapshot_tests(source: &str) -> Vec<(String, BTreeSet<String>)> {
    use proc_macro2::{TokenStream, TokenTree};
    use quote::ToTokens;

    /// `asserted` is whether `tokens` sit inside an `assert_eq!(…)` invocation's arguments.
    fn walk(
        tokens: TokenStream,
        asserted: bool,
        paths: &mut Vec<String>,
        outcomes: &mut BTreeSet<String>,
    ) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for (at, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(group) => {
                    let invokes_assert_eq = at >= 2
                        && matches!(&trees[at - 1], TokenTree::Punct(p) if p.as_char() == '!')
                        && matches!(&trees[at - 2], TokenTree::Ident(ident) if *ident == "assert_eq");
                    walk(
                        group.stream(),
                        asserted || invokes_assert_eq,
                        paths,
                        outcomes,
                    );
                }
                TokenTree::Ident(ident) if *ident == "generate_corpus" => {
                    let Some(TokenTree::Group(arguments)) = trees.get(at + 1) else {
                        continue;
                    };
                    let first = arguments.stream().into_iter().next();
                    if let Some(path) =
                        first.and_then(|first| syn::parse2::<syn::LitStr>(first.into()).ok())
                    {
                        paths.push(path.value());
                    }
                }
                TokenTree::Ident(ident) if asserted && *ident == "Outcome" => {
                    let colon = |at: usize| matches!(trees.get(at), Some(TokenTree::Punct(p)) if p.as_char() == ':');
                    if !(colon(at + 1) && colon(at + 2)) {
                        continue;
                    }
                    if let Some(TokenTree::Ident(variant)) = trees.get(at + 3) {
                        outcomes.insert(variant.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let file = syn::parse_file(source).expect("spargen/tests/snapshot.rs parses");
    let mut tests = Vec::new();
    for item in &file.items {
        let syn::Item::Fn(function) = item else {
            continue;
        };
        if !function
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("test"))
        {
            continue;
        }
        let (mut paths, mut outcomes) = (Vec::new(), BTreeSet::new());
        walk(
            function.block.to_token_stream(),
            false,
            &mut paths,
            &mut outcomes,
        );
        tests.extend(paths.into_iter().map(|path| (path, outcomes.clone())));
    }
    assert!(
        !tests.is_empty(),
        "no `#[test]` in spargen/tests/snapshot.rs calls `generate_corpus` with a path literal: \
         the scan has stopped reading the suite"
    );
    tests
}

/// [`snapshot_tests`] pinned on fixtures, as [`assert_smoke_expectation_reads_the_commands_a_case_needs`]
/// pins the smoke reader: called by the coverage test before its verdict on the real suite.
fn assert_snapshot_tests_count_only_asserted_outcomes() {
    let outcomes = |body: &str| {
        let source = format!(
            "#[test]\nfn case() {{\n    let (report, _) = generate_corpus(\"pet/openapi.yaml\", false);\n    {body}\n}}\n"
        );
        let tests = snapshot_tests(&source);
        let [(path, outcomes)] = tests.as_slice() else {
            panic!(
                "the fixture holds one test, and the scan found {}",
                tests.len()
            );
        };
        assert_eq!(path, "pet/openapi.yaml");
        outcomes.iter().cloned().collect::<Vec<_>>()
    };
    assert_eq!(
        outcomes("assert_eq!(report.outcome(), Outcome::Rejected, \"{report:#?}\");"),
        ["Rejected"]
    );
    assert_eq!(
        outcomes("if true { assert_eq!(report.outcome(), Outcome::Generated); }"),
        ["Generated"]
    );
    for body in [
        "let _ = Outcome::Rejected;",
        "assert_ne!(report.outcome(), Outcome::Rejected);",
        "assert!(matches!(report.outcome(), Outcome::Rejected));",
        "match report.outcome() { Outcome::Rejected => {}, _ => {} }",
    ] {
        assert!(
            outcomes(body).is_empty(),
            "`{body}` asserts no outcome, so it must not count as one"
        );
    }
}

#[test]
fn the_corpus_readme_mirrors_the_manifest() {
    // CLAUDE.md says the manifest's expectations are "mirrored in `corpus/README.md`". Every cell
    // of each case's table row is read against the manifest field it restates, so a mirror that
    // names the case but states another outcome, upstream, revision, path or hash fails too.
    let rows = readme_rows(&read("corpus/README.md"));
    let cases = manifest().cases;
    for case in &cases {
        let row = rows.get(&case.id).unwrap_or_else(|| {
            panic!(
                "`{}` is in the manifest but not in corpus/README.md",
                case.id
            )
        });
        let sha256 = match &case.tree_sha256 {
            Some(tree) => format!("tree `{tree}`; root `{}`", case.sha256),
            None => format!("`{}`", case.sha256),
        };
        for (column, cell, manifest) in [
            ("Upstream", &row.upstream, format!("`{}`", case.upstream)),
            ("Revision", &row.revision, format!("`{}`", case.revision)),
            ("Path", &row.path, format!("`{}`", case.upstream_path)),
            ("SHA-256", &row.sha256, sha256),
            ("Expected", &row.expect, case.expect.clone()),
        ] {
            assert_eq!(
                cell, &manifest,
                "corpus/README.md's `{}` row says {cell} in its {column} column, and the \
                 manifest says {manifest}",
                case.id
            );
        }
    }
    let stale: Vec<&String> = rows
        .keys()
        .filter(|id| !cases.iter().any(|case| case.id == **id))
        .collect();
    assert!(
        stale.is_empty(),
        "corpus/README.md lists {stale:?}, which the manifest does not declare"
    );
}

/// One case row of corpus/README.md's table, each cell trimmed and kept verbatim except
/// `expect`, which is the `Expected` cell read in the manifest's grammar.
struct ReadmeRow {
    upstream: String,
    revision: String,
    path: String,
    sha256: String,
    expect: String,
}

/// corpus/README.md's case table, by case ID. The `Expected` column reads in the manifest's
/// grammar: `Generate` as `generate`, and `Reject `E###` …` as `reject:E###`.
fn readme_rows(readme: &str) -> BTreeMap<String, ReadmeRow> {
    let mut rows = BTreeMap::new();
    for line in readme.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| ID | Upstream | Revision | Path | SHA-256 | Expected |`, the outer two cells empty.
        let [_, id, upstream, revision, path, sha256, expected, _] = cells.as_slice() else {
            continue;
        };
        let Some(id) = id.strip_prefix('`').and_then(|id| id.strip_suffix('`')) else {
            continue;
        };
        let expect = if *expected == "Generate" {
            "generate".to_owned()
        } else {
            let code = expected
                .strip_prefix("Reject `")
                .and_then(|rest| rest.split_once('`'))
                .map(|(code, _)| code)
                .unwrap_or_else(|| {
                    panic!("corpus/README.md's `{id}` row expects {expected:?}, which reads as neither `Generate` nor `Reject `E###``")
                });
            format!("reject:{code}")
        };
        let row = ReadmeRow {
            upstream: (*upstream).to_owned(),
            revision: (*revision).to_owned(),
            path: (*path).to_owned(),
            sha256: (*sha256).to_owned(),
            expect,
        };
        assert!(
            rows.insert(id.to_owned(), row).is_none(),
            "corpus/README.md lists `{id}` twice"
        );
    }
    assert!(
        !rows.is_empty(),
        "corpus/README.md has no case table row: the scan has stopped reading it"
    );
    rows
}
