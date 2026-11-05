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
    assert_eq!(
        to_xml(&point).unwrap(),
        r#"<Point label="origin"><x>3</x><y>-7</y></Point>"#
    );
}

/// A value with no element name to put at the root, a bare scalar or a sequence, has no XML
/// document form: `to_xml` refuses it as `RequestConstruction`, before anything is sent.
#[test]
fn to_xml_refuses_a_value_with_no_root_element() {
    for result in [to_xml(&5i32), to_xml(&vec![1, 2]), to_xml("text")] {
        match result {
            Err(crate::Error::RequestConstruction(_)) => {}
            other => panic!("expected RequestConstruction, got {other:?}"),
        }
    }
}

/// A documented error status whose XML body decodes is `Api`, carrying the decoded value with
/// the response's status.
#[test]
fn a_documented_xml_error_body_decodes_into_api() {
    use crate::tests::{core, poll_ready, response};

    let body = r#"<Point label="missing"><x>0</x><y>0</y></Point>"#;
    let documented = [crate::StatusSpec::Exact(404)];
    match poll_ready(super::classify_error_xml::<Point>(
        &core(),
        response(404, body),
        &documented,
    )) {
        crate::Error::Api(value) => {
            assert_eq!(value.status().as_u16(), 404);
            assert_eq!(
                value.into_inner(),
                Point {
                    x: 0,
                    y: 0,
                    label: "missing".to_owned(),
                }
            );
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

/// An error status the operation does not document is `UnexpectedStatus` through the XML codec
/// as through every other: its body is kept as bytes and never decoded, even when it would.
#[test]
fn an_undocumented_status_is_unexpected_through_the_xml_codec() {
    use crate::tests::{core, poll_ready, response};

    let body = r#"<Point label="down"><x>0</x><y>0</y></Point>"#;
    let documented = [crate::StatusSpec::Exact(404)];
    match poll_ready(super::classify_error_xml::<Point>(
        &core(),
        response(500, body),
        &documented,
    )) {
        crate::Error::UnexpectedStatus {
            status, body: kept, ..
        } => {
            assert_eq!(status.as_u16(), 500);
            assert_eq!(&kept[..], body.as_bytes());
        }
        other => panic!("expected UnexpectedStatus, got {other:?}"),
    }
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
fn malformed_xml_yields_the_parsers_message_as_the_error_path() {
    let error = decode_xml_body::<Point>(&Bytes::from_static(b"not xml")).unwrap_err();
    assert_eq!(
        error,
        "invalid type: string \"not xml\", expected struct Point"
    );
}

/// An empty body is not an XML document (XML 1.0 `document` requires a root element), so a
/// status that documents an XML body and sends none is a decode failure, never a defaulted
/// value. A documented bodyless status never reaches this codec: beside a documented body it is
/// a unit variant of the response enum (#121, #204), and otherwise `()` on the success side or
/// `Error::UnexpectedStatus` on the error side. Pinned for both the struct shape
/// generated XML bodies take and a bare `String`, so the XML codec cannot quietly acquire the
/// text codec's empty-string reading.
#[test]
fn an_empty_xml_body_is_a_decode_failure() {
    let error = decode_xml_body::<Point>(b"").unwrap_err();
    assert_eq!(error, "unexpected `Event::Eof`");
    let error = decode_xml_body::<String>(b"").unwrap_err();
    assert_eq!(error, "unexpected `Event::Eof`");
}

#[test]
fn invalid_utf8_yields_the_utf8_error_as_the_error_path() {
    let error = decode_xml_body::<Point>(&Bytes::from_static(&[0xff, 0xfe])).unwrap_err();
    assert_eq!(error, "invalid utf-8 sequence of 1 bytes from index 0");
}

/// `Error::Decode` documents its body as retained "up to the configured cap"; the XML codec
/// must honour that too, not retain the whole response.
#[test]
fn xml_decode_failure_honours_the_error_body_cap() {
    use crate::tests::poll_ready;

    let mut core = crate::tests::core();
    core.config_mut().max_error_body = 16;
    // Well-formed UTF-8 that is not valid XML, far larger than the cap.
    let response = crate::tests::response(200, "n".repeat(32 * 1024));
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

    let core = crate::tests::core();
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
    let core = crate::tests::core();
    let response = crate::tests::response(204, "");
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
