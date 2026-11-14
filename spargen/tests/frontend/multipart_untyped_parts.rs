//! Untyped `multipart/form-data` parts as raw bytes, and the `contentMediaType` positions that are
//! read rather than reported as `W001`.

use super::*;

/// An untyped multipart part, `{}` or `{contentMediaType: …}` alone, is a file part of raw bytes
/// (#1410): both OpenAPI 3.1 and 3.2 default a part with no `type` to `application/octet-stream`,
/// and 3.2's binary-data guidance spells a raw file part as `contentMediaType` with no `type`.
/// The `contentMediaType` is the part's default `Content-Type`, and is read there, so it draws
/// no `W001`.
#[test]
fn an_untyped_multipart_part_is_a_file_part_of_raw_bytes() {
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, raw]
              properties:
                file: { contentMediaType: image/png }
                raw: {}
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
    for expected in [
        "pub type UploadBodyFile = bytes::Bytes;",
        "pub type UploadBodyRaw = bytes::Bytes;",
        "support::MultipartPart::bytes(body.file.to_vec())",
        "support::MultipartPart::bytes(body.raw.to_vec())",
        ".mime_str(\"image/png\")",
        ".mime_str(\"application/octet-stream\")",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in {code}");
    }
}

/// An Encoding Object's `contentType` overrides the default a `contentMediaType` gives an untyped
/// multipart part, as it overrides any part's default.
#[test]
fn an_encoding_content_type_overrides_an_untyped_parts_content_media_type() {
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file: { contentMediaType: image/png }
            encoding:
              file: { contentType: image/jpeg }
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("support::MultipartPart::bytes(body.file.to_vec())"),
        "{code}"
    );
    assert!(code.contains(".mime_str(\"image/jpeg\")"), "{code}");
    assert!(!code.contains(".mime_str(\"image/png\")"), "{code}");
}

/// An untyped part that names an untyped *component* keeps the component as declared, sent as
/// JSON: retyping it would retype the component for every other reference in the document, as
/// `retyping_a_binary_body_never_rewrites_a_shared_component` holds for a body.
#[test]
fn an_untyped_multipart_part_never_retypes_a_shared_component() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [meta]
              properties:
                meta: { $ref: "#/components/schemas/Opaque" }
      responses: { "204": { description: ok } }
components:
  schemas:
    Opaque: {}
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("pub type Opaque = serde_json::Value;"),
        "{code}"
    );
    assert!(code.contains(".mime_str(\"application/json\")"), "{code}");
}

/// A body schema whose `contentMediaType` restates its media key says what the media type already
/// says, which OpenAPI 3.2 calls redundant rather than ignored, so it draws no `W001`.
#[test]
fn a_content_media_type_restating_its_body_media_type_draws_no_w001() {
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /image:
    put:
      operationId: putImage
      requestBody:
        required: true
        content:
          image/png:
            schema: { contentMediaType: image/png }
      responses:
        "200":
          description: ok
          content:
            Image/PNG:
              schema: { contentMediaType: image/png }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

/// A `contentMediaType` that names no one concrete, sendable media type (a wildcard, or a value
/// that is not a media type) cannot be a part's `Content-Type`: the part is still raw bytes, sent
/// as `application/octet-stream`, and the annotation spargen did not read keeps its `W001`, so no
/// request fails at send time on an unsendable header.
#[test]
fn an_untyped_part_with_an_unsendable_content_media_type_is_octets_and_keeps_its_w001() {
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [wild, malformed]
              properties:
                wild: { contentMediaType: "image/*" }
                malformed: { contentMediaType: "not a media type" }
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::ValidationKeywordIgnored).len(),
        2,
        "{report:#?}"
    );
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    for expected in [
        "pub type UploadBodyWild = bytes::Bytes;",
        "pub type UploadBodyMalformed = bytes::Bytes;",
        ".mime_str(\"application/octet-stream\")",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in {code}");
    }
    assert!(!code.contains(".mime_str(\"image/*\")"), "{code}");
    assert!(!code.contains("not a media type\")"), "{code}");
}

/// A `true` part admits any value, as `{}` does, so it is a file part of raw bytes too.
#[test]
fn a_true_multipart_part_is_a_file_part_of_raw_bytes() {
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [any]
              properties:
                any: true
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
    assert!(
        code.contains("support::MultipartPart::bytes(body.any.to_vec())"),
        "{code}"
    );
    assert!(
        code.contains(".mime_str(\"application/octet-stream\")"),
        "{code}"
    );
}

/// A part reached only through an `allOf` member is raw bytes, but its `contentMediaType` is not
/// read there: it is sent as `application/octet-stream` and keeps its `W001`. The member's own
/// component is not retyped, so its other uses keep the type it declares.
#[test]
fn an_all_of_reached_untyped_part_is_octets_and_leaves_its_component_alone() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              allOf:
                - $ref: "#/components/schemas/Base"
      responses: { "204": { description: ok } }
  /base:
    get:
      operationId: getBase
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Base" }
components:
  schemas:
    Base:
      type: object
      required: [image]
      properties:
        image: { contentMediaType: image/png }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("support::MultipartPart::bytes(body.image.to_vec())"),
        "{code}"
    );
    assert!(
        code.contains(".mime_str(\"application/octet-stream\")"),
        "{code}"
    );
    assert!(!code.contains(".mime_str(\"image/png\")"), "{code}");
    assert!(
        code.contains("pub type BaseImage = serde_json::Value;"),
        "the component's own field is retyped: {code}"
    );
}

/// A component used both as a multipart body and as a JSON response keeps its untyped field as
/// declared: the JSON position still decodes any value there. The multipart position takes a
/// struct of its own whose untyped part is raw bytes, so one use's lowering never retypes a
/// shared component.
#[test]
fn an_untyped_part_of_a_shared_component_body_never_retypes_its_json_uses() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /things:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema: { $ref: "#/components/schemas/Thing" }
      responses: { "204": { description: ok } }
    get:
      operationId: getThing
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Thing" }
components:
  schemas:
    Thing:
      type: object
      required: [meta]
      properties:
        meta: {}
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("pub type ThingMeta = serde_json::Value;"),
        "the component's field is retyped: {code}"
    );
    assert!(
        code.contains("support::MultipartPart::bytes(body.meta.to_vec())"),
        "{code}"
    );
    assert!(
        code.contains(".mime_str(\"application/octet-stream\")"),
        "{code}"
    );
}

/// A request body offering JSON and multipart generates only the entry lowering selects (JSON),
/// so a part's `contentMediaType` on the schema both entries share is never read, and keeps its
/// `W001` while the field stays `serde_json::Value`.
#[test]
fn a_content_media_type_on_an_unselected_multipart_entry_keeps_its_w001() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /things:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Thing" }
          multipart/form-data:
            schema: { $ref: "#/components/schemas/Thing" }
      responses: { "204": { description: ok } }
components:
  schemas:
    Thing:
      type: object
      required: [image]
      properties:
        image: { contentMediaType: image/png }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("pub type ThingImage = serde_json::Value;"),
        "{code}"
    );
}
