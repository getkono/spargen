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

/// The plain JSON Schema 2020-12 dialect URI.
const JSON_SCHEMA_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// A `Pet` union with a `discriminator` mapping `kitty` to `Cat`, `pet_dialect` written on `Pet`
/// and `document_dialect` at the document root. `Holder` holds a second such union at
/// `properties/pet`, mapping `held` to `Cat`, under `holder_dialect`, which `/held` reaches by a
/// JSON Pointer `$ref` into `Holder`.
fn dialect_pets(document_dialect: &str, pet_dialect: &str, holder_dialect: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
{document_dialect}
paths:
  /pet:
    get:
      operationId: getPet
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Pet' }} }}
  /held:
    get:
      operationId: getHeld
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder/properties/pet' }} }}
components:
  schemas:
    Pet:
      {pet_dialect}
      oneOf:
        - {{ $ref: '#/components/schemas/Cat' }}
        - {{ $ref: '#/components/schemas/Dog' }}
      discriminator:
        propertyName: kind
        mapping: {{ kitty: Cat }}
    Holder:
      {holder_dialect}
      type: object
      properties:
        pet:
          oneOf:
            - {{ $ref: '#/components/schemas/Cat' }}
            - {{ $ref: '#/components/schemas/Dog' }}
          discriminator:
            propertyName: kind
            mapping: {{ held: Cat }}
    Cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }} }} }}
    Dog: {{ type: object, required: [kind, bark], properties: {{ kind: {{ type: string }}, bark: {{ type: boolean }} }} }}
"##
    )
}

/// #1452: `discriminator` is OAS vocabulary, which the plain JSON Schema 2020-12 dialect does not
/// define, so under it the keyword is an annotation: it selects nothing, and the union is decoded
/// by its members' schemas as one with no discriminator is. It is reported as `W011` at the
/// keyword, whether the dialect is the document's `jsonSchemaDialect`, a `$schema` on the schema
/// itself, or a `$schema` on a schema enclosing the one a `$ref` points into. A `$schema` naming
/// the OAS dialect inside a plain-dialect document brings the keyword back.
// W011 case: foreign-dialect-keyword
#[test]
fn a_discriminator_under_the_plain_json_schema_dialect_is_w011_and_dispatches_nothing() {
    const PET: &str = "/components/schemas/Pet/discriminator";
    const HELD: &str = "/components/schemas/Holder/properties/pet/discriminator";
    let plain_root = format!("jsonSchemaDialect: {JSON_SCHEMA_2020_12}");
    let plain_schema = format!("$schema: {JSON_SCHEMA_2020_12}");
    let oas_schema = "$schema: https://spec.openapis.org/oas/3.1/dialect/base";
    let cases: [(&str, String, &[&str]); 4] = [
        (
            "document dialect",
            dialect_pets(&plain_root, "", ""),
            &[HELD, PET],
        ),
        ("own $schema", dialect_pets("", &plain_schema, ""), &[PET]),
        (
            "enclosing $schema",
            dialect_pets("", "", &plain_schema),
            &[HELD],
        ),
        (
            "OAS $schema in a plain document",
            dialect_pets(&plain_root, oas_schema, oas_schema),
            &[],
        ),
    ];
    for (what, spec, inert) in cases {
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let mut w011 = located_messages(&report, Code::DeclarationHasNoEffect);
        w011.sort_unstable();
        w011.dedup();
        assert_eq!(
            w011.iter().map(|(pointer, _)| *pointer).collect::<Vec<_>>(),
            inert,
            "{what}: {report:#?}"
        );
        for (_, message) in &w011 {
            assert!(
                message.contains("`discriminator`")
                    && message.contains("JSON Schema 2020-12 dialect"),
                "{what}: {message}"
            );
        }
        for (pointer, tag) in [(PET, "\"kitty\""), (HELD, "\"held\"")] {
            assert_eq!(
                code.contains(tag),
                !inert.contains(&pointer),
                "{what}: {tag} dispatches exactly when its discriminator is read:\n{code}"
            );
        }
    }
}

