//! Schema Object keywords whose value has the wrong JSON shape, or names no JSON type (#716,
//! #861).
//!
//! The Schema Object deserializer used to drop such a value with nothing said — `required: id`
//! became no required properties, `type: strng` no type, `allOf: {}` no members, `maxLength: "5"`
//! no bound — so the generated type silently differed from what the author wrote. The vendored
//! document schema checks a Schema Object only as `object | boolean`, so nothing before the
//! deserializer catches these: each is `E011` at the keyword (or the array element) that holds it.

use super::*;

/// The pointer of the component `U` in [`single_component_document`].
const U: &str = "/components/schemas/U";

/// Each `E011` in `report`, as its pointer.
fn invalid_input_pointers(report: &Report) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::InvalidInput)
        .map(|d| d.pointer.as_str())
        .collect()
}

#[test]
fn e011_an_ill_typed_schema_keyword_is_rejected_at_the_keyword() {
    for (body, at) in [
        ("type: strng", "/type"),
        ("type: 5", "/type"),
        ("type: { kind: string }", "/type"),
        ("type: [string, strng]", "/type/1"),
        ("type: [string, 5]", "/type/1"),
        (
            "type: object\n      properties: { id: { type: string } }\n      required: id",
            "/required",
        ),
        (
            "type: object\n      properties: { id: { type: string } }\n      required: [id, 5]",
            "/required/1",
        ),
        ("allOf: {}", "/allOf"),
        ("oneOf: { $ref: '#/components/schemas/Name' }", "/oneOf"),
        ("anyOf: string", "/anyOf"),
        ("type: array\n      prefixItems: {}", "/prefixItems"),
        ("type: string\n      maxLength: '5'", "/maxLength"),
        ("type: string\n      minLength: -1", "/minLength"),
        ("type: array\n      minItems: 1.5", "/minItems"),
        ("type: array\n      maxItems: true", "/maxItems"),
        ("type: object\n      minProperties: [1]", "/minProperties"),
        ("type: object\n      maxProperties: '2'", "/maxProperties"),
    ] {
        let spec = single_component_document(&format!("      {body}"));
        let report = generate(&spec);
        let expected = format!("{U}{at}");
        assert_eq!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert_eq!(
            invalid_input_pointers(&report),
            [expected.as_str()],
            "{body}: {report:#?}"
        );
    }
}

#[test]
fn e011_an_ill_typed_value_keyword_is_rejected_at_the_keyword() {
    // #861: the numeric, string, boolean, map, and `enum` keywords the #716 fix left reading a
    // value of the wrong shape as absent.
    for (body, at) in [
        ("type: number\n      minimum: '5'", "/minimum"),
        ("type: number\n      maximum: [5]", "/maximum"),
        (
            "type: number\n      exclusiveMinimum: '0'",
            "/exclusiveMinimum",
        ),
        (
            "type: number\n      exclusiveMaximum: { value: 9 }",
            "/exclusiveMaximum",
        ),
        ("type: number\n      multipleOf: true", "/multipleOf"),
        ("type: string\n      pattern: 5", "/pattern"),
        ("type: array\n      uniqueItems: 'yes'", "/uniqueItems"),
        ("type: object\n      properties: []", "/properties"),
        (
            "type: object\n      patternProperties: '^x-'",
            "/patternProperties",
        ),
        ("type: object\n      $defs: [a]", "/$defs"),
        (
            "type: object\n      dependentSchemas: []",
            "/dependentSchemas",
        ),
        ("type: string\n      enum: a", "/enum"),
        ("$ref: 5\n      type: string", "/$ref"),
        ("type: string\n      format: 5", "/format"),
        (
            "type: string\n      contentEncoding: [base64]",
            "/contentEncoding",
        ),
        (
            "type: string\n      contentMediaType: 5",
            "/contentMediaType",
        ),
        ("type: string\n      title: [T]", "/title"),
        (
            "type: string\n      description: { text: d }",
            "/description",
        ),
        ("type: string\n      deprecated: 'true'", "/deprecated"),
        ("type: string\n      readOnly: 1", "/readOnly"),
        ("type: string\n      writeOnly: [true]", "/writeOnly"),
        ("type: string\n      $schema: 5", "/$schema"),
    ] {
        let spec = single_component_document(&format!("      {body}"));
        let report = generate(&spec);
        let expected = format!("{U}{at}");
        assert_eq!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert_eq!(
            invalid_input_pointers(&report),
            [expected.as_str()],
            "{body}: {report:#?}"
        );
    }
}

#[test]
fn an_ill_typed_numeric_keyword_names_the_number_it_expected() {
    let report = generate(&single_component_document(
        "      type: number\n      minimum: '5'",
    ));
    let messages = messages_for(&report, Code::InvalidInput);
    assert_eq!(messages, ["`minimum` must be a number"], "{report:#?}");
}

