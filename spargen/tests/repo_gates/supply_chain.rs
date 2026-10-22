//! The supply-chain audit's scope: what `mise run deny` and `mise run deny-published` audit, the
//! lockfiles each reads, the flags each may pass, and the advisory floors `deny.toml` mirrors.

use std::collections::{BTreeMap, BTreeSet};

use crate::repo::{mise_commands, mise_env, mise_tasks, read, workspace_root};

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

/// The only flags the `cargo fetch` before a lockfile audit may pass; an allow-list like the two
/// above. cargo-audit 0.22.2 updates the crates.io index only when it also fetches the database,
/// so under `--no-fetch` its yank check reads Cargo's local sparse-index cache and nothing else;
/// on a runner whose cache starts empty it printed "couldn't check if the package is yanked" for
/// every entry and exited 0, and spargen 0.5.0's shipped lockfile passed `deny-published` with a
/// yanked `chacha20 0.10.1` in it (#414). `cargo fetch` resolves the lockfile against the registry
/// and so refreshes that cache for every registry entry in it, failing where it cannot reach the
/// registry. `--offline`, `--frozen` and `-Z no-index-update` would read the stale cache instead,
/// and `--target` would refresh one platform's entries only; each is rejected with every other
/// word.
const INDEX_REFRESH_FLAGS: [(&str, bool); 2] = [("--locked", false), ("--manifest-path", true)];

/// What follows every lockfile audit's flags, byte for byte. Under `--no-fetch` an entry whose
/// yank status cargo-audit cannot read is an `error:` line on stderr and exit status 0 (#414), as
/// is an index it cannot open (a `warning:`); with `--deny warnings` a clean audit prints neither,
/// so the guard fails an audit that exited 0 but printed either. Stderr goes to a file in the
/// gitignored `target/` and is printed whether the audit passed or failed, so the log loses
/// nothing; `|| { …; exit 1; }` keeps a failing audit failing under both `sh -c -o errexit` (mise)
/// and `bash -e` (CI), neither of which sets `pipefail`.
const AUDIT_GUARD: &str = "2>target/cargo-audit.stderr || { cat target/cargo-audit.stderr >&2; \
     exit 1; }; cat target/cargo-audit.stderr >&2; if grep -qE 'error|warning' \
     target/cargo-audit.stderr; then echo 'cargo audit exited 0 but printed an error or warning \
     above, so it did not check every entry' >&2; exit 1; fi";

/// The words of a lockfile audit `command` before its `AUDIT_GUARD`, which must end it exactly.
fn guarded_audit_words(command: &str) -> Result<Vec<&str>, String> {
    let (audit, guard) = command
        .split_once(" 2>")
        .ok_or_else(|| format!("`{command}` does not send its stderr through the audit guard"))?;
    if format!("2>{guard}") != AUDIT_GUARD {
        return Err(format!(
            "`{command}` ends in `2>{guard}`, not the audit guard `{AUDIT_GUARD}`"
        ));
    }
    Ok(audit.split_whitespace().collect())
}

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

/// What [`deny_check`] reads from a command's words.
type DenyCheck<'w, 'a> = Option<Option<(&'w [&'a str], &'w [&'a str])>>;

