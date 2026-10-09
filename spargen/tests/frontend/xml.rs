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
    // When a body offers both JSON and XML, media selection deterministically prefers JSON, so the
    // API does not use XML at all — generation succeeds with no E009.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema: { type: object }
          application/json:
            schema: { type: object, required: [id], properties: { id: { type: string } } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
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
