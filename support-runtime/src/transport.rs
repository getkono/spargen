//! The transport seam: how a prepared [`http::Request`] is executed into an [`http::Response`],
//! both carrying the runtime's own body type, [`HttpBody`]. Everything else — URL building, auth
//! attachment, decode, streaming, pagination — operates on the request/response *around* this one
//! step, so swapping the backend swaps only the execute step and leaves the rest of the runtime
//! untouched.
//!
//! The seam is a `dyn`-able trait ([`HttpBackend`]) so the generated `Client` stays non-generic:
//! [`crate::ClientCore`] holds an `Arc<dyn HttpBackend>` rather than a type parameter. The async
//! method returns a manually boxed future ([`ExecuteFuture`], `Send` off wasm) instead of using
//! `async-trait`, so std's `Future`/`Pin`/`Box` carry the abstraction.
//!
//! The currency is the `http` crate's request and response types, so a backend need not be built
//! on reqwest. Failures are reported as a [`crate::TransportError`] carrying the taxonomy class
//! its constructor names; [`crate::send`] maps it to that class's [`crate::Error`] variant.
//! [`ReqwestBackend`] adapts a `reqwest::Client` to the seam in both directions and classifies its
//! failures with [`crate::TransportError::from_reqwest`], so timeout/redirect/protocol
//! classification is identical to executing directly on the reqwest client.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use http_body::{Body as _, Frame, SizeHint};

use crate::wasm::BoxFuture;
use crate::{MaybeSend, MaybeSync, TransportError};

/// The boxed future [`HttpBackend::execute`] returns: an executed [`http::Response`] or a
/// [`TransportError`]. The `'a` lifetime ties the future to the borrow of `&self`, so a backend may
/// borrow its own fields while the request runs (as [`ReqwestBackend`] borrows its client); a
/// `'static` future fits too, since a longer lifetime coerces to the shorter one the trait
/// requires.
///
/// The future is `Send` off `wasm32` and not on it, since reqwest's `fetch`-backed futures are
/// `!Send` on the single-threaded browser target; the `wasm` module explains the gating.
pub type ExecuteFuture<'a> = BoxFuture<'a, Result<http::Response<HttpBody>, TransportError>>;

/// The swappable HTTP transport: it executes a prepared [`http::Request`] into an
/// [`http::Response`], each carrying an [`HttpBody`].
///
/// This is the seam the retry adapter, the middleware interceptor, and the WASM build all rest on:
/// implement it to wrap, replace, or retry the execute step without touching URL building, auth,
/// decode, streaming, or pagination. The default [`ReqwestBackend`] runs the request on a wrapped
/// `reqwest::Client`.
///
/// Implementations must be [`MaybeSend`] + [`MaybeSync`] — `Send + Sync` on native (the `Client` is
/// shared across tasks), vacuous on `wasm32` — and `Debug` (so [`crate::ClientCore`] stays `Debug`).
/// The returned future is manually boxed so the trait is object-safe without an `async-trait`
/// dependency.
pub trait HttpBackend: MaybeSend + MaybeSync + std::fmt::Debug {
    /// Execute a prepared request, yielding the raw response or a transport failure. The request's
    /// URI is absolute. A backend that fails builds its [`TransportError`] with the constructor
    /// naming the failure's class, so [`crate::send`] maps it to the right [`crate::Error`]
    /// variant.
    ///
    /// A request carrying a [`TotalTimeout`] extension is bounded by it: from the moment `execute`
    /// is called until the whole response body has been read. A deadline that elapses fails the
    /// call, or the body read in progress, with a [`TransportError::timeout`] of
    /// [`crate::TimeoutKind::Total`]. [`crate::send`] puts the client's
    /// [`crate::ClientConfig::timeout`] there, so a backend that ignores the extension leaves the
    /// call unbounded. [`ReqwestBackend`] makes it reqwest's own request timeout. A backend that
    /// executes the request more than once through an inner one bounds each execution, not their
    /// sum: [`crate::RetryBackend`] hands every attempt the same extension and awaits its policy's
    /// backoff between them, so a retried call may take each attempt's timeout plus the backoffs.
    fn execute(&self, request: http::Request<HttpBody>) -> ExecuteFuture<'_>;
}

