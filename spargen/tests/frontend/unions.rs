//! `oneOf` / `anyOf` unions: collapse and merge of branches, nullability, trial matching, and the
//! rejections a union draws.

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
    for schema in [
        "anyOf: [{ type: string }, {}]",
        "anyOf: [{ required: [a] }, { required: [b] }]",
    ] {
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
        let found = oracles::unknown(oracles::indistinguishable_variants(&code));
        assert!(found.is_empty(), "{schema}: {found:#?}");
    }
    let (report, _) = generate_with_code(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    \
         U:\n      oneOf: [{ required: [a] }, { required: [b] }]\n",
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
/// (`NB: type: [object, "null"]`), each required-only branch admits `null`, since `required` binds
/// objects only. An `anyOf` needs just one branch to match, so `null` stays valid and the position
/// is `Option<_>`. A `oneOf` needs exactly one, and `null` matches both, so `null` is invalid and
/// the position is required and non-nullable. A `{type: 'null'}` member does not change either
/// answer: under `oneOf`, `null` then matches that member *and* both required-only branches, so it
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
        let keyword = format!("{union_keyword}[required a, required b{null_member}]");
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
          {union_keyword}: [ {{ required: [a] }}, {{ required: [b] }}{null_member} ]
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
/// collapse above spelled without the `$ref` (#402). Branches of nothing but `required` beside
/// `type: object` and `properties` each meet the sibling to the same object, and bare ones each
/// lower to `serde_json::Value`; emitted as two variants, every value matches both, so the
/// exactly-one check fails every decode. The position is that one type instead, and the branch
/// distinctions are reported (`W001`) rather than generating a union nothing can decode. Where only
/// some branches share a type, they become one variant and the others stand. `null` follows the
/// collapse's `oneOf` rule: two branches that both accept it fail exactly-one, so it is invalid.
#[test]
fn an_inline_one_of_whose_branches_lower_to_one_type_collapses_and_warns() {
    let props = "properties: { a: { type: string }, b: { type: string } }";
    let required_branches = "oneOf: [ { required: [a] }, { required: [b] } ]";
    for (shape, body, expect_fields, expect_variants) in [
        (
            "beside type: object",
            format!("      type: object\n      {props}\n      {required_branches}\n"),
            Some(["a", "b"]),
            None,
        ),
        ("bare", format!("      {required_branches}\n"), None, None),
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

    // Beside a nullable object, each `required`-only branch admits `null` (`required` binds objects
    // only), so `null` matches both and the `oneOf` rejects it: the collapsed position is required
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
          {required_branches}
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

/// A `oneOf` admits `null` only when exactly one of its branches does, whether or not any of its
/// variants merge (#563). The branches of `Pick` are typed `[object, 'null']` and stay two variants
/// after the meet with `NB`, which admits `null` too: `null` matches both branches and fails
/// exactly-one, so the position is not `Option`, in the `allOf`-member spelling, in the
/// `$ref`-sibling one, and inline with no meet at all, two nullable scalars or a `null` member
/// beside a nullable branch. Where only one branch accepts `null`, or the union is an `anyOf`, it
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
        // Without a meet the untyped member is not counted, so `null` matches the `null` member
        // alone.
        (
            "inline null member beside an untyped branch",
            "oneOf: [ { type: 'null' }, { required: [a] } ]".to_owned(),
            true,
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

/// A `"null"` in the `type` array beside a `oneOf`/`anyOf` only permits `null`: the union still
/// needs a branch `null` matches (exactly one for `oneOf`, at least one for `anyOf`), so its
/// branches alone decide whether the position is `Option` (#574). Branches typed `object` beside
/// `type: [object, 'null']` all refuse `null`, so the field is not `Option`, with several branches
/// or one, and where the multi-type array is dropped for lowering with nothing else beside it.
/// A branch that accepts `null` itself, or an untyped one that takes it from the `type` array,
/// keeps it valid.
#[test]
fn a_union_admits_null_only_through_a_branch_that_accepts_it() {
    let typed_objects = "\n            - { type: object, required: [a] }\n            \
                         - { type: object, required: [c], properties: { d: { type: string } } }";
    let props = "properties: { a: { type: string }, c: { type: integer } }";
    for (shape, schema, nullable) in [
        (
            "oneOf of typed objects beside a nullable object type",
            format!("type: [object, 'null']\n          {props}\n          oneOf:{typed_objects}"),
            false,
        ),
        (
            "anyOf of typed objects beside a nullable object type",
            format!("type: [object, 'null']\n          {props}\n          anyOf:{typed_objects}"),
            false,
        ),
        (
            "sole typed branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ type: object, required: [a] }} ]"
            ),
            false,
        ),
        (
            "typed scalars beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             oneOf: [ { type: string }, { type: integer } ]"
                .to_owned(),
            false,
        ),
        (
            "anyOf of typed scalars beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             anyOf: [ { type: string }, { type: integer } ]"
                .to_owned(),
            false,
        ),
        (
            "sole typed scalar beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ { type: string } ]".to_owned(),
            false,
        ),
        (
            "a branch that accepts null itself",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ type: [object, 'null'], required: [a] }}\n            \
                 - {{ type: object, required: [c] }}"
            ),
            true,
        ),
        (
            "an untyped branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ required: [a] }}\n            \
                 - {{ type: object, required: [c], properties: {{ d: {{ type: string }} }} }}"
            ),
            true,
        ),
        (
            "an anyOf of untyped object branches beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 anyOf: [ {{ properties: {{ a: {{ type: string }} }} }}, {{ properties: {{ b: {{ \
                 type: string }} }} }} ]"
            ),
            true,
        ),
        (
            "a sole untyped object branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ properties: {{ b: {{ type: string }} }} }} ]"
            ),
            true,
        ),
        // `null` matches both untyped branches, which fails the `oneOf`'s exactly-one.
        (
            "a oneOf of two untyped object branches beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ properties: {{ a: {{ type: string }} }} }}, {{ properties: {{ b: {{ \
                 type: string }} }} }} ]"
            ),
            false,
        ),
        (
            "an untyped object branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { properties: { a: { type: string } } }, { type: string } ]"
                .to_owned(),
            true,
        ),
        (
            "a sole untyped object branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { properties: { a: { type: string } } } ]"
                .to_owned(),
            true,
        ),
        (
            "a null member beside typed scalars and a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             oneOf: [ { type: 'null' }, { type: string }, { type: integer } ]"
                .to_owned(),
            true,
        ),
        // A nested union of untyped branches states nothing about `null` either: `null` matches
        // both of its `anyOf`'s branches, so it matches the nested union.
        (
            "an untyped nested anyOf branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ anyOf: [ {{ required: [a] }}, {{ required: [c] }} ] }}\n            \
                 - {{ type: object, required: [c], properties: {{ d: {{ type: string }} }} }}"
            ),
            true,
        ),
        (
            "a sole untyped nested anyOf branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ anyOf: [ {{ required: [a] }}, {{ required: [c] }} ] }} ]"
            ),
            true,
        ),
        (
            "an untyped nested anyOf branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { anyOf: [ { required: [a] }, { required: [b] } ] }, { type: string } ]"
                .to_owned(),
            true,
        ),
        // `null` matches both branches of the nested `oneOf`, so it matches no branch here.
        (
            "an untyped nested oneOf branch that null matches twice",
            "type: [object, string, 'null']\n          \
             oneOf: [ { oneOf: [ { required: [a] }, { required: [b] } ] }, { type: string } ]"
                .to_owned(),
            false,
        ),
        // A `true` branch lowers to `Value` and takes the array's `null` with no sibling to meet.
        (
            "a true branch beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ true, { type: integer } ]"
                .to_owned(),
            true,
        ),
        (
            "a sole true branch beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ true ]".to_owned(),
            true,
        ),
        // No union keyword at all: the array's own `null` is the synthesized union's null branch.
        (
            "a plain multi-type array",
            "type: [string, integer, 'null']".to_owned(),
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
    }
}

