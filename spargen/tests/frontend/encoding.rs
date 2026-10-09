//! Encoding Objects and the per-part content types of `multipart` and
//! `application/x-www-form-urlencoded` bodies.

use super::*;

#[test]
fn explicit_media_encoding_generates() {
    // The Encoding Object's RFC 6570 mode: an explicit `style`/`explode` selects query-style
    // serialization for that property and makes `contentType` inert.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties: { tags: { type: array, items: { type: string } } }
            encoding:
              tags: { style: form, explode: true }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }
}

#[test]
fn multipart_encoding_content_type_generates() {
    // The Encoding Object's media-type mode: each part is sent as its declared `contentType`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                sdp: { type: string }
                session: { type: object, properties: { id: { type: string } } }
            encoding:
              sdp: { contentType: application/sdp }
              session: { contentType: application/json }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_on_a_json_body_has_no_effect() {
    // `encoding` applies only to form and multipart content; elsewhere the specification says it
    // SHALL be ignored, so it is acknowledged rather than rejected.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
            encoding:
              a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_entry_without_a_property_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties: { a: { type: string } }
            encoding:
              missing: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_nested_encoding_object() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties: { part: { type: object, properties: { a: { type: string } } } }
            encoding:
              part:
                contentType: multipart/mixed
                encoding:
                  a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_wildcard_encoding_content_type() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties: { image: { type: string, contentEncoding: base64 } }
            encoding:
              image: { contentType: "image/*" }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_form_urlencoded_body_requires_an_object_schema() {
    // A non-object form body used to compile and then fail at runtime inside the form encoder.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema: { type: string }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn multipart_form_data_request_body_generates() {
    // A `multipart/form-data` request body whose schema is an object (a file part + a text part) is
    // now supported: it generates without E009 firing. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
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
fn e009_multipart_non_object_body_rejected() {
    // A `multipart/form-data` body whose schema is NOT an object has no properties to enumerate as
    // form parts, so it stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn e009_multipart_deep_object_encoding_rejected() {
    // `deepObject` builds `name[key]=value` query fragments. A multipart part carries its name in
    // `Content-Disposition` and its value alone, so the style has no representation there — it used
    // to generate parts holding only the values, with the keys dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                filter:
                  type: object
                  properties:
                    a: { type: string }
            encoding:
              filter:
                style: deepObject
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_multipart_rfc6570_object_property_rejected() {
    // The specification applies the Encoding Object to the entire value for a non-array property,
    // but defines no part representation for an object. Reusing the query builders silently emitted
    // one part per member carrying only the value.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                meta:
                  type: object
                  properties:
                    a: { type: string }
            encoding:
              meta:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_encoding_delimited_style_with_explode_rejected() {
    // The specification's serialization table marks `spaceDelimited`/`pipeDelimited` with
    // `explode: true` as *n/a*. The identical parameter-side construct is already `E010`; an
    // Encoding Object used to accept it and then ignore the `explode`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: pipeDelimited
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn multipart_rfc6570_array_encoding_generates() {
    // The shapes that *are* defined stay supported: an array property under a delimited or form
    // style, exploded or not.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                names:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: spaceDelimited
                explode: false
              names:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    }
}

#[test]
fn e009_a_form_urlencoded_string_property_declaring_a_binary_family_content_type() {
    // The family rule reaches Encoding Objects through the shared classifier: a form-urlencoded
    // property whose `contentType` names `image/png` is binary, which a form body cannot carry —
    // the disposition `application/octet-stream` already has there — and the message names what
    // was declared, since the schema itself is a plain string a reader cannot call binary.
    //
    // #399: the classification is case-insensitive, so `Application/Octet-Stream` is the binary
    // declaration its lowercase spelling is. Before, the mixed-case spelling missed every
    // classifier arm, fell through to the string's natural codec, and was generated as
    // `FormMode::Text`.
    for declared in [
        "image/png",
        "application/octet-stream",
        "Application/Octet-Stream",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                pic: {{ type: string }}
            encoding:
              pic: {{ contentType: {declared} }}
      responses:
        '204': {{ description: ok }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{declared}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::UnsupportedMediaType
                        && d.message.contains("`pic`")
                        && d.message.contains(&format!("`contentType: {declared}`"))
                }),
                "{declared}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_form_urlencoded_binary_property_without_a_declared_content_type() {
    // The other arm of the same rejection: no Encoding Object at all, so the property is binary
    // on the strength of its own schema (`contentEncoding: base64`, which lowers to bytes) and its
    // `contentType` merely defaulted to `application/octet-stream`. The message must not name a
    // `contentType` the document never wrote.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                pic: { type: string, contentEncoding: base64 }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains("`pic`")
                    && d.message.contains(
                        "is binary, which has no `application/x-www-form-urlencoded` \
                         representation",
                    )
                    && !d.message.contains("declares `contentType:")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_form_urlencoded_binary_property_declaring_text_is_judged_case_insensitively() {
    // #399: a binary property's declared `contentType` is classified lowercased, so `Text/Plain`
    // reaches the outcome `text/plain` does. Before, the mixed-case spelling missed the `text/`
    // arm, fell through to the property's natural octet-stream codec, and was rejected `E009`
    // while its lowercase spelling generated.
    let binary = "{ type: string, contentEncoding: base64 }";
    for declared in ["text/plain", "Text/Plain", "TEXT/PLAIN"] {
        let spec = form_field_declaring(binary, declared);
        assert_ne!(check(&spec).outcome(), Outcome::Rejected, "{declared}");
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{declared}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{declared}: {report:#?}"
        );
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("name: \"part\", mode: support::FormMode::Text"),
            "{declared}: {flat}"
        );
    }
}

#[test]
fn a_multipart_string_property_declaring_a_binary_family_content_type_is_a_text_part() {
    // The same declaration on multipart is unchanged by the family rule: a part is rendered by its
    // property's own lowered type, so a string stays a text part, and the declared `contentType`
    // rides on it as the part's header, exactly as `application/sdp` does.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                pic: { type: string }
            encoding:
              pic: { contentType: image/png }
      responses:
        '204': { description: ok }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("reqwest::multipart::Part::text("), "{code}");
    assert!(code.contains(".mime_str(\"image/png\")"), "{code}");
}

/// A `multipart/form-data` body with one property `part` of the given schema, whose Encoding
/// Object declares the given `contentType`.
fn multipart_part_declaring(schema: &str, content_type: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                part: {schema}
            encoding:
              part: {{ contentType: "{content_type}" }}
      responses:
        '204': {{ description: ok }}
"##
    )
}

#[test]
fn e009_a_multipart_json_rendered_part_declaring_a_non_json_content_type() {
    // A part whose property is not a scalar or bytes is rendered as JSON whatever its
    // `contentType` says, so any declaration that is not JSON would put JSON bytes under a header
    // naming another syntax — `application/xml` over an object is the case #177 reports. Unlike
    // a string under `image/png` (a text part, whose bytes are the string the document
    // described), no reader of the document predicts JSON here, so it is rejected rather than sent.
    // The sent (first) element of a list is what is judged, and named.
    let object = "{ type: object, properties: { id: { type: integer } } }";
    let cases = [
        (object, "application/xml", "application/xml"),
        (object, "text/xml", "text/xml"),
        (object, "text/plain", "text/plain"),
        (
            object,
            "application/octet-stream",
            "application/octet-stream",
        ),
        (object, "image/png", "image/png"),
        // Well-formed, but naming no codec spargen has: still not JSON on the wire.
        (object, "application/yaml", "application/yaml"),
        (
            object,
            "application/xml, application/json",
            "application/xml",
        ),
        (
            "{ type: array, items: { type: string } }",
            "text/csv",
            "text/csv",
        ),
        (
            "{ oneOf: [ { type: string }, { type: object } ] }",
            "application/xml",
            "application/xml",
        ),
        ("{}", "application/xml", "application/xml"),
    ];
    for (schema, declared, sent) in cases {
        let spec = multipart_part_declaring(schema, declared);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{schema} / {declared}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::UnsupportedMediaType
                        && d.message.contains("`part`")
                        && d.message.contains(&format!("`contentType: {sent}`"))
                        && d.message.contains("JSON")
                }),
                "{schema} / {declared}: {report:#?}"
            );
        }
    }
}

