use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Method, Request};

use crate::tests::{
    poll_ready, poll_to_completion, refused_at_execute, request, PendingOnce, Reply,
    RetryTransient, SequenceBackend,
};
use crate::transport::{ExecuteFuture, HttpBackend, ReqwestBackend};

use super::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};

fn retrying(inner: Arc<dyn HttpBackend>, max_retries: u32) -> RetryBackend {
    RetryBackend::new(inner, Arc::new(RetryTransient { max_retries }))
}

#[test]
fn retries_transient_failures_until_success() {
    // 503, 503, then 200: the backend should be hit three times and the final 200 returned.
    let inner = SequenceBackend::statuses([503, 503, 200]);
    let backend = retrying(inner.clone(), 5);
    let response = poll_ready(backend.execute(request())).expect("execute succeeds");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(inner.calls(), 3);
}

#[test]
fn stops_after_max_retries_and_returns_last_failure() {
    // Always 503 with a 2-retry budget: attempts at index 0 and 1 retry, index 2 stops — three
    // executions, and the last (still failing) response is returned rather than an error.
    let inner = SequenceBackend::statuses([503]);
    let backend = retrying(inner.clone(), 2);
    let response = poll_ready(backend.execute(request())).expect("returns the last response");
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(inner.calls(), 3);
}

/// A transport failure the default classifier calls transient is retried like a `503`, and once
/// the budget is spent the last failure itself is returned, not the first and not a response.
#[test]
fn a_transient_transport_failure_is_retried_and_the_last_failure_returned() {
    let inner = SequenceBackend::new([
        Reply::failure(502),
        Reply::failure(503),
        Reply::failure(504),
    ]);
    let backend = retrying(inner.clone(), 2);
    let error = poll_ready(backend.execute(request())).expect_err("every attempt fails");
    assert_eq!(inner.calls(), 3);
    assert!(RetryOutcome::Transport(&error).is_transient());
    let source = std::error::Error::source(&error)
        .expect("a transport error wraps its reqwest error")
        .to_string();
    assert!(source.contains("504"), "{source}");
}

/// Records, each time its wait completes, how many requests the backend had been handed by then.
/// The wait answers `Pending` once before it completes, so it cannot finish unless it is awaited.
struct ObservedWait {
    backend: Arc<SequenceBackend>,
    calls_when_waited: Arc<Mutex<Vec<usize>>>,
}

impl RetryPolicy for ObservedWait {
    fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>> {
        (attempt < 2 && outcome.is_transient()).then(|| {
            let backend = Arc::clone(&self.backend);
            let calls_when_waited = Arc::clone(&self.calls_when_waited);
            Box::pin(PendingOnce::new(async move {
                calls_when_waited.lock().unwrap().push(backend.calls());
            })) as RetryWait<'a>
        })
    }
}

/// The retry loop awaits the policy's wait, and only then replays: the execute future answers
/// `Pending` once per wait, and each wait finishes before the attempt it precedes is sent.
#[test]
fn the_policys_wait_completes_before_each_replay() {
    let inner = SequenceBackend::statuses([503, 503, 200]);
    let calls_when_waited = Arc::new(Mutex::new(Vec::new()));
    let backend = RetryBackend::new(
        inner.clone(),
        Arc::new(ObservedWait {
            backend: Arc::clone(&inner),
            calls_when_waited: Arc::clone(&calls_when_waited),
        }),
    );
    let (response, pending) = poll_to_completion(backend.execute(request()));
    assert_eq!(response.expect("the third attempt succeeds").status(), 200);
    assert_eq!(pending, 2);
    assert_eq!(*calls_when_waited.lock().unwrap(), [1, 2]);
    assert_eq!(inner.calls(), 3);
}

#[test]
fn does_not_retry_non_transient_outcome() {
    // A 400 is not transient: the policy stops immediately, so the backend is hit exactly once.
    let inner = SequenceBackend::statuses([400]);
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

    let inner = SequenceBackend::statuses([503]);
    // A generous retry budget: only the non-cloneable guard can hold it to one execution.
    let backend = retrying(inner.clone(), 10);
    let response = poll_ready(backend.execute(request)).expect("executes once");
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(inner.calls(), 1);
}

