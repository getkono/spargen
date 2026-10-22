use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, Request};

use crate::tests::poll_ready;
use crate::transport::{ExecuteFuture, HttpBackend, ReqwestBackend};

use super::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy};

/// An inner backend that returns a canned response with the status at each call index, counting
/// how many times it was executed. The last status repeats once the sequence is exhausted.
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
        Box::pin(async move {
            Ok(reqwest::Response::from(
                http::Response::builder()
                    .status(status)
                    .body(String::new())
                    .expect("valid synthetic response"),
            ))
        })
    }
}

/// Retries transient outcomes (per the default classifier) until `max_retries` retries have been
/// made, always with an already-ready wait future — no real timer.
struct MaxRetries {
    max_retries: u32,
}

impl RetryPolicy for MaxRetries {
    fn retry<'a>(
        &'a self,
        attempt: u32,
        outcome: &RetryOutcome<'_>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
        if attempt < self.max_retries && outcome.is_transient() {
            // A ready future stands in for the caller's `tokio::time::sleep(delay)` — the retry
            // loop awaits it, but it completes instantly so the test needs no runtime.
            Some(Box::pin(std::future::ready(())))
        } else {
            None
        }
    }
}

fn request() -> Request {
    reqwest::Client::new()
        .request(Method::GET, "https://example.com/op")
        .build()
        .expect("build request")
}

fn retrying(inner: Arc<SequenceBackend>, max_retries: u32) -> RetryBackend {
    RetryBackend::new(inner, Arc::new(MaxRetries { max_retries }))
}

#[test]
fn retries_transient_failures_until_success() {
    // 503, 503, then 200: the backend should be hit three times and the final 200 returned.
    let inner = Arc::new(SequenceBackend::new(vec![503, 503, 200]));
    let backend = retrying(inner.clone(), 5);
    let response = poll_ready(backend.execute(request())).expect("execute succeeds");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(inner.calls(), 3);
}

#[test]
fn stops_after_max_retries_and_returns_last_failure() {
    // Always 503 with a 2-retry budget: attempts at index 0 and 1 retry, index 2 stops — three
    // executions, and the last (still failing) response is returned rather than an error.
    let inner = Arc::new(SequenceBackend::new(vec![503]));
    let backend = retrying(inner.clone(), 2);
    let response = poll_ready(backend.execute(request())).expect("returns the last response");
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(inner.calls(), 3);
}

#[test]
fn does_not_retry_non_transient_outcome() {
    // A 400 is not transient: the policy stops immediately, so the backend is hit exactly once.
    let inner = Arc::new(SequenceBackend::new(vec![400]));
    let backend = retrying(inner.clone(), 5);
    let response = poll_ready(backend.execute(request())).expect("returns the response");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(inner.calls(), 1);
}

#[test]
fn non_cloneable_request_is_executed_exactly_once() {
    // Wrapping a reusable body in `Body::wrap` produces a one-shot streaming body, so
    // `try_clone` returns `None`. Even against a policy that would retry forever, such a request
    // must be executed exactly once (a consumed stream cannot be safely replayed).
    let streaming_body = reqwest::Body::wrap(reqwest::Body::from("payload"));
    let request = reqwest::Client::new()
        .request(Method::POST, "https://example.com/op")
        .body(streaming_body)
        .build()
        .expect("build streaming request");
    assert!(
        request.try_clone().is_none(),
        "a wrapped streaming body must be non-cloneable for this test to be meaningful"
    );

    let inner = Arc::new(SequenceBackend::new(vec![503]));
    // A generous retry budget: only the non-cloneable guard can hold it to one execution.
    let backend = retrying(inner.clone(), 10);
    let response = poll_ready(backend.execute(request)).expect("executes once");
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(inner.calls(), 1);
}

