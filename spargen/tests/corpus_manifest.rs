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

use std::collections::{BTreeMap, BTreeSet};

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
             it absent inherits the action's default for one of the three inputs \
             (`arguments`, `command`, `command-arguments`) that can silently narrow the audit",
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

/// The keys a `mise.toml` task may carry. Every other key changes what the task executes or where:
/// `dir` moves the working directory (`dir = "support-runtime"` shrinks `cargo deny`'s graph from
/// the workspace's 211 crates to that crate's 102), `depends` runs other tasks first, `shell`
/// swaps the interpreter, `file` replaces `run`, `tools` swaps the binaries on `PATH`. None of
/// them has a CI counterpart to be held identical to, so none is accepted. `env` is accepted
/// because it has one -- a job's or step's `env:` -- and is compared against it.
const MISE_TASK_KEYS: [&str; 3] = ["description", "run", "env"];

/// The top-level tables `mise.toml` may carry. `[env]` and `[task_config]` would reach every task
/// without appearing on any of them, `[vars]` feeds `{{vars.…}}` templates in `run`, and
/// `[settings]` can change the shell tasks run under.
const MISE_TOP_LEVEL_KEYS: [&str; 2] = ["tools", "tasks"];

/// Committed configuration mise would merge into `mise.toml`, or task directories it would read
/// tasks from, beside `mise.toml` itself. Any of these could set a task's environment or
/// directory, or shadow a task, where nothing below reads it. `mise.local.toml` is deliberately
/// absent: it is a contributor's own untracked override, not the repository's gate.
const MISE_SHADOW_CONFIGS: [&str; 9] = [
    ".mise.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
    "mise/config.toml",
    ".mise/config.toml",
    "mise-tasks",
    ".mise-tasks",
    "mise/tasks",
    ".mise/tasks",
];

/// `mise.toml`'s `[tasks]`, once nothing outside a task's own `run` and `env` could change what
/// the task executes.
fn mise_tasks() -> toml::Table {
    let root = workspace_root();
    for shadow in MISE_SHADOW_CONFIGS {
        assert!(
            !root.join(shadow).exists(),
            "`{shadow}` exists beside `mise.toml`; mise merges it into the tasks it runs, so the \
             tasks this suite reads are no longer the tasks `mise run` executes"
        );
    }
    let mut mise: toml::Table = toml::from_str(&read("mise.toml")).expect("mise.toml must parse");
    for key in mise.keys() {
        assert!(
            MISE_TOP_LEVEL_KEYS.contains(&key.as_str()),
            "`mise.toml` carries a top-level `[{key}]`, which reaches every task without appearing \
             on any of them; CI has no counterpart for it to be held identical to"
        );
    }
    let Some(toml::Value::Table(tasks)) = mise.remove("tasks") else {
        panic!("`mise.toml` must carry a `[tasks]` table");
    };
    for (name, task) in &tasks {
        let task = task
            .as_table()
            .unwrap_or_else(|| panic!("`[tasks.{name}]` must be a table"));
        for key in task.keys() {
            assert!(
                MISE_TASK_KEYS.contains(&key.as_str()),
                "`[tasks.{name}]` sets `{key}`, which changes what the task executes or where it \
                 executes it, and has no CI counterpart to be held identical to"
            );
        }
    }
    tasks
}

/// The commands a mise task's `run` executes, in order.
fn mise_commands(tasks: &toml::Table, name: &str) -> Vec<String> {
    match &tasks[name]["run"] {
        toml::Value::String(command) => vec![command.clone()],
        toml::Value::Array(commands) => commands
            .iter()
            .map(|command| {
                command
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("every `[tasks.{name}] run` entry must be a command string")
                    })
                    .to_owned()
            })
            .collect(),
        other => {
            panic!("`[tasks.{name}] run` must be a string or an array of strings, not {other}")
        }
    }
}

