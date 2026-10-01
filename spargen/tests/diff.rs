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
    diff_configured(old_spec, new_spec, |spec| spec, |spec| spec)
}

/// [`diff`] with each side's `Spec` passed through its own `configure` first.
fn diff_configured(
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

#[test]
fn documenting_a_bodyless_success_beside_the_body_is_major() {
    // A bodyless `204` beside the `200` body turns the plain `Pet` into a response enum with a
    // `Status204` variant (issue #121), so every consumer reading the `Pet` directly breaks.
    let old = full(&pets_get("listPets", "", PET_REF), PET_SCHEMA);
    let new = full(
        &format!(
            "{}        '204':\n          description: nothing\n",
            pets_get("listPets", "", PET_REF)
        ),
        PET_SCHEMA,
    );
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::SuccessTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

/// `get /pets` whose `responses:` body is `statuses` (8-space indent), for the `default`-beside-
/// success shapes below.
fn pets_responses(statuses: &str) -> String {
    format!(
        "  /pets:
    get:
      operationId: listPets
      responses:
{statuses}"
    )
}

const R200_PET: &str = "        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
";

const R201_STRING: &str = "        '201':
          description: created
          content:
            application/json:
              schema: { type: string }
";

const R204: &str = "        '204':
          description: nothing
";

const R404: &str = "        '404':
          description: missing
";

const RDEFAULT_STRING: &str = "        default:
          description: anything else
          content:
            application/json:
              schema: { type: string }
";

#[test]
fn documenting_default_beside_a_declared_success_changes_only_the_error_type() {
    // Issue #151: while any success status is declared, `default` satisfies no undeclared 2xx, so
    // adding it touches the error side alone, in each success shape — plain, unit, and enum.
    for success in [
        R200_PET.to_owned(),
        R204.to_owned(),
        format!("{R200_PET}{R201_STRING}"),
    ] {
        let old = full(&pets_responses(&success), PET_SCHEMA);
        let new = full(
            &pets_responses(&format!("{success}{RDEFAULT_STRING}")),
            PET_SCHEMA,
        );
        let report = diff(&old, &new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::ErrorTypeChanged],
            "{success}: {:?}",
            report.changes
        );
    }
}

#[test]
fn removing_the_last_declared_success_makes_default_the_success_type_and_is_major() {
    // With no success status left, `default` is what documents a 2xx (issue #115), so the success
    // type moves from `Pet` to `default`'s body: the one edit that brings `default` onto the
    // success side.
    let old = full(
        &pets_responses(&format!("{R200_PET}{R404}{RDEFAULT_STRING}")),
        PET_SCHEMA,
    );
    let new = full(
        &pets_responses(&format!("{R404}{RDEFAULT_STRING}")),
        PET_SCHEMA,
    );
    let report = diff(&old, &new);
    assert!(
        kinds(&report).contains(&ChangeKind::SuccessTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
    let detail = report
        .changes
        .iter()
        .find(|change| change.kind == ChangeKind::SuccessTypeChanged)
        .map(|change| change.detail.clone())
        .unwrap();
    assert!(detail.contains("String"), "{detail}");
}

#[test]
fn renumbering_a_status_in_a_response_enum_is_major_on_either_side() {
    // Issue #211: every body type below is unchanged, so only a status label tells the two
    // signatures apart — a `Status201` variant renamed to `Status202`, or `Status409` to
    // `Status410`, breaks every consumer matching on it. Each pair is an enum on both sides of the
    // edit, so it is the enum arm's labels this pins, not a change of shape.
    let r202_string = R201_STRING.replace("'201'", "'202'");
    let r409_string = R201_STRING.replace("'201'", "'409'");
    let r410_string = R201_STRING.replace("'201'", "'410'");
    for (old, new, kind) in [
        (
            format!("{R200_PET}{R201_STRING}"),
            format!("{R200_PET}{r202_string}"),
            ChangeKind::SuccessTypeChanged,
        ),
        (
            format!("{R200_PET}{R404}{r409_string}"),
            format!("{R200_PET}{R404}{r410_string}"),
            ChangeKind::ErrorTypeChanged,
        ),
    ] {
        let report = diff(
            &full(&pets_responses(&old), PET_SCHEMA),
            &full(&pets_responses(&new), PET_SCHEMA),
        );
        assert_eq!(kinds(&report), vec![kind], "{new}: {:?}", report.changes);
        assert_eq!(report.bump, Impact::Major);
    }
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
fn documenting_a_bodyless_error_status_beside_the_one_error_body_is_major() {
    // A bodyless `403` beside the one bodied `404` turns the newtype over the `404` body into an
    // error enum with a `Status403` variant (issue #204), so every consumer destructuring the
    // newtype breaks.
    let old = full(&with_error("{ type: string }"), PET_SCHEMA);
    let new = full(
        &format!(
            "{}        '403':\n          description: forbidden\n",
            with_error("{ type: string }")
        ),
        PET_SCHEMA,
    );
    let report = diff(&old, &new);
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::ErrorTypeChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

/// `get /pets` with documented `404` and `409` bodies, plus `extra` status entries spliced in at
/// 8-space indent (a bodyless `'410'`, and so on).
fn with_errors(e404: &str, e409: &str, extra: &str) -> String {
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
              schema: {e404}
        '409':
          description: conflict
          content:
            application/json:
              schema: {e409}
{extra}"
    )
}

const MESSAGE_REF: &str = "{ $ref: '#/components/schemas/Message' }";

const MESSAGE_SCHEMA: &str = "    Message:
      type: string
";

#[test]
fn an_alias_equal_error_body_keeps_api_error_body_and_is_patch() {
    // Both statuses `$ref` a string component, then `409` becomes an inline string. The two
    // schemas have different ids, but both bodies are still `String`: the error type keeps
    // `body()` and `ApiErrorBody`, so nothing a consumer wrote breaks.
    let schemas = format!("{PET_SCHEMA}{MESSAGE_SCHEMA}");
    let old = full(&with_errors(MESSAGE_REF, MESSAGE_REF, ""), &schemas);
    let new = full(&with_errors(MESSAGE_REF, "{ type: string }", ""), &schemas);
    let report = diff(&old, &new);
    assert!(report.changes.is_empty(), "changes: {:?}", report.changes);
    assert_eq!(report.bump, Impact::Patch);
}

#[test]
fn losing_api_error_body_is_major_and_gaining_it_is_minor() {
    let uniform = full(
        &with_errors("{ type: string }", "{ type: string }", ""),
        PET_SCHEMA,
    );
    let mixed = full(
        &with_errors("{ type: string }", "{ type: integer }", ""),
        PET_SCHEMA,
    );

    // `body()`, `Error::api_body()`, and every `E: ApiErrorBody` bound stop compiling.
    let lost = diff(&uniform, &mixed);
    let lost_kinds = kinds(&lost);
    assert!(
        lost_kinds.contains(&ChangeKind::ErrorTypeChanged),
        "{lost_kinds:?}"
    );
    assert!(
        lost_kinds.contains(&ChangeKind::ApiErrorBodyRemoved),
        "{lost_kinds:?}"
    );
    assert_eq!(lost.bump, Impact::Major);

    // The reverse gains the trait: additive on its own, though the signature change beside it
    // still makes the pair breaking.
    let gained = diff(&mixed, &uniform);
    let added = gained
        .changes
        .iter()
        .find(|change| change.kind == ChangeKind::ApiErrorBodyAdded)
        .unwrap_or_else(|| panic!("gaining ApiErrorBody is reported: {:?}", gained.changes));
    assert_eq!(added.impact, Impact::Minor);
    assert_eq!(gained.bump, Impact::Major);
}

#[test]
fn api_error_body_loss_is_reported_where_the_error_signature_cannot_see_it() {
    // A bodyless `410` and a `410` whose JSON body is exactly `null` both render as `410:()` in
    // the error signature, so no `ErrorTypeChanged` fires. The `null` body is a third body type
    // beside two `String`s, though, so the error type stops implementing `ApiErrorBody`.
    let bodyless = "        '410':
          description: gone
";
    let null_body = "        '410':
          description: gone
          content:
            application/json:
              schema: { const: null }
";
    let old = full(
        &with_errors("{ type: string }", "{ type: string }", bodyless),
        PET_SCHEMA,
    );
    let new = full(
        &with_errors("{ type: string }", "{ type: string }", null_body),
        PET_SCHEMA,
    );
    let report = diff(&old, &new);
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::ApiErrorBodyRemoved],
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

// --- Keyword-escaped identifiers ----------------------------------------------------------------

/// A `Pet` whose properties include two Rust keywords, so the generated struct carries the raw
/// identifiers `r#type` and `r#gen` — `gen` being reserved only since edition 2024, which is why
/// spargen escapes against the union of every edition's reserved words.
const KEYWORD_PET_SCHEMA: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        type: { type: string }
        gen: { type: string }
";

/// The surface records identifiers exactly as `name` escapes them, `r#` prefix and all, so a
/// keyword-named operation or field is stored as `r#gen` rather than `gen`. That cannot desync the
/// two sides of a diff: `spargen::diff` lowers both specs in one process through one
/// `name::allocate` and one keyword table, and no `Surface` is ever serialized or persisted, so
/// there is no path by which one side is escaped and the other is not. Pinned here because the
/// asymmetry is the plausible-looking bug that this arrangement rules out.
#[test]
fn a_keyword_named_operation_and_field_are_not_a_false_rename() {
    let spec = full(&pets_get("gen", "", PET_REF), KEYWORD_PET_SCHEMA);
    let report = diff(&spec, &spec);

    assert!(
        report.changes.is_empty(),
        "a spec is identical to itself however its identifiers escape: {:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Patch);
}

/// The converse, so the test above proves symmetry rather than an inert detector: a real rename of
/// the same keyword-named operation and field is still classified, and still breaking.
#[test]
fn renaming_a_keyword_named_operation_and_field_is_still_major() {
    let renamed_field = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        kind: { type: string }
        gen: { type: string }
";
    let old = full(&pets_get("gen", "", PET_REF), KEYWORD_PET_SCHEMA);
    let new = full(&pets_get("generate", "", PET_REF), renamed_field);
    let report = diff(&old, &new);

    let kinds = kinds(&report);
    assert!(kinds.contains(&ChangeKind::MethodRenamed), "{kinds:?}");
    assert!(kinds.contains(&ChangeKind::FieldRemoved), "{kinds:?}");
    assert!(kinds.contains(&ChangeKind::FieldAdded), "{kinds:?}");
    assert_eq!(report.bump, Impact::Major);

    // Not vacuous: the field changes are located by the escaped identifier, which is what makes
    // this pair a test of keyword-escaped names rather than of ordinary ones.
    let locations: Vec<&str> = report
        .changes
        .iter()
        .map(|change| change.location.as_str())
        .collect();
    assert!(locations.contains(&"Pet.r#type"), "{locations:?}");

    // The rename is reported in the spelling a consumer writes at the call site.
    let renamed = report
        .changes
        .iter()
        .find(|change| change.kind == ChangeKind::MethodRenamed)
        .expect("the method rename is reported");
    assert!(
        renamed.detail.contains("r#gen") && renamed.detail.contains("generate"),
        "{}",
        renamed.detail
    );
}

/// `get /pets` whose documented `404` narrows the `Problem` component's `type` to `values`.
fn narrowed_problem(values: &str) -> String {
    full(
        &format!(
            "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
        '404':
          description: missing
          content:
            application/problem+json:
              schema:
                allOf:
                  - $ref: '#/components/schemas/Problem'
                  - properties: {{ type: {{ enum: [{values}] }} }}
"
        ),
        "    Problem:
      type: object
      required: [type]
      properties:
        type: { type: string }
",
    )
}

/// The open set's catch-all is surface: an open enum carries it as a variant, and a listed value
/// that takes its name moves it, which is a rename a consumer's `match` sees.
#[test]
fn an_open_enums_catch_all_is_a_variant_of_its_surface() {
    let open = |spec: Spec| spec.open_narrowing(true);

    // A new listed value is the additive rule's variant, as on a closed enum.
    let added = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, c"),
        open,
        open,
    );
    assert_eq!(kinds(&added), vec![ChangeKind::VariantAdded], "{added:?}");
    assert_eq!(added.bump, Impact::Minor);

    // A new listed value spelled `other` takes the catch-all's name, so the catch-all is renamed.
    let renamed = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, other"),
        open,
        open,
    );
    let details: Vec<&str> = renamed
        .changes
        .iter()
        .map(|change| change.detail.as_str())
        .collect();
    assert!(
        details.contains(&"variant removed: `Other(String)`, which held any unlisted value"),
        "{details:?}"
    );
    assert!(details.contains(&"variant added: `other`"), "{details:?}");
    assert_eq!(renamed.bump, Impact::Major);

    // Turning the option on opens the same type in place: it gains the catch-all, which is the
    // additive rule again, and turning it off removes it.
    let opened = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        |spec| spec,
        open,
    );
    assert_eq!(kinds(&opened), vec![ChangeKind::VariantAdded], "{opened:?}");
    assert_eq!(
        opened.changes[0].detail,
        "variant added: `Other(String)`, holding any unlisted value"
    );
    assert_eq!(opened.bump, Impact::Minor);
    let closed = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        open,
        |spec| spec,
    );
    assert_eq!(
        kinds(&closed),
        vec![ChangeKind::VariantRemoved],
        "{closed:?}"
    );
    assert_eq!(closed.bump, Impact::Major);
}

/// A `404` body whose required `kind` property is the `allOf` of `members`, one member per entry,
/// beside a `Listed` component that is the closed set `[x, listed-only]`, and three components that
/// are that set narrowed against a `uuid` string outside any response body: `UuidListed` declares
/// it as a `uuid` string, `UntypedUuidListed` names the format without `type: string`, and
/// `UuidNarrowed` is the `allOf` of `Listed` and a `uuid` string.
fn kind_meeting(members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| format!("                      - {member}\n"))
        .collect();
    full(
        &format!(
            "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
        '404':
          description: missing
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
{members}"
        ),
        "    Listed:
      type: string
      enum: [x, listed-only]
    UuidListed:
      type: string
      format: uuid
      enum: [x, listed-only]
    UntypedUuidListed:
      format: uuid
      enum: [x, listed-only]
    UuidNarrowed:
      allOf:
        - $ref: '#/components/schemas/Listed'
        - type: string
          format: uuid
",
    )
}

