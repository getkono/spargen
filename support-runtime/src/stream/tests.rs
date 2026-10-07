use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use reqwest::{Method, Request};

use super::{
    next_frame, EventStream, FramePoll, Framing, ReconnectPolicy, ReconnectReason, ReconnectWait,
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

// A poll-once driver (noop waker, no async runtime) proving the async `next` await-loop threads
// the pure framing correctly over an in-memory `reqwest::Response`.
use std::future::Future;
use std::task::{Context, Poll, Waker};

fn poll_ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("future was not immediately ready"),
    }
}

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
