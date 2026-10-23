//! Security schemes and requirements.

use super::*;

#[test]
fn e012_unknown_security_scheme() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - undeclared: []
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnknownSecurityScheme));
}

#[test]
fn e012_http_security_scheme_that_cannot_be_attached() {
    // A `digest` scheme used to vanish silently, surfacing only as a confusing E012 at the
    // requirement site naming a scheme the document plainly declares.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    digestAuth:
      type: http
      scheme: digest
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::UnknownSecurityScheme),
        "{report:#?}"
    );
}

#[test]
fn an_http_authentication_scheme_matches_in_any_letter_case() {
    // OpenAPI makes the `http` scheme's `scheme` value case-insensitive (RFC 9110), and
    // `Bearer` is a common spelling. Matching only the lowercase literals rejected it with E012,
    // so every spelling here must lower to the scheme its lowercase form names.
    for (spelling, kind) in [
        ("bearer", "Bearer"),
        ("Bearer", "Bearer"),
        ("BEARER", "Bearer"),
        ("bEaReR", "Bearer"),
        ("basic", "Basic"),
        ("Basic", "Basic"),
        ("BASIC", "Basic"),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      security:
        - token: []
      responses:
        "204": {{ description: No Content }}
components:
  securitySchemes:
    token: {{ type: http, scheme: {spelling} }}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert!(
            !has_code(&report, Code::UnknownSecurityScheme),
            "`{spelling}`: {report:#?}"
        );
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{spelling}`: {report:#?}"
        );
        let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let attached = format!("name:\"token\",kind:support::AuthKind::{kind},");
        assert!(
            compact.contains(&attached),
            "`{spelling}` must attach as `AuthKind::{kind}`:\n{code}"
        );
    }
}

#[test]
fn e004_a_security_scheme_ref_that_cannot_be_followed_says_why() {
    // A security scheme `$ref` is followed one hop into this document's
    // `#/components/securitySchemes/`. Each way that fails used to share one message,
    // "unresolved security scheme reference", which was false on its face for an alias whose
    // target the document plainly declares — so each case pins its own wording, and none of the
    // declared-target cases may call itself unresolved.
    let spec_with = |schemes: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: "https://e.com" }}]
security:
  - A: []
paths:
  /x:
    get:
      responses:
        "204": {{ description: No Content }}
x-elsewhere:
  Bearer: {{ type: http, scheme: bearer }}
components:
  securitySchemes:
{schemes}"##
        )
    };
    let cases = [
        (
            "an alias to an alias",
            spec_with(
                "    A: { $ref: \"#/components/securitySchemes/B\" }\n    B: { $ref: \"#/components/securitySchemes/C\" }\n    C: { type: http, scheme: bearer }\n",
            ),
            "resolves to another security scheme `$ref`; chained security scheme references are \
             not resolved",
        ),
        (
            "an alias to itself",
            spec_with("    A: { $ref: \"#/components/securitySchemes/A\" }\n"),
            "resolves to another security scheme `$ref`; chained security scheme references are \
             not resolved",
        ),
        (
            "an alias to an undeclared scheme",
            spec_with("    A: { $ref: \"#/components/securitySchemes/Missing\" }\n"),
            "no scheme named `Missing` is declared",
        ),
        (
            "a reference outside the security scheme components",
            // The target is a well-formed Security Scheme, so only its location is wrong: a
            // target that is not a Security Scheme at all is rejected earlier, as `E011`, by
            // the metaschema, which validates every `$ref` target at its position.
            spec_with("    A: { $ref: \"#/x-elsewhere/Bearer\" }\n"),
            "does not point into this document's `#/components/securitySchemes/`",
        ),
    ];
    for (what, spec, wording) in &cases {
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            let pointers = e004_pointers(&report, what);
            assert!(
                pointers.contains(&"/components/securitySchemes/A"),
                "{what} via {entry}: E004 must point at the aliasing scheme, not {pointers:?}"
            );
            let messages = messages_for(&report, Code::UnresolvedRef);
            assert!(
                messages.iter().any(|m| m.contains(wording)),
                "{what} via {entry}: expected `{wording}` in {messages:?}"
            );
            if !wording.contains("no scheme named") {
                assert!(
                    messages.iter().all(|m| !m.contains("unresolved")),
                    "{what} via {entry}: a target that is present is not unresolved: {messages:?}"
                );
            }
        }
    }

    // The negative control: the one hop the specification requires still resolves.
    let one_hop = spec_with(
        "    A: { $ref: \"#/components/securitySchemes/B\" }\n    B: { type: http, scheme: bearer }\n",
    );
    let report = generate(&one_hop);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");

    // A target that is not a Security Scheme at all never reaches the location check.
    let not_a_scheme = spec_with("    A: { $ref: \"#/x-elsewhere/Bearer/type\" }\n");
    let report = generate(&not_a_scheme);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn w011_mutual_tls_is_satisfied_by_the_transport() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