/// The enum `kind` lowers to under `open_narrowing`: its variant lines, in order.
fn open_kind_variants(spec_text: &str) -> Vec<String> {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("spec.yaml");
    std::fs::write(&spec_path, spec_text).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap();
    let report = spargen::generate(
        &Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap())
            .open_narrowing(true)
            .build(out.clone())
            .cargo(spargen::CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
    let source = std::fs::read_to_string(&out).unwrap();
    // The embedded runtime's `AuthScheme` has a `kind` field of its own.
    let ty = source
        .split("pub kind: ")
        .skip(1)
        .map(|field| field[..field.find(',').unwrap()].trim())
        .find(|ty| *ty != "AuthKind")
        .unwrap_or_else(|| panic!("no `kind` field:\n{source}"));
    let body = source
        .split(&format!("pub enum {ty} {{"))
        .nth(1)
        .unwrap_or_else(|| panic!("`kind` is `{ty}`, which is not an enum:\n{source}"));
    // No variant carries braces, so the first one closes the enum.
    body[..body.find('}').unwrap()]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
        .map(str::to_owned)
        .collect()
}

/// The `(kind, location)` of every major change in a report.
fn major_changes(report: &DiffReport) -> Vec<(ChangeKind, String)> {
    report
        .changes
        .iter()
        .filter(|change| change.impact == Impact::Major)
        .map(|change| (change.kind, change.location.clone()))
        .collect()
}

/// Under `open_narrowing`, an `allOf`'s members meet in the same set whatever order they are
/// written in: the values every member lists, open when a member meeting a plain `string` opened
/// it. An open set meeting a closed one keeps only their shared values (the description forbids
/// the rest), and two open sets meet in the values both list. Reordering the members is then no
/// more breaking with the option on than with it off: an inline member's type is named for its
/// position either way, so moving the member the field's type comes from renames that type.
#[test]
fn open_narrowing_meets_the_same_set_in_every_member_order() {
    let open = |spec: Spec| spec.open_narrowing(true);
    let string = "{ type: string }";
    let x = "{ const: x }";
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let expected = ["X,", "Other(String),"];

    // An open set meeting a closed one, in all six orders: `string ∩ {x}` opens `{x}` before it
    // meets the closed `Listed`, `string ∩ Listed` opens a copy of `Listed` before it meets the
    // closed `{x}`, or the two closed sets meet before either meets `string`.
    let orders = [
        [string, x, listed],
        [string, listed, x],
        [x, string, listed],
        [x, listed, string],
        [listed, string, x],
        [listed, x, string],
    ];
    let first = kind_meeting(&orders[0]);
    for order in &orders {
        let spec = kind_meeting(order);
        assert_eq!(open_kind_variants(&spec), expected, "order {order:?}");
        let off = diff_configured(&first, &spec, |spec| spec, |spec| spec);
        let on = diff_configured(&first, &spec, open, open);
        assert_eq!(
            major_changes(&on),
            major_changes(&off),
            "reordering to {order:?} breaks more with the option on: {:?}",
            on.changes
        );
    }

    // Two open sets meeting, in both orders: each nested `allOf` narrows a plain `string`.
    let open_x = "{ allOf: [{ type: string }, { const: x }] }";
    let open_xy = "{ allOf: [{ type: string }, { enum: [x, y] }] }";
    for order in [[open_x, open_xy], [open_xy, open_x]] {
        assert_eq!(
            open_kind_variants(&kind_meeting(&order)),
            expected,
            "order {order:?}"
        );
    }
    let (forward, backward) = (
        kind_meeting(&[open_x, open_xy]),
        kind_meeting(&[open_xy, open_x]),
    );
    assert_eq!(
        major_changes(&diff_configured(&forward, &backward, open, open)),
        major_changes(&diff_configured(
            &forward,
            &backward,
            |spec| spec,
            |spec| spec
        )),
    );
}

/// Under `open_narrowing`, a set narrowed against a `uuid` or date string stays closed whatever
/// order the members are written in: only a plain `string` admits the unlisted string an open set
/// holds, and an `allOf` with a formatted member admits only what that format does. Each meet
/// keeps its operands' order-independence (#400): the format is remembered by the set it narrowed,
/// so a later or earlier plain `string` cannot open it, and an already-open set it meets closes.
#[test]
fn open_narrowing_never_opens_a_set_narrowed_against_a_formatted_string() {
    let string = "{ type: string }";
    let x = "{ const: x }";
    let open_x = "{ allOf: [{ type: string }, { const: x }] }";
    let mut opened = Vec::new();
    for format in ["uuid", "date", "date-time"] {
        let formatted = format!("{{ type: string, format: {format} }}");
        let formatted = formatted.as_str();
        let formatted_x = format!("{{ type: string, format: {format}, const: x }}");
        let formatted_x = formatted_x.as_str();
        // The same own-schema set without `type: string`: `format` alone names the format.
        let untyped_x = format!("{{ format: {format}, const: x }}");
        let untyped_x = untyped_x.as_str();
        let mut orders: Vec<Vec<&str>> = vec![
            vec![string, x, formatted],
            vec![string, formatted, x],
            vec![x, string, formatted],
            vec![x, formatted, string],
            vec![formatted, string, x],
            vec![formatted, x, string],
        ];
        // An open set (a nested narrowing of a plain `string`) meeting the formatted string, and a
        // set narrowed against the format in one schema (with or without `type: string`) meeting a
        // plain `string` or an open set, in both orders.
        for [left, right] in [
            [open_x, formatted],
            [formatted_x, string],
            [formatted_x, open_x],
            [untyped_x, string],
            [untyped_x, open_x],
        ] {
            orders.push(vec![left, right]);
            orders.push(vec![right, left]);
        }
        let mut cases: Vec<(Vec<&str>, &[&str])> = orders
            .into_iter()
            .map(|order| (order, &["X,"][..]))
            .collect();
        // A `$ref` target, which is lowered closed and then meets the format and a plain `string`
        // in all six orders (a locked copy of the target where the format meets it first), and a
        // component locked outside the response body meeting a plain `string`, in both orders:
        // one locked by its own schema's format (spelled with and without `type: string`), and
        // one whose `allOf` narrows `Listed` against a `uuid` string, which is lowered where
        // narrowing does not open a set.
        let listed_values: &[&str] = &["X,", "ListedOnly,"];
        let listed = "{ $ref: '#/components/schemas/Listed' }";
        for order in [
            [string, listed, formatted],
            [string, formatted, listed],
            [listed, string, formatted],
            [listed, formatted, string],
            [formatted, string, listed],
            [formatted, listed, string],
        ] {
            cases.push((order.to_vec(), listed_values));
        }
        if format == "uuid" {
            for target in [
                "{ $ref: '#/components/schemas/UuidListed' }",
                "{ $ref: '#/components/schemas/UntypedUuidListed' }",
                "{ $ref: '#/components/schemas/UuidNarrowed' }",
            ] {
                cases.push((vec![target, string], listed_values));
                cases.push((vec![string, target], listed_values));
            }
        }
        for (order, expected) in cases {
            let variants = open_kind_variants(&kind_meeting(&order));
            if variants != expected {
                opened.push((order.join(" & "), variants));
            }
        }
    }
    assert!(opened.is_empty(), "opened against a format: {opened:#?}");
}
