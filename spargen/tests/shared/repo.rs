//! This repository's own gate configuration (`mise.toml` and `.github/workflows/`) through one set
//! of readers, shared by `corpus_manifest.rs`, which holds the corpus-smoke gate to the manifest,
//! and `repo_gates/`, which holds every other gate. Each integration test is its own crate, so
//! each declares this file with `#[path]`.

// Each test crate that declares this module reads the parts its checks need.
#![allow(dead_code)]

use std::collections::BTreeMap;

use camino::Utf8PathBuf;

/// The workspace root, one level above this crate.
pub fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .to_owned()
}

/// The file at `relative` to the workspace root, which must be readable.
pub fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path} must be readable: {error}"))
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
pub fn mise_tasks() -> toml::Table {
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
pub fn mise_commands(tasks: &toml::Table, name: &str) -> Vec<String> {
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
pub fn mise_env(tasks: &toml::Table, name: &str) -> BTreeMap<String, String> {
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
pub fn yaml_document(text: &str, whose: &str) -> yaml_rust2::Yaml {
    let mut documents = yaml_rust2::YamlLoader::load_from_str(text)
        .unwrap_or_else(|error| panic!("{whose} must parse as YAML: {error}"));
    assert!(!documents.is_empty(), "{whose} must carry a YAML document");
    documents.swap_remove(0)
}

/// `.github/workflows/<file>`, parsed.
pub fn workflow(file: &str) -> yaml_rust2::Yaml {
    let path = format!(".github/workflows/{file}");
    yaml_document(&read(&path), &format!("`{path}`"))
}

/// `.github/workflows/ci.yml`, parsed.
pub fn ci_workflow() -> yaml_rust2::Yaml {
    workflow("ci.yml")
}
