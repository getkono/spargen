use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use reqwest::{Method, Request};

use super::{
    next_frame, EventStream, FramePoll, Framing, ReconnectPolicy, ReconnectReason, ReconnectWait,
    SseMetadata,
};
use crate::{ClientCore, Error, ExecuteFuture, HttpBackend};

/// Frame every item currently extractable from `buffer` under `framing`, stopping at the first
/// `NeedMore` (partial frame retained) or `Done` (terminated). Returns the framed payloads as
/// strings plus whether a `Done` terminator was hit.
fn drain(buffer: &mut Vec<u8>, framing: Framing, at_eof: bool) -> (Vec<String>, bool) {
    let mut items = Vec::new();
    loop {
        match next_frame(buffer, framing, at_eof) {
            FramePoll::Item { payload, .. } => {
                items.push(String::from_utf8(payload).unwrap());
            }
            FramePoll::Metadata(_) => continue,
            FramePoll::Done(_) => return (items, true),
            FramePoll::NeedMore => return (items, false),
        }
    }
}

#[test]
fn ndjson_frames_complete_lines_and_skips_blanks() {
    let mut buf = b"{\"a\":1}\n\n{\"a\":2}\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec![r#"{"a":1}"#, r#"{"a":2}"#]);
    assert!(!done);
    assert!(buf.is_empty());
}

