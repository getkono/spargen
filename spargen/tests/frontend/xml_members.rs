//! The `xml` object of a composition member whose definition the merge consumes: an `allOf`
//! member read by its keywords, and a `oneOf`/`anyOf` branch merged away. Such a member names no
//! element of its own, so each of its hints is reported — `E009` where an XML body reaches the
//! merged type, `W006` elsewhere — rather than dropped with no diagnostic (#824).

use super::*;

/// A document whose one operation sends `#/components/schemas/Both` as a `media` body, with
/// `schemas` (indented as entries of `components.schemas`) beside a plain `Pet` component.
fn member_document(media: &str, schemas: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /p:
    post:
      operationId: send
      requestBody:
        content:
          {media}: {{ schema: {{ $ref: "#/components/schemas/Both" }} }}
      responses:
        "204": {{ description: ok }}
components:
  schemas:
{schemas}
    Pet:
      type: object
      properties:
        name: {{ type: string }}
"##
    )
}

/// The disposition one member's hints take: rejected with `E009` naming `hint` under an XML body,
/// and a `W006` naming it, with generation going ahead, under a JSON one.
fn assert_member_hint_reported(schemas: &str, hint: &str) {
    let report = generate(&member_document("application/xml", schemas));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let errors = messages_for(&report, Code::UnsupportedMediaType);
    assert!(
        errors
            .iter()
            .any(|message| message.contains(&format!("`{hint}`"))),
        "{report:#?}"
    );

    let report = generate(&member_document("application/json", schemas));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let warnings = messages_for(&report, Code::XmlHintIgnored);
    assert!(
        warnings
            .iter()
            .any(|message| message.contains(&format!("`{hint}`"))),
        "{report:#?}"
    );
}

/// The issue's own probe: an inline `allOf` member's `namespace`.
#[test]
fn an_all_of_members_xml_object_is_reported_against_the_merged_type() {
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - type: object
          xml: { namespace: "urn:member" }
          properties:
            id: { type: integer }
        - $ref: "#/components/schemas/Pet""##,
        "namespace",
    );
}

/// A member's `xml.name` names no element: the merged struct's root element is the composing
/// schema's, so the member's name is reported as unsupported rather than applied or dropped.
#[test]
fn an_all_of_members_xml_name_names_no_element() {
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - type: object
          xml: { name: Member }
          properties:
            id: { type: integer }
        - $ref: "#/components/schemas/Pet""##,
        "name",
    );
}

/// A member of a nested inline `allOf` is flattened into the outer merge, and so is the nested
/// composition itself: each one's `xml` object is reported against the outer merged type.
#[test]
fn a_nested_all_of_members_xml_object_is_reported() {
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - allOf:
            - type: object
              xml: { prefix: m }
              properties:
                id: { type: integer }
        - $ref: "#/components/schemas/Pet""##,
        "prefix",
    );
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - xml: { namespace: "urn:nested" }
          allOf:
            - type: object
              properties:
                id: { type: integer }
        - $ref: "#/components/schemas/Pet""##,
        "namespace",
    );
}

/// The `xml` object beside a `$ref` member: the member is the target merged with its siblings,
/// and a name there would rename neither.
#[test]
fn a_ref_members_xml_object_is_reported() {
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - $ref: "#/components/schemas/Pet"
          xml: { name: Renamed }
        - type: object
          properties:
            id: { type: integer }"##,
        "name",
    );
}

/// `anyOf` branches that cannot be told apart merge into one type, which consumes each branch's
/// own definition and with it the branch's `xml` object.
#[test]
fn a_merged_union_branchs_xml_object_is_reported() {
    assert_member_hint_reported(
        r##"    Both:
      anyOf:
        - type: object
          xml: { namespace: "urn:a" }
          properties:
            id: { type: integer }
        - type: object
          properties:
            id: { type: integer }"##,
        "namespace",
    );
}

/// The composing schema's own root `xml.name` still names the merged struct's element, and a
/// member carrying no `xml` object reports nothing.
#[test]
fn the_composing_schemas_own_xml_name_still_applies() {
    let (report, code) = generate_with_code(&member_document(
        "application/xml",
        r##"    Both:
      xml: { name: both }
      allOf:
        - type: object
          properties:
            id: { type: integer }
        - $ref: "#/components/schemas/Pet""##,
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(code.contains("rename = \"both\""), "{code}");
}
