use std::convert::Infallible;

use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::{Error, HttpBody};

/// A successful response body paired with its status and headers. Returned inside
/// `Result<ResponseValue<T>, Error<E>>` from every generated operation method.
#[derive(Debug, Clone)]
pub struct ResponseValue<T> {
    status: StatusCode,
    headers: HeaderMap,
    inner: T,
}

impl<T> ResponseValue<T> {
    /// Wrap a decoded body with its status and headers.
    pub fn new(status: StatusCode, headers: HeaderMap, inner: T) -> Self {
        Self {
            status,
            headers,
            inner,
        }
    }

    /// The response status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Consume the wrapper, yielding the decoded body.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Borrow the decoded body.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Map the body while preserving status and headers.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResponseValue<U> {
        ResponseValue {
            status: self.status,
            headers: self.headers,
            inner: f(self.inner),
        }
    }
}

/// A raw binary success body read as it arrives, which a binary operation's `_stream` twin
/// returns in place of the whole `Bytes` its buffered method reads.
///
/// [`Self::next`] yields the body a chunk at a time, and [`Self::collect_bounded`] reads the rest
/// of it into one `Bytes` within a limit the caller names; no collector reads it without one.
/// [`Self::into_body`] hands the body over as an [`HttpBody`], an `http_body::Body`, for a reader
/// of the caller's own. Dropping it abandons the rest of the body.
pub struct ByteStream {
    body: HttpBody,
    status: StatusCode,
    /// The `Content-Length` the response declared, until the first chunk is read.
    declared: Option<u64>,
}

impl ByteStream {
    /// The next chunk of the body, `None` once it has ended, or the failure that interrupted it.
    pub async fn next(&mut self) -> Option<Result<Bytes, Error<Infallible>>> {
        self.declared = None;
        self.body
            .chunk()
            .await
            .map_err(Error::from_transport)
            .transpose()
    }

    /// The rest of the body as one `Bytes`, or [`Error::BodyTooLarge`] once it is known to hold
    /// more than `limit` bytes: before anything is read, where nothing has been and the response
    /// declared a `Content-Length` past `limit`, and otherwise at the chunk that carries it past
    /// `limit`, which is never buffered. Nothing of an over-limit body is returned.
    pub async fn collect_bounded(self, limit: usize) -> Result<Bytes, Error<Infallible>> {
        if self
            .declared
            .is_some_and(|length| crate::dispatch::exceeds(length, limit))
        {
            return Err(Error::BodyTooLarge {
                limit,
                status: self.status,
            });
        }
        self.body
            .collect_within(limit)
            .await
            .map_err(Error::from_transport)?
            .ok_or(Error::BodyTooLarge {
                limit,
                status: self.status,
            })
    }

    /// The body as the transport seam carries it, for a reader of the caller's own.
    pub fn into_body(self) -> HttpBody {
        self.body
    }
}

impl std::fmt::Debug for ByteStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ByteStream")
            .field("body", &self.body)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Wrap a raw binary success response as a [`ByteStream`] beside its status and headers, reading
