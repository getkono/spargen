//! Structural invariants CLAUDE.md states but nothing checked: the subsystem layering DAG, the
//! shape of the embedded runtime sources, the file list that embeds them, that the published
//! crate carries every file its sources include, that every integration test spawning the
//! `cli`-gated binary is itself gated on `cli`, and that no file outgrows the length baseline.
//!
//! `lib.rs` promises that the declarations are diffed against the actual inter-module `use` edges.
//! This suite is where that happens: it needs no extra workspace member and runs under the
//! ordinary `test` gate.
//!
//! Each module holds one gate: `dag` the layering DAG, `runtime_embed` the embedded runtime
//! sources and their embed list, `packaging` the published crate's includes and the `cli` gate on
//! binary-spawning tests, and `file_length` the length limit. This file holds the readers they
//! share.

use std::path::{Path, PathBuf};

mod dag;
mod file_length;
mod packaging;
mod runtime_embed;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path:?} must be readable: {error}"))
}

/// Every `.rs` file under `dir`, recursively, sorted, skipping any directory named in `skip`.
fn rust_files(dir: &Path, skip: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("{dir:?} must be listable: {error}"));
        for entry in entries {
            let path = entry.expect("readable directory entry").path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| skip.iter().any(|skipped| name == *skipped))
                {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