#[test]
fn an_openapi_3_0_boolean_exclusive_bound_is_warned_as_an_unenforced_validation_keyword() {
    // OpenAPI 3.0's `exclusiveMinimum: true` (a modifier of `minimum`) appears in real-world
    // documents labelled 3.1, the pinned OpenAI description among them. Under either reading it
    // constrains only validation, which spargen does not enforce, so the Rust shape is the same
    // and it is `W001`, not a rejection.
    for body in [
        "type: number\n      minimum: 0\n      exclusiveMinimum: true",
        "type: number\n      exclusiveMaximum: false",
    ] {
        let spec = single_component_document(&format!("      {body}"));
        let report = generate(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert!(
            !has_code(&report, Code::InvalidInput),
            "{body}: {report:#?}"
        );
        assert!(
            has_code(&report, Code::ValidationKeywordIgnored),
            "{body}: {report:#?}"
        );
    }
}

// W001 case: oas30-nullable
#[test]
fn an_openapi_3_0_nullable_is_warned_and_not_reinterpreted() {
    // #1342: OpenAPI 3.0's `nullable: true`, which documents converted to 3.1 often keep, is no
    // keyword of JSON Schema 2020-12, so `name` stays a strict `Option<String>` and the `null` its
    // author meant to admit fails decoding. That used to pass the build with nothing said. It is
    // `W001` at the schema carrying it, in 3.1 and 3.2 alike, and the shape is unchanged.
    let property =
        "      type: object\n      properties:\n        name: { type: string, nullable: ";
    for version in ["3.1.0", "3.2.0"] {
        let document = |nullable: &str| {
            single_component_document(&format!("{property}{nullable} }}"))
                .replace("openapi: 3.1.0", &format!("openapi: {version}"))
        };
        let (report, code) = generate_with_code(&document("true"));
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{version}: {report:#?}"
        );
        let warned: Vec<(&str, &str)> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::ValidationKeywordIgnored)
            .map(|d| (d.pointer.as_str(), d.message.as_str()))
            .collect();
        assert_eq!(
            warned,
            [(
                "/components/schemas/U/properties/name",
                "OpenAPI 3.0's `nullable` is not a keyword of OpenAPI 3.1 or 3.2: whether `null` \
                 is valid is read from `type`, `enum`, `const` and the schema's applicators alone"
            )],
            "{version}: {report:#?}"
        );
        assert!(
            code.contains("pub name: Option<Uname>") && code.contains("pub type Uname = String;"),
            "{version}: {code}"
        );

        // `nullable: false` states what the schema means without it, so it changes nothing.
        let report = generate(&document("false"));
        assert!(
            !has_code(&report, Code::ValidationKeywordIgnored),
            "{version}: {report:#?}"
        );
    }
}

// W001 case: oas30-nullable
#[test]
fn an_openapi_3_0_nullable_remedy_fits_the_schema_carrying_it() {
    // Beside a `$ref`, `type: [T, 'null']` is intersected with the target's non-null `type` and
    // still admits no `null`, so that schema is pointed at the `oneOf` form instead.
    let spec = single_component_document(
        "      type: object\n      properties:\n        \
         name: { type: string, nullable: true }\n        \
         base: { $ref: '#/components/schemas/Base', nullable: true }",
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let mut remedies: Vec<(&str, Option<&str>)> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::ValidationKeywordIgnored)
        .map(|d| (d.pointer.as_str(), d.remedy.as_deref()))
        .collect();
    remedies.sort_unstable();
    assert_eq!(
        remedies,
        [
            (
                "/components/schemas/U/properties/base",
                Some(
                    "replace the `$ref` with `oneOf: [{type: 'null'}, {$ref: …}]`: a `type` \
                     beside it is intersected with the target's and still excludes `null`"
                )
            ),
            (
                "/components/schemas/U/properties/name",
                Some("list `null` in the schema's `type` instead, as `type: [T, 'null']`")
            ),
        ],
        "{report:#?}"
    );
}

