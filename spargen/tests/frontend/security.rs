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
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::UnknownSecurityScheme),
            "{report:#?}"
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
    for report in [generate(&one_hop), check(&one_hop)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    }

    // A target that is not a Security Scheme at all never reaches the location check.
    let not_a_scheme = spec_with("    A: { $ref: \"#/x-elsewhere/Bearer/type\" }\n");
    for report in [generate(&not_a_scheme), check(&not_a_scheme)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
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
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn oas32_security_requirement_uri_resolves() {
    // OpenAPI 3.2 lets a requirement name a Security Scheme Object by URI. A component name always
    // wins, per the specification, so only a name matching no component is resolved as a reference.
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("bearer.yaml"),
        "type: http\nscheme: bearer\n",
    )
    .unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
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
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = run_generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out).unwrap(),
    ));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnknownSecurityScheme),
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
