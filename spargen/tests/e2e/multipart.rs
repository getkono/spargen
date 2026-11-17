//! `multipart/form-data` parts on the wire: RFC 6570-mode parts with literal delimiters and a pinned
//! part header, and untyped parts sent as raw file parts.

use super::*;

/// RFC 6570-mode `multipart/form-data` parts must carry literal delimiters, not percent-encoded
/// ones.
///
/// The specification is explicit that "when using RFC6570-style serialization for
/// `multipart/form-data`, URI percent-encoding MUST NOT be applied", but the part values were built
/// from the query-fragment builders, whose delimiters are pre-encoded `%20` / `%7C` triples. Only a
/// look at the raw body catches that; the existing multipart fixtures all use `contentType` mode.
#[test]
fn multipart_rfc6570_parts_carry_literal_delimiters() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Multipart, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [tags, paths, names]
              properties:
                tags:
                  type: array
                  items: { type: string }
                paths:
                  type: array
                  items: { type: string }
                names:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: spaceDelimited
                explode: false
              paths:
                style: pipeDelimited
                explode: false
              names:
                style: form
                explode: true
                # A pinned part header rides on every part the style serialization produces.
                headers:
                  X-Part-Kind:
                    schema: { type: string, const: name }
      responses:
        "204": { description: No Content }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "multipart_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "multipart",
        r##"#![cfg(feature = "blocking")]

mod common;

#[test]
fn rfc6570_multipart_parts_are_not_percent_encoded() {
    let (base, server) = common::serve_no_content();
    let client = multipart_client::BlockingClient::new(&base).unwrap();
    let body = multipart_client::types::UploadBody {
        tags: vec!["blue".into(), "black".into()],
        paths: vec!["a/b".into(), "c".into()],
        names: vec!["ada".into(), "grace".into()],
    };
    client.upload(body).expect("upload round-trips");

    let request = server.join().unwrap().text();

    // `spaceDelimited` joins with a literal space, `pipeDelimited` with a literal `|`.
    assert!(
        request.contains("blue black"),
        "spaceDelimited must join with a literal space: {request}"
    );
    assert!(
        request.contains("a/b|c"),
        "pipeDelimited must join with a literal pipe: {request}"
    );
    // The encoded forms are exactly the defect.
    assert!(
        !request.contains("blue%20black"),
        "a part value must not be percent-encoded: {request}"
    );
    assert!(
        !request.contains("%7C"),
        "a part delimiter must not be percent-encoded: {request}"
    );
    // `form` + `explode` sends one part per item, under the same name (RFC 7578 s4.3).
    assert_eq!(
        request.matches(r#"name="names""#).count(),
        2,
        "an exploded array must send one part per item: {request}"
    );
    // The pinned `X-Part-Kind` header rides on each of those parts, and on no other.
    assert_eq!(
        request.to_ascii_lowercase().matches("x-part-kind: name\r\n").count(),
        2,
        "every style-serialized part must carry its pinned header: {request}"
    );
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "multipart"])
        .status()
        .unwrap();
    assert!(status.success(), "the multipart wire round-trip must pass");
}

/// An untyped multipart part is a file part of raw bytes (#1410): `{contentMediaType: image/png}`
/// sends the caller's octets under `image/png`, and `{}` under `application/octet-stream`, each
/// with a `filename`, where both used to be `serde_json::Value` sent as JSON text.
#[test]
fn untyped_multipart_parts_send_their_octets() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Untyped parts, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, raw]
              properties:
                file: { contentMediaType: image/png }
                raw: {}
      responses:
        "204": { description: No Content }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "untyped_parts_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "untyped_parts",
        r##"#![cfg(feature = "blocking")]

mod common;

#[test]
fn untyped_parts_are_sent_as_raw_file_parts() {
    let (base, server) = common::serve_no_content();
    let client = untyped_parts_client::BlockingClient::new(&base).unwrap();
    let body = untyped_parts_client::types::UploadBody {
        file: bytes::Bytes::from_static(b"\x89PNG\r\n"),
        raw: bytes::Bytes::from_static(b"\x00\x01raw"),
    };
    client.upload(&body).expect("upload round-trips");

    let request = server.join().unwrap();
    let sent = |needle: &[u8]| request.body.windows(needle.len()).any(|window| window == needle);
    let text = request.text();
    for part in [
        &b"name=\"file\"; filename=\"file\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n"[..],
        &b"name=\"raw\"; filename=\"raw\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01raw"[..],
    ] {
        assert!(sent(part), "missing {:?} in {text}", String::from_utf8_lossy(part));
    }
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "untyped_parts"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the untyped-part wire round-trip must pass"
    );
}
