//! Request and response bodies: media-type selection, byte and textual bodies, media ranges,
//! streaming bodies, and `W014` alternatives.

use super::*;

#[test]
fn oas32_component_media_type_references_generate_typed_bodies() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/PetJson'
components:
  mediaTypes:
    PetJson:
      schema:
        type: object
        properties: { id: { type: string } }
        required: [id]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub id"), "{code}");
    assert_no_untyped_value(&code);
}

#[test]
fn oas32_stream_item_schema_types_the_stream_not_dropped() {
    // OpenAPI 3.2 gives a sequential/streaming media its per-item type in `itemSchema` (not
    // `schema`). A `text/event-stream` response typed only via `itemSchema` must lower to a typed
    // streaming body — the operation still generates, the item type is NOT dropped to a bodyless
    // `()`, and no `itemSchema` warning fires (on streaming media it IS used).
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        '200':
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/Event"
components:
  schemas:
    Event:
      type: object
      required: [seq]
      properties:
        seq: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    // check/generate parity.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::Oas32ConstructIgnored),
        "{checked:#?}"
    );
}

#[test]
fn oas32_sse_json_content_schema_types_the_payload() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamAdminEvents
      responses:
        '200':
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/SseEnvelope"
components:
  schemas:
    SseEnvelope:
      type: object
      required: [data]
      properties:
        data:
          type: string
          contentMediaType: application/json
          contentSchema:
            $ref: "#/components/schemas/AdminEvent"
        id: { type: string }
        retry: { type: integer }
    AdminEvent:
      type: object
      required: [kind, libraryId]
      properties:
        kind: { type: string, const: scanStarted }
        libraryId: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::ValidationKeywordIgnored),
        "consumed SSE content annotations must not warn: {report:#?}"
    );
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("EventStream < types :: AdminEvent >")
            || flat.contains("EventStream<types::AdminEvent>"),
        "contentSchema must become the stream payload type: {flat}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::ValidationKeywordIgnored),
        "check/generate must agree that the SSE content annotations are consumed: {checked:#?}"
    );
}

#[test]
fn content_schema_outside_sse_remains_an_explicit_warning() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /value:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  encoded:
                    type: string
                    contentMediaType: application/json
                    contentSchema: { type: object, properties: { id: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "check/generate must report the same content annotation warning: {checked:#?}"
    );
}

#[test]
fn oas32_json_sequence_item_schema_generates_rfc7464_streaming() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json-seq:
              itemSchema:
                type: object
                properties: { id: { type: integer } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("Framing::JsonSequence"), "{code}");
}

#[test]
fn oas32_sequential_schema_is_not_misread_as_an_item_schema() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          description: ok
          content:
            application/jsonl:
              schema:
                type: array
                items: { type: string }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn oas32_item_schema_on_non_streaming_media_warns_w010() {
    // `itemSchema` is only meaningful for sequential/streaming media. On a plain JSON response it is
    // acknowledged with `W010` (not silently dropped) and generation still succeeds via `schema`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: string }
              itemSchema: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
}

#[test]
fn validation_keywords_in_reusable_stream_item_schema_warn_w001() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          content:
            application/x-ndjson:
              $ref: '#/components/mediaTypes/EventStream'
components:
  mediaTypes:
    EventStream:
      itemSchema: { type: string, minLength: 1 }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
}

#[test]
fn e009_unsupported_media_type() {
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
    assert!(has_code(&report, Code::UnsupportedMediaType));
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
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnsupportedMediaType));
}

