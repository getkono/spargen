//! The transport seam: how a prepared [`reqwest::Request`] is executed into a
//! [`reqwest::Response`]. Everything else — URL building, auth attachment, decode, streaming,
//! pagination — operates on the request/response *around* this one step, so swapping the backend
//! swaps only the execute step and leaves the rest of the runtime untouched.
//!
//! The seam is a `dyn`-able trait ([`HttpBackend`]) so the generated `Client` stays non-generic:
//! [`crate::ClientCore`] holds an `Arc<dyn HttpBackend>` rather than a type parameter. The async
//! method returns a manually boxed future ([`ExecuteFuture`], `Send` off wasm) instead of using
//! `async-trait`, so std's `Future`/`Pin`/`Box` carry the abstraction.
//!
//! The currency is reqwest's own types: the trait abstracts *how* a `reqwest::Request` runs, not a
//! full re-typing of requests and responses. Failures are reported as a [`crate::TransportError`]
//! wrapping the underlying `reqwest::Error`; [`crate::send`] reclassifies that error through the
//! taxonomy ([`crate::Error::from_reqwest`]) so timeout/redirect/protocol classification is
//! identical to executing directly on a `reqwest::Client`.

use reqwest::{Request, Response};

use crate::wasm::BoxFuture;
use crate::{MaybeSend, MaybeSync, TransportError};

/// The boxed future [`HttpBackend::execute`] returns: an executed [`Response`] or a
/// [`TransportError`]. The `'a` lifetime ties the future to the borrow of `&self`, so a backend may
/// borrow its own fields while the request runs (as [`ReqwestBackend`] borrows its client); a
/// `'static` future fits too, since a longer lifetime coerces to the shorter one the trait
/// requires.
///
/// The future is `Send` off `wasm32` and not on it, since reqwest's `fetch`-backed futures are
/// `!Send` on the single-threaded browser target; the `wasm` module explains the gating.
pub type ExecuteFuture<'a> = BoxFuture<'a, Result<Response, TransportError>>;

/// The swappable HTTP transport: it executes a prepared [`reqwest::Request`] into a
/// [`reqwest::Response`].
///
/// This is the seam the retry adapter, the middleware interceptor, and the WASM build all rest on:
/// implement it to wrap, replace, or retry the execute step without touching URL building, auth,
/// decode, streaming, or pagination. The default [`ReqwestBackend`] simply runs the request on a
/// wrapped [`reqwest::Client`].
///
/// Implementations must be [`MaybeSend`] + [`MaybeSync`] — `Send + Sync` on native (the `Client` is
/// shared across tasks), vacuous on `wasm32` — and `Debug` (so [`crate::ClientCore`] stays `Debug`).
/// The returned future is manually boxed so the trait is object-safe without an `async-trait`
/// dependency.
pub trait HttpBackend: MaybeSend + MaybeSync + std::fmt::Debug {
    /// Execute a prepared request, yielding the raw response or a transport failure. A backend that
    /// fails should wrap the originating `reqwest::Error` via [`TransportError::new`] so
    /// [`crate::send`] can reclassify timeouts/redirects/protocol errors into the taxonomy.
    fn execute(&self, request: Request) -> ExecuteFuture<'_>;
}

/// The default backend: executes requests on a wrapped [`reqwest::Client`]. This is what
/// `Client::new` and `Client::with_client` install.
#[derive(Debug, Clone)]
pub struct ReqwestBackend {
    client: reqwest::Client,
}

impl ReqwestBackend {
    /// Wrap a `reqwest::Client` as the transport backend.
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// The wrapped client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

impl HttpBackend for ReqwestBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        Box::pin(async move {
            self.client
                .execute(request)
                .await
                .map_err(TransportError::new)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::Method;

    use crate::tests::{poll_ready, SequenceBackend};
    use crate::{send, ClientCore, HttpBackend, ReqwestBackend};

    // On native, ClientCore and the seam trait objects/futures are Send + Sync, so the client
    // works under tokio::spawn / axum (#21). A `+ Send` boxed future or trait-object bound turned
    // non-Send would still compile in the single-threaded tests and example but break
    // multi-threaded use; this catches it at compile time. (On wasm they are !Send, hence the cfg
    // gate.)
    #[cfg(not(target_arch = "wasm32"))]
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ClientCore>();
        assert_send_sync::<Arc<dyn HttpBackend>>();
        assert_send_sync::<ReqwestBackend>();
    };

    // Object-safe: a `ReqwestBackend` installs behind `dyn HttpBackend`, as `ClientCore` stores it.
    const _: fn(ReqwestBackend) -> Arc<dyn HttpBackend> = |backend| Arc::new(backend);

    /// `send` routes the execute step through the installed backend rather than a hard-wired
    /// `reqwest::Client`: the backend sees the request and its response is what `send` returns.
    #[test]
    fn send_routes_through_the_installed_backend() {
        let backend = SequenceBackend::statuses([203]);
        let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
        // Requests are still BUILT on a reqwest client; only the execute step goes through the seam.
        let request = core
            .http()
            .request(Method::GET, "https://example.com/op")
            .build()
            .unwrap();
        let response = poll_ready(send(&core, request)).unwrap();
        assert_eq!(response.status().as_u16(), 203);
        assert_eq!(backend.urls(), ["https://example.com/op"]);
    }

    /// `ClientCore::new` installs a `ReqwestBackend`, which `ClientCore`'s `Debug` names, since
    /// `Debug` is required of every backend.
    #[test]
    fn new_installs_a_reqwest_backend_that_debug_names() {
        let rendered = format!("{:?}", ReqwestBackend::new(reqwest::Client::new()));
        assert!(
            rendered.starts_with("ReqwestBackend { client: Client {"),
            "{rendered}"
        );
        let core = format!("{:?}", ClientCore::new("https://example.com").unwrap());
        assert!(
            core.contains("backend: ReqwestBackend { client: Client {"),
            "{core}"
        );
    }

    /// `client` hands back the wrapped client, not a default one: it still holds the default
    /// header the wrapped client was configured with, which `reqwest::Client`'s `Debug` renders.
    #[test]
    fn client_returns_the_wrapped_client() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-wrapped",
            reqwest::header::HeaderValue::from_static("yes"),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        let backend = ReqwestBackend::new(client);
        let rendered = format!("{:?}", backend.client());
        assert!(rendered.contains("x-wrapped"), "{rendered}");
        assert!(
            !format!("{:?}", reqwest::Client::default()).contains("x-wrapped"),
            "a default client carries no such header"
        );
    }
}
