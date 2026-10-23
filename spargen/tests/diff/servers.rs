//! Server builders, their variables, and the variables' enums.

use super::*;

/// The `/pets` spec with a top-level `servers:` block (`servers` at 2-space indent, or `""` for none).
fn with_servers(servers: &str) -> String {
    format!(
        "openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers:
{servers}paths:
{}components:
  schemas:
{PET_SCHEMA}",
        pets_get("listPets", "", PET_REF)
    )
}

const REGIONS_US_EU: &str = "  - url: 'https://{region}.example.com'
    variables:
      region: { default: us, enum: [us, eu] }
";

const REGIONS_US_EU_AP: &str = "  - url: 'https://{region}.example.com'
    variables:
      region: { default: us, enum: [us, eu, ap] }
";

const REGION_FREE_FORM: &str = "  - url: 'https://{region}.example.com'
    variables:
      region: { default: us }
";

const REGIONS_AND_VERSION: &str = "  - url: 'https://{region}.example.com/{version}'
    variables:
      region: { default: us, enum: [us, eu] }
      version: { default: v1 }
";

const SANDBOX: &str = "  - url: 'https://sandbox.example.com'
";

#[test]
fn adding_a_server_variable_value_is_minor_and_removing_one_is_major() {
    // A variable's `enum` is a generated `pub enum` a consumer passes to its setter (#726).
    let added = diff(
        &with_servers(REGIONS_US_EU),
        &with_servers(REGIONS_US_EU_AP),
    );
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::VariantAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(
        &with_servers(REGIONS_US_EU_AP),
        &with_servers(REGIONS_US_EU),
    );
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::VariantRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn adding_a_server_is_minor_and_removing_one_is_major() {
    let more = format!("{REGIONS_US_EU}{SANDBOX}");
    let added = diff(&with_servers(REGIONS_US_EU), &with_servers(&more));
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::ServerAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(&with_servers(&more), &with_servers(REGIONS_US_EU));
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::ServerRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn adding_a_server_variable_is_minor_and_removing_one_is_major() {
    let added = diff(
        &with_servers(REGIONS_US_EU),
        &with_servers(REGIONS_AND_VERSION),
    );
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::ServerVariableAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(
        &with_servers(REGIONS_AND_VERSION),
        &with_servers(REGIONS_US_EU),
    );
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::ServerVariableRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn a_server_variable_gaining_or_losing_its_enum_is_major_both_ways() {
    // The setter takes `impl Into<String>` on one side and the variable enum on the other.
    for (old, new) in [
        (REGION_FREE_FORM, REGIONS_US_EU),
        (REGIONS_US_EU, REGION_FREE_FORM),
    ] {
        let report = diff(&with_servers(old), &with_servers(new));
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::ServerVariableTypeChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn renaming_a_server_variable_enum_under_the_same_builder_is_major() {
    // `prod`'s `region` enum is `ProdRegion` until a server named `prod-region` takes that
    // spelling: the `Prod` builder keeps its name, and its setter now takes the renamed enum.
    let prod = "  - name: prod
    url: 'https://{region}.example.com'
    variables:
      region: { default: us, enum: [us, eu] }
";
    let taken = format!("  - name: prod-region\n    url: 'https://other.example.com'\n{prod}");
    let report = diff(
        &with_servers(prod).replace("openapi: 3.1.0", "openapi: 3.2.0"),
        &with_servers(&taken).replace("openapi: 3.1.0", "openapi: 3.2.0"),
    );
    let mut found = kinds(&report);
    found.sort_by_key(|kind| kind.code());
    assert_eq!(
        found,
        vec![
            ChangeKind::ServerAdded,
            ChangeKind::ServerVariableTypeChanged
        ],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}
