//! Typed streaming for response bodies delivered as Server-Sent Events (`text/event-stream`) or
//! newline-delimited JSON (`application/x-ndjson`). A streaming operation returns an
//! [`EventStream<T>`] — a *manual* async iterator that yields decoded items one at a time.
//!
//! The iterator is deliberately futures-free: it holds the live [`reqwest::Response`] and pulls
//! bytes incrementally with [`reqwest::Response::chunk`] (which needs no reqwest `stream` feature
//! and no `futures` crate), buffers them, and frames complete items out of the buffer. It does
//! *not* implement `futures::Stream` — that would pull in `futures_core`, outside the runtime's
//! fixed dependency set. Callers drive it with a plain `while let Some(item) = stream.next().await`.
//!
//! On `wasm32` (reqwest's browser `fetch` backend), `chunk()` is unavailable, so the buffer is
//! filled by reading the whole body once with [`reqwest::Response::bytes`] and framing it from
//! memory (the browser buffers the full response regardless). The `EventStream` API and yielded
//! items are identical; only the delivery is non-incremental there.
//!
//! Framing is a pure function ([`next_frame`]) over an owned byte buffer, so the framing/decoding
//! logic is unit-testable without any network IO or async runtime — [`EventStream::next`] is a thin
//! await-chunk loop around it.

use std::convert::Infallible;
use std::marker::PhantomData;

use bytes::Bytes;
use reqwest::Response;
use serde::de::DeserializeOwned;

use crate::Error;

/// How a streaming response body is framed into individual items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Server-Sent Events (`text/event-stream`): events separated by a blank line; the `data:`
    /// field lines within an event are concatenated into one JSON payload. A `data: [DONE]`
    /// sentinel terminates the stream.
    Sse,
    /// Newline-delimited JSON (`application/x-ndjson`): one JSON item per non-empty line.
    Ndjson,
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
    /// The live response while bytes remain; `None` once the body is exhausted or terminated.
    response: Option<Response>,
    /// Bytes read but not yet framed into a complete item. Partial frames live here between chunks.
    buffer: Vec<u8>,
    /// The framing mode for this body.
    framing: Framing,
    /// Once set, the stream is finished and [`Self::next`] returns `None` without further IO.
    done: bool,
    /// `T` is produced, never consumed; the `fn() -> T` marker keeps `EventStream<T>: Send + Sync`
    /// regardless of `T`.
    _marker: PhantomData<fn() -> T>,
}

impl<T> EventStream<T> {
    /// Wrap a streaming response with the framing mode chosen for its media type. The response is
    /// consumed lazily — no bytes are read until the first [`Self::next`] call.
    pub fn new(response: Response, framing: Framing) -> Self {
        Self {
            response: Some(response),
            buffer: Vec::new(),
            framing,
            done: false,
            _marker: PhantomData,
        }
    }
}

impl<T: DeserializeOwned> EventStream<T> {
    /// Yield the next decoded item, or `None` at end of stream. A mid-stream transport error
    /// surfaces as `Some(Err(..))` and terminates the stream (subsequent calls return `None`). A
    /// per-frame decode failure also surfaces as `Some(Err(Error::Decode { .. }))` rather than being
    /// silently skipped, but does NOT terminate the stream — the next call resumes framing the
    /// following items, so a single malformed event does not abandon the rest.
    ///
    /// The item type `T` is decoded per-frame with `serde_json::from_slice`.
    pub async fn next(&mut self) -> Option<Result<T, Error<Infallible>>> {
        if self.done {
            return None;
        }
        loop {
            let at_eof = self.response.is_none();
            match next_frame(&mut self.buffer, self.framing, at_eof) {
                FramePoll::Item(payload) => return Some(deserialize_item::<T>(&payload)),
                FramePoll::Done => {
                    self.done = true;
                    self.response = None;
                    self.buffer.clear();
                    return None;
                }
                FramePoll::NeedMore => {
                    if at_eof {
                        // No complete frame and the body is exhausted: the stream is over.
                        self.done = true;
                        return None;
                    }
                    // Pull more bytes into the buffer (or reach EOF). A transport failure mid-stream
                    // surfaces as `Some(Err(..))` and terminates the stream.
                    if let Err(error) = self.pull().await {
                        self.done = true;
                        self.response = None;
                        return Some(Err(error));
                    }
                }
            }
        }
    }

    /// Read more bytes into `self.buffer`, or drop `self.response` at end of body so the next frame
    /// pass runs with `at_eof`. On native the reqwest backend streams incrementally via
    /// [`reqwest::Response::chunk`]. `chunk` is not available on reqwest's `wasm32` `fetch` backend,
    /// which has no incremental read; see the wasm variant below.
    #[cfg(not(target_arch = "wasm32"))]
    async fn pull(&mut self) -> Result<(), Error<Infallible>> {
        // `next` only calls `pull` when the response is present. Still handle an exhausted state
        // explicitly: runtime state transitions must remain panic-free even if this helper is
        // rearranged in the future.
        let Some(response) = self.response.as_mut() else {
            return Ok(());
        };
        match response.chunk().await {
            Ok(Some(chunk)) => self.buffer.extend_from_slice(&chunk),
            // Clean EOF: drop the response so the next loop reframes with `at_eof`, flushing any
            // trailing complete frame.
            Ok(None) => self.response = None,
            Err(error) => return Err(Error::from_reqwest(error)),
        }
        Ok(())
    }

