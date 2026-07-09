use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A secret credential value. `Debug` is redacted so secrets never leak through logs (PRD FR4).
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a secret.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// Borrow the underlying secret. Use only where the secret must cross the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(***)")
    }
}

/// A failure from an async token provider (PRD FR4).
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

/// An async callback that yields a fresh credential, for rotating tokens (PRD FR4).
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture + Send + Sync>;

/// A per-scheme credential supplied at client construction: a static secret or a token provider
/// for rotation (PRD FR4). Missing required credentials are a construction-time error, not a 401.
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

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secrets are redacted throughout (PRD FR4).
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
    use super::SecretString;

    #[test]
    fn secret_debug_is_redacted() {
        let secret = SecretString::new("token");
        assert_eq!(secret.expose(), "token");
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
    }
}
