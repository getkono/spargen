//! A response body read as it arrives, on every target. Off `wasm32` that is how every backend
//! body is read already. On `wasm32`, `ReqwestBackend` hands a `fetch` response over still unread,
//! and its own body reads it whole; this module reads it chunk by chunk instead, as the browser
//! delivers it, through reqwest's `stream` feature and `futures-core`. An `EventStream` reads its
//! body through it, and so does a binary operation's `_stream` twin, whose `ByteStream` would
//! otherwise hold the whole body before its first chunk.

#[cfg(target_arch = "wasm32")]
use std::pin::Pin;
#[cfg(target_arch = "wasm32")]
use std::task::{Context, Poll};

#[cfg(target_arch = "wasm32")]
use bytes::Bytes;
#[cfg(target_arch = "wasm32")]
use futures_core::Stream;

use crate::HttpBody;

/// `body`, read as it arrives. Off `wasm32` that is the body as the backend delivered it. On
/// `wasm32`, a `fetch` response `ReqwestBackend` received, still unread, is read chunk by chunk as
/// the browser delivers it; any other body comes back unchanged.
pub fn read_as_it_arrives(body: HttpBody) -> HttpBody {
    #[cfg(target_arch = "wasm32")]
    return match body.into_fetch_response() {
        Ok(response) => HttpBody::wrap(FetchChunks(Box::pin(response.bytes_stream()))),
        Err(body) => body,
    };
    #[cfg(not(target_arch = "wasm32"))]
    body
}

/// A `fetch` response body read chunk by chunk. A failed read is yielded as reqwest reported it,
/// the query taken off the URL it carries, which `HttpBody::wrap` reads as an interruption
/// whatever its kind.
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
        self.0.as_mut().poll_next(cx).map(|chunk| {
            chunk.map(|chunk| {
                chunk
                    .map(http_body::Frame::data)
                    .map_err(crate::error::without_query)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::read_as_it_arrives;
    use crate::tests::{poll_ready, Chunks};

    /// Issue #1305: off `wasm32` a backend's body is already read as it arrives, so it comes back
    /// as it went in, its chunks unmerged.
    #[test]
    fn a_native_body_is_read_as_it_was_delivered() {
        let mut body = read_as_it_arrives(Chunks::repeat(3, 2).into_response(200).into_body());
        assert_eq!(poll_ready(body.chunk()).unwrap().unwrap(), "xxx");
        assert_eq!(poll_ready(body.chunk()).unwrap().unwrap(), "xxx");
        assert!(poll_ready(body.chunk()).unwrap().is_none());
    }
}
