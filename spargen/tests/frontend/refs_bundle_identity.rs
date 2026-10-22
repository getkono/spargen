//! Type identity across a bundle of files: contested type names, file-referenced `allOf` members,
//! and root components shadowing sub-file ones.

use super::refs::{generate_two_file_shapes, two_shape_root, SHAPE_ALPHA_YAML, SHAPE_BETA_YAML};
use super::*;

/// The *file* half of the identity key. Two different files each declaring a schema by the same name
/// must stay two types: the key is `file#pointer`, and dropping the file component would collapse
/// them onto one — which is a wrongly *shared* type, the failure `ensure_resolved`'s own fallback
/// comment calls worse than a duplicated one.
///
/// Nothing pinned this but an incidental corpus snapshot, which would report the collapse as a type
/// count and not as a wrong type.
#[test]
fn two_files_declaring_the_same_component_name_stay_two_types() {
    let code = generate_two_file_shapes(false);

    // Two declarations in two files: two types, and each keeps its own field. A collapse would take
    // one of these fields with it.
    let shapes = declared_types(&code, "Shape", |_| true);
    assert_eq!(
        shapes.len(),
        2,
        "two files declare `Shape`; they are different schemas and must stay two types: {shapes:?}"
    );
    // Each kept its own field. A collapse onto one key would have taken one of these with it, which
    // is the wrongly-*shared* type `ensure_resolved`'s fallback comment calls worse than a
    // duplicated one.
    assert!(code.contains("pub alpha:"), "{code}");
    assert!(code.contains("pub beta:"), "{code}");

    // *Which* struct owns which field, not merely that both exist somewhere. Counting types and
    // checking for both fields passes under either assignment of the two names, so on its own it
    // says nothing about what `types::Shape` denotes — and what `types::Shape` denotes is a public
    // API fact a consumer writes into their own code. The contest is decided by each schema's own
    // `file#pointer`, so `a.yaml`'s declaration keeps the bare name; that it survives reordering is
    // `a_contested_type_name_survives_reordering_the_paths_that_reach_it`'s to prove.
    assert_eq!(
        field_owner(&code, "pub alpha:").as_deref(),
        Some("Shape"),
        "the lower `file#pointer` owns the un-suffixed name: {code}"
    );
    assert_eq!(
        field_owner(&code, "pub beta:").as_deref(),
        Some("Shape93360b5f"),
        "and the other carries the pointer-seeded disambiguator: {code}"
    );
}

/// Issue #169: which of two same-named schemas keeps the bare type name was decided by the order
/// lowering met them in, so swapping two `paths` entries — no schema changed — swapped what
/// `types::Shape` denotes, a rename of public items. The name is now awarded on each schema's own
/// `file#pointer`, so both orders must emit the same types module with the same owner per field.
#[test]
fn a_contested_type_name_survives_reordering_the_paths_that_reach_it() {
    let forward = types_module(&generate_two_file_shapes(false));
    let swapped = types_module(&generate_two_file_shapes(true));

    for field in ["pub alpha:", "pub beta:"] {
        assert_eq!(
            field_owner(&forward, field),
            field_owner(&swapped, field),
            "reordering `paths` moved `{field}` to another type"
        );
    }
    assert_eq!(
        field_owner(&swapped, "pub alpha:").as_deref(),
        Some("Shape")
    );
    assert_eq!(
        declared_fields(&forward, "Shape"),
        declared_fields(&swapped, "Shape")
    );
    assert_eq!(
        declared_fields(&forward, "Shape93360b5f"),
        declared_fields(&swapped, "Shape93360b5f")
    );
}

/// A definition with no identity of its own — a boolean-schema `false` property, which lowers to a
/// `Never` type whose provenance falls back to the root document's own (empty) pointer — must never
/// take a contested name from a declared schema. The empty pointer is the lowest a plain ordering
/// could produce, so without the `anonymous` term of the rank the synthesized `Holder x` would take
/// `Holderx` from the component declared under that name. Both component orders are generated, so
/// arrival order cannot be what decides it.
#[test]
fn a_synthesized_type_with_no_identity_never_takes_a_name_from_a_declared_schema() {
    const HOLDER: &str = "    Holder:\n      type: object\n      properties:\n        x: false\n";
    const DECLARED: &str = "    Holderx:\n      type: object\n      required: [declared]\n      \
                            properties: { declared: { type: string } }\n";
    for (first, second) in [(HOLDER, DECLARED), (DECLARED, HOLDER)] {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\npaths: {{}}\n\
             components:\n  schemas:\n{first}{second}"
        ));
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let types = types_module(&code);
        assert_eq!(
            field_owner(&types, "pub declared:").as_deref(),
            Some("Holderx"),
            "the declared schema keeps the bare name: {types}"
        );
        assert!(
            types.contains("pub enum Holderx84222325 {}"),
            "and the synthesized `Never` carries the root pointer's disambiguator: {types}"
        );
    }
}

