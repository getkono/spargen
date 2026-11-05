use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::{AuthError, ResponseValue};

/// The closed error taxonomy shared by every spargen-generated client. `E` is the
/// operation's typed error body (an enum when several error statuses are documented).
///
/// Ten variants are constructed: classes #1 to #9, and #11. Taxonomy class #10, cancellation, is a
/// documented drop-safety guarantee rather than a variant: dropping a returned future is safe and
/// side-effect-free beyond standard HTTP semantics. The enum implements [`std::error::Error`], whose `source()` reaches
/// each retained cause, and `Debug` never leaks secrets.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// #1 — a request could not be built, or failed inside the send before that request produced
    /// a response. Read it as a statement about *one* request, not about the operation: it is
    /// neither proof that nothing reached the server, nor proof that no response was already
    /// produced and delivered to the caller.
    ///
    /// [`RequestError`] types the three credential causes, each raised before anything is sent;
    /// every other cause arrives as [`RequestError::Other`], whose documentation says which of
    /// them are raised before anything is sent and which are not. Read it before retrying.
    RequestConstruction(RequestError),
    /// #2 — DNS failure, connection refused/reset, TLS handshake or certificate error.
    Transport(TransportError),
    /// #3 — connect vs total-request timeout (as configured on the injected client).
    Timeout(TimeoutKind),
    /// #4 — malformed HTTP or decompression failure. A response body the connection cut short is
    /// not this class but [`Error::InterruptedBody`].
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
        /// The raw response body, capped as `ClientConfig::max_error_body` documents.
        body: Bytes,
    },
    /// #8 — the response body failed to deserialize; retains the status and headers the response
    /// carried, the decoder's error message, and the raw body, capped on every path but the one
    /// named on `body` below. A documented error status whose body does not match its schema (a
    /// problem `type` the description does not list, or an HTML page from a proxy in front of the
    /// server) arrives here with its status and headers intact, so the caller can still act on
    /// them — honour a `Retry-After` on a `429`, say.
    Decode {
        /// The status of the response whose body failed to decode: a success status when a
        /// success body did not match its schema, the documented error status when a documented
        /// error body did not. For `EventStream`'s per-frame decode, it is the status of the
        /// response the frame was read from — after a reconnect, the reconnected response's.
        status: StatusCode,
        /// The headers of the response whose body failed to decode — for `EventStream`'s
        /// per-frame decode, of the response the frame was read from, as for `status`.
        headers: HeaderMap,
        /// The decoder's error message, held as the decoder wrote it; `Display` escapes its
        /// control characters, since the message may quote server-supplied input.
        path: String,
        /// The retained raw body, capped at `max_error_body`, except on `EventStream`'s per-frame
        /// decode, which `ClientConfig::max_stream_frame` bounds instead.
        body: Bytes,
        /// Whether bytes were dropped from `body` to meet a cap: `false` means nothing was dropped.
        truncated: bool,
    },
    /// #9 — the connection dropped partway through a response body: mid-stream on a streamed
    /// response, or while any other response body, success or error, was being read.
    InterruptedBody(TransportError),
    /// #11 — a `LinkPaginator` page's `rel="next"` link named a URL the paginator had already
    /// fetched (a self-link, or a longer cycle such as page 1 → page 2 → page 1), so following it
    /// would never end. Every page before it was yielded; nothing was sent for `url`.
    PaginationCycle {
        /// The revisited URL the `next` link resolved to.
        url: reqwest::Url,
    },
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
    ///
    /// reqwest reports every failure of the send itself as a request-kind error, a refused
    /// connection, an unresolvable host, and a failed TLS handshake included. Those are asked for
    /// first (`is_connect`) and become [`Error::Transport`]; only the request-kind errors that are
    /// not connection failures stay [`Error::RequestConstruction`]. reqwest offers no connect
    /// classification on `wasm32`, where the browser's fetch reports failures opaquely, so there
    /// every request-kind error stays [`Error::RequestConstruction`].
    ///
    /// A timeout is asked for before any of those, and the same connect discriminator splits it:
    /// one raised while establishing the connection (the client's `connect_timeout`, which bounds
    /// name resolution, the TCP handshake, and TLS, or the operating system's own handshake
    /// timeout) is [`TimeoutKind::Connect`]; every other one,
    /// the total-request budget elapsing during the connect included, is [`TimeoutKind::Total`].
    /// On `wasm32` every timeout is `Total`, for the reason above.
    ///
    /// A builder-kind error is reqwest refusing the request before sending anything, and it
    /// becomes [`Error::RequestConstruction`] wherever reqwest raises it: from
    /// `RequestBuilder::build`, or from `execute` on a request that built fine — a URL whose
    /// scheme is not `http`/`https`, plain `http` on an `https_only` client, or a URL that is not
    /// a valid URI. Re-sending it is refused again, so it is not transient.
    pub fn from_reqwest(error: reqwest::Error) -> Self {
        Self::from_class(ReqwestClass::of(&error), error)
    }

    /// Build the variant `class` names around `error`.
    fn from_class(class: ReqwestClass, error: reqwest::Error) -> Self {
        match class {
            ReqwestClass::Timeout(kind) => Error::Timeout(kind),
            ReqwestClass::Redirect => Error::Redirect(RedirectError { source: error }),
            ReqwestClass::Protocol => Error::Protocol(ProtocolError { source: error }),
            ReqwestClass::Transport => Error::Transport(TransportError { source: error }),
            ReqwestClass::Request => {
                Error::RequestConstruction(RequestError::Other(RequestCause(Box::new(error))))
            }
        }
    }

    /// Classify a failure reading the body of a response that has already arrived, the same way
    /// `EventStream` classifies one mid-stream: the connection giving out partway through the
    /// body is [`Error::InterruptedBody`], and so transient.
    ///
    /// reqwest reports any failure while reading a body as a decode-kind error, so the cause
    /// decides. The first [`std::io::Error`] in its source chain is the evidence: a reset,
    /// aborted, broken, or unconnected connection, or one that ended before the body did, is an
    /// interruption, including a compressed body that ends before its compressed stream does;
    /// any other kind (corrupt compressed data, a malformed chunked encoding) is not. With no I/O
    /// error in the chain, a body-kind reqwest error, the error itself or one in the chain, is an
    /// interruption. A timeout is asked for first, and everything that is not an interruption is
    /// classified as [`Error::from_reqwest`] classifies it, so a body that is malformed rather
    /// than cut short stays [`Error::Protocol`].
    pub(crate) fn from_body_read(error: reqwest::Error) -> Self {
        if !error.is_timeout() && is_interrupted_read(&error) {
            Error::InterruptedBody(TransportError::new(error))
        } else {
            Self::from_reqwest(error)
        }
    }

    /// Whether the failure is worth retrying: transport failures, timeouts, and a `429` or `5xx`
    /// response — whichever class carries it, including [`Error::Decode`], since a server that
    /// answered `503` is unavailable whether or not its body matched the schema. This is the
    /// rule `RetryOutcome::is_transient` applies to the undecoded response of an idempotent
    /// request. This classifies the failure alone: whether re-sending the request is safe also
    /// depends on its method, which `RetryOutcome::is_idempotent` reads.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
            Error::Api(value) => is_transient_status(value.status()),
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => {
                is_transient_status(*status)
            }
            Error::RequestConstruction(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::PaginationCycle { .. } => false,
        }
    }

    /// The HTTP status the failed call's response carried: `Some` for a documented error status
    /// ([`Error::Api`], the same value as its `ResponseValue::status()`), for an undocumented
    /// status ([`Error::UnexpectedStatus`], which includes an undocumented 2xx), and for a
    /// response whose body failed to decode ([`Error::Decode`], success or documented error
    /// status alike); `None` for every class that produced no response to read a status from.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Error::Api(value) => Some(value.status()),
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => None,
        }
    }
}