#[test]
fn e009_raw_media_requires_a_compatible_schema() {
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
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn e009_request_only_media_is_rejected_in_responses() {
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
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn a_lone_xml_or_streaming_body_beside_a_bodyless_success_still_generates() {
    // Issue #121 makes a single body beside a documented bodyless `204` a success enum. That enum
    // still decodes only one body, so neither the XML nor the streaming multi-status rejection
    // (both narrowed `E009`) may fire for it.
    for media in ["application/xml", "text/event-stream"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            {media}:
              schema: {{ type: object, required: [a], properties: {{ a: {{ type: string }} }} }}
        "204":
          description: No Content
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{media}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
        let checked = check(&spec);
        assert_ne!(
            checked.outcome(),
            Outcome::Rejected,
            "{media}: {checked:#?}"
        );
    }
}

#[test]
fn sse_response_body_generates() {
    // a `text/event-stream` (SSE) success response is now a typed stream, not `E009`. It
    // generates without the code firing, and check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
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
fn ndjson_response_body_generates() {
    // an `application/x-ndjson` success response is a typed stream, not `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /lines:
    get:
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson:
              schema: { type: string }
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
fn json_alternative_wins_over_stream_media_on_same_response() {
    // When a response offers BOTH a whole-body (JSON) and a streaming alternative, media selection
    // deterministically picks JSON — the operation is a normal `ResponseValue<T>`, not a stream —
    // and generation succeeds with no `E009`, disclosing the passed-over stream as `W014`.
    //
    // A clean report alone cannot see the selection: a JSON-plus-stream response generates either
    // way, so the fixture reads the emitted method. One case per streaming framing, each spelled the
    // way its version lowers it to a stream (3.1 `schema`, 3.2 `itemSchema`), with the stream both
    // before and after the JSON key, so neither source order nor framing decides the winner. The
    // control drops the JSON key and requires `EventStream<T>`, proving each stream spelling does
    // lower to a stream here — without it, "no `EventStream`" would hold for a stream that simply
    // never classified.
    let json = "application/json:\n              \
                schema: { type: object, required: [id], properties: { id: { type: string } } }";
    for (version, keyword) in [("3.1.0", "schema"), ("3.2.0", "itemSchema")] {
        for media in [
            "text/event-stream",
            "application/x-ndjson",
            "application/json-seq",
        ] {
            let stream = format!("{media}:\n              {keyword}: {{ type: object }}");
            for (first, second) in [(&stream as &str, json), (json, &stream as &str)] {
                let spec = format!(
                    r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /both:
    get:
      operationId: getBoth
      responses:
        "200":
          description: OK
          content:
            {first}
            {second}
"##
                );
                let (report, code) = generate_with_code(&spec);
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{media} {version}: {report:#?}"
                );
                assert!(
                    !has_code(&report, Code::UnsupportedMediaType),
                    "{media} {version}: {report:#?}"
                );
                assert_eq!(
                    messages_for(&report, Code::AlternativeMediaIgnored),
                    [format!(
                        "`application/json` is selected; the alternative media type(s) \
                         `{media}` are not"
                    )],
                    "{media} {version}: {report:#?}"
                );
                let client = types_module(&code);
                assert!(
                    !client.contains("EventStream<"),
                    "{media} {version}: the stream was selected over JSON: {client}"
                );
                // The success value is the JSON schema's type — the one declaring `id` — and not
                // the stream's open object.
                let flat: String = client.split_whitespace().collect();
                assert!(
                    flat.contains(
                        "pubasyncfnget_both(&self,)->Result<support::ResponseValue<types::ResponseBody>,"
                    ),
                    "{media} {version}: the JSON body is not the success value: {client}"
                );
                assert_eq!(
                    field_owner(&client, "pub id:").as_deref(),
                    Some("ResponseBody"),
                    "{media} {version}: {client}"
                );

                // Drop the JSON key together with the indentation of the line it sits on.
                let control = spec.replace(&format!("\n            {json}"), "");
                assert_ne!(control, spec, "the control must drop the JSON alternative");
                let (report, code) = generate_with_code(&control);
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{media} {version}: {report:#?}"
                );
                assert!(
                    types_module(&code).contains("EventStream<"),
                    "{media} {version}: the control did not stream: {code}"
                );
            }
        }
    }
}

#[test]
fn e009_streaming_request_body_rejected() {
    // Streaming media is response-only: a `text/event-stream` REQUEST body has no representation and
    // stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /push:
    post:
      requestBody:
        content:
          text/event-stream:
            schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

/// The one message the streaming-position rejection carries, so each fixture below proves it is
/// *this* gate that fired rather than some other `E009`.
const STREAM_POSITION_MESSAGE: &str = "is only supported as an operation's single success body";

/// Generate `spec`, require it rejected with the streaming-position `E009`, and hold `check` to the
/// same verdict.
fn assert_stream_position_rejected(spec: &str) {
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        messages_for(&report, Code::UnsupportedMediaType)
            .iter()
            .any(|message| message.contains(STREAM_POSITION_MESSAGE)),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        messages_for(&checked, Code::UnsupportedMediaType)
            .iter()
            .any(|message| message.contains(STREAM_POSITION_MESSAGE)),
        "{checked:#?}"
    );
}

#[test]
fn e009_streaming_default_that_is_also_the_error_body_is_rejected() {
    // #120: a lone streaming `default` is the operation's success source (so the method returns
    // `EventStream<T>`) *and* its error body (`default` documents every undeclared status). The
    // error side is a whole-body decode, so a non-2xx `text/event-stream` body would be handed to
    // the JSON decoder. Nothing can consume the framing there, so it is rejected, as a streaming
    // request body is.
    assert_stream_position_rejected(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        default:
          description: Events
          content:
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##,
    );
}

#[test]
fn e009_streaming_error_status_body_is_rejected() {
    // A streaming body on an explicit error status (or on a `default` beside a declared success)
    // lands in the whole-body error classification, which would decode NDJSON/SSE as one JSON
    // document.
    for errors in [
        r#""4XX":
          description: Failure
          content:
            application/x-ndjson:
              schema: { type: string }"#,
        r#"default:
          description: Failure
          content:
            text/event-stream:
              schema: { type: string }"#,
    ] {
        assert_stream_position_rejected(&format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /events:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: {{ type: string }}
        {errors}
"##
        ));
    }
}

#[test]
fn e009_streaming_body_in_a_multi_status_success_enum_is_rejected() {
    // Streaming is scoped to the single bodied success: beside a second bodied success status the
    // operation lowers to a success enum whose arms decode whole bodies, so the stream would be
    // read as one JSON document (#134: the SSE body `data: {"n":1}` came back as `Decode`). One
    // fixture per streaming framing, each spelled the way its version lowers it to a stream (3.1
    // `schema`, 3.2 `itemSchema`), with the stream on either side of the JSON status so the gate
    // does not depend on which arm the enum decodes first.
    let item = "{ type: object, required: [n], properties: { n: { type: integer } } }";
    let json = "{ type: object, required: [m], properties: { m: { type: string } } }";
    for (version, keyword) in [("3.1.0", "schema"), ("3.2.0", "itemSchema")] {
        for media in [
            "text/event-stream",
            "application/x-ndjson",
            "application/json-seq",
        ] {
            for (stream_status, json_status) in [("200", "201"), ("201", "200")] {
                let spec = format!(
                    r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /events:
    get:
      responses:
        "{stream_status}":
          description: Stream
          content:
            {media}:
              {keyword}: {item}
        "{json_status}":
          description: Made
          content:
            application/json:
              schema: {json}
"##
                );
                // The same stream alone, beside a bodyless status instead of the second bodied
                // one, is the supported shape: it proves this media lowers to a stream here, so
                // the rejection below is the multi-status gate and not a failure to recognise it.
                let control = spec.replace(
                    &format!(
                        "\"{json_status}\":\n          description: Made\n          content:\n            \
                         application/json:\n              schema: {json}\n"
                    ),
                    &format!("\"{json_status}\": {{ description: Made }}\n"),
                );
                assert_ne!(control, spec, "the control must drop the second body");
                let (report, code) = generate_with_code(&control);
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{media} {version}: {report:#?}"
                );
                assert!(code.contains("EventStream<"), "{media} {version}: {code}");

                assert_stream_position_rejected(&spec);
            }
        }
    }
}

#[test]
fn a_streaming_success_beside_whole_body_errors_still_generates() {
    // The control for the rejections above: the stream as the single bodied success, beside a
    // bodyless success, a JSON error status, and a JSON `default`, is the supported shape.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        "200":
          description: Stream
          content:
            text/event-stream:
              schema: { type: string }
        "204": { description: Nothing yet }
        "404":
          description: Missing
          content:
            application/json:
              schema: { type: string }
        default:
          description: Failure
          content:
            application/json:
              schema: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("EventStream<"), "{code}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    assert_ne!(check(spec).outcome(), Outcome::Rejected, "{report:#?}");
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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

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
        assert_ne!(check(&spec).outcome(), Outcome::Rejected, "{range}");
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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

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
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }

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

#[test]
fn e009_a_concrete_type_outside_the_byte_families_stays_unsupported() {
    // `application/*` mixes binary (`pdf`) with textual (`sdp`) subtypes and `font/*` is not
    // claimed, so none of them may be read as bytes on the strength of a prefix. This is the
    // response-side twin of `e009_unsupported_media_type`, and what keeps the openai corpus
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
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "{media}: {report:#?}"
            );
        }
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
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "{media}: {report:#?}"
            );
        }
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
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn a_concrete_media_type_outranks_a_range_that_precedes_it() {
    // Ranges rank below every codec, so a concrete sibling wins wherever it sits in the document
    // (the one type ranked below the ranges is a concrete `image`/`audio`/`video` member, and only
    // on a response; pinned elsewhere). Two ranges at the same rank still tie by source order, as
    // equal-ranked concrete media already do.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            application/json: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // JSON won, so the alternative that was dropped is the range — and that one really is a
    // narrowing, because a JSON body and an opaque one decode differently.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
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
    for report in [generate(spec), check(spec)] {
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
fn e009_a_media_range_cannot_be_a_request_content_type() {
    // A range describes what a server may return, not what a client sends. A generated request puts
    // its media key on the wire verbatim, and `Content-Type: video/*` is not a dispatchable header
    // — while choosing a concrete member of the family would be spargen inventing what the document
    // declined to say.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w014_still_fires_when_an_octet_alternative_constrains_a_shape() {
    // The suppression may only cover alternatives that really do decode identically. An
    // octet-classified alternative carrying an object schema would be *rejected* by the octet gate,
    // not turned into bytes, so it is a genuine narrowing and must still be reported.
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
            application/octet-stream: {}
            video/*: { schema: { type: object, properties: { a: { type: string } } } }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }

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
    for report in [generate(with_encoding), check(with_encoding)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_still_fires_when_an_alternative_carries_a_validation_keyword() {
    // A validation keyword never changes the storage type, so both bodies are `bytes::Bytes` — but
    // the document still said something about one of them, and an alternative that says something
    // is not interchangeable with one that says nothing.
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
            video/*: { schema: { maxLength: 10 } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_the_always_false_schema() {
    // `false` is the one boolean schema that constrains — to nothing at all. It is not the empty
    // schema and must not be read as one.
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
            video/*: { schema: false }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_every_kind_of_constraining_alternative() {
    // `constrains_nothing` is a long conjunction, and dropping any one term would silently widen
    // the suppression — the exhaustive destructure it is built on catches a field that is *added*
    // to `Schema`, never one that stops being consulted. Each schema below trips exactly one term,
    // so deleting that term turns this test red instead of turning a real narrowing silent.
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
            application/octet-stream: {{ schema: {{}} }}
            video/*: {{ schema: {constraining} }}
"##
        );
        let report = generate(&spec);
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "an alternative constrained by `{constraining}` was suppressed as decoding \
             identically: {report:#?}"
        );
    }
}

#[test]
fn a_binary_body_whose_schema_failed_to_lower_is_not_retyped_to_bytes() {
    // Retyping to `Bytes` applies to a body that *said nothing*. A body that declared a schema
    // which then failed to lower has already reported its own failure, and quietly handing it back
    // as bytes would turn that error into a silently different type.
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
            application/octet-stream: { schema: { $ref: "#/components/schemas/Loop" } }
components:
  schemas:
    Loop: { $ref: "#/components/schemas/Loop" }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
    }
}

#[test]
fn a_media_range_is_matched_case_insensitively() {
    // Media types are case-insensitive (RFC 9110 § 8.3.1). Reading `TEXT/*` as a binary family
    // would be silently wrong rather than loudly unsupported.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            TEXT/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "an upper-case text range is still text: {code}"
    );
}

#[test]
fn e009_a_range_naming_no_family_is_unsupported() {
    // `/*` has no type before the slash, so it names nothing. It stays unsupported rather than
    // being read as opaque octets on the strength of its suffix alone.
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
            "/*": { schema: {} }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn e009_a_media_range_with_an_extra_slash_is_unsupported() {
    // `a/b/*` ends in `/*`, but what precedes the suffix is `a/b`, which is not a type name. It was
    // read as the family `a/b` and generated as opaque octets. A key that is not a media range names
    // no family, so it is unsupported like any other key that is not a media type.
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
            "a/b/*": { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

/// Assert that each `(key, request, schema)` case, as the sole `content` key of a request body
/// (`request`) or a response, is rejected with `E009` through both `generate` and `check`.
fn assert_each_media_key_is_unsupported(cases: &[(&str, bool, &str)]) {
    fn document(key: &str, request: bool, schema: &str) -> String {
        if request {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "{key}": {{ schema: {schema} }}
      responses:
        "204": {{ description: No Content }}
"##
            )
        } else {
            format!(
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
            "{key}": {{ schema: {schema} }}
"##
            )
        }
    }
    for &(key, request, schema) in cases {
        let spec = document(key, request, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{key}` through {entry}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "`{key}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_media_key_that_is_not_a_restricted_name_is_unsupported() {
    // A media type is exactly one `/` between two RFC 6838 § 4.2 restricted names. Each key below
    // breaks that, yet most reached a codec through an arm that matched only part of the key: the
    // `text/` prefix, the `application/…+json` suffix, or a range's `/*`. `image/jpeg/extra` pins
    // the concrete binary family, which must not be read as `image` octets.
    // One byte past the 127-byte limit on a restricted name.
    let too_long = format!("text/{}", "a".repeat(128));
    assert_each_media_key_is_unsupported(&[
        ("image/jpeg/extra", true, "{}"),
        ("text/plain/extra", false, "{ type: string }"),
        ("application/vnd.a/b+json", true, "{ type: object }"),
        ("text/", false, "{ type: string }"),
        ("text/pl ain", false, "{ type: string }"),
        ("**/*", false, "{}"),
        (too_long.as_str(), false, "{ type: string }"),
    ]);
}

#[test]
fn e009_a_wildcard_inside_a_name_is_unsupported() {
    // `*` is a whole-name wildcard, never part of a name: `*/json` is no range (a range fixes the
    // type and wildcards the subtype), and `image/pn*` is no type at all. Neither ever reached an
    // arm, so `text/pl*in` and `application/vn*+json` are here to discriminate: without the rule,
    // the `text/` prefix arm and the `+json` suffix arm would accept them.
    assert_each_media_key_is_unsupported(&[
        ("*/json", false, "{ type: object }"),
        ("image/pn*", false, "{}"),
        ("text/pl*in", false, "{ type: string }"),
        ("application/vn*+json", false, "{ type: object }"),
    ]);
}

#[test]
fn e009_a_name_starting_with_a_symbol_is_unsupported() {
    // `.`, `-` and the other symbols RFC 6838 § 4.2 permits may follow the first byte of a
    // restricted name but may not be it, in the type position or the subtype position.
    assert_each_media_key_is_unsupported(&[
        (".type/x", false, "{ type: string }"),
        ("-x/y", false, "{ type: string }"),
        ("text/.plain", false, "{ type: string }"),
        ("text/-plain", false, "{ type: string }"),
    ]);
}

#[test]
fn w014_a_malformed_key_beside_a_well_formed_sibling_is_ignored() {
    // A malformed key does not reject the whole map when a well-formed sibling exists: only that
    // key is dropped, and it is named under W014 like any other alternative that is not generated.
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
            "text/plain/extra": { schema: { type: string } }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::AlternativeMediaIgnored
                    && diagnostic.message
                        == "`application/json` is selected; the alternative media type(s) \
                            `text/plain/extra` are not"
            }),
            "{report:#?}"
        );
    }
    assert!(code.contains("pub type ResponseBodyid = i64;"), "{code}");
}

