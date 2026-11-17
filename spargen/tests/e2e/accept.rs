//! The `Accept` a request sends (#1401): the essence of each bodied success response's chosen
//! media, once each in document order, so a stream asks for its framing and a response declaring
//! several media asks for the one its decoder reads; none where no success is bodied, or where the
//! chosen key is a structured-suffix range. Driven over a local server, which records the head of
//! each request.

use super::*;

/// A request asks, in `Accept`, for the media every bodied success response is decoded from: a
/// stream for its framing, a response declaring several media for the one the decoder reads, and
/// two bodied statuses for both. An operation with no bodied success leaves `Accept` to the HTTP
/// stack.
#[test]
fn accept_names_the_media_each_success_is_decoded_from() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Accept, version: 1.0.0 }
paths:
  /chat/stream:
    get:
      operationId: streamChat
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { $ref: "#/components/schemas/Chunk" }
  /chat/lines:
    get:
      operationId: streamChatLines
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson:
              schema: { $ref: "#/components/schemas/Chunk" }
  /pets/{id}:
    get:
      operationId: getPet
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema: { $ref: "#/components/schemas/Pet" }
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
    delete:
      operationId: deletePet
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "204":
          description: deleted
  /problems:
    get:
      operationId: getProblem
      responses:
        "200":
          description: any JSON-suffixed problem document
          content:
            application/*+json:
              schema: { $ref: "#/components/schemas/Pet" }
  /reports:
    post:
      operationId: makeReport
      responses:
        "200":
          description: the report
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
        "202":
          description: queued
          content:
            text/plain:
              schema: { type: string }
        "204":
          description: nothing to report
components:
  schemas:
    Chunk:
      type: object
      properties:
        text: { type: string }
    Pet:
      type: object
      properties:
        name: { type: string }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "accept_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "accept",
        r##"#![cfg(feature = "blocking")]

mod common;

fn client(base: &str) -> accept_client::BlockingClient {
    accept_client::BlockingClient::new(base).unwrap()
}

#[test]
fn a_stream_asks_for_its_framing() {
    let (base, server) = common::serve_once("text/event-stream", "200 OK", b"");
    drop(client(&base).stream_chat().expect("the stream opens"));
    let request = server.join().unwrap();
    assert_eq!(request.header("accept"), ["text/event-stream"], "{}", request.head);

    let (base, server) = common::serve_once("application/x-ndjson", "200 OK", b"");
    drop(client(&base).stream_chat_lines().expect("the stream opens"));
    let request = server.join().unwrap();
    assert_eq!(request.header("accept"), ["application/x-ndjson"], "{}", request.head);
}

#[test]
fn a_response_declaring_several_media_asks_for_the_one_it_decodes() {
    let (base, server) = common::serve_once("application/json", "200 OK", br#"{"name":"a"}"#);
    client(&base).get_pet("1").expect("the JSON pet decodes");
    let request = server.join().unwrap();
    assert_eq!(request.header("accept"), ["application/json"], "{}", request.head);
}

#[test]
fn two_bodied_statuses_ask_for_both_media() {
    let (base, server) = common::serve_once("text/plain", "202 Accepted", b"queued");
    client(&base).make_report().expect("the queued reply decodes");
    let request = server.join().unwrap();
    assert_eq!(
        request.header("accept"),
        ["application/json, text/plain"],
        "{}",
        request.head
    );
}

#[test]
fn a_structured_suffix_range_leaves_accept_to_the_http_stack() {
    // `application/*+json` is no RFC 9110 media range, so it is never sent as one.
    let (base, server) =
        common::serve_once("application/problem+json", "200 OK", br#"{"name":"a"}"#);
    client(&base).get_problem().expect("the suffixed JSON decodes");
    let request = server.join().unwrap();
    assert_eq!(request.header("accept"), ["*/*"], "{}", request.head);
}

#[test]
fn no_bodied_success_leaves_accept_to_the_http_stack() {
    let (base, server) = common::serve_no_content();
    client(&base).delete_pet("1").expect("the delete succeeds");
    let request = server.join().unwrap();
    assert_eq!(request.header("accept"), ["*/*"], "{}", request.head);
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "accept"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each request must ask for the media its success decodes"
    );
}
