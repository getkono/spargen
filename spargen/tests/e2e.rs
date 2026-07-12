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

#[test]
fn pattern_properties_capture_into_typed_overflow_map() {
    // The declared `host` field is typed; every non-declared property is captured by the flatten
    // `BTreeMap<String, String>` overflow that `patternProperties` lowered to.
    let headers: basic_client::types::Headers =
        serde_json::from_str(r#"{"host": "h", "x-a": "1", "x-b": "2"}"#).unwrap();
    assert_eq!(headers.host.as_deref(), Some("h"));
    assert_eq!(headers.additional.get("x-a").map(String::as_str), Some("1"));
    assert_eq!(headers.additional.get("x-b").map(String::as_str), Some("2"));
}

#[test]
fn null_mixed_enum_field_is_option_of_enum() {
    // The null-mixed `Priority` enum lowered to a real Rust enum used behind `Option`: an absent
    // field and an explicit `null` both deserialize to `None`; a string value to the variant.
    let absent: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n"}"#).unwrap();
    assert_eq!(absent.priority, None);

    let explicit_null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": null}"#).unwrap();
    assert_eq!(explicit_null.priority, None);

    let set: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": "high"}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
}

#[test]
fn component_nullability_propagates_through_ref() {
    // A REQUIRED field referencing the nullable `Priority` component is `Option<Priority>`: the key
    // must be present, but `null` deserializes to `None` and a string to the variant. This only
    // holds if the component's nullability propagated to the `$ref` use site.
    let null_priority: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": null, "history": []}"#).unwrap();
    assert_eq!(null_priority.priority, None);

    // An array of the nullable component is `Vec<Option<Priority>>`: a `null` element is accepted.
    let set: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": "high", "history": ["low", null]}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
    assert_eq!(
        set.history,
        vec![Some(basic_client::types::Priority::Low), None]
    );
}

#[test]
fn all_of_merged_struct_carries_every_member_field() {
    // `Account` merged a `$ref` base (id, required), an inline member (label, required) and a
    // sibling property (owner, optional). Required fields are plain, the optional is `Option`, and a
    // payload carrying all three deserializes into the single flattened struct.
    let account: basic_client::types::Account =
        serde_json::from_str(r#"{"id": "a1", "label": "L", "owner": "o"}"#).unwrap();
    assert_eq!(account.id, "a1");
    assert_eq!(account.label, "L");
    assert_eq!(account.owner.as_deref(), Some("o"));
}

#[test]
fn discriminated_union_round_trips_with_tag() {
    // Cat DECLARES `petType` as a required property — the shape that broke serde internal tagging
    // ("missing field petType"). The custom buffer-to-Value Deserialize hands the WHOLE value to the
    // variant, so Cat's own `pet_type` field is filled, and re-serialization keeps the tag.
    let pet: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "cat", "name": "Whiskers"}"#).unwrap();
    match &pet {
        basic_client::types::Pet::Cat(cat) => {
            assert_eq!(cat.name, "Whiskers");
            assert_eq!(cat.pet_type, "cat");
        }
        other => panic!("expected Cat variant, got {other:?}"),
    }
    let json = serde_json::to_value(&pet).unwrap();
    assert_eq!(json["petType"], "cat");
    assert_eq!(json["name"], "Whiskers");

    // Dog does NOT declare `petType`; the custom Serialize re-inserts the tag it would otherwise
    // lack, and deserialization still routes by the tag.
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "dog", "bark": true}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    let json = serde_json::to_value(&dog).unwrap();
    assert_eq!(json["petType"], "dog");
    assert_eq!(json["bark"], true);
}

#[test]
fn nullable_variant_union_resolves_null_at_option() {
    // A `null` payload resolves at the outer `Option` (variant nullability hoisted to the union),
    // and non-null string/array content routes to the right disjoint variant and re-serializes as a
    // bare value.
    let null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": null}"#).unwrap();
    assert!(null.notes.is_none());

    let text: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": "hi"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&text.notes).unwrap(),
        serde_json::json!("hi")
    );

    let list: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": ["a", "b"]}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&list.notes).unwrap(),
        serde_json::json!(["a", "b"])
    );
}

