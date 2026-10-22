//! The subsystem layering DAG: each subsystem's `//! layer-deps:` header against the `crate::`
//! edges it takes, the DAG table in `lib.rs`, and the CLI's facade-only header.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{read, rust_files, workspace_root};

/// The subsystems that are modules of the library, in the order `lib.rs` documents them. `cli` is
/// deliberately absent: it has no `mod.rs` and is `#[path]`-included into the binary, so `crate::`
/// inside it means the binary crate, not the library. Its header is checked separately.
const SUBSYSTEMS: &[&str] = &[
    "diag", "source", "ir", "oas31", "name", "support", "codegen", "emit", "compat", "surface",
];

/// Facade plumbing: named in `lib.rs` as explicitly *not* subsystems, and therefore never a legal
/// dependency of one. A subsystem reaching into these inverts the layering.
const FACADE_PLUMBING: &[&str] = &["cache", "config", "runtime_contract"];

/// The `#[cfg(test)]` modules `lib.rs` declares for the in-crate tests of every subsystem: shared
/// test helpers, compiled only into the test build. Neither a subsystem nor facade plumbing, so a
/// subsystem's test code reaching one takes no layering edge; each is admitted by name here.
const TEST_SUPPORT: &[&str] = &["test_support"];

/// Every `.rs` file belonging to a subsystem. `support/runtime/` is excluded: those entries are
/// symlinks to the `support-runtime` crate's sources, reached only through `include_str!` and never
/// compiled as spargen modules, so their `crate::` paths name the *other* crate's root.
fn subsystem_files(root: &Path, subsystem: &str) -> Vec<PathBuf> {
    rust_files(&root.join("spargen/src").join(subsystem), &["runtime"])
}

/// The modules `lib.rs` declares with `mod <name>;` (any visibility, any attributes), in order.
fn library_modules(lib: &str) -> Vec<String> {
    lib.lines()
        .filter_map(|line| {
            let rest = line
                .trim()
                .strip_prefix("pub(crate) ")
                .unwrap_or(line.trim());
            let rest = rest.strip_prefix("pub ").unwrap_or(rest);
            rest.strip_prefix("mod ")?
                .strip_suffix(';')
                .map(str::to_owned)
        })
        .collect()
}

/// Every module of the library is a subsystem, facade plumbing, or test support, each by name, so
/// a module `lib.rs` adds is classified before a subsystem can reach it unseen: `edges` counts
/// only the first two kinds.
#[test]
fn every_library_module_is_a_subsystem_plumbing_or_test_support() {
    let lib = read(&workspace_root().join("spargen/src/lib.rs"));
    let declared: BTreeSet<String> = library_modules(&lib).into_iter().collect();
    let classified: BTreeSet<String> = SUBSYSTEMS
        .iter()
        .chain(FACADE_PLUMBING)
        .chain(TEST_SUPPORT)
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        declared, classified,
        "the modules lib.rs declares and `SUBSYSTEMS` + `FACADE_PLUMBING` + `TEST_SUPPORT` have \
         drifted: classify each module by name in exactly one of the lists"
    );
}

/// The `//! layer-deps:` header of a module, as a set of declared subsystem names.
fn declared_deps(source: &str, path: &Path) -> BTreeSet<String> {
    let line = source
        .lines()
        .find(|line| line.trim_start().starts_with("//! layer-deps:"))
        .unwrap_or_else(|| panic!("{path:?} has no `//! layer-deps:` header"));
    let (_, list) = line
        .split_once("layer-deps:")
        .expect("the marker is present");
    list.split(',')
        .map(|dep| dep.trim().trim_matches('`').to_owned())
        .filter(|dep| !dep.is_empty())
        .collect()
}

