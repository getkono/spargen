//! How an `EventStream` fails and stops (#1330): a mid-stream read failure reported by its
//! transport class, an SSE event the body ends in the middle of, and the reconnect answers that
//! end the stream without asking the policy again.

use std::sync::Arc;

use super::{
    prepared, reconnectable_stream, reconnectable_stream_over, response, sse_reply, EventStream,
    Framing, ImmediateReconnect,
};
use crate::tests::{poll_ready, Chunk, Chunks, Reply, SequenceBackend};
use crate::{ClientCore, Error, HttpBody};
use reqwest::Method;

/// The policy calls `stream` saw, as `(attempt, reason)`.
fn reasons(policy: &ImmediateReconnect) -> Vec<(u32, &'static str)> {
    policy
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(attempt, reason, _)| (*attempt, *reason))
        .collect()
}

/// Issue #1330: a `204` answering a reconnect is the server's "stop reconnecting", whatever the
/// framing: the stream ends cleanly, and nothing is resent however many attempts the policy would
/// still allow.
#[test]
fn a_204_reconnect_response_ends_the_stream() {
    for framing in [Framing::SseJsonData, Framing::Ndjson] {
        let backend = SequenceBackend::new([Reply::status(204, "")]);
        let policy = Arc::new(ImmediateReconnect {
            max_attempts: 5,
            ..ImmediateReconnect::default()
        });
        let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
        let request = crate::tests::builder(Method::GET, "https://example.com/events")
            .build()
            .unwrap();
        let initial = match framing {
            Framing::Ndjson => "{\"seq\":1}\n",
            _ => "data: {\"seq\":1}\n\n",
        };
        let mut stream: EventStream<serde_json::Value> = EventStream::new_reconnectable(
            response(initial),
            framing,
            &core,
            prepared(&core, request),
        )
        .with_reconnect(policy.clone())
        .unwrap();

        assert_eq!(
            poll_ready(stream.next()).unwrap().unwrap(),
            serde_json::json!({"seq": 1}),
            "{framing:?}"
        );
        assert!(poll_ready(stream.next()).is_none(), "{framing:?}");
        assert!(poll_ready(stream.next()).is_none(), "{framing:?}");
        assert_eq!(backend.calls(), 1, "{framing:?}");
        assert_eq!(reasons(&policy), [(0, "eof")], "{framing:?}");
    }
}

/// Issue #1330: an SSE reconnect answered with a body that is not `text/event-stream` (a proxy's
/// HTML page, or no `Content-Type` at all) is not framed as events: the stream yields a `Decode`
/// error carrying that response's status, headers and capped body, and ends without consulting
/// the policy again.
#[test]
fn an_sse_reconnect_response_that_is_not_an_event_stream_ends_the_stream() {
    for (reply, content_type) in [
        (
            Reply::status(200, "<html>maintenance</html>").header("content-type", "text/html"),
            Some("text/html"),
        ),
        (Reply::status(200, "data: {\"seq\":2}\n\n"), None),
    ] {
        let backend = SequenceBackend::new([reply]);
        let policy = Arc::new(ImmediateReconnect {
            max_attempts: 5,
            ..ImmediateReconnect::default()
        });
        let mut stream =
            reconnectable_stream("data: {\"seq\":1}\n\n", backend.clone(), policy.clone());

        assert_eq!(
            poll_ready(stream.next()).unwrap().unwrap(),
            serde_json::json!({"seq": 1})
        );
        match poll_ready(stream.next()) {
            Some(Err(
                ref error @ Error::Decode {
                    status,
                    ref headers,
                    ref path,
                    ref body,
                    truncated,
                },
            )) => {
                assert_eq!(status, 200);
                assert_eq!(headers.get("x-response").unwrap(), "backend");
                assert!(path.contains("text/event-stream"), "{path}");
                assert!(!truncated);
                assert!(!body.is_empty());
                assert!(!error.is_transient());
                if let Some(content_type) = content_type {
                    assert!(path.contains(content_type), "{path}");
                }
            }
            other => panic!("expected a Decode error, got {other:?}"),
        }
        assert!(poll_ready(stream.next()).is_none());
        assert_eq!(backend.calls(), 1);
        assert_eq!(reasons(&policy), [(0, "eof")]);
    }
}

/// The `Content-Type` an SSE reconnect is held to is the media type alone: its case and its
/// parameters do not matter.
#[test]
fn an_sse_reconnect_accepts_event_stream_with_parameters_in_any_case() {
    let backend = SequenceBackend::new([Reply::status(200, "data: {\"seq\":2}\n\n")
        .header("content-type", "Text/Event-Stream; charset=utf-8")]);
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream("data: {\"seq\":1}\n\n", backend, policy);
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
}

/// A body read the connection cuts short partway through an item yields `InterruptedBody`, a
/// transient failure, and with no reconnect policy ends the stream: the partial item is never
/// yielded.
#[test]
fn a_body_that_fails_mid_item_yields_interrupted_body_and_ends_the_stream() {
    let initial = Chunks::new([Chunk::data("{\"a\":1}\n{\"b\":"), Chunk::Fail]).into_response(200);
    let mut stream: EventStream<serde_json::Value> = EventStream::new(initial, Framing::Ndjson);
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"a": 1})
    );
    match poll_ready(stream.next()) {
        Some(Err(error @ Error::InterruptedBody(_))) => assert!(error.is_transient()),
        other => panic!("expected InterruptedBody, got {other:?}"),
    }
    assert!(poll_ready(stream.next()).is_none());
}

