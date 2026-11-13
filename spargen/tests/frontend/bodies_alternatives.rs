//! Request and response bodies offering alternative media: the `W014` that discloses an
//! alternative the selection passed over, and the rules under which one that decodes alike goes
//! unreported — never for an alternative that constrains its body, decodes differently, or cannot
//! be decoded at all. Beside them, the other spelling of an entry that constrains nothing: a
//! request entry with no `schema`, which takes any value its media carries.

use super::*;

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
fn w014_is_silent_when_every_alternative_decodes_identically() {
    // The ranged-media response shape from #72, as emitted for a byte range: three keys, one
    // representation. Every opaque-octets body is `bytes::Bytes` whatever its schema says, so
    // generating one of them gives up nothing and the warning was pure noise on a common shape.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /media/{id}:
    get:
      operationId: getMedia
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: OK
          content:
            video/*: { schema: {} }
            audio/*: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_an_alternative_spargen_cannot_decode() {
    // The narrowing above must not swallow a real one: an unregistered media type sitting beside a
    // byte body is a documented surface that is genuinely not generated.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            application/sdp: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_a_referenced_media_object() {
    // A 3.2 Media Type Object may itself be a Reference Object, which parses with no `schema` of
    // its own. Reading that absence as "constrains nothing" would call the alternative opaque on
    // the strength of a field the `$ref` spelling never sets, and drop the warning for a structured
    // body — the byte-identical inline spelling reports it, so the two must agree.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { $ref: "#/components/mediaTypes/Manifest" }
components:
  mediaTypes:
    Manifest:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_schema_is_a_ref() {
    // The other place a reference hides. Proving what a `$ref` points at would mean resolving it
    // during selection; answering "unknown" as "not opaque" only keeps a warning that was already
    // being reported.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: { $ref: "#/components/schemas/Manifest" } }
components:
  schemas:
    Manifest: { type: object, properties: { a: { type: string } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_text_alternatives_that_only_look_identical() {
    // The identical-decode rule stays confined to octet-stream. Two textual bodies that constrain
    // nothing look interchangeable and are not: this pair is `serde_json::Value` twice, while the
    // same pair written without `schema:` at all is `()` twice, and a mixed pair is one of each.
    // Only the octet gate collapses every body it admits onto one type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: {} }
            text/csv: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_when_two_empty_spellings_lower_differently() {
    // `constrains nothing` has two spellings that disagree outside the octet gate: no `schema` key
    // lowers to `()`, `schema: {}` lowers to `Any`. Suppressing between them would make the *order*
    // of two content keys decide the response type, in silence.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: {}
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    // The selection is the one with no schema at all, so the body really is unit-typed. The
    // `schema: {}` sibling would have been `serde_json::Value`, which is the whole point: the two
    // spellings are not interchangeable, so neither may silence the other.
    assert!(
        code.contains("support::ResponseValue<()>"),
        "the no-schema selection should lower to `()`: {code}"
    );
}

#[test]
fn w014_fires_for_an_octet_request_alternative_that_decodes_alike() {
    // A request narrows at the wire even when both entries are `bytes::Bytes`: the selected media
    // key becomes the `Content-Type` verbatim, so this client only ever sends octet-stream to a
    // server documented as also accepting `video/*`. The range-as-request rejection does not cover
    // this — that fires on the media actually selected, and a suppressed alternative never is.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
          video/*: { schema: {} }
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
fn w014_fires_when_an_octet_alternative_describes_itself_outside_its_schema() {
    // A Media Type Object says things outside `schema`. `itemSchema` in particular carries a type,
    // so an entry declaring one is not interchangeable with an empty body even though its `schema`
    // is empty — reading only `schema` is how that slipped through before.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { itemSchema: { type: string } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );

    // `encoding` is the other half of the same claim, and the explain text names it. It is inert on
    // an octet media, but an entry that spells it out is still saying more than an empty one.
    let with_encoding = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { encoding: { part: { contentType: text/plain } } }
"##;
    let report = generate(with_encoding);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_sequential_alternatives_with_different_item_types() {
    // A sequential media's item type lives in `itemSchema`, outside the body schema entirely. Two
    // entries can both constrain nothing in `schema` and still stream different types.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson: { itemSchema: { type: string } }
            application/jsonl: { itemSchema: { type: integer } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_request_body_alternatives_that_decode_alike() {
    // A request narrows at the wire, not at the type. The chosen media key becomes `Content-Type`
    // verbatim, so a server documented as accepting both is only ever sent one — whatever the Rust
    // types do.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/json: { schema: {} }
          application/vnd.acme.v2+json: { schema: {} }
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
fn w014_still_fires_when_the_text_selection_is_typed() {
    // The *selection* has to reach the shared representation too. A textual body carrying a string
    // enum lowers to a typed value rather than to `String`, so an opaque sibling beside it really
    // does document something the client will never accept.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string, enum: [a, b] } }
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn an_octet_selection_with_a_binary_schema_still_silences_an_opaque_range() {
    // Octet-stream is the one codec whose gate admits only bodies that collapse to `bytes::Bytes`,
    // so the selection does not have to be empty for the alternatives to decode identically. The
    // 3.0 spelling of a binary body beside the 3.1 one narrows nothing.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { type: string, format: binary } }
            video/*: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_every_kind_of_constraining_alternative() {
    // `constrains_nothing` is a long conjunction, and dropping any one term would silently widen
    // the suppression — the exhaustive destructure it is built on catches a field that is *added*
    // to `Schema`, never one that stops being consulted. Each schema below trips exactly one term,
    // so deleting that term turns this test red instead of turning a real narrowing silent.
    //
    // Three rows say why they constrain. An octet-classified alternative carrying an object schema
    // would be rejected by the octet gate, not turned into bytes, so it is a genuine narrowing. A
    // validation keyword never changes the storage type, so both bodies are `bytes::Bytes`, but an
    // alternative that says something is not interchangeable with one that says nothing. And
    // `false` is the one boolean schema that constrains, to nothing at all, so it is not read as the
    // empty schema. The octet selection says nothing in either spelling, an empty schema or none.
    for constraining in [
        "{ type: object, properties: { a: { type: string } } }",
        "{ required: [a] }",
        "{ additionalProperties: false }",
        "{ patternProperties: { \"^x-\": { type: string } } }",
        "{ items: { type: string } }",
        "{ prefixItems: [{ type: string }] }",
        "{ allOf: [{ type: string }] }",
        "{ oneOf: [{ type: string }] }",
        "{ anyOf: [{ type: string }] }",
        "{ enum: [a, b] }",
        "{ const: a }",
        "{ format: uuid }",
        "{ contentEncoding: base64 }",
        "{ contentMediaType: application/json }",
        "{ maxLength: 10 }",
        "false",
    ] {
        for octet in ["{ schema: {} }", "{}"] {
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
            application/octet-stream: {octet}
            video/*: {{ schema: {constraining} }}
"##
            );
            let report = generate(&spec);
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                has_code(&report, Code::AlternativeMediaIgnored),
                "an alternative constrained by `{constraining}` beside `{octet}` was suppressed \
                 as decoding identically: {report:#?}"
            );
        }
    }
}

/// A request body offering one media entry with no `schema`, for `media`, `required` as given.
fn schemaless_request_body_spec(media: &str, required: bool) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: {required}
        content:
          {media}: {{}}
      responses:
        "204": {{ description: No Content }}
"##
    )
}

#[test]
fn a_request_body_with_no_schema_is_sent_as_any_value_of_its_media() {
    // A media entry with no `schema` constrains nothing, so the body takes any value its media
    // carries: any JSON value (as `schema: {}` does), any text, any octets. It was lowered untyped
    // instead, so the method took no body argument and sent none, with no diagnostic (#1339).
    for (media, ty) in [
        ("application/json", "serde_json::Value"),
        ("application/merge-patch+json", "serde_json::Value"),
        ("application/xml", "serde_json::Value"),
        ("text/plain", "String"),
        ("application/octet-stream", "bytes::Bytes"),
    ] {
        for required in [true, false] {
            let (report, code) = generate_with_code(&schemaless_request_body_spec(media, required));
            assert_eq!(report.outcome(), Outcome::Generated, "{media}: {report:#?}");
            assert!(
                code.contains(&format!("pub type PostUbody = {ty};")),
                "{media}: the body should be typed `{ty}`: {code}"
            );
            let signature = code
                .split_once("pub async fn post_u(")
                .and_then(|(_, rest)| rest.split_once(')'))
                .map(|(arguments, _)| arguments)
                .unwrap_or_else(|| panic!("{media}: no `post_u` method: {code}"));
            assert!(
                signature.contains("body:"),
                "{media}: `post_u` should take the body as an argument: {signature}"
            );
            assert!(
                code.contains(&format!("CONTENT_TYPE, \"{media}\"")),
                "{media}: the body should be sent under its media: {code}"
            );
        }
    }
}

#[test]
fn a_form_request_body_with_no_schema_is_still_refused() {
    // A form or multipart body is sent field by field from an object schema's properties, so one
    // with no schema has nothing to send: it is refused (`E009`), never typed as any value.
    for media in ["application/x-www-form-urlencoded", "multipart/form-data"] {
        let (report, code) = generate_with_code(&schemaless_request_body_spec(media, true));
        assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
        assert!(code.is_empty(), "{media}: {code}");
    }
}