/// A document reached through `..`, outside the root document's directory, is ranked by its full
/// loaded path, which begins with the filesystem root and so sorts before any root-relative spelling.
/// `z.yaml` sits outside and `a.yaml` inside: a key spelled by file name alone would hand the bare
/// `Shape` to `a.yaml`, and one spelled by discovery order would move it when `paths` is reordered.
/// Both orders must give it to the outside file.
#[test]
fn a_contested_type_name_ranks_a_file_outside_the_root_directory_by_its_full_loaded_path() {
    for swap in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let root_dir = dir.join("spec");
        std::fs::create_dir_all(&root_dir).unwrap();
        let refs = [
            ("a", "./a.yaml#/components/schemas/Shape"),
            ("z", "../z.yaml#/components/schemas/Shape"),
        ];
        std::fs::write(root_dir.join("openapi.yaml"), two_shape_root(refs, swap)).unwrap();
        std::fs::write(root_dir.join("a.yaml"), SHAPE_BETA_YAML).unwrap();
        std::fs::write(dir.join("z.yaml"), SHAPE_ALPHA_YAML).unwrap();
        let out = dir.join("client.rs");
        let report = run_generate(&build(root_dir.join("openapi.yaml"), out.clone()));
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let code = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            field_owner(&code, "pub alpha:").as_deref(),
            Some("Shape"),
            "swap={swap}: the outside file's full path ranks first: {code}"
        );
        assert_eq!(
            field_owner(&code, "pub beta:").as_deref(),
            Some("Shape93360b5f"),
            "swap={swap}: {code}"
        );
    }
}

/// The nullability half of the memo entry. A shared sub-file component whose own schema admits
/// `null` must reach every use site as `Option<T>`; the memo stores nullability beside the id
/// precisely so a cache hit and a first use agree on it.
///
/// Forcing it false emits `pub maybe: Maybe` where `Option<Maybe>` is correct — a field that rejects
/// a payload the schema declares as valid — and nothing else in the suite is red.
#[test]
fn a_nullable_sub_file_component_reaches_every_use_as_an_option() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Holder",
        r##"
components:
  schemas:
    Holder:
      type: object
      required: [first, second]
      properties:
        first: { $ref: '#/components/schemas/Maybe' }
        second: { $ref: '#/components/schemas/Maybe' }
    Maybe:
      type: [string, 'null']
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    // `required` on both, so the `Option` can only come from the component's own nullability — and
    // it must come through on the second use (a memo hit) exactly as on the first.
    assert!(
        code.contains("pub first: Option<Maybe>"),
        "the first use must carry the component's nullability: {code}"
    );
    assert!(
        code.contains("pub second: Option<Maybe>"),
        "and so must the memo hit: {code}"
    );
}

/// Decision 23's invariant, applied to the sub-file spelling.
///
/// Every spelling now asks the DOCUMENT whether a `$ref` closes a cycle through a schema enclosing
/// it, so the verdict is order-independent by construction. With siblings on both edges it was
/// order-independent even when the sub-file spelling still asked `is_in_progress_root`: the
/// reservation is made *before* the body is lowered, so whichever of a mutually recursive pair is
/// lowered first, the re-entrant edge meets an open reservation. This fixture writes the same two
/// schemas in both orders and requires one verdict; the one-edge fixture below is the case that
/// lowering-state question got wrong.
#[test]
fn a_sub_file_cycle_verdict_does_not_depend_on_the_order_the_schemas_are_declared() {
    const A: &str = r##"    A:
      type: [object, 'null']
      properties:
        b:
          $ref: './lib.yaml#/components/schemas/B'
          type: [object, 'null']
          properties:
            extra: { type: string }
"##;
    // Siblings on BOTH back-edges, so the conjunction meets an open reservation whichever schema is
    // entered first. Siblings on one edge only are the harder case — whether the conjunction meets
    // one then depends on the entry point — and are pinned separately below.
    const B: &str = r##"    B:
      type: [object, 'null']
      properties:
        a:
          $ref: './lib.yaml#/components/schemas/A'
          type: [object, 'null']
          properties:
            extra: { type: string }
"##;

    let mut verdicts = Vec::new();
    for (order, lib) in [
        ("A first", format!("components:\n  schemas:\n{A}{B}")),
        ("B first", format!("components:\n  schemas:\n{B}{A}")),
    ] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/A", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{order}/{entry}: {report:#?}"
            );
            assert!(
                has_code(report, Code::AllOfIrreconcilable),
                "{order}/{entry}: {report:#?}"
            );
        }
        assert!(
            !code.contains("= ();"),
            "{order}: the failed intersection collapsed to the exact JSON null type: {code}"
        );
        // The outcome alone is held up by `intersect_types`' fail-closed arm, which rejects with
        // the generic empty-intersection wording whatever the back-edge predicate answered. Naming
        // the cause is what requires the predicate itself to be right, so it is asserted here too.
        let messages = messages_for(&generated, Code::AllOfIrreconcilable);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("closes a reference cycle")),
            "{order}: the rejection must name the recursion as the cause: {messages:?}"
        );
        verdicts.push((
            order,
            messages_for(&generated, Code::AllOfIrreconcilable)
                .iter()
                .map(|m| (*m).to_owned())
                .collect::<Vec<_>>(),
        ));
    }
    // Not merely the same outcome — the same diagnosis. Reordering a YAML map is a no-op in
    // OpenAPI, so it may not change what the tool says about the document either.
    assert_eq!(
        verdicts[0].1, verdicts[1].1,
        "reordering two sub-file schemas changed the diagnosis: {verdicts:?}"
    );
}

