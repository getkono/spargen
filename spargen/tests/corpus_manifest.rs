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
/// over the result. Measured on this tree with cargo-deny 0.19.9 (then the `mise.toml` pin), as
/// `cargo-deny --log-level warn --manifest-path ./Cargo.toml --all-features <flag> check
/// advisories`: `--exclude rustls` and `--target wasm32-unknown-unknown` each turn
/// `advisories FAILED` (RUSTSEC-2026-0285, reached only through reqwest's TLS feature) into
/// `advisories ok`, exit 1 to exit 0. The other five do *not* flip that verdict here and are
/// rejected as the same class of flag rather than on a measured flip -- `--offline` was
/// measured against an already-populated advisory database, and `--no-default-features` is
/// overridden by the `--all-features` this same value is required to carry. Not exhaustive:
/// see `the_deny_gate_states_the_feature_scope_it_audits`, and #238. `-t` is clap's short alias
/// for `--target` (the only one of these flags whose short form cargo-deny's `--help` lists, in
/// 0.19.9 and in 0.20.2 alike), and is the same flag: `--all-features -t wasm32-unknown-unknown`
/// drops rustls from `cargo deny list` exactly as the long spelling does.
///
/// Applied to `mise run deny`'s commands, which CI's `deny` job runs byte for byte.
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

/// Directories mise reads configuration or tasks from beside `mise.toml`. Each is mise's own, so
/// each is rejected whole: `.config/mise/`, `mise/` and `.mise/` hold `config.toml`,
/// `config.<env>.toml`, `conf.d/*.toml` (whose `[env]` reaches every task) and a `tasks/`
/// directory of file tasks (a file task shadows a same-named `mise.toml` task), and `mise-tasks/`
/// and `.mise-tasks/` are file-task directories. Measured against mise 2026.8.14 by planting each
/// candidate in a scratch project and reading `mise config ls`, `mise tasks ls` and `mise env`.
const MISE_SHADOW_DIRS: [&str; 5] = [".config/mise", "mise", ".mise", "mise-tasks", ".mise-tasks"];

/// Single files mise reads beside `mise.toml`, measured the same way: `.rtx.toml` is the legacy
/// config name, `.tool-versions` swaps tool versions, and `.miserc.toml` can set `MISE_ENV`, which
/// loads `mise.<env>.toml` into every task.
const MISE_SHADOW_FILES: [&str; 3] = [".rtx.toml", ".tool-versions", ".miserc.toml"];

/// Whether `name`, a file in the workspace root (`in_config` false) or in `.config/` (true), is a
/// mise config file other than `mise.toml` itself: `[.]mise[.<env>].toml` at the root,
/// `mise[.<env>].toml` in `.config/`. A `.local` variant (`mise.local.toml`,
/// `mise.<env>.local.toml`) is a contributor's own uncommitted override rather than the
/// repository's gate, so it is let through.
fn is_shadow_mise_config(name: &str, in_config: bool) -> bool {
    let bare = if in_config {
        name
    } else {
        name.strip_prefix('.').unwrap_or(name)
    };
    let Some(profile) = bare
        .strip_prefix("mise")
        .and_then(|rest| rest.strip_suffix(".toml"))
    else {
        return false;
    };
    if !(profile.is_empty() || profile.starts_with('.')) || profile.ends_with(".local") {
        return false;
    }
    in_config || name != "mise.toml"
}

/// `mise.toml`'s `[tasks]`, once nothing outside a task's own `run` and `env` could change what
/// the task executes.
fn mise_tasks() -> toml::Table {
    let root = workspace_root();
    for shadow in MISE_SHADOW_DIRS.iter().chain(&MISE_SHADOW_FILES) {
        assert!(
            !root.join(shadow).exists(),
            "`{shadow}` exists beside `mise.toml`; mise merges it into the tasks it runs, so the \
             tasks this suite reads are no longer the tasks `mise run` executes"
        );
    }
    for (dir, in_config) in [(root.clone(), false), (root.join(".config"), true)] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let name = entry.expect("a directory entry").file_name();
            let name = name.to_string_lossy();
            assert!(
                !is_shadow_mise_config(&name, in_config),
                "`{dir}/{name}` is a mise config file beside `mise.toml`; mise merges it into the \
                 tasks it runs (a `mise.<env>.toml` once `MISE_ENV` names it), so the tasks this \
                 suite reads are no longer the tasks `mise run` executes"
            );
        }
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

/// The first YAML document of `text`, which `whose` names in a failure.
fn yaml_document(text: &str, whose: &str) -> yaml_rust2::Yaml {
    let mut documents = yaml_rust2::YamlLoader::load_from_str(text)
        .unwrap_or_else(|error| panic!("{whose} must parse as YAML: {error}"));
    assert!(!documents.is_empty(), "{whose} must carry a YAML document");
    documents.swap_remove(0)
}

/// `.github/workflows/<file>`, parsed.
fn workflow(file: &str) -> yaml_rust2::Yaml {
    let path = format!(".github/workflows/{file}");
    yaml_document(&read(&path), &format!("`{path}`"))
}

fn ci_workflow() -> yaml_rust2::Yaml {
    workflow("ci.yml")
}

