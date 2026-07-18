//! The transport seam: how a prepared [`reqwest::Request`] is executed into a
//! [`reqwest::Response`]. Everything else — URL building, auth attachment, decode, streaming,
//! pagination — operates on the request/response *around* this one step, so swapping the backend
//! swaps only the execute step and leaves the rest of the runtime untouched.
//!
//! The seam is a `dyn`-able trait ([`HttpBackend`]) so the generated `Client` stays non-generic:
//! [`crate::ClientCore`] holds an `Arc<dyn HttpBackend>` rather than a type parameter. The async
//! method returns a manually boxed future (`Pin<Box<dyn Future + Send + '_>>`) instead of using
//! `async-trait`, so the runtime's dependency set stays exactly reqwest/serde/serde_json/bytes/
//! secrecy — std's `Future`/`Pin`/`Box` carry the abstraction.
//!
//! The currency is reqwest's own types: the trait abstracts *how* a `reqwest::Request` runs, not a
//! full re-typing of requests and responses. Failures are reported as a [`crate::TransportError`]
//! wrapping the underlying `reqwest::Error`; [`crate::send`] reclassifies that error through the
//! taxonomy ([`crate::Error::from_reqwest`]) so timeout/redirect/protocol classification is
//! identical to executing directly on a `reqwest::Client`.

use std::future::Future;
use std::pin::Pin;

use reqwest::{Request, Response};

use crate::{MaybeSend, MaybeSync, TransportError};

/// The boxed future [`HttpBackend::execute`] returns: an executed [`Response`] or a
/// [`TransportError`]. The `'a` lifetime ties the future to the borrow of `&self`, but a backend is
/// free to return a `'static` future (as [`ReqwestBackend`] does) — a longer lifetime coerces to
/// the shorter one the trait requires.
///
/// The future is `Send` on native (a backend is shared across tasks) but not on `wasm32`, where
/// reqwest's `fetch`-backed futures are `!Send`. `Send` is an auto trait and so cannot be swapped
/// for the non-auto [`MaybeSend`] as an extra trait-object bound; the alias is `cfg`-gated instead.
#[cfg(not(target_arch = "wasm32"))]
pub type ExecuteFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Response, TransportError>> + Send + 'a>>;
/// The boxed future [`HttpBackend::execute`] returns (the wasm variant: no `Send`, since reqwest's
/// `fetch`-backed futures are `!Send` on the single-threaded browser target).
#[cfg(target_arch = "wasm32")]
pub type ExecuteFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, TransportError>> + 'a>>;

/// The swappable HTTP transport: it executes a prepared [`reqwest::Request`] into a
/// [`reqwest::Response`].
///
/// This is the seam #17 (retry), #20 (middleware), and #21 (WASM) build on: implement it to wrap,
/// replace, or retry the execute step without touching URL building, auth, decode, streaming, or
/// pagination. The default [`ReqwestBackend`] simply runs the request on a wrapped
/// [`reqwest::Client`].
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
/// `Client::new` and `Client::with_client` install, so the out-of-the-box behavior is unchanged.
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
        // Clone the (cheaply Arc-backed) client so the returned future is self-contained (`'static`)
        // and free of any borrow of `&self`; this keeps the seam trivially object-safe.
        let client = self.client.clone();
        Box::pin(async move { client.execute(request).await.map_err(TransportError::new) })
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use reqwest::{Method, Request};

    use crate::{send, ClientCore, HttpBackend, ReqwestBackend};

    // Regression guard for the #21 conditional-Send refactor: on native, ClientCore and the seam
    // trait objects/futures MUST stay Send + Sync so the client works under tokio::spawn / axum. A
    // future change that turned a `+ Send` boxed future or a trait-object bound into a non-Send
    // form would still compile in the single-threaded tests/example but break multi-threaded use;
    // this catches it at compile time. (On wasm these are intentionally !Send, hence the cfg gate.)
    #[cfg(not(target_arch = "wasm32"))]
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ClientCore>();
        assert_send_sync::<Arc<dyn HttpBackend>>();
        assert_send_sync::<ReqwestBackend>();
    };

    use super::ExecuteFuture;

    /// The backend paths here never actually suspend on a socket (the canned response is ready
    /// immediately), so a single poll with a noop waker is enough — no async runtime needed.
    fn poll_ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future was not immediately ready"),
        }
    }

    /// A custom backend that records the request URL it was handed and returns a canned in-memory
    /// `reqwest::Response` synthesized without a socket — proving `send` routes the execute step
    /// through the installed backend rather than a hard-wired `reqwest::Client`.
    #[derive(Debug, Default)]
    struct RecordingBackend {
        seen: Mutex<Option<String>>,
    }

    impl HttpBackend for RecordingBackend {
        fn execute(&self, request: Request) -> ExecuteFuture<'_> {
            *self.seen.lock().expect("mutex not poisoned") = Some(request.url().to_string());
            Box::pin(async {
                Ok(reqwest::Response::from(
                    http::Response::builder()
                        .status(200)
                        .body(r#"{"ok":true}"#.to_owned())
                        .expect("valid synthetic response"),
                ))
            })
        }
    }

    #[test]
    fn send_routes_through_the_installed_backend() {
        let backend = Arc::new(RecordingBackend::default());
        let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
        // Requests are still BUILT on a reqwest client; only the execute step goes through the seam.
        let request = core
            .http()
            .request(Method::GET, "https://example.com/op")
            .build()
            .unwrap();
        let response = poll_ready(send(&core, request)).unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        // The swapped backend actually saw the request, so dispatch used it, not a default client.
        assert_eq!(
            backend.seen.lock().unwrap().as_deref(),
            Some("https://example.com/op")
        );
    }

    #[test]
    fn default_reqwest_backend_constructs_and_is_object_safe() {
        let backend = ReqwestBackend::new(reqwest::Client::new());
        // Object-safe: usable behind `dyn` exactly as `ClientCore` stores it.
        let _erased: Arc<dyn HttpBackend> = Arc::new(backend);
        // `new` / `with_client` install a default `ReqwestBackend` under the hood.
        let _core = ClientCore::new("https://example.com").unwrap();
    }
}
