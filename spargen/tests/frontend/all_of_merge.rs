//! `allOf` member merging: single, `$ref`-plus-inline, nested and sibling-property members merge
//! into one struct, repeated properties intersect, irreconcilable members are `E013`, a `$ref`
//! member's shape-bearing siblings and a required-only field take part in the merge, `E013`'s
//! explain covers its causes, and a rejected or superseded merge reports no `default` or `xml`
//! diagnostic of its own.

use super::*;

/// A single-member `allOf` now MERGES into one typed struct instead of being rejected (E013 is
/// repurposed to mean "irreconcilable composition"). Generation succeeds with no E013.
#[test]
fn all_of_single_member_merges_into_struct() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Composed:
      allOf:
        - type: object
          properties:
            a: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Composed"), ["a"], "{types}");
    assert_eq!(
        field_owner(&types, "pub a:").as_deref(),
        Some("Composed"),
        "{types}"
    );
}

/// `allOf: [{$ref: Base}, {properties: {extra}}]` flattens the referenced component's fields plus
/// the inline member's fields into one struct.
#[test]
fn all_of_ref_plus_inline_members_merge() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
        - type: object
          properties:
            extra: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // `Derived` carries `Base`'s required `id` (plain) and the inline member's optional `extra`.
    let types = types_module(&code);
    assert_eq!(
        declared_fields(&types, "Derived"),
        ["id", "extra"],
        "{types}"
    );
    let derived = &types[types
        .find("pub struct Derived ")
        .expect("Derived is a struct")..];
    assert_eq!(
        field_type(derived, "pub id:").as_deref(),
        Some("Baseid"),
        "{types}"
    );
    assert_eq!(
        field_type(derived, "pub extra:").as_deref(),
        Some("Option<DerivedMember1extra>"),
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "DerivedMember1extra").as_deref(),
        Some("i64"),
        "{types}"
    );
}

/// A nested `allOf` (an `allOf` member that itself has an `allOf`) flattens recursively into one
/// struct.
#[test]
fn all_of_nested_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nested:
      allOf:
        - allOf:
            - type: object
              properties:
                a: { type: string }
        - type: object
          properties:
            b: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // One struct carrying the inner `allOf`'s `a` beside the outer member's `b`.
    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Nested"), ["a", "b"], "{types}");
}

/// `allOf` beside the enclosing schema's own sibling `properties`: both sets of fields merge.
#[test]
fn all_of_beside_sibling_properties_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Sibling:
      type: object
      properties:
        own: { type: string }
      allOf:
        - type: object
          properties:
            base: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // Both sets of fields land in the one struct.
    let types = types_module(&code);
    assert_eq!(
        declared_fields(&types, "Sibling"),
        ["base", "own"],
        "{types}"
    );
}

/// Repeated properties in an `allOf` are intersections. Compatible refinements retain the narrower
/// typed shape recursively: integer within number, enum within string, non-null within nullable,
/// exact null, and nested array/object item constraints.
#[test]
fn all_of_recursively_intersects_compatible_property_types() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Refined:
      allOf:
        - type: object
          properties:
            run_id: { type: number }
            status: { type: string }
            marker: { type: [string, "null"] }
            items:
              type: array
              items: { type: [object, "null"] }
        - type: object
          properties:
            run_id: { type: integer }
            status: { type: string, enum: [queued, complete] }
            marker: { type: "null" }
            items:
              type: array
              items:
                type: object
                required: [name]
                properties:
                  name: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // Each property keeps the narrower side of its intersection, read through the merged
    // struct's own field types rather than from a type name appearing somewhere.
    let types = types_module(&code);
    let refined = &types[types
        .find("pub struct Refined ")
        .expect("Refined is a struct")..];
    let target = |field: &str| {
        let ty = field_type(refined, &format!("pub {field}:"))
            .unwrap_or_else(|| panic!("`Refined` has no `{field}` field: {types}"));
        let inner = ty
            .strip_prefix("Option<")
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or_else(|| panic!("optional `{field}` is `{ty}`: {types}"));
        alias_target(&types, inner).unwrap_or_else(|| format!("enum or struct {inner}"))
    };
    // integer within number.
    assert_eq!(target("run_id"), "i64", "{types}");
    // enum within string: a two-variant enum, not a plain `String`.
    let status = field_type(refined, "pub status:").expect("a status field");
    let status = status.trim_start_matches("Option<").trim_end_matches('>');
    assert_eq!(
        enum_variants(&types, status),
        ["Queued", "Complete"],
        "{types}"
    );
    // exact null within nullable string.
    assert_eq!(target("marker"), "()", "{types}");
    // A non-null item struct carrying `name`, within nullable untyped items.
    let items = target("items");
    let item = items
        .strip_prefix("Vec<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or_else(|| panic!("`items` is `{items}`: {types}"));
    assert!(
        !item.starts_with("Option<"),
        "the item stayed nullable: {types}"
    );
    assert_eq!(declared_fields(&types, item), ["name"], "{types}");
}

