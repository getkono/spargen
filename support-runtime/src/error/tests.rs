use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::{AuthError, ResponseValue, TransportError};

use super::{Error, RequestError, TimeoutKind, TransportClass};

#[test]
fn request_message_source_displays_the_message() {
    let error = Error::<std::convert::Infallible>::request_message("no credential for `token`");
    assert!(matches!(error, Error::RequestConstruction(_)));
    let source = std::error::Error::source(&error).expect("request errors carry a source");
    assert_eq!(source.to_string(), "no credential for `token`");
}

/// The missing-credential cause is the payload itself, so the chain ends at `RequestError`:
/// its `source()` is `None`, where `Other` reaches one level further, to its boxed cause.
///
/// The rendered text groups the missing schemes per alternative, in declaration order —
/// `(missing: key + token)` — rather than listing them once, sorted and deduplicated across
/// the whole requirement. The grouping is the payload's whole point, and the exact string
/// below pins it.
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
const REQUEST_VARIANTS: usize = 4;

/// Each variant's position in `every_request_variant`. Indices are dense and unique, which is
/// what `every_request_variant_lists_each_variant_exactly_once` checks.
///
/// Adding a variant to `RequestError`: classify it here, raise `REQUEST_VARIANTS`, and list a
/// value of it in `every_request_variant`. Adding one to `Error` is the same, through
/// `error_variant_index`, `ERROR_VARIANTS` and `every_variant`. What holds each step:
///
/// - The exhaustive match demands the classification at compile time.
/// - `every_error_variant_is_counted_by_its_enumeration` in
///   `spargen/tests/layering/runtime_embed.rs` holds `REQUEST_VARIANTS` and `ERROR_VARIANTS`, read
///   from this file's text, to the variant counts in `error.rs`'s text.
/// - The array length then demands a listed value at compile time, and the bijection test
///   rejects a duplicate of another variant padding the array.
///
/// The count is read from the text because `std::mem::variant_count` is nightly, and each
/// in-source alternative (a macro emitting the count beside the enum, `strum::EnumCount`, or a
/// `#[cfg_attr(test, derive(..))]`) would sit above the marker the embed splits on and ship in
/// every generated client.
fn request_variant_index(error: &RequestError) -> usize {
    match error {
        RequestError::MissingCredential { .. } => 0,
        RequestError::CredentialProvider { .. } => 1,
        RequestError::Other(_) => 2,
        RequestError::CredentialMismatch { .. } => 3,
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
        RequestError::CredentialMismatch {
            scheme: "login",
            required: "http basic",
            registered: "Provider",
        },
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

/// Every variant renders a non-empty string that names its own cause.
///
/// `CredentialProvider` renders a fixed sentence naming the scheme, not the provider's own
/// text (`"refresh rejected"`), which is one level further out as its `source()`, the
/// provider's `AuthError`. The chain is three levels — `Error`, `RequestError`, `AuthError` —
/// and the middle one does not repeat the last, so a full-chain renderer prints each piece
/// once. A consumer that prints exactly one `.source()` level, or that binds
/// `Err(Error::RequestConstruction(e))` and prints `{e}`, sees the scheme sentence.
/// `RequestError` is publicly re-exported, so that level is directly printable and is not an
/// internal detail.
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
            RequestError::CredentialMismatch { .. } => {
                "the `Credential::Provider` registered for security scheme `login` cannot \
                 satisfy its `http basic` type"
            }
        };
        assert_eq!(rendered, expected, "a variant does not name its cause");
    }
}

/// `MissingCredential` and `CredentialMismatch` *are* the whole cause, so they end the chain;
/// the other two carry a separate cause and must hand it over. A consumer walking the chain
/// must not find a phantom source, nor lose a real one.
#[test]
fn request_source_is_present_exactly_where_the_cause_is_separate() {
    for error in every_request_variant() {
        let expected = match &error {
            RequestError::MissingCredential { .. } | RequestError::CredentialMismatch { .. } => {
                false
            }
            RequestError::CredentialProvider { .. } | RequestError::Other(_) => true,
        };
        assert_eq!(
            std::error::Error::source(&error).is_some(),
            expected,
            "source() disagrees for {error}"
        );
    }
}