#[test]
fn well_formed_media_keys_still_generate() {
    // The restricted-name check must not reject what real descriptions write: dotted vendor `+json`
    // types, a key with parameters (stripped before the check), both kinds of range, every
    // non-alphanumeric byte RFC 6838 permits, and a subtype at exactly the 127-byte limit.
    let at_limit = format!("text/{}", "a".repeat(127));
    let keys = [
        ("application/vnd.github+json", "{ type: object }"),
        ("application/vnd.github.v3.star+json", "{ type: object }"),
        ("text/plain; charset=utf-8", "{ type: string }"),
        ("*/*", "{}"),
        ("application/*", "{}"),
        ("text/x-a!b#c$d&e^f_g.h+i", "{ type: string }"),
        (at_limit.as_str(), "{ type: string }"),
    ];
    // One operation per key, each with a single content entry, so no key competes with another.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for (index, (key, schema)) in keys.into_iter().enumerate() {
        spec += &format!(
            r##"  /op{index}:
    get:
      operationId: op{index}
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
        );
    }
    let (report, code) = generate_with_code(&spec);
    let checked = check(&spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
    }
    // Seven operations share the `ResponseBody` name, so each alias carries a disambiguating suffix.
    let opaque = code
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("pub type ResponseBody") && line.ends_with(" = bytes::Bytes;")
        })
        .count();
    assert_eq!(
        opaque, 2,
        "exactly the `*/*` and `application/*` ranges are opaque octets: {code}"
    );
}

