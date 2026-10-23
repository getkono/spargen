//! Request and response bodies, second part: `format: binary` in every position, optional request
//! bodies, the basic `W014` alternative pair, byte bodies across the binary media families and
//! their ranking against media ranges, CBOR as a byte body, and nullable raw bodies.

use super::*;

#[test]
fn binary_format_in_param_and_text_body_positions_generate() {
    // Regression guard for `format: binary` → `bytes::Bytes` in positions rendered as strings: a
    // binary PATH param, a binary QUERY param, and a `text/plain` body of `format: binary` must all
    // generate cleanly (the e2e suite compile-verifies they do not silently miscompile). `Bytes` is
    // not `Display`; params are remapped to `String` and a Bytes body is sent raw, never `.to_string`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob/{token}:
    get:
      parameters:
        - name: token
          in: path
          required: true
          schema: { type: string, format: binary }
        - name: cursor
          in: query
          schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
  /raw:
    post:
      requestBody:
        required: true
        content:
          text/plain:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    // The `bytes::Bytes` aliases the two parameters lowered to are replaced by their `String`
    // copies, so nothing names them and they are not emitted (#927).
    for alias in ["Token", "Cursor"] {
        assert_eq!(alias_target(&code, alias), None, "{code}");
    }
}

#[test]
fn binary_format_in_content_parameters_emits_no_bytes_alias() {
    // The `content:` parameter call site of the same elision (#927): a JSON and a `text/*`
    // `content` parameter whose schema lowers to `bytes::Bytes` take its `String` copy, so the
    // bytes alias the schema lowered to is named by nothing and is not emitted.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      parameters:
        - name: filter
          in: query
          content:
            application/json:
              schema: { type: string, format: binary }
        - name: tag
          in: header
          content:
            text/plain:
              schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for alias in ["Filter", "Tag"] {
        assert_eq!(alias_target(&code, alias), None, "{code}");
    }
    assert!(!code.contains("= bytes::Bytes;"), "{code}");
}

#[test]
fn a_binary_parameter_keeps_the_component_it_refs_emitted() {
    // Eliding the bytes type a binary parameter's `String` copy replaces (#927) must not reach a
    // `$ref`'d component: the parameter lowers it first, and its memo hands the same type to the
    // body that names it afterwards.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob/{token}:
    post:
      parameters:
        - name: token
          in: path
          required: true
          schema: { $ref: "#/components/schemas/Blob" }
      requestBody:
        required: true
        content:
          application/octet-stream:
            schema: { $ref: "#/components/schemas/Blob" }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Blob: { type: string, format: binary }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        alias_target(&code, "Blob").as_deref(),
        Some("bytes::Bytes"),
        "{code}"
    );
}

#[test]
fn optional_request_bodies_take_an_option_argument() {
    // `requestBody.required` was dropped entirely, so an optional body was indistinguishable from
    // a required one and the caller had to invent a value.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /required:
    post:
      operationId: postRequired
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
      responses:
        "204": { description: No Content }
  /optional:
    post:
      operationId: postOptional
      requestBody:
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("body: Option<&"),
        "an optional body is passed as an Option: {code}"
    );
    assert!(
        code.contains("body: &"),
        "a required body is passed by reference: {code}"
    );
}

#[test]
fn w014_alternative_media_type_is_not_generated() {
    // A generated method sends and decodes exactly one media type, so a body offering both JSON and
    // XML narrows to JSON. That is a real reduction of the documented surface and used to happen
    // with no diagnostic at all — the one silent disposition left in the media path.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: string }
          application/xml:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn a_single_media_type_draws_no_alternative_warning() {
    // The warning must fire only when something is actually dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn an_empty_schema_on_a_binary_media_type_is_a_byte_body() {
    // OpenAPI 3.1 aligned Schema Objects with JSON Schema 2020-12 and removed `format: binary`, so
    // `schema: {}` — or no schema at all — is how a 3.1 document says "any octets" on a media type
    // that already carries the meaning. It used to be rejected with `E009` for not being
    // `type: string`, which is the 3.0 spelling 3.1 retired.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    post:
      operationId: putBlob
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {}
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    // The point of the fix: a byte body must not degrade to `serde_json::Value`, and retyping the
    // untyped schema must not leave a second alias behind.
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        !code.contains("= serde_json::Value;"),
        "an octet body must not lower to an untyped value: {code}"
    );
}

