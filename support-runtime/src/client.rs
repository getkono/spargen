use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;

use crate::{Credential, Error, HttpBackend, LinkPaginator, ReqwestBackend};

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
    ///   `wasm32`, `ReqwestBackend` delivers every `fetch` body whole, an error body of a
    ///   streaming operation included, so there only retention is guaranteed to be bounded.
    /// - A success body that fails to decode is capped on retention only. Deserialization needs
    ///   the whole body, so it is always read whole.
    /// - [`Error::Decode`] reports truncation through its `truncated` field. [`Error::Api`] and
    ///   [`Error::UnexpectedStatus`] have no such field, so where they are capped they are capped
    ///   silently.
    /// - One path is **not** capped by it: `EventStream`'s per-frame decode, which
    ///   [`Self::max_stream_frame`] bounds instead.
    pub max_error_body: usize,
    /// Maximum bytes `EventStream` buffers for one item whose terminator has not arrived yet
    /// (default 16 MiB): an SSE event before its blank line, an ndjson line before its newline, a
    /// JSON text sequence record before the next separator.
    ///
    /// Once more than this many bytes of such an item are buffered, the stream yields
    /// [`Error::Decode`] retaining at most `max_error_body` bytes of them, with `truncated` set,
    /// and then ends, without consulting a reconnect policy. Complete items read in the same
    /// chunk ahead of the over-limit one are yielded first. Bytes arrive a chunk at a time, so the
    /// buffer may hold up to one chunk more than this limit before the check runs, and an item
    /// whose terminator arrives in the same chunk that takes it past the limit is framed
    /// normally.
    ///
    /// It also bounds what a per-frame decode failure retains: an item within the limit that
    /// fails to decode is reported as [`Error::Decode`] retaining the whole frame, uncapped by
    /// `max_error_body`, with `truncated` unset.
    pub max_stream_frame: usize,
    /// How long establishing a connection may take — name resolution, the TCP handshake and TLS —
    /// before the request fails with [`Error::Timeout`] (default 30 s; `None` waits as long as
    /// the operating system does).
    ///
    /// It is read once, when [`ClientCore::with_config`] or [`ClientCore::new`] builds the
    /// `reqwest::Client`, so changing it through [`ClientCore::config_mut`] afterwards changes
    /// nothing. A client injected with [`ClientCore::with_client`] keeps its own connect timeout
    /// instead, and a backend installed with [`ClientCore::with_backend`] or
    /// [`ClientCore::with_client_and_backend`] executes requests on its own terms. reqwest's `fetch` backend has no connect timeout, so on `wasm32` it is ignored.
    pub connect_timeout: Option<Duration>,
    /// How long a read on an established connection may wait for its next bytes — the response
    /// head, or the next chunk of a body, a streamed one included — before the request fails with
    /// [`Error::Timeout`] (default 5 min; `None` waits indefinitely). Each read that arrives
    /// restarts it, so it bounds a stall, not the length of a response. An event stream whose
    /// server sends nothing, not even a comment or heartbeat, for longer than this fails with
    /// [`Error::Timeout`], which a reconnect policy may answer by resuming it.
    ///
    /// It is read when the `reqwest::Client` is built and applies where it applies, exactly as
    /// [`Self::connect_timeout`] describes; reqwest's `fetch` backend has no read timeout
    /// either, so on `wasm32` it is ignored.
    pub read_timeout: Option<Duration>,
    /// How long one non-streaming request may take in total, from connecting until its whole
    /// response body has been read, before it fails with [`Error::Timeout`] (default `None`: no
    /// total bound, only the connect and read timeouts).
    ///
    /// It is read each time [`crate::send`] sends a request, so changing it through
    /// [`ClientCore::config_mut`] applies from the next request on, whichever constructor built
    /// the core, and it is honoured on `wasm32` too. A generated client's `with_timeout` sets it
    /// on the client value it returns, which is how one call gets a deadline of its own. It
    /// travels on the request itself: a request built with its own
    /// `reqwest::RequestBuilder::timeout` keeps that one,
    /// [`ReqwestBackend`] enforces it, and a caller-supplied [`HttpBackend`] that runs the
    /// request on a `reqwest::Client` enforces it as well. A streaming response is exempt,
    /// because a stream may rightly stay open indefinitely: an operation returning an
    /// `EventStream` sends its request with [`crate::send_streaming`], which never applies it, and
    /// each reconnect resends that same request.
    pub timeout: Option<Duration>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_error_body: 64 * 1024,
            max_stream_frame: 16 * 1024 * 1024,
            connect_timeout: Some(Duration::from_secs(30)),
            read_timeout: Some(Duration::from_secs(5 * 60)),
            timeout: None,
        }
    }
}

/// The shared core every generated `Client` wraps: the `reqwest::Client` used to BUILD requests
/// (the BYO-client injection point for TLS choice, proxies, timeouts), the swappable transport
/// [`HttpBackend`] that EXECUTES them, the base URL, configuration, and per-scheme credentials.
///
/// Request building stays on a concrete `reqwest::Client`; only the execute step is routed through
/// the backend (see [`crate::send`]). `new`/`with_client` install the default [`ReqwestBackend`];
/// `with_backend` plugs a caller-supplied transport, and `with_client_and_backend` takes both.
///
/// The generated `Client` wraps one of these and documents its own constructors; each of its
/// operation methods delegates to the shared dispatch routines.
#[derive(Debug, Clone)]
pub struct ClientCore {
    http: reqwest::Client,
    backend: Arc<dyn HttpBackend>,
    base_url: Url,
    config: ClientConfig,
    credentials: HashMap<String, Credential>,
}

