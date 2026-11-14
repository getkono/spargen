//! The opt-in `testing` module (#1268): a client generated with `Spec::testing`, compiled and
//! linted, driven over its typed `MockBackend` — routed by operation, answered with typed replies
//! the client decodes, recorded, and failing loudly on an unrouted request — and absent without
//! the switch.

use super::*;

/// One operation per reply shape: an exact JSON success beside a JSON `404` (`getPet`), a bodyless
/// `204` beside a JSON `4XX` range and a `text/plain` `default` (`deletePet`), a JSON request body
/// (`createPet`), raw octets (`getPhoto`), and a `text/plain` success (`getGreeting`).
const MOCK_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
components:
  schemas:
    Pet:
      type: object
      required: [id, name]
      properties:
        id: { type: integer, format: int64 }
        name: { type: string }
    Problem:
      type: object
      required: [code]
      properties:
        code: { type: string }
paths:
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Pet" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
  /pets/{petId}:
    parameters:
      - { name: petId, in: path, required: true, schema: { type: integer, format: int64 } }
    get:
      operationId: getPet
      responses:
        "200":
          description: found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
        "404":
          description: missing
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Problem" }
    delete:
      operationId: deletePet
      responses:
        "204": { description: deleted }
        "4XX":
          description: refused
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Problem" }
        default:
          description: failed
          content:
            text/plain:
              schema: { type: string }
  /pets/{petId}/photo:
    parameters:
      - { name: petId, in: path, required: true, schema: { type: integer, format: int64 } }
    get:
      operationId: getPhoto
      responses:
        "200":
          description: the photo
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
  /greeting:
    get:
      operationId: getGreeting
      responses:
        "200":
          description: a greeting
          content:
            text/plain:
              schema: { type: string }
"##;

#[test]
fn the_testing_module_routes_records_and_answers_typed_replies() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "pets.yaml", MOCK_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "mock_client", "2021", |spec| {
        spec.with_testing(spargen::TestingModule::Emitted)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "mock", MOCK_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the testing module must pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "mock"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the MockBackend must route, record and answer each operation"
    );
}

/// `BASIC_SPEC` emits every response codec, stream framing and status selector, so its `testing`
/// module, under every feature, is held to the strict consumer.
#[test]
fn the_basic_spec_testing_module_compiles_and_lints_clean() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "basic.yaml", BASIC_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "basic_testing", "2021", |spec| {
        spec.with_testing(spargen::TestingModule::Emitted)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub mod testing"),
        "the module is emitted"
    );
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
        "the BASIC_SPEC testing module must pass clippy -D warnings"
    );
}

/// One operation named for each crate the `testing` module spells a path through, each with a JSON,
/// raw-octet and text reply, so a `pub mod {method}` there shadows the crate unless the module
/// writes its paths from the crate root.
const SHADOWING_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Shadows, version: 1.0.0 }
components:
  schemas:
    Item:
      type: object
      properties:
        name: { type: string }
paths:
  /http:
    get:
      operationId: http
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
  /bytes:
    get:
      operationId: bytes
      responses:
        "200":
          description: ok
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
  /std:
    get:
      operationId: std
      responses:
        "200":
          description: ok
          content:
            text/plain:
              schema: { type: string }
  /serde:
    get:
      operationId: serde
      responses:
        "200": { description: ok }
  /serde-json:
    get:
      operationId: serde_json
      responses:
        "200": { description: ok }
  /http-body:
    get:
      operationId: http_body
      responses:
        "200": { description: ok }
  /testing:
    get:
      operationId: testing
      responses:
        "200": { description: ok }
  /names:
    get:
      operationId: getNames
      responses:
        "200":
          description: a top-level XML sequence
          content:
            application/xml:
              schema: { type: array, items: { type: string } }
  /items:
    get:
      operationId: getItems
      responses:
        "200":
          description: a top-level XML sequence of objects
          content:
            application/xml:
              schema: { type: array, items: { $ref: "#/components/schemas/Item" } }
  /claimed:
    get:
      operationId: getClaimed
      responses:
        "200":
          description: exact
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
        "2XX":
          description: range
          content:
            text/plain:
              schema: { type: string }
        "404": { description: exact without a body }
        "4XX": { description: range without a body }
        default:
          description: fallback
          content:
            text/plain:
              schema: { type: string }
"##;

