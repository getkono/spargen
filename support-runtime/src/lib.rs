//! # support-runtime
//!
//! The freestanding runtime support code shipped *inside* every spargen-generated client: the
//! dispatch routines, the FR5 error taxonomy, [`ResponseValue<T>`], and auth plumbing. It is real,
//! standalone-compilable source (compiled and linted here in its own right, PRD §7.5) that the
//! `codegen` subsystem embeds verbatim via `include_str!` (PRD §2.3 rule 3).
//!
//! No spargen crate ever appears in a consumer's runtime graph: this crate is `publish = false`
//! and its only dependencies are the near-universal `reqwest` / `serde` / `serde_json` / `bytes`
//! set (PRD §2.1).
//!
//! ## Fault-tolerance guarantees (PRD FR5)
//!
//! The generated client never panics on network input; every failure maps to one [`Error`]
//! variant. Taxonomy class **#10, cancellation**, is a *documented drop-safety guarantee* rather
//! than an enum variant: dropping a returned future is safe and side-effect-free beyond standard
//! HTTP semantics. The other nine classes are the constructed [`Error`] variants.

#![forbid(unsafe_code)]
// The FR3 API contract returns `Result<ResponseValue<T>, Error<E>>` with `Error` unboxed, and the
// FR5 taxonomy deliberately retains headers/bodies for forensics — so `Error` is intentionally
// large and is passed by value rather than boxed.
#![allow(clippy::result_large_err)]
mod auth;
mod client;
mod dispatch;
mod error;
mod response;

pub use auth::{AuthError, Credential, SecretString, TokenFuture, TokenProvider};
pub use client::{ClientConfig, ClientCore};
pub use dispatch::{build_url, classify_error, decode_success, send};
pub use error::{Error, ProtocolError, RedirectError, RequestError, TimeoutKind, TransportError};
pub use response::ResponseValue;
