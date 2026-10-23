//! The Rust toolchain every gate runs on and the tool versions CI installs, each held to the pin
//! `rust-toolchain.toml`, the workspace `rust-version`, or `mise.toml`'s `[tools]` states.

use std::collections::{BTreeMap, BTreeSet};

use crate::repo::{ci_workflow, mise_commands, mise_tasks, read, workflow};
use crate::{workflow_files, workflow_jobs};

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
        for (job, steps) in workflow_jobs(&file, &workflow) {
            for step in steps {
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
        for (job, steps) in workflow_jobs(&file, &workflow) {
            // What this job has installed so far: a tool a job runs must be installed by that
            // job, earlier, since jobs share no runner. A pin installed by some other job would
            // pass a workflow-wide check while this job ran whatever the runner image carries.
            let mut in_job = BTreeSet::new();
            for step in steps {
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
