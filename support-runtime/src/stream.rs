//! Typed streaming for response bodies delivered as Server-Sent Events (`text/event-stream`) or
//! newline-delimited JSON (`application/x-ndjson`). A streaming operation returns an
//! [`EventStream<T>`], a standard [`futures_core::Stream`] with a dependency-free inherent
//! `next().await` convenience method. Reqwest's body stream keeps delivery incremental on native
//! and browser targets; dropping `EventStream` drops that body stream and cancels the transfer.
//!
//! Framing is a pure function ([`next_frame`]) over an owned byte buffer, so the framing/decoding
//! logic is unit-testable without network IO or an async runtime.

use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use reqwest::{Request, Response};
use serde::de::DeserializeOwned;

use crate::{send, unexpected_status, ClientCore, Error, MaybeSend, MaybeSync, TransportError};

/// The failure yielded by a streaming response after its initial HTTP response was accepted.
pub type StreamError = Error<Infallible>;

#[cfg(not(target_arch = "wasm32"))]
type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
#[cfg(target_arch = "wasm32")]
type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>>>>;

/// The caller-provided wait before an automatic reconnect attempt.
#[cfg(not(target_arch = "wasm32"))]
pub type ReconnectWait = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
/// The caller-provided wait before an automatic reconnect attempt (wasm variant).
#[cfg(target_arch = "wasm32")]
pub type ReconnectWait = Pin<Box<dyn Future<Output = ()> + 'static>>;

#[cfg(not(target_arch = "wasm32"))]
type ReconnectFuture = Pin<Box<dyn Future<Output = Result<Response, StreamError>> + Send>>;
#[cfg(target_arch = "wasm32")]
type ReconnectFuture = Pin<Box<dyn Future<Output = Result<Response, StreamError>>>>;

/// Why an automatic SSE reconnect is being considered.
#[derive(Debug)]
pub enum ReconnectReason<'a> {
    /// The server closed the response body cleanly.
    EndOfStream,
    /// Reading or re-establishing the stream failed.
    Failure(&'a StreamError),
}

/// Opt-in automatic reconnect policy.
///
/// Returning a wait future requests another connection; returning `None` stops. The policy owns
/// attempt limits and timing. `attempt` starts at zero, increases after each accepted reconnect,
/// and resets after a successfully decoded item. `server_delay` is the latest valid SSE `retry:`
/// value, when present.
pub trait ReconnectPolicy: MaybeSend + MaybeSync {
    /// Decide whether to reconnect and provide the future that waits until the next attempt.
    fn reconnect(
        &self,
        attempt: u32,
        reason: ReconnectReason<'_>,
        server_delay: Option<Duration>,
    ) -> Option<ReconnectWait>;
}

/// How a streaming response body is framed into individual items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Server-Sent Events (`text/event-stream`): events separated by a blank line; the `data:`
    /// field lines within an event are concatenated into one JSON payload. A `data: [DONE]`
    /// sentinel terminates the stream.
    Sse,
    /// Standards-compliant Server-Sent Events converted into JSON objects with `data`, `event`,
    /// `id`, and `retry` fields before deserializing the OpenAPI 3.2 `itemSchema` type.
    SseEvent,
    /// OpenAPI 3.2 SSE whose `data` string contains JSON described by `contentSchema`; metadata is
    /// retained on the stream while the decoded JSON payload is yielded directly.
    SseJsonData,
    /// Newline-delimited JSON (`application/x-ndjson`): one JSON item per non-empty line.
    Ndjson,
    /// RFC 7464 JSON Text Sequences (`application/json-seq` / `application/*+json-seq`).
    JsonSequence,
}

/// A typed async stream of items decoded from a streaming response body (SSE or ndjson).
///
/// Yielded by generated operation methods whose success response is a streaming media type, in
/// place of the usual `ResponseValue<T>`. Drive it manually:
///
/// ```ignore
/// let mut stream = client.watch_events().await?;
/// while let Some(item) = stream.next().await {
///     let event = item?; // a decoded `T`, or an `Error` on a decode/transport failure
/// }
/// ```
///
/// Dropping the stream is safe and cancels the underlying transfer (standard HTTP drop semantics).
pub struct EventStream<T> {
    state: StreamState,
    /// Bytes read but not yet framed into a complete item. Partial frames live here between chunks.
    buffer: Vec<u8>,
    /// The framing mode for this body.
    framing: Framing,
    last_event_id: Option<String>,
    reconnect_delay: Option<Duration>,
    reconnect: Option<ReconnectContext>,
    /// `T` is produced, never consumed; the `fn() -> T` marker keeps `T` from imposing unrelated
    /// auto-trait bounds on the stream.
    _marker: PhantomData<fn() -> T>,
}

