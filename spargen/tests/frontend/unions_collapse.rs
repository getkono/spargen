//! Union collapse and merge: a `oneOf` member accepting every value warns, a multi-type array
//! generates a typed union, branches that lower to one type or one structure collapse or merge with
//! `W001`, and a `oneOf` whose `null` matches two branches rejects `null`.

use super::*;

/// A `oneOf` member that lowers to `serde_json::Value` beside another accepts every value, so every
/// value another member accepts matches two and fails exactly-one (#535). The union is kept, since
/// it is what the document says, and the dead variants are reported as `W001` at the union: inline,
/// as a `$ref` member to an untyped component, under a discriminator no member's tag selects, as
/// an `allOf`'s sole member, and after the meet beside a `$ref` or an `allOf`, whose merge is held
/// back until then. The variant's position is named, as a member before a meet and a variant
/// after one.
#[test]
fn a_one_of_member_accepting_every_value_warns() {
    let cases = [
        ("oneOf: [{ type: string }, {}]", "`oneOf`'s member 1 lowers"),
        ("oneOf: [true, { type: integer }]", "`oneOf`'s member 0 lowers"),
        (
            "oneOf: [{ type: string }, { $ref: '#/components/schemas/Any' }]\n    Any: {}",
            "`oneOf`'s member 1 lowers",
        ),
        (
            "{ oneOf: [{ type: string }, {}], discriminator: { propertyName: k } }",
            "`oneOf`'s member 1 lowers",
        ),
        (
            "{ allOf: [{ oneOf: [{ type: string }, {}] }] }",
            "`oneOf`'s member 1 lowers",
        ),
        (
            "{ type: object, properties: { p: { oneOf: [{ type: string }, {}] } } }",
            "`oneOf`'s member 1 lowers",
        ),
        (
            "{ $ref: '#/components/schemas/Any', oneOf: [{ type: string }, {}] }\n    Any: {}",
            "`$ref` and its `oneOf` sibling intersect to a `oneOf` whose variant 1 lowers",
        ),
        (
            "{ allOf: [{ $ref: '#/components/schemas/Any' }], oneOf: [{ type: string }, {}] }\n    \
             Any: {}",
            "`allOf` and the `oneOf` beside it intersect to a `oneOf` whose variant 1 lowers",
        ),
    ];
    for (schema, says) in cases {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    U:\n      {schema}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        let untyped: Vec<&Diagnostic> = report
            .diagnostics()
            .iter()
            .filter(|d| {
                d.message
                    .contains("to `serde_json::Value`, which accepts every value")
            })
            .collect();
        assert_eq!(untyped.len(), 1, "{schema}: {report:#?}");
        assert_eq!(untyped[0].code, Code::ValidationKeywordIgnored, "{schema}");
        assert_eq!(untyped[0].severity, Severity::Warning, "{schema}");
        // At the union itself: the schema, or the `allOf` member or property that holds it.
        let union_at = if schema.contains("allOf: [{ oneOf") {
            "/components/schemas/U/allOf/0"
        } else if schema.contains("properties") {
            "/components/schemas/U/properties/p"
        } else {
            "/components/schemas/U"
        };
        assert_eq!(untyped[0].pointer.as_str(), union_at, "{schema}");
        assert!(untyped[0].message.contains(says), "{schema}: {untyped:#?}");
        // The union stands: the typed variant still excludes the values it accepts.
        assert!(code.contains("pub enum U"), "{schema}");
        // The oracle sees the `serde_json::Value` variant, which the warning explains.
        assert!(
            !oracles::indistinguishable_variants(&code).is_empty(),
            "{schema}"
        );
    }
}

