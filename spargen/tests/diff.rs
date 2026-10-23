//! `spargen diff` semver-impact classification. Each test crafts an old/new pair of inline specs
//! and asserts the classified change and the overall recommended bump. The surface model and its
//! classification policy live in `spargen/src/surface/`.
//!
//! This file holds the shared spec builders and the snapshotting `diff` helper. Every
//! `insta::assert_snapshot!` sits in this file, because insta names a snapshot after the module
//! path and the source file of the call: from here it is `snapshots/diff__<test>.snap`. The
//! fixtures for the remaining change kinds sit in one module per surface area under `diff/`, each
//! reaching the helpers through `use super::*`.

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
    diff_configured(old_spec, new_spec, |spec| spec, |spec| spec)
}

/// [`diff`] with each side's `Spec` passed through its own `configure` first.
fn diff_configured(
    old_spec: &str,
    new_spec: &str,
    configure_old: impl FnOnce(Spec) -> Spec,
    configure_new: impl FnOnce(Spec) -> Spec,
) -> DiffReport {
    let report = unsnapshotted_diff(old_spec, new_spec, configure_old, configure_new);
    snapshot(&report);
    report
}

/// [`diff_configured`] without the snapshot, for a property test that diffs generated pairs.
fn unsnapshotted_diff(
    old_spec: &str,
    new_spec: &str,
    configure_old: impl FnOnce(Spec) -> Spec,
    configure_new: impl FnOnce(Spec) -> Spec,
) -> DiffReport {
    let temp = tempfile::tempdir().unwrap();
    let old_path = temp.path().join("old.yaml");
    let new_path = temp.path().join("new.yaml");
    std::fs::write(&old_path, old_spec).unwrap();
    std::fs::write(&new_path, new_spec).unwrap();
    let old = configure_old(Spec::new(Utf8PathBuf::from_path_buf(old_path).unwrap()));
    let new = configure_new(Spec::new(Utf8PathBuf::from_path_buf(new_path).unwrap()));
    spargen::diff(&old, &new).expect("both specs should lower")
}

std::thread_local! {
    /// How many reports the running test has snapshotted so far. libtest runs every test on a
    /// thread of its own, so this counts per test.
    static SNAPSHOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Snapshot the whole [`fingerprint`] of a report — every change's impact, code, location and
/// `detail` label, and the bump — so a fixture pins the labels a consumer reads, not only the
/// kinds its assertions name. Every report a fixture diffs is snapshotted, in order: the first as
/// `diff__<test>.snap`, the next as `diff__<test>-2.snap`, and so on.
fn snapshot(report: &DiffReport) {
    let thread = std::thread::current();
    let test = thread
        .name()
        .expect("libtest names each test's thread after the test")
        .rsplit("::")
        .next()
        .expect("a thread name has a last segment")
        .to_owned();
    let index = SNAPSHOTS.with(|count| {
        count.set(count.get() + 1);
        count.get()
    });
    let name = if index == 1 {
        test
    } else {
        format!("{test}-{index}")
    };
    insta::assert_snapshot!(name, fingerprint(report).join("\n"));
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
    assert_eq!(kinds(&report), vec![ChangeKind::RequiredFieldAdded]);
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
    assert_eq!(
        kinds(&report),
        vec![
            ChangeKind::FieldRemoved,
            ChangeKind::OperationAdded,
            ChangeKind::OptionalParamAdded,
        ],
        "{:?}",
        report.changes
    );
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

/// A path item whose response is a dynamic reference (`E006`): it rejects the whole spec unless
/// carve omits it.
const DYNAMIC_REF_OP: &str = "  /bad:
    get:
      operationId: getBad
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $dynamicRef: '#meta' }
";

/// With `carve` set on each side, `diff` carves each spec the way `check` and generation do and
/// compares the carved surfaces; without it, the same pair is a rejection on both sides.
#[test]
fn carve_diffs_the_carved_surfaces_of_a_pair_that_lowers_only_under_carve() {
    let old = spec("", "id", PET_PROPS, DYNAMIC_REF_OP);
    let new = spec("", "id", PET_PROPS, &format!("{DYNAMIC_REF_OP}{EXTRA_OP}"));

    let uncarved = rejection(&old, &new);
    assert!(
        uncarved.old_spec().is_some() && uncarved.new_spec().is_some(),
        "{uncarved}"
    );

    let report = diff_configured(&old, &new, |spec| spec.carve(true), |spec| spec.carve(true));
    assert_eq!(kinds(&report), [ChangeKind::OperationAdded], "{report:?}");
    assert_eq!(report.bump, Impact::Minor);

    // The carved operation is on neither surface: each carved side diffs clean against the same
    // spec written without `/bad` and not carved, where a surviving `GET /bad` would be a removal
    // or an addition.
    let carve = |spec: Spec| spec.carve(true);
    let old_without_bad = base();
    let new_without_bad = spec("", "id", PET_PROPS, EXTRA_OP);
    let old_side = unsnapshotted_diff(&old, &old_without_bad, carve, |spec| spec);
    assert!(old_side.changes.is_empty(), "{old_side:?}");
    let new_side = unsnapshotted_diff(&new_without_bad, &new, |spec| spec, carve);
    assert!(new_side.changes.is_empty(), "{new_side:?}");
}

/// A spec with two operations carve can omit (`E006`) and a root-level rejection no omit rule
/// reaches (`E002`, an unsupported `jsonSchemaDialect`): the carve driver omits both operations and
/// then cannot make the rest lower.
const UNCARVABLE: &str = "openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/dialect
paths:
  /bad:
    get:
      operationId: getBad
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $dynamicRef: '#meta' }
  /worse:
    get:
      operationId: getWorse
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $dynamicRef: '#meta' }
";

