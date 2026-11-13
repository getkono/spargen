//! # support-runtime
//!
//! The freestanding runtime support code shipped *inside* every spargen-generated client. It is real,
//! standalone-compilable source (compiled and linted here in its own right) that spargen embeds
//! into generated output: its `support` subsystem carries each file via `include_str!`, and
//! `codegen` emits the text above each file's test module, with `crate::` rewritten to
//! `super::`.
//!
//! No spargen crate ever appears in a consumer's runtime graph: this crate is `publish = false`.
//! The dependency set a generated client needs, and what each conditionally embedded module adds
//! to it, is stated in the book's runtime chapter (`docs/book/src/runtime.md`) and printed for one
//! spec by `spargen deps`; here, each optional dependency sits behind the feature that gates its
//! module, and the comment on that module's `mod` below names it.
//!
//! ## Fault-tolerance guarantees
//!
//! The generated client never panics on network input; every failure maps to one [`Error`]
//! variant. Taxonomy class **#10, cancellation**, is a *documented drop-safety guarantee* rather
//! than an enum variant: dropping a returned future is safe and side-effect-free beyond standard
//! HTTP semantics. The other eleven classes are the constructed [`Error`] variants.

#![forbid(unsafe_code)]
// The API contract returns `Result<ResponseValue<T>, Error<E>>` with `Error` unboxed, and the
// taxonomy deliberately retains headers/bodies for forensics — so `Error` is intentionally
// large and is passed by value rather than boxed.
#![allow(clippy::result_large_err)]
mod auth;
// The `contentEncoding: base64` codec needs no dependency beyond `bytes` and `serde`; a generated
// client embeds this module only when a schema it emits carries that keyword.
mod base64;
// The blocking facade owns a current-thread tokio runtime, so it pulls in the optional `tokio`
// dependency and is compiled only under the `blocking` feature on a non-`wasm32` target, the same
// `cfg` a generated client gates its embedded copy on: nothing tokio-related is referenced unless
// the consumer opts in, and a wasm build never reaches it.
#[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
mod blocking;
// Reading a `wasm32` `fetch` response as it arrives pulls in the optional `futures-core` dependency
// and reqwest's `stream` feature, so it is compiled only under the `stream` feature; a generated
// client embeds this module only when its spec has a sequential response or a binary `_stream`
// twin, the two readers that read a body as it arrives.
#[cfg(feature = "stream")]
mod chunked;
mod client;
// The RFC 3339 `DateTime`/`Date` newtypes pull in the optional `time` dependency, so they are
// compiled only under the `time` feature; a generated client embeds this module only when its spec
// has a `format: date-time` or `format: date`.
#[cfg(feature = "time")]
mod datetime;
// The same two newtypes over `chrono` and `jiff`, the other typed date-time backends, each compiled
// under the feature of its crate. A generated client embeds the one its backend selects, in place of
// `datetime`, under that module's name; here they sit beside it under their own names, reached by
// path, so every backend is built and tested together.
#[cfg(feature = "chrono")]
pub mod datetime_chrono;
#[cfg(feature = "jiff")]
pub mod datetime_jiff;
mod dispatch;
mod error;
mod header;
mod middleware;
mod paginate;
mod parameter;
mod request;
mod response;
mod retry;
// The RFC 3339 codec the `chrono` and `jiff` backends share; a generated client embeds it beside
// either of them.
#[cfg(any(feature = "chrono", feature = "jiff"))]
mod rfc3339;
// Typed streaming of sequential response bodies pulls in the optional `futures-core` dependency
// and reqwest's `stream` feature, so it is compiled only under the `stream` feature; a generated
// client embeds this module only when its spec has a sequential (SSE, ndjson or JSON-seq) response.
#[cfg(feature = "stream")]
mod stream;
mod transport;
// The `MaybeSend`/`MaybeSync` conditional-bound abstraction. Compiled on every target (it is
// `Send`/`Sync` on native and vacuous on `wasm32`), so the transport seam and its helpers carry one
// set of bounds that builds both natively and on the browser `fetch` backend.
mod wasm;
// The XML codec pulls in the optional `quick-xml` dependency, so it is compiled only under the
// `xml` feature. A generated client embeds this module only when its spec uses an XML body.
#[cfg(feature = "xml")]
mod xml;

