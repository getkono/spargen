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
//! `.github/workflows/` and `mise.toml` live here beside the corpus ones rather than in a file of
//! their own. CLAUDE.md's testing-strategy table gives each kind a row naming this file, and the
//! two rows together name every test here
//! (`the_testing_strategy_table_names_every_test_in_its_suite`).
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
        // Every diagnostic's declared claim must be one the run's outcome admits (#413). This
        // checks the declared claim only, not the message prose: a message that states an outcome
        // its claim does not declare (#174's "is generated", which was built `Independent`) passes
        // here. `frontend.rs`'s `claim_violations` reads the prose and is what catches that shape.
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

/// The only global flags a `cargo deny` audit command may pass, with whether each takes a value.
/// An allow-list, not a deny-list (#238): a word outside it fails the audit's test whatever it is,
/// so a flag nobody has measured, or one a later cargo-deny adds, is rejected until it is argued
/// onto this list. Each entry is here because it cannot shrink the graph cargo-deny resolves:
/// `--all-features` is the widest feature selection there is; `--locked` only makes `cargo
/// metadata` fail where it would rewrite `Cargo.lock`; `--manifest-path` and `--config` take
/// values their callers hold to named files (the root or an example `Cargo.toml`, and
/// `deny.toml`).
///
/// A deny-list of graph-narrowing flags (`--exclude`, `--target`/`-t`, `--exclude-dev`,
/// `--exclude-unpublished`, `--offline`, `--frozen`, `--no-default-features`) stood here before,
/// and `--metadata-path` was not on it: on this tree with cargo-deny 0.20.2, `cargo deny
/// --all-features --locked --metadata-path <default-feature cargo metadata> list` drops
/// `rustls@0.23.45` from the audited graph, as `--exclude rustls` does, with every word the old
/// test asked for still present. `-L`/`--log-level`, `-f`/`--format` and `-c`/`--color` change
/// only what is printed; they are off the list because no audit passes them, not because they
/// narrow.
const DENY_AUDIT_FLAGS: [(&str, bool); 4] = [
    ("--all-features", false),
    ("--locked", false),
    ("--manifest-path", true),
    ("--config", true),
];

/// The only flags a `cargo audit` lockfile audit command may pass, with whether each takes a
/// value; like `DENY_AUDIT_FLAGS`, an allow-list (#238). `lockfile_audits` holds each one's value:
/// `--db` to the printed checkout, `--no-fetch` to that checkout staying the one printed, `--deny`
/// to `warnings`, `--file` to a committed lockfile, and every `--ignore` to `deny.toml`'s
/// `[advisories] ignore`. cargo-audit's `--no-yanked`, `--target-arch`, `--target-os` and
/// `--stale` all narrow what it reports, and are rejected here with every other word.
const LOCKFILE_AUDIT_FLAGS: [(&str, bool); 5] = [
    ("--db", true),
    ("--no-fetch", false),
    ("--deny", true),
    ("--file", true),
    ("--ignore", true),
];

/// Every word of `words` read as a flag from `allowed`, in order, each paired with its value:
/// `--flag value` or `--flag=value` where the flag takes one, the bare flag where it does not.
/// Any other word -- a flag outside `allowed`, a short alias, a value with no flag before it, a
/// value-taking flag with no value, or a value on a flag that takes none -- is an `Err` naming it.
fn allowed_flags<'a>(
    words: &[&'a str],
    allowed: &[(&'static str, bool)],
) -> Result<Vec<(&'static str, Option<&'a str>)>, String> {
    let mut flags = Vec::new();
    let mut rest = words.iter();
    while let Some(&word) = rest.next() {
        let (flag, attached) = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (word, None),
        };
        let &(flag, takes_value) = allowed
            .iter()
            .find(|(name, _)| *name == flag)
            .ok_or_else(|| format!("`{word}` is not one of the allowed flags"))?;
        let value = match (takes_value, attached) {
            (false, None) => None,
            (false, Some(_)) => return Err(format!("`{word}`: `{flag}` takes no value")),
            (true, Some(value)) => Some(value),
            (true, None) => Some(rest.next().copied().unwrap_or_default()),
        };
        if value.is_some_and(|value| value.is_empty() || value.starts_with('-')) {
            return Err(format!("`{flag}` has no value"));
        }
        flags.push((flag, value));
    }
    Ok(flags)
}

/// What a `cargo deny` audit's global flags select.
#[derive(Debug, Default, PartialEq)]
struct DenyAuditFlags<'a> {
    all_features: bool,
    locked: bool,
    manifest: Option<&'a str>,
    config: Option<&'a str>,
}

/// The global flags of a `cargo deny` audit (the words between `cargo deny` and `check`), held to
/// `DENY_AUDIT_FLAGS`, each passed at most once. A leading `./` on a path value is dropped.
fn deny_audit_flags<'a>(globals: &[&'a str]) -> Result<DenyAuditFlags<'a>, String> {
    let mut selected = DenyAuditFlags::default();
    for (flag, value) in allowed_flags(globals, &DENY_AUDIT_FLAGS)? {
        let value = value.map(|path| path.trim_start_matches("./"));
        let repeated = match flag {
            "--all-features" => std::mem::replace(&mut selected.all_features, true),
            "--locked" => std::mem::replace(&mut selected.locked, true),
            "--manifest-path" => selected
                .manifest
                .replace(value.unwrap_or_default())
                .is_some(),
            "--config" => selected.config.replace(value.unwrap_or_default()).is_some(),
            other => unreachable!("`{other}` is in `DENY_AUDIT_FLAGS` but not read here"),
        };
        if repeated {
            return Err(format!("`{flag}` is passed more than once"));
        }
    }
    Ok(selected)
}