/// Issue #1330: an event the server cut off at EOF is never dispatched, and the `id` it carried is
/// not committed, so the reconnect resumes from the last event the caller received and the server
/// replays the one that was cut off.
#[test]
fn an_sse_event_cut_off_at_eof_is_not_dispatched_and_its_id_is_not_replayed() {
    let backend = SequenceBackend::new([
        sse_reply(200, "id: 7\ndata: {\"seq\":7}\n\n"),
        sse_reply(200, ""),
    ]);
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let mut stream = reconnectable_stream(
        "id: 6\ndata: {\"seq\":6}\n\nid: 7\ndata: {\"seq\":",
        backend.clone(),
        policy,
    );

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 6})
    );
    assert_eq!(stream.last_event_id(), Some("6"));
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 7})
    );
    assert!(poll_ready(stream.next()).is_none());
    assert_eq!(
        backend.header("last-event-id"),
        [Some("6".to_owned()), Some("7".to_owned())]
    );
}

/// A body that yields `data`, then fails with `failure`, which keeps its class through
/// [`HttpBody::wrap`].
struct FailsAfter {
    data: Option<bytes::Bytes>,
    failure: Option<crate::TransportError>,
}

impl http_body::Body for FailsAfter {
    type Data = bytes::Bytes;
    type Error = crate::TransportError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<bytes::Bytes>, crate::TransportError>>>
    {
        let this = &mut *self;
        std::task::Poll::Ready(match this.data.take() {
            Some(data) => Some(Ok(http_body::Frame::data(data))),
            None => this.failure.take().map(Err),
        })
    }
}

/// A `200` response whose body yields `data`, then fails with `failure`.
fn failing_response(data: &str, failure: crate::TransportError) -> crate::tests::Response {
    http::Response::builder()
        .status(200)
        .body(HttpBody::wrap(FailsAfter {
            data: Some(bytes::Bytes::copy_from_slice(data.as_bytes())),
            failure: Some(failure),
        }))
        .expect("valid synthetic response")
}

/// Issue #1330: a mid-stream read failure surfaces as the variant its transport class names, as
/// `ByteStream` reports it: a stalled read is `Timeout`, as `ClientConfig::read_timeout` promises,
/// and a corrupt body is a `Protocol` failure, which is not transient.
#[test]
fn a_mid_stream_read_failure_surfaces_as_its_transport_class() {
    let timeout = crate::TransportError::timeout(crate::TimeoutKind::Total, "read timed out");
    let mut stream: EventStream<serde_json::Value> =
        EventStream::new(failing_response("{\"a\":1}\n", timeout), Framing::Ndjson);
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"a": 1})
    );
    match poll_ready(stream.next()) {
        Some(Err(Error::Timeout(crate::TimeoutKind::Total))) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
    assert!(poll_ready(stream.next()).is_none());

    let corrupt = crate::TransportError::protocol("corrupt gzip frame");
    let mut stream: EventStream<serde_json::Value> =
        EventStream::new(failing_response("{\"a\":", corrupt), Framing::Ndjson);
    match poll_ready(stream.next()) {
        Some(Err(error @ Error::Protocol(_))) => assert!(!error.is_transient()),
        other => panic!("expected Protocol, got {other:?}"),
    }
    assert!(poll_ready(stream.next()).is_none());
}

/// Issue #1330: a mid-stream timeout is still offered to the reconnect policy, which resumes the
/// stream from the last event ID.
#[test]
fn a_mid_stream_timeout_is_offered_to_the_reconnect_policy() {
    let backend =
        SequenceBackend::new([sse_reply(200, "data: {\"seq\":2}\n\n"), sse_reply(200, "")]);
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let timeout = crate::TransportError::timeout(crate::TimeoutKind::Total, "read timed out");
    let initial = failing_response("id: 1\ndata: {\"seq\":1}\n\n", timeout);
    let mut stream = reconnectable_stream_over(initial, backend.clone(), policy.clone());

    let mut items = Vec::new();
    while let Some(item) = poll_ready(stream.next()) {
        items.push(item.unwrap());
    }
    assert_eq!(
        items,
        [serde_json::json!({"seq": 1}), serde_json::json!({"seq": 2})]
    );
    assert_eq!(
        backend.header("last-event-id"),
        [Some("1".to_owned()), Some("1".to_owned())]
    );
    assert_eq!(
        policy
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|(attempt, reason, _)| (*attempt, *reason))
            .collect::<Vec<_>>(),
        [(0, "failure"), (0, "eof"), (1, "eof")]
    );
}

/// The bytes of an event the failed connection left unfinished are dropped before the replay's
/// first bytes arrive. Joined, the two would frame `{"seq":2}`; apart, the replay's leading
/// `2}` line is a field the framer ignores, so the next item is `{"seq":3}`.
#[test]
fn a_reconnect_after_an_interrupted_body_discards_the_partial_event() {
    let backend = SequenceBackend::new([
        sse_reply(200, "2}\n\ndata: {\"seq\":3}\n\n"),
        sse_reply(200, ""),
    ]);
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    let initial = Chunks::new([
        Chunk::data("data: {\"seq\":1}\n\ndata: {\"seq\":"),
        Chunk::Fail,
    ])
    .into_response(200);
    let mut stream = reconnectable_stream_over(initial, backend.clone(), policy.clone());

    let mut items = Vec::new();
    while let Some(item) = poll_ready(stream.next()) {
        items.push(item.unwrap());
    }
    assert_eq!(
        items,
        [serde_json::json!({"seq": 1}), serde_json::json!({"seq": 3})]
    );
    assert_eq!(backend.calls(), 2);
    assert_eq!(
        policy
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|(attempt, reason, _)| (*attempt, *reason))
            .collect::<Vec<_>>(),
        [(0, "failure"), (0, "eof"), (1, "eof")]
    );
}
