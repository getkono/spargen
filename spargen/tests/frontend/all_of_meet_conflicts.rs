//! The meets an `allOf` produces and their conflicts: a re-emitted meet leaves no unused
//! definition, a meet's diagnostics are located at the met property, a conflict on an optional
//! property leaves the object inhabited, an inhabited intersection with no Rust type is rejected,
//! and an irreconcilable meet of nullable objects is the null type.

use super::*;

/// Three more sites re-emit a meet's kind under the enclosing schema's own name, as an all-scalar
/// `allOf` and a `$ref`-sibling intersection do (#401), so the meet's own definition is unused
/// unless the re-emitted kind refers to it (#462). Each leaked one: a union whose sole non-null
/// member meets the union's siblings left `…Constrained`, a union whose siblings exclude all but
/// one member left the survivor's `…Variant0Constrained`, and a `$ref` to a union refined by
/// untyped sibling keywords left the met union `…ReferenceIntersection`. Each is now discarded,
/// while a type the re-emitted kind does refer to — the `$ref` case's met branch, or the nested
/// struct a sole member's meet builds for a property both sides declare — is kept. Where the union
/// is left one variant, its members' and its sibling keywords' own lowerings go with it (#987).
#[test]
fn a_re_emitted_union_meet_emits_no_meet_its_result_does_not_use() {
    fn spec(field: &str) -> String {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Holder' }}\n\
             components:\n  schemas:\n    \
             Pet: {{ type: object, properties: {{ name: {{ type: string }} }} }}\n    \
             Box: {{ type: object, properties: {{ sub: {{ type: object, properties: {{ q: {{ type: string }} }} }} }} }}\n    \
             Target: {{ oneOf: [{{ type: object, properties: {{ name: {{ type: string }} }} }}, {{ type: string }}] }}\n    \
             Holder: {{ type: object, properties: {{ field: {field} }} }}\n"
        )
    }
    /// The types lowered for `Holder.field`, in source order.
    fn field_types(code: &str) -> Vec<String> {
        types_module(code)
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .filter(|name| name.starts_with("Holderfield"))
            .map(str::to_owned)
            .collect()
    }

    let cases: [(&str, &str, &[&str], &str); 4] = [
        (
            "sole non-null member",
            "{ oneOf: [{ $ref: '#/components/schemas/Pet' }, { type: 'null' }], required: [name] }",
            &["Holderfield"],
            "HolderfieldConstrained",
        ),
        (
            "sole non-null member whose meet builds a nested struct",
            "{ oneOf: [{ $ref: '#/components/schemas/Box' }, { type: 'null' }], \
             properties: { sub: { type: object, required: [q] } } }",
            &["HolderfieldConstrainedsub", "Holderfield"],
            "HolderfieldConstrained",
        ),
        (
            "one member left by the siblings",
            "{ oneOf: [{ $ref: '#/components/schemas/Pet' }, { type: integer }], type: object, \
             required: [name] }",
            &["Holderfield"],
            "HolderfieldVariant0Constrained",
        ),
        (
            "$ref to a union with untyped siblings",
            "{ $ref: '#/components/schemas/Target', required: [name] }",
            &[
                "HolderfieldConstraint",
                "HolderfieldReferenceIntersectionVariant0",
                "Holderfield",
            ],
            "HolderfieldReferenceIntersection",
        ),
    ];
    for (case, field, expected, discarded) in cases {
        let (report, code) = generate_with_code(&spec(field));
        assert_eq!(report.outcome(), Outcome::Generated, "{case}: {report:#?}");
        let mut lowered = field_types(&code);
        // The sibling's own lowered field alias, which a `$ref`'s sibling keywords still emit, is
        // not what this pins.
        lowered.retain(|name| name != "HolderfieldConstraintname");
        assert_eq!(lowered, expected, "{case}: `{discarded}` is unused");
    }
}

