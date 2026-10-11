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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::MethodRenamed],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::RequestBodyAdded],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::RequestBodyRemoved],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::RequestBodyTypeChanged],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::SuccessTypeChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

/// The success-type detail of a report whose only change is the `listPets` success type.
fn success_type_detail(old: &str, new: &str) -> String {
    let report = diff(old, new);
    let details: Vec<&str> = report
        .changes
        .iter()
        .filter(|change| change.kind == ChangeKind::SuccessTypeChanged)
        .map(|change| change.detail.as_str())
        .collect();
    assert_eq!(details.len(), 1, "{:?}", report.changes);
    assert_eq!(report.bump, Impact::Major);
    details[0].to_owned()
}

#[test]
fn a_one_position_tuple_is_labelled_with_its_trailing_comma() {
    // Issue #449: `(i64)` is Rust for a parenthesized `i64`; the generated type is `(i64,)`.
    let pair_ref = "{ $ref: '#/components/schemas/Pair' }";
    let scalar = full(
        &pets_get("listPets", "", pair_ref),
        "    Pair: { type: integer }\n",
    );
    let single = full(
        &pets_get("listPets", "", pair_ref),
        "    Pair:
      type: array
      prefixItems: [ { type: integer } ]
      items: false
",
    );
    assert_eq!(
        success_type_detail(&scalar, &single),
        "success type `i64` -> `(i64,)`"
    );
    // Two or more positions keep the plain comma-joined label.
    let double = full(
        &pets_get("listPets", "", pair_ref),
        "    Pair:
      type: array
      prefixItems: [ { type: integer }, { type: string } ]
      items: false
",
    );
    assert_eq!(
        success_type_detail(&single, &double),
        "success type `(i64,)` -> `(i64, String)`"
    );
}

#[test]
fn a_self_referential_array_is_labelled_by_its_newtype_name() {
    // Issue #650: the surface expanded an array by recursing into its item, so an array component
    // whose items name itself overflowed the stack. Codegen emits it as the nominal newtype `Node`
    // (#648), so that is the label, and a plain array alias over it is still `Vec<Node>`.
    let schemas = "    Node:
      type: array
      items: { $ref: '#/components/schemas/Node' }
    Into:
      type: array
      items: { $ref: '#/components/schemas/Node' }
";
    let node = full(
        &pets_get("listPets", "", "{ $ref: '#/components/schemas/Node' }"),
        schemas,
    );
    let into = full(
        &pets_get("listPets", "", "{ $ref: '#/components/schemas/Into' }"),
        schemas,
    );
    let same = diff(&node, &node);
    assert!(same.changes.is_empty(), "{:?}", same.changes);
    assert_eq!(same.bump, Impact::Patch);
    assert_eq!(
        success_type_detail(&node, &into),
        "success type `Node` -> `Vec<Node>`"
    );
}

/// A `Node` array component whose items are `items`, beside an `Other` array whose items name
/// `Node` back, so that each closes an alias cycle whenever `items` reaches `Node` again.
fn cycle_schemas(items: &str) -> String {
    format!(
        "    Node:
      type: array
      items: {items}
    Other:
      type: array
      items: {{ $ref: '#/components/schemas/Node' }}
"
    )
}

#[test]
fn changing_what_a_cycle_member_newtype_wraps_is_major() {
    // Issue #654: `Node` is the newtype `pub struct Node(pub Vec<…>)` (#648), and every use site
    // renders it by name, so only its own surface entry can see the public `.0` field change type.
    // `Other` closes a cycle only once `Node` names it: before that it is the alias
    // `pub type Other = Vec<Node>`, so the same public name changes kind beside the field change.
    let node_ref = "{ $ref: '#/components/schemas/Node' }";
    let to_self = full(
        &pets_get("listPets", "", node_ref),
        &cycle_schemas("{ $ref: '#/components/schemas/Node' }"),
    );
    let to_other = full(
        &pets_get("listPets", "", node_ref),
        &cycle_schemas("{ $ref: '#/components/schemas/Other' }"),
    );
    let report = diff(&to_self, &to_other);
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::FieldTypeChanged, ChangeKind::TypeKindChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.changes[0].location, "Node.0");
    assert_eq!(
        report.changes[0].detail,
        "field type `Vec<Node>` -> `Vec<Other>`"
    );
    assert_eq!(report.changes[1].location, "Other");
    assert_eq!(report.changes[1].detail, "type kind `alias` -> `newtype`");
    assert_eq!(report.bump, Impact::Major);

    let back = diff(&to_other, &to_self);
    assert_eq!(
        kinds(&back),
        vec![ChangeKind::FieldTypeChanged, ChangeKind::TypeKindChanged],
        "{:?}",
        back.changes
    );
    assert_eq!(back.changes[1].detail, "type kind `newtype` -> `alias`");
}