#[test]
fn retyping_a_binary_body_never_rewrites_a_shared_component() {
    // Retyping the untyped body replaces the definition in place when it is anonymous. A childless
    // component (`Opaque: {}`) is lifted into its reserved id and is then the graph's *last*
    // definition too, so "last inserted" alone cannot tell the two apart — and rewriting a named
    // component would silently retype it for every other reference in the document.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200":
          description: OK
          content:
            application/octet-stream:
              schema: { $ref: "#/components/schemas/Opaque" }
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Opaque" }
components:
  schemas:
    Opaque: {}
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type Opaque = serde_json::Value;"),
        "the component must stay exactly as declared: {code}"
    );
    assert!(!code.contains("pub type Opaque = bytes::Bytes;"), "{code}");
}

#[test]
fn a_media_range_is_generated_as_the_family_it_names() {
    // Media *ranges* are permitted `content` keys. They classified as nothing, so a response body
    // whose only entry was `video/*` was rejected outright rather than read as its family.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: OK
          content:
            video/*: { schema: {} }
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn a_concrete_image_audio_or_video_type_is_a_byte_body() {
    // #82's second finding: `image/*` was accepted while `image/jpeg` — the same family, named
    // exactly — still fell to `E009`. A concrete member of a family that RFC 6838 reserves for
    // non-textual data is opaque octets under the same gate as `application/octet-stream`, in
    // both 3.1 spellings (`schema: {}`, no schema) and the 3.0 one (`format: binary`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: {} }
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: OK
          content:
            video/mp4: { schema: { type: string, format: binary } }
  /track:
    get:
      operationId: getTrack
      responses:
        "200":
          description: OK
          content:
            audio/mpeg: {}
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert_eq!(code.matches("= bytes::Bytes;").count(), 3, "{code}");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

#[test]
fn a_concrete_binary_request_body_is_sent_as_its_own_content_type() {
    // Unlike a range, `image/png` is a dispatchable `Content-Type`, so it is accepted as a request
    // body and the header names it verbatim rather than `application/octet-stream`.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
}

#[test]
fn octet_stream_outranks_a_concrete_binary_type_listed_before_it() {
    // A concrete family member ranks just *below* `application/octet-stream`, so a document that
    // generated before the family rule existed keeps its selection: here `image/png` came first
    // and was merely an unsupported alternative, and it still is one. Octet-stream is selected,
    // the body is `bytes::Bytes`, and `image/png` — whose `format: byte` schema constrains
    // something, so it is not proved to decode identically — is reported as ignored (`W014`),
    // never rejected by the octet gate (`E009`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork:
    get:
      operationId: getArtwork
      responses:
        "200":
          description: OK
          content:
            image/png: { schema: { type: string, format: byte } }
            application/octet-stream: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`application/octet-stream` is selected")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );

    // The same tie on the request side decides the wire `Content-Type`: octet-stream wins there
    // too, so the header a client already sent does not switch to the family type.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
          application/octet-stream: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"application/octet-stream\""), "{code}");
    assert!(!code.contains("\"image/png\""), "{code}");
}

#[test]
fn a_concrete_binary_type_ranks_last_except_against_a_request_range() {
    // A concrete family member sits at the very end of the ladder, below every other key spargen
    // can classify — octet-stream, text, the sequential kinds, and on a response every range. Each
    // document here but (iv) generated before the family rule existed with `image/png` as an
    // unsupported alternative; its selection, body type, outcome and wire `Content-Type` must not
    // move now that `image/png` classifies. The exception is a request offering a range, which it
    // cannot send as `Content-Type`: that was rejected before, and now sends the concrete key.

    // (i) Text keeps a response: `String`, and `image/png` is the alternative not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/csv: { schema: { type: string } }
            image/png: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`text/csv` is selected")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );

    // (ii) Text keeps a request, and with it the header a client already sent.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    put:
      operationId: putNote
      requestBody:
        required: true
        content:
          text/plain: { schema: { type: string } }
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("\"text/plain\""), "{code}");
    assert!(!code.contains("\"image/png\""), "{code}");

    // (iii) A sequential kind keeps a response: the stream is still the selection.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              itemSchema: { type: object, required: [seq], properties: { seq: { type: integer } } }
            image/png: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );

    // (iv) A range is the one key a request cannot send — `Content-Type` must be concrete — so
    // on a request it is considered only once no concrete key classifies. Beside `image/png` the
    // concrete key is sent and the range is reported as the alternative not generated, rather
    // than the whole operation being rejected because an unsendable alternative was listed.
    for range in ["video/*", "*/*", "text/*"] {
        let range_schema = if range == "text/*" {
            "{ type: string }"
        } else {
            "{}"
        };
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          "{range}": {{ schema: {range_schema} }}
          image/png: {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{range}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{range}: {report:#?}"
        );
        assert!(
            code.contains("pub type RequestBody = bytes::Bytes;"),
            "{range}: {code}"
        );
        assert!(code.contains("\"image/png\""), "{range}: {code}");
        assert!(!code.contains(&format!("\"{range}\"")), "{range}: {code}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is selected")
                    && d.message.contains(&format!("`{range}`"))
            }),
            "{range}: {report:#?}"
        );
    }

    // ... but only a concrete key that classifies: `application/pdf` names no codec, so the range
    // is still the selection and the request is still rejected for naming no sendable
    // `Content-Type`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          application/pdf: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains("`video/*` is a media range")
            }),
            "{report:#?}"
        );
    }

    // ... and the preferred concrete key still meets the octet gate: an object under `image/png`
    // is rejected on its own account, not waved through because a range stood beside it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          image/png: { schema: { type: object, properties: { a: { type: string } } } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message
                        .contains("`image/png` requires a string-like or binary schema")
            }),
            "{report:#?}"
        );
    }

    // (v) The `text/*` range keeps a response too: it generates as text beside an `image/png` of
    // another family, and a concrete family member ranks below it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            image/png: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`text/*` is selected")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );

    // (vi) ... whatever schema `image/png` carries: a losing alternative never reaches the octet
    // gate, so an object under `image/png` beside `text/*` generates as text instead of `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            image/png: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");

    // (vii) `*/*` keeps a response the same way: it generates as bytes beside an `image/png`
    // whatever schema `image/png` carries — an object there is reported as the alternative not
    // generated (`W014`), never rejected by the octet gate (`E009`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            image/png: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`*/*` is selected")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );

    // (viii) The family's own range outranks its concrete member: `image/*` beside `image/png`
    // with a constraining schema generates from the range.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/*: { schema: {} }
            image/png: { schema: { type: string, format: byte } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn a_request_prefers_a_concrete_binary_key_listed_before_a_range() {
    // Source order is only the last tie-break: a concrete `image/png` listed *before* `video/*`
    // is sent for the same reason it is when listed after — a range is no `Content-Type` — and
    // the range is still reported as the alternative not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
    assert!(!code.contains("\"video/*\""), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is selected")
                    && d.message.contains("`video/*`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_concrete_binary_key_is_the_sendable_sibling_that_withholds_a_suffix_range() {
    // A structured-suffix range such as `application/*+json` is withheld from a request's choice
    // while a sibling can be sent. A concrete `image/png` is such a sibling: it classifies, is no
    // range, and is not streaming. So the request sends `image/png`, and the withheld range is
    // reported as the alternative not generated rather than selected and then refused.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          application/*+json: { schema: { type: object } }
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
    assert!(!code.contains("\"application/*+json\""), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is selected")
                    && d.message.contains("`application/*+json`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_request_offering_only_ranges_is_rejected_on_the_first() {
    // With no concrete key at all every candidate is a range, so the ladder and then source order
    // decide as before: `video/*` ties `*/*` at the same rank and, listed first, is selected, and
    // the selection is then rejected as a request `Content-Type` (`E009`). The rejection alone
    // reports the refused selection: no `W014` names `*/*` as passed over for it (#110).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          "*/*": { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let rejections: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnsupportedMediaType)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            rejections,
            [
                "media type `video/*` is a media range, which describes a family rather than the \
                 concrete `Content-Type` a request must send"
            ],
            "{report:#?}"
        );
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

