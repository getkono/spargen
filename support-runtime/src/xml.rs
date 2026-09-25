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
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    match decode_xml_body::<T>(&body) {
        Ok(value) => Ok(ResponseValue::new(status, headers, value)),
        Err(path) => {
            let (body, truncated) = crate::dispatch::cap_body(body, core.config().max_error_body);
            Err(Error::Decode {
                status,
                path,
                body,
                truncated,
            })
        }
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
        match decode_xml_body::<E>(&body) {
            Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
            Err(path) => Error::Decode {
                status,
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

/// Deserialize an already-read XML body into `T`, returning a human-readable error string (invalid
/// UTF-8 or a quick-xml parse error) suitable for [`Error::Decode`]'s `path`. The XML analogue of
/// [`crate::decode_text_body`]: a multi-status success enum reads the body once and decodes the arm
/// its status selects through this.
///
/// An empty body always fails, whatever `T` is: XML 1.0 requires a root element, so there is no
/// empty document to decode. This differs from [`crate::decode_text_body`], which reads an empty
/// body as the empty string. A documented bodyless status never reaches this codec: it is a unit
/// variant of the response enum on the success side and on an error side with several documented
/// bodies, and `Error::UnexpectedStatus` on an error side with one documented body or none.
pub fn decode_xml_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let text = std::str::from_utf8(body).map_err(|error| error.to_string())?;
    quick_xml::de::from_str::<T>(text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use serde::{Deserialize, Serialize};

    use super::{decode_xml_body, to_xml};

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
        let decoded: Point = decode_xml_body(&Bytes::from(xml.into_bytes())).unwrap();
        assert_eq!(decoded, point);
    }

    #[test]
    fn malformed_xml_yields_a_nonempty_error_path() {
        let error = decode_xml_body::<Point>(&Bytes::from_static(b"not xml")).unwrap_err();
        assert!(!error.is_empty());
    }

    /// An empty body is not an XML document (XML 1.0 `document` requires a root element), so a
    /// status that documents an XML body and sends none is a decode failure, never a defaulted
    /// value. A documented bodyless status never reaches this codec (#121): it is a unit variant of
    /// the response enum on the success side and on an error side with several documented
    /// bodies, and `Error::UnexpectedStatus` otherwise. Pinned for both the struct shape generated XML bodies take and a bare
    /// `String`, so the XML codec cannot quietly acquire the text codec's empty-string reading.
    #[test]
    fn an_empty_xml_body_is_a_decode_failure() {
        let error = decode_xml_body::<Point>(b"").unwrap_err();
        assert!(!error.is_empty());
        let error = decode_xml_body::<String>(b"").unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn invalid_utf8_yields_a_decode_error_path() {
        let error = decode_xml_body::<Point>(&Bytes::from_static(&[0xff, 0xfe])).unwrap_err();
        assert!(!error.is_empty());
    }

    /// `Error::Decode` documents its body as retained "up to the configured cap"; the XML codec
    /// must honour that too, not retain the whole response.
    #[test]
    fn xml_decode_failure_honours_the_error_body_cap() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        fn poll_ready<F: Future>(future: F) -> F::Output {
            let mut future = std::pin::pin!(future);
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(value) => value,
                Poll::Pending => panic!("future was not immediately ready"),
            }
        }

        let mut core = crate::ClientCore::new("https://example.com").unwrap();
        core.config_mut().max_error_body = 16;
        // Well-formed UTF-8 that is not valid XML, far larger than the cap.
        let response = reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .body("n".repeat(32 * 1024))
                .expect("valid synthetic response"),
        );
        match poll_ready(super::decode_success_xml::<Point>(&core, response)) {
            Err(crate::Error::Decode {
                body, truncated, ..
            }) => {
                assert!(
                    truncated,
                    "oversized XML decode body reported as untruncated"
                );
                assert_eq!(body.len(), 16);
            }
            other => panic!("expected a capped Decode error, got {other:?}"),
        }
    }

    /// Both XML helpers that raise `Decode` keep the status of the response they failed to
    /// decode. Neither status is `200`, so a hard-coded status cannot pass.
    #[test]
    fn xml_decode_failures_keep_the_response_status() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        fn poll_ready<F: Future>(future: F) -> F::Output {
            let mut future = std::pin::pin!(future);
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(value) => value,
                Poll::Pending => panic!("future was not immediately ready"),
            }
        }
        fn response(status: u16) -> reqwest::Response {
            reqwest::Response::from(
                http::Response::builder()
                    .status(status)
                    .body("not xml <")
                    .expect("valid synthetic response"),
            )
        }

        let core = crate::ClientCore::new("https://example.com").unwrap();
        match poll_ready(super::decode_success_xml::<Point>(&core, response(203))) {
            Err(error @ crate::Error::Decode { .. }) => {
                assert_eq!(error.status().map(|s| s.as_u16()), Some(203));
            }
            other => panic!("expected a Decode error, got {other:?}"),
        }
        let documented = [crate::StatusSpec::Exact(422)];
        match poll_ready(super::classify_error_xml::<Point>(
            &core,
            response(422),
            &documented,
        )) {
            error @ crate::Error::Decode { .. } => {
                assert_eq!(error.status().map(|s| s.as_u16()), Some(422));
            }
            other => panic!("expected a Decode error, got {other:?}"),
        }
    }
}