/// An untyped `oneOf` branch that the `allOf`'s untyped object keywords refine is no longer
/// `serde_json::Value` (#535): the refiner meets every branch of no category, so the branch the
/// union wrote as `{}` is generated as a typed struct, and nothing reports it as accepting every
/// value. The check runs on the union the refiners leave, not on the meet before them.
#[test]
fn a_refined_untyped_one_of_branch_does_not_warn() {
    for refiner in [
        "{ properties: { a: { type: string } } }",
        "{ required: [a] }",
    ] {
        let schema = format!("{{ allOf: [{{ oneOf: [{{ type: string }}, {{}}] }}, {refiner}] }}");
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    U:\n      {schema}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        let untyped: Vec<&Diagnostic> = report
            .diagnostics()
            .iter()
            .filter(|d| {
                d.message
                    .contains("to `serde_json::Value`, which accepts every value")
            })
            .collect();
        assert!(untyped.is_empty(), "{schema}: {untyped:#?}\n{code}");
        assert!(code.contains("pub enum U"), "{schema}");
        // The generated union `U` has no `serde_json::Value` variant, and the pre-meet union the
        // `allOf` member lowered to, which had one, is no longer emitted (#561).
        let found = oracles::indistinguishable_variants(&code);
        assert!(found.is_empty(), "{schema}: {found:#?}");
    }
}

/// A `serde_json::Value` variant of an `anyOf` stays silent (#535): one match decodes an `anyOf`
/// and the most specific match is selected, so a typed variant still takes every value it
/// accepts and the `serde_json::Value` variant, the lowering of an untyped member, only the rest.
/// [`oracles::indistinguishable_variants`] holds only a trial-matched `oneOf` to typed variants,
/// so it finds nothing here. Neither does a `oneOf` whose untyped members all merge into one
/// variant, which the merge already reports (#402).
#[test]
fn an_untyped_any_of_member_stays_silent() {
    for schema in ["anyOf: [{ type: string }, {}]", "anyOf: [{}, {}]"] {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    U:\n      {schema}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{schema}: {report:#?}");
        assert!(code.contains("= serde_json::Value;"), "{schema}");
        let found = oracles::indistinguishable_variants(&code);
        assert!(found.is_empty(), "{schema}: {found:#?}");
    }
    let (report, _) = generate_with_code(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    \
         U:\n      oneOf: [{}, {}]\n",
    );
    let messages: Vec<&str> = report
        .diagnostics()
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("the union is that one type"),
        "{messages:#?}"
    );
}

#[test]
fn multi_type_array_generates_a_typed_union() {
    let spec = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /value:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: [string, integer] }
"#;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("enum ResponseBody"), "{code}");
    assert_no_untyped_value(&code);
}

/// A collapsed union keeps the nullability its keyword gives it. Beside a nullable target
/// (`NB: type: [object, "null"]`), each branch that states nothing admits `null`. An `anyOf` needs
/// just one branch to match, so `null` stays valid and the position is `Option<_>`. A `oneOf`
/// needs exactly one, and `null` matches both, so `null` is invalid and the position is required
/// and non-nullable. A `{type: 'null'}` member does not change either answer: under `oneOf`,
/// `null` then matches that member *and* both keywordless branches, so it
/// still fails the exactly-one rule, while under `anyOf` it was already valid.
#[test]
fn a_collapsed_union_sibling_beside_a_nullable_target_keeps_its_keywords_nullability() {
    for (union_keyword, null_member, nullable) in [
        ("anyOf", "", true),
        ("oneOf", "", false),
        ("anyOf", ", { type: 'null' }", true),
        ("oneOf", ", { type: 'null' }", false),
    ] {
        // The case label every assertion message carries.
        let keyword = format!("{union_keyword}[{{}}, {{}}{null_member}]");
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Holder:
      type: object
      properties:
        x:
          $ref: '#/components/schemas/NB'
          {union_keyword}: [ {{}}, {{}}{null_member} ]
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{keyword} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/Holder/properties/x"
                }),
                "{keyword} via {entry}: the collapse must still warn: {report:#?}"
            );
        }
        let types = types_module(&code);
        let x = field_type(&types, "pub x")
            .unwrap_or_else(|| panic!("{keyword}: no `x` field: {types}"));
        assert_eq!(
            x.starts_with("Option<"),
            nullable,
            "{keyword}: `x` is `{x}`, but `null` is {} here: {types}",
            if nullable { "valid" } else { "invalid" }
        );
    }
}