/// With `carve` set, a side the carve driver cannot make lower is rejected with the carve driver's
/// own report, the one `check` returns under carve: it carries a `W009` for each construct carved
/// before the driver gave up, and `batch_cap` does not bound it.
#[test]
fn carve_reports_an_uncarvable_side_with_the_carve_drivers_report() {
    let temp = tempfile::tempdir().unwrap();
    let old_path = temp.path().join("old.yaml");
    let new_path = temp.path().join("new.yaml");
    std::fs::write(&old_path, base()).unwrap();
    std::fs::write(&new_path, UNCARVABLE).unwrap();
    let old = Spec::new(Utf8PathBuf::from_path_buf(old_path).unwrap()).carve(true);
    let new = Spec::new(Utf8PathBuf::from_path_buf(new_path).unwrap())
        .carve(true)
        .batch_cap(1);

    let rejection = spargen::diff(&old, &new).expect_err("the new side cannot be carved to lower");
    assert!(rejection.old_spec().is_none(), "{rejection}");
    let report = rejection.new_spec().expect("the new side is rejected");
    assert_eq!(report.outcome(), spargen::Outcome::Rejected, "{report:#?}");
    assert!(!report.truncated(), "{report:#?}");
    assert!(report.diagnostics().len() > 1, "{report:#?}");
    let carved = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == "W009")
        .count();
    assert_eq!(carved, 2, "one W009 per carved operation: {report:#?}");
    let residual: Vec<_> = report
        .errors()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    assert_eq!(residual, ["E002"], "{report:#?}");

    // `check` under carve returns the same report for the same spec.
    let checked = spargen::check(&new);
    assert_eq!(
        format!("{:?}", checked.diagnostics()),
        format!("{:?}", report.diagnostics())
    );
}

/// A document that never lowers, for the rejection fixtures.
const UNLOWERABLE: &str = "not: a valid openapi document\n";

/// `spargen::diff` over two inline specs that are expected not to lower together.
fn rejection(old_spec: &str, new_spec: &str) -> spargen::DiffRejection {
    let temp = tempfile::tempdir().unwrap();
    let old_path = temp.path().join("old.yaml");
    let new_path = temp.path().join("new.yaml");
    std::fs::write(&old_path, old_spec).unwrap();
    std::fs::write(&new_path, new_spec).unwrap();
    let old = Spec::new(Utf8PathBuf::from_path_buf(old_path).unwrap());
    let new = Spec::new(Utf8PathBuf::from_path_buf(new_path).unwrap());
    spargen::diff(&old, &new).expect_err("a side does not lower, so there is no diff")
}

/// The top-level keys of a rejection's JSON rendering, in the order they are written.
fn json_keys(rejection: &spargen::DiffRejection) -> Vec<String> {
    let rendered = serde_json::to_string(rejection).expect("a rejection serializes");
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a rejection serializes as an object")
        .keys()
        .cloned()
        .collect();
    // `serde_json::Map` may re-sort, so order by where each key appears in the written text.
    keys.sort_by_key(|key| rendered.find(&format!("\"{key}\":")).unwrap());
    keys
}

