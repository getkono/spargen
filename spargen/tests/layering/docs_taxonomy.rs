//! The generated client's error taxonomy against the Markdown that describes it: every
//! `Error::X` and `RequestError::X` a document cites is a variant `support-runtime` declares, and
//! the documents that lay out the taxonomy name every one. The declared names are read from
//! `support-runtime/src/error.rs` with the counter
//! `every_error_variant_is_counted_by_its_enumeration` holds to the in-crate enumerations.

use std::collections::BTreeSet;
use std::path::Path;

use crate::{enum_variants, read, workspace_root};

/// The documents that describe the error taxonomy to a consumer as a whole, so each must name
/// every variant of both enums. The book's Generated API page is the one account the Getting
/// Started page points to.
const TAXONOMY_DOCUMENTS: [&str; 1] = ["docs/book/src/generated-api.md"];

/// Documents that cite individual variants without describing the whole taxonomy. They are
/// held only to citing variants that exist, which every scanned document is; listing them here
/// makes the scan prove it reached them.
const CITING_DOCUMENTS: [&str; 2] = ["docs/support-matrix.md", "docs/book/src/getting-started.md"];

/// The variant names of `Error` and of `RequestError`, read from their declarations.
fn declared_variants() -> (BTreeSet<String>, BTreeSet<String>) {
    let source = read(&workspace_root().join("support-runtime/src/error.rs"));
    (
        enum_variants(&source, "pub enum Error<E> {")
            .into_iter()
            .collect(),
        enum_variants(&source, "pub enum RequestError {")
            .into_iter()
            .collect(),
    )
}

/// Every `Error::Name` and `RequestError::Name` spelled in `text`, as (line, enum, variant).
/// Only a capitalised member is a variant, so `Error::status()` and the other methods are not
/// read. A path the name continues (`io::Error::Other`) or extends (`StreamError::…`) names
/// some other type, and is skipped.
fn cited_variants(text: &str) -> Vec<(usize, &'static str, String)> {
    let continues = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == ':';
    let mut cited = Vec::new();
    for (number, line) in text.lines().enumerate() {
        for (at, marker) in line.match_indices("Error::") {
            let (enumeration, before) = match line[..at].strip_suffix("Request") {
                Some(before) => ("RequestError", before),
                None => ("Error", &line[..at]),
            };
            if before.ends_with(continues) {
                continue;
            }
            let name: String = line[at + marker.len()..]
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if name.starts_with(|ch: char| ch.is_ascii_uppercase()) {
                cited.push((number + 1, enumeration, name));
            }
        }
    }
    cited
}

#[test]
fn the_citation_reader_reads_variants_of_these_two_enums_only() {
    let text = "`Error::Api` and `RequestError::Other`, but not `Error::status()`,\n\
                `io::Error::Other`, `StreamError::Decode`, or `MyRequestError::Gone`.\n\
                (`Error::UnexpectedStatus { .. }`)";
    assert_eq!(
        cited_variants(text),
        [
            (1, "Error", "Api".to_owned()),
            (1, "RequestError", "Other".to_owned()),
            (3, "Error", "UnexpectedStatus".to_owned()),
        ]
    );
}

/// Every Markdown file under `root`, root-relative with `/` separators, sorted. Skipped:
/// hidden and `target` directories, the vendored specification texts under `references/`,
/// and `CHANGELOG.md`, whose entries name variants as they were at each release.
fn repository_markdown(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            let name = path
                .file_name()
                .expect("a directory entry has a name")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                if !(name.starts_with('.') || name == "target" || name == "references") {
                    walk(root, &path, found);
                }
            } else if name.ends_with(".md") && name != "CHANGELOG.md" {
                let relative = path
                    .strip_prefix(root)
                    .expect("the walk stays under the root");
                let parts: Vec<_> = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// A document that names `Error::X` or `RequestError::X` is describing the generated client's
/// public error surface, and no other gate reads it: the `docs/` checks in spargen hold
/// diagnostic codes only. So every such citation in the repository's Markdown must name a
/// variant that exists — a variant renamed or removed in the runtime fails until every
/// document stops naming it.
#[test]
fn every_error_variant_a_document_cites_exists() {
    let (errors, requests) = declared_variants();
    let root = workspace_root();
    let documents = repository_markdown(&root);
    for expected in TAXONOMY_DOCUMENTS.iter().chain(&CITING_DOCUMENTS) {
        assert!(
            documents.iter().any(|document| document == expected),
            "`{expected}` is not among the scanned documents {documents:?}"
        );
    }

    let mut stale = Vec::new();
    for document in &documents {
        let text = read(&root.join(document));
        for (line, enumeration, name) in cited_variants(&text) {
            let declared = if enumeration == "Error" {
                &errors
            } else {
                &requests
            };
            if !declared.contains(&name) {
                stale.push(format!("{document}:{line}: `{enumeration}::{name}`"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "these documents cite error variants that do not exist (`Error` has {errors:?}, \
         `RequestError` has {requests:?}):\n{}",
        stale.join("\n")
    );
}

/// The other direction: a variant added to either enum is a breaking change to every generated
/// client, so the documents that lay out the taxonomy must name it, fully qualified, before it
/// lands.
#[test]
fn the_taxonomy_documents_name_every_error_variant() {
    let (errors, requests) = declared_variants();
    let root = workspace_root();
    for document in TAXONOMY_DOCUMENTS {
        let text = read(&root.join(document));
        let cited: BTreeSet<(&str, String)> = cited_variants(&text)
            .into_iter()
            .map(|(_, enumeration, name)| (enumeration, name))
            .collect();
        let unnamed: Vec<String> = errors
            .iter()
            .map(|name| ("Error", name))
            .chain(requests.iter().map(|name| ("RequestError", name)))
            .filter(|(enumeration, name)| !cited.contains(&(*enumeration, (*name).clone())))
            .map(|(enumeration, name)| format!("`{enumeration}::{name}`"))
            .collect();
        assert!(
            unnamed.is_empty(),
            "{document} lays out the error taxonomy but never names {}: add each to its \
             error-taxonomy passage",
            unnamed.join(", ")
        );
    }
}