/// A mise task's `env`, empty when it states none.
fn mise_env(tasks: &toml::Table, name: &str) -> BTreeMap<String, String> {
    tasks[name].get("env").map_or_else(BTreeMap::new, |env| {
        env.as_table()
            .unwrap_or_else(|| panic!("`[tasks.{name}] env` must be a table"))
            .iter()
            .map(|(key, value)| {
                let value = value.as_str().unwrap_or_else(|| {
                    panic!("`[tasks.{name}] env.{key}` must be a string, as CI's `env:` values are")
                });
                (key.clone(), value.to_owned())
            })
            .collect()
    })
}

fn ci_workflow() -> yaml_rust2::Yaml {
    let ci = read(".github/workflows/ci.yml");
    let mut documents = yaml_rust2::YamlLoader::load_from_str(&ci)
        .expect("`.github/workflows/ci.yml` must parse as YAML");
    assert!(
        !documents.is_empty(),
        "`.github/workflows/ci.yml` must carry a YAML document"
    );
    documents.swap_remove(0)
}

#[test]
fn the_mise_deny_task_audits_the_graph_ci_audits() {
    // `mise run deny` is the supply-chain gate CLAUDE.md hands a contributor, and CI spells its
    // own copy out rather than calling it. The two had drifted: the task ran a bare `cargo deny
    // check`, which resolves default features only and so has no `rustls` in its graph, and
    // reported `advisories ok` on the very lockfile CI failed with RUSTSEC-2026-0285 (#141). The
    // flag later arrived in an unrelated commit with nothing holding it there.
    //
    // The policy is identity: a mise task runs exactly what its CI job runs, neither stricter nor
    // narrower. `every_mise_task_runs_exactly_what_its_ci_job_runs` holds the tasks whose CI job
    // is `run:` steps; CI's deny job is a cargo-deny-action step instead, whose argv the action
    // composes as `cargo-deny --log-level warn --manifest-path ./Cargo.toml <arguments> check
    // <command-arguments>`, so this test holds the task to that composition. The task's global
    // flags must be exactly the words CI passes as `arguments` -- tightening either side (say,
    // `--locked`, #146) reds here until the other follows. `--log-level warn` and
    // `--manifest-path ./Cargo.toml` are cargo-deny's own defaults when run from the workspace
    // root, so the task states neither, and may not: a `--manifest-path`, or a `dir`/`env` on the
    // task (`mise_tasks` rejects the first; the assertion below the second), changes the graph --
    // `dir = "support-runtime"` audits 102 crates rather than the workspace's 211. The task is
    // also held to the rules the CI step is held to: `--all-features`, no graph-narrowing flag,
    // and a bare `check`.
    let workflow = ci_workflow();
    let ci_arguments: Vec<String> = workflow["jobs"]["deny"]["steps"]
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
        ci_arguments
            .iter()
            .any(|argument| argument == "--all-features"),
        "CI's root-manifest cargo-deny-action step passes no `--all-features`, so there is no \
         CI feature scope for `mise run deny` to be held to"
    );

    let tasks = mise_tasks();
    assert!(
        mise_env(&tasks, "deny").is_empty(),
        "`[tasks.deny]` sets an `env`, and CI's cargo-deny-action step sets none; an environment \
         variable such as `CARGO_TARGET_DIR` or `CARGO_NET_OFFLINE` changes what cargo-deny \
         resolves"
    );
    let commands = mise_commands(&tasks, "deny");

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
    assert_eq!(
        audits.len(),
        commands.len(),
        "`mise run deny` runs {commands:?}, which is more than cargo-deny audits; CI's deny job \
         runs nothing else, and the two must run the same thing"
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

        let mut local = globals.to_vec();
        local.sort_unstable();
        let mut remote: Vec<&str> = ci_arguments.iter().map(String::as_str).collect();
        remote.sort_unstable();
        assert_eq!(
            local, remote,
            "`mise run deny` runs `{command}`, whose global flags are not the words CI's \
             cargo-deny-action step passes as `arguments`; the two gates must run the same audit, \
             or one can pass on a lockfile the other fails"
        );
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

/// How a CI job and the mise tasks relate. Every job in `ci.yml` and every task in `mise.toml`
/// is named by exactly one row of [`PAIRINGS`], so a new job or task cannot arrive unclassified.
enum Pairing {
    /// The job's `run:` steps, in order and with their `env:`, are exactly the tasks' `run`
    /// entries in the order listed, with each task's `env`. The job's other steps may only be
    /// [`PROVISIONING_ACTIONS`] or a `run:` step named here, which installs a tool rather than
    /// gating anything.
    Identical {
        job: &'static str,
        tasks: &'static [&'static str],
        provisioning: &'static [&'static str],
    },
    /// Held identical by another test, because the job's gate is an action rather than `run:`.
    HeldBy {
        job: &'static str,
        task: &'static str,
        test: &'static str,
    },
    /// Not identical, and making them so is a maintainer decision this suite does not take.
    /// Listed so the gap is visible, and so the pairing cannot silently disappear.
    Pending {
        job: Option<&'static str>,
        tasks: &'static [&'static str],
        why: &'static str,
    },
    /// A task with no CI counterpart by nature: it rewrites the tree or installs hooks.
    LocalOnly {
        task: &'static str,
        why: &'static str,
    },
}