enum StreamState {
    Body(BodyStream),
    Eof,
    Waiting(ReconnectWait),
    Connecting(ReconnectFuture),
    Done,
}

enum OwnedReconnectReason {
    EndOfStream,
    Failure(StreamError),
}

struct ReconnectContext {
    core: ClientCore,
    request: Request,
    policy: Option<Arc<dyn ReconnectPolicy>>,
    attempt: u32,
}

impl<T> EventStream<T> {
    /// Wrap a streaming response with the framing mode chosen for its media type. The response is
    /// consumed lazily — no bytes are read until the first [`Self::next`] call.
    pub fn new(response: Response, framing: Framing) -> Self {
        Self {
            state: StreamState::Body(Box::pin(response.bytes_stream())),
            buffer: Vec::new(),
            framing,
            last_event_id: None,
            reconnect_delay: None,
            reconnect: None,
            _marker: PhantomData,
        }
    }

    /// Build a stream that can opt into reconnecting the prepared request. Generated operation
    /// methods use this constructor; ordinary callers use [`Self::with_reconnect`] to enable it.
    pub fn new_reconnectable(
        response: Response,
        framing: Framing,
        core: ClientCore,
        request: Option<Request>,
    ) -> Self {
        let last_event_id = request.as_ref().and_then(|request| {
            request
                .headers()
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        });
        Self {
            state: StreamState::Body(Box::pin(response.bytes_stream())),
            buffer: Vec::new(),
            framing,
            last_event_id,
            reconnect_delay: None,
            reconnect: request.map(|request| ReconnectContext {
                core,
                request,
                policy: None,
                attempt: 0,
            }),
            _marker: PhantomData,
        }
    }

    /// Enable automatic reconnects using a caller-owned policy and timer.
    pub fn with_reconnect(mut self, policy: Arc<dyn ReconnectPolicy>) -> Result<Self, StreamError> {
        let Some(reconnect) = self.reconnect.as_mut() else {
            return Err(Error::request_message(
                "this streaming request cannot be cloned safely for automatic reconnect",
            ));
        };
        reconnect.policy = Some(policy);
        Ok(self)
    }

    /// The latest SSE event ID, including an empty ID that resets replay state.
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// The latest valid server-provided SSE `retry:` delay.
    pub fn reconnect_delay(&self) -> Option<Duration> {
        self.reconnect_delay
    }
}

impl<T: DeserializeOwned> EventStream<T> {
    /// Yield the next decoded item, or `None` at end of stream. A mid-stream transport error that
    /// is not accepted by an enabled reconnect policy surfaces as `Some(Err(..))` and terminates the
    /// stream (subsequent calls return `None`). A per-frame decode failure also surfaces as
    /// `Some(Err(Error::Decode { .. }))` rather than being silently skipped, but does NOT terminate
    /// the stream — the next call resumes framing the following items, so a single malformed event
    /// does not abandon the rest.
    ///
    /// The item type `T` is decoded per-frame with `serde_json::from_slice`.
    pub async fn next(&mut self) -> Option<Result<T, StreamError>> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }
}

impl<T: DeserializeOwned> Stream for EventStream<T> {
    type Item = Result<T, StreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            let at_eof = matches!(this.state, StreamState::Eof);
            match next_frame(&mut this.buffer, this.framing, at_eof) {
                FramePoll::Item { payload, metadata } => {
                    this.apply_metadata(metadata);
                    let item = deserialize_item::<T>(&payload);
                    if item.is_ok() {
                        if let Some(reconnect) = this.reconnect.as_mut() {
                            reconnect.attempt = 0;
                        }
                    }
                    return Poll::Ready(Some(item));
                }
                FramePoll::Metadata(metadata) => {
                    this.apply_metadata(metadata);
                    continue;
                }
                FramePoll::Done(metadata) => {
                    this.apply_metadata(metadata);
                    this.state = StreamState::Done;
                    this.buffer.clear();
                    return Poll::Ready(None);
                }
                FramePoll::NeedMore => {}
            }