/// The words of a cargo-deny command split at its `check`, as `(globals, checks)`: the words
/// between `cargo deny` (or `cargo-deny`) and `check`, and the words after it. The outer `None`
/// is a command that does not run cargo-deny; the inner one, a cargo-deny command with no `check`.
fn deny_check<'w, 'a>(words: &'w [&'a str]) -> DenyCheck<'w, 'a> {
    let skip = if words.starts_with(&["cargo", "deny"]) {
        2
    } else if words.first() == Some(&"cargo-deny") {
        1
    } else {
        return None;
    };
    let check = words.iter().position(|word| *word == "check");
    Some(check.map(|check| (&words[skip..check], &words[check + 1..])))
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
        let Some(check) = deny_check(&words) else {
            panic!(
                "`mise run deny` runs `{command}`, which is not a cargo-deny audit; the gate runs \
                 nothing else"
            );
        };
        let (globals, which) = check
            .unwrap_or_else(|| panic!("`mise run deny` runs `{command}`, which is not `check`"));

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
    for narrowing in [
        "--offline",
        "--frozen",
        "-Z no-index-update",
        "--target x86_64-unknown-linux-gnu",
        "--manifest-path",
    ] {
        let line = format!("--locked {narrowing}");
        let flags: Vec<&str> = line.split_whitespace().collect();
        assert!(
            allowed_flags(&flags, &INDEX_REFRESH_FLAGS).is_err(),
            "`cargo fetch {line}` passes `allowed_flags`, so the lockfile audit test admits it"
        );
    }

    // The guard: the audit's own words come back, and an audit that drops, discards, or softens
    // the guard is rejected (#414).
    let audit = "cargo audit --db target/db --no-fetch --deny warnings --file Cargo.lock";
    assert_eq!(
        guarded_audit_words(&format!("{audit} {AUDIT_GUARD}")),
        Ok(audit.split_whitespace().collect::<Vec<_>>())
    );
    for unguarded in [
        audit.to_owned(),
        format!("{audit} 2>/dev/null"),
        format!("{audit} {AUDIT_GUARD} || true"),
        format!("{audit} {}", AUDIT_GUARD.replacen("exit 1", "exit 0", 1)),
        format!(
            "{audit} {}",
            AUDIT_GUARD.replace("error|warning", "nothing")
        ),
    ] {
        assert!(
            guarded_audit_words(&unguarded).is_err(),
            "`{unguarded}` passes `guarded_audit_words`, so the lockfile audit test admits it"
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
    // `spargen` ships the `cli` binary, so a scan that misses it is not reading the workspace, and
    // an empty set would equal an audit task that downloads nothing.
    assert!(
        binaries.contains("spargen"),
        "found the published binary crates {binaries:?}, which omit `spargen` and its `cli` \
         binary: the workspace scan has stopped reading the members"
    );
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
        if lockfile_audit_step(&words).is_some() {
            continue;
        }
        let Some(check) = deny_check(&words) else {
            continue;
        };
        let (globals, which) =
            check.unwrap_or_else(|| panic!("`{command}` is not a `cargo deny check`"));
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
    /// `cargo fetch …`: refresh the index cache an audit's yank check reads (#414).
    IndexRefresh,
    /// `cargo audit …`: audit every entry of one lockfile.
    Audit,
}

/// Which lockfile-audit step a command's words are, if any.
fn lockfile_audit_step<'a>(words: &[&'a str]) -> Option<LockfileAuditStep<'a>> {
    if words.starts_with(&["cargo", "deny", "fetch"]) {
        Some(LockfileAuditStep::Fetch)
    } else if words.starts_with(&["cargo", "fetch"]) {
        Some(LockfileAuditStep::IndexRefresh)
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
/// every `cargo audit`, each over that checkout without fetching, denying every warning kind,
/// ignoring exactly what `deny.toml` ignores, ending in `AUDIT_GUARD`, and preceded by a `cargo
/// fetch` of the workspace that lockfile belongs to (`--locked` outside `examples/`, as that
/// workspace's `cargo deny check` is).
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
    // The lockfile of each workspace a `cargo fetch` refreshed, with whether it passed `--locked`.
    let mut refreshed: BTreeMap<String, bool> = BTreeMap::new();
    for command in mise_commands(&tasks, task) {
        let words: Vec<&str> = command.split_whitespace().collect();
        match lockfile_audit_step(&words) {
            None => {}
            Some(LockfileAuditStep::IndexRefresh) => {
                let flags =
                    allowed_flags(&words[2..], &INDEX_REFRESH_FLAGS).unwrap_or_else(|error| {
                        panic!(
                            "`{command}`: {error}. The index refresh passes only the flags \
                             `INDEX_REFRESH_FLAGS` allows, {INDEX_REFRESH_FLAGS:?} (#414)"
                        )
                    });
                let count = |flag: &str| flags.iter().filter(|(name, _)| *name == flag).count();
                assert!(
                    count("--locked") <= 1 && count("--manifest-path") <= 1,
                    "`{command}` passes a flag more than once"
                );
                let manifest = flags
                    .iter()
                    .find_map(|(name, value)| (*name == "--manifest-path").then_some(*value))
                    .flatten()
                    .unwrap_or("Cargo.toml")
                    .trim_start_matches("./");
                let lockfile = manifest
                    .strip_suffix("Cargo.toml")
                    .filter(|dir| dir.is_empty() || dir.ends_with('/'))
                    .map(|dir| format!("{dir}Cargo.lock"))
                    .unwrap_or_else(|| panic!("`{command}` names `{manifest}`, not a Cargo.toml"));
                assert!(
                    refreshed.insert(lockfile, count("--locked") == 1).is_none(),
                    "`mise run {task}` refreshes the index for `{manifest}` twice"
                );
            }
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
                let words = guarded_audit_words(&command).unwrap_or_else(|error| {
                    panic!(
                        "{error}. Under `--no-fetch` an unreadable yank status is an `error:` \
                         line and exit status 0, so an audit without the guard passes a lockfile \
                         it did not check (#414)"
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
                let locked = refreshed.get(file).copied().unwrap_or_else(|| {
                    panic!(
                        "`mise run {task}` audits `{file}` without a `cargo fetch` of its \
                         workspace before it, so the yank check reads whatever index cache the \
                         machine has, and none on a fresh runner (#414)"
                    )
                });
                assert_eq!(
                    locked,
                    !file.starts_with("examples/"),
                    "`mise run {task}` refreshes the index for `{file}` with `--locked` {}; the \
                     refresh is locked exactly where that workspace's `cargo deny check` is, so \
                     it resolves the lockfile the audit reads",
                    if locked { "set" } else { "unset" }
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
