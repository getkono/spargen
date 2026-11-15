//! Binary content a serde codec carries (#1417): `format: binary` read as the `String` its wire
//! holds inside a JSON, XML or form body and kept as bytes where the bytes are the wire, and
//! `contentEncoding: base64url` and the other `contentEncoding` values.

use super::*;

/// The Rust type of the field `field` names, with every alias on the way expanded: `Vec<T>`'s item
/// as well.
fn resolved_field_type(code: &str, field: &str) -> Option<String> {
    fn resolve(code: &str, ty: &str) -> String {
        if let Some(item) = ty
            .strip_prefix("Vec<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            return format!("Vec<{}>", resolve(code, item));
        }
        match alias_target(code, ty) {
            Some(target) => resolve(code, &target),
            None => ty.to_owned(),
        }
    }
    field_type(code, field).map(|ty| resolve(code, &ty))
}

/// The `W001`s a run reports for reading `format: binary` as text.
fn binary_format_warnings(report: &Report) -> usize {
    messages_for(report, Code::ValidationKeywordIgnored)
        .iter()
        .filter(|message| message.contains("`format: binary` is an annotation"))
        .count()
}

/// `format: binary` is an annotation in OpenAPI 3.1 and 3.2 (it has no effect on how a value is
/// encoded), so inside a serde-encoded body — a JSON member, an array item, a form field — the
/// value is the JSON string the wire carries, a `String`, and the reading is reported as `W001`
/// (#1417). `bytes::Bytes` there would be written as an array of numbers.
// W001 case: binary-format
#[test]
fn w001_format_binary_inside_a_serde_body_is_a_string() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /json:
    post:
      operationId: sendJson
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [data, chunks]
              properties:
                data: { type: string, format: binary }
                chunks: { type: array, items: { type: string, format: binary } }
      responses: { "204": { description: ok } }
  /form:
    post:
      operationId: sendForm
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              required: [blob]
              properties:
                blob: { type: string, format: binary }
      responses: { "204": { description: ok } }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        resolved_field_type(&code, "pub data:").as_deref(),
        Some("String"),
        "{code}"
    );
    assert_eq!(
        resolved_field_type(&code, "pub chunks:").as_deref(),
        Some("Vec<String>"),
        "{code}"
    );
    assert_eq!(
        resolved_field_type(&code, "pub blob:").as_deref(),
        Some("String"),
        "{code}"
    );
    let types = types_module(&code);
    assert!(!types.contains("bytes::Bytes"), "{types}");
    assert_eq!(binary_format_warnings(&report), 3, "{report:#?}");
}

/// The bytes are the wire itself in a whole body of any media and in a multipart part, so there
/// `format: binary` stays raw `bytes::Bytes` with no warning; one component used both as an
/// octet-stream body and as a JSON member is `bytes::Bytes` in the body and `String` in the member
/// (#1417).
#[test]
fn format_binary_stays_raw_bytes_where_the_bytes_are_the_wire() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    post:
      operationId: putBlob
      requestBody:
        required: true
        content:
          application/octet-stream:
            schema: { $ref: "#/components/schemas/Blob" }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [echo]
                properties:
                  echo: { $ref: "#/components/schemas/Blob" }
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
                file: { type: string, format: binary }
      responses: { "204": { description: ok } }
components:
  schemas:
    Blob: { type: string, format: binary }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("pub type Blob = bytes::Bytes;"), "{code}");
    assert!(
        flat.contains("body: impl Into<support::HttpBody>,"),
        "{code}"
    );
    assert_eq!(
        resolved_field_type(&code, "pub file:").as_deref(),
        Some("bytes::Bytes"),
        "{code}"
    );
    assert_eq!(
        resolved_field_type(&code, "pub echo:").as_deref(),
        Some("String"),
        "{code}"
    );
    assert_eq!(
        binary_format_warnings(&report),
        1,
        "only the JSON member is reported: {report:#?}"
    );
}

/// `contentEncoding: base64url` (RFC 4648 §5), which OpenAPI 3.2 names beside `base64`, is typed
/// bytes as `base64` is: a newtype whose serde form is the URL-safe text, and it wins over a
/// `format: binary` beside it (#1417).
#[test]
fn content_encoding_base64url_is_a_bytes_newtype() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [plain, marked]
              properties:
                plain: { type: string, contentEncoding: base64url }
                marked: { type: string, format: binary, contentEncoding: base64url }
      responses: { "204": { description: ok } }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    for newtype in ["UploadBodyPlain", "UploadBodyMarked"] {
        assert!(
            flat.contains(&format!("pub struct {newtype}(pub bytes::Bytes);")),
            "{code}"
        );
    }
    assert!(flat.contains("support::serialize_base64url("), "{code}");
    assert!(flat.contains("support::deserialize_base64url("), "{code}");
    assert!(!flat.contains("support::serialize_base64("), "{code}");
}

/// A `contentEncoding` other than `base64` or `base64url` (`base32`, `quoted-printable`, ...) has
/// no codec here, so the value stays the `String` the wire carries and the keyword is reported
/// rather than dropped in silence (#1417).
// W001 case: content-encoding
#[test]
fn w001_an_unsupported_content_encoding_is_reported() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Holder:
      type: object
      required: [text]
      properties:
        text: { type: string, contentEncoding: base32 }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        resolved_field_type(&code, "pub text:").as_deref(),
        Some("String"),
        "{code}"
    );
    assert!(
        messages_for(&report, Code::ValidationKeywordIgnored)
            .iter()
            .any(|message| message
                .contains("`contentEncoding: base32` names no encoding spargen decodes")),
        "{report:#?}"
    );
}

/// A union variant is a serde position too, so its `format: binary` branch is the `String` its
/// wire holds wherever it stands (#1417). Beside a plain string branch of a `oneOf` the two are
/// then variants no value tells apart, so they merge as any two `String` branches do, reported as
/// `W001` at the union (#1511): the whole union is `String` where every branch merges, one
/// variant of the enum where only some do, and a union holding such a union merges in turn. An
/// `anyOf` keeps both, since one match decodes it.
// W001 case: indistinguishable-branches
#[test]
fn a_binary_union_variant_is_a_string_and_merges_with_a_string_branch() {
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed: { oneOf: [{ type: string, format: binary }, { type: integer }] }
    Alike: { oneOf: [{ type: string, format: binary }, { type: string }] }
    Partly: { oneOf: [{ type: string, format: binary }, { type: string }, { type: integer }] }
    Nested:
      oneOf:
        - oneOf: [{ type: string, format: binary }, { type: string }]
        - { type: string }
    Either: { anyOf: [{ type: string, format: binary }, { type: string }] }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = types_module(&code);
    assert_eq!(
        alias_target(&types, "MixedVariant0").as_deref(),
        Some("String"),
        "{types}"
    );
    for merged in ["Alike", "Nested"] {
        assert_eq!(
            alias_target(&types, merged).as_deref(),
            Some("String"),
            "{merged}: {types}"
        );
    }
    assert_eq!(enum_variants(&types, "Partly").len(), 2, "{types}");
    assert_eq!(enum_variants(&types, "Either").len(), 2, "{types}");
    assert!(
        !types.contains("bytes::Bytes"),
        "no serde position keeps raw bytes: {types}"
    );
    for (union, warned) in [
        ("Alike", true),
        ("Partly", true),
        ("Nested", true),
        ("Mixed", false),
        ("Either", false),
    ] {
        let at = format!("/components/schemas/{union}");
        assert_eq!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == at
                    && d.message.contains("lower to the same generated type")
            }),
            warned,
            "{union}: {report:#?}"
        );
    }
}