/// No request-construction variant is classified transient, and none carries a response: no
/// status, no typed body, because taxonomy #1 never has one to carry. Non-transmission is
/// *not* the shared reason — `RequestError::Other` can arrive from inside reqwest's send, so
/// it is the one variant where "the request was never sent" does not hold; see its own
/// documentation before retrying on it. Pinned over every `RequestError` variant so a new one
/// cannot arrive misclassified.
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
const ERROR_VARIANTS: usize = 11;

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
        Error::PaginationCycle { .. } => 9,
        Error::BodyTooLarge { .. } => 10,
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
        Error::Transport(TransportError::from_reqwest(reqwest_error())),
        Error::Timeout(TimeoutKind::Total),
        // The field is private, but these tests live in the defining module.
        Error::Protocol(super::ProtocolError {
            source: Box::new(reqwest_error()),
        }),
        Error::Redirect(super::RedirectError {
            source: Box::new(reqwest_error()),
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
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: false,
        },
        Error::InterruptedBody(TransportError::interrupted(reqwest_error())),
        Error::PaginationCycle {
            url: reqwest::Url::parse("https://x/items?page=1").unwrap(),
        },
        Error::BodyTooLarge {
            limit: 1024,
            status: StatusCode::CREATED,
        },
    ]
}

/// `from_reqwest` is the taxonomy: every failure a [`crate::HttpBackend`] reports is mapped
/// through it, which is what keeps a custom transport classifying identically to executing on
/// a `reqwest::Client` directly. Its timeout, redirect, decode, and request branches all need a
/// live connection attempt to reach (reqwest exposes no constructor for its own error; the two
/// timeout kinds are driven over a real client in `spargen/tests/e2e.rs`), so what is pinned
/// here is the builder branch, the one reqwest raises without any I/O.
///
/// A builder-kind error means reqwest refused the request: nothing was sent, and re-sending
/// it is refused again. reqwest raises it from `RequestBuilder::build` (an unparseable URL)
/// and also from `execute` on a request that built fine — a URL whose scheme is not
/// `http`/`https`, or plain `http` on an `https_only` client. The second arrives through the
/// backend as a `TransportError`, so without this branch the same refusal would be a
/// non-retryable `RequestConstruction` at build time and a retryable `Transport` at execute.
#[test]
fn from_reqwest_classifies_a_refused_request_as_request_construction() {
    // reqwest answers without touching the network, so the first poll is ready.
    let refused = crate::tests::refused_at_execute().map(|(client, request)| {
        crate::tests::poll_ready(client.execute(request)).expect_err("reqwest refuses the request")
    });
    for source in std::iter::once(reqwest_error()).chain(refused) {
        assert!(source.is_builder(), "{source}");
        let error = Error::<ApiBody>::from_reqwest(source);
        assert!(
            matches!(error, Error::RequestConstruction(RequestError::Other(_))),
            "{error}"
        );
        assert!(!error.is_transient(), "{error}");
        assert!(std::error::Error::source(&error).is_some());
    }
}

/// `TransportError::is_transient` is what `RetryOutcome::is_transient` answers for a transport
/// failure of an idempotent request, and it is decided on a borrow, apart from the variant
/// `from_transport` builds. Pair every class with that variant so the two cannot disagree, and
/// every public constructor with the variant its documentation names.
#[test]
fn every_transport_class_is_transient_exactly_when_its_variant_is() {
    let source = || std::io::Error::other("stand-in cause");
    let constructed = [
        (
            TransportClass::Timeout(TimeoutKind::Connect),
            TransportError::timeout(TimeoutKind::Connect, source()),
        ),
        (
            TransportClass::Timeout(TimeoutKind::Total),
            TransportError::timeout(TimeoutKind::Total, source()),
        ),
        (TransportClass::Redirect, TransportError::redirect(source())),
        (TransportClass::Protocol, TransportError::protocol(source())),
        (
            TransportClass::Connection,
            TransportError::connection(source()),
        ),
        (TransportClass::Request, TransportError::request(source())),
        (
            TransportClass::Interrupted,
            TransportError::interrupted(source()),
        ),
    ];
    for (class, transport) in constructed {
        // Exhaustive, so a class added without a row here does not compile.
        let expected = |error: &Error<ApiBody>| match class {
            TransportClass::Timeout(kind) => {
                matches!(error, Error::Timeout(found) if *found == kind)
            }
            TransportClass::Redirect => matches!(error, Error::Redirect(_)),
            TransportClass::Protocol => matches!(error, Error::Protocol(_)),
            TransportClass::Connection => matches!(error, Error::Transport(_)),
            TransportClass::Request => {
                matches!(error, Error::RequestConstruction(RequestError::Other(_)))
            }
            TransportClass::Interrupted => matches!(error, Error::InterruptedBody(_)),
        };
        assert_eq!(transport.class, class);
        let transient = transport.is_transient();
        assert_eq!(class.is_transient(), transient, "{class:?}");
        let error = Error::<ApiBody>::from_transport(transport);
        assert!(expected(&error), "{class:?}: {error:?}");
        assert_eq!(error.is_transient(), transient, "{class:?}");
        // Every class but a timeout keeps the cause, reachable as its own type.
        let mut cause = std::error::Error::source(&error);
        let mut kept = false;
        while let Some(current) = cause {
            kept |= current.is::<std::io::Error>();
            cause = current.source();
        }
        assert_eq!(
            kept,
            !matches!(class, TransportClass::Timeout(_)),
            "{class:?}"
        );
    }
    // The borrowed classification is the one `from_reqwest` applies to the same error.
    let transport = TransportError::from_reqwest(reqwest_error());
    assert_eq!(
        transport.is_transient(),
        Error::<ApiBody>::from_reqwest(reqwest_error()).is_transient()
    );
}

