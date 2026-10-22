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

/// The file modules `lib.rs` declares, one per line as `mod <name>;`, `pub mod <name>;` or
/// `pub(crate) mod <name>;`, in order. An inline `mod <name> { … }` is not read.
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

/// Every file module `lib.rs` declares is a subsystem, facade plumbing, or test support, each by
/// name, so a file module `lib.rs` adds is classified before a subsystem can reach it unseen:
/// `edges` counts only the first two kinds.
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

/// Whether an attribute's tokens (the stream inside `#[…]`) are a `cfg` that holds only in a test
/// build: `cfg(test)`, or an `all(…)` with a test-only member (`cfg(all(test, feature = "x"))`).
/// `cfg(any(test, …))` and `cfg(not(test))` also hold outside a test build, so they are not.
fn is_test_only_cfg(attr: proc_macro2::TokenStream) -> bool {
    use proc_macro2::{Delimiter, TokenTree};
    fn predicate(tokens: &[TokenTree]) -> bool {
        match tokens {
            [TokenTree::Ident(name)] => name == "test",
            [TokenTree::Ident(all), TokenTree::Group(group)]
                if all == "all" && group.delimiter() == Delimiter::Parenthesis =>
            {
                let members: Vec<TokenTree> = group.stream().into_iter().collect();
                members
                    .split(|tree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ','))
                    .any(predicate)
            }
            _ => false,
        }
    }
    let trees: Vec<TokenTree> = attr.into_iter().collect();
    match trees.as_slice() {
        [TokenTree::Ident(cfg), TokenTree::Group(group)]
            if cfg == "cfg" && group.delimiter() == Delimiter::Parenthesis =>
        {
            predicate(&group.stream().into_iter().collect::<Vec<_>>())
        }
        _ => false,
    }
}

/// Where the edge reader stands: `root` is the identifier that names the crate root (`crate` in a
/// library subsystem, `spargen` for the CLI's facade paths), and `depth`, where it is known, is how
/// many modules below the library root the tokens sit, so a `super::` chain that climbs to the
/// root (`super::ir` in a subsystem's `mod.rs`) is a root path too. `in_attr` says the tokens sit
/// inside an attribute other than `#[doc]`.
#[derive(Clone, Copy)]
struct Reading<'a> {
    root: &'a str,
    depth: Option<usize>,
    in_attr: bool,
}

/// The first segment of the path whose remaining segments start at `tree`: an identifier, or `*`
/// for a glob, or for a grouped `{a, b::c}` the head of each member (`a`, `b`).
fn insert_heads(tree: Option<&proc_macro2::TokenTree>, out: &mut BTreeSet<String>) {
    use proc_macro2::{Delimiter, TokenTree};
    match tree {
        Some(TokenTree::Ident(head)) => {
            out.insert(head.to_string());
        }
        Some(TokenTree::Punct(punct)) if punct.as_char() == '*' => {
            out.insert("*".to_owned());
        }
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
            let members: Vec<TokenTree> = group.stream().into_iter().collect();
            for member in members
                .split(|tree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ','))
            {
                insert_heads(member.first(), out);
            }
        }
        _ => {}
    }
}