/// The top-level tables `deny.toml` may carry: the four checks' policies. Like `DENY_AUDIT_FLAGS`,
/// an allow-list, because the file narrows the graph as surely as a flag, from outside any
/// command: `[graph]` carries `targets`, `exclude`, `features`, `no-default-features`,
/// `exclude-dev` and `exclude-unpublished`, the file-side spellings of the graph flags
/// `DENY_AUDIT_FLAGS` leaves out (`[graph] exclude = ["rustls"]` drops `rustls@0.23.45` from
/// `cargo deny --all-features --locked list` on this tree, cargo-deny 0.20.2).
const DENY_TOML_TABLES: [&str; 4] = ["advisories", "bans", "licenses", "sources"];

/// The positional values `cargo deny check` takes, per cargo-deny 0.20.2's `check --help`.
const CHECK_NAMES: [&str; 5] = ["advisories", "bans", "licenses", "sources", "all"];

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
    // Every committed lockfile is audited: the root one -- no `--manifest-path`, or one naming the
    // root `Cargo.toml` -- and each example workspace's, which gating jobs compile (for wasm32
    // too) and whose resolves carry crates the root one never reaches (#184). Each audit must
    // pass `--all-features`, only flags `DENY_AUDIT_FLAGS` allows, and a bare `check`, since
    // `check licenses` drops `advisories` and `check advisories` drops the licence, ban and source
    // checks the examples were outside of. The root audit must also pass `--locked`; an example
    // audit must not need to (its `spargen` path-package stamp goes stale on every release, so no
    // gate that compiles an example is locked), and must pass `--config deny.toml`, so it runs
    // under the one policy rather than whatever cargo-deny would discover beside the example; the
    // root audit reads that same file, by default or by name.
    //
    // Together these hold every input cargo-deny builds the graph from (#238): its argv, whose
    // every word is allow-listed (containment of `--all-features` alone admitted `--all-features
    // --exclude rustls`, and a deny-list of such flags admitted `--metadata-path`);
    // the policy file's `[graph]`, which `DENY_TOML_TABLES` excludes; the task's environment, held
    // empty; and its working directory, which neither the task (`MISE_TASK_KEYS`) nor its CI job
    // (`WORKFLOW_KEYS`, `JOB_KEYS`) can move off the repository root.
    let deny: toml::Table = toml::from_str(&read("deny.toml")).expect("deny.toml parses");
    let unexpected: Vec<&String> = deny
        .keys()
        .filter(|key| !DENY_TOML_TABLES.contains(&key.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "deny.toml carries {unexpected:?}; only the checks' policies {DENY_TOML_TABLES:?} may \
         appear there, since `[graph]` narrows the graph every audit resolves from outside any \
         command (`[graph] exclude = [\"rustls\"]` drops rustls from it)"
    );
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

    // Every example workspace that commits a lockfile, by manifest path.
    let mut examples: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(workspace_root().join("examples")).expect("examples/ is listed")
    {
        let dir = entry.expect("an examples/ entry is readable").path();
        if dir.join("Cargo.toml").is_file() && dir.join("Cargo.lock").is_file() {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .expect("UTF-8 names");
            examples.insert(format!("examples/{name}/Cargo.toml"));
        }
    }
    assert!(
        examples.len() >= 3,
        "found only {examples:?} under examples/; the scan is not finding the example workspaces"
    );

    let mut root_audits = 0usize;
    let mut example_audits: BTreeSet<String> = BTreeSet::new();
    for command in &commands {
        let words: Vec<&str> = command.split_whitespace().collect();
        // The lockfile-complete audit and the database fetch and revision line it reads are held
        // by `the_lockfile_audit_reads_every_committed_lockfile`.
        if lockfile_audit_step(&words).is_some() {
            continue;
        }
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

        let flags = deny_audit_flags(globals).unwrap_or_else(|error| {
            panic!(
                "`mise run deny` runs `{command}`: {error}. An audit passes only the flags \
                 `DENY_AUDIT_FLAGS` allows, {DENY_AUDIT_FLAGS:?}, each of which cannot shrink the \
                 graph cargo-deny resolves; `--all-features --exclude rustls` still contains \
                 `--all-features` and drops rustls from it (#238)"
            )
        });
        let manifest = flags.manifest.unwrap_or("Cargo.toml");
        if manifest == "Cargo.toml" {
            root_audits += 1;
            // Without `--locked`, cargo-deny's `cargo metadata` rewrites a lockfile that does not
            // match the manifests and audits the rewrite: a skewed `Cargo.lock` (rustls 0.23.45
            // with rustls-webpki 0.103.13) reported green and was silently corrected, and a
            // deleted one was regenerated and reported `advisories ok` (#146). With it, both fail
            // -- the deleted lockfile with "cannot create the lock file ... because --locked was
            // passed". `--frozen` would also hold the lockfile, but it implies `--offline`, which
            // is not on `DENY_AUDIT_FLAGS`.
            assert!(
                flags.locked,
                "`mise run deny` runs `{command}`, which does not pass `--locked`, so a lockfile \
                 that does not match the manifests is rewritten and the rewrite is audited \
                 instead of the committed `Cargo.lock`"
            );
            assert!(
                matches!(flags.config, None | Some("deny.toml")),
                "`mise run deny` runs `{command}`, which audits the root workspace under a policy \
                 other than the repository's `deny.toml`"
            );
        } else if examples.contains(manifest) {
            example_audits.insert(manifest.to_owned());
            assert_eq!(
                flags.config,
                Some("deny.toml"),
                "`mise run deny` runs `{command}`, which does not pass `--config deny.toml`, so \
                 the example is audited under whatever policy cargo-deny finds beside it rather \
                 than the repository's one"
            );
        } else {
            panic!(
                "`mise run deny` runs `{command}`, whose `--manifest-path {manifest}` is neither \
                 the root `Cargo.toml` nor an example workspace's"
            );
        }

        assert!(
            flags.all_features,
            "`mise run deny` runs `{command}`, which does not pass `--all-features`, so the \
             audited graph has no TLS stack in it"
        );
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
    assert_eq!(
        example_audits, examples,
        "`mise run deny` must audit every example workspace's committed lockfile: gating jobs \
         compile them, and their resolves carry crates the root one never reaches (#184)"
    );
}

#[test]
fn the_audit_flag_allow_lists_admit_only_what_they_name() {
    // The gate tests above read the audit commands through `deny_audit_flags` and
    // `allowed_flags`; this holds those readers to rejecting what they do not name (#238). Every
    // rejected value below still contains `--all-features --locked`, which is all the test asked
    // for before a deny-list, and all a deny-list could establish after it.
    let words = |line: &'static str| line.split_whitespace().collect::<Vec<_>>();
    assert_eq!(
        deny_audit_flags(&words("--all-features --locked")),
        Ok(DenyAuditFlags {
            all_features: true,
            locked: true,
            ..DenyAuditFlags::default()
        })
    );
    assert_eq!(
        deny_audit_flags(&words(
            "--manifest-path=./examples/petstore/Cargo.toml --config deny.toml --all-features"
        )),
        Ok(DenyAuditFlags {
            all_features: true,
            manifest: Some("examples/petstore/Cargo.toml"),
            config: Some("deny.toml"),
            ..DenyAuditFlags::default()
        })
    );
    for narrowing in [
        // Measured on this tree with cargo-deny 0.20.2 to drop rustls from `cargo deny list`.
        "--exclude rustls",
        "--metadata-path target/default-features-metadata.json",
        // Measured in #238 with cargo-deny 0.19.9 to turn `advisories FAILED` into `ok`, in each
        // spelling clap accepts.
        "--target wasm32-unknown-unknown",
        "-t wasm32-unknown-unknown",
        "-twasm32-unknown-unknown",
        "--target=wasm32-unknown-unknown",
        // Graph flags of the same kind, and a feature selection beside `--all-features`.
        "--exclude-dev",
        "--exclude-unpublished",
        "--workspace",
        "--offline",
        "--frozen",
        "--no-default-features",
        "--features cli",
        // Output-only flags no audit passes: off the list, so rejected with the rest.
        "--log-level off",
        "-L off",
        "--format json",
        // Malformed spellings of allowed flags.
        "--locked",
        "--all-features=true",
        "--config",
        "--config --all-features",
        "--manifest-path=",
        "Cargo.toml",
    ] {
        let line = format!("--all-features --locked {narrowing}");
        let globals: Vec<&str> = line.split_whitespace().collect();
        assert!(
            deny_audit_flags(&globals).is_err(),
            "`cargo deny {line} check` passes `deny_audit_flags`, so the gate tests admit it"
        );
    }
    for narrowing in [
        "--no-yanked",
        "--target-arch x86",
        "--target-os windows",
        "--stale",
        "-n",
        "--file",
    ] {
        let line =
            format!("--db target/db --no-fetch --deny warnings --file Cargo.lock {narrowing}");
        let flags: Vec<&str> = line.split_whitespace().collect();
        assert!(
            allowed_flags(&flags, &LOCKFILE_AUDIT_FLAGS).is_err(),
            "`cargo audit {line}` passes `allowed_flags`, so the lockfile audit test admits it"
        );
    }
}

/// The published workspace crates that ship a binary, by package name. `cargo package` puts
/// `Cargo.lock` into every `.crate`, but only a binary's is ever resolved against: `cargo install
/// --locked` installs its pins, while a library's lockfile is ignored by everything that depends on
/// it.
fn published_binary_crates() -> BTreeSet<String> {
    let root: toml::Table = toml::from_str(&read("Cargo.toml")).expect("Cargo.toml parses");
    let members = root["workspace"]["members"]
        .as_array()
        .expect("the workspace lists its members");
    let mut binaries = BTreeSet::new();
    for member in members {
        let member = member.as_str().expect("workspace members are paths");
        let manifest: toml::Table = toml::from_str(&read(&format!("{member}/Cargo.toml")))
            .unwrap_or_else(|error| panic!("`{member}/Cargo.toml` must parse: {error}"));
        let package = &manifest["package"];
        if package.get("publish").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let dir = workspace_root().join(member);
        if manifest.contains_key("bin")
            || dir.join("src/main.rs").exists()
            || dir.join("src/bin").is_dir()
        {
            let name = package["name"].as_str().expect("a package has a name");
            binaries.insert(name.to_owned());
        }
    }
    binaries
}

#[test]
fn the_published_lockfile_audit_covers_every_shipped_binary() {
    // `deny` audits the committed `Cargo.lock`; a fix there reaches crates.io only when a release
    // carries it, and until then `cargo install spargen --features cli --locked` installed the
    // shipped `rustls 0.23.41` (RUSTSEC-2026-0285) with every check green (#178). `mise run
    // deny-published` (CI's `deny-published` job, byte for byte) audits the shipped lockfile.
    // This holds it to every published crate that ships a binary, so a second binary crate
    // cannot be published unaudited, and holds each audit to the lockfile as shipped.
    let tasks = mise_tasks();
    let commands = mise_commands(&tasks, "deny-published");
    let script = commands.join("\n");
    let downloaded: BTreeSet<String> = script
        .split("https://static.crates.io/crates/")
        .skip(1)
        .map(|rest| rest.split('/').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        downloaded,
        published_binary_crates(),
        "`mise run deny-published` must download the latest `.crate` of exactly the published \
         crates that ship a binary: their `Cargo.lock` is what `cargo install --locked` installs"
    );

    let mut audited = BTreeSet::new();
    for command in &commands {
        let words: Vec<&str> = command.split_whitespace().collect();
        if !words.starts_with(&["cargo", "deny"]) || lockfile_audit_step(&words).is_some() {
            continue;
        }
        let check = words
            .iter()
            .position(|word| *word == "check")
            .unwrap_or_else(|| panic!("`{command}` is not a `cargo deny check`"));
        let (globals, which) = (&words[2..check], &words[check + 1..]);
        let flags = deny_audit_flags(globals).unwrap_or_else(|error| {
            panic!(
                "`{command}`: {error}. An audit passes only the flags `DENY_AUDIT_FLAGS` allows, \
                 {DENY_AUDIT_FLAGS:?}, each of which cannot shrink the graph cargo-deny resolves \
                 (#238)"
            )
        });
        let manifest = flags
            .manifest
            .unwrap_or_else(|| panic!("`{command}` names no `--manifest-path`"));
        let krate = manifest
            .strip_prefix("target/deny-published/")
            .and_then(|rest| rest.strip_suffix("/Cargo.toml"))
            .unwrap_or_else(|| {
                panic!("`{command}` audits `{manifest}`, not a crate extracted by this task")
            });
        audited.insert(krate.to_owned());
        // `--locked`: cargo-deny's `cargo metadata` would otherwise re-resolve a lockfile that no
        // longer matches and audit the re-resolution, which is not what `--locked` installs.
        // `--all-features`: the TLS stack reaches the graph only through features (#147).
        // `--config deny.toml`: the same advisory policy, ignores and `yanked` included, as `deny`.
        assert!(flags.locked, "`{command}` does not pass `--locked`");
        assert!(
            flags.all_features,
            "`{command}` does not pass `--all-features`"
        );
        assert_eq!(
            flags.config,
            Some("deny.toml"),
            "`{command}` does not audit under the repository's `deny.toml`"
        );
        // After `check` come only check names: a word there that is not one is one of `check`'s
        // own flags, and those are held to none, as `deny`'s bare `check` holds them.
        assert!(
            which.iter().all(|word| CHECK_NAMES.contains(word)),
            "`{command}` passes {which:?} after `check`, which are not all check names {CHECK_NAMES:?}"
        );
        assert!(
            which.is_empty() || which.contains(&"advisories") || which.contains(&"all"),
            "`{command}` narrows `check` to {which:?}, which drops `advisories`"
        );
    }
    assert_eq!(
        audited, downloaded,
        "every crate `mise run deny-published` downloads must be audited, and only those"
    );
}

/// One step of the lockfile-complete advisory audit an audit task runs (#187).
#[derive(Debug, PartialEq)]
enum LockfileAuditStep<'a> {
    /// `cargo deny fetch db`: refresh the RustSec checkout under `deny.toml`'s `db-path`.
    Fetch,
    /// `git -C <checkout> log -1 --format=…`: print the commit that checkout is at.
    Revision(&'a str),
    /// `cargo audit …`: audit every entry of one lockfile.
    Audit,
}

/// Which lockfile-audit step a command's words are, if any.
fn lockfile_audit_step<'a>(words: &[&'a str]) -> Option<LockfileAuditStep<'a>> {
    if words.starts_with(&["cargo", "deny", "fetch"]) {
        Some(LockfileAuditStep::Fetch)
    } else if words.starts_with(&["cargo", "audit"]) || words.first() == Some(&"cargo-audit") {
        Some(LockfileAuditStep::Audit)
    } else if words.first() == Some(&"git") {
        Some(LockfileAuditStep::Revision(
            words
                .windows(2)
                .find(|pair| pair[0] == "-C")
                .map_or("", |pair| pair[1]),
        ))
    } else {
        None
    }
}