#[test]
fn e013_all_of_conflicting_property_types_rejected() {
    let report = generate(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Mixing an object member with a scalar member has no single representable type → E013.
#[test]
fn e013_all_of_object_scalar_mix_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      allOf:
        - type: object
          properties:
            a: { type: string }
        - type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// The boolean schema `false` admits no value, so an `allOf` holding it admits none either. It is
/// rejected as the empty composition it is, at a component root and inside a property alike, and
/// located at the `false` entry itself: its pointer and the span of the `false` it names (#832).
#[test]
fn e013_an_all_of_member_that_is_false() {
    for (place, schemas, pointer) in [
        (
            "component",
            "    Probe:\n      allOf: [ { type: object, properties: { a: { type: string } } }, false \
             ]\n",
            "/components/schemas/Probe/allOf/1",
        ),
        (
            "property",
            "    Probe:\n      type: object\n      properties:\n        p: { allOf: [ { type: \
             string }, false ] }\n",
            "/components/schemas/Probe/properties/p/allOf/1",
        ),
    ] {
        let spec = with_schemas("3.1.0", schemas);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{place}: {report:#?}");
            assert_eq!(
                messages_for(&report, Code::AllOfIrreconcilable),
                ["an `allOf` member is `false`"],
                "{place}: {report:#?}"
            );
            let diagnostic = report
                .diagnostics()
                .iter()
                .find(|d| d.code == Code::AllOfIrreconcilable)
                .expect("the E013 asserted above");
            assert_eq!(diagnostic.pointer.as_str(), pointer, "{place}: {report:#?}");
            let span = diagnostic.span.expect("the `false` entry has a span");
            assert_eq!(
                &spec[span.start.offset..span.end.offset],
                "false",
                "{place}: {report:#?}"
            );
        }
    }
}

/// Lay out a root document whose `Wrap` component is `allOf: [member]`, beside a `lib.yaml`
/// sub-file, and run both entry points. Returns `(generate, check, generated source)`.
fn all_of_member_layout(member: &str) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  \
             /w:\n    \
             get:\n      \
             operationId: getW\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Wrap' }}\n\
             components:\n  \
             schemas:\n    \
             Wrap:\n      \
             allOf:\n        \
             - {member}\n    \
             Leaf:\n      \
             type: object\n      \
             properties: {{ a: {{ type: string }} }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        "Leaf:\n  type: object\n  properties: { a: { type: string } }\n",
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    (generated, checked, code)
}

/// Issue #185: an `allOf` member that is a `$ref` with shape-bearing siblings used to contribute
/// its target alone. `gather_member` returned as soon as it had pushed the target, so the member's
/// own `properties`/`required`/`type` never reached the merge: a property vanished from the
/// generated client with no diagnostic, and a member contradicting its target generated a type
/// that looked inhabited. `$ref` is an applicator, so the member is the target AND its siblings,
/// and inside an `allOf` that is two more conjuncts of the same merge.
///
/// Both local arms are driven: the root-component spelling and the file-reference spelling, which
/// takes the inlining arm. The remote arm has its own fixture,
/// `refs_remote::a_remote_all_of_member_ref_intersects_its_shape_bearing_siblings`.
#[test]
fn an_all_of_member_ref_intersects_its_shape_bearing_siblings() {
    for target in ["#/components/schemas/Leaf", "./lib.yaml#/Leaf"] {
        // The dropped-property case: the member adds a required field its target lacks.
        let (generated, checked, code) = all_of_member_layout(&format!(
            "{{ $ref: '{target}', type: object, properties: {{ dropped: {{ type: integer }} }}, \
             required: [dropped] }}"
        ));
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target} via {entry}: {report:#?}"
            );
        }
        assert_eq!(
            declared_fields(&code, "Wrap"),
            vec!["a", "dropped"],
            "{target}: the member's own property must survive beside its target's: {code}"
        );
        let dropped = field_type(&code, "pub dropped").unwrap_or_default();
        assert!(
            !dropped.starts_with("Option<"),
            "{target}: the member's `required` must survive too: {code}"
        );
        assert!(
            code.contains(&format!("pub type {dropped} = i64;")),
            "{target}: the member's property keeps its own `integer` type: {code}"
        );

        // The contradictory cases, which are empty intersections and must be E013 at `Wrap`: a
        // property both sides type differently that the member requires, and a scalar sibling
        // beside an object target.
        for siblings in [
            "properties: { a: { type: integer } }, required: [a]",
            "type: string",
        ] {
            let (generated, checked, _) =
                all_of_member_layout(&format!("{{ $ref: '{target}', {siblings} }}"));
            for (entry, report) in [("generate", &generated), ("check", &checked)] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{target} + `{siblings}` via {entry} must be rejected: {report:#?}"
                );
                let pointers: Vec<&str> = report
                    .diagnostics()
                    .iter()
                    .filter(|d| d.code == Code::AllOfIrreconcilable)
                    .map(|d| d.pointer.as_str())
                    .collect();
                assert_eq!(
                    pointers,
                    vec!["/components/schemas/Wrap"],
                    "{target} + `{siblings}` via {entry}: {report:#?}"
                );
            }
        }

        // An optional contradiction empties only the property, not the object. The one-member
        // spelling must type it exactly as the two-member spelling of the same conjunction does
        // (`allOf: [{$ref}, {properties}]`), and never as the target's `String`.
        let (generated, _, one_member) = all_of_member_layout(&format!(
            "{{ $ref: '{target}', properties: {{ a: {{ type: integer }} }} }}"
        ));
        assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
        let (split_report, _, two_members) = all_of_member_layout(&format!(
            "{{ $ref: '{target}' }}\n        - {{ properties: {{ a: {{ type: integer }} }} }}"
        ));
        assert_ne!(
            split_report.outcome(),
            Outcome::Rejected,
            "{split_report:#?}"
        );
        assert_ne!(
            field_type(&one_member, "pub a").as_deref(),
            Some("Option<String>"),
            "{target}: the contradicted property must not keep the target's type: {one_member}"
        );
        // Field by field rather than whole modules: the two spellings name the dead per-member
        // alias of `a: integer` differently (`WrapMember0Constrainta`, `WrapMember1a`).
        assert_eq!(
            declared_fields(&one_member, "Wrap"),
            declared_fields(&two_members, "Wrap"),
            "{target}: one member and two members are one conjunction"
        );
        assert_eq!(
            field_type(&one_member, "pub a:"),
            field_type(&two_members, "pub a:"),
            "{target}: one member and two members are one conjunction"
        );
    }
}