#[test]
fn a_structured_suffix_range_response_generates_json() {
    // `application/*+json` is a media range over every structured JSON subtype (RFC 9110's
    // media-range grammar, with RFC 6838 § 4.2.8 structured syntax suffixes). The support matrix
    // promises it as JSON, and a response offering it alone is decoded as JSON rather than
    // rejected for its `*`.
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
            "application/*+json":
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
    }
    assert!(
        code.contains("pub struct ResponseBody {")
            && code.contains("pub type ResponseBodyid = i64;"),
        "a typed JSON body, not bytes or text: {code}"
    );
}

#[test]
fn w014_a_structured_suffix_range_ties_its_codec_on_a_response() {
    // Unlike a family range, which every concrete sibling outranks, `application/*+json` ranks
    // level with `application/json` (the support matrix and `E009` say so). On a response nothing
    // withholds it, so the tie is broken by source order: whichever is listed first is generated,
    // and the other is reported as the alternative that is not. The two schemas differ so the
    // generated body shows which key won.
    let range = r#""application/*+json": { schema: { type: object, required: [fromRange], properties: { fromRange: { type: integer } } } }"#;
    let json = r#""application/json": { schema: { type: object, required: [fromJson], properties: { fromJson: { type: integer } } } }"#;
    for (first, second, generated, other, field, other_field) in [
        (
            range,
            json,
            "application/*+json",
            "application/json",
            "from_range",
            "from_json",
        ),
        (
            json,
            range,
            "application/json",
            "application/*+json",
            "from_json",
            "from_range",
        ),
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
            {first}
            {second}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert!(
            code.contains(&format!("pub {field}: "))
                && !code.contains(&format!("pub {other_field}: ")),
            "`{generated}` listed first is generated, `{other}` is not: {code}"
        );
        let expected =
            format!("`{generated}` is selected; the alternative media type(s) `{other}` are not");
        for report in [report, check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_cannot_be_a_request_content_type() {
    // A request puts its media key on the wire verbatim, and `Content-Type: application/*+json`
    // names a family rather than a type, exactly like `video/*`. It is rejected as a range.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message.contains("is a media range")),
            "{report:#?}"
        );
    }
}

/// A request body whose `content` lists `application/*+json` first and then `sibling`.
fn suffix_range_request_document(sibling: &str, sibling_schema: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": {{ schema: {{ type: object }} }}
          "{sibling}": {{ schema: {sibling_schema} }}
      responses:
        "204": {{ description: No Content }}
"##
    )
}

