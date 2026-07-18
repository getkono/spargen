//! Interceptor middleware for the transport seam: [`MiddlewareBackend`] wraps any inner
//! [`HttpBackend`] with an ordered chain of [`Middleware`] that can observe/modify a request before
//! it is sent and the response after, short-circuit before the transport runs, and do async work
//! around the call.
//!
//! ## The `Next` continuation (tower-like, no dependencies)
//!
//! A middleware receives the request plus a [`Next`] handle. Calling [`Next::run`] advances to the
//! rest of the chain — the next middleware, or, when the chain is exhausted, the inner
//! [`HttpBackend`]. A middleware may inspect/modify the request before calling `run`, inspect the
//! [`Response`] after, or return a response *without* calling `run` at all to short-circuit. This is
//! the classic "onion" middleware shape, expressed with std's `Future`/`Pin`/`Box` — no
//! `async-trait`, no `tower`, no `futures`, so the runtime's dependency set stays exactly
//! reqwest/serde/serde_json/bytes/secrecy.
//!
//! ### How the lifetimes work without `async-trait`
//!
//! [`Middleware::handle`] is `handle<'a>(&'a self, request, next: Next<'a>) -> …Future… + 'a`: the
//! `'a` ties the borrow of the middleware, the [`Next`] it is handed, and the boxed future it
//! returns to a single lifetime. [`Next<'a>`] holds only borrows — `&'a [Arc<dyn Middleware>]` for
//! the remaining chain and `&'a Arc<dyn HttpBackend>` for the inner transport — so advancing the
//! chain never clones or reallocates; it just narrows the slice. [`MiddlewareBackend::execute`]
//! returns [`ExecuteFuture<'_>`], whose `'_` is exactly the borrow of `&self` the [`Next`] is built
//! over, so the whole chain's future safely borrows the (Arc-held) middleware and backend for the
//! duration of the call. On native targets `Arc<dyn Middleware>`/`Arc<dyn HttpBackend>` are
//! `Send + Sync` (via the `MaybeSend`/`MaybeSync` supertraits, which collapse to `Send`/`Sync`
//! off wasm), so references to them are `Send` and the boxed futures stay `Send`; on `wasm32`
//! these bounds are relaxed to match reqwest's single-threaded `fetch` backend.
//!
//! ## Ordering
//!
//! The first middleware in the vector runs **outermost**: it sees the request first and the response
//! last. The last middleware runs innermost, closest to the transport. So
//! `MiddlewareBackend::new(inner).layer(a).layer(b)` runs `a` around `b` around `inner`.
//!
//! ## Composability
//!
//! [`MiddlewareBackend`] wraps *any* [`HttpBackend`], so it composes freely with the retry adapter
//! ([`crate::RetryBackend`]) and the default [`crate::ReqwestBackend`] in either order. Wrapping a
//! `RetryBackend` in a `MiddlewareBackend` runs the middleware **once** around the whole retry loop;
//! wrapping a `MiddlewareBackend` in a `RetryBackend` re-runs the middleware **per attempt**. Pick
//! the nesting that matches whether an interceptor should observe each attempt or just the final
//! outcome.
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! let inner: Arc<dyn HttpBackend> = Arc::new(ReqwestBackend::new(reqwest::Client::new()));
//! let backend: Arc<dyn HttpBackend> = Arc::new(
//!     MiddlewareBackend::new(inner).layer(Arc::new(MyLogger)),
//! );
//! let client = Client::with_backend(backend, "https://api.example.com")?;
//! ```

use std::sync::Arc;

use reqwest::Request;

use crate::transport::{ExecuteFuture, HttpBackend};
use crate::{MaybeSend, MaybeSync};

/// One link in the interceptor chain: it observes/modifies a request on the way in, calls
/// [`Next::run`] to proceed (eventually reaching the transport), and observes/modifies the
/// [`reqwest::Response`] on the way out — or returns early to short-circuit.
///
/// Implementations must be [`MaybeSend`] + [`MaybeSync`] — `Send + Sync` on native (the `Client` is
/// shared across tasks), vacuous on `wasm32` — and `Debug` (so [`MiddlewareBackend`] — and thus
/// [`crate::ClientCore`] — stays `Debug`). The returned future is manually boxed so the trait is
/// object-safe without an `async-trait` dependency.
pub trait Middleware: MaybeSend + MaybeSync + std::fmt::Debug {
    /// Handle a request. Call `next.run(request)` to proceed to the rest of the chain (eventually
    /// the transport), or return a response without calling it to short-circuit. May inspect/modify
    /// the request before and the [`reqwest::Response`] after.
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> ExecuteFuture<'a>;
}