/// The first path segment after every `root::` in `tokens`, at any depth of nesting, and after
/// every `super::` chain that climbs to the library root. Lexing with `proc_macro2` rather than
/// scanning lines means a comment is never read (a rustdoc intra-doc link such as
/// `[`crate::name`]` is prose, not an edge: its text becomes a `#[doc]` string literal), and a
/// path rustfmt wraps across lines is still one path.
///
/// Only production code is read. An item behind an outer test-only `cfg` (see
/// [`is_test_only_cfg`]) is skipped through its end, the first `;`, `,` or `{…}` block after the
/// attribute, which ends every item shape the crate puts behind one (`mod tests { … }`,
/// `mod tests;`, an `impl` or `fn` block, a `use`) and a field or variant. Where one of the three
/// comes earlier inside the item (`impl<A, B>`), the rest of the item is read as production code,
/// so the skip errs toward reading too much, never toward hiding an edge outside the item. An
/// inner `#![cfg(test)]` skips the rest of its module. A file that only a test-only `mod name;` reaches is left out by
/// [`test_only_module_files`] instead.
///
/// A string literal inside an attribute other than `#[doc]` is read too, because attributes such
/// as `#[serde(with = "crate::source::f")]` name a path as a string: where it lexes as Rust, its
/// paths are edges like any other. A string outside every attribute (`let _ = "crate::x"`) and a
/// `#[doc]` string stay prose, and so does a path an `include_str!` or `include_bytes!` names: it
/// is a file, not a module, and `packaging.rs` holds those.
fn path_heads(tokens: proc_macro2::TokenStream, reading: Reading<'_>, out: &mut BTreeSet<String>) {
    use proc_macro2::{Delimiter, TokenTree};
    let is_punct = |tree: Option<&TokenTree>, ch: char| matches!(tree, Some(TokenTree::Punct(punct)) if punct.as_char() == ch);
    let is_ident = |tree: Option<&TokenTree>, name: &str| matches!(tree, Some(TokenTree::Ident(ident)) if ident == name);
    let is_path_sep = |trees: &[TokenTree], at: usize| {
        is_punct(trees.get(at), ':') && is_punct(trees.get(at + 1), ':')
    };
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut skipping = false;
    for (at, tree) in trees.iter().enumerate() {
        // `#[...]`, or `#![...]` for an inner attribute.
        let attr = match tree {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Bracket && at > 0 => {
                if is_punct(trees.get(at - 1), '#') {
                    Some((group, false))
                } else if at > 1
                    && is_punct(trees.get(at - 1), '!')
                    && is_punct(trees.get(at - 2), '#')
                {
                    Some((group, true))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((group, inner)) = attr {
            if is_test_only_cfg(group.stream()) {
                if inner {
                    return;
                }
                skipping = true;
                continue;
            }
        }
        if skipping {
            let ends_item = is_punct(Some(tree), ';')
                || is_punct(Some(tree), ',')
                || matches!(tree, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace);
            skipping = !ends_item;
            continue;
        }
        match tree {
            TokenTree::Group(group) => {
                if attr.is_some() {
                    let is_doc = matches!(
                        group.stream().into_iter().next(),
                        Some(TokenTree::Ident(name)) if name == "doc"
                    );
                    if !is_doc {
                        path_heads(
                            group.stream(),
                            Reading {
                                in_attr: true,
                                ..reading
                            },
                            out,
                        );
                    }
                } else {
                    // An inline `mod name { … }` is one module deeper below the root.
                    let opens_module = group.delimiter() == Delimiter::Brace
                        && at > 1
                        && matches!(trees.get(at - 1), Some(TokenTree::Ident(_)))
                        && is_ident(trees.get(at - 2), "mod");
                    let depth = if opens_module {
                        reading.depth.map(|depth| depth + 1)
                    } else {
                        reading.depth
                    };
                    path_heads(group.stream(), Reading { depth, ..reading }, out);
                }
            }
            TokenTree::Literal(literal) if reading.in_attr => {
                let Ok(text) =
                    syn::parse2::<syn::LitStr>(TokenTree::Literal(literal.clone()).into())
                else {
                    continue;
                };
                if let Ok(inner) = text.value().parse::<proc_macro2::TokenStream>() {
                    path_heads(inner, reading, out);
                }
            }
            TokenTree::Ident(ident) if *ident == reading.root && is_path_sep(&trees, at + 1) => {
                insert_heads(trees.get(at + 3), out);
            }
            // The first `super` of a path (not one inside `a::super::b` or `pub(super)`).
            TokenTree::Ident(ident)
                if *ident == "super"
                    && is_path_sep(&trees, at + 1)
                    && !(at > 0 && is_punct(trees.get(at - 1), ':')) =>
            {
                let Some(depth) = reading.depth else {
                    continue;
                };
                let mut next = at;
                let mut climbed = 0;
                while is_ident(trees.get(next), "super") && is_path_sep(&trees, next + 1) {
                    climbed += 1;
                    next += 3;
                }
                if climbed >= depth {
                    insert_heads(trees.get(next), out);
                }
            }
            _ => {}
        }
    }
}

/// The first segment of every `root::` path in the production code of `source`, lexed as Rust,
/// with `super::` chains read from `depth` modules below the library root where that is known.
fn source_path_heads(
    source: &str,
    root: &str,
    depth: Option<usize>,
    path: &Path,
) -> BTreeSet<String> {
    let mut heads = BTreeSet::new();
    path_heads(
        lex(source, path),
        Reading {
            root,
            depth,
            in_attr: false,
        },
        &mut heads,
    );
    heads
}

fn lex(source: &str, path: &Path) -> proc_macro2::TokenStream {
    source
        .parse()
        .unwrap_or_else(|error| panic!("{path:?} must lex as Rust: {error}"))
}

/// How many modules below the library root the file at `relative` (a path under `spargen/src/`)
/// sits: `ir/mod.rs` is `ir` (one), `ir/responses.rs` and `ir/responses/mod.rs` are
/// `ir::responses` (two).
fn module_depth(relative: &Path) -> usize {
    let components = relative.components().count();
    if relative.file_name().is_some_and(|name| name == "mod.rs") {
        components - 1
    } else {
        components
    }
}

/// The files of the modules a test-only `cfg` gates at the top level of `file`: for each
/// `#[cfg(test)] mod name;`, the `name.rs` beside the file's own child modules and the `name/`
/// directory there. A test-only `mod name;` nested inside an inline module is not read.
fn test_only_module_files(file: &Path, source: &str) -> Vec<PathBuf> {
    use proc_macro2::{Delimiter, TokenTree};
    let trees: Vec<TokenTree> = lex(source, file).into_iter().collect();
    let children = if file.file_name().is_some_and(|name| name == "mod.rs") {
        file.parent().expect("a file has a parent").to_path_buf()
    } else {
        file.with_extension("")
    };
    let mut gated = Vec::new();
    for (at, tree) in trees.iter().enumerate() {
        let TokenTree::Group(group) = tree else {
            continue;
        };
        let is_outer_attr = group.delimiter() == Delimiter::Bracket
            && at > 0
            && matches!(&trees[at - 1], TokenTree::Punct(punct) if punct.as_char() == '#');
        if !is_outer_attr || !is_test_only_cfg(group.stream()) {
            continue;
        }
        let item: Vec<&TokenTree> = trees[at + 1..]
            .iter()
            .take_while(|tree| match tree {
                TokenTree::Punct(punct) => punct.as_char() != ';',
                TokenTree::Group(group) => group.delimiter() != Delimiter::Brace,
                _ => true,
            })
            .collect();
        let ends_in_semicolon = matches!(
            trees.get(at + 1 + item.len()),
            Some(TokenTree::Punct(punct)) if punct.as_char() == ';'
        );
        if let [.., TokenTree::Ident(keyword), TokenTree::Ident(name)] = item.as_slice() {
            if keyword == "mod" && ends_in_semicolon {
                gated.push(children.join(format!("{name}.rs")));
                gated.push(children.join(name.to_string()));
            }
        }
    }
    gated
}

/// What a path from the library root may name besides a module: every name a `use` in `lib.rs`
/// brings into the root, mapped to the first segment of its path (`Spec` → `config`), and every
/// `#[macro_export]` macro, mapped to the subsystem that defines it (`omit` → `compat`).
fn root_names(root: &Path) -> BTreeMap<String, String> {
    fn walk(tree: &syn::UseTree, first: Option<&str>, out: &mut BTreeMap<String, String>) {
        match tree {
            syn::UseTree::Path(path) => {
                let segment = path.ident.to_string();
                walk(&path.tree, Some(first.unwrap_or(&segment)), out);
            }
            syn::UseTree::Name(name) => {
                let name = name.ident.to_string();
                out.insert(name.clone(), first.unwrap_or(&name).to_owned());
            }
            syn::UseTree::Rename(rename) => {
                let original = rename.ident.to_string();
                out.insert(
                    rename.rename.to_string(),
                    first.unwrap_or(&original).to_owned(),
                );
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    walk(item, first, out);
                }
            }
            syn::UseTree::Glob(_) => panic!(
                "lib.rs glob-imports into the crate root, so the layering reader cannot tell \
                 which module a root path names: import by name"
            ),
        }
    }
    let lib_path = root.join("spargen/src/lib.rs");
    let lib: syn::File = syn::parse_str(&read(&lib_path))
        .unwrap_or_else(|error| panic!("{lib_path:?} must parse: {error}"));
    let mut names = BTreeMap::new();
    for item in &lib.items {
        if let syn::Item::Use(item) = item {
            walk(&item.tree, None, &mut names);
        }
    }
    for subsystem in SUBSYSTEMS {
        for file in subsystem_files(root, subsystem) {
            let parsed: syn::File = syn::parse_str(&read(&file))
                .unwrap_or_else(|error| panic!("{file:?} must parse: {error}"));
            for item in &parsed.items {
                let syn::Item::Macro(item) = item else {
                    continue;
                };
                let exported = item
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("macro_export"));
                if let (true, Some(name)) = (exported, &item.ident) {
                    names.insert(name.to_string(), (*subsystem).to_owned());
                }
            }
        }
    }
    names
}

/// The edge a root path's first segment takes: the module it is, or the module a root re-export
/// or exported macro of that name comes from. A name the root re-exports from outside the crate
/// (`Utf8Path`) takes none. Any other segment (`self`, a glob, an item `lib.rs` defines itself,
/// such as `crate::generate`) reaches the facade, which `FACADE` names.
fn root_edge(head: &str, names: &BTreeMap<String, String>) -> Option<String> {
    let is_module = |name: &str| SUBSYSTEMS.contains(&name) || FACADE_PLUMBING.contains(&name);
    if is_module(head) {
        return Some(head.to_owned());
    }
    match names.get(head) {
        Some(module) if is_module(module) => Some(module.clone()),
        Some(_) => None,
        None => Some(FACADE.to_owned()),
    }
}

/// The edge name for a path into the crate root's own items, which no subsystem may take.
const FACADE: &str = "facade";

/// The inter-subsystem edges the production code of a subsystem file actually takes: every root
/// path (`crate::`, or a `super::` chain that climbs to the root) mapped by [`root_edge`], less
/// the file's own subsystem.
fn edges(
    source: &str,
    own: &str,
    path: &Path,
    depth: usize,
    names: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    source_path_heads(source, "crate", Some(depth), path)
        .into_iter()
        .filter_map(|head| root_edge(&head, names))
        .filter(|edge| edge != own)
        .collect()
}

/// The production edges of every file of `subsystem`, less the files only a test-only module
/// declaration reaches.
fn subsystem_edges(
    root: &Path,
    subsystem: &str,
    names: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let src = root.join("spargen/src");
    let files = subsystem_files(root, subsystem);
    let gated: Vec<PathBuf> = files
        .iter()
        .flat_map(|file| test_only_module_files(file, &read(file)))
        .collect();
    let mut taken = BTreeSet::new();
    for file in files
        .iter()
        .filter(|file| !gated.iter().any(|gated| file.starts_with(gated)))
    {
        let relative = file
            .strip_prefix(&src)
            .expect("a subsystem file is under src");
        taken.extend(edges(
            &read(file),
            subsystem,
            file,
            module_depth(relative),
            names,
        ));
    }
    taken
}

/// A fixed root namespace for the reader's fixtures: two re-exports and an exported macro.
fn fixture_names() -> BTreeMap<String, String> {
    BTreeMap::from(
        [
            ("Spec", "config"),
            ("Code", "diag"),
            ("Utf8Path", "camino"),
            ("omit", "compat"),
        ]
        .map(|(name, module)| (name.to_owned(), module.to_owned())),
    )
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
    Utf8Path,
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
        edges(
            source,
            "codegen",
            Path::new("fixture.rs"),
            2,
            &fixture_names()
        ),
        BTreeSet::from(["diag", "ir", "name", "source", "surface"].map(str::to_owned)),
        "a re-export from outside the crate is not a module, `codegen` is the file's own, \
         comments, docs and string literals outside attributes take no edge, and a path-valued \
         string in a non-doc attribute does"
    );
}

