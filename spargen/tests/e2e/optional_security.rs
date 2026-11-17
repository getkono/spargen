//! An optional (`{}`) security requirement (#1449): a generated client sends a registered
//! credential wherever `{}` is listed, and sends anonymously only when none is registered.

use super::*;

/// An operation listing `{}` before a credentialed alternative is security-optional, not
/// anonymous-first: a generated client with a bearer token registered sends it, and one with
/// nothing registered sends the call with no `Authorization` header and no error. Both orders
/// behave the same, which only the request a server reads can show.
#[test]
fn an_optional_security_requirement_sends_a_registered_credential() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: OptionalAuth, version: "1" }
paths:
  /items:
    get:
      operationId: listItems
      security: [ {}, { bearer: [] } ]
      responses:
        "204": { description: listed }
  /tags:
    get:
      operationId: listTags
      security: [ { bearer: [] }, {} ]
      responses:
        "204": { description: listed }
components:
  securitySchemes:
    bearer: { type: http, scheme: bearer }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "optional_auth_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "optional_auth", OPTIONAL_AUTH_TEST);
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "optional_auth"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "an optional requirement must send a registered credential"
    );
}

const OPTIONAL_AUTH_TEST: &str = r##"#![cfg(feature = "blocking")]

mod common;

#[test]
fn a_registered_bearer_is_sent_wherever_the_empty_alternative_is_listed() {
    let (base, server) = common::serve(vec![common::no_content(); 2]);
    let client = optional_auth_client::BlockingClient::new(&base)
        .unwrap()
        .with_bearer(optional_auth_client::SecretString::from("t0k3n"));
    client.list_items().expect("list_items round-trips");
    client.list_tags().expect("list_tags round-trips");
    for request in server.join().unwrap() {
        assert_eq!(request.header("authorization"), ["Bearer t0k3n"], "{}", request.line());
    }
}

#[test]
fn nothing_registered_sends_anonymously_wherever_the_empty_alternative_is_listed() {
    let (base, server) = common::serve(vec![common::no_content(); 2]);
    let client = optional_auth_client::BlockingClient::new(&base).unwrap();
    client.list_items().expect("list_items round-trips");
    client.list_tags().expect("list_tags round-trips");
    for request in server.join().unwrap() {
        assert!(request.header("authorization").is_empty(), "{}", request.text());
    }
}
"##;
