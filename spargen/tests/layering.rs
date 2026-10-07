//! Structural invariants CLAUDE.md states but nothing checked: the subsystem layering DAG, the
//! shape of the embedded runtime sources, the file list that embeds them, that the published
//! crate carries every file its sources include, and that every integration test spawning the
//! `cli`-gated binary is itself gated on `cli`.
//!
//! `lib.rs` promises that the declarations are diffed against the actual inter-module `use` edges.
//! This suite is where that happens: it needs no extra workspace member and runs under the
//! ordinary `test` gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Spec};

/// The subsystems that are modules of the library, in the order `lib.rs` documents them. `cli` is
/// deliberately absent: it has no `mod.rs` and is `#[path]`-included into the binary, so `crate::`
/// inside it means the binary crate, not the library. Its header is checked separately.
const SUBSYSTEMS: &[&str] = &[
    "diag", "source", "ir", "oas31", "name", "support", "codegen", "emit", "compat", "surface",
];

/// Facade plumbing: named in `lib.rs` as explicitly *not* subsystems, and therefore never a legal
/// dependency of one. A subsystem reaching into these inverts the layering.
const FACADE_PLUMBING: &[&str] = &["cache", "config", "runtime_contract"];

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

/// Every `.rs` file belonging to a subsystem. `support/runtime/` is excluded: those entries are
/// symlinks to the `support-runtime` crate's sources, reached only through `include_str!` and never
/// compiled as spargen modules, so their `crate::` paths name the *other* crate's root.
fn subsystem_files(root: &Path, subsystem: &str) -> Vec<PathBuf> {
    let dir = root.join("spargen/src").join(subsystem);
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("subsystem directory exists") {
            let path = entry.expect("readable directory entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "runtime") {
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
/// or `cli` (header checked by `the_cli_declares_its_dependency_on_the_facade`), or `bin`, which
/// holds the binary's entry point and is no subsystem.
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
        .chain(&["cli"])
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        on_disk, declared,
        "the directories under spargen/src/ and `SUBSYSTEMS` (plus `cli`) have drifted: a \
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

/// The runtime sources are embedded by splitting on the literal `#[cfg(test)]` and keeping
/// everything before it (`codegen/emit.rs`). That is only sound while each file contains the marker
/// at most once and puts nothing after the test module: a second occurrence — or the literal string
/// in a doc comment above the tests — would silently truncate embedded runtime source.
#[test]
fn each_runtime_source_carries_its_test_module_last_and_only_once() {
    let root = workspace_root();
    let dir = root.join("support-runtime/src");
    let (mut sources, mut with_tests) = (0, 0);
    for entry in std::fs::read_dir(&dir).expect("support-runtime/src exists") {
        let path = entry.expect("readable directory entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        sources += 1;
        let source = read(&path);
        let occurrences = source.matches("#[cfg(test)]").count();
        assert!(
            occurrences <= 1,
            "{path:?} contains `#[cfg(test)]` {occurrences} times; the embed splits on the first \
             one, so everything after it would be dropped from generated output"
        );
        let Some((_, tail)) = source.split_once("#[cfg(test)]") else {
            continue;
        };
        with_tests += 1;
        // Nothing but the test module may follow: the first item after the marker is `mod tests`,
        // and that module is the file's last item.
        assert!(
            tail.trim_start().starts_with("mod tests"),
            "{path:?} puts something other than `mod tests` after `#[cfg(test)]`"
        );
        assert_eq!(
            test_module_position(&source, &path),
            Some(TestModule::Last),
            "{path:?} declares items after its `#[cfg(test)]` module"
        );
    }
    assert!(
        sources > 0 && with_tests > 0,
        "read {sources} runtime sources and {with_tests} test modules: the check has stopped \
         reading support-runtime/src"
    );
}

/// Where a parsed file's `#[cfg(test)] mod tests` sits among its items.
#[derive(Debug, PartialEq)]
enum TestModule {
    Last,
    NotLast,
}

/// The position of the item `#[cfg(test)] mod tests` in `source`, parsed with `syn`, or `None`
/// when the file has no such item.
fn test_module_position(source: &str, path: &Path) -> Option<TestModule> {
    let file = syn::parse_file(source).unwrap_or_else(|error| panic!("{path:?} parses: {error}"));
    let is_test_module = |item: &syn::Item| {
        let syn::Item::Mod(module) = item else {
            return false;
        };
        module.ident == "tests"
            && module.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr
                        .parse_args::<syn::Ident>()
                        .is_ok_and(|arg| arg == "test")
            })
    };
    let at = file.items.iter().position(is_test_module)?;
    Some(if at + 1 == file.items.len() {
        TestModule::Last
    } else {
        TestModule::NotLast
    })
}

#[test]
fn the_test_module_reader_finds_an_item_after_the_module() {
    let path = Path::new("fixture.rs");
    let last = "pub fn embedded() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
    assert_eq!(test_module_position(last, path), Some(TestModule::Last));

    // An item after the module that also closes at column 0: a search for the last `\n}` took
    // its brace for the module's, and read the module as last.
    let after = "#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn stray() {\n}\n";
    assert_eq!(test_module_position(after, path), Some(TestModule::NotLast));

    assert_eq!(test_module_position("pub fn embedded() {}\n", path), None);
}

/// The sentence an embedded comment block must carry to name a test-only item. It is the one
/// honest way to point a maintainer at a test from above the split marker: it tells the consumer
/// reading the embedded copy that the names are absent there, rather than sending them to look.
const STRIPPED_DISCLOSURE: &str =
    "That test module is stripped when this file is embedded into a generated client";

/// Every identifier in `text`, in order, as Rust lexes one (ASCII is all the runtime uses).
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
}

/// The identifiers a comment line uses as code: every one inside a backtick code span (which is
/// how rustdoc names an item, intra-doc links included), and outside one only those carrying an
/// underscore. Test helpers are often plain English words (`string`, `calls`), so a bare word in
/// prose is not read as a reference to one.
fn code_names(line: &str) -> impl Iterator<Item = &str> {
    line.split('`').enumerate().flat_map(|(index, part)| {
        let in_span = index % 2 == 1;
        identifiers(part).filter(move |word| in_span || word.contains('_'))
    })
}

/// The names a test module declares as items: what a maintainer-facing sentence would direct the
/// reader to. Only a line that *starts* with an item keyword (after visibility and qualifiers)
/// counts, so prose in a comment ("the type of …") or a string literal declares nothing. `tests`
/// itself is left out, since it is also an English word; the "test module" phrase rule covers a
/// reference to the module.
fn test_only_declarations(tail: &str) -> BTreeSet<String> {
    const QUALIFIERS: &[&str] = &["pub", "crate", "super", "async", "unsafe", "const"];
    const INTRODUCERS: &[&str] = &[
        "fn",
        "const",
        "static",
        "struct",
        "enum",
        "type",
        "trait",
        "mod",
        "macro_rules",
    ];
    let mut names = BTreeSet::new();
    for line in tail.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut words = identifiers(line).peekable();
        // `const fn` names the function, so a qualifier is skipped only while an introducer (or
        // another qualifier) still follows it.
        while let Some(word) = words.next() {
            let next = words.peek().copied();
            if QUALIFIERS.contains(&word)
                && next
                    .is_some_and(|next| QUALIFIERS.contains(&next) || INTRODUCERS.contains(&next))
            {
                continue;
            }
            if INTRODUCERS.contains(&word) {
                if let Some(name) = next.filter(|name| *name != "tests") {
                    names.insert(name.to_owned());
                }
            }
            break;
        }
    }
    names
}