#[test]
fn an_array_alias_becoming_a_cycle_member_newtype_changes_its_kind() {
    // Review of #656: `pub type Status = Vec<String>` turning into `pub struct Status(pub Vec<…>)`
    // breaks code that names `Status` and uses it as a `Vec`, beyond the success type the use site
    // already reports, so it is a Major kind change rather than a Minor `type-added` (and the
    // reverse not a `type-removed`).
    let alias = "    Status:
      type: array
      items: { type: string }
";
    let newtype = "    Status:
      type: array
      items: { $ref: '#/components/schemas/Status' }
";
    let report = diff(
        &full(TWO_OPS, &with_status(alias)),
        &full(TWO_OPS, &with_status(newtype)),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::SuccessTypeChanged, ChangeKind::TypeKindChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.changes[1].location, "Status");
    assert_eq!(report.changes[1].detail, "type kind `alias` -> `newtype`");
    assert_eq!(report.bump, Impact::Major);

    let back = diff(
        &full(TWO_OPS, &with_status(newtype)),
        &full(TWO_OPS, &with_status(alias)),
    );
    assert_eq!(
        kinds(&back),
        vec![ChangeKind::SuccessTypeChanged, ChangeKind::TypeKindChanged],
        "{:?}",
        back.changes
    );
    assert_eq!(back.changes[1].detail, "type kind `newtype` -> `alias`");
    assert_eq!(back.bump, Impact::Major);
}

const PET_WITH_NODE_OWNER: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        owner: { $ref: '#/components/schemas/Node' }
    Node:
      type: array
      items: { $ref: '#/components/schemas/Node' }
";

#[test]
fn adding_a_cycle_member_newtype_is_minor_and_removing_one_is_major() {
    // Issue #654: a newtype is a public type like a struct, so it is reported as one when it
    // appears or disappears; the field pointing at it changes with it.
    let without = full(&pets_get("listPets", "", PET_REF), PET_WITH_INLINE_OWNER);
    let with = full(&pets_get("listPets", "", PET_REF), PET_WITH_NODE_OWNER);

    let added = diff(&without, &with);
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::FieldTypeChanged, ChangeKind::TypeAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.changes[1].location, "Node");

    let removed = diff(&with, &without);
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::TypeRemoved, ChangeKind::FieldTypeChanged],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn a_struct_becoming_a_cycle_member_newtype_changes_its_kind() {
    let newtype = "    Status:
      type: array
      items: { $ref: '#/components/schemas/Status' }
";
    let report = diff(
        &full(TWO_OPS, &with_status(STATUS_STRUCT)),
        &full(TWO_OPS, &with_status(newtype)),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::TypeKindChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.changes[0].detail, "type kind `struct` -> `newtype`");
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::SuccessTypeChanged],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::SuccessTypeChanged],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::ErrorTypeChanged],
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
    assert_eq!(
        kinds(&lost),
        vec![
            ChangeKind::ApiErrorBodyRemoved,
            ChangeKind::ErrorTypeChanged
        ],
        "{:?}",
        lost.changes
    );
    assert_eq!(lost.bump, Impact::Major);

    // The reverse gains the trait: additive on its own, though the signature change beside it
    // still makes the pair breaking.
    let gained = diff(&mixed, &uniform);
    assert_eq!(
        kinds(&gained),
        vec![ChangeKind::ErrorTypeChanged, ChangeKind::ApiErrorBodyAdded],
        "{:?}",
        gained.changes
    );
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