/// An inline `oneOf` whose branches lower to one and the same generated type is the `$ref`-sibling
/// collapse above spelled without the `$ref` (#402). Branches that state nothing beside
/// `type: object` and `properties` each meet the sibling to the same object, and bare ones each
/// lower to `serde_json::Value`; emitted as two variants, every value matches both, so the
/// exactly-one check fails every decode. The position is that one type instead, and the branch
/// distinctions are reported (`W001`) rather than generating a union nothing can decode. Where only
/// some branches share a type, they become one variant and the others stand. `null` follows the
/// collapse's `oneOf` rule: two branches that both accept it fail exactly-one, so it is invalid.
#[test]
fn an_inline_one_of_whose_branches_lower_to_one_type_collapses_and_warns() {
    let props = "properties: { a: { type: string }, b: { type: string } }";
    let keywordless_branches = "oneOf: [ {}, {} ]";
    for (shape, body, expect_fields, expect_variants) in [
        (
            "beside type: object",
            format!("      type: object\n      {props}\n      {keywordless_branches}\n"),
            Some(["a", "b"]),
            None,
        ),
        (
            "bare",
            format!("      {keywordless_branches}\n"),
            None,
            None,
        ),
        (
            "partly shared",
            "      oneOf: [ { type: string, minLength: 1 }, { type: string, maxLength: 3 }, { \
             type: integer } ]\n"
                .to_owned(),
            None,
            Some(2),
        ),
    ] {
        let spec = single_component_document(&body);
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{shape} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/U"
                        && d.message.contains("same generated type")
                }),
                "{shape} via {entry}: the collapse must warn at `U`: {report:#?}"
            );
        }
        let types = types_module(&code);
        match (expect_fields, expect_variants) {
            (Some(fields), _) => assert_eq!(
                declared_fields(&types, "U"),
                fields,
                "{shape}: `U` must be the one object both branches lower to: {types}"
            ),
            (None, Some(count)) => assert_eq!(
                enum_variants(&types, "U").len(),
                count,
                "{shape}: the two string branches must be one variant: {types}"
            ),
            (None, None) => assert!(
                types.contains("pub type U = serde_json::Value;"),
                "{shape}: `U` must be the one untyped value both branches lower to: {types}"
            ),
        }
        assert!(
            !types.contains("pub enum U ") || expect_variants.is_some(),
            "{shape}: `U` must not be a union of indistinguishable variants: {types}"
        );
    }

    // Beside a nullable object, each branch that states nothing admits `null`, so `null` matches
    // both and the `oneOf` rejects it: the collapsed position is required
    // and non-nullable, as the `$ref` spelling of the same document is.
    let spec = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      type: object
      properties:
        x:
          type: [object, 'null']
          {props}
          {keywordless_branches}
      required: [x]
"##
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let types = types_module(&code);
    let x = field_type(&types, "pub x").unwrap_or_else(|| panic!("no `x` field: {types}"));
    assert!(
        !x.starts_with("Option<"),
        "`x` is `{x}`, but `null` matches both branches and is invalid: {types}"
    );
    assert_eq!(declared_fields(&types, &x), ["a", "b"], "{types}");

    // An `anyOf` of the same branches decodes: one match is enough, so it is not collapsed.
    let spec = single_component_document(&format!(
        "      type: object\n      {props}\n      anyOf: [ \
                                            {{ required: [a] }}, {{ required: [b] }} ]\n"
    ));
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        enum_variants(&types_module(&code), "U").len(),
        2,
        "an `anyOf` keeps its variants: {code}"
    );

    // Exactly one merged branch accepts `null`: `null` matches that branch alone, so it stays valid
    // and the merged position stays `Option<String>`.
    let one_nullable = "oneOf: [ { type: [string, 'null'] }, { type: string } ]";
    let spec = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      type: object
      properties:
        x:
          {one_nullable}
      required: [x]