#[test]
fn a_multipart_part_keeps_a_declared_content_type_its_bytes_agree_with() {
    // The boundary of the rejection above: a JSON-rendered part declaring JSON (plain or a
    // `+json` structured suffix, in any letter case, since media types are case-insensitive) is
    // what it says, and a scalar or bytes part carries any well-formed declaration as its header —
    // a string is the text the document described and bytes are whatever the caller supplies, so
    // neither is re-encoded against the header.
    let object = "{ type: object, properties: { id: { type: integer } } }";
    let cases = [
        (object, "application/json", "serde_json::to_string("),
        (object, "Application/JSON", "serde_json::to_string("),
        (object, "application/vnd.api+json", "serde_json::to_string("),
        (
            "{ type: array, items: { type: integer } }",
            "application/json; charset=utf-8",
            "serde_json::to_string(",
        ),
        ("{ type: string }", "application/xml", "Part::text("),
        ("{ type: integer }", "text/csv", "Part::text("),
        (
            "{ type: string, contentEncoding: base64 }",
            "application/xml",
            "Part::bytes(",
        ),
    ];
    for (schema, declared, rendering) in cases {
        let spec = multipart_part_declaring(schema, declared);
        assert_ne!(
            check(&spec).outcome(),
            Outcome::Rejected,
            "{schema} / {declared}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{schema} / {declared}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{schema} / {declared}: {report:#?}"
        );
        assert!(code.contains(rendering), "{schema} / {declared}: {code}");
        assert!(
            code.contains(&format!(".mime_str(\"{declared}\")")),
            "{schema} / {declared}: {code}"
        );
    }
}