#[test]
fn retry_outcome_exposes_status_and_transience() {
    let response = reqwest::Response::from(
        http::Response::builder()
            .status(429)
            .body(String::new())
            .expect("valid synthetic response"),
    );
    let outcome = RetryOutcome::Response(&response);
    assert_eq!(
        outcome.status(),
        Some(reqwest::StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(outcome.is_transient());
    assert!(outcome.response().is_some());
    assert!(outcome.transport_error().is_none());
    assert!(outcome.result().is_ok());
}

/// `RetryOutcome::is_transient` documents itself as exactly what `Error::is_transient` answers
/// for the error the outcome becomes. For a response that is every status-carrying class:
/// documented (`Api`), undocumented (`UnexpectedStatus`), and undecodable (`Decode`). Both
/// sides are held to that agreement, and to the rule itself (`429` or `5xx`), at every
/// status code from `100` to `599`.
#[test]
fn a_response_outcome_is_transient_exactly_when_its_error_is() {
    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use reqwest::StatusCode;

    use crate::{Error, ResponseValue};

    for code in 100u16..=599 {
        let status = StatusCode::from_u16(code).expect("a three-digit status");
        let response = reqwest::Response::from(
            http::Response::builder()
                .status(code)
                .body(String::new())
                .expect("valid synthetic response"),
        );
        let outcome = RetryOutcome::Response(&response).is_transient();
        let errors: [Error<String>; 3] = [
            Error::Api(ResponseValue::new(status, HeaderMap::new(), String::new())),
            Error::UnexpectedStatus {
                status,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            },
            Error::Decode {
                status,
                headers: HeaderMap::new(),
                path: "x".to_owned(),
                body: Bytes::new(),
                truncated: false,
            },
        ];
        for error in &errors {
            assert_eq!(outcome, error.is_transient(), "{code}: {error}");
        }
        assert_eq!(
            outcome,
            code == 429 || (500..=599).contains(&code),
            "{code}"
        );
    }
}

/// Counts executions while delegating to the real reqwest transport, so a test can see how
/// many times `RetryBackend` re-sent a request that reqwest refuses before any I/O.
#[derive(Debug)]
struct CountingBackend {
    inner: ReqwestBackend,
    calls: AtomicU32,
}

impl HttpBackend for CountingBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(request)
    }
}

/// A request reqwest refuses to send is refused identically on every attempt: nothing about
/// waiting and re-sending can change a URL scheme. reqwest raises that refusal from `execute`
/// itself — the request built fine — as a builder-kind error, which the backend reports as a
/// `TransportError`. Two ways to reach it with a base URL `ClientCore` accepts: a scheme
/// other than `http`/`https` (it has a host, so `build` passes), and plain `http` on a client
/// configured `https_only`. Neither may be retried, even against a generous budget; the
/// refusal is returned after exactly one execution. reqwest answers without touching the
/// network, so the future is ready on the first poll.
#[test]
fn a_request_reqwest_refuses_to_send_is_executed_exactly_once() {
    let https_only = reqwest::Client::builder()
        .https_only(true)
        .build()
        .expect("build an https-only client");
    for (client, url) in [
        (reqwest::Client::new(), "ftp://example.com/op"),
        (https_only, "http://example.com/op"),
    ] {
        let request = client
            .request(Method::GET, url)
            .build()
            .expect("reqwest builds the request; it refuses it only at execute");
        let inner = Arc::new(CountingBackend {
            inner: ReqwestBackend::new(client),
            calls: AtomicU32::new(0),
        });
        let backend = RetryBackend::new(inner.clone(), Arc::new(MaxRetries { max_retries: 5 }));
        let error = poll_ready(backend.execute(request)).expect_err("reqwest refuses the request");
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<reqwest::Error>())
            .expect("a transport error wraps a reqwest error");
        assert!(source.is_builder(), "{url}: {error}");
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1, "{url}");
        let outcome = RetryOutcome::Transport(&error);
        assert!(
            !outcome.is_transient(),
            "{url}: a builder-kind refusal is permanent"
        );
        // A transport outcome carries no response: every accessor reports the failure instead.
        assert!(outcome.response().is_none(), "{url}");
        assert!(
            std::ptr::eq(
                outcome.transport_error().expect("a transport error"),
                &error
            ),
            "{url}"
        );
        assert_eq!(outcome.status(), None, "{url}");
        assert!(
            outcome
                .result()
                .is_err_and(|failure| std::ptr::eq(failure, &error)),
            "{url}"
        );
    }
}

#[test]
fn exponential_backoff_doubles_and_clamps() {
    let base = Duration::from_millis(100);
    let max = Duration::from_secs(1);
    // attempt 0 → base, then doubling: 100ms, 200ms, 400ms, 800ms.
    assert_eq!(
        exponential_backoff(0, base, max),
        Duration::from_millis(100)
    );
    assert_eq!(
        exponential_backoff(1, base, max),
        Duration::from_millis(200)
    );
    assert_eq!(
        exponential_backoff(2, base, max),
        Duration::from_millis(400)
    );
    assert_eq!(
        exponential_backoff(3, base, max),
        Duration::from_millis(800)
    );
    // attempt 4 would be 1600ms but clamps to the 1s ceiling; a huge attempt saturates, not
    // panics, and still clamps.
    assert_eq!(exponential_backoff(4, base, max), max);
    assert_eq!(exponential_backoff(1000, base, max), max);
}
