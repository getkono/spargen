//! `corpus/manifest.toml` as the single source of corpus expectations.
//!
//! CLAUDE.md names the manifest as where a pinned spec's expected outcome lives, and says
//! expectations change "only with a reviewed reason". Nothing read it: the same expectations were
//! restated by hand in the `corpus-smoke` mise task, again in the CI job, and again in
//! `tests/snapshot.rs`. They had already drifted — `openai-openapi` was in the manifest and the
//! snapshot suite but in neither smoke copy.
//!
//! This suite drives the manifest itself, and holds the other copies to it. It is also where this
//! repository's assertions over its own CI configuration have collected, so the gates over
//! `.github/workflows/ci.yml` live here beside the corpus ones rather than in a file of their own.
//!
//! One manifest field stays unchecked: `tree_sha256`, carried by `openapi-boilerplate` alone. How
//! it was constructed is recorded nowhere, and no natural definition over that directory
//! reproduces it, so verifying it would mean inventing a rule and calling the result a guarantee.
//! Every case's `sha256` — the per-file pin the support documents cite — is verified below.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use sha2::{Digest, Sha256};
use spargen::{Outcome, Spec};

#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(rename = "case")]
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    path: String,
    /// The pinned content hash of `path`. Not optional: a case that omits it would vendor a spec
    /// nothing pins, which is the drift this field exists to prevent.
    sha256: String,
    /// `generate` or `reject:E###`.
    expect: String,
}

fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .to_owned()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path} must be readable: {error}"))
}

fn manifest() -> Manifest {
    toml::from_str(&read("corpus/manifest.toml")).expect("corpus/manifest.toml must parse")
}

impl Case {
    /// The diagnostic code a `reject:E###` expectation names, or `None` for `generate`.
    fn rejection_code(&self) -> Option<&str> {
        self.expect.strip_prefix("reject:")
    }
}

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
                let codes: BTreeSet<&str> = report
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect();
                assert!(
                    codes.contains(expected),
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
    // same list, so both are held to the manifest.
    let mise = read("mise.toml");
    let ci = read(".github/workflows/ci.yml");

    for case in manifest().cases {
        assert!(
            names_case(&mise, &case.path),
            "`{}` is in the manifest but `mise run corpus-smoke` never checks it",
            case.id
        );
        assert!(
            names_case(&ci, &case.path),
            "`{}` is in the manifest but the CI corpus-smoke job never checks it",
            case.id
        );
    }
}

#[test]
fn the_corpus_smoke_gate_writes_only_inside_the_checkout() {
    // A fixed name under the shared /tmp is owned by whoever ran the gate first; on a sticky
    // /tmp every later user's redirect fails with `Permission denied` before spargen runs
    // (#93). Both copies of the gate write under the gitignored `target/corpus-smoke/`.
    for file in ["mise.toml", ".github/workflows/ci.yml"] {
        let text = read(file);
        assert!(
            !text.contains("/tmp/"),
            "`{file}` writes to a fixed `/tmp/` path; corpus-smoke outputs belong under `target/corpus-smoke/`"
        );
    }
}

/// Flags that narrow what cargo-deny *resolves or consults*, rather than which checks it runs
/// over the result. Measured on this tree with the `mise.toml` pin (cargo-deny 0.19.9), as
/// `cargo-deny --log-level warn --manifest-path ./Cargo.toml --all-features <flag> check
/// advisories`: `--exclude rustls` and `--target wasm32-unknown-unknown` each turn
/// `advisories FAILED` (RUSTSEC-2026-0285, reached only through reqwest's TLS feature) into
/// `advisories ok`, exit 1 to exit 0. The other five do *not* flip that verdict here and are
/// rejected as the same class of flag rather than on a measured flip -- `--offline` was
/// measured against an already-populated advisory database, and `--no-default-features` is
/// overridden by the `--all-features` this same value is required to carry. Not exhaustive:
/// see `the_deny_gate_states_the_feature_scope_it_audits`, and #238. `-t` is clap's short alias
/// for `--target` (the only one of these flags cargo-deny 0.19.9's `--help` gives a short form),
/// and is the same flag: `--all-features -t wasm32-unknown-unknown` drops rustls from
/// `cargo deny list` exactly as the long spelling does.
///
/// Shared by both copies of the deny gate: CI's cargo-deny-action step and `mise run deny`.
const GRAPH_NARROWING_FLAGS: [&str; 8] = [
    "--exclude",
    "--target",
    "-t",
    "--exclude-dev",
    "--exclude-unpublished",
    "--offline",
    "--frozen",
    "--no-default-features",
];