/// The cause a body source may fail with, before it is read as a [`TransportError`].
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A streaming body behind the seam: `Send + Sync` off `wasm32` and neither on it, as the `wasm`
/// module explains. `Sync` is what lets [`ReqwestBackend`] hand a streaming request body to
/// reqwest, which requires it.
#[cfg(not(target_arch = "wasm32"))]
type BoxBody = Pin<Box<dyn http_body::Body<Data = Bytes, Error = TransportError> + Send + Sync>>;
/// A streaming body behind the seam (the wasm variant: neither `Send` nor `Sync`).
#[cfg(target_arch = "wasm32")]
type BoxBody = Pin<Box<dyn http_body::Body<Data = Bytes, Error = TransportError>>>;

/// The body of a request or response crossing the transport seam: bytes held in memory, or a
/// stream of them read as an [`http_body::Body`] whose failures are [`TransportError`]s.
///
/// An in-memory body clones ([`Self::try_clone`]), which is what lets [`crate::RetryBackend`]
/// resend a request and `EventStream` reconnect one; a streaming body is read once.
/// [`Self::wrap`] adopts any `http_body::Body`, so a backend built on another HTTP stack hands its
/// response body over without copying it.
pub struct HttpBody {
    inner: Inner,
}

enum Inner {
    /// Bytes held in memory: read as one data frame, then the end of the body.
    Full(Bytes),
    /// A body read frame by frame.
    Streaming(BoxBody),
    /// A response body reqwest's `fetch` backend received.
    #[cfg(target_arch = "wasm32")]
    Fetch(FetchBody),
}

impl HttpBody {
    /// A body with no bytes.
    pub fn empty() -> Self {
        Self::from(Bytes::new())
    }

    /// Adopt a streaming body. Its data frames are read as `Bytes`; a failure that is a
    /// [`TransportError`] keeps its class, and any other is
    /// [an interruption](TransportError::interrupted).
    pub fn wrap<B>(body: B) -> Self
    where
        B: http_body::Body + MaybeSend + MaybeSync + 'static,
        B::Data: Into<Bytes>,
        B::Error: Into<BoxError>,
    {
        Self {
            inner: Inner::Streaming(Box::pin(SourceBody {
                inner: Box::pin(body),
            })),
        }
    }