/// A cycle-closing `$ref` branch to an untyped object component takes the `null` a dropped
/// multi-type array permits, as an inline untyped branch does (#574): the component's struct is
/// still a reservation when the branch is lowered, so whether `null` matches it is read from the
/// target's own keywords, and `properties` is vacuous on `null`.
#[test]
fn a_cycle_closing_ref_branch_to_an_untyped_object_takes_the_type_arrays_null() {
    for (shape, schema) in [
        (
            "a sole cycle-closing branch",
            "type: [object, array, 'null']\n          oneOf: [ { $ref: '#/components/schemas/Holder' } ]",
        ),
        (
            "a cycle-closing branch beside a typed one",
            "type: [object, string, 'null']\n          \
             oneOf: [ { $ref: '#/components/schemas/Holder' }, { type: string } ]",
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
    Holder:
      properties:
        child:
          {schema}
      required: [child]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{shape}: {report:#?}");
        let types = types_module(&code);
        let child = field_type(&types, "pub child")
            .unwrap_or_else(|| panic!("{shape}: no `child`: {types}"));
        assert!(
            child.starts_with("Option<"),
            "{shape}: `child` is `{child}`, but `null` is valid here: {types}"
        );
    }
}

/// Issue #541, beside a union: a `$ref` to an untyped object component `U` admits `null` without
/// deciding it when the meet also carries a `oneOf`/`anyOf`, as the inline untyped member does.
/// The `$ref` with a union sibling (`refUnion`) is met in `meet_ref_union_sibling`, and the
/// `$ref` member beside a union member (`memberRef`) or beside a union on the `allOf` itself
/// (`besideRef`) is combined as a composition; each recorded `U`'s non-null struct as a decision,
/// so it denied the `null` the inline spelling of the same meet admits. The `type: object`
/// controls stay non-null. `refUnion`'s branches state `'null'` themselves: a `$ref`'s union
/// sibling of untyped branches is lowered apart from the sibling's `type`, and denies `null`
/// whatever the target (#567), so only branches that admit it show the target's answer.
#[test]
fn a_ref_to_an_untyped_object_component_admits_null_beside_a_union() {
    const COMPONENTS: &str = r##"
components:
  schemas:
    U: { properties: { u: { type: string } } }
    Holder:
      type: object
      required: [refUnion, inlineUnion, memberRef, memberInline, besideRef, besideInline, refUnionObject, memberRefObject]
      properties:
        refUnion: { $ref: '#/components/schemas/U', type: [object, 'null'], anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        inlineUnion: { type: [object, 'null'], properties: { u: { type: string } }, anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        memberRef: { allOf: [{ $ref: '#/components/schemas/U' }, { type: [object, 'null'], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
        memberInline: { allOf: [{ properties: { u: { type: string } } }, { type: [object, 'null'], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
        besideRef: { type: [object, 'null'], allOf: [{ $ref: '#/components/schemas/U' }], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }
        besideInline: { type: [object, 'null'], allOf: [{ properties: { u: { type: string } } }], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }
        refUnionObject: { $ref: '#/components/schemas/U', type: object, anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        memberRefObject: { allOf: [{ $ref: '#/components/schemas/U' }, { type: object, anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
"##;
    const EXPECTED: [(&str, bool); 8] = [
        ("pub ref_union:", true),
        ("pub inline_union:", true),
        ("pub member_ref:", true),
        ("pub member_inline:", true),
        ("pub beside_ref:", true),
        ("pub beside_inline:", true),
        ("pub ref_union_object:", false),
        ("pub member_ref_object:", false),
    ];
    let root = format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
{COMPONENTS}"##
    );
    let (generated, root_code) = generate_with_code(&root);
    let checked = check(&root);
    let (split_generated, split_checked, split_code) =
        split("./lib.yaml#/components/schemas/Holder", COMPONENTS);
    for (entry, report) in [
        ("root/generate", &generated),
        ("root/check", &checked),
        ("split/generate", &split_generated),
        ("split/check", &split_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for (spelling, code) in [("root", &root_code), ("split", &split_code)] {
        for (field, nullable) in EXPECTED {
            let ty = field_type(code, field)
                .unwrap_or_else(|| panic!("{spelling}: no `{field}` field: {code}"));
            assert_eq!(
                ty.starts_with("Option<"),
                nullable,
                "{spelling}: `{field}` is `{ty}`, but `null` is {} here: {code}",
                if nullable { "valid" } else { "invalid" }
            );
        }
    }
}

/// Issue #581: a union whose only typed branch denies `null` (`type: object`, `type: string`, the
/// `false` schema, or an `enum` or `const` that leaves `null` out), beside an untyped object
/// branch, met with an untyped object composition. The untyped branch leaves `null` undecided, so
/// every spelling of the meet keeps the non-null struct: a `$ref` to the untyped `U` with the
/// union as its sibling, `U` as an `allOf` member beside the union or with the union as a second
/// member, and the same three with `U`'s body written inline. The `allOf` spellings over `U` read
/// the typed branch as the union deciding `null`, made the composition nullable for it, and
/// generated `Option<Pick>` where the others generate `Pick`.
/// A branch that states nothing (`true`, `{}`, annotations alone) leaves `null` undecided as the
/// untyped branch does (#588): the inline spelling, and the untyped sibling keywords `items` or
/// `required` alone, met its `Value` with the sibling's object half, took that half's `null` as
/// the branch's own, and generated `Option<Pick>`.
/// A typed branch that admits `null` (`type: [object, 'null']`) still decides it in the `anyOf`
/// `allOf` spellings, which stay `Option`.
#[test]
fn a_union_of_a_non_null_typed_branch_beside_an_untyped_one_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for (typed, admits) in [
            (
                "{ type: object, properties: { a: { type: string } } }",
                false,
            ),
            ("{ type: string }", false),
            ("false", false),
            ("{ enum: [x, y] }", false),
            ("{ const: x }", false),
            // A branch that states nothing (#588): the `true` schema, `{}`, and a branch of
            // annotations alone accept `null` as the untyped branch beside them does, and decide
            // it no more than it does.
            ("true", false),
            ("{}", false),
            ("{ description: d }", false),
            (
                "{ type: [object, 'null'], properties: { a: { type: string } } }",
                true,
            ),
        ] {
            let branches = format!("[ {typed}, {{ properties: {{ b: {{ type: string }} }} }} ]");
            // The nullable control is pinned in the `anyOf` `allOf` spellings only: every
            // spelling of the nullable rows is pinned by
            // `a_union_of_a_nullable_typed_branch_beside_an_untyped_one_agrees_across_spellings`.
            if admits && keyword == "oneOf" {
                continue;
            }
            let mut sites = vec![
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
            ];
            if !admits {
                sites.push(format!("{{ {u}, {keyword}: {branches} }}"));
                sites.push(format!(
                    "{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                ));
                // Untyped sibling keywords alone leave `null` to the union as `U` does: an
                // array-only `items`, and an object-only `required` that `null` satisfies.
                sites.push(format!(
                    "{{ items: {{ type: string }}, {keyword}: {branches} }}"
                ));
                sites.push(format!("{{ required: [u], {keyword}: {branches} }}"));
            }
            rows.extend(sites.into_iter().map(|site| (site, admits)));
        }
    }
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
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
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "undecided" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #590: a union of a `$ref` branch to the non-null object `Y` beside an untyped object
/// branch, met with the untyped object `U`, keeps the non-null struct in all six spellings of the
/// meet, as the same branch written inline does
/// (`a_union_of_a_non_null_typed_branch_beside_an_untyped_one_keeps_null_undecided`): a `$ref` to
/// `U` with the union as its sibling, `U` as an `allOf` member beside the union or with the union
/// as a second member, and the same three with `U`'s body written inline. The `allOf` spellings
/// over `U` read the `$ref` branch as the union deciding `null` without following it to `Y`, made
/// the composition nullable for it, and generated `Option<Pick>`. The branch reached through the
/// alias `Z` of `Y`, a `$ref` to the untyped `U` whose sibling `type: object` denies `null`, and
/// a `$ref` to the component `W` that is that same `$ref`-with-sibling, agree with the plain
/// `$ref` to `Y`. A `$ref` branch to the nullable
/// `N` still decides `null` in the `anyOf` `allOf` spellings, which stay `Option`.
#[test]
fn a_union_of_a_ref_branch_to_a_non_null_object_beside_an_untyped_one_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        // `Y` itself, the alias `Z` of it, the untyped `U` under a sibling `type: object`,
        // which denies `null` by the branch's own keyword whatever its target says, and `W`,
        // the component spelling of that same `$ref`-with-sibling branch.
        for denying in [
            "{ $ref: '#/components/schemas/Y' }",
            "{ $ref: '#/components/schemas/Z' }",
            "{ $ref: '#/components/schemas/U', type: object }",
            "{ $ref: '#/components/schemas/W' }",
        ] {
            let branches = format!("[ {denying}, {{ properties: {{ b: {{ type: string }} }} }} ]");
            rows.extend(
                [
                    format!("{{ {u}, {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                    format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                ]
                .into_iter()
                .map(|site| (site, false)),
            );
        }
    }
    // The nullable control: a `$ref` branch whose target admits `null` decides it.
    let nullable =
        "[ { $ref: '#/components/schemas/N' }, { properties: { b: { type: string } } } ]";
    rows.push((
        format!("{{ allOf: [ {{ {u} }} ], anyOf: {nullable} }}"),
        true,
    ));
    rows.push((
        format!("{{ allOf: [ {{ {u} }}, {{ anyOf: {nullable} }} ] }}"),
        true,
    ));
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
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
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Y: {{ type: object, properties: {{ y: {{ type: string }} }} }}
    Z: {{ $ref: '#/components/schemas/Y' }}
    W: {{ $ref: '#/components/schemas/U', type: object }}
    N: {{ type: [object, 'null'], properties: {{ n: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{site}, via {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "undecided" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #586: a union of one typed branch that admits `null` (`type: [object, 'null']`,
/// `type: 'null'`, `enum: [x, null]`, `const: null` or `type: [string, 'null']`) beside an
/// untyped object branch, met with the untyped object `U`, gets one answer per row across all six
/// spellings: a `$ref` to `U` with the union as its sibling, `U` as an `allOf` member beside the
/// union or with the union as a second member, and the same three with `U`'s body written inline,
/// plus the `$ref` spelling with an untyped `required` beside the union, which meets it through
/// the same target, and two untyped sibling keywords alone, an array-only `items`, which does not
/// reach an object branch, and an object-only `required`, which `null` satisfies: both leave
/// `null` to the union as `U` does. `U` decides nothing about `null`, so an `anyOf` admits it through the typed
/// branch. A `oneOf` denies it: the untyped branch beside the typed one is counted as accepting
/// `null` too (#563), so `null` is in two branches and fails exactly-one; a `const: null` branch,
/// whose exact-`null` variant is not a nullable `Ty`, is counted as one of them. The
/// `$ref`-sibling spellings denied `null` in the `anyOf` rows, and the inline spellings admitted
/// it in the `oneOf` row with the nullable object branch. A branch that states nothing (`true`,
/// `{}`) in place of the untyped one gets the same answers (#592): the `$ref` and `allOf`
/// spellings did not count it as taking `null` from the conjunct the union is met with, so an
/// `anyOf` beside `const: null` denied `null` there, and a `oneOf` beside a nullable typed branch
/// counted `null` in one branch alone and admitted it.
#[test]
fn a_union_of_a_nullable_typed_branch_beside_an_untyped_one_agrees_across_spellings() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for typed in [
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "{ type: 'null' }",
            "{ enum: [x, null] }",
            "{ const: null }",
            "{ type: [string, 'null'] }",
        ] {
            let admits = keyword == "anyOf";
            // A branch that states nothing (`true`, `{}`) in place of the untyped one accepts
            // `null` wherever the untyped one does (#588, #592), so the answers stand in every
            // spelling.
            for other in ["{ properties: { b: { type: string } } }", "true", "{}"] {
                let branches = format!("[ {typed}, {other} ]");
                rows.extend(
                    [
                        format!("{{ {u}, {keyword}: {branches} }}"),
                        format!("{{ {u}, required: [u], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                        format!(
                            "{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                        ),
                        format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                        format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                        format!("{{ required: [u], {keyword}: {branches} }}"),
                    ]
                    .into_iter()
                    .map(|site| (site, admits)),
                );
            }
        }
    }
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
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
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{site}, via {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #592: a union branch that states nothing (`true`, `{}`), met with a conjunct that admits
/// `null` (the nullable object `NB`) through a `$ref` sibling or an `allOf`, takes that `null`
/// once. `Value` is the identity of the meet, so the meet hands the branch the conjunct's `null`
/// after the union has already counted it: an `anyOf` keeps it on the union alone, so the
/// position is `Option` and no variant is, and a `oneOf` whose `null` another branch also matches
/// (`type: 'null'`) keeps it nowhere. A `oneOf` whose only branch `null` matches is that branch
/// keeps it in that variant (`a_ref_sibling_one_of_whose_branches_differ_only_in_nullability_collapses`).
/// A union the meet narrows to the branch alone, beside a `type: string` branch the object
/// excludes, keeps the `anyOf`'s `null` on the position.
#[test]
fn a_stated_nothing_branch_met_with_a_nullable_conjunct_takes_its_null_once() {
    let nb = "$ref: '#/components/schemas/NB'";
    let c = "{ properties: { c: { type: string } } }";
    let rows = [
        (
            format!("{{ {nb}, required: [a], anyOf: [ true, {c} ] }}"),
            true,
        ),
        (
            format!("{{ {nb}, required: [a], anyOf: [ {{}}, {c} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], anyOf: [ true, {c} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }}, {{ anyOf: [ true, {c} ] }} ] }}"),
            true,
        ),
        (format!("{{ {nb}, anyOf: [ true, {c} ] }}"), true),
        (
            format!("{{ {nb}, required: [a], oneOf: [ {{ type: 'null' }}, true, {c} ] }}"),
            false,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ {{ type: 'null' }}, true, {c} ] }}"),
            false,
        ),
        (
            format!("{{ {nb}, anyOf: [ {{}}, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], anyOf: [ true, {{ type: string }} ] }}"),
            true,
        ),
    ];
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
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
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
        let union = pick
            .strip_prefix("Option<")
            .and_then(|inner| inner.strip_suffix('>'))
            .unwrap_or(&pick);
        let nullable_variants: Vec<String> = enum_variants(&types, union)
            .into_iter()
            .filter(|variant| variant.contains("(Option<"))
            .collect();
        if !nullable_variants.is_empty() {
            mismatches.push(format!(
                "{site}: `null` is the position's alone, but variants accept it: \
                 {nullable_variants:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #597: a `oneOf` whose only branch `null` matches states nothing (`true`, `{}`), beside a
/// `type: string` branch the nullable object `NB` excludes, met with `NB` through a `$ref`
/// sibling or an `allOf`. The meet narrows the union to that branch alone, and `null` is valid
/// there, since `NB` admits it and exactly one branch matches it, so every spelling generates
/// `Option<Pick>`, as the inline spelling does. Where another branch matches `null` too
/// (`type: 'null'`, `type: [string, 'null']`) it matches two, and where the conjunct (`N`) denies
/// it nothing admits it, so those stay `Pick`.
#[test]
fn a_one_of_narrowed_to_its_stated_nothing_branch_keeps_the_conjuncts_null() {
    let nb = "$ref: '#/components/schemas/NB'";
    let rows = [
        (format!("{{ {nb}, oneOf: [ {{}}, {{ type: string }} ] }}"), true),
        (format!("{{ {nb}, oneOf: [ true, {{ type: string }} ] }}"), true),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ true, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ {{}}, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }}, {{ oneOf: [ true, {{ type: string }} ] }} ] }}"),
            true,
        ),
        (
            "{ type: [object, 'null'], properties: { a: { type: string } }, \
             oneOf: [ true, { type: string } ] }"
                .to_owned(),
            true,
        ),
        (
            format!("{{ {nb}, oneOf: [ {{ type: 'null' }}, true, {{ type: string }} ] }}"),
            false,
        ),
        (
            format!(
                "{{ allOf: [ {{ {nb} }} ], oneOf: [ {{ type: 'null' }}, {{}}, {{ type: string }} ] }}"
            ),
            false,
        ),
        (
            format!("{{ {nb}, oneOf: [ {{}}, {{ type: [string, 'null'] }} ] }}"),
            false,
        ),
        (
            "{ $ref: '#/components/schemas/N', oneOf: [ {}, { type: string } ] }".to_owned(),
            false,
        ),
        (
            "{ allOf: [ { $ref: '#/components/schemas/N' } ], oneOf: [ true, { type: string } ] }"
                .to_owned(),
            false,
        ),
    ];
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
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
    N:
      type: object
      properties:
        a: {{ type: string }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{site}, via {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #588's other spellings of a branch that states nothing, met with an untyped object
/// composition: a union of that branch alone (`[ true ]`, `[ {} ]`, `[ $ref ]` to a `true` or `{}`
/// component), and of that branch beside a typed branch that denies `null` (`type: string`,
/// `type: integer`). Nothing here decides `null`, so every spelling keeps the non-null struct, as
/// the `$ref`-sibling spelling over `U` already did. The inline-sibling spellings (`properties`,
/// `items`, `required`) met the branch's `Value` with the sibling's object half, took that half's
/// `null` as the branch's own, and generated `Option<Pick>`; so did the inline `allOf`-member
/// spellings of a union of that branch alone, which collapses to the branch. A `false` branch
/// beside an untyped one denies `null` and so decides nothing either (#581), whether written
/// inline or as a `$ref` to a `false` component.
#[test]
fn a_union_of_a_stated_nothing_branch_alone_or_beside_a_non_null_type_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let t = "{ $ref: '#/components/schemas/T' }";
    let e = "{ $ref: '#/components/schemas/E' }";
    let mut rows: Vec<String> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for branches in [
            "[ true ]".to_owned(),
            "[ {} ]".to_owned(),
            format!("[ {t} ]"),
            format!("[ {e} ]"),
            "[ { type: string }, true ]".to_owned(),
            "[ { type: string }, {} ]".to_owned(),
            "[ { type: integer }, true ]".to_owned(),
            format!("[ {t}, {{ properties: {{ b: {{ type: string }} }} }} ]"),
            format!("[ {e}, {{ properties: {{ b: {{ type: string }} }} }} ]"),
            "[ { $ref: '#/components/schemas/R' } ]".to_owned(),
            "[ { $ref: '#/components/schemas/V' }, { type: string } ]".to_owned(),
            "[ false, { properties: { b: { type: string } } } ]".to_owned(),
            "[ { $ref: '#/components/schemas/F' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
            "[ { $ref: '#/components/schemas/G' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
            "[ { not: {} } ]".to_owned(),
            "[ { $ref: '#/components/schemas/X' } ]".to_owned(),
            "[ { not: {} }, { properties: { b: { type: string } } } ]".to_owned(),
            "[ { $ref: '#/components/schemas/X' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
        ] {
            // The `allOf` spellings over `U` of a `$ref` branch to the `true` or `{}` component
            // generated `Option<Pick>` until a `$ref` branch was read through its target (#594),
            // as did those of a `$ref` to an alias `R` of `T`, of a `$ref` to the untyped object
            // `V` beside a branch that denies `null` (the inline `V` is undecided, #581), of a
            // `$ref` to the `false` component `F` (or its alias `G`) beside an untyped branch,
            // which denies `null` as the inline `false` does, and of a `$ref` to the `{ not: {} }`
            // component `X`, which states no type and so decides nothing, as inline. A union of a
            // `false` branch alone is uninhabited, so it has no `null` to decide:
            // `a_union_of_a_false_branch_alone_is_uninhabited_in_every_spelling_of_its_conjunction`
            // pins its spellings (#615).
            rows.extend([
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ {u}, {keyword}: {branches} }}"),
                format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                format!("{{ required: [u], {keyword}: {branches} }}"),
            ]);
        }
    }
    let mut mismatches = Vec::new();
    for site in rows {
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
    T: true
    E: {{}}
    R: {{ $ref: '#/components/schemas/T' }}
    F: false
    G: {{ $ref: '#/components/schemas/F' }}
    X: {{ not: {{}} }}
    V: {{ properties: {{ v: {{ type: string }} }} }}
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{site}, via {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") {
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is undecided here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A union whose only branch is `false` admits no value, and so does every conjunction it is part
/// of, however the conjunction is spelled: beside or inside an `allOf` over an object, beside a
/// `$ref` to one (alone, with untyped object keywords, or with `type: object`), beside the same
/// object written inline, beside
/// `type: object`, or alone. Each spelling generates the uninhabited `Pick` with no `E013` or
/// `E007` (#615). The `allOf`-over-`U` and `$ref`-to-`U` spellings were
/// `E013` and the `type: object` ones `E007`, while the inline
/// object spellings and the bare union generated the uninhabited type. The `false` branch is also
/// written as a `$ref` to the `false` component `F` and to its alias `G`, and beside a `null`
/// member under a `type` that denies `null`, which leaves the same empty set.
#[test]
fn a_union_of_a_false_branch_alone_is_uninhabited_in_every_spelling_of_its_conjunction() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<String> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for branches in [
            "[ false ]",
            "[ { $ref: '#/components/schemas/F' } ]",
            "[ { $ref: '#/components/schemas/G' } ]",
        ] {
            rows.extend([
                // The `$ref` arm alone, and beside the untyped object keywords that scope a
                // refiner over the union (`meet_ref_union_sibling`).
                format!("{{ {u}, {keyword}: {branches} }}"),
                format!("{{ {u}, required: [u], {keyword}: {branches} }}"),
                format!(
                    "{{ {u}, properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                ),
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                format!("{{ required: [u], {keyword}: {branches} }}"),
                format!("{{ {keyword}: {branches} }}"),
                format!("{{ type: object, {keyword}: {branches} }}"),
                format!("{{ type: object, {u}, {keyword}: {branches} }}"),
                format!("{{ type: object, allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
            ]);
        }
    }
    rows.push("{ type: object, oneOf: [ false, { type: 'null' } ] }".to_owned());
    rows.push("{ type: object, anyOf: [ false, { type: 'null' } ] }".to_owned());
    let mut mismatches = Vec::new();
    for site in rows {
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
    F: false
    G: {{ $ref: '#/components/schemas/F' }}
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (generated, code) = generate_with_code(&spec);
        let checked = check(&spec);
        let mut verdicts = Vec::new();
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            if report.outcome() == Outcome::Rejected
                || has_code(report, Code::AllOfIrreconcilable)
                || has_code(report, Code::NonDisjointUnion)
            {
                verdicts.push(format!("via {entry}: {:?}", codes(report)));
            }
        }
        if !verdicts.is_empty() {
            mismatches.push(format!("{site}: {verdicts:?}"));
            continue;
        }
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick");
        if pick.as_deref() != Some("Pick") || !types.contains("pub enum Pick {}") {
            mismatches.push(format!(
                "{site}: `pick` is {pick:?}, not the uninhabited `Pick`: {types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A union of a `$ref` to the `true` component `T` beside an untyped object branch, met with the
/// untyped sibling keyword `items` or `required` alone, generates `Pick`: neither the `T` branch
/// nor the untyped one decides `null`, and the sibling keyword never adds it. `spargen-v0.5.0`
/// generated `Option<Pick>` for every row here; #593 changed them, a released change of a
/// generated field type, so this fixture pins the exact type rather than only "not optional" (#595).
#[test]
fn a_ref_to_a_true_component_beside_an_untyped_object_branch_under_items_or_required_is_not_optional(
) {
    let mut mismatches = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for sibling in ["items: { type: string }", "required: [u]"] {
            let site = format!(
                "{{ {sibling}, {keyword}: [ {{ $ref: '#/components/schemas/T' }}, \
                 {{ properties: {{ b: {{ type: string }} }} }} ] }}"
            );
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
    T: true
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
            );
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{site}, via {entry}: {report:#?}"
                );
            }
            let (_, code) = generate_with_code(&spec);
            let types = types_module(&code);
            let pick = field_type(&types, "pub pick")
                .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
            if pick != "Pick" {
                mismatches.push(format!("{site}: `pick` is `{pick}`, expected `Pick`"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `oneOf` one of whose members is the union itself.
///
/// The member is a cycle-closing back-edge, so its `Ty` points at the union's own reservation. The
/// emitted `Deserialize` therefore opens with `serde_json::from_value::<Box<Loop>>(value.clone())`
/// — **the same impl, on the same value, with no base case** — so every decode recurses until the
/// stack is exhausted. It compiles, and the `e2e` gate compiles generated output rather than
/// decoding through every type, so nothing could have caught it.
///
/// A variant that *is* the whole union constrains nothing and cannot be decoded, so the right answer
/// is a rejection. Both spellings and the root document are pinned; as with the sibling case above,
/// the root form reproduces byte-identically on `2aa5ada`.
#[test]
fn a_union_variant_that_is_the_union_itself_is_rejected() {
    const LIB: &str = r##"
components:
  schemas:
    Loop:
      oneOf:
        - { $ref: 'PREFIX#/components/schemas/Loop' }
        - { type: string }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Loop",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a variant that is the whole union decodes by re-entering its \
                 own `Deserialize` on the same value: {report:#?}"
            );
            assert!(
                has_code(report, Code::NonDisjointUnion),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        // The runtime shape itself: nothing may emit a `Deserialize` arm that calls back into the
        // same type on the same value. That is what makes this a hang rather than a wrong type.
        assert!(
            !code.contains("from_value::<Box<Loop>>"),
            "{spelling}: the emitted decoder re-enters itself with no base case: {code}"
        );
    }

    let root = format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Loop' }} }}
{}"##,
        LIB.replace("PREFIX", "")
    );
    let report = generate(&root);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

/// A union that collapses to its sole non-null member, whose intersection with the enclosing
/// schema's own sibling keywords is irreconcilable (`type: object` against `type: string`), leaves
/// the union with no variant. Before the rejection existed, the collapse `?`-propagated the failed
/// intersection with no diagnostic, so the run came back clean with the response body silently
/// dropped — the fourth behaviour. It must be refused with `E007` at the schema that carries the
/// union, through both entry points.
#[test]
fn a_sole_union_member_irreconcilable_with_its_siblings_is_rejected() {
    let spec = r##"
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
            application/json:
              schema:
                type: object
                properties:
                  a: { type: string }
                oneOf:
                  - { type: string }
                  - { type: "null" }
"##;
    let pointer = "/paths/~1u/get/responses/200/content/application~1json/schema";
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e007: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::NonDisjointUnion)
            .collect();
        assert!(
            e007.iter().any(
                |d| d.pointer.as_str() == pointer && d.message.contains("sole non-null member")
            ),
            "{entry}: E007 for the empty sole-member intersection must point at `{pointer}`, \
             not at {:?}\n{report:#?}",
            e007.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn overlapping_numeric_one_of_generates_with_typed_trial_matching() {
    // `integer | number` overlaps on integral payloads. The generated typed trial union enforces
    // exact-one matching at runtime (`1` is ambiguous; `1.5` selects number).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: integer
        - type: number
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");

    // The typed union the name promises: both branches survive, each with its own numeric type.
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant0").as_deref(),
        Some("i64"),
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant1").as_deref(),
        Some("f64"),
        "{types}"
    );
}

#[test]
fn overlapping_object_one_of_generates_with_typed_trial_matching() {
    // Object variants that overlap structurally use typed trial matching and exact-one semantics.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: object
          required: [kind]
          properties: { kind: { type: string }, a: { type: string } }
        - type: object
          required: [kind]
          properties: { kind: { type: string }, b: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // Both object branches survive as typed structs, each keeping its own distinguishing field.
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "Uvariant0"),
        ["kind", "a"],
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "Uvariant1"),
        ["kind", "b"],
        "{types}"
    );
}

#[test]
fn string_integer_union_generates() {
    // `string | integer` occupy distinct JSON type categories (string vs number) → provably disjoint
    // → GENERATES (this replaced the old, incorrect E007 fixture, which asserted rejection here).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn union_sibling_constraints_intersect_every_branch() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    StringOnly:
      type: string
      oneOf:
        - type: string
        - type: integer
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");

    // The sibling `type: string` intersects each branch: the integer branch admits nothing a
    // string does, so what remains is the string branch alone, not a two-variant union.
    let types = types_module(&code);
    assert_eq!(
        alias_target(&types, "StringOnly").as_deref(),
        Some("String"),
        "{types}"
    );
    assert!(!types.contains("pub enum StringOnly "), "{types}");
}

#[test]
fn disjoint_string_array_union_generates() {
    // `string | string[]` occupy distinct JSON type categories (string vs array) → provably disjoint
    // (ollama's dominant shape). Generates without E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn required_key_disjoint_objects_generate() {
    // Two CLOSED object variants (`additionalProperties: false`) each with a unique required key
    // (`a` / `b`) → provably disjoint by key presence → GENERATES with a content-inspecting custom
    // Deserialize. Closed is required for this fast path; open variants use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      additionalProperties: false
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      additionalProperties: false
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn open_object_union_generates_with_typed_trial_matching() {
    // Open objects cannot use the required-key fast path, so they use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_combined_one_of_and_any_of_applicators_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
      anyOf:
        - type: string
        - type: boolean
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));

    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion));
}

#[test]
fn nullable_variant_hoists_to_option() {
    // A variant that is itself nullable (`{type: [string, "null"]}`) has its nullability HOISTED to
    // the union: the union becomes `Option<Enum>` and the string/array variants stay disjoint. A
    // `null` payload resolves at the outer `Option`, so the custom Deserialize only sees non-null.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
    Holder:
      type: object
      required: [u]
      properties:
        u: { $ref: "#/components/schemas/U" }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");

    // The hoist itself: a required use of the union is `Option<U>` (the nullability the variant
    // carried), and the string variant's own type is the non-null `String`.
    let types = types_module(&code);
    assert_eq!(
        field_type(&types, "pub u:").as_deref(),
        Some("Option<U>"),
        "{types}"
    );
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant0").as_deref(),
        Some("String"),
        "{types}"
    );
}

#[test]
fn nullable_union_collapses_to_option() {
    // A 2-member union where one member is `{type: "null"}` strips the null and collapses to
    // `Option<String>` — no enum, no E007. Generates.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: "null"
    Holder:
      type: object
      required: [u]
      properties:
        u: { $ref: "#/components/schemas/U" }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");

    // The collapse: no enum, `U` names the string itself, and the stripped null reappears as the
    // `Option` a required use of `U` carries.
    let types = types_module(&code);
    assert!(!types.contains("pub enum U "), "{types}");
    assert_eq!(
        alias_target(&types, "U").as_deref(),
        Some("String"),
        "{types}"
    );
    assert_eq!(
        field_type(&types, "pub u:").as_deref(),
        Some("Option<U>"),
        "{types}"
    );
}

#[test]
fn null_mixed_scalar_enum_generates() {
    // A `null` member is stripped; the remaining homogeneous string scalars lower as a nullable
    // enum (`Option<Enum>`). No E008, and generation succeeds. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Severity:
      type: [string, "null"]
      enum: [low, medium, high, null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::NonScalarEnum), "{checked:#?}");
}

#[test]
fn all_null_enum_generates_as_exact_null() {
    // A value set of only `null` has no scalar remainder: it lowers to exact JSON null (`()`) rather
    // than an unconstrained value or E008.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nothing:
      enum: [null]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");

    let types = types_module(&code);
    assert_eq!(
        alias_target(&types, "Nothing").as_deref(),
        Some("()"),
        "{types}"
    );
}

/// A union whose only non-null member has no typed intersection with the enclosing schema's own
/// sibling constraints has no representable variant left. The multi-variant path already rejects
/// that with `E007` once every variant is excluded, so the one-member collapse reports the same
/// terminal code — otherwise the code would depend on how many variants the author happened to
/// write. It used to be dropped silently.
#[test]
fn e007_fires_when_a_single_real_member_union_contradicts_its_sibling() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Collapsed:
      type: integer
      oneOf:
        - { type: string }
        - { type: 'null' }
"##;
    let generated = generate(spec);
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(
        has_code(&generated, Code::NonDisjointUnion),
        "{generated:#?}"
    );
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion), "{checked:#?}");
}

/// An untyped object or array applicator beside `oneOf`/`anyOf` lowered to `TypeKind::Any`, which
/// intersects as identity, so `required`, `additionalProperties`, `items` and `prefixItems` alone
/// vanished from every branch with no diagnostic (#282). In JSON Schema 2020-12 such a keyword is
/// vacuously satisfied by an instance of another category, so it refines exactly the branches of
/// its own category and leaves the rest as they are; where no branch has its category it reaches
/// nothing the union accepts, which is the contradiction the `$ref` spelling already rejects.
#[test]
fn an_untyped_union_sibling_refines_only_the_branches_of_its_category() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n";
    const AB: &str = "    A: { type: object, properties: { a: { type: string } } }\n    B: { \
                      type: object, properties: { b: { type: string } } }\n";
    // Every variant of `pub enum {name}`, each as the type it wraps.
    let variants_of = |types: &str, name: &str| -> Vec<String> {
        enum_variants(types, name)
            .iter()
            .filter_map(|variant| {
                let (_, inner) = variant.split_once('(')?;
                let inner = inner.trim_end_matches(')');
                let inner = inner
                    .strip_prefix("Box<")
                    .and_then(|boxed| boxed.strip_suffix('>'))
                    .unwrap_or(inner);
                Some(inner.to_owned())
            })
            .collect()
    };
    let variant_types = |types: &str| variants_of(types, "U");
    // The declaration of field `a` inside `pub struct {name}`, or empty where it has none.
    let field_a = |types: &str, name: &str| -> String {
        types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with(&format!("pub struct {name} ")))
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .find(|line| line.starts_with("pub a: "))
            .unwrap_or_default()
            .to_owned()
    };
    let generates = |spec: &str| -> String {
        for report in [check(spec), generate(spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{spec}\n{report:#?}");
            for code in [Code::DeclarationHasNoEffect, Code::AllOfIrreconcilable] {
                assert!(!has_code(&report, code), "{spec}\n{report:#?}");
            }
        }
        types_module(&generate_with_code(spec).1)
    };

    // The issue's reproduction: both branches are objects, so both now require `a`. `A` declares
    // it and keeps its type; `B` does not, so `a` joins it as a required unconstrained field.
    let spec = format!(
        "{HEAD}{AB}    U:\n      oneOf: [{{ $ref: '#/components/schemas/A' }}, {{ $ref: \
         '#/components/schemas/B' }}]\n      required: [a]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        assert!(
            declared_fields(&types, variant).contains(&"a".to_owned()),
            "`{variant}` does not carry `a`:\n{types}"
        );
        let a = field_a(&types, variant);
        assert!(
            !a.is_empty() && !a.contains("Option<"),
            "`required: [a]` beside the union left `a` optional in `{variant}`: {a}\n{types}"
        );
    }

    // A mixed-category union: each refiner reaches its own category's branch and leaves the
    // string branch alone, so no branch is dropped and nothing warns.
    for (keyword, other, sibling, requires_a) in [
        (
            "required",
            "{ type: object, properties: { a: { type: string } } }",
            "required: [a]",
            true,
        ),
        (
            "additionalProperties",
            "{ type: object, properties: { a: { type: string } } }",
            "additionalProperties: false",
            false,
        ),
        (
            "items",
            "{ type: array, items: { type: number } }",
            "items: { type: integer }",
            false,
        ),
        (
            "prefixItems",
            "{ type: array }",
            "prefixItems: [{ type: integer }]",
            false,
        ),
    ] {
        let spec =
            format!("{HEAD}    U:\n      oneOf: [{{ type: string }}, {other}]\n      {sibling}\n");
        let types = generates(&spec);
        let variants = variant_types(&types);
        assert_eq!(
            variants.len(),
            2,
            "`{keyword}` beside a mixed union dropped a branch:\n{types}"
        );
        assert!(
            types.contains(&format!("pub type {} = String;", variants[0])),
            "`{keyword}` beside a mixed union touched the string branch:\n{types}"
        );
        if requires_a {
            let line = field_a(&types, &variants[1]);
            assert!(
                !line.is_empty() && !line.contains("Option<"),
                "`{keyword}` did not reach the object branch: {line}\n{types}"
            );
        }
    }
    // The refiners change the branches they reach: compared with the same union bare.
    for (sibling, branch) in [
        (
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        ("prefixItems: [{ type: integer }]", "{ type: array }"),
    ] {
        let bare = generates(&format!(
            "{HEAD}    U:\n      oneOf: [{{ type: string }}, {branch}]\n"
        ));
        let refined = generates(&format!(
            "{HEAD}    U:\n      oneOf: [{{ type: string }}, {branch}]\n      {sibling}\n"
        ));
        assert_ne!(
            bare, refined,
            "`{sibling}` changed nothing beside the union"
        );
    }

    // Object and array refiners with no `type` beside a union are not a contradiction there: each
    // set refines its own category's branches.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ type: string }}, {{ type: object, properties: {{ a: {{ \
         type: string }} }} }}, {{ type: array, items: {{ type: number }} }}]\n      required: \
         [a]\n      items: {{ type: integer }}\n"
    );
    let types = generates(&spec);
    assert_eq!(variant_types(&types).len(), 3, "{types}");

    // A nested union branch is refined branch by branch too, so its string branch stays.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ oneOf: [{{ type: string }}, {{ type: object, properties: \
         {{ a: {{ type: string }} }} }}] }}, {{ type: integer }}]\n      required: [a]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    let nested = variants_of(&types, &variants[0]);
    assert_eq!(nested.len(), 2, "the nested union lost a branch:\n{types}");
    assert!(
        nested
            .iter()
            .any(|inner| types.contains(&format!("pub type {inner} = String;"))),
        "the nested union lost its string branch:\n{types}"
    );

    // The sole non-null member is refined the same way, and keeps the union's `null`.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ type: object, properties: {{ a: {{ type: string }} }} }}, \
         {{ type: 'null' }}]\n      required: [a]\n    Holder:\n      type: object\n      \
         required: [u]\n      properties:\n        u: {{ $ref: '#/components/schemas/U' }}\n"
    );
    let types = generates(&spec);
    assert!(types.contains("pub u: Option<U>"), "{types}");
    let a = field_a(&types, "U");
    assert!(
        !a.is_empty() && !a.contains("Option<"),
        "`required: [a]` did not reach the sole member: {a}\n{types}"
    );

    // A branch that states no category takes the one the keywords establish, as an untyped `$ref`
    // target does. This is the shape of GitHub's `secret-scanning-custom-pattern-to-update`:
    // `properties` beside an `anyOf` of `required`-only branches is an object in every branch, so
    // every branch requires `v`. (What each branch's own `required` contributes is decided where
    // the branch is lowered, not here.)
    let spec = format!(
        "{HEAD}    U:\n      required: [v]\n      properties:\n        v: {{ type: integer \
         }}\n        x: {{ type: string }}\n        y: {{ type: string }}\n      anyOf: [{{ \
         required: [x] }}, {{ required: [y] }}]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        let line = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with(&format!("pub struct {variant} ")))
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .find(|line| line.starts_with("pub v: "))
            .unwrap_or_default()
            .to_owned();
        assert!(
            !line.is_empty() && !line.contains("Option<"),
            "branch `{variant}` is not the object that requires `v`: {line}\n{types}"
        );
    }

    // A refiner that reaches no branch of its category constrains nothing the union accepts: the
    // union generates exactly as it is, and the keyword is acknowledged with `W011` rather than
    // dropped in silence, through both entry points.
    for (case, members, sibling) in [
        (
            "no object branch",
            "[{ type: string }, { type: integer }]",
            "required: [a]",
        ),
        (
            "no array branch",
            "[{ type: string }, { type: object }]",
            "items: { type: integer }",
        ),
        (
            "sole member of another category",
            "[{ type: string }, { type: 'null' }]",
            "required: [a]",
        ),
    ] {
        let spec = format!("{HEAD}    U:\n      oneOf: {members}\n      {sibling}\n");
        for report in [check(&spec), generate(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{case}: {report:#?}"
            );
            let messages = messages_for(&report, Code::DeclarationHasNoEffect);
            assert_eq!(messages.len(), 1, "{case}: {report:#?}");
            assert!(
                messages[0].contains("no branch of its union has that category"),
                "{case}: {messages:?}"
            );
        }
        // The union and the types its branches wrap are the bare union's. (The lowered keyword
        // itself is still emitted as an unused type of its own, as every lowered sibling is.)
        let shape = |spec: &str| {
            let types = types_module(&generate_with_code(spec).1);
            let mut shape = enum_variants(&types, "U");
            if shape.is_empty() {
                shape.extend(
                    types
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| line.starts_with("pub type U "))
                        .map(str::to_owned),
                );
            }
            for inner in variants_of(&types, "U") {
                shape.extend(
                    types
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| line.starts_with(&format!("pub type {inner} ")))
                        .map(str::to_owned),
                );
            }
            shape
        };
        let bare = format!("{HEAD}    U:\n      oneOf: {members}\n");
        let refined = shape(&spec);
        assert!(!refined.is_empty(), "{case}: no shape for `U`");
        assert_eq!(
            refined,
            shape(&bare),
            "{case}: a keyword that reaches no branch changed the union"
        );
    }

    // Where the sibling carries both halves and neither reaches a branch of its category, each
    // half is acknowledged with a `W011` of its own: reporting only the object half dropped the
    // array keywords in silence. All three sites that meet a scoped sibling are covered: a
    // multi-variant union, a sole non-null member, and a `$ref` to a union.
    for (case, schema, needle) in [
        (
            "multi-variant union",
            "    U:\n      oneOf: [{ type: string }, { type: integer }]\n      required: [a]\n      \
             items: { type: integer }\n",
            "no branch of its union has that category",
        ),
        (
            "sole non-null member",
            "    U:\n      oneOf: [{ type: string }, { type: 'null' }]\n      required: [a]\n      \
             items: { type: integer }\n",
            "no branch of its union has that category",
        ),
        (
            "`$ref` to a union",
            "    Target: { oneOf: [{ type: string }, { type: integer }] }\n    U:\n      $ref: \
             '#/components/schemas/Target'\n      required: [a]\n      items: { type: integer }\n",
            "no branch of its target union has that category",
        ),
    ] {
        let spec = format!("{HEAD}{schema}");
        for report in [check(&spec), generate(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{case}: {report:#?}"
            );
            let messages = messages_for(&report, Code::DeclarationHasNoEffect);
            assert_eq!(
                messages.len(),
                2,
                "{case}: one W011 per unreached half: {report:#?}"
            );
            assert!(
                messages.iter().all(|message| message.contains(needle)),
                "{case}: {messages:?}"
            );
            for keywords in ["`required`", "`items`"] {
                assert_eq!(
                    messages
                        .iter()
                        .filter(|message| message.contains(keywords))
                        .count(),
                    1,
                    "{case}: exactly one W011 names {keywords}: {messages:?}"
                );
            }
        }
    }

    // A branch that states no category can be given none when object and array keywords come
    // together, or when a multi-type `type` array beside them admits another category too: that is
    // rejected through both entry points rather than generated without them.
    for (case, schema) in [
        (
            "both kinds",
            "      oneOf: [{}, { type: string }]\n      required: [a]\n      items: { type: \
             integer }",
        ),
        (
            "a type array admitting another category",
            "      type: [object, string]\n      oneOf: [{}, { type: string }]\n      required: \
             [a]",
        ),
    ] {
        let spec = format!("{HEAD}    U:\n{schema}\n");
        for report in [check(&spec), generate(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            let messages = messages_for(&report, Code::AllOfIrreconcilable);
            assert_eq!(messages.len(), 1, "{case}: {report:#?}");
            assert!(
                messages[0].contains("union member 0 states no JSON category"),
                "{case}: {messages:?}"
            );
        }
    }

    // A multi-type `type` array deleted for lowering still excludes the branches of the categories
    // it omits, and its object keywords still reach the object branch.
    let spec = format!(
        "{HEAD}    U:\n      type: [object, integer]\n      oneOf: [{{ type: string }}, {{ type: \
         integer }}, {{ type: object, properties: {{ a: {{ type: string }} }} }}]\n      required: \
         [a]\n"
    );
    for report in [check(&spec), generate(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::DeclarationHasNoEffect);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(messages[0].contains("union member 0"), "{messages:?}");
    }
    let types = types_module(&generate_with_code(&spec).1);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    let a = field_a(&types, &variants[1]);
    assert!(
        !a.is_empty() && !a.contains("Option<"),
        "`required: [a]` beside a type array did not reach the object branch: {a}\n{types}"
    );

    // A typed sibling still speaks for every branch: `type: object` excludes the string one.
    let spec = format!(
        "{HEAD}    U:\n      type: object\n      oneOf: [{{ type: string }}, {{ type: object, \
         properties: {{ a: {{ type: string }} }} }}]\n      required: [a]\n"
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

/// Site B widened `E007`'s emission set, so its message has to describe the construct that actually
/// reached it. A right code under a wrong message is still a wrong diagnostic: reusing the
/// multi-variant path's wording verbatim would say "every variant impossible" of a union with one
/// variant, and would inherit the same unsatisfiability claim `intersect_types` cannot support.
/// This pins the sole-member wording, the empty-or-unrepresentable hedge, and that the message does
/// not name some other cause entirely.
#[test]
fn the_single_member_union_rejection_names_its_own_cause() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Collapsed:
      type: integer
      oneOf:
        - { type: string }
        - { type: 'null' }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::NonDisjointUnion);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("sole non-null member"),
        "{:?}",
        messages[0]
    );
    assert!(
        messages[0].contains("empty or unrepresentable"),
        "{:?}",
        messages[0]
    );
    // The causes `E007`'s explain already named must not be borrowed for this one.
    assert!(!messages[0].contains("discriminator"), "{:?}", messages[0]);
    assert!(!messages[0].contains("`anyOf`"), "{:?}", messages[0]);

    // The pointer, which both new `E013` sites pin and this one did not. It is not decoration:
    // `compat::carve_rules` maps it to the smallest omittable construct, so a diagnostic carrying
    // the document root instead of the offending node yields no rule, and a carvable rejection
    // becomes an un-carvable residual that ends the whole run `Rejected`. `carve.rs` proves that
    // consequence for this site end to end.
    let pointers: Vec<&str> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::NonDisjointUnion)
        .map(|d| d.pointer.as_str())
        .collect();
    assert_eq!(
        pointers,
        vec!["/components/schemas/Collapsed"],
        "the sole-member collapse must report at the offending component, not the document root: \
         {report:#?}"
    );

    // check/generate parity for the same site, which the `E013` sites also assert.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion), "{checked:#?}");

    // And the abstinence Site B spends five lines of comment justifying: the multi-variant path
    // emits a per-variant `W011` before its `E007` and this one deliberately does not, because
    // `W011` means "declared construct has no effect" and describes something dropped from output
    // that still exists — when the SOLE member is excluded no enum is generated at all, so there is
    // no surviving construct for the warning to describe. Filtering to `NonDisjointUnion` above
    // cannot see a second code, so adding exactly that `W011` survived the whole suite. One cause,
    // one diagnostic.
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "the sole-member collapse emitted `W011` beside its `E007`: two diagnostics for one cause, \
         and the `W011` would assert a generated enum that does not exist: {report:#?}"
    );
    assert_eq!(
        report.diagnostics().len(),
        1,
        "the sole-member collapse must report exactly its own cause: {report:#?}"
    );
}