#[test]
fn the_deny_gate_states_the_feature_scope_it_audits() {
    // `--all-features` is what puts a TLS stack in the audited graph: under default features
    // `rustls` is absent from the workspace entirely, so an advisory gate run without the flag
    // passes because it can see nothing (#147). `mise run deny` ran a bare `cargo deny check`
    // and reported `advisories ok` on the very lockfile CI failed with RUSTSEC-2026-0285 (#141).
    //
    // CI's `deny` job runs `mise run deny`'s commands byte for byte, on the same cargo-deny
    // (`every_mise_task_runs_exactly_what_its_ci_job_runs`,
    // `ci_installs_exactly_the_tool_versions_mise_pins`), so holding the task's commands here
    // holds both gates. What identity cannot catch is both sides narrowing together, which is
    // what this test is for.
    //
    // Every command must be a cargo-deny audit, and the task must set no `env` (`CARGO_TARGET_DIR`
    // or `CARGO_NET_OFFLINE` change what cargo-deny resolves; `mise_tasks` already rejects `dir`).
    // The narrowing rules apply to each command that audits the *root* manifest -- no
    // `--manifest-path`, or one naming the root `Cargo.toml` -- and at least one must: applying
    // them to every command would turn "no command may narrow the gate" into "every command must
    // be maximal", which reds #184's cheapest shape (a second audit per example workspace with
    // `check advisories`). A root audit must pass `--all-features`, no graph-narrowing flag, and
    // a bare `check`, since `check licenses` drops `advisories`. The deny-list is a **list, not a
    // proof** (#238): it rejects the graph-narrowing flags measured to hide this tree's live
    // advisory, and cannot establish that some other flag does not narrow.
    let tasks = mise_tasks();
    assert!(
        mise_env(&tasks, "deny").is_empty(),
        "`[tasks.deny]` sets an `env`; an environment variable such as `CARGO_TARGET_DIR` or \
         `CARGO_NET_OFFLINE` changes what cargo-deny resolves"
    );
    let commands = mise_commands(&tasks, "deny");
    assert!(
        !commands.is_empty(),
        "`mise run deny` runs nothing, so neither gate audits the dependency graph"
    );

    let mut root_audits = 0usize;
    for command in &commands {
        let words: Vec<&str> = command.split_whitespace().collect();
        let skip = if words.starts_with(&["cargo", "deny"]) {
            2
        } else if words.first() == Some(&"cargo-deny") {
            1
        } else {
            panic!(
                "`mise run deny` runs `{command}`, which is not a cargo-deny audit; the gate runs \
                 nothing else"
            );
        };
        let check = words
            .iter()
            .position(|word| *word == "check")
            .unwrap_or_else(|| panic!("`mise run deny` runs `{command}`, which is not `check`"));
        let globals = &words[skip..check];
        let which = &words[check + 1..];

        let mut manifest = None;
        let mut rest = globals.iter();
        while let Some(word) = rest.next() {
            if *word == "--manifest-path" {
                manifest = rest.next().copied();
            } else if let Some(path) = word.strip_prefix("--manifest-path=") {
                manifest = Some(path);
            }
        }
        if manifest.is_some_and(|path| path.trim_start_matches("./") != "Cargo.toml") {
            continue;
        }
        root_audits += 1;

        assert!(
            globals.contains(&"--all-features"),
            "`mise run deny` runs `{command}`, which does not pass `--all-features`, so the \
             audited graph has no TLS stack in it"
        );
        for word in globals {
            let flag = flag_of(word);
            assert!(
                !GRAPH_NARROWING_FLAGS.contains(&flag),
                "`mise run deny` runs `{command}`, whose `{flag}` shrinks the graph cargo-deny \
                 resolves rather than the checks it runs over it; `--all-features {flag} …` \
                 still contains `--all-features` and still drops RUSTSEC-2026-0285. Strictly \
                 stricter values such as `--all-features --locked` are deliberately still accepted"
            );
        }
        assert!(
            which.is_empty(),
            "`mise run deny` runs `{command}`, which narrows `check` to {which:?}; anything \
             narrower than a bare `check` can drop `advisories`, which is the check #147 is about"
        );
    }
    assert!(
        root_audits > 0,
        "no `mise run deny` command audits the root `Cargo.toml`: every one names a \
         `--manifest-path` elsewhere, so the workspace this gate exists to audit is audited by \
         nothing"
    );
}

/// How a CI job and the mise tasks relate. Every job in the [`GATE_WORKFLOWS`] and every task in
/// `mise.toml` is named by exactly one row of [`PAIRINGS`], so a new job or task cannot arrive
/// unclassified. There is no "not yet identical" row: every job is held to its tasks, and every
/// difference between the two is a named, literal exception inside a row.
enum Pairing {
    /// The job runs exactly what the tasks run; see [`Pair`].
    Identical(Pair),
    /// A task with no CI counterpart by nature: it rewrites the tree or installs hooks.
    LocalOnly {
        task: &'static str,
        why: &'static str,
    },
}

/// A job held to its tasks. The job's `run:` gate steps, in order and with their `env:`, are
/// exactly the tasks' `run` entries in the order listed, with each task's `env`, once each
/// [`Rewrite`] is applied to CI's side. Every other step is one of `ci_only`, in order, byte for
/// byte as parsed YAML.
struct Pair {
    /// The file under `.github/workflows/`.
    workflow: &'static str,
    job: &'static str,
    tasks: &'static [&'static str],
    ci_only: &'static [CiOnly],
    /// The job's `if:`, pinned literally, where the job has one.
    job_if: Option<&'static str>,
    rewrites: &'static [Rewrite],
}

const PAIR: Pair = Pair {
    workflow: "ci.yml",
    job: "",
    tasks: &[],
    ci_only: &[],
    job_if: None,
    rewrites: &[],
};

