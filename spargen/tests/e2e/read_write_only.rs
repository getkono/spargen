//! `readOnly` and `writeOnly` members on the wire: a `required` one is required in its own
//! direction only, and a relaxed one decodes as absent in unions, defaults and `allOf` merges.

use super::*;

/// A `required` property marked `readOnly` or `writeOnly` is required in one direction only (#1326):
/// a request built with `new` leaves the `readOnly` `id` off, JSON and multipart alike, even when
/// it is set, and a response that leaves the `writeOnly` `password` off decodes.
#[test]
fn read_and_write_only_members_round_trip_in_their_own_direction() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Users, version: 1.0.0 }
paths:
  /users:
    post:
      operationId: createUser
      requestBody:
        required: true
        content:
          application/json: { schema: { $ref: '#/components/schemas/User' } }
      responses:
        "201":
          description: Created
          content:
            application/json: { schema: { $ref: '#/components/schemas/User' } }
  /uploads:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data: { schema: { $ref: '#/components/schemas/User' } }
      responses:
        "204": { description: No Content }
components:
  schemas:
    User:
      type: object
      required: [id, name, password]
      properties:
        id: { type: integer, readOnly: true }
        name: { type: string }
        password: { type: string, writeOnly: true }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "users_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let codes: Vec<_> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        [
            Code::ReadWriteOnlyRequirednessRelaxed,
            Code::ReadWriteOnlyRequirednessRelaxed
        ],
        "{report:#?}"
    );

    write_fixture_test(
        &out,
        "users",
        r##"#![cfg(feature = "blocking")]

mod common;

use users_client::types::User;

#[test]
fn a_request_omits_the_read_only_member_and_a_response_omits_the_write_only_one() {
    let (base, server) = common::serve(vec![
        common::response(
            "201 Created",
            &[("Content-Type", "application/json")],
            br#"{"id":7,"name":"ada"}"#,
        ),
        common::response("204 No Content", &[], b""),
    ]);
    let client = users_client::BlockingClient::new(&base).unwrap();

    // `new` takes the `writeOnly` member and not the `readOnly` one; setting that one anyway
    // sends nothing.
    let user = User::new("ada".to_owned(), "secret".to_owned()).with_id(99);
    assert_eq!(user.password.as_deref(), Some("secret"));
    let created = client.create_user(&user).expect("the create round-trips").into_inner();
    assert_eq!(created.id, Some(7));
    assert_eq!(created.name, "ada");
    assert_eq!(created.password, None);
    client.upload(&user).expect("the upload round-trips");

    let requests = server.join().unwrap();
    let json = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert_eq!(json, r#"{"name":"ada","password":"secret"}"#);
    let multipart = String::from_utf8_lossy(&requests[1].body).into_owned();
    assert!(multipart.contains("name=\"name\""), "{multipart}");
    assert!(multipart.contains("name=\"password\""), "{multipart}");
    assert!(!multipart.contains("name=\"id\""), "{multipart}");
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "users"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the read-only and write-only members must round-trip in their own direction"
    );
}

/// A relaxed `required` member may be absent from a decoded payload everywhere decoding reads it:
/// a union does not route by its key, an absent one takes its schema `default` as every other
/// optional member does, and an `allOf` merge relaxes it whichever member says `readOnly`.
#[test]
fn a_relaxed_member_decodes_as_absent_in_unions_defaults_and_all_of_merges() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Relaxed, version: 1.0.0 }
paths:
  /read-only-key:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json: { schema: { $ref: '#/components/schemas/ReadOnlyKey' } }
  /write-only-key:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json: { schema: { $ref: '#/components/schemas/WriteOnlyKey' } }
  /defaulted:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json: { schema: { $ref: '#/components/schemas/Defaulted' } }
  /merged:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json: { schema: { $ref: '#/components/schemas/Merged' } }
components:
  schemas:
    ReadOnlyKey:
      oneOf:
        - $ref: '#/components/schemas/Stored'
        - $ref: '#/components/schemas/Named'
    WriteOnlyKey:
      oneOf:
        - $ref: '#/components/schemas/Login'
        - $ref: '#/components/schemas/Named'
    Stored:
      type: object
      additionalProperties: false
      required: [id]
      properties:
        id: { type: integer, readOnly: true }
    Login:
      type: object
      additionalProperties: false
      required: [secret]
      properties:
        secret: { type: string, writeOnly: true }
    Named:
      type: object
      additionalProperties: false
      required: [name]
      properties:
        name: { type: string }
    Defaulted:
      type: object
      required: [state, token]
      properties:
        state: { type: string, readOnly: true, default: active }
        token: { type: string, writeOnly: true, default: none }
    Merged:
      allOf:
        - type: object
          required: [id]
          properties:
            id: { type: integer }
        - type: object
          properties:
            id: { type: integer, readOnly: true }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "relaxed_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "relaxed",
        r##"use relaxed_client::types::{Defaulted, Merged, ReadOnlyKey, WriteOnlyKey};

#[test]
fn a_union_routes_a_payload_without_a_relaxed_key() {
    let named: ReadOnlyKey = serde_json::from_str(r#"{"name":"x"}"#).expect("a `Named`");
    assert!(matches!(named, ReadOnlyKey::Named(_)), "{named:?}");
    let stored: ReadOnlyKey = serde_json::from_str(r#"{"id":7}"#).expect("a `Stored`");
    assert!(matches!(stored, ReadOnlyKey::Stored(_)), "{stored:?}");
    // A response leaves the `writeOnly` key out, so it selects no variant by its presence.
    let login: WriteOnlyKey = serde_json::from_str("{}").expect("a `Login` without its secret");
    assert!(matches!(login, WriteOnlyKey::Login(_)), "{login:?}");
    let named: WriteOnlyKey = serde_json::from_str(r#"{"name":"x"}"#).expect("a `Named`");
    assert!(matches!(named, WriteOnlyKey::Named(_)), "{named:?}");
}

#[test]
fn an_absent_relaxed_member_takes_its_schema_default() {
    let defaulted: Defaulted = serde_json::from_str("{}").expect("both members may be absent");
    assert_eq!(defaulted.state.as_deref(), Some("active"));
    assert_eq!(defaulted.token.as_deref(), Some("none"));
}

#[test]
fn an_all_of_member_marking_the_property_read_only_relaxes_it() {
    let merged: Merged = serde_json::from_str("{}").expect("the read-only `id` may be absent");
    assert_eq!(merged.id, None);
    assert_eq!(serde_json::to_string(&merged.with_id(7)).unwrap(), "{}");
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--test", "relaxed"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a relaxed member must decode as absent in unions, defaults and allOf merges"
    );
}
