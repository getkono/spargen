//! XML bodies and hints, including OpenAPI 3.2's `nodeType`.

use super::*;

/// One sub-file schema carrying `xml.name`/`xml.attribute`, used as the **XML** body of one
/// operation and the **JSON** body of another.
///
/// A serde `rename` applies to every format, so `gate_xml_field_renames` suppresses XML hints on any
/// type that is not used exclusively as an XML body. The resolved-reference memo changed what that
/// policy sees: before it, the two operations lowered the sub-file schema to two types — the XML
/// one dedicated and keeping `#[serde(rename = "@Ident")]`, the JSON one suppressed — and now they
/// share one type, which is reachable from both and is therefore suppressed for both. **The XML on
/// the wire moved**, and the `W006` count did not change, so an upgrading consumer had nothing to
/// compare.
///
/// The suppression stands: giving an XML use its own type would reintroduce two types for one
/// target, which the memo exists to prevent. What this test pins is that the warning says which of
/// its two quite different situations it is in, so a consumer can tell "your hint was inert" from
/// "your XML body's field names just changed".
#[test]
fn a_schema_shared_between_an_xml_and_a_non_xml_body_says_so() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /xml:
    get:
      operationId: getXml
      responses:
        '200':
          description: ok
          content:
            application/xml: { schema: { $ref: './lib.yaml#/components/schemas/Item' } }
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Item' } }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id:
          type: string
          xml: { name: Ident, attribute: true }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap();

    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // One target, one type — what the resolved-reference memo guarantees.
    assert_eq!(
        declared_types(&code, "Item", |tail| tail.is_empty()).len(),
        1,
        "{code}"
    );
    // And the warning names the situation it is actually in.
    let shared: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::XmlHintIgnored)
        .collect();
    assert!(!shared.is_empty(), "{report:#?}");
    assert!(
        shared.iter().any(|d| d
            .message
            .contains("shared between an XML body and a non-XML")),
        "the schema IS used as an XML body, so the warning must say the XML body's own field \
         names are affected rather than that the hint was never reachable: {report:#?}"
    );

    // The control, and the other half of the same `W006`: a schema carrying XML hints that is never
    // used as an XML body at all. Its hint is inert, nothing on any wire moved, and it must NOT
    // borrow the shared wording — otherwise one message covers both and pins neither.
    let inert = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Item' } }
components:
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id:
          type: string
          xml: { name: Ident, attribute: true }
"##;
    let report = generate(inert);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let inert_warnings: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::XmlHintIgnored)
        .collect();
    assert!(!inert_warnings.is_empty(), "{report:#?}");
    assert!(
        inert_warnings.iter().all(|d| !d
            .message
            .contains("shared between an XML body and a non-XML")),
        "this schema is never an XML body, so nothing was shared: {report:#?}"
    );
    assert!(
        inert_warnings
            .iter()
            .any(|d| d.message.contains("never used as an XML body")),
        "{report:#?}"
    );
}

#[test]
fn oas32_xml_attribute_node_type_maps_to_the_existing_typed_xml_path() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: attribute } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(code.contains("@id"), "{code}");
}

/// OpenAPI 3.2 says `attribute`/`wrapped` MUST NOT be present when `nodeType` is. The two
/// spellings can disagree and the specification names no winner, so the document is rejected
/// rather than resolved by a guess.
#[test]
fn e011_xml_deprecated_flag_beside_node_type_is_rejected() {
    for (deprecated, node_type) in [("attribute: true", "element"), ("wrapped: true", "element")] {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id:
                    type: array
                    items: {{ type: string }}
                    xml: {{ nodeType: {node_type}, {deprecated} }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{deprecated}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::InvalidInput),
                "{deprecated}: {report:#?}"
            );
        }
    }
}

/// The specification enumerates exactly five `nodeType` values, and the document schema does not
/// validate Schema Objects, so nothing else catches an unknown one. It must take a disposition
/// rather than being ignored — `E009` on a type actually serialized as XML.
#[test]
fn e009_unknown_xml_node_type_is_dispositioned_not_ignored() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: fragment } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