/// A sub-file component that shares its NAME with a root component in a cycle is not in that cycle.
///
/// The root declares `Item` and `Other`, each referring to the other. `lib.yaml` declares its own
/// `Item`, whose property carries siblings beside a `$ref` to the root's `Other`. Nothing reaches
/// `lib.yaml`'s `Item` from `Other`, so there is no cycle through it and the intersection is an
/// ordinary one. The back-edge test used to take the enclosing name from the sub-file pointer and
/// look it up in the ROOT document's map, where `Other` does reach an `Item` — the root's — and so
/// it rejected with `E013`. Renaming the sub-file component is the control: the document means the
/// same thing under either name and must get the same verdict.
#[test]
fn a_sub_file_component_named_like_a_root_cycle_member_is_not_a_back_edge() {
    for name in ["Item", "Leaf"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: './lib.yaml#/components/schemas/{name}' }} }}
  /r:
    get:
      operationId: getR
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Item' }} }}
components:
  schemas:
    Item:
      type: object
      properties:
        other: {{ $ref: '#/components/schemas/Other' }}
    Other:
      type: object
      properties:
        item: {{ $ref: '#/components/schemas/Item' }}
"##
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("lib.yaml"),
            format!(
                r##"
components:
  schemas:
    {name}:
      type: object
      properties:
        o:
          $ref: '#/components/schemas/Other'
          type: object
          properties:
            extra: {{ type: string }}
"##
            ),
        )
        .unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
        for (run, report) in [("generate", &generated), ("check", &checked)] {
            assert!(
                !has_code(report, Code::AllOfIrreconcilable),
                "{name}/{run}: no cycle passes through the sub-file component: {report:#?}"
            );
            assert!(report.outcome().is_success(), "{name}/{run}: {report:#?}");
        }
        assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
        let code = std::fs::read_to_string(&out).unwrap();
        assert!(
            code.contains("pub extra:"),
            "{name}: the sibling's property must survive the intersection: {code}"
        );
    }
}

/// A sub-file schema that reaches a **root document** component twice, by explicit file reference.
///
/// `ensure_resolved` routes a resolved target that lands inside the root document's own component
/// map back through `ensure_component`, so `components` stays that target's single identity. That
/// routing can look like code that executes but constrains nothing, and without this test it is:
/// disabling it leaves every other suite green and gives **`["RootOne", "RootOne55e60dbe"]`** —
/// two public types for one declared component, which is the precise defect the resolved-reference
/// memo exists to prevent.
///
/// The reason a second memo is not harmless is that it is a second *identity*: `resolved_components`
/// would key the same schema by `file#pointer` while `components` keys it by name, and neither would
/// see the other's entry.
#[test]
fn a_root_component_reached_by_file_reference_keeps_the_root_map_as_its_identity() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Holder' } }
components:
  schemas:
    RootOne:
      type: object
      required: [id]
      properties: { id: { type: string } }
"##,
    )
    .unwrap();
    // Both properties address the root document's `RootOne` from inside the sub-file, spelled as a
    // file reference — the only spelling that reaches the routing branch.
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Holder:
      type: object
      required: [first, second]
      properties:
        first: { $ref: './openapi.yaml#/components/schemas/RootOne' }
        second: { $ref: './openapi.yaml#/components/schemas/RootOne' }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();

    let roots = declared_types(&code, "RootOne", |_| true);
    assert_eq!(
        roots.len(),
        1,
        "one declared component, one generated type — a resolved reference that lands on a root \
         component must not take a second identity beside the root map: {roots:?}"
    );
    // Both uses reached it, so the count is not met by losing one of them.
    assert!(code.contains("pub first: RootOne"), "{code}");
    assert!(code.contains("pub second: RootOne"), "{code}");
}

