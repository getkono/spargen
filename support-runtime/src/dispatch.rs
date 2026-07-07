//! The small set of dispatch routines shared within a generated client (PRD NFR2): build URL →
//! attach auth → send → classify status → decode. Sharing happens *within* a generated client
//! (not via a shared crate), so per-operation functions stay thin `#[inline]` shims.
//!
//! URL/send/classification are non-generic; only body decode is generic, monomorphized once per
//! distinct body type — the one place monomorphization is unavoidable.

use std::convert::Infallible;

use reqwest::{Request, Response, Url};
use serde::de::DeserializeOwned;

use crate::{ClientCore, Error, ResponseValue};

/// Build a request URL from the base URL and pre-rendered path plus query pairs. Paths compile to
/// static segment concatenation — no runtime regex (PRD NFR1). Non-generic.
pub fn build_url(
    core: &ClientCore,
    path: &str,
    query: &[(&str, String)],
) -> Result<Url, Error<Infallible>> {
    todo!()
}

/// Send a prepared request, mapping transport/timeout/protocol/redirect failures into the taxonomy
/// (PRD FR5 #2–#5). Non-generic.
pub async fn send(core: &ClientCore, request: Request) -> Result<Response, Error<Infallible>> {
    todo!()
}

/// Decode a success response body into `T`, wrapping it with status and headers. Monomorphized once
/// per body type. Decode failures become [`Error::Decode`] with the serde path and a capped body.
pub async fn decode_success<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    todo!()
}

/// Classify a non-success response into either the operation's typed error body ([`Error::Api`],
/// #6) or an [`Error::UnexpectedStatus`] (#7), retaining at most `max_error_body` bytes (PRD D7).
pub async fn classify_error<E>(core: &ClientCore, response: Response) -> Error<E>
where
    E: DeserializeOwned,
{
    todo!()
}
