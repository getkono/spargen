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
/// value. A documented bodyless status reaches this codec only on an error side with one
/// documented body, when that body sits under a range (`4XX`) or `default` covering the
/// bodyless status, and then it is this `Decode`; elsewhere it is a unit variant of the
/// response enum (#121) or `Error::UnexpectedStatus`. Pinned for both the struct shape
/// generated XML bodies take and a bare `String`, so the XML codec cannot quietly acquire the
/// text codec's empty-string reading.
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
    use crate::tests::poll_ready;

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

/// Both XML helpers that raise `Decode` keep the status and headers of the response they
/// failed to decode. Neither status is `200` and each response carries a header value of its
/// own, so neither a hard-coded status nor an empty header map can pass.
#[test]
fn xml_decode_failures_keep_the_response_status_and_headers() {
    use crate::tests::poll_ready;
    fn response(status: u16, retry_after: &'static str) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .header("retry-after", retry_after)
                .body("not xml <")
                .expect("valid synthetic response"),
        )
    }
    fn assert_decode<E: std::fmt::Debug>(error: crate::Error<E>, expected: u16, retry_after: &str) {
        match &error {
            crate::Error::Decode {
                status, headers, ..
            } => {
                assert_eq!(status.as_u16(), expected);
                assert_eq!(
                    headers.get("retry-after").map(|value| value.as_bytes()),
                    Some(retry_after.as_bytes()),
                    "the XML Decode error for {expected} lost its response headers"
                );
            }
            other => panic!("expected a Decode error, got {other:?}"),
        }
        assert_eq!(error.status().map(|s| s.as_u16()), Some(expected));
    }

    let core = crate::ClientCore::new("https://example.com").unwrap();
    let success = poll_ready(super::decode_success_xml::<Point>(
        &core,
        response(203, "1"),
    ));
    assert_decode(success.unwrap_err(), 203, "1");
    let documented = [crate::StatusSpec::Exact(429)];
    let error = poll_ready(super::classify_error_xml::<Point>(
        &core,
        response(429, "2"),
        &documented,
    ));
    assert_decode(error, 429, "2");
}

/// A zero-length body has no root element, so the XML codec rejects it as `Decode`, keeping the
/// status and the (empty, untruncated) body — through the single-body codec and the per-arm
/// `decode_xml_body` a multi-status enum uses.
#[test]
fn xml_codec_rejects_a_zero_length_success_body() {
    use crate::tests::poll_ready;

    assert!(decode_xml_body::<Point>(b"").is_err());
    assert!(decode_xml_body::<String>(b"").is_err());
    let core = crate::ClientCore::new("https://example.com").unwrap();
    let response = reqwest::Response::from(
        http::Response::builder()
            .status(204)
            .body("")
            .expect("valid synthetic response"),
    );
    match poll_ready(super::decode_success_xml::<Point>(&core, response)) {
        Err(crate::Error::Decode {
            status,
            headers: _,
            path,
            body,
            truncated,
        }) => {
            assert_eq!(status.as_u16(), 204);
            assert!(!path.is_empty());
            assert!(body.is_empty());
            assert!(!truncated);
        }
        other => panic!("expected a Decode error for an empty body, got {other:?}"),
    }
}