/// Test-only code takes no edge: an item behind `cfg(test)` or `cfg(all(test, …))`, inline or
/// declared out of line, while `cfg(any(test, …))` and `cfg(not(test))` code is production.
#[test]
fn the_edge_reader_skips_test_only_code() {
    let source = r#"
#[cfg(test)]
use crate::diag::Code;
#[cfg(test)]
impl Default for Options { fn default() -> Self { crate::name::make() } }
#[cfg(all(test, feature = "remote-fetch"))]
mod remote_tests;
#[cfg(any(test, feature = "x"))]
use crate::ir::Api;
#[cfg(not(test))]
fn production() { crate::support::go() }
#[cfg(test)]
mod tests {
    use crate::compat::Omit;
}
fn after() -> crate::source::Spec { todo!() }
struct Fields {
    #[cfg(test)]
    probe: crate::emit::Probe,
    kept: crate::name::Names,
}
"#;
    assert_eq!(
        edges(
            source,
            "codegen",
            Path::new("fixture.rs"),
            2,
            &fixture_names()
        ),
        BTreeSet::from(["ir", "name", "source", "support"].map(str::to_owned)),
        "a test-only item or field is skipped through its end, and the next one is read"
    );
    assert_eq!(
        test_only_module_files(Path::new("src/codegen/client.rs"), source),
        [
            PathBuf::from("src/codegen/client/remote_tests.rs"),
            PathBuf::from("src/codegen/client/remote_tests"),
        ]
    );
    assert_eq!(
        test_only_module_files(
            Path::new("src/codegen/mod.rs"),
            "#[cfg(test)]\nmod tests;\n"
        ),
        [
            PathBuf::from("src/codegen/tests.rs"),
            PathBuf::from("src/codegen/tests"),
        ]
    );
    assert!(
        edges(
            "#![cfg(test)]\nuse crate::diag::Code;\n",
            "codegen",
            Path::new("fixture.rs"),
            2,
            &fixture_names()
        )
        .is_empty(),
        "an inner `#![cfg(test)]` makes the whole module test-only"
    );
}

