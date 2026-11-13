//! `send`, `prepare` and the body limits a sent response is read under, over a canned backend.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;

use crate::dispatch::{decode_success_bytes, unexpected_status};
use crate::tests::{poll_ready, response, Chunks};
use crate::{ClientCore, Error, ResponseValue};

/// A transport that answers with the responses it was given, in order, and records the headers
/// and the [`Tag`] extension of every request it executes.
#[derive(Debug, Default)]
struct Canned {
    responses: std::sync::Mutex<std::collections::VecDeque<crate::tests::Response>>,
    seen: std::sync::Mutex<Vec<(http::HeaderMap, Option<Tag>)>>,
    /// The [`crate::TotalTimeout`] each request carried, in order.
    timeouts: std::sync::Mutex<Vec<Option<crate::TotalTimeout>>>,
}

/// An extension type a caller's backend reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag(u32);

impl Canned {
    fn answering(responses: impl IntoIterator<Item = crate::tests::Response>) -> Arc<Self> {
        Arc::new(Self {
            responses: std::sync::Mutex::new(responses.into_iter().collect()),
            seen: std::sync::Mutex::default(),
            timeouts: std::sync::Mutex::default(),
        })
    }
}

impl crate::HttpBackend for Canned {
    fn execute(&self, request: http::Request<crate::HttpBody>) -> crate::ExecuteFuture<'_> {
        self.seen.lock().unwrap().push((
            request.headers().clone(),
            request.extensions().get::<Tag>().copied(),
        ));
        self.timeouts
            .lock()
            .unwrap()
            .push(request.extensions().get::<crate::TotalTimeout>().copied());
        let response = self.responses.lock().unwrap().pop_front();
        Box::pin(async move { Ok(response.expect("a canned response per request")) })
    }
}

/// A core over `backend` whose options are `options`.
fn core_with(backend: Arc<Canned>, options: crate::RequestOptions) -> ClientCore {
    let mut core = ClientCore::with_backend(backend, "https://example.com").unwrap();
    core.set_options(options);
    core
}

/// Options limiting every body to `limit` bytes.
fn max_body(limit: usize) -> crate::RequestOptions {
    crate::RequestOptions {
        max_body: Some(limit),
        ..crate::RequestOptions::default()
    }
}

/// `response` with a `Content-Length` of `length`.
fn declaring(mut response: crate::tests::Response, length: usize) -> crate::tests::Response {
    response
        .headers_mut()
        .insert(reqwest::header::CONTENT_LENGTH, length.into());
    response
}

/// Send one request through `core` and read its success body as raw bytes.
fn send_and_read(core: &ClientCore) -> Result<ResponseValue<Bytes>, Error<Infallible>> {
    let response = poll_ready(crate::dispatch::send(core, crate::tests::request()))?;
    poll_ready(decode_success_bytes(core, response))
}

/// Issue #1260: the options' headers reach every request, except a name the request already
/// carries, which keeps exactly its own values; every value of a name it lacks is added.
#[test]
fn send_adds_the_option_headers_the_request_lacks() {
    let backend = Canned::answering([response(200, ""), response(200, "")]);
    let mut options = crate::RequestOptions::default();
    options.headers.insert("x-own", "option".parse().unwrap());
    options.headers.append("x-many", "1".parse().unwrap());
    options.headers.append("x-many", "2".parse().unwrap());
    let core = core_with(backend.clone(), options);
    let request = crate::tests::builder(reqwest::Method::GET, "https://example.com/op")
        .header("x-own", "request")
        .build()
        .unwrap();
    poll_ready(crate::dispatch::send(&core, request)).unwrap();
    poll_ready(crate::dispatch::send_streaming(
        &core,
        crate::tests::request(),
    ))
    .unwrap();
    let seen = backend.seen.lock().unwrap();
    let own: Vec<_> = seen[0].0.get_all("x-own").iter().collect();
    assert_eq!(own, ["request"]);
    for (headers, _) in seen.iter() {
        let many: Vec<_> = headers.get_all("x-many").iter().collect();
        assert_eq!(many, ["1", "2"]);
    }
    assert_eq!(seen[1].0["x-own"], "option");
}

