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
//! [`HttpBody::try_clone`] returns `None` when the body is a one-shot stream that cannot be
//! rewound. Such a request is executed **exactly once** and its outcome returned unretried — silently
//! replaying half a consumed stream would send a corrupt body. Requests with an in-memory (or empty)
//! body clone freely and retry normally; the clone carries the request's method, URI, version,
//! headers and extensions too.
//!
//! ## Idempotency
//!
//! A request that timed out waiting for its response may already have been processed, so
//! re-sending it repeats whatever it did. [`RetryOutcome::is_transient`], the default classifier,
//! therefore answers `true` only for a request that is safe to repeat: one whose method is
//! idempotent by [RFC 9110 §9.2.2](https://www.rfc-editor.org/rfc/rfc9110#section-9.2.2) (`GET`,
//! `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`), or one the caller marked idempotent by sending an
//! `Idempotency-Key` header, which asks the server to apply the request at most once. A policy that
//! retries anything else keys on [`RetryOutcome::status`] / [`RetryOutcome::transport_error`]
//! directly.
//!
//! ## `Retry-After` and sizing the backoff
//!
//! A `429` or `503` may say how long to wait in a `Retry-After` header, as delay-seconds or as an
//! HTTP-date. [`RetryOutcome::retry_after`] parses both, and [`RetryOutcome::backoff`] takes that
//! delay as a floor under [`exponential_backoff`], so the client never comes back sooner than the
//! server asked. The floor is not capped by the backoff's `max`: a policy unwilling to wait that
//! long stops instead of retrying early.
//!
//! Size `base` and `max` against the failures being retried: a backoff shorter than the network's
//! loss bursts or the server's outages lands every retry inside the same bad state and spends the
//! budget there. Pick a `base` above a typical loss burst and a `max` above a typical outage, so
//! later attempts arrive after the condition has cleared.
//!
//! ```ignore
//! use std::sync::Arc;
//! use std::time::{Duration, SystemTime};
//!
//! // A policy that retries transient failures of idempotent requests up to five times with
//! // exponential backoff, never sooner than the server's `Retry-After`, using the caller's tokio
//! // timer for the wait — no timer lives in the runtime. The wait is a `RetryWait`, which is `Send`
//! // on native and not on `wasm32`, so the policy compiles on both.
//! struct Backoff;
//! impl RetryPolicy for Backoff {
//!     fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>> {
//!         if attempt < 5 && outcome.is_transient() {
//!             let max = Duration::from_secs(5);
//!             let wait = outcome.backoff(attempt, Duration::from_millis(100), max, SystemTime::now());
//!             // A server asking for more than a minute is not worth waiting on here.
//!             (wait <= Duration::from_secs(60)).then(|| Box::pin(tokio::time::sleep(wait)) as RetryWait<'a>)
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

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::RETRY_AFTER;
use reqwest::{Method, StatusCode};

use crate::error::is_transient_status;
use crate::transport::{try_clone_request, ExecuteFuture, HttpBackend};
use crate::wasm::BoxFuture;
use crate::{HttpBody, MaybeSend, MaybeSync, TransportError};

/// A request as the transport seam carries it.
type Request = http::Request<HttpBody>;
/// A response as the transport seam carries it.
type Response = http::Response<HttpBody>;

/// The boxed backoff future a [`RetryPolicy`] returns to request a retry after the wait completes.
///
/// `Send` off `wasm32` and not on it, as the `wasm` module explains.
pub type RetryWait<'a> = BoxFuture<'a, ()>;

/// What one execution of a request produced, handed to [`RetryPolicy::retry`] so it can decide
/// whether to retry: the [`http::Request`] that was sent, and either the raw [`http::Response`] the inner
/// backend returned (any status — success or error) or the [`TransportError`] it failed with (DNS,
/// connect, timeout, …).
///
/// The policy inspects it to make its decision: [`RetryOutcome::status`] exposes the response status
/// (e.g. retry on `429`/`5xx`), [`RetryOutcome::transport_error`] the transport failure (e.g. retry
/// on a timeout), [`RetryOutcome::method`] the request method, [`RetryOutcome::retry_after`] the
/// server's requested delay, and [`RetryOutcome::is_transient`] applies the runtime's own
/// classifier as a sensible default.
#[derive(Debug, Clone, Copy)]
pub struct RetryOutcome<'a> {
    request: &'a Request,
    result: Result<&'a Response, &'a TransportError>,
}

impl<'a> RetryOutcome<'a> {
    /// The outcome of sending `request`: `Ok` with the response the inner backend returned, or
    /// `Err` with the transport failure it reported.
    pub fn new(request: &'a Request, result: Result<&'a Response, &'a TransportError>) -> Self {
        Self { request, result }
    }