/// The comment lines of `head` (a runtime source above its `#[cfg(test)]` marker) that break the
/// rule `embedded_comments_name_no_test_only_item_undisclosed` states, each as `LINE: TEXT` with
/// the test-only items it names.
///
/// A comment block is a run of consecutive comment lines, and both the disclosure and the phrase
/// "test module" are matched against the block's joined, whitespace-normalised text, so neither
/// depends on where the prose happens to wrap. A block that mentions the phrase reports every one
/// of its lines; otherwise a line is reported only when it names a test-only item itself.
fn comment_violations(head: &str, test_only: &BTreeSet<String>) -> Vec<String> {
    let mut violations = Vec::new();
    let mut block: Vec<(usize, &str)> = Vec::new();
    let lines: Vec<&str> = head.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            block.push((index + 1, trimmed));
        }
        let block_ends = !trimmed.starts_with("//") || index + 1 == lines.len();
        if !block_ends || block.is_empty() {
            continue;
        }
        let text = block
            .iter()
            .flat_map(|(_, line)| {
                line.trim_start_matches('/')
                    .trim_start_matches('!')
                    .split_whitespace()
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !text.contains(STRIPPED_DISCLOSURE) {
            let mentions_test_module = text.to_ascii_lowercase().contains("test module");
            for (number, line) in &block {
                let named: Vec<&str> = code_names(line)
                    .filter(|word| test_only.contains(*word))
                    .collect();
                if !named.is_empty() || mentions_test_module {
                    violations.push(format!("{number}: {line}  (names {named:?})"));
                }
            }
        }
        block.clear();
    }
    violations
}

