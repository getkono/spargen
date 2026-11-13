//! Request and response bodies that are `E009` whatever position they hold: a media type that
//! names no codec spargen has, inside or outside the byte families, a raw body whose schema its
//! codec cannot carry, and request-only media in a response, beside the textual, vendor and
//! structured-JSON media that generate.

use super::*;

// E009 case: no-codec
#[test]
fn e009_a_media_type_naming_no_codec_is_unsupported() {
    // A genuinely unsupported media (`application/pdf`) still rejects with E009 — the narrowing only
    // added JSON-adjacent/XML/streaming media, not arbitrary binary content types.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/pdf:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        ["media type `application/pdf` is not supported"],
        "{report:#?}"
    );
}

#[test]
fn textual_vendor_and_structured_json_media_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /html:
    get:
      responses:
        "200":
          description: OK
          content:
            text/html:
              schema: { type: string }
  /octocat:
    get:
      responses:
        "200":
          description: OK
          content:
            application/octocat-stream:
              schema: { type: string }
  /problem:
    get:
      responses:
        "200":
          description: OK
          content:
            application/problem+json:
              schema:
                type: object
                properties: { detail: { type: string } }
"##;
    let generated = generate(spec);
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(!has_code(&generated, Code::UnsupportedMediaType));
}

// E009 case: raw-schema
#[test]
fn e009_an_html_response_with_an_object_schema_is_a_raw_schema_mismatch() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: not representable as raw text
          content:
            text/html:
              schema: { type: object, properties: { value: { type: string } } }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        ["media type `text/html` requires a scalar (string, number, integer or boolean) or binary response schema"],
        "{report:#?}"
    );
}

// E009 case: raw-schema
#[test]
fn e009_a_text_request_body_with_an_array_schema_is_a_raw_schema_mismatch() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          text/plain:
            schema: { type: array, items: { type: integer } }
      responses:
        "204": { description: ok }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        [
            "media type `text/plain` requires a scalar (string, number, integer or boolean) or \
             binary schema that can be sent as a raw body"
        ],
        "{report:#?}"
    );
}

/// Every scalar schema travels as raw text both ways (#1323): an integer, number or boolean, an
/// integer enum, and a union of them generate under `text/plain`, with no `E009`.
#[test]
fn a_text_body_of_any_scalar_schema_generates() {
    for schema in [
        "{ type: integer }",
        "{ type: integer, format: int32 }",
        "{ type: number }",
        "{ type: boolean }",
        "{ type: integer, enum: [1, 2] }",
        "{ oneOf: [{ type: integer }, { type: boolean }] }",
    ] {
        let report = generate(&format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          text/plain: {{ schema: {schema} }}
      responses:
        "200":
          description: ok
          content:
            text/plain: {{ schema: {schema} }}
"##
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        assert!(!has_code(&report, Code::UnsupportedMediaType), "{schema}");
    }
}

// E009 case: form-response
#[test]
fn e009_a_form_urlencoded_response_body_is_request_only_media() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: unsupported response codec
          content:
            application/x-www-form-urlencoded:
              schema: { type: object }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        [
            "media type `application/x-www-form-urlencoded` is supported for request bodies, not \
             response bodies"
        ],
        "{report:#?}"
    );
}

#[test]
fn e009_a_concrete_type_outside_the_byte_families_stays_unsupported() {
    // `application/*` mixes binary (`pdf`) with textual (`sdp`) subtypes and `font/*` is not
    // claimed, so none of them may be read as bytes on the strength of a prefix. This is the
    // response-side twin of `e009_a_media_type_naming_no_codec_is_unsupported`, and what keeps the openai corpus
    // expectation honest.
    // A family key with no subtype, or with a wildcard that does not make it a range, is not a
    // member of the family either.
    for media in [
        "application/pdf",
        "application/sdp",
        "font/woff2",
        "image/",
        "image/pn*",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{media}": {{ schema: {{}} }}
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
    }

    // On a request such a key would be sent verbatim as `Content-Type`, so it must be rejected
    // there too rather than read as bytes.
    for media in ["image/", "image/pn*"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    put:
      operationId: putX
      requestBody:
        required: true
        content:
          "{media}": {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
    }
}

#[test]
fn e009_a_structured_suffix_member_of_a_byte_family_stays_unsupported() {
    // `image/svg+xml` sits in the `image` family, but its RFC 6838 `+xml` suffix says the payload
    // is a text syntax, so bytes would be the silently-wrong reading the family rule exists to
    // avoid. It stays `E009`, as it was before the family rule existed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /logo:
    get:
      operationId: getLogo
      responses:
        "200":
          description: OK
          content:
            image/svg+xml: { schema: {} }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}