/// A root path need not spell a module: a re-export or an exported macro is mapped to the module
/// it comes from, a `super::` chain that climbs to the root is read as a root path, and anything
/// else under the root (an item `lib.rs` defines, `self`, a glob) is a facade edge.
#[test]
fn the_edge_reader_maps_root_reexports_and_super_paths() {
    let names = fixture_names();
    let read =
        |source: &str, depth| edges(source, "compat", Path::new("fixture.rs"), depth, &names);
    assert_eq!(
        read("use crate::{Spec, Code};\nfn f() { crate::omit!(); }\n", 2),
        BTreeSet::from(["config", "diag"].map(str::to_owned)),
        "`Spec` and `Code` are re-exports, `omit` is compat's own macro"
    );
    assert_eq!(
        read("use super::ir::Api;\nuse super::super::diag::Code;\n", 1),
        BTreeSet::from(["diag", "ir"].map(str::to_owned)),
        "from a subsystem's `mod.rs`, `super::` is the crate root"
    );
    assert_eq!(
        read(
            "use super::rule::Omit;\nuse super::super::name::Names;\npub(super) fn f() {}\n",
            2
        ),
        BTreeSet::from(["name".to_owned()]),
        "from a file below `mod.rs`, one `super::` stays inside the subsystem"
    );
    assert_eq!(
        read(
            "mod inner {\n    use super::name::Names;\n    use super::super::ir::Api;\n}\n",
            1
        ),
        BTreeSet::from(["ir".to_owned()]),
        "an inline module is one level deeper: from inside one in `mod.rs`, one `super` reaches \
         only `mod.rs` and two reach the root"
    );
    for source in [
        "use crate::generate;\n",
        "use crate::{self as root};\n",
        "use super::*;\n",
    ] {
        assert_eq!(
            read(source, 1),
            BTreeSet::from([FACADE.to_owned()]),
            "{source:?} reaches the crate root's own items"
        );
    }
}

