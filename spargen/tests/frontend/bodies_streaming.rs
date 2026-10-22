//! Request and response bodies, first part: OAS 3.2 media-type components, streaming media
//! (`itemSchema`, `contentSchema`, SSE, NDJSON and JSON sequences), the basic `E009` media
//! rejections, and the single-success-body position a stream must hold.

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
}

/// Media types are case-insensitive (RFC 9110 § 8.3.1), so an SSE key spelled in another letter
/// case is the same stream: it is lowered, its `contentSchema` types the payload, and a validation
/// keyword inside that schema warns exactly as it does under the lowercase key (#709).
#[test]
fn an_sse_key_in_another_letter_case_is_the_same_stream() {
    let spec = |media: &str| {
        format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        '200':
          description: ok
          content:
            {media}:
              itemSchema:
                type: object
                required: [data]
                properties:
                  data:
                    type: string
                    contentMediaType: application/json
                    contentSchema: {{ type: string, maxLength: 5 }}
"##
        )
    };
    let (lower_report, lower_code) = generate_with_code(&spec("text/event-stream"));
    assert_ne!(
        lower_report.outcome(),
        Outcome::Rejected,
        "{lower_report:#?}"
    );
    for media in ["TEXT/EVENT-STREAM", "Text/Event-Stream"] {
        let spec = spec(media);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            code.contains("EventStream"),
            "{media} must lower as a stream: {code}"
        );
        assert_eq!(
            has_code(&report, Code::ValidationKeywordIgnored),
            has_code(&lower_report, Code::ValidationKeywordIgnored),
            "{media} must warn as `text/event-stream` does: {report:#?}"
        );
        assert_eq!(
            messages_for(&report, Code::ValidationKeywordIgnored),
            messages_for(&lower_report, Code::ValidationKeywordIgnored),
            "{media}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
        assert_eq!(
            code.matches("EventStream<").count(),
            lower_code.matches("EventStream<").count(),
            "{media}"
        );
    }
}

#[test]
fn a_body_key_in_another_letter_case_classifies_as_its_lowercase_spelling() {
    // One case rule for every media key, not only the stream: `Application/JSON` is the JSON body
    // `application/json` is, on a request and on a response: the same client, up to any place the
    // document's spelling of the key is quoted.
    let spec = |media: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          {media}:
            schema: {{ $ref: '#/components/schemas/Pet' }}
      responses:
        '200':
          description: ok
          content:
            {media}:
              schema: {{ $ref: '#/components/schemas/Pet' }}
components:
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        name: {{ type: string }}
"##
        )
    };
    let (lower_report, lower_code) = generate_with_code(&spec("application/json"));
    assert_ne!(
        lower_report.outcome(),
        Outcome::Rejected,
        "{lower_report:#?}"
    );
    for media in ["Application/JSON", "APPLICATION/JSON"] {
        let spec = spec(media);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{media}: {report:#?}"
        );
        assert_eq!(
            module_body(&code.replace(media, "application/json")),
            module_body(&lower_code),
            "{media} must generate what `application/json` does"
        );
    }
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
fn oas32_item_schema_on_a_non_streaming_request_body_warns_w010() {
    // The request-body counterpart of the response fixture above, lowered by its own site: the
    // body is sent by `schema`, and the `itemSchema` beside it is acknowledged, not dropped.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    post:
      requestBody:
        content:
          application/json:
            schema: { type: string }
            itemSchema: { type: integer }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::Oas32ConstructIgnored),
            [
                "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; on this \
                 request body it is not used"
            ],
            "{report:#?}"
        );
    }
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
}
