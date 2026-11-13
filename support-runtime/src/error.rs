use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde_json::{Map, Value};

use crate::{AuthError, ResponseValue};

/// The closed error taxonomy shared by every spargen-generated client. `E` is the
/// operation's typed error body (an enum when several error statuses are documented).
///
/// Twelve variants are constructed: classes #1 to #9, and #11 to #13. Taxonomy class #10, cancellation, is a
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
    /// #4 — malformed HTTP or decompression failure. On native targets, a response body the
    /// connection cut short is not this class but [`Error::InterruptedBody`]; on `wasm32`, a
    /// non-streamed one is this class, as [`Error::InterruptedBody`] describes.
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
    /// response, or, on native targets, while any other response body, success or error, was
    /// being read. On `wasm32`, the `fetch` backend reports a non-streamed body that fails to
    /// read as a JavaScript error, which carries no evidence that the connection gave out, so
    /// that failure is [`Error::Protocol`] there.
    InterruptedBody(TransportError),
    /// #11 — a `LinkPaginator` page's `rel="next"` link named a URL the paginator had already
    /// fetched (a self-link, or a longer cycle such as page 1 → page 2 → page 1), so following it
    /// would never end. Every page before it was yielded; nothing was sent for `url`.
    PaginationCycle {
        /// The revisited URL the `next` link resolved to, without its userinfo and query, where a
        /// credential would ride.
        url: url::Url,
    },
    /// #12 — a non-streaming operation's response body exceeded the limit the call was made
    /// under: [`crate::RequestOptions::max_body`], or for a success body
    /// `ClientConfig::max_success_body`, and for a raw binary success body where neither is set
    /// `ClientConfig::DEFAULT_MAX_BINARY_BODY`; or the limit a [`crate::ByteStream`]'s
    /// `collect_bounded` was given. A `Content-Length` past it is refused before the body is
    /// read, unless the response carries no body (a `HEAD` request's answer, or a `1xx`, `204` or
    /// `304`); a body that grows past it is abandoned at the chunk that crosses it. Nothing of the
    /// body is retained. Not transient: the same limit refuses the same response again.
    BodyTooLarge {
        /// The limit, in bytes, the body exceeded.
        limit: usize,
        /// The status of the response whose body exceeded it.
        status: StatusCode,
    },
    /// #13 — a `LinkPaginator` page's `rel="next"` link named a URL whose scheme, host or port
    /// differs from the first page the paginator fetched: another host, another port, or an
    /// `https` chain dropping to `http`. Following it would send whatever the injected client
    /// attaches to every request (a default `Authorization` header, say) to that origin. Every
    /// page before it was yielded; nothing was sent for `to`. Not transient: the same link is
    /// refused again.
    PaginationOriginChanged {
        /// The first page the paginator fetched, whose origin every later page must share,
        /// without its userinfo and query, where a credential would ride.
        from: url::Url,
        /// The refused URL the `next` link resolved to, without its userinfo and query.
        to: url::Url,
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

    /// The taxonomy variant a backend's [`TransportError`] stands for: the class its constructor
    /// named decides. A timeout keeps only its [`TimeoutKind`]; a request-kind failure is
    /// [`RequestError::Other`] with the cause as its source; the redirect, protocol, connection
    /// and interruption classes keep it behind [`RedirectError`], [`ProtocolError`], and the
    /// transport error itself.
    pub(crate) fn from_transport(error: TransportError) -> Self {
        let TransportError { class, source } = error;
        match class {
            TransportClass::Timeout(kind) => Error::Timeout(kind),
            TransportClass::Redirect => Error::Redirect(RedirectError { source }),
            TransportClass::Protocol => Error::Protocol(ProtocolError { source }),
            TransportClass::Connection => Error::Transport(TransportError { class, source }),
            TransportClass::Interrupted => Error::InterruptedBody(TransportError { class, source }),
            TransportClass::Request => {
                Error::RequestConstruction(RequestError::Other(RequestCause(source)))
            }
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
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => false,
        }
    }

    /// The HTTP status the failed call's response carried: `Some` for a documented error status
    /// ([`Error::Api`], the same value as its `ResponseValue::status()`), for an undocumented
    /// status ([`Error::UnexpectedStatus`], which includes an undocumented 2xx), and for a
    /// response whose body failed to decode ([`Error::Decode`], success or documented error
    /// status alike), and for a response whose body exceeded its limit ([`Error::BodyTooLarge`]);
    /// `None` for every class that produced no response to read a status from.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Error::Api(value) => Some(value.status()),
            Error::UnexpectedStatus { status, .. }
            | Error::Decode { status, .. }
            | Error::BodyTooLarge { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. } => None,
        }
    }

    /// The headers of the response the failed call received: `Some` for [`Error::Api`] (the same
    /// map as its `ResponseValue::headers()`), [`Error::UnexpectedStatus`], and [`Error::Decode`],
    /// and `None` for every other class: those that produced no response, and
    /// [`Error::BodyTooLarge`], which keeps its response's status alone. A `Retry-After` on a
    /// `429` or `503` is read here whichever of those classes carried it.
    pub fn headers(&self) -> Option<&HeaderMap> {
        match self {
            Error::Api(value) => Some(value.headers()),
            Error::UnexpectedStatus { headers, .. } | Error::Decode { headers, .. } => {
                Some(headers)
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => None,
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
            Error::PaginationOriginChanged { from, to } => {
                Error::PaginationOriginChanged { from, to }
            }
            Error::BodyTooLarge { limit, status } => Error::BodyTooLarge { limit, status },
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
    type Body;
    /// The documented body this value carries, if its status documents one.
    fn body(&self) -> Option<&Self::Body>;
    /// The documented body this value carries, moved out, if its status documents one: `Some`
    /// exactly when [`body`](Self::body) is.
    fn into_body(self) -> Option<Self::Body>
    where
        Self: Sized;
}

impl ApiErrorBody for std::convert::Infallible {
    type Body = std::convert::Infallible;
    // Mutation testing: replacing either body with `None` is an equivalent mutant, declared rather
    // than killed. `Infallible` is uninhabited, so no `self` exists to call them with and no
    // test, or caller, can observe what they return.
    fn body(&self) -> Option<&Self::Body> {
        match *self {}
    }
    fn into_body(self) -> Option<Self::Body> {
        match self {}
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
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => None,
        }
    }

    /// The documented API error body, moved out of the error: `Some` exactly when
    /// [`Error::api_body`] is, for the same body. Read [`Error::status`] and [`Error::headers`]
    /// first where they are wanted too, since this consumes the error.
    pub fn into_api_body(self) -> Option<E::Body> {
        match self {
            Error::Api(value) => value.into_inner().into_body(),
            // As in `api_body`: every other class carries no typed body.
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::UnexpectedStatus { .. }
            | Error::Decode { .. }
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => None,
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
/// these member names it happens to carry. Every other member is an extension member, kept as
/// JSON in [`ProblemDetails::extensions`].
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
    /// Every member besides the five above, by name: RFC 9457's extension members, such as a
    /// validation problem's `errors`. A standard member present with another type than the RFC
    /// gives it is not an extension, and is in neither place.
    pub extensions: Map<String, Value>,
}

impl ProblemDetails {
    /// The members of `body` as it serializes to JSON: `None` when it does not serialize to a JSON
    /// object (a string, an array, `null`, or a failed serialization).
    pub fn of<T: serde::Serialize + ?Sized>(body: &T) -> Option<Self> {
        Self::from_value(serde_json::to_value(body).ok()?)
    }

    /// The members of a raw JSON body: `None` when the bytes are not a JSON object, which includes
    /// a body truncated by the error-body cap.
    pub fn from_json(body: &[u8]) -> Option<Self> {
        Self::from_value(serde_json::from_slice(body).ok()?)
    }

    fn from_value(value: Value) -> Option<Self> {
        let Value::Object(mut extensions) = value else {
            return None;
        };
        let mut text = |name: &str| match extensions.remove(name) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        };
        let problem_type = text("type");
        let title = text("title");
        let detail = text("detail");
        let instance = text("instance");
        let status = extensions
            .remove("status")
            .as_ref()
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| (100..=599).contains(status));
        Some(Self {
            problem_type,
            title,
            status,
            detail,
            instance,
            extensions,
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
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => None,
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
            // Origins only: the first page's URL is the caller's, and its query may hold a key.
            Error::PaginationOriginChanged { from, to } => write!(
                f,
                "pagination origin changed: the next link leaves {} for {}",
                from.origin().ascii_serialization(),
                to.origin().ascii_serialization()
            ),
            Error::BodyTooLarge { limit, status } => {
                write!(f, "response body ({status}) exceeds its {limit}-byte limit")
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
            | Error::PaginationCycle { .. }
            | Error::PaginationOriginChanged { .. }
            | Error::BodyTooLarge { .. } => None,
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
/// provider fails. All three are raised before anything is sent, and so is
/// [`RequestError::MissingBoundParameter`], when an operation declares a required header the
/// client binds and no value is bound to it. Every other
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
    /// anything but `Credential::Basic` under an `http basic` one, or a `Credential::Basic` whose
    /// username holds `:`, which RFC 7617 §2 forbids (the one case `registered` is `"Basic"` and
    /// `required` is `"http basic"`). Raised before anything is sent,
    /// and before a registered token provider is asked for a token. The payload is the whole cause,
    /// so `source()` is `None`.
    ///
    /// The registration is what needs correcting: attaching credentials never falls through to a
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
    /// The operation declares a required header that the generated client binds once, as a
    /// `Spec::bind_header` build option named it, and no value is bound to it on the client
    /// (`with_bound_header`). Raised before anything is sent. The payload is the whole cause, so
    /// `source()` is `None`.
    MissingBoundParameter {
        /// The header's name, as the operation declares it.
        name: &'static str,
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
    /// as [`TransportError::from_reqwest`] describes.
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

/// The failure an [`crate::HttpBackend`] reports, or an [`crate::HttpBody`] yields while it is
/// read, carrying the taxonomy class it belongs to; and the payload of the transport-layer
/// variants that keep it (taxonomy #2 / #9).
///
/// A backend picks the class through the constructor it builds the error with, and the
/// runtime's `send` maps the error to that class's [`Error`] variant: [`Self::connection`] to
/// [`Error::Transport`], [`Self::timeout`] to [`Error::Timeout`], [`Self::protocol`] to
/// [`Error::Protocol`], [`Self::redirect`] to [`Error::Redirect`], [`Self::request`] to
/// [`RequestError::Other`], and [`Self::interrupted`] to [`Error::InterruptedBody`].
/// [`Self::from_reqwest`] reads the class off a `reqwest::Error`, so [`crate::ReqwestBackend`]
/// classifies exactly as executing on a `reqwest::Client` always has. The cause is reachable
/// through [`std::error::Error::source`], and downcasts to its own type there.
#[derive(Debug)]
pub struct TransportError {
    class: TransportClass,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl TransportError {
    fn new(class: TransportClass, source: impl Into<BoxError>) -> Self {
        Self {
            class,
            source: source.into(),
        }
    }

    /// A connection-level failure: name resolution, a refused or reset connection, a TLS
    /// handshake or certificate error. It becomes [`Error::Transport`], which is transient.
    pub fn connection(source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Connection, source)
    }

    /// A timeout of the given kind. It becomes [`Error::Timeout`], which is transient; the cause
    /// is dropped there, as the variant carries only its [`TimeoutKind`].
    pub fn timeout(kind: TimeoutKind, source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Timeout(kind), source)
    }

    /// Malformed HTTP, or a body that could not be decoded (decompression). It becomes
    /// [`Error::Protocol`], which is not transient.
    pub fn protocol(source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Protocol, source)
    }

    /// Redirect-policy exhaustion. It becomes [`Error::Redirect`], which is not transient.
    pub fn redirect(source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Redirect, source)
    }

    /// The backend refused the request, or failed inside the send without a connection failure to
    /// show for it. It becomes [`RequestError::Other`], which is not transient; that variant's
    /// documentation says why such a request may already have been transmitted.
    pub fn request(source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Request, source)
    }

    /// The connection gave out partway through a body. It becomes [`Error::InterruptedBody`],
    /// which is transient; an [`crate::HttpBody`] whose source fails with an error of any other
    /// type reports it as this class.
    pub fn interrupted(source: impl Into<BoxError>) -> Self {
        Self::new(TransportClass::Interrupted, source)
    }

    /// Classify a `reqwest::Error` from executing a request into the closest runtime taxonomy
    /// class, keeping it as the source. Each class is named below by the [`Error`] variant
    /// the runtime's `send` maps it to.
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
    /// becomes [`Error::RequestConstruction`] wherever reqwest raises it: from `execute` on a
    /// request it was handed — a URL whose scheme is not `http`/`https`, plain `http` on an
    /// `https_only` client, or a URI that is not a URL. Re-sending it is refused again, so it is
    /// not transient.
    ///
    /// The query is taken off the URL the error carries, so neither the error's `Display` nor
    /// its `Debug` prints an `apiKey` query credential; the scheme, host and path stay.
    pub fn from_reqwest(error: reqwest::Error) -> Self {
        Self::new(TransportClass::of(&error), without_query(error))
    }

    /// Classify a `reqwest::Error` from reading the body of a response that has already
    /// arrived, the same way `EventStream` classifies one mid-stream: the connection giving out
    /// partway through the body is an interruption, and so transient.
    ///
    /// reqwest wraps any failure while reading a body in an error of its own, decode-kind or
    /// body-kind depending on how the body is read, so the cause under that wrapper decides. The
    /// first [`std::io::Error`] in its source chain is the evidence: a reset, aborted, broken,
    /// or unconnected connection, or one that ended before the body did, is an interruption,
    /// including a compressed body that ends before its compressed stream does; any other kind
    /// (corrupt compressed data, a malformed chunked encoding) is not. With no I/O error in the
    /// chain, a body-kind reqwest error in the chain is an interruption. A timeout is asked for
    /// first, and everything else is a protocol failure, so a body that is malformed rather than
    /// cut short stays [`Error::Protocol`]. On `wasm32`, reqwest's `fetch` backend reports a
    /// failed read over a JavaScript error, with neither an I/O error nor a body-kind error in
    /// its chain, so every such failure there is [`Error::Protocol`]. The query is taken off the
    /// URL the error carries, as for [`Self::from_reqwest`].
    pub(crate) fn from_reqwest_body(error: reqwest::Error) -> Self {
        let class = if error.is_timeout() {
            TransportClass::of(&error)
        } else if is_interrupted_read(&error) {
            TransportClass::Interrupted
        } else {
            TransportClass::Protocol
        };
        Self::new(class, without_query(error))
    }

    /// The error a body source failed with: a [`TransportError`] keeps its own class, and any
    /// other error is an interruption.
    pub(crate) fn from_body_source(error: BoxError) -> Self {
        match error.downcast::<TransportError>() {
            Ok(error) => *error,
            Err(other) => Self::new(TransportClass::Interrupted, other),
        }
    }

    /// Whether the [`Error`] variant [`crate::send`] maps this failure to is transient, decided
    /// on a borrow: [`crate::RetryBackend`] asks before it has to hand the error back unconsumed.
    pub(crate) fn is_transient(&self) -> bool {
        self.class.is_transient()
    }
}

/// `error` with the query taken off the URL it carries, so the error's `Display` and `Debug`
/// print the request's scheme, host and path but none of its query: an `apiKey` query credential
/// rides there, and the runtime keeps a reqwest error as the cause of the error a caller logs. Its class is read before this, so it classifies as before.
pub(crate) fn without_query(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    error
}

/// The boxed cause a [`TransportError`] carries.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The taxonomy class of a [`TransportError`], decided without consuming it, so the retry
/// adapter and the taxonomy read one classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportClass {
    Timeout(TimeoutKind),
    Redirect,
    Protocol,
    Connection,
    Request,
    Interrupted,
}

impl TransportClass {
    /// The class of a `reqwest::Error` from executing a request. The order is the
    /// classification: a timeout first, split by the connect discriminator; then redirect and
    /// decode; then a connection failure, which reqwest also reports as request-kind; then
    /// reqwest's request and builder kinds; anything else falls back to a connection failure.
    fn of(error: &reqwest::Error) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let connect = error.is_connect();
        #[cfg(target_arch = "wasm32")]
        let connect = false;
        if error.is_timeout() {
            TransportClass::Timeout(if connect {
                TimeoutKind::Connect
            } else {
                TimeoutKind::Total
            })
        } else if error.is_redirect() {
            TransportClass::Redirect
        } else if error.is_decode() {
            TransportClass::Protocol
        } else if connect {
            TransportClass::Connection
        } else if error.is_request() || error.is_builder() {
            TransportClass::Request
        } else {
            TransportClass::Connection
        }
    }

    /// [`Error::is_transient`] of the variant [`Error::from_transport`] builds for this class.
    fn is_transient(self) -> bool {
        match self {
            TransportClass::Timeout(_)
            | TransportClass::Connection
            | TransportClass::Interrupted => true,
            TransportClass::Redirect | TransportClass::Protocol | TransportClass::Request => false,
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

/// Whether a failed body read is an interruption, as [`TransportError::from_reqwest_body`]
/// describes. The error itself is reqwest's wrapper of the read failure, so only its causes are
/// read.
fn is_interrupted_read(error: &reqwest::Error) -> bool {
    let mut body_kind = false;
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
    /// [`TransportError::from_reqwest`].
    Connect,
    /// Every other timeout: the client's total-request `timeout` elapsed, whichever phase the
    /// request was in (connecting included), or a `read_timeout` elapsed on an established
    /// connection.
    Total,
}

/// Protocol-layer failure — malformed HTTP or decompression (taxonomy #4).
#[derive(Debug)]
pub struct ProtocolError {
    source: BoxError,
}

/// Redirect-policy exhaustion (taxonomy #5).
#[derive(Debug)]
pub struct RedirectError {
    source: BoxError,
}

/// The `FromStr` error of a generated closed string enum: the text is none of the values the
/// enum lists. It carries the rejected text and the enum's Rust name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownVariant {
    type_name: &'static str,
    value: String,
}

impl UnknownVariant {
    /// `value` was parsed as the enum named `type_name`, which lists no such value.
    pub fn new(type_name: &'static str, value: impl Into<String>) -> Self {
        Self {
            type_name,
            value: value.into(),
        }
    }

    /// The Rust name of the enum the text was parsed as.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The rejected text, exactly as it was given.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Display for UnknownVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not a value of {}", self.value, self.type_name)
    }
}

impl std::error::Error for UnknownVariant {}

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
                required: "http basic",
                registered: "Basic",
            } => write!(
                f,
                "the `Credential::Basic` registered for security scheme `{scheme}` has a username \
                 holding `:`, which `http basic` cannot carry (RFC 7617 §2)"
            ),
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
            RequestError::MissingBoundParameter { name } => write!(
                f,
                "the operation requires header `{name}`, and the client binds no value to it"
            ),
            RequestError::Other(cause) => write!(f, "{cause}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RequestError::MissingCredential { .. }
            | RequestError::CredentialMismatch { .. }
            | RequestError::MissingBoundParameter { .. } => None,
            RequestError::CredentialProvider { source, .. } => Some(source),
            // The boxed cause itself, not its wrapper, so `downcast_ref` reaches the cause's own
            // type.
            RequestError::Other(cause) => {
                Some(cause.0.as_ref() as &(dyn std::error::Error + 'static))
            }
        }
    }
}

macro_rules! impl_source_error {
    ($ty:ty, $label:literal) => {
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}: {}", $label, self.source)
            }
        }

        impl std::error::Error for $ty {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                // The boxed cause itself, not the box, so `downcast_ref` reaches its own type.
                Some(self.source.as_ref() as &(dyn std::error::Error + 'static))
            }
        }
    };
}

impl_source_error!(TransportError, "transport error");
impl_source_error!(ProtocolError, "protocol error");
impl_source_error!(RedirectError, "redirect error");

#[cfg(test)]
mod tests;