/// A `GET /pets/{id}` operation carrying a path `id` of `path_type` beside a query `id` of
/// `query_type`: the same wire name in two locations.
fn same_named_params(path_type: &str, query_type: &str) -> String {
    full(
        &format!(
            "  /pets/{{id}}:
    get:
      operationId: getPet
      parameters:
        - name: id
          in: path
          required: true
          schema: {{ type: {path_type} }}
        - name: id
          in: query
          required: false
          schema: {{ type: {query_type} }}
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {PET_REF}
"
        ),
        PET_SCHEMA,
    )
}

#[test]
fn a_parameter_is_keyed_by_its_location_as_well_as_its_name() {
    // A path `id` and a query `id` are two parameters; keyed by name alone, one overwrote the
    // other and a breaking change to the overwritten one was reported as patch.
    let old = same_named_params("string", "string");

    for (new, changed) in [
        (same_named_params("integer", "string"), "path"),
        (same_named_params("string", "integer"), "query"),
    ] {
        let report = diff(&old, &new);
        let changes: Vec<(ChangeKind, &str)> = report
            .changes
            .iter()
            .map(|change| (change.kind, change.location.as_str()))
            .collect();
        let location = format!("GET /pets/{{id}} param `id` ({changed})");
        assert_eq!(
            changes,
            vec![(ChangeKind::ParamTypeChanged, location.as_str())],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }

    // Unchanged, the pair is two entries that both match: no change at all.
    let report = diff(&old, &same_named_params("string", "string"));
    assert!(report.changes.is_empty(), "{:?}", report.changes);
}

/// `listPets` declaring one optional parameter per `(name, in, type)` triple.
fn optional_params(params: &[(&str, &str, &str)]) -> String {
    let mut lines = String::from("      parameters:\n");
    for (name, location, ty) in params {
        lines.push_str(&format!(
            "        - name: {name}\n          in: {location}\n          required: false\n          schema: {{ type: {ty} }}\n"
        ));
    }
    spec(&lines, "id", PET_PROPS, "")
}

fn kinds_at(report: &DiffReport) -> Vec<(ChangeKind, &str)> {
    report
        .changes
        .iter()
        .map(|change| (change.kind, change.location.as_str()))
        .collect()
}

#[test]
fn moving_a_parameter_to_another_location_compares_it_as_one_parameter() {
    // The generated argument is named from the wire name, so a move alone leaves the signature as
    // it was: patch, not a removal plus an addition.
    let old = optional_params(&[("limit", "query", "integer")]);
    let report = diff(&old, &optional_params(&[("limit", "header", "integer")]));
    assert!(report.changes.is_empty(), "{:?}", report.changes);
    assert_eq!(report.bump, Impact::Patch);

    // A move that also changes the type reports the type change once, naming both locations.
    let report = diff(&old, &optional_params(&[("limit", "header", "string")]));
    assert_eq!(
        kinds_at(&report),
        vec![(
            ChangeKind::ParamTypeChanged,
            "GET /pets param `limit` (query -> header)"
        )],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);

    // A name declared in two locations on a side is ambiguous: its keys are not paired, so the
    // query `id` that became a header `id` beside an unchanged cookie `id` is a removal and an
    // addition.
    let report = diff(
        &optional_params(&[("id", "cookie", "string"), ("id", "query", "string")]),
        &optional_params(&[("id", "cookie", "string"), ("id", "header", "string")]),
    );
    assert_eq!(
        kinds_at(&report),
        vec![
            (ChangeKind::ParamRemoved, "GET /pets param `id` (query)"),
            (
                ChangeKind::OptionalParamAdded,
                "GET /pets param `id` (header)"
            ),
        ],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
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

    // The field pointing at the type changes with it, so each direction also reports that field.
    let added = diff(&without, &with);
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::FieldTypeChanged, ChangeKind::TypeAdded],
        "{:?}",
        added.changes
    );

    let removed = diff(&with, &without);
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::TypeRemoved, ChangeKind::FieldTypeChanged],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::TypeKindChanged],
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
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::VariantTypeChanged],
        "a union payload change is one changed variant, not a remove and an add: {:?}",
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
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldRequirednessChanged],
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

    assert_eq!(
        kinds(&report),
        vec![
            ChangeKind::MethodRenamed,
            ChangeKind::FieldRemoved,
            ChangeKind::FieldAdded,
        ],
        "{:?}",
        report.changes
    );
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

