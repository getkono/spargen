use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use reqwest::header::HeaderValue;
use reqwest::RequestBuilder;

use crate::{ClientCore, Error, RequestError};

// Secret handling is delegated to `secrecy` rather than hand-rolled: it zeroizes on drop and
// redacts `Debug`, and it is already a near-universal transitive dependency of rustls-based
// stacks. Re-exported so generated code and consumers use one vocabulary.
pub use secrecy::{ExposeSecret, SecretString};

/// A failure from an async token provider.
#[derive(Debug)]
pub struct AuthError {
    message: String,
}

impl AuthError {
    /// Build an authentication-provider failure from a displayable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthError {}

/// The future returned by a [`TokenProvider`]. `Send` on native (it is awaited inside a client
/// shared across tasks); on `wasm32` the browser is single-threaded and reqwest-adjacent futures are
/// `!Send`, so the `Send` bound is dropped there. `Send` is an auto trait and cannot be swapped for
/// the non-auto [`crate::MaybeSend`] as an extra trait-object bound, so the alias is `cfg`-gated.
#[cfg(not(target_arch = "wasm32"))]
pub type TokenFuture = Pin<Box<dyn Future<Output = Result<SecretString, AuthError>> + Send>>;
/// The future returned by a [`TokenProvider`] (the wasm variant: no `Send`).
#[cfg(target_arch = "wasm32")]
pub type TokenFuture = Pin<Box<dyn Future<Output = Result<SecretString, AuthError>>>>;

/// An async callback that yields a fresh credential, for rotating tokens. `Send + Sync` on native
/// (the provider is shared across tasks); vacuous on `wasm32`, matching [`TokenFuture`].
#[cfg(not(target_arch = "wasm32"))]
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture + Send + Sync>;
/// An async callback that yields a fresh credential (the wasm variant: no `Send + Sync`).
#[cfg(target_arch = "wasm32")]
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture>;

/// A per-scheme credential supplied at client construction: a static secret or a token provider
/// for rotation.
///
/// Registration checks nothing: a credential is stored under whatever `securitySchemes` key it is
/// given, and is matched against that scheme's [`AuthKind`] only when an operation that requires
/// the scheme is called. Every failure below is therefore an error of that call, raised as a
/// request-construction error before anything is sent — never a 401 from the server:
///
/// - no alternative of the operation's requirement has every scheme registered:
///   [`super::RequestError::MissingCredential`];
/// - the selected alternative has a credential of a variant its scheme cannot carry:
///   [`super::RequestError::CredentialMismatch`];
/// - the selected alternative's [`Credential::Provider`] fails:
///   [`super::RequestError::CredentialProvider`].
///
/// Which variant a scheme accepts:
///
/// - `http basic` ([`AuthKind::Basic`]) accepts only [`Credential::Basic`];
/// - every scheme that carries a token — `http bearer`, `oauth2` and `openIdConnect` (all three
///   [`AuthKind::Bearer`]) and `apiKey` in a header, query or cookie — accepts [`Credential::Bearer`],
///   [`Credential::ApiKey`] and [`Credential::Provider`], and rejects [`Credential::Basic`];
/// - `mutualTLS` ([`AuthKind::MutualTls`]) is satisfied by the transport and never reads a
///   registered credential, so one registered under it is ignored.
#[derive(Clone)]
pub enum Credential {
    /// `Authorization: Bearer <token>`.
    Bearer(SecretString),
    /// HTTP basic auth.
    Basic {
        /// The username.
        username: String,
        /// The password.
        password: SecretString,
    },
    /// An `apiKey` value.
    ApiKey(SecretString),
    /// A rotating token supplied on demand.
    ///
    /// The provider yields one secret, so it stands in for [`Credential::Bearer`] or
    /// [`Credential::ApiKey`]: it is accepted under every scheme that carries a token (`http
    /// bearer`, `oauth2`, `openIdConnect`, and `apiKey` in a header, query or cookie) and is asked
    /// for a fresh token each time a call attaches it. A provider that fails fails that call as
    /// [`super::RequestError::CredentialProvider`], with its [`AuthError`] as the source.
    ///
    /// It is **not** accepted under an `http basic` scheme, which needs a username and password
    /// ([`Credential::Basic`]). There the call fails as
    /// [`super::RequestError::CredentialMismatch`] with `required: "http basic"` and
    /// `registered: "Provider"`, and the provider is never called. Under a `mutualTLS` scheme it is
    /// ignored and never called, since the transport's client certificate satisfies the scheme.
    Provider(TokenProvider),
}

/// How a security scheme carries its credential on the wire. Generated code builds these as
/// static tables from the spec's `securitySchemes`; `oauth2`/`openIdConnect` schemes attach their
/// caller-supplied token as a bearer credential.
#[derive(Debug, Clone, Copy)]
pub enum AuthKind {
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// `Authorization: Basic <base64>`.
    Basic,
    /// An `apiKey` sent as the named request header.
    ApiKeyHeader(&'static str),
    /// An `apiKey` sent as the named query parameter.
    ApiKeyQuery(&'static str),
    /// An `apiKey` sent as the named cookie.
    ApiKeyCookie(&'static str),
    /// `mutualTLS`. Satisfied by the client certificate configured on the injected
    /// `reqwest::Client`, so the request carries nothing extra and no credential is registered.
    MutualTls,
}

/// One scheme reference inside an operation's security requirement: the `securitySchemes` key the
/// credential is registered under, plus how it is carried.
#[derive(Debug, Clone, Copy)]
pub struct AuthScheme {
    /// The `components.securitySchemes` key.
    pub name: &'static str,
    /// The wire carrier.
    pub kind: AuthKind,
}

impl Credential {
    /// The variant's name, as `Debug` renders it and as
    /// [`super::RequestError::CredentialMismatch`] reports it in `registered`.
    fn kind_name(&self) -> &'static str {
        match self {
            Credential::Bearer(_) => "Bearer",
            Credential::Basic { .. } => "Basic",
            Credential::ApiKey(_) => "ApiKey",
            Credential::Provider(_) => "Provider",
        }
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secrets are redacted throughout.
        write!(f, "Credential::{}(***)", self.kind_name())
    }
}

/// Attach credentials for an operation's security requirement. `requirements` is an OR
/// of alternatives, each an AND of schemes; the first alternative whose schemes all have a
/// registered credential wins, deterministically. An empty alternative (`{}` in the spec) marks
/// security optional and always satisfies. If no alternative is satisfiable the request fails
/// before it is sent — [`RequestError::MissingCredential`], never a silent 401 — and a registered
/// token provider that fails does too, as [`RequestError::CredentialProvider`].
///
/// **Selection is on registration, not on success, and there is no fall-through.** Once an
/// alternative is chosen, a failure while attaching it — a token provider that errors, or a
/// credential registered under a kind its scheme cannot use — fails the call, even when a later
/// alternative is fully registered and would have succeeded. Falling through would send
/// credentials the caller's registration did not select, silently, on a call they had expressed a
/// different intent for; an error they can see is the better failure.
///
/// A caller who wants the *other* alternative unregisters a scheme of the earlier one with
/// [`ClientCore::remove_credential`] (the generated client's `without_credential`); selection then
/// passes over the earlier alternative and reaches the later one on the next call. Registering the
/// fallback as well is not enough on its own, because selection stops at the first satisfiable
/// alternative and the earlier one stays satisfied until one of its schemes is removed.
pub async fn attach_auth(
    core: &ClientCore,
    request: RequestBuilder,
    requirements: &[&[AuthScheme]],
) -> Result<RequestBuilder, Error<Infallible>> {
    // No requirement means "attach nothing", not "unauthenticated". Without this, an empty slice
    // would fall through the loop below without choosing anything, and the call would fail as
    // `MissingCredential` naming no schemes at all. Generated output never produces an empty slice
    // — `codegen/dispatch.rs` omits it for an operation with no `security` — but this function
    // is public in the runtime crate and reachable from sibling code in whichever module `include!`s
    // a generated client, so this is a contract, not dead code, and spargen's own runtime tests
    // (which are not embedded) hold it to that.
    if requirements.is_empty() {
        return Ok(request);
    }
    // Selection and reporting are one computation: an alternative is chosen exactly when the list
    // of its unregistered schemes is empty, and that same list is what `MissingCredential`
    // reports for it. So every reported inner list is non-empty by construction, and the outer
    // list is non-empty because `requirements` is.
    let mut alternatives: Vec<Vec<&'static str>> = Vec::with_capacity(requirements.len());
    for alternative in requirements {
        let mut missing = Vec::new();
        let mut registered = Vec::with_capacity(alternative.len());
        for scheme in *alternative {
            match resolve(core, scheme) {
                Resolution::Transport => {}
                Resolution::Registered(credential) => registered.push((scheme, credential)),
                Resolution::Missing => missing.push(scheme.name),
            }
        }
        if missing.is_empty() {
            let mut request = request;
            for (scheme, credential) in registered {
                request = apply_credential(request, scheme, credential).await?;
            }
            return Ok(request);
        }
        alternatives.push(missing);
    }
    Err(Error::RequestConstruction(
        RequestError::MissingCredential { alternatives },
    ))
}

/// How one scheme of a security alternative is satisfied, if at all.
enum Resolution<'a> {
    /// Satisfied by the transport itself; nothing is attached to the request.
    Transport,
    /// Satisfied by the credential registered under the scheme's name.
    Registered(&'a Credential),
    /// Not satisfied: the caller still has to register a credential for it.
    Missing,
}

/// The single answer to "does this scheme block its alternative?", read by both the selection
/// and the `MissingCredential` report in [`attach_auth`]. The match is exhaustive over
/// [`AuthKind`], so a new kind has to state how it is satisfied before it compiles.
fn resolve<'a>(core: &'a ClientCore, scheme: &AuthScheme) -> Resolution<'a> {
    match scheme.kind {
        // `mutualTLS` is satisfied by the transport's client certificate, so it never needs a
        // registered credential and never blocks an alternative from being chosen.
        AuthKind::MutualTls => Resolution::Transport,
        AuthKind::Bearer
        | AuthKind::Basic
        | AuthKind::ApiKeyHeader(_)
        | AuthKind::ApiKeyQuery(_)
        | AuthKind::ApiKeyCookie(_) => match core.credential(scheme.name) {
            Some(credential) => Resolution::Registered(credential),
            None => Resolution::Missing,
        },
    }
}

async fn apply_credential(
    request: RequestBuilder,
    scheme: &AuthScheme,
    credential: &Credential,
) -> Result<RequestBuilder, Error<Infallible>> {
    // A provider yields a single secret, usable anywhere a bearer token or apiKey fits. Only a kind
    // that takes a token asks it for one, so a provider registered under a scheme that takes none
    // is the same mismatch whether or not a refresh would have succeeded. `attach_auth` skips
    // `MutualTls` before calling this, so the `MutualTls` term below is never evaluated from there;
    // it is deliberate defence, so a caller that bypasses the skip still never fetches a token.
    let takes_token = !matches!(scheme.kind, AuthKind::Basic | AuthKind::MutualTls);
    let token: Option<SecretString> = match credential {
        Credential::Bearer(secret) | Credential::ApiKey(secret) => Some(secret.clone()),
        Credential::Provider(provider) if takes_token => {
            Some(provider().await.map_err(|source| {
                Error::RequestConstruction(RequestError::CredentialProvider {
                    scheme: scheme.name,
                    source,
                })
            })?)
        }
        Credential::Provider(_) | Credential::Basic { .. } => None,
    };
    match scheme.kind {
        AuthKind::Basic => match credential {
            Credential::Basic { username, password } => {
                Ok(request.basic_auth(username, Some(password.expose_secret())))
            }
            _ => Err(credential_mismatch(scheme.name, "http basic", credential)),
        },
        AuthKind::Bearer => match token {
            Some(token) => Ok(request.bearer_auth(token.expose_secret())),
            None => Err(credential_mismatch(scheme.name, "bearer", credential)),
        },
        AuthKind::ApiKeyHeader(name) => match token {
            Some(token) => Ok(request.header(name, sensitive_value(token.expose_secret())?)),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
        AuthKind::ApiKeyQuery(name) => match token {
            Some(token) => Ok(request.query(&[(name, token.expose_secret())])),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
        // Satisfied by the transport; `attach_auth` never reaches this arm.
        AuthKind::MutualTls => Ok(request),
        AuthKind::ApiKeyCookie(name) => match token {
            Some(token) => {
                let cookie = format!("{name}={}", token.expose_secret());
                Ok(request.header(reqwest::header::COOKIE, sensitive_value(&cookie)?))
            }
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
    }
}

fn sensitive_value(secret: &str) -> Result<HeaderValue, Error<Infallible>> {
    let mut value = HeaderValue::from_str(secret).map_err(Error::request_construction)?;
    value.set_sensitive(true);
    Ok(value)
}

fn credential_mismatch(
    scheme: &'static str,
    required: &'static str,
    credential: &Credential,
) -> Error<Infallible> {
    Error::RequestConstruction(RequestError::CredentialMismatch {
        scheme,
        required,
        registered: credential.kind_name(),
    })
}

#[cfg(test)]
mod tests;
