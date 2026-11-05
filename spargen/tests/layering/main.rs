//! Structural invariants CLAUDE.md states but nothing checked: the subsystem layering DAG, the
//! shape of the embedded runtime sources, the file list that embeds them, that the published
//! crate carries every file its sources include, that every integration test spawning the
//! `cli`-gated binary is itself gated on `cli`, that no file outgrows the length baseline, and
//! that the Markdown describing the runtime's error taxonomy names exactly its variants.
//!
//! `lib.rs` promises that the declarations are diffed against the actual inter-module `use` edges.
//! This suite is where that happens: it needs no extra workspace member and runs under the
//! ordinary `test` gate.
//!
//! Each module holds one gate: `dag` the layering DAG, `runtime_embed` the embedded runtime
//! sources and their embed list, `packaging` the published crate's includes and the `cli` gate on
//! binary-spawning tests, `file_length` the length limit, and `docs_taxonomy` the error variants
//! the repository's Markdown cites. This file holds the readers they share.

use std::path::{Path, PathBuf};

mod dag;
mod docs_taxonomy;
mod file_length;
mod packaging;
mod runtime_embed;

#[path = "../shared/repo.rs"]
mod repo;

/// [`repo::workspace_root`] as the `std` path these gates walk with.
fn workspace_root() -> PathBuf {
    repo::workspace_root().into_std_path_buf()
}

/// [`repo::read_file`]: the file at `path`, which must be readable.
fn read(path: &Path) -> String {
    repo::read_file(path)
}

/// The names of the top-level variants of `header`'s enum body in `source`: every identifier that
/// opens a comma-separated item at the body's own depth. Line comments are dropped first, so doc
/// text cannot contribute a name, and a variant's attributes, fields and payload all sit one
/// bracket deeper than its name, so none of them does either.
fn enum_variants(source: &str, header: &str) -> Vec<String> {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("`{header}` is declared in support-runtime/src/error.rs"))
        + header.len();
    let body: String = source[start..]
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");

    let mut variants = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    // Whether the item being read has produced its name yet.
    let mut named = false;
    for ch in body.chars() {
        if depth == 0 && !named && (ch.is_alphanumeric() || ch == '_') {
            current.push(ch);
            continue;
        }
        if !current.is_empty() {
            variants.push(std::mem::take(&mut current));
            named = true;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => break,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => named = false,
            _ => {}
        }
    }
    variants
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