/// The generated `pub async fn {name}` method, from its signature to the next method.
fn method_source<'a>(code: &'a str, name: &str) -> &'a str {
    let head = format!("pub async fn {name}(");
    let start = code.find(&head).unwrap_or_else(|| panic!("{head}\n{code}"));
    let rest = &code[start..];
    let end = rest[head.len()..]
        .find("pub async fn ")
        .map_or(rest.len(), |end| end + head.len());
    &rest[..end]
}

#[test]
fn cbor_and_cbor_suffixed_bodies_are_byte_bodies_announced_by_their_own_name() {
    // #900: CBOR has no codec here, and needs none: its bodies are handed over byte for byte. So
    // `application/cbor` and any `+cbor` suffix lower exactly as `application/octet-stream` does,
    // except that the key itself, not `application/octet-stream`, goes on the wire as the request
    // `Content-Type` and the response `Accept`.
    let operation = |path: &str, id: &str, media: &str| {
        format!(
            r##"
  /{path}:
    post:
      operationId: {id}
      requestBody:
        required: true
        content:
          {media}: {{ schema: {{ type: string, format: binary }} }}
      responses:
        "200":
          description: OK
          content:
            {media}: {{ schema: {{ type: string, format: binary }} }}"##
        )
    };
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths:{}{}{}{}\n",
        operation("octet", "sendOctets", "application/octet-stream"),
        operation("cbor", "sendCbor", "application/cbor"),
        operation("cose", "sendCose", "application/cose+cbor"),
        operation("shouted", "sendShouted", "Application/CBOR"),
    );
    let (report, code) = generate_with_code(&spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let octets = method_source(&code, "send_octets");
    assert!(!octets.contains("ACCEPT"), "{octets}");
    // The signature with each `types::` alias spelled as what it aliases, since every operation's
    // body aliases are named for itself.
    let signature = |method: &str| {
        let mut signature = method[..method.find('{').unwrap()].to_owned();
        while let Some(start) = signature.find("types::") {
            let name_start = start + "types::".len();
            let name_end = signature[name_start..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map_or(signature.len(), |end| name_start + end);
            let target = alias_target(&code, &signature[name_start..name_end])
                .unwrap_or_else(|| panic!("an alias in {signature}"));
            signature.replace_range(start..name_end, &target);
        }
        signature
    };
    assert!(
        signature(octets).contains("body: &bytes::Bytes"),
        "{octets}"
    );
    for (name, error, key) in [
        ("send_cbor", "SendCborError", "application/cbor"),
        ("send_cose", "SendCoseError", "application/cose+cbor"),
        ("send_shouted", "SendShoutedError", "Application/CBOR"),
    ] {
        let method = method_source(&code, name);
        // The same byte-body shape the octet-stream operation takes and returns.
        assert_eq!(
            signature(method).replacen(name, "send_octets", 1).replacen(
                error,
                "SendOctetsError",
                1
            ),
            signature(octets),
            "{method}"
        );
        assert!(
            method.contains(&format!("reqwest::header::CONTENT_TYPE, \"{key}\"")),
            "{method}"
        );
        assert!(
            method.contains(&format!("reqwest::header::ACCEPT, \"{key}\"")),
            "{method}"
        );
        assert!(!method.contains("application/octet-stream"), "{method}");
    }
}

#[test]
fn e009_a_cbor_body_still_needs_a_binary_schema() {
    // A schema'd CBOR document would need a CBOR codec, which spargen does not have: an object
    // under `application/cbor` meets the octet gate's refusal, on either side, as it would under
    // `application/octet-stream`.
    for (position, document) in [
        (
            "request",
            r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /doc:
    put:
      operationId: putDoc
      requestBody:
        required: true
        content:
          application/cbor: { schema: { type: object, properties: { a: { type: string } } } }
      responses:
        "204": { description: No Content }
"##,
        ),
        (
            "response",
            r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            application/foo+cbor: { schema: { type: object, properties: { a: { type: string } } } }
"##,
        ),
    ] {
        for report in [generate(document), check(document)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{position}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::UnsupportedMediaType
                        && d.message.contains("requires a string-like or binary")
                }),
                "{position}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_reports_what_a_cbor_response_asks_against() {
    // A selected CBOR response is announced in `Accept`, so a byte-identical alternative is asked
    // against at the wire and is reported; one selected ahead of a CBOR alternative sends no
    // `Accept`, and narrows nothing.
    let document = |first: &str, second: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            {first}: {{ schema: {{}} }}
            {second}: {{ schema: {{}} }}
"##
        )
    };
    let report = generate(&document("application/cbor", "application/octet-stream"));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::AlternativeMediaIgnored),
        [
            "`application/cbor` is selected; the alternative media type(s) \
          `application/octet-stream` are not"
        ],
        "{report:#?}"
    );
    let report = generate(&document("application/octet-stream", "application/cbor"));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );

    // An error response's CBOR key is never announced in `Accept`, and its alternatives are
    // reported all the same: the selection is made per response, before `Accept` is known.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /doc:
    get:
      operationId: getDoc
      responses:
        "204": { description: No Content }
        "400":
          description: Bad
          content:
            application/cbor: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::AlternativeMediaIgnored),
        [
            "`application/cbor` is selected; the alternative media type(s) \
          `application/octet-stream` are not"
        ],
        "{report:#?}"
    );
    let method = method_source(&code, "get_doc");
    assert!(!method.contains("ACCEPT"), "{method}");
}