#[test]
fn well_typed_value_keywords_generate_cleanly() {
    let spec = single_component_document(
        "      type: object\n      title: U\n      description: a U\n      deprecated: false\n      \
         properties:\n        n: { type: number, minimum: 0, maximum: 9.5, exclusiveMinimum: -1, \
         exclusiveMaximum: 10, multipleOf: 0.5, readOnly: true }\n        \
         s: { type: string, pattern: '^a', format: date-time, contentEncoding: base64, \
         contentMediaType: image/png, enum: [a, b], writeOnly: true }\n        \
         l: { type: array, items: { type: string }, uniqueItems: true }\n      \
         patternProperties: {}\n      $defs: {}\n      dependentSchemas: {}",
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn a_required_string_is_rejected_rather_than_read_as_no_required_property() {
    // The issue's reproduction: `id` used to generate as `Option<String>` with nothing reported.
    let spec = single_component_document(
        "      type: object\n      properties: { id: { type: string } }\n      required: id",
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::InvalidInput);
    assert_eq!(messages, ["expected an array"], "{report:#?}");
}

#[test]
fn an_unknown_type_name_is_named_in_the_rejection() {
    let report = generate(&single_component_document("      type: strng"));
    let messages = messages_for(&report, Code::InvalidInput);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(messages[0].contains("`strng`"), "{report:#?}");
}

#[test]
fn an_integral_count_written_as_a_float_is_accepted() {
    // JSON Schema 2020-12 (core, section 4.2.2) reads `5.0` as the integer 5, so a count keyword
    // written that way is a well-typed bound, not an ill-typed value.
    for body in [
        "type: string\n      maxLength: 5.0",
        "type: array\n      items: { type: string }\n      minItems: 0.0",
        "type: object\n      maxProperties: 2.0",
    ] {
        let spec = single_component_document(&format!("      {body}"));
        let report = generate(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert!(
            !has_code(&report, Code::InvalidInput),
            "{body}: {report:#?}"
        );
    }
}

#[test]
fn well_typed_schema_keywords_generate_cleanly() {
    let spec = single_component_document(
        "      type: [object]\n      properties: { id: { type: string } }\n      required: [id]\n      minProperties: 1",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
    // `required: [id]` made `id` a required field.
    let types = types_module(&code);
    let id = field_type(&types, "pub id:").unwrap_or_default();
    assert!(
        !id.is_empty() && !id.starts_with("Option<"),
        "`id` is `{id}`: {types}"
    );
}

/// Each `(pointer, message)` of `code` in `report`.
fn located_messages(report: &Report, code: Code) -> Vec<(&str, &str)> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == code)
        .map(|d| (d.pointer.as_str(), d.message.as_str()))
        .collect()
}

#[test]
fn e006_a_recursive_ref_or_anchor_is_rejected_like_a_dynamic_ref() {
    // #1446: draft 2019-09's `$recursiveRef`, which 2020-12 replaced with `$dynamicRef`, used to
    // be an unknown keyword, so the property holding it generated as `serde_json::Value` with
    // nothing said, though its author meant a recursive type.
    for (body, at) in [
        (
            "      type: object\n      properties: { c: { $recursiveRef: '#' } }",
            "/components/schemas/U/properties/c",
        ),
        ("      $recursiveAnchor: true\n      type: object", U),
    ] {
        let report = generate(&single_component_document(body));
        assert_eq!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert_eq!(
            located_messages(&report, Code::DynamicRefRejected),
            [(
                at,
                "`$recursiveRef` and `$recursiveAnchor` are JSON Schema 2019-09 keywords, which \
                 2020-12 replaced with `$dynamicRef`/`$dynamicAnchor`, and require dynamic schema \
                 scope evaluation"
            )],
            "{body}: {report:#?}"
        );
        let remedy = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::DynamicRefRejected)
            .and_then(|d| d.remedy.as_deref())
            .unwrap_or_default();
        assert!(
            remedy.contains("JSON Pointer `$ref`"),
            "{body}: {report:#?}"
        );
    }
}

// W001 case: pre-2020-keyword
#[test]
fn a_pre_2020_12_keyword_is_warned_and_generates() {
    // #1446: draft 2019-09/draft-7's `additionalItems` and `dependencies` are no keywords of
    // JSON Schema 2020-12, so the constraints they state used to be dropped with nothing said.
    // Each is `W001` at the schema carrying it, naming the 2020-12 keywords that replace it.
    for (body, expected) in [
        (
            "      type: array\n      items: { type: string }\n      additionalItems: false",
            (
                U,
                "`additionalItems` is a JSON Schema keyword older than 2020-12, which replaced \
                 it with `items` beside `prefixItems`: the constraint it states is not enforced",
            ),
        ),
        (
            "      type: object\n      properties: { a: { type: string } }\n      \
             dependencies: { a: [b], b: { required: [a] } }",
            (
                U,
                "`dependencies` is a JSON Schema keyword older than 2020-12, which split it into \
                 `dependentRequired` and `dependentSchemas`: the constraint it states is not \
                 enforced",
            ),
        ),
    ] {
        let report = generate(&single_component_document(body));
        assert_ne!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert_eq!(
            located_messages(&report, Code::ValidationKeywordIgnored),
            [expected],
            "{body}: {report:#?}"
        );
    }
}

#[test]
fn a_reference_under_a_pre_2020_12_keyword_is_resolved() {
    // The subschemas `additionalItems` and `dependencies` hold are parsed and audited like the
    // other unlowered applicators', so a reference under one that resolves nowhere is still
    // reported rather than dropped with the keyword.
    for body in [
        "      type: array\n      items: { type: string }\n      \
         additionalItems: { $ref: '#/components/schemas/Missing' }",
        "      type: object\n      properties: { a: { type: string } }\n      \
         dependencies: { a: { $ref: '#/components/schemas/Missing' } }",
    ] {
        let report = generate(&single_component_document(body));
        assert_eq!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        assert!(
            has_code(&report, Code::UnresolvedRef),
            "{body}: {report:#?}"
        );
    }
}