/// A step a job runs that its tasks do not, as the literal YAML of the step. Pinned whole, so
/// appending `&& cargo test` to a `run:`, adding `continue-on-error:` or `if:`, or moving a
/// toolchain's `@ref` all fail.
struct CiOnly {
    step: &'static str,
    /// Empty for provisioning: a `uses:` of one of [`PROVISIONING_ACTIONS`], with at most a
    /// `with:`. Otherwise this step is a named exception, and this says why mise has no
    /// counterpart for it.
    why: &'static str,
}

const fn provision(step: &'static str) -> CiOnly {
    CiOnly { step, why: "" }
}

const CHECKOUT: CiOnly = provision("uses: actions/checkout@v4");
const CHECKOUT_LFS: CiOnly = provision("uses: actions/checkout@v4\nwith:\n  lfs: true");
// These name `rust-toolchain.toml`'s `channel`, a concrete release, so a toolchain bump changes
// them together with the workflows (`ci_installs_the_rust_toolchain_this_file_pins`).
const STABLE: CiOnly = provision("uses: dtolnay/rust-toolchain@1.98.1");
const STABLE_CLIPPY: CiOnly =
    provision("uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: clippy");
const STABLE_CLIPPY_WASM: CiOnly = provision(
    "uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: clippy\n  targets: wasm32-unknown-unknown",
);
const CACHE: CiOnly = provision("uses: Swatinem/rust-cache@v2");

/// A named difference between CI's command and the task's: `ci` is replaced by `mise` in CI's
/// commands before they are compared, and `ci` must occur exactly once in them. Everything
/// outside `ci` is still compared byte for byte, and `ci` itself is pinned literally.
struct Rewrite {
    ci: &'static str,
    mise: &'static str,
    why: &'static str,
}

/// A workflow whose every job must be named by a [`PAIRINGS`] row, with the top-level keys that
/// decide when and whether its jobs run pinned literally (compared as parsed YAML). A `paths:`
/// filter under `pull_request:`, a narrower `branches:`, or a different `cancel-in-progress`
/// would make CI gate less than the tasks do without touching a single job.
struct GateWorkflow {
    file: &'static str,
    on: &'static str,
    /// `None` where the workflow has no `concurrency:`.
    concurrency: Option<&'static str>,
    /// `None` where the workflow has no `permissions:`.
    permissions: Option<&'static str>,
}

/// Every workflow under `.github/workflows/` (`.yml` or `.yaml`) is one of these or one of
/// [`NON_GATE_WORKFLOWS`]; an unclassified file fails, so a new workflow cannot run a gate no
/// pairing sees.
const GATE_WORKFLOWS: [GateWorkflow; 2] = [
    GateWorkflow {
        file: "ci.yml",
        on: "push:\n  branches: [master]\npull_request:",
        concurrency: Some("group: ci-${{ github.ref }}\ncancel-in-progress: true"),
        permissions: None,
    },
    GateWorkflow {
        file: "benchmarks.yml",
        on: "push:\n  tags: [\"v*\"]\nworkflow_dispatch:",
        concurrency: None,
        permissions: None,
    },
];

/// Workflows that gate nothing, each with why. None of their jobs is paired, so a gate added to
/// one would run unseen: keep this list to workflows that are not gates by nature.
const NON_GATE_WORKFLOWS: [(&str, &str); 1] = [(
    "release-plz.yml",
    "publishes releases and maintains the release PR; it gates nothing a contributor could run \
     first",
)];

/// The top-level keys a gate workflow may carry. `name` is cosmetic, `env` is held to
/// [`COSMETIC_WORKFLOW_ENV`], and `on`, `concurrency` and `permissions` are pinned by the
/// workflow's [`GateWorkflow`] row. Anything else (`defaults:` can change the shell or directory
/// of every `run:` step without appearing on any of them) is rejected.
const WORKFLOW_KEYS: [&str; 6] = ["name", "on", "env", "concurrency", "permissions", "jobs"];

/// The file names under `.github/workflows/` that GitHub runs.
fn workflow_files() -> BTreeSet<String> {
    let dir = workspace_root().join(".github/workflows");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot list `{dir}`: {error}"))
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
        .collect()
}