/// The fan-out bound for an `allOf` member addressed by **file reference** (issue #163).
///
/// Such a member is not given a type of its own: `allOf` is an applicator, so the target's fields
/// are flattened into the enclosing object. That arm used to re-expand the target at every use, so
/// a branching reuse graph whose edges are `allOf: [$ref]` generated 2^(DEPTH+1) - 1 types — 2047
/// here, 32,767 at depth 14 — from DEPTH + 1 declared schemas, with no diagnostic. The target's
/// contribution is now expanded once per resolved `file#pointer` and replayed at every later use.
///
/// The exact count is one type for the response's own target plus one per `allOf` property site
/// per *declaration* — `L0`..`L(DEPTH-1)` each declare two — which is `2 * DEPTH + 1`. The shape
/// of every type is pinned too, so the bound cannot be met by dropping what a member contributes:
/// flattening still copies the target's fields into each enclosing struct.
#[test]
fn a_file_referenced_all_of_member_reused_through_a_deep_graph_is_expanded_once() {
    const DEPTH: usize = 10;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..DEPTH {
        lib.push_str(&format!(
            "    L{level}:\n      type: object\n      properties:\n        a: {{ allOf: [{{ $ref: \
             './lib.yaml#/components/schemas/L{next}' }}] }}\n        b: {{ allOf: [{{ $ref: \
             './lib.yaml#/components/schemas/L{next}' }}] }}\n",
            next = level + 1
        ));
    }
    lib.push_str(&format!(
        "    L{DEPTH}:\n      type: object\n      properties: {{ id: {{ type: string }} }}\n"
    ));

    let (generated, checked, code) = split("./lib.yaml#/components/schemas/L0", &lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    let types = types_module(&code);
    // Every lowered type here is named from an `L<digit>` hint; the client scaffolding that
    // `types_module` also carries is not.
    let declared = declared_types(&types, "L", |tail| {
        tail.starts_with(|character: char| character.is_ascii_digit())
    });
    assert_eq!(
        declared.len(),
        2 * DEPTH + 1,
        "{} declared schemas generated {} types: {declared:?}",
        DEPTH + 1,
        declared.len()
    );
    // Every generated struct is either an interior level (`a` and `b`, flattened from the member) or
    // the leaf level (`id`), so each wrapper carries exactly what its target contributes.
    let leaves = declared
        .iter()
        .filter(|ty| declared_fields(&types, ty) == ["id"])
        .count();
    let interior = declared
        .iter()
        .filter(|ty| declared_fields(&types, ty) == ["a", "b"])
        .count();
    assert_eq!(
        (interior, leaves),
        (2 * DEPTH - 1, 2),
        "every type must carry its target's flattened fields: {declared:?}"
    );
}

/// A replayed contribution is the member's, not the enclosing composition's.
///
/// Expanding a file-referenced `allOf` member once and replaying it is only sound if nothing the
/// enclosing `allOf` does to the merged fields — promoting a field to required, intersecting a
/// repeated property — leaks back into what the member contributes to the next use. `Strict`
/// requires `id` beside the member and `Loose` does not, so the two uses of one member must differ
/// exactly there, in whichever order they are lowered.
#[test]
fn a_file_referenced_all_of_member_contributes_the_same_fields_to_every_use() {
    let lib = r##"
components:
  schemas:
    Holder:
      type: object
      required: [strict, loose]
      properties:
        strict: { $ref: './lib.yaml#/components/schemas/Strict' }
        loose: { $ref: './lib.yaml#/components/schemas/Loose' }
    Strict:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Base'
        - required: [id]
    Loose:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Base'
        - properties: { note: { type: string } }
    Base:
      type: object
      properties: { id: { type: string } }
"##;
    let strict_first =
        "        strict: { $ref: './lib.yaml#/components/schemas/Strict' }\n        \
                        loose: { $ref: './lib.yaml#/components/schemas/Loose' }\n";
    let loose_first = "        loose: { $ref: './lib.yaml#/components/schemas/Loose' }\n        \
                       strict: { $ref: './lib.yaml#/components/schemas/Strict' }\n";
    assert!(lib.contains(strict_first));
    for order in [lib.to_owned(), lib.replace(strict_first, loose_first)] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &order);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        let types = types_module(&code);
        assert_eq!(declared_fields(&types, "Strict"), ["id"], "{types}");
        assert_eq!(declared_fields(&types, "Loose"), ["id", "note"], "{types}");
        let strict_id = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with("pub struct Strict "))
            .find(|line| line.starts_with("pub id:"))
            .map(str::to_owned);
        let loose_id = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with("pub struct Loose "))
            .find(|line| line.starts_with("pub id:"))
            .map(str::to_owned);
        let strict_id = strict_id.expect("`Strict` declares `id`");
        let loose_id = loose_id.expect("`Loose` declares `id`");
        let inner = strict_id
            .strip_prefix("pub id: ")
            .and_then(|ty| ty.strip_suffix(','))
            .expect("a field line");
        assert!(
            !inner.starts_with("Option<"),
            "`Strict` requires `id`: {types}"
        );
        assert_eq!(
            loose_id,
            format!("pub id: Option<{inner}>,"),
            "`Strict`'s requirement must not leak into `Loose` through the shared member, and \
             both uses must name the one type the member's property lowered to: {types}"
        );
    }
}

