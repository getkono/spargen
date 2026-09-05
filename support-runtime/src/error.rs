use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::ResponseValue;

/// The closed error taxonomy shared by every spargen-generated client. `E` is the
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
    /// #8 — the response body failed to deserialize; retains the serde error path and the raw
    /// body, capped on every path but the two named on `body` below.
    Decode {
        /// The serde deserialization error path.
        path: String,
        /// The retained raw body, capped at `max_error_body` by the dispatch and decode helpers.
        ///
        /// Two paths do not reach the cap, for different reasons, and retain more than it:
        ///
        /// - The generated shim for an operation with more than one documented success status
        ///   reads through `read_success_body`, which does not take the cap. Threading it through
        ///   changes emitted output, so this is deferred rather than accepted. The same shim
        ///   reaches `UnexpectedStatus` uncapped too.
        /// - `EventStream`'s per-frame decode retains one frame, already detached by the framer.
        ///   The cap does not apply because the bound there is the frame, not the response — but
        ///   the frame buffer itself is unbounded, so this is a real gap, not a safe exemption.
        body: Bytes,
        /// Whether bytes were dropped from `body` to meet the cap.
        ///
        /// `false` therefore means "nothing was dropped", not "the cap was applied" — on the two
        /// paths above nothing is dropped because nothing is capped.
        truncated: bool,
    },
    /// #9 — the connection dropped mid-stream on a streamed response.
    InterruptedBody(TransportError),
}

impl<E> Error<E> {
    /// Build a request-construction error from any owned error value.
    pub fn request_construction(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::RequestConstruction(RequestError {
            source: Some(Box::new(source)),
        })
    }

    /// Build a request-construction error from a static message.
    pub fn request_message(message: impl Into<String>) -> Self {
        Self::RequestConstruction(RequestError {
            source: Some(Box::new(MessageError(message.into()))),
        })
    }

    /// Classify a reqwest error into the closest runtime taxonomy class.
    pub fn from_reqwest(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Error::Timeout(TimeoutKind::Total)
        } else if error.is_redirect() {
            Error::Redirect(RedirectError { source: error })
        } else if error.is_decode() {
            Error::Protocol(ProtocolError { source: error })
        } else if error.is_request() {
            Error::RequestConstruction(RequestError {
                source: Some(Box::new(error)),
            })
        } else {
            Error::Transport(TransportError { source: error })
        }
    }

    /// Whether the failure is worth retrying: transport failures, timeouts, `429`, and `5xx`
    /// Lets callers wrap any retry policy around the client without spargen
    /// shipping one.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
            Error::Api(value) => {
                value.status() == StatusCode::TOO_MANY_REQUESTS || value.status().is_server_error()
            }
            Error::UnexpectedStatus { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            Error::RequestConstruction(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::Decode { .. } => false,
        }
    }
}

impl Error<std::convert::Infallible> {
    /// Widen a never-typed failure into any operation's error type. Dispatch routines that cannot
    /// produce a typed API error return `Error<Infallible>`; generated shims widen at the call
    /// site via `.map_err(Error::widen)`.
    pub fn widen<E>(self) -> Error<E> {
        match self {
            Error::RequestConstruction(e) => Error::RequestConstruction(e),
            Error::Transport(e) => Error::Transport(e),
            Error::Timeout(e) => Error::Timeout(e),
            Error::Protocol(e) => Error::Protocol(e),
            Error::Redirect(e) => Error::Redirect(e),
            // Statically uninhabited: an `Error<Infallible>` cannot hold an API error body.
            #[allow(unreachable_code)]
            Error::Api(value) => match value.into_inner() {},
            Error::UnexpectedStatus {
                status,
                headers,
                body,
            } => Error::UnexpectedStatus {
                status,
                headers,
                body,
            },
            Error::Decode {
                path,
                body,
                truncated,
            } => Error::Decode {
                path,
                body,
                truncated,
            },
            Error::InterruptedBody(e) => Error::InterruptedBody(e),
        }
    }
}

/// Implemented by the generated error shapes that carry one documented body type, so code
/// generic over operations can reach that body without naming each `E`.
///
/// Three shapes implement it: a multi-status enum whose bodied statuses reference the same
/// schema (its `body` is `None` for a documented bodyless status, or a `null` payload), the
/// single-body newtype (`Body` is the bare schema type, so a nullable body answers `None` for
/// `null` exactly as the enum does), and the uninhabited `Infallible` shape, so `Error::api_body`
/// exists on those operations. An enum whose statuses carry different body types has no
/// implementation: there is no single body to hand back, and the compile error is the signal.
pub trait ApiErrorBody {
    /// The documented error body type.
    type Body: ?Sized;
    /// The documented body this value carries, if its status documents one.
    fn body(&self) -> Option<&Self::Body>;
}