/// The rule's matching, pinned on fixtures rather than on whatever the runtime sources say today:
/// the phrase and the disclosure are read across the lines a comment block wraps over.
#[test]
fn comment_violations_read_each_comment_block_as_one_text() {
    let test_only: BTreeSet<String> = ["helper_fn".to_owned()].into();

    let wrapped = "/// The contract is held by the test\n/// module below.\nfn embedded() {}\n";
    assert_eq!(
        comment_violations(wrapped, &test_only),
        [
            "1: /// The contract is held by the test  (names [])",
            "2: /// module below.  (names [])",
        ],
        "the phrase \"test module\" wrapped across two lines must still be caught"
    );

    let one_line = "// See the TEST MODULE.\nfn embedded() {}\n";
    assert_eq!(
        comment_violations(one_line, &test_only),
        ["1: // See the TEST MODULE.  (names [])"]
    );

    let named = "// Checked by `helper_fn`.\n// Nothing else here.\nfn embedded() {}\n";
    assert_eq!(
        comment_violations(named, &test_only),
        ["1: // Checked by `helper_fn`.  (names [\"helper_fn\"])"],
        "without the phrase, only the line that names the item is reported"
    );

    let (first, rest) = STRIPPED_DISCLOSURE
        .split_once(' ')
        .expect("the disclosure has more than one word");
    let disclosed = format!(
        "// Checked by `helper_fn` in the test module. {first}\n// {rest}.\nfn embedded() {{}}\n"
    );
    assert!(
        comment_violations(&disclosed, &test_only).is_empty(),
        "a disclosure wrapped across lines still covers its block"
    );

    let apart = "// the test\nfn embedded() {}\n// module\n";
    assert!(
        comment_violations(apart, &test_only).is_empty(),
        "separate comment blocks are not joined across code"
    );
}