/// A file-referenced `allOf` member whose target is itself an `allOf`, or a bare `$ref` alias,
/// contributes what that target constrains (issue #306).
///
/// The member's target used to be read only for object keywords and scalar keywords. A body that
/// is `allOf: [...]` or a bare `$ref` carries neither, so it was taken for a pure annotation and
/// contributed nothing: `Holder` generated as an empty struct, both required properties gone, and
/// no diagnostic. The root-component spelling of the same shape always lowered the target first,
/// so the two spellings are held to the same fields here.
#[test]
fn a_file_referenced_all_of_member_whose_target_is_an_all_of_or_an_alias_contributes_its_fields() {
    const LIB: &str = r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: 'PREFIX#/components/schemas/Inner'
        - $ref: 'PREFIX#/components/schemas/Alias'
        - $ref: 'PREFIX#/components/schemas/Narrowed'
    Inner:
      allOf:
        - { type: object, required: [x], properties: { x: { type: string } } }
    Alias: { $ref: 'PREFIX#/components/schemas/Other' }
    Other: { type: object, required: [y], properties: { y: { type: integer } } }
    Narrowed:
      $ref: 'PREFIX#/components/schemas/Loose'
      required: [z]
    Loose: { type: object, properties: { z: { type: boolean } } }
"##;
    for (spelling, prefix) in [("explicit", "./lib.yaml"), ("bare", "")] {
        let lib = LIB.replace("PREFIX", prefix);
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().is_empty(),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        let types = types_module(&code);
        assert_eq!(
            declared_fields(&types, "Holder"),
            ["x", "y", "z"],
            "{spelling}: every member's target must contribute its properties: {types}"
        );
        // Each is required — by the `allOf` target, by the alias's target, and by the alias's own
        // `required` sibling — so none may be optional. The bare spelling lowers `Narrowed` to a
        // type of its own first, and that path still drops a `$ref`'s bare `required` sibling
        // (#140), so `z`'s requirement is asserted only where the target is flattened.
        let required: &[&str] = if prefix.is_empty() {
            &["x", "y"]
        } else {
            &["x", "y", "z"]
        };
        let holder: String = types
            .lines()
            .skip_while(|line| !line.trim_start().starts_with("pub struct Holder "))
            .take_while(|line| !line.trim_start().starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        for field in required {
            let ty = field_type(&holder, &format!("pub {field}:")).unwrap_or_default();
            assert!(
                !ty.is_empty() && !ty.starts_with("Option<"),
                "{spelling}: `{field}` is required: {types}"
            );
        }
    }
}

/// Expanding a file-referenced `allOf` member's target through its own `$ref` and `allOf` gives the
/// expansion a way back to where it started, and nothing reserves a type along the way. Every such
/// loop is rejected as the cycle it is, in the code the same loop reports elsewhere: a chain of bare
/// aliases is `E004`'s alias cycle, as `ensure_resolved` reports it; a loop through an `allOf` body
/// is `E013`'s recursive member, as the root document reports it. None may walk to the depth cap
/// (`E014`), and none may generate.
#[test]
fn a_file_referenced_all_of_member_that_reaches_itself_is_rejected_as_a_cycle() {
    let cases = [
        (
            "alias cycle",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: './lib.yaml#/components/schemas/B' }
    B: { $ref: './lib.yaml#/components/schemas/A' }
"##,
            Code::UnresolvedRef,
        ),
        (
            "allOf of itself",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Inner'
    Inner:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Inner'
"##,
            Code::AllOfIrreconcilable,
        ),
        (
            "alias into an allOf of the alias",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: './lib.yaml#/components/schemas/B' }
    B:
      allOf:
        - { type: object, properties: { x: { type: string } } }
        - $ref: './lib.yaml#/components/schemas/A'
"##,
            Code::AllOfIrreconcilable,
        ),
        // The same loop with the alias spelled as the sub-file's own bare component name. That
        // link reaches `B` through the component arm, which lowers `B` to a reserved type rather
        // than flattening it, so the loop still passes through `B`'s body and is `E013`, not an
        // alias cycle.
        (
            "bare alias into an allOf of the alias",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: '#/components/schemas/B' }
    B:
      allOf:
        - { type: object, properties: { x: { type: string } } }
        - $ref: './lib.yaml#/components/schemas/A'
"##,
            Code::AllOfIrreconcilable,
        ),
    ];
    for (shape, lib, expected) in cases {
        let (generated, checked, _) = split("./lib.yaml#/components/schemas/Holder", lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{shape}/{entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| d.code == expected
                    && (d.message.contains("cycle") || d.message.contains("recursive"))),
                "{shape}/{entry}: the rejection must be {expected:?} naming the cycle: {report:#?}"
            );
            assert!(
                !has_code(report, Code::SchemaNestingTooDeep),
                "{shape}/{entry}: a loop is a cycle, not a deep chain: {report:#?}"
            );
            if expected != Code::UnresolvedRef {
                assert!(
                    !has_code(report, Code::UnresolvedRef),
                    "{shape}/{entry}: a loop through a body is not an alias cycle: {report:#?}"
                );
            }
        }
    }
}

