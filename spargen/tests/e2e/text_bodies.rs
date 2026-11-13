//! `text/plain` bodies of every scalar schema (#1323): a request body is rendered as the bare text
//! of its value, never JSON-quoted, whether its schema is `{}`, an integer, a number, a boolean or
//! a union of strings; and a response body of each decodes from that same text, through the single
//! body codec and a multi-status arm alike. Compiled and linted with its `testing` module, and
//! driven over the typed `MockBackend`, which records the bytes each request sends.

use super::*;

/// One echo operation per scalar schema under `text/plain`, and one multi-status success whose two
/// arms are an integer and a boolean.
const TEXT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Text, version: 1.0.0 }
components:
  schemas:
    Special:
      type: string
      enum: [special]
    Word:
      anyOf:
        - type: string
        - $ref: "#/components/schemas/Special"
paths:
  /count:
    post:
      operationId: echoCount
      requestBody:
        required: true
        content:
          text/plain: { schema: { type: integer, format: int64 } }
      responses:
        "200":
          description: the count
          content:
            text/plain: { schema: { type: integer, format: int64 } }
  /ratio:
    post:
      operationId: echoRatio
      requestBody:
        required: true
        content:
          text/plain: { schema: { type: number } }
      responses:
        "200":
          description: the ratio
          content:
            text/plain: { schema: { type: number } }
  /flag:
    post:
      operationId: echoFlag
      requestBody:
        required: true
        content:
          text/plain: { schema: { type: boolean } }
      responses:
        "200":
          description: the flag
          content:
            text/plain: { schema: { type: boolean } }
  /note:
    post:
      operationId: echoNote
      requestBody:
        required: true
        content:
          text/plain: { schema: {} }
      responses:
        "200":
          description: the note
          content:
            text/plain: { schema: {} }
  /word:
    post:
      operationId: echoWord
      requestBody:
        required: true
        content:
          text/plain: { schema: { $ref: "#/components/schemas/Word" } }
      responses:
        "200":
          description: the word
          content:
            text/plain: { schema: { $ref: "#/components/schemas/Word" } }
  /mixed:
    get:
      operationId: getMixed
      responses:
        "200":
          description: a count
          content:
            text/plain: { schema: { type: integer } }
        "201":
          description: a flag
          content:
            text/plain: { schema: { type: boolean } }
"##;

#[test]
fn text_bodies_of_every_scalar_schema_round_trip_as_bare_text() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "text.yaml", TEXT_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "text_client", "2021", |spec| {
        spec.testing(true)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "text", TEXT_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "text bodies of every scalar schema must compile and pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "text"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each text body must reach the wire as bare text and decode back"
    );
}

const TEXT_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use text_client::{ops, testing, types};

/// Drive a call to completion: the mock answers from memory, so nothing it awaits pends.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn client(mock: &testing::MockBackend) -> text_client::Client {
    text_client::Client::with_backend(Arc::new(mock.clone()), "http://mock").unwrap()
}

/// The one request `operation` sent: its body as text, and its `Content-Type`.
fn sent(mock: &testing::MockBackend, operation: ops::Operation) -> (String, String) {
    let calls = mock.calls(operation);
    assert_eq!(calls.len(), 1);
    (
        String::from_utf8(calls[0].body().to_vec()).unwrap(),
        calls[0].header("content-type").unwrap().to_owned(),
    )
}

#[test]
fn an_integer_is_sent_and_read_as_its_digits() {
    let mock = testing::MockBackend::new()
        .on_echo_count(|_| testing::echo_count::Reply::status_200(&42));
    let echoed = block_on(client(&mock).echo_count(&-7)).unwrap();
    assert_eq!(echoed.into_inner(), 42);
    assert_eq!(sent(&mock, ops::ECHO_COUNT), ("-7".to_owned(), "text/plain".to_owned()));
}

#[test]
fn a_number_is_sent_and_read_as_its_shortest_spelling() {
    let mock = testing::MockBackend::new()
        .on_echo_ratio(|_| testing::echo_ratio::Reply::status_200(&0.1));
    let echoed = block_on(client(&mock).echo_ratio(&1.5)).unwrap();
    assert_eq!(echoed.into_inner(), 0.1);
    assert_eq!(sent(&mock, ops::ECHO_RATIO).0, "1.5");
}

#[test]
fn a_boolean_is_sent_and_read_as_its_keyword() {
    let mock = testing::MockBackend::new()
        .on_echo_flag(|_| testing::echo_flag::Reply::status_200(&false));
    let echoed = block_on(client(&mock).echo_flag(&true)).unwrap();
    assert!(!echoed.into_inner());
    assert_eq!(sent(&mock, ops::ECHO_FLAG).0, "true");
}

#[test]
fn an_untyped_string_is_sent_unquoted() {
    let mock = testing::MockBackend::new().on_echo_note(|_| {
        testing::echo_note::Reply::status_200(&serde_json::Value::String("back".to_owned()))
    });
    let note = serde_json::Value::String("hi".to_owned());
    let echoed = block_on(client(&mock).echo_note(&note)).unwrap();
    assert_eq!(echoed.into_inner(), serde_json::Value::String("back".to_owned()));
    assert_eq!(sent(&mock, ops::ECHO_NOTE).0, "hi");
}

#[test]
fn a_union_of_strings_is_sent_as_the_text_of_its_variant() {
    let mock = testing::MockBackend::new().on_echo_word(|call| {
        let word: types::Word = serde_json::from_value(serde_json::Value::String(
            String::from_utf8(call.body().to_vec()).unwrap(),
        ))
        .unwrap();
        testing::echo_word::Reply::status_200(&word)
    });
    let word: types::Word = serde_json::from_value(serde_json::json!("special")).unwrap();
    let echoed = block_on(client(&mock).echo_word(&word)).unwrap();
    assert_eq!(
        serde_json::to_value(echoed.into_inner()).unwrap(),
        serde_json::json!("special")
    );
    assert_eq!(sent(&mock, ops::ECHO_WORD).0, "special");
}

#[test]
fn each_arm_of_a_multi_status_text_success_reads_its_own_scalar() {
    let mock = testing::MockBackend::new()
        .on_get_mixed(|_| testing::get_mixed::Reply::status_200(&12));
    let count = block_on(client(&mock).get_mixed()).unwrap().into_inner();
    assert_eq!(format!("{count:?}"), "Status200(12)");

    let mock = testing::MockBackend::new()
        .on_get_mixed(|_| testing::get_mixed::Reply::from_reply(testing::Reply::new(201).body("true")));
    let flag = block_on(client(&mock).get_mixed()).unwrap().into_inner();
    assert_eq!(format!("{flag:?}"), "Status201(true)");
}
"##;