/// A field an object carries only because its `required` names the key is no declaration of the
/// property, so in a composition it takes its type from the other sides as well: a side's
/// `additionalProperties` value schema constrains every key that side does not declare, and a
/// side that declares the property supplies its type and metadata outright, exactly as it does for
/// the same composition with no `required` in it. Before, the placeholder's own type (unconstrained,
/// or the requiring side's `additionalProperties` value) was intersected like a declaration: a typed
/// `additionalProperties` on the other side never reached it, so a string-valued key became
/// `serde_json::Value`; and a declaration on the other side was intersected with the requiring
/// side's value schema, which rejected a composition the same document without the `required`
/// generates.
#[test]
fn a_required_only_field_takes_its_type_from_every_side_of_a_composition() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n    Labels: { \
                        type: object, additionalProperties: { type: string } }\n    Base: { type: \
                        object, properties: { id: { type: integer } } }\n    Req: { type: object, \
                        required: [a] }\n";
    let types_for = |schema: &str| {
        let spec = format!("{HEAD}    Thing: {schema}\n");
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{schema}: {report:#?}"
        );
        types_module(&code)
    };
    // The field of `Thing` itself: `Req` carries an `a` of its own.
    let thing_field = |types: &str, name: &str| {
        let prefix = format!("pub {name}: ");
        types
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with("pub struct Thing "))
            .take_while(|line| *line != "}")
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
    };
    let field = |schema: &str, name: &str| {
        let types = types_for(schema);
        let ty = thing_field(&types, name)
            .unwrap_or_else(|| panic!("`{schema}` generated no `{name}` field:\n{types}"))
            .trim_end_matches(',')
            .to_owned();
        // The field is written through its own named alias; report what that alias names.
        let alias = format!("pub type {ty} = ");
        let resolved = types
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&alias))
            .map_or(ty.clone(), |rest| rest.trim_end_matches(';').to_owned());
        format!("pub {name}: {resolved},")
    };

    // The other side's typed `additionalProperties` reaches the required key, in the `$ref`
    // spelling and in both orders of the `allOf` spelling.
    for schema in [
        "{ $ref: '#/components/schemas/Labels', required: [a] }",
        "{ allOf: [{ $ref: '#/components/schemas/Labels' }, { required: [a] }] }",
        "{ allOf: [{ required: [a] }, { $ref: '#/components/schemas/Labels' }] }",
        "{ allOf: [{ $ref: '#/components/schemas/Labels' }, { $ref: '#/components/schemas/Req' }] }",
    ] {
        assert_eq!(field(schema, "a"), "pub a: String,", "{schema}");
    }

    // The other side's declaration is the property's type (required, so no `Option`), and the
    // requiring side's value schema does not reach it, as it does not without the `required`.
    for schema in [
        "{ allOf: [{ $ref: '#/components/schemas/Base' }, { type: object, additionalProperties: \
         { type: string }, required: [id] }] }",
        "{ allOf: [{ type: object, additionalProperties: { type: string }, required: [id] }, \
         { $ref: '#/components/schemas/Base' }] }",
        "{ $ref: '#/components/schemas/Base', additionalProperties: { type: string }, \
         required: [id] }",
    ] {
        assert_eq!(field(schema, "id"), "pub id: i64,", "{schema}");
    }

    // A component's required-only field keeps giving way when the component is copied into an
    // `allOf`: the later member's declaration supplies the metadata.
    let types = types_for(
        "{ allOf: [{ $ref: '#/components/schemas/Req' }, { type: object, properties: { a: { \
         type: string, deprecated: true } } }] }",
    );
    let lines: Vec<&str> = types
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("pub struct Thing "))
        .collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("pub a: "))
        .unwrap_or_else(|| panic!("no `a` field:\n{types}"));
    assert!(
        lines[..at]
            .iter()
            .rev()
            .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
            .any(|line| line.contains("Deprecated per the spec")),
        "the declaring member's `deprecated` was lost to the component's placeholder:\n{types}"
    );

    // Every side constrains the undeclared key, so value schemas that share no value leave no
    // value for the required key, even where `additionalProperties: false` keeps the two maps
    // themselves from being intersected.
    let spec = format!(
        "{HEAD}    Thing: {{ allOf: [{{ $ref: '#/components/schemas/Labels' }}, {{ type: object, \
         additionalProperties: false }}, {{ type: object, additionalProperties: {{ type: integer \
         }}, required: [a] }}] }}\n"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("property `a` is required but no `allOf` member declares it"),
            "{messages:?}"
        );
    }
}

