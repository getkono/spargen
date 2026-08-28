//! # support-runtime
//!
//! The freestanding runtime support code shipped *inside* every spargen-generated client: the
//! dispatch routines, the error taxonomy, [`ResponseValue<T>`], and auth plumbing. It is real,
//! standalone-compilable source (compiled and linted here in its own right) that the
//! `codegen` subsystem embeds verbatim via `include_str!`.
//!
//! No spargen crate ever appears in a consumer's runtime graph: this crate is `publish = false`
//! and its only dependencies are the near-universal `reqwest` / `serde` / `serde_json` / `bytes`
//! / `secrecy` set plus `futures-core` for APIs that emit streaming operations.
//!
//! ## Fault-tolerance guarantees
//!
//! The generated client never panics on network input; every failure maps to one [`Error`]
//! variant. Taxonomy class **#10, cancellation**, is a *documented drop-safety guarantee* rather
//! than an enum variant: dropping a returned future is safe and side-effect-free beyond standard
//! HTTP semantics. The other nine classes are the constructed [`Error`] variants.

#![forbid(unsafe_code)]
// The API contract returns `Result<ResponseValue<T>, Error<E>>` with `Error` unboxed, and the
// taxonomy deliberately retains headers/bodies for forensics — so `Error` is intentionally
// large and is passed by value rather than boxed.
#![allow(clippy::result_large_err)]
mod auth;
// The blocking facade owns a current-thread tokio runtime, so it pulls in the optional `tokio`
// dependency and is compiled only under the `blocking` feature. A generated client embeds this
// module unconditionally but gates it on the same feature, so nothing tokio-related is referenced
// unless the consumer opts in.
#[cfg(feature = "blocking")]
mod blocking;
mod client;
// The RFC 3339 `DateTime`/`Date` newtypes pull in the optional `time` dependency, so they are
// compiled only under the `time` feature; a generated client embeds this module only when its spec
// has a `format: date-time` or `format: date`.
#[cfg(feature = "time")]
mod datetime;
mod dispatch;
mod error;
mod header;
mod middleware;
mod paginate;
mod parameter;
mod response;
mod retry;
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
    AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture,
    TokenProvider,
};
#[cfg(feature = "blocking")]
pub use blocking::BlockingRuntime;
pub use client::{ClientConfig, ClientCore};
#[cfg(feature = "time")]
pub use datetime::{Date, DateTime, ParseError};
pub use dispatch::{
    attach_auth, build_url, build_url_on, build_url_with_query_string,
    build_url_with_query_string_on, classify_error, classify_error_bytes, classify_error_text,
    decode_success, decode_success_bytes, decode_success_text, decode_text_body, read_error_body,
    read_success_body, send, unexpected_status, StatusSpec,
};
pub use error::{Error, ProtocolError, RedirectError, RequestError, TimeoutKind, TransportError};
pub use header::{parse_header, require_header, HeaderError, HeaderShape};
pub use middleware::{Middleware, MiddlewareBackend, Next};
pub use paginate::{next_link, LinkPaginator};
pub use parameter::{
    encode, serialize_deep_object, serialize_delimited, serialize_form, serialize_form_body,
    serialize_label, serialize_matrix, serialize_multipart_values, serialize_simple, Delimiter,
    FormMode, FormProperty, FormStyle, ParameterError, PercentEncoding,
};
pub use response::ResponseValue;
pub use retry::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};
pub use stream::{
    EventStream, Framing, ReconnectPolicy, ReconnectReason, ReconnectWait, StreamError,
};
pub use transport::{ExecuteFuture, HttpBackend, ReqwestBackend};
pub use wasm::{MaybeSend, MaybeSync};
#[cfg(feature = "xml")]
pub use xml::{classify_error_xml, decode_success_xml, to_xml};
