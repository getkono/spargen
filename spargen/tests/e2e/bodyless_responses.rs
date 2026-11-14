//! Responses that carry no body by RFC 9110 (#1341): every response to a `HEAD` request, and a
//! `204`, `205` or `304` one, is read as bodyless even where the document declares `content` for
//! it, so a call answered with the empty body the server sends succeeds instead of failing to
//! decode one. Compiled and linted with its `testing` module, and driven over the typed
//! `MockBackend`.

use super::*;

/// A `HEAD` that reuses its `GET` response's JSON body and documents a header, a listing whose
/// `204` and `304` declare content, and a `PUT` whose `205` does.
const BODYLESS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Bodyless, version: 1.0.0 }
paths:
  /pets/{id}:
    parameters:
      - { name: id, in: path, required: true, schema: { type: string } }
    get:
      operationId: getPet
      responses:
        "200": { $ref: "#/components/responses/Pet" }
    head:
      operationId: probePet
      responses:
        "200": { $ref: "#/components/responses/Pet" }
  /items:
    get:
      operationId: listItems
      responses:
        "200":
          description: the items
          content:
            application/json: { schema: { type: array, items: { type: string } } }
        "204":
          description: no items
          content:
            application/json: { schema: { type: array, items: { type: string } } }
        "304":
          description: unchanged
          content:
            application/json: { schema: { type: array, items: { type: string } } }
    put:
      operationId: resetItems
      responses:
        "205":
          description: reset
          content:
            text/plain: { schema: { type: string } }
components:
  responses:
    Pet:
      description: the pet
      headers:
        X-Total: { required: true, schema: { type: integer } }
      content:
        application/json:
          schema:
            type: object
            required: [name]
            properties: { name: { type: string } }
"##;

#[test]
fn a_bodyless_response_with_declared_content_decodes_no_body() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "bodyless.yaml", BODYLESS_SPEC);
    let out = temp.path().join("client");
    let report =
        generate_configured_fixture_crate(&spec, &out, "bodyless_client", "2021", |spec| {
            spec.with_testing(spargen::TestingModule::Emitted)
        });
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .count(),
        4,
        "{report:#?}"
    );

    write_fixture_test(&out, "bodyless", BODYLESS_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "bodyless responses must compile and pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "bodyless"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a bodyless response must succeed on the empty body the server sends"
    );
}

const BODYLESS_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use bodyless_client::testing;

/// Drive a call to completion: the mock answers from memory, so nothing it awaits pends.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn client(mock: &testing::MockBackend) -> bodyless_client::Client {
    bodyless_client::Client::with_backend(Arc::new(mock.clone()), "http://mock").unwrap()
}

#[test]
fn a_head_response_is_the_unit_value_with_its_headers_typed() {
    let mock = testing::MockBackend::new().on_probe_pet(|_| {
        testing::probe_pet::Reply::from_reply(testing::Reply::new(200).header("x-total", "3"))
    });
    let response = block_on(client(&mock).probe_pet("rex")).unwrap();
    let headers = bodyless_client::ProbePetStatus200Headers::from_response(&response).unwrap();
    assert_eq!(headers.x_total, 3);
    let () = response.into_inner();
}

#[test]
fn the_get_beside_it_still_decodes_the_shared_body() {
    let mock = testing::MockBackend::new().on_get_pet(|_| {
        testing::get_pet::Reply::from_reply(
            testing::Reply::new(200)
                .header("x-total", "3")
                .header("content-type", "application/json")
                .body(r#"{"name":"rex"}"#),
        )
    });
    let pet = block_on(client(&mock).get_pet("rex")).unwrap().into_inner();
    assert_eq!(pet.name, "rex");
}

#[test]
fn a_204_with_declared_content_is_a_unit_variant() {
    let mock = testing::MockBackend::new()
        .on_list_items(|_| testing::list_items::Reply::from_reply(testing::Reply::new(204)));
    let listed = block_on(client(&mock).list_items()).unwrap().into_inner();
    assert!(matches!(listed, bodyless_client::ListItemsResponse::Status204));
}

#[test]
fn a_205_with_declared_content_is_the_unit_value() {
    let mock = testing::MockBackend::new()
        .on_reset_items(|_| testing::reset_items::Reply::from_reply(testing::Reply::new(205)));
    let () = block_on(client(&mock).reset_items()).unwrap().into_inner();
}

#[test]
fn a_304_with_declared_content_is_an_error_read_without_a_body() {
    let mock = testing::MockBackend::new()
        .on_list_items(|_| testing::list_items::Reply::from_reply(testing::Reply::new(304)));
    let error = block_on(client(&mock).list_items()).unwrap_err();
    assert_eq!(error.status().map(|status| status.as_u16()), Some(304));
}
"##;