/// Everything above `#[cfg(test)]` in a runtime source ships verbatim in every generated client,
/// and the test module below it does not (`generated_output_carries_no_test_module`). So a comment
/// up there that names a test-only item, or points at "the test module", is true in this
/// repository and false in the copy a consumer reads: the item it sends them to does not exist
/// there. Such a comment block is allowed only when it says so, by carrying
/// [`STRIPPED_DISCLOSURE`].
///
/// A test-only item is one a test module declares and no embedded code line mentions; a name that
/// embedded code also uses exists, in some form, in the consumer's copy. `lib.rs` is not embedded
/// (the generated `support` module replaces it), so neither its code nor its comments count.
#[test]
fn embedded_comments_name_no_test_only_item_undisclosed() {
    let root = workspace_root();
    let dir = root.join("support-runtime/src");
    let mut sources: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .expect("support-runtime/src exists")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_none_or(|name| name != "lib.rs"))
        .map(|path| {
            let source = read(&path);
            (path, source)
        })
        .collect();
    sources.sort();

    let mut test_only = BTreeSet::new();
    let mut embedded_code = BTreeSet::new();
    for (_, source) in &sources {
        let (head, tail) = source
            .split_once("#[cfg(test)]")
            .unwrap_or((source.as_str(), ""));
        test_only.extend(test_only_declarations(tail));
        for line in head.lines() {
            let code = line.split_once("//").map_or(line, |(code, _)| code);
            embedded_code.extend(identifiers(code).map(str::to_owned));
        }
    }
    let test_only: BTreeSet<String> = test_only.difference(&embedded_code).cloned().collect();
    assert!(
        !test_only.is_empty(),
        "no test-only item was found, so this check has stopped reading the test modules"
    );

    let mut violations = Vec::new();
    for (path, source) in &sources {
        let head = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(head, _)| head);
        let file = path.strip_prefix(&root).unwrap_or(path).display();
        violations.extend(
            comment_violations(head, &test_only)
                .into_iter()
                .map(|violation| format!("{file}:{violation}")),
        );
    }
    assert!(
        violations.is_empty(),
        "these comments sit above `#[cfg(test)]`, so they ship in every generated client, but they \
         name a test-only item or the test module, neither of which exists in that copy. Reword \
         them, or state it in the same comment block with the sentence \
         {STRIPPED_DISCLOSURE:?}:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_embed_list_names_every_runtime_source() {
    let root = workspace_root();
    let registry = read(&root.join("spargen/src/support/mod.rs"));
    let embedded: BTreeSet<String> = registry
        .match_indices("include_str!(\"runtime/")
        .map(|(at, marker)| {
            registry[at + marker.len()..]
                .split('"')
                .next()
                .expect("a closing quote")
                .to_owned()
        })
        .collect();

    // `lib.rs` is the crate root of `support-runtime`: it wires the modules together and is
    // replaced by the generated `support` module, so it is the one file that is never embedded.
    let on_disk: BTreeSet<String> = std::fs::read_dir(root.join("support-runtime/src"))
        .expect("support-runtime/src exists")
        .map(|entry| entry.expect("readable directory entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".rs") && name != "lib.rs")
        .collect();
    assert!(
        !embedded.is_empty() && !on_disk.is_empty(),
        "read {} `include_str!(\"runtime/…\")` entries and {} runtime sources: an empty side \
         means the scan has stopped reading it, and empty equals empty",
        embedded.len(),
        on_disk.len()
    );

    assert_eq!(
        embedded, on_disk,
        "`runtime_files()` is hand-maintained and has drifted from support-runtime/src: a file \
         listed but absent breaks the build, and a file present but unlisted is silently missing \
         from every generated client"
    );

    let symlinked: BTreeSet<String> = std::fs::read_dir(root.join("spargen/src/support/runtime"))
        .expect("the runtime symlink directory exists")
        .map(|entry| entry.expect("readable directory entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".rs"))
        .collect();
    assert_eq!(
        symlinked, on_disk,
        "the `src/support/runtime/` symlinks are what `cargo publish` follows, so a missing link \
         ships a crate that cannot build"
    );
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

/// The value of `const NAME: usize = N;` in `source`.
fn declared_count(source: &str, name: &str) -> usize {
    let marker = format!("const {name}: usize = ");
    let at = source
        .find(&marker)
        .unwrap_or_else(|| panic!("`{marker}` is declared in support-runtime/src/error.rs"))
        + marker.len();
    source[at..]
        .split(';')
        .next()
        .and_then(|literal| literal.trim().parse().ok())
        .unwrap_or_else(|| panic!("`{name}` is an integer literal"))
}

/// `every_request_variant` and `every_variant` are arrays of `REQUEST_VARIANTS` and
/// `ERROR_VARIANTS` entries, and the in-crate bijection tests hold each list to exactly one value
/// per index below its count. What neither can see is a variant added to the enum while the count
/// stays put: the new variant is then never constructed, so every exhaustiveness test reading the
/// list passes over an incomplete one. No stable language construct yields a variant count, so the
/// count is taken from the declaration text here, where nothing reaches generated output.
#[test]
fn every_error_variant_is_counted_by_its_enumeration() {
    let source = read(&workspace_root().join("support-runtime/src/error.rs"));
    for (header, count, list) in [
        (
            "pub enum RequestError {",
            "REQUEST_VARIANTS",
            "every_request_variant",
        ),
        ("pub enum Error<E> {", "ERROR_VARIANTS", "every_variant"),
    ] {
        let variants = enum_variants(&source, header);
        let declared = declared_count(&source, count);
        assert_eq!(
            variants.len(),
            declared,
            "`{header}` declares {} variants ({variants:?}) but `{count}` is {declared}: set \
             `{count}` to the variant count and list a value of each variant in `{list}`",
            variants.len()
        );
    }
}

#[test]
fn the_variant_counter_reads_names_not_payloads() {
    let source = "
        pub enum Sample<E> {
            /// A doc comment, with a comma, naming NotAVariant.
            #[allow(dead_code)]
            Unit,
            Tuple(Vec<(u8, E)>, String), // trailing, comment
            Struct {
                /// field doc
                field: [u8; 2],
                other: Option<E>,
            },
            Last}
        pub enum After { Ignored }
    ";
    assert_eq!(
        enum_variants(source, "pub enum Sample<E> {"),
        ["Unit", "Tuple", "Struct", "Last"]
    );
}

#[test]
fn generated_output_carries_no_test_module() {
    const SPEC: &str = r#"
openapi: 3.1.0
info: { title: Embed, version: 1.0.0 }
servers: [{ url: "https://example.com" }]
paths:
  /things:
    get:
      operationId: listThings
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: array, items: { type: string } }
"#;

    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, SPEC).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();

    let report = spargen::generate(
        &Spec::new(spec_path)
            .build(out.clone())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("#[cfg(test)]"),
        "the embedded runtime's test modules must be stripped: generated output would otherwise \
         carry test-only `crate::` imports that do not survive the module renesting"
    );
    assert!(
        !generated.contains("mod tests"),
        "generated output must not carry a `mod tests`"
    );
}

/// The path literal of every `include_str!` and `include_bytes!` in `source`: every `include_str!`
/// target in source order, then every `include_bytes!` target in source order. Lines that
/// open with `//` are skipped, so prose citing the macro contributes nothing; whitespace between
/// the parenthesis and the literal is allowed, since rustfmt breaks a long call there.
fn include_targets(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut targets = Vec::new();
    for marker in ["include_str!(", "include_bytes!("] {
        for (at, _) in code.match_indices(marker) {
            let Some(literal) = code[at + marker.len()..].trim_start().strip_prefix('"') else {
                continue;
            };
            let path = literal.split('"').next().expect("a closing quote");
            targets.push(path.to_owned());
        }
    }
    targets
}

#[test]
fn the_include_scanner_reads_path_literals_and_skips_comments() {
    let source = r#"
//! Embedded with `include_str!("prose.txt")`, which this line only cites.
const A: &str = include_str!("a.txt");
const B: &[u8] = include_bytes!("../b.bin");
const C: (&str, &str) = (
    "c",
    include_str!(
        "sub/c.rs"
    ),
);
    // include_str!("commented.txt")
const D: &str = include_str!(concat!("not", "a", "literal"));
"#;
    assert_eq!(include_targets(source), ["a.txt", "sub/c.rs", "../b.bin"]);
}

/// Every file an `include_str!` or `include_bytes!` in `spargen/src` names is in the package
/// `cargo publish` uploads.
///
/// `cargo publish --dry-run` builds the packaged crate, so an include the library compiles is
/// already held there. An include under `#[cfg(test)]` is not: the library builds without its
/// target, so an `exclude` in `Cargo.toml` or a `.gitignore` pattern could drop the file from the
/// `.crate` and every packaging gate stays green, while `cargo test` on the published crate no
/// longer compiles. `src/runtime_contract_e023_explain.txt`, the second copy of `E023`'s explain
/// body, is such a target (#203). The rule is stated over every include rather than over that file,
/// so a new test fixture read the same way is held without anyone remembering to list it.
#[test]
fn every_included_file_ships_in_the_published_crate() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_dir.join("src");

    let mut sources = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable source directory") {
            let path = entry.expect("readable directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();

    // Resolved lexically against the including file's directory and never canonicalized: the
    // `src/support/runtime/` entries are symlinks, and the package lists them at their own paths.
    let mut included = BTreeMap::new();
    for source in &sources {
        let dir = source
            .parent()
            .expect("a source file has a directory")
            .strip_prefix(crate_dir)
            .expect("sources live under the crate directory");
        for target in include_targets(&read(source)) {
            let mut resolved = dir.to_path_buf();
            for component in Path::new(&target).components() {
                match component {
                    std::path::Component::ParentDir => {
                        assert!(
                            resolved.pop(),
                            "{source:?} includes {target:?} above the crate"
                        );
                    }
                    std::path::Component::CurDir => {}
                    other => resolved.push(other),
                }
            }
            let resolved = resolved
                .to_str()
                .expect("UTF-8 path")
                .replace(std::path::MAIN_SEPARATOR, "/");
            let site = source
                .strip_prefix(crate_dir)
                .expect("under the crate directory");
            included.insert(resolved, site.to_path_buf());
        }
    }
    assert!(
        !included.is_empty(),
        "no include found under {src:?}: the scanner has stopped reading the sources"
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args([
            "package",
            "--list",
            "--allow-dirty",
            "--offline",
            "-p",
            "spargen",
        ])
        .current_dir(crate_dir)
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "`cargo package --list` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packaged: BTreeSet<String> = String::from_utf8(output.stdout)
        .expect("UTF-8 file list")
        .lines()
        .map(str::to_owned)
        .collect();

    let missing: Vec<String> = included
        .iter()
        .filter(|(path, _)| !packaged.contains(*path))
        .map(|(path, source)| format!("{path} (included by {})", source.display()))
        .collect();
    assert!(
        missing.is_empty(),
        "these files are included by spargen's sources but absent from `cargo package --list`, so \
         the published crate does not carry them: {missing:#?}"
    );
}

/// The compile-time variable cargo sets to the `spargen` binary's path for integration tests.
const SPAWN_VARIABLE: &str = "CARGO_BIN_EXE_spargen";

/// The one attribute accepted as a `cli` gate, with its whitespace removed.
const CLI_GATE: &str = "#[cfg(feature=\"cli\")]";

/// A source file with every comment and every literal's contents blanked to spaces, byte for byte,
/// so an offset into `code` is the same offset into the source.
struct Masked {
    code: Vec<u8>,
    /// Each string literal, as the offset its token starts at (the `r` of a raw string, otherwise
    /// the opening quote) and its contents, escapes left unprocessed.
    literals: Vec<(usize, String)>,
}

fn is_ident_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn mask(source: &str) -> Masked {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut literals = Vec::new();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut code[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |offset| at + offset);
            blank(&mut code, at, end);
            at = end;
        } else if rest.starts_with(b"/*") {
            let (mut end, mut depth) = (at + 2, 1);
            while end < bytes.len() && depth > 0 {
                if bytes[end..].starts_with(b"/*") {
                    depth += 1;
                    end += 2;
                } else if bytes[end..].starts_with(b"*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += 1;
                }
            }
            blank(&mut code, at, end);
            at = end;
        } else if bytes[at] == b'r'
            && (at == 0
                || !is_ident_byte(bytes[at - 1])
                || (bytes[at - 1] == b'b' && (at < 2 || !is_ident_byte(bytes[at - 2]))))
            && rest[1..]
                .iter()
                .find(|&&byte| byte != b'#')
                .is_some_and(|&byte| byte == b'"')
        {
            let hashes = rest[1..].iter().take_while(|&&byte| byte == b'#').count();
            let open = at + 1 + hashes + 1;
            let mut closing = vec![b'"'];
            closing.extend(std::iter::repeat_n(b'#', hashes));
            let close = bytes[open..]
                .windows(closing.len())
                .position(|window| window == closing.as_slice())
                .map_or(bytes.len(), |offset| open + offset);
            literals.push((at, source[open..close].to_owned()));
            blank(&mut code, open, close);
            at = (close + closing.len()).min(bytes.len());
        } else if bytes[at] == b'"' {
            let mut close = at + 1;
            while close < bytes.len() && bytes[close] != b'"' {
                close += if bytes[close] == b'\\' { 2 } else { 1 };
            }
            let close = close.min(bytes.len());
            literals.push((at, source[at + 1..close].to_owned()));
            blank(&mut code, at + 1, close);
            at = close + 1;
        } else if bytes[at] == b'\'' {
            // A char literal is blanked; a lifetime, which has no closing quote, is left alone.
            if bytes.get(at + 1) == Some(&b'\\') {
                let from = (at + 3).min(bytes.len());
                let close = bytes[from..]
                    .iter()
                    .position(|&byte| byte == b'\'')
                    .map_or(bytes.len(), |offset| from + offset);
                blank(&mut code, at + 1, close);
                at = close + 1;
            } else {
                let width = source[at + 1..].chars().next().map_or(0, char::len_utf8);
                if width > 0 && bytes.get(at + 1 + width) == Some(&b'\'') {
                    blank(&mut code, at + 1, at + 1 + width);
                    at += width + 2;
                } else {
                    at += 1;
                }
            }
        } else {
            at += 1;
        }
    }
    Masked { code, literals }
}

/// The offset of every `env!("CARGO_BIN_EXE_spargen")` or `option_env!` of it in the code, raw
/// literals, whitespace inside the invocation, and each macro delimiter (`(`, `[`, `{`) included.
/// A comment or a string that only cites the invocation is blanked by [`mask`], so it names no
/// site.
fn spawn_sites(masked: &Masked) -> Vec<usize> {
    fn trim_end(code: &[u8]) -> &[u8] {
        let kept = code
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map_or(0, |last| last + 1);
        &code[..kept]
    }
    masked
        .literals
        .iter()
        .filter(|(_, contents)| contents == SPAWN_VARIABLE)
        .filter_map(|&(at, _)| {
            let (&open, before) = trim_end(&masked.code[..at]).split_last()?;
            if !matches!(open, b'(' | b'[' | b'{') {
                return None;
            }
            let before = trim_end(before);
            let before = trim_end(before.strip_suffix(b"!")?);
            let name_start = before
                .iter()
                .rposition(|&byte| !is_ident_byte(byte))
                .map_or(0, |last| last + 1);
            matches!(&before[name_start..], b"env" | b"option_env").then_some(at)
        })
        .collect()
}

/// The span of each item at the top level of `code`, as delimited by a `;` or a closing `}` at
/// bracket depth zero. The file's inner attributes and comments ride at the front of the first.
fn top_level_items(code: &[u8]) -> Vec<(usize, usize)> {
    let (mut items, mut depth, mut start) = (Vec::new(), 0usize, 0);
    for (at, &byte) in code.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    items.push((start, at + 1));
                    start = at + 1;
                }
            }
            b';' if depth == 0 => {
                items.push((start, at + 1));
                start = at + 1;
            }
            _ => {}
        }
    }
    if start < code.len() {
        items.push((start, code.len()));
    }
    items
}