pub use auth::{
    attach_auth, attach_auth_replayable, replay_rejected, token_provider, AuthError, AuthKind,
    AuthScheme, Credential, ExposeSecret, RejectionCheck, Replay, SecretString, TokenFuture,
    TokenProvider, TokenRefresher,
};
pub use base64::{deserialize_base64, serialize_base64};
#[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
pub use blocking::BlockingRuntime;
#[cfg(feature = "stream")]
pub use chunked::read_as_it_arrives;
pub use client::{ClientConfig, ClientCore, RequestOptions};
#[cfg(feature = "time")]
pub use datetime::{Date, DateParseError, DateTime};
pub use dispatch::{
    build_url_on, build_url_with_query_string_on, classify_error, classify_error_bytes,
    classify_error_text, classify_with, decode_success, decode_success_bytes, decode_success_text,
    decode_success_with, decode_text_body, execute, prepare, send, send_streaming,
    unexpected_status, with_body_limits, StatusSpec,
};
pub use error::{
    ApiErrorBody, ApiErrorProblem, Error, ProblemDetails, ProtocolError, RedirectError,
    RequestCause, RequestError, TimeoutKind, TransportError, UnknownVariant,
};
pub use header::{parse_header, require_header, HeaderError, HeaderShape};
pub use middleware::{Middleware, MiddlewareBackend, Next};
pub use paginate::{next_link, LinkPaginator};
pub use parameter::{
    encode, serialize_deep_object, serialize_delimited, serialize_form, serialize_form_body,
    serialize_label, serialize_matrix, serialize_multipart_values, serialize_simple, Delimiter,
    FormMode, FormProperty, FormStyle, PercentEncoding,
};
pub use request::{MultipartForm, MultipartPart, RequestBuilder};
pub use response::{stream_success_bytes, ByteStream, ResponseValue};
pub use retry::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};
#[cfg(feature = "stream")]
pub use stream::{
    EventStream, Framing, ReconnectPolicy, ReconnectReason, ReconnectWait, StreamError,
};
pub use transport::{
    try_clone_request, ExecuteFuture, HttpBackend, HttpBody, ReqwestBackend, TotalTimeout,
};
pub use wasm::{MaybeSend, MaybeSync};
#[cfg(feature = "xml")]
pub use xml::{classify_error_xml, decode_success_xml, decode_xml_body, to_xml};