/// `E013`'s explain is what `spargen explain E013` prints, and this change REWROTE it: the code was
/// repurposed from "irreconcilable allOf composition" to a composition-generic one, and the text
/// moved with it. Nothing held the new text to the code. Measured before this fixture existed:
/// reverting the explain to its `allOf`-only wording, and replacing it with text flatly
/// contradicting the code ("a `$ref` replaces the containing schema: its sibling keywords are
/// discarded, never intersected"), BOTH survived the entire suite.
///
/// The assertions are claims, not typography — a string-equality snapshot would pin the wording and
/// go stale on every edit without ever catching a false clause. Each one below is a promise some
/// other fixture in this suite enforces against the code, so the two cannot drift apart silently.
#[test]
fn the_composition_explain_covers_every_cause_that_reports_it() {
    let explain = Code::AllOfIrreconcilable.explain();

    // Both constructs that report it, and that `$ref` siblings are INTERSECTED rather than
    // discarded — the sentence the whole change exists to make true.
    assert!(explain.contains("`allOf`"), "{explain}");
    assert!(
        explain.contains("`$ref` is an applicator"),
        "the explain must say why a `$ref`'s siblings are not discarded: {explain}"
    );
    assert!(
        explain.contains("instead of being discarded"),
        "the explain must say the siblings are intersected rather than discarded: {explain}"
    );

    // The two-tier sibling-keyword rule, pinned against the code by
    // `every_sibling_keyword_the_explain_names_is_actually_intersected`.
    assert!(
        explain.contains("A sibling bears a shape of its own through"),
        "{explain}"
    );
    // The category rule for untyped applicators, and the consequences
    // `a_ref_sibling_applicator_establishes_its_category_and_keeps_the_targets_null`,
    // `an_untyped_union_sibling_refines_only_the_branches_of_its_category` and
    // `a_required_name_no_property_declares_is_still_required` pin against the code.
    assert!(
        explain.contains("establish an object and the array keywords"),
        "{explain}"
    );
    assert!(
        explain.contains("the target's nullability stands"),
        "{explain}"
    );
    assert!(
        explain.contains("refine only the branches of their own category"),
        "{explain}"
    );
    assert!(
        explain.contains("are acknowledged with `W011` rather than rejected"),
        "{explain}"
    );
    assert!(
        explain.contains("A `required` name no `properties` entry declares is still a requirement"),
        "{explain}"
    );
    assert!(
        explain.contains("In a composition such a field is no declaration of the property"),
        "{explain}"
    );

    // The array-arm doctrine. `intersect_structs` now mirrors it for an optional conflicting
    // property, so withdrawing this sentence would leave that behaviour unexplained.
    assert!(
        explain.contains("uninhabited item type"),
        "the explain must keep the doctrine that an empty item intersection stays representable: \
         {explain}"
    );

    // The hedge the round-1 repair put on every message that reports this code: `intersect_types`
    // fails for an empty intersection AND for an inhabited one with no single Rust type,
    // and the text must not claim the first when it may be the second.
    assert!(explain.contains("empty or unrepresentable"), "{explain}");

    // The rejection causes, including the recursive one all three spellings now share — stated as a
    // property of the document rather than of lowering order, which is what makes the verdict
    // reproducible when `components.schemas` is reordered.
    assert!(explain.contains("closes a reference cycle"), "{explain}");
    assert!(
        !explain.contains("not yet known") && !explain.contains("still being lowered"),
        "the explain still describes the recursive cause as a lowering-order fact, which the \
         guard no longer is: {explain}"
    );

    // Presence assertions cannot see a contradiction ADDED after them. Appending a paragraph
    // saying sibling keywords are "discarded and never intersected, so none of the above applies"
    // left every assertion above true and eleven suites green. The remedy is the last thing the
    // explain says, so anything appended moves it — which is a structural rule, not typography.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E013`'s explain must end with its remedy; text after it can contradict everything \
         above and no presence assertion would notice: {explain}"
    );
    // And the in-place forms of the same contradiction.
    for denial in [
        "never intersected",
        "none of the above",
        "always exactly its target",
        "siblings are discarded",
    ] {
        assert!(
            !explain.contains(denial),
            "`E013`'s explain contradicts the code it documents (`{denial}`): {explain}"
        );
    }
}

