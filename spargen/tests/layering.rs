//! Structural invariants CLAUDE.md states but nothing checked: the subsystem layering DAG, the
//! shape of the embedded runtime sources, and the file list that embeds them.
//!
//! `lib.rs` promises that "the future `xtask lint-layers` job diffs those declarations against the
//! actual inter-module `use` edges". The `xtask` member is still reserved, so until it exists the
//! diff happens here — it needs no new workspace member and runs under the ordinary `test` gate.

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
