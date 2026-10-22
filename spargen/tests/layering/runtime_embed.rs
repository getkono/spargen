//! The embedded runtime sources: each carries its test module last and once, no comment above it
//! names what the embed strips, the embed list names every source, the variant counts its tests
//! enumerate match the enums, and generated output carries no test module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Spec};

use crate::{read, workspace_root};

/// The runtime sources are embedded by splitting on the literal `#[cfg(test)]` and keeping
/// everything before it (`codegen/runtime.rs`). That is only sound while each file contains the marker
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
        // An out-of-line `mod tests;` must have its file: reading it panics when it is missing.
        runtime_test_text(&path, &source);
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

/// The text of the test module of the runtime source at `path`, or `None` when `source` carries no
/// `#[cfg(test)]` marker. An inline module is the text after the marker; one declared out of line
/// (`mod tests;`) is read from `<stem>/tests.rs` beside `path`, and a missing file panics rather
/// than reading as an empty module, so a check over test text cannot pass by reading nothing.
fn runtime_test_text(path: &Path, source: &str) -> Option<String> {
    let (_, tail) = source.split_once("#[cfg(test)]")?;
    Some(if tail.trim() == "mod tests;" {
        read(&path.with_extension("").join("tests.rs"))
    } else {
        tail.to_owned()
    })
}

#[test]
fn the_test_text_reader_follows_an_out_of_line_module() {
    let path = Path::new("fixture.rs");
    let inline = "pub fn embedded() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
    assert_eq!(
        runtime_test_text(path, inline).as_deref(),
        Some("\nmod tests {\n    fn t() {}\n}\n")
    );
    assert_eq!(runtime_test_text(path, "pub fn embedded() {}\n"), None);

    let error = workspace_root().join("support-runtime/src/error.rs");
    let out_of_line =
        runtime_test_text(&error, "#[cfg(test)]\nmod tests;\n").expect("a marker yields test text");
    assert!(
        out_of_line.contains("const REQUEST_VARIANTS: usize"),
        "`mod tests;` beside error.rs reads support-runtime/src/error/tests.rs"
    );
}

#[test]
#[should_panic(expected = "must be readable")]
fn the_test_text_reader_rejects_a_missing_out_of_line_module() {
    let missing = workspace_root().join("support-runtime/src/no_such_source.rs");
    runtime_test_text(&missing, "pub fn embedded() {}\n#[cfg(test)]\nmod tests;\n");
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

/// Everything above `#[cfg(test)]` in a runtime source is embedded into every generated client,
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
    for (path, source) in &sources {
        let head = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(head, _)| head);
        if let Some(tests) = runtime_test_text(path, source) {
            test_only.extend(test_only_declarations(&tests));
        }
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
        .unwrap_or_else(|| panic!("`{marker}` is declared in support-runtime/src/error/tests.rs"))
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
    // The enums are embedded production code in `error.rs`; the counts and lists are in its
    // out-of-line test module, which must be there for the counts to be read at all.
    let path = workspace_root().join("support-runtime/src/error.rs");
    let source = read(&path);
    let tests = runtime_test_text(&path, &source)
        .expect("support-runtime/src/error.rs carries a `#[cfg(test)]` test module");
    for (header, count, list) in [
        (
            "pub enum RequestError {",
            "REQUEST_VARIANTS",
            "every_request_variant",
        ),
        ("pub enum Error<E> {", "ERROR_VARIANTS", "every_variant"),
    ] {
        let variants = enum_variants(&source, header);
        let declared = declared_count(&tests, count);
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