/// `E007`'s published explain is what `spargen explain E007` prints, and Site B added a cause it did
/// not describe. The `oneOf`-plus-`anyOf` and `defaultMapping` causes must survive, and the new one
/// must be there beside them — the same standard this change applied to `E013`'s text.
#[test]
fn the_union_explain_covers_every_cause_that_reports_it() {
    let explain = Code::NonDisjointUnion.explain();
    assert!(explain.contains("`oneOf` and `anyOf`"), "{explain}");
    assert!(explain.contains("defaultMapping"), "{explain}");
    assert!(explain.contains("no branch at all"), "{explain}");
    assert!(explain.contains("single non-null member"), "{explain}");

    // The `W011` clause had no reader while its three neighbours did, and it is the clause Site B's
    // abstinence rests on: a branch the adjacent constraints exclude is dropped with `W011` *while
    // the rest of the enum stands*, which is exactly what does not happen when the excluded branch
    // is the only one. `the_single_member_union_rejection_names_its_own_cause` asserts the
    // behaviour; without this the published reason for it could be deleted silently.
    assert!(
        explain.contains("`W011`"),
        "the explain must say what happens to a branch the adjacent constraints exclude: {explain}"
    );
    assert!(
        explain.contains("while the rest of the enum stands"),
        "the explain must keep the clause that distinguishes an excluded branch from an excluded \
         sole member, which is why one warns and the other does not: {explain}"
    );

    // The same anti-append guard as `E013`'s: presence assertions cannot see a contradiction added
    // after them, and the remedy is the last thing the explain says.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E007`'s explain must end with its remedy: {explain}"
    );
}