/// A body source's failure keeps the class a `TransportError` names, and any other error is an
/// interruption.
#[test]
fn a_body_source_failure_keeps_its_class_or_is_an_interruption() {
    let classed = TransportError::from_body_source(Box::new(TransportError::protocol("bad")));
    assert_eq!(classed.class, TransportClass::Protocol);
    let other = TransportError::from_body_source(Box::new(std::fmt::Error));
    assert_eq!(other.class, TransportClass::Interrupted);
    assert!(std::error::Error::source(&other).is_some_and(|c| c.is::<std::fmt::Error>()));
}

/// A body stream that answers its first poll with `error`.
struct FailingBody(Option<BoxError>);

/// The error a `FailingBody` fails with.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl futures_core::Stream for FailingBody {
    type Item = Result<Bytes, BoxError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.0.take().map(Err))
    }
}

/// What reading a body through `ReqwestBackend`'s adapter fails with when its stream fails with
/// an I/O error of `kind`.
fn failed_body_read(kind: std::io::ErrorKind) -> TransportError {
    body_read_failing_with(Box::new(std::io::Error::new(kind, "body read failed")))
}

/// What reading a body through `ReqwestBackend`'s adapter fails with when its stream fails with
/// `error`.
fn body_read_failing_with(error: BoxError) -> TransportError {
    let body = reqwest::Body::wrap_stream(FailingBody(Some(error)));
    crate::tests::poll_ready(crate::tests::response(200, body).into_body().collect())
        .expect_err("the body fails to read")
}

/// The reqwest error a failed body read through the adapter carries as its cause.
fn reqwest_cause(failure: &TransportError) -> &reqwest::Error {
    std::error::Error::source(failure)
        .and_then(|cause| cause.downcast_ref::<reqwest::Error>())
        .expect("reqwest's error is the cause")
}

/// With no I/O error anywhere in its source chain, a failed body read is an interruption exactly
/// when a body-kind reqwest error is in the chain under reqwest's own wrapper. reqwest's
/// streaming `Body` wraps whatever its stream failed with as a body-kind error, and reading the
/// response body wraps that once more, so a stream failing with a non-I/O error gives reqwest's
/// wrapper over a body-kind error over that error.
#[test]
fn a_failed_body_read_with_no_io_error_is_interrupted_by_a_body_kind_cause() {
    let failure = body_read_failing_with(Box::new(std::fmt::Error));
    let source = reqwest_cause(&failure);
    assert!(!source.is_timeout());
    let mut chain = std::error::Error::source(source);
    let (mut io, mut body_kind) = (false, false);
    while let Some(cause) = chain {
        io |= cause.is::<std::io::Error>();
        body_kind |= cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_body);
        chain = cause.source();
    }
    assert!(!io && body_kind, "{source:?}");
    let error = Error::<ApiBody>::from_transport(failure);
    assert!(matches!(error, Error::InterruptedBody(_)), "{error:?}");
    assert!(error.is_transient());
}

