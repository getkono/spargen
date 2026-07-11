use std::collections::HashMap;
use std::convert::Infallible;

use reqwest::Url;

use crate::{Credential, Error};

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

/// The shared core every generated `Client` wraps: the injected `reqwest::Client` (the BYO-client
/// injection point for TLS choice, proxies, middleware, timeouts), the base URL,
/// configuration, and per-scheme credentials.
///
/// The generated `Client` exposes `Client::new(base_url)` and
/// `Client::with_client(reqwest::Client, base_url)`, and one `#[inline]` method per operation that
/// delegates to the non-generic dispatch routines.
#[derive(Debug, Clone)]
pub struct ClientCore {
    http: reqwest::Client,
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
    /// proxies, middleware, and timeouts.
    pub fn with_client(client: reqwest::Client, base_url: &str) -> Result<Self, Error<Infallible>> {
        let base_url = Url::parse(base_url).map_err(Error::request_construction)?;
        Ok(Self {
            http: client,
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

    /// The injected HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
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