    /// The bytes of an in-memory body; `None` for a streaming one.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.inner {
            Inner::Full(bytes) => Some(bytes),
            Inner::Streaming(_) => None,
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(_) => None,
        }
    }

    /// A copy of an in-memory body, sharing its bytes; `None` for a streaming one, which can be
    /// read only once.
    pub fn try_clone(&self) -> Option<Self> {
        match &self.inner {
            Inner::Full(bytes) => Some(Self::from(bytes.clone())),
            Inner::Streaming(_) => None,
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(_) => None,
        }
    }

    /// The next chunk of data, or `None` once the body has ended. Frames that carry no data
    /// (trailers) are skipped.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>, TransportError> {
        std::future::poll_fn(|cx| self.poll_chunk(cx)).await
    }

    /// [`Self::chunk`] as a poll, for a reader that is itself polled (`EventStream`).
    pub(crate) fn poll_chunk(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, TransportError>> {
        loop {
            match Pin::new(&mut *self).poll_frame(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(None)),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        return Poll::Ready(Ok(Some(data)));
                    }
                }
            }
        }
    }

    /// The whole body. An in-memory body is handed over without a copy, and so is a streaming
    /// one that arrives as a single chunk.
    pub(crate) async fn collect(self) -> Result<Bytes, TransportError> {
        // Nothing held in memory exceeds `usize::MAX` bytes, so the body is always within it.
        Ok(self.collect_within(usize::MAX).await?.unwrap_or_default())
    }

    /// The whole body, or `None` once it is known to hold more than `limit` bytes: reading stops
    /// at the chunk that would carry it past `limit`, which is never buffered, so at most `limit`
    /// bytes are. An in-memory body is handed over without a copy, and so is a streaming one that
    /// arrives as a single chunk.
    pub(crate) async fn collect_within(
        mut self,
        limit: usize,
    ) -> Result<Option<Bytes>, TransportError> {
        if let Inner::Full(bytes) = self.inner {
            return Ok((bytes.len() <= limit).then_some(bytes));
        }
        let Some(first) = self.chunk().await? else {
            return Ok(Some(Bytes::new()));
        };
        if first.len() > limit {
            return Ok(None);
        }
        let Some(second) = self.chunk().await? else {
            return Ok(Some(first));
        };
        let mut length = first.len().saturating_add(second.len());
        if length > limit {
            return Ok(None);
        }
        let mut buffered = BytesMut::with_capacity(length);
        buffered.extend_from_slice(&first);
        buffered.extend_from_slice(&second);
        while let Some(chunk) = self.chunk().await? {
            length = length.saturating_add(chunk.len());
            if length > limit {
                return Ok(None);
            }
            buffered.extend_from_slice(&chunk);
        }
        Ok(Some(buffered.freeze()))
    }

    /// A body reqwest reads, read through [`TransportError::from_reqwest_body`].
    #[cfg(not(target_arch = "wasm32"))]
    fn from_reqwest_stream(body: reqwest::Body) -> Self {
        Self {
            inner: Inner::Streaming(Box::pin(ReqwestBody { inner: body })),
        }
    }

    /// The `fetch` response a body reqwest's `fetch` backend received still holds, before anything
    /// has read it; any other body comes back unchanged. The `allow` is for a build without the
    /// `stream` module, its only caller.
    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub(crate) fn into_fetch_response(self) -> Result<reqwest::Response, Self> {
        match self.inner {
            Inner::Fetch(FetchBody::Unread(response)) => Ok(response),
            inner => Err(Self { inner }),
        }
    }

    /// The body as reqwest sends it. reqwest's `fetch` backend on `wasm32` takes no stream, so
    /// there a streaming body is refused as a request-kind failure.
    fn into_reqwest(self) -> Result<reqwest::Body, TransportError> {
        match self.inner {
            Inner::Full(bytes) => Ok(reqwest::Body::from(bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            Inner::Streaming(body) => Ok(reqwest::Body::wrap(body)),
            #[cfg(target_arch = "wasm32")]
            Inner::Streaming(_) | Inner::Fetch(_) => Err(TransportError::request(
                "reqwest's fetch backend cannot send a streaming request body",
            )),
        }
    }
}

impl http_body::Body for HttpBody {
    type Data = Bytes;
    type Error = TransportError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        match &mut self.inner {
            Inner::Full(bytes) if bytes.is_empty() => Poll::Ready(None),
            Inner::Full(bytes) => Poll::Ready(Some(Ok(Frame::data(std::mem::take(bytes))))),
            Inner::Streaming(body) => body.as_mut().poll_frame(cx),
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.inner {
            Inner::Full(bytes) => bytes.is_empty(),
            Inner::Streaming(body) => body.is_end_stream(),
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(_) => false,
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.inner {
            Inner::Full(bytes) => SizeHint::with_exact(bytes.len() as u64),
            Inner::Streaming(body) => body.size_hint(),
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(_) => SizeHint::default(),
        }
    }
}

impl Default for HttpBody {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for HttpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Full(bytes) => f.debug_tuple("HttpBody").field(&bytes.len()).finish(),
            Inner::Streaming(_) => f.write_str("HttpBody(streaming)"),
            #[cfg(target_arch = "wasm32")]
            Inner::Fetch(_) => f.write_str("HttpBody(fetch)"),
        }
    }
}

impl From<Bytes> for HttpBody {
    fn from(bytes: Bytes) -> Self {
        Self {
            inner: Inner::Full(bytes),
        }
    }
}

impl From<Vec<u8>> for HttpBody {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from(Bytes::from(bytes))
    }
}

impl From<String> for HttpBody {
    fn from(text: String) -> Self {
        Self::from(Bytes::from(text))
    }
}

impl From<&'static str> for HttpBody {
    fn from(text: &'static str) -> Self {
        Self::from(Bytes::from_static(text.as_bytes()))
    }
}

impl From<&'static [u8]> for HttpBody {
    fn from(bytes: &'static [u8]) -> Self {
        Self::from(Bytes::from_static(bytes))
    }
}

/// An adopted body: its data read as `Bytes`, and its failures as [`TransportError`]s.
struct SourceBody<B> {
    inner: Pin<Box<B>>,
}

impl<B> http_body::Body for SourceBody<B>
where
    B: http_body::Body,
    B::Data: Into<Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = TransportError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        self.inner.as_mut().poll_frame(cx).map(|frame| {
            frame.map(|frame| {
                frame
                    .map(|frame| frame.map_data(Into::into))
                    .map_err(|error| TransportError::from_body_source(error.into()))
            })
        })
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// A body reqwest reads, its failures classified by [`TransportError::from_reqwest_body`].
#[cfg(not(target_arch = "wasm32"))]
struct ReqwestBody {
    inner: reqwest::Body,
}