/// The same unknown hint on a type that is never serialized as XML genuinely has no effect, so it
/// warns rather than rejecting — the existing `W006` path, now reached by unknown values too.
#[test]
fn w006_unknown_xml_node_type_on_a_never_xml_type_warns() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: fragment } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn xml_request_body_generates() {
    // an `application/xml` request body lowers to a typed struct and generates (no E009);
    // it is serialized through the runtime's quick-xml codec. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [name]
              properties:
                name: { type: string }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn xml_response_body_generates() {
    // a `text/xml` response body lowers to a typed struct and generates (no E009); it is
    // decoded through the runtime's quick-xml codec rather than serde_json.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            text/xml:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn json_alternative_wins_over_xml_on_same_body() {
    // When a body offers both JSON and XML, media selection deterministically prefers JSON, in
    // either key order, so the API does not use XML at all: generation succeeds with no E009 and
    // the XML codec runtime is not embedded.
    let spec = |content: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      requestBody:
        content:
{content}      responses:
        "204": {{ description: No Content }}
"##
        )
    };
    let xml = "          application/xml:\n            schema: { type: object }\n";
    let json = "          application/json:\n            schema: { type: object, required: [id], properties: { id: { type: string } } }\n";
    // The XML-only control shows the embedded XML codec runtime naming `quick_xml`, so the marker
    // is present whenever that runtime is.
    let (control, control_code) = generate_with_code(&spec(xml));
    assert_eq!(control.outcome(), Outcome::Generated, "{control:#?}");
    assert!(control_code.contains("quick_xml::"), "{control_code}");
    for content in [format!("{xml}{json}"), format!("{json}{xml}")] {
        let (report, code) = generate_with_code(&spec(&content));
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(
            !code.contains("quick_xml::"),
            "XML runtime embedded:\n{code}"
        );
    }
}