/// The first path segment after every `root::` in `tokens`, at any depth. A grouped
/// `root::{a, b::c}` yields the head of each member (`a`, `b`). Lexing with `proc_macro2` rather
/// than scanning lines means a comment is never read (a rustdoc intra-doc link such as
/// `[`crate::name`]` is prose, not an edge: its text becomes a `#[doc]` string literal), and a
/// path rustfmt wraps across lines is still one path.
///
/// A string literal inside an attribute other than `#[doc]` is read too, because attributes such
/// as `#[serde(with = "crate::source::f")]` name a path as a string: where it lexes as Rust, its
/// paths are edges like any other. `in_attr` says the tokens sit inside such an attribute. A
/// string outside every attribute (`let _ = "crate::x"`) and a `#[doc]` string stay prose.
fn path_heads(
    tokens: proc_macro2::TokenStream,
    root: &str,
    in_attr: bool,
    out: &mut BTreeSet<String>,
) {
    use proc_macro2::{Delimiter, TokenTree};
    let is_punct = |tree: Option<&TokenTree>, ch: char| matches!(tree, Some(TokenTree::Punct(punct)) if punct.as_char() == ch);
    let is_colon = |tree: Option<&TokenTree>| is_punct(tree, ':');
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (at, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(group) => {
                // `#[...]`, or `#![...]` for an inner attribute.
                let opens_attr = group.delimiter() == Delimiter::Bracket
                    && at > 0
                    && (is_punct(trees.get(at - 1), '#')
                        || (at > 1
                            && is_punct(trees.get(at - 1), '!')
                            && is_punct(trees.get(at - 2), '#')));
                if opens_attr {
                    let is_doc = matches!(
                        group.stream().into_iter().next(),
                        Some(TokenTree::Ident(name)) if name == "doc"
                    );
                    if !is_doc {
                        path_heads(group.stream(), root, true, out);
                    }
                } else {
                    path_heads(group.stream(), root, in_attr, out);
                }
            }
            TokenTree::Literal(literal) if in_attr => {
                let Ok(text) =
                    syn::parse2::<syn::LitStr>(TokenTree::Literal(literal.clone()).into())
                else {
                    continue;
                };
                if let Ok(inner) = text.value().parse::<proc_macro2::TokenStream>() {
                    path_heads(inner, root, true, out);
                }
            }
            TokenTree::Ident(ident)
                if *ident == root && is_colon(trees.get(at + 1)) && is_colon(trees.get(at + 2)) =>
            {
                match trees.get(at + 3) {
                    Some(TokenTree::Ident(head)) => {
                        out.insert(head.to_string());
                    }
                    Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
                        let members: Vec<TokenTree> = group.stream().into_iter().collect();
                        for member in members.split(
                            |tree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ','),
                        ) {
                            if let Some(TokenTree::Ident(head)) = member.first() {
                                out.insert(head.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// The first segment of every `root::` path in `source`, lexed as Rust.
fn source_path_heads(source: &str, root: &str, path: &Path) -> BTreeSet<String> {
    let tokens: proc_macro2::TokenStream = source
        .parse()
        .unwrap_or_else(|error| panic!("{path:?} must lex as Rust: {error}"));
    let mut heads = BTreeSet::new();
    path_heads(tokens, root, false, &mut heads);
    heads
}

/// The inter-subsystem edges a file actually takes: every `crate::` path whose first segment is
/// another subsystem or facade plumbing.
fn edges(source: &str, own: &str, path: &Path) -> BTreeSet<String> {
    source_path_heads(source, "crate", path)
        .into_iter()
        .filter(|ident| {
            ident != own
                && (SUBSYSTEMS.contains(&ident.as_str())
                    || FACADE_PLUMBING.contains(&ident.as_str()))
        })
        .collect()
}

/// The edge reader's handling of the path shapes a line scan got wrong or never saw: a grouped
/// `use`, a nested group, a path wrapped across lines, a path-valued attribute string, and prose
/// that only cites a path.
#[test]
fn the_edge_reader_reads_grouped_paths_and_skips_comments() {
    let source = r#"
//! Cites [`crate::emit`] in prose.
use crate::{ir, diag::{Code, Diagnostic}};
use crate::{
    name::Scope,
    self as root,
};
// crate::compat is only mentioned here.
/* crate::surface, in a block comment */
/// A doc comment naming `crate::cache`.
#[doc = "an explicit doc attribute naming crate::config"]
struct Report {
    #[serde(serialize_with = "crate::surface::write")]
    path: String,
    #[serde(rename = "not a path {")]
    other: String,
}
fn wrapped() -> crate
    ::source::Spec {
    let _ = "crate::support";
    pub(crate) fn inner() {}
    crate::codegen::run()
}
"#;
    assert_eq!(
        edges(source, "codegen", Path::new("fixture.rs")),
        BTreeSet::from(["diag", "ir", "name", "source", "surface"].map(str::to_owned)),
        "`self` is not a subsystem, `codegen` is the file's own, comments, docs and string \
         literals outside attributes take no edge, and a path-valued string in a non-doc \
         attribute does"
    );
}

/// The DAG table in the `lib.rs` module docs, as `subsystem -> allowed dependencies`. The table is
/// the human-readable contract; the headers are the machine-readable one, and they must agree.
fn dag_table(lib: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut table = BTreeMap::new();
    for line in lib.lines() {
        let Some(row) = line.trim_start().strip_prefix("//! |") else {
            continue;
        };
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        let [subsystem, allowed, ..] = cells.as_slice() else {
            continue;
        };
        let subsystem = subsystem.trim_matches('`');
        if !SUBSYSTEMS.contains(&subsystem) {
            continue;
        }
        let allowed = allowed
            .split(',')
            .map(|dep| dep.trim().trim_matches('`').to_owned())
            .filter(|dep| SUBSYSTEMS.contains(&dep.as_str()))
            .collect();
        table.insert(subsystem.to_owned(), allowed);
    }
    table
}

#[test]
fn every_subsystem_declares_the_dependencies_it_actually_takes() {
    let root = workspace_root();
    let mut total_edges = 0;
    let mut tracked_seen = BTreeSet::new();
    for subsystem in SUBSYSTEMS {
        let module = root.join("spargen/src").join(subsystem).join("mod.rs");
        let declared = declared_deps(&read(&module), &module);

        let mut taken: BTreeSet<String> = BTreeSet::new();
        for file in subsystem_files(&root, subsystem) {
            taken.extend(edges(&read(&file), subsystem, &file));
        }
        total_edges += taken.len();

        let undeclared: Vec<&String> = taken.difference(&declared).collect();
        assert!(
            undeclared.is_empty(),
            "subsystem `{subsystem}` reaches {undeclared:?} but its `//! layer-deps:` header \
             declares only {declared:?} — declare the edge (and add it to the DAG table in \
             lib.rs), or stop taking it"
        );
        let unused: Vec<&String> = declared
            .difference(&taken)
            .filter(|dep| {
                let tracked = OVER_DECLARED_TRACKED
                    .iter()
                    .any(|(module, edge, _)| module == subsystem && edge == dep);
                if tracked {
                    tracked_seen.insert((*subsystem, dep.as_str().to_owned()));
                }
                !tracked
            })
            .collect();
        assert!(
            unused.is_empty(),
            "subsystem `{subsystem}` declares {unused:?} in its `//! layer-deps:` header but takes \
             no such edge — drop it from the header and the DAG table in lib.rs, or the header \
             no longer describes the module"
        );

        for plumbing in FACADE_PLUMBING {
            assert!(
                !taken.contains(*plumbing),
                "subsystem `{subsystem}` reaches facade plumbing `{plumbing}`, inverting the \
                 layering"
            );
        }
    }
    assert!(
        total_edges > 0,
        "no subsystem takes any `crate::` edge, so the edge reader has stopped reading the sources"
    );
    let stale: Vec<_> = OVER_DECLARED_TRACKED
        .iter()
        .filter(|(module, edge, _)| !tracked_seen.contains(&(*module, (*edge).to_owned())))
        .collect();
    assert!(
        stale.is_empty(),
        "these tracked over-declarations are no longer declared-but-untaken; remove them: \
         {stale:?}"
    );
}

/// Declared `//! layer-deps:` edges a subsystem does not take, as `(subsystem, edge, issue)`, each
/// with the issue that removes it. An entry that stops being needed fails the test, so this only
/// shrinks.
const OVER_DECLARED_TRACKED: &[(&str, &str, &str)] = &[];

/// The subsystem directories under `spargen/src/`: each is a library subsystem in `SUBSYSTEMS`,
/// or `cli` (header checked by `the_cli_declares_its_dependency_on_the_facade`), or
/// `runtime_contract`, facade plumbing (in `FACADE_PLUMBING`) split into a directory module, or
/// `bin`, which holds the binary's entry point and is no subsystem.
#[test]
fn every_source_directory_is_a_declared_subsystem() {
    let src = workspace_root().join("spargen/src");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&src)
        .expect("spargen/src exists")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.is_dir())
        .map(|path| {
            path.file_name()
                .expect("a directory has a name")
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != "bin")
        .collect();
    let declared: BTreeSet<String> = SUBSYSTEMS
        .iter()
        .chain(&["cli", "runtime_contract"])
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        on_disk, declared,
        "the directories under spargen/src/ and `SUBSYSTEMS` (plus `cli` and \
         `runtime_contract`) have drifted: a \
         subsystem missing from the list has its `//! layer-deps:` header and edges checked by \
         nothing"
    );
}

#[test]
fn the_declared_headers_agree_with_the_dag_table_in_lib_rs() {
    let root = workspace_root();
    let table = dag_table(&read(&root.join("spargen/src/lib.rs")));

    for subsystem in SUBSYSTEMS {
        let module = root.join("spargen/src").join(subsystem).join("mod.rs");
        let declared = declared_deps(&read(&module), &module);
        let documented = table
            .get(*subsystem)
            .unwrap_or_else(|| panic!("the lib.rs DAG table has no row for `{subsystem}`"));
        assert_eq!(
            &declared, documented,
            "`{subsystem}` declares {declared:?} in its header but the lib.rs DAG table says \
             {documented:?}"
        );
    }
}

#[test]
fn the_cli_declares_its_dependency_on_the_facade() {
    // `cli` has no `mod.rs`; its header rides on `run.rs`, and it depends on the facade rather
    // than on any subsystem. CLAUDE.md still lists it as a subsystem, so the header must exist.
    // Inside `cli`, `crate::` names the binary crate, so the facade edge is a `spargen::` path; a
    // `crate::<subsystem>` path there would name a module the binary does not have.
    let root = workspace_root();
    let run = root.join("spargen/src/cli/run.rs");
    let declared = declared_deps(&read(&run), &run);
    assert_eq!(
        declared,
        BTreeSet::from(["facade".to_owned()]),
        "spargen/src/cli/run.rs must declare exactly `//! layer-deps: facade`"
    );

    let mut taken = BTreeSet::new();
    for file in subsystem_files(&root, "cli") {
        let source = read(&file);
        if !source_path_heads(&source, "spargen", &file).is_empty() {
            taken.insert("facade".to_owned());
        }
        taken.extend(edges(&source, "cli", &file));
    }
    assert_eq!(
        taken, declared,
        "the `cli` files take {taken:?}, and run.rs declares {declared:?}: the CLI reaches the \
         library through the `spargen::` facade and nothing else"
    );
}