"##
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let types = types_module(&code);
    let x = field_type(&types, "pub x").unwrap_or_else(|| panic!("no `x` field: {types}"));
    let inner = x
        .strip_prefix("Option<")
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or_else(|| panic!("`x` is `{x}`, but `null` matches one branch only: {types}"));
    assert!(
        inner == "String" || types.contains(&format!("pub type {inner} = String;")),
        "`x` must be the one `String` both branches lower to: {types}"
    );
}

/// The merge above for `oneOf` branches that lower to distinct generated items of one structure
/// (#492): two inline objects with the same fields, `required` flags, and `additionalProperties`
/// policy, nested ones included, or two string enums listing one value set in any order. Each is
/// its own nominal item, but no value tells them apart, so they become one variant with `W001`.
/// Branches that differ in a `required` flag, a value, the `additionalProperties` policy (`false`,
/// `true`, or a schema, or two schemas that decode different values), or a field's XML hint in an
/// XML body (`xml.name`, `xml.attribute`) decode different values, so they stay distinct variants,
/// unwarned. (String-enum openness is pinned in `ir/types.rs`'s tests: every `oneOf` branch is
/// lowered closed, so no document puts an open and a closed set of one value set side by side.)
#[test]
fn one_of_branches_of_one_structure_merge_and_warn() {
    let object = "{ type: object, additionalProperties: false, required: [a], properties: { a: { \
                  type: string } } }";
    let nested = "{ type: object, required: [n], properties: { n: { type: object, properties: { \
                  a: { type: string } } } } }";
    let optional = "{ type: object, additionalProperties: false, properties: { a: { type: string \
                    } } }";
    let additional = |policy: &str| {
        format!(
            "{{ type: object, additionalProperties: {policy}, required: [a], properties: {{ a: {{ \
             type: string }} }} }}"
        )
    };
    let hinted = |xml: &str| {
        format!(
            "{{ type: object, required: [a], properties: {{ a: {{ type: string, xml: {xml} }} }} }}"
        )
    };
    let plain = "{ type: object, required: [a], properties: { a: { type: string } } }";
    let (json, xml) = ("application/json", "application/xml");
    for (shape, members, media, merged, variants) in [
        (
            "identical objects",
            format!("{object}, {object}"),
            json,
            true,
            None,
        ),
        (
            "identical nested objects",
            format!("{nested}, {nested}"),
            json,
            true,
            None,
        ),
        (
            "string enums of one value set",
            "{ enum: [x, y] }, { enum: [y, x] }, { type: integer }".to_owned(),
            json,
            true,
            Some(2),
        ),
        (
            "a different required flag",
            format!("{object}, {optional}"),
            json,
            false,
            Some(2),
        ),
        (
            "different enum values",
            "{ enum: [x, y] }, { enum: [x, z] }".to_owned(),
            json,
            false,
            Some(2),
        ),
        (
            "additionalProperties false beside true",
            format!("{}, {}", additional("false"), additional("true")),
            json,
            false,
            Some(2),
        ),
        (
            "additionalProperties false beside a schema",
            format!(
                "{}, {}",
                additional("false"),
                additional("{ type: integer }")
            ),
            json,
            false,
            Some(2),
        ),
        (
            "additionalProperties true beside a schema",
            format!(
                "{}, {}",
                additional("true"),
                additional("{ type: integer }")
            ),
            json,
            false,
            Some(2),
        ),
        (
            "additionalProperties schemas of different types",
            format!(
                "{}, {}",
                additional("{ type: integer }"),
                additional("{ type: boolean }")
            ),
            json,
            false,
            Some(2),
        ),
        (
            "an xml.name hint",
            format!("{}, {plain}", hinted("{ name: b }")),
            xml,
            false,
            Some(2),
        ),
        (
            "an xml.attribute hint",
            format!("{}, {plain}", hinted("{ attribute: true }")),
            xml,
            false,
            Some(2),
        ),
    ] {
        let spec = single_component_document(&format!("      oneOf: [ {members} ]\n"))
            .replace("application/json", media);
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{shape} via {entry}: {report:#?}"
            );
            assert_eq!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/U"
                        && d.message.contains("identically structured")
                }),
                merged,
                "{shape} via {entry}: {report:#?}"
            );
        }
        let types = types_module(&code);
        match variants {
            Some(count) => assert_eq!(
                enum_variants(&types, "U").len(),
                count,
                "{shape}: wrong variant count: {types}"
            ),
            None => assert!(
                types.contains("pub struct U ") && !types.contains("pub enum U "),
                "{shape}: `U` must be the one object both branches lower to: {types}"
            ),
        }
    }
}