/// The enum `kind` lowers to under `open_narrowing`: its variant lines, in order.
fn open_kind_variants(spec_text: &str) -> Vec<String> {
    let source = generated_source(spec_text, true);
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

/// Under `open_narrowing`, the copy an intersection makes of a `$ref`'d set to open or lock it is
/// emitted only where the lowered type uses it (#401). An all-scalar `allOf`, and a `$ref` with
/// sibling keywords, re-emit their result under their own name, so a copy no later meet keeps is
/// unused: `ListedOpen` (`string` meeting `Listed`) and the locked copy `UuidNarrowed`'s `allOf`
/// makes of `Listed` were both public types nothing referred to. Turning the option on therefore
/// adds no type: it only opens the types the option-off output already has.
#[test]
fn open_narrowing_adds_no_type_the_output_does_not_use() {
    let open = |spec: Spec| spec.open_narrowing(true);
    let string = "{ type: string }";
    let x = "{ const: x }";
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let mut specs: Vec<(String, String)> = [
        vec![string, x, listed],
        vec![string, listed, x],
        vec![x, string, listed],
        vec![x, listed, string],
        vec![listed, string, x],
        vec![listed, x, string],
        vec![string, listed],
        vec![listed, string],
        vec![string, listed, listed],
    ]
    .into_iter()
    .map(|order| (order.join(" & "), kind_meeting(&order)))
    .collect();
    // The `$ref`-sibling spelling of `string ∩ Listed`, whose result is re-emitted the same way.
    specs.push((
        "$ref Listed beside type: string".to_owned(),
        kind_meeting(&[string]).replace(
            "                    allOf:\n                      - { type: string }\n",
            "                    $ref: '#/components/schemas/Listed'\n                    type: string\n",
        ),
    ));
    let mut added = Vec::new();
    for (case, spec) in &specs {
        let report = diff_configured(spec, spec, |spec| spec, open);
        // Opening is the option's only effect: one `VariantAdded` per opened set (`kind`, and each
        // member a nested `anyOf` names), and nothing else.
        let kinds = kinds(&report);
        assert!(
            !kinds.is_empty() && kinds.iter().all(|kind| *kind == ChangeKind::VariantAdded),
            "{case}: the option only opens `kind`: {report:?}"
        );
        added.extend(
            report
                .changes
                .iter()
                .filter(|change| change.kind == ChangeKind::TypeAdded)
                .map(|change| format!("{case}: {}", change.location)),
        );
    }
    assert!(added.is_empty(), "types only the option adds: {added:#?}");
}

/// The meets an all-scalar `allOf` folds its members through emit only what its re-emitted result
/// uses, with the option on or off (#401). Two sets that each list a value the other does not meet
/// in a new set, `…Intersection1`, which the `allOf` re-emits as `kind`'s own type: the new set is
/// then unused, and is not emitted. A result that refers to a meet's insert keeps it: two arrays of
/// those sets meet in an array of the new set, which stays the array's item type, while the meet's
/// own array alias, which the re-emitted array replaces, is not emitted (#428).
#[test]
fn an_all_of_emits_no_meet_its_result_does_not_use() {
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let xy = "{ enum: [x, y] }";
    let string = "{ type: string }";
    for open in [false, true] {
        for order in [
            vec![listed, xy],
            vec![xy, listed],
            vec![listed, xy, string],
            vec![string, xy, listed],
        ] {
            let source = generated_source(&kind_meeting(&order), open);
            assert!(
                !source.contains("pub enum ResponseBodykindIntersection"),
                "open: {open}, {order:?} emits a superseded meet:\n{source}"
            );
        }
        let arrays = kind_meeting(&[
            "{ type: array, items: { $ref: '#/components/schemas/Listed' } }",
            "{ type: array, items: { enum: [x, y] } }",
        ]);
        let source = generated_source(&arrays, open);
        let item = "ResponseBodykindIntersection1Item";
        assert!(
            source.contains(&format!("pub enum {item} {{"))
                && source.contains(&format!("pub type ResponseBodykind = Vec<{item}>;")),
            "open: {open}, the array's meet keeps its item set:\n{source}"
        );
        assert!(
            !source.contains("pub type ResponseBodykindIntersection1 "),
            "open: {open}, the array's superseded meet is emitted:\n{source}"
        );
    }
}

/// A `404` body that is the `allOf` of `members`, one per entry, beside the closed set `Listed`.
fn object_all_of(members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| format!("                  - {member}\n"))
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
                allOf:
{members}"
        ),
        "    Listed: { type: string, enum: [x, listed-only] }
