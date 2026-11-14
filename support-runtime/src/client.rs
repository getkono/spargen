use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

#[cfg(not(target_arch = "wasm32"))]
use crate::error::CrossOriginRedirect;
#[cfg(not(target_arch = "wasm32"))]
use crate::paginate::{same_origin, without_credentials};
use crate::{
    Credential, Error, ExposeSecret, HttpBackend, LinkPaginator, ReqwestBackend, SecretString,
};

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
    /// How long one non-streaming request may take in total, from connecting until its whole
    /// response body has been read, before it fails with [`Error::Timeout`] (default `None`: no
    /// total bound, only the connect and read timeouts).
    ///
    /// It is read each time the runtime's `send` sends a request, so changing it through
    /// [`ClientCore::config_mut`] applies from the next request on, whichever constructor built
    /// the core, and it is honoured on `wasm32` too. A generated client's `with_timeout` sets it
    /// on the client value it returns, which is how one call gets a deadline of its own. It
    /// travels on the request itself, as a [`crate::TotalTimeout`] extension: a request that
    /// already carries one keeps it, as does one given one by [`RequestOptions::extensions`],
    /// which `send` applies first, [`ReqwestBackend`] enforces it, and so does a caller-supplied
    /// [`HttpBackend`] that delegates to a `ReqwestBackend` or reads the extension itself. Behind
    /// a [`crate::RetryBackend`] it bounds each attempt, not the retried call as a whole. A call
    /// replayed after a refreshable credential was rejected and refreshed sends the replay with a
    /// `TotalTimeout` of its own, so the call as a whole may take this long for the first
    /// exchange, the refresh as long as it takes, and this long again for the replay. A
    /// streaming response is exempt,
    /// because a stream may rightly stay open indefinitely: an operation returning an
    /// `EventStream` sends its request with the runtime's `send_streaming`, which never applies
    /// it, and each reconnect is sent the same way.
    pub timeout: Option<Duration>,
    /// Maximum bytes of a success body a non-streaming operation reads before it fails with
    /// [`Error::BodyTooLarge`] (default `None`: a structured success body is read whole, however
    /// large, and a raw binary one up to [`Self::DEFAULT_MAX_BINARY_BODY`]).
    ///
    /// A response whose `Content-Length` declares more is refused before any of its body is read,
    /// and one that grows past it is abandoned at the chunk that crosses it, which forgoes reuse
    /// of that connection; peak memory is the limit, not what the server chose to send. The
    /// answer to a `HEAD` request and a `1xx`, `204` or `304` response carry no body, so their
    /// `Content-Length`, which describes a representation they do not carry, refuses nothing;
    /// whatever body they do carry is bounded as it is read.
    /// [`RequestOptions::max_body`], set per client value with a generated client's
    /// `with_options`, replaces it. It is read each time the runtime's `send` sends a request, as
    /// [`Self::timeout`] is. An operation returning an `EventStream` is exempt: its items are
    /// bounded by [`Self::max_stream_frame`] instead. So is each raw binary body a `_stream` twin
    /// returns as a [`crate::ByteStream`], whose reader picks its own limit; any other success body
    /// the twin of a success enum reads is read within this one.
    pub max_success_body: Option<usize>,
}

impl ClientConfig {
    /// The most bytes a raw binary success body (`application/octet-stream`, `image/*`, …) read
    /// whole is allowed to hold where neither [`RequestOptions::max_body`] nor
    /// [`Self::max_success_body`] sets a limit: 64 MiB. Either of those replaces it, larger or
    /// smaller; a body past it fails with [`Error::BodyTooLarge`]. Every operation whose success
    /// holds a raw binary body, as its one success body or as a status of its success enum, has a
    /// `_stream` twin, which hands that body over unread as a [`crate::ByteStream`] instead: a
    /// download this limit would refuse is read through it.
    pub const DEFAULT_MAX_BINARY_BODY: usize = 64 * 1024 * 1024;
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_error_body: 64 * 1024,
            max_stream_frame: 16 * 1024 * 1024,
            timeout: None,
            max_success_body: None,
        }
    }
}

