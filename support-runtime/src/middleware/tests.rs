use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Method, Request, Response, StatusCode};

use crate::tests::poll_ready;
use crate::transport::{ExecuteFuture, HttpBackend};
use crate::RetryBackend;

use super::{Middleware, MiddlewareBackend, Next};

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
        *self.seen_headers.lock().expect("mutex not poisoned") = Some(request.headers().clone());
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

/// `middlewares` reports the chain in run order: what `layer` appended, outermost first, and
/// nothing for a bare `new`.
#[test]
fn middlewares_lists_the_chain_in_run_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let outer: Arc<dyn Middleware> = Arc::new(Tracer {
        label: "outer",
        log: log.clone(),
    });
    let inner_mw: Arc<dyn Middleware> = Arc::new(Tracer {
        label: "inner",
        log,
    });
    let bare = MiddlewareBackend::new(Arc::new(RecordingBackend::default()));
    assert!(bare.middlewares().is_empty());
    let backend = bare.layer(outer.clone()).layer(inner_mw.clone());
    let chain = backend.middlewares();
    assert_eq!(chain.len(), 2);
    assert!(Arc::ptr_eq(&chain[0], &outer));
    assert!(Arc::ptr_eq(&chain[1], &inner_mw));
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

/// An inner backend answering with the status at each call index (the last repeats), counting
/// executions.
#[derive(Debug)]
struct SequenceBackend {
    statuses: Vec<u16>,
    calls: AtomicU32,
}

impl SequenceBackend {
    fn new(statuses: Vec<u16>) -> Self {
        Self {
            statuses,
            calls: AtomicU32::new(0),
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HttpBackend for SequenceBackend {
    fn execute(&self, _request: Request) -> ExecuteFuture<'_> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
        let status = *self
            .statuses
            .get(index)
            .or_else(|| self.statuses.last())
            .expect("at least one canned status");
        Box::pin(async move { Ok(canned(status)) })
    }
}

/// A middleware that counts how many times it runs and records each status it sees come back.
#[derive(Debug, Default)]
struct Counting {
    runs: AtomicU32,
    seen: Mutex<Vec<u16>>,
}

impl Middleware for Counting {
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> ExecuteFuture<'a> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let response = next.run(request).await?;
            self.seen
                .lock()
                .expect("mutex not poisoned")
                .push(response.status().as_u16());
            Ok(response)
        })
    }
}

/// Retries every transient outcome, with an already-ready wait so no runtime is needed.
struct RetryTransient;

impl crate::RetryPolicy for RetryTransient {
    fn retry<'a>(
        &'a self,
        attempt: u32,
        outcome: &crate::RetryOutcome<'_>,
    ) -> Option<std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
        if attempt < 5 && outcome.is_transient() {
            Some(Box::pin(std::future::ready(())))
        } else {
            None
        }
    }
}

/// The module docs' composability claim, observed in both nestings over `503, 503, 200`: a
/// `MiddlewareBackend` around a `RetryBackend` runs its middleware once, around the whole retry
/// loop, and sees only the final `200`; a `RetryBackend` around a `MiddlewareBackend` runs the
/// middleware once per attempt, and it sees every status.
#[test]
fn retry_nesting_decides_whether_middleware_runs_once_or_per_attempt() {
    // Middleware outside the retry loop.
    let transport = Arc::new(SequenceBackend::new(vec![503, 503, 200]));
    let counting = Arc::new(Counting::default());
    let as_dyn: Arc<dyn Middleware> = counting.clone();
    let retry: Arc<dyn HttpBackend> = Arc::new(RetryBackend::new(
        transport.clone(),
        Arc::new(RetryTransient),
    ));
    let backend = MiddlewareBackend::new(retry).layer(as_dyn.clone());
    assert_eq!(backend.middlewares().len(), 1);
    assert!(Arc::ptr_eq(&backend.middlewares()[0], &as_dyn));
    let response = poll_ready(backend.execute(request())).expect("retried chain succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(transport.calls(), 3);
    assert_eq!(counting.runs.load(Ordering::SeqCst), 1);
    assert_eq!(*counting.seen.lock().unwrap(), vec![200]);

    // Middleware inside the retry loop.
    let transport = Arc::new(SequenceBackend::new(vec![503, 503, 200]));
    let counting = Arc::new(Counting::default());
    let as_dyn: Arc<dyn Middleware> = counting.clone();
    let chain = MiddlewareBackend::new(transport.clone()).layer(as_dyn.clone());
    assert_eq!(chain.middlewares().len(), 1);
    assert!(Arc::ptr_eq(&chain.middlewares()[0], &as_dyn));
    let backend = RetryBackend::new(Arc::new(chain), Arc::new(RetryTransient));
    let response = poll_ready(backend.execute(request())).expect("retried chain succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(transport.calls(), 3);
    assert_eq!(counting.runs.load(Ordering::SeqCst), 3);
    assert_eq!(*counting.seen.lock().unwrap(), vec![503, 503, 200]);
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