#[cfg(not(target_arch = "wasm32"))]
impl http_body::Body for ReqwestBody {
    type Data = Bytes;
    type Error = TransportError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        Pin::new(&mut self.inner)
            .poll_frame(cx)
            .map(|frame| frame.map(|frame| frame.map_err(TransportError::from_reqwest_body)))
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// A response body reqwest's `fetch` backend reads whole, its failure classified by
/// [`TransportError::from_reqwest_body`]. Until it is first polled it holds the response itself,
/// which the `stream` module takes back to read chunk by chunk as the browser delivers them
/// ([`HttpBody::into_fetch_response`]), since only an API with a streaming response carries the
/// dependencies that read needs.
// Mutation testing: replacing `HttpBody::into_fetch_response` or `FetchBody`'s `poll_frame`
// survives the native suite and is declared rather than killed. Both are compiled only for
// `wasm32`, so no native test reaches them; on `wasm32` the runtime is compile-checked, not
// tested.
#[cfg(target_arch = "wasm32")]
enum FetchBody {
    /// Not read yet.
    Unread(reqwest::Response),
    /// Being read whole.
    Reading(BoxFuture<'static, Result<Bytes, reqwest::Error>>),
    /// Read, or failed.
    Done,
}

#[cfg(target_arch = "wasm32")]
impl http_body::Body for FetchBody {
    type Data = Bytes;
    type Error = TransportError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        loop {
            match std::mem::replace(&mut *self, FetchBody::Done) {
                FetchBody::Unread(response) => {
                    *self = FetchBody::Reading(Box::pin(response.bytes()));
                }
                FetchBody::Reading(mut whole) => {
                    return match std::future::Future::poll(whole.as_mut(), cx) {
                        Poll::Pending => {
                            *self = FetchBody::Reading(whole);
                            Poll::Pending
                        }
                        Poll::Ready(read) => Poll::Ready(Some(
                            read.map(Frame::data)
                                .map_err(TransportError::from_reqwest_body),
                        )),
                    };
                }
                FetchBody::Done => return Poll::Ready(None),
            }
        }
    }
}

/// The total timeout of one request, from connecting until its whole response body has been
/// read, carried among the request's extensions. [`crate::send`] gives a request without one the
/// client's [`crate::ClientConfig::timeout`] this way, and [`ReqwestBackend`] enforces it; a
/// backend on another HTTP stack reads it from the request it executes and bounds the exchange by
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotalTimeout(pub std::time::Duration);

/// A copy of `request` to send again, its method, URI, version, headers, extensions and body
/// alike; `None` when its body is a stream, which can be read only once. A streaming operation
/// keeps one of the request it sends, to reconnect.
pub fn try_clone_request(request: &http::Request<HttpBody>) -> Option<http::Request<HttpBody>> {
    let mut clone = http::Request::new(request.body().try_clone()?);
    *clone.method_mut() = request.method().clone();
    *clone.uri_mut() = request.uri().clone();
    *clone.version_mut() = request.version();
    *clone.headers_mut() = request.headers().clone();
    *clone.extensions_mut() = request.extensions().clone();
    Some(clone)
}

/// A request from the seam, as reqwest sends it, its [`TotalTimeout`] become reqwest's own request
/// timeout. The URI is parsed back into a URL; one that does not parse is refused as a
/// request-kind failure, before anything is sent.
fn request_into_reqwest(
    request: http::Request<HttpBody>,
) -> Result<reqwest::Request, TransportError> {
    let (parts, body) = request.into_parts();
    let timeout = parts
        .extensions
        .get::<TotalTimeout>()
        .map(|timeout| timeout.0);
    let body = body.into_reqwest()?;
    let mut request = reqwest::Request::try_from(http::Request::from_parts(parts, body))
        .map_err(TransportError::from_reqwest)?;
    if timeout.is_some() {
        *request.timeout_mut() = timeout;
    }
    Ok(request)
}

/// A response reqwest received, as the seam carries it. Off `wasm32` the body is reqwest's own,
/// read as it arrives; on `wasm32` it is the `fetch` response's, read whole unless `EventStream`
/// takes it back to stream it. One function for both targets, with the target-specific steps
/// `cfg`-gated inside it, so a native test reaches it and no mutation of it survives by being
/// compiled out.
pub(crate) fn response_from_reqwest(
    #[allow(unused_mut)] mut response: reqwest::Response,
) -> http::Response<HttpBody> {
    #[cfg(not(target_arch = "wasm32"))]
    return http::Response::<reqwest::Body>::from(response).map(HttpBody::from_reqwest_stream);
    #[cfg(target_arch = "wasm32")]
    {
        let status = response.status();
        let headers = std::mem::take(response.headers_mut());
        let mut converted = http::Response::new(HttpBody {
            inner: Inner::Fetch(FetchBody::Unread(response)),
        });
        *converted.status_mut() = status;
        *converted.headers_mut() = headers;
        converted
    }
}

/// The default backend: executes requests on a wrapped [`reqwest::Client`], adapting each request
/// from the seam to reqwest and each response back. This is what `Client::new` and
/// `Client::with_client` install.
///
/// An in-memory request body moves to reqwest without a copy off `wasm32`, and a streaming one
/// is streamed; reqwest's `fetch` backend on `wasm32` sends no streaming body, and refuses one as
/// a request-kind failure. A failure to execute is classified by
/// [`TransportError::from_reqwest`], and one while reading the response body by the rules
/// [`crate::Error::InterruptedBody`] and [`crate::Error::Protocol`] describe.
#[derive(Debug, Clone)]
pub struct ReqwestBackend {
    client: reqwest::Client,
}

impl ReqwestBackend {
    /// Wrap a `reqwest::Client` as the transport backend.
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// The wrapped client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

impl HttpBackend for ReqwestBackend {
    fn execute(&self, request: http::Request<HttpBody>) -> ExecuteFuture<'_> {
        Box::pin(async move {
            let request = request_into_reqwest(request)?;
            let response = self
                .client
                .execute(request)
                .await
                .map_err(TransportError::from_reqwest)?;
            Ok(response_from_reqwest(response))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use http_body::Body as _;
    use reqwest::Method;

    use super::{request_into_reqwest, response_from_reqwest, TotalTimeout};
    use crate::tests::{poll_ready, request, reqwest_response, Chunk, Chunks, SequenceBackend};
    use crate::{
        send, ClientCore, Error, HttpBackend, HttpBody, RequestBuilder, ReqwestBackend,
        TransportError,
    };

    // On native, ClientCore and the seam trait objects/futures are Send + Sync, so the client
    // works under tokio::spawn / axum (#21). A `+ Send` boxed future or trait-object bound turned
    // non-Send would still compile in the single-threaded tests and example but break
    // multi-threaded use; this catches it at compile time. (On wasm they are !Send, hence the cfg
    // gate.)
    #[cfg(not(target_arch = "wasm32"))]
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ClientCore>();
        assert_send_sync::<Arc<dyn HttpBackend>>();
        assert_send_sync::<ReqwestBackend>();
        assert_send_sync::<HttpBody>();
    };

    // Object-safe: a `ReqwestBackend` installs behind `dyn HttpBackend`, as `ClientCore` stores it.
    const _: fn(ReqwestBackend) -> Arc<dyn HttpBackend> = |backend| Arc::new(backend);

    /// `send` routes the execute step through the installed backend rather than a hard-wired
    /// `reqwest::Client`: the backend sees the request and its response is what `send` returns.
    #[test]
    fn send_routes_through_the_installed_backend() {
        let backend = SequenceBackend::statuses([203]);
        let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
        let response = poll_ready(send(&core, request())).unwrap();
        assert_eq!(response.status().as_u16(), 203);
        assert_eq!(backend.urls(), ["https://example.com/op"]);
    }

    /// `ClientCore::new` installs a `ReqwestBackend`, which `ClientCore`'s `Debug` names, since
    /// `Debug` is required of every backend.
    #[test]
    fn new_installs_a_reqwest_backend_that_debug_names() {
        let rendered = format!("{:?}", ReqwestBackend::new(reqwest::Client::new()));
        assert!(
            rendered.starts_with("ReqwestBackend { client: Client {"),
            "{rendered}"
        );
        let core = format!("{:?}", ClientCore::new("https://example.com").unwrap());
        assert!(
            core.contains("backend: ReqwestBackend { client: Client {"),
            "{core}"
        );
    }

    /// `client` hands back the wrapped client, not a default one: it still holds the default
    /// header the wrapped client was configured with, which `reqwest::Client`'s `Debug` renders.
    #[test]
    fn client_returns_the_wrapped_client() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-wrapped",
            reqwest::header::HeaderValue::from_static("yes"),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        let backend = ReqwestBackend::new(client);
        let rendered = format!("{:?}", backend.client());
        assert!(rendered.contains("x-wrapped"), "{rendered}");
        assert!(
            !format!("{:?}", reqwest::Client::default()).contains("x-wrapped"),
            "a default client carries no such header"
        );
    }