/// The row key of the facade in the `lib.rs` DAG table, and the one cell it may carry.
const FACADE_ROW: (&str, &str) = ("facade (`lib.rs`)", "all of the above");

/// The DAG table in the `lib.rs` module docs, as `row -> allowed cell`, every row read: the
/// header and separator rows are the only ones skipped. The table is the human-readable contract;
/// the headers are the machine-readable one, and they must agree.
fn dag_table(lib: &str) -> BTreeMap<String, String> {
    let mut table = BTreeMap::new();
    for line in lib.lines() {
        let Some(row) = line.trim_start().strip_prefix("//! |") else {
            continue;
        };
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        let [subsystem, allowed, ..] = cells.as_slice() else {
            continue;
        };
        if *subsystem == "Subsystem" || subsystem.starts_with('-') {
            continue;
        }
        let key = subsystem
            .strip_prefix('`')
            .and_then(|key| key.strip_suffix('`'));
        let key = key.filter(|key| !key.contains('`')).unwrap_or(subsystem);
        let previous = table.insert(key.to_owned(), (*allowed).to_owned());
        assert!(
            previous.is_none(),
            "the lib.rs DAG table has two `{key}` rows"
        );
    }
    table
}

/// The dependencies a DAG-table cell allows: `—` (with any parenthetical after it) is none,
/// otherwise each comma-separated entry's leading name, backticks stripped. Every name must be a
/// subsystem or the facade, so a misspelled entry fails rather than vanishing.
fn table_deps(row: &str, cell: &str) -> BTreeSet<String> {
    if cell.starts_with('—') {
        return BTreeSet::new();
    }
    cell.split(',')
        .map(|dep| {
            let dep = dep.trim().trim_matches('`');
            assert!(
                SUBSYSTEMS.contains(&dep) || dep == FACADE,
                "the lib.rs DAG table's `{row}` row allows `{dep}`, which is neither a subsystem \
                 nor the facade"
            );
            dep.to_owned()
        })
        .collect()
}