/// An `application/x-www-form-urlencoded` body with one property `part` of the given schema, whose
/// Encoding Object declares the given `contentType`.
fn form_field_declaring(schema: &str, content_type: &str) -> String {
    multipart_part_declaring(schema, content_type)
        .replace("multipart/form-data:", "application/x-www-form-urlencoded:")
}

#[test]
fn e009_a_form_urlencoded_json_rendered_field_declaring_a_non_json_content_type() {
    // #321: a form field has no header, but its `contentType` is the syntax its value is
    // serialized in, and spargen serializes a non-scalar field only as JSON. Before this,
    // `application/xml` over an object fell back to that JSON with nothing reported, and
    // `text/plain` was honoured as `FormMode::Text`, which refuses a nested value and so failed
    // every call of the generated operation. Both are refused, as on multipart, naming the sent
    // (first) element of a list.
    let object = "{ type: object, properties: { id: { type: integer } } }";
    let cases = [
        (object, "application/xml", "application/xml"),
        (object, "text/plain", "text/plain"),
        (object, "Text/Plain", "Text/Plain"),
        (object, "application/yaml", "application/yaml"),
        (
            object,
            "application/octet-stream",
            "application/octet-stream",
        ),
        (object, "text/plain, application/json", "text/plain"),
        (
            "{ type: array, items: { type: string } }",
            "text/plain",
            "text/plain",
        ),
        (
            "{ oneOf: [ { type: string }, { type: object } ] }",
            "text/plain",
            "text/plain",
        ),
        ("{}", "application/xml", "application/xml"),
    ];
    for (schema, declared, sent) in cases {
        let spec = form_field_declaring(schema, declared);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{schema} / {declared}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::UnsupportedMediaType
                        && d.message.contains("`part`")
                        && d.message.contains(&format!("`contentType: {sent}`"))
                        && d.message.contains("form field only as JSON")
                }),
                "{schema} / {declared}: {report:#?}"
            );
        }
    }
}