/// Two authored `oneOf` branches that are arrays of uninhabited items decode the same values (only
/// `[]`), so they merge with `W001` into one type, as the branches of one structure above do:
/// each `items: false` lowers to its own empty `pub enum`, but neither decodes a value, so no value
/// tells the two arrays apart (#643). `U` is the one array, not an enum with a variant per branch.
#[test]
fn one_of_arrays_of_uninhabited_items_merge_and_warn() {
    let spec = single_component_document(
        "      oneOf: [ { type: array, items: false }, { type: array, items: false } ]\n",
    );
    let (report, code) = generate_with_code(&spec);
    for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/components/schemas/U"
            }),
            "via {entry}: {report:#?}"
        );
    }
    let types = types_module(&code);
    assert!(
        types.contains("pub type U = Vec<Uvariant0Item>;") && !types.contains("pub enum U "),
        "`U` must be the one array both branches lower to: {types}"
    );
}

/// A `oneOf` admits `null` only when exactly one of its branches does, whether or not any of its
/// variants merge (#563). The branches of `Pick` are typed `[object, 'null']` and stay two variants
/// after the meet with `NB`, which admits `null` too: `null` matches both branches and fails
/// exactly-one, so the position is not `Option`, in the `allOf`-member spelling, in the
/// `$ref`-sibling one, and inline with no meet at all, two nullable scalars or a `null` member
/// beside a nullable branch or an untyped object one (#622). Where only one branch accepts `null`, or the union is an `anyOf`, it
/// stays valid.
#[test]
fn a_one_of_whose_null_matches_two_unmerged_branches_rejects_null() {
    let nullable_objects =
        "\n            - { type: [object, 'null'], required: [a] }\n            \
                            - { type: [object, 'null'], required: [b] }";
    let nullable_refs = "\n            - $ref: '#/components/schemas/NA'\n            \
                         - $ref: '#/components/schemas/NC'";
    let discriminator = "discriminator: { propertyName: kind }";
    for (shape, schema, nullable) in [
        (
            "allOf member",
            format!(
                "allOf:\n          - $ref: '#/components/schemas/NB'\n          - oneOf:\
                 {nullable_objects}"
            ),
            false,
        ),
        (
            "$ref sibling",
            format!("$ref: '#/components/schemas/NB'\n          oneOf:{nullable_objects}"),
            false,
        ),
        // The typed branch's `null` is hoisted onto the union before the meet, and the untyped
        // one takes `null` from `NB` in the meet: the two variants stay distinct, and both accept
        // `null`.
        (
            "allOf member, one typed and one untyped branch",
            "allOf:\n          - $ref: '#/components/schemas/NB'\n          - oneOf:\n            \
             - { type: [object, 'null'], required: [a] }\n            - { required: [b] }"
                .to_owned(),
            false,
        ),
        (
            "inline nullable scalars",
            "oneOf: [ { type: [string, 'null'] }, { type: [integer, 'null'] } ]".to_owned(),
            false,
        ),
        (
            "inline null member beside a nullable branch",
            "oneOf: [ { type: 'null' }, { type: [string, 'null'] }, { type: integer } ]".to_owned(),
            false,
        ),
        (
            "inline null member beside its only nullable branch",
            "oneOf: [ { type: 'null' }, { type: [string, 'null'] } ]".to_owned(),
            false,
        ),
        // The untyped member takes `null` from the sibling in the meet, as the multi-member path's
        // untyped variants do, so `null` is in its branch and the `null` member's.
        (
            "null member beside an untyped branch met with a nullable type, its only nullable \
             branch",
            "type: [object, 'null']\n          oneOf: [ { type: 'null' }, { required: [a] } ]"
                .to_owned(),
            false,
        ),
        (
            "null member beside an untyped branch met with `NB`, its only nullable branch",
            "$ref: '#/components/schemas/NB'\n          \
             oneOf: [ { type: 'null' }, { required: [a] } ]"
                .to_owned(),
            false,
        ),
        (
            "allOf member, null member beside an untyped branch, its only nullable branch",
            "allOf:\n          - $ref: '#/components/schemas/NB'\n          \
             - oneOf: [ { type: 'null' }, { required: [a] } ]"
                .to_owned(),
            false,
        ),
        // Without a meet the untyped member is counted all the same: `required` constrains
        // objects alone, so `null` matches it and the `null` member (#622).
        (
            "inline null member beside an untyped branch, its only nullable branch",
            "oneOf: [ { type: 'null' }, { required: [a] } ]".to_owned(),
            false,
        ),
        // `null` carries no tag, so a discriminator does not exempt the count.
        (
            "discriminated nullable $refs",
            format!("oneOf:{nullable_refs}\n          {discriminator}"),
            false,
        ),
        (
            "allOf member, discriminated nullable $refs",
            format!(
                "allOf:\n          - $ref: '#/components/schemas/NB'\n          - oneOf:\
                 {nullable_refs}\n            {discriminator}"
            ),
            false,
        ),
        (
            "inline single nullable branch",
            "oneOf: [ { type: [string, 'null'] }, { type: integer } ]".to_owned(),
            true,
        ),
        (
            "inline null member beside non-null branches",
            "oneOf: [ { type: 'null' }, { type: string }, { type: integer } ]".to_owned(),
            true,
        ),
        (
            "inline anyOf",
            "anyOf: [ { type: [string, 'null'] }, { type: [integer, 'null'] } ]".to_owned(),
            true,
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    NA:
      type: [object, 'null']
      properties:
        kind: {{ type: string }}
        a: {{ type: string }}
      required: [kind]
    NC:
      type: [object, 'null']
      properties:
        kind: {{ type: string }}
        c: {{ type: string }}
      required: [kind]
    Holder:
      type: object
      properties:
        x:
          {schema}
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{shape}: {report:#?}");
        let types = types_module(&code);
        let x = field_type(&types, "pub x").unwrap_or_else(|| panic!("{shape}: no `x`: {types}"));
        assert_eq!(
            x.starts_with("Option<"),
            nullable,
            "{shape}: `x` is `{x}`, but `null` is {} here: {types}",
            if nullable { "valid" } else { "invalid" }
        );
        // With a single real branch the position is that branch's own type, not a union.
        let single = shape.contains("only nullable branch");
        if !nullable && !single {
            let union = x.trim_start_matches("Box<").trim_end_matches('>');
            let variants = enum_variants(&types, union);
            assert!(
                variants.len() >= 2,
                "{shape}: `{union}` must stay a union of distinct variants: {types}"
            );
            assert!(
                !variants.iter().any(|variant| variant.contains("(Option<")),
                "{shape}: no variant may accept `null`, which matches two branches: {variants:?}"
            );
        }
    }
}

/// A union with one real member is that member re-emitted under the union's own name, so the
/// member's own lowering, which took the same name first, is a second copy nothing names (#926):
/// `S0` emitted `pub struct S071def3c4 {}` beside `pub struct S0 {}`. Only the re-emitted type is
/// emitted, with or without a `null` member and a sibling meet; the harness's
/// [`assert_no_unreferenced_copy`] holds every other fixture to the same.
#[test]
fn a_union_with_one_real_member_emits_no_copy_of_it() {
    for (shape, body) in [
        ("a single-branch `oneOf`", "{ oneOf: [ { type: object } ] }"),
        (
            "a single-branch `anyOf` beside a `null` member",
            "{ anyOf: [ { type: string }, { type: 'null' } ] }",
        ),
        (
            "a single-branch `oneOf` met with a sibling",
            "{ oneOf: [ { type: object, properties: { a: { type: string } } } ], required: [a] }",
        ),
    ] {
        let spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    S0: {body}\n"
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{shape}: {report:#?}");
        let types = types_module(&code);
        let declared: Vec<&str> = types
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';', '=']).next())
            .filter(|name| name.starts_with("S0"))
            .collect();
        // A copy takes the name with the stable 8-hex-digit suffix of its pointer.
        let copies: Vec<&&str> = declared
            .iter()
            .filter(|name| {
                name.len() > 8
                    && name[name.len() - 8..]
                        .chars()
                        .all(|c| c.is_ascii_hexdigit())
            })
            .collect();
        assert!(
            declared.contains(&"S0") && copies.is_empty(),
            "{shape}: `S0` and no copy of it: {declared:?}\n{code}"
        );
    }
}

