use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use reqwest::Url;

use crate::{Credential, Error, HttpBackend, ReqwestBackend};

/// Client-wide configuration.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Maximum bytes of a response body retained on error variants; the rest is dropped
    /// (default 64 KiB).
    ///
    /// Where it applies, and what says so:
    ///
    /// - Bodies read as errors are capped, and on native targets the cap bounds *reading* too —
    ///   an over-cap body is abandoned partway, which forgoes reuse of that connection. On
    ///   `wasm32` the `fetch` backend has no incremental read, so there only retention is bounded.
    /// - A success body that fails to decode is capped on retention only. Deserialization needs
    ///   the whole body, so it is always read whole.
    /// - [`Error::Decode`](crate::Error::Decode) reports truncation through its `truncated` field.
    ///   [`Error::Api`](crate::Error::Api) and
    ///   [`Error::UnexpectedStatus`](crate::Error::UnexpectedStatus) have no such field, so where
    ///   they are capped they are capped silently.
    /// - Two paths are **not** capped at all: the generated shim for an operation with more than
    ///   one documented success status, which reads through
    ///   [`read_success_body`](crate::read_success_body) and reaches both `Decode` and
    ///   `UnexpectedStatus`; and `EventStream`'s per-frame decode, which retains one frame drawn
    ///   from a frame buffer that is itself unbounded.
    pub max_error_body: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_error_body: 64 * 1024,
        }
    }
}

/// The shared core every generated `Client` wraps: the `reqwest::Client` used to BUILD requests
/// (the BYO-client injection point for TLS choice, proxies, timeouts), the swappable transport
/// [`HttpBackend`] that EXECUTES them, the base URL, configuration, and per-scheme credentials.
///
/// Request building stays on a concrete `reqwest::Client`; only the execute step is routed through
/// the backend (see [`crate::send`]). `new`/`with_client` install the default [`ReqwestBackend`],
/// so their behavior is unchanged; `with_backend` plugs a caller-supplied transport.
///
/// The generated `Client` exposes `Client::new(base_url)`,
/// `Client::with_client(reqwest::Client, base_url)`, and
/// `Client::with_backend(Arc<dyn HttpBackend>, base_url)`, plus one `#[inline]` method per operation
/// that delegates to the non-generic dispatch routines.
#[derive(Debug, Clone)]
pub struct ClientCore {
    http: reqwest::Client,
    backend: Arc<dyn HttpBackend>,
    base_url: Url,
    config: ClientConfig,
    credentials: HashMap<String, Credential>,
}

impl ClientCore {
    /// Build a core with a default `reqwest::Client` and the given base URL. Returns a
    /// request-construction error if the base URL is invalid.
    pub fn new(base_url: &str) -> Result<Self, Error<Infallible>> {
        Self::with_client(reqwest::Client::new(), base_url)
    }

    /// Build a core with a caller-supplied `reqwest::Client` — the injection point for TLS backend,
    /// proxies, and timeouts. The client both builds requests and, wrapped in the default
    /// [`ReqwestBackend`], executes them.
    pub fn with_client(client: reqwest::Client, base_url: &str) -> Result<Self, Error<Infallible>> {
        let backend = Arc::new(ReqwestBackend::new(client.clone()));
        Self::assemble(client, backend, base_url)
    }

    /// Build a core with a caller-supplied transport [`HttpBackend`] — the injection point for
    /// retry, middleware, or an entirely non-reqwest transport. Requests are still BUILT on a
    /// default `reqwest::Client`; only the execute step goes through `backend`.
    pub fn with_backend(
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
    ) -> Result<Self, Error<Infallible>> {
        Self::assemble(reqwest::Client::new(), backend, base_url)
    }

    fn assemble(
        http: reqwest::Client,
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
    ) -> Result<Self, Error<Infallible>> {
        let base_url = Url::parse(base_url).map_err(Error::request_construction)?;
        Ok(Self {
            http,
            backend,
            base_url,
            config: ClientConfig::default(),
            credentials: HashMap::new(),
        })
    }

    /// The retention/config settings.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Mutably borrow the retention/config settings.
    pub fn config_mut(&mut self) -> &mut ClientConfig {
        &mut self.config
    }

    /// The base URL.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// The injected HTTP client used to BUILD requests.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The transport backend that EXECUTES requests. [`crate::send`] dispatches through this;
    /// retry/middleware layers can clone the `Arc` to wrap it.
    pub fn backend(&self) -> &Arc<dyn HttpBackend> {
        &self.backend
    }

    /// Register a credential for a named security scheme.
    pub fn set_credential(&mut self, scheme: &str, credential: Credential) {
        self.credentials.insert(scheme.to_owned(), credential);
    }

    /// Retrieve a registered credential by scheme name.
    pub fn credential(&self, scheme: &str) -> Option<&Credential> {
        self.credentials.get(scheme)
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::{ClientConfig, ClientCore};
    use crate::Credential;

    #[test]
    fn an_invalid_base_url_is_a_request_construction_error() {
        // The only fallible step in construction. A generated client cannot be built at all
        // without a parseable base, so this must be an error rather than a panic or a silent
        // fallback to a default host.
        let error = ClientCore::new("not a url").unwrap_err();
        assert!(
            error.to_string().contains("request construction"),
            "{error}"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn a_base_url_is_parsed_and_retained() {
        let core = ClientCore::new("https://example.com/api/").unwrap();
        assert_eq!(core.base_url().as_str(), "https://example.com/api/");
    }

    #[test]
    fn the_default_error_body_cap_is_64_kib() {
        // Generated clients retain at most this much of an error body; the rest is dropped and
        // the error flags truncation. Consumers tune it through `config_mut`.
        assert_eq!(ClientConfig::default().max_error_body, 64 * 1024);
        let core = ClientCore::new("https://example.com").unwrap();
        assert_eq!(core.config().max_error_body, 64 * 1024);
    }

    #[test]
    fn the_error_body_cap_is_tunable_through_config_mut() {
        let mut core = ClientCore::new("https://example.com").unwrap();
        core.config_mut().max_error_body = 128;
        assert_eq!(core.config().max_error_body, 128);
    }

    #[test]
    fn credentials_round_trip_per_scheme_and_the_last_write_wins() {
        let mut core = ClientCore::new("https://example.com").unwrap();
        assert!(core.credential("token").is_none());

        core.set_credential("token", Credential::Bearer(SecretString::from("first")));
        assert!(matches!(
            core.credential("token"),
            Some(Credential::Bearer(_))
        ));
        // Registration is keyed by scheme name, so re-registering replaces rather than accumulates
        // — this is how a consumer rotates a static credential.
        core.set_credential(
            "token",
            Credential::Basic {
                username: "u".to_owned(),
                password: SecretString::from("p"),
            },
        );
        assert!(matches!(
            core.credential("token"),
            Some(Credential::Basic { .. })
        ));
        assert!(core.credential("other").is_none());
    }

    #[test]
    fn with_client_installs_the_supplied_client_for_building_requests() {
        let client = reqwest::Client::new();
        let core = ClientCore::with_client(client, "https://example.com").unwrap();
        // The injected client is the one requests are built on; the default backend wraps a clone
        // of it for execution.
        assert!(core
            .http()
            .request(reqwest::Method::GET, "https://example.com/x")
            .build()
            .is_ok());
    }
}
