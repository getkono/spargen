use std::collections::HashMap;
use std::convert::Infallible;

use reqwest::Url;

use crate::{Credential, Error};

/// Client-wide configuration.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Maximum bytes of a response body retained on error variants; the rest is dropped and the
    /// error flags truncation (default 64 KiB; PRD D7).
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
/// injection point for TLS choice, proxies, middleware, timeouts — PRD FR3), the base URL,
/// configuration, and per-scheme credentials.
///
/// The generated `Client` exposes `Client::new(base_url)` and
/// `Client::with_client(reqwest::Client, base_url)`, and one `#[inline]` method per operation that
/// delegates to the non-generic dispatch routines (PRD FR3, NFR2).
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
        todo!()
    }

    /// Build a core with a caller-supplied `reqwest::Client` — the injection point for TLS backend,
    /// proxies, middleware, and timeouts (PRD FR3).
    pub fn with_client(client: reqwest::Client, base_url: &str) -> Result<Self, Error<Infallible>> {
        todo!()
    }

    /// The retention/config settings.
    pub fn config(&self) -> &ClientConfig {
        todo!()
    }

    /// The base URL.
    pub fn base_url(&self) -> &Url {
        todo!()
    }

    /// The injected HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        todo!()
    }

    /// Register a credential for a named security scheme (PRD FR4).
    pub fn set_credential(&mut self, scheme: &str, credential: Credential) {
        todo!()
    }
}
