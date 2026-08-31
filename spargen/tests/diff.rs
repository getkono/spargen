//! `spargen diff` semver-impact classification. Each test crafts an old/new pair of inline specs
//! and asserts the classified change and the overall recommended bump. The surface model and its
//! classification policy live in `spargen/src/surface/`.

use camino::Utf8PathBuf;
use spargen::{ChangeKind, DiffReport, Impact, Spec};

/// Assemble a minimal, valid 3.1 spec from its variable parts:
/// * `params` — the `get` operation's `parameters:` block (6-space indent), or `""` for none;
/// * `pet_required` — the comma-separated `required` list for the `Pet` schema;
/// * `pet_props` — the `Pet` property lines (8-space indent), each newline-terminated;
/// * `extra_path` — an additional path item under `paths:` (2-space indent), or `""` for none.
fn spec(params: &str, pet_required: &str, pet_props: &str, extra_path: &str) -> String {
    format!(
        "openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /pets:
    get:
      operationId: listPets
{params}      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {{ $ref: '#/components/schemas/Pet' }}
{extra_path}components:
  schemas:
    Pet:
      type: object
      required: [{pet_required}]
      properties:
{pet_props}"
    )
}

const PET_PROPS: &str = "        id: { type: integer }\n        name: { type: string }\n";

const PARAM_OPTIONAL_INT: &str = "      parameters:
        - name: limit
          in: query
          required: false
          schema: { type: integer }
";

const PARAM_REQUIRED_INT: &str = "      parameters:
        - name: limit
          in: query
          required: true
          schema: { type: integer }
";

const PARAM_OPTIONAL_STRING: &str = "      parameters:
        - name: limit
          in: query
          required: false
          schema: { type: string }
";

const EXTRA_OP: &str = "  /owners:
    get:
      operationId: listOwners
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: string }
";

/// The base spec: one operation, no params, a `Pet` with a required `id` and an optional `name`.
fn base() -> String {
    spec("", "id", PET_PROPS, "")
}

/// Diff two inline specs, asserting both lowered successfully, and return the report.
fn diff(old_spec: &str, new_spec: &str) -> DiffReport {
    let temp = tempfile::tempdir().unwrap();
    let old_path = temp.path().join("old.yaml");
    let new_path = temp.path().join("new.yaml");
    std::fs::write(&old_path, old_spec).unwrap();
    std::fs::write(&new_path, new_spec).unwrap();
    let old = Spec::new(Utf8PathBuf::from_path_buf(old_path).unwrap());
    let new = Spec::new(Utf8PathBuf::from_path_buf(new_path).unwrap());
    spargen::diff(&old, &new).expect("both specs should lower")
}

/// The kinds present in a report, for order-independent membership assertions.
fn kinds(report: &DiffReport) -> Vec<ChangeKind> {
    report.changes.iter().map(|change| change.kind).collect()
}

/// A stable textual fingerprint of a report (for determinism assertions).
fn fingerprint(report: &DiffReport) -> Vec<String> {
    let mut lines: Vec<String> = report
        .changes
        .iter()
        .map(|change| {
            format!(
                "{}|{}|{}|{}",
                change.impact.as_str(),
                change.kind.code(),
                change.location,
                change.detail
            )
        })
        .collect();
    lines.push(format!("bump={}", report.bump.as_str()));
    lines
}

#[test]
fn identical_specs_are_patch() {
    let report = diff(&base(), &base());
    assert!(report.changes.is_empty(), "changes: {:?}", report.changes);
    assert_eq!(report.bump, Impact::Patch);
    assert_eq!(report.summary(), "patch: no public API changes");
}

#[test]
fn docs_only_change_is_patch() {
    // Adding a `description` to a property changes rustdoc only, not the field's (type, required)
    // surface — so the diff is a no-op patch.
    let documented =
        "        id: { type: integer }\n        name: { type: string, description: The name. }\n";
    let report = diff(&base(), &spec("", "id", documented, ""));
    assert!(report.changes.is_empty(), "changes: {:?}", report.changes);
    assert_eq!(report.bump, Impact::Patch);
}

#[test]
fn added_operation_is_minor() {
    let report = diff(&base(), &spec("", "id", PET_PROPS, EXTRA_OP));
    assert_eq!(kinds(&report), vec![ChangeKind::OperationAdded]);
    assert_eq!(report.bump, Impact::Minor);
}