/// Issue #454: two `allOf` members that both declare an object-typed property `p` meet it in a
/// struct of its own (`PetpIntersection`). That struct carried the document root's provenance, so
/// a `W005` against it named the type as `` in `` `` and a `W006` against it had an empty pointer
/// and a span covering the whole document. Both must name the property whose types were met.
fn nested_meet_spec(first: &str, second: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: t, version: "1" }}
paths:
  /p:
    get:
      operationId: getP
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: {{ $ref: "#/components/schemas/Pet" }}
components:
  schemas:
    Pet:
      allOf:
        - type: object
          properties:
            p:
              type: object
              properties:
                {first}
        - type: object
          properties:
            p:
              type: object
              properties:
                {second}
"##
    )
}

#[test]
fn a_nested_all_of_meet_locates_its_diagnostics_at_the_met_property() {
    const MET: &str = "/components/schemas/Pet/allOf/0/properties/p";

    let w005 = nested_meet_spec(
        "a: { type: string, default: zzz }",
        "a: { type: string, enum: [a, b] }",
    );
    let w006 = nested_meet_spec(
        "a: { type: string, xml: { namespace: \"http://example.com/ns\" } }",
        "b: { type: string }",
    );
    for (entry, run) in [
        ("generate", generate as fn(&str) -> Report),
        ("check", check),
    ] {
        let report = run(&w005);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        assert!(
            !messages.is_empty(),
            "{entry}: `zzz` is dropped: {report:#?}"
        );
        for message in &messages {
            assert!(
                message.contains(&format!("in `{MET}`")) && !message.contains("in ``"),
                "{entry}: the `W005` names the met property, not the root: {message}"
            );
        }

        let report = run(&w006);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let ignored: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::XmlHintIgnored)
            .collect();
        assert!(
            !ignored.is_empty(),
            "{entry}: the namespace is ignored: {report:#?}"
        );
        for diagnostic in ignored {
            assert_eq!(
                diagnostic.pointer.as_str(),
                MET,
                "{entry}: every `W006` points at the met property: {report:#?}"
            );
            let span = diagnostic.span.expect("a located `W006`");
            assert!(
                span.start.line > 2,
                "{entry}: the `W006` span is the property's, not the document's: {span:?}"
            );
        }
    }
}

/// A meet's recorded location belongs to the id the meet struct was inserted at, and lifting a
/// component root out of that id frees it for the next insert. `A`'s `$ref`-sibling meet is
/// located at `B` and lifted into `A`'s reserved id; the struct `C` then reuses the freed id, and
/// its `W006` must point at `C`, not inherit the lifted meet's `B` location.
#[test]
fn a_freed_meet_id_does_not_lend_its_location_to_the_next_type() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    B:
      type: object
    A:
      $ref: '#/components/schemas/B'
      properties:
        m: { type: integer }
    C:
      type: object
      properties:
        x:
          type: string
          xml: { namespace: "http://example.com/ns" }
"##;
    for (entry, run) in [
        ("generate", generate as fn(&str) -> Report),
        ("check", check),
    ] {
        let report = run(spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let ignored: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::XmlHintIgnored)
            .collect();
        assert!(
            !ignored.is_empty(),
            "{entry}: the namespace is ignored: {report:#?}"
        );
        for diagnostic in ignored {
            assert!(
                diagnostic
                    .pointer
                    .as_str()
                    .starts_with("/components/schemas/C"),
                "{entry}: the `W006` for `C.x` points at `C`: {report:#?}"
            );
        }
    }
}