/// An operation whose method is `http`, `bytes`, `std`, `serde`, `serde_json`, `http_body` or
/// `testing` gets a `testing::{method}` module of that name, and the `testing` module still compiles
/// and lints clean; a top-level XML sequence reply reads back through the client, and a range or
/// `default` constructor refuses a status a more precise entry documents.
#[test]
fn a_reply_module_named_for_a_crate_does_not_shadow_it() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "shadows.yaml", SHADOWING_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "shadow_client", "2021", |spec| {
        spec.with_testing(spargen::TestingModule::Emitted)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    for module in [
        "http",
        "bytes",
        "std",
        "serde",
        "serde_json",
        "http_body",
        "testing",
    ] {
        assert!(
            generated.contains(&format!("pub mod {module} {{")),
            "the `{module}` reply module is emitted"
        );
    }
    write_fixture_test(&out, "shadow", SHADOW_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a reply module named for a crate or for `testing` must not shadow it or trip a lint"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "shadow"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "XML sequence replies must decode, and claimed statuses must be refused"
    );
}

const SHADOW_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use shadow_client::testing;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn client(mock: &testing::MockBackend) -> shadow_client::Client {
    shadow_client::Client::with_backend(Arc::new(mock.clone()), "http://mock").unwrap()
}

#[test]
fn a_top_level_xml_sequence_of_strings_reads_back() {
    let names = vec!["a".to_owned(), "b".to_owned()];
    let mock = testing::MockBackend::new().on_get_names({
        let names = names.clone();
        move |_| testing::get_names::Reply::status_200(&names)
    });
    assert_eq!(block_on(client(&mock).get_names()).unwrap().into_inner(), names);
}

#[test]
fn a_top_level_xml_sequence_of_objects_reads_back() {
    let items: Vec<shadow_client::types::Item> =
        serde_json::from_value(serde_json::json!([{ "name": "a" }, { "name": "b" }])).unwrap();
    let mock = testing::MockBackend::new().on_get_items({
        let items = items.clone();
        move |_| testing::get_items::Reply::status_200(&items)
    });
    let read = block_on(client(&mock).get_items()).unwrap().into_inner();
    assert_eq!(serde_json::to_value(&read).unwrap(), serde_json::to_value(&items).unwrap());
}

#[test]
fn an_empty_top_level_xml_sequence_is_still_encoded() {
    let _ = testing::get_names::Reply::status_200(&Vec::new());
}

#[test]
fn a_range_constructor_answers_a_status_no_exact_entry_documents() {
    let mock = testing::MockBackend::new()
        .on_get_claimed(|_| testing::get_claimed::Reply::status_2xx(201, &"made".to_owned()));
    let reply = block_on(client(&mock).get_claimed()).unwrap();
    assert_eq!(reply.status(), 201);
}

#[test]
#[should_panic(expected = "use `status_200`")]
fn a_range_constructor_refuses_a_status_an_exact_entry_documents() {
    let _ = testing::get_claimed::Reply::status_2xx(200, &"x".to_owned());
}

#[test]
#[should_panic(expected = "use `status_404`")]
fn a_bodyless_range_constructor_refuses_a_status_an_exact_entry_documents() {
    let _ = testing::get_claimed::Reply::status_4xx(404);
}

#[test]
#[should_panic(expected = "use `status_200`")]
fn the_default_constructor_refuses_a_status_an_exact_entry_documents() {
    let _ = testing::get_claimed::Reply::default(200, &"x".to_owned());
}

#[test]
#[should_panic(expected = "use `status_2xx`")]
fn the_default_constructor_refuses_a_status_a_range_entry_documents() {
    let _ = testing::get_claimed::Reply::default(250, &"x".to_owned());
}

#[test]
fn the_default_constructor_answers_a_status_nothing_else_documents() {
    let mock = testing::MockBackend::new()
        .on_get_claimed(|_| testing::get_claimed::Reply::default(503, &"down".to_owned()));
    let error = block_on(client(&mock).get_claimed()).unwrap_err();
    assert_eq!(error.status().map(|status| status.as_u16()), Some(503));
}
"##;

/// The switch is off by default, and nothing of the module is emitted then.
#[test]
fn no_testing_module_is_emitted_by_default() {
    let off = generate_text(MOCK_SPEC, |spec| spec);
    assert!(!off.contains("pub mod testing"), "{off}");
    assert!(!off.contains("MockBackend"), "{off}");
    let on = generate_text(MOCK_SPEC, |spec| {
        spec.with_testing(spargen::TestingModule::Emitted)
    });
    assert!(on.contains("pub mod testing"), "{on}");
}

