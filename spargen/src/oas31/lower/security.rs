//! Security Scheme Objects and Security Requirement Objects.

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ir::{ApiKeyLoc, HttpScheme, SchemeId, SecurityScheme, SecuritySchemeDef};
use crate::oas31::{Document, RefOr, Resolver, SecurityRequirement};

pub(super) fn lower_security_requirement(
    requirement: &SecurityRequirement,
) -> crate::ir::SecurityRequirement {
    crate::ir::SecurityRequirement(
        requirement
            .0
            .iter()
            .map(|(name, scopes)| (SchemeId(name.clone()), scopes.clone()))
            .collect(),
    )
}

/// Resolve security requirement names that are URIs rather than declared component names.
pub(super) fn resolve_external_security_schemes(
    document: &Document,
    resolver: &Resolver,
    schemes: &mut IndexMap<SchemeId, SecuritySchemeDef>,
    diags: &mut Diagnostics,
) {
    let mut wanted: Vec<(String, crate::diag::Provenance)> = Vec::new();
    let mut collect = |requirements: &[SecurityRequirement], at: &crate::diag::Provenance| {
        for requirement in requirements {
            for name in requirement.0.keys() {
                wanted.push((name.clone(), at.clone()));
            }
        }
    };
    collect(&document.security, &document.provenance);
    for item in document.paths.items.values() {
        for operation in item.operations.values() {
            if let Some(security) = &operation.security {
                collect(security, &operation.provenance);
            }
        }
    }
    for (name, at) in wanted {
        if schemes.contains_key(&SchemeId(name.clone())) {
            continue;
        }
        // Only a name that looks like a reference is worth resolving; a plain unknown name is an
        // ordinary undeclared-scheme error, reported at the requirement site.
        let reference = match name.strip_prefix("./") {
            Some(rest) => rest.to_owned(),
            None if name.contains('/') || name.contains('#') || name.contains(':') => name.clone(),
            None => continue,
        };
        let from = resolver.written_in(&at);
        let Ok(object) = resolver.resolve_component(
            &reference,
            from,
            crate::oas31::deserialize::parse_security_scheme,
            diags,
        ) else {
            continue;
        };
        let mut resolved = IndexMap::new();
        resolved.insert(name.clone(), RefOr::Item(object));
        let mut document = document.clone();
        document.components.security_schemes = resolved;
        for (id, scheme) in lower_security_schemes(&document, diags) {
            schemes.insert(id, scheme);
        }
    }
}

