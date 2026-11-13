//! Integer and boolean `enum`/`const` sets as checked newtypes (#1327), compiled under clippy and
//! driven over the wire: a listed value round-trips, an unlisted one fails to decode and to
//! convert, and an open set (`open_narrowing`, a response body's own schema) decodes it.

use super::*;

const CHECKED_SETS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Levels, version: 1.0.0 }
paths:
  /levels:
    post:
      operationId: putLevel
      parameters:
        - name: min
          in: query
          required: true
          schema: { $ref: '#/components/schemas/Level' }
      requestBody:
        required: true
        content:
          application/json: { schema: { $ref: '#/components/schemas/Setting' } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [code, setting]
                properties:
                  code:
                    allOf: [{ type: integer, format: int32 }, { enum: [1, 2] }]
                  setting: { $ref: '#/components/schemas/Setting' }
components:
  schemas:
    Level:
      type: integer
      format: int32
      enum: [-1, 1, 3]
    Setting:
      type: object
      required: [level, on]
      properties:
        level: { $ref: '#/components/schemas/Level' }
        on: { type: boolean, const: true }
        tier: { type: integer, format: uint8, enum: [1, 2], default: 2 }
        repeated: { type: integer, enum: [1, 1.0, 2] }
        twice: { type: boolean, enum: [true, true] }
"##;

#[test]
fn checked_sets_round_trip_and_refuse_an_unlisted_value() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", CHECKED_SETS_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "levels_client", "2021", |spec| {
        spec.with_open_narrowing(spargen::Narrowing::Open)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the checked newtypes must pass clippy -D warnings"
    );

    write_fixture_test(
        &out,
        "checked",
        r##"#![cfg(feature = "blocking")]

mod common;

use levels_client::types::{Level, Setting, SettingOn, SettingTier};

#[test]
fn a_listed_value_converts_and_an_unlisted_one_does_not() {
    assert_eq!(Level::try_from(3), Ok(Level::V3));
    assert_eq!(Level::V_MINUS_1.get(), -1);
    assert_eq!(i32::from(Level::V1), 1);
    assert_eq!(Level::ALL, &[Level::V_MINUS_1, Level::V1, Level::V3]);
    assert_eq!(Level::V3.to_string(), "3");
    let refused = Level::try_from(2).unwrap_err();
    assert_eq!(refused.type_name(), "Level");
    assert_eq!(refused.value(), "2");
    assert_eq!(SettingOn::TRUE.get(), true);
    assert!(SettingOn::try_from(false).is_err());
}

#[test]
fn decoding_refuses_an_unlisted_value_and_applies_a_listed_default() {
    let setting: Setting = serde_json::from_str(r#"{"level":-1,"on":true}"#).unwrap();
    assert_eq!(setting.level, Level::V_MINUS_1);
    assert_eq!(setting.tier, Some(SettingTier::V2));
    assert_eq!(
        serde_json::to_string(&setting).unwrap(),
        r#"{"level":-1,"on":true,"tier":2}"#
    );
    for unlisted in [
        r#"{"level":2,"on":true}"#,
        r#"{"level":1,"on":false}"#,
        r#"{"level":1,"on":true,"tier":3}"#,
        r#"{"level":3000000000,"on":true}"#,
    ] {
        assert!(
            serde_json::from_str::<Setting>(unlisted).is_err(),
            "{unlisted} decoded"
        );
    }
}

#[test]
fn an_open_response_set_decodes_an_unlisted_value_and_a_closed_one_refuses_it() {
    let (base, server) = common::serve(vec![
        common::response(
            "200 OK",
            &[("Content-Type", "application/json")],
            br#"{"code":7,"setting":{"level":3,"on":true}}"#,
        ),
        common::response(
            "200 OK",
            &[("Content-Type", "application/json")],
            br#"{"code":1,"setting":{"level":2,"on":true}}"#,
        ),
    ]);
    let client = levels_client::BlockingClient::new(&base).unwrap();
    let setting = Setting::new(Level::V1, SettingOn::TRUE);
    let reply = client
        .put_level(Level::V_MINUS_1, &setting)
        .expect("an open set decodes an unlisted value")
        .into_inner();
    assert_eq!(reply.code.get(), 7);
    assert_eq!(reply.setting.level, Level::V3);
    assert!(client.put_level(Level::V3, &setting).is_err());

    let requests = server.join().unwrap();
    assert!(requests[0].line().contains("?min=-1 "), "{}", requests[0].line());
    assert_eq!(
        String::from_utf8_lossy(&requests[0].body),
        r#"{"level":1,"on":true,"tier":2}"#
    );
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "checked"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the checked newtypes must round-trip a listed value and refuse an unlisted one"
    );
}