/// An object `allOf` merge that is rejected from inside its member loop reports no `W005` for the
/// `default`s it had read by then: which member's `default` the merged field keeps is decided
/// once, after the loop (#577), and a rejected merge emits no field to keep one. Before #577 the
/// pairwise fold had already reported one of M0's `x` and M1's `y` (both required, so neither
/// applies) by the time M2 was read, so a partial, order-dependent `W005` stood beside the
/// rejection. Each case puts the rejecting member last, after the two conflicting `default`s, and
/// pins the three in-loop rejections: an `additionalProperties` conflict, a reference cycle
/// through an `additionalProperties` value schema, and a repeated property whose types share
/// values no single Rust type represents.
#[test]
fn an_all_of_merge_rejected_inside_its_member_loop_reports_no_default_it_read() {
    let base = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    M0: { type: object, required: [a], properties: { a: { type: \
                string, default: x }, b: B0 }, additionalProperties: { type: string } }\n    \
                M1: { type: object, required: [a], properties: { a: { type: string, default: y \
                } } }\n    M2: M2BODY\n    Holder: { allOf: [{ $ref: '#/components/schemas/M0' \
                }, { $ref: '#/components/schemas/M1' }, { $ref: '#/components/schemas/M2' }] }\n";
    for (label, b0, m2, message) in [
        (
            "additionalProperties conflict",
            "{ type: string }",
            "{ type: object, additionalProperties: { type: integer } }",
            "`allOf` members declare conflicting `additionalProperties`",
        ),
        (
            "additionalProperties reference cycle",
            "{ type: string }",
            "{ type: object, additionalProperties: { $ref: '#/components/schemas/Holder' } }",
            "closes a reference cycle back to the schema being lowered",
        ),
        (
            "unrepresentable property meet",
            "{ type: array, prefixItems: [{ type: number }, { type: number }], items: false }",
            "{ type: object, properties: { b: { type: array, items: { type: string } } } }",
            "share values no single Rust type represents",
        ),
    ] {
        let spec = base.replace("B0", b0).replace("M2BODY", m2);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}, {entry}: {report:#?}\n{spec}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.message.contains(message)),
                "{label}, {entry}: {report:#?}"
            );
            let reported: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::SchemaDefaultNotApplied)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                reported,
                Vec::<&str>::new(),
                "{label}, {entry}: {report:#?}"
            );
        }
    }
}

