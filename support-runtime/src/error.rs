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
    /// #8 — the response body failed to deserialize; retains the serde error path and (capped) raw
    /// body.
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

    use crate::ResponseValue;

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
}
