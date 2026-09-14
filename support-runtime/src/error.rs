use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::{AuthError, ResponseValue};

/// The closed error taxonomy shared by every spargen-generated client. `E` is the
/// operation's typed error body (an enum when several error statuses are documented).
///
/// Nine variants are constructed; taxonomy class #10 (cancellation) is a documented drop-safety
/// guarantee, not a variant (see the crate docs). Every variant implements [`std::error::Error`]
/// with full source chains, and `Debug` never leaks secrets.
///
/// Adding a variant: raise `ERROR_VARIANTS` in this file's test module and list a value of it in
/// `every_variant`, for the reasons `request_variant_index` sets out.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// #1 — a request could not be built, or failed inside the send before that request produced
    /// a response. Read it as a statement about *one* request, not about the operation: it is
    /// neither proof that nothing reached the server, nor proof that no response was already
    /// produced and delivered to the caller.
    ///
    /// Pre-send, raised while the request is still being assembled — nothing was transmitted:
    /// no registered credential satisfies the operation's security requirement, a registered token
    /// provider failed, the base URL is invalid, or a parameter or body did not serialize.
    ///
    /// Not pre-send: an error reqwest classifies as a request error. reqwest raises that class
    /// from inside the send itself, so the request may already have been transmitted and the
    /// server may already have acted on it. Retrying it is not safe for a non-idempotent call.
    ///
    /// After a response was accepted: when the API has streaming (sequential) responses, the
    /// generated client also embeds `EventStream`, which raises this class when the stored request
    /// cannot be cloned for an automatic reconnect — from `with_reconnect`, and from `poll_next`
    /// once the reconnect wait has elapsed, which is mid-stream, after frames have already been
    /// yielded to the caller. `StreamError`, that module's alias for this same enum, documents
    /// itself as the failure yielded by a streaming response *after its initial HTTP response was
    /// accepted*; the two sentences describe one case. Re-driving such an operation from the start
    /// is not a safe recovery — it re-delivers events the caller has already consumed and acted
    /// on.
    ///
    /// [`RequestError`] types the two credential causes; every other cause — reqwest's own
    /// request-error class and both reconnect-clone failures included — arrives as
    /// [`RequestError::Other`].
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
        Self::RequestConstruction(RequestError::Other(RequestCause(Box::new(source))))
    }

    /// Build a request-construction error from a static message.
    pub fn request_message(message: impl Into<String>) -> Self {
        Self::RequestConstruction(RequestError::Other(RequestCause(Box::new(MessageError(
            message.into(),
        )))))
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
            Error::RequestConstruction(RequestError::Other(RequestCause(Box::new(error))))
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

    /// The HTTP status the failed call's response carried: `Some` for a documented error status
    /// ([`Error::Api`], the same value as its `ResponseValue::status()`) and for an undocumented
    /// status ([`Error::UnexpectedStatus`], which includes an undocumented 2xx), `None` for every
    /// class that has no status. That includes [`Error::Decode`], which does not keep the status of
    /// the response it failed to decode.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Error::Api(value) => Some(value.status()),
            Error::UnexpectedStatus { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::Decode { .. }
            | Error::InterruptedBody(_) => None,
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
/// Three shapes implement it: a multi-status enum whose bodied statuses carry the same body type
/// (one schema, or schemas that generate the same Rust type; its `body` is `None` for a
/// documented bodyless status, or a `null` payload), the
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
    /// whose `E` reports a body. Its status is [`Error::status`], the same value as the
    /// `ResponseValue::status()` inside `Api`.
    pub fn api_body(&self) -> Option<&E::Body> {
        match self {
            Error::Api(value) => value.inner().body(),
            // Every other class carries no typed body. Listed, not wildcarded, like every other
            // match over the taxonomy here: a new variant must decide whether it carries one.
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::UnexpectedStatus { .. }
            | Error::Decode { .. }
            | Error::InterruptedBody(_) => None,
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
///
/// The two credential causes are the ones a consumer routes on — they mean "unauthenticated", not
/// "malformed request" — so each is a variant of its own: [`RequestError::MissingCredential`] when
/// no registered credential satisfies the requirement, and [`RequestError::CredentialProvider`]
/// when a registered token provider fails. Both are raised before anything is sent. Every other
/// cause arrives as [`RequestError::Other`] with its source attached — and [`RequestError::Other`]
/// is **not** uniformly pre-send; see its own documentation before retrying on it.
///
/// This runtime is embedded in the consumer's own crate, where `#[non_exhaustive]` does not affect
/// match exhaustiveness, so a new variant here is a breaking change of the generated output; the
/// attribute is kept for a consumer that re-exports the generated module across a crate boundary.
///
/// Adding a variant: raise `REQUEST_VARIANTS` in this file's test module and list a value of it in
/// `every_request_variant`. The compiler will demand the classification arms on its own, but it
/// cannot demand the value — `request_variant_index` documents precisely why, and which ways of
/// getting this wrong are caught.
#[derive(Debug)]
#[non_exhaustive]
pub enum RequestError {
    /// The operation carries a security requirement and no registered credential satisfies any of
    /// its alternatives. Raised before anything is sent. The payload is the whole cause, so
    /// `source()` is `None`.
    MissingCredential {
        /// One entry per alternative of the requirement, in declaration order: that alternative's
        /// `securitySchemes` keys that have no registered credential, in declaration order.
        /// `mutualTLS` keys never appear (the transport satisfies them).
        ///
        /// The runtime never builds an empty outer list, nor an empty inner one — it only reaches
        /// this variant with at least one unregistered scheme per alternative. The field is public
        /// inside the consumer's crate, though, so the invariant is not enforced by the type;
        /// `Display` therefore omits the `(missing: …)` clause entirely rather than rendering an
        /// empty one, and skips an empty alternative when listing.
        alternatives: Vec<Vec<&'static str>>,
    },
    /// The selected alternative's token provider returned an error. Raised before anything is
    /// sent; `source()` is the provider's [`AuthError`].
    CredentialProvider {
        /// The `securitySchemes` key the provider is registered under.
        scheme: &'static str,
        /// What the provider reported.
        source: AuthError,
    },
    /// Any other request-construction failure, with the cause reachable through `source()`.
    ///
    /// Pre-send: an unparseable base URL, a parameter or body that did not serialize, or a
    /// credential registered under the wrong kind for its scheme.
    ///
    /// Not pre-send: an error reqwest classifies as a request error. reqwest raises that class
    /// from inside the send, so the request may already have been transmitted. This variant is
    /// therefore the one place in taxonomy #1 where "the request was never sent" does not hold,
    /// and a caller that retries on it must treat the call as possibly-already-applied.
    ///
    /// After a response was accepted: when the API has streaming responses, the embedded
    /// `EventStream`'s two reconnect-clone failures land here as well. Nothing was transmitted for
    /// the reconnect itself, but the stream's initial response was accepted and frames may already
    /// have been yielded, so the possibly-already-applied reading above is not the one that
    /// describes them — re-driving the operation from the start re-delivers consumed events
    /// instead. See [`Error::RequestConstruction`] for the class.
    Other(RequestCause),
}

/// The opaque cause of [`RequestError::Other`]. It displays as the underlying failure; reach the
/// failure itself (and downcast it) through [`std::error::Error::source`] on the [`RequestError`].
#[derive(Debug)]
pub struct RequestCause(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for RequestCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for RequestCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
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
        match self {
            RequestError::MissingCredential { alternatives } => {
                f.write_str(
                    "no registered credential satisfies the operation's security requirement",
                )?;
                // `alternatives` is public inside the consumer's crate, so a hand-built value can
                // name nothing. The runtime never builds one, so every value it does build renders
                // exactly as it did before: a non-empty clause, in declaration order.
                let mut named = alternatives
                    .iter()
                    .filter(|alternative| !alternative.is_empty())
                    .peekable();
                if named.peek().is_none() {
                    return Ok(());
                }
                f.write_str(" (missing: ")?;
                for (index, alternative) in named.enumerate() {
                    if index > 0 {
                        f.write_str(" or ")?;
                    }
                    f.write_str(&alternative.join(" + "))?;
                }
                f.write_str(")")
            }
            RequestError::CredentialProvider { scheme, .. } => write!(
                f,
                "the token provider registered for security scheme `{scheme}` failed"
            ),
            RequestError::Other(cause) => write!(f, "{cause}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RequestError::MissingCredential { .. } => None,
            RequestError::CredentialProvider { source, .. } => Some(source),
            // The boxed cause itself, not its wrapper, so the chain and `downcast_ref` stay exactly
            // as they were when the box was a private field.
            RequestError::Other(cause) => {
                Some(cause.0.as_ref() as &(dyn std::error::Error + 'static))
            }
        }
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

    use crate::{AuthError, ResponseValue, TransportError};

    use super::{Error, RequestError, TimeoutKind};

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

    /// The missing-credential cause is the payload itself, so the chain ends at `RequestError` —
    /// one level shorter than the message-only error it replaced, which wrapped its text in a
    /// `RequestCause`. The rendered text is *not* what that error said: master rendered
    /// `(schemes: key, token)`, sorted and deduplicated across the whole requirement, and this
    /// renders `(missing: key + token)`, grouped per alternative in declaration order. The break
    /// is deliberate (the grouping is the payload's whole point) and is what the exact string
    /// below pins.
    #[test]
    fn a_missing_credential_is_typed_and_ends_the_cause_chain() {
        let error = Error::<ApiBody>::RequestConstruction(RequestError::MissingCredential {
            alternatives: vec![vec!["key", "token"]],
        });
        assert!(!error.is_transient());
        assert_eq!(error.to_string(), "request construction failed");
        let source = std::error::Error::source(&error).expect("the typed cause is the source");
        assert_eq!(
            source.to_string(),
            "no registered credential satisfies the operation's security requirement \
             (missing: key + token)"
        );
        assert!(std::error::Error::source(source).is_none());
    }

    /// Any other cause is opaque: it cannot be moved out of `Other`, but it renders as itself and
    /// `source()` reaches the cause (not its wrapper) at the same depth, so it still downcasts.
    #[test]
    fn an_other_cause_is_opaque_and_reachable_through_source() {
        #[derive(Debug)]
        struct Cause;

        impl std::fmt::Display for Cause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("cause")
            }
        }

        impl std::error::Error for Cause {}

        let error = Error::<ApiBody>::request_construction(Cause);
        let Error::RequestConstruction(RequestError::Other(cause)) = &error else {
            panic!("expected Other, got {error:?}");
        };
        assert_eq!(cause.to_string(), "cause");
        let source = std::error::Error::source(&error).expect("the request error is the source");
        assert_eq!(source.to_string(), "cause");
        let inner = std::error::Error::source(source).expect("the cause is reachable");
        assert!(inner.downcast_ref::<Cause>().is_some());
        assert!(std::error::Error::source(inner).is_none());
    }

    /// `RequestCause` overrides `source` to forward to the boxed cause's *own* source rather than
    /// to the box. Nothing else in the runtime calls it — `RequestError::source` hands out the box
    /// itself — so without this the override could be deleted and nothing would notice.
    #[test]
    fn a_request_cause_forwards_source_to_the_boxed_causes_own_source() {
        #[derive(Debug)]
        struct Inner;

        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("inner")
            }
        }

        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);

        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("outer")
            }
        }

        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let error = Error::<ApiBody>::request_construction(Outer(Inner));
        let Error::RequestConstruction(RequestError::Other(cause)) = &error else {
            panic!("expected Other, got {error:?}");
        };
        assert_eq!(cause.to_string(), "outer");
        let forwarded = std::error::Error::source(cause)
            .expect("the wrapper forwards to the boxed cause's own source");
        assert_eq!(forwarded.to_string(), "inner");
        assert!(forwarded.downcast_ref::<Inner>().is_some());
    }

    /// How many variants `RequestError` has. `every_request_variant` returns an array of exactly
    /// this length, so raising it will not compile until a value of the new variant is listed.
    const REQUEST_VARIANTS: usize = 3;

    /// Each variant's position in `every_request_variant`. Indices are dense and unique, which is
    /// what `every_request_variant_lists_each_variant_exactly_once` checks.
    ///
    /// **What this actually enforces, and what it does not.** The match being exhaustive means a
    /// variant added to `RequestError` cannot compile without being *classified* here and in every
    /// other match over the enum. Whether it is *listed* in `every_request_variant` — and so
    /// whether anything it claims is ever compared against anything — is enforced only in part.
    /// Each of these was run:
    ///
    /// - Adding a variant, classifying it everywhere, giving it the next free index, and raising
    ///   `REQUEST_VARIANTS` to match: **caught**, at compile time — the array is then one element
    ///   short of its own declared length.
    /// - Padding that array with a duplicate of some other variant to make it compile: **caught**,
    ///   by the bijection test — two entries take one index, and another index is unoccupied.
    /// - Adding a variant, classifying it, giving it the next free index, and leaving
    ///   `REQUEST_VARIANTS` alone: **not caught**. Nothing ever evaluates this function on a value
    ///   of the new variant, because no such value is ever constructed.
    ///
    /// That last case cannot be closed from inside stable Rust: no construct yields a variant
    /// count, so no assertion can know the list is short. Closing it needs the enum declared
    /// through a macro that emits the count alongside it, which would ship a `macro_rules!`
    /// definition of a public type into every generated client. So raising `REQUEST_VARIANTS` is a
    /// convention the enum's own doc states, and the two mechanical guards above catch every way
    /// of getting it wrong once it is raised.
    fn request_variant_index(error: &RequestError) -> usize {
        match error {
            RequestError::MissingCredential { .. } => 0,
            RequestError::CredentialProvider { .. } => 1,
            RequestError::Other(_) => 2,
        }
    }

    /// One value of every `RequestError` variant, mirroring `every_variant` for `Error`. Listing a
    /// variant here is what makes its display, source, transience and response accessors actually
    /// get asserted; an exhaustive match alone only forces it to be *classified*. See
    /// `request_variant_index` for exactly how much of that listing is mechanically enforced.
    fn every_request_variant() -> [RequestError; REQUEST_VARIANTS] {
        [
            RequestError::MissingCredential {
                alternatives: vec![vec!["token"], vec!["key", "tenant"]],
            },
            RequestError::CredentialProvider {
                scheme: "token",
                source: AuthError::new("refresh rejected"),
            },
            // The field is private, but these tests live in the defining module.
            RequestError::Other(super::RequestCause(Box::new(super::MessageError(
                "bad path segment".to_owned(),
            )))),
        ]
    }

    /// The enumeration is a bijection onto the variant set: every entry takes a distinct index
    /// inside the declared count, and every index is occupied. Without this, a variant could be
    /// added, classified in each exhaustive match, and never constructed — so nothing it claims
    /// would ever be compared against anything.
    #[test]
    fn every_request_variant_lists_each_variant_exactly_once() {
        let mut seen = [false; REQUEST_VARIANTS];
        for error in every_request_variant() {
            let index = request_variant_index(&error);
            assert!(
                index < REQUEST_VARIANTS,
                "`{error}` takes index {index}, outside the declared count of \
                 {REQUEST_VARIANTS}: raise `REQUEST_VARIANTS` and list a value of the new variant \
                 in `every_request_variant`"
            );
            assert!(!seen[index], "two entries share index {index}");
            seen[index] = true;
        }
        assert!(
            seen.iter().all(|occupied| *occupied),
            "an index in 0..{REQUEST_VARIANTS} is unoccupied: `every_request_variant` is missing \
             a variant"
        );
    }

    #[test]
    fn every_request_variant_displays_exactly_what_names_its_cause() {
        for error in every_request_variant() {
            let rendered = error.to_string();
            assert!(!rendered.is_empty(), "a variant renders as an empty string");
            let expected = match &error {
                RequestError::MissingCredential { .. } => {
                    "no registered credential satisfies the operation's security requirement \
                     (missing: token or key + tenant)"
                }
                RequestError::CredentialProvider { .. } => {
                    "the token provider registered for security scheme `token` failed"
                }
                RequestError::Other(_) => "bad path segment",
            };
            assert_eq!(rendered, expected, "a variant does not name its cause");
        }
    }

    /// `MissingCredential` *is* the whole cause, so it ends the chain; the other two carry a
    /// separate cause and must hand it over. A consumer walking the chain must not find a phantom
    /// source, nor lose a real one.
    #[test]
    fn request_source_is_present_exactly_where_the_cause_is_separate() {
        for error in every_request_variant() {
            let expected = match &error {
                RequestError::MissingCredential { .. } => false,
                RequestError::CredentialProvider { .. } | RequestError::Other(_) => true,
            };
            assert_eq!(
                std::error::Error::source(&error).is_some(),
                expected,
                "source() disagrees for {error}"
            );
        }
    }

    /// Whatever the cause, a request was never sent: nothing to retry, no status, no typed body.
    /// Pinned over every `RequestError` variant so a new one cannot arrive misclassified.
    #[test]
    fn no_request_variant_is_transient_or_carries_a_response() {
        for request_error in every_request_variant() {
            let error = Error::<ApiBody>::RequestConstruction(request_error);
            assert!(!error.is_transient(), "{error} classified as transient");
            assert_eq!(error.status(), None);
            assert!(error.api_body().is_none());
        }
    }

    /// The runtime never builds an alternative list with nothing to name, but the fields are public
    /// inside the consumer's crate. Rendering stays total rather than trailing an empty clause.
    #[test]
    fn a_missing_credential_with_nothing_to_name_renders_without_the_clause() {
        let empty = RequestError::MissingCredential {
            alternatives: Vec::new(),
        };
        assert_eq!(
            empty.to_string(),
            "no registered credential satisfies the operation's security requirement"
        );
        let all_empty = RequestError::MissingCredential {
            alternatives: vec![Vec::new(), Vec::new()],
        };
        assert_eq!(all_empty.to_string(), empty.to_string());
        // A named alternative beside an empty one still renders, without a stray separator.
        let mixed = RequestError::MissingCredential {
            alternatives: vec![Vec::new(), vec!["token"], Vec::new()],
        };
        assert_eq!(
            mixed.to_string(),
            "no registered credential satisfies the operation's security requirement \
             (missing: token)"
        );
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

    /// How many variants `Error` has. `every_variant` returns an array of exactly this length, so
    /// raising it will not compile until a value of the new variant is listed.
    const ERROR_VARIANTS: usize = 9;

    /// Each variant's position in `every_variant`. Indices are dense and unique. This is the same
    /// guard `request_variant_index` carries, applied to the other taxonomy that is a semver
    /// surface — and it enforces exactly as much, and as little, as that function documents.
    fn error_variant_index(error: &Error<ApiBody>) -> usize {
        match error {
            Error::RequestConstruction(_) => 0,
            Error::Transport(_) => 1,
            Error::Timeout(_) => 2,
            Error::Protocol(_) => 3,
            Error::Redirect(_) => 4,
            Error::Api(_) => 5,
            Error::UnexpectedStatus { .. } => 6,
            Error::Decode { .. } => 7,
            Error::InterruptedBody(_) => 8,
        }
    }

    /// The enumeration is a bijection onto the variant set. See
    /// `every_request_variant_lists_each_variant_exactly_once` for why an exhaustive match alone
    /// is not enough.
    #[test]
    fn every_variant_lists_each_variant_exactly_once() {
        let mut seen = [false; ERROR_VARIANTS];
        for error in every_variant() {
            let index = error_variant_index(&error);
            assert!(
                index < ERROR_VARIANTS,
                "`{error}` takes index {index}, outside the declared count of {ERROR_VARIANTS}: \
                 raise `ERROR_VARIANTS` and list a value of the new variant in `every_variant`"
            );
            assert!(!seen[index], "two entries share index {index}");
            seen[index] = true;
        }
        assert!(
            seen.iter().all(|occupied| *occupied),
            "an index in 0..{ERROR_VARIANTS} is unoccupied: `every_variant` is missing a variant"
        );
    }

    /// One value of every variant. The match in each test below is exhaustive over this array by
    /// construction, so a variant added to `Error` is classified by the compiler;
    /// `error_variant_index` is what additionally forces it to be *listed*.
    fn every_variant() -> [Error<ApiBody>; ERROR_VARIANTS] {
        [
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

    /// `status` answers exactly for the two classes that carry a response status; every other class,
    /// including `Decode`, has none to report.
    #[test]
    fn status_is_present_exactly_on_the_two_status_variants() {
        for error in every_variant() {
            let expected = match &error {
                Error::Api(value) => Some(value.status()),
                Error::UnexpectedStatus { status, .. } => Some(*status),
                Error::RequestConstruction(_)
                | Error::Transport(_)
                | Error::Timeout(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::Decode { .. }
                | Error::InterruptedBody(_) => None,
            };
            assert_eq!(error.status(), expected, "status() disagrees for {error}");
        }
        let statuses: Vec<_> = every_variant().iter().filter_map(Error::status).collect();
        assert_eq!(statuses, [StatusCode::BAD_REQUEST, StatusCode::IM_A_TEAPOT]);
    }

    /// Generated clients hold `Error<Infallible>` for an operation with no documented error body,
    /// so `status` is pinned on that instantiation too, over every variant it can hold (`Api` is
    /// statically unreachable there): only `UnexpectedStatus` answers, with its own code.
    #[test]
    fn status_on_an_uninhabited_api_error_is_present_only_for_unexpected_status() {
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
        for error in &narrow {
            let expected = match error {
                Error::Api(value) => match *value.inner() {},
                Error::UnexpectedStatus { status, .. } => Some(*status),
                Error::RequestConstruction(_)
                | Error::Transport(_)
                | Error::Timeout(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::Decode { .. }
                | Error::InterruptedBody(_) => None,
            };
            assert_eq!(error.status(), expected, "status() disagrees for {error}");
        }
        let statuses: Vec<_> = narrow.iter().filter_map(Error::status).collect();
        assert_eq!(statuses, [StatusCode::IM_A_TEAPOT]);
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
            Error::RequestConstruction(RequestError::MissingCredential {
                alternatives: vec![vec!["token"]],
            }),
            Error::RequestConstruction(RequestError::CredentialProvider {
                scheme: "token",
                source: AuthError::new("x"),
            }),
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

        // The typed request-construction cause keeps its payload too.
        let widened: Error<ApiBody> = Error::<std::convert::Infallible>::RequestConstruction(
            RequestError::MissingCredential {
                alternatives: vec![vec!["a"], vec!["b"]],
            },
        )
        .widen();
        let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = widened
        else {
            panic!("widen changed the variant");
        };
        assert_eq!(alternatives, [vec!["a"], vec!["b"]]);
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
            let expected = match &error {
                // The documented API error carries the operation's typed body.
                Error::Api(_) => true,
                // Every other class carries none.
                Error::RequestConstruction(_)
                | Error::Transport(_)
                | Error::Timeout(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::UnexpectedStatus { .. }
                | Error::Decode { .. }
                | Error::InterruptedBody(_) => false,
            };
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

    /// `status` and `api_body` read one `Error` from two sides, so they must agree on what each
    /// class carries: a documented API error answers both from the same `ResponseValue`, an
    /// undocumented status has a status but no typed body, and every other class has neither.
    #[test]
    fn status_and_api_body_agree_on_every_variant() {
        for error in every_variant() {
            match &error {
                Error::Api(value) => {
                    assert_eq!(error.status(), Some(value.status()), "{error}");
                    let same_body =
                        match (error.api_body(), super::ApiErrorBody::body(value.inner())) {
                            (Some(answered), Some(carried)) => std::ptr::eq(answered, carried),
                            _ => false,
                        };
                    assert!(
                        same_body,
                        "api_body is not the Api value's own body: {error}"
                    );
                }
                Error::UnexpectedStatus { status, .. } => {
                    assert_eq!(error.status(), Some(*status), "{error}");
                    assert!(error.api_body().is_none(), "{error}");
                }
                Error::RequestConstruction(_)
                | Error::Transport(_)
                | Error::Timeout(_)
                | Error::Protocol(_)
                | Error::Redirect(_)
                | Error::Decode { .. }
                | Error::InterruptedBody(_) => {
                    assert!(error.status().is_none(), "{error}");
                    assert!(error.api_body().is_none(), "{error}");
                }
            }
        }
    }

    /// The no-documented-error shape is `Error<Infallible>`; `api_body` must still exist there so
    /// generic callers compile against every operation, and it can only ever be `None`.
    #[test]
    fn an_uninhabited_error_body_is_never_present() {
        let error = Error::<std::convert::Infallible>::Timeout(TimeoutKind::Total);
        assert!(error.api_body().is_none());
    }
}