/// The attributes leading the item that starts at `start`, as `(inner, whitespace-free text)`.
fn leading_attributes(source: &str, code: &[u8], start: usize) -> Vec<(bool, String)> {
    let skip_whitespace = |mut at: usize| {
        while code.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let mut attributes = Vec::new();
    let mut at = skip_whitespace(start);
    while code.get(at) == Some(&b'#') {
        let mut open = skip_whitespace(at + 1);
        let inner = code.get(open) == Some(&b'!');
        if inner {
            open = skip_whitespace(open + 1);
        }
        if code.get(open) != Some(&b'[') {
            break;
        }
        let mut depth = 0usize;
        let mut end = open;
        while end < code.len() {
            match code[end] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        let text: String = source[at..(end + 1).min(source.len())]
            .chars()
            .filter(|char| !char.is_whitespace())
            .collect();
        attributes.push((inner, text));
        at = skip_whitespace(end + 1);
    }
    attributes
}

/// The line of every spawn of the `spargen` binary in `source` that no `cli` gate covers.
///
/// A spawn is gated when the file carries `#![cfg(feature = "cli")]`, or when the top-level item
/// containing it carries `#[cfg(feature = "cli")]`. Only that literal attribute counts (whitespace
/// aside), and only on the top-level item: a gate on an item nested inside an ungated module, or a
/// predicate such as `all(feature = "cli", ...)`, is reported, so the rule is checkable without a
/// cfg evaluator. Gate the outer item instead.
fn ungated_spawns(source: &str) -> Vec<usize> {
    let masked = mask(source);
    let sites = spawn_sites(&masked);
    let items = top_level_items(&masked.code);
    let inner_gate = format!("#!{}", &CLI_GATE[1..]);
    let attributes: Vec<Vec<(bool, String)>> = items
        .iter()
        .map(|&(start, _)| leading_attributes(source, &masked.code, start))
        .collect();
    if attributes
        .iter()
        .flatten()
        .any(|(inner, text)| *inner && *text == inner_gate)
    {
        return Vec::new();
    }
    sites
        .into_iter()
        .filter(|&site| {
            let item = items
                .iter()
                .position(|&(start, end)| (start..end).contains(&site))
                .expect("every offset lies in some top-level span");
            !attributes[item]
                .iter()
                .any(|(inner, text)| !*inner && text == CLI_GATE)
        })
        .map(|site| source[..site].matches('\n').count() + 1)
        .collect()
}

/// The source path of every `[[test]]` target `manifest` declares with `cli` among its
/// `required-features`, relative to the crate directory.
fn cli_gated_targets(manifest: &str) -> BTreeSet<String> {
    let manifest: toml::Table = toml::from_str(manifest).expect("the manifest parses");
    let Some(toml::Value::Array(tests)) = manifest.get("test") else {
        return BTreeSet::new();
    };
    tests
        .iter()
        .filter_map(toml::Value::as_table)
        .filter(|target| {
            target
                .get("required-features")
                .and_then(toml::Value::as_array)
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature.as_str() == Some("cli"))
                })
        })
        .map(
            |target| match target.get("path").and_then(toml::Value::as_str) {
                Some(path) => path.to_owned(),
                None => format!(
                    "tests/{}.rs",
                    target
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .expect("a [[test]] target without a path has a name")
                ),
            },
        )
        .collect()
}