#[test]
fn disjoint_union_round_trips_without_wrapper() {
    // A `string` payload deserializes to the string variant and re-serializes as a BARE string —
    // no tag, no wrapper (Issue #9, strategy B custom Serialize).
    let text: basic_client::types::StringOrList =
        serde_json::from_str(r#""hello""#).unwrap();
    assert_eq!(serde_json::to_string(&text).unwrap(), r#""hello""#);

    // An `array` payload deserializes to the array variant and re-serializes as a bare array.
    let list: basic_client::types::StringOrList =
        serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(serde_json::to_string(&list).unwrap(), r#"["a","b"]"#);
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
        # Optional nullable query param (Issue #6): `type: [integer, "null"]` lowers to a nullable
        # `Ty`, which `ty_tokens` renders as `Option<i64>`. The params struct must NOT wrap it again
        # (`Option<Option<i64>>` would not serialize — `Option<i64>: !Display`).
        - name: filter
          in: query
          schema:
            type: [integer, "null"]
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
        priority:
          $ref: "#/components/schemas/Priority"
        # Discriminated union (Issue #9): an internally-tagged enum over object `$ref` variants.
        pet:
          $ref: "#/components/schemas/Pet"
        # Undiscriminated but provably-disjoint union (string vs array JSON category): an enum with a
        # content-inspecting custom Deserialize/Serialize — no wrapper on the wire.
        alias:
          $ref: "#/components/schemas/StringOrList"
        # Nullable union variant (Issue #9 fix 2): the string variant is `{type: [string, null]}`;
        # its nullability is HOISTED to the union so this field is `Option<...>` and a `null` payload
        # resolves to `None` rather than erroring in the custom Deserialize.
        notes:
          $ref: "#/components/schemas/StringListOrNull"
    # Discriminated union: `petType` selects the object variant. Cat DECLARES `petType` as a required
    # property (the shape that broke serde internal tagging — "missing field petType"); the custom
    # buffer-to-Value Deserialize hands the WHOLE value to the variant, so Cat keeps its own tag.
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
    # Dog does NOT declare `petType`; on serialize the custom Serialize re-inserts the tag.
    Dog:
      type: object
      required: [bark]
      properties:
        bark:
          type: boolean
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
    # Disjoint by JSON type category: a bare string or a list of strings. Serializes WITHOUT any tag
    # or wrapper — the active variant's inner value is emitted directly.
    StringOrList:
      oneOf:
        - type: string
        - type: array
          items:
            type: string
    # Nullable-variant union: the string member is nullable, hoisted to make the whole union nullable.
    StringListOrNull:
      oneOf:
        - type: [string, "null"]
        - type: array
          items:
            type: string
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
    # Null-mixed enum (Issue #6): the `null` member is stripped and the remaining homogeneous string
    # scalars lower as a real Rust enum; the `"null"` in the type array makes every use nullable, so
    # a field of this type is emitted as `Option<Priority>`. An absent or `null` value deserializes
    # to `None`; a string value to the matching variant.
    Priority:
      type: [string, "null"]
      enum: [low, medium, high, null]
    # Propagation of component nullability through `$ref` (Issue #6): a REQUIRED field whose type is
    # the nullable `Priority` component must still be `Option<Priority>` (present, but may be `null`),
    # and an array of the component must be `Vec<Option<Priority>>` (a null element is accepted).
    # Before propagation these emitted `Priority` / `Vec<Priority>` and rejected a conforming `null`.
    Ticket:
      type: object
      required: [priority, history]
      properties:
        priority:
          $ref: "#/components/schemas/Priority"
        history:
          type: array
          items:
            $ref: "#/components/schemas/Priority"
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
    # `patternProperties` composed with an explicit property: the declared `host` field plus a typed
    # overflow map (`#[serde(flatten)] BTreeMap<String, String>`) for the pattern-matched keys. The
    # key regex is validation-only (W001) and not enforced by the map.
    Headers:
      type: object
      properties:
        host:
          type: string
      patternProperties:
        "^x-": { type: string }
    # Object-ness comes *only* from `patternProperties` (no `type`, no `properties`): still a struct
    # with empty fields and a typed overflow map, not an untyped `Any`.
    Tags:
      patternProperties:
        "^tag-": { type: string }
    # A declared property literally named `additional` alongside a typed overflow map: the synthetic
    # flatten field must be allocated in the field scope and disambiguated, or two `pub additional:`
    # fields would collide and the generated crate would fail to compile.
    Bag:
      type: object
      properties:
        additional:
          type: string
      patternProperties:
        "^x-": { type: integer }
    # allOf merge (Issue #8): `Account` flattens a `$ref` base (id, required), an inline member
    # (label, required) and the enclosing schema's own sibling property (owner, optional) into ONE
    # struct. All fields must be present and correctly typed in the generated `Account` type.
    AccountBase:
      type: object
      required: [id]
      properties:
        id:
          type: string
    Account:
      type: object
      properties:
        owner:
          type: string
      allOf:
        - $ref: "#/components/schemas/AccountBase"
        - type: object
          required: [label]
          properties:
            label:
              type: string
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