#[test]
fn a_form_urlencoded_field_keeps_a_declared_content_type_it_is_serialized_in() {
    // The boundary of the rejection above: a non-scalar field declaring JSON, in any letter case
    // or as a `+json` type, is serialized as the JSON it declares; a scalar field declaring
    // `text/plain` (or a syntax with no codec, whose text is the value itself) is sent as text,
    // and one declaring JSON as a JSON value; and RFC 6570 serialization of an object makes its
    // `contentType` inert, so the rule does not reach it.
    let object = "{ type: object, properties: { id: { type: integer } } }";
    let cases = [
        (object, "application/json", "FormMode::Json"),
        (object, "Application/JSON", "FormMode::Json"),
        (object, "application/vnd.api+json", "FormMode::Json"),
        (
            "{ type: array, items: { type: integer } }",
            "application/json",
            "FormMode::Json",
        ),
        ("{ type: string }", "text/plain", "FormMode::Text"),
        ("{ type: string }", "application/xml", "FormMode::Text"),
        ("{ type: integer }", "application/json", "FormMode::Json"),
        // #399: the codec is chosen from the declared media type case-insensitively, as the
        // refusal above judges it, so a scalar declaring `Application/JSON` is the JSON value its
        // lowercase spelling is, not the text its natural codec would make it.
        ("{ type: integer }", "Application/JSON", "FormMode::Json"),
        ("{ type: object }", "Application/JSON", "FormMode::Json"),
        (
            "{ type: string }",
            "Application/Vnd.Api+JSON",
            "FormMode::Json",
        ),
        ("{ type: string }", "TEXT/PLAIN", "FormMode::Text"),
    ];
    for (schema, declared, rendering) in cases {
        let spec = form_field_declaring(schema, declared);
        assert_ne!(
            check(&spec).outcome(),
            Outcome::Rejected,
            "{schema} / {declared}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{schema} / {declared}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{schema} / {declared}: {report:#?}"
        );
        // The emitted property literal, not the bare variant path, which the embedded runtime's
        // own source also spells; whitespace is collapsed, since the emitter wraps the literal.
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
        let property = format!("name: \"part\", mode: support::{rendering}");
        assert!(flat.contains(&property), "{schema} / {declared}: {code}");
    }
    let styled = form_field_declaring(object, "text/plain").replace(
        "part: { contentType: \"text/plain\" }",
        "part: { contentType: \"text/plain\", style: deepObject }",
    );
    assert!(styled.contains("style: deepObject"), "{styled}");
    let (report, code) = generate_with_code(&styled);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("name: \"part\", mode: support::FormMode::Style")
            && flat.contains("support::FormStyle::DeepObject"),
        "{code}"
    );
}