#[test]
fn retry_outcome_exposes_status_and_transience() {
    let response = crate::tests::response(429, String::new());
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
/// status code from `100` to `599`. For `Decode` that is the rule `Error::is_transient` states:
/// a `429` or `5xx` whose body did not match the schema is still the server saying "try again",
/// while a `2xx` or `4xx` body that does not decode reproduces on retry.
#[test]
fn a_response_outcome_is_transient_exactly_when_its_error_is() {
    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use reqwest::StatusCode;

    use crate::{Error, ResponseValue};

    for code in 100u16..=599 {
        let status = StatusCode::from_u16(code).expect("a three-digit status");
        let response = crate::tests::response(code, String::new());
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
    for (client, request) in refused_at_execute() {
        let url = request.url().to_string();
        let inner = Arc::new(CountingBackend {
            inner: ReqwestBackend::new(client),
            calls: AtomicU32::new(0),
        });
        let backend = retrying(inner.clone(), 5);
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

/// The manual `Debug` renders the type and its inner backend, and marks the policy it leaves out.
#[test]
fn debug_names_the_backend_and_its_inner_backend() {
    let backend = retrying(SequenceBackend::statuses([200]), 0);
    let rendered = format!("{backend:?}");
    assert!(rendered.starts_with("RetryBackend {"), "{rendered}");
    assert!(rendered.contains("inner: SequenceBackend"), "{rendered}");
    assert!(rendered.ends_with(".. }"), "{rendered}");
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

/// `base * 2^attempt` overflowing `Duration` falls back to `max` rather than panicking, at a
/// small attempt with a huge base and at a huge attempt whose factor has saturated.
#[test]
fn exponential_backoff_saturates_to_max_when_base_times_factor_overflows() {
    let max = Duration::from_secs(30);
    let huge = Duration::from_secs(u64::MAX / 2 + 1);
    assert!(huge.checked_mul(2).is_none());
    assert_eq!(exponential_backoff(1, huge, max), max);
    assert_eq!(exponential_backoff(u32::MAX, Duration::MAX, max), max);
    let day = Duration::from_secs(86_400);
    assert!(day.checked_mul(u32::MAX).is_some());
    assert_eq!(exponential_backoff(u32::MAX, day, max), max);
}

/// The clamp applies before any doubling: a base already above `max` waits `max` at attempt 0.
#[test]
fn exponential_backoff_never_exceeds_max_even_at_attempt_zero() {
    let max = Duration::from_millis(250);
    assert_eq!(exponential_backoff(0, Duration::from_secs(1), max), max);
    assert_eq!(exponential_backoff(0, max, max), max);
    assert_eq!(
        exponential_backoff(0, Duration::from_millis(249), max),
        Duration::from_millis(249)
    );
}

proptest::proptest! {
    /// Over every attempt, base and ceiling, the wait never exceeds `max`, never shrinks as the
    /// attempt grows, and is exactly `base * 2^attempt` wherever that product is below `max`.
    #[test]
    fn exponential_backoff_is_monotone_and_bounded_by_max(
        attempt in 0u32..80,
        base_nanos in proptest::num::u64::ANY,
        max_nanos in proptest::num::u64::ANY,
    ) {
        let base = Duration::from_nanos(base_nanos);
        let max = Duration::from_nanos(max_nanos);
        let wait = exponential_backoff(attempt, base, max);
        proptest::prop_assert!(wait <= max);
        proptest::prop_assert!(wait <= exponential_backoff(attempt.saturating_add(1), base, max));
        let exact = 2u32
            .checked_pow(attempt)
            .and_then(|factor| base.checked_mul(factor))
            .filter(|product| *product < max);
        if let Some(product) = exact {
            proptest::prop_assert_eq!(wait, product);
        }
    }
}
