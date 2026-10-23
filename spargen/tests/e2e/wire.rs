//! Values on the wire: dates, a per-operation server override, RFC 6570 multipart parts, and CBOR
//! bodies.

use super::*;

/// `format: date-time` and `format: date` must reach the wire as RFC 3339 strings.
///
/// This is the test whose absence let a wire defect ship: every other date fixture only
/// *compile*-checks the mapping, and `time`'s own serde representation compiles perfectly well
/// while emitting a nine-element integer sequence (without its `serde-human-readable` feature) or a
/// space-separated `2023-11-14 22:13:20.0 +00:00:00` (with it). Neither is RFC 3339, which is what
/// JSON Schema 2020-12 — and therefore OpenAPI 3.1/3.2 — defines these formats to be. So the
/// assertions here are on the bytes, in a request body, a response body, and two query parameters.
#[test]
fn date_and_date_time_reach_the_wire_as_rfc3339() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, DATE_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "dates_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    // The model resolves to the embedded newtypes, not to `time`'s own types — naming those in a
    // model is exactly the defect, since they carry the non-RFC-3339 serde implementation. (The
    // newtype *definitions* name them, which is why this checks the aliases rather than the file.)
    assert!(
        generated.contains("pub type Eventat = DateTime;"),
        "a date-time property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        generated.contains("pub type Eventday = Date;"),
        "a date property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        !generated.contains("pub type Eventat = time::")
            && !generated.contains("pub type Eventday = time::"),
        "no model alias may name time's own serde types"
    );
    assert!(
        generated.contains("pub struct DateTime(pub time::OffsetDateTime)"),
        "the RFC 3339 newtype module must be embedded for a spec that uses dates"
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/dates.rs"), DATE_WIRE_TEST).unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "dates"])
        .status()
        .unwrap();
    assert!(status.success(), "the date-time wire round-trip must pass");
}

const DATE_WIRE_TEST: &str = r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn dates_are_rfc3339_on_the_wire_in_both_directions() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");

        // Query parameters: RFC 3339 text, percent-encoded as query data (`:` -> `%3A`). A
        // sequence-serialized datetime could not appear here at all.
        assert!(
            request_line.contains("since=2023-11-14T22%3A13%3A20Z"),
            "date-time query parameter must be RFC 3339: {request_line}"
        );
        assert!(
            request_line.contains("on=2023-11-14"),
            "date query parameter must be a full-date: {request_line}"
        );

        // Request body: JSON strings, not the nine- and two-element integer arrays `time`'s own
        // `Serialize` produces without `serde-human-readable`.
        assert_eq!(
            body, r#"{"at":"2023-11-14T22:13:20Z","day":"2023-11-14"}"#,
            "date fields must serialize as RFC 3339 strings: {body}"
        );

        let payload = r#"{"at":"2024-02-29T01:02:03.5+05:30","day":"2024-02-29"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let base = format!("http://{addr}");
    let client = dates_client::BlockingClient::new(&base).unwrap();

    let at = dates_client::DateTime(
        time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
    );
    let day = dates_client::Date(
        time::Date::from_calendar_date(2023, time::Month::November, 14).unwrap(),
    );
    let event = dates_client::types::Event { at, day };

    let params = dates_client::CreateEventParams::default()
        .since(at)
        .on(day);
    let response = client
        .create_event(Some(params), &event)
        .expect("create_event round-trips");

    // Decoding: an offset and a subsecond survive the round trip as the server wrote them.
    let decoded = response.into_inner();
    assert_eq!(decoded.day.to_string(), "2024-02-29");
    assert_eq!(decoded.at.to_string(), "2024-02-29T01:02:03.5+05:30");
    // The newtype is transparent: `time`'s API is one deref away.
    assert_eq!(decoded.at.year(), 2024);

    server.join().unwrap();
}
"##;

/// A Path Item `servers` override must send that operation to a *different host*, while its
/// siblings keep the client's base URL.
///
/// This is the wire half of the fix: the runtime already had `build_url_on` for exactly this, with
/// its own unit test, but codegen never passed anything but `None` — so an override compiled fine
/// and silently went to the wrong server. Only a second listener can tell the two apart.
#[test]
fn a_server_override_sends_the_operation_to_another_host() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // Bound before generation: the override URL is baked into the generated code, so the port has
    // to be known first. This listener is served from *this* process while the generated crate's
    // test process drives the client against it.
    let override_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let override_addr = override_listener.local_addr().unwrap();
    let override_server = std::thread::spawn(move || {
        let (mut stream, _) = override_listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        format!(
            r##"
openapi: 3.1.0
info: {{ title: Servers, version: 1.0.0 }}
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "204": {{ description: No Content }}
  /upload:
    servers:
      - url: http://{override_addr}
    post:
      operationId: uploadItem
      responses:
        "204": {{ description: No Content }}
"##
        ),
    )
    .unwrap();
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "servers_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/servers.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

// The base-URL operation must reach the client's own base, proving the override is scoped to the
// path item that declares it rather than applied to the whole client.
#[test]
fn the_unoverridden_operation_still_uses_the_client_base_url() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let base_server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let base = format!("http://{addr}");
    let client = servers_client::BlockingClient::new(&base).unwrap();

    // Goes to the client's base URL.
    client.list_pets().expect("list_pets round-trips");
    let seen = base_server.join().unwrap();
    assert!(
        seen.starts_with("GET /pets "),
        "the base server should have served /pets: {seen}"
    );

    // Goes to the override host, which lives in the *parent* test process; reaching it at all is
    // the proof, since this process never bound that port.
    client.upload_item().expect("upload_item round-trips");
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "servers"])
        .status()
        .unwrap();
    assert!(status.success(), "the server-override round-trip must pass");

    let seen = override_server.join().unwrap();
    assert!(
        seen.starts_with("POST /upload "),
        "the override host should have served /upload: {seen}"
    );
    assert!(
        seen.contains(&format!("host: {override_addr}")),
        "the request must carry the override host, not the document server: {seen}"
    );
}

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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
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
    )
    .unwrap();
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "multipart_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/multipart.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn rfc6570_multipart_parts_are_not_percent_encoded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let base = format!("http://{addr}");
    let client = multipart_client::BlockingClient::new(&base).unwrap();
    let body = multipart_client::types::RequestBody {
        tags: vec!["blue".into(), "black".into()],
        paths: vec!["a/b".into(), "c".into()],
        names: vec!["ada".into(), "grace".into()],
    };
    client.upload(&body).expect("upload round-trips");

    let request = server.join().unwrap();

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
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "multipart"])
        .status()
        .unwrap();
    assert!(status.success(), "the multipart wire round-trip must pass");
}

