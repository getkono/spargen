//! Per-diagnostic frontend coverage: one minimal inline spec per rejection/warning code, asserting
//! the code fires and the pipeline outcome is what the taxonomy promises. Rejections travel through
//! `generate`; the E013 case also proves `check` runs the same lowering (check/generate parity).

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget, Report};

/// Run `generate` on an inline spec written into a throwaway tempdir, returning the report. The
/// tempdir (and any written output) is discarded once the report — which owns its data — is built.
fn generate(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    let out = temp.path().join("client.rs");
    spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out).unwrap()),
    ))
}

/// As [`generate`], but through the `check` entry point (no codegen/emit).
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    spargen::check(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ))
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics.iter().any(|d| d.code == code)
}

#[test]
fn e002_unsupported_dialect() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/not-the-base
paths: {}
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedDialect));
}

#[test]
fn pattern_properties_lowers_to_typed_map_with_w001() {
    // A representable `patternProperties` now GENERATES a typed overflow map instead of being
    // rejected. Two inline `{type: string}` value schemas under different patterns collapse to one
    // `BTreeMap<String, String>` (bounded structural equivalence over leaf primitives). The key
    // regexes are validation-only and acknowledged as `W001`, never silently dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^x-": { type: string }
        "^y-": { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    // check/generate parity: the same disposition is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "{checked:#?}"
    );
}

#[test]
fn pattern_properties_cyclic_array_values_terminate() {
    // Mutually-recursive array value schemas (`A = [B]`, `B = [A]`) form a cycle in the structural
    // homogeneity comparison. The visited-pair guard must terminate (return an outcome, never abort
    // with a stack overflow) and, since both patterns lower to the same array type, GENERATE one
    // typed overflow map. The check/generate parity assertion catches a regression that reintroduces
    // the crash.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: array, items: { $ref: "#/components/schemas/B" } }
    B: { type: array, items: { $ref: "#/components/schemas/A" } }
    Thing:
      type: object
      patternProperties:
        "^a-": { $ref: "#/components/schemas/A" }
        "^b-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e005_pattern_properties_heterogeneous_rejected() {
    // Two pattern value schemas that lower to different types cannot share one typed map → E005.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: string }
    B: { type: integer }
    Thing:
      type: object
      patternProperties:
        "^s-": { $ref: "#/components/schemas/A" }
        "^i-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
    // check/generate parity: the rejection fires in `check` too.
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::PatternPropertiesRejected));
}

#[test]
fn e005_pattern_properties_with_deny_rejected() {
    // `patternProperties` + `additionalProperties: false` cannot be faithfully represented → E005.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      additionalProperties: false
      patternProperties:
        "^x-": { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
}

#[test]
fn e006_dynamic_ref_rejected() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      $dynamicRef: "#meta"
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DynamicRefRejected));
}

#[test]
fn e007_non_disjoint_union() {
    // `integer | number` share the JSON numeric category (they overlap on the wire), so the union is
    // NOT provably disjoint → E007 (narrowed). A payload `1` could match either variant.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: integer
        - type: number
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
    // check/generate parity.
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion));
}

#[test]
fn e007_overlapping_required_keys_rejected() {
    // Two object variants that share their only required key are not disjoint by key presence, and
    // both are the Object JSON category → no proof holds → E007.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: object
          required: [kind]
          properties: { kind: { type: string }, a: { type: string } }
        - type: object
          required: [kind]
          properties: { kind: { type: string }, b: { type: string } }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
}

