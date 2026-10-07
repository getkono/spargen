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
/// Adding a variant, in spargen's own sources: raise `ERROR_VARIANTS` and list a value of the new
/// variant in `every_variant`, both in the test module of `support-runtime/src/error.rs`, for the
/// reasons `request_variant_index` there sets out. That test module is stripped when this file is
/// embedded into a generated client, so none of those three names exist in the copy a consumer
/// reads. Name the new variant, too, in the error-taxonomy passages of spargen's `README.md` and
/// `docs/book/src/getting-started.md`.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// #1 — a request could not be built, or failed inside the send before that request produced
    /// a response. Read it as a statement about *one* request, not about the operation: it is
    /// neither proof that nothing reached the server, nor proof that no response was already
    /// produced and delivered to the caller.
    ///
    /// Pre-send, raised while the request is still being assembled — nothing was transmitted:
    /// no registered credential satisfies the operation's security requirement, a registered
    /// credential is of a kind its scheme cannot carry, a registered token provider failed, the
    /// base URL is invalid, or a parameter or body did not serialize. Also
    /// pre-send: reqwest refusing the request when asked to send it (a builder-kind error), as it
    /// does for a URL scheme other than `http`/`https` or plain `http` on an `https_only` client.
    ///
    /// Not pre-send: an error reqwest classifies as a request error. reqwest raises that class
    /// from inside the send itself, so the request may already have been transmitted and the
    /// server may already have acted on it. Retrying it is not safe for a non-idempotent call.
    /// A connection that was never established is not in this class: reqwest reports it as a
    /// request error too, but it arrives as [`Error::Transport`] (off `wasm32`).
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
    /// [`RequestError`] types the three credential causes; every other cause — reqwest's own
    /// request-error and builder-error classes and both reconnect-clone failures included —
    /// arrives as
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
        /// The raw response body, capped at `max_error_body` by the dispatch helpers; unlike
        /// `Decode`, this variant carries no flag saying whether bytes were dropped to meet the
        /// cap. The generated shim for an operation with more than one documented success status
        /// reads through `read_success_body`, which does not take the cap, so on that path the
        /// body is retained whole (see `Decode`'s `body`).
        body: Bytes,
    },
    /// #8 — the response body failed to deserialize; retains the status and headers the response
    /// carried, the serde error path, and the raw body, capped on every path but the two named on
    /// `body` below. A documented error status whose body does not match its schema (a problem
    /// `type` the description does not list, or an HTML page from a proxy in front of the server)
    /// arrives here with its status and headers intact, so the caller can still act on them —
    /// honour a `Retry-After` on a `429`, say.
    Decode {
        /// The status of the response whose body failed to decode: a success status when a
        /// success body did not match its schema, the documented error status when a documented
        /// error body did not. For `EventStream`'s per-frame decode, it is the status of the
        /// response the frame was read from — after a reconnect, the reconnected response's.
        status: StatusCode,
        /// The headers of the response whose body failed to decode — for `EventStream`'s
        /// per-frame decode, of the response the frame was read from, as for `status`.
        headers: HeaderMap,
        /// The serde deserialization error path, held as serde wrote it; `Display` escapes its
        /// control characters, since the message may quote server-supplied input.
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

    /// Build the variant `class` names around `error`. Split from [`Self::from_reqwest`] so a test
    /// can pair every class with the variant it produces.
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

    /// Whether the failure is worth retrying: transport failures, timeouts, and a `429` or `5xx`
    /// response — whichever class carries it, including [`Error::Decode`], since a server that
    /// answered `503` is unavailable whether or not its body matched the schema. This is the
    /// rule `RetryOutcome::is_transient` applies to the undecoded response. Lets callers wrap any
    /// retry policy around the client without spargen shipping one.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
            Error::Api(value) => {
                value.status() == StatusCode::TOO_MANY_REQUESTS || value.status().is_server_error()
            }
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            Error::RequestConstruction(_) | Error::Protocol(_) | Error::Redirect(_) => false,
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
            | Error::InterruptedBody(_) => None,
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
            Error::Decode { status, path, .. } => {
                // `path` is serde's message, which quotes the server-supplied input (an unknown
                // enum variant, say) verbatim; its control characters are escaped so a value
                // holding a LF, CR or tab cannot break or forge the message's line.
                write!(f, "response decode failed ({status}) at ")?;
                for c in path.chars() {
                    if c.is_control() {
                        write!(f, "{}", c.escape_debug())?;
                    } else {
                        write!(f, "{c}")?;
                    }
                }
                Ok(())
            }
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
///
/// Adding a variant, in spargen's own sources: raise `REQUEST_VARIANTS` and list a value of the new
/// variant in `every_request_variant`, both in the test module of `support-runtime/src/error.rs`.
/// The compiler will demand the classification arms on its own, but it cannot demand the value —
/// `request_variant_index` there documents precisely why, and which ways of getting this wrong are
/// caught. That test module is stripped when this file is embedded into a generated client, so
/// none of those three names exist in the copy a consumer reads. Name the new variant, too, in the
/// error-taxonomy passages of spargen's `README.md` and `docs/book/src/getting-started.md`.
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
    /// Registration selected the alternative, so there is no fall-through to a later one: the
    /// registration is what needs correcting.
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
    /// Pre-send: an unparseable base URL, or a parameter, body, or credential value that did not
    /// serialize.
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

/// Which timeout elapsed (taxonomy #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// The connect timeout: no connection was established in time — the client's
    /// `connect_timeout` (which covers name resolution, the TCP handshake, and TLS) elapsed, or
    /// the operating system gave up on the handshake. Never reported on `wasm32`, where the fetch
    /// backend does not say which phase timed out.
    Connect,
    /// Every other timeout: the client's total-request `timeout` elapsed, whichever phase the
    /// request was in (connecting included), or a `read_timeout` elapsed on an established
    /// connection. On `wasm32` this is every timeout.
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
mod tests;