            match std::mem::replace(&mut this.state, StreamState::Done) {
                StreamState::Body(mut body) => match body.as_mut().poll_next(cx) {
                    Poll::Pending => {
                        this.state = StreamState::Body(body);
                        return Poll::Pending;
                    }
                    Poll::Ready(Some(Ok(chunk))) => {
                        this.state = StreamState::Body(body);
                        this.buffer.extend_from_slice(&chunk);
                    }
                    Poll::Ready(Some(Err(error))) => {
                        let error = Error::InterruptedBody(TransportError::new(error));
                        if let Some(error) =
                            this.schedule_reconnect(OwnedReconnectReason::Failure(error))
                        {
                            return Poll::Ready(Some(Err(error)));
                        }
                    }
                    Poll::Ready(None) => this.state = StreamState::Eof,
                },
                StreamState::Eof => {
                    let _ = this.schedule_reconnect(OwnedReconnectReason::EndOfStream);
                    if matches!(this.state, StreamState::Done) {
                        return Poll::Ready(None);
                    }
                }
                StreamState::Waiting(mut wait) => match wait.as_mut().poll(cx) {
                    Poll::Pending => {
                        this.state = StreamState::Waiting(wait);
                        return Poll::Pending;
                    }
                    Poll::Ready(()) => {
                        let Some(future) = this.reconnect_future() else {
                            this.state = StreamState::Done;
                            return Poll::Ready(Some(Err(Error::request_message(
                                "streaming request could not be cloned for reconnect",
                            ))));
                        };
                        this.state = StreamState::Connecting(future);
                    }
                },
                StreamState::Connecting(mut future) => match future.as_mut().poll(cx) {
                    Poll::Pending => {
                        this.state = StreamState::Connecting(future);
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(response)) => {
                        this.state = StreamState::Body(Box::pin(response.bytes_stream()));
                    }
                    Poll::Ready(Err(error)) => {
                        if let Some(error) =
                            this.schedule_reconnect(OwnedReconnectReason::Failure(error))
                        {
                            return Poll::Ready(Some(Err(error)));
                        }
                    }
                },
                StreamState::Done => return Poll::Ready(None),
            }
        }
    }
}

impl<T> EventStream<T> {
    fn apply_metadata(&mut self, metadata: SseMetadata) {
        if let Some(id) = metadata.id {
            self.last_event_id = Some(id);
        }
        if let Some(retry) = metadata.retry {
            self.reconnect_delay = Some(Duration::from_millis(retry));
        }
    }

    /// Schedule a reconnect. `Some(error)` means the policy declined a failure and it must be
    /// yielded; `None` means either reconnecting was scheduled or clean EOF terminated normally.
    fn schedule_reconnect(&mut self, reason: OwnedReconnectReason) -> Option<StreamError> {
        let Some(reconnect) = self.reconnect.as_mut() else {
            self.state = StreamState::Done;
            return match reason {
                OwnedReconnectReason::EndOfStream => None,
                OwnedReconnectReason::Failure(error) => Some(error),
            };
        };
        let Some(policy) = reconnect.policy.as_ref() else {
            self.state = StreamState::Done;
            return match reason {
                OwnedReconnectReason::EndOfStream => None,
                OwnedReconnectReason::Failure(error) => Some(error),
            };
        };
        let reason_ref = match &reason {
            OwnedReconnectReason::EndOfStream => ReconnectReason::EndOfStream,
            OwnedReconnectReason::Failure(error) => ReconnectReason::Failure(error),
        };
        let Some(wait) = policy.reconnect(reconnect.attempt, reason_ref, self.reconnect_delay)
        else {
            self.state = StreamState::Done;
            return match reason {
                OwnedReconnectReason::EndOfStream => None,
                OwnedReconnectReason::Failure(error) => Some(error),
            };
        };
        reconnect.attempt = reconnect.attempt.saturating_add(1);
        // A partial event belongs to the closed connection and must never be concatenated with the
        // first bytes from the replay connection.
        self.buffer.clear();
        self.state = StreamState::Waiting(wait);
        None
    }

    fn reconnect_future(&self) -> Option<ReconnectFuture> {
        let reconnect = self.reconnect.as_ref()?;
        let core = reconnect.core.clone();
        let mut request = reconnect.request.try_clone()?;
        match self.last_event_id.as_deref() {
            Some("") => {
                request.headers_mut().remove("last-event-id");
            }
            Some(id) => {
                let value = reqwest::header::HeaderValue::from_str(id).ok()?;
                request.headers_mut().insert("last-event-id", value);
            }
            None => {}
        }
        Some(Box::pin(async move {
            let response = send(&core, request).await?;
            if response.status().is_success() {
                Ok(response)
            } else {
                Err(unexpected_status(&core, response).await)
            }
        }))
    }
}

/// The outcome of attempting to frame one item out of the buffer.
#[derive(Debug, PartialEq, Eq)]
enum FramePoll {
    /// A complete JSON payload was framed and removed from the buffer.
    Item {
        payload: Vec<u8>,
        metadata: SseMetadata,
    },
    /// An SSE event updated metadata without dispatching a data payload.
    Metadata(SseMetadata),
    /// A terminator was reached (SSE `[DONE]` sentinel): the stream ends.
    Done(SseMetadata),
    /// No complete frame is available yet; read more bytes (or, at EOF, the stream ends).
    NeedMore,
}

