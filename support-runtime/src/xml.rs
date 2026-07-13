//! XML request/response body codec (feature `xml`).
//!
//! Serializes typed request bodies to XML and decodes XML response bodies through `quick-xml`'s
//! serde integration, mirroring the JSON paths in [`crate::dispatch`]. Compiled only under the
//! `xml` feature — enabled by a spec with an `application/xml` / `text/xml` body — so the default
//! runtime and every non-XML generated client never reference `quick-xml`.

use std::convert::Infallible;

use bytes::Bytes;
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
/// [`Error::Decode`] with the quick-xml error path and the (whole) body retained.
pub async fn decode_success_xml<T>(
    _core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    match from_xml_bytes::<T>(&body) {
        Ok(value) => Ok(ResponseValue::new(status, headers, value)),
        Err(path) => Err(Error::Decode {
            path,
            body,
            truncated: false,
        }),
    }
}

/// Classify a non-success XML response into the operation's typed error body `E`. A documented
/// status parses into `E` ([`Error::Api`], falling back to [`Error::Decode`] on a parse failure); an
/// undocumented status is [`Error::UnexpectedStatus`]. The XML analogue of
/// [`crate::classify_error`], reusing [`crate::read_error_body`] so the error-body cap is identical.
pub async fn classify_error_xml<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    let (status, headers, body, truncated) = match crate::read_error_body::<E>(core, response).await
    {
        Ok(parts) => parts,
        Err(error) => return error,
    };
    if documented.iter().any(|spec| spec.matches(status)) {
        match from_xml_bytes::<E>(&body) {
            Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
            Err(path) => Error::Decode {
                path,
                body,
                truncated,
            },
        }
    } else {
        Error::UnexpectedStatus {
            status,
            headers,
            body,
        }
    }
}

/// Deserialize XML bytes into `T`, returning a human-readable error string (invalid UTF-8 or a
/// quick-xml parse error) suitable for [`Error::Decode`]'s `path`.
fn from_xml_bytes<T: DeserializeOwned>(body: &Bytes) -> Result<T, String> {
    let text = std::str::from_utf8(body).map_err(|error| error.to_string())?;
    quick_xml::de::from_str::<T>(text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use serde::{Deserialize, Serialize};

    use super::{from_xml_bytes, to_xml};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Point {
        x: i32,
        y: i32,
        #[serde(rename = "@label")]
        label: String,
    }

    #[test]
    fn to_xml_serializes_elements_and_attributes() {
        // A plain field is a child element; a `@`-prefixed field (the `xml.attribute` convention) is
        // an attribute on the root element.
        let point = Point {
            x: 3,
            y: -7,
            label: "origin".to_owned(),
        };
        let xml = to_xml(&point).unwrap();
        assert!(xml.contains("label=\"origin\""), "{xml}");
        assert!(xml.contains("<x>3</x>"), "{xml}");
        assert!(xml.contains("<y>-7</y>"), "{xml}");
    }

    #[test]
    fn xml_round_trips_a_struct() {
        let point = Point {
            x: 1,
            y: 2,
            label: "p".to_owned(),
        };
        let xml = to_xml(&point).unwrap();
        let decoded: Point = from_xml_bytes(&Bytes::from(xml.into_bytes())).unwrap();
        assert_eq!(decoded, point);
    }

    #[test]
    fn malformed_xml_yields_a_nonempty_error_path() {
        let error = from_xml_bytes::<Point>(&Bytes::from_static(b"not xml")).unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn invalid_utf8_yields_a_decode_error_path() {
        let error = from_xml_bytes::<Point>(&Bytes::from_static(&[0xff, 0xfe])).unwrap_err();
        assert!(!error.is_empty());
    }
}
