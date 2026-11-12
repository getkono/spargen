//! Typed streaming for response bodies delivered as Server-Sent Events (`text/event-stream`),
//! newline-delimited JSON (`application/x-ndjson`), or RFC 7464 JSON Text Sequences
//! (`application/json-seq`); [`Framing`] names each mode. A streaming operation returns an
//! [`EventStream<T>`], a standard [`futures_core::Stream`] with a dependency-free inherent
//! `next().await` convenience method. It reads the response's [`HttpBody`] chunk by chunk as the
//! backend delivers it: `ReqwestBackend` delivers chunks incrementally on native targets; on
//! `wasm32`, reqwest's fetch implementation may buffer the body internally, and the stream API and
//! framing are the same. Dropping `EventStream` drops that body and cancels the transfer.
//!
//! Framing is a pure function ([`next_frame`]) over an owned [`FrameBuffer`], with no network IO
//! and no async runtime. The buffer remembers how far the framer has already searched, so each
//! byte is searched once however many chunks the item it belongs to arrives in.

use std::convert::Infallible;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

use crate::dispatch::{cap_body, send_on, unexpected_status_capped};
use crate::transport::try_clone_request;
use crate::wasm::BoxFuture;
use crate::{ClientConfig, ClientCore, Error, HttpBackend, HttpBody, MaybeSend, MaybeSync};

/// The failure yielded by a streaming response after its initial HTTP response was accepted.
pub type StreamError = Error<Infallible>;

/// A response as the transport seam carries it.
type Response = http::Response<HttpBody>;

/// The caller-provided wait before an automatic reconnect attempt. `Send` off `wasm32` and not on
/// it, as the `wasm` module explains.
pub type ReconnectWait = BoxFuture<'static, ()>;

type ReconnectFuture = BoxFuture<'static, Result<Response, StreamError>>;

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

/// A typed async stream of items decoded from a streaming response body (SSE, ndjson, or a JSON
/// Text Sequence).
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
    /// The status of the response whose body is being framed — the initial response, or the
    /// latest accepted reconnect. A per-frame decode failure reports it as `Error::Decode`'s
    /// `status`.
    status: StatusCode,
    /// The headers of that same response, reported as `Error::Decode`'s `headers`.
    headers: HeaderMap,
    /// Bytes read but not yet framed into a complete item. Partial frames live here between chunks.
    buffer: FrameBuffer,
    /// `ClientConfig::max_stream_frame`: the most bytes `buffer` may hold for an item whose
    /// terminator has not arrived before the stream fails.
    max_frame: usize,
    /// `ClientConfig::max_error_body`: the bytes of an over-limit item its error retains.
    max_error_body: usize,
    /// The framing mode for this body.
    framing: Framing,
    /// Whether the current connection's SSE body may still open with a UTF-8 byte order mark not
    /// yet stripped: set for every SSE body the stream opens, the first and each reconnect's, and
    /// cleared once its first bytes rule a mark in or out.
    bom_pending: bool,
    last_event_id: Option<String>,
    reconnect_delay: Option<Duration>,
    reconnect: Option<ReconnectContext>,
    /// `T` is produced, never consumed; the `fn() -> T` marker keeps `T` from imposing unrelated
    /// auto-trait bounds on the stream.
    _marker: PhantomData<fn() -> T>,
}

enum StreamState {
    Body(HttpBody),
    Eof,
    Waiting(ReconnectWait),
    Connecting(ReconnectFuture),
    Done,
}

enum OwnedReconnectReason {
    EndOfStream,
    Failure(StreamError),
}

/// What a reconnect resends and through what: the client's transport alone, shared rather than
/// copied with the rest of its [`ClientCore`]. A reconnect's error body is capped at the stream's
/// own `max_error_body`, taken from the same client configuration.
struct ReconnectContext {
    backend: Arc<dyn HttpBackend>,
    request: http::Request<HttpBody>,
    policy: Option<Arc<dyn ReconnectPolicy>>,
    attempt: u32,
}