#[test]
fn the_dag_table_reader_reads_every_row() {
    let lib = "//! | Subsystem | May depend on |\n//! |-----------|---------------|\n\
               //! | `diag`    | —             |\n\
               //! | `support` | — (a handle, which compiles standalone) |\n\
               //! | `emit`    | `codegen`, `ir` |\n\
               //! | `cli`     | facade        |\n\
               //! | `unknown` | `ir`          |\n\
               //! | facade (`lib.rs`) | all of the above |\n";
    let table = dag_table(lib);
    assert_eq!(
        table.keys().map(String::as_str).collect::<Vec<_>>(),
        ["cli", "diag", "emit", FACADE_ROW.0, "support", "unknown"],
        "a row outside `SUBSYSTEMS` is read, not dropped"
    );
    assert_eq!(table[FACADE_ROW.0], FACADE_ROW.1);
    assert!(table_deps("diag", &table["diag"]).is_empty());
    assert!(table_deps("support", &table["support"]).is_empty());
    assert_eq!(
        table_deps("emit", &table["emit"]),
        BTreeSet::from(["codegen", "ir"].map(str::to_owned))
    );
    assert_eq!(
        table_deps("cli", &table["cli"]),
        BTreeSet::from([FACADE.to_owned()])
    );
}

#[test]
#[should_panic(expected = "neither a subsystem nor the facade")]
fn the_dag_table_reader_rejects_an_unknown_dependency() {
    table_deps("emit", "`codegn`");
}

#[test]
fn every_subsystem_declares_the_dependencies_it_actually_takes() {
    let root = workspace_root();
    let names = root_names(&root);
    let mut total_edges = 0;
    for subsystem in SUBSYSTEMS {
        let module = root.join("spargen/src").join(subsystem).join("mod.rs");
        let declared = declared_deps(&read(&module), &module);

        let taken = subsystem_edges(&root, subsystem, &names);
        total_edges += taken.len();

        assert!(
            !taken.contains(FACADE),
            "subsystem `{subsystem}` reaches an item of the crate root itself (an item lib.rs \
             defines, `crate::self`, or a glob of the root), inverting the layering: name the \
             subsystem that owns it"
        );
        let undeclared: Vec<&String> = taken.difference(&declared).collect();
        assert!(
            undeclared.is_empty(),
            "subsystem `{subsystem}` reaches {undeclared:?} but its `//! layer-deps:` header \
             declares only {declared:?} — declare the edge (and add it to the DAG table in \
             lib.rs), or stop taking it"
        );
        let unused: Vec<&String> = declared.difference(&taken).collect();
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
}

/// A cycle in `graph` (`node -> dependencies`), as the path that closes it, or `None` where the
/// graph is acyclic. A dependency that is not itself a key has no outgoing edges.
fn find_cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    fn visit(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        path: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(at) = path.iter().position(|seen| seen == node) {
            let mut cycle = path[at..].to_vec();
            cycle.push(node.to_owned());
            return Some(cycle);
        }
        if done.contains(node) {
            return None;
        }
        path.push(node.to_owned());
        for dep in graph.get(node).into_iter().flatten() {
            if let Some(cycle) = visit(dep, graph, path, done) {
                return Some(cycle);
            }
        }
        path.pop();
        done.insert(node.to_owned());
        None
    }
    let mut done = BTreeSet::new();
    graph
        .keys()
        .find_map(|node| visit(node, graph, &mut Vec::new(), &mut done))
}