/// Issue #1260: the options' extensions reach the backend on every request, streaming or not.
#[test]
fn send_copies_the_option_extensions_onto_each_request() {
    let backend = Canned::answering([response(200, ""), response(200, ""), response(200, "")]);
    let mut options = crate::RequestOptions::default();
    options.extensions.insert(Tag(7));
    let core = core_with(backend.clone(), options);
    poll_ready(crate::dispatch::send(&core, crate::tests::request())).unwrap();
    poll_ready(crate::dispatch::send_streaming(
        &core,
        crate::tests::request(),
    ))
    .unwrap();
    let plain = core_with(backend.clone(), crate::RequestOptions::default());
    poll_ready(crate::dispatch::send(&plain, crate::tests::request())).unwrap();
    let tags: Vec<_> = backend
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|seen| seen.1)
        .collect();
    assert_eq!(tags, [Some(Tag(7)), Some(Tag(7)), None]);
}

/// A total timeout's precedence: the request's own, then the options' extensions, then
/// `ClientConfig::timeout`, which a streaming send never applies; an options' timeout is an
/// explicit deadline and bounds a streaming send too.
#[test]
fn an_option_total_timeout_wins_over_the_configured_one() {
    use crate::TotalTimeout;
    use std::time::Duration;
    let backend = Canned::answering([
        response(200, ""),
        response(200, ""),
        response(200, ""),
        response(200, ""),
        response(200, ""),
    ]);
    let mut options = crate::RequestOptions::default();
    options
        .extensions
        .insert(TotalTimeout(Duration::from_secs(2)));
    let mut core = core_with(backend.clone(), options);
    core.config_mut().timeout = Some(Duration::from_secs(30));
    poll_ready(crate::dispatch::send(&core, crate::tests::request())).unwrap();
    let mut own = crate::tests::request();
    own.extensions_mut()
        .insert(TotalTimeout(Duration::from_secs(5)));
    poll_ready(crate::dispatch::send(&core, own)).unwrap();
    poll_ready(crate::dispatch::send_streaming(
        &core,
        crate::tests::request(),
    ))
    .unwrap();
    let mut plain = core_with(backend.clone(), crate::RequestOptions::default());
    plain.config_mut().timeout = Some(Duration::from_secs(30));
    poll_ready(crate::dispatch::send(&plain, crate::tests::request())).unwrap();
    poll_ready(crate::dispatch::send_streaming(
        &plain,
        crate::tests::request(),
    ))
    .unwrap();
    let seconds = |secs| Some(TotalTimeout(Duration::from_secs(secs)));
    assert_eq!(
        *backend.timeouts.lock().unwrap(),
        [seconds(2), seconds(5), seconds(2), seconds(30), None]
    );
}

/// An extension of a type the request already carries keeps the request's own value.
#[test]
fn a_request_keeps_its_own_extension_over_the_options() {
    let mut options = crate::RequestOptions::default();
    options.extensions.insert(Tag(1));
    let mut request = crate::tests::seam_request();
    request.extensions_mut().insert(Tag(2));
    crate::dispatch::apply_options(&options, &mut request);
    assert_eq!(request.extensions().get::<Tag>(), Some(&Tag(2)));
}

/// Issue #1260: a success body within `max_body` decodes, the limit itself included.
#[test]
fn a_success_body_within_max_body_is_read_whole() {
    let backend = Canned::answering([declaring(Chunks::repeat(4, 3).into_response(200), 12)]);
    let core = core_with(backend, max_body(12));
    assert_eq!(send_and_read(&core).unwrap().into_inner().len(), 12);
}

/// Issue #1260: a `Content-Length` past `max_body` is refused before any of the body is read.
#[test]
fn a_declared_length_past_max_body_is_refused_unread() {
    let chunks = Chunks::repeat(4, 3);
    let pulled = chunks.pulled();
    let backend = Canned::answering([declaring(chunks.into_response(200), 12)]);
    let core = core_with(backend, max_body(11));
    let error = send_and_read(&core).unwrap_err();
    assert!(
        matches!(error, Error::BodyTooLarge { limit: 11, status } if status == 200),
        "{error:?}"
    );
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// Issue #1260: a body with no declared length is abandoned at the chunk that carries it past
/// `max_body`, and nothing after it is read.
#[test]
fn an_undeclared_body_is_abandoned_at_the_chunk_crossing_max_body() {
    let chunks = Chunks::repeat(4, 10);
    let pulled = chunks.pulled();
    let backend = Canned::answering([chunks.into_response(201)]);
    let core = core_with(backend, max_body(10));
    let error = send_and_read(&core).unwrap_err();
    assert!(
        matches!(error, Error::BodyTooLarge { limit: 10, status } if status == 201),
        "{error:?}"
    );
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 3);
}

