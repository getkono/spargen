//! A retry adapter for the transport seam: [`RetryBackend`] wraps any inner [`HttpBackend`] and
//! re-executes a request per a caller-supplied [`RetryPolicy`], returning the last outcome once the
//! policy stops or the request can no longer be replayed.
//!
//! ## Bring-your-own timing (no async timer in the runtime)
//!
//! The runtime's dependency set is fixed at reqwest/serde/serde_json/bytes/secrecy — it has no
//! async timer of its own and never pulls in `tokio`. So the *wait* between attempts is supplied by
//! the caller: [`RetryPolicy::retry`] returns the backoff as a boxed `Future` that the caller builds
//! with their own runtime's timer (e.g. `tokio::time::sleep`). [`RetryBackend`] simply `.await`s
//! that future; it never sleeps itself. A pure [`exponential_backoff`] helper computes the delay
//! [`Duration`] a policy hands to its timer, so exponential backoff needs no runtime support either.
//!
//! ## Request replay and cloneability
//!
//! A retry re-sends the *same* request, which means it must be cloned before each attempt.
//! [`reqwest::Request::try_clone`] returns `None` when the body is a one-shot stream that cannot be
//! rewound. Such a request is executed **exactly once** and its outcome returned unretried — silently
//! replaying half a consumed stream would send a corrupt body. Requests with an in-memory (or empty)
//! body clone freely and retry normally.
//!
//! ```ignore
//! use std::sync::Arc;
//! use std::time::Duration;
//! use std::pin::Pin;
//! use std::future::Future;
//!
//! // A policy that retries transient failures up to five times with exponential backoff,
//! // using the caller's tokio timer for the wait — no timer lives in the runtime.
//! struct Backoff;
//! impl RetryPolicy for Backoff {
//!     fn retry<'a>(
//!         &'a self,
//!         attempt: u32,
//!         outcome: &RetryOutcome<'_>,
//!     ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
//!         if attempt < 5 && outcome.is_transient() {
//!             let wait = exponential_backoff(attempt, Duration::from_millis(100), Duration::from_secs(5));
//!             Some(Box::pin(tokio::time::sleep(wait)))
//!         } else {
//!             None
//!         }
//!     }
//! }
//!
//! let inner: Arc<dyn HttpBackend> = Arc::new(ReqwestBackend::new(reqwest::Client::new()));
//! let backend: Arc<dyn HttpBackend> = Arc::new(RetryBackend::new(inner, Arc::new(Backoff)));
//! let client = Client::with_backend(backend, "https://api.example.com")?;
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Request, Response, StatusCode};

use crate::transport::{ExecuteFuture, HttpBackend};
use crate::{MaybeSend, MaybeSync, TransportError};

/// The boxed backoff future a [`RetryPolicy`] returns to request a retry after the wait completes.
///
/// `Send` on native (the retry loop's future is shared across tasks) but not on `wasm32`, where the
/// single-threaded browser has no `Send` futures. As with [`crate::ExecuteFuture`], `Send` is an
/// auto trait and cannot be swapped for the non-auto [`MaybeSend`] as an extra trait-object bound,
/// so this alias is `cfg`-gated rather than expressed through `MaybeSend`.
#[cfg(not(target_arch = "wasm32"))]
pub type RetryWait<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
/// The boxed backoff future a [`RetryPolicy`] returns (the wasm variant: no `Send`).
#[cfg(target_arch = "wasm32")]
pub type RetryWait<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