/// The lockfiles `task`'s `cargo audit` commands read, after holding the task to the audit's
/// shape: one `cargo deny fetch db`, then one line printing the fetched checkout's commit, then
/// every `cargo audit`, each over that checkout without fetching, denying every warning kind, and
/// ignoring exactly what `deny.toml` ignores.
fn lockfile_audits(task: &str) -> BTreeSet<String> {
    let deny: toml::Table = toml::from_str(&read("deny.toml")).expect("deny.toml parses");
    let advisories = deny["advisories"]
        .as_table()
        .expect("deny.toml has `[advisories]`");
    let db_path = advisories
        .get("db-path")
        .and_then(toml::Value::as_str)
        .expect("deny.toml's `[advisories]` names a `db-path` the audit can read the revision of");
    assert!(
        db_path.starts_with("target/"),
        "deny.toml's `db-path = {db_path:?}` is outside the gitignored `target/`"
    );
    let ignored: BTreeSet<String> = advisories
        .get("ignore")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| match entry {
            toml::Value::String(id) => id.clone(),
            toml::Value::Table(entry) => entry["id"]
                .as_str()
                .expect("an `[advisories] ignore` entry names its `id`")
                .to_owned(),
            other => panic!("unexpected `[advisories] ignore` entry {other}"),
        })
        .collect();
    // cargo-audit reads `.cargo/audit.toml` on its own, so an ignore or `--no-yanked` there would
    // narrow the audit without appearing in any command.
    assert!(
        !workspace_root().join(".cargo/audit.toml").exists(),
        "`.cargo/audit.toml` configures cargo-audit outside the gate's commands; state it on them"
    );

    let tasks = mise_tasks();
    let mut fetched = false;
    let mut checkout: Option<String> = None;
    let mut files = BTreeSet::new();
    for command in mise_commands(&tasks, task) {
        let words: Vec<&str> = command.split_whitespace().collect();
        match lockfile_audit_step(&words) {
            None => {}
            Some(LockfileAuditStep::Fetch) => {
                assert_eq!(
                    words,
                    ["cargo", "deny", "fetch", "db"],
                    "`mise run {task}` runs `{command}`; the audit fetches the database only, \
                     under the root `deny.toml`"
                );
                assert!(
                    !fetched && checkout.is_none() && files.is_empty(),
                    "`mise run {task}` fetches the database more than once, or after printing or \
                     auditing against it"
                );
                fetched = true;
            }
            Some(LockfileAuditStep::Revision(path)) => {
                assert!(
                    fetched && checkout.is_none() && files.is_empty(),
                    "`mise run {task}` must print the database revision once, after `cargo deny \
                     fetch db` and before any `cargo audit`"
                );
                assert!(
                    path.strip_prefix(db_path)
                        .and_then(|rest| rest.strip_prefix("/advisory-db-"))
                        .is_some_and(|hash| !hash.is_empty() && !hash.contains('/')),
                    "`mise run {task}` prints the revision of `{path}`, which is not cargo-deny's \
                     RustSec checkout under `db-path = {db_path:?}`"
                );
                assert!(
                    words.windows(2).any(|pair| pair == ["log", "-1"]) && command.contains("%H"),
                    "`mise run {task}` runs `{command}`, which does not print the checkout's \
                     commit hash"
                );
                checkout = Some(path.to_owned());
            }
            Some(LockfileAuditStep::Audit) => {
                let checkout = checkout.as_deref().unwrap_or_else(|| {
                    panic!(
                        "`mise run {task}` runs `{command}` before printing the database revision \
                         it audits against"
                    )
                });
                let skip = if words.first() == Some(&"cargo-audit") {
                    1
                } else {
                    2
                };
                let flags =
                    allowed_flags(&words[skip..], &LOCKFILE_AUDIT_FLAGS).unwrap_or_else(|error| {
                        panic!(
                            "`{command}`: {error}. A lockfile audit passes only the flags \
                             `LOCKFILE_AUDIT_FLAGS` allows, {LOCKFILE_AUDIT_FLAGS:?} (#238)"
                        )
                    });
                // The value of a flag passed exactly once; `--no-fetch`'s is `Some("")`.
                let value = |flag: &str| {
                    let mut values = flags.iter().filter(|(name, _)| *name == flag);
                    match (values.next(), values.next()) {
                        (Some((_, value)), None) => Some(value.unwrap_or_default()),
                        _ => None,
                    }
                };
                assert_eq!(
                    value("--db"),
                    Some(checkout),
                    "`{command}` must read, once, the checkout whose revision was printed, \
                     `{checkout}`"
                );
                assert_eq!(
                    value("--no-fetch"),
                    Some(""),
                    "`{command}` fetches its own database, so its revision is not the printed one"
                );
                assert_eq!(
                    value("--deny"),
                    Some("warnings"),
                    "`{command}` must deny every warning kind (yanked, unmaintained, unsound), as \
                     deny.toml's `[advisories]` does, and pass `--deny` once"
                );
                let ignores: BTreeSet<String> = flags
                    .iter()
                    .filter(|(name, _)| *name == "--ignore")
                    .filter_map(|(_, id)| id.map(str::to_owned))
                    .collect();
                assert_eq!(
                    ignores, ignored,
                    "`{command}` must ignore exactly the advisories deny.toml's `[advisories] \
                     ignore` does: one policy, not two"
                );
                let file = value("--file")
                    .unwrap_or_else(|| panic!("`{command}` names no single `--file`"))
                    .trim_start_matches("./");
                assert!(
                    files.insert(file.to_owned()),
                    "`mise run {task}` audits `{file}` twice"
                );
            }
        }
    }
    files
}

