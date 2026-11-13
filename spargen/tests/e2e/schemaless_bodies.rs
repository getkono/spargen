//! Request bodies declared with no `schema` (#1339): each is sent as the type the `{}` schema takes
//! under its media — any JSON value, any text, any octets — under its declared `Content-Type`,
//! required or not. Compiled and linted with its `testing` module, and driven over the typed
//! `MockBackend`, which records the bytes each request sends.

use super::*;

/// One operation per media whose schema-less body is typed, the text one optional.
const SCHEMALESS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Schemaless, version: 1.0.0 }
paths:
  /json:
    post:
      operationId: postJson
      requestBody:
        required: true
        content:
          application/merge-patch+json: {}
      responses:
        "204": { description: sent }
  /text:
    post:
      operationId: postText
      requestBody:
        content:
          text/plain: {}
      responses:
        "204": { description: sent }
  /octets:
    post:
      operationId: postOctets
      requestBody:
        required: true
        content:
          application/octet-stream: {}
      responses:
        "204": { description: sent }
"##;

#[test]
fn a_request_body_with_no_schema_reaches_the_wire() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "schemaless.yaml", SCHEMALESS_SPEC);
    let out = temp.path().join("client");
    let report =
        generate_configured_fixture_crate(&spec, &out, "schemaless_client", "2021", |spec| {
            spec.with_testing(spargen::TestingModule::Emitted)
        });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "schemaless", SCHEMALESS_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "schema-less request bodies must compile and pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "schemaless"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each schema-less request body must reach the wire under its media"
    );
}

const SCHEMALESS_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use schemaless_client::{ops, testing};

/// Drive a call to completion: the mock answers from memory, so nothing it awaits pends.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn client(mock: &testing::MockBackend) -> schemaless_client::Client {
    schemaless_client::Client::with_backend(Arc::new(mock.clone()), "http://mock").unwrap()
}

/// The one request `operation` sent: its body, and its `Content-Type` where it set one.
fn sent(mock: &testing::MockBackend, operation: ops::Operation) -> (Vec<u8>, Option<String>) {
    let calls = mock.calls(operation);
    assert_eq!(calls.len(), 1);
    (
        calls[0].body().to_vec(),
        calls[0].header("content-type").map(str::to_owned),
    )
}

fn no_content() -> testing::Reply {
    testing::Reply::new(204)
}

#[test]
fn any_json_value_is_sent_as_json_under_its_media() {
    let mock = testing::MockBackend::new()
        .on_post_json(|_| testing::post_json::Reply::from_reply(no_content()));
    let patch = serde_json::json!({ "name": "rex", "tags": null });
    block_on(client(&mock).post_json(&patch)).unwrap();
    let (body, content_type) = sent(&mock, ops::POST_JSON);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), patch);
    assert_eq!(content_type.as_deref(), Some("application/merge-patch+json"));
}

#[test]
fn any_text_is_sent_as_itself_and_an_absent_body_sends_nothing() {
    let mock = testing::MockBackend::new()
        .on_post_text(|_| testing::post_text::Reply::from_reply(no_content()));
    block_on(client(&mock).post_text(Some(&"a \"quoted\" note".to_owned()))).unwrap();
    assert_eq!(
        sent(&mock, ops::POST_TEXT),
        (b"a \"quoted\" note".to_vec(), Some("text/plain".to_owned()))
    );

    let mock = testing::MockBackend::new()
        .on_post_text(|_| testing::post_text::Reply::from_reply(no_content()));
    block_on(client(&mock).post_text(None)).unwrap();
    assert_eq!(sent(&mock, ops::POST_TEXT), (Vec::new(), None));
}

#[test]
fn any_octets_are_sent_verbatim() {
    let mock = testing::MockBackend::new()
        .on_post_octets(|_| testing::post_octets::Reply::from_reply(no_content()));
    block_on(client(&mock).post_octets(vec![0_u8, 159, 255])).unwrap();
    assert_eq!(
        sent(&mock, ops::POST_OCTETS),
        (vec![0_u8, 159, 255], Some("application/octet-stream".to_owned()))
    );
}
"##;