    /// A request from the seam reaches reqwest with its method, URL, headers and in-memory body
    /// intact, the body sharing its bytes rather than copied, and with no timeout of its own
    /// unless it carries a `TotalTimeout`, which becomes reqwest's request timeout.
    #[test]
    fn a_seam_request_crosses_into_reqwest_intact() {
        let bytes = Bytes::from_static(b"payload");
        let seam = RequestBuilder::new(
            Method::PUT,
            url::Url::parse("https://example.com/op?q=1").unwrap(),
        )
        .header("x-trace", "abc")
        .body(bytes.clone())
        .build()
        .unwrap();
        let back = request_into_reqwest(seam).unwrap();
        assert_eq!(back.method(), Method::PUT);
        assert_eq!(back.url().as_str(), "https://example.com/op?q=1");
        assert_eq!(back.headers()["x-trace"], "abc");
        let sent = back.body().and_then(reqwest::Body::as_bytes).unwrap();
        assert_eq!(sent, b"payload");
        assert_eq!(sent.as_ptr(), bytes.as_ptr());
        assert_eq!(back.timeout(), None);

        let mut timed = request();
        timed
            .extensions_mut()
            .insert(TotalTimeout(Duration::from_secs(7)));
        let back = request_into_reqwest(timed).unwrap();
        assert_eq!(back.timeout(), Some(&Duration::from_secs(7)));
    }