/// How the `reqwest::Client` a core's default backend runs on bounds connecting and reading.
///
/// It is construction-only: [`ClientCore::with_config`] takes it by value and builds the client
/// from it, and [`ClientCore::new`] builds one from the default. A client injected with
/// [`ClientCore::with_client`] keeps its own timeouts instead, and a backend installed with
/// [`ClientCore::with_backend`] executes requests on its own terms. reqwest's `fetch` backend has
/// neither timeout, so on `wasm32` both are ignored.
///
/// The client built from it follows a redirect only within the origin of the request (the same
/// scheme, host and port), up to ten hops, and refuses one to another origin with
/// [`Error::Redirect`], whose [`crate::RedirectError::cross_origin_target`] names the target
/// without its userinfo and query; nothing is sent there. On `wasm32` the browser's fetch follows
/// redirects by its own policy instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConfig {
    /// How long establishing a connection may take — name resolution, the TCP handshake and TLS —
    /// before the request fails with [`Error::Timeout`] (default 30 s; `None` waits as long as
    /// the operating system does).
    pub connect_timeout: Option<Duration>,
    /// How long a read on an established connection may wait for its next bytes — the response
    /// head, or the next chunk of a body, a streamed one included — before the request fails with
    /// [`Error::Timeout`] (default 5 min; `None` waits indefinitely). Each read that arrives
    /// restarts it, so it bounds a stall, not the length of a response. An event stream whose
    /// server sends nothing, not even a comment or heartbeat, for longer than this fails with
    /// [`Error::Timeout`], which a reconnect policy may answer by resuming it.
    pub read_timeout: Option<Duration>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Some(Duration::from_secs(30)),
            read_timeout: Some(Duration::from_secs(5 * 60)),
        }
    }
}

/// Options every call made through one client value carries: a generated client's `with_options`
/// sets them on the client value it returns, so a call that needs its own derives a client for it
/// (`client.clone().with_options(options)`) rather than sharing mutable state with other calls.
///
/// They are read each time the runtime's `send` or `send_streaming` sends a request, after the
/// request is built and its credentials attached.
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    /// Maximum bytes of a response body a non-streaming operation reads before it fails with
    /// [`Error::BodyTooLarge`]; `None` (the default) leaves [`ClientConfig::max_success_body`] in
    /// force.
    ///
    /// On a success body it replaces [`ClientConfig::max_success_body`], and bounds the read as
    /// that field describes: a `Content-Length` past it is refused before the body is read (unless
    /// the response carries no body, as that field says), and a
    /// body that grows past it is abandoned at the chunk that crosses it. An error body is read no
    /// further than [`ClientConfig::max_error_body`] in any case; where this limit is the smaller
    /// of the two, an error body past it fails with [`Error::BodyTooLarge`] the same way, instead
    /// of being truncated. An operation returning an `EventStream` is exempt, success and error
    /// body alike: [`ClientConfig::max_stream_frame`] bounds its items.
    pub max_body: Option<usize>,
    /// Extensions copied onto each request this client value sends, where an
    /// [`HttpBackend`] reads them from the `http::Request` it executes. An extension of a type the
    /// request already carries is left as the request has it. A [`crate::TotalTimeout`] here is
    /// the call's deadline: it replaces [`ClientConfig::timeout`] on a non-streaming operation,
    /// and, being asked for explicitly, also bounds an operation returning an `EventStream`,
    /// which the configured timeout leaves unbounded.
    pub extensions: http::Extensions,
    /// Headers added to each request this client value sends, every operation's included. A
    /// header the request already carries — an operation's own header parameter, its content
    /// type, or an attached credential — is left as the request has it, and every value of a name
    /// the request lacks is added. `ClientCore`'s `Debug` prints them; mark a secret one with
    /// `HeaderValue::set_sensitive` to redact it there.
    pub headers: http::HeaderMap,
}

/// The shared core every generated `Client` wraps: the swappable transport [`HttpBackend`] that
/// executes requests, the base URL, configuration, and per-scheme credentials.
///
/// Requests are built by the runtime itself, as an `http::Request`, whichever backend executes
/// them; the runtime's `send` hands each one to the backend. `new`/`with_config` install the
/// default [`ReqwestBackend`] on a `reqwest::Client` built from the configuration,
/// `with_client` installs one on a caller-supplied `reqwest::Client`, and `with_backend` plugs a
/// caller-supplied transport.
///
/// The generated `Client` wraps one of these and documents its own constructors; each of its
/// operation methods delegates to the shared dispatch routines.
#[derive(Debug, Clone)]
pub struct ClientCore {
    backend: Arc<dyn HttpBackend>,
    base_url: Url,
    config: ClientConfig,
    options: RequestOptions,
    credentials: HashMap<String, Credential>,
    /// Bound header values, keyed by the header name in ASCII lowercase. Held as `SecretString`s,
    /// as credentials' secrets are, so `Debug` prints each bound name and redacts its value.
    bound_headers: HashMap<String, SecretString>,
}

impl ClientCore {
    /// Build a core with the default [`ClientConfig`] and [`TransportConfig`] and the given base
    /// URL, as [`Self::with_config`] does.
    pub fn new(base_url: &str) -> Result<Self, Error<Infallible>> {
        Self::with_config(
            base_url,
            ClientConfig::default(),
            TransportConfig::default(),
        )
    }

