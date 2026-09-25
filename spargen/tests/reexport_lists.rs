//! The generated client's re-export lists, held to each other and to a golden file.
//!
//! The names a generated client re-exports are written down in three places: the standalone
//! runtime crate's `pub use` list (`support-runtime/src/lib.rs`), the `pub use` list `emit_support`
//! writes into the embedded `support` module, and the root re-export lists in
//! `spargen/src/codegen/emit.rs` that both the generated root `pub use` and the operation
//! error-type naming read. Nothing but this suite holds them to each other: a name dropped from the
//! embedded module still compiles when no generated code happens to use it, and a name added to the
//! root surface changes the public API of every generated client without any other gate noticing.
//!
//! Everything here reads the *emitted* module rather than spargen's source, so what is pinned is
//! what a consumer receives:
//!
//! - the embedded `support` module re-exports exactly what the runtime crate's `lib.rs` does,
//!   module by module;
//! - every root re-export resolves to something the `support` module re-exports;
//! - no operation's generated error type shadows a root re-export, even for operation IDs chosen
//!   to collide with each one;
//! - the root `pub use` surface is a golden file (`snapshots/reexport_lists__root_surface.snap`),
//!   so any change to the generated public runtime surface is a reviewable diff.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Spec};
use syn::{Item, UseTree, Visibility};

/// One JSON operation and nothing conditional: only the unconditional root re-exports appear.
const PLAIN_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Plain, version: 1.0.0 }
servers:
  - url: https://example.com
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { type: string }
"##;

/// A body in XML, a sequential response, and a date-time field: every conditionally embedded runtime
/// module and every conditional re-export is emitted. `{collisions}` is replaced with operations
/// whose IDs would name an error type after a root re-export.
const FULL_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Full, version: 1.0.0 }
servers:
  - url: https://example.com
paths:
  /xml:
    get:
      operationId: getXml
      responses:
        "200":
          description: ok
          content:
            application/xml:
              schema: { $ref: "#/components/schemas/XmlBody" }
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: events
          content:
            text/event-stream:
              schema: { type: string }
  /when:
    get:
      operationId: getWhen
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Stamped" }
{collisions}
components:
  schemas:
    XmlBody:
      type: object
      properties:
        value: { type: string }
    Stamped:
      type: object
      required: [at]
      properties:
        at: { type: string, format: date-time }
"##;

/// Generate `spec` and parse the emitted module.
fn generate(spec: &str) -> syn::File {
    let dir = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(dir.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, spec).unwrap();
    let out = Utf8PathBuf::from_path_buf(dir.path().join("api.rs")).unwrap();
    let report = spargen::generate(
        &Spec::new(spec_path)
            .build(out.clone())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    syn::parse_file(&std::fs::read_to_string(&out).unwrap()).expect("generated module parses")
}

fn full_spec(collisions: &str) -> String {
    FULL_SPEC.replace("{collisions}", collisions)
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Every `(path, name)` a `use` tree brings in, `path` being the segments before the leaf.
fn use_leaves(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, String)>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            use_leaves(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => out.push((prefix.clone(), name.ident.to_string())),
        UseTree::Rename(rename) => out.push((prefix.clone(), rename.rename.to_string())),
        UseTree::Glob(_) => out.push((prefix.clone(), "*".to_owned())),
        UseTree::Group(group) => {
            for item in &group.items {
                use_leaves(item, prefix, out);
            }
        }
    }
}

/// The `pub use` items among `items`, as `source module -> names`.
fn pub_reexports(items: &[Item]) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for item in items {
        let Item::Use(item) = item else { continue };
        if !is_pub(&item.vis) {
            continue;
        }
        let mut leaves = Vec::new();
        use_leaves(&item.tree, &mut Vec::new(), &mut leaves);
        for (path, name) in leaves {
            map.entry(path.join("::")).or_default().insert(name);
        }
    }
    map
}

/// The items of the generated module's embedded `support` module.
fn support_items(file: &syn::File) -> &[Item] {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Mod(module) if module.ident == "support" => {
                module.content.as_ref().map(|(_, items)| items.as_slice())
            }
            _ => None,
        })
        .expect("the generated module embeds a `support` module")
}

/// The names the generated root re-exports out of the embedded runtime. (The root's only other
/// `pub use` is the glob over its own generated blocking facade.)
fn root_reexports(file: &syn::File) -> BTreeSet<String> {
    pub_reexports(&file.items)
        .remove("support")
        .expect("the generated root re-exports from `support`")
}

