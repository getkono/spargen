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