impl<T> EventStream<T> {
    /// Wrap a streaming response with the framing mode chosen for its media type. The response is
    /// consumed lazily — no bytes are read until the first [`Self::next`] call. It holds no
    /// [`ClientCore`], so its limits are `ClientConfig::default()`'s.
    /// Crate-visible, like [`Self::new_reconnectable`], which builds on it.
    pub(crate) fn new(response: Response, framing: Framing) -> Self {
        let config = ClientConfig::default();
        let (parts, body) = response.into_parts();
        Self {
            status: parts.status,
            headers: parts.headers,
            state: StreamState::Body(streamed(body)),
            buffer: FrameBuffer::default(),
            max_frame: config.max_stream_frame,
            max_error_body: config.max_error_body,
            framing,
            bom_pending: is_sse(framing),
            last_event_id: None,
            reconnect_delay: None,
            reconnect: None,
            _marker: PhantomData,
        }
    }

    /// Build a stream that can opt into reconnecting the prepared request. Generated operation
    /// methods use this constructor; ordinary callers use [`Self::with_reconnect`] to enable it.
    /// Crate-visible: the generated operation methods share the consumer's crate with the
    /// embedded runtime, so they reach it while the [`Framing`] it takes stays outside the
    /// generated public API. The `allow` is for this crate's own build: its caller is generated code.
    ///
    /// `request` is the clone the operation kept of the [`crate::prepare`]d request it sent, the
    /// core's [`crate::RequestOptions`] and its `ops::Operation` already on it, so every reconnect
    /// carries them too; `None` (a generated `decode_{method}`, which never saw the request) leaves
    /// the stream nothing to resend, as [`Self::resending`] describes.
    #[allow(dead_code)]
    pub(crate) fn new_reconnectable(
        response: Response,
        framing: Framing,
        core: &ClientCore,
        request: Option<http::Request<HttpBody>>,
    ) -> Self {
        Self {
            max_frame: core.config().max_stream_frame,
            max_error_body: core.config().max_error_body,
            ..Self::new(response, framing)
        }
        .resending(core, request)
    }

    /// This stream, able to reconnect by resending `request` through the core's backend once
    /// [`Self::with_reconnect`] enables it, and seeded with the `Last-Event-ID` `request` carries;
    /// with `None` (a request whose body could not be cloned) it has nothing to resend. A
    /// generated operation hands its `decode_{method}`'s stream the request it sent this way.
    /// Crate-visible, as [`Self::new_reconnectable`] is.
    #[allow(dead_code)]
    pub(crate) fn resending(
        mut self,
        core: &ClientCore,
        request: Option<http::Request<HttpBody>>,
    ) -> Self {
        self.last_event_id = request.as_ref().and_then(|request| {
            request
                .headers()
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        });
        self.reconnect = request.map(|request| ReconnectContext {
            backend: Arc::clone(core.backend()),
            request,
            policy: None,
            attempt: 0,
        });
        self
    }

    /// Enable automatic reconnects using a caller-owned policy and timer.
    ///
    /// Returns [`Error::RequestConstruction`] when the stream holds no request it can resend,
    /// because the operation's request could not be cloned (a streaming request body, for example).
    pub fn with_reconnect(mut self, policy: Arc<dyn ReconnectPolicy>) -> Result<Self, StreamError> {
        let Some(reconnect) = self.reconnect.as_mut() else {
            return Err(Error::request_message(
                "this streaming request cannot be cloned safely for automatic reconnect",
            ));
        };
        reconnect.policy = Some(policy);
        Ok(self)
    }