    /// Build a core with the given base URL and configuration, executing requests on the default
    /// [`ReqwestBackend`] over a `reqwest::Client` built from `transport`'s
    /// [connect](TransportConfig::connect_timeout) and [read](TransportConfig::read_timeout)
    /// timeouts. Returns a request-construction error if the base URL is not an `http` or `https`
    /// URL, or reqwest cannot build the client (its TLS backend fails to initialise).
    pub fn with_config(
        base_url: &str,
        config: ClientConfig,
        transport: TransportConfig,
    ) -> Result<Self, Error<Infallible>> {
        let base_url = parse_base_url(base_url)?;
        let backend = Arc::new(ReqwestBackend::new(http_client(&transport)?));
        Ok(Self::assemble(backend, base_url, config))
    }

    /// Build a core executing requests on a caller-supplied `reqwest::Client` — the injection
    /// point for TLS backend, proxies, and timeouts — wrapped in the default [`ReqwestBackend`],
    /// as [`Self::with_backend`] would with `Arc::new(ReqwestBackend::new(client))`. The client's
    /// own connect and read timeouts apply, and no [`TransportConfig`] is read;
    /// [`ClientConfig::timeout`] still applies to each request the runtime's `send` sends.
    ///
    /// The client keeps its own redirect policy too, not the same-origin one [`TransportConfig`]
    /// describes: under reqwest's default it follows a redirect to any origin, and every header
    /// reqwest does not strip on a cross-origin hop (an `apiKey` header, a bound header, a
    /// [`RequestOptions::headers`] entry) travels with it.
    pub fn with_client(client: reqwest::Client, base_url: &str) -> Result<Self, Error<Infallible>> {
        Self::with_backend(Arc::new(ReqwestBackend::new(client)), base_url)
    }

    /// Build a core with a caller-supplied transport [`HttpBackend`] — the injection point for
    /// retry, middleware, or an entirely non-reqwest transport — and the default
    /// [`ClientConfig`]; nothing else is built, so several cores can share one backend. Every
    /// request is executed by `backend`, so connecting and reading are bounded however `backend`
    /// bounds them. [`ClientConfig::timeout`] travels on each request the runtime's `send` sends,
    /// as a [`crate::TotalTimeout`] extension, for `backend` to enforce. Redirects are followed,
    /// or not, as `backend` decides, not by the same-origin policy [`TransportConfig`] describes.
    /// Returns a request-construction error if the base URL is not an `http` or `https` URL.
    pub fn with_backend(
        backend: Arc<dyn HttpBackend>,
        base_url: &str,
    ) -> Result<Self, Error<Infallible>> {
        let base_url = parse_base_url(base_url)?;
        Ok(Self::assemble(backend, base_url, ClientConfig::default()))
    }

    fn assemble(backend: Arc<dyn HttpBackend>, base_url: Url, config: ClientConfig) -> Self {
        Self {
            backend,
            base_url,
            config,
            options: RequestOptions::default(),
            credentials: HashMap::new(),
            bound_headers: HashMap::new(),
        }
    }

    /// The retention/config settings.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Mutably borrow the retention/config settings.
    pub fn config_mut(&mut self) -> &mut ClientConfig {
        &mut self.config
    }

    /// The options every request this core sends carries (the default ones until
    /// [`Self::set_options`] replaces them).
    pub fn options(&self) -> &RequestOptions {
        &self.options
    }

    /// Replace the options every later request this core sends carries.
    pub fn set_options(&mut self, options: RequestOptions) {
        self.options = options;
    }

    /// The base URL.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Replace the base URL every later request is resolved against, keeping the backend,
    /// configuration and credentials. Returns a request-construction error, leaving the base URL
    /// as it was, if `base_url` is not an `http` or `https` URL.
    pub fn set_base_url(&mut self, base_url: &str) -> Result<(), Error<Infallible>> {
        self.base_url = parse_base_url(base_url)?;
        Ok(())
    }

    /// The transport backend that executes requests. The runtime's `send` dispatches through this;
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
    /// This is how a caller reaches a later security alternative: an operation attaches the first
    /// alternative whose schemes all have a registered credential, so removing a scheme of an
    /// earlier one lets the next call pass over it. Removing a scheme that was never registered is
    /// a no-op returning `None`.
    pub fn remove_credential(&mut self, scheme: &str) -> Option<Credential> {
        self.credentials.remove(scheme)
    }

    /// Retrieve a registered credential by scheme name.
    pub fn credential(&self, scheme: &str) -> Option<&Credential> {
        self.credentials.get(scheme)
    }

