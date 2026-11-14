//! The typed credential setters each security scheme gives `Client` and `BlockingClient`.

use super::*;

/// The `/pets` spec with `schemes` as its `components.securitySchemes` body (4-space indent).
fn with_schemes(schemes: &str) -> String {
    format!(
        "openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
{}components:
  securitySchemes:
{schemes}  schemas:
{PET_SCHEMA}",
        pets_get("listPets", "", PET_REF)
    )
}

const BEARER: &str = "    bearer: { type: http, scheme: bearer }\n";
const BEARER_AS_BASIC: &str = "    bearer: { type: http, scheme: basic }\n";
const BEARER_RENAMED: &str = "    token: { type: http, scheme: bearer }\n";
const KEY: &str = "    key: { type: apiKey, in: header, name: X-Key }\n";
const MTLS: &str = "    mtls: { type: mutualTLS }\n";

#[test]
fn adding_a_credential_scheme_is_minor_and_removing_one_is_major() {
    let more = format!("{BEARER}{KEY}");
    let added = diff(&with_schemes(BEARER), &with_schemes(&more));
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::CredentialSchemeAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(&with_schemes(&more), &with_schemes(BEARER));
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::CredentialSchemeRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

#[test]
fn renaming_a_credential_scheme_is_major() {
    let renamed = diff(&with_schemes(BEARER), &with_schemes(BEARER_RENAMED));
    assert_eq!(
        kinds(&renamed),
        vec![
            ChangeKind::CredentialSchemeRemoved,
            ChangeKind::CredentialSchemeAdded
        ],
        "{:?}",
        renamed.changes
    );
    assert_eq!(renamed.bump, Impact::Major);
}

#[test]
fn a_scheme_moving_between_a_token_and_http_basic_is_major() {
    let changed = diff(&with_schemes(BEARER), &with_schemes(BEARER_AS_BASIC));
    assert_eq!(
        kinds(&changed),
        vec![ChangeKind::CredentialSettersChanged],
        "{:?}",
        changed.changes
    );
    assert_eq!(changed.bump, Impact::Major);
}

/// A `mutualTLS` scheme takes no credential, so it gives the clients no setter to add or remove.
#[test]
fn a_mutual_tls_scheme_is_no_credential_surface() {
    let more = format!("{BEARER}{MTLS}");
    let added = diff(&with_schemes(BEARER), &with_schemes(&more));
    assert_eq!(kinds(&added), vec![], "{:?}", added.changes);
    assert_eq!(added.bump, Impact::Patch);
}