#[test]
fn e009_a_malformed_encoding_content_type_is_unsupported() {
    // An Encoding Object's `contentType` is held to the same RFC 6838 well-formedness rule as a
    // `content` key. A value that is no media type at all used to fall through to the property's
    // natural codec with nothing reported, and was then sent verbatim: a multipart part attached
    // it through `mime_str`, which fails only when a request is built. Only the element a client
    // sends (the first of the list, split outside quoted-strings) is checked, and its parameters
    // are held to RFC 9110 § 5.6.6 (#248).
    let body = |media: &str, content_type: &str| {
        // The value sits in a double-quoted YAML scalar.
        let content_type = content_type
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\t', "\\t");
        format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          {media}:
            schema:
              type: object
              properties:
                note: {{ type: string }}
            encoding:
              note: {{ contentType: "{content_type}" }}
      responses:
        "204": {{ description: No Content }}
"##
        )
    };
    for media in ["multipart/form-data", "application/x-www-form-urlencoded"] {
        for content_type in [
            "text/plain/extra",
            "text/plain/extra; charset=utf-8",
            "text/plain/extra, text/plain",
            "text",
            "",
            "text/",
            "/plain",
            "te xt/plain",
            "application/vnd.a/b+json",
        ] {
            let spec = body(media, content_type);
            let first = content_type.split(',').next().unwrap_or_default().trim();
            for report in [generate(&spec), check(&spec)] {
                assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
                assert!(
                    report.diagnostics().iter().any(|diagnostic| {
                        diagnostic.code == Code::UnsupportedMediaType
                            && diagnostic.message
                                == format!(
                                    "`encoding.note.contentType: {first}` is not a media type"
                                )
                    }),
                    "{media} / {content_type:?}: {report:#?}"
                );
            }
        }
    }
    // A well-formed type spargen has no codec for keeps the documented fallback: it rides on the
    // part as its header, and the value is rendered from the property's own type.
    let (report, code) = generate_with_code(&body("multipart/form-data", "application/sdp"));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("mime_str(\"application/sdp\")"), "{code}");
    assert!(
        code.contains("reqwest::multipart::Part::text(value.to_string())"),
        "the part is built from the string property, not from the declared type: {code}"
    );
    // A well-formed, classified type with parameters, and a malformed element after the one that
    // is sent, both still generate.
    for content_type in ["text/plain; charset=utf-8", "text/plain, text/plain/extra"] {
        let spec = body("multipart/form-data", content_type);
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "{content_type:?}: {report:#?}"
            );
        }
    }
    // A parameter that is not `token "=" ( token / quoted-string )` used to generate silently and
    // fail in `mime_str` (`text/plain; foo` is mime's `MissingEqual`). Each case names the element
    // the diagnostic quotes: the list is split only at a comma outside a quoted-string.
    let malformed = [
        ("text/plain; foo", "text/plain; foo"),
        ("text/plain; =utf-8", "text/plain; =utf-8"),
        ("text/plain; charset=", "text/plain; charset="),
        ("text/plain; charset=utf 8", "text/plain; charset=utf 8"),
        ("text/plain; charset = utf-8", "text/plain; charset = utf-8"),
        ("text/plain; a=b c=d", "text/plain; a=b c=d"),
        ("text/plain; a=b; c", "text/plain; a=b; c"),
        ("text/plain; charset=\"utf-8", "text/plain; charset=\"utf-8"),
        ("text/plain; a=\"x\"y", "text/plain; a=\"x\"y"),
        ("text/plain; a=\"x, y", "text/plain; a=\"x, y"),
        ("text/plain; foo, text/plain", "text/plain; foo"),
    ];
    // Well-formed under RFC 9110, but a quoted value `mime_str` refuses: empty, or holding a `"`
    // or a tab. Only a multipart part sends its `contentType`, so only multipart rejects these.
    let unsendable = [
        "text/plain; a=\"\"",
        "text/plain; a=\"x\\\"y\"",
        "text/plain; a=\"x\ty\"",
        "text/plain; a=\"x\\\ty\"",
    ];
    for media in ["multipart/form-data", "application/x-www-form-urlencoded"] {
        let unsendable_here: &[&str] = if media == "multipart/form-data" {
            &unsendable
        } else {
            &[]
        };
        let cases = malformed
            .iter()
            .map(|(content_type, first)| {
                (
                    *content_type,
                    format!(
                        "`encoding.note.contentType: {first}` has a parameter that is not \
                         `name=value` under RFC 9110 § 5.6.6"
                    ),
                )
            })
            .chain(unsendable_here.iter().map(|content_type| {
                (
                    *content_type,
                    format!(
                        "`encoding.note.contentType: {content_type}` has a quoted parameter value \
                         the generated client cannot send: an empty value, or one holding a `\"` \
                         or a tab"
                    ),
                )
            }));
        for (content_type, message) in cases {
            let spec = body(media, content_type);
            for report in [generate(&spec), check(&spec)] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{media} / {content_type:?}: {report:#?}"
                );
                assert!(
                    report.diagnostics().iter().any(|diagnostic| {
                        diagnostic.code == Code::UnsupportedMediaType
                            && diagnostic.message == message
                    }),
                    "{media} / {content_type:?}: {report:#?}"
                );
            }
        }
    }
    // A form-urlencoded field's `contentType` only picks the codec its value is rendered with; it
    // is never sent as a header, so an RFC-valid parameter `mime_str` would refuse is no reason to
    // reject the document.
    for content_type in unsendable {
        let spec = body("application/x-www-form-urlencoded", content_type);
        let report = check(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{content_type:?}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{content_type:?}: {report:#?}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{content_type:?}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{content_type:?}: {report:#?}"
        );
        assert!(!code.contains("mime_str"), "{content_type:?}: {code}");
    }
    // Well-formed parameter lists generate, and the part carries the canonical spelling: names
    // and values as written, whitespace around `;` and empty parameters dropped (RFC 9110 admits
    // both, `mime_str` refuses both). A comma inside a quoted value does not end the element, and
    // a `*` in a parameter value is not a wildcard.
    for (content_type, sent) in [
        ("text/plain; charset=utf-8", "text/plain; charset=utf-8"),
        ("text/plain;charset=utf-8", "text/plain; charset=utf-8"),
        ("text/plain ; charset=utf-8", "text/plain; charset=utf-8"),
        ("text/plain;\tcharset=utf-8 ", "text/plain; charset=utf-8"),
        ("text/plain;", "text/plain"),
        ("text/plain;; charset=utf-8;", "text/plain; charset=utf-8"),
        (
            "text/plain; charset=\"utf-8\"",
            "text/plain; charset=\"utf-8\"",
        ),
        ("text/plain; name=\"a, b\"", "text/plain; name=\"a, b\""),
        (
            "text/plain; name=\"a,b\", text/plain/extra",
            "text/plain; name=\"a,b\"",
        ),
        ("text/plain; name=\"a\\\\b\"", "text/plain; name=\"a\\\\b\""),
        ("text/plain; a=b; c=\"d e\"", "text/plain; a=b; c=\"d e\""),
        ("text/plain; a=*", "text/plain; a=*"),
        ("text/plain; a=b, text/plain; foo", "text/plain; a=b"),
        ("text/plain; a=\"caf\u{e9}\"", "text/plain; a=\"caf\u{e9}\""),
        ("text/plain; a=\"x;y\"", "text/plain; a=\"x;y\""),
    ] {
        let spec = body("multipart/form-data", content_type);
        let report = check(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{content_type:?}: {report:#?}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{content_type:?}: {report:#?}"
        );
        let literal = format!("mime_str({sent:?})");
        assert!(
            code.contains(&literal),
            "{content_type:?} sends {literal}: {code}"
        );
    }
}

#[test]
fn e009_a_rejected_multipart_request_selection_claims_nothing_is_generated() {
    // `multipart/form-data` outranks `application/octet-stream`, so it is selected, and then its
    // shape gate refuses a non-object schema. That rejection is the whole report for this body: no
    // `W014` names the octet alternative as passed over for a refused multipart selection
    // (#110), and the gate stops there rather than lowering the refused body's encoding.
    let spec = request_body_document(&[
        ("multipart/form-data", "{ type: string }"),
        ("application/octet-stream", "{}"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::UnsupportedMediaType),
            [
                "a `multipart/form-data` request body must be an object schema; its properties \
                 are the form parts, so a non-object multipart body is not representable"
            ],
            "{report:#?}"
        );
    }
}

/// `encoding.headers` accepts a Header Object or a `$ref` to one. The reference was never
/// resolved, so a target that pins a `const` was reported as pinning no value (`W011`) and the
/// part shipped without the header. Resolving it makes the two spellings equivalent.
#[test]
fn encoding_header_ref_is_resolved_and_pins_its_const() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/PartKind' }
      responses:
        '204': { description: ok }
components:
  headers:
    PartKind:
      description: which part this is
      schema: { type: string, const: profile }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "a resolved header that pins a const has an effect: {report:#?}"
    );
    assert!(code.contains("X-Part-Kind"), "{code}");
    assert!(code.contains("profile"), "{code}");
}