/// `from_reqwest_body` reads the I/O error under a failed body read: a connection that gave out
/// is a transient `InterruptedBody`, corrupt or malformed bytes (what a decompressor or a chunked
/// decoder reports) the permanent `Protocol`, and a timed-out read a `Total` timeout.
#[test]
fn a_failed_body_read_is_interrupted_exactly_when_the_connection_gave_out() {
    use std::io::ErrorKind;
    for (kind, interrupted) in [
        (ErrorKind::ConnectionReset, true),
        (ErrorKind::ConnectionAborted, true),
        (ErrorKind::BrokenPipe, true),
        (ErrorKind::NotConnected, true),
        (ErrorKind::UnexpectedEof, true),
        (ErrorKind::InvalidData, false),
        (ErrorKind::InvalidInput, false),
        (ErrorKind::Other, false),
    ] {
        let failure = failed_body_read(kind);
        assert!(
            !reqwest_cause(&failure).is_timeout(),
            "{kind:?}: {failure:?}"
        );
        let error = Error::<ApiBody>::from_transport(failure);
        if interrupted {
            assert!(
                matches!(error, Error::InterruptedBody(_)),
                "{kind:?}: {error:?}"
            );
        } else {
            assert!(matches!(error, Error::Protocol(_)), "{kind:?}: {error:?}");
        }
        assert_eq!(error.is_transient(), interrupted, "{kind:?}");
    }
    let timed_out = Error::<ApiBody>::from_transport(failed_body_read(ErrorKind::TimedOut));
    assert!(
        matches!(timed_out, Error::Timeout(TimeoutKind::Total)),
        "{timed_out:?}"
    );
}

/// A failed body read with neither an I/O error nor a body-kind error under reqwest's wrapper is
/// a protocol failure, whatever kind the wrapper itself is.
#[test]
fn a_body_read_error_with_no_evidence_of_interruption_is_a_protocol_failure() {
    let error =
        Error::<ApiBody>::from_transport(TransportError::from_reqwest_body(reqwest_error()));
    assert!(matches!(error, Error::Protocol(_)), "{error:?}");
}

/// The borrowed classification answers `true` too, not only `false`: the builder-kind error
/// above is the one permanent class, so a transient source is needed beside it. A connect
/// failure needs a socket and a reactor, which only the `blocking` feature's test below runs;
/// reqwest's status-kind error needs neither, and it reaches the same connection class a failed
/// connection does, by the fall-through `TransportClass::of` ends in. A retry policy keying on
/// the default classifier therefore retries it.
#[test]
fn a_transport_class_failure_is_transient_on_the_borrow_and_to_a_retry_policy() {
    let status_error = || {
        crate::tests::reqwest_response(503, String::new())
            .error_for_status()
            .expect_err("a 503 is an error status")
    };
    let source = status_error();
    assert!(source.is_status() && !source.is_connect(), "{source}");
    assert_eq!(TransportClass::of(&source), TransportClass::Connection);
    let transport = TransportError::from_reqwest(source);
    assert!(transport.is_transient());
    let request = crate::tests::seam_request();
    assert!(crate::RetryOutcome::new(&request, Err(&transport)).is_transient());
    let error = Error::<ApiBody>::from_reqwest(status_error());
    assert!(matches!(error, Error::Transport(_)), "{error}");
    assert!(error.is_transient());
}

/// A connection the peer refuses is the class `from_reqwest` asks for first among reqwest's
/// request-kind errors: `is_connect` makes it `Transport`, transient on the error and on the
/// borrow alike, and never `RequestConstruction`. Driven over a real socket on the `blocking`
/// feature's runtime: a port bound and then released refuses the next connection to it.
#[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
#[test]
fn a_refused_connection_is_a_transient_transport_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("bind an ephemeral port")
        .port();
    let url = format!("http://127.0.0.1:{port}/op");
    let runtime = crate::BlockingRuntime::new().expect("current-thread runtime builds");
    let core = crate::tests::core();
    let request = || core.http().get(&url).build().unwrap();
    let seam_request = || crate::transport::request_from_reqwest(request()).unwrap();

    let failure = runtime
        .block_on(crate::HttpBackend::execute(
            core.backend().as_ref(),
            seam_request(),
        ))
        .expect_err("nothing listens on a released port");
    assert!(failure.is_transient(), "{failure}");
    assert!(crate::RetryOutcome::new(&seam_request(), Err(&failure)).is_transient());

    let error = runtime
        .block_on(crate::send(&core, request()))
        .expect_err("nothing listens on a released port");
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert!(error.is_transient());
}