#[test]
fn ndjson_retains_a_partial_line_across_chunks() {
    // A line split across two chunks: the tail `{"a":` is retained until the rest arrives.
    let mut buf = b"{\"a\":1}\n{\"a\":".to_vec();
    let (items, _) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec![r#"{"a":1}"#]);
    // The incomplete second line stays buffered (no newline yet), not emitted.
    assert_eq!(buf, b"{\"a\":");

    buf.extend_from_slice(b"2}\n");
    let (items, _) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec![r#"{"a":2}"#]);
    assert!(buf.is_empty());
}

#[test]
fn ndjson_emits_trailing_line_without_newline_at_eof() {
    let mut buf = b"{\"a\":1}\n{\"a\":2}".to_vec();
    // Not at EOF: only the newline-terminated line frames; the tail is retained.
    let (items, _) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec![r#"{"a":1}"#]);
    assert_eq!(buf, b"{\"a\":2}");
    // At EOF: the trailing complete line without a newline is flushed.
    let (items, _) = drain(&mut buf, Framing::Ndjson, true);
    assert_eq!(items, vec![r#"{"a":2}"#]);
    assert!(buf.is_empty());
}

#[test]
fn ndjson_tolerates_crlf() {
    let mut buf = b"{\"a\":1}\r\n{\"a\":2}\r\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec![r#"{"a":1}"#, r#"{"a":2}"#]);
}

#[test]
fn ndjson_strips_one_trailing_cr_from_an_unterminated_line_at_eof() {
    let mut buf = b"{\"a\":1}\r".to_vec();
    let (items, _) = drain(&mut buf, Framing::Ndjson, true);
    assert_eq!(items, vec![r#"{"a":1}"#]);
    assert!(buf.is_empty());

    // Exactly one `\r` is stripped; a second one stays in the payload.
    let mut buf = b"{\"a\":1}\r\r".to_vec();
    let (items, _) = drain(&mut buf, Framing::Ndjson, true);
    assert_eq!(items, vec!["{\"a\":1}\r"]);
}

#[test]
fn json_sequence_frames_record_separator_delimited_values() {
    let mut buf = b"\x1e{\"a\":1}\n\x1e{\n  \"a\": 2\n}\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::JsonSequence, true);
    assert_eq!(items, vec![r#"{"a":1}"#, "{\n  \"a\": 2\n}"]);
    assert!(!done);
    assert!(buf.is_empty());
}

#[test]
fn sse_concatenates_multiple_data_lines_and_ignores_other_fields() {
    // Two `data:` lines join with a newline; the `event:`/`id:` fields and the `:` comment are
    // ignored. The blank line terminates the event.
    let mut buf = b": keep-alive\nevent: message\nid: 7\ndata: {\"a\":\ndata: 1}\n\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::Sse, false);
    assert_eq!(items, vec!["{\"a\":\n1}"]);
    assert!(!done);
    assert!(buf.is_empty());
}

#[test]
fn oas32_sse_frames_the_parsed_event_as_a_json_object() {
    let mut buf =
        b": ignored\nevent: add\nid: 7\nretry: 5\ndata: first\ndata: second\nunknown: ignored\n\n"
            .to_vec();
    let (items, done) = drain(&mut buf, Framing::SseEvent, false);
    assert_eq!(items.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&items[0]).unwrap();
    assert_eq!(
        event,
        serde_json::json!({
            "data": "first\nsecond",
            "event": "add",
            "id": "7",
            "retry": 5
        })
    );
    assert!(!done);
}

#[test]
fn oas32_sse_does_not_treat_done_data_as_a_private_sentinel() {
    let mut buf = b"data: [DONE]\n\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::SseEvent, false);
    assert_eq!(items.len(), 1);
    assert!(!done);
}

#[test]
fn oas32_sse_json_data_yields_payload_and_retains_metadata() {
    let mut buf = b"id: event-7\nretry: 1250\ndata: {\"kind\":\"ready\"}\n\n".to_vec();
    let frame = next_frame(&mut buf, Framing::SseJsonData, false);
    let FramePoll::Item { payload, metadata } = frame else {
        panic!("expected one dispatched SSE event");
    };
    assert_eq!(payload, br#"{"kind":"ready"}"#);
    assert_eq!(metadata.id.as_deref(), Some("event-7"));
    assert_eq!(metadata.retry, Some(1_250));
}

#[test]
fn sse_metadata_only_event_is_observable_to_the_stream() {
    let mut stream: EventStream<serde_json::Value> = EventStream::new(
        response("id: checkpoint\nretry: 2500\n\ndata: {\"ok\":true}\n\n"),
        Framing::SseJsonData,
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"ok": true})
    );
    assert_eq!(stream.last_event_id(), Some("checkpoint"));
    assert_eq!(
        stream.reconnect_delay(),
        Some(std::time::Duration::from_millis(2_500))
    );
}

#[test]
fn sse_strips_only_one_leading_space_after_colon() {
    let mut buf = b"data:  two-spaces\n\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    // One space is stripped; the second is preserved as payload.
    assert_eq!(items, vec![" two-spaces"]);
}

#[test]
fn sse_retains_a_partial_event_until_the_blank_line() {
    // No blank-line terminator yet: nothing frames and the bytes are retained verbatim.
    let mut buf = b"data: {\"a\":1}\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    assert!(items.is_empty());
    assert_eq!(buf, b"data: {\"a\":1}\n");
    // The blank line arrives in the next chunk; now the event frames.
    buf.extend_from_slice(b"\n");
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    assert_eq!(items, vec![r#"{"a":1}"#]);
    assert!(buf.is_empty());
}

#[test]
fn sse_done_sentinel_terminates_the_stream() {
    let mut buf = b"data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"a\":2}\n\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::Sse, false);
    // The item before `[DONE]` is delivered; `[DONE]` ends the stream, so the later event is
    // never reached.
    assert_eq!(items, vec![r#"{"a":1}"#]);
    assert!(done);
}

#[test]
fn sse_tolerates_crlf_terminators() {
    let mut buf = b"data: {\"a\":1}\r\n\r\ndata: {\"a\":2}\r\n\r\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    assert_eq!(items, vec![r#"{"a":1}"#, r#"{"a":2}"#]);
}

#[test]
fn sse_flushes_a_final_event_without_a_trailing_blank_line_at_eof() {
    let mut buf = b"data: {\"a\":1}".to_vec();
    // Not at EOF: the unterminated event is retained.
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    assert!(items.is_empty());
    // At EOF: the final event is flushed even without a closing blank line.
    let (items, _) = drain(&mut buf, Framing::Sse, true);
    assert_eq!(items, vec![r#"{"a":1}"#]);
}

#[test]
fn sse_strips_the_trailing_cr_from_an_unterminated_last_line_at_eof() {
    let mut buf = b"data: {\"a\":1}\r".to_vec();
    let (items, _) = drain(&mut buf, Framing::Sse, true);
    assert_eq!(items, vec![r#"{"a":1}"#]);
    assert!(buf.is_empty());
}

#[test]
fn malformed_json_frame_surfaces_as_a_decode_error() {
    // Framing yields the raw bytes; the async `next` deserializes them. Deserialize the framed
    // payload the same way `next` does and assert a malformed frame is a `Decode` error, not a
    // silent skip.
    let mut buf = b"not json\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Ndjson, false);
    assert_eq!(items, vec!["not json"]);
    let decoded: Result<serde_json::Value, Error<std::convert::Infallible>> =
        super::deserialize_item(
            reqwest::StatusCode::OK,
            &reqwest::header::HeaderMap::new(),
            items[0].as_bytes(),
        );
    assert!(matches!(
        decoded,
        Err(Error::Decode {
            status: reqwest::StatusCode::OK,
            ..
        })
    ));
}

#[test]
fn well_formed_json_frame_deserializes() {
    let decoded: Result<serde_json::Value, Error<std::convert::Infallible>> =
        super::deserialize_item(
            reqwest::StatusCode::OK,
            &reqwest::header::HeaderMap::new(),
            br#"{"a":1}"#,
        );
    assert_eq!(decoded.unwrap(), serde_json::json!({"a": 1}));
}

// The tests below drive the async `next` await-loop poll-once (noop waker, no async runtime),
// proving it threads the pure framing correctly over an in-memory `reqwest::Response`.
use crate::tests::poll_ready;

fn response(body: &str) -> reqwest::Response {
    response_with_status(200, body)
}

/// Every synthetic initial response carries `x-response: initial`, and every response a
/// [`SequenceBackend`] returns carries `x-response: reconnected`, so a test can tell which
/// response's headers a `Decode` error reports.
fn response_with_status(status: u16, body: &str) -> reqwest::Response {
    reqwest::Response::from(
        http::Response::builder()
            .status(status)
            .header("x-response", "initial")
            .body(body.to_owned())
            .expect("valid synthetic response"),
    )
}

#[derive(Debug)]
struct SequenceBackend {
    responses: Mutex<VecDeque<(u16, String)>>,
    headers: Mutex<Vec<(Option<String>, Option<String>)>>,
}

impl SequenceBackend {
    fn new(responses: impl IntoIterator<Item = (u16, &'static str)>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|(status, body)| (status, body.to_owned()))
                    .collect(),
            ),
            headers: Mutex::new(Vec::new()),
        }
    }
}

impl HttpBackend for SequenceBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        let last_event_id = request
            .headers()
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let cookie = request
            .headers()
            .get(reqwest::header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        self.headers.lock().unwrap().push((last_event_id, cookie));
        let (status, body) = self.responses.lock().unwrap().pop_front().unwrap();
        Box::pin(async move {
            Ok(reqwest::Response::from(
                http::Response::builder()
                    .status(status)
                    .header("x-response", "reconnected")
                    .body(body)
                    .expect("valid synthetic response"),
            ))
        })
    }
}

#[derive(Default)]
struct ImmediateReconnect {
    max_attempts: u32,
    seen: Mutex<Vec<(u32, &'static str, Option<std::time::Duration>)>>,
}

impl ReconnectPolicy for ImmediateReconnect {
    fn reconnect(
        &self,
        attempt: u32,
        reason: ReconnectReason<'_>,
        server_delay: Option<std::time::Duration>,
    ) -> Option<ReconnectWait> {
        let reason = match reason {
            ReconnectReason::EndOfStream => "eof",
            ReconnectReason::Failure(_) => "failure",
        };
        self.seen
            .lock()
            .unwrap()
            .push((attempt, reason, server_delay));
        (attempt < self.max_attempts).then(|| Box::pin(async {}) as ReconnectWait)
    }
}

fn reconnectable_stream(
    initial_body: &str,
    backend: Arc<SequenceBackend>,
    policy: Arc<ImmediateReconnect>,
) -> EventStream<serde_json::Value> {
    let core = ClientCore::with_backend(backend, "https://example.com").unwrap();
    let request = core
        .http()
        .request(Method::GET, "https://example.com/events")
        .header(reqwest::header::COOKIE, "session=secret")
        .build()
        .unwrap();
    let stream = EventStream::new_reconnectable(
        response(initial_body),
        Framing::SseJsonData,
        core,
        Some(request),
    );
    match stream.with_reconnect(policy) {
        Ok(stream) => stream,
        Err(error) => panic!("reconnect should be available: {error}"),
    }
}

#[test]
fn reconnect_replays_last_event_id_and_preserves_cookie_headers() {
    let backend = Arc::new(SequenceBackend::new([(200, "data: {\"seq\":2}\n\n")]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream(
        "id: evt-1\nretry: 750\ndata: {\"seq\":1}\n\n",
        backend.clone(),
        policy.clone(),
    );

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(
        backend.headers.lock().unwrap().as_slice(),
        &[(Some("evt-1".to_owned()), Some("session=secret".to_owned()))]
    );
    assert_eq!(
        policy.seen.lock().unwrap().as_slice(),
        &[(0, "eof", Some(std::time::Duration::from_millis(750)))]
    );
}

/// An SSE `id` holding a control character other than NUL is accepted by the framer but cannot be
/// sent as a `Last-Event-ID` header value. The reconnect still happens, without the header, so
/// neither that id nor a `Last-Event-ID` the caller set on the original request reaches the
/// server; `last_event_id` keeps reporting the id the server sent.
#[test]
fn an_unsendable_sse_id_reconnects_without_last_event_id() {
    let backend = Arc::new(SequenceBackend::new([(200, "data: {\"seq\":2}\n\n")]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let request = core
        .http()
        .request(Method::GET, "https://example.com/events")
        .header("last-event-id", "evt-0")
        .header(reqwest::header::COOKIE, "session=secret")
        .build()
        .unwrap();
    let stream: EventStream<serde_json::Value> = EventStream::new_reconnectable(
        response("id: a\u{1}b\ndata: {\"seq\":1}\n\n"),
        Framing::SseJsonData,
        core,
        Some(request),
    );
    let mut stream = match stream.with_reconnect(policy.clone()) {
        Ok(stream) => stream,
        Err(error) => panic!("reconnect should be available: {error}"),
    };

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(stream.last_event_id(), Some("a\u{1}b"));
    match poll_ready(stream.next()) {
        Some(Ok(item)) => assert_eq!(item, serde_json::json!({"seq": 2})),
        other => panic!("expected the reconnected item, got {other:?}"),
    }
    assert_eq!(
        backend.headers.lock().unwrap().as_slice(),
        &[(None, Some("session=secret".to_owned()))]
    );
    assert_eq!(policy.seen.lock().unwrap().as_slice(), &[(0, "eof", None)]);
}

/// An empty `id:` resets replay state, so the reconnect sends no `Last-Event-ID`: neither an
/// empty header, nor the server's earlier id, nor the one the original request carried.
#[test]
fn an_empty_sse_id_reconnects_without_last_event_id() {
    let backend = Arc::new(SequenceBackend::new([(200, "data: {\"seq\":3}\n\n")]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let request = core
        .http()
        .request(Method::GET, "https://example.com/events")
        .header("last-event-id", "evt-0")
        .header(reqwest::header::COOKIE, "session=secret")
        .build()
        .unwrap();
    let stream: EventStream<serde_json::Value> = EventStream::new_reconnectable(
        response("id: evt-1\ndata: {\"seq\":1}\n\nid:\ndata: {\"seq\":2}\n\n"),
        Framing::SseJsonData,
        core,
        Some(request),
    );
    let mut stream = match stream.with_reconnect(policy.clone()) {
        Ok(stream) => stream,
        Err(error) => panic!("reconnect should be available: {error}"),
    };

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(stream.last_event_id(), Some("evt-1"));
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(stream.last_event_id(), Some(""));
    match poll_ready(stream.next()) {
        Some(Ok(item)) => assert_eq!(item, serde_json::json!({"seq": 3})),
        other => panic!("expected the reconnected item, got {other:?}"),
    }
    assert_eq!(
        backend.headers.lock().unwrap().as_slice(),
        &[(None, Some("session=secret".to_owned()))]
    );
    assert_eq!(policy.seen.lock().unwrap().as_slice(), &[(0, "eof", None)]);
}

#[test]
fn declined_reconnect_failure_preserves_the_original_error_variant() {
    let backend = Arc::new(SequenceBackend::new([(503, "unavailable")]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream("", backend, policy.clone());

    let error = poll_ready(stream.next()).unwrap().unwrap_err();
    assert!(matches!(error, Error::UnexpectedStatus { .. }));
    assert_eq!(
        policy
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, reason, _)| *reason)
            .collect::<Vec<_>>(),
        vec!["eof", "failure"]
    );
}

#[test]
fn next_drives_framing_over_an_in_memory_response() {
    let mut stream: EventStream<serde_json::Value> =
        EventStream::new(response("{\"a\":1}\n{\"a\":2}\n"), Framing::Ndjson);
    let first = poll_ready(stream.next()).unwrap().unwrap();
    assert_eq!(first, serde_json::json!({"a": 1}));
    let second = poll_ready(stream.next()).unwrap().unwrap();
    assert_eq!(second, serde_json::json!({"a": 2}));
    // End of the in-memory body: the stream is exhausted.
    assert!(poll_ready(stream.next()).is_none());
}

/// A malformed frame is a `Decode` error that carries the status and headers of the response
/// the stream is reading — a non-`200` here, so a hard-coded status cannot pass.
#[test]
fn next_yields_a_decode_error_for_a_malformed_item() {
    let mut stream: EventStream<serde_json::Value> =
        EventStream::new(response_with_status(203, "not json\n"), Framing::Ndjson);
    let item = poll_ready(stream.next()).unwrap();
    match item {
        Err(error @ Error::Decode { .. }) => {
            assert_eq!(
                error.status(),
                Some(reqwest::StatusCode::from_u16(203).unwrap())
            );
            let Error::Decode { headers, .. } = error else {
                unreachable!()
            };
            assert_eq!(headers.get("x-response").unwrap(), "initial");
        }
        other => panic!("expected a Decode error, got {other:?}"),
    }
}

/// After a reconnect, a malformed frame reports the reconnected response's status and headers,
/// not the initial one's: the frame was read from the new body.
#[test]
fn a_decode_error_after_a_reconnect_reports_the_reconnected_status_and_headers() {
    let backend = Arc::new(SequenceBackend::new([(203, "data: not json\n\n")]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream("data: {\"seq\":1}\n\n", backend, policy);

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    match poll_ready(stream.next()).unwrap() {
        Err(Error::Decode {
            status, headers, ..
        }) => {
            assert_eq!(status.as_u16(), 203);
            assert_eq!(headers.get("x-response").unwrap(), "reconnected");
        }
        other => panic!("expected a Decode error from the reconnected body, got {other:?}"),
    }
}

#[test]
fn next_resumes_after_a_decode_error() {
    // A single malformed frame surfaces as a Decode error but must NOT abandon the rest of the
    // stream: the following well-formed items are still yielded (documented contract).
    let mut stream: EventStream<serde_json::Value> =
        EventStream::new(response("not json\n{\"a\":1}\n"), Framing::Ndjson);
    assert!(matches!(
        poll_ready(stream.next()).unwrap(),
        Err(Error::Decode { .. })
    ));
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"a": 1})
    );
    assert!(poll_ready(stream.next()).is_none());
}

/// The reconnect `attempt` a policy sees resets after a successfully decoded item, and only
/// then: a frame that fails to decode leaves the count where it was.
#[test]
fn the_reconnect_attempt_resets_after_a_decoded_item_and_not_after_a_decode_error() {
    let backend = Arc::new(SequenceBackend::new([
        (200, "data: {\"seq\":2}\n\n"),
        (200, "data: not json\n\n"),
        (200, ""),
    ]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 2,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream("data: {\"seq\":1}\n\n", backend, policy.clone());

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    // EOF at attempt 0 reconnects; the decoded item resets the count to 0.
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    // EOF at attempt 0 again reconnects; the undecodable frame does not reset the count.
    assert!(matches!(
        poll_ready(stream.next()).unwrap(),
        Err(Error::Decode { .. })
    ));
    // EOF at attempt 1 reconnects to an empty body; EOF at attempt 2 is declined.
    assert!(poll_ready(stream.next()).is_none());
    assert_eq!(
        policy
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|(attempt, reason, _)| (*attempt, *reason))
            .collect::<Vec<_>>(),
        vec![(0, "eof"), (0, "eof"), (1, "eof"), (2, "eof")]
    );
}

/// A stream built without a replayable request refuses a reconnect policy rather than
/// accepting one it could never act on.
#[test]
fn with_reconnect_refuses_a_stream_that_holds_no_replayable_request() {
    let policy: Arc<dyn ReconnectPolicy> = Arc::new(ImmediateReconnect::default());
    let core = ClientCore::new("https://example.com").unwrap();
    for stream in [
        EventStream::<serde_json::Value>::new(response(""), Framing::Sse),
        EventStream::new_reconnectable(response(""), Framing::Sse, core, None),
    ] {
        match stream.with_reconnect(policy.clone()) {
            Err(Error::RequestConstruction(cause)) => assert_eq!(
                cause.to_string(),
                "this streaming request cannot be cloned safely for automatic reconnect"
            ),
            Err(error) => panic!("expected RequestConstruction, got {error}"),
            Ok(_) => panic!("a stream without a request accepted a reconnect policy"),
        }
    }
}

/// A request whose body is a one-shot stream cannot be cloned for the replay. Once the policy's
/// wait elapses, the stream yields that as `RequestConstruction`, sends nothing, and ends.
#[test]
fn a_reconnect_whose_request_cannot_be_cloned_fails_once_and_ends_the_stream() {
    let backend = Arc::new(SequenceBackend::new([]));
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 5,
        ..ImmediateReconnect::default()
    });
    let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let request = core
        .http()
        .request(Method::POST, "https://example.com/events")
        .body(reqwest::Body::wrap(reqwest::Body::from("one-shot")))
        .build()
        .unwrap();
    assert!(request.try_clone().is_none());
    let stream: EventStream<serde_json::Value> =
        EventStream::new_reconnectable(response(""), Framing::SseJsonData, core, Some(request));
    let mut stream = match stream.with_reconnect(policy.clone()) {
        Ok(stream) => stream,
        Err(error) => panic!("the stream holds a request, so reconnect is available: {error}"),
    };

    match poll_ready(stream.next()) {
        Some(Err(Error::RequestConstruction(cause))) => assert_eq!(
            cause.to_string(),
            "streaming request could not be cloned for reconnect"
        ),
        other => panic!("expected RequestConstruction, got {other:?}"),
    }
    assert!(poll_ready(stream.next()).is_none());
    assert!(backend.headers.lock().unwrap().is_empty());
    assert_eq!(policy.seen.lock().unwrap().as_slice(), &[(0, "eof", None)]);
}

/// An `id` field holding a NUL byte is ignored, as the SSE specification requires, so the
/// stream keeps the last ID it accepted.
#[test]
fn an_sse_id_holding_nul_is_ignored() {
    let mut buf = b"id: a\0b\ndata: 1\n\n".to_vec();
    assert_eq!(
        next_frame(&mut buf, Framing::SseJsonData, false),
        FramePoll::Item {
            payload: b"1".to_vec(),
            metadata: SseMetadata::default(),
        }
    );
    let mut stream: EventStream<serde_json::Value> = EventStream::new(
        response("id: one\ndata: 1\n\nid: t\0wo\ndata: 2\n\n"),
        Framing::SseJsonData,
    );
    assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), 1);
    assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), 2);
    assert_eq!(stream.last_event_id(), Some("one"));
}

/// A `retry` field counts only when its value is one or more ASCII digits that fit a `u64`;
/// anything else is ignored.
#[test]
fn an_sse_retry_counts_only_as_ascii_digits() {
    for (field, retry) in [
        ("retry: 0042", Some(42)),
        ("retry: 12a", None),
        ("retry: -5", None),
        ("retry: 1.5", None),
        ("retry:  7", None),
        ("retry:", None),
        ("retry: 99999999999999999999999", None),
    ] {
        let mut buf = format!("{field}\ndata: 1\n\n").into_bytes();
        assert_eq!(
            next_frame(&mut buf, Framing::SseJsonData, false),
            FramePoll::Item {
                payload: b"1".to_vec(),
                metadata: SseMetadata { id: None, retry },
            },
            "{field}"
        );
    }
    // An ignored `retry` leaves the one an earlier line of the same event set.
    for field in ["retry: 12a", "retry:"] {
        let mut buf = format!("retry: 5\n{field}\ndata: 1\n\n").into_bytes();
        assert_eq!(
            next_frame(&mut buf, Framing::SseJsonData, false),
            FramePoll::Item {
                payload: b"1".to_vec(),
                metadata: SseMetadata {
                    id: None,
                    retry: Some(5)
                },
            },
            "{field}"
        );
    }
    // An event carrying only an ignored `retry` dispatches nothing at all.
    let mut buf = b"retry: soon\n\n".to_vec();
    assert_eq!(
        next_frame(&mut buf, Framing::SseJsonData, false),
        FramePoll::NeedMore
    );
    assert!(buf.is_empty());
}

/// A line with no colon is a field name with an empty value: a bare `data` dispatches an empty
/// payload, a bare `event` names the empty event type, a bare `id` resets the last ID, and an
/// unknown bare name is ignored.
#[test]
fn an_sse_line_without_a_colon_is_a_field_with_an_empty_value() {
    let mut buf = b"data\n\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::Sse, false);
    assert_eq!(items, vec![""]);

    let mut buf = b"event\ndata: x\n\n".to_vec();
    let (items, _) = drain(&mut buf, Framing::SseEvent, false);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&items[0]).unwrap(),
        serde_json::json!({"data": "x", "event": ""})
    );

    let mut buf = b"unknown\n\n".to_vec();
    assert_eq!(
        next_frame(&mut buf, Framing::Sse, false),
        FramePoll::NeedMore
    );
    assert!(buf.is_empty());

    let mut stream: EventStream<serde_json::Value> = EventStream::new(
        response("id: one\ndata: 1\n\nid\ndata: 2\n\n"),
        Framing::SseJsonData,
    );
    assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), 1);
    assert_eq!(stream.last_event_id(), Some("one"));
    assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), 2);
    assert_eq!(stream.last_event_id(), Some(""));
}

/// An SSE event that dispatches nothing is consumed and the same call frames the complete event
/// buffered behind it, before EOF too, rather than asking for more bytes.
#[test]
fn sse_frames_the_event_behind_one_that_dispatches_nothing() {
    for at_eof in [false, true] {
        let mut buf = b": keep-alive\n\ndata: 1\n\n".to_vec();
        assert_eq!(
            next_frame(&mut buf, Framing::SseJsonData, at_eof),
            FramePoll::Item {
                payload: b"1".to_vec(),
                metadata: SseMetadata::default(),
            },
            "at_eof = {at_eof}"
        );
        assert!(buf.is_empty());
    }
}

/// A reconnectable stream starts from the `Last-Event-ID` its prepared request already carries,
/// before any event has been read.
#[test]
fn a_reconnectable_stream_starts_from_the_requests_last_event_id() {
    let core = ClientCore::new("https://example.com").unwrap();
    let request = core
        .http()
        .request(Method::GET, "https://example.com/events")
        .header("last-event-id", "evt-0")
        .build()
        .unwrap();
    let stream: EventStream<serde_json::Value> = EventStream::new_reconnectable(
        response("data: 1\n\n"),
        Framing::SseJsonData,
        core,
        Some(request),
    );
    assert_eq!(stream.last_event_id(), Some("evt-0"));
}

/// JSON text sequences: bytes before the first record separator are discarded, a buffer with
/// no separator is held until EOF and dropped there, and an empty record frames nothing and is
/// consumed so the same call reaches the record after it.
#[test]
fn json_sequence_discards_junk_and_skips_empty_records() {
    let mut buf = b"junk\x1e1\n".to_vec();
    let (items, done) = drain(&mut buf, Framing::JsonSequence, true);
    assert_eq!(items, vec!["1"]);
    assert!(!done);
    assert!(buf.is_empty());

    let mut buf = b"no separator yet".to_vec();
    assert_eq!(
        next_frame(&mut buf, Framing::JsonSequence, false),
        FramePoll::NeedMore
    );
    assert_eq!(buf, b"no separator yet");
    assert_eq!(
        next_frame(&mut buf, Framing::JsonSequence, true),
        FramePoll::NeedMore
    );
    assert!(buf.is_empty());

    // A record still open before EOF waits for its terminating separator.
    let mut buf = b"\x1e{\"a\":".to_vec();
    assert_eq!(
        next_frame(&mut buf, Framing::JsonSequence, false),
        FramePoll::NeedMore
    );
    assert_eq!(buf, b"\x1e{\"a\":");

    // An empty record is consumed and the same call frames the record behind it, before EOF too.
    for at_eof in [false, true] {
        let mut buf = b"\x1e \n\x1e2\n\x1e".to_vec();
        assert_eq!(
            next_frame(&mut buf, Framing::JsonSequence, at_eof),
            FramePoll::Item {
                payload: b"2".to_vec(),
                metadata: SseMetadata::default(),
            }
        );
        assert_eq!(buf, b"\x1e");
    }
}

/// A stream whose `ClientCore` bounds an unterminated item at `max_stream_frame` bytes and
/// retains 4 bytes of an error body, holding a replayable request and the given policy.
fn bounded_stream(
    body: &str,
    framing: Framing,
    max_stream_frame: usize,
    policy: Arc<ImmediateReconnect>,
) -> EventStream<serde_json::Value> {
    let backend = Arc::new(SequenceBackend::new([(200, "{\"a\":2}\n")]));
    let mut core = ClientCore::with_backend(backend, "https://example.com").unwrap();
    core.config_mut().max_stream_frame = max_stream_frame;
    core.config_mut().max_error_body = 4;
    let request = core
        .http()
        .request(Method::GET, "https://example.com/events")
        .build()
        .unwrap();
    match EventStream::new_reconnectable(response(body), framing, core, Some(request))
        .with_reconnect(policy)
    {
        Ok(stream) => stream,
        Err(error) => panic!("reconnect should be available: {error}"),
    }
}

/// Once more than `max_stream_frame` bytes of an item whose terminator has not arrived are
/// buffered, the stream yields a `Decode` error retaining `max_error_body` bytes of them and
/// ends: the reconnect policy is not consulted, and the over-limit item is never yielded, though
/// it is valid JSON the end of the body would otherwise have flushed. The complete item ahead of
/// it in the same chunk is yielded first.
#[test]
fn an_unterminated_item_past_max_stream_frame_is_a_decode_error_that_ends_the_stream() {
    for (framing, body, retained) in [
        (
            Framing::Ndjson,
            "{\"a\":1}\n{\"b\":\"0123456789\"}",
            &b"{\"b\""[..],
        ),
        (
            Framing::SseJsonData,
            "data: {\"a\":1}\n\ndata: {\"b\":\"0123\"}",
            &b"data"[..],
        ),
        (
            Framing::JsonSequence,
            "\x1e\n\x1e{\"a\":1}\n\x1e{\"b\":\"0123456789\"}",
            &b"\x1e{\"b"[..],
        ),
    ] {
        let policy = Arc::new(ImmediateReconnect {
            max_attempts: 5,
            ..ImmediateReconnect::default()
        });
        let mut stream = bounded_stream(body, framing, 16, policy.clone());
        assert_eq!(
            poll_ready(stream.next()).unwrap().unwrap(),
            serde_json::json!({"a": 1}),
            "{framing:?}"
        );
        match poll_ready(stream.next()) {
            Some(Err(Error::Decode {
                status,
                headers,
                path,
                body,
                truncated,
            })) => {
                assert_eq!(status.as_u16(), 200);
                assert_eq!(headers.get("x-response").unwrap(), "initial");
                assert_eq!(
                    path,
                    "stream item exceeds max_stream_frame (16 bytes) before its terminator"
                );
                assert_eq!(&body[..], retained, "{framing:?}");
                assert!(truncated);
            }
            other => panic!("{framing:?}: expected a Decode error, got {other:?}"),
        }
        assert!(poll_ready(stream.next()).is_none(), "{framing:?}");
        assert!(policy.seen.lock().unwrap().is_empty(), "{framing:?}");
    }
}

/// An unterminated item of exactly `max_stream_frame` bytes stays buffered, and the end of the
/// body flushes it as an ordinary item.
#[test]
fn an_unterminated_item_at_max_stream_frame_is_still_framed() {
    let policy = Arc::new(ImmediateReconnect::default());
    let mut stream = bounded_stream(
        "{\"a\":1}\n{\"b\":\"01234567\"}",
        Framing::Ndjson,
        16,
        policy,
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"b": "01234567"})
    );
    assert!(poll_ready(stream.next()).is_none());
}

/// Empty JSON text sequence records do not hold back the complete records behind them: every
/// record in a chunk is framed before what is left is measured against `max_stream_frame`, so a
/// valid sequence whose items each fit is yielded whole even though the chunk exceeds the bound.
#[test]
fn json_sequence_records_behind_an_empty_record_are_framed_under_max_stream_frame() {
    let policy = Arc::new(ImmediateReconnect::default());
    let mut stream = bounded_stream(
        "\x1e\n\x1e{\"a\":1}\n\x1e{\"a\":2}\n\x1e{\"a\":3}\n\x1e",
        Framing::JsonSequence,
        16,
        policy.clone(),
    );
    for a in 1..=3 {
        assert_eq!(
            poll_ready(stream.next()).unwrap().unwrap(),
            serde_json::json!({ "a": a })
        );
    }
    assert!(poll_ready(stream.next()).is_none());
}

/// The default bound is the documented 16 MiB, and a stream built without a `ClientCore` takes
/// it.
#[test]
fn max_stream_frame_defaults_to_sixteen_mebibytes() {
    assert_eq!(
        crate::ClientConfig::default().max_stream_frame,
        16 * 1024 * 1024
    );
    let stream: EventStream<serde_json::Value> = EventStream::new(response(""), Framing::Ndjson);
    assert_eq!(stream.max_frame, 16 * 1024 * 1024);
}