/// Frame the next item out of `buffer`, consuming the bytes it uses. `at_eof` is `true` once no
/// further bytes will arrive, which lets a trailing frame not terminated by a delimiter still be
/// emitted. Pure: no IO, no async — the unit tests drive it directly.
fn next_frame(buffer: &mut Vec<u8>, framing: Framing, at_eof: bool) -> FramePoll {
    match framing {
        Framing::Ndjson => ndjson_next(buffer, at_eof),
        Framing::Sse => sse_next(buffer, at_eof, SseOutput::LegacyData),
        Framing::SseEvent => sse_next(buffer, at_eof, SseOutput::Envelope),
        Framing::SseJsonData => sse_next(buffer, at_eof, SseOutput::JsonData),
        Framing::JsonSequence => json_sequence_next(buffer, at_eof),
    }
}

/// ndjson framing: each complete `\n`-terminated line (CRLF tolerated) is one item; empty lines are
/// skipped. At EOF a final line without a trailing newline is emitted.
fn ndjson_next(buffer: &mut Vec<u8>, at_eof: bool) -> FramePoll {
    loop {
        if let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
            let mut end = nl;
            if end > 0 && buffer[end - 1] == b'\r' {
                end -= 1;
            }
            let line = buffer[..end].to_vec();
            buffer.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            return frame_item(line);
        }
        if at_eof {
            let mut end = buffer.len();
            if end > 0 && buffer[end - 1] == b'\r' {
                end -= 1;
            }
            let line = buffer[..end].to_vec();
            buffer.clear();
            if line.is_empty() {
                return FramePoll::NeedMore;
            }
            return frame_item(line);
        }
        return FramePoll::NeedMore;
    }
}

/// RFC 7464 framing: each item starts with ASCII Record Separator (`0x1E`) and runs until the next
/// separator (or EOF). Newlines are record terminators/formatting and are trimmed around the JSON
/// text, while embedded newlines remain valid JSON whitespace.
fn json_sequence_next(buffer: &mut Vec<u8>, at_eof: bool) -> FramePoll {
    let Some(first) = buffer.iter().position(|byte| *byte == 0x1e) else {
        if at_eof {
            buffer.clear();
        }
        return FramePoll::NeedMore;
    };
    if first > 0 {
        buffer.drain(..first);
    }
    let end = buffer[1..]
        .iter()
        .position(|byte| *byte == 0x1e)
        .map(|index| index + 1);
    let Some(end) = end.or(at_eof.then_some(buffer.len())) else {
        return FramePoll::NeedMore;
    };
    let mut payload = buffer[1..end].to_vec();
    while payload.last().is_some_and(u8::is_ascii_whitespace) {
        payload.pop();
    }
    let start = payload
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(payload.len());
    payload.drain(..start);
    if end == buffer.len() {
        buffer.clear();
    } else {
        buffer.drain(..end);
    }
    if payload.is_empty() {
        FramePoll::NeedMore
    } else {
        frame_item(payload)
    }
}

fn frame_item(payload: Vec<u8>) -> FramePoll {
    FramePoll::Item {
        payload,
        metadata: SseMetadata::default(),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SseMetadata {
    id: Option<String>,
    retry: Option<u64>,
}

#[derive(Default)]
struct SseFields {
    data: Vec<u8>,
    event: Option<Vec<u8>>,
    id: Option<Vec<u8>>,
    retry: Option<u64>,
}

#[derive(Clone, Copy)]
enum SseOutput {
    LegacyData,
    Envelope,
    JsonData,
}

/// SSE framing. Events are parsed once, then rendered as the legacy JSON-in-`data` payload, the
/// OpenAPI 3.2 event envelope, or the JSON payload described by `data.contentSchema`.
fn sse_next(buffer: &mut Vec<u8>, at_eof: bool, output: SseOutput) -> FramePoll {
    loop {
        let mut pos = 0;
        let mut fields = SseFields::default();
        let mut saw_terminator = false;
        while let Some((line, next)) = take_line(buffer, pos) {
            pos = next;
            if line.is_empty() {
                saw_terminator = true;
                break;
            }
            append_sse_field(line, &mut fields);
        }
        if !saw_terminator {
            if !at_eof {
                return FramePoll::NeedMore;
            }
            if pos < buffer.len() {
                let mut line = &buffer[pos..];
                if line.last() == Some(&b'\r') {
                    line = &line[..line.len() - 1];
                }
                append_sse_field(line, &mut fields);
            }
            buffer.clear();
        } else {
            buffer.drain(..pos);
        }
        match finish_sse_fields(fields, output) {
            FramePoll::NeedMore if !buffer.is_empty() => continue,
            result => return result,
        }
    }
}

fn append_sse_field(line: &[u8], fields: &mut SseFields) {
    if line.first() == Some(&b':') {
        return;
    }
    let (field, value) = match line.iter().position(|&b| b == b':') {
        Some(colon) => {
            let mut value = &line[colon + 1..];
            if value.first() == Some(&b' ') {
                value = &value[1..];
            }
            (&line[..colon], value)
        }
        None => (line, &b""[..]),
    };
    match field {
        b"data" => {
            fields.data.extend_from_slice(value);
            fields.data.push(b'\n');
        }
        b"event" => fields.event = Some(value.to_vec()),
        b"id" if !value.contains(&0) => fields.id = Some(value.to_vec()),
        b"retry" if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
            fields.retry = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse().ok());
        }
        _ => {}
    }
}