/// Whether a response status is transient: a `429` or any `5xx`. [`Error::is_transient`] applies
/// it to every class that carries a status, and `RetryOutcome::is_transient` to the undecoded
/// response of an idempotent request.
pub(crate) fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
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
                status,
                headers,
                path,
                body,
                truncated,
            } => Error::Decode {
                status,
                headers,
                path,
                body,
                truncated,
            },
            Error::InterruptedBody(e) => Error::InterruptedBody(e),
            Error::PaginationCycle { url } => Error::PaginationCycle { url },
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
    // Mutation testing: replacing this body with `None` is an equivalent mutant, declared rather
    // than killed. `Infallible` is uninhabited, so no `&self` exists to call it with and no
    // test, or caller, can observe what it returns.
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
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => None,
        }
    }
}

/// The RFC 9457 problem-details members of an error response body, read by name.
///
/// Each member is `Some` only when the body is a JSON object that carries it with the type RFC 9457
/// gives it: a string for `type`, `title`, `detail`, and `instance`, and an integer in the HTTP
/// status range for `status`. A member that is absent, or present with another type, is `None`;
/// the RFC's own rule for such a member is to ignore it. Nothing here asserts that the server meant
/// the body as problem details: an object body that is not one still answers with whichever of
/// these member names it happens to carry. Extension members are not read; the typed body, or the
/// raw one, still has them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProblemDetails {
    /// The `type` member: a URI reference naming the problem type. RFC 9457 reads an absent one
    /// as `about:blank`, which [`ProblemDetails::problem_type_or_blank`] applies.
    pub problem_type: Option<String>,
    /// The `title` member: a short summary of the problem type.
    pub title: Option<String>,
    /// The `status` member: the status code the origin server generated for this occurrence.
    pub status: Option<u16>,
    /// The `detail` member: an explanation specific to this occurrence.
    pub detail: Option<String>,
    /// The `instance` member: a URI reference naming this occurrence.
    pub instance: Option<String>,
}

