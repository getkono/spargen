//! Schema Object keywords whose value has the wrong JSON shape, or names no JSON type (#716).
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
        let generated = generate(&spec);
        let checked = check(&spec);
        let expected = format!("{U}{at}");
        for report in [&generated, &checked] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
            assert_eq!(
                invalid_input_pointers(report),
                [expected.as_str()],
                "{body}: {report:#?}"
            );
        }
    }
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
