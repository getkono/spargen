use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Method;

use crate::tests::{
    poll_ready, poll_to_completion, refused_at_execute, seam_request as request, Chunk, Chunks,
    PendingOnce, Reply, RetryTransient, SequenceBackend,
};
use crate::transport::{try_clone_request, ExecuteFuture, HttpBackend, ReqwestBackend};
use crate::{HttpBody, RequestBuilder};

/// A request as the transport seam carries it.
type Request = http::Request<HttpBody>;

use super::{
    days_from_civil, exponential_backoff, parse_http_date, parse_retry_after, year_of_day,
    RetryBackend, RetryOutcome, RetryPolicy, RetryWait,
};

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
    assert!(RetryOutcome::new(&request(), Err(&error)).is_transient());
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
    // An adopted streaming body is one-shot, so the request does not clone. Even against a
    // policy that would retry forever, such a request must be executed exactly once (a consumed
    // stream cannot be safely replayed).
    let request = http::Request::builder()
        .method(Method::POST)
        .uri("https://example.com/op")
        .body(HttpBody::wrap(Chunks::new([Chunk::data("payload")])))
        .expect("build streaming request");
    assert!(
        try_clone_request(&request).is_none(),
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
    let sent = request();
    let outcome = RetryOutcome::new(&sent, Ok(&response));
    assert!(std::ptr::eq(outcome.request(), &sent));
    assert_eq!(*outcome.method(), Method::GET);
    assert_eq!(
        outcome.status(),
        Some(reqwest::StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(outcome.is_transient());
    assert!(outcome.response().is_some());
    assert!(outcome.transport_error().is_none());
    assert!(outcome.result().is_ok());
}

/// `RetryOutcome::is_transient` documents itself, for an idempotent request such as this `GET`,
/// as exactly what `Error::is_transient` answers for the error the outcome becomes. For a
/// response that is every status-carrying class:
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
        let outcome = RetryOutcome::new(&request(), Ok(&response)).is_transient();
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
        let url = request.uri().to_string();
        let probe = try_clone_request(&request).expect("a bodyless request clones");
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
        let outcome = RetryOutcome::new(&probe, Err(&error));
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

/// A request with `method` and, where `key` is `Some`, that `Idempotency-Key`.
fn request_with(method: Method, key: Option<&str>) -> Request {
    let url = url::Url::parse("https://example.com/op").expect("a valid URL");
    let builder = RequestBuilder::new(method, url);
    let builder = match key {
        Some(key) => builder.header("Idempotency-Key", key),
        None => builder,
    };
    builder.body("payload").build().expect("build request")
}

/// The fixture for the bug: a `POST` that drew a `503` may already have been applied, so the
/// default classifier never retries it, however generous the budget; the backend sees it once.
#[test]
fn a_non_idempotent_request_is_not_retried_by_the_default_classifier() {
    for method in [Method::POST, Method::PATCH, Method::CONNECT] {
        let inner = SequenceBackend::statuses([503, 200]);
        let backend = retrying(inner.clone(), 5);
        let response =
            poll_ready(backend.execute(request_with(method.clone(), None))).expect("responds");
        assert_eq!(response.status(), 503, "{method}");
        assert_eq!(inner.calls(), 1, "{method}");

        let inner = SequenceBackend::new([Reply::failure(503), Reply::status(200, "")]);
        let backend = retrying(inner.clone(), 5);
        poll_ready(backend.execute(request_with(method.clone(), None)))
            .expect_err("the transport failure is returned unretried");
        assert_eq!(inner.calls(), 1, "{method}");
    }
}

/// An `Idempotency-Key` marks a request of any method as safe to repeat, so a `POST` carrying
/// one is retried like a `GET`, and every replay carries the same key.
#[test]
fn an_idempotency_key_makes_any_method_retryable() {
    let inner = SequenceBackend::statuses([503, 503, 200]);
    let backend = retrying(inner.clone(), 5);
    let request = request_with(Method::POST, Some("k-1"));
    let response = poll_ready(backend.execute(request)).expect("execute succeeds");
    assert_eq!(response.status(), 200);
    assert_eq!(inner.calls(), 3);
    assert_eq!(
        inner.header("idempotency-key"),
        vec![Some("k-1".to_owned()); 3]
    );
}

/// Every method RFC 9110 §9.2.2 calls idempotent is retried on a transient outcome, and every
/// other one only with an `Idempotency-Key`; the classification of the failure itself is the
/// same for all of them.
#[test]
fn is_idempotent_follows_rfc_9110_and_the_idempotency_key() {
    let response = crate::tests::response(503, String::new());
    let extension = Method::from_bytes(b"PURGE").expect("an extension method");
    let cases = [
        (Method::GET, true),
        (Method::HEAD, true),
        (Method::OPTIONS, true),
        (Method::TRACE, true),
        (Method::PUT, true),
        (Method::DELETE, true),
        (Method::POST, false),
        (Method::PATCH, false),
        (Method::CONNECT, false),
        (extension, false),
    ];
    for (method, idempotent) in cases {
        let bare = request_with(method.clone(), None);
        let outcome = RetryOutcome::new(&bare, Ok(&response));
        assert_eq!(*outcome.method(), method);
        assert_eq!(outcome.is_idempotent(), idempotent, "{method}");
        assert_eq!(outcome.is_transient(), idempotent, "{method}");
        let keyed = request_with(method.clone(), Some("k"));
        let outcome = RetryOutcome::new(&keyed, Ok(&response));
        assert!(outcome.is_idempotent(), "{method} with a key");
        assert!(outcome.is_transient(), "{method} with a key");
    }
    // A non-transient status stays non-retryable whatever the method.
    let ok = crate::tests::response(400, String::new());
    let keyed = request_with(Method::POST, Some("k"));
    assert!(!RetryOutcome::new(&keyed, Ok(&ok)).is_transient());
}

/// Records the method each outcome the loop hands the policy carries, and never retries.
#[derive(Default)]
struct RecordMethod(Mutex<Vec<Method>>);

impl RetryPolicy for RecordMethod {
    fn retry<'a>(&'a self, _attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>> {
        self.0.lock().unwrap().push(outcome.method().clone());
        None
    }
}

/// The loop hands the policy the request it sent, so the outcome's method is the request's.
#[test]
fn the_policy_sees_the_method_of_the_request_sent() {
    let policy = Arc::new(RecordMethod::default());
    let backend = RetryBackend::new(SequenceBackend::statuses([503]), policy.clone());
    poll_ready(backend.execute(request_with(Method::DELETE, None))).expect("responds");
    assert_eq!(*policy.0.lock().unwrap(), [Method::DELETE]);
}

/// 1994-11-06T08:49:37Z, the instant RFC 9110's HTTP-date examples name, as epoch seconds.
const RFC_EXAMPLE: i64 = 784_111_777;

fn at(seconds: i64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).expect("after the epoch"))
}