    /// The latest SSE event ID, including an empty ID that resets replay state. An automatic
    /// reconnect sends it as `Last-Event-ID` when it is a valid header value; an empty ID, or one
    /// holding a control character a header value refuses, reconnects without that header.
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
    /// does not abandon the rest. An item that passes `ClientConfig::max_stream_frame` before its
    /// terminator arrives ends the stream instead, as that limit documents.
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
                    let item = deserialize_item::<T>(this.status, &this.headers, &payload);
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
                // Every complete item is framed out first, so what is left is one item still
                // waiting for its terminator. At EOF the framer has already flushed it.
                FramePoll::NeedMore if this.buffer.len() > this.max_frame => {
                    return Poll::Ready(Some(Err(this.frame_overflow())));
                }
                FramePoll::NeedMore => {}
            }

            match std::mem::replace(&mut this.state, StreamState::Done) {
                StreamState::Body(mut body) => match body.poll_chunk(cx) {
                    Poll::Pending => {
                        this.state = StreamState::Body(body);
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(Some(chunk))) => {
                        this.state = StreamState::Body(body);
                        this.buffer.extend_from_slice(&chunk);
                        if this.bom_pending {
                            this.bom_pending = this.buffer.strip_leading_bom();
                        }
                    }
                    // Whatever class the read failure carries, the connection is gone partway
                    // through the body, so it is the interruption a reconnect policy is asked
                    // about.
                    Poll::Ready(Err(error)) => {
                        let error = Error::InterruptedBody(error);
                        if let Some(error) =
                            this.schedule_reconnect(OwnedReconnectReason::Failure(error))
                        {
                            return Poll::Ready(Some(Err(error)));
                        }
                    }
                    Poll::Ready(Ok(None)) => this.state = StreamState::Eof,
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
                        let (parts, body) = response.into_parts();
                        this.status = parts.status;
                        this.headers = parts.headers;
                        this.bom_pending = is_sse(this.framing);
                        this.state = StreamState::Body(streamed(body));
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
    /// End the stream over an item that passed `max_frame` before its terminator, returning the
    /// `Decode` error that reports it. The connection is dropped with the body stream, and no
    /// reconnect is attempted: the server that sent the item is likely to send it again.
    fn frame_overflow(&mut self) -> StreamError {
        self.state = StreamState::Done;
        // `truncated` is set whatever the cap: the item itself is incomplete.
        let (body, _) = cap_body(self.buffer.take_bytes(), self.max_error_body);
        Error::Decode {
            status: self.status,
            headers: self.headers.clone(),
            path: format!(
                "stream item exceeds max_stream_frame ({} bytes) before its terminator",
                self.max_frame
            ),
            body,
            truncated: true,
        }
    }

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
            return self.stop(reason);
        };
        let Some(policy) = reconnect.policy.as_ref() else {
            return self.stop(reason);
        };
        let reason_ref = match &reason {
            OwnedReconnectReason::EndOfStream => ReconnectReason::EndOfStream,
            OwnedReconnectReason::Failure(error) => ReconnectReason::Failure(error),
        };
        let Some(wait) = policy.reconnect(reconnect.attempt, reason_ref, self.reconnect_delay)
        else {
            return self.stop(reason);
        };
        reconnect.attempt = reconnect.attempt.saturating_add(1);
        // A partial event belongs to the closed connection and must never be concatenated with the
        // first bytes from the replay connection.
        self.buffer.clear();
        self.state = StreamState::Waiting(wait);
        None
    }

    /// End the stream without reconnecting: a clean end of stream yields nothing further, and a
    /// failure is handed back to be yielded.
    fn stop(&mut self, reason: OwnedReconnectReason) -> Option<StreamError> {
        self.state = StreamState::Done;
        match reason {
            OwnedReconnectReason::EndOfStream => None,
            OwnedReconnectReason::Failure(error) => Some(error),
        }
    }

    fn reconnect_future(&self) -> Option<ReconnectFuture> {
        let reconnect = self.reconnect.as_ref()?;
        let backend = Arc::clone(&reconnect.backend);
        let max_error_body = self.max_error_body;
        let mut request = try_clone_request(&reconnect.request)?;
        // An empty id resets replay state, so the header goes. So does an id `HeaderValue` refuses
        // (the framer drops an id holding NUL, and keeps every other control character but the
        // line-ending LF and CR): the server's latest resume point cannot be sent, and replaying
        // an older one would claim a position it has superseded, so the reconnect proceeds as a
        // fresh subscription instead of ending.
        if let Some(id) = self.last_event_id.as_deref() {
            match reqwest::header::HeaderValue::from_str(id) {
                Ok(value) if !id.is_empty() => {
                    request.headers_mut().insert("last-event-id", value);
                }
                _ => {
                    request.headers_mut().remove("last-event-id");
                }
            }
        }
        Some(Box::pin(async move {
            let response = send_on(backend.as_ref(), request).await?;
            if response.status().is_success() {
                Ok(response)
            } else {
                Err(unexpected_status_capped(max_error_body, response).await)
            }
        }))
    }
}