/// The continuation handed to a [`Middleware`]: it represents "the rest of the chain" — the
/// remaining middlewares followed by the inner transport.
///
/// Holds only borrows (`'a`) into the [`MiddlewareBackend`], so advancing the chain is allocation-
/// free. Call [`Next::run`] to proceed; dropping a `Next` without calling `run` is how a middleware
/// short-circuits.
pub struct Next<'a> {
    /// The middlewares that still have to run, in order; the first is the next to run.
    remaining: &'a [Arc<dyn Middleware>],
    /// The transport reached once `remaining` is empty.
    inner: &'a Arc<dyn HttpBackend>,
}

impl<'a> Next<'a> {
    /// Proceed to the rest of the chain: run the next middleware, or — when no middlewares remain —
    /// execute the request on the inner transport. Returns the eventual [`reqwest::Response`] or the
    /// [`crate::TransportError`] the transport failed with.
    pub fn run(self, request: Request) -> ExecuteFuture<'a> {
        match self.remaining.split_first() {
            // Peel off the outermost remaining middleware and hand it a `Next` over the rest.
            Some((first, rest)) => first.handle(
                request,
                Next {
                    remaining: rest,
                    inner: self.inner,
                },
            ),
            // Chain exhausted: hand the request to the transport. `execute` borrows `*inner` for
            // `'a`, so the future it returns is `'a` — exactly what this method promises.
            None => self.inner.execute(request),
        }
    }
}

/// An [`HttpBackend`] that runs an ordered chain of [`Middleware`] around an inner backend.
///
/// The first middleware in the chain runs outermost (see the [module docs](self) for ordering and
/// composability). Purely additive: it takes effect only when a caller wraps their backend in it and
/// installs it via `Client::with_backend`.
#[derive(Clone)]
pub struct MiddlewareBackend {
    inner: Arc<dyn HttpBackend>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareBackend {
    /// Start a chain around `inner` with no middlewares; add them with [`Self::layer`].
    pub fn new(inner: Arc<dyn HttpBackend>) -> Self {
        Self {
            inner,
            middlewares: Vec::new(),
        }
    }

    /// Wrap `inner` with a pre-built ordered list of middlewares (index 0 runs outermost).
    pub fn with_middlewares(
        inner: Arc<dyn HttpBackend>,
        middlewares: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self { inner, middlewares }
    }

    /// Append a middleware to the chain and return `self`, for builder-style construction. Each
    /// appended middleware runs *inside* the ones added before it (the first added is outermost).
    #[must_use]
    pub fn layer(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// The wrapped inner backend.
    pub fn inner(&self) -> &Arc<dyn HttpBackend> {
        &self.inner
    }

    /// The middleware chain, in run order (index 0 is outermost).
    pub fn middlewares(&self) -> &[Arc<dyn Middleware>] {
        &self.middlewares
    }
}

// `Middleware: Debug`, so the chain is `Debug`; a manual impl keeps the field names tidy and matches
// `RetryBackend`'s non-exhaustive style.
impl std::fmt::Debug for MiddlewareBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareBackend")
            .field("inner", &self.inner)
            .field("middlewares", &self.middlewares)
            .finish()
    }
}