/// A file-referenced `allOf` member whose expansion lowers a property that refers back to that
/// member is an ordinary recursive type, not a member recursive through its own composition.
///
/// `Holder`'s member `Node` flattens `NodeBase`, whose `children` items refer to `Node`. Lowering
/// those items gives `Node` a type of its own, with its own reservation, and that lowering flattens
/// `NodeBase` again. It is a new type, so re-entering `NodeBase` there is not a loop of the outer
/// expansion. The root-document spelling of the same shape always generated, and every spelling is
/// held to it here.
#[test]
fn a_file_referenced_all_of_member_with_a_property_back_edge_generates() {
    const LIB: &str = r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: 'PREFIX#/components/schemas/Node'
    Node:
      allOf:
        - $ref: 'PREFIX#/components/schemas/NodeBase'
    NodeBase:
      type: object
      properties:
        children:
          type: array
          items: { $ref: 'PREFIX#/components/schemas/Node' }
"##;
    let mut runs = Vec::new();
    for (spelling, prefix) in [("explicit", "./lib.yaml"), ("bare", "")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Holder",
            &LIB.replace("PREFIX", prefix),
        );
        runs.push((spelling, generated, checked, code));
    }
    let root = format!(
        "openapi: 3.1.0\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         servers: [{{ url: 'https://e.com' }}]\n\
         paths:\n  \
         /u:\n    \
         get:\n      \
         operationId: getU\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '#/components/schemas/Holder' }}\n{}",
        LIB.replace("PREFIX", "").trim_start_matches('\n')
    );
    let (generated, code) = generate_with_code(&root);
    runs.push(("root", generated, check(&root), code));
    for (spelling, generated, checked, code) in &runs {
        for (entry, report) in [("generate", generated), ("check", checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a property back-edge is recursion a type boxes: {report:#?}"
            );
            assert!(
                !has_code(report, Code::AllOfIrreconcilable),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        assert_eq!(
            declared_fields(&types_module(code), "Holder"),
            ["children"],
            "{spelling}: `Holder` carries `NodeBase`'s property: {code}"
        );
    }
}

/// A long, acyclic chain of file-referenced `allOf` members through alias and `allOf` targets is
/// bounded by the depth cap, as a long `$ref` chain is, rather than by the stack.
#[test]
fn a_long_file_referenced_all_of_member_chain_still_exceeds_the_depth_cap() {
    let depth = 200;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..depth {
        let next = format!("./lib.yaml#/components/schemas/L{}", level + 1);
        if level % 2 == 0 {
            lib.push_str(&format!("    L{level}: {{ $ref: '{next}' }}\n"));
        } else {
            lib.push_str(&format!(
                "    L{level}: {{ allOf: [{{ $ref: '{next}' }}] }}\n"
            ));
        }
    }
    lib.push_str(&format!(
        "    L{depth}:\n      type: object\n      properties: {{ id: {{ type: string }} }}\n"
    ));
    lib.push_str("    Holder: { allOf: [{ $ref: './lib.yaml#/components/schemas/L0' }] }\n");
    let (generated, checked, _) = split("./lib.yaml#/components/schemas/Holder", &lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: {report:#?}"
        );
    }
}

/// One schema that is both a file-referenced `allOf` member and a direct `$ref` is lowered twice —
/// once to its own type by `ensure_resolved`, once flattened as a member — and the two copies must
/// not compete for one name.
///
/// The member's body is named for the target, so a hint equal to the one `ensure_resolved` gives
/// the direct type would put two types on one name, and the naming scope hands the bare name to
/// whichever lowering reaches it first. Reordering two properties would then move `Basemeta` (and,
/// for a scalar target, `Code` itself) from the direct type to the member's copy. Both orders must
/// generate the same set of names, with the direct type and its nested types on the bare ones.
#[test]
fn a_schema_used_as_file_referenced_all_of_member_and_direct_ref_names_both_copies_stably() {
    let lib = r##"
components:
  schemas:
    Holder:
      type: object
      properties:
        direct: { $ref: './lib.yaml#/components/schemas/Base' }
        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }
        code: { $ref: './lib.yaml#/components/schemas/Code' }
        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }
    Base:
      type: object
      properties:
        meta: { type: object, properties: { tag: { type: string } } }
    Code:
      type: string
      enum: [a, b]
"##;
    let direct_first = "        direct: { $ref: './lib.yaml#/components/schemas/Base' }\n        \
                        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }\n        \
                        code: { $ref: './lib.yaml#/components/schemas/Code' }\n        \
                        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }\n";
    let member_first = "        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }\n        \
                        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }\n        \
                        code: { $ref: './lib.yaml#/components/schemas/Code' }\n        \
                        direct: { $ref: './lib.yaml#/components/schemas/Base' }\n";
    assert!(lib.contains(direct_first));
    let mut names = Vec::new();
    for order in [lib.to_owned(), lib.replace(direct_first, member_first)] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &order);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        let types = types_module(&code);
        let field_type = |owner: &str, field: &str| {
            types
                .lines()
                .map(str::trim_start)
                .skip_while(|line| !line.starts_with(&format!("pub struct {owner} ")))
                .take_while(|line| !line.starts_with('}'))
                .find_map(|line| line.strip_prefix(&format!("pub {field}: ")))
                .map(|ty| ty.trim_end_matches(',').to_owned())
        };
        assert_eq!(
            field_type("Holder", "direct").as_deref(),
            Some("Option<Base>"),
            "{types}"
        );
        assert_eq!(
            field_type("Base", "meta").as_deref(),
            Some("Option<Basemeta>"),
            "{types}"
        );
        assert_eq!(
            field_type("Holder", "code").as_deref(),
            Some("Option<Code>"),
            "{types}"
        );
        let wrapped_meta = field_type("Holderwrapped", "meta").expect("the member's `meta`");
        assert_ne!(wrapped_meta, "Option<Basemeta>", "{types}");
        let mut declared: Vec<String> = types
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                line.strip_prefix("pub struct ")
                    .or_else(|| line.strip_prefix("pub enum "))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .map(str::to_owned)
            .collect();
        declared.sort();
        names.push((declared, wrapped_meta, field_type("Holder", "wrappedCode")));
    }
    assert_eq!(names[0], names[1], "lowering order renamed a type");
}

