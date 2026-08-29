use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
/// for rotation. Missing required credentials are a construction-time error, not a 401.
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

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secrets are redacted throughout.
        let kind = match self {
            Credential::Bearer(_) => "Bearer",
            Credential::Basic { .. } => "Basic",
            Credential::ApiKey(_) => "ApiKey",
            Credential::Provider(_) => "Provider",
        };
        write!(f, "Credential::{kind}(***)")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{AuthError, Credential, ExposeSecret, SecretString, TokenFuture};

    #[test]
    fn credential_debug_is_redacted() {
        let secret = SecretString::from("s3cr3t");
        assert_eq!(secret.expose_secret(), "s3cr3t");
        let credential = Credential::Bearer(secret);
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
    }

    /// `Debug` is hand-written precisely so a credential cannot reach a log. Only the `Bearer` arm
    /// was covered, and `Basic` is the one that holds a username in the clear alongside the secret
    /// — so it is the arm where a naive derive would leak most.
    #[test]
    fn every_credential_kind_redacts_its_secret_and_names_its_kind() {
        let cases: Vec<(Credential, &str)> = vec![
            (Credential::Bearer(SecretString::from("s3cr3t")), "Bearer"),
            (
                Credential::Basic {
                    username: "aladdin".to_owned(),
                    password: SecretString::from("s3cr3t"),
                },
                "Basic",
            ),
            (Credential::ApiKey(SecretString::from("s3cr3t")), "ApiKey"),
            (
                Credential::Provider(Arc::new(|| {
                    Box::pin(async { Ok(SecretString::from("s3cr3t")) }) as TokenFuture
                })),
                "Provider",
            ),
        ];

        for (credential, kind) in cases {
            let rendered = format!("{credential:?}");
            assert_eq!(rendered, format!("Credential::{kind}(***)"));
            assert!(!rendered.contains("s3cr3t"), "{rendered} leaks the secret");
            // The username is not a secret, but it is still an identity: `Debug` prints neither.
            assert!(
                !rendered.contains("aladdin"),
                "{rendered} leaks the username"
            );
        }
    }

    #[test]
    fn an_auth_error_displays_and_chains_as_an_error() {
        let error = AuthError::new("token endpoint returned 503");
        assert_eq!(error.to_string(), "token endpoint returned 503");
        assert!(std::error::Error::source(&error).is_none());
    }
}