impl ApiErrorBody for std::convert::Infallible {
    type Body = std::convert::Infallible;
    fn body(&self) -> Option<&Self::Body> {
        match *self {}
    }
}

impl<E: ApiErrorBody> Error<E> {
    /// The documented API error body, whichever status carried it: `Some` only for [`Error::Api`]
    /// whose `E` reports a body. The status itself stays on the `ResponseValue` inside `Api`.
    pub fn api_body(&self) -> Option<&E::Body> {
        match self {
            Error::Api(value) => value.inner().body(),
            // Every other class carries no typed body; a wildcard so a variant added to the
            // taxonomy does not have to be listed here.
            _ => None,
        }
    }
}

impl<E: std::fmt::Display> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::RequestConstruction(_) => f.write_str("request construction failed"),
            Error::Transport(_) => f.write_str("transport failed"),
            Error::Timeout(kind) => write!(f, "{kind:?} timeout elapsed"),
            Error::Protocol(_) => f.write_str("protocol error"),
            Error::Redirect(_) => f.write_str("redirect policy exhausted"),
            Error::Api(value) => write!(f, "documented API error ({})", value.status()),
            Error::UnexpectedStatus { status, .. } => {
                write!(f, "unexpected response status {status}")
            }
            Error::Decode { path, .. } => write!(f, "response decode failed at {path}"),
            Error::InterruptedBody(_) => f.write_str("response body was interrupted"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::RequestConstruction(e) => Some(e),
            Error::Transport(e) => Some(e),
            Error::Protocol(e) => Some(e),
            Error::Redirect(e) => Some(e),
            Error::Api(value) => Some(value.inner()),
            Error::InterruptedBody(e) => Some(e),
            Error::Timeout(_) | Error::UnexpectedStatus { .. } | Error::Decode { .. } => None,
        }
    }
}

#[derive(Debug)]
struct MessageError(String);

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

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

impl TransportError {
    /// Wrap a `reqwest::Error` as a transport failure. [`crate::HttpBackend`] implementations, whose
    /// currency is reqwest's own types, report failures through this so [`crate::send`] can
    /// reclassify them via [`Error::from_reqwest`] — keeping timeout/redirect/protocol
    /// classification identical to executing directly on a `reqwest::Client`.
    pub fn new(source: reqwest::Error) -> Self {
        Self { source }
    }

    /// Consume the transport error, yielding the underlying `reqwest::Error` so [`crate::send`] can
    /// run it back through the full taxonomy classifier.
    pub(crate) fn into_source(self) -> reqwest::Error {
        self.source
    }
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

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{source}"),
            None => f.write_str("request construction failed"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

macro_rules! impl_reqwest_source_error {
    ($ty:ty, $label:literal) => {
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}: {}", $label, self.source)
            }
        }

        impl std::error::Error for $ty {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.source)
            }
        }
    };
}