#[test]
fn the_lockfile_audit_reads_every_committed_lockfile() {
    // cargo-deny audits the dependency graph it activates, and filters out every lockfile entry
    // no feature activates. Cargo locks the target of a weak `dep?/feature` without enabling it,
    // so reqwest's `quinn?/ring` put quinn, `rand 0.10` and a yanked `chacha20 0.10.1` into
    // `Cargo.lock`: 278 entries, 261 audited, and `yanked = "deny"` green over the yank at every
    // feature scope (#187). `cargo audit` reads every entry of the file it is given; this holds
    // `deny` to running it over every committed lockfile, and `deny-published` over every shipped
    // one, each against the database revision the task prints, so a green run's log names the
    // advisory-database state it was reached against. CI's jobs run these commands byte for byte
    // (`every_mise_task_runs_exactly_what_its_ci_job_runs`).
    let mut committed = BTreeSet::from(["Cargo.lock".to_owned()]);
    for entry in std::fs::read_dir(workspace_root().join("examples")).expect("examples/ is listed")
    {
        let dir = entry.expect("an examples/ entry is readable").path();
        if dir.join("Cargo.lock").is_file() {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .expect("UTF-8 names");
            committed.insert(format!("examples/{name}/Cargo.lock"));
        }
    }
    assert!(
        committed.len() >= 4,
        "found only {committed:?}; the scan is not finding the example lockfiles"
    );
    assert_eq!(
        lockfile_audits("deny"),
        committed,
        "`mise run deny` must run `cargo audit` over every committed lockfile, and only those"
    );

    let shipped: BTreeSet<String> = published_binary_crates()
        .into_iter()
        .map(|krate| format!("target/deny-published/{krate}/Cargo.lock"))
        .collect();
    assert_eq!(
        lockfile_audits("deny-published"),
        shipped,
        "`mise run deny-published` must run `cargo audit` over the lockfile of every published \
         binary it extracts, and only those"
    );
}

