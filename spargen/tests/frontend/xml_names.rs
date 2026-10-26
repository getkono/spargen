//! `xml.name` held to the XML 1.1 `Name` production, wherever the `xml` object is written.

use super::*;

/// The spec the `xml.name` fixtures below share: an XML response whose component schema `Pet` is
/// reached by the body, a JSON request whose `Tag` no XML body reaches, and, under 3.2, `Note`,
/// whose root `nodeType: text` leaves its name ignored. `{pet_root}` and the other slots hold
/// each position's `xml` object, empty where the fixture leaves it out.
fn xml_name_spec(pet_root: &str, id: &str, items: &str, tag: &str, note: &str) -> String {
    format!(
        r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema: {{ $ref: '#/components/schemas/Pet' }}
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                tag: {{ $ref: '#/components/schemas/Tag' }}
                note: {{ $ref: '#/components/schemas/Note' }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
    Pet:
      type: object
      {pet_root}
      properties:
        id: {{ type: string, {id} }}
        names:
          type: array
          items: {{ type: string, {items} }}
    Tag:
      type: object
      properties:
        label: {{ type: string, {tag} }}
    Note:
      type: string
      {note}
"##
    )
}

/// An explicit `xml.name` is written as the element or attribute name, and quick-xml refuses to
/// write one that is not an XML 1.1 `Name`, so a document carrying one used to generate and then
/// fail on every serialization of the body (#997). It is rejected as `E011` at the `name` keyword,
/// in every position an `xml` object is read in: a schema's root, a property, an array's `items`,
/// a property of a type no XML body reaches, and a root whose node type leaves the name ignored.
#[test]
fn e011_an_xml_name_that_is_not_an_xml_name_is_rejected_wherever_it_is_written() {
    for bad in ["", "first name", "1st", "-dash", ".dot"] {
        let hint = format!("xml: {{ name: '{bad}' }}");
        let text = format!("xml: {{ name: '{bad}', nodeType: text }}");
        let positions = [
            (
                xml_name_spec(&hint, "", "", "", ""),
                "/components/schemas/Pet/xml/name",
            ),
            (
                xml_name_spec("", &hint, "", "", ""),
                "/components/schemas/Pet/properties/id/xml/name",
            ),
            (
                xml_name_spec("", "", &hint, "", ""),
                "/components/schemas/Pet/properties/names/items/xml/name",
            ),
            (
                xml_name_spec("", "", "", &hint, ""),
                "/components/schemas/Tag/properties/label/xml/name",
            ),
            (
                xml_name_spec("", "", "", "", &text),
                "/components/schemas/Note/xml/name",
            ),
        ];
        for (spec, pointer) in positions {
            for report in [generate(&spec), check(&spec)] {
                assert_eq!(report.outcome(), Outcome::Rejected, "`{bad}` at {pointer}");
                assert!(
                    report
                        .diagnostics()
                        .iter()
                        .any(|d| d.code == Code::InvalidInput
                            && d.pointer.as_str() == pointer
                            && d.message.contains("is not an XML name")),
                    "`{bad}` at {pointer}: {report:#?}"
                );
            }
        }
    }
}

/// The control: names the production admits — a namespace-style `:`, a leading `_`, a non-ASCII
/// letter, and the `-`, `.` and digits it admits after the first character — generate in every
/// position the rejection above covers.
#[test]
fn an_xml_name_the_xml_name_production_admits_generates() {
    for good in ["ns:pet", "_pet", "été", "a-1.b"] {
        let hint = format!("xml: {{ name: '{good}' }}");
        let text = format!("xml: {{ name: '{good}', nodeType: text }}");
        let spec = xml_name_spec(&hint, &hint, &hint, &hint, &text);
        let report = generate(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{good}`: {report:#?}");
        assert!(
            !has_code(&report, Code::InvalidInput),
            "`{good}`: {report:#?}"
        );
    }
}