/// A property whose two sides cannot be intersected does not make the composition empty unless
/// some instance is obliged to carry it. When the property is optional on BOTH sides, `{}` and
/// `{"zz": 1}` still satisfy the whole schema — an independent Draft 2020-12 validator confirms
/// both — so rejecting the document deletes a body that has valid instances.
///
/// This is `E013`'s own published doctrine, which the array arm of `intersect_non_null` already
/// implements: "an empty array-item intersection becomes an uninhabited item type so the valid
/// empty array remains representable". `intersect_structs` propagated the failure instead. It now
/// mirrors the array arm: the field takes an uninhabited type, so the instances that remain are
/// exactly the ones that omit it.
///
/// Requiring the property on either side is the real empty composition, and both controls must
/// keep rejecting — the validator says nothing satisfies them.
#[test]
fn a_property_conflict_on_an_optional_property_does_not_empty_the_object() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // Both sites that reach `intersect_structs` with a `$ref`-sibling composition: the `$ref` arm
    // of `lower_schema_inner`, and the sole-real-member union collapse.
    let inhabited: &[(&str, &str, &str)] = &[
        (
            "a `$ref` sibling conflicting on a property neither side requires",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "a sole-member union conflicting on a property neither side requires",
            "type: object\n                properties: { a: { type: integer } }\n                oneOf: [{ type: object, properties: { a: { type: string } } }]",
            "components:\n  schemas:\n    Unused: { type: string }\n",
        ),
    ];
    for (what, body, components) in inhabited {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` still admits `{{}}`, so it must not be rejected: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable)
                && !has_code(&report, Code::NonDisjointUnion),
            "`{what}` reported an irreconcilable composition: {report:#?}"
        );
        // Not silently widened to something that accepts `{"a": 1}`: the field itself is
        // uninhabited, so only instances omitting the property can be built or decoded.
        assert!(
            code.contains("no JSON value can inhabit schema"),
            "`{what}` must give the conflicting property an uninhabited type: {code}"
        );
        assert_no_untyped_value(&code);
    }

    // The controls. Requiring the property on either side obliges every instance to carry a value
    // no type admits, so the composition really is empty and the rejection is right.
    let empty: &[(&str, &str, &str)] = &[
        (
            "the sibling requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                required: [a]\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "the target requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, required: [a], properties: { a: { type: string } } }\n",
        ),
    ];
    for (what, body, components) in empty {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        let report = generate(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` admits no value at all, so it must still be rejected: {report:#?}"
        );
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` did not report E013: {report:#?}"
        );
    }
}

/// Four spellings of one conjunction — a `$ref` with sibling `properties`, the same pair written
/// as `allOf` members, a sole-member `oneOf` beside `properties`, and two inline `allOf` members —
/// have identical valid sets when a property conflicts and is optional on every side: `{}` and
/// `{"zz": 1}` satisfy all four and no `a`-bearing instance satisfies any. The intersection path
/// typed the property uninhabited; the `allOf` merge, which does not go through
/// `intersect_structs`, rejected the same document with `E013`. So whether a document was an error
/// depended on which equivalent spelling its author chose.
///
/// Every spelling below must now agree, and so must every control: requiring the property anywhere
/// — on the member that declares it, on a later member that only lists it in `required` (read
/// after the conflict is seen, so the decision cannot be made at the conflict), on the enclosing
/// schema, or on the `$ref` target — empties the object, and all of those reject.
#[test]
fn an_all_of_conflict_on_an_optional_property_agrees_with_every_other_spelling() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;
    const OBJ: &str =
        "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n";
    const OBJ_REQUIRED: &str = "components:\n  schemas:\n    Obj: { type: object, required: [a], properties: { a: { type: string } } }\n";
    const UNUSED: &str = "components:\n  schemas:\n    Unused: { type: string }\n";

    let inhabited: &[(&str, &str, &str)] = &[
        (
            "a `$ref` with a conflicting sibling property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            OBJ,
        ),
        (
            "the same pair as `allOf` members",
            "allOf:\n                  - $ref: '#/components/schemas/Obj'\n                  - { type: object, properties: { a: { type: integer } } }",
            OBJ,
        ),
        (
            "a sole-member `oneOf` beside a conflicting property",
            "type: object\n                properties: { a: { type: integer } }\n                oneOf: [{ type: object, properties: { a: { type: string } } }]",
            UNUSED,
        ),
        (
            "two inline `allOf` members",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "an `allOf` member conflicting with the enclosing schema's own property",
            "type: object\n                properties: { a: { type: integer } }\n                allOf:\n                  - { type: object, properties: { a: { type: string } } }",
            UNUSED,
        ),
        (
            "a third `allOf` member meeting an already-uninhabited property",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }\n                  - { type: object, properties: { a: { type: boolean } } }",
            UNUSED,
        ),
    ];
    for (what, body, components) in inhabited {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` still admits `{{}}`, so it must not be rejected: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable)
                && !has_code(&report, Code::NonDisjointUnion),
            "`{what}` reported an irreconcilable composition: {report:#?}"
        );
        assert!(
            code.contains("no JSON value can inhabit schema"),
            "`{what}` must give the conflicting property an uninhabited type: {code}"
        );
        assert!(
            code.contains("pub a: Option<"),
            "`{what}` must keep the conflicting property optional: {code}"
        );
        assert_no_untyped_value(&code);
    }

    let empty: &[(&str, &str, &str)] = &[
        (
            "the member that declares the property requires it",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, required: [a], properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "a later member requires the property without declaring it",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }\n                  - { type: object, required: [a] }",
            UNUSED,
        ),
        (
            "the enclosing schema requires the property",
            "type: object\n                required: [a]\n                allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "the `$ref` member's target requires the property",
            "allOf:\n                  - $ref: '#/components/schemas/Obj'\n                  - { type: object, properties: { a: { type: integer } } }",
            OBJ_REQUIRED,
        ),
        (
            "the `$ref` sibling spelling of the same requirement",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            OBJ_REQUIRED,
        ),
    ];
    for (what, body, components) in empty {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        let report = generate(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` admits no value at all, so it must still be rejected: {report:#?}"
        );
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` did not report E013: {report:#?}"
        );
    }
}