/// An object `allOf` whose members repeat an object property meets that property pair by pair, and
/// the struct an earlier pair met it in is superseded by the next meet and not emitted (#428). The
/// post-lowering passes that report `W005` (#404) and `W006` read only emitted types, so a
/// superseded meet reports nothing of its own: three members say exactly what the two members
/// without the superseded one say. Before #428 the three-member `string`/`enum`/`integer` spelling
/// warned `W005` against the dead `string`∩`enum` struct while the two-member spelling, whose
/// result is the same uninhabited field, was clean.
///
/// This pins the two spellings agreeing. In the first two cases the emitted struct itself drops
/// the default — the field the first one empties included (#453) — and both spellings must report
/// it once.
#[test]
fn a_superseded_all_of_meet_reports_no_default_or_xml_diagnostic_of_its_own() {
    fn spec(media: &str, members: &[&str]) -> String {
        let mut spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            {media}:\n              \
             schema:\n                allOf:\n",
        );
        for k in members {
            spec.push_str(&format!(
                "                  - {{ type: object, properties: {{ inner: {{ type: object, \
                 properties: {{ k: {k} }} }} }} }}\n"
            ));
        }
        spec
    }
    fn reported(report: &Report) -> Vec<(&'static str, String, String)> {
        let mut reported: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| matches!(d.code, Code::SchemaDefaultNotApplied | Code::XmlHintIgnored))
            .map(|d| (d.code.as_str(), d.pointer.to_string(), d.message.clone()))
            .collect();
        reported.sort();
        reported
    }

    // (body media type, three members, the two left once the superseded middle one is dropped,
    // `W005`/`W006` count)
    let json = "application/json";
    let cases: [(&str, [&str; 3], [&str; 2], usize); 4] = [
        (
            json,
            [
                "{ type: string, default: z }",
                "{ enum: [a, b] }",
                "{ type: integer }",
            ],
            ["{ type: string, default: z }", "{ type: integer }"],
            // The emptied field drops `z`, reported once at its `default` (#453).
            1,
        ),
        (
            json,
            [
                "{ type: string, default: z }",
                "{ enum: [a, b, c] }",
                "{ enum: [a, b] }",
            ],
            ["{ type: string, default: z }", "{ enum: [a, b] }"],
            1,
        ),
        (
            json,
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, maxLength: 9 }",
                "{ type: string, minLength: 1 }",
            ],
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, minLength: 1 }",
            ],
            // The first member's own `inner` struct and the merged one both ignore the rename.
            // The merged struct is located at the first member's `inner` (#454), so the two
            // `W006` are one diagnostic, at the hint the author wrote.
            1,
        ),
        // Under an XML body the merged struct is XML-dedicated and keeps its rename. The first
        // member's own `inner` struct and the superseded meet are reached from no body, so neither
        // is a type of the output (#924), and a pass reading either would report a `W006` for a
        // hint the output honours.
        (
            "application/xml",
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, maxLength: 9 }",
                "{ type: string, minLength: 1 }",
            ],
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, minLength: 1 }",
            ],
            0,
        ),
    ];
    for (media, three, two, count) in cases {
        let (three, two) = (spec(media, &three), spec(media, &two));
        for (entry, run) in [
            ("generate", generate as fn(&str) -> Report),
            ("check", check),
        ] {
            let (with_superseded, without) = (run(&three), run(&two));
            assert_ne!(
                with_superseded.outcome(),
                Outcome::Rejected,
                "{entry}: {with_superseded:#?}"
            );
            assert_eq!(
                reported(&with_superseded),
                reported(&without),
                "{entry}: the superseded meet must report nothing the two-member spelling does \
                 not:\n{three}"
            );
            assert_eq!(reported(&without).len(), count, "{entry}: {without:#?}");
        }
    }
}