impl ProblemDetails {
    /// The members of `body` as it serializes to JSON: `None` when it does not serialize to a JSON
    /// object (a string, an array, `null`, or a failed serialization).
    pub fn of<T: serde::Serialize + ?Sized>(body: &T) -> Option<Self> {
        Self::from_value(&serde_json::to_value(body).ok()?)
    }

    /// The members of a raw JSON body: `None` when the bytes are not a JSON object, which includes
    /// a body truncated by the error-body cap.
    pub fn from_json(body: &[u8]) -> Option<Self> {
        Self::from_value(&serde_json::from_slice(body).ok()?)
    }

    fn from_value(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        let text = |name: &str| {
            object
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        Some(Self {
            problem_type: text("type"),
            title: text("title"),
            status: object
                .get("status")
                .and_then(serde_json::Value::as_u64)
                .and_then(|status| u16::try_from(status).ok())
                .filter(|status| (100..=599).contains(status)),
            detail: text("detail"),
            instance: text("instance"),
        })
    }

    /// The problem type, with RFC 9457's default applied: `about:blank` when the body carries no
    /// string `type` member.
    pub fn problem_type_or_blank(&self) -> &str {
        self.problem_type.as_deref().unwrap_or("about:blank")
    }
}

/// Implemented by every generated operation error type, so code generic over operations can read
/// the RFC 9457 members of whichever documented error body a failure carried, without naming each
/// status's body type.
///
/// Every shape implements it: the multi-status enum answers from the variant's body (and `None`
/// for a documented bodyless status), the single-body newtype from its one body, and the
/// uninhabited `Infallible` shape never answers. A body read as raw bytes (`bytes::Bytes`) answers
/// `None`. [`Error::problem`] is the reader to call; this trait is its bound.
pub trait ApiErrorProblem {
    /// The problem-details members of the documented body this value carries, if its status
    /// documents one that serializes to a JSON object.
    fn problem(&self) -> Option<ProblemDetails>;
}

impl ApiErrorProblem for std::convert::Infallible {
    // Mutation testing: replacing this body with `None` or `Some(Default::default())` is an
    // equivalent mutant, declared rather than killed. `Infallible` is uninhabited, so no `&self`
    // exists to call it with and no test, or caller, can observe what it returns.
    fn problem(&self) -> Option<ProblemDetails> {
        match *self {}
    }
}

impl<E: ApiErrorProblem> Error<E> {
    /// The RFC 9457 problem-details members of the failed call's error response body, from
    /// whichever class carried one.
    ///
    /// - [`Error::Api`]: the documented body, read through `E`'s [`ApiErrorProblem`], so it works
    ///   the same across every status of every operation, whatever body type each one decoded to.
    /// - [`Error::Decode`] and [`Error::UnexpectedStatus`] with a `4xx` or `5xx` status: the raw
    ///   body, parsed as JSON. This is how a documented error status whose body did not match its
    ///   schema — a problem `type` the description does not list — still yields its `type` and
    ///   `detail`. A success status is not read: its body was never an error response.
    /// - Every other class produced no response body, and answers `None`.
    pub fn problem(&self) -> Option<ProblemDetails> {
        match self {
            Error::Api(value) => value.inner().problem(),
            Error::UnexpectedStatus { status, body, .. } | Error::Decode { status, body, .. } => {
                if status.is_client_error() || status.is_server_error() {
                    ProblemDetails::from_json(body)
                } else {
                    None
                }
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => None,
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
            Error::Decode { status, path, .. } => {
                // `path` is the decoder's message, which quotes the server-supplied input (an
                // unknown enum variant, say) verbatim; its control characters are escaped so a
                // value holding a LF, CR or tab cannot break or forge the message's line.
                write!(f, "response decode failed ({status}) at ")?;
                write_escaped(f, path)
            }
            Error::InterruptedBody(_) => f.write_str("response body was interrupted"),
            Error::PaginationCycle { url } => {
                write!(f, "pagination cycle: the next link revisits {url}")
            }
        }
    }
}

/// Write `text` with each control character replaced by its `escape_debug` form, so text quoted
/// from a server (a LF, CR or tab included) cannot break or forge the line it is written into.
pub(crate) fn write_escaped(f: &mut std::fmt::Formatter<'_>, text: &str) -> std::fmt::Result {
    for c in text.chars() {
        if c.is_control() {
            write!(f, "{}", c.escape_debug())?;
        } else {
            write!(f, "{c}")?;
        }
    }
    Ok(())
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
            Error::Timeout(_)
            | Error::UnexpectedStatus { .. }
            | Error::Decode { .. }
            | Error::PaginationCycle { .. } => None,
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
/// The three credential causes are the ones a consumer routes on — they mean "unauthenticated",
/// not "malformed request" — so each is a variant of its own: [`RequestError::MissingCredential`]
/// when no registered credential satisfies the requirement,
/// [`RequestError::CredentialMismatch`] when the selected alternative has a credential registered
/// that its scheme cannot carry, and [`RequestError::CredentialProvider`] when a registered token
/// provider fails. All three are raised before anything is sent. Every other
/// cause arrives as [`RequestError::Other`] with its source attached — and [`RequestError::Other`]
/// is **not** uniformly pre-send; see its own documentation before retrying on it.
///
/// This runtime is embedded in the consumer's own crate, where `#[non_exhaustive]` does not affect
/// match exhaustiveness, so a new variant here is a breaking change of the generated output; the
/// attribute is kept for a consumer that re-exports the generated module across a crate boundary.
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
    /// The selected alternative has a credential registered under one of its schemes, but of a
    /// kind that scheme cannot carry — a `Credential::Basic` under a bearer or `apiKey` scheme, or
    /// anything but `Credential::Basic` under an `http basic` one. Raised before anything is sent,
    /// and before a registered token provider is asked for a token. The payload is the whole cause,
    /// so `source()` is `None`.
    ///
    /// The registration is what needs correcting: [`crate::attach_auth`] never falls through to a
    /// later alternative.
    CredentialMismatch {
        /// The `securitySchemes` key the credential is registered under.
        scheme: &'static str,
        /// What the scheme carries on the wire: `"http basic"`, `"bearer"` (also every `oauth2`
        /// and `openIdConnect` scheme, which attach their token as a bearer credential), or
        /// `"apiKey"`.
        required: &'static str,
        /// The `Credential` variant registered under `scheme`: `"Bearer"`, `"Basic"`, `"ApiKey"`,
        /// or `"Provider"`.
        registered: &'static str,
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
    /// Pre-send, raised before anything is transmitted: an unparseable base URL; a parameter,
    /// body, or credential value that did not serialize; or reqwest refusing the request when
    /// asked to send it (a builder-kind error), as it does for a URL scheme other than
    /// `http`/`https` or plain `http` on an `https_only` client.
    ///
    /// Not pre-send: an error reqwest classifies as a request error. reqwest raises that class
    /// from inside the send, so the request may already have been transmitted and the server may
    /// already have acted on it. This variant is therefore the one place in taxonomy #1 where "the
    /// request was never sent" does not hold, and a caller that retries on it must treat the call
    /// as possibly-already-applied. A connection that was never established is classified apart,
    /// as [`Error::from_reqwest`] describes.
    ///
    /// After a response was accepted: when the API has streaming (sequential) responses, the
    /// embedded `EventStream` raises this when the stored request cannot be cloned for an
    /// automatic reconnect — from `with_reconnect`, and from `poll_next` once the reconnect wait
    /// has elapsed, mid-stream, after frames may already have been yielded. Nothing was
    /// transmitted for the reconnect itself, so the possibly-already-applied reading above does
    /// not describe it; re-driving the operation from the start re-delivers events the caller has
    /// already consumed instead.
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

    /// Whether [`Error::from_reqwest`] would classify this failure as transient, decided on a
    /// borrow: [`crate::RetryBackend`] asks before it has to hand the error back unconsumed.
    pub(crate) fn is_transient(&self) -> bool {
        ReqwestClass::of(&self.source).is_transient()
    }
}

/// The taxonomy class [`Error::from_reqwest`] assigns a `reqwest::Error`, decided without
/// consuming it, so the retry adapter and the taxonomy read one classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReqwestClass {
    Timeout(TimeoutKind),
    Redirect,
    Protocol,
    Transport,
    Request,
}

impl ReqwestClass {
    /// The order is the classification: a timeout first, split by the connect discriminator; then
    /// redirect and decode; then a connection failure, which reqwest also reports as request-kind;
    /// then reqwest's request and builder kinds; anything else falls back to transport.
    fn of(error: &reqwest::Error) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let connect = error.is_connect();
        #[cfg(target_arch = "wasm32")]
        let connect = false;
        if error.is_timeout() {
            ReqwestClass::Timeout(if connect {
                TimeoutKind::Connect
            } else {
                TimeoutKind::Total
            })
        } else if error.is_redirect() {
            ReqwestClass::Redirect
        } else if error.is_decode() {
            ReqwestClass::Protocol
        } else if connect {
            ReqwestClass::Transport
        } else if error.is_request() || error.is_builder() {
            ReqwestClass::Request
        } else {
            ReqwestClass::Transport
        }
    }