/// A response carrying `Retry-After` `values`, one header line each.
fn response_with_retry_after(values: &[&str]) -> crate::tests::Response {
    let mut builder = http::Response::builder().status(503);
    for value in values {
        builder = builder.header("retry-after", *value);
    }
    crate::tests::from_http(builder.body(String::new()).expect("valid response"))
}

/// The three HTTP-date forms RFC 9110 §5.6.7 obliges a recipient to accept all name the same
/// instant, and every malformed variant of them is refused.
#[test]
fn every_http_date_form_parses_and_malformed_dates_do_not() {
    let now = RFC_EXAMPLE;
    for value in [
        "Sun, 06 Nov 1994 08:49:37 GMT",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
        "Sun Nov 06 08:49:37 1994",
    ] {
        assert_eq!(parse_http_date(value, now), Some(RFC_EXAMPLE), "{value}");
    }
    assert_eq!(
        parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT", now),
        Some(0)
    );
    assert_eq!(
        parse_http_date("Thu, 29 Feb 2024 23:59:60 GMT", now),
        Some(1_709_251_200),
        "a leap day and a leap second"
    );
    for value in [
        "",
        "Sun, 6 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 94 08:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 UTC",
        "Sun, 06 Nov 1994 08:49:37 GMT trailing",
        "Sun, 06 Foo 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 24:00:00 GMT",
        "Sun, 06 Nov 1994 08:60:00 GMT",
        "Sun, 06 Nov 1994 08:49:61 GMT",
        "Sun, 06 Nov 1994 08:49 GMT",
        "Sun, 31 Nov 1994 08:49:37 GMT",
        "Sun, 29 Feb 1900 08:49:37 GMT",
        "Sun, 00 Nov 1994 08:49:37 GMT",
        "Sundae, 06 Nov 1994 08:49:37 GMT",
        "Sunday, 06-Nov-1994 08:49:37 GMT",
        "Sunday, 06-Nov-94-1 08:49:37 GMT",
        "Sun Nov  6 08:49:37 94",
        "Sun Nov   6 08:49:37 1994",
        "Sun Nov 6 08:49:37 1994",
        "Xyz Nov  6 08:49:37 1994",
        "Sun Nov  6 08:49:37 1994 ",
        "Sun,06 Nov 1994 08:49:37 GMT",
    ] {
        assert_eq!(parse_http_date(value, now), None, "{value:?}");
    }
}