/// `ClientConfig::max_success_body` bounds a success body when the options set no limit, and the
/// options' `max_body` replaces it, looser or tighter.
#[test]
fn max_body_replaces_the_configured_success_limit() {
    let configured = |options| {
        let backend = Canned::answering([Chunks::repeat(4, 2).into_response(200)]);
        let mut core = core_with(backend, options);
        core.config_mut().max_success_body = Some(4);
        send_and_read(&core).map(|value| value.into_inner().len())
    };
    assert!(matches!(
        configured(crate::RequestOptions::default()),
        Err(Error::BodyTooLarge { limit: 4, .. })
    ));
    assert_eq!(configured(max_body(8)).unwrap(), 8);
    assert!(matches!(
        configured(max_body(7)),
        Err(Error::BodyTooLarge { limit: 7, .. })
    ));
}

/// Issue #1264: `with_body_limits` gives a response a caller obtained on its own the limit the
/// core sets, as a send would, and leaves a response a send already limited as it was.
#[test]
fn with_body_limits_limits_a_bare_response_and_keeps_a_sent_ones() {
    let read = |core: &ClientCore, response| {
        poll_ready(decode_success_bytes(core, response)).map(|value| value.into_inner().len())
    };
    let limited = core_with(Canned::answering([]), max_body(4));
    let bare = Chunks::repeat(4, 2).into_response(200);
    assert!(matches!(
        read(&limited, crate::dispatch::with_body_limits(&limited, bare)),
        Err(Error::BodyTooLarge { limit: 4, .. })
    ));

    let backend = Canned::answering([Chunks::repeat(4, 2).into_response(200)]);
    let sender = core_with(backend, max_body(4));
    let sent = poll_ready(crate::dispatch::send(&sender, crate::tests::request())).unwrap();
    let unlimited = core_with(Canned::answering([]), crate::RequestOptions::default());
    assert!(matches!(
        read(
            &unlimited,
            crate::dispatch::with_body_limits(&unlimited, sent)
        ),
        Err(Error::BodyTooLarge { limit: 4, .. })
    ));
    let bare = Chunks::repeat(4, 2).into_response(200);
    assert_eq!(
        read(
            &unlimited,
            crate::dispatch::with_body_limits(&unlimited, bare)
        )
        .unwrap(),
        8
    );
}

/// Issue #1264: `prepare` builds, unsent, the request a send hands the backend, the options'
/// headers and extensions on it, streaming or not; `send` and `send_streaming` are each `prepare`
/// then `execute`, so their own tests pin the total timeout it gives a non-streaming request.
#[test]
fn prepare_adds_the_options_without_sending() {
    let backend = Canned::answering([]);
    let mut options = crate::RequestOptions::default();
    options.headers.insert("x-option", "set".parse().unwrap());
    options.extensions.insert(Tag(3));
    let core = core_with(backend.clone(), options);
    for streaming in [false, true] {
        let request = crate::dispatch::prepare(&core, crate::tests::request(), streaming);
        assert_eq!(
            request.headers()["x-option"],
            "set",
            "streaming: {streaming}"
        );
        assert_eq!(request.extensions().get::<Tag>(), Some(&Tag(3)));
    }
    assert!(backend.seen.lock().unwrap().is_empty());
}

/// With no limit set anywhere, a success body within the binary default is read whole, as before.
#[test]
fn a_success_body_is_read_whole_within_the_binary_default() {
    let backend = Canned::answering([Chunks::repeat(1024, 64).into_response(200)]);
    let core = core_with(backend, crate::RequestOptions::default());
    assert_eq!(send_and_read(&core).unwrap().into_inner().len(), 64 * 1024);
}

