//! Header bindings (#1262): a client generated with `Spec::bind_header`, compiled and linted, that
//! sends the bound value from every operation declaring the header and refuses a required one
//! while none is bound.

use super::*;

/// `listItems` requires `X-Api-Version` beside a required `X-Trace`; `getStatus` declares it
/// optional, spelled in lower case.
const BOUND_HEADERS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Versioned, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - { name: X-Api-Version, in: header, required: true, schema: { type: string } }
        - { name: X-Trace, in: header, required: true, schema: { type: string } }
      responses:
        "204": { description: listed elsewhere }
  /status:
    get:
      operationId: getStatus
      parameters:
        - { name: x-api-version, in: header, schema: { type: string } }
      responses:
        "204": { description: up }
"##;

#[test]
fn a_bound_header_is_sent_from_the_client_and_required_where_declared() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "versioned.yaml", BOUND_HEADERS_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "bound_headers", "2021", |spec| {
        spec.bind_header("X-Api-Version")
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "bound", BOUND_HEADERS_TEST);
    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--all-targets",
            "--features",
            "blocking",
            "--",
            "-D",
            "warnings",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the bound client must pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "bound"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each operation must send the bound header, or refuse a required one left unbound"
    );
}

const BOUND_HEADERS_TEST: &str = r##"#![cfg(feature = "blocking")]

mod common;

#[test]
fn every_operation_declaring_the_header_sends_the_bound_value() {
    // Two requests reach the server: `getStatus` before anything is bound, and `listItems` after.
    let (base, server) = common::serve(vec![common::no_content(); 2]);
    let client = bound_headers::BlockingClient::new(&base).unwrap();

    // Unbound, the required header fails the call before anything is sent...
    match client.list_items("t-1") {
        Err(bound_headers::Error::RequestConstruction(
            bound_headers::RequestError::MissingBoundParameter { name },
        )) => assert_eq!(name, "X-Api-Version"),
        other => panic!("expected MissingBoundParameter, got {other:?}"),
    }
    // ...and the optional one is left off.
    assert_eq!(client.get_status().unwrap().status(), 204);

    // A binding in any letter case reaches each operation, in its own spelling of the header.
    let client = client.with_bound_header("x-API-version", "2026-01-01").unwrap();
    assert_eq!(client.list_items("t-2").unwrap().status(), 204);

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].header("x-api-version").is_empty(), "{}", requests[0].text());
    assert_eq!(requests[1].header("x-api-version"), ["2026-01-01"], "{}", requests[1].text());
    assert_eq!(requests[1].header("x-trace"), ["t-2"], "{}", requests[1].text());
}

/// Issue #1303: a bound value is held as a secret, so the request an operation builds marks it
/// sensitive, and that request's `Debug` (what a backend or middleware logging it prints) does not
/// carry it.
#[test]
fn the_built_request_marks_the_bound_value_sensitive() {
    let client = bound_headers::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_bound_header("X-Api-Version", "TENANTSECRET")
        .unwrap();
    let mut built = std::pin::pin!(client.list_items_request("t-1"));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(request) = std::future::Future::poll(built.as_mut(), &mut cx) else {
        panic!("a request needing no credential builds on the first poll");
    };
    let request = request.unwrap();
    let value = &request.headers()["x-api-version"];
    assert_eq!(value, "TENANTSECRET");
    assert!(value.is_sensitive());
    let debug = format!("{request:?}");
    assert!(!debug.contains("TENANTSECRET"), "{debug}");
    // The header the caller passes as an argument stays as it is.
    assert!(!request.headers()["x-trace"].is_sensitive());
}

/// Issue #1335: a value no request could carry as a header value is refused when it is bound,
/// on the async and the blocking client alike, as `RequestConstruction`, instead of failing every
/// later call; the refusal does not quote the value.
#[test]
fn an_invalid_bound_value_is_refused_when_it_is_bound() {
    let client = bound_headers::Client::new("http://127.0.0.1:1").unwrap();
    let error = client.with_bound_header("X-Api-Version", "a\r\nInjected: 1").unwrap_err();
    assert!(matches!(error, bound_headers::Error::RequestConstruction(_)), "{error:?}");
    assert!(!format!("{error:?}").contains("Injected"), "{error:?}");

    let blocking = bound_headers::BlockingClient::new("http://127.0.0.1:1").unwrap();
    let error = blocking.with_bound_header("X-Api-Version", "nul\0").unwrap_err();
    assert!(matches!(error, bound_headers::Error::RequestConstruction(_)), "{error:?}");
}
"##;