/// Lower `components.securitySchemes`.
///
/// Every declared scheme gets a disposition here rather than only when something references it: a
/// scheme that silently vanished used to surface — if at all — as a confusing `E012` at the
/// requirement site, naming a scheme the document plainly declares.
pub(super) fn lower_security_schemes(
    document: &Document,
    diags: &mut Diagnostics,
) -> IndexMap<SchemeId, SecuritySchemeDef> {
    let mut schemes = IndexMap::new();
    for (name, scheme) in &document.components.security_schemes {
        let scheme = match scheme {
            RefOr::Item(scheme) => scheme,
            // A `$ref` to another scheme component of this document resolves, one hop only. Each
            // way it can fail gets its own message: calling a declared-but-aliased scheme
            // "unresolved" would send the reader looking for a declaration that is right there.
            RefOr::Ref(reference) => {
                let resolved = match reference
                    .reference
                    .strip_prefix("#/components/securitySchemes/")
                {
                    None => Err(format!(
                        "security scheme `$ref` `{}` does not point into this document's \
                         `#/components/securitySchemes/`; only a reference to a scheme the \
                         same document declares is resolved",
                        reference.reference
                    )),
                    Some(target) => match document.components.security_schemes.get(target) {
                        Some(RefOr::Item(target)) => Ok(target),
                        None => Err(format!(
                            "unresolved security scheme reference `{}`: no scheme named \
                             `{target}` is declared under `#/components/securitySchemes/`",
                            reference.reference
                        )),
                        // One level of indirection is what the specification requires, and a
                        // chain would need its own cycle guard (an alias to itself is one).
                        Some(RefOr::Ref(_)) => Err(format!(
                            "security scheme `$ref` `{}` resolves to another security scheme \
                             `$ref`; chained security scheme references are not resolved",
                            reference.reference
                        )),
                    },
                };
                match resolved {
                    Ok(target) => target,
                    Err(message) => {
                        // E004 case: undeclared-component, declined-hop
                        Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
                            .message(message)
                            .remedy(
                                "reference a scheme declared directly, not as another `$ref`, \
                                 under this document's `#/components/securitySchemes/`",
                            )
                            .emit(diags);
                        continue;
                    }
                }
            }
        };
        let lowered = match scheme.scheme_type.as_str() {
            "http" => match scheme.scheme.as_deref() {
                Some("bearer") => SecurityScheme::Http(HttpScheme::Bearer),
                Some("basic") => SecurityScheme::Http(HttpScheme::Basic),
                other => {
                    // `digest`, `negotiate`, and friends need a challenge/response exchange that a
                    // statically-attached credential cannot perform.
                    Diagnostic::error(Code::UnknownSecurityScheme, scheme.provenance.clone())
                        .message(format!(
                            "`http` security scheme `{}` uses authentication scheme `{}`, which \
                             spargen cannot attach",
                            name,
                            other.unwrap_or("<missing>")
                        ))
                        .remedy(
                            "use `bearer` or `basic`, or omit this API segment with \
                             spargen::omit!",
                        )
                        .emit(diags);
                    continue;
                }
            },
            "apiKey" => {
                let location = match scheme.location.as_deref() {
                    Some("header") => ApiKeyLoc::Header,
                    Some("query") => ApiKeyLoc::Query,
                    Some("cookie") => ApiKeyLoc::Cookie,
                    // The document schema requires a valid `in` for `apiKey`.
                    _ => continue,
                };
                SecurityScheme::ApiKey {
                    location,
                    name: scheme.name.clone().unwrap_or_else(|| name.clone()),
                }
            }
            "oauth2" => SecurityScheme::OAuth2,
            "openIdConnect" => SecurityScheme::OpenIdConnect,
            "mutualTLS" => {
                // W011 case: mutual-tls
                Diagnostic::warning(Code::DeclarationHasNoEffect, scheme.provenance.clone())
                    .message(format!(
                        "`mutualTLS` scheme `{name}` is satisfied by the client certificate on the \
                         injected `reqwest::Client`, so no credential is registered for it"
                    ))
                    .remedy(
                        "configure the certificate on the client passed to `Client::with_client`",
                    )
                    .emit(diags);
                SecurityScheme::MutualTls
            }
            // The document schema closes the `type` enum.
            _ => continue,
        };
        schemes.insert(
            SchemeId(name.clone()),
            SecuritySchemeDef {
                kind: lowered,
                docs: security_scheme_docs(name, scheme),
            },
        );
    }
    schemes
}

/// Render the documentation a Security Scheme Object carries into rustdoc lines.
///
/// A caller of `Client::with_credential` needs exactly this to know what to register: the token
/// format, where a token is obtained, and whether the scheme is on its way out. None of it changes
/// a byte on the wire, which is why it is documentation rather than lowered structure.
fn security_scheme_docs(name: &str, scheme: &crate::oas31::SecuritySchemeObject) -> Vec<String> {
    let mut docs = Vec::new();
    let kind = match scheme.scheme_type.as_str() {
        "http" => match scheme.scheme.as_deref() {
            Some(inner) => format!("`http` (`{inner}`)"),
            None => "`http`".to_owned(),
        },
        other => format!("`{other}`"),
    };
    docs.push(format!("- `{name}` — {kind}."));
    if scheme.deprecated {
        docs.push("  - **Deprecated.**".to_owned());
    }
    if let Some(description) = &scheme.description {
        docs.push(format!("  - {}", description.replace('\n', " ")));
    }
    if let Some(format) = &scheme.bearer_format {
        docs.push(format!("  - Bearer format: `{format}`."));
    }
    if let Some(url) = &scheme.open_id_connect_url {
        docs.push(format!("  - OpenID Connect discovery: <{url}>"));
    }
    if let Some(url) = &scheme.oauth2_metadata_url {
        docs.push(format!("  - OAuth 2 metadata: <{url}>"));
    }
    for flow in &scheme.flows {
        docs.push(format!("  - Flow `{}`:", flow.name));
        for (label, url) in [
            ("authorization", &flow.authorization_url),
            ("token", &flow.token_url),
            ("refresh", &flow.refresh_url),
            ("device authorization", &flow.device_authorization_url),
        ] {
            if let Some(url) = url {
                docs.push(format!("    - {label}: <{url}>"));
            }
        }
        for (scope, description) in &flow.scopes {
            let description = description.replace('\n', " ");
            if description.is_empty() {
                docs.push(format!("    - scope `{scope}`"));
            } else {
                docs.push(format!("    - scope `{scope}` — {description}"));
            }
        }
    }
    docs
}
