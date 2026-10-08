//! The generated client's re-export lists, held to each other and to a golden file.
//!
//! The names a generated client re-exports are written down in three places: the standalone
//! runtime crate's `pub use` list (`support-runtime/src/lib.rs`), the `pub use` list `emit_support`
//! writes into the embedded `support` module, and the root re-export lists in
//! `spargen/src/codegen/runtime.rs` that both the generated root `pub use` and the operation
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
//! - every runtime type a root re-export's public inherent signatures or associated types
//!   mention (a `FromStr::Err`, say) is re-exported at the root too, so a caller can write down
//!   the type of every value it is handed;
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
    // the moment it is added. The ID is `base` in camelCase, so a multi-word base such as
    // `DateParse` collides as `dateParse` rather than as `dateparse`, which names `DateparseError`.
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
    assert!(
        bases.contains(&"DateParse"),
        "the fixture no longer exercises a multi-word collision: {reexported:?}"
    );
    let camel = |base: &str| base[..1].to_ascii_lowercase() + &base[1..];
    let collisions: String = bases
        .iter()
        .map(|base| {
            let id = camel(base);
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
            camel(base)
        );
    }
}

/// Every item of the embedded `support` module, its nested modules' items included.
fn support_items_deep(items: &[Item]) -> Vec<&Item> {
    let mut out = Vec::new();
    for item in items {
        out.push(item);
        if let Item::Mod(module) = item {
            if let Some((_, inner)) = &module.content {
                out.extend(support_items_deep(inner));
            }
        }
    }
    out
}

/// Every identifier `tokens` mentions, at any depth of nesting.
fn idents(tokens: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
    for tree in tokens {
        match tree {
            proc_macro2::TokenTree::Ident(ident) => {
                out.insert(ident.to_string());
            }
            proc_macro2::TokenTree::Group(group) => idents(group.stream(), out),
            _ => {}
        }
    }
}

/// Runtime types a root re-export mentions that the root does not yet re-export, each with the
/// issue that decides it. An entry that stops being needed fails the test, so this only shrinks.
const UNNAMEABLE_TRACKED: &[(&str, &str)] = &[];

#[test]
fn every_runtime_type_a_root_reexport_names_is_nameable_at_the_root() {
    // The root re-export list has to cover every runtime type a caller meets through a type it can
    // already name, or the caller holds a value whose type it cannot write down: `"…".parse::<Date>()`
    // returns `Result<Date, <Date as FromStr>::Err>`, so `FromStr::Err` must be at the root too
    // (#256). The embedded `support` module is private, so the root is the only path there is.
    // Checked here: every public inherent method's signature, and every associated type of every
    // impl, on a type the root re-exports. A runtime type they mention must be re-exported too.
    let mut tracked_seen = BTreeSet::new();
    for spec in [PLAIN_SPEC.to_owned(), full_spec("")] {
        let generated = generate(&spec);
        let root = root_reexports(&generated);
        let items = support_items_deep(support_items(&generated));
        let runtime_types: BTreeSet<String> = items
            .iter()
            .filter_map(|item| match item {
                Item::Struct(item) if is_pub(&item.vis) => Some(item.ident.to_string()),
                Item::Enum(item) if is_pub(&item.vis) => Some(item.ident.to_string()),
                Item::Trait(item) if is_pub(&item.vis) => Some(item.ident.to_string()),
                Item::Type(item) if is_pub(&item.vis) => Some(item.ident.to_string()),
                _ => None,
            })
            .collect();
        assert!(
            !root.is_empty() && !runtime_types.is_empty(),
            "read {} root re-exports and {} public runtime types: an empty side means the scan \
             has stopped reading the generated module",
            root.len(),
            runtime_types.len()
        );
        let (mut impls_examined, mut members_examined) = (0usize, 0usize);
        let mut unnameable = BTreeSet::new();
        for item in &items {
            let Item::Impl(block) = item else { continue };
            let syn::Type::Path(self_ty) = block.self_ty.as_ref() else {
                continue;
            };
            let Some(self_name) = self_ty.path.segments.last().map(|s| s.ident.to_string()) else {
                continue;
            };
            if !root.contains(&self_name) {
                continue;
            }
            impls_examined += 1;
            let mut mentioned = BTreeSet::new();
            for member in &block.items {
                match member {
                    syn::ImplItem::Type(assoc) => {
                        members_examined += 1;
                        idents(quote::ToTokens::to_token_stream(&assoc.ty), &mut mentioned);
                    }
                    syn::ImplItem::Fn(method) if block.trait_.is_none() && is_pub(&method.vis) => {
                        members_examined += 1;
                        idents(
                            quote::ToTokens::to_token_stream(&method.sig),
                            &mut mentioned,
                        );
                    }
                    _ => {}
                }
            }
            for name in mentioned {
                if runtime_types.contains(&name) && !root.contains(&name) {
                    unnameable.insert(format!("{name} (through {self_name})"));
                }
            }
        }
        assert!(
            impls_examined > 0 && members_examined > 0,
            "examined {impls_examined} impls of root re-exported types and {members_examined} of \
             their public methods and associated types: the scan has stopped reading the impls"
        );
        unnameable.retain(|found| {
            let tracked = UNNAMEABLE_TRACKED.iter().any(|(entry, _)| entry == found);
            if tracked {
                tracked_seen.insert(found.clone());
            }
            !tracked
        });
        assert!(
            unnameable.is_empty(),
            "a root re-export exposes runtime types the root does not re-export: {unnameable:?}"
        );
    }
    let stale: Vec<_> = UNNAMEABLE_TRACKED
        .iter()
        .filter(|(entry, _)| !tracked_seen.contains(*entry))
        .collect();
    assert!(
        stale.is_empty(),
        "tracked exceptions no longer needed; remove them: {stale:?}"
    );
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