/// Issue #934: a `oneOf` an `allOf`'s scalar meet leaves with every variant distinct is emitted
/// once, under its own hint. Its variants' `null` is the one decision's to settle once the
/// conjunction is lowered, so the meet inserts no second copy of the union to clear it, and no
/// generated type takes the 8-hex-digit suffix a copy is named with. Here two `oneOf` members,
/// each a branch that states nothing beside a one-value string enum, meet `type: [string, 'null']`
/// in every member order and nested in an inner `allOf`.
#[test]
fn a_met_one_of_whose_variants_stay_distinct_is_emitted_once_under_its_hint() {
    let a = "{ oneOf: [ {}, { type: string, enum: [a] } ] }";
    let b = "{ oneOf: [ {}, { type: string, enum: [b] } ] }";
    let ty = "{ type: [string, 'null'] }";
    let mut mismatches = Vec::new();
    for site in [
        format!("{{ allOf: [ {ty}, {a}, {b} ] }}"),
        format!("{{ allOf: [ {a}, {ty}, {b} ] }}"),
        format!("{{ allOf: [ {a}, {b}, {ty} ] }}"),
        format!("{{ allOf: [ {ty}, {{ allOf: [ {a}, {b} ] }} ] }}"),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{site}: {report:#?}");
        let types = types_module(&code);
        // A copy takes the name with the stable 8-hex-digit suffix of its pointer.
        let copies: Vec<&str> = types
            .lines()
            .map(str::trim_start)
            .filter_map(|line| line.strip_prefix("pub enum "))
            .filter_map(|rest| rest.split([' ', '<', '{']).next())
            .filter(|name| {
                name.starts_with("Pick")
                    && name.len() > 8
                    && name[name.len() - 8..]
                        .chars()
                        .all(|c| c.is_ascii_hexdigit())
            })
            .collect();
        if !copies.is_empty() || !types.contains("pub enum PickIntersection2Variant0 {") {
            mismatches.push(format!(
                "{site}: the met union is not `PickIntersection2Variant0` alone: {copies:?}\n{types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #934: a union a sibling meet or a `null` count leaves one variant is that variant's
/// type, re-emitted under the union's name, so the variant's own type, which nothing else names,
/// is not emitted beside it, as a union the one decision collapses to one variant leaves none.
/// Here a branch the sibling excludes, an exact-`null` branch `null` matches beside another, and
/// branches that state nothing or hold scalar keywords alone each leave one variant, whose type is
/// the named `…Variant` or `…Constraint` type.
#[test]
fn a_union_left_one_variant_emits_no_copy_of_that_variants_type() {
    let mut mismatches = Vec::new();
    for (site, variant) in [
        (
            "{ type: [string, 'null'], oneOf: [ { items: { type: string } }, { type: string } ] }",
            "PickVariant1",
        ),
        (
            "{ type: [string, 'null'], oneOf: [ { items: { type: string } }, { const: null }, \
             { type: string } ] }",
            "PickVariant2",
        ),
        (
            "{ type: [string, 'null'], oneOf: [ {}, {} ] }",
            "PickConstraint",
        ),
        (
            "{ type: [string, 'null'], oneOf: [ {}, { const: null } ] }",
            "PickConstraint",
        ),
        (
            "{ type: [string, 'null'], oneOf: [ { minLength: 1 }, { type: [string, 'null'] } ] }",
            "PickConstraint",
        ),
        (
            "{ required: [u], oneOf: [ { const: null }, {} ] }",
            "PickConstraint",
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{site}: {report:#?}");
        let types = types_module(&code);
        let declared = [" = ", " {"].iter().any(|after| {
            ["pub struct ", "pub type "]
                .iter()
                .any(|item| types.contains(&format!("{item}{variant}{after}")))
        });
        if declared {
            mismatches.push(format!(
                "{site}: `{variant}` is emitted beside `Pick`\n{types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #987: a union a sibling meet or a merge leaves one variant is that variant's kind,
/// re-emitted under the union's own name, so what the union lowered on the way is unused unless
/// that kind names it: each member's own lowering (`PickVariant<N>`, with its `…Item`) and the
/// sibling keywords' own lowering (`PickConstraint`). A branch the sibling excludes, branches the
/// one-variant merge folds, a branch of scalar keywords alone, and a sole member each leave one
/// variant here. Every `Pick…` type declared beside `Pick` is named by another declaration: where
/// the kind is a scalar that is none, and where it is an array, the item type it names.
#[test]
fn a_union_left_one_variant_emits_none_of_its_members_or_siblings_own_lowerings() {
    let mut mismatches = Vec::new();
    for site in [
        "{ type: [string, 'null'], oneOf: [ { items: { type: string } }, { type: string } ] }",
        "{ type: [string, 'null'], anyOf: [ { items: { type: string } }, { type: string } ] }",
        "{ type: [string, 'null'], oneOf: [ { items: { type: string } }, { const: null }, \
         { type: string } ] }",
        "{ type: [string, 'null'], oneOf: [ {}, {} ] }",
        "{ type: [string, 'null'], oneOf: [ {}, { const: null } ] }",
        "{ type: [integer, 'null'], oneOf: [ { minLength: 1 }, { type: integer } ] }",
        "{ type: [string, 'null'], oneOf: [ { minLength: 1 }, { type: [string, 'null'] } ] }",
        "{ type: [string, 'null'], oneOf: [ { minLength: 1 } ] }",
        "{ type: [array, 'null'], oneOf: [ { items: { type: string } }, { type: string } ] }",
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{site}: {report:#?}");
        let types = types_module(&code);
        let declared: Vec<&str> = types
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';', '=']).next())
            .filter(|name| name.starts_with("Pick"))
            .collect();
        // A name occurs once, in its own declaration, where nothing else names it.
        let occurrences = |name: &str| {
            types
                .match_indices(name)
                .filter(|(at, _)| {
                    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
                    !types[..*at].ends_with(ident) && !types[at + name.len()..].starts_with(ident)
                })
                .count()
        };
        let unnamed: Vec<&&str> = declared
            .iter()
            .filter(|name| **name != "Pick" && occurrences(name) < 2)
            .collect();
        if !declared.contains(&"Pick") || !unnamed.is_empty() {
            mismatches.push(format!("{site}: nothing names {unnamed:?}\n{types}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