    /// The `wasm32` buffer fill: reqwest's browser `fetch` backend exposes no incremental `chunk()`,
    /// so read the whole body once with [`reqwest::Response::bytes`] and frame it from memory. The
    /// browser buffers the full response regardless, so the same items are yielded — just not
    /// incrementally (a documented wasm limitation). Dropping the response signals EOF to the next
    /// frame pass, which then flushes every buffered item.
    #[cfg(target_arch = "wasm32")]
    async fn pull(&mut self) -> Result<(), Error<Infallible>> {
        // See the native variant: exhausted state is harmless and must never become a panic.
        let Some(response) = self.response.take() else {
            return Ok(());
        };
        let bytes = response.bytes().await.map_err(Error::from_reqwest)?;
        self.buffer.extend_from_slice(&bytes);
        Ok(())
    }
}

/// The outcome of attempting to frame one item out of the buffer.
#[derive(Debug, PartialEq, Eq)]
enum FramePoll {
    /// A complete JSON payload was framed and removed from the buffer.
    Item(Vec<u8>),
    /// A terminator was reached (SSE `[DONE]` sentinel): the stream ends.
    Done,
    /// No complete frame is available yet; read more bytes (or, at EOF, the stream ends).
    NeedMore,
}

/// Frame the next item out of `buffer`, consuming the bytes it uses. `at_eof` is `true` once no
/// further bytes will arrive, which lets a trailing frame not terminated by a delimiter still be
/// emitted. Pure: no IO, no async — the unit tests drive it directly.
fn next_frame(buffer: &mut Vec<u8>, framing: Framing, at_eof: bool) -> FramePoll {
    match framing {
        Framing::Ndjson => ndjson_next(buffer, at_eof),
        Framing::Sse => sse_next(buffer, at_eof),
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
            return FramePoll::Item(line);
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
            return FramePoll::Item(line);
        }
        return FramePoll::NeedMore;
    }
}

/// SSE framing: accumulate lines until a blank line terminates an event, concatenating the event's
/// `data:` field lines into one payload. Comment lines (`:`…) and non-`data` fields are ignored. A
/// `data: [DONE]` payload ends the stream. At EOF a final event not closed by a blank line is
/// flushed. Only a full event (or an EOF flush) consumes bytes, so a partial event is retained.
fn sse_next(buffer: &mut Vec<u8>, at_eof: bool) -> FramePoll {
    loop {
        let mut pos = 0;
        let mut data: Vec<u8> = Vec::new();
        let mut saw_terminator = false;
        while let Some((line, next)) = take_line(buffer, pos) {
            pos = next;
            if line.is_empty() {
                saw_terminator = true;
                break;
            }
            append_data_line(line, &mut data);
        }
        if !saw_terminator {
            if at_eof {
                // Flush a final event: fold any last line lacking a trailing newline into `data`.
                if pos < buffer.len() {
                    let mut line = &buffer[pos..];
                    if line.last() == Some(&b'\r') {
                        line = &line[..line.len() - 1];
                    }
                    if !line.is_empty() {
                        append_data_line(line, &mut data);
                    }
                }
                buffer.clear();
                return finish_event(data);
            }
            // Wait for the blank-line terminator before consuming anything.
            return FramePoll::NeedMore;
        }
        buffer.drain(..pos);
        match finish_event(data) {
            // An event with no `data:` field (e.g. only comments or a keep-alive) dispatches
            // nothing; keep framing the next event rather than yielding an empty item.
            FramePoll::NeedMore => continue,
            other => return other,
        }
    }
}

/// Turn an event's assembled `data` payload into a poll result: empty → nothing to dispatch, the
/// `[DONE]` sentinel → end of stream, otherwise the payload (its single trailing `\n` stripped).
fn finish_event(mut data: Vec<u8>) -> FramePoll {
    if data.is_empty() {
        return FramePoll::NeedMore;
    }
    if data.last() == Some(&b'\n') {
        data.pop();
    }
    if data == b"[DONE]" {
        return FramePoll::Done;
    }
    FramePoll::Item(data)
}

/// Append one SSE line's contribution to the event `data`. A comment line (leading `:`) or a
/// non-`data` field (`event:`/`id:`/`retry:`/unknown) contributes nothing; a `data:` line appends
/// its value (one optional leading space stripped) followed by a `\n`, per the SSE data model.
fn append_data_line(line: &[u8], data: &mut Vec<u8>) {
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
        // A field name with no colon carries an empty value (per the SSE grammar).
        None => (line, &b""[..]),
    };
    if field == b"data" {
        data.extend_from_slice(value);
        data.push(b'\n');
    }
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
    use super::{next_frame, EventStream, FramePoll, Framing};
    use crate::Error;

    /// Frame every item currently extractable from `buffer` under `framing`, stopping at the first
    /// `NeedMore` (partial frame retained) or `Done` (terminated). Returns the framed payloads as
    /// strings plus whether a `Done` terminator was hit.
    fn drain(buffer: &mut Vec<u8>, framing: Framing, at_eof: bool) -> (Vec<String>, bool) {
        let mut items = Vec::new();
        loop {
            match next_frame(buffer, framing, at_eof) {
                FramePoll::Item(payload) => items.push(String::from_utf8(payload).unwrap()),
                FramePoll::Done => return (items, true),
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