/// #1452: under the plain JSON Schema 2020-12 dialect a malformed `discriminator` is an unknown
/// keyword's value, not a malformed Discriminator Object, so it is not `E011`; and `xml` is OAS
/// vocabulary too, so a hint that would be `E009` on an XML body is `W006` instead, the body
/// generating without it.
#[test]
fn oas_vocabulary_under_the_plain_json_schema_dialect_is_read_as_annotation() {
    let spec = |dialect: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /item:
    get:
      operationId: getItem
      responses:
        '200':
          description: ok
          content:
            application/xml: {{ schema: {{ $ref: '#/components/schemas/Item' }} }}
components:
  schemas:
    Item:
      {dialect}
      type: object
      required: [id]
      xml: {{ namespace: 'urn:example' }}
      discriminator: {{ mapping: 3 }}
      properties:
        id: {{ type: string }}
"##
        )
    };
    let report = generate(&spec(""));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");

    let report = generate(&spec(&format!("$schema: {JSON_SCHEMA_2020_12}")));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let pointers = |code| {
        located_messages(&report, code)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pointers(Code::XmlHintIgnored),
        ["/components/schemas/Item/xml"],
        "{report:#?}"
    );
    assert_eq!(
        pointers(Code::DeclarationHasNoEffect),
        ["/components/schemas/Item/discriminator"],
        "{report:#?}"
    );
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
}

/// A referenced sub-file holding a `Pet` union whose `discriminator` maps `kitty` to `Cat`, with
/// `header` written at the file's root.
fn dialect_sub_file(header: &str) -> String {
    format!(
        r##"
{header}
components:
  schemas:
    Pet:
      oneOf:
        - {{ $ref: '#/components/schemas/Cat' }}
        - {{ $ref: '#/components/schemas/Dog' }}
      discriminator:
        propertyName: kind
        mapping: {{ kitty: Cat }}
    Cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }} }} }}
    Dog: {{ type: object, required: [kind, bark], properties: {{ kind: {{ type: string }}, bark: {{ type: boolean }} }} }}
"##
    )
}

/// #1452: `jsonSchemaDialect` is the default "within this OAS document" (3.2.0.md, Specifying
/// Schema Dialects), so it does not reach into a referenced file. A sub-file that is a complete
/// OpenAPI document reads Schema Objects under its own `jsonSchemaDialect`, or the OAS dialect when
/// it declares none; one that is not a complete document is read under the OAS dialect, as the
/// specification says it SHOULD be. A sub-file `jsonSchemaDialect` spargen does not read is
/// `E002` there, as at the root.
#[test]
fn a_referenced_file_reads_its_own_dialect_default_not_the_roots() {
    const PET: &str = "/components/schemas/Pet/discriminator";
    let root = |dialect: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
{dialect}
servers: [{{ url: 'https://e.com' }}]
paths:
  /pet:
    get:
      operationId: getPet
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: './lib.yaml#/components/schemas/Pet' }} }}
"##
        )
    };
    let plain = format!("jsonSchemaDialect: {JSON_SCHEMA_2020_12}");
    let complete = "openapi: 3.1.0\ninfo: { title: L, version: 1.0.0 }";
    let complete_plain = format!("{complete}\n{plain}");
    let cases: [(&str, String, String, bool); 4] = [
        (
            "fragment file under a plain root",
            root(&plain),
            dialect_sub_file(""),
            false,
        ),
        (
            "complete file without a dialect under a plain root",
            root(&plain),
            dialect_sub_file(complete),
            false,
        ),
        (
            "complete plain file under an OAS root",
            root(""),
            dialect_sub_file(&complete_plain),
            true,
        ),
        (
            "fragment file declaring a dialect it is not a document to declare",
            root(""),
            dialect_sub_file(&plain),
            false,
        ),
    ];
    for (what, root, lib, inert) in cases {
        let (report, _, code) =
            generate_and_check_files(&[("openapi.yaml", &root), ("lib.yaml", &lib)]);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let w011: Vec<_> = located_messages(&report, Code::DeclarationHasNoEffect)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .filter(|pointer| *pointer == PET)
            .collect();
        assert_eq!(!w011.is_empty(), inert, "{what}: {report:#?}");
        assert_eq!(
            code.contains("\"kitty\""),
            !inert,
            "{what}: the tag dispatches exactly when the discriminator is read:\n{code}"
        );
    }

    let (report, _, _) = generate_and_check_files(&[
        ("openapi.yaml", &root("")),
        (
            "lib.yaml",
            &dialect_sub_file(&format!(
                "{complete}\njsonSchemaDialect: https://json-schema.org/draft/2019-09/schema"
            )),
        ),
    ]);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        located_messages(&report, Code::UnsupportedDialect)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .collect::<Vec<_>>(),
        ["/jsonSchemaDialect"],
        "{report:#?}"
    );
}