const PAIRINGS: &[Pairing] = &[
    Pairing::Identical {
        job: "fmt",
        tasks: &["fmt-check"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "clippy",
        tasks: &["lint"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "check",
        tasks: &["check"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "powerset",
        tasks: &["powerset"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "runtime-dependencies",
        tasks: &["runtime-dependencies"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "corpus-smoke",
        tasks: &["corpus-smoke"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "github-api",
        tasks: &["github-api"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "example",
        tasks: &["example"],
        provisioning: &[],
    },
    Pairing::Identical {
        job: "docs",
        tasks: &["docs", "doc-links"],
        provisioning: &["Install mdBook"],
    },
    Pairing::HeldBy {
        job: "deny",
        task: "deny",
        test: "the_mise_deny_task_audits_the_graph_ci_audits",
    },
    Pairing::Pending {
        job: Some("test"),
        tasks: &["test"],
        why: "CI also runs `cargo bench --no-run --workspace`; `mise run test` is the pre-push \
              hook, so adding it there slows every push",
    },
    Pairing::Pending {
        job: Some("commits"),
        tasks: &["commit-range"],
        why: "CI checks `base.sha..head.sha` of the pull request, the task `origin/master..HEAD`",
    },
    Pairing::Pending {
        job: Some("msrv"),
        tasks: &[],
        why: "no mise task; it needs the 1.88.0 toolchain installed",
    },
    Pairing::Pending {
        job: Some("package"),
        tasks: &[],
        why: "no mise task; one step is conditional on the pull request not being a release PR",
    },
    Pairing::Pending {
        job: None,
        tasks: &["bench"],
        why: "the counterpart is `benchmarks.yml`, which passes shortened criterion timings",
    },
    Pairing::LocalOnly {
        task: "fmt",
        why: "rewrites the tree; `fmt-check` is the gate CI mirrors",
    },
    Pairing::LocalOnly {
        task: "lint-fix",
        why: "rewrites the tree; `lint` is the gate CI mirrors",
    },
    Pairing::LocalOnly {
        task: "commit-msg",
        why: "validates one message as it is written; CI checks the range",
    },
    Pairing::LocalOnly {
        task: "hooks",
        why: "installs the git hooks",
    },
];

/// Actions a paired job may use besides its gate steps. Each provisions a checkout, a toolchain,
/// a cache, or a binary; none runs a gate. An action outside this list may be a gate of its own
/// (cargo-deny-action is), which a `run:`-step comparison would never see.
const PROVISIONING_ACTIONS: [&str; 4] = [
    "actions/checkout@",
    "dtolnay/rust-toolchain@",
    "swatinem/rust-cache@",
    "taiki-e/install-action@",
];

/// Workflow-level `env:` entries that reach every job but change only how cargo prints, not what
/// it resolves or checks.
const COSMETIC_WORKFLOW_ENV: [&str; 1] = ["CARGO_TERM_COLOR"];

/// A YAML `env:` mapping as strings.
fn yaml_env(env: &yaml_rust2::Yaml, whose: &str) -> BTreeMap<String, String> {
    if env.is_badvalue() {
        return BTreeMap::new();
    }
    env.as_hash()
        .unwrap_or_else(|| panic!("{whose} `env:` must be a mapping"))
        .iter()
        .map(|(key, value)| {
            let key = key
                .as_str()
                .unwrap_or_else(|| panic!("{whose} `env:` keys must be strings"));
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                panic!("{whose} `env.{key}` must be a string, as a mise task's `env` values are")
            });
            (key.to_owned(), value)
        })
        .collect()
}

#[test]
fn every_mise_task_runs_exactly_what_its_ci_job_runs() {
    // CI spells each gate out rather than calling `mise run`, so every gate exists twice. The
    // policy is that the two copies are identical -- the same commands with the same flags, in
    // the same order, under the same environment -- and neither is stricter or narrower than the
    // other. #141 was the deny gate auditing a smaller graph locally than in CI; the same audit
    // found `corpus-smoke` letting a crashed `check` through to its `grep` locally, and
    // `example`'s leak check discarding a failing `cargo tree` locally, where CI failed on both.
    //
    // Commands are compared as strings after trimming, so an identical gate means byte-identical
    // commands. mise runs each `run` entry under `sh -c -o errexit` and CI each step under
    // `bash -e`, so a command that is byte-identical also fails the same way in both.
    let workflow = ci_workflow();
    let tasks = mise_tasks();

    let workflow_env = yaml_env(&workflow["env"], "the workflow's");
    for key in workflow_env.keys() {
        assert!(
            COSMETIC_WORKFLOW_ENV.contains(&key.as_str()),
            "`ci.yml` sets `{key}` for every job, which no mise task sets; either set it on each \
             task too or, if it only changes how output looks, list it in COSMETIC_WORKFLOW_ENV"
        );
    }
    assert!(
        workflow["defaults"].is_badvalue(),
        "`ci.yml` sets workflow `defaults:`, which can change the shell or directory of every \
         `run:` step without appearing on any of them"
    );

    let jobs = workflow["jobs"]
        .as_hash()
        .expect("`ci.yml` must carry a `jobs:` mapping");
    let ci_jobs: BTreeSet<&str> = jobs
        .keys()
        .map(|job| job.as_str().expect("job ids are strings"))
        .collect();
    let mut paired_jobs = BTreeSet::new();
    let mut paired_tasks = BTreeSet::new();
    let mut claim = |jobs: &[&'static str], tasks: &[&'static str]| {
        for job in jobs {
            assert!(paired_jobs.insert(*job), "job `{job}` is paired twice");
        }
        for task in tasks {
            assert!(paired_tasks.insert(*task), "task `{task}` is paired twice");
        }
    };
    for pairing in PAIRINGS {
        match pairing {
            Pairing::Identical { job, tasks, .. } => claim(&[*job], tasks),
            Pairing::HeldBy { job, task, test } => {
                assert!(
                    read("spargen/tests/corpus_manifest.rs").contains(&format!("fn {test}()")),
                    "`{job}` is held by `{test}`, which does not exist"
                );
                claim(&[*job], &[*task]);
            }
            Pairing::Pending { job, tasks, why } => {
                assert!(!why.is_empty(), "a pending pairing must say why");
                claim(job.as_slice(), tasks);
            }
            Pairing::LocalOnly { task, why } => {
                assert!(!why.is_empty(), "a local-only task must say why");
                claim(&[], &[*task]);
            }
        }
    }
    let mise_tasks: BTreeSet<&str> = tasks.keys().map(String::as_str).collect();
    assert_eq!(
        ci_jobs, paired_jobs,
        "every CI job must be named by one PAIRINGS row, and every row's job must exist"
    );
    assert_eq!(
        mise_tasks, paired_tasks,
        "every mise task must be named by one PAIRINGS row, and every row's task must exist"
    );

    for pairing in PAIRINGS {
        let Pairing::Identical {
            job: name,
            tasks: task_names,
            provisioning,
        } = pairing
        else {
            continue;
        };
        let job = &workflow["jobs"][*name];
        for key in [
            "if",
            "continue-on-error",
            "defaults",
            "strategy",
            "container",
        ] {
            assert!(
                job[key].is_badvalue(),
                "the `{name}` job sets `{key}:`, which changes whether or how its steps run; \
                 its mise counterpart has nothing to match it with"
            );
        }
        let job_env = yaml_env(&job["env"], &format!("the `{name}` job's"));

        let mut ci = Vec::new();
        let mut provisioned = BTreeSet::new();
        for step in job["steps"]
            .as_vec()
            .unwrap_or_else(|| panic!("the `{name}` job must carry a list of steps"))
        {
            if let Some(uses) = step["uses"].as_str() {
                let uses = uses.to_ascii_lowercase();
                assert!(
                    PROVISIONING_ACTIONS
                        .iter()
                        .any(|action| uses.starts_with(action)),
                    "the `{name}` job uses `{uses}`, which is not a provisioning action; if it \
                     gates anything, the mise task never runs it"
                );
                continue;
            }
            let run = step["run"]
                .as_str()
                .unwrap_or_else(|| panic!("a `{name}` step has neither `uses:` nor `run:`"));
            if let Some(step_name) = step["name"]
                .as_str()
                .filter(|step_name| provisioning.contains(step_name))
            {
                provisioned.insert(step_name);
                continue;
            }
            let step_map = step
                .as_hash()
                .unwrap_or_else(|| panic!("a `{name}` step must be a mapping"));
            for key in step_map.keys() {
                let key = key.as_str().unwrap_or_default();
                assert!(
                    ["name", "run", "env"].contains(&key),
                    "the `{name}` job's `{run}` step sets `{key}:`, which changes whether, where, \
                     or how it runs; its mise counterpart has nothing to match it with"
                );
            }
            let mut env = job_env.clone();
            env.extend(yaml_env(&step["env"], &format!("a `{name}` step's")));
            ci.push((env, run.trim().to_owned()));
        }
        for step_name in *provisioning {
            assert!(
                provisioned.contains(step_name),
                "the `{name}` job has no `{step_name}` step to skip as provisioning"
            );
        }

        let local: Vec<(BTreeMap<String, String>, String)> = task_names
            .iter()
            .flat_map(|task| {
                let env = mise_env(&tasks, task);
                mise_commands(&tasks, task)
                    .into_iter()
                    .map(move |command| (env.clone(), command.trim().to_owned()))
            })
            .collect();

        assert_eq!(
            local,
            ci,
            "`mise run {}` does not run exactly what CI's `{name}` job runs (left: mise, right: \
             CI; each entry is its environment and its command). The two must be identical -- \
             change whichever side is wrong, not only one of them",
            task_names.join("` + `mise run ")
        );
    }
}

#[test]
fn the_quality_list_quotes_its_tasks_verbatim() {
    // CLAUDE.md's Quality block glosses some tasks with the command they run. A gloss that is a
    // command is a claim about the task, so it must be the task's command exactly.
    let tasks = mise_tasks();
    let claude = read("CLAUDE.md");
    let mut quoted = 0usize;
    for line in claude.lines() {
        let Some(rest) = line.strip_prefix("mise run ") else {
            continue;
        };
        let Some((task, gloss)) = rest.split_once('#') else {
            continue;
        };
        let (task, gloss) = (task.trim(), gloss.trim());
        // `cargo hack: every feature combination…` is prose about a command, not a command.
        if !gloss.starts_with("cargo ") || gloss.contains(':') {
            continue;
        }
        assert!(
            tasks.contains_key(task),
            "CLAUDE.md lists `mise run {task}`, which mise.toml does not define"
        );
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
