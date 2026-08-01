use std::process::Command;

use camino::Utf8PathBuf;
use spargen::{Config, Outcome, OutputTarget};

#[test]
fn optional_nullable_presence_and_stream_types_are_public_contracts() {
    let temp = tempfile::tempdir().expect("temporary generation directory");
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC).expect("write focused OpenAPI fixture");
    let out = temp.path().join("client");

    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec).expect("UTF-8 spec path"),
        OutputTarget::Crate {
            dir: Utf8PathBuf::from_path_buf(out.clone()).expect("UTF-8 output path"),
            name: "surface_client".to_owned(),
        },
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).expect("create generated integration tests");
    std::fs::write(
        out.join("tests/public_surface.rs"),
        r#"
use surface_client::types::UpdateDescription;

fn accepts_public_stream_types(
    _stream: Option<surface_client::EventStream<serde_json::Value>>,
    _framing: surface_client::Framing,
) {
}

#[test]
fn optional_nullable_property_preserves_all_three_wire_states() {
    let absent = UpdateDescription { description: None };
    assert_eq!(serde_json::to_value(&absent).unwrap(), serde_json::json!({}));
    assert_eq!(
        serde_json::from_value::<UpdateDescription>(serde_json::json!({}))
            .unwrap()
            .description,
        None
    );

    let cleared = UpdateDescription {
        description: Some(None),
    };
    assert_eq!(
        serde_json::to_value(&cleared).unwrap(),
        serde_json::json!({"description": null})
    );
    assert_eq!(
        serde_json::from_value::<UpdateDescription>(
            serde_json::json!({"description": null}),
        )
        .unwrap()
        .description,
        Some(None)
    );

    let replaced = UpdateDescription {
        description: Some(Some("replacement".to_owned())),
    };
    assert_eq!(
        serde_json::to_value(&replaced).unwrap(),
        serde_json::json!({"description": "replacement"})
    );
    assert_eq!(
        serde_json::from_value::<UpdateDescription>(
            serde_json::json!({"description": "replacement"}),
        )
        .unwrap()
        .description,
        Some(Some("replacement".to_owned()))
    );

    accepts_public_stream_types(None, surface_client::Framing::Ndjson);
}
"#,
    )
    .expect("write generated public-surface test");

    let status = Command::new("cargo")
        .args(["test", "--quiet"])
        .current_dir(&out)
        .status()
        .expect("run generated crate tests");
    assert!(status.success());
}

const SPEC: &str = r##"
openapi: 3.1.0
info: { title: Focused surface, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        "200":
          description: typed events
          content:
            application/x-ndjson:
              schema:
                $ref: "#/components/schemas/Event"
components:
  schemas:
    Event:
      type: object
      required: [message]
      properties:
        message: { type: string }
    UpdateDescription:
      type: object
      properties:
        description:
          type: [string, "null"]
          x-spargen-preserve-null: true
"##;