/// An RFC 850 two-digit year is the year with those digits not more than 50 years after now's.
#[test]
fn an_rfc_850_year_is_placed_within_fifty_years_of_now() {
    let now = days_from_civil(2026, 11, 5) * 86_400;
    let year = |value: &str| {
        year_of_day(
            parse_http_date(value, now)
                .expect("parses")
                .div_euclid(86_400),
        )
    };
    assert_eq!(year("Sunday, 06-Nov-94 08:49:37 GMT"), 1994);
    assert_eq!(year("Thursday, 01-Jan-76 00:00:00 GMT"), 2076);
    assert_eq!(year("Friday, 01-Jan-77 00:00:00 GMT"), 1977);
    assert_eq!(year("Monday, 01-Jan-26 00:00:00 GMT"), 2026);
}

/// Delay-seconds read as seconds (saturating past `u64`), a date as the time until it (zero once
/// it has passed), surrounding whitespace ignored, and anything else refused.
#[test]
fn a_retry_after_value_is_read_as_seconds_or_as_the_time_until_its_date() {
    let now = at(RFC_EXAMPLE - 30);
    let cases: [(&str, Option<Duration>); 9] = [
        ("120", Some(Duration::from_secs(120))),
        ("0", Some(Duration::ZERO)),
        (" 7\t", Some(Duration::from_secs(7))),
        (
            "99999999999999999999999",
            Some(Duration::from_secs(u64::MAX)),
        ),
        (
            "Sun, 06 Nov 1994 08:49:37 GMT",
            Some(Duration::from_secs(30)),
        ),
        ("Sat, 05 Nov 1994 08:49:37 GMT", Some(Duration::ZERO)),
        ("-5", None),
        ("1.5", None),
        ("soon", None),
    ];
    for (value, expected) in cases {
        assert_eq!(parse_retry_after(value, now), expected, "{value:?}");
    }
    // A clock before the epoch reads every date as still ahead of it.
    let before_epoch = std::time::UNIX_EPOCH - Duration::from_secs(1);
    assert_eq!(
        parse_retry_after("Thu, 01 Jan 1970 00:00:10 GMT", before_epoch),
        Some(Duration::from_secs(10))
    );
}