    /// The request this attempt sent, as the next attempt would resend it.
    pub fn request(&self) -> &'a Request {
        self.request
    }

    /// The request's method.
    pub fn method(&self) -> &'a Method {
        self.request.method()
    }

    /// The response, if the attempt produced one (any status). `None` on a transport failure.
    pub fn response(&self) -> Option<&'a Response> {
        self.result.ok()
    }

    /// The transport failure, if the attempt failed before producing a response. `None` otherwise.
    pub fn transport_error(&self) -> Option<&'a TransportError> {
        self.result.err()
    }

    /// The response status, if any. `None` on a transport failure — a policy keying purely on
    /// status can treat that as "retry" or "stop" as it sees fit.
    pub fn status(&self) -> Option<StatusCode> {
        self.response().map(Response::status)
    }

    /// The outcome as a `Result`, mirroring what the inner backend returned.
    pub fn result(&self) -> Result<&'a Response, &'a TransportError> {
        self.result
    }

    /// Whether the request is safe to send again: its method is idempotent by RFC 9110 §9.2.2
    /// (`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`), or it carries an `Idempotency-Key`
    /// header, which marks a request of any method as one the server applies at most once.
    pub fn is_idempotent(&self) -> bool {
        is_idempotent_method(self.method()) || self.request.headers().contains_key(IDEMPOTENCY_KEY)
    }

    /// Whether the outcome is worth retrying by the runtime's default classifier: the request
    /// [is idempotent](Self::is_idempotent), and the failure is one [`crate::Error::is_transient`]
    /// calls transient for the error this outcome becomes. A `429` or `5xx` response is transient.
    /// A transport failure is transient when the class its [`TransportError`] carries is: a
    /// timeout, a failed connection, or an interrupted body is, and a redirect-policy, protocol,
    /// or request-kind failure ([`crate::RequestError::Other`]) is not. [`crate::ReqwestBackend`]
    /// classifies as [`crate::TransportError::from_reqwest`] documents, so on `wasm32` a failed connection
    /// is request-kind there. A `POST` or `PATCH` without an `Idempotency-Key` is never
    /// transient here, since the server may already have applied it. Policies are free to ignore
    /// this and key on [`Self::status`] / [`Self::transport_error`] directly.
    pub fn is_transient(&self) -> bool {
        self.is_idempotent()
            && match self.result {
                Err(error) => error.is_transient(),
                Ok(response) => is_transient_status(response.status()),
            }
    }

    /// The delay the response's `Retry-After` header asks for, measured from `now`: its
    /// delay-seconds, or the time from `now` until its HTTP-date (zero once that date has passed).
    /// All three HTTP-date forms RFC 9110 §5.6.7 requires a recipient to accept are read: the
    /// IMF-fixdate, the obsolete RFC 850 form, and the asctime form. `None` on a transport failure,
    /// and for a response with no `Retry-After`, more than one, or one that does not parse.
    ///
    /// `now` is the caller's clock (`SystemTime::now()` off `wasm32`); the runtime reads none.
    pub fn retry_after(&self, now: SystemTime) -> Option<Duration> {
        let mut values = self.response()?.headers().get_all(RETRY_AFTER).iter();
        let value = values.next()?;
        if values.next().is_some() {
            return None;
        }
        parse_retry_after(value.to_str().ok()?, now)
    }

    /// The wait before the next attempt: [`exponential_backoff`] of `attempt`, `base` and `max`,
    /// raised to the response's [`Self::retry_after`] delay where that is longer. The server's
    /// delay is a floor the backoff's `max` does not cap; a policy unwilling to wait that long
    /// stops instead of retrying.
    pub fn backoff(
        &self,
        attempt: u32,
        base: Duration,
        max: Duration,
        now: SystemTime,
    ) -> Duration {
        let wait = exponential_backoff(attempt, base, max);
        self.retry_after(now).map_or(wait, |floor| wait.max(floor))
    }
}

/// The header that marks a request idempotent whatever its method
/// (draft-ietf-httpapi-idempotency-key-header).
const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// Whether `method` is idempotent by RFC 9110 §9.2.2.
fn is_idempotent_method(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::TRACE,
        Method::PUT,
        Method::DELETE,
    ]
    .contains(method)
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
    /// `None` to stop and return the outcome. The wait is a [`RetryWait`].
    fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>>;
}