#[test]
fn the_cycle_finder_finds_a_cycle_and_only_a_cycle() {
    let graph = |edges: &[(&str, &[&str])]| -> BTreeMap<String, BTreeSet<String>> {
        edges
            .iter()
            .map(|(node, deps)| {
                (
                    (*node).to_owned(),
                    deps.iter().map(|dep| (*dep).to_owned()).collect(),
                )
            })
            .collect()
    };
    let diamond = graph(&[
        ("emit", &["codegen"]),
        ("codegen", &["ir", "name"]),
        ("name", &["ir"]),
        ("ir", &["diag"]),
    ]);
    assert_eq!(
        find_cycle(&diamond),
        None,
        "a shared dependency is no cycle"
    );
    let looped = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"]), ("d", &[])]);
    assert_eq!(
        find_cycle(&looped),
        Some(["a", "b", "c", "a"].map(str::to_owned).to_vec())
    );
    let own = graph(&[("a", &["a"])]);
    assert_eq!(
        find_cycle(&own),
        Some(["a", "a"].map(str::to_owned).to_vec())
    );
}

/// The headers declare a DAG: no subsystem reaches itself through its declared dependencies, so
/// the pipeline order `lib.rs` documents exists.
#[test]
fn the_declared_layering_is_acyclic() {
    let root = workspace_root();
    let graph: BTreeMap<String, BTreeSet<String>> = SUBSYSTEMS
        .iter()
        .map(|subsystem| {
            let module = root.join("spargen/src").join(subsystem).join("mod.rs");
            (
                (*subsystem).to_owned(),
                declared_deps(&read(&module), &module),
            )
        })
        .collect();
    assert!(
        graph.values().any(|deps| !deps.is_empty()),
        "no subsystem declares a dependency, so the headers are not being read"
    );
    if let Some(cycle) = find_cycle(&graph) {
        panic!(
            "the `//! layer-deps:` headers declare a cycle, so they describe no layering: {}",
            cycle.join(" -> ")
        );
    }
}

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

/// The lib.rs DAG table has exactly one row per subsystem, `cli` and the facade, and every
/// subsystem row and the `cli` row equal the header they document.
#[test]
fn the_declared_headers_agree_with_the_dag_table_in_lib_rs() {
    let root = workspace_root();
    let table = dag_table(&read(&root.join("spargen/src/lib.rs")));

    let rows: BTreeSet<&str> = table.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = SUBSYSTEMS
        .iter()
        .copied()
        .chain(["cli", FACADE_ROW.0])
        .collect();
    assert_eq!(
        rows, expected,
        "the lib.rs DAG table's rows and the subsystems (plus `cli` and the facade) have drifted"
    );
    assert_eq!(
        table[FACADE_ROW.0], FACADE_ROW.1,
        "the facade row of the lib.rs DAG table"
    );

    let headers = SUBSYSTEMS
        .iter()
        .map(|subsystem| {
            (
                *subsystem,
                root.join("spargen/src").join(subsystem).join("mod.rs"),
            )
        })
        .chain([("cli", root.join("spargen/src/cli/run.rs"))]);
    for (row, module) in headers {
        let declared = declared_deps(&read(&module), &module);
        let documented = table_deps(row, &table[row]);
        assert_eq!(
            declared, documented,
            "`{row}` declares {declared:?} in its header but the lib.rs DAG table says \
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
        if !source_path_heads(&source, "spargen", None, &file).is_empty() {
            taken.insert(FACADE.to_owned());
        }
        taken.extend(
            source_path_heads(&source, "crate", None, &file)
                .into_iter()
                .filter(|head| {
                    SUBSYSTEMS.contains(&head.as_str()) || FACADE_PLUMBING.contains(&head.as_str())
                }),
        );
    }
    assert_eq!(
        taken, declared,
        "the `cli` files take {taken:?}, and run.rs declares {declared:?}: the CLI reaches the \
         library through the `spargen::` facade and nothing else"
    );
}