/// A sub-file's own `#/components/schemas/<name>` when the **root** document declares that name
/// too. The root wins, and that is now said out loud.
///
/// Before this change the unshadowed branch did not resolve at all — it was issue #107's silent
/// drop — so only one of the two branches existed and no precedence had to be chosen. Making the
/// sub-file reference resolve makes both live, and therefore chooses. Measured: with the root
/// declaring `Shared`, `lib.yaml`'s own `$ref: '#/components/schemas/Shared'` yields the **root's**
/// `Shared`; delete that one unrelated root component and the same unchanged `$ref` yields the
/// sub-file's. So adding a component to the root retargets a reference written in another file.
///
/// Root-wins is kept — changing it is a generated-API break — but "silently" is the fourth
/// behaviour the standing invariants forbid, and prose in a comment does not remove silence. `W011`
/// (a declaration that has no effect) is exactly what the sub-file's own `Shared` is here, and it
/// already has a code, a matrix cell and an `errors.md` row, so no ordinal moves.
#[test]
fn a_sub_file_component_shadowed_by_the_root_is_acknowledged() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Wrapper' } }
components:
  schemas:
    Shared:
      type: object
      required: [from_root]
      properties: { from_root: { type: string } }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Wrapper:
      type: object
      required: [inner]
      properties:
        inner: { $ref: '#/components/schemas/Shared' }
    Shared:
      type: object
      required: [from_sub_file]
      properties: { from_sub_file: { type: string } }
"##,
    )
    .unwrap();

    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let shadow = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .map(|d| d.message.clone())
            .find(|message| message.contains("Shared"));
        let shadow = shadow.unwrap_or_else(|| {
            panic!("{entry}: the shadowed declaration is not acknowledged: {report:#?}")
        });
        // Naming one namespace is not enough: the reader has to be told which declaration was read
        // and which one was not.
        assert!(
            shadow.contains("lib.yaml"),
            "{entry}: the message must name the file whose declaration lost: {shadow}"
        );
    }
    // Root-wins is the behaviour being documented, not changed.
    assert!(code.contains("from_root"), "{code}");
    assert!(!code.contains("from_sub_file"), "{code}");
}

