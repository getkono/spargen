//! `xml.name` held to the XML 1.1 `Name` production, wherever the `xml` object is written, and a
//! component name that is no XML name not applied as the element name OpenAPI infers from it.

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

/// An explicit `xml.name` is written as the element or attribute name. quick-xml refuses to write
/// a non-empty one that is not an XML 1.1 `Name`, so a document carrying one used to generate and
/// then fail on every serialization of the body (#997), and writes an empty one as malformed XML
/// (`<>…</>`), so a document carrying that used to generate a client that sent it. It is rejected as `E011` at the `name` keyword,
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

/// A component key may be no XML name (a leading digit, `-` or `.`), and quick-xml refuses to
/// write such an element at run time. The name OpenAPI infers from it is not applied: each struct
/// an XML body reaches reports it (`W006`) and keeps its Rust name, while a struct no XML body
/// reaches has nothing to report.
#[test]
fn a_component_name_that_is_no_xml_name_is_reported_and_not_applied() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /receipts:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/1-receipt' } }
      responses:
        '200':
          description: ok
          content:
            application/xml: { schema: { $ref: '#/components/schemas/-order' } }
  /dots:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/.dot' } }
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/2-json' } }
components:
  schemas:
    1-receipt:
      type: object
      properties:
        id: { type: integer }
    '-order':
      type: object
      properties:
        id: { type: integer }
    .dot:
      type: object
      properties:
        id: { type: integer }
    2-json:
      type: object
      properties:
        id: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut messages = messages_for(&report, Code::XmlHintIgnored);
    messages.sort_unstable();
    assert_eq!(messages.len(), 3, "{report:#?}");
    for (message, name) in messages.iter().zip(["-order", ".dot", "1-receipt"]) {
        assert!(
            message.contains(&format!("component name `{name}` is not an XML name")),
            "{message}"
        );
    }
    // Each struct keeps its Rust name on the wire: no container rename at all.
    for ty in ["N1Receipt", "Order", "Dot", "N2Json"] {
        let renames: Vec<_> = struct_attributes(&code, ty)
            .into_iter()
            .filter(|line| line.contains("rename"))
            .collect();
        assert!(renames.is_empty(), "{ty}: {renames:?}\n{code}");
    }
}