#[test]
fn e009_wire_changing_xml_hint_on_an_xml_body() {
    // `wrapped`/`namespace` change the XML wire. Ignoring them on a type that IS serialized as XML
    // would put structurally different bytes on the wire while reporting success, so they reject.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id]
              properties:
                id:
                  type: string
                  xml: { attribute: true, name: "Id" }
                tags:
                  type: array
                  items: { type: string }
                  xml: { wrapped: true, namespace: "urn:example" }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w006_unsupported_xml_hint_on_a_non_xml_type_warns_but_generates() {
    // The same hint on a type never serialized as XML genuinely has no effect, so it is
    // acknowledged and the document is not refused for it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [id]
              properties:
                id:
                  type: string
                  xml: { attribute: true, name: "Id" }
                tags:
                  type: array
                  items: { type: string }
                  xml: { wrapped: true, namespace: "urn:example" }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn json_only_schema_with_xml_hints_suppresses_rename_and_warns_w006() {
    // regression guard: a schema carrying `xml.name`/`xml.attribute` but reachable only
    // from a JSON body must NOT have the format-agnostic serde rename applied (it would corrupt
    // JSON). The suppression is acknowledged with W006 (never silent), and generation still succeeds.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    let checked = check(spec);
    assert!(has_code(&checked, Code::XmlHintIgnored), "{checked:#?}");
}

#[test]
fn xml_dedicated_schema_applies_hints_without_w006() {
    // A schema used *exclusively* as an XML body is XML-dedicated, so `xml.name`/`xml.attribute` are
    // honored — no suppression, and with no unsupported (namespace/prefix/wrapped) hint present, no
    // W006 fires at all.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn schema_shared_by_json_and_xml_ops_suppresses_rename_and_warns_w006() {
    // A component referenced by BOTH a JSON operation and an XML operation is non-dedicated (it is
    // non-XML-reachable), so the rename is suppressed to keep JSON correct, with W006.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /json:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
  /xml:
    post:
      requestBody:
        content:
          application/xml:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Shared:
      type: object
      required: [id]
      properties:
        id: { type: integer, xml: { attribute: true } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn xml_body_in_multi_status_enum_is_rejected() {
    // XML decode is scoped to single-body success/error. An XML body that would land in a
    // multi-status success enum (two bodied success statuses) is rejected cleanly with narrowed E009
    // rather than silently decoded as JSON.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema: { type: object, required: [a], properties: { a: { type: string } } }
        "201":
          description: Created
          content:
            application/json:
              schema: { type: object, required: [b], properties: { b: { type: string } } }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    // check/generate parity: the same rejection is reached without emitting.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

// --- OpenAPI 3.2 XML nodeType defaulting ----------------------------------------------------
//
// 3.2 replaced `attribute`/`wrapped` with `nodeType` and gave it a defaulting table: `$ref` and
// `type: array` schemas default to `none`, everything else to `element`. Before these fixtures
// `nodeType` was a plain string match, so `nodeType: element` on an array — which is exactly
// `wrapped: true` — was accepted and emitted unwrapped XML, while `wrapped: true` was rejected.

#[test]
fn e009_element_node_type_on_an_array_is_rejected_like_wrapped() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: element }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w006_element_node_type_on_an_array_never_serialized_as_xml_warns() {
    // Same declaration on a type that never reaches the wire as XML genuinely has no effect.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: element }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn none_node_type_on_an_array_is_the_default_and_generates() {
    // `none` is the 3.2 default for an array, so restating it is a no-op and takes no
    // disposition. This was previously rejected outright.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: none }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn e009_none_node_type_on_a_scalar_property_is_rejected() {
    // On a scalar the default is `element`, so `none` deletes a node from the wire.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                name:
                  type: string
                  xml: { nodeType: none }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

// --- Root and `items` XML Objects (#703) ----------------------------------------------------
//
// Only a property's own `xml` object used to be read. The one on a schema's root, which names the
// element a body serializes as, and the one on an array's `items`, which names each list element,
// were dropped with no diagnostic, so a `Pet` with `xml: { name: pet, namespace: ... }` went on
// the wire as `<Pet>` with no namespace and nothing said so.

/// The attribute lines directly above `pub struct {ty} {` in the generated `types` module.
fn struct_attributes(code: &str, ty: &str) -> Vec<String> {
    let types = types_module(code);
    let declaration = format!("pub struct {ty} {{");
    let at = types
        .find(&declaration)
        .unwrap_or_else(|| panic!("no `{declaration}` in the generated module:\n{code}"));
    types[..at]
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| line.trim_start().starts_with("#["))
        .map(|line| line.trim().to_owned())
        .collect()
}

/// A component root's `xml.name` names the element an XML body serializes as. A serde container
/// rename reaches no JSON wire (serde_json never reads a struct's name), so unlike a field rename it
/// is applied even where the type also travels as JSON.
#[test]
fn root_xml_name_renames_the_element_a_body_serializes_as() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Pet' } }
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Pet' } }
components:
  schemas:
    Pet:
      type: object
      xml: { name: pet }
      properties:
        id: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(
        struct_attributes(&code, "Pet").contains(&r#"#[serde(rename = "pet")]"#.to_owned()),
        "{code}"
    );
}

/// Without a root `xml.name`, OpenAPI infers a component schema's element name as the component
/// name. A struct an XML body reaches takes it as its container rename wherever it differs from
/// the Rust identifier; an explicit root `xml.name` still wins, and a struct no XML body reaches
/// carries none.
#[test]
fn a_component_name_names_the_element_an_xml_body_serializes_as() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/pet-item' } }
      responses:
        '200':
          description: ok
          content:
            application/xml: { schema: { $ref: '#/components/schemas/Pet' } }
  /orders:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/order_item' } }
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/json-item' } }
components:
  schemas:
    pet-item:
      type: object
      properties:
        id: { type: integer }
    Pet:
      type: object
      properties:
        id: { type: integer }
    order_item:
      type: object
      xml: { name: order }
      properties:
        id: { type: integer }
    json-item:
      type: object
      properties:
        id: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    let renames = |ty: &str| -> Vec<String> {
        struct_attributes(&code, ty)
            .into_iter()
            .filter(|line| line.starts_with("#[serde(rename"))
            .collect()
    };
    assert_eq!(
        renames("PetItem"),
        [r#"#[serde(rename = "pet-item")]"#],
        "{code}"
    );
    assert!(renames("Pet").is_empty(), "{code}");
    assert_eq!(
        renames("OrderItem"),
        [r#"#[serde(rename = "order")]"#],
        "{code}"
    );
    assert!(renames("JsonItem").is_empty(), "{code}");
}

/// The inline schema of an XML body has no component name to infer the element from, so OpenAPI
/// requires its `xml.name`; that name is honoured the same way.
#[test]
fn an_inline_xml_body_root_name_is_honoured() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /docs:
    post:
      operationId: putDocument
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              xml: { name: document }
              properties:
                animals: { type: string }
      responses:
        '204': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(code.contains(r#"#[serde(rename = "document")]"#), "{code}");
}

/// The issue's reproduction: a root namespace the codec cannot write, on a type an XML body
/// serializes. Ignoring it would put un-namespaced XML on the wire while reporting success.
#[test]
fn e009_root_xml_namespace_on_an_xml_body_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Pet' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Pet:
      type: object
      xml: { name: pet, namespace: 'urn:x' }
      properties:
        id: { type: integer }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::UnsupportedMediaType);
        assert!(
            messages.iter().any(|m| m.contains("`namespace`")),
            "{report:#?}"
        );
    }
}

/// The same namespace on a type no XML body reaches changes no wire, so it warns.
#[test]
fn w006_root_xml_namespace_on_a_never_xml_type_warns() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/json: { schema: { $ref: '#/components/schemas/Pet' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Pet:
      type: object
      xml: { name: pet, prefix: p, namespace: 'urn:x' }
      properties:
        id: { type: integer }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::XmlHintIgnored);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("`namespace`") && m.contains("`prefix`")),
            "{report:#?}"
        );
    }
}