/// Issue #1263: with no limit set anywhere, a raw binary success body is read within
/// `ClientConfig::DEFAULT_MAX_BINARY_BODY`, a `Content-Length` past it refused before any of the
/// body is read; a structured success body under the same declaration is read whole.
#[test]
fn a_binary_success_body_is_bounded_by_the_binary_default() {
    let default = crate::ClientConfig::DEFAULT_MAX_BINARY_BODY;
    assert_eq!(default, 64 * 1024 * 1024);
    let chunks = Chunks::repeat(4, 2);
    let pulled = chunks.pulled();
    let backend = Canned::answering([declaring(chunks.into_response(200), default + 1)]);
    let core = core_with(backend, crate::RequestOptions::default());
    let error = send_and_read(&core).unwrap_err();
    assert!(
        matches!(error, Error::BodyTooLarge { limit, status } if limit == default && status == 200),
        "{error:?}"
    );
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 0);

    let backend = Canned::answering([declaring(
        Chunks::repeat(4, 2).into_response(200),
        default + 1,
    )]);
    let core = core_with(backend, crate::RequestOptions::default());
    let response = poll_ready(crate::dispatch::send(&core, crate::tests::request())).unwrap();
    let structured = poll_ready(crate::dispatch::decode_success_with(
        &core,
        response,
        |body| Ok(body.len()),
    ));
    assert_eq!(structured.unwrap().into_inner(), 8);
}

/// Issue #1263: a limit the call is made under replaces the binary default, larger or smaller.
#[test]
fn a_set_limit_replaces_the_binary_default() {
    let default = crate::ClientConfig::DEFAULT_MAX_BINARY_BODY;
    let read = |options, configured: Option<usize>| {
        let response = declaring(Chunks::repeat(4, 2).into_response(200), default + 1);
        let mut core = core_with(Canned::answering([response]), options);
        core.config_mut().max_success_body = configured;
        send_and_read(&core).map(|value| value.into_inner().len())
    };
    // Larger: the declared length is within it, so the body is read.
    assert_eq!(read(max_body(default + 1), None).unwrap(), 8);
    assert_eq!(
        read(crate::RequestOptions::default(), Some(default + 1)).unwrap(),
        8
    );
    // Smaller: the 8 bytes cross it as they are read.
    let backend = Canned::answering([Chunks::repeat(4, 2).into_response(200)]);
    let error = send_and_read(&core_with(backend, max_body(5))).unwrap_err();
    assert!(
        matches!(error, Error::BodyTooLarge { limit: 5, .. }),
        "{error:?}"
    );
}

/// A streaming operation's response carries no limit: `send_streaming` leaves its bodies to
/// `max_stream_frame` and `max_error_body`.
#[test]
fn a_streaming_send_applies_no_body_limit() {
    let backend = Canned::answering([
        Chunks::repeat(4, 4).into_response(200),
        Chunks::repeat(4, 4).into_response(500),
    ]);
    let mut core = core_with(backend, max_body(2));
    core.config_mut().max_success_body = Some(2);
    let response = poll_ready(crate::dispatch::send_streaming(
        &core,
        crate::tests::request(),
    ))
    .unwrap();
    let read = poll_ready(decode_success_bytes(&core, response)).unwrap();
    assert_eq!(read.into_inner().len(), 16);
    let response = poll_ready(crate::dispatch::send_streaming(
        &core,
        crate::tests::request(),
    ))
    .unwrap();
    let error = poll_ready(unexpected_status::<Infallible>(&core, response));
    assert!(
        matches!(&error, Error::UnexpectedStatus { body, .. } if body.len() == 16),
        "{error:?}"
    );
}

/// Issue #1260: an error body past a `max_body` below `max_error_body` is refused, declared or
/// grown; at or above `max_error_body`, the error body is truncated to the cap as before.
#[test]
fn max_body_below_the_error_cap_refuses_an_oversized_error_body() {
    let error_for = |response, limit| {
        let backend = Canned::answering([response]);
        let mut core = core_with(backend, max_body(limit));
        core.config_mut().max_error_body = 8;
        let response = poll_ready(crate::dispatch::send(&core, crate::tests::request())).unwrap();
        poll_ready(unexpected_status::<Infallible>(&core, response))
    };
    let grown = error_for(Chunks::repeat(4, 4).into_response(503), 6);
    assert!(
        matches!(grown, Error::BodyTooLarge { limit: 6, status } if status == 503),
        "{grown:?}"
    );
    let declared = error_for(declaring(Chunks::repeat(4, 4).into_response(404), 16), 7);
    assert!(
        matches!(declared, Error::BodyTooLarge { limit: 7, status } if status == 404),
        "{declared:?}"
    );
    let within = error_for(Chunks::repeat(2, 3).into_response(500), 6);
    assert!(
        matches!(&within, Error::UnexpectedStatus { body, .. } if body.len() == 6),
        "{within:?}"
    );
    let truncated = error_for(Chunks::repeat(4, 4).into_response(500), 8);
    assert!(
        matches!(&truncated, Error::UnexpectedStatus { body, .. } if &body[..] == b"xxxxxxxx"),
        "{truncated:?}"
    );
}

