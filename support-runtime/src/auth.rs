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

/// The future returned by a [`TokenProvider`].
pub type TokenFuture = Pin<Box<dyn Future<Output = Result<SecretString, AuthError>> + Send>>;

/// An async callback that yields a fresh credential, for rotating tokens.
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture + Send + Sync>;

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
    use super::{Credential, ExposeSecret, SecretString};

    #[test]
    fn credential_debug_is_redacted() {
        let secret = SecretString::from("s3cr3t");
        assert_eq!(secret.expose_secret(), "s3cr3t");
        let credential = Credential::Bearer(secret);
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
    }
}