#[test]
fn w014_a_structured_suffix_range_yields_to_a_sendable_request_sibling() {
    // `application/*+json` ranks with the concrete JSON types, so listed first it would win the
    // tie by source order and then be refused as a range, rejecting a body that offered something
    // sendable. While a concrete sibling can be sent, the range is not a candidate: the sibling is
    // generated and the range is reported as the alternative that is not, whatever the sibling's
    // rank (`text/plain` ranks below every JSON type).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("application/merge-patch+json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            let expected = format!(
                "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_with_no_sendable_request_sibling_is_unsupported() {
    // With nothing beside it that a request could send, the suffix range is still what the body
    // offers, and it is still refused as a range. Neither another range nor a streaming media
    // counts as sendable.
    for (sibling, schema) in [("video/*", "{}"), ("text/event-stream", "{ type: string }")] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::UnsupportedMediaType
                        && diagnostic
                            .message
                            .starts_with("media type `application/*+json` is a media range")
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_a_suffix_range_listed_after_a_sendable_request_sibling_is_withheld() {
    // Order does not decide it. A sendable sibling listed before the range is generated and the
    // range is named as not generated, even for a sibling whose rank the range's rank 0 would
    // otherwise beat (`text/plain`).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = request_body_document(&[
            (sibling, schema),
            ("application/*+json", "{ type: object }"),
        ]);
        let expected = format!(
            "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert_eq!(
                messages_with_code(&report, Code::AlternativeMediaIgnored),
                [expected.as_str()],
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_every_suffix_range_beside_a_sendable_request_sibling_is_withheld() {
    // Every structured-suffix range that classifies is withheld, not only the first, and all of
    // them are named in one report.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/*+json-seq", "{ type: object }"),
        ("application/json", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json`, `application/*+json-seq` are not"
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_suffix_range_beside_only_an_unclassified_request_sibling_is_a_media_range() {
    // A sibling that does not classify is not sendable, so nothing is withheld: the range is still
    // the only thing the body offers, and it is refused as a range.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/pdf", "{}"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            messages_with_code(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message
                    .starts_with("media type `application/*+json` is a media range")),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_sendable_request_sibling_that_fails_its_own_gate_is_reported_for_itself() {
    // Sendable is decided by classification alone. A `text/plain` sibling carrying an object schema
    // is still chosen over the range, and then refused by the raw-text gate for its own reason
    // rather than the range's. Neither the withheld range nor anything else is then reported as
    // passed over for a `text/plain` selection that is then refused (#110).
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("text/plain", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        let unsupported = messages_with_code(&report, Code::UnsupportedMediaType);
        assert!(
            unsupported
                .iter()
                .all(|message| !message.contains("is a media range")),
            "{report:#?}"
        );
        assert!(
            unsupported
                .iter()
                .any(|message| message.contains("text/plain")),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_a_withheld_suffix_range_beside_two_request_entries_is_reported_separately() {
    // Pinned as it stands. `choose_media` names the alternatives it passed over, and the withheld
    // range gets its own W014 right after, so this body carries two: both true, always in this
    // order, and both emitted only once the selection has passed every request-body gate.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/json", "{ type: object }"),
        ("application/xml", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/xml` are not",
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json` are not",
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_rejected_response_selection_claims_nothing_is_generated() {
    // The response side of #110: `text/plain` is selected over `text/html` (same rank, listed
    // first) and refused by the raw-text gate, so its `E009` alone reports it and no `W014` names
    // `text/html` as passed over.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: object } }
            text/html: { schema: { type: object } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::UnsupportedMediaType),
            ["media type `text/plain` requires a string-like or binary response schema"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_an_accepted_response_selection_still_reports_its_alternatives() {
    // The counterpart: the same shape with a string schema passes the gate, so the narrowing it
    // makes is still disclosed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string } }
            text/html: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_on_a_document_rejected_elsewhere_claims_only_the_selection() {
    // `/page` passes every one of its own gates, so its `W014` is emitted; `/doc` is rejected, so
    // the run generates nothing — and `check` never generates on any document. The message must
    // therefore assert only what its emission site decides, the selection, and never that anything
    // "is generated" (#174). The `openai_openapi` corpus snapshot carries this shape at scale:
    // `Rejected` with `E009` beside many `W014`s.
    //
    // This pins the wording. The principle is held by `run_generate`/`run_check`, which compare
    // each diagnostic's structured `claim` with the run's outcome (#413); see
    // `a_generation_claim_on_w014_is_contradicted_wherever_174_found_it_false`.
    let spec = W014_REJECTED_ELSEWHERE;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn a_sequential_media_outranks_a_text_range() {
    // `text/*` used to classify as `Text` by accident of the `text/` prefix arm, at the same rank
    // as a concrete textual type and *above* sequential media — so this response was a whole-body
    // `String`. As a range it now ranks below both, and the concrete streaming type wins. That is a
    // change to the generated public API, so it is pinned rather than left to be rediscovered.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    // Two media types that decode differently: the range really is not generated.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}