#[test]
fn the_spawn_gate_scanner_rejects_every_ungated_spawn() {
    let source = r##"
//! Spawns `env!("CARGO_BIN_EXE_spargen")`, which this line only cites.
#![cfg(feature = "remote-fetch")]
use std::process::Command;

/* A block comment citing env!("CARGO_BIN_EXE_spargen") /* nested */ names no site. */
#[cfg(feature = "cli")]
fn gated() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
}

const BRACES: &str = "}}{";
const CHAR: char = '}';

#[test]
#[cfg(feature = "remote-fetch")]
fn gated_on_another_feature<'a>() {
    let _ = Command::new(env ! ( "CARGO_BIN_EXE_spargen" ));
}

#[test]
fn ungated_raw() {
    let _ = option_env!(r#"CARGO_BIN_EXE_spargen"#);
}

fn cites_it() -> &'static str {
    "env!(\"CARGO_BIN_EXE_spargen\")"
}

mod nested {
    #[cfg(feature = "cli")]
    fn gated_below_an_ungated_module() {
        let _ = std::process::Command::new(core::env!("CARGO_BIN_EXE_spargen"));
    }
}

#[cfg(all(feature = "cli", unix))]
fn a_predicate_is_not_the_gate() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}

#[test]
fn ungated_brackets() {
    let _ = env!["CARGO_BIN_EXE_spargen"];
    let _ = option_env! [ "CARGO_BIN_EXE_spargen" ];
}