/// What one execution of a request produced, handed to [`RetryPolicy::retry`] so it can decide
/// whether to retry. It is either the raw [`Response`] the inner backend returned (any status —
/// success or error), or the [`TransportError`] it failed with (DNS, connect, timeout, …).
///
/// The policy inspects it to make its decision: [`RetryOutcome::status`] exposes the response status
/// (e.g. retry on `429`/`5xx`), [`RetryOutcome::transport_error`] the transport failure (e.g. retry
/// on a timeout), and [`RetryOutcome::is_transient`] applies the runtime's own transient-failure
/// classifier as a sensible default.
#[derive(Debug)]
pub enum RetryOutcome<'a> {
    /// The inner backend returned a response with this status; the body has not been read yet.
    Response(&'a Response),
    /// The inner backend failed at the transport layer before producing a response.
    Transport(&'a TransportError),
}

impl<'a> RetryOutcome<'a> {
    /// The response, if the attempt produced one (any status). `None` on a transport failure.
    pub fn response(&self) -> Option<&Response> {
        match self {
            RetryOutcome::Response(response) => Some(response),
            RetryOutcome::Transport(_) => None,
        }
    }

    /// The transport failure, if the attempt failed before producing a response. `None` otherwise.
    pub fn transport_error(&self) -> Option<&TransportError> {
        match self {
            RetryOutcome::Transport(error) => Some(error),
            RetryOutcome::Response(_) => None,
        }
    }

    /// The response status, if any. `None` on a transport failure — a policy keying purely on
    /// status can treat that as "retry" or "stop" as it sees fit.
    pub fn status(&self) -> Option<StatusCode> {
        self.response().map(Response::status)
    }

    /// The outcome as a `Result`, mirroring what the inner backend returned.
    pub fn result(&self) -> Result<&Response, &TransportError> {
        match self {
            RetryOutcome::Response(response) => Ok(response),
            RetryOutcome::Transport(error) => Err(error),
        }
    }

    /// Whether the outcome is transient by the runtime's default classifier — the same rule
    /// [`crate::Error::is_transient`] applies: any transport failure, plus a `429` or `5xx`
    /// response. Policies are free to ignore this and key on [`Self::status`] /
    /// [`Self::transport_error`] directly.
    pub fn is_transient(&self) -> bool {
        match self {
            RetryOutcome::Transport(_) => true,
            RetryOutcome::Response(response) => {
                let status = response.status();
                status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
        }
    }
}

/// A caller-supplied retry policy: it decides whether to retry and, crucially, *provides the wait*.
///
/// [`RetryBackend`] calls [`retry`](RetryPolicy::retry) after each attempt. Returning
/// `Some(future)` means "retry after awaiting this future"; the future carries the backoff delay,
/// built with the caller's own async timer (e.g. `tokio::time::sleep`) — this is how timing stays
/// caller-owned and the runtime avoids a `tokio` dependency. Returning `None` stops retrying and the
/// last outcome is returned.
pub trait RetryPolicy: MaybeSend + MaybeSync {
    /// Called after attempt `attempt` (0-based) produced `outcome`. Return `Some(wait)` to retry
    /// after awaiting `wait` (which encapsulates the backoff delay using the caller's timer), or
    /// `None` to stop and return the outcome. The wait is a [`RetryWait`] — a boxed future that is
    /// `Send` on native and `!Send` on `wasm32`.
    fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>>;
}

/// An [`HttpBackend`] that retries requests through an inner backend per a [`RetryPolicy`].
///
/// Purely additive: it only takes effect when a caller wraps their backend in it and installs it via
/// `Client::with_backend`. See the [module docs](self) for the timing and cloneability contracts.
#[derive(Clone)]
pub struct RetryBackend {
    inner: Arc<dyn HttpBackend>,
    policy: Arc<dyn RetryPolicy>,
}

impl RetryBackend {
    /// Wrap `inner` so requests are retried according to `policy`.
    pub fn new(inner: Arc<dyn HttpBackend>, policy: Arc<dyn RetryPolicy>) -> Self {
        Self { inner, policy }
    }

    /// The wrapped inner backend.
    pub fn inner(&self) -> &Arc<dyn HttpBackend> {
        &self.inner
    }
}

// `RetryPolicy` is not `Debug` (it is a caller-supplied closure-like object), so derive would not
// apply; the manual impl keeps `RetryBackend: Debug` — required for `HttpBackend` — without
// constraining the policy.
impl std::fmt::Debug for RetryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryBackend")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl HttpBackend for RetryBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        // Own clones of the `Arc`s so the returned future is self-contained (`'static`) — no borrow
        // of `&self`, matching the seam's object-safety expectations.
        let inner = self.inner.clone();
        let policy = self.policy.clone();
        Box::pin(async move {
            let mut attempt: u32 = 0;
            loop {
                // Clone the request for THIS attempt, keeping the original available for a retry. A
                // one-shot streaming body cannot be replayed (`try_clone` is `None`): execute the
                // original exactly once and return, never retrying a body we cannot resend intact.
                let Some(replay) = request.try_clone() else {
                    return inner.execute(request).await;
                };
                let result = inner.execute(replay).await;
                // Ask the policy within a scope that ends the `outcome` borrow of `result` before we
                // await the wait or return the result. The returned wait future borrows `policy`
                // (which outlives the loop), not `outcome`, so it escapes the scope cleanly.
                let decision = {
                    let outcome = match &result {
                        Ok(response) => RetryOutcome::Response(response),
                        Err(error) => RetryOutcome::Transport(error),
                    };
                    policy.retry(attempt, &outcome)
                };
                match decision {
                    Some(wait) => {
                        wait.await;
                        // Saturate rather than wrap so a pathological policy that never stops still
                        // reports a monotonic, bounded attempt number.
                        attempt = attempt.saturating_add(1);
                    }
                    None => return result,
                }
            }
        })
    }
}

/// Exponential backoff: `base * 2^attempt`, clamped to `max`. A pure helper a [`RetryPolicy`] can
/// feed to its own async timer; the runtime never sleeps on it.
///
/// `attempt` is 0-based, so attempt 0 waits `base`, attempt 1 waits `2 * base`, and so on. Overflow
/// (a large `attempt` or `base`) saturates to `max` rather than panicking.
pub fn exponential_backoff(attempt: u32, base: Duration, max: Duration) -> Duration {
    // `2^attempt` saturates at `u32::MAX`; a `base * factor` overflow then falls back to `max`.
    let factor = 2u32.saturating_pow(attempt);
    base.checked_mul(factor).unwrap_or(max).min(max)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use reqwest::{Method, Request};

    use crate::transport::{ExecuteFuture, HttpBackend};

    use super::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy};

    /// The retry loop never actually suspends here: the inner backend's canned responses are ready
    /// immediately and the test policy's wait is an already-ready future, so a single poll with a
    /// noop waker drives the whole thing — no async runtime, no real sleep.
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
}