/// A union whose own null acceptance came from a stripped `{type: "null"}` member or a `"null"` in
/// the enclosing `type` array is still satisfied by `null`, and must not be rejected as having no
/// variant. `lower_union` folds that acceptance into a local *before* it intersects, so the member
/// it hands `intersect_types` carries `nullable: false` and `type_accepts_null` — which reads
/// `Ty::nullable` — cannot see it. Both collapse paths had the blind spot: the sole-real-member
/// site, and the pre-existing every-variant-excluded site below it.
///
/// The `$ref` spelling of the identical instance set already emits `()`
/// (`the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate` pins it), so two
/// spellings of one schema disagreed. An independent Draft 2020-12 validator says `null` is the one
/// value each of the rescued documents admits, and that each control admits nothing at all.
#[test]
fn a_union_that_null_still_satisfies_is_not_rejected_as_having_no_variant() {
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

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // Sole real member: `null` satisfies the enclosing `type` array AND the null-only member,
        // so `null` is the only value satisfying the whole schema.
        (
            "a sole-member `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        (
            "a sole-member `anyOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                anyOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // Every real variant excluded: the same blind spot on the multi-variant path.
        (
            "an every-variant-excluded `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        // The controls that must keep rejecting: the union admits null but the sibling refuses it,
        // so nothing satisfies both and the rejection is right.
        (
            "a sole-member union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an every-variant-excluded union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            false,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            if *null_satisfies {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` is satisfied by `null`, so {entry} must not reject it: {report:#?}"
                );
                assert!(
                    !has_code(&report, Code::NonDisjointUnion),
                    "`{what}` reported E007 through {entry}: {report:#?}"
                );
            } else {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` admits no value at all, so {entry} must still reject it: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::NonDisjointUnion),
                    "`{what}` did not report E007 through {entry}: {report:#?}"
                );
            }
        }
        if *null_satisfies {
            // The exact JSON null type, not a silently dropped body and not `Option<()>`: `null` is
            // the only satisfying value, so the response type has exactly one inhabitant. Followed
            // through the operation's own signature rather than by guessing an alias name — the
            // sole-member path lowers its member under the same hint, so the union's own def gets a
            // disambiguating suffix.
            let (_, code) = generate_with_code(&spec);
            let body = code
                .split("ResponseValue<types::")
                .nth(1)
                .and_then(|rest| rest.split('>').next())
                .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
                .trim()
                .to_owned();
            assert!(
                code.contains(&format!("pub type {body} = ();")),
                "`{what}` must lower its response body to the exact JSON null type, but \
                 `{body}` is not `()`: {code}"
            );
        }
    }

    // The other half of restoring the union's null acceptance ONTO the member: nullability now
    // flows THROUGH the intersection instead of around it, so the sibling gets a say in it. It
    // previously did not, and the union's acceptance was OR-ed back on afterwards regardless —
    // `{type: [string], oneOf: [{type: string}, {type: 'null'}]}` emitted `Option<String>` for a
    // schema the enclosing `type` array forbids `null` in. An independent Draft 2020-12 validator
    // rejects `null` for the first two rows and accepts it for the third.
    //
    // (what it exercises, the schema body, whether the generated response is optional)
    let nullability: &[(&str, &str, bool)] = &[
        (
            "a null member under a sibling `type` array that excludes null",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a nullable sole member under a sibling that excludes null",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        (
            "a null member under a sibling `type` array that admits null",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];
    for (what, body, optional) in nullability {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body: {code}",
            if *optional { "have" } else { "not have" }
        );
    }
}

/// A null-only union MEMBER and a `"null"` in the enclosing `type` array are not the same fact, and
/// only one of them can rescue an empty intersection.
///
/// A member supplies a branch that `null` validates against. A `"null"` in the enclosing `type`
/// array only *permits* null — `oneOf` still demands exactly one matching member and `anyOf` at
/// least one, so with no null member there is nothing for `null` to match and the schema admits
/// nothing at all. Folding both into one flag made those two cases indistinguishable: an
/// independent Draft 2020-12 validator says the first row below is satisfied by `null` and the
/// second by **nothing**, and they produced byte-identical output — the tool could no longer tell
/// "accepts only null" from "accepts nothing", and typed a body `()` that no real payload decodes
/// into, with `check` reporting clean.
#[test]
fn only_a_null_member_can_rescue_an_empty_union_intersection() {
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

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // A null MEMBER: `null` matches it, and the enclosing `type` array permits null, so `null`
        // satisfies the whole schema.
        (
            "a sole-member `oneOf` with a null member beside a nullable type array",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // The SAME enclosing type array with NO null member. Nothing satisfies this: `null` matches
        // no member, and anything matching the `string` member fails the `type` array.
        (
            "a sole-member `oneOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }]",
            false,
        ),
        (
            "an `anyOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                anyOf: [{ type: string }]",
            false,
        ),
        // The same distinction on the every-variant-excluded path.
        (
            "an every-variant-excluded union with a null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        (
            "an every-variant-excluded union whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }]",
            false,
        ),
    ];

    let mut emitted: Vec<(&str, String)> = Vec::new();
    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            if *null_satisfies {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` is satisfied by `null`, so {entry} must not reject it: {report:#?}"
                );
            } else {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` admits no value at all, so {entry} must reject it rather than type \
                     the body `()`: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::NonDisjointUnion),
                    "`{what}` did not report E007 through {entry}: {report:#?}"
                );
            }
        }
        let (_, code) = generate_with_code(&spec);
        emitted.push((what, code));
    }

    // What each generating row lowers TO, asserted positively.
    //
    // This replaces a differential that could not fail on the defect it named. It compared row 0
    // (two members, lowers to `()`) against a one-member probe (lowers to `i64`) and required them
    // to differ — but two documents of different member counts and different result types differ in
    // every reachable state, independent of the null fact, so the `assert_ne!` was true whatever
    // the production code did. Measured directly: under the re-merged-flag mutation the fixture
    // fails at the per-case outcome assertion above, and with those assertions neutralised the
    // fixture PASSED. It was held up entirely by its neighbours.
    //
    // Among the rows above, varying only the null fact flips a row between generating and
    // rejecting, so no pair of THEM is a differential. One that generates on both sides does
    // exist, and is asserted after the control below. Here the pin is positive: a row only `null`
    // satisfies must lower to the exact JSON null
    // type — not to `serde_json::Value`, not to `Option<T>` of anything — and that is a statement a
    // mutation can falsify without changing any outcome, which is what the removed assertion could
    // not manage.
    for (what, code) in &emitted {
        let module = types_module(code);
        if module.is_empty() {
            // A rejected row emits nothing; its verdict is asserted per case above.
            continue;
        }
        assert!(
            module.contains("= ();"),
            "`{what}` is satisfied by `null` and nothing else, so it must lower to the exact JSON \
             null type: {module}"
        );
    }
    assert!(
        !types_module(&emitted[0].1).is_empty(),
        "the null-only row generated nothing: {:?}",
        emitted[0].0
    );

    // The control the whole repair must not disturb: a non-empty intersection under the same
    // nullable type array is unaffected either way.
    let control = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }]"
        )
    );
    let (report, code) = generate_with_code(&control);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("= i64;"),
        "the non-empty intersection must still lower to its narrowed type: {code}"
    );

    // The differential that varies ONLY the null fact and generates on both sides: the control,
    // and the control with a `{type: 'null'}` member added. The member is what makes `null` a
    // valid body, so it — and only it — must make the response optional. The null fact lives on
    // the operation signature, not in the types module: `types_module` is nullability-blind here
    // (both documents emit the same `= i64;` alias), which is why this reads the signature.
    let with_null_member = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }, { type: 'null' }]"
        )
    );
    let (partner_report, partner) = generate_with_code(&with_null_member);
    assert_ne!(
        partner_report.outcome(),
        Outcome::Rejected,
        "{partner_report:#?}"
    );
    assert!(
        !code.contains("ResponseValue<Option<"),
        "without a null member `null` matches no branch, so the response is not optional: {code}"
    );
    assert!(partner.contains("= i64;"), "{partner}");
    assert!(
        partner.contains("ResponseValue<Option<"),
        "the null member is a branch `null` satisfies, so the response must be optional: {partner}"
    );
}