/// A root node type the codec cannot produce — a text node, no element at all for an object, or an
/// attribute — is dispositioned rather than dropped.
#[test]
fn e009_a_root_node_type_the_codec_cannot_produce_is_rejected() {
    for xml in [
        "{ nodeType: text }",
        "{ nodeType: none }",
        "{ nodeType: attribute }",
    ] {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: {{ schema: {{ $ref: '#/components/schemas/Pet' }} }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
    Pet:
      type: object
      xml: {xml}
      properties:
        id: {{ type: integer }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{xml}: {report:#?}");
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "{xml}: {report:#?}"
            );
        }
    }
}

/// `xml.name` on a root that is no struct (here a string component) has no container to rename.
#[test]
fn e009_a_root_name_on_a_type_with_no_element_of_its_own_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Wrapper' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Wrapper:
      type: object
      properties:
        code: { $ref: '#/components/schemas/Code' }
    Code:
      type: string
      xml: { name: code-value }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::UnsupportedMediaType);
        assert!(messages.iter().any(|m| m.contains("`name`")), "{report:#?}");
    }
}

/// A root `xml.name` on a struct names an element only where the schema is a component or a Media
/// Type Object's schema. An array component's inline `items` object and a `oneOf` member are never
/// an XML document's root, so a container rename there names nothing: the name is unsupported,
/// `E009` where an XML body reaches the struct and `W006` where none does, and the struct carries
/// no container rename.
#[test]
fn a_root_name_on_a_struct_that_names_no_element_of_its_own_is_dispositioned() {
    let sites = [
        (
            "an array component's items",
            r##"
    Holder:
      type: object
      properties:
        pets: { $ref: '#/components/schemas/Pets' }
    Pets:
      type: array
      items:
        type: object
        xml: { name: pet }
        properties:
          id: { type: integer }
"##,
        ),
        (
            "a oneOf member",
            r##"
    Holder:
      type: object
      properties:
        choice:
          oneOf:
            - type: object
              xml: { name: left }
              required: [l]
              properties:
                l: { type: integer }
              additionalProperties: false
            - type: object
              required: [r]
              properties:
                r: { type: string }
              additionalProperties: false
"##,
        ),
    ];
    for (site, schemas) in sites {
        for (media, xml_body) in [("application/xml", true), ("application/json", false)] {
            let spec = format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /pets:
    post:
      requestBody:
        content:
          {media}: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:{schemas}"##
            );
            let expected = if xml_body {
                Code::UnsupportedMediaType
            } else {
                Code::XmlHintIgnored
            };
            for report in [generate(&spec), check(&spec)] {
                assert_eq!(
                    report.outcome() == Outcome::Rejected,
                    xml_body,
                    "{site} under {media}: {report:#?}"
                );
                assert!(
                    messages_for(&report, expected)
                        .iter()
                        .any(|m| m.contains("`name` on the schema's own `xml` object")),
                    "{site} under {media}: {report:#?}"
                );
            }
            if !xml_body {
                let (_, code) = generate_with_code(&spec);
                assert!(
                    !code.contains(r#"#[serde(rename = "pet")]"#)
                        && !code.contains(r#"#[serde(rename = "left")]"#),
                    "{site} under {media}: {code}"
                );
            }
        }
    }
}

/// An array's `xml.name` names nothing while the array stays unwrapped (its node type defaults to
/// `none`, under which the name SHALL be ignored), so restating it takes no disposition at all.
#[test]
fn a_root_array_name_under_its_default_node_type_is_a_no_op() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Holder' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Holder:
      type: object
      properties:
        tags: { $ref: '#/components/schemas/Tags' }
    Tags:
      type: array
      xml: { name: tags }
      items: { type: string }
"##;
    let (report, _) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
}