security:
  - mtls: []
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    mtls:
      type: mutualTLS
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

#[test]
fn oas32_security_requirement_uri_resolves() {
    // OpenAPI 3.2 lets a requirement name a Security Scheme Object by URI. A component name always
    // wins, per the specification, so only a name matching no component is resolved as a reference.
    let (report, _, code) = generate_and_check_files(&[
        (
            "openapi.yaml",
            r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
security:
  - "./bearer.yaml": []
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
        ),
        ("bearer.yaml", "type: http\nscheme: bearer\n"),
    ]);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnknownSecurityScheme),
        "{report:#?}"
    );
    // The resolved scheme is attached to the operation under the requirement's own key: a
    // requirement dropped on the way would generate cleanly and attach nothing.
    let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains(
            "support::attach_auth(&self.core,request,&[&[support::AuthScheme{\
             name:\"./bearer.yaml\",kind:support::AuthKind::Bearer,},][..],],)"
        ),
        "the URI requirement must attach as `AuthKind::Bearer`:\n{code}"
    );
}

#[test]
fn a_urn_style_security_requirement_key_gets_todays_diagnostic() {
    // The loader and lowering disagree on whether a key containing `:` is a URI reference: the
    // loader does not fetch a `urn:`-style key, while lowering tries to resolve it. This pins the
    // diagnostic that disagreement produces today, so a change to either `:` rule shows up here.
    let report = generate(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
security:
  - "urn:example:bearer": []
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let summary: Vec<_> = report
        .diagnostics()
        .iter()
        .map(|d| (d.code, d.pointer.as_str()))
        .collect();
    assert_eq!(
        summary,
        [(Code::UnknownSecurityScheme, "/paths/~1x/get")],
        "{report:#?}"
    );
    assert!(
        report.diagnostics()[0]
            .message
            .contains("`urn:example:bearer`"),
        "{report:#?}"
    );
}

#[test]
fn security_scheme_documentation_reaches_credential_registration() {
    // The support matrix promises `bearerFormat`, flows, `openIdConnectUrl` and deprecation become
    // rustdoc on credential registration. `SecuritySchemeObject` used to carry four fields and none
    // of these, so every one of them was dropped without a trace.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - oauth: [read]
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
      bearerFormat: JWT
      description: A short-lived service token.
      deprecated: true
    oidc:
      type: openIdConnect
      openIdConnectUrl: https://id.example.com/.well-known/openid-configuration
    oauth:
      type: oauth2
      oauth2MetadataUrl: https://id.example.com/.well-known/oauth-authorization-server
      flows:
        authorizationCode:
          authorizationUrl: https://id.example.com/authorize
          tokenUrl: https://id.example.com/token
          scopes:
            read: Read your data
        deviceAuthorization:
          deviceAuthorizationUrl: https://id.example.com/device
          tokenUrl: https://id.example.com/token
          scopes:
            read: Read your data
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for expected in [
        "Bearer format: `JWT`",
        "A short-lived service token.",
        "**Deprecated.**",
        "OpenID Connect discovery: <https://id.example.com/.well-known/openid-configuration>",
        "OAuth 2 metadata: <https://id.example.com/.well-known/oauth-authorization-server>",
        "Flow `authorizationCode`",
        // OpenAPI 3.2's device flow, which `docs/openapi-3.2.md` also claims is documented.
        "Flow `deviceAuthorization`",
        "device authorization: <https://id.example.com/device>",
        "scope `read` — Read your data",
    ] {
        assert!(code.contains(expected), "missing {expected:?} in: {code}");
    }
}