/// Each side that failed to lower is reported under its own label, old before new, in the
/// accessors, `rejections`, `Display` and the JSON keys alike; a side that lowered is absent from
/// all four.
#[test]
fn a_rejection_names_exactly_the_sides_that_failed() {
    let new_only = rejection(&base(), UNLOWERABLE);
    let old_only = rejection(UNLOWERABLE, &base());
    let both = rejection(UNLOWERABLE, UNLOWERABLE);

    let sides = |rejection: &spargen::DiffRejection| {
        rejection
            .rejections()
            .map(|(side, _)| side.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(sides(&new_only), ["new"]);
    assert_eq!(sides(&old_only), ["old"]);
    assert_eq!(sides(&both), ["old", "new"]);
    assert_eq!(spargen::Side::Old.as_str(), "old");
    assert_eq!(spargen::Side::New.as_str(), "new");

    let old_report = old_only.old_spec().expect("the old side failed");
    assert!(old_only.new_spec().is_none());
    assert_eq!(old_report.outcome(), spargen::Outcome::Rejected);
    assert!(old_report.errors().next().is_some(), "{old_report:#?}");
    let new_report = new_only.new_spec().expect("the new side failed");
    assert!(new_only.old_spec().is_none());
    assert_eq!(new_report.outcome(), spargen::Outcome::Rejected);
    assert!(both.old_spec().is_some() && both.new_spec().is_some());

    // `Display` frames each failing side's report with a `<side> spec rejected:` line.
    assert_eq!(
        new_only.to_string(),
        format!("new spec rejected:\n{new_report}")
    );
    assert_eq!(
        old_only.to_string(),
        format!("old spec rejected:\n{old_report}")
    );
    assert_eq!(
        both.to_string(),
        format!(
            "old spec rejected:\n{}new spec rejected:\n{}",
            both.old_spec().unwrap(),
            both.new_spec().unwrap()
        )
    );

    // The JSON rendering keys each report by the same label, and serializes it as the report.
    assert_eq!(json_keys(&new_only), ["new"]);
    assert_eq!(json_keys(&old_only), ["old"]);
    assert_eq!(json_keys(&both), ["old", "new"]);
    let value = serde_json::to_value(&old_only).unwrap();
    assert_eq!(value["old"], serde_json::to_value(old_report).unwrap());
}

/// A report carrying breaking and additive changes: `summary` counts each impact, and `Display`
/// lists every change, then the summary, then the bump. The display snapshot records today's bytes.
#[test]
fn a_mixed_report_summarizes_and_displays_every_change() {
    let only_id = "        id: { type: integer }\n";
    let report = diff(&base(), &spec(PARAM_REQUIRED_INT, "id", only_id, EXTRA_OP));
    assert_eq!(report.bump, Impact::Major);
    assert_eq!(report.summary(), "major: 2 breaking, 1 additive change(s)");

    let rendered = report.to_string();
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), report.changes.len() + 2, "{rendered}");
    for (line, change) in lines.iter().zip(&report.changes) {
        assert_eq!(*line, change.to_string());
    }
    assert_eq!(lines[report.changes.len()], report.summary());
    assert_eq!(lines[report.changes.len() + 1], "recommended bump: major");
    assert!(!rendered.ends_with('\n'), "{rendered:?}");
    insta::assert_snapshot!(
        "a_mixed_report_summarizes_and_displays_every_change-display",
        rendered
    );

    // A report with only additive changes is summarized under its own bump.
    let additive = diff(&base(), &spec("", "id", PET_PROPS, EXTRA_OP));
    assert_eq!(
        additive.summary(),
        "minor: 0 breaking, 1 additive change(s)"
    );
}

// --- The remaining change kinds -----------------------------------------------------------------
//
// The nine kinds above were the ones with fixtures. The rest were classified by policy alone, with
// nothing proving the classifier ever produces them; the modules below drive each one out of a
// real pair of specs. `spargen/src/surface/tests.rs` holds the guard that keeps this list complete.

#[path = "diff/headers.rs"]
mod headers;
#[path = "diff/json.rs"]
mod json;
#[path = "diff/keywords.rs"]
mod keywords;
#[path = "diff/labels.rs"]
mod labels;
#[path = "diff/narrowing.rs"]
mod narrowing;
#[path = "diff/operations.rs"]
mod operations;
#[path = "diff/servers.rs"]
mod servers;
#[path = "diff/types.rs"]
mod types;

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

const PET_WITH_INLINE_OWNER: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        owner: { type: string }
";

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

/// The module `spec_text` generates, with `open_narrowing` set to `open`.
fn generated_source(spec_text: &str, open: bool) -> String {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("spec.yaml");
    std::fs::write(&spec_path, spec_text).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap();
    let report = spargen::generate(
        &Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap())
            .open_narrowing(open)
            .build(out.clone())
            .cargo(spargen::CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
    std::fs::read_to_string(&out).unwrap()
}