#[test]
fn advisory_floors_are_manifest_requirements() {
    // A lockfile bump that clears an advisory is undone by one `cargo update -p <crate> --precise
    // <old>`, with every gate green once the advisory database stops naming it (#179). And it never
    // reached a consumer at all: Cargo ignores a dependency's lockfile, so a `build.rs` enabling
    // `remote-fetch` resolved rustls under reqwest's own `0.23.4` requirement. The floor therefore
    // lives in `spargen/Cargo.toml`, where the resolver enforces it for this workspace and for
    // every consumer, and `deny.toml` mirrors it as a `[bans] deny` entry naming the advisory.
    // This holds each ban's floor to a direct requirement stating exactly that release.
    let deny: toml::Table = toml::from_str(&read("deny.toml")).expect("deny.toml parses");
    let manifest: toml::Table =
        toml::from_str(&read("spargen/Cargo.toml")).expect("spargen/Cargo.toml parses");
    let dependencies = manifest["dependencies"]
        .as_table()
        .expect("spargen/Cargo.toml has a [dependencies] table");
    let bans = deny
        .get("bans")
        .and_then(|bans| bans.get("deny"))
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut floors = BTreeMap::new();
    for ban in bans {
        let ban = ban.as_table().expect("a `[bans] deny` entry is a table");
        // cargo-deny reads the version in a `crate = "name@version"` spec as one exact release:
        // measured with cargo-deny 0.20.2, `crate = "rustls@<0.23.45"` banned 0.23.45 itself and
        // passed 0.23.41. Only the `name` + `version` form takes a requirement.
        if let Some(spec) = ban.get("crate").and_then(toml::Value::as_str) {
            let version = spec.split_once('@').map(|(_, version)| version);
            assert!(
                version.is_none_or(|version| version.starts_with(|c: char| c.is_ascii_digit())),
                "`[bans] deny` entry `crate = \"{spec}\"`: cargo-deny matches that version as one \
                 exact release, not a range; state a floor as `name = ..., version = \"<X.Y.Z\"`"
            );
            continue;
        }
        let name = ban["name"]
            .as_str()
            .expect("a `[bans] deny` entry names its crate");
        let Some(version) = ban.get("version").and_then(toml::Value::as_str) else {
            continue;
        };
        let floor = version.strip_prefix('<').unwrap_or_else(|| {
            panic!(
                "`[bans] deny` entry for `{name}` bans `{version}`, which is not a `<X.Y.Z` floor"
            )
        });
        semver::Version::parse(floor).unwrap_or_else(|error| {
            panic!("`[bans] deny` entry for `{name}`: `{floor}` is not a release: {error}")
        });
        let reason = ban
            .get("reason")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.contains("RUSTSEC-"),
            "`[bans] deny` floor for `{name}` must name the advisory it enforces in `reason`"
        );
        assert!(
            floors.insert(name, floor).is_none(),
            "`deny.toml` bans more than one floor for `{name}`"
        );
    }
    assert!(
        floors.contains_key("rustls"),
        "`deny.toml` no longer floors rustls at the release that clears RUSTSEC-2026-0285 (#179)"
    );

    for (name, floor) in floors {
        let requirement = match dependencies.get(name) {
            Some(toml::Value::String(requirement)) => Some(requirement.as_str()),
            Some(toml::Value::Table(table)) => table.get("version").and_then(toml::Value::as_str),
            _ => None,
        };
        assert_eq!(
            requirement,
            Some(floor),
            "`deny.toml` floors `{name}` at {floor}, so `spargen/Cargo.toml` must require \
             `{name} = \"{floor}\"` directly: the ban guards only this repository's lockfile, and \
             only the manifest requirement reaches a consumer's resolve"
        );
    }
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
const GATE_WORKFLOWS: [GateWorkflow; 3] = [
    GateWorkflow {
        file: "ci.yml",
        on: "push:\n  branches: [master]\npull_request:",
        concurrency: Some("group: ci-${{ github.ref }}\ncancel-in-progress: true"),
        permissions: None,
    },
    // `ci.yml`'s triggers plus a daily schedule on `master` (and a manual one): the audit is
    // non-hermetic, so a new advisory must be found on `master` rather than by whichever open pull
    // request runs next (#146). Dropping `schedule:` fails here.
    GateWorkflow {
        file: "deny.yml",
        on: "push:\n  branches: [master]\npull_request:\nschedule:\n  - cron: \"17 6 * * *\"\nworkflow_dispatch:",
        concurrency: Some("group: deny-${{ github.ref }}\ncancel-in-progress: true"),
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
        job: "release-preview",
        tasks: &["release-preview"],
        ci_only: &[
            provision("uses: actions/checkout@v4\nwith:\n  fetch-depth: 0"),
            STABLE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: release-plz@0.3.160"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "deny.yml",
        job: "deny",
        tasks: &["deny"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-deny@0.20.2,cargo-audit@0.22.2",
            ),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "deny.yml",
        job: "deny-published",
        tasks: &["deny-published"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-deny@0.20.2,cargo-audit@0.22.2",
            ),
        ],
        // The published artefact changes only when a release publishes, never with a pull
        // request's diff; failing pull requests on it would block the release pull request that
        // carries the fix (#178). Locally the task runs whenever it is asked for.
        job_if: Some("github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'"),
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
    // whose later lines are bare `cargo check`, both of which check on the toolchain
    // `rust-toolchain.toml` pins rather than on `rust-version`. So each command
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

/// The `(tool, version)` pairs a line's `cargo [+<toolchain>] install` / `binstall` commands
/// install, with `None` for a tool installed without a version (whatever is newest at run time).
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
    let mut found = Vec::new();
    for command in shell_commands(line) {
        // `cargo install`, and `cargo +<toolchain> install`: a toolchain override between the two
        // words changes which cargo runs, not what it installs.
        let Some(sub) = cargo_subcommand(&command) else {
            continue;
        };
        if !["install", "binstall"].contains(&command[sub]) {
            continue;
        }
        let mut version = None;
        let mut crates = Vec::new();
        let mut rest = command[sub + 1..].iter();
        while let Some(word) = rest.next() {
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
        found.extend(crates.into_iter().map(|krate| match krate.split_once('@') {
            Some((name, pinned)) => (name.to_owned(), Some(pinned.to_owned())),
            None => (krate.to_owned(), version.clone()),
        }));
    }
    found
}

/// A shell line split into its simple commands (at `&&`, `||`, `;`, `|`), each as its words
/// with leading `VAR=value` assignments and shell keywords (`if`, `then`, `!`, ...) dropped, so
/// the first word is the program that runs.
fn shell_commands(line: &str) -> Vec<Vec<&str>> {
    let mut commands = vec![Vec::new()];
    for word in line.split_whitespace() {
        // A separator glued to a word (`check;`) still ends the command.
        let (word, ends) = match word.strip_suffix(';') {
            Some(stripped) => (stripped, true),
            None => (word, false),
        };
        if ["&&", "||", ";", "|"].contains(&word) {
            commands.push(Vec::new());
            continue;
        }
        let current = commands.last_mut().expect("never empty");
        let leading = current.is_empty();
        let keyword = [
            "if", "then", "else", "elif", "do", "while", "until", "!", "exec",
        ];
        let assignment = word
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty() && !name.starts_with('-'));
        if !(leading && (keyword.contains(&word) || assignment || word.is_empty())) {
            current.push(word);
        }
        if ends {
            commands.push(Vec::new());
        }
    }
    commands.retain(|command| !command.is_empty());
    commands
}

/// The index of `cargo`'s subcommand in a simple command, past an optional `+<toolchain>`.
/// `cargo` is looked for anywhere in the command, not only first, so a wrapper (`time cargo
/// install …`) cannot hide it; a false match fails loudly rather than passing silently.
fn cargo_subcommand(command: &[&str]) -> Option<usize> {
    let cargo = command
        .iter()
        .position(|word| *word == "cargo" || word.ends_with("/cargo"))?;
    let sub = if command
        .get(cargo + 1)
        .is_some_and(|word| word.starts_with('+'))
    {
        cargo + 2
    } else {
        cargo + 1
    };
    (sub < command.len()).then_some(sub)
}

/// The `[tools]`-pinned tools a shell line runs: `cargo [+<toolchain>] <sub>` runs `cargo-<sub>`
/// where that is pinned, and a pinned tool's name anywhere in any other command runs it
/// (`mdbook build`, `convco check`). Installing a tool is not running it.
fn tools_run(line: &str, pins: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut tools = BTreeSet::new();
    for command in shell_commands(line) {
        if let Some(sub) = cargo_subcommand(&command) {
            if ["install", "binstall"].contains(&command[sub]) {
                continue;
            }
            tools.insert(format!("cargo-{}", command[sub]));
        }
        for word in command {
            tools.insert(word.rsplit('/').next().unwrap_or(word).to_owned());
        }
    }
    tools.retain(|tool| pins.contains_key(tool));
    tools
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
    // tool is caught by the second half: the tool is then pinned but never installed. And since
    // jobs share no runner, a job that runs a pinned tool must install it itself, before running
    // it; an install in another job proves nothing about this one.
    /// One thing a step does to a pinned tool: install `(name, version)`, or run it.
    enum Event {
        Install((String, Option<String>)),
        Run(String),
    }
    let pins = mise_tool_pins();
    let mut installed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for file in workflow_files() {
        let workflow = workflow(&file);
        let Some(jobs) = workflow["jobs"].as_hash() else {
            continue;
        };
        for (job, body) in jobs {
            let job = job.as_str().unwrap_or_default();
            // What this job has installed so far: a tool a job runs must be installed by that
            // job, earlier, since jobs share no runner. A pin installed by some other job would
            // pass a workflow-wide check while this job ran whatever the runner image carries.
            let mut in_job = BTreeSet::new();
            for step in body["steps"].as_vec().into_iter().flatten() {
                // The step's installs and runs in the order the step performs them, so a `run: |`
                // block that runs a tool on one line and installs it on a later one is caught:
                // only an install on an earlier command, line, or step precedes a run.
                let mut events = Vec::new();
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
                            events.push(Event::Install(match tool.split_once('@') {
                                Some((name, version)) => {
                                    (name.to_owned(), Some(version.to_owned()))
                                }
                                None => (tool.to_owned(), None),
                            }));
                        }
                    } else if lowered.starts_with("release-plz/action@") {
                        // The action installs release-plz itself, at its `version:` input or, when
                        // that is absent, at a default that moves with the action's ref. It is
                        // the binary that writes the published CHANGELOG, so it must be the one
                        // `mise run release-preview` previews with (#190).
                        events.push(Event::Install((
                            "release-plz".to_owned(),
                            step["with"]["version"].as_str().map(str::to_owned),
                        )));
                    }
                }
                if let Some(run) = step["run"].as_str() {
                    for line in run.lines() {
                        for command in shell_commands(line) {
                            // One simple command re-joined parses back to itself alone.
                            let command = command.join(" ");
                            events.extend(cargo_installs(&command).into_iter().map(Event::Install));
                            events.extend(tools_run(&command, &pins).into_iter().map(Event::Run));
                        }
                    }
                }
                for event in events {
                    let (tool, version) = match event {
                        Event::Install(install) => install,
                        Event::Run(tool) => {
                            assert!(
                                in_job.contains(&tool),
                                "`{file}`'s `{job}` job runs `{tool}` without installing it \
                                 earlier in the same job; jobs share no runner, so it would run \
                                 whatever binary the runner carries rather than the {} `[tools]` \
                                 in mise.toml pins",
                                pins[&tool]
                            );
                            continue;
                        }
                    };
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
                    installed
                        .entry(tool.clone())
                        .or_default()
                        .insert(job.to_owned());
                    in_job.insert(tool);
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
fn the_release_preview_never_smudges_lfs_content() {
    // The preview clones this checkout, and `release-plz update` then checks out other revisions
    // and this branch again; each checkout smudges every LFS file that differs, fetching it from
    // the clone's `origin`, which is this checkout. CI's checkout holds no LFS objects, so a pull
    // request that added a corpus file failed the preview with "remote missing object" (#357)
    // while `GIT_LFS_SKIP_SMUDGE=1` covered only the clone. No packaged file is an LFS object, so
    // every command that checks files out skips it. The pairing test holds CI to the same lines.
    let tasks = mise_tasks();
    let task_env = mise_env(&tasks, "release-preview");
    let mut checked = Vec::new();
    for line in mise_commands(&tasks, "release-preview") {
        for segment in line.split("&&") {
            let words: Vec<&str> = segment.split_whitespace().collect();
            let program = words
                .iter()
                .position(|word| !word.contains('='))
                .unwrap_or(words.len());
            let (assignments, command) = words.split_at(program);
            let checks_out = match command {
                ["release-plz", ..] => true,
                ["git", rest @ ..] => rest.contains(&"clone"),
                _ => false,
            };
            if !checks_out {
                continue;
            }
            let skipped = assignments.contains(&"GIT_LFS_SKIP_SMUDGE=1")
                || task_env.get("GIT_LFS_SKIP_SMUDGE").map(String::as_str) == Some("1");
            assert!(
                skipped,
                "`mise run release-preview` runs `{}` without `GIT_LFS_SKIP_SMUDGE=1`: it would \
                 fetch LFS content from a checkout that holds none",
                segment.trim()
            );
            checked.push(command[0].to_owned());
        }
    }
    assert!(
        ["git", "release-plz"]
            .iter()
            .all(|program| checked.iter().any(|seen| seen == program)),
        "the release preview no longer clones and runs release-plz ({checked:?}); revisit this test"
    );
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
    // in. Two assertions over CI configuration sat here for several commits while the only row
    // naming this file described the corpus manifest (#230), so a broken gate was reported by a
    // suite that row gave nobody debugging it a reason to read, and the next such assertion had
    // no documented home. The rows whose suite is this file name each of its tests, and each
    // name they cite is a test it defines, so neither side drifts from the other unseen.
    const SUITE: &str = "spargen/tests/corpus_manifest.rs";
    let source = read(SUITE);
    let mut defined = BTreeSet::new();
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
        defined.insert(name.to_owned());
    }
    assert!(
        defined.len() > 1,
        "found {defined:?} in {SUITE}; the scan is not finding its tests"
    );

    let claude = read("CLAUDE.md");
    let mut rows = 0usize;
    let mut named = BTreeSet::new();
    for line in claude.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| subsystem | suite | what to cover |` splits into five cells, the outer two empty.
        let [_, _, suite, cover, _] = cells.as_slice() else {
            continue;
        };
        if !suite.contains(&format!("`{SUITE}`")) {
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
        "no row of CLAUDE.md's testing-strategy table names `{SUITE}` as its suite"
    );
    let unnamed: Vec<_> = defined.difference(&named).collect();
    assert!(
        unnamed.is_empty(),
        "{SUITE} defines {unnamed:?}, which no testing-strategy row naming it as the suite \
         mentions: add each to the row whose subject it tests, or give it a row of its own"
    );
    let stale: Vec<_> = named.difference(&defined).collect();
    assert!(
        stale.is_empty(),
        "CLAUDE.md's testing-strategy rows for {SUITE} cite {stale:?}, which it does not define"
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