/// The flag a single argv word spells. `--flag value` and `--flag=value` are the same flag to
/// clap, and so are `-t value`, `-t=value`, and the attached `-tvalue`: a short flag is its first
/// two characters.
fn flag_of(token: &str) -> &str {
    match token.strip_prefix('-') {
        Some(rest) if !rest.starts_with('-') && rest.len() > 1 => token.get(..2).unwrap_or(token),
        _ => token.split_once('=').map_or(token, |(flag, _)| flag),
    }
}

#[test]
fn the_deny_gate_states_the_feature_scope_it_audits() {
    // `--all-features` is what puts a TLS stack in the audited graph: under default features
    // `rustls` is absent from the workspace entirely, so an advisory gate run without the flag
    // passes because it can see nothing (#147). The action's own defaults happen to match, which
    // is exactly why deleting these lines would read as tidying rather than as narrowing the gate.
    //
    // Asserted over the parsed document rather than over the text, and per step rather than per
    // job. A line-level assertion over the job's text cannot tell this step's `with:` from one
    // hung on `actions/checkout`, cannot see an `if:` that stops the job running at all, and reds
    // on a requoted or strictly stricter value that audits exactly the same graph.
    //
    // Both scopes are guarded, because a gate that does not run and a gate whose failure is
    // swallowed are indistinguishable from a gate that audits nothing: `if:` and
    // `continue-on-error:` are checked on the job map *and* on the step map. `continue-on-error:`
    // is read by *value* rather than by presence, since `false` is byte-for-byte GitHub's own
    // default and neutralises nothing; `if:` is banned by presence, because its value is an
    // expression that cannot be evaluated here.
    //
    // Narrowing is guarded on three inputs, and only on the steps that audit the *root*
    // manifest. Two of them reach `check`'s `[WHICH]...` positional -- `command` and
    // `command-arguments` -- since either alone drops `advisories`, and `command-arguments` must
    // be *stated* empty rather than absent: leaving it absent would pin the action's default,
    // which is the one thing this gate exists to stop being load-bearing. The third is
    // `arguments` itself, which the entrypoint's unquoted `cargo-deny $*` splices straight into
    // the argv, so any word in it is a flag: `--all-features --target wasm32-unknown-unknown`
    // contains `--all-features`, reads as *added* coverage, and drops RUSTSEC-2026-0285. The
    // deny-list below is a **list, not a proof** -- it rejects the graph-narrowing flags that
    // were measured to hide this tree's live advisory, and cannot establish that some other
    // `arguments` value does not narrow. Containment rather than equality is deliberate:
    // `--all-features --locked` is strictly stricter and is #146's own ask.
    //
    // Scope: `if:` and `continue-on-error:` are asserted on *every* matching step, because any
    // of them being neutralised is this job not running what it says it runs. The three
    // narrowing assertions apply only where `manifest-path` is absent or names the root
    // `Cargo.toml`, because applying them everywhere turns "no step may narrow the gate" into
    // "every step must be maximal" -- which reds #184's cheapest shape (a cargo-deny step per
    // example workspace with `command-arguments: advisories`, root `deny.toml` untouched; the
    // root policy is red on bans and licenses for all three examples and green on advisories).
    // At least one root-manifest audit is required, so scoping by `manifest-path` cannot be used
    // to empty the gate by pointing its only step somewhere else.
    let ci = read(".github/workflows/ci.yml");
    let documents = yaml_rust2::YamlLoader::load_from_str(&ci)
        .expect("`.github/workflows/ci.yml` must parse as YAML");
    let workflow = documents
        .first()
        .expect("`.github/workflows/ci.yml` must carry a YAML document");

    let deny = &workflow["jobs"]["deny"];
    assert!(
        !deny.is_badvalue(),
        "`.github/workflows/ci.yml` must define a `deny` job"
    );
    assert!(
        deny["if"].is_badvalue(),
        "the `deny` job map carries an `if:` key, so the whole audit can be conditioned out"
    );
    assert!(
        deny["continue-on-error"].is_badvalue()
            || deny["continue-on-error"].as_bool() == Some(false),
        "the `deny` job map sets `continue-on-error:` to something other than `false`, so a \
         failing audit need not fail the gate"
    );

    let steps = deny["steps"]
        .as_vec()
        .expect("the `deny` job must carry a list of steps");
    // GitHub resolves `uses: owner/repo@ref` case-insensitively, so the comparison is too:
    // `embarkstudios/cargo-deny-action@v2` is a working spelling and must not red a gate that
    // audits the same graph. *Every* match is audited, not the first and not exactly one: a
    // second step is only unread if the test declines to read it, and forbidding one would
    // forbid the obvious shape of #184 (a cargo-deny step per example workspace manifest) for no
    // gain -- GitHub runs steps in order and fails the job on the first failure, so a later step
    // cannot weaken an earlier one.
    let audits: Vec<_> = steps
        .iter()
        .filter(|step| {
            step["uses"].as_str().is_some_and(|uses| {
                uses.to_ascii_lowercase()
                    .starts_with("embarkstudios/cargo-deny-action@")
            })
        })
        .collect();
    assert!(
        !audits.is_empty(),
        "the `deny` job runs no step that `uses: EmbarkStudios/cargo-deny-action@…`, so nothing \
         in it audits the dependency graph"
    );

    let mut root_audits = 0usize;
    for audit in audits {
        assert!(
            audit["if"].is_badvalue(),
            "the cargo-deny-action step map carries an `if:` key, so the audit can be \
             conditioned out while the `deny` job it sits in still reports success"
        );
        assert!(
            audit["continue-on-error"].is_badvalue()
                || audit["continue-on-error"].as_bool() == Some(false),
            "the cargo-deny-action step map sets `continue-on-error:` to something other than \
             `false`, so a failing audit would leave the `deny` job green"
        );

        // `manifest-path` absent means the action's `./Cargo.toml` default, which is the
        // workspace root; `Cargo.toml` and `./Cargo.toml` are the same file and both spellings
        // are accepted so that writing the default out does not red the gate.
        let audits_root_manifest = match audit["with"]["manifest-path"].as_str() {
            None => true,
            Some(path) => path.trim_start_matches("./") == "Cargo.toml",
        };
        if !audits_root_manifest {
            continue;
        }
        root_audits += 1;

        let arguments = audit["with"]["arguments"]
            .as_str()
            .expect("the cargo-deny-action step must state `with: { arguments: … }` of its own");
        assert!(
            arguments
                .split_whitespace()
                .any(|token| token == "--all-features"),
            "the cargo-deny-action step's `arguments: {arguments}` does not pass `--all-features`"
        );
        for token in arguments.split_whitespace() {
            let flag = flag_of(token);
            assert!(
                !GRAPH_NARROWING_FLAGS.contains(&flag),
                "the cargo-deny-action step's `arguments: {arguments}` passes `{flag}`, which \
                 shrinks the graph cargo-deny resolves rather than the checks it runs over it: \
                 the entrypoint splices `arguments` into an unquoted `cargo-deny $*`, so \
                 `--all-features {flag} …` still contains `--all-features` and still drops \
                 RUSTSEC-2026-0285. Strictly stricter values such as `--all-features --locked` \
                 are deliberately still accepted"
            );
        }

        let command = audit["with"]["command"]
            .as_str()
            .expect("the cargo-deny-action step must state `with: { command: … }` of its own");
        assert_eq!(
            command, "check",
            "the cargo-deny-action step's `command` selects a subset of the checks; anything \
             narrower than a bare `check` drops `advisories`, which is the check #147 is about"
        );

        let command_arguments = audit["with"]["command-arguments"].as_str().expect(
            "the cargo-deny-action step must state `with: { command-arguments: \"\" } ` of its \
             own: it is the second input feeding `check`'s `[WHICH]...` positional, and leaving \
             it absent inherits the action's default for the one remaining input that can \
             silently narrow the audit",
        );
        assert_eq!(
            command_arguments, "",
            "the cargo-deny-action step's `command-arguments` narrows `check`'s `[WHICH]...` \
             positional: `command-arguments: licenses` composes `cargo-deny --all-features check \
             licenses` and drops `advisories` exactly as a narrowed `command` does, leaving \
             `command: check` true and this suite otherwise green"
        );
    }

    assert!(
        root_audits > 0,
        "no cargo-deny-action step in the `deny` job audits the root `Cargo.toml`: every one \
         states a `manifest-path` pointing elsewhere, so the workspace this gate exists to audit \
         is audited by nothing and the three narrowing assertions above never run"
    );
}