#[test]
fn ungated_braces() {
    let _ = env! { "CARGO_BIN_EXE_spargen" };
}

const A_TUPLE: (&str,) = ("CARGO_BIN_EXE_spargen",);
const AN_ARRAY: [&str; 1] = ["CARGO_BIN_EXE_spargen"];
"##;
    assert_eq!(ungated_spawns(source), [18, 23, 33, 39, 44, 45, 50]);
}

#[test]
fn the_spawn_gate_scanner_accepts_a_file_or_item_gate() {
    let file_gated = r#"
//! Every test here spawns the binary.
#![cfg(feature = "cli")]
fn spawn() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}
"#;
    assert_eq!(ungated_spawns(file_gated), Vec::<usize>::new());

    let item_gated = r#"
/// A documented, gated module.
#[cfg( feature = "cli" )]
mod spawning {
    fn spawn() {
        let _ = env!("CARGO_BIN_EXE_spargen");
    }
}

#[cfg(feature = "cli")]
#[test]
fn spawns() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}

#[cfg(feature = "cli")]
fn spawns_through_other_delimiters() {
    let _ = env!["CARGO_BIN_EXE_spargen"];
    let _ = option_env! { "CARGO_BIN_EXE_spargen" };
}
"#;
    assert_eq!(ungated_spawns(item_gated), Vec::<usize>::new());
}

