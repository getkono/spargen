use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::ResponseValue;

/// The closed error taxonomy shared by every spargen-generated client (PRD FR5). `E` is the
/// operation's typed error body (an enum when several error statuses are documented).
///
/// Nine variants are constructed; taxonomy class #10 (cancellation) is a documented drop-safety
/// guarantee, not a variant (see the crate docs). Every variant implements [`std::error::Error`]
/// with full source chains, and `Debug` never leaks secrets.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// #1 — invalid base URL, or parameter/body serialization failure (near-impossible by
    /// construction).
    RequestConstruction(RequestError),
    /// #2 — DNS failure, connection refused/reset, TLS handshake or certificate error.
    Transport(TransportError),
    /// #3 — connect vs total-request timeout (as configured on the injected client).
    Timeout(TimeoutKind),
    /// #4 — malformed HTTP or decompression failure.
    Protocol(ProtocolError),
    /// #5 — redirect-policy exhaustion (per the injected client's policy).
    Redirect(RedirectError),
    /// #6 — a documented non-success status parsed into the operation's typed error body.
    Api(ResponseValue<E>),
    /// #7 — an undocumented status; the raw body is preserved for forensics.
    UnexpectedStatus {
        /// The response status.
        status: StatusCode,
        /// The response headers.
        headers: HeaderMap,
        /// The raw response body.
        body: Bytes,
    },
    /// #8 — the response body failed to deserialize; retains the serde error path and (capped) raw
    /// body (PRD D7).
    Decode {
        /// The serde deserialization error path.
        path: String,
        /// The retained raw body (up to the configured cap).
        body: Bytes,
        /// Whether the retained body was truncated at the cap.
        truncated: bool,
    },
    /// #9 — the connection dropped mid-stream on a streamed response.
    InterruptedBody(TransportError),
}

impl<E> Error<E> {
    /// Whether the failure is worth retrying: transport failures, timeouts, `429`, and `5xx`
    /// (PRD FR5, §3.2.6). Lets callers wrap any retry policy around the client without spargen
    /// shipping one.
    pub fn is_transient(&self) -> bool {
        todo!()
    }
}

impl<E: std::fmt::Display> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl<E: std::error::Error + 'static> std::error::Error for Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

/// Request-construction failure (taxonomy #1).
#[derive(Debug)]
pub struct RequestError {
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

/// Transport-layer failure (taxonomy #2 / #9).
#[derive(Debug)]
pub struct TransportError {
    source: reqwest::Error,
}

/// Which timeout elapsed (taxonomy #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// The connect timeout.
    Connect,
    /// The total-request timeout.
    Total,
}

/// Protocol-layer failure — malformed HTTP or decompression (taxonomy #4).
#[derive(Debug)]
pub struct ProtocolError {
    source: reqwest::Error,
}

/// Redirect-policy exhaustion (taxonomy #5).
#[derive(Debug)]
pub struct RedirectError {
    source: reqwest::Error,
}

macro_rules! impl_source_error {
    ($ty:ty, $field:ident) => {
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                todo!()
            }
        }
        impl std::error::Error for $ty {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                todo!()
            }
        }
    };
}

impl_source_error!(RequestError, source);
impl_source_error!(TransportError, source);
impl_source_error!(ProtocolError, source);
impl_source_error!(RedirectError, source);