/// `readOnly` and `writeOnly` members at the top level, in a nested model, in an array of models,
/// nullable and not, over JSON and XML, beside a request body carrying the same model (#1416).
const DIRECTION_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
components:
  schemas:
    Pet:
      type: object
      required: [id, name]
      properties:
        id: { type: integer, format: int64, readOnly: true }
        name: { type: string }
        password: { type: string, writeOnly: true }
        owner: { $ref: "#/components/schemas/Owner" }
        checked: { type: [string, "null"], readOnly: true }
        hint: { type: [string, "null"], writeOnly: true }
    Owner:
      type: object
      required: [id]
      properties:
        id: { type: integer, format: int64, readOnly: true }
        secret: { type: string, writeOnly: true }
    Tag:
      type: object
      required: [id, label]
      properties:
        id: { type: integer, format: int64, readOnly: true }
        label: { type: string }
        secret: { type: string, writeOnly: true }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: every pet
          content:
            application/json:
              schema: { type: array, items: { $ref: "#/components/schemas/Pet" } }
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Pet" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
  /pets/{petId}:
    parameters:
      - { name: petId, in: path, required: true, schema: { type: integer, format: int64 } }
    get:
      operationId: getPet
      responses:
        "200":
          description: found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
  /tag:
    get:
      operationId: getTag
      responses:
        "200":
          description: a tag
          content:
            application/xml:
              schema: { $ref: "#/components/schemas/Tag" }
"##;

/// A typed reply encodes its body in the response direction, as a server writes it: every
/// `readOnly` member written and every `writeOnly` one left out, at any depth, over JSON and XML.
/// A model serialized outside a reply, a request body included, keeps the request direction.
#[test]
fn a_typed_reply_writes_read_only_members_and_leaves_write_only_ones_out() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "pets.yaml", DIRECTION_SPEC);
    let out = temp.path().join("client");
    let report =
        generate_configured_fixture_crate(&spec, &out, "direction_client", "2021", |spec| {
            spec.with_testing(spargen::TestingModule::Emitted)
        });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "direction", DIRECTION_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the response-direction encoding must pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "direction"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a typed reply must carry readOnly members and leave writeOnly ones out"
    );
}

const DIRECTION_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use direction_client::Nullable;
use direction_client::{ops, testing, types};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn client(mock: &testing::MockBackend) -> direction_client::Client {
    direction_client::Client::with_backend(Arc::new(mock.clone()), "http://mock").unwrap()
}

/// A pet with every member set, `readOnly` and `writeOnly` ones included.
fn pet(id: i64) -> types::Pet {
    types::Pet::new("rex".to_owned())
        .with_id(id)
        .with_password("hunter2".to_owned())
        .with_owner(types::Owner::new().with_id(3).with_secret("s3cret".to_owned()))
        .with_checked("yes".to_owned())
        .with_hint("h".to_owned())
}

/// What a client decodes from `pet(id)` written as a server writes it.
fn assert_read_back(read: &types::Pet, id: i64) {
    assert_eq!(read.id, Some(id), "{read:?}");
    assert_eq!(read.name, "rex", "{read:?}");
    assert_eq!(read.checked, Nullable::Value("yes".to_owned()), "{read:?}");
    assert_eq!(read.password, None, "{read:?}");
    assert_eq!(read.hint, Nullable::Absent, "{read:?}");
    let owner = read.owner.as_ref().expect("the nested owner");
    assert_eq!(owner.id, Some(3), "{read:?}");
    assert_eq!(owner.secret, None, "{read:?}");
}

#[test]
fn a_json_reply_carries_read_only_members_and_drops_write_only_ones() {
    let mock = testing::MockBackend::new().on_get_pet(|_| testing::get_pet::Reply::status_200(&pet(7)));
    let read = block_on(client(&mock).get_pet(7)).unwrap().into_inner();
    assert_read_back(&read, 7);
}

#[test]
fn an_array_reply_keeps_the_rule_for_every_member() {
    let mock = testing::MockBackend::new()
        .on_list_pets(|_| testing::list_pets::Reply::status_200(&vec![pet(1), pet(2)]));
    let read = block_on(client(&mock).list_pets()).unwrap().into_inner();
    assert_eq!(read.len(), 2);
    assert_read_back(&read[0], 1);
    assert_read_back(&read[1], 2);
}