#[test]
fn is_transient_classifies_every_variant() {
    for error in every_variant() {
        let expected = match &error {
            // Worth retrying: the failure is about the connection, not the request.
            Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
            // A response status is retryable only when the server said so, whichever class
            // carries it: documented, undocumented, or undecodable.
            Error::Api(value) => {
                value.status() == StatusCode::TOO_MANY_REQUESTS || value.status().is_server_error()
            }
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            // Deterministic failures: retrying reproduces them.
            Error::RequestConstruction(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::PaginationCycle { .. }
            | Error::BodyTooLarge { .. } => false,
        };
        assert_eq!(
            error.is_transient(),
            expected,
            "is_transient disagrees for {error}"
        );
    }
}

/// `status` answers exactly for the four classes that carry a response status; every other
/// class produced no response to read one from.
#[test]
fn status_is_present_exactly_on_the_four_status_variants() {
    for error in every_variant() {
        let expected = match &error {
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
            | Error::PaginationCycle { .. } => None,
        };
        assert_eq!(error.status(), expected, "status() disagrees for {error}");
    }
    let statuses: Vec<_> = every_variant().iter().filter_map(Error::status).collect();
    assert_eq!(
        statuses,
        [
            StatusCode::BAD_REQUEST,
            StatusCode::IM_A_TEAPOT,
            StatusCode::OK,
            StatusCode::CREATED
        ]
    );
}

/// Generated clients hold `Error<Infallible>` for an operation with no documented error body,
/// so `status` is pinned on that instantiation too, over every variant it can hold (`Api` is
/// statically unreachable there): `UnexpectedStatus` and `Decode` answer, each with its own
/// code.
#[test]
fn status_on_an_uninhabited_api_error_is_present_only_for_the_response_variants() {
    let narrow: Vec<Error<std::convert::Infallible>> = vec![
        Error::request_message("bad path segment"),
        Error::Transport(TransportError::from_reqwest(reqwest_error())),
        Error::Timeout(TimeoutKind::Connect),
        Error::Protocol(super::ProtocolError {
            source: Box::new(reqwest_error()),
        }),
        Error::Redirect(super::RedirectError {
            source: Box::new(reqwest_error()),
        }),
        Error::UnexpectedStatus {
            status: StatusCode::IM_A_TEAPOT,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"teapot"),
        },
        Error::Decode {
            status: StatusCode::PARTIAL_CONTENT,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: true,
        },
        Error::InterruptedBody(TransportError::interrupted(reqwest_error())),
        Error::PaginationCycle {
            url: reqwest::Url::parse("https://x/items?page=1").unwrap(),
        },
        Error::BodyTooLarge {
            limit: 8,
            status: StatusCode::ACCEPTED,
        },
    ];
    for error in &narrow {
        let expected = match error {
            Error::Api(value) => match *value.inner() {},
            Error::UnexpectedStatus { status, .. }
            | Error::Decode { status, .. }
            | Error::BodyTooLarge { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => None,
        };
        assert_eq!(error.status(), expected, "status() disagrees for {error}");
    }
    let statuses: Vec<_> = narrow.iter().filter_map(Error::status).collect();
    assert_eq!(
        statuses,
        [
            StatusCode::IM_A_TEAPOT,
            StatusCode::PARTIAL_CONTENT,
            StatusCode::ACCEPTED
        ]
    );
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
            Error::Api(ResponseValue::new(status, HeaderMap::new(), ApiBody("x"))).is_transient(),
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
            Error::PaginationCycle { .. } => "pagination cycle",
            Error::BodyTooLarge { .. } => "1024-byte limit",
        };
        assert!(
            rendered.contains(expected),
            "{rendered:?} does not name its class ({expected:?})"
        );
    }
}

/// A decode failure's message names the status it arrived with, so a log line tells a drifted
/// `200` from an error page served under a documented status without matching the variant.
#[test]
fn a_decode_error_displays_its_status_and_path() {
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::BAD_GATEWAY,
        headers: HeaderMap::new(),
        path: "items[0].id".to_owned(),
        body: Bytes::new(),
        truncated: false,
    };
    assert_eq!(
        error.to_string(),
        "response decode failed (502 Bad Gateway) at items[0].id"
    );
}

