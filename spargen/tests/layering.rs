//! Structural invariants CLAUDE.md states but nothing checked: the subsystem layering DAG, the
//! shape of the embedded runtime sources, the file list that embeds them, and that the published
//! crate carries every file its sources include.
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

/// The inter-subsystem edges a file actually takes. Comment lines are skipped: a rustdoc intra-doc
/// link such as `[`crate::name`]` is prose, not a `use` edge, and counting it would make the lint
/// fire on documentation.
fn edges(source: &str, own: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("crate::") {
            rest = &rest[at + "crate::".len()..];
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if ident != own
                && (SUBSYSTEMS.contains(&ident.as_str())
                    || FACADE_PLUMBING.contains(&ident.as_str()))
            {
                found.insert(ident);
            }
        }
    }
    found
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
    for subsystem in SUBSYSTEMS {
        let module = root.join("spargen/src").join(subsystem).join("mod.rs");
        let declared = declared_deps(&read(&module), &module);

        let mut taken: BTreeSet<String> = BTreeSet::new();
        for file in subsystem_files(&root, subsystem) {
            taken.extend(edges(&read(&file), subsystem));
        }

        let undeclared: Vec<&String> = taken.difference(&declared).collect();
        assert!(
            undeclared.is_empty(),
            "subsystem `{subsystem}` reaches {undeclared:?} but its `//! layer-deps:` header \
             declares only {declared:?} — declare the edge (and add it to the DAG table in \
             lib.rs), or stop taking it"
        );

        for plumbing in FACADE_PLUMBING {
            assert!(
                !taken.contains(*plumbing),
                "subsystem `{subsystem}` reaches facade plumbing `{plumbing}`, inverting the \
                 layering"
            );
        }
    }
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
    let run = workspace_root().join("spargen/src/cli/run.rs");
    let source = read(&run);
    assert!(
        source.contains("//! layer-deps: facade"),
        "spargen/src/cli/run.rs must declare `//! layer-deps: facade`"
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
    for entry in std::fs::read_dir(&dir).expect("support-runtime/src exists") {
        let path = entry.expect("readable directory entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
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
        // Nothing but the test module may follow: the first item after the marker is `mod tests`,
        // and no column-0 item may appear after that module closes.
        assert!(
            tail.trim_start().starts_with("mod tests"),
            "{path:?} puts something other than `mod tests` after `#[cfg(test)]`"
        );
        let after_module = tail
            .rfind("\n}")
            .map(|at| &tail[at + 2..])
            .unwrap_or_default();
        assert!(
            after_module.trim().is_empty(),
            "{path:?} declares items after its `#[cfg(test)]` module: {:?}",
            after_module.trim()
        );
    }
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