fn finish_sse_fields(mut fields: SseFields, output: SseOutput) -> FramePoll {
    let metadata = SseMetadata {
        id: fields
            .id
            .as_deref()
            .map(|id| String::from_utf8_lossy(id).into_owned()),
        retry: fields.retry,
    };
    if fields.data.is_empty() {
        return if metadata == SseMetadata::default() {
            FramePoll::NeedMore
        } else {
            FramePoll::Metadata(metadata)
        };
    }
    fields.data.pop();
    let payload = match output {
        SseOutput::LegacyData => {
            if fields.data == b"[DONE]" {
                return FramePoll::Done(metadata);
            }
            fields.data
        }
        SseOutput::JsonData => fields.data,
        SseOutput::Envelope => {
            let mut object = serde_json::Map::new();
            object.insert(
                "data".to_owned(),
                serde_json::Value::String(String::from_utf8_lossy(&fields.data).into_owned()),
            );
            if let Some(event) = fields.event {
                object.insert(
                    "event".to_owned(),
                    serde_json::Value::String(String::from_utf8_lossy(&event).into_owned()),
                );
            }
            if let Some(id) = fields.id {
                object.insert(
                    "id".to_owned(),
                    serde_json::Value::String(String::from_utf8_lossy(&id).into_owned()),
                );
            }
            if let Some(retry) = fields.retry {
                object.insert("retry".to_owned(), serde_json::Value::from(retry));
            }
            serde_json::Value::Object(object).to_string().into_bytes()
        }
    };
    FramePoll::Item { payload, metadata }
}

/// If `buf[from..]` holds a complete `\n`-terminated line, return it (trailing `\r\n`/`\n` stripped)
/// and the index just past the newline. Returns `None` when no full line is buffered yet.
fn take_line(buf: &[u8], from: usize) -> Option<(&[u8], usize)> {
    let rest = &buf[from..];
    let nl = rest.iter().position(|&b| b == b'\n')?;
    let mut line = &rest[..nl];
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    Some((line, from + nl + 1))
}

/// Decode one framed JSON payload into `T`. A parse failure becomes [`Error::Decode`] carrying the
/// serde path and the raw frame — never a silent skip.
fn deserialize_item<T: DeserializeOwned>(payload: &[u8]) -> Result<T, Error<Infallible>> {
    serde_json::from_slice::<T>(payload).map_err(|error| Error::Decode {
        path: error.to_string(),
        body: Bytes::copy_from_slice(payload),
        truncated: false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use reqwest::{Method, Request};

    use super::{
        next_frame, EventStream, FramePoll, Framing, ReconnectPolicy, ReconnectReason,
        ReconnectWait,
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
        let mut buf = b": ignored\nevent: add\nid: 7\nretry: 5\ndata: first\ndata: second\nunknown: ignored\n\n".to_vec();
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
            super::deserialize_item(items[0].as_bytes());
        assert!(matches!(decoded, Err(Error::Decode { .. })));
    }

    #[test]
    fn well_formed_json_frame_deserializes() {
        let decoded: Result<serde_json::Value, Error<std::convert::Infallible>> =
            super::deserialize_item(br#"{"a":1}"#);
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
        reqwest::Response::from(
            http::Response::builder()
                .status(200)
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

    #[test]
    fn next_yields_a_decode_error_for_a_malformed_item() {
        let mut stream: EventStream<serde_json::Value> =
            EventStream::new(response("not json\n"), Framing::Ndjson);
        let item = poll_ready(stream.next()).unwrap();
        assert!(matches!(item, Err(Error::Decode { .. })));
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
}
