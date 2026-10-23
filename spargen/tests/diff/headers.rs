//! Typed response-header structs and their fields.

use super::*;

/// One `get /pets` operation, named `operation_id`, whose `200` response documents `headers` (a
/// whole `headers:` block at 10-space indent, or `""` for none).
fn with_headers(operation_id: &str, headers: &str) -> String {
    full(
        &format!(
            "  /pets:
    get:
      operationId: {operation_id}
      responses:
        '200':
          description: ok
{headers}          content:
            application/json:
              schema: {PET_REF}
"
        ),
        PET_SCHEMA,
    )
}

const RATE_LIMIT_INT: &str = "          headers:
            X-Rate-Limit:
              required: true
              schema: { type: integer }
";

const RATE_LIMIT_STRING: &str = "          headers:
            X-Rate-Limit:
              required: true
              schema: { type: string }
";

const RATE_LIMIT_OPTIONAL: &str = "          headers:
            X-Rate-Limit:
              schema: { type: integer }
";

const RATE_LIMIT_AND_OPTIONAL_REQUEST_ID: &str = "          headers:
            X-Rate-Limit:
              required: true
              schema: { type: integer }
            X-Request-Id:
              schema: { type: string }
";

const RATE_LIMIT_AND_REQUIRED_REQUEST_ID: &str = "          headers:
            X-Rate-Limit:
              required: true
              schema: { type: integer }
            X-Request-Id:
              required: true
              schema: { type: string }
";

#[test]
fn changing_a_response_headers_type_is_major() {
    // The header accessor's field type changes, so every read of it breaks (#726).
    let report = diff(
        &with_headers("listPets", RATE_LIMIT_INT),
        &with_headers("listPets", RATE_LIMIT_STRING),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::ResponseHeaderTypeChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn adding_a_response_header_struct_is_minor_and_removing_one_is_major() {
    let added = diff(
        &with_headers("listPets", ""),
        &with_headers("listPets", RATE_LIMIT_INT),
    );
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::ResponseHeadersAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(
        &with_headers("listPets", RATE_LIMIT_INT),
        &with_headers("listPets", ""),
    );
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::ResponseHeadersRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn adding_an_optional_response_header_is_minor_and_a_required_one_major() {
    let optional = diff(
        &with_headers("listPets", RATE_LIMIT_INT),
        &with_headers("listPets", RATE_LIMIT_AND_OPTIONAL_REQUEST_ID),
    );
    assert_eq!(
        kinds(&optional),
        vec![ChangeKind::ResponseHeaderAdded],
        "{:?}",
        optional.changes
    );
    assert_eq!(optional.bump, Impact::Minor);

    // `from_headers` now fails on a response without it.
    let required = diff(
        &with_headers("listPets", RATE_LIMIT_INT),
        &with_headers("listPets", RATE_LIMIT_AND_REQUIRED_REQUEST_ID),
    );
    assert_eq!(
        kinds(&required),
        vec![ChangeKind::RequiredResponseHeaderAdded],
        "{:?}",
        required.changes
    );
    assert_eq!(required.bump, Impact::Major);

    let removed = diff(
        &with_headers("listPets", RATE_LIMIT_AND_OPTIONAL_REQUEST_ID),
        &with_headers("listPets", RATE_LIMIT_INT),
    );
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::ResponseHeaderRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn flipping_a_response_header_between_required_and_optional_is_major_both_ways() {
    // `T` ↔ `Option<T>` on the header struct's field.
    for (old, new) in [
        (RATE_LIMIT_INT, RATE_LIMIT_OPTIONAL),
        (RATE_LIMIT_OPTIONAL, RATE_LIMIT_INT),
    ] {
        let report = diff(
            &with_headers("listPets", old),
            &with_headers("listPets", new),
        );
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::ResponseHeaderRequirednessChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn renaming_an_operation_id_renames_its_response_header_struct() {
    // The struct is named after the operation, so code naming it breaks beside the method's callers.
    let report = diff(
        &with_headers("listPets", RATE_LIMIT_INT),
        &with_headers("fetchPets", RATE_LIMIT_INT),
    );
    assert_eq!(
        kinds(&report),
        vec![
            ChangeKind::MethodRenamed,
            ChangeKind::ResponseHeadersRenamed
        ],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}