#[test]
fn removed_operation_is_major() {
    let report = diff(&spec("", "id", PET_PROPS, EXTRA_OP), &base());
    assert_eq!(kinds(&report), vec![ChangeKind::OperationRemoved]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn added_optional_param_is_minor() {
    let report = diff(&base(), &spec(PARAM_OPTIONAL_INT, "id", PET_PROPS, ""));
    assert_eq!(kinds(&report), vec![ChangeKind::OptionalParamAdded]);
    assert_eq!(report.bump, Impact::Minor);
}

#[test]
fn added_required_param_is_major() {
    let report = diff(&base(), &spec(PARAM_REQUIRED_INT, "id", PET_PROPS, ""));
    assert_eq!(kinds(&report), vec![ChangeKind::RequiredParamAdded]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn changed_param_type_is_major() {
    let old = spec(PARAM_OPTIONAL_INT, "id", PET_PROPS, "");
    let new = spec(PARAM_OPTIONAL_STRING, "id", PET_PROPS, "");
    let report = diff(&old, &new);
    assert_eq!(kinds(&report), vec![ChangeKind::ParamTypeChanged]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn added_optional_field_is_minor() {
    let with_tag = "        id: { type: integer }\n        name: { type: string }\n        tag: { type: string }\n";
    let report = diff(&base(), &spec("", "id", with_tag, ""));
    assert_eq!(kinds(&report), vec![ChangeKind::FieldAdded]);
    assert_eq!(report.bump, Impact::Minor);
}

#[test]
fn added_required_field_is_major() {
    // A newly-required field breaks every existing constructor of the type.
    let with_tag = "        id: { type: integer }\n        name: { type: string }\n        tag: { type: string }\n";
    let report = diff(&base(), &spec("", "id, tag", with_tag, ""));
    assert!(kinds(&report).contains(&ChangeKind::RequiredFieldAdded));
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn removed_field_is_major() {
    let only_id = "        id: { type: integer }\n";
    let report = diff(&base(), &spec("", "id", only_id, ""));
    assert_eq!(kinds(&report), vec![ChangeKind::FieldRemoved]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn changed_field_type_is_major() {
    let id_string = "        id: { type: string }\n        name: { type: string }\n";
    let report = diff(&base(), &spec("", "id", id_string, ""));
    assert_eq!(kinds(&report), vec![ChangeKind::FieldTypeChanged]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn overall_bump_is_max_impact_across_mixed_changes() {
    // New spec: adds an optional param (minor) AND removes a field (major) AND adds an operation
    // (minor). The overall bump is the max — major — and every change is reported.
    let only_id = "        id: { type: integer }\n";
    let old = base();
    let new = spec(PARAM_OPTIONAL_INT, "id", only_id, EXTRA_OP);
    let report = diff(&old, &new);
    let kinds = kinds(&report);
    assert!(kinds.contains(&ChangeKind::OptionalParamAdded), "{kinds:?}");
    assert!(kinds.contains(&ChangeKind::FieldRemoved), "{kinds:?}");
    assert!(kinds.contains(&ChangeKind::OperationAdded), "{kinds:?}");
    assert_eq!(report.bump, Impact::Major);
    // Deterministic order: most-severe first.
    assert_eq!(report.changes[0].impact, Impact::Major);
}

#[test]
fn same_pair_twice_is_identical() {
    // Determinism: diffing the same pair twice yields a byte-identical report.
    let only_id = "        id: { type: integer }\n";
    let old = base();
    let new = spec(PARAM_OPTIONAL_INT, "id", only_id, EXTRA_OP);
    let first = diff(&old, &new);
    let second = diff(&old, &new);
    assert_eq!(fingerprint(&first), fingerprint(&second));
}

#[test]
fn rejecting_spec_reports_cleanly_without_a_diff() {
    // A spec that fails to lower must be reported as a rejection, not crash, and yield no diff.
    let temp = tempfile::tempdir().unwrap();
    let old_path = temp.path().join("old.yaml");
    let new_path = temp.path().join("new.yaml");
    std::fs::write(&old_path, base()).unwrap();
    std::fs::write(&new_path, "not: a valid openapi document\n").unwrap();
    let old = Spec::new(Utf8PathBuf::from_path_buf(old_path).unwrap());
    let new = Spec::new(Utf8PathBuf::from_path_buf(new_path).unwrap());
    let outcome = spargen::diff(&old, &new);
    let rejection = outcome.expect_err("the new spec does not lower, so there is no diff");
    assert!(rejection.old_spec().is_none());
    assert!(rejection.new_spec().is_some());
}

// --- The remaining change kinds -----------------------------------------------------------------
//
// The nine kinds above were the ones with fixtures. The rest were classified by policy alone, with
// nothing proving the classifier ever produces them; each of the tests below drives one out of a
// real pair of specs. `spargen/src/surface/mod.rs` holds the guard that keeps this list complete.

/// A spec assembled from a whole `paths:` body and a whole `components.schemas:` body, for the
/// shapes the narrower `spec` helper above cannot express (request bodies, error responses, enums).
fn full(paths: &str, schemas: &str) -> String {
    format!(
        "openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
{paths}components:
  schemas:
{schemas}"
    )
}

/// One `get /pets` operation whose 200 body is `success`, with `extra` operation lines spliced in
/// at 6-space indent (a `requestBody:`, and so on).
fn pets_get(operation_id: &str, extra: &str, success: &str) -> String {
    format!(
        "  /pets:
    get:
      operationId: {operation_id}
{extra}      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {success}
"
    )
}

const PET_REF: &str = "{ $ref: '#/components/schemas/Pet' }";

const PET_SCHEMA: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        name: { type: string }
";

#[test]
fn renaming_an_operation_id_renames_the_method_and_is_major() {
    // Same path and method, different `operationId`: the generated callable renames, so every call
    // site breaks even though the endpoint is unchanged.
    let old = full(&pets_get("listPets", "", PET_REF), PET_SCHEMA);
    let new = full(&pets_get("fetchPets", "", PET_REF), PET_SCHEMA);
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::MethodRenamed),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

const REQUEST_BODY_PET: &str = "      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Pet' }
";

const REQUEST_BODY_STRING: &str = "      requestBody:
        required: true
        content:
          application/json:
            schema: { type: string }
";

#[test]
fn adding_a_request_body_is_major() {
    // A new required `&T` argument on an existing method.
    let old = full(&pets_get("listPets", "", PET_REF), PET_SCHEMA);
    let new = full(&pets_get("listPets", REQUEST_BODY_PET, PET_REF), PET_SCHEMA);
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::RequestBodyAdded),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn removing_a_request_body_is_major() {
    let old = full(&pets_get("listPets", REQUEST_BODY_PET, PET_REF), PET_SCHEMA);
    let new = full(&pets_get("listPets", "", PET_REF), PET_SCHEMA);
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::RequestBodyRemoved),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn changing_a_request_body_type_is_major() {
    let old = full(&pets_get("listPets", REQUEST_BODY_PET, PET_REF), PET_SCHEMA);
    let new = full(
        &pets_get("listPets", REQUEST_BODY_STRING, PET_REF),
        PET_SCHEMA,
    );
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::RequestBodyTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn changing_the_success_type_is_major() {
    let old = full(&pets_get("listPets", "", PET_REF), PET_SCHEMA);
    let new = full(&pets_get("listPets", "", "{ type: string }"), PET_SCHEMA);
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::SuccessTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

/// `get /pets` with a documented `404` whose body is `error_schema`.
fn with_error(error_schema: &str) -> String {
    format!(
        "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {{ $ref: '#/components/schemas/Pet' }}
        '404':
          description: missing
          content:
            application/json:
              schema: {error_schema}
"
    )
}

#[test]
fn changing_a_documented_error_type_is_major() {
    // The typed error body is part of the operation's `Result`, so changing it breaks every
    // `match` a consumer wrote against it.
    let old = full(&with_error("{ type: string }"), PET_SCHEMA);
    let new = full(&with_error("{ type: integer }"), PET_SCHEMA);
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::ErrorTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn removing_a_parameter_is_major() {
    let report = diff(&spec(PARAM_OPTIONAL_INT, "id", PET_PROPS, ""), &base());
    assert_eq!(kinds(&report), vec![ChangeKind::ParamRemoved]);
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn flipping_a_parameter_between_required_and_optional_is_major_both_ways() {
    // Either direction changes the method signature: a required parameter is positional, an
    // optional one is a `…Params` field.
    let optional = spec(PARAM_OPTIONAL_INT, "id", PET_PROPS, "");
    let required = spec(PARAM_REQUIRED_INT, "id", PET_PROPS, "");

    for (old, new) in [(&optional, &required), (&required, &optional)] {
        let report = diff(old, new);
        assert_eq!(kinds(&report), vec![ChangeKind::ParamRequirednessChanged]);
        assert_eq!(report.bump, Impact::Major);
    }
}

const PET_AND_OWNER: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        owner: { $ref: '#/components/schemas/Owner' }
    Owner:
      type: object
      required: [name]
      properties:
        name: { type: string }
";

const PET_WITH_INLINE_OWNER: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        owner: { type: string }
";

#[test]
fn adding_a_public_type_is_minor_and_removing_one_is_major() {
    let without = full(&pets_get("listPets", "", PET_REF), PET_WITH_INLINE_OWNER);
    let with = full(&pets_get("listPets", "", PET_REF), PET_AND_OWNER);

    let added = diff(&without, &with);
    assert!(
        kinds(&added).contains(&ChangeKind::TypeAdded),
        "{:?}",
        added.changes
    );

    let removed = diff(&with, &without);
    assert!(
        kinds(&removed).contains(&ChangeKind::TypeRemoved),
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

/// Two operations, so both `Pet` and `Status` are reachable from the generated surface.
const TWO_OPS: &str = "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
  /status:
    get:
      operationId: getStatus
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Status' }
";

const PET_MINIMAL: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
";

fn with_status(status: &str) -> String {
    format!("{PET_MINIMAL}{status}")
}

const STATUS_STRUCT: &str = "    Status:
      type: object
      required: [state]
      properties:
        state: { type: string }
";

const STATUS_ENUM_TWO: &str = "    Status:
      type: string
      enum: [active, retired]
";

const STATUS_ENUM_THREE: &str = "    Status:
      type: string
      enum: [active, retired, pending]
";

#[test]
fn changing_a_types_generation_kind_is_major() {
    // The same named type going from `struct` to `enum` breaks every construction and every field
    // access, even though the name is unchanged.
    let report = diff(
        &full(TWO_OPS, &with_status(STATUS_STRUCT)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
    );
    assert!(
        kinds(&report).contains(&ChangeKind::TypeKindChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn adding_an_enum_variant_is_minor_and_removing_one_is_major() {
    // The documented additive rule: a new value a consumer may now receive is minor.
    let added = diff(
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_THREE)),
    );
    assert_eq!(kinds(&added), vec![ChangeKind::VariantAdded]);
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(
        &full(TWO_OPS, &with_status(STATUS_ENUM_THREE)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
    );
    assert_eq!(kinds(&removed), vec![ChangeKind::VariantRemoved]);
    assert_eq!(removed.bump, Impact::Major);
}

const UNION_STRING_OR_INT: &str = "    Status:
      oneOf:
        - { type: string }
        - { type: integer }
";

const UNION_STRING_OR_BOOL: &str = "    Status:
      oneOf:
        - { type: string }
        - { type: boolean }
";

#[test]
fn changing_a_union_variant_payload_type_is_major() {
    let report = diff(
        &full(TWO_OPS, &with_status(UNION_STRING_OR_INT)),
        &full(TWO_OPS, &with_status(UNION_STRING_OR_BOOL)),
    );
    let kinds = kinds(&report);
    assert!(
        kinds.contains(&ChangeKind::VariantTypeChanged)
            || (kinds.contains(&ChangeKind::VariantAdded)
                && kinds.contains(&ChangeKind::VariantRemoved)),
        "a union payload change must be reported, not silently dropped: {:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn flipping_a_field_between_required_and_optional_is_major_both_ways() {
    // `T` ↔ `Option<T>` on a public struct field.
    let required_name = spec("", "id, name", PET_PROPS, "");
    let optional_name = base();

    for (old, new) in [
        (&optional_name, &required_name),
        (&required_name, &optional_name),
    ] {
        let report = diff(old, new);
        assert!(
            kinds(&report).contains(&ChangeKind::FieldRequirednessChanged),
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

// --- The JSON wire shape ------------------------------------------------------------------------

/// `--format json` is the machine surface of `spargen diff`, and the CLI renders it straight from
/// `DiffReport`'s `Serialize` (`spargen/src/cli/run.rs`). The enum-level pinning lives in
/// `surface`'s own tests; this drives a real report through `serde_json` so the field names, the
/// nesting, and the spellings a script actually parses are all fixed at once.
#[test]
fn the_json_report_names_kinds_by_code_and_impacts_in_lowercase() {
    let report = diff(&base(), &spec("", "id", PET_PROPS, EXTRA_OP));
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).expect("a report serializes"))
            .unwrap();

    assert_eq!(json["bump"], "minor");
    let changes = json["changes"].as_array().expect("changes is an array");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0]["kind"], "operation-added");
    assert_eq!(changes[0]["impact"], "minor");
    assert_eq!(changes[0]["location"], "GET /owners");
    assert!(
        changes[0]["detail"].is_string(),
        "detail is a human string: {:?}",
        changes[0]
    );
}

/// The `major` spelling is the one `--exit-code` callers branch on, so pin it separately from the
/// additive case rather than assuming the enum renders uniformly.
#[test]
fn a_breaking_json_report_spells_the_bump_major() {
    let report = diff(&base(), &spec(PARAM_REQUIRED_INT, "id", PET_PROPS, ""));
    let rendered = serde_json::to_string(&report).expect("a report serializes");

    assert!(rendered.contains(r#""bump":"major""#), "{rendered}");
    assert!(
        rendered.contains(r#""kind":"required-param-added""#),
        "{rendered}"
    );
    // The Rust variant names are an implementation detail and must not reach the wire.
    assert!(!rendered.contains("RequiredParamAdded"), "{rendered}");
    assert!(!rendered.contains("Major"), "{rendered}");
}