/// An [`HttpBackend`] that retries requests through an inner backend per a [`RetryPolicy`].
///
/// It takes effect only when a caller wraps their backend in it and installs it via
/// `Client::with_backend`. See this module's docs for the timing and cloneability contracts.
///
/// A request's [`crate::TotalTimeout`] bounds each attempt, not the whole retried call: every
/// replay carries the same extension to the inner backend, and the policy's backoff runs between
/// attempts outside any of them. A call retried `n` times may therefore take `n + 1` timeouts plus
/// the backoffs; a policy that must meet an overall deadline stops retrying once its waits would
/// pass it.
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
        // The future borrows `self` for the `'_` the seam's `ExecuteFuture` allows, so the inner
        // backend and the policy are reached through that borrow rather than cloned per request.
        Box::pin(async move {
            let mut attempt: u32 = 0;
            loop {
                // Clone the request for THIS attempt, keeping the original available for a retry. A
                // one-shot streaming body cannot be replayed (`try_clone` is `None`): execute the
                // original exactly once and return, never retrying a body we cannot resend intact.
                let Some(replay) = try_clone_request(&request) else {
                    return self.inner.execute(request).await;
                };
                let result = self.inner.execute(replay).await;
                // Ask the policy within a scope that ends the `outcome` borrow of `result` before we
                // await the wait or return the result. The returned wait future borrows the policy
                // (which outlives the loop), not `outcome`, so it escapes the scope cleanly.
                let decision = {
                    let outcome = RetryOutcome::new(&request, result.as_ref());
                    self.policy.retry(attempt, &outcome)
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

/// A `Retry-After` field value as the delay it asks for from `now` (RFC 9110 §10.2.3): its
/// delay-seconds, saturating where they exceed `u64`, or the time until its HTTP-date, zero once
/// that has passed. `None` when the value is neither.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim_matches([' ', '\t']);
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
    });
    let at = parse_http_date(value, now)?;
    Some(Duration::from_secs(
        u64::try_from(at.saturating_sub(now)).unwrap_or(0),
    ))
}

const SHORT_DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const LONG_DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// An HTTP-date (RFC 9110 §5.6.7) as seconds since the Unix epoch, in any of its three forms:
/// `Sun, 06 Nov 1994 08:49:37 GMT`, `Sunday, 06-Nov-94 08:49:37 GMT`, and
/// `Sun Nov  6 08:49:37 1994`. `now`, in the same seconds, places the RFC 850 form's two-digit
/// year: the year with those last two digits that is not more than 50 years after `now`'s.
fn parse_http_date(value: &str, now: i64) -> Option<i64> {
    let (year, month, day, time) = if let Some((name, rest)) = value.split_once(", ") {
        if SHORT_DAYS.contains(&name) {
            // IMF-fixdate: `06 Nov 1994 08:49:37 GMT`.
            let mut fields = rest.split(' ');
            let day = digits(fields.next()?, 2)?;
            let month = month(fields.next()?)?;
            let year = i64::from(digits(fields.next()?, 4)?);
            let time = fields.next()?;
            (fields.next()? == "GMT" && fields.next().is_none()).then_some(())?;
            (year, month, day, time)
        } else if LONG_DAYS.contains(&name) {
            // rfc850-date: `06-Nov-94 08:49:37 GMT`.
            let mut fields = rest.split(' ');
            let mut date = fields.next()?.split('-');
            let day = digits(date.next()?, 2)?;
            let month = month(date.next()?)?;
            let short_year = i64::from(digits(date.next()?, 2)?);
            date.next().is_none().then_some(())?;
            let time = fields.next()?;
            (fields.next()? == "GMT" && fields.next().is_none()).then_some(())?;
            let current = year_of_day(now.div_euclid(SECONDS_PER_DAY));
            let mut year = current - current.rem_euclid(100) + short_year;
            if year > current + 50 {
                year -= 100;
            }
            (year, month, day, time)
        } else {
            return None;
        }
    } else {
        // asctime-date: `Sun Nov  6 08:49:37 1994`, the day of month space-padded.
        let name = value.get(0..3)?;
        let separators = [3, 7, 10, 19]
            .iter()
            .all(|&at| value.get(at..=at) == Some(" "));
        (SHORT_DAYS.contains(&name) && separators && value.len() == 24).then_some(())?;
        let month = month(value.get(4..7)?)?;
        let day = value.get(8..10)?;
        let day = digits(day.strip_prefix(' ').unwrap_or(day), day.trim_start().len())?;
        let year = i64::from(digits(value.get(20..24)?, 4)?);
        (year, month, day, value.get(11..19)?)
    };
    let mut clock = time.split(':');
    let hour = digits(clock.next()?, 2)?;
    let minute = digits(clock.next()?, 2)?;
    let second = digits(clock.next()?, 2)?;
    let valid = clock.next().is_none()
        && hour < 24
        && minute < 60
        && second <= 60
        && (1..=days_in_month(year, month)).contains(&day);
    valid.then(|| {
        days_from_civil(year, month, day) * SECONDS_PER_DAY
            + i64::from(hour * 3600 + minute * 60 + second)
    })
}

const SECONDS_PER_DAY: i64 = 86_400;

/// `text` as a decimal number, when it is exactly `len` ASCII digits.
fn digits(text: &str, len: usize) -> Option<u32> {
    (text.len() == len && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The 1-based month a three-letter month name names.
fn month(name: &str) -> Option<u32> {
    let index = MONTHS.iter().position(|month| *month == name)?;
    u32::try_from(index + 1).ok()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * i64::from((month + 9) % 12) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian year of a day counted from 1970-01-01 (Hinnant's `civil_from_days`).
fn year_of_day(days: i64) -> i64 {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let year = year_of_era + era * 400;
    // March-based months 10 and 11 are January and February of the next civil year.
    if shifted_month >= 10 {
        year + 1
    } else {
        year
    }
}

#[cfg(test)]
mod tests;
