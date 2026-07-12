//! # support-runtime
//!
//! The freestanding runtime support code shipped *inside* every spargen-generated client: the
//! dispatch routines, the error taxonomy, [`ResponseValue<T>`], and auth plumbing. It is real,
//! standalone-compilable source (compiled and linted here in its own right) that the
//! `codegen` subsystem embeds verbatim via `include_str!`.
//!
//! No spargen crate ever appears in a consumer's runtime graph: this crate is `publish = false`
//! and its only dependencies are the near-universal `reqwest` / `serde` / `serde_json` / `bytes`
//! / `secrecy` set.
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
mod client;
mod dispatch;
mod error;
mod response;
mod stream;

pub use auth::{
    AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture,
    TokenProvider,
};
pub use client::{ClientConfig, ClientCore};
pub use dispatch::{
    attach_auth, build_url, classify_error, decode_success, read_error_body, read_success_body,
    send, unexpected_status, StatusSpec,
};
pub use error::{Error, ProtocolError, RedirectError, RequestError, TimeoutKind, TransportError};
pub use response::ResponseValue;
pub use stream::{EventStream, Framing};