/// The matched pair for the warning above: a reference that **names its own document** shadowed
/// nothing, and must not be told that it did.
///
/// Only the bare-fragment spelling can be shadowed. `#/components/schemas/Shared` addresses the
/// file it is written in, so writing it inside `lib.yaml` asks for `lib.yaml`'s `Shared` and is
/// given the root's — the whole of `W011`, and what `W011`'s own explain text scopes it to.
/// `./openapi.yaml#/components/schemas/Shared` asks for exactly one declaration and gets that one.
///
/// It warned on both. `ensure_resolved` routes any target that lands in the root's component map
/// back to `ensure_component` carrying the *referring site's* provenance, and the warning was
/// raised there on the strength of three facts that never included how the reference was written.
/// So a deliberately disambiguated reference drew a message quoting a `$ref` string absent from
/// that site, and a remedy — "address the file-local one explicitly with a relative-file
/// reference" — naming the form already in use. Worse, the same string through the nullable-alias
/// path correctly did *not* warn, so one reference got two answers depending only on whether the
/// root's component happened to be open at the time.
///
/// Two further cases vary the **sub-file's body** rather than the spelling, because varying the
/// spelling cannot reach every answer the warning gives.
///
/// The third drops `Shared` from `lib.yaml` altogether: a sub-file referring to a root component
/// it does not redeclare, which is the commonest shape a multi-file description has. Nothing is
/// shadowed, so nothing may be reported. That negative is decided in `Resolver::declares_locally`,
/// by the one line that asks whether the node the local address names actually exists —
/// `Bundle::reference_target` computes an address and never checks, so for an in-document fragment
/// it answers `Some` unconditionally. Without the third case that line is unheld: deleting it
/// leaves the whole workspace suite green while every such description gains a warning naming a
/// declaration it does not contain. The spelling cases cannot cover it, because their `shadows:
/// false` half is turned away by the bare-fragment gate in `warn_if_root_shadows_the_referring_file`
/// and never reaches `declares_locally` at all.
///
/// The fourth drives `W011`'s **own remedy**: "address the file-local one explicitly with a
/// relative-file reference". The explicit spelling driven above is `./openapi.yaml#…`, the *root's*
/// — the one the remedy does not recommend. `./lib.yaml#/components/schemas/Shared` is the one it
/// does, and it must both silence the warning and change the answer, binding the sub-file's
/// `Shared` rather than the root's. Otherwise a precedence change could leave spargen printing a
/// remedy that no longer works, with the suite green.
///
/// All four are driven against one pair of files so the fixture cannot pass by the warning having
/// gone dead: the bare-over-a-local-declaration case must still fire, in the same run.
#[test]
fn an_explicit_relative_file_reference_to_the_roots_component_is_not_a_shadowing() {
    let root = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Wrapper' } }
components:
  schemas:
    Shared:
      type: object
      required: [from_root]
      properties: { from_root: { type: string } }
"##;
    let lib = |spelling: &str, declares_shared: bool| {
        let shared = if declares_shared {
            "\n    Shared:\n      type: object\n      required: [from_sub_file]\n      \
             properties: { from_sub_file: { type: string } }"
        } else {
            ""
        };
        format!(
            r##"
components:
  schemas:
    Wrapper:
      type: object
      required: [inner]
      properties:
        inner: {{ $ref: '{spelling}' }}{shared}
"##
        )
    };

    for (case, spelling, declares_shared, shadows, from) in [
        (
            "the bare fragment, redeclared locally",
            "#/components/schemas/Shared",
            true,
            true,
            "from_root",
        ),
        (
            "the root addressed explicitly",
            "./openapi.yaml#/components/schemas/Shared",
            true,
            false,
            "from_root",
        ),
        (
            "the bare fragment, nothing local to shadow",
            "#/components/schemas/Shared",
            false,
            false,
            "from_root",
        ),
        (
            "the remedy's own spelling",
            "./lib.yaml#/components/schemas/Shared",
            true,
            false,
            "from_sub_file",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), root).unwrap();
        std::fs::write(dir.join("lib.yaml"), lib(spelling, declares_shared)).unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{case}/{entry}: {report:#?}"
            );
            let raised = report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect && d.message.contains("Shared"));
            assert_eq!(
                raised,
                shadows,
                "{case} ({spelling}, lib.yaml {} `Shared`)/{entry}: W011 must fire when the bare \
                 fragment is written over a declaration the referring file makes itself, and in \
                 no other case — every other case has one declaration in play, so nothing of the \
                 author's is without effect and the remedy would ask for a spelling that is \
                 already written or would change which declaration is read: {report:#?}",
                if declares_shared {
                    "declares"
                } else {
                    "does not declare"
                }
            );
        }

        // Which declaration was actually read. The first three cases all reach the root's, by
        // precedence or because it is the only one; the fourth is the remedy W011 prints, and the
        // point of driving it is that it does something different — it binds the sub-file's own
        // `Shared`. Read on the bound type's fields rather than its name, because the emitter
        // suffixes the sub-file's colliding copy and a substring test would pass on the wrong
        // answer.
        let absent = if from == "from_root" {
            "from_sub_file"
        } else {
            "from_root"
        };
        let inner = field_type(&code, "pub inner")
            .unwrap_or_else(|| panic!("{case}: no `inner` field at all: {code}"));
        let bound = inner
            .rsplit_once('<')
            .map_or(inner.as_str(), |(_, tail)| tail)
            .trim_end_matches('>');
        let fields = declared_fields(&code, bound);
        assert!(
            fields.iter().any(|field| field == from) && !fields.iter().any(|field| field == absent),
            "{case}: `inner` bound `{inner}`, whose fields are {fields:?}, expected `{from}`: \
             {code}"
        );
    }
}
