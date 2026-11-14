//! Generated identifiers bound to wire names: a `…Params` field, a struct field or a string enum
//! variant that keeps its spelling but now sends or reads another wire name compiles at every call
//! site and silently changes what it means, so it is major (#1422).

use super::*;

/// The codes of a report's changes, in report order.
fn codes(report: &DiffReport) -> Vec<&'static str> {
    report
        .changes
        .iter()
        .map(|change| change.kind.code())
        .collect()
}

/// `get /pets` taking the optional query parameters `names`, in that order.
fn with_query(names: &[&str]) -> String {
    let params: String = names
        .iter()
        .map(|name| {
            format!(
                "        - name: {name}
          in: query
          schema: {{ type: integer }}
"
            )
        })
        .collect();
    full(
        &pets_get("listPets", &format!("      parameters:\n{params}"), PET_REF),
        PET_SCHEMA,
    )
}

/// A `Pet` whose optional string properties are `names`, in that order.
fn pet_with(names: &[&str]) -> String {
    let props: String = names
        .iter()
        .map(|name| format!("        {name}: {{ type: string }}\n"))
        .collect();
    full(
        &pets_get("listPets", "", PET_REF),
        &format!(
            "    Pet:
      type: object
      properties:
{props}"
        ),
    )
}

/// A `Kind` string enum listing `values`, in that order, reachable from `get /kind`.
fn kind_with(values: &str) -> String {
    full(
        &pets_get("getKind", "", "{ $ref: '#/components/schemas/Kind' }"),
        &format!(
            "    Kind:
      type: string
      enum: [{values}]
"
        ),
    )
}

#[test]
fn a_params_field_rebound_to_another_wire_name_is_major() {
    // `page-size` sorts before `pageSize`, so it takes `page_size` (and `with_page_size`), and a
    // caller's `.with_page_size(10)` would now send `page-size`.
    let report = diff(
        &with_query(&["pageSize"]),
        &with_query(&["pageSize", "page-size"]),
    );
    assert!(
        codes(&report).contains(&"identifier-rebound"),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn a_struct_field_rebound_to_another_wire_name_is_major() {
    // `foo_bar` is spelled as its identifier, so it takes `foo_bar` from `fooBar`.
    let report = diff(&pet_with(&["fooBar"]), &pet_with(&["fooBar", "foo_bar"]));
    assert!(
        codes(&report).contains(&"identifier-rebound"),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn a_property_inserted_ahead_of_one_spelled_as_its_identifier_does_not_take_it() {
    // Declaration order no longer decides who keeps a contested spelling: `foo_bar` keeps
    // `foo_bar`, and the new `fooBar` is a disambiguated optional field, which is additive.
    let report = diff(&pet_with(&["foo_bar"]), &pet_with(&["fooBar", "foo_bar"]));
    assert_eq!(codes(&report), vec!["field-added"], "{:?}", report.changes);
    assert_eq!(report.bump, Impact::Minor);
}

#[test]
fn reordering_colliding_properties_renames_nothing() {
    let report = diff(
        &pet_with(&["fooBar", "foo-bar"]),
        &pet_with(&["foo-bar", "fooBar"]),
    );
    assert!(report.changes.is_empty(), "{:?}", report.changes);
}

#[test]
fn an_enum_variant_rebound_to_another_wire_value_is_major() {
    // `A_B` sorts before `a-b`, so it takes `AB`, and `Kind::AB` would now send `A_B`.
    let report = diff(&kind_with("a-b"), &kind_with("A_B, a-b"));
    assert!(
        codes(&report).contains(&"identifier-rebound"),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn a_value_inserted_ahead_of_one_spelled_as_its_variant_does_not_take_it() {
    let report = diff(&kind_with("AB"), &kind_with("a-b, AB"));
    assert_eq!(
        codes(&report),
        vec!["variant-added"],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Minor);
}