/// The body a stream reads its items from. Off `wasm32` that is the response body as the backend
/// delivered it. On `wasm32`, a `fetch` response `ReqwestBackend` received, still unread, is read
/// chunk by chunk as the browser delivers it, through reqwest's `stream` feature, which an API
/// with a streaming response requires; `ReqwestBackend` alone would read it whole, which for an
/// event stream that never ends would never yield an item.
fn streamed(body: HttpBody) -> HttpBody {
    #[cfg(target_arch = "wasm32")]
    return match body.into_fetch_response() {
        Ok(response) => HttpBody::wrap(FetchChunks(Box::pin(response.bytes_stream()))),
        Err(body) => body,
    };
    #[cfg(not(target_arch = "wasm32"))]
    body
}

/// A `fetch` response body read chunk by chunk. A failed read is yielded as reqwest reported it,
/// which the stream reads as an interruption whatever its kind.
// Mutation testing: replacing `FetchChunks`'s `poll_frame` survives the native suite and is
// declared rather than killed. It is compiled only for `wasm32`, so no native test reaches it; on
// `wasm32` the runtime is compile-checked, not tested.
#[cfg(target_arch = "wasm32")]
struct FetchChunks(Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>>>>);

#[cfg(target_arch = "wasm32")]
impl http_body::Body for FetchChunks {
    type Data = Bytes;
    type Error = reqwest::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, reqwest::Error>>> {
        self.0
            .as_mut()
            .poll_next(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(http_body::Frame::data)))
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

/// Bytes read but not yet framed, with how far the framer has already looked into them, so a
/// frame arriving over many chunks is searched once rather than from its start on every chunk.
/// Framed bytes leave the front through `BytesMut::split_to`, which moves no bytes, so framing
/// many items out of one buffer costs nothing per item for what stays behind them.
///
/// The raw bytes of the item in progress stay in `bytes` until it is framed, so `len` is what
/// `ClientConfig::max_stream_frame` measures and what an over-limit item's error reports.
#[derive(Debug, Default)]
struct FrameBuffer {
    bytes: BytesMut,
    /// SSE only: the offset just past the last complete line of the event in progress. Every line
    /// before it has already been read into `event`.
    line_start: usize,
    /// Where the next terminator search starts: the bytes before it (from `line_start`, for SSE)
    /// hold none of the terminators the framing in use looks for.
    scanned: usize,
    /// SSE only: the fields the lines before `line_start` set.
    event: SseFields,
}

impl FrameBuffer {
    fn extend_from_slice(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Drop every byte and everything read from them.
    fn clear(&mut self) {
        self.bytes.clear();
        self.line_start = 0;
        self.scanned = 0;
        self.event = SseFields::default();
    }

    /// Take every byte, leaving the buffer empty.
    fn take_bytes(&mut self) -> Bytes {
        let bytes = std::mem::take(&mut self.bytes).freeze();
        self.clear();
        bytes
    }

    /// Split off the first `at` bytes, which the caller has framed. The search offsets restart at
    /// the new front; `event` is the caller's to take.
    fn consume(&mut self, at: usize) -> BytesMut {
        self.line_start = 0;
        self.scanned = 0;
        self.bytes.split_to(at)
    }

    /// [`strip_leading_bom`] over the bytes. The search restarts at the front, since a mark the
    /// bytes lost moves every offset.
    fn strip_leading_bom(&mut self) -> bool {
        self.line_start = 0;
        self.scanned = 0;
        self.event = SseFields::default();
        strip_leading_bom(&mut self.bytes)
    }
}

/// Frame the next item out of `buffer`, consuming the bytes it uses. `at_eof` is `true` once no
/// further bytes will arrive, which lets a trailing frame not terminated by a delimiter still be
/// emitted. Pure: no IO, no async.
fn next_frame(buffer: &mut FrameBuffer, framing: Framing, at_eof: bool) -> FramePoll {
    match framing {
        Framing::Ndjson => ndjson_next(buffer, at_eof),
        Framing::Sse => sse_next(buffer, at_eof, SseOutput::LegacyData),
        Framing::SseEvent => sse_next(buffer, at_eof, SseOutput::Envelope),
        Framing::SseJsonData => sse_next(buffer, at_eof, SseOutput::JsonData),
        Framing::JsonSequence => json_sequence_next(buffer, at_eof),
    }
}

/// The ASCII Record Separator that opens each RFC 7464 record.
const RECORD_SEPARATOR: u8 = 0x1e;

/// The index of the first `byte` in `buffer` at or after `from`.
fn find_from(buffer: &[u8], from: usize, byte: u8) -> Option<usize> {
    buffer[from..]
        .iter()
        .position(|&b| b == byte)
        .map(|index| from + index)
}

/// ndjson framing: each complete `\n`-terminated line (CRLF tolerated) is one item; empty lines are
/// skipped. At EOF a final line without a trailing newline is emitted.
fn ndjson_next(buffer: &mut FrameBuffer, at_eof: bool) -> FramePoll {
    loop {
        if let Some(nl) = find_from(&buffer.bytes, buffer.scanned, b'\n') {
            let line = buffer.consume(nl + 1);
            let line = trim_cr(&line[..nl]);
            if line.is_empty() {
                continue;
            }
            return frame_item(line.to_vec());
        }
        if at_eof {
            let line = trim_cr(&buffer.bytes).to_vec();
            buffer.clear();
            if line.is_empty() {
                return FramePoll::NeedMore;
            }
            return frame_item(line);
        }
        buffer.scanned = buffer.len();
        return FramePoll::NeedMore;
    }
}

/// RFC 7464 framing: each item starts with ASCII Record Separator (`0x1E`) and runs until the next
/// separator (or EOF). Newlines are record terminators/formatting and are trimmed around the JSON
/// text, while embedded newlines remain valid JSON whitespace.
fn json_sequence_next(buffer: &mut FrameBuffer, at_eof: bool) -> FramePoll {
    loop {
        // Bytes before the first separator are discarded once it arrives, and held until then.
        if buffer.bytes.first() != Some(&RECORD_SEPARATOR) {
            let Some(first) = find_from(&buffer.bytes, buffer.scanned, RECORD_SEPARATOR) else {
                if at_eof {
                    buffer.clear();
                } else {
                    buffer.scanned = buffer.len();
                }
                return FramePoll::NeedMore;
            };
            buffer.consume(first);
        }
        // The record's own separator is at 0, so the search for the next one starts past it.
        let end = find_from(&buffer.bytes, buffer.scanned.max(1), RECORD_SEPARATOR);
        let Some(end) = end.or(at_eof.then_some(buffer.len())) else {
            buffer.scanned = buffer.len();
            return FramePoll::NeedMore;
        };
        let record = buffer.consume(end);
        let payload = record[1..].trim_ascii();
        // An empty record frames nothing; keep going so every complete record behind it is
        // framed before the caller reads more bytes or measures what is left.
        if !payload.is_empty() {
            return frame_item(payload.to_vec());
        }
    }
}

/// Whether `framing` reads its body as Server-Sent Events, whose stream may open with a UTF-8 byte
/// order mark.
fn is_sse(framing: Framing) -> bool {
    matches!(
        framing,
        Framing::Sse | Framing::SseEvent | Framing::SseJsonData
    )
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

#[derive(Debug, Default)]
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
/// The lines of the event in progress are read into `buffer.event` as they complete, so a later
/// call resumes after the last complete line instead of reading the event again from its start.
fn sse_next(buffer: &mut FrameBuffer, at_eof: bool, output: SseOutput) -> FramePoll {
    loop {
        let terminator = loop {
            match take_line(&buffer.bytes, buffer.line_start, buffer.scanned) {
                Ok((line, next)) => {
                    buffer.line_start = next;
                    buffer.scanned = next;
                    if line.is_empty() {
                        break Some(next);
                    }
                    append_sse_field(line, &mut buffer.event);
                }
                Err(scanned) => {
                    buffer.scanned = scanned;
                    break None;
                }
            }
        };
        let fields = match terminator {
            Some(end) => {
                buffer.consume(end);
                std::mem::take(&mut buffer.event)
            }
            None if !at_eof => return FramePoll::NeedMore,
            None => {
                // Mutation testing: replacing `<` with `<=` survives every test and is declared
                // rather than killed: at `line_start == len` the field appended is the empty line,
                // whose empty name matches no field, so `append_sse_field` leaves the event as it
                // was.
                if buffer.line_start < buffer.len() {
                    append_sse_field(
                        trim_cr(&buffer.bytes[buffer.line_start..]),
                        &mut buffer.event,
                    );
                }
                let fields = std::mem::take(&mut buffer.event);
                buffer.clear();
                fields
            }
        };
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

/// If `buf[from..]` holds a complete SSE line, return it without its terminator and the index just
/// past that terminator. `scanned`, at or past `from`, is where the search for the terminator
/// starts: `buf[from..scanned]` is known to hold none. As the SSE specification's grammar
/// requires, a line ends at CRLF, at a bare LF, or at a bare CR. Returns `Err` with the offset the
/// next search starts from when no full line is buffered yet, which includes a CR that is the last
/// buffered byte: the byte after it decides whether it ends the line alone or begins a CRLF, so it
/// is held, and searched again, until that byte arrives (at EOF, `sse_next` takes the tail
/// itself).
fn take_line(buf: &[u8], from: usize, scanned: usize) -> Result<(&[u8], usize), usize> {
    let Some(end) = buf[scanned..]
        .iter()
        .position(|&b| b == b'\n' || b == b'\r')
        .map(|index| scanned + index)
    else {
        return Err(buf.len());
    };
    let next = match (buf[end], buf.get(end + 1)) {
        (b'\r', None) => return Err(end),
        (b'\r', Some(b'\n')) => end + 2,
        _ => end + 1,
    };
    Ok((&buf[from..end], next))
}

/// The UTF-8 byte order mark an SSE body may open with, which the SSE specification strips.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Strip a UTF-8 byte order mark from the start of a freshly opened SSE body. Returns `true` while
/// `buffer` is still a proper prefix of the mark (empty included), so the decision waits for more
/// bytes, and `false` once it is made, whether or not a mark was removed.
fn strip_leading_bom(buffer: &mut BytesMut) -> bool {
    if buffer.len() < UTF8_BOM.len() && UTF8_BOM.starts_with(buffer) {
        return true;
    }
    if buffer.starts_with(UTF8_BOM) {
        let _ = buffer.split_to(UTF8_BOM.len());
    }
    false
}

/// `line` without one trailing `\r`, so a CRLF line ending reads the same as a bare `\n` to the
/// ndjson framer, and an SSE tail at EOF ending in a CR loses that line ending.
fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Decode one framed JSON payload into `T`. A parse failure becomes [`Error::Decode`] carrying the
/// status and headers of the response the frame was read from, serde's error message, and the raw
/// frame — never a silent skip. The headers are cloned only on failure.
fn deserialize_item<T: DeserializeOwned>(
    status: StatusCode,
    headers: &HeaderMap,
    payload: &[u8],
) -> Result<T, Error<Infallible>> {
    serde_json::from_slice::<T>(payload).map_err(|error| Error::Decode {
        status,
        headers: headers.clone(),
        path: error.to_string(),
        body: Bytes::copy_from_slice(payload),
        truncated: false,
    })
}

#[cfg(test)]
mod tests;
