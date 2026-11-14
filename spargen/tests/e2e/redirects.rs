//! Redirects on the default backend (#1397): a client built with `Client::new` follows a `3xx` only
//! within the origin of the request it sent, and refuses one to another scheme, host or port as
//! `Error::Redirect` before anything reaches that origin, so an `apiKey` header and a bound header
//! never leave for it. Driven over two real listeners, since only a socket shows what was sent.

use super::*;

/// `getFile` sends an `apiKey` header credential and a bound `X-Tenant` header.
const REDIRECTS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Redirected, version: 1.0.0 }
components:
  securitySchemes:
    key: { type: apiKey, in: header, name: X-API-Key }
security: [{ key: [] }]
paths:
  /files/{id}:
    get:
      operationId: getFile
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
        - { name: X-Tenant, in: header, schema: { type: string } }
      responses:
        "204": { description: found }
"##;

#[test]
fn the_default_backend_refuses_a_redirect_to_another_origin() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "redirected.yaml", REDIRECTS_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "redirects", "2021", |spec| {
        spec.add_bound_header("X-Tenant")
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "redirects", REDIRECTS_TEST);
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
        "the redirected client must pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "redirects"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a cross-origin redirect must be refused unsent, and a same-origin one followed"
    );
}

const REDIRECTS_TEST: &str = r##"#![cfg(feature = "blocking")]

mod common;

use std::net::TcpListener;

fn client(base: &str) -> redirects::BlockingClient {
    redirects::BlockingClient::new(base)
        .unwrap()
        .with_key(redirects::SecretString::from("s3cret"))
        .with_bound_header("X-Tenant", "TENANTSECRET")
        .unwrap()
}

/// A listener on another port of the same host, which is another origin; it must accept nothing.
fn other_origin() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, format!("127.0.0.1:{port}"))
}

#[test]
fn a_cross_origin_redirect_is_refused_unsent() {
    let (b, b_authority) = other_origin();
    let location = format!("http://user:PASSWORD@{b_authority}/blob/42?sig=QUERYSECRET");
    let (base, a) = common::serve_one(common::response(
        "302 Found",
        &[("Location", &location)],
        b"",
    ));

    let error = client(&base).get_file("42").unwrap_err();
    let redirects::Error::Redirect(redirect) = &error else {
        panic!("expected Error::Redirect, got {error:?}");
    };
    let target = redirect
        .cross_origin_target()
        .expect("a refused cross-origin hop names its target");
    assert_eq!(target.as_str(), format!("http://{b_authority}/blob/42"));

    // A sent the credential and the bound header, as asked; nothing reached B.
    let request = a.join().unwrap();
    assert_eq!(request.header("x-api-key"), ["s3cret"], "{}", request.text());
    assert_eq!(request.header("x-tenant"), ["TENANTSECRET"], "{}", request.text());
    match b.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the other origin must receive no connection: {other:?}"),
    }

    // Neither rendering carries the target's userinfo or query, or a credential.
    for rendered in [format!("{error}"), format!("{error:?}"), chain(&error)] {
        for secret in ["PASSWORD", "QUERYSECRET", "s3cret", "TENANTSECRET"] {
            assert!(!rendered.contains(secret), "{secret} in {rendered}");
        }
    }
    assert!(chain(&error).contains(&format!("http://{b_authority}/blob/42")), "{}", chain(&error));
}

/// The error and every source under it, as a full-chain reporter prints them.
fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        text.push_str(": ");
        text.push_str(&current.to_string());
        cause = current.source();
    }
    text
}

#[test]
fn a_same_origin_redirect_is_followed_with_its_headers() {
    let (base, a) = common::serve(vec![
        common::response("302 Found", &[("Location", "/files/43")], b""),
        common::no_content(),
    ]);

    assert_eq!(client(&base).get_file("42").unwrap().status(), 204);
    let requests = a.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].line(), "GET /files/43 HTTP/1.1");
    assert_eq!(requests[1].header("x-api-key"), ["s3cret"], "{}", requests[1].text());
    assert_eq!(requests[1].header("x-tenant"), ["TENANTSECRET"], "{}", requests[1].text());
}

#[test]
fn an_eleventh_same_origin_redirect_exhausts_the_policy() {
    let hop = common::response("302 Found", &[("Location", "/files/again")], b"");
    let (base, a) = common::serve(vec![hop; 11]);

    let error = client(&base).get_file("42").unwrap_err();
    let redirects::Error::Redirect(redirect) = &error else {
        panic!("expected Error::Redirect, got {error:?}");
    };
    assert_eq!(redirect.cross_origin_target(), None);
    assert_eq!(a.join().unwrap().len(), 11);
}
"##;
