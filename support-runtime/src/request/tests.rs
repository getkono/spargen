use bytes::Bytes;
use http::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use http::Method;
use url::Url;

use super::{boundary, MultipartForm, MultipartPart, RequestBuilder};
use crate::tests::{poll_ready, Chunk, Chunks};
use crate::{Error, HttpBody};

fn url(text: &str) -> Url {
    Url::parse(text).expect("a valid URL")
}

/// A form's encoded body as text, with its boundary replaced by `B`.
fn encoded(form: MultipartForm) -> (String, String) {
    let (content_type, body) = form.encode_with(|_| b"B".to_vec());
    (
        String::from_utf8(content_type).unwrap(),
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

/// A builder yields the request it was given: method, URL, every header value in order, and the
/// body, in memory.
#[test]
fn a_built_request_carries_what_the_builder_was_given() {
    let request = RequestBuilder::new(Method::PUT, url("https://example.com/op?q=1"))
        .header("x-trace", "abc")
        .header("x-trace", "def")
        .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
        .body("payload")
        .build()
        .unwrap();
    assert_eq!(request.method(), Method::PUT);
    assert_eq!(request.uri(), "https://example.com/op?q=1");
    let traces: Vec<_> = request.headers().get_all("x-trace").iter().collect();
    assert_eq!(traces, ["abc", "def"]);
    assert_eq!(request.headers()[CONTENT_TYPE], "text/plain");
    assert_eq!(request.body().as_bytes(), Some(&b"payload"[..]));
}

/// A request given no body is sent with an empty one, in memory, so it clones.
#[test]
fn a_bodyless_request_has_an_empty_body() {
    let request = RequestBuilder::new(Method::GET, url("https://example.com/op"))
        .build()
        .unwrap();
    assert_eq!(request.body().as_bytes(), Some(&b""[..]));
    assert!(request.headers().is_empty());
}

/// A header that does not convert fails the builder: every later step leaves it failed, it does
/// not clone, and `build` reports the failure as request construction.
#[test]
fn a_header_that_does_not_convert_fails_the_build() {
    for builder in [
        RequestBuilder::new(Method::GET, url("https://example.com/op")).header("bad name", "v"),
        RequestBuilder::new(Method::GET, url("https://example.com/op")).header("x-ok", "bad\nv"),
    ] {
        let mut builder = builder
            .header("x-after", "1")
            .body("ignored")
            .multipart(MultipartForm::new());
        assert!(builder.try_clone().is_none());
        assert!(builder.url_mut().is_none());
        assert!(builder.headers_mut().is_none());
        assert!(matches!(
            builder.build(),
            Err(Error::RequestConstruction(_))
        ));
    }
}

/// An in-memory body moves into the request without a copy, so it shares its bytes, and the
/// builder clones; a streaming one is read as it streams and clones nowhere (issue #1263).
#[test]
fn an_in_memory_body_moves_without_a_copy_and_a_stream_is_never_replayed() {
    let bytes = Bytes::from(vec![7_u8; 32]);
    let builder =
        RequestBuilder::new(Method::PUT, url("https://example.com/op")).body(bytes.clone());
    let clone = builder.try_clone().expect("an in-memory body clones");
    let request = builder.build().unwrap();
    assert_eq!(request.body().as_bytes().unwrap().as_ptr(), bytes.as_ptr());
    assert_eq!(clone.build().unwrap().body().as_bytes(), Some(&bytes[..]));

    let source = Chunks::new([Chunk::data("ab"), Chunk::data("cd")]);
    let pulled = source.pulled();
    let builder = RequestBuilder::new(Method::POST, url("https://example.com/op"))
        .body(HttpBody::wrap(source));
    assert!(builder.try_clone().is_none());
    let request = builder.build().unwrap();
    assert!(crate::try_clone_request(&request).is_none());
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(poll_ready(request.into_body().collect()).unwrap(), "abcd");
    assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// The URL and headers a credential step rewrites are the ones the request is built with.
#[test]
fn the_url_and_headers_are_rewritten_in_place() {
    let mut builder = RequestBuilder::new(Method::GET, url("https://example.com/op?a=1"));
    builder
        .url_mut()
        .unwrap()
        .query_pairs_mut()
        .append_pair("k", "v w");
    builder
        .headers_mut()
        .unwrap()
        .insert("x-set", HeaderValue::from_static("1"));
    let request = builder.build().unwrap();
    assert_eq!(request.uri(), "https://example.com/op?a=1&k=v+w");
    assert_eq!(request.headers()["x-set"], "1");
}

/// A form encodes each part in the order it was added, a name repeating as given, with its
/// `Content-Disposition`, then its `Content-Type`, then its other headers, and the closing
/// delimiter last; the request's `Content-Type` names the boundary, replacing one set before.
#[test]
fn a_form_encodes_its_parts_in_order() {
    let mut headers = HeaderMap::new();
    headers.insert("x-part", HeaderValue::from_static("p"));
    let form = MultipartForm::new()
        .part("name", MultipartPart::text("Rex"))
        .part(
            "image".to_owned(),
            MultipartPart::bytes(vec![0_u8, 1])
                .mime_str("image/png")
                .unwrap()
                .headers(headers),
        )
        .part("name", MultipartPart::text("Fido"));
    let (content_type, body) = encoded(form);
    assert_eq!(content_type, "multipart/form-data; boundary=B");
    assert_eq!(
        body,
        "--B\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nRex\r\n\
         --B\r\nContent-Disposition: form-data; name=\"image\"\r\nContent-Type: image/png\r\n\
         x-part: p\r\n\r\n\u{0}\u{1}\r\n\
         --B\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nFido\r\n\
         --B--\r\n"
    );
    assert_eq!(encoded(MultipartForm::new()).1, "--B--\r\n");

    let request = RequestBuilder::new(Method::POST, url("https://example.com/op"))
        .header(CONTENT_TYPE, "application/json")
        .multipart(MultipartForm::new().part("a", MultipartPart::text("1")))
        .build()
        .unwrap();
    let content_type = request
        .headers()
        .get_all(CONTENT_TYPE)
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(content_type.len(), 1);
    let content_type = content_type[0].to_str().unwrap();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let body = std::str::from_utf8(request.body().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body,
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--{boundary}--\r\n"
        )
    );
    assert!(crate::try_clone_request(&request).is_some());
}

/// A part name is quoted with `"`, CR and LF percent-encoded, and every other byte, non-ASCII
/// included, written as it is.
#[test]
fn a_part_name_escapes_only_quote_cr_and_lf() {
    let form = MultipartForm::new().part("a\"b\r\nc%d é", MultipartPart::text(""));
    assert_eq!(
        encoded(form).1,
        "--B\r\nContent-Disposition: form-data; name=\"a%22b%0D%0Ac%d é\"\r\n\r\n\r\n--B--\r\n"
    );
}

/// A candidate boundary that occurs in a part, in its bytes or its header lines, is passed over
/// for the next seed's.
#[test]
fn a_boundary_a_part_contains_is_passed_over() {
    let candidates = |seed: u64| -> Vec<u8> {
        match seed {
            0 => b"in-body".to_vec(),
            1 => b"in-name".to_vec(),
            _ => b"clear".to_vec(),
        }
    };
    let form = MultipartForm::new().part("in-name", MultipartPart::text("text holding in-body"));
    let (content_type, body) = form.encode_with(candidates);
    assert_eq!(content_type, b"multipart/form-data; boundary=clear");
    assert!(body.starts_with(b"--clear\r\n"));
}

/// A boundary is 32 lowercase hexadecimal digits, and two forms are not given the same one.
#[test]
fn a_boundary_is_hex_and_differs_between_forms() {
    let first = boundary(0);
    assert_eq!(first.len(), 32);
    assert!(first
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)));
    assert_ne!(first, boundary(0));
}

/// A part `Content-Type` that is not a header value is refused as request construction.
#[test]
fn a_part_content_type_that_is_no_header_value_is_refused() {
    assert!(matches!(
        MultipartPart::text("x").mime_str("text/plain\n"),
        Err(Error::RequestConstruction(_))
    ));
}

/// A URL the `http` crate does not accept as a URI is refused at `build`, as request
/// construction, before anything is sent.
#[test]
fn a_url_that_is_no_uri_is_refused_at_build() {
    // `http` refuses a URI longer than it can index; `url` holds it.
    let long = format!("https://example.com/{}", "a".repeat(70_000));
    let refused = RequestBuilder::new(Method::GET, url(&long)).build();
    assert!(matches!(refused, Err(Error::RequestConstruction(_))));
}