/// none of its body: the success branch of a binary operation's `_stream` twin.
pub fn stream_success_bytes(response: http::Response<HttpBody>) -> ResponseValue<ByteStream> {
    let (parts, body) = response.into_parts();
    let declared = crate::dispatch::declared_length(&parts.headers);
    ResponseValue::new(
        parts.status,
        parts.headers,
        ByteStream {
            body,
            status: parts.status,
            declared,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use reqwest::header::HeaderMap;
    use reqwest::StatusCode;

    use super::{stream_success_bytes, ByteStream, ResponseValue};
    use crate::tests::{poll_ready, Chunk, Chunks};
    use crate::Error;

    #[test]
    fn maps_body_without_losing_metadata() {
        // A present header, so "preserving headers" is observed rather than vacuous on an empty map.
        let mut headers = HeaderMap::new();
        headers.insert("x-trace", "abc".parse().unwrap());
        let value = ResponseValue::new(StatusCode::CREATED, headers, 41);
        let mapped = value.map(|value| value + 1);
        assert_eq!(mapped.status(), StatusCode::CREATED);
        assert_eq!(mapped.headers().len(), 1);
        assert_eq!(mapped.headers()["x-trace"], "abc");
        assert_eq!(*mapped.inner(), 42);
    }

    #[test]
    fn accessors_expose_status_headers_and_inner() {
        let mut headers = HeaderMap::new();
        headers.insert("x-trace", "abc".parse().unwrap());
        let value = ResponseValue::new(StatusCode::OK, headers, "body".to_owned());

        assert_eq!(value.status(), StatusCode::OK);
        assert_eq!(value.headers()["x-trace"], "abc");
        assert_eq!(value.inner(), "body");
        assert_eq!(value.into_inner(), "body");
    }

    /// A response whose body plays `chunks`, declaring `length` where one is given.
    fn streamed(chunks: Chunks, length: Option<usize>) -> ResponseValue<ByteStream> {
        let mut response = chunks.into_response(200);
        if let Some(length) = length {
            response
                .headers_mut()
                .insert(reqwest::header::CONTENT_LENGTH, length.into());
        }
        stream_success_bytes(response)
    }

    /// Issue #1263: the stream keeps the response's status and headers and yields the body chunk
    /// by chunk, then `None`.
    #[test]
    fn a_byte_stream_yields_each_chunk_then_ends() {
        let value = streamed(Chunks::repeat(3, 2), Some(6));
        assert_eq!(value.status(), StatusCode::OK);
        assert_eq!(value.headers()[reqwest::header::CONTENT_LENGTH], "6");
        let mut stream = value.into_inner();
        assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), "xxx");
        assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), "xxx");
        assert!(poll_ready(stream.next()).is_none());
        assert!(format!("{stream:?}").starts_with("ByteStream"));
    }

    /// Issue #1263: a failed read is the interrupted-body error, not the end of the body.
    #[test]
    fn a_byte_stream_reports_a_failed_read() {
        let mut stream = streamed(Chunks::new([Chunk::data("ab"), Chunk::Fail]), None).into_inner();
        assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), "ab");
        let error = poll_ready(stream.next()).unwrap().unwrap_err();
        assert!(matches!(error, Error::InterruptedBody(_)), "{error:?}");
    }

    /// Issue #1263: `collect_bounded` reads a body within its limit whole, the limit included.
    #[test]
    fn collect_bounded_reads_a_body_within_its_limit() {
        let stream = streamed(Chunks::repeat(4, 3), Some(12)).into_inner();
        assert_eq!(poll_ready(stream.collect_bounded(12)).unwrap().len(), 12);
    }

    /// Issue #1263: a declared `Content-Length` past the limit is refused before any of the body
    /// is read.
    #[test]
    fn collect_bounded_refuses_a_declared_length_past_its_limit_unread() {
        let chunks = Chunks::repeat(4, 3);
        let pulled = chunks.pulled();
        let stream = streamed(chunks, Some(12)).into_inner();
        let error = poll_ready(stream.collect_bounded(11)).unwrap_err();
        assert!(
            matches!(error, Error::BodyTooLarge { limit: 11, status } if status == 200),
            "{error:?}"
        );
        assert_eq!(pulled.load(Ordering::SeqCst), 0);
    }

    /// Issue #1263: an undeclared body is abandoned at the chunk that carries it past the limit,
    /// and nothing after it is read.
    #[test]
    fn collect_bounded_abandons_a_body_at_the_chunk_crossing_its_limit() {
        let chunks = Chunks::repeat(4, 10);
        let pulled = chunks.pulled();
        let stream = streamed(chunks, None).into_inner();
        let error = poll_ready(stream.collect_bounded(10)).unwrap_err();
        assert!(
            matches!(error, Error::BodyTooLarge { limit: 10, .. }),
            "{error:?}"
        );
        assert_eq!(pulled.load(Ordering::SeqCst), 3);
    }

    /// Issue #1263: once a chunk has been read, the declared length no longer describes what is
    /// left, so `collect_bounded` bounds the rest as it reads it.
    #[test]
    fn collect_bounded_after_a_read_bounds_only_the_rest() {
        let mut stream = streamed(Chunks::repeat(4, 3), Some(12)).into_inner();
        assert_eq!(poll_ready(stream.next()).unwrap().unwrap().len(), 4);
        assert_eq!(poll_ready(stream.collect_bounded(8)).unwrap().len(), 8);
    }

    /// Issue #1263: the body handed over is the rest of the response's body.
    #[test]
    fn into_body_hands_over_the_rest_of_the_body() {
        let mut stream = streamed(Chunks::repeat(2, 2), None).into_inner();
        assert_eq!(poll_ready(stream.next()).unwrap().unwrap(), "xx");
        assert_eq!(poll_ready(stream.into_body().collect()).unwrap(), "xx");
    }
}