/// A schema must lower to the same nullability whether it is written inline or named as a
/// component. `ensure_component` computed `schema_is_nullable(schema)` *before* the body was
/// lowered and wrote it back over whatever the body computed — and `schema_is_nullable` is three
/// disjuncts over `types`, `enum_values` and `const_value` that never look at `oneOf`, `anyOf`,
/// `$ref` or `allOf`. So **every decision `lower_union` makes about null was discarded at a
/// `components.schemas` boundary**, which is the dominant spelling in real descriptions.
///
/// The comment there claimed nullability was "a pure function of the component's own schema — the
/// same inputs `lower_schema`/`lower_enum` use". That is true for a plain type/enum/const body and
/// false for every composed one.
///
/// Both oracles agree on each row, and each row is asserted in EVERY spelling, so none can drift
/// from the others again. There are four, because three functions reserve a type before lowering
/// its body and each once wrote the provisional answer back over the body's: `ensure_component`
/// (a root component), `ensure_resolved` (a sub-file component) and `ensure_remote` (a vendored
/// remote schema). A per-site fixture pins one row in one direction; this table pins every row
/// through every site, so reverting any one of the three to the provisional answer fails a row.
/// Each spelling is used by two operations, because the first use returns the freshly lowered `Ty`
/// and the second the cached one, and each path carried the overwrite separately.
#[test]
fn a_component_and_an_inline_schema_agree_about_null() {
    use sha2::{Digest, Sha256};

    const REMOTE_URL: &str = "https://api.example.com/schemas/body.yaml";
    const REMOTE_PATH: &str = "api.example.com/schemas/body.yaml";

    /// The root document: two operations whose `200` bodies are each `schema`, over `components`.
    fn root(schema: &str, components: &str) -> String {
        let schema = schema.replace('\n', "\n                ");
        let operation = |path: &str, id: &str| {
            format!(
                "  {path}:\n    get:\n      operationId: {id}\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                {schema}\n"
            )
        };
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n{}{}components:\n  schemas:\n{components}",
            operation("/u", "fetch"),
            operation("/v", "again"),
        )
    }
    fn component(body: &str) -> String {
        format!("    Body:\n      {}\n", body.replace('\n', "\n      "))
    }
    /// Write `files` (paths relative to the root document's directory) and generate.
    fn run(files: &[(&str, String)]) -> (Report, String) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        let out = dir.join("client.rs");
        let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        (report, code)
    }
    /// `body` generated in each spelling: `(spelling, report, emitted source)`.
    fn every_spelling(body: &str) -> Vec<(&'static str, Report, String)> {
        let ignore = "    Ignore: { type: string }\n";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{REMOTE_URL}\"\nsha256 = \"{:x}\"\npath = \"{REMOTE_PATH}\"\n",
            Sha256::digest(body.as_bytes())
        );
        let vendored = format!(".spargen/vendor/{REMOTE_PATH}");
        let spellings = [
            ("inline", vec![("openapi.yaml", root(body, ignore))]),
            (
                "root component",
                vec![(
                    "openapi.yaml",
                    root("$ref: '#/components/schemas/Body'", &component(body)),
                )],
            ),
            (
                "sub-file component",
                vec![
                    (
                        "openapi.yaml",
                        root("$ref: './lib.yaml#/components/schemas/Body'", ignore),
                    ),
                    (
                        "lib.yaml",
                        format!("components:\n  schemas:\n{}", component(body)),
                    ),
                ],
            ),
            (
                "vendored remote",
                vec![
                    (
                        "openapi.yaml",
                        root(&format!("$ref: '{REMOTE_URL}'"), ignore),
                    ),
                    ("spargen.lock", lock),
                    (&vendored, body.to_owned()),
                ],
            ),
        ];
        spellings
            .into_iter()
            .map(|(spelling, files)| {
                let (report, code) = run(&files);
                (spelling, report, code)
            })
            .collect()
    }

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // `schema_is_nullable` sees the `"null"` in the type array and says nullable. The union says
        // otherwise, and the union is right: with no null MEMBER there is no branch for `null` to
        // match, so `oneOf` fails.
        (
            "a union whose type array admits null but whose members supply no null branch",
            "type: [string, 'null']\noneOf: [{ type: string }]",
            false,
        ),
        // The mirror: the type array is silent about null, the members are not.
        (
            "a union whose null branch comes from a member, under no type array",
            "properties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a union under a wide nullable type array",
            "type: [object, array, 'null']\nproperties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling denies null through `enum`, which `schema_is_nullable` also reads — but it
        // reads the ENUM, not the intersection, so it must still agree.
        (
            "a union whose `enum` sibling excludes null",
            "enum: ['a', 'b']\noneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a union whose `enum` sibling includes null",
            "enum: ['a', null]\noneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // A plain body, where the reserve-time answer and the body's agree. This is the case the
        // old comment described, and it must not move.
        (
            "a plain nullable type array with no composition",
            "type: [string, 'null']",
            true,
        ),
        (
            "a plain non-nullable type",
            "type: string",
            false,
        ),
    ];

    // Every emitted operation return (both operations, on every client surface emitted), so a wrong
    // answer on either the lowered or the cached path shows.
    let returns = |code: &str| code.matches("support::ResponseValue<").count();
    let optional_returns = |code: &str| {
        code.matches("support::ResponseValue<Option<types::")
            .count()
    };

    for (what, body, null_satisfies) in cases {
        let mut seen = Vec::new();
        for (spelling, report, code) in every_spelling(body) {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` ({spelling}): {report:#?}"
            );
            assert!(
                returns(&code) >= 2,
                "`{what}` ({spelling}): both operations must be emitted: {code}"
            );
            let optional = optional_returns(&code);
            assert_eq!(
                optional,
                if *null_satisfies { returns(&code) } else { 0 },
                "`{what}` as {spelling} must {} an optional response body on both operations — \
                 `null` is {} under this schema: {code}",
                if *null_satisfies { "have" } else { "not have" },
                if *null_satisfies { "valid" } else { "invalid" }
            );
            seen.push((spelling, optional));
        }
        assert_eq!(seen.len(), 4, "`{what}`: every spelling must run: {seen:?}");
        assert!(
            seen.iter().all(|(_, optional)| *optional == seen[0].1),
            "`{what}` lowers to different nullability across spellings: {seen:?}"
        );
    }

    // A component whose union admits ONLY `null` must be the exact null type in every spelling too
    // — the component boundary previously wrapped it back into an `Option`.
    let only_null = "type: [integer, 'null']\noneOf: [{ type: string }, { type: 'null' }]";
    for (spelling, report, code) in every_spelling(only_null) {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{spelling}: {report:#?}"
        );
        assert_eq!(
            optional_returns(&code),
            0,
            "`{spelling}`: the intersection is `{{null}}`, so the type already has exactly one \
             inhabitant and must not be wrapped in `Option`: {code}"
        );
    }
}

/// #450: a nullable union of two or more non-null branches, refined by untyped object keywords
/// that every non-null branch contradicts, still admits `null`, which those keywords are
/// vacuously satisfied by. The scoped meet an untyped `allOf` member beside the union and an
/// untyped `$ref` sibling of the union both take rejected it with `E013`, where the inline sibling
/// spelling of the same keywords types it as the exact JSON null type. Every spelling now agrees.
/// A refiner that itself denies `null` (`type: object`) leaves no value at all, and still rejects.
#[test]
fn a_nullable_union_whose_every_branch_a_scoped_refiner_excludes_is_null() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n    Cat:\n      \
                        type: object\n      required: [kind]\n      properties: { kind: { type: \
                        string } }\n    Dog:\n      type: object\n      required: [kind, bark]\n      \
                        properties: { kind: { type: string }, bark: { type: boolean } }\n";
    const UNION: &str = "[{ $ref: '#/components/schemas/Cat' }, { $ref: \
                         '#/components/schemas/Dog' }, { type: 'null' }]";
    let head = HEAD;
    let holder =
        "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
                  p: { $ref: '#/components/schemas/Pet' }\n";
    let refined = "properties: { kind: { type: integer } }";

    let spellings = [
        (
            "an untyped `allOf` member",
            format!("    Pet:\n      allOf: [{{ {refined} }}]\n      oneOf: {UNION}\n"),
        ),
        (
            "untyped `$ref` siblings",
            format!(
                "    U:\n      oneOf: {UNION}\n    Pet:\n      $ref: '#/components/schemas/U'\n      \
                 {refined}\n"
            ),
        ),
        (
            "inline siblings (the control)",
            format!("    Pet:\n      {refined}\n      oneOf: {UNION}\n"),
        ),
        // The scoped meet also reaches a nullable union nested as a branch of the target union
        // (`meet_refiner`'s union arm). Its every non-null branch is excluded, so it is `null`,
        // and the object branch beside it is excluded too: `null` is the only value left.
        (
            "an untyped `allOf` member beside a nested nullable union",
            format!(
                "    Pet:\n      allOf: [{{ {refined} }}]\n      anyOf:\n        - oneOf: \
                 {UNION}\n        - {{ type: object, required: [kind], properties: {{ kind: {{ \
                 type: string }} }} }}\n"
            ),
        ),
    ];
    for (spelling, pet) in &spellings {
        let spec = format!("{head}{pet}{holder}");
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}, {entry}: a nullable union refined to only `null` rejected: \
                 {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{spelling}, {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        assert!(
            types.contains("pub type Pet = ();"),
            "{spelling}: a nullable union refined to only `null` is not the null type:\n{types}"
        );
        // The use site agrees too: the exact null type is the whole value set, so the holder's
        // field is `Pet` itself, never an `Option` over it.
        assert!(
            types.contains("pub p: Pet,"),
            "{spelling}: the holder's field is not the bare null type:\n{types}"
        );
    }

    // The rescue is `null`'s, so it needs a refiner that admits `null`. A typed `allOf` member
    // denies it, and then no value satisfies the schema.
    let spec = format!(
        "{head}    Pet:\n      allOf: [{{ type: object, {refined} }}]\n      oneOf: {UNION}\n{holder}"
    );
    for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "{entry}: {report:#?}"
        );
    }
}