/// Two types with no typed intersection are not always an empty intersection. `uuid` and
/// `contentEncoding: base64` are both annotations on a string in 2020-12, so every string satisfies
/// both and `["x"]` is a valid instance of an array whose two item schemas are those; there is
/// simply no single Rust type for the meet. Only a *provably empty* meet — `uuid` against
/// `integer`, disjoint JSON types — may be typed uninhabited, because only then are the instances
/// the uninhabited type still admits (`[]`, an object omitting the property) exactly the valid ones.
///
/// Every site that used to read "no typed intersection" as "empty" is driven here: the array item
/// (under `allOf` and under a `$ref` with siblings), an optional property (under a `$ref` with
/// siblings and under `allOf`), the null-only collapse of two nullable sides, a union branch the
/// enclosing schema's siblings meet, a branch of a referenced union, and a tuple position under
/// array items. Each typed the inhabited
/// case as uninhabited, as `()`, or dropped the branch, and generated. Each now rejects with
/// `E013`, and the provably empty control beside it keeps generating as before.
#[test]
fn an_inhabited_intersection_with_no_rust_type_is_rejected_not_typed_uninhabited() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;
    // `OTHER` is the second side's schema: the unrepresentable `base64` string, or the disjoint
    // `integer` control.
    let cases: &[(&str, &str, &str)] = &[
        (
            "array items under `allOf`",
            "allOf:\n                  - { type: object, properties: { ids: { type: array, items: { $ref: '#/components/schemas/Id' } } } }\n                  - { type: object, properties: { ids: { type: array, items: OTHER } } }",
            "components:\n  schemas:\n    Id: { type: string, format: uuid }\n",
        ),
        (
            "array items under a `$ref` with siblings",
            "{ $ref: '#/components/schemas/Ids', type: array, items: OTHER }",
            "components:\n  schemas:\n    Ids: { type: array, items: { type: string, format: uuid } }\n",
        ),
        (
            "an optional property under a `$ref` with siblings",
            "{ $ref: '#/components/schemas/Obj', type: object, properties: { a: OTHER } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string, format: uuid } } }\n",
        ),
        (
            "an optional property under `allOf`",
            "allOf:\n                  - { type: object, properties: { a: { type: string, format: uuid } } }\n                  - { type: object, properties: { a: OTHER } }",
            "components:\n  schemas:\n    Unused: { type: string }\n",
        ),
    ];
    for (what, body, components) in cases {
        for (other, inhabited) in [
            ("{ type: string, contentEncoding: base64 }", true),
            ("{ type: integer }", false),
        ] {
            let spec = format!(
                "{HEAD}{}{components}",
                PATH.replace("BODY", &body.replace("OTHER", other))
            );
            let (report, code) = generate_with_code(&spec);
            if inhabited {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what}: a uuid/base64 meet is inhabited but has no Rust type, so it must be \
                     rejected rather than typed uninhabited: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::AllOfIrreconcilable),
                    "{what}: must report E013: {report:#?}"
                );
            } else {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what}: a uuid/integer meet is provably empty, so it must keep generating: \
                     {report:#?}"
                );
                assert!(
                    code.contains("no JSON value can inhabit schema"),
                    "{what}: the empty meet must still be typed uninhabited: {code}"
                );
            }
        }
    }

    // The sites that do not fall back to `Never`, but read the same answer in other ways: the
    // null-only collapse of two nullable sides typed the position `()`, and a union branch the
    // siblings could not be typed against was dropped (with a `W011` claiming it cannot satisfy
    // them, or with nothing at all under a `$ref`), leaving a lone `uuid` for a union that also
    // admits every other string. And a tuple position that cannot meet is not an empty tuple:
    // `prefixItems` does not require the array to reach it, so `[]` satisfies both `[string]` and
    // `[integer]` tuples, and an outer array of them is not only `[]`.
    let components = "components:\n  schemas:\n    N: { type: [string, 'null'], format: uuid }\n    \
                      U: { anyOf: [{ type: string, contentEncoding: base64 }, { type: string }] }\n    \
                      Pairs: { type: array, items: { type: array, prefixItems: [{ type: string }], items: false } }\n";
    for (what, body) in [
        (
            "array items that are tuples whose positions do not meet",
            "{ $ref: '#/components/schemas/Pairs', type: array, items: { type: array, prefixItems: [{ type: integer }], items: false } }",
        ),
        (
            "two nullable sides whose non-null shapes have no Rust type",
            "{ $ref: '#/components/schemas/N', type: [string, 'null'], contentEncoding: base64 }",
        ),
        (
            "a union branch under the enclosing schema's siblings",
            "{ type: string, format: uuid, anyOf: [{ type: string, contentEncoding: base64 }, { type: string }] }",
        ),
        (
            "a branch of a referenced union under a `$ref`'s siblings",
            "{ $ref: '#/components/schemas/U', type: string, format: uuid }",
        ),
    ] {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        let report = generate(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{what}: must reject rather than narrow: {report:#?}"
        );
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "{what}: must report E013: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{what}: must not claim a branch cannot satisfy the siblings: {report:#?}"
        );
    }

    // An unrepresentable property does not settle a struct meet on its own: `intersect_structs`
    // defers it, because a LATER property that some side requires and whose types share no value
    // proves every object empty, and then the array of them is exactly `[]` — `Vec<Never>`, not a
    // rejection. The uuid/base64 property `a` comes first so the deferral is what is exercised;
    // with the required property's types compatible instead, nothing empties the object and the
    // deferred unrepresentable answer stands.
    let components = "components:\n  schemas:\n    Objs: { type: array, items: { type: object, \
                      properties: { a: { type: string, format: uuid }, b: { type: string } } } }\n";
    for (required_type, empty) in [("integer", true), ("string", false)] {
        let body = format!(
            "{{ $ref: '#/components/schemas/Objs', type: array, items: {{ type: object, \
             properties: {{ a: {{ type: string, contentEncoding: base64 }}, \
             b: {{ type: {required_type} }} }}, required: [b] }} }}"
        );
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", &body));
        let (report, code) = generate_with_code(&spec);
        if empty {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "a required string/integer property empties every item, so it must keep \
                 generating the empty array despite the earlier uuid/base64 one: {report:#?}"
            );
            assert!(
                code.contains("no JSON value can inhabit schema"),
                "the emptied item must be typed uninhabited: {code}"
            );
            let never = code
                .lines()
                .find_map(|line| {
                    line.trim()
                        .strip_prefix("pub enum ")
                        .and_then(|rest| rest.strip_suffix(" {}"))
                })
                .unwrap_or_else(|| panic!("no uninhabited enum was emitted: {code}"));
            assert!(
                code.contains(&format!("Vec<{never}>")),
                "the body must be an array of the uninhabited item `{never}`: {code}"
            );
        } else {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "with no empty property the uuid/base64 one is unrepresentable, so it must be \
                 rejected: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "must report E013: {report:#?}"
            );
        }
    }
}