/// The names of the items the generated root defines itself.
fn root_definitions(file: &syn::File) -> BTreeSet<String> {
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) => Some(item.ident.to_string()),
            Item::Enum(item) => Some(item.ident.to_string()),
            Item::Type(item) => Some(item.ident.to_string()),
            Item::Trait(item) => Some(item.ident.to_string()),
            Item::Fn(item) => Some(item.sig.ident.to_string()),
            Item::Mod(item) => Some(item.ident.to_string()),
            Item::Const(item) => Some(item.ident.to_string()),
            Item::Static(item) => Some(item.ident.to_string()),
            Item::Union(item) => Some(item.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// The generated root's `pub use` items, printed exactly as they are emitted.
fn root_surface(file: &syn::File) -> String {
    let items = file
        .items
        .iter()
        .filter(|item| matches!(item, Item::Use(item) if is_pub(&item.vis)))
        .cloned()
        .collect();
    prettyplease::unparse(&syn::File {
        shebang: None,
        attrs: Vec::new(),
        items,
    })
}

#[test]
fn the_embedded_support_module_reexports_exactly_what_the_runtime_crate_does() {
    let lib_rs = std::fs::read_to_string(
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../support-runtime/src/lib.rs"),
    )
    .unwrap();
    let runtime = pub_reexports(&syn::parse_file(&lib_rs).unwrap().items);
    let generated = generate(&full_spec(""));
    let embedded = pub_reexports(support_items(&generated));
    assert!(
        !runtime.is_empty(),
        "no `pub use` found in support-runtime/src/lib.rs"
    );
    assert_eq!(
        embedded, runtime,
        "the `pub use` list `emit_support` writes into the embedded `support` module (left) \
         differs from support-runtime/src/lib.rs (right)"
    );
}

#[test]
fn every_root_reexport_names_something_the_support_module_reexports() {
    for spec in [PLAIN_SPEC.to_owned(), full_spec("")] {
        let generated = generate(&spec);
        let support: BTreeSet<String> = pub_reexports(support_items(&generated))
            .into_values()
            .flatten()
            .collect();
        let missing: Vec<_> = root_reexports(&generated)
            .into_iter()
            .filter(|name| !support.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "the root re-exports {missing:?}, which the embedded `support` module does not"
        );
    }
}

#[test]
fn no_operation_error_type_shadows_a_root_reexport() {
    // One operation per root re-export named `{Base}Error`, with the operation ID that would name
    // its error type exactly that. Derived from the emitted surface, so a new re-export is covered
    // the moment it is added.
    let reexported = root_reexports(&generate(&full_spec("")));
    let bases: Vec<&str> = reexported
        .iter()
        .filter_map(|name| name.strip_suffix("Error"))
        .filter(|base| !base.is_empty())
        .collect();
    assert!(
        bases.contains(&"Request"),
        "the fixture no longer exercises a collision: {reexported:?}"
    );
    let collisions: String = bases
        .iter()
        .map(|base| {
            let id = base.to_ascii_lowercase();
            format!(
                "  /collide/{id}:\n    get:\n      operationId: {id}\n      responses:\n        \
                 \"200\":\n          description: ok\n        \"404\":\n          description: \
                 missing\n          content:\n            application/json:\n              schema: \
                 {{ type: string }}\n"
            )
        })
        .collect();
    let generated = generate(&full_spec(&collisions));
    let defined = root_definitions(&generated);
    let root = root_reexports(&generated);
    let shadowed: Vec<_> = defined.intersection(&root).collect();
    assert!(
        shadowed.is_empty(),
        "generated items shadow root re-exports: {shadowed:?}"
    );
    for base in bases {
        let widened = format!("{base}OperationError");
        assert!(
            defined.contains(&widened),
            "operation `{}` did not get the widened error type `{widened}`",
            base.to_ascii_lowercase()
        );
    }
}

#[test]
fn the_generated_root_surface_matches_its_golden_file() {
    let surface = format!(
        "# A spec with no conditional runtime module\n\n{}\n\
         # A spec with XML, sequential, and date-time bodies\n\n{}",
        root_surface(&generate(PLAIN_SPEC)),
        root_surface(&generate(&full_spec(""))),
    );
    insta::assert_snapshot!("root_surface", surface);
}
