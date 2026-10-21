//! XML request/response body codec (feature `xml`).
//!
//! Serializes typed request bodies to XML and decodes XML response bodies through `quick-xml`'s
//! serde integration, mirroring the JSON paths in [`crate::dispatch`]. Compiled only under the
//! `xml` feature — enabled by a spec with an `application/xml` / `text/xml` body — so the default
//! runtime and every non-XML generated client never reference `quick-xml`.

use std::convert::Infallible;

use reqwest::Response;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{ClientCore, Error, ResponseValue, StatusSpec};

/// Serialize a request body to an XML string. A serialization failure is a request-construction
/// error (taxonomy #1) — the value has no XML representation — never a panic. The `@name` serde
/// convention on a field maps to an XML attribute; `xml.name` renames the element/attribute.
pub fn to_xml<T>(value: &T) -> Result<String, Error<Infallible>>
where
    T: Serialize + ?Sized,
{
    quick_xml::se::to_string(value).map_err(Error::request_construction)
}

/// Decode an XML success response body into `T`, wrapping it with status and headers. The XML
/// analogue of [`crate::decode_success`]; a parse failure (invalid UTF-8 or malformed XML) becomes
/// [`Error::Decode`] with the quick-xml error path and a body capped at `max_error_body`. An empty
/// body is not an XML document and is a parse failure too (see [`decode_xml_body`]).
pub async fn decode_success_xml<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    crate::dispatch::decode_success_with(core, response, |body| decode_xml_body::<T>(body)).await
}

/// Classify a non-success XML response into the operation's typed error body `E`. A documented
/// status parses into `E` ([`Error::Api`], falling back to [`Error::Decode`] on a parse failure); an
/// undocumented status is [`Error::UnexpectedStatus`]. The XML analogue of
/// [`crate::classify_error`], sharing its classification body so the error-body cap is identical.
pub async fn classify_error_xml<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    crate::dispatch::classify_with(core, response, documented, |body| {
        decode_xml_body::<E>(body)
    })
    .await
}

/// Deserialize an already-read XML body into `T`, returning a human-readable error string (invalid
/// UTF-8 or a quick-xml parse error) suitable for [`Error::Decode`]'s `path`. The XML analogue of
/// [`crate::decode_text_body`]: a multi-status success or error enum reads the body once and
/// decodes the arm its status selects through this.
///
/// An empty body always fails, whatever `T` is: XML 1.0 requires a root element, so there is no
/// empty document to decode. This differs from [`crate::decode_text_body`], which reads an empty
/// body as the empty string. A documented bodyless status beside a documented body is a unit
/// variant of the response enum, on the success side and the error side alike, and is not decoded
/// here.
pub fn decode_xml_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let text = std::str::from_utf8(body).map_err(|error| error.to_string())?;
    quick_xml::de::from_str::<T>(text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