impl HttpBackend for MiddlewareBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        // Build a `Next` over the full chain and run it. The returned future borrows `&self` for the
        // `'_` of `execute`, which is exactly the `'a` the `Next` (and every middleware future in the
        // chain) is tied to — no `Arc` clones needed to satisfy the borrow checker.
        Next {
            remaining: &self.middlewares,
            inner: &self.inner,
        }
        .run(request)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use reqwest::header::{HeaderName, HeaderValue};
    use reqwest::{Method, Request, Response, StatusCode};

    use crate::transport::{ExecuteFuture, HttpBackend};
    use crate::RetryBackend;

    use super::{Middleware, MiddlewareBackend, Next};

    /// The chain never actually suspends on a socket here: the inner backend's canned response is
    /// ready immediately, so one poll with a noop waker drives the whole thing — no async runtime.
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

    fn canned(status: u16) -> Response {
        Response::from(
            http::Response::builder()
                .status(status)
                .body(String::new())
                .expect("valid synthetic response"),
        )
    }

    fn request() -> Request {
        reqwest::Client::new()
            .request(Method::GET, "https://example.com/op")
            .build()
            .expect("build request")
    }

    /// An inner backend that records the headers it was handed and counts executions, returning a
    /// canned `200` synthesized without a socket.
    #[derive(Debug, Default)]
    struct RecordingBackend {
        seen_headers: Mutex<Option<reqwest::header::HeaderMap>>,
        calls: AtomicU32,
    }

    impl RecordingBackend {
        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }

        fn seen_header(&self, name: &str) -> Option<String> {
            self.seen_headers
                .lock()
                .expect("mutex not poisoned")
                .as_ref()
                .and_then(|headers| headers.get(name).cloned())
                .map(|value| value.to_str().expect("ascii header").to_owned())
        }
    }

    impl HttpBackend for RecordingBackend {
        fn execute(&self, request: Request) -> ExecuteFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.seen_headers.lock().expect("mutex not poisoned") =
                Some(request.headers().clone());
            Box::pin(async { Ok(canned(200)) })
        }
    }

    /// A middleware that MODIFIES the outgoing request by inserting a header before proceeding.
    #[derive(Debug)]
    struct InsertHeader {
        name: HeaderName,
        value: HeaderValue,
    }

    impl Middleware for InsertHeader {
        fn handle<'a>(&'a self, mut request: Request, next: Next<'a>) -> ExecuteFuture<'a> {
            request
                .headers_mut()
                .insert(self.name.clone(), self.value.clone());
            next.run(request)
        }
    }

    /// A middleware that OBSERVES/records the response status on the way out.
    #[derive(Debug)]
    struct ObserveStatus {
        seen: Arc<Mutex<Option<StatusCode>>>,
    }

    impl Middleware for ObserveStatus {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> ExecuteFuture<'a> {
            let seen = self.seen.clone();
            Box::pin(async move {
                let response = next.run(request).await?;
                *seen.lock().expect("mutex not poisoned") = Some(response.status());
                Ok(response)
            })
        }
    }

    /// A middleware that SHORT-CIRCUITS: it returns a canned response WITHOUT calling `next`, so the
    /// rest of the chain (and the transport) never runs.
    #[derive(Debug)]
    struct ShortCircuit {
        status: u16,
    }

    impl Middleware for ShortCircuit {
        fn handle<'a>(&'a self, _request: Request, _next: Next<'a>) -> ExecuteFuture<'a> {
            let status = self.status;
            Box::pin(async move { Ok(canned(status)) })
        }
    }

    /// A middleware that appends `"<label>:before"`/`"<label>:after"` markers around `next`, so a
    /// shared log reveals the nesting order of a composed chain.
    #[derive(Debug)]
    struct Tracer {
        label: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Middleware for Tracer {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> ExecuteFuture<'a> {
            let log = self.log.clone();
            let label = self.label;
            Box::pin(async move {
                log.lock()
                    .expect("mutex not poisoned")
                    .push(format!("{label}:before"));
                let result = next.run(request).await;
                log.lock()
                    .expect("mutex not poisoned")
                    .push(format!("{label}:after"));
                result
            })
        }
    }

    #[test]
    fn middleware_modifies_outgoing_request_seen_by_inner_backend() {
        let inner = Arc::new(RecordingBackend::default());
        let backend = MiddlewareBackend::new(inner.clone()).layer(Arc::new(InsertHeader {
            name: HeaderName::from_static("x-trace"),
            value: HeaderValue::from_static("on"),
        }));
        let response = poll_ready(backend.execute(request())).expect("chain succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        // The inner backend actually received the header the middleware inserted.
        assert_eq!(inner.seen_header("x-trace").as_deref(), Some("on"));
        assert_eq!(inner.calls(), 1);
    }

    #[test]
    fn middleware_observes_response_after_transport() {
        let inner = Arc::new(RecordingBackend::default());
        let seen = Arc::new(Mutex::new(None));
        let backend =
            MiddlewareBackend::new(inner).layer(Arc::new(ObserveStatus { seen: seen.clone() }));
        let response = poll_ready(backend.execute(request())).expect("chain succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        // The observer captured the response status the inner backend produced.
        assert_eq!(*seen.lock().unwrap(), Some(StatusCode::OK));
    }

    #[test]
    fn short_circuit_middleware_never_hits_inner_backend() {
        let inner = Arc::new(RecordingBackend::default());
        let backend =
            MiddlewareBackend::new(inner.clone()).layer(Arc::new(ShortCircuit { status: 418 }));
        let response = poll_ready(backend.execute(request())).expect("short-circuit succeeds");
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        // The transport was never reached: short-circuit returned before calling `next`.
        assert_eq!(inner.calls(), 0);
    }

    #[test]
    fn two_middlewares_compose_outer_wraps_inner() {
        let inner = Arc::new(RecordingBackend::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        // `outer` is layered first, so it runs outermost; `inner_mw` runs closest to the transport.
        let backend = MiddlewareBackend::new(inner.clone())
            .layer(Arc::new(Tracer {
                label: "outer",
                log: log.clone(),
            }))
            .layer(Arc::new(Tracer {
                label: "inner",
                log: log.clone(),
            }));
        let response = poll_ready(backend.execute(request())).expect("chain succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(inner.calls(), 1);
        // Onion order: outer opens first and closes last, inner is nested within it.
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "outer:before".to_owned(),
                "inner:before".to_owned(),
                "inner:after".to_owned(),
                "outer:after".to_owned(),
            ]
        );
    }

    #[test]
    fn with_middlewares_matches_layered_order() {
        // `with_middlewares(vec![outer, inner])` is equivalent to `.layer(outer).layer(inner)`.
        let inner = Arc::new(RecordingBackend::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let outer: Arc<dyn Middleware> = Arc::new(Tracer {
            label: "outer",
            log: log.clone(),
        });
        let inner_mw: Arc<dyn Middleware> = Arc::new(Tracer {
            label: "inner",
            log: log.clone(),
        });
        let backend = MiddlewareBackend::with_middlewares(inner, vec![outer, inner_mw]);
        poll_ready(backend.execute(request())).expect("chain succeeds");
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "outer:before".to_owned(),
                "inner:before".to_owned(),
                "inner:after".to_owned(),
                "outer:after".to_owned(),
            ]
        );
    }

    #[test]
    fn composes_over_retry_backend() {
        // Composability: `MiddlewareBackend` wraps ANY `HttpBackend`, including a `RetryBackend`. Here
        // a retry backend with a no-op policy (never retries) sits under a header-injecting
        // middleware; the request still reaches the innermost recording backend with the header.
        #[derive(Debug)]
        struct NeverRetry;
        impl crate::RetryPolicy for NeverRetry {
            fn retry<'a>(
                &'a self,
                _attempt: u32,
                _outcome: &crate::RetryOutcome<'_>,
            ) -> Option<std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
                None
            }
        }

        let recording = Arc::new(RecordingBackend::default());
        let retry: Arc<dyn HttpBackend> =
            Arc::new(RetryBackend::new(recording.clone(), Arc::new(NeverRetry)));
        let backend = MiddlewareBackend::new(retry).layer(Arc::new(InsertHeader {
            name: HeaderName::from_static("x-trace"),
            value: HeaderValue::from_static("via-retry"),
        }));
        let response = poll_ready(backend.execute(request())).expect("composed chain succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            recording.seen_header("x-trace").as_deref(),
            Some("via-retry")
        );
        assert_eq!(recording.calls(), 1);
    }

    #[test]
    fn empty_chain_delegates_straight_to_inner() {
        // A `MiddlewareBackend` with no layers is a transparent pass-through to the inner transport.
        let inner = Arc::new(RecordingBackend::default());
        let backend = MiddlewareBackend::new(inner.clone());
        let response = poll_ready(backend.execute(request())).expect("pass-through succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(inner.calls(), 1);
    }

    #[test]
    fn is_object_safe_and_debug() {
        let inner: Arc<dyn HttpBackend> = Arc::new(RecordingBackend::default());
        let backend: Arc<dyn HttpBackend> = Arc::new(MiddlewareBackend::new(inner));
        // `Debug` is required for `HttpBackend`; the manual impl renders without panicking.
        assert!(format!("{backend:?}").contains("MiddlewareBackend"));
    }
}