#[test]
fn string_integer_union_generates() {
    // `string | integer` occupy distinct JSON type categories (string vs number) → provably disjoint
    // → GENERATES (this replaced the old, incorrect E007 fixture, which asserted rejection here).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn discriminated_union_with_mapping_generates() {
    // A `discriminator` with an explicit mapping over object `$ref` variants → an internally-tagged
    // enum. Generates without E007. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Dog:
      type: object
      required: [bark]
      properties: { bark: { type: boolean } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_discriminated_non_object_variant_rejected() {
    // A discriminated variant that is not an object (a primitive) cannot be internally tagged → E007.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - type: string
      discriminator:
        propertyName: petType
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
}

#[test]
fn disjoint_string_array_union_generates() {
    // `string | string[]` occupy distinct JSON type categories (string vs array) → provably disjoint
    // (ollama's dominant shape). Generates without E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn required_key_disjoint_objects_generate() {
    // Two CLOSED object variants (`additionalProperties: false`) each with a unique required key
    // (`a` / `b`) → provably disjoint by key presence → GENERATES with a content-inspecting custom
    // Deserialize. Closed is required for soundness (see `e007_open_object_required_key_rejected`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      additionalProperties: false
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      additionalProperties: false
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_open_object_required_key_rejected() {
    // OPEN object variants (default `additionalProperties`) are NOT provably disjoint by required
    // key: a payload for B could carry A's key `a` as an extra field and be misrouted → E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn nullable_variant_hoists_to_option() {
    // A variant that is itself nullable (`{type: [string, "null"]}`) has its nullability HOISTED to
    // the union: the union becomes `Option<Enum>` and the string/array variants stay disjoint. A
    // `null` payload resolves at the outer `Option`, so the custom Deserialize only sees non-null.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn nullable_union_collapses_to_option() {
    // A 2-member union where one member is `{type: "null"}` strips the null and collapses to
    // `Option<String>` — no enum, no E007. Generates.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: "null"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e008_non_scalar_enum() {
    // Mixed scalar kinds with no null are genuinely unrepresentable: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      enum: ["a", 1]
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn e008_stays_for_object_member_enum() {
    // Object (or array) enum members have no scalar-variant representation: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Structured:
      enum: [{ a: 1 }]
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn null_mixed_scalar_enum_generates() {
    // A `null` member is stripped; the remaining homogeneous string scalars lower as a nullable
    // enum (`Option<Enum>`). No E008, and generation succeeds. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Severity:
      type: [string, "null"]
      enum: [low, medium, high, null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::NonScalarEnum), "{checked:#?}");
}

#[test]
fn all_null_enum_generates_as_nullable() {
    // A value set of only `null` has no scalar remainder: it lowers to a faithful nullable untyped
    // value rather than being rejected.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nothing:
      enum: [null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e009_unsupported_media_type() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn multipart_form_data_request_body_generates() {
    // A `multipart/form-data` request body whose schema is an object (a file part + a text part) is
    // now supported: it generates without E009 firing. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn e009_multipart_non_object_body_rejected() {
    // A `multipart/form-data` body whose schema is NOT an object has no properties to enumerate as
    // form parts, so it stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn binary_format_in_param_and_text_body_positions_generate() {
    // Regression guard for `format: binary` → `bytes::Bytes` in positions rendered as strings: a
    // binary PATH param, a binary QUERY param, and a `text/plain` body of `format: binary` must all
    // generate cleanly (the e2e suite compile-verifies they do not silently miscompile). `Bytes` is
    // not `Display`; params are remapped to `String` and a Bytes body is sent raw, never `.to_string`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob/{token}:
    get:
      parameters:
        - name: token
          in: path
          required: true
          schema: { type: string, format: binary }
        - name: cursor
          in: query
          schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
  /raw:
    post:
      requestBody:
        required: true
        content:
          text/plain:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e010_unsupported_parameter_style() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: deepObject
          schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedParameterStyle));
}

#[test]
fn e012_unknown_security_scheme() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - undeclared: []
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnknownSecurityScheme));
}

/// A single-member `allOf` now MERGES into one typed struct instead of being rejected (E013 is
/// repurposed to mean "irreconcilable composition"). Generation succeeds with no E013.
#[test]
fn all_of_single_member_merges_into_struct() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Composed:
      allOf:
        - type: object
          properties:
            a: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf: [{$ref: Base}, {properties: {extra}}]` flattens the referenced component's fields plus
/// the inline member's fields into one struct.
#[test]
fn all_of_ref_plus_inline_members_merge() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
        - type: object
          properties:
            extra: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A nested `allOf` (an `allOf` member that itself has an `allOf`) flattens recursively into one
/// struct.
#[test]
fn all_of_nested_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nested:
      allOf:
        - allOf:
            - type: object
              properties:
                a: { type: string }
        - type: object
          properties:
            b: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf` beside the enclosing schema's own sibling `properties`: both sets of fields merge.
#[test]
fn all_of_beside_sibling_properties_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Sibling:
      type: object
      properties:
        own: { type: string }
      allOf:
        - type: object
          properties:
            base: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A property declared with different lowered types in two `allOf` members is irreconcilable → E013.
const ALL_OF_CONFLICT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Conflict:
      allOf:
        - type: object
          properties:
            x: { type: string }
        - type: object
          properties:
            x: { type: integer }
"##;

#[test]
fn e013_all_of_conflicting_property_types_rejected() {
    let report = generate(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Mixing an object member with a scalar member has no single representable type → E013.
#[test]
fn e013_all_of_object_scalar_mix_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      allOf:
        - type: object
          properties:
            a: { type: string }
        - type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// `check` must run the same lowering as `generate`, so an irreconcilable `allOf` rejects
/// identically through both entry points (check/generate parity).
#[test]
fn e013_check_generate_parity() {
    let report = check(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// A self-referential component (`Node.next -> Node`) once recursed forever, then was rejected as
/// E014. It must now generate: the cycle-closing `$ref` is boxed so the recursive type is finite.
#[test]
fn self_recursive_ref_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Node:
      type: object
      properties:
        next:
          $ref: "#/components/schemas/Node"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "recursive schema must not raise an error: {report:#?}"
    );
}

/// Mutually-recursive components (`A -> B -> A`, including recursion through an array) must also
/// generate: exactly one back-edge in the cycle is boxed.
#[test]
fn mutually_recursive_refs_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      properties:
        b:
          $ref: "#/components/schemas/B"
    B:
      type: object
      properties:
        children:
          type: array
          items:
            $ref: "#/components/schemas/A"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "mutually-recursive schemas must not raise an error: {report:#?}"
    );
}

#[test]
fn w001_validation_keyword_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
components:
  schemas:
    Age:
      type: integer
      minimum: 0
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ValidationKeywordIgnored));
}