#[test]
fn an_xml_reply_carries_read_only_members_and_drops_write_only_ones() {
    let tag = types::Tag::new("blue".to_owned()).with_id(5).with_secret("s".to_owned());
    let mock = testing::MockBackend::new().on_get_tag(move |_| testing::get_tag::Reply::status_200(&tag));
    let read = block_on(client(&mock).get_tag()).unwrap().into_inner();
    assert_eq!(read.id, Some(5), "{read:?}");
    assert_eq!(read.label, "blue", "{read:?}");
    assert_eq!(read.secret, None, "{read:?}");
}

#[test]
fn a_request_body_keeps_the_request_direction() {
    let mock = testing::MockBackend::new().on_create_pet(|call| {
        let sent: types::Pet = call.json().unwrap();
        testing::create_pet::Reply::status_201(&sent.with_id(11).with_checked("yes".to_owned()))
    });
    let created = block_on(client(&mock).create_pet(&pet(99))).unwrap().into_inner();
    assert_eq!(created.id, Some(11), "{created:?}");
    assert_eq!(created.password, None, "{created:?}");

    let calls = mock.calls(ops::CREATE_PET);
    let sent: serde_json::Value = serde_json::from_slice(calls[0].body()).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({
            "name": "rex",
            "password": "hunter2",
            "owner": { "secret": "s3cret" },
            "hint": "h",
        })
    );
}

#[test]
fn a_model_serialized_outside_a_reply_keeps_the_request_direction() {
    let _ = testing::get_pet::Reply::status_200(&pet(7));
    assert_eq!(
        serde_json::to_value(pet(7)).unwrap(),
        serde_json::json!({
            "name": "rex",
            "password": "hunter2",
            "owner": { "secret": "s3cret" },
            "hint": "h",
        })
    );
}
"##;

const MOCK_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use mock_client::{ops, testing, types};

/// Drive a call to completion: the mock answers from memory, so nothing it awaits pends.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

fn pet(id: i64, name: &str) -> types::Pet {
    serde_json::from_value(serde_json::json!({ "id": id, "name": name })).unwrap()
}

fn problem(code: &str) -> types::Problem {
    serde_json::from_value(serde_json::json!({ "code": code })).unwrap()
}

fn client(mock: &testing::MockBackend) -> mock_client::Client {
    mock_client::Client::with_backend(Arc::new(mock.clone()), "http://mock/v1").unwrap()
}

#[test]
fn a_typed_success_reply_is_decoded_and_the_call_recorded() {
    let mock = testing::MockBackend::new()
        .on_get_pet(|call| testing::get_pet::Reply::status_200(&pet(7, call.path())));
    let client = client(&mock);

    let found = block_on(client.get_pet(7)).unwrap();
    assert_eq!(found.status(), 200);
    assert_eq!(found.into_inner(), pet(7, "/v1/pets/7"));

    let calls = mock.calls(ops::GET_PET);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].operation(), Some(ops::GET_PET));
    assert_eq!(calls[0].method(), "GET");
    assert_eq!(calls[0].path(), "/v1/pets/7");
    assert!(mock.calls(ops::DELETE_PET).is_empty());
}

#[test]
fn a_typed_error_reply_is_the_typed_error() {
    let mock = testing::MockBackend::new()
        .on_get_pet(|_| testing::get_pet::Reply::status_404(&problem("pet.missing")));
    let error = block_on(client(&mock).get_pet(1)).unwrap_err();
    assert_eq!(error.status().map(|status| status.as_u16()), Some(404));
    let rendered = format!("{error:?}");
    assert!(rendered.contains("pet.missing"), "{rendered}");
    assert!(matches!(error, mock_client::Error::Api(_)), "{rendered}");
}