// Shared by every module's tests. `lib.rs` is never embedded (the generated `support` module
// replaces it), and this module is its last item, so the embed's split on the test marker in each
// runtime source is unaffected.
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use reqwest::Method;

    use crate::transport::response_from_reqwest;
    use crate::{
        ClientCore, ExecuteFuture, HttpBackend, HttpBody, RequestBuilder, RetryOutcome,
        RetryPolicy, RetryWait, TransportError,
    };

    /// A response as the transport seam carries it.
    pub(crate) type Response = http::Response<HttpBody>;

    /// Drives `future` with one poll and a noop waker, panicking if it is not ready. The runtime's
    /// tests use no async runtime: their transports answer from canned in-memory responses and
    /// their waits are already-ready futures, so the first poll completes them.
    pub(crate) fn poll_ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future was not immediately ready"),
        }
    }

    /// Drives `future` to completion with a noop waker, polling again after every `Pending`, and
    /// returns its output with how many polls answered `Pending`. Each fake that answers `Pending`
    /// ([`PendingOnce`], [`Chunk::Pending`], [`Reply::pending`]) is ready on the following poll, so
    /// a bounded number of re-polls completes the future; one that is still pending after that is
    /// a hang, and fails the test instead of spinning.
    pub(crate) fn poll_to_completion<F: Future>(future: F) -> (F::Output, usize) {
        const MAX_POLLS: usize = 1_000;
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for pending in 0..MAX_POLLS {
            if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return (value, pending);
            }
        }
        panic!("future still pending after {MAX_POLLS} polls");
    }

    /// A future that answers its first poll with `Pending` and every later one as `inner` does,
    /// so a test can reach the branch a caller takes while it waits.
    pub(crate) struct PendingOnce<F> {
        polled: bool,
        inner: Pin<Box<F>>,
    }

    impl<F: Future> PendingOnce<F> {
        pub(crate) fn new(inner: F) -> Self {
            Self {
                polled: false,
                inner: Box::pin(inner),
            }
        }
    }

    impl<F: Future> Future for PendingOnce<F> {
        type Output = F::Output;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
            if !self.polled {
                self.polled = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.inner.as_mut().poll(cx)
        }
    }

    /// A `ClientCore` on the default transport. Tests that never send through it, or that build
    /// absolute request URLs, need nothing more specific.
    pub(crate) fn core() -> ClientCore {
        ClientCore::new("https://example.com").expect("a valid base URL")
    }

    /// A builder for a `method` request to the absolute `url`.
    pub(crate) fn builder(method: Method, url: &str) -> RequestBuilder {
        RequestBuilder::new(method, url::Url::parse(url).expect("a valid URL"))
    }

    /// A `GET https://example.com/op` as the transport seam carries it.
    pub(crate) fn request() -> http::Request<HttpBody> {
        builder(Method::GET, "https://example.com/op")
            .build()
            .expect("build request")
    }

    /// [`request`], under the name the transport-level tests read it by.
    pub(crate) fn seam_request() -> http::Request<HttpBody> {
        request()
    }

    /// An in-memory reqwest response (no server, no runtime) with `status` and `body`.
    pub(crate) fn reqwest_response(
        status: u16,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(body.into())
                .expect("valid synthetic response"),
        )
    }

    /// An `http::Response` as reqwest would have received it, crossed into the seam the way
    /// `ReqwestBackend` hands one over, so its body reads through reqwest.
    pub(crate) fn from_http<T: Into<reqwest::Body>>(response: http::Response<T>) -> Response {
        response_from_reqwest(reqwest::Response::from(response))
    }

    /// An in-memory response with `status` and `body`, crossed into the seam the way
    /// `ReqwestBackend` hands one over, so its body reads through reqwest.
    pub(crate) fn response(status: u16, body: impl Into<reqwest::Body>) -> Response {
        response_from_reqwest(reqwest_response(status, body))
    }

    /// The two `GET` requests reqwest refuses at `execute`, without any I/O, as a builder-kind
    /// error: a URL whose scheme is not `http`/`https`, and plain `http` on a client configured
    /// `https_only`. Each comes with the client that refuses it.
    pub(crate) fn refused_at_execute() -> [(reqwest::Client, http::Request<HttpBody>); 2] {
        let https_only = reqwest::Client::builder()
            .https_only(true)
            .build()
            .expect("build an https-only client");
        [
            (reqwest::Client::new(), "ftp://example.com/op"),
            (https_only, "http://example.com/op"),
        ]
        .map(|(client, url)| {
            let request = RequestBuilder::new(Method::GET, url::Url::parse(url).expect("a URL"))
                .build()
                .expect("the request builds; reqwest refuses it only at execute");
            (client, request)
        })
    }

    /// A transport failure that `TransportError::from_reqwest` classifies as a connection failure,
    /// and so as transient, built without a socket: reqwest's status-kind error, which falls
    /// through to the same class a failed connection reaches. `status` appears in its message, so
    /// a test can tell two failures apart.
    pub(crate) fn transport_failure(status: u16) -> TransportError {
        TransportError::from_reqwest(
            reqwest_response(status, String::new())
                .error_for_status()
                .expect_err("an error status"),
        )
    }

    /// One step of a scripted response body.
    pub(crate) enum Chunk {
        /// A chunk of body bytes.
        Data(Bytes),
        /// One poll that answers `Pending`; the next poll moves on to the following step. Only the
        /// `stream` module's tests script one, so it is unused without that feature.
        #[cfg_attr(not(feature = "stream"), allow(dead_code))]
        Pending,
        /// A failed read, as a dropped connection reports it.
        Fail,
    }

    impl Chunk {
        pub(crate) fn data(bytes: impl Into<Vec<u8>>) -> Self {
            Self::Data(Bytes::from(bytes.into()))
        }
    }

    /// A response body that plays a script of [`Chunk`]s, then ends, after a trailers frame where
    /// one was given. It counts the data chunks read, so a test can see where a reader stopped
    /// pulling. It is a `futures_core::Stream` for reqwest to wrap and an `http_body::Body` for
    /// `HttpBody::wrap` to adopt.
    pub(crate) struct Chunks {
        script: VecDeque<Chunk>,
        pulled: Arc<AtomicUsize>,
        trailers: Option<http::HeaderMap>,
    }

    impl Chunks {
        pub(crate) fn new(script: impl IntoIterator<Item = Chunk>) -> Self {
            Self {
                script: script.into_iter().collect(),
                pulled: Arc::new(AtomicUsize::new(0)),
                trailers: None,
            }
        }

        /// End the body with a trailers frame holding `trailers`.
        pub(crate) fn with_trailers(mut self, trailers: http::HeaderMap) -> Self {
            self.trailers = Some(trailers);
            self
        }

        /// `count` chunks of `size` bytes of `x`.
        pub(crate) fn repeat(size: usize, count: usize) -> Self {
            Self::new((0..count).map(|_| Chunk::data(vec![b'x'; size])))
        }

        /// The count of data chunks read so far, which stays readable after the body is consumed.
        pub(crate) fn pulled(&self) -> Arc<AtomicUsize> {
            Arc::clone(&self.pulled)
        }

        /// A response with `status` whose body plays this script, read through reqwest.
        pub(crate) fn into_response(self, status: u16) -> Response {
            response(status, reqwest::Body::wrap_stream(self))
        }
    }

    impl http_body::Body for Chunks {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Bytes>, std::io::Error>>> {
            if self.script.is_empty() {
                return Poll::Ready(
                    self.trailers
                        .take()
                        .map(|t| Ok(http_body::Frame::trailers(t))),
                );
            }
            futures_core::Stream::poll_next(self, cx)
                .map(|chunk| chunk.map(|chunk| chunk.map(http_body::Frame::data)))
        }
    }

    impl futures_core::Stream for Chunks {
        type Item = Result<Bytes, std::io::Error>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.get_mut();
            match this.script.pop_front() {
                None => Poll::Ready(None),
                Some(Chunk::Data(bytes)) => {
                    this.pulled.fetch_add(1, Ordering::SeqCst);
                    Poll::Ready(Some(Ok(bytes)))
                }
                Some(Chunk::Pending) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Some(Chunk::Fail) => Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "connection reset mid-body",
                )))),
            }
        }
    }

    /// What a [`SequenceBackend`] answers one request with.
    #[derive(Debug, Clone)]
    pub(crate) struct Reply {
        outcome: Result<(u16, String), u16>,
        headers: Vec<(&'static str, String)>,
        pending: bool,
    }

    impl Reply {
        /// A response with `status` and `body`.
        pub(crate) fn status(status: u16, body: &str) -> Self {
            Self {
                outcome: Ok((status, body.to_owned())),
                headers: Vec::new(),
                pending: false,
            }
        }

        /// The [`transport_failure`] for `status`.
        pub(crate) fn failure(status: u16) -> Self {
            Self {
                outcome: Err(status),
                headers: Vec::new(),
                pending: false,
            }
        }

        /// Add a response header.
        pub(crate) fn header(mut self, name: &'static str, value: &str) -> Self {
            self.headers.push((name, value.to_owned()));
            self
        }

        /// Answer the first poll of the execute future with `Pending`. Only the `stream` module's
        /// tests ask for it, so it is unused without that feature.
        #[cfg_attr(not(feature = "stream"), allow(dead_code))]
        pub(crate) fn pending(mut self) -> Self {
            self.pending = true;
            self
        }
    }

    /// A transport that answers each request with the next [`Reply`] of its script, the last one
    /// repeating once the script is spent, and records every request it is handed. Each response
    /// also carries `x-response: backend`, so a test can tell it from one built by hand.
    #[derive(Debug)]
    pub(crate) struct SequenceBackend {
        script: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<Sent>>,
    }

    /// One request a [`SequenceBackend`] was handed: its URL, its headers, and its body where
    /// that is in memory.
    type Sent = (String, HeaderMap, Option<Vec<u8>>);

    impl SequenceBackend {
        pub(crate) fn new(script: impl IntoIterator<Item = Reply>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            })
        }

        /// One `status` response with an empty body per entry.
        pub(crate) fn statuses(statuses: impl IntoIterator<Item = u16>) -> Arc<Self> {
            Self::new(statuses.into_iter().map(|status| Reply::status(status, "")))
        }

        /// How many requests it has been handed.
        pub(crate) fn calls(&self) -> usize {
            self.requests.lock().unwrap().len()
        }

        /// The URL of every request, in order.
        pub(crate) fn urls(&self) -> Vec<String> {
            let requests = self.requests.lock().unwrap();
            requests.iter().map(|(url, ..)| url.clone()).collect()
        }

        /// Header `name` of every request, in order, `None` where a request lacked it.
        pub(crate) fn header(&self, name: &str) -> Vec<Option<String>> {
            let requests = self.requests.lock().unwrap();
            requests
                .iter()
                .map(|(_, headers, _)| {
                    headers
                        .get(name)
                        .map(|value| value.to_str().expect("a visible-ASCII header").to_owned())
                })
                .collect()
        }

        /// The body of every request, in order, `None` where a request's body was a stream.
        pub(crate) fn bodies(&self) -> Vec<Option<Vec<u8>>> {
            let requests = self.requests.lock().unwrap();
            requests.iter().map(|(.., body)| body.clone()).collect()
        }
    }

    impl HttpBackend for SequenceBackend {
        fn execute(&self, request: http::Request<HttpBody>) -> ExecuteFuture<'_> {
            self.requests.lock().unwrap().push((
                request.uri().to_string(),
                request.headers().clone(),
                request.body().as_bytes().map(<[u8]>::to_vec),
            ));
            let Reply {
                outcome,
                headers,
                pending,
            } = {
                let mut script = self.script.lock().unwrap();
                if script.len() > 1 {
                    script.pop_front()
                } else {
                    script.front().cloned()
                }
                .expect("a SequenceBackend holds at least one reply")
            };
            let reply = async move {
                match outcome {
                    Ok((status, body)) => {
                        let mut builder = http::Response::builder()
                            .status(status)
                            .header("x-response", "backend");
                        for (name, value) in headers {
                            builder = builder.header(name, value);
                        }
                        Ok(builder
                            .body(HttpBody::from(body))
                            .expect("valid synthetic response"))
                    }
                    Err(status) => Err(transport_failure(status)),
                }
            };
            if pending {
                Box::pin(PendingOnce::new(reply))
            } else {
                Box::pin(reply)
            }
        }
    }

    /// Retries every transient outcome until `max_retries` retries have been made, with an
    /// already-ready wait standing in for the caller's timer.
    pub(crate) struct RetryTransient {
        pub(crate) max_retries: u32,
    }

    impl RetryPolicy for RetryTransient {
        fn retry<'a>(&'a self, attempt: u32, outcome: &RetryOutcome<'_>) -> Option<RetryWait<'a>> {
            (attempt < self.max_retries && outcome.is_transient())
                .then(|| Box::pin(std::future::ready(())) as RetryWait<'a>)
        }
    }
}