",
    )
}

/// An object `allOf` whose members repeat a property meets it member by member, and a later
/// member's meet supersedes an earlier one's: `string` meeting `Listed` makes the open copy
/// `ListedOpen`, and `const: x` then narrows the field past it (#428). The struct the merge emits
/// refers only to the last meet, so the superseded ones are not emitted — not the open copy, and
/// not, when the repeated property is itself an object, the struct an earlier pair of members met
/// in. Unlike the re-emitted results of #401, the merged struct does refer to some of the meets'
/// inserts, interleaved with the ones it does not, so this holds for each insert on its own.
/// Turning `open_narrowing` on therefore adds no type, in any order of the members.
#[test]
fn an_object_all_of_emits_no_meet_a_later_member_superseded() {
    let object = |property: &str, ty: &str| {
        format!("{{ type: object, required: [{property}], properties: {{ {property}: {ty} }} }}")
    };
    let open = |spec: Spec| spec.open_narrowing(true);
    let types = [
        "{ type: string }",
        "{ $ref: '#/components/schemas/Listed' }",
        "{ const: x }",
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut added = Vec::new();
    for nested in [false, true] {
        for order in orders {
            let members: Vec<String> = order
                .iter()
                .map(|&index| {
                    let kind = object("kind", types[index]);
                    if nested {
                        object("inner", &kind)
                    } else {
                        kind
                    }
                })
                .collect();
            let members: Vec<&str> = members.iter().map(String::as_str).collect();
            let spec = object_all_of(&members);
            let case = format!("nested: {nested}, {order:?}");
            for open in [false, true] {
                let source = generated_source(&spec, open);
                // No order meets `kind` last against `Listed` alone, so no field keeps its copy.
                assert!(
                    !source.contains("ListedOpen"),
                    "{case}, open: {open} emits a superseded open copy:\n{source}"
                );
                // The second pair of `inner` structs meets in the one the body refers to; the
                // first pair's meet is superseded.
                assert!(
                    source
                        .matches("pub struct ResponseBodyinnerIntersection")
                        .count()
                        == usize::from(nested),
                    "{case}, open: {open} emits a superseded struct meet:\n{source}"
                );
            }
            let report = diff_configured(&spec, &spec, |spec| spec, open);
            added.extend(
                report
                    .changes
                    .iter()
                    .filter(|change| change.kind == ChangeKind::TypeAdded)
                    .map(|change| format!("{case}: {}", change.location)),
            );
        }
    }
    assert!(added.is_empty(), "types only the option adds: {added:#?}");
}

// --- Labels against the generated code ----------------------------------------------------------
//
// `canon_ty` renders the types `spargen diff` labels a change with, and codegen declares them;
// nothing else ties the two. The property below generates one specification per case, reads the
// type codegen declared for a field, and compares it with the label the diff gives that field.

/// A schema tree for one field: leaves cover every rendering `canon_ty` has a rule for that a
/// field can carry (scalars and their formats, scalar enums, nominal objects, untyped, `null`),
/// composed by arrays, closed tuples and nullability.
#[derive(Debug, Clone)]
enum Schema {
    Leaf(&'static str),
    Array(Box<Schema>),
    Tuple(Vec<Schema>),
    Nullable(Box<Schema>),
}

const NULL: &str = r#"{"type":"null"}"#;

impl Schema {
    fn json(&self) -> String {
        match self {
            Schema::Leaf(leaf) => (*leaf).to_owned(),
            Schema::Array(item) => format!(r#"{{"type":"array","items":{}}}"#, item.json()),
            Schema::Tuple(items) => {
                let items: Vec<String> = items.iter().map(Schema::json).collect();
                format!(
                    r#"{{"type":"array","prefixItems":[{}],"items":false}}"#,
                    items.join(",")
                )
            }
            Schema::Nullable(inner) => format!(r#"{{"anyOf":[{},{NULL}]}}"#, inner.json()),
        }
    }
}

fn schema() -> impl proptest::strategy::Strategy<Value = Schema> {
    use proptest::prelude::*;
    let leaf = proptest::sample::select(vec![
        r#"{"type":"string"}"#,
        r#"{"type":"integer","format":"int32"}"#,
        r#"{"type":"integer"}"#,
        r#"{"type":"number"}"#,
        r#"{"type":"boolean"}"#,
        r#"{"type":"string","format":"uuid"}"#,
        r#"{"type":"string","format":"date-time"}"#,
        r#"{"type":"string","format":"date"}"#,
        r#"{"type":"string","enum":["a","b"]}"#,
        r#"{"type":"integer","enum":[1,2]}"#,
        r#"{"type":"boolean","enum":[true]}"#,
        r#"{}"#,
        NULL,
        r##"{"$ref":"#/components/schemas/Leaf"}"##,
        r#"{"type":"object","properties":{"x":{"type":"string"}}}"#,
    ])
    .prop_map(Schema::Leaf);
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|item| Schema::Array(Box::new(item))),
            proptest::collection::vec(inner.clone(), 1..4).prop_map(Schema::Tuple),
            inner
                .prop_filter("one `null` member at a time", |schema| {
                    !matches!(schema, Schema::Nullable(_) | Schema::Leaf(NULL))
                })
                .prop_map(|schema| Schema::Nullable(Box::new(schema))),
        ]
    })
}