#[test]
fn range_default_and_bodyless_replies_carry_the_status_given() {
    let mock = testing::MockBackend::new().on_delete_pet(|call| match call.path() {
        "/v1/pets/1" => testing::delete_pet::Reply::status_204(),
        "/v1/pets/2" => testing::delete_pet::Reply::status_4xx(409, &problem("pet.busy")),
        _ => testing::delete_pet::Reply::default(503, &"down for maintenance".to_owned()),
    });
    let client = client(&mock);

    assert_eq!(block_on(client.delete_pet(1)).unwrap().status(), 204);
    let busy = block_on(client.delete_pet(2)).unwrap_err();
    assert_eq!(busy.status().map(|status| status.as_u16()), Some(409));
    assert!(format!("{busy:?}").contains("pet.busy"), "{busy:?}");
    assert!(matches!(busy, mock_client::Error::Api(_)), "{busy:?}");
    let down = block_on(client.delete_pet(3)).unwrap_err();
    assert_eq!(down.status().map(|status| status.as_u16()), Some(503));
    assert!(format!("{down:?}").contains("down for maintenance"), "{down:?}");
    assert!(matches!(down, mock_client::Error::Api(_)), "{down:?}");
    assert_eq!(mock.calls(ops::DELETE_PET).len(), 3);
}

#[test]
#[should_panic(expected = "is not a 4XX status")]
fn a_range_reply_refuses_a_status_outside_its_range() {
    let _ = testing::delete_pet::Reply::status_4xx(500, &problem("never"));
}

#[test]
fn the_request_body_is_recorded_and_decodes_as_json() {
    let mock = testing::MockBackend::new().on_create_pet(|call| {
        let sent: types::Pet = call.json().unwrap();
        testing::create_pet::Reply::status_201(&sent)
    });
    let created = block_on(client(&mock).create_pet(&pet(3, "Tom"))).unwrap();
    assert_eq!(created.into_inner(), pet(3, "Tom"));
    let calls = mock.calls(ops::CREATE_PET);
    assert_eq!(calls[0].json::<types::Pet>().unwrap(), pet(3, "Tom"));
    assert_eq!(calls[0].header("content-type"), Some("application/json"));
}

#[test]
fn octet_and_text_replies_are_decoded_by_their_own_codecs() {
    let mock = testing::MockBackend::new()
        .on_get_photo(|_| testing::get_photo::Reply::status_200(&b"\x89PNG"[..]))
        .on_get_greeting(|_| testing::get_greeting::Reply::status_200(&"hello".to_owned()));
    let client = client(&mock);
    assert_eq!(&block_on(client.get_photo(1)).unwrap().into_inner()[..], b"\x89PNG");
    assert_eq!(block_on(client.get_greeting()).unwrap().into_inner(), "hello");
}

#[test]
fn an_undocumented_reply_is_built_untyped() {
    let mock = testing::MockBackend::new().on_get_greeting(|_| {
        testing::get_greeting::Reply::from_reply(testing::Reply::new(418).body("teapot"))
    });
    let error = block_on(client(&mock).get_greeting()).unwrap_err();
    assert_eq!(error.status().map(|status| status.as_u16()), Some(418));
}

#[test]
fn an_unrouted_request_fails_loudly_naming_its_operation() {
    let mock = testing::MockBackend::new()
        .on_get_pet(|_| testing::get_pet::Reply::status_404(&problem("unused")));
    let error = block_on(client(&mock).delete_pet(9)).unwrap_err();
    // `Error::RequestConstruction`, whose source chain reaches the `Unrouted` the mock failed with.
    let mut source = std::error::Error::source(&error);
    let unrouted = loop {
        match source {
            Some(cause) => match cause.downcast_ref::<testing::Unrouted>() {
                Some(unrouted) => break unrouted,
                None => source = cause.source(),
            },
            None => panic!("expected an Unrouted source, got {error:?}"),
        }
    };
    let message = unrouted.to_string();
    assert!(message.contains("`deletePet`"), "{message}");
    assert!(message.contains("DELETE /pets/{petId}"), "{message}");
    assert!(message.contains("http://mock/v1/pets/9"), "{message}");
    assert_eq!(unrouted.call().operation(), Some(ops::DELETE_PET));
    // Recorded like every other request.
    assert_eq!(mock.calls(ops::DELETE_PET).len(), 1);
    assert_eq!(mock.all_calls().len(), 1);
}

#[test]
fn the_latest_registration_wins() {
    let mock = testing::MockBackend::new()
        .on_get_greeting(|_| testing::get_greeting::Reply::status_200(&"first".to_owned()))
        .on_get_greeting(|_| testing::get_greeting::Reply::status_200(&"second".to_owned()));
    assert_eq!(block_on(client(&mock).get_greeting()).unwrap().into_inner(), "second");
}
"##;