/// The outcome reads exactly one `Retry-After` off the response: none on a transport failure, on
/// a response without one, or on one carrying two.
#[test]
fn retry_after_reads_the_single_header_of_a_response() {
    let now = at(RFC_EXAMPLE);
    let sent = request();
    let response = response_with_retry_after(&["5"]);
    let outcome = RetryOutcome::new(&sent, Ok(&response));
    assert_eq!(outcome.retry_after(now), Some(Duration::from_secs(5)));
    let bare = response_with_retry_after(&[]);
    assert_eq!(RetryOutcome::new(&sent, Ok(&bare)).retry_after(now), None);
    let twice = response_with_retry_after(&["5", "6"]);
    assert_eq!(RetryOutcome::new(&sent, Ok(&twice)).retry_after(now), None);
    let unreadable = crate::tests::from_http(
        http::Response::builder()
            .header(
                "retry-after",
                http::HeaderValue::from_bytes(b"\xff").expect("an opaque value"),
            )
            .body(String::new())
            .expect("valid response"),
    );
    assert_eq!(
        RetryOutcome::new(&sent, Ok(&unreadable)).retry_after(now),
        None
    );
    let failure = crate::tests::transport_failure(503);
    assert_eq!(
        RetryOutcome::new(&sent, Err(&failure)).retry_after(now),
        None
    );
}

/// The server's `Retry-After` is a floor under the exponential backoff: a longer one wins, even
/// past `max`; a shorter one, or none, leaves the backoff as it is.
#[test]
fn backoff_takes_the_servers_delay_as_a_floor() {
    let now = at(RFC_EXAMPLE);
    let sent = request();
    let (base, max) = (Duration::from_millis(100), Duration::from_secs(1));
    let long = response_with_retry_after(&["30"]);
    let outcome = RetryOutcome::new(&sent, Ok(&long));
    assert_eq!(outcome.backoff(0, base, max, now), Duration::from_secs(30));
    let dated = response_with_retry_after(&["Sun, 06 Nov 1994 08:49:39 GMT"]);
    let outcome = RetryOutcome::new(&sent, Ok(&dated));
    assert_eq!(outcome.backoff(0, base, max, now), Duration::from_secs(2));
    let short = response_with_retry_after(&["0"]);
    let outcome = RetryOutcome::new(&sent, Ok(&short));
    for attempt in 0..6 {
        assert_eq!(
            outcome.backoff(attempt, base, max, now),
            exponential_backoff(attempt, base, max)
        );
    }
    let bare = response_with_retry_after(&[]);
    let outcome = RetryOutcome::new(&sent, Ok(&bare));
    assert_eq!(
        outcome.backoff(2, base, max, now),
        Duration::from_millis(400)
    );
}

proptest::proptest! {
    /// Every IMF-fixdate of a valid day and time parses to the instant its fields name, read back
    /// through the inverse day count.
    #[test]
    fn an_imf_fixdate_parses_to_the_instant_it_names(
        days in -719_162i64..2_932_896,
        second_of_day in 0i64..86_400,
    ) {
        let year = year_of_day(days);
        let start = days_from_civil(year, 1, 1);
        proptest::prop_assert!(start <= days && days < days_from_civil(year + 1, 1, 1));
        let (mut month, mut first) = (1u32, start);
        while month < 12 && days_from_civil(year, month + 1, 1) <= days {
            month += 1;
            first = days_from_civil(year, month, 1);
        }
        let day = days - first + 1;
        let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
            [usize::try_from(days.rem_euclid(7)).unwrap()];
        let month_name = super::MONTHS[usize::try_from(month - 1).unwrap()];
        let value = format!(
            "{weekday}, {day:02} {month_name} {year:04} {:02}:{:02}:{:02} GMT",
            second_of_day / 3600,
            second_of_day / 60 % 60,
            second_of_day % 60,
        );
        proptest::prop_assert_eq!(
            parse_http_date(&value, 0),
            Some(days * 86_400 + second_of_day),
            "{}", value
        );
    }

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
