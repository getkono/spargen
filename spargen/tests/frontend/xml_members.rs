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
    assert_member_hint_reported_by(|media| generate(&member_document(media, schemas)), hint);
}

/// [`assert_member_hint_reported`] over whatever `run` reports for the document under each media
/// type, so a fixture laid out beside vendored remote documents is judged by the same rule.
fn assert_member_hint_reported_by(run: impl Fn(&str) -> Report, hint: &str) {
    let report = run("application/xml");
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let errors = messages_for(&report, Code::UnsupportedMediaType);
    assert!(
        errors
            .iter()
            .any(|message| message.contains(&format!("`{hint}`"))),
        "{report:#?}"
    );

    let report = run("application/json");
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

/// A member of a component's `allOf`, where that component reaches the body only as a `$ref`
/// member of another composition: the member's contribution is merged into that composition too,
/// so it is judged by every type that merges it, not by the component's own reachability alone.
#[test]
fn a_member_merged_through_a_component_ref_is_judged_by_the_type_merging_it() {
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - $ref: "#/components/schemas/Shared"
        - $ref: "#/components/schemas/Pet"
    Shared:
      allOf:
        - type: object
          xml: { namespace: "urn:shared" }
          properties:
            id: { type: integer }"##,
        "namespace",
    );
    // Two `$ref` hops: `Both` merges `Middle`, which merges `Shared`.
    assert_member_hint_reported(
        r##"    Both:
      allOf:
        - $ref: "#/components/schemas/Middle"
        - $ref: "#/components/schemas/Pet"
    Middle:
      allOf:
        - $ref: "#/components/schemas/Shared"
    Shared:
      allOf:
        - type: object
          xml: { prefix: s }
          properties:
            id: { type: integer }"##,
        "prefix",
    );
}

/// The remote counterpart: `Both` merges a pinned remote document whose own `allOf` member
/// carries the hint, and that document reaches the body only as `Both`'s `$ref` member, so the
/// member is judged by `Both`'s XML body rather than by the remote type's own reachability.
#[test]
fn a_member_merged_through_a_remote_ref_is_judged_by_the_type_merging_it() {
    const SHARED_URL: &str = "https://api.example.com/schemas/shared.yaml";
    const SHARED_PATH: &str = "api.example.com/schemas/shared.yaml";
    const SHARED_YAML: &str = "allOf:\n  - type: object\n    xml: { namespace: \"urn:remote\" }\n    properties:\n      id: { type: integer }\n";
    const SHARED_SHA256: &str = "84ffeec6c43b14582abbb3e46efdac70ec2cec418926d3a2b0b666cdb810b577";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{SHARED_URL}\"\nsha256 = \"{SHARED_SHA256}\"\npath = \
         \"{SHARED_PATH}\"\n"
    );
    let schemas = format!(
        r##"    Both:
      allOf:
        - $ref: "{SHARED_URL}"
        - $ref: "#/components/schemas/Pet""##
    );
    for check_only in [false, true] {
        assert_member_hint_reported_by(
            |media| {
                let document = member_document(media, &schemas);
                let vendor = [(SHARED_PATH, SHARED_YAML)];
                let (report, _temp, _out) =
                    super::refs_remote::run_layout(&document, Some(&lock), &vendor, check_only);
                // The pin is live, so the document reaches lowering rather than being rejected
                // for drift or for being unpinned.
                assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
                assert!(
                    !has_code(&report, Code::AbsoluteRefUnsupported),
                    "{report:#?}"
                );
                report
            },
            "namespace",
        );
    }
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
