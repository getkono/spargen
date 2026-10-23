//! A retry adapter for the transport seam: [`RetryBackend`] wraps any inner [`HttpBackend`] and
//! re-executes a request per a caller-supplied [`RetryPolicy`], returning the last outcome once the
//! policy stops or the request can no longer be replayed.
//!
//! ## Bring-your-own timing (no async timer in the runtime)
//!
//! The runtime has no async timer of its own, and the retry path never pulls in `tokio`, so the
//! *wait* between attempts is supplied by the caller: [`RetryPolicy::retry`] returns the backoff
//! as a boxed `Future` that the caller builds with their own runtime's timer (e.g.
//! `tokio::time::sleep`).
//! [`RetryBackend`] simply `.await`s that future; it never sleeps itself. A pure
//! [`exponential_backoff`] helper computes the delay [`Duration`] a policy hands to its timer, so
//! exponential backoff needs no runtime support either.
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
//!
//! // A policy that retries transient failures up to five times with exponential backoff,
//! // using the caller's tokio timer for the wait — no timer lives in the runtime. The wait is a
//! // `RetryWait`, which is `Send` on native and not on `wasm32`, so the policy compiles on both.
//! struct Backoff;
//! impl RetryPolicy for Backoff {
//!     fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>> {
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

use crate::error::is_transient_status;
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

    /// Whether the outcome is transient by the runtime's default classifier: exactly what
    /// [`crate::Error::is_transient`] answers for the error this outcome becomes. A `429` or `5xx`
    /// response is transient. A transport failure is classified as [`crate::Error::from_reqwest`]
    /// classifies it, so a timeout or a failed connection is transient, and a redirect-policy or
    /// protocol failure is not. Neither is a request reqwest refused before sending anything — a
    /// URL scheme other than `http`/`https`, plain `http` on an `https_only` client — which is
    /// refused again on every attempt, nor any other request-kind failure, which reqwest may raise
    /// after the request was transmitted. On `wasm32` that includes a failed connection: reqwest
    /// offers no connect classification there, so it stays a request-kind failure and is not
    /// transient. Policies are free to ignore this and key on
    /// [`Self::status`] / [`Self::transport_error`] directly.
    pub fn is_transient(&self) -> bool {
        match self {
            RetryOutcome::Transport(error) => error.is_transient(),
            RetryOutcome::Response(response) => is_transient_status(response.status()),
        }
    }
}

/// A caller-supplied retry policy: it decides whether to retry and, crucially, *provides the wait*.
///
/// [`RetryBackend`] calls [`retry`](RetryPolicy::retry) after each attempt. Returning
/// `Some(future)` means "retry after awaiting this future"; the future carries the backoff delay,
/// built with the caller's own async timer (e.g. `tokio::time::sleep`) — this is how timing stays
/// caller-owned and the retry path avoids a `tokio` dependency. Returning `None` stops retrying and the
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
/// `Client::with_backend`. See this module's docs for the timing and cloneability contracts.
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
mod tests;