/// The specification's own "XML Arrays" examples: an unwrapped array's items are emitted directly
/// under the parent, each named by `items.xml.name`, and the array's own `xml.name` has no effect.
#[test]
fn items_xml_name_names_each_list_element() {
    for array_name in ["", "xml: { name: aliens }"] {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /docs:
    post:
      operationId: putDocument
      requestBody:
        content:
          application/xml: {{ schema: {{ $ref: '#/components/schemas/Document' }} }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
    Document:
      type: object
      xml: {{ name: document }}
      properties:
        animals:
          type: array
          {array_name}
          items:
            type: string
            xml: {{ name: animal }}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
        assert!(
            code.contains(r#"rename = "animal","#),
            "{array_name}: {code}"
        );
        assert!(!code.contains("aliens"), "{array_name}: {code}");
    }
}

/// A component struct reached as an array property's items names each list element by its own
/// root `xml.name`, as OpenAPI 3.2's "XML Arrays" reads a `$ref` item (node type `none`, so the
/// referenced schema produces its element). quick-xml writes a nested struct under the field's
/// name and never reads the struct's container rename there, so the root name has to become the
/// field's: without it each element went on the wire as `<pets>` while `check` printed `clean`.
/// A name written beside the `$ref` on the items still wins, and an owner that also travels as
/// JSON keeps its field name under the shared-type `W006`, like every other field rename. An owner
/// no XML body reaches derives no rename at all, so the root name alone raises no `W006` there.
#[test]
fn a_component_root_name_names_each_element_of_an_array_property_of_it() {
    let pet = "Pet: { type: object, xml: { name: pet }, properties: { id: { type: integer } } }";
    for (version, pets, list, element) in [
        (
            "3.1.0",
            "{ type: array, items: { $ref: '#/components/schemas/Pet' } }",
            "",
            "pet",
        ),
        (
            "3.2.0",
            "{ type: array, items: { $ref: '#/components/schemas/Pet' } }",
            "",
            "pet",
        ),
        (
            "3.2.0",
            "{ $ref: '#/components/schemas/PetList' }",
            "PetList: { type: array, items: { $ref: '#/components/schemas/Pet' } }",
            "pet",
        ),
        (
            "3.2.0",
            "{ type: array, items: { $ref: '#/components/schemas/Pet', xml: { name: critter } } }",
            "",
            "critter",
        ),
    ] {
        for reach in ["xml", "shared", "json"] {
            let body_media = if reach == "json" {
                "application/json"
            } else {
                "application/xml"
            };
            let json_use = if reach == "shared" {
                "
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Owner' } }"
            } else {
                ""
            };
            let spec = format!(
                r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /owners:
    post:
      requestBody:
        content:
          {body_media}: {{ schema: {{ $ref: '#/components/schemas/Owner' }} }}
      responses:
        '204': {{ description: ok }}{json_use}
components:
  schemas:
    Owner:
      type: object
      properties:
        pets: {pets}
    {pet}
    {list}
"##
            );
            let case = format!("{version} {pets} reach={reach}");
            let (report, code) = generate_with_code(&spec);
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            let field_rename = format!(r#"rename = "{element}","#);
            match reach {
                "shared" => {
                    assert!(
                        messages_for(&report, Code::XmlHintIgnored)
                            .iter()
                            .any(|m| m.contains("shared between an XML body")),
                        "{case}: {report:#?}"
                    );
                    assert!(!code.contains(&field_rename), "{case}: {code}");
                }
                "json" => {
                    // The root name alone derives no rename on an owner no XML body reaches, so it
                    // has nothing to warn about; a name written on the items is a hint like any
                    // other and keeps its `W006`.
                    let written = pets.contains("critter");
                    assert_eq!(
                        has_code(&report, Code::XmlHintIgnored),
                        written,
                        "{case}: {report:#?}"
                    );
                    assert!(!code.contains(&field_rename), "{case}: {code}");
                }
                _ => {
                    assert!(
                        !has_code(&report, Code::XmlHintIgnored),
                        "{case}: {report:#?}"
                    );
                    assert!(code.contains(&field_rename), "{case}: {code}");
                }
            }
            assert!(
                struct_attributes(&code, "Pet").contains(&r#"#[serde(rename = "pet")]"#.to_owned()),
                "{case}: {code}"
            );
        }
    }
}

/// An `items` hint the codec cannot write, on a list an XML body serializes: rejected, and named as
/// the items' hint so the author can find it.
#[test]
fn e009_an_unsupported_items_hint_on_an_xml_body_is_rejected() {
    for (version, items_xml) in [
        ("3.1.0", "{ namespace: 'urn:x' }"),
        ("3.1.0", "{ prefix: p }"),
        ("3.1.0", "{ attribute: true }"),
        ("3.2.0", "{ nodeType: cdata }"),
    ] {
        let spec = format!(
            r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /docs:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              xml: {{ name: document }}
              properties:
                animals:
                  type: array
                  items: {{ type: string, xml: {items_xml} }}
      responses:
        '204': {{ description: ok }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{items_xml}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnsupportedMediaType);
            assert!(
                messages.iter().any(|m| m.contains("`items.")),
                "{items_xml}: {report:#?}"
            );
        }
    }
}

/// An `xml` object beside a bare `$ref` names the referenced type's element at one use, but that
/// type is shared with every other use, so no rename can honour it: `E009` on an XML body (here the
/// specification's own "named root element set at the point of use" example and its 3.1 spelling),
/// and on a component alias, whose `$ref` resolves to the same shared type.
#[test]
fn e009_an_xml_name_beside_a_bare_ref_is_rejected() {
    for (version, schema, alias) in [
        (
            "3.2.0",
            "{ $ref: '#/components/schemas/Documentation', xml: { nodeType: element, name: StoredDocument } }",
            "",
        ),
        (
            "3.1.0",
            "{ $ref: '#/components/schemas/Documentation', xml: { name: StoredDocument } }",
            "",
        ),
        (
            "3.1.0",
            "{ $ref: '#/components/schemas/Stored' }",
            "Stored: { $ref: '#/components/schemas/Documentation', xml: { name: StoredDocument } }",
        ),
    ] {
        let spec = format!(
            r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /docs:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema: {schema}
components:
  schemas:
    {alias}
    Documentation:
      type: object
      properties:
        content: {{ type: string }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
            let messages = messages_for(&report, Code::UnsupportedMediaType);
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("`name`") && m.contains("beside a `$ref`")),
                "{schema}: {report:#?}"
            );
        }
    }
    // Under a JSON body nothing is serialized as XML, so the same hint only warns.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /docs:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Documentation', xml: { name: StoredDocument } }
components:
  schemas:
    Documentation:
      type: object
      properties:
        content: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        messages_for(&report, Code::XmlHintIgnored)
            .iter()
            .any(|m| m.contains("beside a `$ref`")),
        "{report:#?}"
    );
}

/// The same `items` hint where no XML body reaches the list warns instead.
#[test]
fn w006_an_unsupported_items_hint_on_a_never_xml_type_warns() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /docs:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                animals:
                  type: array
                  items: { type: string, xml: { namespace: 'urn:x' } }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::XmlHintIgnored);
        assert!(
            messages.iter().any(|m| m.contains("`items.namespace`")),
            "{report:#?}"
        );
    }
}