impl_reqwest_source_error!(TransportError, "transport error");
impl_reqwest_source_error!(ProtocolError, "protocol error");
impl_reqwest_source_error!(RedirectError, "redirect error");

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use reqwest::StatusCode;

    use crate::{ResponseValue, TransportError};

    use super::{Error, TimeoutKind};

    #[test]
    fn retry_classifier_includes_timeouts_and_5xx() {
        let timeout = Error::<String>::Timeout(TimeoutKind::Total);
        assert!(timeout.is_transient());

        let status = Error::<String>::UnexpectedStatus {
            status: StatusCode::SERVICE_UNAVAILABLE,
            headers: HeaderMap::new(),
            body: Bytes::new(),
        };
        assert!(status.is_transient());
    }

    #[test]
    fn retry_classifier_excludes_client_errors() {
        let api = Error::Api(ResponseValue::new(
            StatusCode::BAD_REQUEST,
            HeaderMap::new(),
            "bad".to_owned(),
        ));
        assert!(!api.is_transient());
    }

    #[test]
    fn widen_preserves_the_variant() {
        let narrow = Error::<std::convert::Infallible>::Timeout(TimeoutKind::Total);
        let widened: Error<String> = narrow.widen();
        assert!(matches!(widened, Error::Timeout(TimeoutKind::Total)));
    }

    #[test]
    fn request_message_source_displays_the_message() {
        let error = Error::<std::convert::Infallible>::request_message("no credential for `token`");
        assert!(matches!(error, Error::RequestConstruction(_)));
        let source = std::error::Error::source(&error).expect("request errors carry a source");
        assert_eq!(source.to_string(), "no credential for `token`");
    }

    /// A typed API error body that is itself an `Error`, so `Error::source` can reach it.
    #[derive(Debug)]
    struct ApiBody(&'static str);

    impl std::fmt::Display for ApiBody {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for ApiBody {}

    /// A genuine `reqwest::Error`, obtained the only way that needs neither a network nor an async
    /// runtime: an unparseable URL, whose failure `RequestBuilder::build` surfaces.
    fn reqwest_error() -> reqwest::Error {
        reqwest::Client::new()
            .request(reqwest::Method::GET, "not a url")
            .build()
            .expect_err("an unparseable URL fails to build")
    }

    /// One value of every variant. The match in each test below is exhaustive over this list by
    /// construction, so a variant added to `Error` has to be added here — and then classified.
    fn every_variant() -> Vec<Error<ApiBody>> {
        vec![
            Error::request_message("bad path segment"),
            Error::Transport(TransportError::new(reqwest_error())),
            Error::Timeout(TimeoutKind::Total),
            // The field is private, but these tests live in the defining module.
            Error::Protocol(super::ProtocolError {
                source: reqwest_error(),
            }),
            Error::Redirect(super::RedirectError {
                source: reqwest_error(),
            }),
            Error::Api(ResponseValue::new(
                StatusCode::BAD_REQUEST,
                HeaderMap::new(),
                ApiBody("bad request"),
            )),
            Error::UnexpectedStatus {
                status: StatusCode::IM_A_TEAPOT,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            },
            Error::Decode {
                path: "items[0].id".to_owned(),
                body: Bytes::from_static(b"{}"),
                truncated: false,
            },
            Error::InterruptedBody(TransportError::new(reqwest_error())),
        ]
    }

    /// `from_reqwest` is the taxonomy: every failure a [`crate::HttpBackend`] reports is mapped
    /// through it, which is what keeps a custom transport classifying identically to executing on
    /// a `reqwest::Client` directly. Its timeout, redirect, decode, and request branches all need a
    /// live connection attempt to reach (reqwest exposes no constructor for its own error), so what
    /// is pinned here is the *fallback*: an error reqwest does not classify becomes `Transport`,
    /// and a `Transport` failure is retryable. A backend that reported a permanent failure reqwest
    /// leaves unclassified would therefore be retried, so the fallback is worth stating explicitly.
    #[test]
    fn from_reqwest_falls_back_to_transport_for_an_unclassified_error() {
        let error = Error::<ApiBody>::from_reqwest(reqwest_error());
        assert!(matches!(error, Error::Transport(_)), "{error}");
        assert!(error.is_transient());
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn is_transient_classifies_every_variant() {
        for error in every_variant() {
            let expected = match &error {
                // Worth retrying: the failure is about the connection, not the request.
                Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
                // A documented or undocumented status is retryable only when the server said so.
                Error::Api(value) => {
                    value.status() == StatusCode::TOO_MANY_REQUESTS
                        || value.status().is_server_error()
                }
                Error::UnexpectedStatus { status, .. } => {
                    *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                }
                // Deterministic failures: retrying reproduces them.
                Error::RequestConstruction(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::Decode { .. } => false,
            };
            assert_eq!(
                error.is_transient(),
                expected,
                "is_transient disagrees for {error}"
            );
        }
    }

    #[test]
    fn a_retryable_status_is_retryable_through_both_status_variants() {
        for status in [StatusCode::TOO_MANY_REQUESTS, StatusCode::BAD_GATEWAY] {
            assert!(
                Error::<ApiBody>::UnexpectedStatus {
                    status,
                    headers: HeaderMap::new(),
                    body: Bytes::new(),
                }
                .is_transient(),
                "{status} should be transient as an undocumented status"
            );
            assert!(
                Error::Api(ResponseValue::new(status, HeaderMap::new(), ApiBody("x")))
                    .is_transient(),
                "{status} should be transient as a documented status"
            );
        }
        // The boundary: 499 is a client error, 500 is not.
        assert!(!Error::<ApiBody>::UnexpectedStatus {
            status: StatusCode::from_u16(499).unwrap(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
        .is_transient());
    }

    #[test]
    fn every_variant_displays_something_that_names_its_class() {
        for error in every_variant() {
            let rendered = error.to_string();
            assert!(!rendered.is_empty(), "a variant renders as an empty string");
            let expected = match &error {
                Error::RequestConstruction(_) => "request construction failed",
                Error::Transport(_) => "transport failed",
                Error::Timeout(_) => "timeout elapsed",
                Error::Protocol(_) => "protocol error",
                Error::Redirect(_) => "redirect policy exhausted",
                Error::Api(_) => "documented API error",
                Error::UnexpectedStatus { .. } => "unexpected response status",
                Error::Decode { .. } => "response decode failed",
                Error::InterruptedBody(_) => "response body was interrupted",
            };
            assert!(
                rendered.contains(expected),
                "{rendered:?} does not name its class ({expected:?})"
            );
        }
    }

    /// The variants that wrap a cause expose it; the three that carry only data do not. A caller
    /// walking the chain must not find a phantom source, nor lose a real one.
    #[test]
    fn source_is_present_exactly_where_the_taxonomy_carries_a_cause() {
        for error in every_variant() {
            let expected = match &error {
                Error::RequestConstruction(_)
                | Error::Transport(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::Api(_)
                | Error::InterruptedBody(_) => true,
                Error::Timeout(_) | Error::UnexpectedStatus { .. } | Error::Decode { .. } => false,
            };
            assert_eq!(
                std::error::Error::source(&error).is_some(),
                expected,
                "source() disagrees for {error}"
            );
        }
    }

    /// `widen` is applied by every generated shim, so dropping or reclassifying a variant there
    /// would silently change the error a consumer matches on. `Api` is statically unreachable in an
    /// `Error<Infallible>` and so is not in this list.
    #[test]
    fn widen_preserves_every_reachable_variant() {
        let narrow: Vec<Error<std::convert::Infallible>> = vec![
            Error::request_message("bad path segment"),
            Error::Transport(TransportError::new(reqwest_error())),
            Error::Timeout(TimeoutKind::Connect),
            Error::Protocol(super::ProtocolError {
                source: reqwest_error(),
            }),
            Error::Redirect(super::RedirectError {
                source: reqwest_error(),
            }),
            Error::UnexpectedStatus {
                status: StatusCode::IM_A_TEAPOT,
                headers: HeaderMap::new(),
                body: Bytes::from_static(b"teapot"),
            },
            Error::Decode {
                path: "items[0].id".to_owned(),
                body: Bytes::from_static(b"{}"),
                truncated: true,
            },
            Error::InterruptedBody(TransportError::new(reqwest_error())),
        ];

        for error in narrow {
            let before = error.to_string();
            let transient = error.is_transient();
            let widened: Error<ApiBody> = error.widen();
            assert_eq!(widened.to_string(), before, "widen changed the variant");
            assert_eq!(
                widened.is_transient(),
                transient,
                "widen changed retryability of {before}"
            );
        }

        // The payload fields survive, not just the discriminant.
        let widened: Error<ApiBody> = Error::<std::convert::Infallible>::Decode {
            path: "a.b".to_owned(),
            body: Bytes::from_static(b"raw"),
            truncated: true,
        }
        .widen();
        let Error::Decode {
            path,
            body,
            truncated,
        } = widened
        else {
            panic!("widen changed the variant");
        };
        assert_eq!(path, "a.b");
        assert_eq!(body, Bytes::from_static(b"raw"));
        assert!(truncated);
    }

    impl super::ApiErrorBody for ApiBody {
        type Body = str;
        fn body(&self) -> Option<&str> {
            Some(self.0)
        }
    }

    /// `api_body` is `Some` exactly on the documented-API variant; every other class carries no
    /// typed body, and a future variant added to `every_variant` is classified here too.
    #[test]
    fn api_body_is_present_exactly_on_the_documented_api_error() {
        for error in every_variant() {
            let expected = matches!(error, Error::Api(_));
            assert_eq!(
                error.api_body().is_some(),
                expected,
                "api_body disagrees for {error}"
            );
        }
        let api = Error::Api(ResponseValue::new(
            StatusCode::BAD_REQUEST,
            HeaderMap::new(),
            ApiBody("bad request"),
        ));
        assert_eq!(api.api_body(), Some("bad request"));
    }

    /// The no-documented-error shape is `Error<Infallible>`; `api_body` must still exist there so
    /// generic callers compile against every operation, and it can only ever be `None`.
    #[test]
    fn an_uninhabited_error_body_is_never_present() {
        let error = Error::<std::convert::Infallible>::Timeout(TimeoutKind::Total);
        assert!(error.api_body().is_none());
    }
}
