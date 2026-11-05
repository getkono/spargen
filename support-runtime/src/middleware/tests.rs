use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Request, StatusCode};

use crate::tests::{poll_ready, request, response, RetryTransient, SequenceBackend};
use crate::transport::{ExecuteFuture, HttpBackend};
use crate::RetryBackend;

use super::{Middleware, MiddlewareBackend, Next};

/// An inner transport answering every request with an empty `200`.
fn ok_backend() -> Arc<SequenceBackend> {
    SequenceBackend::statuses([200])
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
        Box::pin(async move { Ok(response(status, String::new())) })
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
    let inner = ok_backend();
    let backend = MiddlewareBackend::new(inner.clone()).layer(Arc::new(InsertHeader {
        name: HeaderName::from_static("x-trace"),
        value: HeaderValue::from_static("on"),
    }));
    let response = poll_ready(backend.execute(request())).expect("chain succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    // The inner backend actually received the header the middleware inserted.
    assert_eq!(inner.header("x-trace"), [Some("on".to_owned())]);
}

#[test]
fn middleware_observes_response_after_transport() {
    let inner = ok_backend();
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
    let inner = ok_backend();
    let backend =
        MiddlewareBackend::new(inner.clone()).layer(Arc::new(ShortCircuit { status: 418 }));
    let response = poll_ready(backend.execute(request())).expect("short-circuit succeeds");
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    // The transport was never reached: short-circuit returned before calling `next`.
    assert_eq!(inner.calls(), 0);
}

#[test]
fn two_middlewares_compose_outer_wraps_inner() {
    let inner = ok_backend();
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
    let inner = ok_backend();
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
    let recording = ok_backend();
    let retry: Arc<dyn HttpBackend> = Arc::new(RetryBackend::new(
        recording.clone(),
        Arc::new(RetryTransient { max_retries: 0 }),
    ));
    let backend = MiddlewareBackend::new(retry).layer(Arc::new(InsertHeader {
        name: HeaderName::from_static("x-trace"),
        value: HeaderValue::from_static("via-retry"),
    }));
    let response = poll_ready(backend.execute(request())).expect("composed chain succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(recording.header("x-trace"), [Some("via-retry".to_owned())]);
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

/// The module docs' composability claim, observed in both nestings over `503, 503, 200`: a
/// `MiddlewareBackend` around a `RetryBackend` runs its middleware once, around the whole retry
/// loop, and sees only the final `200`; a `RetryBackend` around a `MiddlewareBackend` runs the
/// middleware once per attempt, and it sees every status.
#[test]
fn retry_nesting_decides_whether_middleware_runs_once_or_per_attempt() {
    // Middleware outside the retry loop.
    let transport = SequenceBackend::statuses([503, 503, 200]);
    let counting = Arc::new(Counting::default());
    let as_dyn: Arc<dyn Middleware> = counting.clone();
    let retry: Arc<dyn HttpBackend> = Arc::new(RetryBackend::new(
        transport.clone(),
        Arc::new(RetryTransient { max_retries: 5 }),
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
    let transport = SequenceBackend::statuses([503, 503, 200]);
    let counting = Arc::new(Counting::default());
    let as_dyn: Arc<dyn Middleware> = counting.clone();
    let chain = MiddlewareBackend::new(transport.clone()).layer(as_dyn.clone());
    assert_eq!(chain.middlewares().len(), 1);
    assert!(Arc::ptr_eq(&chain.middlewares()[0], &as_dyn));
    let backend = RetryBackend::new(Arc::new(chain), Arc::new(RetryTransient { max_retries: 5 }));
    let response = poll_ready(backend.execute(request())).expect("retried chain succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(transport.calls(), 3);
    assert_eq!(counting.runs.load(Ordering::SeqCst), 3);
    assert_eq!(*counting.seen.lock().unwrap(), vec![503, 503, 200]);
}

#[test]
fn empty_chain_delegates_straight_to_inner() {
    // A `MiddlewareBackend` with no layers is a transparent pass-through to the inner transport.
    let inner = ok_backend();
    let backend = MiddlewareBackend::new(inner.clone());
    let response = poll_ready(backend.execute(request())).expect("pass-through succeeds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(inner.calls(), 1);
}

// `MiddlewareBackend` installs behind `dyn HttpBackend`, as `ClientCore` stores its transport.
const _: fn(MiddlewareBackend) -> Arc<dyn HttpBackend> = |backend| Arc::new(backend);

/// `Debug` is required of every `HttpBackend`. The chain renders its inner transport and each
/// layer in order, so a `ClientCore` printed while debugging shows what a request runs through.
#[test]
fn debug_names_the_inner_backend_and_each_layer_in_order() {
    let backend = MiddlewareBackend::new(ok_backend())
        .layer(Arc::new(InsertHeader {
            name: HeaderName::from_static("x-trace"),
            value: HeaderValue::from_static("on"),
        }))
        .layer(Arc::new(ShortCircuit { status: 418 }));
    let rendered = format!("{backend:?}");
    assert!(
        rendered.starts_with("MiddlewareBackend { inner: SequenceBackend {"),
        "{rendered}"
    );
    let layers = rendered
        .split_once("middlewares: [")
        .map(|(_, layers)| layers)
        .unwrap_or_else(|| panic!("the layers are rendered: {rendered}"));
    assert!(
        layers.starts_with("InsertHeader {") && layers.contains("}, ShortCircuit { status: 418 }]"),
        "{rendered}"
    );
}
