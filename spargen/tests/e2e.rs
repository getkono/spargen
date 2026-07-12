use std::process::Command;

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget};

#[test]
fn generates_standalone_crate_for_basic_oas31_api() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Crate {
            dir: Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
            name: "basic_client".to_owned(),
        },
    ));

    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let status = Command::new("cargo")
        .arg("check")
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new("cargo")
        .args(["clippy", "--", "-D", "warnings"])
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    // Prove the wired serde defaults actually deserialize: an absent optional field with a
    // representable scalar default fills in the default instead of `None`, while a required field
    // (default rustdoc-only) still comes from the payload.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/defaults.rs"),
        r##"
#[test]
fn absent_optional_fields_use_schema_defaults() {
    let settings: basic_client::types::Settings =
        serde_json::from_str(r#"{"retries": 7}"#).unwrap();
    assert_eq!(settings.color.as_deref(), Some("red"));
    assert_eq!(settings.enabled, Some(true));
    assert_eq!(settings.ratio, Some(1.5));
    assert_eq!(settings.retries, 7);
    assert_eq!(settings.mode, Some(basic_client::types::Mode::Auto));
}
"##,
    )
    .unwrap();
    let status = Command::new("cargo")
        .arg("test")
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn rejects_openapi_30_without_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        BASIC_SPEC.replace("openapi: 3.1.0", "openapi: 3.0.3"),
    )
    .unwrap();

    let report = spargen::check(&Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ));

    assert_eq!(report.outcome, Outcome::Rejected);
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == Code::UnsupportedOpenApiVersion));
}

#[test]
fn omit_overlay_removes_unsupported_operation() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC_WITH_UNSUPPORTED_OPERATION).unwrap();
    let out = temp.path().join("client.rs");
    let mut config = Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out).unwrap()),
    );
    config.omit = spargen::omit! {
        operations {
            post "/upload";
        }
    };

    let report = spargen::generate(&config);

    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == Code::OmittedConstruct));
}

const BASIC_SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Basic
  version: 1.0.0
servers:
  - url: https://example.com/api
paths:
  /users/{id}:
    get:
      operationId: getUser
      security:
        - bearer: []
        - apiKey: []
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
        - name: page
          in: query
          schema:
            type: integer
            default: 1
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/User"
components:
  securitySchemes:
    bearer:
      type: http
      scheme: bearer
    apiKey:
      type: apiKey
      in: header
      name: X-Api-Key
  schemas:
    User:
      type: object
      required: [id, name]
      properties:
        id:
          type: string
        name:
          type: string
        tree:
          $ref: "#/components/schemas/TreeNode"
        category:
          $ref: "#/components/schemas/Category"
        dict:
          $ref: "#/components/schemas/Dict"
    # Self-recursive: `parent` is a direct back-edge (→ Option<Box<TreeNode>>) and `children`
    # recurses through an array (→ Vec<TreeNode>; the Vec supplies the indirection). Without
    # boxing the direct `parent` back-edge the
    # generated struct would have infinite size and fail to compile.
    TreeNode:
      type: object
      required: [value]
      properties:
        value:
          type: string
        parent:
          $ref: "#/components/schemas/TreeNode"
        children:
          type: array
          items:
            $ref: "#/components/schemas/TreeNode"
    # Mutually recursive: Category <-> Item. One of the two edges in the cycle is boxed.
    Category:
      type: object
      required: [name]
      properties:
        name:
          type: string
        item:
          $ref: "#/components/schemas/Item"
    Item:
      type: object
      required: [label]
      properties:
        label:
          type: string
        category:
          $ref: "#/components/schemas/Category"
    # Self-recursive through additionalProperties (→ BTreeMap<String, Dict>; the map supplies
    # the indirection).
    Dict:
      type: object
      additionalProperties:
        $ref: "#/components/schemas/Dict"
    # `default` on the component schema itself → documented on the generated `Mode` type.
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
    # Exercises schema `default`: representable scalar defaults on optional fields are wired via
    # generated serde providers; a required field's default is rustdoc-only.
    Settings:
      type: object
      required: [retries]
      properties:
        color:
          type: string
          default: red
        enabled:
          type: boolean
          default: true
        ratio:
          type: number
          default: 1.5
        retries:
          type: integer
          default: 3
        # Out-of-range for i32: must NOT be serde-wired (rustdoc-only, W005). If a regression wired
        # `Some(5000000000)` into `Option<i32>`, the generated crate's `cargo check` would fail.
        wide:
          type: integer
          format: int32
          default: 5000000000
        mode:
          $ref: "#/components/schemas/Mode"
          default: auto
"##;

const SPEC_WITH_UNSUPPORTED_OPERATION: &str = r#"
openapi: 3.1.0
info:
  title: Upload
  version: 1.0.0
paths:
  /health:
    get:
      responses:
        "204":
          description: No Content
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
      responses:
        "204":
          description: No Content
"#;