    /// [`Error::is_transient`] of the variant [`Error::from_reqwest`] builds for this class.
    fn is_transient(self) -> bool {
        match self {
            ReqwestClass::Timeout(_) | ReqwestClass::Transport => true,
            ReqwestClass::Redirect | ReqwestClass::Protocol | ReqwestClass::Request => false,
        }
    }
}

/// The I/O error kinds that mean the connection gave out under a body being read, rather than
/// that the bytes which did arrive were malformed.
const CONNECTION_LOST: [std::io::ErrorKind; 5] = [
    std::io::ErrorKind::ConnectionReset,
    std::io::ErrorKind::ConnectionAborted,
    std::io::ErrorKind::BrokenPipe,
    std::io::ErrorKind::NotConnected,
    std::io::ErrorKind::UnexpectedEof,
];

/// Whether a failed body read is an interruption, as [`Error::from_body_read`] describes.
fn is_interrupted_read(error: &reqwest::Error) -> bool {
    if !error.is_body() && !error.is_decode() {
        return false;
    }
    let mut body_kind = error.is_body();
    let mut cause = std::error::Error::source(error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            return CONNECTION_LOST.contains(&io.kind());
        }
        body_kind |= current
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_body);
        cause = current.source();
    }
    body_kind
}

/// Which timeout elapsed (taxonomy #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// The connect timeout: no connection was established in time — the client's
    /// `connect_timeout` (which covers name resolution, the TCP handshake, and TLS) elapsed, or
    /// the operating system gave up on the handshake. Never reported on `wasm32`; see
    /// [`Error::from_reqwest`].
    Connect,
    /// Every other timeout: the client's total-request `timeout` elapsed, whichever phase the
    /// request was in (connecting included), or a `read_timeout` elapsed on an established
    /// connection.
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
                // name nothing; it renders without the clause. Every value the runtime builds
                // renders a non-empty clause, in declaration order.
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
            RequestError::CredentialMismatch {
                scheme,
                required,
                registered,
            } => write!(
                f,
                "the `Credential::{registered}` registered for security scheme `{scheme}` cannot \
                 satisfy its `{required}` type"
            ),
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
            RequestError::MissingCredential { .. } | RequestError::CredentialMismatch { .. } => {
                None
            }
            RequestError::CredentialProvider { source, .. } => Some(source),
            // The boxed cause itself, not its wrapper, so `downcast_ref` reaches the cause's own
            // type.
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
mod tests;