    /// A streaming request body reaches reqwest as a stream, its bytes all there when read.
    #[test]
    fn a_streaming_request_body_crosses_into_reqwest_as_a_stream() {
        let source = Chunks::new([Chunk::data("ab"), Chunk::data("cd")]);
        let mut seam = request();
        *seam.body_mut() = HttpBody::wrap(source);
        let mut back = request_into_reqwest(seam).unwrap();
        let body = back.body_mut().take().expect("a body");
        assert!(body.as_bytes().is_none());
        let body = HttpBody::wrap(body);
        assert_eq!(poll_ready(body.collect()).unwrap(), "abcd");
    }

    /// A request URI that is not a URL reqwest accepts is refused before anything is sent, as a
    /// request-kind failure.
    #[test]
    fn a_relative_request_uri_is_refused_as_request_construction() {
        let seam = http::Request::builder()
            .uri("/relative")
            .body(HttpBody::empty())
            .unwrap();
        let error = request_into_reqwest(seam).unwrap_err();
        assert!(matches!(
            Error::<()>::from_transport(error),
            Error::RequestConstruction(_)
        ));
    }

    /// A response reqwest received reaches the seam with its status and headers, and its body
    /// reads back whole.
    #[test]
    fn a_reqwest_response_crosses_the_seam_intact() {
        let mut received = reqwest_response(201, "created");
        received
            .headers_mut()
            .insert("x-trace", "abc".parse().unwrap());
        let seam = response_from_reqwest(received);
        assert_eq!(seam.status().as_u16(), 201);
        assert_eq!(seam.headers()["x-trace"], "abc");
        assert_eq!(poll_ready(seam.into_body().collect()).unwrap(), "created");
    }

    /// An in-memory body is one data frame and then the end; it reports its exact size, and a
    /// clone shares its bytes.
    #[test]
    fn an_in_memory_body_reads_as_one_frame() {
        let mut body = HttpBody::from("abc");
        assert_eq!(body.size_hint().exact(), Some(3));
        assert!(!body.is_end_stream());
        let clone = body.try_clone().unwrap();
        assert_eq!(poll_ready(body.chunk()).unwrap().unwrap(), "abc");
        assert!(body.is_end_stream());
        assert_eq!(poll_ready(body.chunk()).unwrap(), None);
        assert_eq!(poll_ready(clone.collect()).unwrap(), "abc");
        assert!(HttpBody::empty().is_end_stream());
        assert_eq!(poll_ready(HttpBody::default().collect()).unwrap(), "");
    }

