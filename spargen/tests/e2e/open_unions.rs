//! `open_unions`: the generated code for an open discriminated union, compiled and linted,
//! decoding listed and unlisted tags and writing an unknown payload back (#1233).

use super::*;

/// A discriminated union whose description promises new kinds (`ServerEvent`), one with an
/// untagged inline member (`Notice`), and one with a member named `Unknown`, used as a request
/// body (`Shape`).
const OPEN_UNIONS_SPEC: &str = include_str!("specs/open_unions.yaml");

/// The doc line the open variant's `value` field carries, which only an open union emits.
const OPEN_VARIANT_DOC: &str = "The whole payload, as it arrived.";

#[test]
fn open_unions_decodes_an_unlisted_tag_into_the_unknown_variant() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "events.yaml", OPEN_UNIONS_SPEC);

    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "open_unions", "2021", |spec| {
        spec.open_unions(true)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the open unions must pass clippy -D warnings"
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/open.rs"),
        r##"use open_unions::types::{Notice, ServerEvent, Shape};

/// `decoded` failed because the type refused a well-formed value: a `Data` error, never a syntax
/// or end-of-input one, which would mean the test's own document was malformed and proved nothing.
#[track_caller]
fn assert_refused<T: std::fmt::Debug>(decoded: serde_json::Result<T>, what: &str) {
    match decoded {
        Ok(value) => panic!("{what}: the type admits no such value, yet produced {value:?}"),
        Err(error) => assert_eq!(
            error.classify(),
            serde_json::error::Category::Data,
            "{what}: refused for the wrong reason: {error}"
        ),
    }
}

#[test]
fn a_listed_tag_selects_its_variant_and_an_unlisted_one_is_kept() {
    let tick: ServerEvent = serde_json::from_str(r#"{"kind":"tick","n":3}"#).unwrap();
    assert!(matches!(tick, ServerEvent::Tick(ref tick) if tick.n == 3), "{tick:?}");

    let wire = serde_json::json!({"kind": "resize", "w": 3});
    let unknown: ServerEvent = serde_json::from_value(wire.clone()).unwrap();
    let ServerEvent::Unknown { tag, value } = &unknown else {
        panic!("an unlisted tag decodes into `Unknown`: {unknown:?}");
    };
    assert_eq!(tag, "resize");
    assert_eq!(value, &wire);
    // It is written back as it arrived.
    assert_eq!(serde_json::to_value(&unknown).unwrap(), wire);

    // A listed tag whose payload is malformed is still refused, not taken as unknown.
    assert_refused(
        serde_json::from_str::<ServerEvent>(r#"{"kind":"tick"}"#),
        "a tick missing `n`",
    );
    // So is a payload with no tag, or one that is not a string: there is no tag to keep.
    assert_refused(
        serde_json::from_str::<ServerEvent>(r#"{"n":3}"#),
        "a missing tag",
    );
    assert_refused(
        serde_json::from_str::<ServerEvent>(r#"{"kind":7}"#),
        "a non-string tag",
    );
}

#[test]
fn a_constructed_unknown_payload_is_written_with_its_tag() {
    let event = ServerEvent::Unknown {
        tag: "resize".to_owned(),
        value: serde_json::json!({"w": 3}),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({"kind": "resize", "w": 3})
    );
    // A tag the value already carries is kept, as a tagged variant keeps its own field.
    let event = ServerEvent::Unknown {
        tag: "resize".to_owned(),
        value: serde_json::json!({"kind": "other"}),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({"kind": "other"})
    );
    // A non-object value has nowhere to carry the tag.
    let event = ServerEvent::Unknown {
        tag: "resize".to_owned(),
        value: serde_json::json!([1]),
    };
    assert!(serde_json::to_value(&event).is_err());
}

#[test]
fn an_untagged_member_is_tried_before_the_unknown_variant() {
    let note: Notice = serde_json::from_str(r#"{"kind":"memo","note":"hi"}"#).unwrap();
    assert!(!matches!(note, Notice::Unknown { .. }), "{note:?}");
    let unknown: Notice = serde_json::from_str(r#"{"kind":"memo"}"#).unwrap();
    assert!(
        matches!(unknown, Notice::Unknown { ref tag, .. } if tag == "memo"),
        "{unknown:?}"
    );
}

#[test]
fn a_member_named_unknown_keeps_its_name() {
    let member: Shape = serde_json::from_str(r#"{"shape":"Unknown"}"#).unwrap();
    assert!(matches!(member, Shape::Unknown(_)), "{member:?}");
    let unlisted: Shape = serde_json::from_str(r#"{"shape":"square","side":2}"#).unwrap();
    let debug = format!("{unlisted:?}");
    assert!(
        debug.contains(r#"tag: "square""#) && !matches!(unlisted, Shape::Unknown(_)),
        "{debug}"
    );
}
"##,
    )
    .unwrap();
    let status = fixture_cargo(&out)
        .args(["test", "--test", "open"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the open unions must decode as documented"
    );
}

/// The option off, no union is open; on, a union with a 3.2 `defaultMapping` keeps its fallback
/// and gains no `Unknown` variant, since the fallback already takes every unlisted tag.
#[test]
fn open_unions_opens_only_a_discriminated_union_without_a_fallback() {
    let closed = spec_module("events.yaml", OPEN_UNIONS_SPEC, |spec| spec);
    assert!(!closed.contains(OPEN_VARIANT_DOC), "{closed}");
    let open = spec_module("events.yaml", OPEN_UNIONS_SPEC, |spec| {
        spec.open_unions(true)
    });
    assert_eq!(
        open.matches(OPEN_VARIANT_DOC).count(),
        3,
        "each of the three unions opens:\n{open}"
    );

    let fallback = r##"
openapi: 3.2.0
info: { title: Fallback, version: 1.0.0 }
paths:
  /outcome:
    get:
      operationId: getOutcome
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                oneOf:
                  - $ref: "#/components/schemas/Done"
                  - $ref: "#/components/schemas/Other"
                discriminator:
                  propertyName: outcome
                  mapping:
                    done: "#/components/schemas/Done"
                  defaultMapping: "#/components/schemas/Other"
components:
  schemas:
    Done:
      type: object
      required: [outcome]
      properties:
        outcome: { type: string }
    Other:
      type: object
      required: [outcome]
      properties:
        outcome: { type: string }
"##;
    let module = spec_module("fallback.yaml", fallback, |spec| spec.open_unions(true));
    assert!(!module.contains(OPEN_VARIANT_DOC), "{module}");
}
