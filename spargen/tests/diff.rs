//! `spargen diff` semver-impact classification. Each test crafts an old/new pair of inline specs
//! and asserts the classified change and the overall recommended bump. The surface model and its
//! classification policy live in `spargen/src/surface/`.

use camino::Utf8PathBuf;
use spargen::{ChangeKind, Config, DiffReport, Impact, OutputTarget};

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
    let old = Config::new(
        Utf8PathBuf::from_path_buf(old_path).unwrap(),
        OutputTarget::Module("unused-old.rs".into()),
    );
    let new = Config::new(
        Utf8PathBuf::from_path_buf(new_path).unwrap(),
        OutputTarget::Module("unused-new.rs".into()),
    );
    let outcome = spargen::diff(&old, &new);
    assert!(
        outcome.old_rejection.is_none() && outcome.new_rejection.is_none(),
        "both specs should lower; old_rejection={:?} new_rejection={:?}",
        outcome.old_rejection,
        outcome.new_rejection
    );
    outcome.report.expect("both specs lowered => a report")
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
    let old = Config::new(
        Utf8PathBuf::from_path_buf(old_path).unwrap(),
        OutputTarget::Module("unused-old.rs".into()),
    );
    let new = Config::new(
        Utf8PathBuf::from_path_buf(new_path).unwrap(),
        OutputTarget::Module("unused-new.rs".into()),
    );
    let outcome = spargen::diff(&old, &new);
    assert!(outcome.report.is_none());
    assert!(outcome.old_rejection.is_none());
    assert!(outcome.new_rejection.is_some());
}