/// An irreconcilable meet of nullable objects leaves exactly `null`: no object satisfies both
/// members, whose required property types share no value, and both admit `null`. Every spelling
/// of that conjunction lowers it to the null type `()`, as the `$ref`-sibling spelling already
/// did; the `allOf` spellings, and a `$ref` beside untyped object keywords, rejected it with
/// `E013` (#542). A member that denies `null` leaves nothing at all, which stays `E013` in every
/// spelling.
#[test]
fn an_irreconcilable_meet_of_nullable_objects_is_the_null_type() {
    let document = |second_type: &str, p: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    M0:\n      type: [object, 'null']\n      required: [a]\n      \
             properties: {{ a: {{ type: string }}, b: {{ type: string }} }}\n    M1:\n      \
             type: {second_type}\n      required: [b]\n      \
             properties: {{ b: {{ type: integer }} }}\n    Holder:\n      type: object\n      \
             required: [p]\n      properties:\n        p: {p}\n"
        )
    };
    let spellings = |second_type: &str| {
        [
            (
                "an `allOf` of `$ref`s",
                "{ allOf: [{ $ref: '#/components/schemas/M0' }, \
                 { $ref: '#/components/schemas/M1' }] }"
                    .to_owned(),
            ),
            (
                "an inline `allOf`",
                format!(
                    "{{ allOf: [{{ type: [object, 'null'], properties: {{ b: {{ type: string }} }} }}, \
                     {{ type: {second_type}, required: [b], \
                     properties: {{ b: {{ type: integer }} }} }}] }}"
                ),
            ),
            (
                "keywords beside an `allOf` member",
                format!(
                    "{{ type: {second_type}, required: [b], properties: {{ b: {{ type: integer }} }}, \
                     allOf: [{{ type: [object, 'null'], properties: {{ b: {{ type: string }} }} }}] }}"
                ),
            ),
            (
                "a `$ref` beside typed keywords",
                format!(
                    "{{ $ref: '#/components/schemas/M0', type: {second_type}, required: [b], \
                     properties: {{ b: {{ type: integer }} }} }}"
                ),
            ),
        ]
    };

    for (second_type, admits_null) in [("[object, 'null']", true), ("object", false)] {
        for (what, p) in spellings(second_type) {
            let spec = document(second_type, &p);
            let (report, code) = generate_with_code(&spec);
            if !admits_null {
                assert_eq!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
                assert!(
                    has_code(&report, Code::AllOfIrreconcilable),
                    "{what}: {report:#?}"
                );
                continue;
            }
            assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {report:#?}"
            );
            let ty = field_type(&types_module(&code), "pub p:")
                .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
            assert!(
                code.contains(&format!("pub type {ty} = ();")),
                "{what}: only `null` satisfies both members, so `Holder.p` is the null type: {code}"
            );
        }
    }

    // A `$ref` to a nullable object beside untyped object keywords: the keywords admit `null`
    // without deciding it (#425), so this meet is null-only too, not a category contradiction.
    let untyped = document(
        "[object, 'null']",
        "{ $ref: '#/components/schemas/M0', properties: { a: { type: integer } } }",
    );
    let (report, code) = generate_with_code(&untyped);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    let ty = field_type(&types_module(&code), "pub p:")
        .unwrap_or_else(|| panic!("no `Holder.p`: {code}"));
    assert!(code.contains(&format!("pub type {ty} = ();")), "{code}");

    // The schema's own `type` listing `null` does not make `null` valid where its `allOf` members
    // deny it: no value satisfies the schema, so that stays `E013`, never the null type.
    let denied = document(
        "object",
        "{ type: [object, 'null'], allOf: [{ type: object, properties: { b: { type: string } } }, \
         { $ref: '#/components/schemas/M1' }] }",
    );
    let (report, _) = generate_with_code(&denied);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // The schema's own `type`, `enum` or `const` beside the `allOf` of the two nullable members
    // is no object keyword, so it contributes no member, yet it constrains every value: one that
    // excludes `null` leaves nothing, which stays `E013`; one that admits it leaves `null`. So
    // does a `oneOf` member beside them, met with the null type the object members leave.
    let members = "{ $ref: '#/components/schemas/M0' }, { $ref: '#/components/schemas/M1' }";
    for (what, p, null_only) in [
        (
            "own `type: string`",
            format!("{{ type: string, allOf: [{members}] }}"),
            false,
        ),
        (
            "own `enum: [1]`",
            format!("{{ enum: [1], allOf: [{members}] }}"),
            false,
        ),
        (
            "own `const: x`",
            format!("{{ const: x, allOf: [{members}] }}"),
            false,
        ),
        (
            "own `type: [string, 'null']`",
            format!("{{ type: [string, 'null'], allOf: [{members}] }}"),
            true,
        ),
        (
            "own `enum: [null]`",
            format!("{{ enum: [null], allOf: [{members}] }}"),
            true,
        ),
        // A nested `allOf` member is flattened into the outer meet, and its own `type`, `enum` or
        // `const` constrains every value just as the outer schema's does (#569).
        (
            "a nested member's own `type: string`",
            format!("{{ allOf: [{{ type: string, allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a nested member's own `enum: [1]`",
            format!("{{ allOf: [{{ enum: [1], allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a nested member's own `const: x`",
            format!("{{ allOf: [{{ const: x, allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a twice-nested member's own `type: string`",
            format!("{{ allOf: [{{ allOf: [{{ type: string, allOf: [{members}] }}] }}] }}"),
            false,
        ),
        (
            "a `$ref` member's own `type: string` beside a nested `allOf`",
            "{ allOf: [{ $ref: '#/components/schemas/M0', type: string, \
             allOf: [{ $ref: '#/components/schemas/M1' }] }] }"
                .to_owned(),
            false,
        ),
        (
            "a nested member's own `type: [string, 'null']`",
            format!("{{ allOf: [{{ type: [string, 'null'], allOf: [{members}] }}] }}"),
            true,
        ),
        (
            "a nested member's own `enum: [null]`",
            format!("{{ allOf: [{{ enum: [null], allOf: [{members}] }}] }}"),
            true,
        ),
        (
            "a `oneOf` member admitting `null`",
            format!(
                "{{ allOf: [{members}, {{ oneOf: [{{ type: string }}, {{ type: 'null' }}] }}] }}"
            ),
            true,
        ),
        (
            "a `oneOf` member denying `null`",
            format!(
                "{{ allOf: [{members}, {{ oneOf: [{{ type: string }}, {{ type: integer }}] }}] }}"
            ),
            false,
        ),
    ] {
        let spec = document("[object, 'null']", &p);
        let (report, code) = generate_with_code(&spec);
        if !null_only {
            assert_eq!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {report:#?}"
            );
            continue;
        }
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{what}: {report:#?}"
        );
        let ty = field_type(&types_module(&code), "pub p:")
            .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
        assert!(
            code.contains(&format!("pub type {ty} = ();")),
            "{what}: only `null` is left, so `Holder.p` is the null type: {code}"
        );
    }

    // A non-component `$ref` member is expanded in place, so a target that is itself a nested
    // `allOf` is flattened into the outer meet, and its own `type` constrains every value just as
    // an inline member's does. The bare `$ref` and the `allOf` holding it are one conjunction, so
    // both stay `E013`, in `generate` and `check` alike; a target whose `type` admits `null`
    // leaves the null type in both spellings.
    for (target_type, null_only) in [("string", false), ("[string, 'null']", true)] {
        for (what, p) in [
            ("a bare `$ref`", "{ $ref: '#/x-defs/Q' }"),
            (
                "an `allOf` of the `$ref`",
                "{ allOf: [{ $ref: '#/x-defs/Q' }] }",
            ),
            (
                "an `allOf` of an alias of the `$ref`",
                "{ allOf: [{ $ref: '#/x-defs/R' }] }",
            ),
        ] {
            let what = format!("{what} to a nested `allOf` of `type: {target_type}`");
            let spec = format!(
                "{}x-defs:\n  Q:\n    type: {target_type}\n    allOf: [{members}]\n  R:\n    \
                 $ref: '#/x-defs/Q'\n",
                document("[object, 'null']", p)
            );
            let (report, code) = generate_with_code(&spec);
            if !null_only {
                assert_eq!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
                assert!(
                    has_code(&report, Code::AllOfIrreconcilable),
                    "{what}: {report:#?}"
                );
                continue;
            }
            assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {report:#?}"
            );
            let ty = field_type(&types_module(&code), "pub p:")
                .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
            assert!(
                code.contains(&format!("pub type {ty} = ();")),
                "{what}: only `null` is left, so `Holder.p` is the null type: {code}"
            );
        }
    }
}