    /// Bind `value` to the header `name`, matched in any letter case, replacing any value already
    /// bound to it. An operation of a generated client whose header parameter the build bound
    /// (`Spec::bind_header`) sends this value, as written, instead of taking an argument for it.
    /// `ClientCore`'s `Debug` prints the bound name and redacts the value, and the operation sends
    /// it marked sensitive (`HeaderValue::set_sensitive`), so the request's `Debug` redacts it
    /// too.
    ///
    /// Returns a request-construction error, leaving any value already bound to `name` in place,
    /// if `value` is not a valid header value: it holds a control character other than a tab,
    /// such as CR, LF, NUL or DEL. The error does not quote the value.
    pub fn bind_header(
        &mut self,
        name: &str,
        value: impl Into<String>,
    ) -> Result<(), Error<Infallible>> {
        let value = value.into();
        if http::HeaderValue::from_str(&value).is_err() {
            return Err(Error::request_message(format!(
                "the value bound to the header `{}` is not a valid header value",
                name.escape_debug()
            )));
        }
        self.bound_headers
            .insert(name.to_ascii_lowercase(), SecretString::from(value));
        Ok(())
    }

    /// Unbind the header `name`, matched in any letter case, returning the value bound to it if
    /// there was one. An operation that requires the header then fails with
    /// [`crate::RequestError::MissingBoundParameter`] before anything is sent.
    pub fn unbind_header(&mut self, name: &str) -> Option<String> {
        self.bound_headers
            .remove(&name.to_ascii_lowercase())
            .map(|value| value.expose_secret().to_owned())
    }

    /// The value bound to the header `name`, matched in any letter case.
    pub fn bound_header(&self, name: &str) -> Option<&str> {
        self.bound_headers
            .get(&name.to_ascii_lowercase())
            .map(ExposeSecret::expose_secret)
    }

    /// Start a [`LinkPaginator<T>`] over a `Link`-header-paginated collection, beginning at `first`.
    /// See the `paginate` module's docs for the full pattern and the authentication caveat.
    pub fn paginate_links<T>(&self, first: Url) -> LinkPaginator<T> {
        LinkPaginator::new(self.clone(), first)
    }
}

/// Parse `base_url` as a base URL: refused, as a request-construction error, unless it is an
/// absolute `http` or `https` URL.
fn parse_base_url(base_url: &str) -> Result<Url, Error<Infallible>> {
    let url = Url::parse(base_url).map_err(Error::request_construction)?;
    require_http_base(url)
}

/// `url` itself if its scheme is `http` or `https`, which the URL Standard parses with a host and
/// a path every request path can be joined onto; any other scheme (`mailto:x`, or `localhost` in
/// `localhost:8080`) is refused as a request-construction error naming it. The message carries
/// the scheme alone, never the URL, which may hold userinfo.
pub(crate) fn require_http_base(url: Url) -> Result<Url, Error<Infallible>> {
    match url.scheme() {
        "http" | "https" => Ok(url),
        scheme => Err(Error::request_message(format!(
            "a base URL must be an `http` or `https` URL, not one with the scheme `{scheme}`"
        ))),
    }
}

/// The `reqwest::Client` a core's default backend runs on, with `config`'s connect and read
/// timeouts and the same-origin redirect policy [`redirect_verdict`] decides.
/// reqwest's `fetch` backend has none of the three, so on `wasm32` `config` is not read and the
/// browser follows redirects as its own fetch policy decides.
fn http_client(config: &TransportConfig) -> Result<reqwest::Client, Error<Infallible>> {
    let builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    let builder = {
        let mut builder =
            builder.redirect(reqwest::redirect::Policy::custom(
                |attempt| match redirect_verdict(attempt.url(), attempt.previous()) {
                    Ok(()) => attempt.follow(),
                    Err(refusal) => attempt.error(refusal),
                },
            ));
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

/// How many redirects the default backend follows for one request, as reqwest's default policy
/// does.
#[cfg(not(target_arch = "wasm32"))]
const MAX_REDIRECTS: usize = 10;

/// Whether the default backend follows a redirect to `next`, `previous` being every URL already
/// requested in the chain, the request's own first. It follows one only within the origin of the
/// request it was sent as (the same scheme, host and port), since every header the request carries
/// travels with the hop: an `apiKey` credential, a bound header, a `RequestOptions` header. A hop
/// to another origin is refused with a [`CrossOriginRedirect`] naming the target without its
/// userinfo and query, and a hop past the [`MAX_REDIRECTS`]th as too many redirects.
#[cfg(not(target_arch = "wasm32"))]
fn redirect_verdict(
    next: &Url,
    previous: &[Url],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(origin) = previous.first() {
        if !same_origin(origin, next) {
            return Err(Box::new(CrossOriginRedirect {
                target: without_credentials(next.clone()),
            }));
        }
    }
    if previous.len() > MAX_REDIRECTS {
        return Err(format!("too many redirects (more than {MAX_REDIRECTS})").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