/// #457: the path quotes server-supplied text; its LF, CR, tab and other control characters
/// are escaped so the message stays on one line, while the field keeps serde's text.
#[test]
fn a_decode_error_escapes_its_paths_control_characters() {
    let path = "unknown variant `x\ny\r\tz\u{1b}`, expected `ready`";
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        path: path.to_owned(),
        body: Bytes::new(),
        truncated: false,
    };
    assert_eq!(
        error.to_string(),
        r"response decode failed (200 OK) at unknown variant `x\ny\r\tz\u{1b}`, expected `ready`"
    );
    let Error::Decode { path: kept, .. } = error else {
        unreachable!()
    };
    assert_eq!(kept, path);
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
            Error::Timeout(_)
            | Error::UnexpectedStatus { .. }
            | Error::Decode { .. }
            | Error::PaginationCycle { .. }
            | Error::BodyTooLarge { .. } => false,
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
        Error::Transport(TransportError::from_reqwest(reqwest_error())),
        Error::Timeout(TimeoutKind::Connect),
        Error::Protocol(super::ProtocolError {
            source: Box::new(reqwest_error()),
        }),
        Error::Redirect(super::RedirectError {
            source: Box::new(reqwest_error()),
        }),
        Error::UnexpectedStatus {
            status: StatusCode::IM_A_TEAPOT,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"teapot"),
        },
        Error::Decode {
            status: StatusCode::SERVICE_UNAVAILABLE,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: true,
        },
        Error::InterruptedBody(TransportError::interrupted(reqwest_error())),
        Error::PaginationCycle {
            url: reqwest::Url::parse("https://x/items?page=1").unwrap(),
        },
        Error::BodyTooLarge {
            limit: 8,
            status: StatusCode::OK,
        },
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

    // The payload fields survive, not just the discriminant: the timeout keeps its kind, ...
    for kind in [TimeoutKind::Connect, TimeoutKind::Total] {
        let widened: Error<ApiBody> = Error::<std::convert::Infallible>::Timeout(kind).widen();
        assert!(
            matches!(widened, Error::Timeout(widened_kind) if widened_kind == kind),
            "{kind:?}"
        );
    }

    // ... the decode failure every field ...
    let mut sent = HeaderMap::new();
    sent.insert(
        "retry-after",
        reqwest::header::HeaderValue::from_static("30"),
    );
    let widened: Error<ApiBody> = Error::<std::convert::Infallible>::Decode {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        headers: sent.clone(),
        path: "a.b".to_owned(),
        body: Bytes::from_static(b"raw"),
        truncated: true,
    }
    .widen();
    let Error::Decode {
        status,
        headers,
        path,
        body,
        truncated,
    } = widened
    else {
        panic!("widen changed the variant");
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(headers, sent);
    assert_eq!(path, "a.b");
    assert_eq!(body, Bytes::from_static(b"raw"));
    assert!(truncated);

    // ... and the typed request-construction cause its alternatives.
    let widened: Error<ApiBody> =
        Error::<std::convert::Infallible>::RequestConstruction(RequestError::MissingCredential {
            alternatives: vec![vec!["a"], vec!["b"]],
        })
        .widen();
    let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = widened
    else {
        panic!("widen changed the variant");
    };
    assert_eq!(alternatives, [vec!["a"], vec!["b"]]);

    // ... and the pagination cycle the URL it revisited.
    let revisited = reqwest::Url::parse("https://x/items?page=2").unwrap();
    let widened: Error<ApiBody> = Error::<std::convert::Infallible>::PaginationCycle {
        url: revisited.clone(),
    }
    .widen();
    let Error::PaginationCycle { url } = widened else {
        panic!("widen changed the variant");
    };
    assert_eq!(url, revisited);

    // ... and the oversized body its limit and status.
    let widened: Error<ApiBody> = Error::<std::convert::Infallible>::BodyTooLarge {
        limit: 4096,
        status: StatusCode::NOT_FOUND,
    }
    .widen();
    let Error::BodyTooLarge { limit, status } = widened else {
        panic!("widen changed the variant");
    };
    assert_eq!((limit, status), (4096, StatusCode::NOT_FOUND));
}

impl super::ApiErrorBody for ApiBody {
    type Body = &'static str;
    fn body(&self) -> Option<&&'static str> {
        Some(&self.0)
    }
    fn into_body(self) -> Option<&'static str> {
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
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. }
            | Error::BodyTooLarge { .. } => false,
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
    assert_eq!(api.api_body(), Some(&"bad request"));
}

/// `into_api_body` is `api_body` by value: `Some` on exactly the variants `api_body` answers
/// `Some` on, with the same body, and `None` everywhere else.
#[test]
fn into_api_body_moves_out_exactly_what_api_body_borrows() {
    for error in every_variant() {
        let borrowed = error.api_body().copied();
        let rendered = error.to_string();
        assert_eq!(
            error.into_api_body(),
            borrowed,
            "into_api_body disagrees with api_body for {rendered}"
        );
    }
    let api = Error::Api(ResponseValue::new(
        StatusCode::BAD_REQUEST,
        HeaderMap::new(),
        ApiBody("bad request"),
    ));
    assert_eq!(api.into_api_body(), Some("bad request"));
}