/// A CBOR body travels as the exact bytes it is (#900): `application/cbor` and a `+cbor` suffixed
/// type are each sent as their own `Content-Type` and asked for as their own `Accept`, and the
/// response comes back untouched, while an `application/octet-stream` operation still sets no
/// `Accept` of its own.
#[test]
fn cbor_bodies_reach_the_wire_under_their_own_media_type() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        r##"
openapi: 3.1.0
info: { title: Cbor, version: 1.0.0 }
paths:
  /directory:
    post:
      operationId: publishDirectory
      requestBody:
        required: true
        content:
          application/cbor:
            schema: { type: string, format: binary }
      responses:
        "200":
          description: the signed directory
          content:
            application/cbor:
              schema: { type: string, format: binary }
  /receipt:
    post:
      operationId: fetchReceipt
      requestBody:
        required: true
        content:
          application/cose+cbor:
            schema: { type: string, format: binary }
      responses:
        "200":
          description: the signed receipt
          content:
            application/cose+cbor:
              schema: { type: string, format: binary }
  /blob:
    post:
      operationId: putBlob
      requestBody:
        required: true
        content:
          application/octet-stream:
            schema: { type: string, format: binary }
      responses:
        "200":
          description: the blob
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
"##,
    )
    .unwrap();
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "cbor_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/cbor.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

/// Serve one request, answering `body` under `content_type`, and return the request as received:
/// its lowercased head and its body.
fn serve_once(
    content_type: &'static str,
    body: &'static [u8],
) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 4096];
        let (head, length) = loop {
            let read = stream.read(&mut buf).unwrap();
            assert!(read > 0, "the request ended before its head did");
            received.extend_from_slice(&buf[..read]);
            if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |length| length.trim().parse::<usize>().unwrap());
                received.drain(..end + 4);
                break (head, length);
            }
        };
        while received.len() < length {
            let read = stream.read(&mut buf).unwrap();
            assert!(read > 0, "the request ended before its body did");
            received.extend_from_slice(&buf[..read]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        (head, received)
    });
    (format!("http://{addr}"), server)
}

/// A CBOR map whose bytes no re-encoding would preserve: a non-canonical length prefix.
const SIGNED: &[u8] = b"\xb9\x00\x01\x61k\x42\x00\xff";

#[test]
fn cbor_is_sent_and_asked_for_under_its_own_media_type() {
    for (media, call) in [
        (
            "application/cbor",
            (|client: &cbor_client::BlockingClient, body: &bytes::Bytes| {
                client.publish_directory(body)
            }) as fn(&_, &_) -> _,
        ),
        ("application/cose+cbor", |client, body| client.fetch_receipt(body)),
    ] {
        let (base, server) = serve_once(media, SIGNED);
        let client = cbor_client::BlockingClient::new(&base).unwrap();
        let returned = call(&client, &bytes::Bytes::from_static(SIGNED))
            .expect("the CBOR call round-trips")
            .into_inner();
        let (head, body) = server.join().unwrap();
        assert_eq!(header(&head, "content-type"), [media], "{head}");
        // The declared type replaces the HTTP stack's default `*/*`, rather than joining it.
        assert_eq!(header(&head, "accept"), [media], "{head}");
        assert_eq!(body, SIGNED, "the request body must be the caller's exact bytes");
        assert_eq!(&returned[..], SIGNED, "the response must be the server's exact bytes");
    }
}

/// Every value of the header `name` in a lowercased request head, in order.
fn header<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(field, _)| *field == name)
        .map(|(_, value)| value.trim())
        .collect()
}

#[test]
fn octet_stream_still_leaves_accept_to_the_http_stack() {
    let (base, server) = serve_once("application/octet-stream", SIGNED);
    let client = cbor_client::BlockingClient::new(&base).unwrap();
    client
        .put_blob(&bytes::Bytes::from_static(SIGNED))
        .expect("the octet-stream call round-trips");
    let (head, _) = server.join().unwrap();
    assert_eq!(header(&head, "content-type"), ["application/octet-stream"], "{head}");
    // The generated method sets no `Accept`; what arrives is the HTTP stack's own default.
    assert_eq!(header(&head, "accept"), ["*/*"], "{head}");
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "cbor"])
        .status()
        .unwrap();
    assert!(status.success(), "the CBOR wire round-trip must pass");
}
