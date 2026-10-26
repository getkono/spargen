//! Operations: the client method's name, its request body, its success and error types, and its
//! parameters.

use super::*;

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
fn flipping_a_request_body_between_required_and_optional_is_major_both_ways() {
    // A required body is a `&T` argument and an optional one `Option<&T>`, so either direction
    // changes the method signature and breaks every caller.
    let optional_body = REQUEST_BODY_PET.replace("required: true", "required: false");
    let required = full(&pets_get("listPets", REQUEST_BODY_PET, PET_REF), PET_SCHEMA);
    let optional = full(&pets_get("listPets", &optional_body, PET_REF), PET_SCHEMA);

    for (old, new) in [(&required, &optional), (&optional, &required)] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::RequestBodyRequirednessChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn a_request_body_changing_type_and_requiredness_reports_both() {
    let optional_string = REQUEST_BODY_STRING.replace("required: true", "required: false");
    let old = full(&pets_get("listPets", REQUEST_BODY_PET, PET_REF), PET_SCHEMA);
    let new = full(&pets_get("listPets", &optional_string, PET_REF), PET_SCHEMA);
    let report = diff(&old, &new);
    assert_eq!(
        kinds(&report),
        vec![
            ChangeKind::RequestBodyRequirednessChanged,
            ChangeKind::RequestBodyTypeChanged
        ],
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

/// The `Pet` spec with `nope` (4-space indent) appended to its schemas, for the uninhabited-
/// component fixtures below: `Nope` is referenced by nothing, so it is the only change.
fn with_nope(nope: &str) -> String {
    full(
        &pets_get("listPets", "", PET_REF),
        &format!("{PET_SCHEMA}{nope}"),
    )
}

const NOPE_NEVER: &str = "    Nope: false\n";

const NOPE_STRUCT: &str = "    Nope:
      type: object
      properties:
        y: { type: string }
";

#[test]
fn adding_an_uninhabited_component_is_minor_and_removing_one_is_major() {
    // Issue #661: a `false` component is emitted as the empty `pub enum Nope {}`, a public type a
    // consumer can name, so it appears and disappears like any other.
    let added = diff(&with_nope(""), &with_nope(NOPE_NEVER));
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::TypeAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.changes[0].location, "Nope");
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(&with_nope(NOPE_NEVER), &with_nope(""));
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::TypeRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.changes[0].location, "Nope");
    assert_eq!(removed.bump, Impact::Major);

    let unchanged = diff(&with_nope(NOPE_NEVER), &with_nope(NOPE_NEVER));
    assert!(unchanged.changes.is_empty(), "{:?}", unchanged.changes);
    assert_eq!(unchanged.bump, Impact::Patch);
}

#[test]
fn a_struct_becoming_uninhabited_changes_its_kind() {
    // Issue #661: `Nope` exists on both sides, so the change is a kind change, not a removal of
    // the struct beside an addition of nothing (or the reverse).
    let report = diff(&with_nope(NOPE_STRUCT), &with_nope(NOPE_NEVER));
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::TypeKindChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.changes[0].location, "Nope");
    assert_eq!(
        report.changes[0].detail,
        "type kind `struct` -> `empty enum`"
    );
    assert_eq!(report.bump, Impact::Major);

    let back = diff(&with_nope(NOPE_NEVER), &with_nope(NOPE_STRUCT));
    assert_eq!(
        kinds(&back),
        vec![ChangeKind::TypeKindChanged],
        "{:?}",
        back.changes
    );
    assert_eq!(back.changes[0].detail, "type kind `empty enum` -> `struct`");
    assert_eq!(back.bump, Impact::Major);
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

/// `listPets` with one optional nullable integer query parameter `limit`, declared by `declaration`
/// (its `schema:` or `content:` lines at 10-space indent).
fn nullable_limit(declaration: &str) -> String {
    spec(
        &format!(
            "      parameters:
        - name: limit
          in: query
          required: false
{declaration}"
        ),
        "id",
        PET_PROPS,
        "",
    )
}

#[test]
fn moving_a_nullable_parameter_onto_or_off_json_content_is_major_both_ways() {
    // A nullable JSON `content` parameter sends `Some(None)` as `null`, so its field is
    // `Option<Option<T>>`, where the same schema serialized by a style is `Option<T>` (#996).
    let styled = nullable_limit("          schema: { type: [integer, 'null'] }\n");
    let json = nullable_limit(
        "          content:
            application/json: { schema: { type: [integer, 'null'] } }
",
    );
    let text = nullable_limit(
        "          content:
            text/plain: { schema: { type: [integer, 'null'] } }
",
    );
    for (old, new) in [(&styled, &json), (&json, &styled), (&text, &json)] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::ParamTypeChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
    // A `text/*` parameter has no token for `null` either, so it keeps the styled field type.
    let report = diff(&styled, &text);
    assert!(report.changes.is_empty(), "{:?}", report.changes);
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