#[test]
fn the_mise_deny_task_audits_the_graph_ci_audits() {
    // `mise run deny` is the supply-chain gate CLAUDE.md hands a contributor, and CI spells its
    // own copy out rather than calling it, so the two are kept in step by hand. They were not:
    // the task ran a bare `cargo deny check`, which resolves default features only and so has no
    // `rustls` in its graph, and reported `advisories ok` on the very lockfile CI failed with
    // RUSTSEC-2026-0285 (#141). The flag later arrived in an unrelated commit with nothing
    // holding it there.
    //
    // Held to CI rather than to a literal: every word CI's root-manifest cargo-deny-action step
    // passes as `arguments` must also be a global flag of the task, so tightening CI (say,
    // `--all-features --locked`, #146) reds here until the local gate follows. The task is also
    // held to the rules the CI step is held to -- `--all-features`, no graph-narrowing flag, a
    // bare `check` with no `[WHICH]...` narrowing it, and no `--manifest-path` pointing away from
    // the workspace root.
    let ci = read(".github/workflows/ci.yml");
    let documents = yaml_rust2::YamlLoader::load_from_str(&ci)
        .expect("`.github/workflows/ci.yml` must parse as YAML");
    let workflow = documents
        .first()
        .expect("`.github/workflows/ci.yml` must carry a YAML document");
    let ci_arguments: BTreeSet<String> = workflow["jobs"]["deny"]["steps"]
        .as_vec()
        .expect("the `deny` job must carry a list of steps")
        .iter()
        .filter(|step| {
            step["uses"].as_str().is_some_and(|uses| {
                uses.to_ascii_lowercase()
                    .starts_with("embarkstudios/cargo-deny-action@")
            }) && step["with"]["manifest-path"]
                .as_str()
                .is_none_or(|path| path.trim_start_matches("./") == "Cargo.toml")
        })
        .filter_map(|step| step["with"]["arguments"].as_str())
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect();
    assert!(
        ci_arguments.contains("--all-features"),
        "CI's root-manifest cargo-deny-action step passes no `--all-features`, so there is no \
         CI feature scope for `mise run deny` to be held to"
    );

    let mise: toml::Value = toml::from_str(&read("mise.toml")).expect("mise.toml must parse");
    let commands: Vec<&str> = match &mise["tasks"]["deny"]["run"] {
        toml::Value::String(command) => vec![command.as_str()],
        toml::Value::Array(commands) => commands
            .iter()
            .map(|command| {
                command
                    .as_str()
                    .expect("every `[tasks.deny] run` entry must be a command string")
            })
            .collect(),
        other => panic!("`[tasks.deny] run` must be a string or an array of strings, not {other}"),
    };

    let audits: Vec<Vec<&str>> = commands
        .iter()
        .map(|command| command.split_whitespace().collect::<Vec<_>>())
        .filter(|words| {
            words.starts_with(&["cargo", "deny"]) || words.first() == Some(&"cargo-deny")
        })
        .collect();
    assert!(
        !audits.is_empty(),
        "`mise run deny` runs no `cargo deny` command, so the local gate audits nothing"
    );

    for words in audits {
        let command = words.join(" ");
        let skip = if words[0] == "cargo-deny" { 1 } else { 2 };
        let check = words
            .iter()
            .position(|word| *word == "check")
            .unwrap_or_else(|| panic!("`mise run deny` runs `{command}`, which is not `check`"));
        let globals = &words[skip..check];
        let which = &words[check + 1..];

        for argument in &ci_arguments {
            assert!(
                globals.contains(&argument.as_str()),
                "`mise run deny` runs `{command}` but CI's cargo-deny-action step passes \
                 `{argument}`, so the local gate audits a different graph from CI's and can \
                 pass on a lockfile CI fails"
            );
        }
        for word in globals {
            let flag = flag_of(word);
            assert!(
                !GRAPH_NARROWING_FLAGS.contains(&flag),
                "`mise run deny` runs `{command}`, whose `{flag}` shrinks the graph cargo-deny \
                 resolves below the one CI audits"
            );
            assert_ne!(
                flag, "--manifest-path",
                "`mise run deny` runs `{command}`, which names a `--manifest-path`; CI audits \
                 the workspace root, and so must the local gate"
            );
        }
        assert!(
            which.is_empty(),
            "`mise run deny` runs `{command}`, which narrows `check` to {which:?}; CI runs a \
             bare `check`, so every check it runs must run locally too"
        );
    }
}

#[test]
fn the_snapshot_suite_covers_every_manifest_case() {
    // "Per-corpus outcome plus a sorted diagnostic histogram" — five of nine cases had one, so
    // four real-world specs could change what they produce with no reviewable diff anywhere.
    let suite = read("spargen/tests/snapshot.rs");
    for case in manifest().cases {
        assert!(
            names_case(&suite, &case.path),
            "`{}` has no snapshot in spargen/tests/snapshot.rs",
            case.id
        );
    }
}

#[test]
fn the_corpus_readme_mirrors_the_manifest() {
    // CLAUDE.md says the manifest's expectations are "mirrored in `corpus/README.md`".
    let readme = read("corpus/README.md");
    for case in manifest().cases {
        assert!(
            names_case(&readme, &case.id),
            "`{}` is in the manifest but not in corpus/README.md",
            case.id
        );
    }
}