const PAIRINGS: &[Pairing] = &[
    Pairing::Identical(Pair {
        job: "fmt",
        tasks: &["fmt-check"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: rustfmt"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "clippy",
        tasks: &["lint"],
        ci_only: &[CHECKOUT, STABLE_CLIPPY, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "test",
        tasks: &["test", "bench-build"],
        ci_only: &[CHECKOUT_LFS, STABLE_CLIPPY, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "check",
        tasks: &["check"],
        ci_only: &[CHECKOUT, STABLE, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "msrv",
        tasks: &["msrv"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@1.88.0"),
            CACHE,
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "powerset",
        tasks: &["powerset"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-hack@0.6.39"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "runtime-dependencies",
        tasks: &["runtime-dependencies"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@nightly"),
            STABLE_CLIPPY_WASM,
            CACHE,
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "package",
        tasks: &["package"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            CiOnly {
                step: "if: >-\n  (github.event_name == 'pull_request' && !startsWith(github.head_ref, 'release-plz-')) ||\n  (github.event_name == 'push' && !contains(github.event.head_commit.message, 'release-plz-'))\nrun: cargo publish --dry-run -p spargen-macro --config 'patch.crates-io.spargen.path=\"spargen\"'",
                why: "runs only outside release-plz PRs, a condition that exists only in CI; on a \
                      release PR the macro's registry dependency is not published yet",
            },
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "corpus-smoke",
        tasks: &["corpus-smoke"],
        ci_only: &[CHECKOUT_LFS, STABLE, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "github-api",
        tasks: &["github-api"],
        ci_only: &[CHECKOUT_LFS, STABLE_CLIPPY_WASM, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "example",
        tasks: &["example"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: Swatinem/rust-cache@v2\nwith:\n  workspaces: |\n    examples/petstore\n    examples/petstore-macro",
            ),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "docs",
        tasks: &["docs", "doc-links"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            CiOnly {
                step: "name: Install mdBook\nrun: cargo install mdbook --version 0.5.4 --locked",
                why: "installs the mdBook that `[tools]` in mise.toml provisions locally",
            },
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "deny",
        tasks: &["deny"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-deny@0.20.2"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "commits",
        tasks: &["commit-range"],
        ci_only: &[
            provision("uses: actions/checkout@v4\nwith:\n  fetch-depth: 0"),
            CiOnly {
                step: "name: Install convco\nrun: cargo install convco --version 0.7.1 --locked",
                why: "installs the convco that `[tools]` in mise.toml provisions locally",
            },
        ],
        job_if: Some("github.event_name == 'pull_request'"),
        rewrites: &[Rewrite {
            ci: "${{ github.event.pull_request.base.sha }}..${{ github.event.pull_request.head.sha }}",
            mise: "origin/master..HEAD",
            why: "the outgoing range comes from the pull request event in CI and from the \
                  remote-tracking branch locally; the job runs only on pull requests for the \
                  same reason",
        }],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "benchmarks.yml",
        job: "bench",
        tasks: &["bench"],
        ci_only: &[
            CHECKOUT_LFS,
            STABLE,
            CACHE,
            CiOnly {
                step: "name: Upload benchmark results\nuses: actions/upload-artifact@v4\nwith:\n  name: benchmarks-${{ github.ref_name }}\n  path: |\n    bench-results.txt\n    target/criterion/**\n  if-no-files-found: error",
                why: "publishes the recorded results as the release artifact",
            },
        ],
        rewrites: &[
            Rewrite {
                ci: "set -o pipefail\n",
                mise: "",
                why: "a step's default `bash -e` has no `pipefail`, so without it a failing \
                      `cargo bench` would exit through `tee` with status 0; mise's command has \
                      no pipe to need it",
            },
            Rewrite {
                ci: " | tee bench-results.txt",
                mise: "",
                why: "captures the output for the artifact without changing what runs",
            },
        ],
        ..PAIR
    }),
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

/// Actions a [`provision`] step may use. Each provisions a checkout, a toolchain, a cache, or a
/// binary; none runs a gate. An action outside this list may be a gate of its own (the
/// cargo-deny-action the `deny` job once ran was), which a `run:`-step comparison would never see.
const PROVISIONING_ACTIONS: [&str; 4] = [
    "actions/checkout@",
    "dtolnay/rust-toolchain@",
    "swatinem/rust-cache@",
    "taiki-e/install-action@",
];

/// Workflow-level `env:` entries that reach every job but change only how cargo prints, not what
/// it resolves or checks.
const COSMETIC_WORKFLOW_ENV: [&str; 1] = ["CARGO_TERM_COLOR"];

/// The keys a paired job may carry besides an `if:` its row pins. `runs-on` is held to
/// `ubuntu-latest` below. `timeout-minutes` is allowed unpinned: it bounds the runner's wall clock
/// and can only turn a run red, never green, so it cannot make CI narrower than the task. Every
/// other key (`needs`, `strategy`, `container`, `services`, `defaults`, `continue-on-error`, ...)
/// changes whether or how the steps run, and mise has nothing to match it with.
const JOB_KEYS: [&str; 5] = ["name", "runs-on", "steps", "env", "timeout-minutes"];

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

/// The keys of a YAML mapping, as strings.
fn yaml_keys<'a>(map: &'a yaml_rust2::Yaml, whose: &str) -> Vec<&'a str> {
    map.as_hash()
        .unwrap_or_else(|| panic!("{whose} must be a mapping"))
        .keys()
        .map(|key| key.as_str().unwrap_or_default())
        .collect()
}

/// Holds a workflow's top level to what no task can see, and returns its `jobs:`.
fn gate_workflow(gate: &GateWorkflow) -> yaml_rust2::Yaml {
    let file = gate.file;
    let workflow = workflow(file);
    for key in yaml_keys(&workflow, &format!("`{file}`")) {
        assert!(
            WORKFLOW_KEYS.contains(&key),
            "`{file}` sets top-level `{key}:`, which can change whether or how every job runs; \
             no mise task has anything to match it with"
        );
    }
    for (key, pinned) in [
        ("on", Some(gate.on)),
        ("concurrency", gate.concurrency),
        ("permissions", gate.permissions),
    ] {
        let expected = pinned.map_or(yaml_rust2::Yaml::BadValue, |text| {
            yaml_document(text, &format!("`{file}`'s pinned `{key}:`"))
        });
        assert_eq!(
            workflow[key], expected,
            "`{file}`'s `{key}:` is not the one its GATE_WORKFLOWS row pins; a trigger filter or a \
             cancellation rule can skip a gate for a change the mise task would check"
        );
    }
    for key in yaml_env(&workflow["env"], &format!("`{file}`'s")).keys() {
        assert!(
            COSMETIC_WORKFLOW_ENV.contains(&key.as_str()),
            "`{file}` sets `{key}` for every job, which no mise task sets; either set it on each \
             task too or, if it only changes how output looks, list it in COSMETIC_WORKFLOW_ENV"
        );
    }
    workflow["jobs"].clone()
}

/// Checks `job`'s own keys and walks its steps: each is the next of `ci_only` (literally) or a
/// gate `run:` step. Returns the gate steps as (environment, trimmed command).
fn gate_steps(
    file: &str,
    name: &str,
    job: &yaml_rust2::Yaml,
    job_if: Option<&str>,
    ci_only: &[CiOnly],
) -> Vec<(BTreeMap<String, String>, String)> {
    assert!(!job.is_badvalue(), "`{file}` has no `{name}` job");
    for key in yaml_keys(job, &format!("the `{name}` job")) {
        assert!(
            JOB_KEYS.contains(&key) || (key == "if" && job_if.is_some()),
            "the `{name}` job in `{file}` sets `{key}:`, which changes whether or how its steps \
             run; its mise counterpart has nothing to match it with"
        );
    }
    assert_eq!(
        job["if"].as_str(),
        job_if,
        "the `{name}` job's `if:` is not the one its PAIRINGS row pins"
    );
    assert_eq!(
        job["runs-on"].as_str(),
        Some("ubuntu-latest"),
        "the `{name}` job must run on `ubuntu-latest`, the platform its tasks are held on"
    );
    let job_env = yaml_env(&job["env"], &format!("the `{name}` job's"));

    let mut expected = ci_only
        .iter()
        .map(|pinned| {
            let step = yaml_document(pinned.step, &format!("a `{name}` CI-only step"));
            if pinned.why.is_empty() {
                let keys = yaml_keys(&step, &format!("a `{name}` provisioning step"));
                assert!(
                    keys.iter().all(|key| ["uses", "with"].contains(key)),
                    "the `{name}` provisioning step `{}` carries more than `uses:` and `with:`; \
                     give it a `why` as a named exception",
                    pinned.step
                );
                let uses = step["uses"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                assert!(
                    PROVISIONING_ACTIONS
                        .iter()
                        .any(|action| uses.starts_with(action)),
                    "the `{name}` provisioning step uses `{uses}`, which is not a provisioning \
                     action; if it gates anything, the mise task never runs it"
                );
            }
            (pinned.step, step)
        })
        .peekable();

    let mut gates = Vec::new();
    for step in job["steps"]
        .as_vec()
        .unwrap_or_else(|| panic!("the `{name}` job must carry a list of steps"))
    {
        if expected.peek().is_some_and(|(_, pinned)| pinned == step) {
            expected.next();
            continue;
        }
        let keys = yaml_keys(step, &format!("a `{name}` step"));
        if let Some(uses) = step["uses"].as_str() {
            panic!(
                "the `{name}` job uses `{uses}` in a step no CI-only row pins literally; pin it in \
                 the job's PAIRINGS row (its `@ref` and `with:` included)"
            );
        }
        let run = step["run"]
            .as_str()
            .unwrap_or_else(|| panic!("a `{name}` step has neither `uses:` nor `run:`"));
        for key in keys {
            assert!(
                ["name", "run", "env"].contains(&key),
                "the `{name}` job's `{run}` step sets `{key}:`, which changes whether, where, or \
                 how it runs; its mise counterpart has nothing to match it with"
            );
        }
        let mut env = job_env.clone();
        env.extend(yaml_env(&step["env"], &format!("a `{name}` step's")));
        gates.push((env, run.trim().to_owned()));
    }
    if let Some((pinned, _)) = expected.next() {
        panic!(
            "the `{name}` job has no step `{pinned}` where its PAIRINGS row pins one (the pinned \
             steps must appear in order, byte for byte)"
        );
    }
    gates
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
    // `bash -e`, so a command that is byte-identical also fails the same way in both. Every step
    // that is not compared -- checkouts, toolchains, caches, tool installs, and the few named
    // exceptions -- is pinned as literal YAML in its row, so none of them can grow a gate, a
    // condition, or a different toolchain unseen.
    let tasks = mise_tasks();
    let mut paired_jobs = BTreeSet::new();
    let mut paired_tasks = BTreeSet::new();
    let mut claim = |jobs: &[(&'static str, &'static str)], tasks: &[&'static str]| {
        for job in jobs {
            assert!(paired_jobs.insert(*job), "job {job:?} is paired twice");
        }
        for task in tasks {
            assert!(paired_tasks.insert(*task), "task `{task}` is paired twice");
        }
    };
    for pairing in PAIRINGS {
        match pairing {
            Pairing::Identical(pair) => claim(&[(pair.workflow, pair.job)], pair.tasks),
            Pairing::LocalOnly { task, why } => {
                assert!(!why.is_empty(), "a local-only task must say why");
                claim(&[], &[*task]);
            }
        }
    }
    let classified: BTreeSet<String> = GATE_WORKFLOWS
        .iter()
        .map(|gate| gate.file)
        .chain(NON_GATE_WORKFLOWS.iter().map(|(file, why)| {
            assert!(!why.is_empty(), "a non-gate workflow must say why");
            *file
        }))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        workflow_files(),
        classified,
        "every workflow under `.github/workflows/` must be a GATE_WORKFLOWS row (its jobs paired \
         with mise tasks) or a NON_GATE_WORKFLOWS entry saying why it gates nothing, and every \
         listed file must exist"
    );
    let mut workflow_jobs = BTreeMap::new();
    let mut ci_jobs = BTreeSet::new();
    for gate in &GATE_WORKFLOWS {
        let file = gate.file;
        let jobs = gate_workflow(gate);
        for job in jobs
            .as_hash()
            .unwrap_or_else(|| panic!("`{file}` must carry a `jobs:` mapping"))
            .keys()
        {
            ci_jobs.insert((file, job.as_str().expect("job ids are strings").to_owned()));
        }
        workflow_jobs.insert(file, jobs);
    }
    let mise_tasks: BTreeSet<&str> = tasks.keys().map(String::as_str).collect();
    let paired_jobs: BTreeSet<(&str, String)> = paired_jobs
        .into_iter()
        .map(|(file, job)| (file, job.to_owned()))
        .collect();
    assert_eq!(
        ci_jobs, paired_jobs,
        "every job in a GATE_WORKFLOWS workflow must be named by one PAIRINGS row, and every row's job \
         must exist"
    );
    assert_eq!(
        mise_tasks, paired_tasks,
        "every mise task must be named by one PAIRINGS row, and every row's task must exist"
    );

    for pairing in PAIRINGS {
        let pair = match pairing {
            Pairing::Identical(pair) => pair,
            Pairing::LocalOnly { .. } => continue,
        };
        let name = pair.job;
        let mut ci = gate_steps(
            pair.workflow,
            name,
            &workflow_jobs[pair.workflow][name],
            pair.job_if,
            pair.ci_only,
        );
        for rewrite in pair.rewrites {
            assert!(!rewrite.why.is_empty(), "a rewrite must say why");
            let found: usize = ci
                .iter()
                .map(|(_, command)| command.matches(rewrite.ci).count())
                .sum();
            assert_eq!(
                found, 1,
                "the `{name}` job's commands carry `{}` {found} times; its PAIRINGS row rewrites \
                 it exactly once, so CI's side of that exception has changed",
                rewrite.ci
            );
            for (_, command) in &mut ci {
                *command = command.replace(rewrite.ci, rewrite.mise).trim().to_owned();
            }
        }

        let local: Vec<(BTreeMap<String, String>, String)> = pair
            .tasks
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
            "`mise run {}` does not run exactly what `{}`'s `{name}` job runs (left: mise, right: \
             CI after its row's named rewrites; each entry is its environment and its command). \
             The two must be identical -- change whichever side is wrong, not only one of them",
            pair.tasks.join("` + `mise run "),
            pair.workflow,
        );
    }
}

/// The workspace `rust-version`, spelled as the rustup toolchain that installs it.
fn msrv_toolchain() -> String {
    let manifest: toml::Table = toml::from_str(&read("Cargo.toml")).expect("Cargo.toml parses");
    let declared = manifest["workspace"]["package"]["rust-version"]
        .as_str()
        .expect("the workspace declares `rust-version`");
    // Cargo accepts `1.88`; rustup's toolchain spelling is the full `1.88.0`.
    match declared.split('.').count() {
        2 => format!("{declared}.0"),
        3 => declared.to_owned(),
        _ => panic!("`rust-version = {declared:?}` is not `major.minor[.patch]`"),
    }
}

#[test]
fn the_msrv_gate_runs_on_the_declared_rust_version() {
    // A toolchain file overrides rustup's default toolchain, and `rust-toolchain.toml` pins a
    // newer release than `rust-version`. `dtolnay/rust-toolchain@1.88.0` only sets that default,
    // so CI's msrv job ran a bare `cargo check` on the file's toolchain (then `stable`): the
    // master log said the stable toolchain "is currently in use (overridden by
    // '.../rust-toolchain.toml')". Only an explicit `cargo +<toolchain>` outranks the file. The
    // pairing test holds the two sides to each other, so reverting both to a bare `cargo check`
    // passed it; this holds both to the manifest's `rust-version`.
    let toolchain = msrv_toolchain();
    let prefix = format!("cargo +{toolchain} ");
    // A prefix check alone passes `cargo +1.88.0 fetch && cargo check …` or a `run: |` block
    // whose later lines are bare `cargo check`, both of which check on stable. So each command
    // must be one line with no shell control operator or substitution: one pinned cargo call.
    let pinned = |command: &str| {
        let command = command.trim();
        command.starts_with(&prefix)
            && !command.contains(['\n', '\r', ';', '&', '|', '`'])
            && !command.contains("$(")
    };

    let tasks = mise_tasks();
    let local = mise_commands(&tasks, "msrv");
    assert!(!local.is_empty(), "`mise run msrv` runs nothing");
    for command in &local {
        assert!(
            pinned(command),
            "`mise run msrv` runs `{command}`, which is not a single `{prefix}…` invocation (one \
             line, no shell operators); without an explicit toolchain on every cargo call \
             `rust-toolchain.toml` selects its pinned release, not `rust-version`"
        );
    }

    let job = &ci_workflow()["jobs"]["msrv"];
    let steps = job["steps"].as_vec().expect("the `msrv` job carries steps");
    let runs: Vec<&str> = steps
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect();
    assert!(!runs.is_empty(), "CI's `msrv` job runs nothing");
    for run in runs {
        assert!(
            pinned(run),
            "CI's `msrv` job runs `{run}`, which is not a single `{prefix}…` invocation (one \
             line, no shell operators); without an explicit toolchain on every cargo call \
             `rust-toolchain.toml` selects its pinned release, not `rust-version`"
        );
    }
    let toolchains: Vec<&str> = steps
        .iter()
        .filter_map(|step| step["uses"].as_str())
        .filter_map(|uses| uses.strip_prefix("dtolnay/rust-toolchain@"))
        .collect();
    assert_eq!(
        toolchains,
        [toolchain.as_str()],
        "CI's `msrv` job must install exactly the `rust-version` toolchain its commands name"
    );
}

/// The `dtolnay/rust-toolchain@` refs a workflow job may install instead of
/// `rust-toolchain.toml`'s `channel`, as (file, job, ref, why). [`MSRV_REF`] stands for the
/// workspace `rust-version` toolchain, which `the_msrv_gate_runs_on_the_declared_rust_version`
/// holds the job's commands to.
const TOOLCHAIN_EXCEPTIONS: [(&str, &str, &str, &str); 2] = [
    (
        "ci.yml",
        "msrv",
        MSRV_REF,
        "the declared `rust-version` floor, a published contract separate from the development \
         toolchain",
    ),
    (
        "ci.yml",
        "runtime-dependencies",
        "nightly",
        "`-Z direct-minimal-versions` exists only on nightly; the one floating toolchain, named \
         in CLAUDE.md",
    ),
];

/// Placeholder in [`TOOLCHAIN_EXCEPTIONS`] for the `rust-version` toolchain.
const MSRV_REF: &str = "<rust-version>";

#[test]
fn ci_installs_the_rust_toolchain_this_file_pins() {
    // `rust-toolchain.toml` selects the toolchain for every local `cargo` call, so for every
    // `mise run` gate. CI's toolchain steps install the release that file names, so a gate and
    // its CI job compile, lint and format with the same rustc. When both named the moving
    // `stable` channel, a local toolchain was as new as its last `rustup update` and CI's as new
    // as the day it ran, so a new clippy lint could fail one side and not the other. A bump is a
    // PR of its own that changes the file and every workflow step together; this test fails on
    // any one changed alone.
    let file: toml::Table =
        toml::from_str(&read("rust-toolchain.toml")).expect("rust-toolchain.toml parses");
    let channel = file["toolchain"]["channel"]
        .as_str()
        .expect("`rust-toolchain.toml` sets `[toolchain] channel`");
    assert!(
        channel.split('.').count() == 3 && channel.split('.').all(|p| p.parse::<u64>().is_ok()),
        "`rust-toolchain.toml` sets `channel = {channel:?}`, which is not a concrete `1.x.y` \
         release: a channel name moves without a commit, so local gates and CI drift apart"
    );
    let msrv = msrv_toolchain();

    let mut used = BTreeSet::new();
    let mut seen = 0usize;
    for file in workflow_files() {
        let workflow = workflow(&file);
        let Some(jobs) = workflow["jobs"].as_hash() else {
            continue;
        };
        for (job, body) in jobs {
            let job = job.as_str().unwrap_or_default();
            for step in body["steps"].as_vec().into_iter().flatten() {
                let Some(uses) = step["uses"].as_str() else {
                    continue;
                };
                let lowered = uses.to_ascii_lowercase();
                let Some(reference) = lowered.strip_prefix("dtolnay/rust-toolchain@") else {
                    continue;
                };
                seen += 1;
                assert!(
                    step["with"]["toolchain"].is_badvalue(),
                    "`{file}`'s `{job}` job passes `with: {{ toolchain: … }}` to `{uses}`, which \
                     overrides the ref; name the toolchain in the ref alone"
                );
                if reference == channel {
                    continue;
                }
                let exception = TOOLCHAIN_EXCEPTIONS.iter().position(|(f, j, r, _)| {
                    *f == file
                        && *j == job
                        && (*r == reference || (*r == MSRV_REF && msrv == reference))
                });
                let Some(exception) = exception else {
                    panic!(
                        "`{file}`'s `{job}` job installs `{uses}`, and `rust-toolchain.toml` pins \
                         `{channel}`; a local gate and its CI job must run the same rustc. \
                         Change both together, or name a deliberate exception in \
                         TOOLCHAIN_EXCEPTIONS"
                    )
                };
                used.insert(exception);
            }
        }
    }
    assert!(
        seen > 0,
        "no workflow installs a Rust toolchain; this test reads nothing"
    );
    for (index, (file, job, reference, why)) in TOOLCHAIN_EXCEPTIONS.iter().enumerate() {
        assert!(!why.is_empty(), "a toolchain exception must say why");
        assert!(
            used.contains(&index),
            "TOOLCHAIN_EXCEPTIONS names `{reference}` for `{file}`'s `{job}` job, which installs no \
             such toolchain; drop the stale exception"
        );
    }
}

/// `mise.toml` `[tools]` entries CI has no counterpart for, each with why. Every other pinned tool
/// must be installed by CI at exactly the pinned version.
const LOCAL_ONLY_TOOLS: [(&str, &str); 1] = [(
    "hk",
    "the git hook manager; CI runs each gate's commands itself and installs no hooks",
)];

/// `mise.toml`'s `[tools]` as tool name to pinned version. The name drops the backend
/// (`cargo:convco` is `convco`, `aqua:EmbarkStudios/cargo-deny` is `cargo-deny`), which is the
/// name `cargo install` and `taiki-e/install-action` spell the same tool with.
fn mise_tool_pins() -> BTreeMap<String, String> {
    let mise: toml::Table = toml::from_str(&read("mise.toml")).expect("mise.toml must parse");
    let tools = mise["tools"]
        .as_table()
        .expect("`mise.toml` must carry a `[tools]` table");
    tools
        .iter()
        .map(|(key, version)| {
            let name = key.rsplit_once(':').map_or(key.as_str(), |(_, tool)| tool);
            let name = name.rsplit_once('/').map_or(name, |(_, tool)| tool);
            let version = version
                .as_str()
                .unwrap_or_else(|| panic!("`[tools] {key}` must be a version string"));
            assert!(
                !version.is_empty() && version.split('.').all(|part| part.parse::<u64>().is_ok()),
                "`[tools] {key} = {version:?}` is not an exact version, so what mise installs \
                 moves without a commit and nothing can hold CI to it"
            );
            (name.to_owned(), version.to_owned())
        })
        .collect()
}

/// The `(tool, version)` pairs a `cargo install` / `cargo binstall` line installs, with `None`
/// for a tool installed without a version (which is whatever is newest at run time).
fn cargo_installs(line: &str) -> Vec<(String, Option<String>)> {
    // Flags of `cargo install` that take a value, so the value is not read as a crate name.
    const VALUED: [&str; 16] = [
        "--version",
        "--vers",
        "--root",
        "--git",
        "--branch",
        "--tag",
        "--rev",
        "--path",
        "--registry",
        "--index",
        "--target",
        "--features",
        "-F",
        "--profile",
        "--jobs",
        "-j",
    ];
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(start) = words
        .windows(2)
        .position(|pair| pair[0] == "cargo" && ["install", "binstall"].contains(&pair[1]))
    else {
        return Vec::new();
    };
    let mut version = None;
    let mut crates = Vec::new();
    let mut rest = words[start + 2..].iter();
    while let Some(word) = rest.next() {
        if ["&&", "||", ";", "|"].contains(word) {
            break;
        }
        if let Some((flag, value)) = word.split_once('=') {
            if flag == "--version" || flag == "--vers" {
                version = Some(value.to_owned());
            }
            continue;
        }
        if *word == "--version" || *word == "--vers" {
            version = rest.next().map(|value| (*value).to_owned());
        } else if VALUED.contains(word) {
            rest.next();
        } else if !word.starts_with('-') {
            crates.push(*word);
        }
    }
    crates
        .into_iter()
        .map(|krate| match krate.split_once('@') {
            Some((name, pinned)) => (name.to_owned(), Some(pinned.to_owned())),
            None => (krate.to_owned(), version.clone()),
        })
        .collect()
}

#[test]
fn ci_installs_exactly_the_tool_versions_mise_pins() {
    // The pairing test holds each CI job's commands identical to its mise task's, but a command
    // is only identical if the binary running it is: `cargo deny check` under cargo-deny 0.19.9
    // and under 0.20.2 are different audits (0.20.0 added a lint and removed CLI flags), and CI's
    // deny gate ran the action's bundled 0.20.2 while mise pinned 0.19.9 (#228). The same held for
    // cargo-hack, which CI installed at whatever was newest. So every tool CI installs must be one
    // `[tools]` pins, at exactly that version, and every pinned tool must be installed by CI
    // unless LOCAL_ONLY_TOOLS says why not. A gate action that carries its own copy of a pinned
    // tool is caught by the second half: the tool is then pinned but never installed.
    let pins = mise_tool_pins();
    let mut installed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for file in workflow_files() {
        let workflow = workflow(&file);
        let Some(jobs) = workflow["jobs"].as_hash() else {
            continue;
        };
        for (job, body) in jobs {
            let job = job.as_str().unwrap_or_default();
            for step in body["steps"].as_vec().into_iter().flatten() {
                let mut found = Vec::new();
                if let Some(uses) = step["uses"].as_str() {
                    let lowered = uses.to_ascii_lowercase();
                    if lowered.starts_with("taiki-e/install-action@") {
                        // `taiki-e/install-action@<tool>` installs the newest release of `<tool>`;
                        // only `with: { tool: <tool>@<version> }` names a version.
                        let tools = step["with"]["tool"].as_str().unwrap_or_else(|| {
                            panic!(
                                "`{file}`'s `{job}` job uses `{uses}` without `with: {{ tool: … }}`, \
                                 which installs the newest release rather than the version \
                                 `mise.toml` pins; use `taiki-e/install-action@v2` with \
                                 `tool: <name>@<version>`"
                            )
                        });
                        for tool in tools.split([',', '\n']).map(str::trim) {
                            if tool.is_empty() {
                                continue;
                            }
                            found.push(match tool.split_once('@') {
                                Some((name, version)) => {
                                    (name.to_owned(), Some(version.to_owned()))
                                }
                                None => (tool.to_owned(), None),
                            });
                        }
                    }
                }
                if let Some(run) = step["run"].as_str() {
                    found.extend(run.lines().flat_map(cargo_installs));
                }
                for (tool, version) in found {
                    let pinned = pins.get(&tool).unwrap_or_else(|| {
                        panic!(
                            "`{file}`'s `{job}` job installs `{tool}`, which `[tools]` in \
                             mise.toml does not pin; the mise task that runs it locally would \
                             run whatever is on PATH"
                        )
                    });
                    assert_eq!(
                        version.as_deref(),
                        Some(pinned.as_str()),
                        "`{file}`'s `{job}` job installs `{tool}` at {version:?}, and `[tools]` in \
                         mise.toml pins {pinned}; the two gates must run the same binary"
                    );
                    installed.entry(tool).or_default().insert(job.to_owned());
                }
            }
        }
    }

    for (tool, why) in LOCAL_ONLY_TOOLS {
        assert!(!why.is_empty(), "a local-only tool must say why");
        assert!(
            pins.contains_key(tool),
            "LOCAL_ONLY_TOOLS names `{tool}`, which `[tools]` in mise.toml does not pin"
        );
        assert!(
            !installed.contains_key(tool),
            "LOCAL_ONLY_TOOLS names `{tool}`, which CI installs; drop it from the list"
        );
    }
    for tool in pins.keys() {
        assert!(
            installed.contains_key(tool) || LOCAL_ONLY_TOOLS.iter().any(|(local, _)| local == tool),
            "`[tools]` in mise.toml pins `{tool}`, and no CI job installs it: either CI runs its \
             gate through a binary it gets some other way (an action's bundled copy, a runner \
             image's), whose version nothing holds to the pin, or the tool is local-only and \
             belongs in LOCAL_ONLY_TOOLS with why"
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