#[test]
fn w002_server_initiated_flow_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
webhooks:
  newThing:
    post:
      responses:
        "200": { description: OK }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ServerInitiatedFlowIgnored));
}

const W005_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        count:
          type: integer
          default: "not-a-number"
        meta:
          type: object
          default: { a: 1 }
"##;

#[test]
fn w005_schema_default_not_applied_still_generates() {
    let report = generate(W005_SPEC);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// `check` runs the same lowering as `generate`, so the W005 disposition fires identically.
#[test]
fn w005_check_generate_parity() {
    let report = check(W005_SPEC);
    assert_eq!(report.outcome, Outcome::Clean, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// A representable scalar default on an optional field is applied via serde and must not raise
/// W005 (or any error): generation succeeds and the field is documented with its default.
#[test]
fn representable_scalar_default_applies_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        color:
          type: string
          default: "red"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A parameter `default` is documented in rustdoc (never serde-wired) — generation is clean and
/// must NOT raise W005 (parameters always have a documentation home).
#[test]
fn parameter_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      parameters:
        - name: per_page
          in: query
          schema: { type: integer, default: 30 }
        - name: sort
          in: query
          required: true
          schema: { type: string, default: name }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` on a component schema itself (here an enum) is documented on the generated named
/// type — generation is clean, with no W005 and no double-handling.
#[test]
fn component_root_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` in a structural position with no field home — array `items` and an
/// `additionalProperties` value — is non-silent: it fires W005 and still generates.
#[test]
fn structural_defaults_fire_w005_and_still_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Tags:
      type: array
      items: { type: string, default: hi }
    Counts:
      type: object
      additionalProperties: { type: integer, default: 5 }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// An out-of-range integer default for the field's width (`int32` here) is NOT representable: it
/// must fire W005 and stay rustdoc-only, never rendered into a literal that fails to compile.
#[test]
fn out_of_range_int_default_fires_w005_and_is_not_wired() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        big:
          type: integer
          format: int32
          default: 5000000000
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// A component that is a bare `$ref` with a sibling `default` drops the default when the reference
/// resolves; it must be acknowledged with W005 rather than lost silently, and still generate.
#[test]
fn component_root_ref_with_default_fires_w005_and_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Bar:
      type: string
    Alias:
      $ref: "#/components/schemas/Bar"
      default: aliased
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

#[test]
fn multi_status_success_bodies_generate_a_typed_enum_without_w003() {
    // Two success statuses with DIFFERENT bodies used to degrade to `serde_json::Value` (W003).
    // W003 is retired: the success type is now a typed per-operation response enum, generated with
    // no diagnostic at all.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyA" }
        "201":
          description: Created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyB" }
components:
  schemas:
    BodyA:
      type: object
      properties:
        a: { type: string }
    BodyB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    // No diagnostics at all — the retired W003 must not fire under any code.
    assert!(report.diagnostics.is_empty(), "{report:#?}");
}

#[test]
fn multi_status_error_bodies_generate_a_typed_enum_without_w003() {
    // Two error statuses with DIFFERENT bodies likewise generate a typed error enum, no W003.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string }
        "404":
          description: Not Found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrA" }
        "409":
          description: Conflict
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrB" }
components:
  schemas:
    ErrA:
      type: object
      properties:
        a: { type: string }
    ErrB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics.is_empty(), "{report:#?}");
}

#[test]
fn multi_status_enum_precedence_emits_exact_arm_before_range_and_a_bodyless_unit_variant() {
    // Both classes list a RANGE before an overlapping EXACT in document order (and mix in a bodyless
    // 204). The emitter must reorder to exact-before-range so a real 200/409 dispatches to its exact
    // variant, and the bodyless 204 must appear as a payload-free unit variant — never a silent drop.
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      operationId: getP
      responses:
        "2XX":
          description: RangeOk
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeOk" } } }
        "200":
          description: ExactOk
          content: { application/json: { schema: { $ref: "#/components/schemas/ExactOk" } } }
        "204":
          description: No Content
        "4XX":
          description: RangeErr
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeErr" } } }
        "409":
          description: Conflict
          content: { application/json: { schema: { $ref: "#/components/schemas/Conflict" } } }
components:
  schemas:
    RangeOk: { type: object, properties: { r: { type: string } } }
    ExactOk: { type: object, properties: { e: { type: string } } }
    RangeErr: { type: object, properties: { x: { type: string } } }
    Conflict: { type: object, properties: { c: { type: string } } }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out.clone()).unwrap()),
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics.is_empty(), "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    // Success dispatch: the exact 200 arm is emitted (and thus checked) before the 2XX range arm.
    let exact_200 = code.find("Exact(200u16)").expect("exact 200 selector");
    let range_2xx = code.find("Range(2u8)").expect("2XX range selector");
    assert!(
        exact_200 < range_2xx,
        "exact 200 must precede the 2XX range in the emitted decode chain"
    );
    // Error classification: the exact 409 arm precedes the 4XX range arm.
    let exact_409 = code.find("Exact(409u16)").expect("exact 409 selector");
    let range_4xx = code.find("Range(4u8)").expect("4XX range selector");
    assert!(
        exact_409 < range_4xx,
        "exact 409 must precede the 4XX range in the emitted classification chain"
    );
    // The bodyless 204 is a payload-free unit variant, not dropped and not a `serde_json::Value`.
    assert!(
        code.contains("Status204,"),
        "bodyless 204 must emit a unit variant"
    );
}