/// `declares_more_than` reads only a well-formed `Content-Length`: a missing, malformed or
/// in-limit one declares nothing past the limit.
#[test]
fn only_a_well_formed_content_length_declares_a_body_past_the_limit() {
    let with = |value: &str| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::CONTENT_LENGTH, value.parse().unwrap());
        headers
    };
    assert!(crate::dispatch::declares_more_than(&with("11"), 10));
    assert!(!crate::dispatch::declares_more_than(&with("10"), 10));
    assert!(!crate::dispatch::declares_more_than(&with("ten"), 10));
    assert!(!crate::dispatch::declares_more_than(&with("-1"), 0));
    assert!(!crate::dispatch::declares_more_than(
        &reqwest::header::HeaderMap::new(),
        0
    ));
}

/// A `HEAD https://example.com/op` as the transport seam carries it.
fn head_request() -> http::Request<crate::HttpBody> {
    crate::tests::builder(reqwest::Method::HEAD, "https://example.com/op")
        .build()
        .expect("build request")
}

/// Issue #1308: the answer to a `HEAD` request, and a `1xx`, `204` or `304` response, carries no
/// body whatever its `Content-Length` says, so a length past `max_body` is not refused: the
/// operation's status error classifies it.
#[test]
fn a_bodiless_response_declaring_a_length_past_max_body_classifies_as_its_status() {
    for (head, status) in [
        (true, 404),
        (true, 200),
        (false, 304),
        (false, 204),
        (false, 103),
    ] {
        let backend = Canned::answering([declaring(response(status, ""), 5000)]);
        let core = core_with(backend, max_body(1024));
        let request = if head {
            head_request()
        } else {
            crate::tests::request()
        };
        let sent = poll_ready(crate::dispatch::send(&core, request)).unwrap();
        let error = poll_ready(unexpected_status::<Infallible>(&core, sent));
        assert!(
            matches!(&error, Error::UnexpectedStatus { status: got, body, .. }
                if *got == status && body.is_empty()),
            "HEAD: {head}, {status}: {error:?}"
        );
    }
}

/// Issue #1308: a `HEAD` request's raw binary success declaring more than the binary default is
/// read, empty, rather than refused; a `204` success past `max_body` is read the same way.
#[test]
fn a_bodiless_success_declaring_a_length_past_its_limit_decodes() {
    let default = crate::ClientConfig::DEFAULT_MAX_BINARY_BODY;
    let backend = Canned::answering([declaring(response(200, ""), default + 1)]);
    let core = core_with(backend, crate::RequestOptions::default());
    let sent = poll_ready(crate::dispatch::send(&core, head_request())).unwrap();
    let read = poll_ready(decode_success_bytes(&core, sent)).unwrap();
    assert!(read.into_inner().is_empty());

    let backend = Canned::answering([declaring(response(204, ""), 5000)]);
    let core = core_with(backend, max_body(1024));
    let read = send_and_read(&core).unwrap();
    assert_eq!((read.status().as_u16(), read.inner().len()), (204, 0));
}

/// Issue #1308: a bodiless response whose body does carry bytes is still bounded as it is read,
/// abandoned at the chunk that carries it past `max_body`.
#[test]
fn a_bodiless_response_that_carries_a_body_is_bounded_as_it_is_read() {
    let chunks = Chunks::repeat(4, 4);
    let pulled = chunks.pulled();
    let backend = Canned::answering([declaring(chunks.into_response(304), 16)]);
    let core = core_with(backend, max_body(6));
    let sent = poll_ready(crate::dispatch::send(&core, crate::tests::request())).unwrap();
    let error = poll_ready(unexpected_status::<Infallible>(&core, sent));
    assert!(
        matches!(error, Error::BodyTooLarge { limit: 6, status } if status == 304),
        "{error:?}"
    );
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 2);
}