/// An `encoding.headers` reference that resolves to nothing is `E004` — and only `E004`, so one
/// defect does not also collect a "pins no value" warning naming a second, wrong reason.
#[test]
fn e004_unresolvable_encoding_header_ref_does_not_also_warn() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/Missing' }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

// --- Media Type Object dispositions outside the request body -------------------------------
//
// `resolve_media_object` is the seam every Media Type Object passes through. Before these
// fixtures, only the request-body path dispositioned `encoding`; the same declaration on a
// response, a parameter, a header, or a `components.mediaTypes` entry was dropped in silence.

#[test]
fn w011_encoding_on_a_response_media_type_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: object, properties: { a: { type: string } } }
              encoding:
                a: { contentType: text/plain }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_on_a_parameter_content_media_type_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          content:
            application/json:
              schema: { type: object, properties: { a: { type: string } } }
              encoding:
                a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_reached_through_a_component_media_type_reference_has_no_effect() {
    // The declaration is one `$ref` hop away from its use site. Resolving it is what makes the
    // disposition reachable at all.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/Payload'
components:
  mediaTypes:
    Payload:
      schema: { type: object, properties: { a: { type: string } } }
      encoding:
        a: { contentType: text/plain }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_media_type_reference_summary_documents_the_use_site() {
    // A Reference Object's own `summary`/`description` documents this use site, which one shared
    // generated item cannot express — the same disposition parameters and responses already get.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/Payload'
              summary: the payload as returned by this operation
components:
  mediaTypes:
    Payload:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

/// A Header Object `$ref` with its own `summary`/`description` is `W011`'s reference-docs case,
/// like a Parameter, Request Body or Response reference (#398). The header resolver once skipped
/// the note, so the override was dropped with nothing said, at both places a header is resolved:
/// a response's `headers` and a multipart `encoding` entry's `headers`. Each documented reference
/// is reported exactly once, and an undocumented one not at all.
#[test]
fn w011_header_reference_summary_documents_the_use_site() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: object, properties: { a: { type: string } } }
            encoding:
              a:
                headers:
                  X-Part: { $ref: '#/components/headers/Part', summary: the part tag }
      responses:
        '200':
          description: ok
          headers:
            X-Rate: { $ref: '#/components/headers/Rate', description: per-minute budget }
            X-Plain: { $ref: '#/components/headers/Plain' }
components:
  headers:
    Rate: { schema: { type: integer } }
    Plain: { schema: { type: integer } }
    Part: { schema: { type: string, const: tag } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let noted = |target: &str| {
            let wanted = format!("the `summary`/`description` on the reference to `{target}`");
            report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .filter(|d| d.message.starts_with(&wanted))
                .count()
        };
        assert_eq!(noted("#/components/headers/Rate"), 1, "{report:#?}");
        assert_eq!(noted("#/components/headers/Part"), 1, "{report:#?}");
        assert_eq!(noted("#/components/headers/Plain"), 0, "{report:#?}");
    }
}