#[test]
fn cbor_is_a_concrete_cbor_suffix_under_any_type_and_never_a_suffix_range() {
    // A `+cbor` subtype counts under any top-level type, not only `application/`; a suffix
    // *range* names no concrete type and stays unsupported.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /doc:
    post:
      operationId: sendDoc
      requestBody:
        required: true
        content:
          text/foo+cbor: { schema: { type: string, format: binary } }
      responses:
        "200":
          description: OK
          content:
            text/foo+cbor: { schema: { type: string, format: binary } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let method = method_source(&code, "send_doc");
    assert!(
        method.contains("reqwest::header::CONTENT_TYPE, \"text/foo+cbor\""),
        "{method}"
    );
    assert!(
        method.contains("reqwest::header::ACCEPT, \"text/foo+cbor\""),
        "{method}"
    );

    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            application/*+cbor: { schema: { type: string, format: binary } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType && d.message.contains("application/*+cbor")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_is_silent_across_concrete_and_ranged_byte_bodies() {
    // `image/jpeg`, `image/png`, `image/*` and `application/octet-stream` over empty schemas are one
    // representation four times over; picking one narrows nothing.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork:
    get:
      operationId: getArtwork
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: {} }
            image/png: { schema: {} }
            image/*: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_a_concrete_binary_request_alternative() {
    // A request sends exactly one `Content-Type`, so a server documented as also accepting
    // `image/png` is narrowed at the wire whatever the decoded type — the octet exemption is a
    // response rule, and a concrete family alternative must not slip into it on a request.
    for (selection, alternative) in [
        ("application/octet-stream", "image/png"),
        ("image/jpeg", "image/png"),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          {selection}: {{ schema: {{}} }}
          {alternative}: {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::AlternativeMediaIgnored
                        && d.message.contains(&format!("`{selection}` is selected"))
                        && d.message.contains(&format!("`{alternative}`"))
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn a_concrete_binary_type_is_matched_case_insensitively() {
    // `IMAGE/*` already reads as its family; `IMAGE/JPEG` must agree with it.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            IMAGE/JPEG: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn e009_a_concrete_binary_type_still_needs_a_binary_schema() {
    // The family says "octets"; the schema still has to agree. An object under `image/jpeg` is
    // rejected by the octet gate exactly as it is under `application/octet-stream`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");

    // The request gate is reached through the same octet classification, and must name the
    // schema it refuses rather than call the family unsupported.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    put:
      operationId: putPhoto
      requestBody:
        required: true
        content:
          image/jpeg: { schema: { type: object, properties: { a: { type: string } } } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message
                        .contains("requires a string-like or binary schema")
            }),
            "{report:#?}"
        );
    }
}

/// The raw-body schemas that admit `null` (#104), one per spelling: a union with `null` over
/// `format: binary`, over `contentEncoding: base64`, and over a `$ref` to a binary component, the
/// `anyOf` form, and the type-array form `type: [string, 'null']`, which used to lose its `null`
/// before any gate saw it.
const NULLABLE_BYTE_SCHEMAS: &[&str] = &[
    "{ oneOf: [ { type: string, format: binary }, { type: 'null' } ] }",
    "{ anyOf: [ { type: string, format: binary }, { type: 'null' } ] }",
    "{ oneOf: [ { type: string, contentEncoding: base64 }, { type: 'null' } ] }",
    "{ oneOf: [ { $ref: '#/components/schemas/Blob' }, { type: 'null' } ] }",
    "{ type: [string, 'null'], format: binary }",
    "{ type: [string, 'null'], contentEncoding: base64 }",
    "{ $ref: '#/components/schemas/NullableBlob' }",
];

const NULLABLE_BYTE_COMPONENTS: &str = r#"
components:
  schemas:
    Blob: { type: string, format: binary }
    NullableBlob: { type: [string, 'null'], format: binary }
"#;

fn assert_rejected_as_nullable_raw_body(spec: &str) {
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{spec}\n{report:#?}");
        assert!(
            messages_for(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message.contains("has no wire representation of `null`")),
            "{spec}\n{report:#?}"
        );
    }
}

#[test]
fn e009_a_nullable_raw_request_body_is_rejected() {
    // A raw request body is sent verbatim as octets, and octets carry no `null`: an
    // `Option<bytes::Bytes>` argument would ask the caller for a value the wire cannot send (the
    // absent body is `required: false`, which is a different construct). Every spelling used to
    // pass the frontend clean and generate `.body(body.clone())` over `Option<Bytes>`, which does
    // not compile (`E0277`) — under octet-stream, and under text and JSON too, because a `Bytes`
    // body is sent raw whatever its media.
    for version in ["3.1.0", "3.2.0"] {
        for media in ["application/octet-stream", "text/plain", "application/json"] {
            for required in [true, false] {
                for schema in NULLABLE_BYTE_SCHEMAS {
                    let spec = format!(
                        r#"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: {required}
        content:
          {media}: {{ schema: {schema} }}
      responses: {{ "204": {{ description: ok }} }}
{NULLABLE_BYTE_COMPONENTS}"#
                    );
                    assert_rejected_as_nullable_raw_body(&spec);
                }
            }
        }
    }

    // The raw text codec has the same hole: `text/plain` over a nullable string used to emit
    // `.body(body.to_string())` over `&Option<String>`, which does not compile (`E0599`).
    for schema in [
        "{ oneOf: [ { type: string }, { type: 'null' } ] }",
        "{ type: [string, 'null'] }",
    ] {
        let spec = format!(
            r#"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          text/plain: {{ schema: {schema} }}
      responses: {{ "204": {{ description: ok }} }}
"#
        );
        assert_rejected_as_nullable_raw_body(&spec);
    }
}

#[test]
fn e009_a_nullable_byte_response_body_is_rejected() {
    // A byte response is read as the raw octets of the body, so `null` is never what arrives; the
    // byte decoder has no `Option` to build, and the union spelling used to generate a
    // `ResponseValue<Bytes>` where `ResponseValue<Option<Bytes>>` was declared (`E0308`), while the
    // type-array spelling silently decoded plain `Bytes`. Success and error bodies, single and
    // multi-status, go through the same gate.
    let responses = [
        r#"{ "200": { description: ok, content: { MEDIA: { schema: SCHEMA } } } }"#,
        r#"{ "204": { description: ok }, "400": { description: bad, content: { MEDIA: { schema: SCHEMA } } } }"#,
        r#"{ "200": { description: ok, content: { MEDIA: { schema: SCHEMA } } }, "201": { description: ok, content: { application/json: { schema: { type: string } } } } }"#,
    ];
    for media in ["application/octet-stream", "text/plain", "application/json"] {
        for schema in NULLABLE_BYTE_SCHEMAS {
            for shape in responses {
                let responses = shape.replace("MEDIA", media).replace("SCHEMA", schema);
                let spec = format!(
                    r#"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /d:
    get:
      operationId: download
      responses: {responses}
{NULLABLE_BYTE_COMPONENTS}"#
                );
                assert_rejected_as_nullable_raw_body(&spec);
            }
        }
    }
}

#[test]
fn a_nullable_byte_string_keeps_its_null_outside_a_raw_body() {
    // `type: [string, 'null']` with `format: binary` or `contentEncoding: base64` is `bytes::Bytes`
    // that also admits `null`, exactly as the union spelling is. Where the value is a JSON member,
    // or a multipart part, `null` has a representation, so the field is `Option<bytes::Bytes>` —
    // it used to be plain `Bytes`, which rejects the very `null` the schema allows.
    let (report, code) = generate_with_code(
        r#"
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
              required: [typed, united]
              properties:
                typed: { type: [string, 'null'], contentEncoding: base64 }
                united: { oneOf: [ { type: string, contentEncoding: base64 }, { type: 'null' } ] }
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("pub type RequestBodytyped = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        code.contains("pub typed: Option<RequestBodytyped>,"),
        "{code}"
    );
    assert!(code.contains("pub united: Option<"), "{code}");

    // A nullable raw *text* response stays supported: the text codec decodes through serde, so
    // `Option<String>` is built (always `Some`) and compiles. Only the byte codec, which has no
    // `Option` to build, is rejected.
    let report = generate(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /d:
    get:
      operationId: download
      responses:
        "200": { description: ok, content: { text/plain: { schema: { type: [string, 'null'] } } } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
}
