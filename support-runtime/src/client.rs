use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use reqwest::Url;

use crate::{Credential, Error, HttpBackend, ReqwestBackend};

/// Client-wide configuration.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Maximum bytes of a response body retained on error variants; the rest is dropped and the
    /// error flags truncation (default 64 KiB).
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