/// A specification whose `Pet` has one required field, `subject`, of schema `subject`.
fn subject_spec(subject: &str) -> String {
    format!(
        r##"{{"openapi":"3.1.0","info":{{"title":"T","version":"1.0.0"}},
"paths":{{"/pets":{{"get":{{"operationId":"getPet","responses":{{"200":{{"description":"ok",
"content":{{"application/json":{{"schema":{{"$ref":"#/components/schemas/Pet"}}}}}}}}}}}}}}}},
"components":{{"schemas":{{
"Pet":{{"type":"object","required":["subject"],"properties":{{"subject":{subject}}}}},
"Marker":{{"type":"object","properties":{{"m":{{"type":"string"}}}}}},
"Leaf":{{"type":"object","properties":{{"x":{{"type":"integer"}}}}}}}}}}}}"##
    )
}

/// Every `type` alias the generated module declares outside the embedded runtime, by name.
fn aliases(items: &[syn::Item], out: &mut std::collections::BTreeMap<String, syn::Type>) {
    for item in items {
        match item {
            syn::Item::Type(alias) => {
                out.insert(alias.ident.to_string(), (*alias.ty).clone());
            }
            syn::Item::Mod(module) if module.ident != "support" => {
                if let Some((_, items)) = &module.content {
                    aliases(items, out);
                }
            }
            _ => {}
        }
    }
}

/// The declared type of `struct_name`'s field `field` in the generated module.
fn field_type(items: &[syn::Item], struct_name: &str, field: &str) -> Option<syn::Type> {
    items.iter().find_map(|item| match item {
        syn::Item::Struct(item) if item.ident == struct_name => item
            .fields
            .iter()
            .find(|candidate| candidate.ident.as_ref().is_some_and(|ident| ident == field))
            .map(|field| field.ty.clone()),
        syn::Item::Mod(module) if module.ident != "support" => module
            .content
            .as_ref()
            .and_then(|(_, items)| field_type(items, struct_name, field)),
        _ => None,
    })
}

