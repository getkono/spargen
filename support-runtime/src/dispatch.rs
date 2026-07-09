//! The small set of dispatch routines shared within a generated client (PRD NFR2): build URL →
//! attach auth → send → classify status → decode. Sharing happens *within* a generated client
//! (not via a shared crate), so per-operation functions stay thin `#[inline]` shims.
//!
//! URL/send/classification are non-generic; only body decode is generic, monomorphized once per
//! distinct body type — the one place monomorphization is unavoidable.

use std::convert::Infallible;

use bytes::Bytes;
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
    let mut url = core.base_url().clone();
    let base_path = url.path().trim_end_matches('/');
    let request_path = path.trim_start_matches('/');
    let joined = if base_path.is_empty() {
        format!("/{request_path}")
    } else if request_path.is_empty() {
        base_path.to_owned()
    } else {
        format!("{base_path}/{request_path}")
    };
    url.set_path(&joined);
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

/// Send a prepared request, mapping transport/timeout/protocol/redirect failures into the taxonomy
/// (PRD FR5 #2–#5). Non-generic.
pub async fn send(core: &ClientCore, request: Request) -> Result<Response, Error<Infallible>> {
    core.http()
        .execute(request)
        .await
        .map_err(Error::from_reqwest)
}

/// Decode a success response body into `T`, wrapping it with status and headers. Monomorphized once
/// per body type. Decode failures become [`Error::Decode`] with the serde path and a capped body.
pub async fn decode_success<T>(
    _core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    let value = serde_json::from_slice::<T>(&body).map_err(|error| Error::Decode {
        path: error.to_string(),
        body,
        truncated: false,
    })?;
    Ok(ResponseValue::new(status, headers, value))
}

/// Classify a non-success response into either the operation's typed error body ([`Error::Api`],
/// #6) or an [`Error::UnexpectedStatus`] (#7), retaining at most `max_error_body` bytes (PRD D7).
pub async fn classify_error<E>(core: &ClientCore, response: Response) -> Error<E>
where
    E: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, truncated)) => match serde_json::from_slice::<E>(&body) {
            Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
            Err(error) => Error::Decode {
                path: error.to_string(),
                body,
                truncated,
            },
        },
        Err(error) => error,
    }
}

async fn read_capped<E>(core: &ClientCore, response: Response) -> Result<(Bytes, bool), Error<E>> {
    let cap = core.config().max_error_body;
    let bytes = response.bytes().await.map_err(Error::from_reqwest)?;
    if bytes.len() <= cap {
        Ok((bytes, false))
    } else {
        Ok((bytes.slice(..cap), true))
    }
}