/// `headers` answers where `status` does, from the same response: the `ResponseValue`'s own map
/// for a documented API error, and the variant's field for an undocumented status and an
/// undecodable body. Every class that received no response has neither, and an oversized body
/// keeps its status alone.
#[test]
fn headers_are_present_exactly_where_a_status_is_and_are_the_responses_own() {
    for error in every_variant() {
        match &error {
            Error::Api(value) => {
                let answered = error.headers().expect("an API error carries headers");
                assert!(std::ptr::eq(answered, value.headers()), "{error}");
            }
            Error::UnexpectedStatus { headers, .. } | Error::Decode { headers, .. } => {
                let answered = error
                    .headers()
                    .expect("a received response carries headers");
                assert!(std::ptr::eq(answered, headers), "{error}");
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => {
                assert!(error.headers().is_none(), "{error}");
                assert!(error.status().is_none(), "{error}");
            }
            Error::BodyTooLarge { .. } => {
                assert!(error.headers().is_none(), "{error}");
                assert!(error.status().is_some(), "{error}");
            }
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
    let throttled = Error::<ApiBody>::UnexpectedStatus {
        status: StatusCode::TOO_MANY_REQUESTS,
        headers,
        body: Bytes::new(),
    };
    let retry_after = throttled
        .headers()
        .and_then(|headers| headers.get(reqwest::header::RETRY_AFTER));
    assert_eq!(retry_after.map(|value| value.as_bytes()), Some(&b"120"[..]));
}

/// `status` and `api_body` read one `Error` from two sides, so they must agree on what each
/// class carries: a documented API error answers both from the same `ResponseValue`, an
/// undocumented status and an undecodable body have a status but no typed body, and every
/// other class has neither.
#[test]
fn status_and_api_body_agree_on_every_variant() {
    for error in every_variant() {
        match &error {
            Error::Api(value) => {
                assert_eq!(error.status(), Some(value.status()), "{error}");
                let same_body = match (error.api_body(), super::ApiErrorBody::body(value.inner())) {
                    (Some(answered), Some(carried)) => std::ptr::eq(answered, carried),
                    _ => false,
                };
                assert!(
                    same_body,
                    "api_body is not the Api value's own body: {error}"
                );
            }
            Error::UnexpectedStatus { status, .. }
            | Error::Decode { status, .. }
            | Error::BodyTooLarge { status, .. } => {
                assert_eq!(error.status(), Some(*status), "{error}");
                assert!(error.api_body().is_none(), "{error}");
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. } => {
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
    assert!(error.problem().is_none());
}

/// `ApiBody` stands in for a generated error type whose documented body is a problem object:
/// it answers with a fixed problem, so the test can tell the typed path from the raw one.
impl super::ApiErrorProblem for ApiBody {
    fn problem(&self) -> Option<super::ProblemDetails> {
        Some(super::ProblemDetails {
            detail: Some(self.0.to_owned()),
            ..super::ProblemDetails::default()
        })
    }
}

/// `problem` answers from the typed body on `Api`, from the raw body on the two classes that
/// retain one (only for an error status), and `None` everywhere else.
#[test]
fn problem_is_read_exactly_where_an_error_body_was_received() {
    for error in every_variant() {
        let problem = error.problem();
        match &error {
            Error::Api(_) => {
                assert_eq!(
                    problem.and_then(|problem| problem.detail).as_deref(),
                    Some("bad request"),
                    "the typed body answers on Api"
                );
            }
            // `every_variant`'s undocumented `418` carries an empty body, and its `Decode` a
            // `200` success status: neither is a problem object.
            Error::UnexpectedStatus { .. } | Error::Decode { .. } => {
                assert!(problem.is_none(), "{error}");
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_)
            | Error::PaginationCycle { .. }
            | Error::BodyTooLarge { .. } => assert!(problem.is_none(), "{error}"),
        }
    }
}

const PROBLEM: &[u8] = br#"{"type":"https://example.com/probs/out-of-credit","title":"Out of credit","status":403,"detail":"Your balance is 30.","instance":"/account/12345/msgs/abc","balance":30}"#;

/// A documented error status whose body failed to decode — a problem `type` the description
/// does not list — still yields its members, which is what #268 needed of a generic reader.
#[test]
fn a_decode_failure_on_an_error_status_reads_the_raw_problem() {
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::FORBIDDEN,
        headers: HeaderMap::new(),
        path: "type".to_owned(),
        body: Bytes::from_static(PROBLEM),
        truncated: false,
    };
    assert_eq!(
        error.problem(),
        Some(super::ProblemDetails {
            problem_type: Some("https://example.com/probs/out-of-credit".to_owned()),
            title: Some("Out of credit".to_owned()),
            status: Some(403),
            detail: Some("Your balance is 30.".to_owned()),
            instance: Some("/account/12345/msgs/abc".to_owned()),
            extensions: serde_json::Map::from_iter([("balance".to_owned(), 30.into())]),
        })
    );
    let undocumented = Error::<ApiBody>::UnexpectedStatus {
        status: StatusCode::SERVICE_UNAVAILABLE,
        headers: HeaderMap::new(),
        body: Bytes::from_static(PROBLEM),
    };
    assert_eq!(
        undocumented
            .problem()
            .and_then(|problem| problem.problem_type),
        Some("https://example.com/probs/out-of-credit".to_owned())
    );
}

/// A success status is never read, even when its body looks like a problem object: an
/// undecodable `200` is a success body that did not match, not an error response.
#[test]
fn a_success_status_is_never_read_as_a_problem() {
    for status in [StatusCode::OK, StatusCode::NOT_MODIFIED] {
        let decode = Error::<ApiBody>::Decode {
            status,
            headers: HeaderMap::new(),
            path: String::new(),
            body: Bytes::from_static(PROBLEM),
            truncated: false,
        };
        assert!(decode.problem().is_none(), "{status}");
        let unexpected = Error::<ApiBody>::UnexpectedStatus {
            status,
            headers: HeaderMap::new(),
            body: Bytes::from_static(PROBLEM),
        };
        assert!(unexpected.problem().is_none(), "{status}");
    }
}

/// Members are read by their RFC 9457 type: a wrongly-typed member is `None` without costing
/// the others, a non-object body answers `None` as a whole, and so does a truncated one.
#[test]
fn problem_members_are_read_only_with_their_rfc_types() {
    let mixed = super::ProblemDetails::from_json(
        br#"{"type":7,"title":"t","status":"403","detail":null,"instance":"/i"}"#,
    )
    .expect("an object body");
    assert_eq!(
        mixed,
        super::ProblemDetails {
            problem_type: None,
            title: Some("t".to_owned()),
            status: None,
            detail: None,
            instance: Some("/i".to_owned()),
            extensions: serde_json::Map::new(),
        }
    );
    assert_eq!(mixed.problem_type_or_blank(), "about:blank");
    for status in ["99", "600", "70000", "-1", "403.5"] {
        let body = format!(r#"{{"status":{status}}}"#);
        let read = super::ProblemDetails::from_json(body.as_bytes()).expect("an object");
        assert_eq!(
            read.status, None,
            "status {status} is outside the HTTP range"
        );
    }
    for body in [
        &b"[]"[..],
        b"\"text\"",
        b"null",
        b"<html>",
        b"",
        &PROBLEM[..40],
    ] {
        assert!(super::ProblemDetails::from_json(body).is_none());
    }
}

/// Every member but the five standard names is an extension, kept as its JSON, nested values
/// included; a standard name is never an extension, even when its type keeps it out of its field.
#[test]
fn extension_members_are_every_member_but_the_standard_five() {
    let read = super::ProblemDetails::from_json(
        br#"{"type":"t","title":7,"status":"400","detail":"d","instance":null,
             "errors":[{"pointer":"/name","detail":"required"}],"retry":true,"Type":"x"}"#,
    )
    .expect("an object body");
    assert_eq!(
        serde_json::Value::Object(read.extensions),
        serde_json::json!({
            "errors": [{"pointer": "/name", "detail": "required"}],
            "retry": true,
            "Type": "x",
        })
    );
    let bare = super::ProblemDetails::from_json(br#"{"title":"t"}"#).expect("an object body");
    assert!(bare.extensions.is_empty());
}

/// `of` reads a typed value through its serialization, so it agrees with `from_json` over the
/// bytes that value would put on the wire.
#[test]
fn a_typed_body_reads_as_its_serialization() {
    let value: serde_json::Value = serde_json::from_slice(PROBLEM).unwrap();
    assert_eq!(
        super::ProblemDetails::of(&value),
        super::ProblemDetails::from_json(PROBLEM)
    );
    assert_eq!(super::ProblemDetails::of("not an object"), None);
    assert_eq!(super::ProblemDetails::of(&Option::<u8>::None), None);
}