#[test]
fn the_target_gate_reader_reads_required_features() {
    let manifest = r#"
[package]
name = "example"

[[bin]]
name = "spargen"
required-features = ["cli"]

[[test]]
name = "cli"
required-features = ["cli"]

[[test]]
name = "elsewhere"
path = "tests/elsewhere/main.rs"
required-features = ["remote-fetch", "cli"]

[[test]]
name = "other"
required-features = ["remote-fetch"]
"#;
    assert_eq!(
        cli_gated_targets(manifest),
        BTreeSet::from([
            "tests/cli.rs".to_owned(),
            "tests/elsewhere/main.rs".to_owned()
        ])
    );
}

/// Every integration test that spawns the `spargen` binary compiles only under the `cli` feature.
///
/// `env!("CARGO_BIN_EXE_spargen")` expands to a path even when `cli` is off and the binary, whose
/// target requires `cli`, is not built, so an ungated spawn passes `mise run test`
/// (`--all-features`) and then fails, or runs a stale binary left at that path, under a `cargo
/// test` without the feature (#391). A spawning file is held either by its `[[test]]` target
/// carrying `required-features = ["cli"]` in `spargen/Cargo.toml` (as `tests/cli.rs` does), or by
/// [`ungated_spawns`]'s item-level rule (as `tests/carve.rs` and `tests/config.rs` are).
#[test]
fn every_test_that_spawns_the_binary_is_gated_on_the_cli_feature() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let gated_targets = cli_gated_targets(&read(&crate_dir.join("Cargo.toml")));

    let mut sources = Vec::new();
    let mut stack = vec![crate_dir.join("tests")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable tests directory") {
            let path = entry.expect("readable directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();

    let (mut by_target, mut by_item, mut violations) = (0, 0, Vec::new());
    for path in &sources {
        let source = read(path);
        if spawn_sites(&mask(&source)).is_empty() {
            continue;
        }
        let relative = path
            .strip_prefix(crate_dir)
            .expect("under the crate directory")
            .to_str()
            .expect("UTF-8 path")
            .replace(std::path::MAIN_SEPARATOR, "/");
        if gated_targets.contains(&relative) {
            by_target += 1;
            continue;
        }
        by_item += 1;
        violations.extend(
            ungated_spawns(&source)
                .into_iter()
                .map(|line| format!("{relative}:{line}")),
        );
    }
    assert!(
        by_target > 0 && by_item > 0,
        "expected spawning tests gated both by target and by item, found {by_target} and \
         {by_item}: the scanner has stopped reading the tests"
    );
    assert!(
        violations.is_empty(),
        "these spawns of `{SPAWN_VARIABLE}` compile without the `cli` feature, where the binary \
         is not built; gate the top-level item with `#[cfg(feature = \"cli\")]`, the file with \
         `#![cfg(feature = \"cli\")]`, or the target with `required-features = [\"cli\"]`: \
         {violations:#?}"
    );
}