impl ClientCore {
    /// Build a core with the default [`ClientConfig`] and the given base URL, as
    /// [`Self::with_config`] does.
    pub fn new(base_url: &str) -> Result<Self, Error<Infallible>> {
        Self::with_config(base_url, ClientConfig::default())
    }

    /// Build a core with the given base URL and configuration on a `reqwest::Client` built from
    /// `config`'s [connect](ClientConfig::connect_timeout) and
    /// [read](ClientConfig::read_timeout) timeouts, which both builds requests and, wrapped in
    /// the default [`ReqwestBackend`], executes them. Returns a request-construction error if the
    /// base URL is invalid or reqwest cannot build the client (its TLS backend fails to
    /// initialise).
    pub fn with_config(base_url: &str, config: ClientConfig) -> Result<Self, Error<Infallible>> {
        let client = http_client(&config)?;
        let backend = Arc::new(ReqwestBackend::new(client.clone()));
        Self::assemble(client, backend, base_url, config)
    }

    /// Build a core with a caller-supplied `reqwest::Client` — the injection point for TLS backend,
    /// proxies, and timeouts. The client both builds requests and, wrapped in the default
    /// [`ReqwestBackend`], executes them, so its own connect and read timeouts apply instead of
    /// [`ClientConfig::connect_timeout`] and [`ClientConfig::read_timeout`];
    /// [`ClientConfig::timeout`] still applies to each request [`crate::send`] sends.
    pub fn with_client(client: reqwest::Client, base_url: &str) -> Result<Self, Error<Infallible>> {
        let backend = Arc::new(ReqwestBackend::new(client.clone()));
        Self::assemble(client, backend, base_url, ClientConfig::default())
    }

    /// Build a core with a caller-supplied transport [`HttpBackend`] — the injection point for
    /// retry, middleware, or an entirely non-reqwest transport. Requests are still BUILT on a
    /// `reqwest::Client` configured as [`Self::new`]'s is; only the execute step goes through
    /// `backend`, so connecting and reading are bounded however `backend` bounds them.
    /// [`ClientConfig::timeout`] travels on each request [`crate::send`] sends, for a backend that
    /// runs it on a `reqwest::Client` to enforce.
    ///
    /// Each call builds that request-building `reqwest::Client`, TLS initialisation included;
    /// [`Self::with_client_and_backend`] takes one the caller already has instead.
    pub fn with_backend(
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
    ) -> Result<Self, Error<Infallible>> {
        let client = http_client(&ClientConfig::default())?;
        Self::with_client_and_backend(client, backend, base_url)
    }

    /// Build a core with both a caller-supplied `reqwest::Client`, which BUILDS requests, and a
    /// caller-supplied transport [`HttpBackend`], which EXECUTES them; nothing is built besides,
    /// so several cores can share one client and one backend. Only the execute step goes through
    /// `backend`, so connecting and reading are bounded however `backend` bounds them, and
    /// timeouts configured on `client` itself apply only where `backend` runs requests on it. The
    /// configuration is the default [`ClientConfig`]; [`ClientConfig::timeout`] travels on each
    /// request [`crate::send`] sends, as [`Self::with_backend`] describes. Returns a
    /// request-construction error if the base URL is invalid.
    pub fn with_client_and_backend(
        client: reqwest::Client,
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
    ) -> Result<Self, Error<Infallible>> {
        Self::assemble(client, backend, base_url, ClientConfig::default())
    }

    fn assemble(
        http: reqwest::Client,
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
        config: ClientConfig,
    ) -> Result<Self, Error<Infallible>> {
        let base_url = Url::parse(base_url).map_err(Error::request_construction)?;
        Ok(Self {
            http,
            backend,
            base_url,
            config,
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

    /// Replace the base URL every later request is resolved against, keeping the client, backend,
    /// configuration and credentials. Returns a request-construction error, leaving the base URL
    /// as it was, if `base_url` is invalid.
    pub fn set_base_url(&mut self, base_url: &str) -> Result<(), Error<Infallible>> {
        self.base_url = Url::parse(base_url).map_err(Error::request_construction)?;
        Ok(())
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

    /// Register a credential for a named security scheme, replacing any already registered for it.
    pub fn set_credential(&mut self, scheme: &str, credential: Credential) {
        self.credentials.insert(scheme.to_owned(), credential);
    }

    /// Unregister the credential for a named security scheme, returning it if one was registered.
    ///
    /// This is how a caller reaches a later security alternative, as [`crate::attach_auth`]
    /// describes. Removing a scheme that was never registered is a no-op returning `None`.
    pub fn remove_credential(&mut self, scheme: &str) -> Option<Credential> {
        self.credentials.remove(scheme)
    }

    /// Retrieve a registered credential by scheme name.
    pub fn credential(&self, scheme: &str) -> Option<&Credential> {
        self.credentials.get(scheme)
    }

    /// Start a [`LinkPaginator<T>`] over a `Link`-header-paginated collection, beginning at `first`.
    /// See the `paginate` module's docs for the full pattern and the authentication caveat.
    pub fn paginate_links<T>(&self, first: Url) -> LinkPaginator<T> {
        LinkPaginator::new(self.clone(), first)
    }
}

/// The `reqwest::Client` a core builds for itself, with `config`'s connect and read timeouts.
/// reqwest's `fetch` backend has neither, so on `wasm32` `config` is not read.
fn http_client(config: &ClientConfig) -> Result<reqwest::Client, Error<Infallible>> {
    let builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    let builder = {
        let mut builder = builder;
        if let Some(timeout) = config.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        if let Some(timeout) = config.read_timeout {
            builder = builder.read_timeout(timeout);
        }
        builder
    };
    #[cfg(target_arch = "wasm32")]
    let _ = config;
    builder.build().map_err(Error::request_construction)
}

#[cfg(test)]
mod tests;
