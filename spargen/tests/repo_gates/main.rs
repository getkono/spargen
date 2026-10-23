//! This repository's assertions over its own gate configuration: `mise.toml`,
//! `.github/workflows/`, `rust-toolchain.toml`, `deny.toml`, and CLAUDE.md. Each module holds one
//! kind of gate: `supply_chain` the advisory audits, `pairing` each CI job against its mise tasks,
//! `toolchain` the Rust toolchain and tool pins, and `claude_md` CLAUDE.md's Quality list and
//! testing-strategy table. This file holds the workflow readers more than one of them uses, and
//! `shared/repo.rs` the `mise.toml` and file readers this suite shares with `corpus_manifest.rs`.
//!
//! CLAUDE.md's "repository gates" row names this suite and every test in it
//! (`the_testing_strategy_table_names_every_test_in_its_suite`).

use std::collections::BTreeSet;

#[path = "../shared/repo.rs"]
mod repo;

mod claude_md;
mod pairing;
mod supply_chain;
mod toolchain;

use repo::workspace_root;

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

/// Every job of `workflow` (the parsed `.github/workflows/<file>`) with its steps. A scan over
/// steps that skipped a workflow without `jobs`, or a job without `steps`, would pass over it
/// unseen, so each must carry at least one of each; a job of another shape (a reusable-workflow
/// `uses:` job) fails here until the scans that read this learn to follow it.
fn workflow_jobs<'a>(
    file: &str,
    workflow: &'a yaml_rust2::Yaml,
) -> Vec<(&'a str, &'a Vec<yaml_rust2::Yaml>)> {
    let jobs = workflow["jobs"]
        .as_hash()
        .unwrap_or_else(|| panic!("`{file}` carries no `jobs:` map"));
    assert!(!jobs.is_empty(), "`{file}` declares no job");
    jobs.iter()
        .map(|(job, body)| {
            let job = job
                .as_str()
                .unwrap_or_else(|| panic!("`{file}` names a job with a non-string key"));
            let steps = body["steps"]
                .as_vec()
                .filter(|steps| !steps.is_empty())
                .unwrap_or_else(|| {
                    panic!("`{file}`'s `{job}` job carries no `steps:` for this suite to read")
                });
            (job, steps)
        })
        .collect()
}