#[test]
fn w011_prefix_encoding_on_form_urlencoded_has_no_effect() {
    // The specification scopes `prefixEncoding`/`itemEncoding` to `multipart`, so on a form body
    // they are inert rather than an error — this was previously over-rejected as E009.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema: { type: object, properties: { a: { type: string } } }
            prefixEncoding:
              - { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_prefix_encoding_on_multipart_is_rejected() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: object, properties: { a: { type: string } } }
            prefixEncoding:
              - { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_item_encoding_on_multipart_is_rejected() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: object, properties: { a: { type: string } } }
            itemEncoding: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn response_header_content_media_type_reference_resolves() {
    // Reading `schema` off an unresolved Reference Object found `None` and dropped the typed
    // accessor with nothing said. Resolving first is what makes the header typed.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json:
                  $ref: '#/components/mediaTypes/Meta'
          content:
            application/json:
              schema: { type: string }
components:
  mediaTypes:
    Meta:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("x_meta"),
        "the typed header accessor should be generated: {code}"
    );
}

#[test]
fn w011_response_header_content_without_a_schema_is_reported() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json: {}
          content:
            application/json:
              schema: { type: string }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w010_item_schema_on_a_response_header_is_ignored() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json:
                  schema: { type: object, properties: { a: { type: string } } }
                  itemSchema: { type: string }
          content:
            application/json:
              schema: { type: string }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::Oas32ConstructIgnored),
            "{report:#?}"
        );
    }
}