    /// `collect_within` hands back a body of at most `limit` bytes, in memory or streamed in one
    /// chunk or several, and `None` for one past it, without reading past the crossing chunk.
    #[test]
    fn collect_within_reads_a_body_up_to_its_limit() {
        let read = |body: HttpBody, limit| poll_ready(body.collect_within(limit)).unwrap();
        assert_eq!(read(HttpBody::from("abc"), 3).unwrap(), "abc");
        assert_eq!(read(HttpBody::from("abc"), 2), None);
        assert_eq!(read(HttpBody::empty(), 0).unwrap(), "");
        let streamed = |chunks: &[&str]| {
            HttpBody::wrap(Chunks::new(chunks.iter().map(|chunk| Chunk::data(*chunk))))
        };
        assert_eq!(read(streamed(&[]), 0).unwrap(), "");
        assert_eq!(read(streamed(&["abcd"]), 4).unwrap(), "abcd");
        assert_eq!(read(streamed(&["abcd"]), 3), None);
        assert_eq!(read(streamed(&["ab", "cd"]), 4).unwrap(), "abcd");
        assert_eq!(read(streamed(&["ab", "cd"]), 3), None);
        assert_eq!(read(streamed(&["ab", "cd", "ef"]), 6).unwrap(), "abcdef");
        let crossing = Chunks::new([Chunk::data("ab"), Chunk::data("cd"), Chunk::data("ef")]);
        let pulled = crossing.pulled();
        assert_eq!(read(HttpBody::wrap(crossing), 5), None);
        assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 3);
        let failed = HttpBody::wrap(Chunks::new([Chunk::data("a"), Chunk::Fail]));
        assert!(poll_ready(failed.collect_within(8)).is_err());
    }

    /// Each `From` conversion holds exactly the bytes it was given.
    #[test]
    fn every_conversion_holds_its_bytes() {
        let bodies = [
            HttpBody::from(Bytes::from_static(b"x")),
            HttpBody::from(b"x".to_vec()),
            HttpBody::from("x".to_owned()),
            HttpBody::from("x"),
            HttpBody::from(&b"x"[..]),
        ];
        for body in bodies {
            assert_eq!(body.as_bytes(), Some(&b"x"[..]));
        }
    }

    /// An adopted body is read as it streams: chunks concatenate, a failure that is not a
    /// `TransportError` is an interruption, and one that is keeps its class.
    #[test]
    fn an_adopted_body_streams_and_classifies_its_failures() {
        let streamed = HttpBody::wrap(Chunks::new([Chunk::data("a"), Chunk::data("b")]));
        assert!(streamed.try_clone().is_none());
        assert_eq!(format!("{streamed:?}"), "HttpBody(streaming)");
        assert_eq!(poll_ready(streamed.collect()).unwrap(), "ab");

        let failed = HttpBody::wrap(Chunks::new([Chunk::data("a"), Chunk::Fail]));
        let error = poll_ready(failed.collect()).unwrap_err();
        assert!(matches!(
            Error::<()>::from_transport(error),
            Error::InterruptedBody(_)
        ));

        let classed = HttpBody::wrap(ClassedFailure);
        let error = poll_ready(classed.collect()).unwrap_err();
        assert!(matches!(
            Error::<()>::from_transport(error),
            Error::Protocol(_)
        ));
    }

    /// A body whose first poll fails with a protocol-class `TransportError`.
    struct ClassedFailure;

    impl http_body::Body for ClassedFailure {
        type Data = Bytes;
        type Error = TransportError;

        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<http_body::Frame<Bytes>, TransportError>>> {
            std::task::Poll::Ready(Some(Err(TransportError::protocol("malformed"))))
        }
    }

    /// Trailers carry no data, so a read skips them and goes on to the end of the body.
    #[test]
    fn a_trailers_frame_is_skipped() {
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-checksum", "1".parse().unwrap());
        let body = HttpBody::wrap(Chunks::new([Chunk::data("a")]).with_trailers(trailers));
        assert_eq!(poll_ready(body.collect()).unwrap(), "a");
    }
}