/// Render `ty` in the label's notation, expanding every alias in `aliases`. The normalisations are
/// the ones `canon_ty`'s documentation states: a path is named by its last segment
/// (`types::Leaf`, `bytes::Bytes`, `serde_json::Value`), `Box` is not rendered, and a `format`
/// scalar is labelled by its format (`Uuid`, `DateTime`, `Date`) whatever Rust type the `uuid` and
/// `time` mappings choose for it, so both sides spell those `String` here.
fn normalise(ty: &syn::Type, aliases: &std::collections::BTreeMap<String, syn::Type>) -> String {
    match ty {
        syn::Type::Tuple(tuple) => {
            let elements: Vec<String> = tuple
                .elems
                .iter()
                .map(|element| normalise(element, aliases))
                .collect();
            match elements.as_slice() {
                [only] => format!("({only},)"),
                _ => format!("({})", elements.join(", ")),
            }
        }
        syn::Type::Path(path) => {
            let segment = path.path.segments.last().expect("a path has a segment");
            let name = segment.ident.to_string();
            let arguments: Vec<String> = match &segment.arguments {
                syn::PathArguments::AngleBracketed(arguments) => arguments
                    .args
                    .iter()
                    .map(|argument| match argument {
                        syn::GenericArgument::Type(ty) => normalise(ty, aliases),
                        other => panic!(
                            "unexpected generic argument `{}`",
                            quote::ToTokens::to_token_stream(other)
                        ),
                    })
                    .collect(),
                syn::PathArguments::None => Vec::new(),
                other => panic!(
                    "unexpected path arguments `{}`",
                    quote::ToTokens::to_token_stream(other)
                ),
            };
            match (name.as_str(), arguments.as_slice()) {
                ("Box", [inner]) => inner.clone(),
                ("Uuid" | "DateTime" | "Date", []) => "String".to_owned(),
                (_, []) => match aliases.get(&name) {
                    Some(target) => normalise(target, aliases),
                    None => name,
                },
                (_, arguments) => format!("{name}<{}>", arguments.join(", ")),
            }
        }
        other => format!("<unexpected {}>", quote::ToTokens::to_token_stream(other)),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 48,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    })]

    /// The type `spargen diff` labels a field with is the type codegen declares for it, once the
    /// declared aliases are expanded and the documented normalisations applied: the same nesting,
    /// the same tuple arity (a one-position tuple is `(T,)` on both sides, #449), and the same
    /// nullability.
    #[test]
    fn a_field_type_label_is_the_type_codegen_declares(subject in schema()) {
        let old = subject_spec(r##"{"$ref":"#/components/schemas/Marker"}"##);
        let new = subject_spec(&subject.json());
        let source = generated_source(&new, false);
        let file = syn::parse_file(&source).expect("the generated module parses");
        let mut declared = std::collections::BTreeMap::new();
        aliases(&file.items, &mut declared);
        let field = field_type(&file.items, "Pet", "subject").expect("`Pet.subject` is emitted");
        let generated = normalise(&field, &declared);

        let report = unsnapshotted_diff(&old, &new, |spec| spec, |spec| spec);
        let labels: Vec<&str> = report
            .changes
            .iter()
            .filter(|change| change.kind == ChangeKind::FieldTypeChanged)
            .map(|change| change.detail.as_str())
            .collect();
        let [label] = labels[..] else {
            panic!("one field-type change expected: {:?}", report.changes);
        };
        let canonical = label
            .strip_prefix("field type `Marker` -> `")
            .and_then(|rest| rest.strip_suffix('`'))
            .unwrap_or_else(|| panic!("unexpected label {label:?}"));
        let parsed: syn::Type = syn::parse_str(canonical)
            .unwrap_or_else(|error| panic!("the label {canonical:?} is not a type: {error}"));
        let labelled = normalise(&parsed, &std::collections::BTreeMap::new());
        proptest::prop_assert_eq!(
            labelled,
            generated,
            "label {:?}, schema {}",
            canonical,
            subject.json()
        );
    }
}
