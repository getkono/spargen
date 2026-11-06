//! Values on the wire: dates, a per-operation server override, an operation's URL accessor against
//! the request it describes, RFC 6570 multipart parts, CBOR bodies, an explicit `null` content
//! parameter, cookie parameters and cookie credentials in one `Cookie` header, and the mock
//! server's reading of a chunked request body.

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
    let spec = write_spec(temp.path(), "openapi.yaml", DATE_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "dates_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    // The model resolves to the embedded newtypes, not to `time`'s own types — naming those in a
    // model is exactly the defect, since they carry the non-RFC-3339 serde implementation. (The
    // newtype *definitions* name them, which is why this checks the aliases rather than the file.)
    assert!(
        generated.contains("pub type EventAt = DateTime;"),
        "a date-time property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        generated.contains("pub type EventDay = Date;"),
        "a date property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        !generated.contains("pub type EventAt = time::")
            && !generated.contains("pub type EventDay = time::"),
        "no model alias may name time's own serde types"
    );
    assert!(
        generated.contains("pub struct DateTime(pub time::OffsetDateTime)"),
        "the RFC 3339 newtype module must be embedded for a spec that uses dates"
    );

    write_fixture_test(&out, "dates", DATE_WIRE_TEST);

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "dates"])
        .status()
        .unwrap();
    assert!(status.success(), "the date-time wire round-trip must pass");
}

const DATE_WIRE_TEST: &str = r##"#![cfg(feature = "blocking")]

mod common;

#[test]
fn dates_are_rfc3339_on_the_wire_in_both_directions() {
    let (base, server) = common::serve_once(
        "application/json",
        "200 OK",
        br#"{"at":"2024-02-29T01:02:03.5+05:30","day":"2024-02-29"}"#,
    );
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

    let request = server.join().unwrap();
    let request_line = request.line();
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
    let body = String::from_utf8_lossy(&request.body);
    assert_eq!(
        body, r#"{"at":"2023-11-14T22:13:20Z","day":"2023-11-14"}"#,
        "date fields must serialize as RFC 3339 strings: {body}"
    );

    // Decoding: an offset and a subsecond survive the round trip as the server wrote them.
    let decoded = response.into_inner();
    assert_eq!(decoded.day.to_string(), "2024-02-29");
    assert_eq!(decoded.at.to_string(), "2024-02-29T01:02:03.5+05:30");
    // The newtype is transparent: `time`'s API is one deref away.
    assert_eq!(decoded.at.year(), 2024);
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
    // Bound before generation: the override URL is baked into the generated code, so the port has
    // to be known first. This server runs in *this* process while the generated crate's test
    // process drives the client against it.
    let (override_base, override_server) = mock_server::serve_no_content();
    let override_addr = override_base.strip_prefix("http://").unwrap();

    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        &format!(
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
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "servers_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "servers",
        r##"#![cfg(feature = "blocking")]

mod common;

// The base-URL operation must reach the client's own base, proving the override is scoped to the
// path item that declares it rather than applied to the whole client.
#[test]
fn the_unoverridden_operation_still_uses_the_client_base_url() {
    let (base, base_server) = common::serve_no_content();
    let client = servers_client::BlockingClient::new(&base).unwrap();

    // Goes to the client's base URL.
    client.list_pets().expect("list_pets round-trips");
    let seen = base_server.join().unwrap().text();
    assert!(
        seen.starts_with("GET /pets "),
        "the base server should have served /pets: {seen}"
    );

    // Goes to the override host, which lives in the *parent* test process; reaching it at all is
    // the proof, since this process never bound that port.
    client.upload_item().expect("upload_item round-trips");
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "servers"])
        .status()
        .unwrap();
    assert!(status.success(), "the server-override round-trip must pass");

    let seen = override_server.join().unwrap().text();
    assert!(
        seen.starts_with("POST /upload "),
        "the override host should have served /upload: {seen}"
    );
    assert!(
        seen.contains(&format!("host: {override_addr}")),
        "the request must carry the override host, not the document server: {seen}"
    );
}

/// An operation's `{method}_url` accessor must hand back exactly the URL its method sends the
/// request to: the same path-parameter and query encoding, against the client's base URL with its
/// path prefix, and through a path item's server override. Each URL is compared with the request
/// line a local server then receives, so the accessor cannot drift from the request it describes.
/// A required header parameter shapes no URL, so the accessor does not ask for it, and an `apiKey`
/// query credential rides on the request alone.
#[test]
fn an_operation_url_is_the_url_its_request_is_sent_to() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Urls, version: "1" }
paths:
  /v1/files/{file_id}/stream:
    get:
      operationId: stream_file
      parameters:
        - { name: file_id, in: path, required: true, schema: { type: string } }
        - { name: X-Trace, in: header, required: true, schema: { type: string } }
      responses:
        "204": { description: streamed elsewhere }
  /v1/auth/login:
    get:
      operationId: login
      parameters:
        - { name: redirect, in: query, required: false, schema: { type: string } }
      responses:
        "302": { description: to the identity provider }
  /blobs/{name}:
    servers:
      - url: /edge/
    get:
      operationId: read_blob
      parameters:
        - { name: name, in: path, required: true, schema: { type: string } }
      responses:
        "204": { description: read elsewhere }
  /v1/search:
    get:
      operationId: search
      security: [ { key: [] } ]
      parameters:
        - { name: q, in: query, required: true, schema: { type: string } }
      responses:
        "204": { description: results elsewhere }
components:
  securitySchemes:
    key: { type: apiKey, in: query, name: api_key }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "urls_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(&out, "urls", URL_ACCESSOR_TEST);
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "urls"])
        .status()
        .unwrap();
    assert!(status.success(), "each URL accessor must name the URL sent");
}

const URL_ACCESSOR_TEST: &str = r##"#![cfg(feature = "blocking")]

mod common;

/// Serve `count` requests on a fresh port, answering each with `204`, and return the base URL,
/// with a path prefix, and the request lines in the order they arrived.
fn serve(count: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let (base, server) = common::serve(vec![common::no_content(); count]);
    let lines = std::thread::spawn(move || {
        let requests = server.join().unwrap();
        requests.iter().map(|request| request.line().to_owned()).collect()
    });
    (format!("{base}/api"), lines)
}

/// The request line a request to `url` carries.
fn request_line(url: &reqwest::Url) -> String {
    match url.query() {
        Some(query) => format!("GET {}?{query} HTTP/1.1", url.path()),
        None => format!("GET {} HTTP/1.1", url.path()),
    }
}

#[test]
fn each_url_accessor_names_the_url_its_operation_sends_to() {
    let (base, server) = serve(4);
    let client = urls_client::BlockingClient::new(&base).unwrap();

    let stream = client.stream_file_url("a b/c").unwrap();
    assert_eq!(stream.as_str(), format!("{base}/v1/files/a%20b%2Fc/stream"));
    let login = client
        .login_url(urls_client::LoginParams::default().redirect("/".to_owned()))
        .unwrap();
    assert_eq!(login.as_str(), format!("{base}/v1/auth/login?redirect=%2F"));
    let bare_login = client.login_url(None).unwrap();
    assert_eq!(bare_login.as_str(), format!("{base}/v1/auth/login"));
    let blob = client.read_blob_url("x y").unwrap();
    // The path item's server override replaces the base path, on the client's host.
    let host = base.strip_suffix("/api").unwrap();
    assert_eq!(blob.as_str(), format!("{host}/edge/blobs/x%20y"));

    // The async client's accessor is the one the blocking client forwards to.
    assert_eq!(client.inner().stream_file_url("a b/c").unwrap(), stream);

    client.stream_file("a b/c", "trace").unwrap();
    // `login` documents only `302`, so the server's `204` is an undocumented status: the request
    // line is what is compared, not the outcome.
    let _ = client.login(urls_client::LoginParams::default().redirect("/".to_owned()));
    let _ = client.login(None);
    client.read_blob("x y").unwrap();
    let seen = server.join().unwrap();
    assert_eq!(
        seen,
        vec![
            request_line(&stream),
            request_line(&login),
            request_line(&bare_login),
            request_line(&blob),
        ]
    );
}

/// The request carries the `apiKey` query credential beside its own query; the URL its accessor
/// hands back is that URL less the credential.
#[test]
fn a_url_accessor_leaves_off_the_query_api_key_its_request_sends() {
    let (base, server) = serve(1);
    let client = urls_client::BlockingClient::new(&base).unwrap().with_credential(
        "key",
        urls_client::Credential::ApiKey(urls_client::SecretString::from("s3cret")),
    );

    let url = client.search_url("a b").unwrap();
    assert_eq!(url.as_str(), format!("{base}/v1/search?q=a%20b"));
    client.search("a b").unwrap();
    let seen = server.join().unwrap();
    assert_eq!(seen, vec!["GET /api/v1/search?q=a%20b&api_key=s3cret HTTP/1.1".to_owned()]);
}
"##;

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
    let body = multipart_client::types::RequestBody {
        tags: vec!["blue".into(), "black".into()],
        paths: vec!["a/b".into(), "c".into()],
        names: vec!["ada".into(), "grace".into()],
    };
    client.upload(&body).expect("upload round-trips");

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

/// A CBOR body travels as the exact bytes it is (#900): `application/cbor` and a `+cbor` suffixed
/// type are each sent as their own `Content-Type` and asked for as their own `Accept`, and the
/// response comes back untouched, while an `application/octet-stream` operation still sets no
/// `Accept` of its own.
#[test]
fn cbor_bodies_reach_the_wire_under_their_own_media_type() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
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
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "cbor_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    write_fixture_test(
        &out,
        "cbor",
        r##"#![cfg(feature = "blocking")]

mod common;

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
        let (base, server) = common::serve_once(media, "200 OK", SIGNED);
        let client = cbor_client::BlockingClient::new(&base).unwrap();
        let returned = call(&client, &bytes::Bytes::from_static(SIGNED))
            .expect("the CBOR call round-trips")
            .into_inner();
        let request = server.join().unwrap();
        let head = &request.head;
        assert_eq!(request.header("content-type"), [media], "{head}");
        // The declared type replaces the HTTP stack's default `*/*`, rather than joining it.
        assert_eq!(request.header("accept"), [media], "{head}");
        assert_eq!(request.body, SIGNED, "the request body must be the caller's exact bytes");
        assert_eq!(&returned[..], SIGNED, "the response must be the server's exact bytes");
    }
}

#[test]
fn octet_stream_still_leaves_accept_to_the_http_stack() {
    let (base, server) = common::serve_once("application/octet-stream", "200 OK", SIGNED);
    let client = cbor_client::BlockingClient::new(&base).unwrap();
    client
        .put_blob(&bytes::Bytes::from_static(SIGNED))
        .expect("the octet-stream call round-trips");
    let request = server.join().unwrap();
    let head = &request.head;
    assert_eq!(request.header("content-type"), ["application/octet-stream"], "{head}");
    // The generated method sets no `Accept`; what arrives is the HTTP stack's own default.
    assert_eq!(request.header("accept"), ["*/*"], "{head}");
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "cbor"])
        .status()
        .unwrap();
    assert!(status.success(), "the CBOR wire round-trip must pass");
}

/// An optional nullable JSON `content` parameter has three states on the wire (#996): absent, the
/// JSON `null`, and a value. Its `…Params` field is `Option<Option<T>>`, so `Some(None)` must
/// reach the server as the text `null` in every location that carries one, not as an omission.
#[test]
fn an_optional_nullable_json_content_parameter_sends_an_explicit_null() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Nulls, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - name: filter
          in: query
          content:
            application/json: { schema: { type: [integer, 'null'] } }
        - name: X-Tag
          in: header
          content:
            application/json: { schema: { type: [string, 'null'] } }
        - name: session
          in: cookie
          content:
            application/json: { schema: { type: [integer, 'null'] } }
      responses:
        "204": { description: No Content }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "nulls_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    write_fixture_test(
        &out,
        "nulls",
        r##"#![cfg(feature = "blocking")]

mod common;

use common::header;

/// Send one `listItems` call with `params` and return the lowercased request head it arrived as.
fn request_head(params: nulls_client::ListItemsParams) -> String {
    let (base, server) = common::serve_no_content();
    let client = nulls_client::BlockingClient::new(&base).unwrap();
    client.list_items(params).expect("the call round-trips");
    server.join().unwrap().head.to_ascii_lowercase()
}

#[test]
fn absent_null_and_a_value_are_three_requests() {
    let absent = request_head(nulls_client::ListItemsParams::default());
    assert!(absent.starts_with("get /items http/1.1"), "{absent}");
    assert!(header(&absent, "x-tag").is_empty(), "{absent}");
    assert!(header(&absent, "cookie").is_empty(), "{absent}");

    let null = request_head(
        nulls_client::ListItemsParams::default()
            .filter(None)
            .x_tag(None)
            .session(None),
    );
    assert!(null.starts_with("get /items?filter=null http/1.1"), "{null}");
    assert_eq!(header(&null, "x-tag"), ["null"], "{null}");
    assert_eq!(header(&null, "cookie"), ["session=null"], "{null}");

    let value = request_head(
        nulls_client::ListItemsParams::default()
            .filter(Some(7))
            .x_tag(Some("a".to_owned()))
            .session(Some(3)),
    );
    assert!(value.starts_with("get /items?filter=7 http/1.1"), "{value}");
    assert_eq!(header(&value, "x-tag"), ["\"a\""], "{value}");
    assert_eq!(header(&value, "cookie"), ["session=3"], "{value}");
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "nulls"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the explicit-null wire round-trip must pass"
    );
}

/// RFC 6265 §5.4 allows a request one `Cookie` header. An operation with a cookie parameter whose
/// security alternative names two cookie `apiKey` schemes sends all three cookies in that one
/// header: the parameters first, then each credential in the alternative's order.
#[test]
fn cookie_parameters_and_cookie_api_keys_share_one_cookie_header() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Cookies, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      security: [ { session: [], csrf: [] } ]
      parameters:
        - { name: theme, in: cookie, schema: { type: string } }
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    session: { type: apiKey, in: cookie, name: SESSION }
    csrf: { type: apiKey, in: cookie, name: CSRF }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "cookies_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    write_fixture_test(
        &out,
        "cookies",
        r##"#![cfg(feature = "blocking")]

mod common;

use common::header;

/// Send one `listItems` call with `params` and return the request head it arrived as.
fn request_head(params: cookies_client::ListItemsParams) -> String {
    let (base, server) = common::serve_no_content();
    let client = cookies_client::BlockingClient::new(&base)
        .unwrap()
        .with_credential(
            "session",
            cookies_client::Credential::ApiKey(cookies_client::SecretString::from("s3ss")),
        )
        .with_credential(
            "csrf",
            cookies_client::Credential::ApiKey(cookies_client::SecretString::from("t0k")),
        );
    client.list_items(params).expect("the call round-trips");
    server.join().unwrap().head
}

#[test]
fn every_cookie_rides_in_one_header() {
    let with_param =
        request_head(cookies_client::ListItemsParams::default().theme("dark".to_owned()));
    assert_eq!(
        header(&with_param, "cookie"),
        ["theme=dark; SESSION=s3ss; CSRF=t0k"],
        "{with_param}"
    );

    let without_param = request_head(cookies_client::ListItemsParams::default());
    assert_eq!(
        header(&without_param, "cookie"),
        ["SESSION=s3ss; CSRF=t0k"],
        "{without_param}"
    );
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "cookies"])
        .status()
        .unwrap();
    assert!(status.success(), "every cookie must share one header");
}

/// The mock server ends a chunked request body at its zero-size chunk's trailer section, read
/// through every chunk-size line, and not where the bytes received so far happen to end in
/// `0\r\n\r\n`.
#[test]
fn the_mock_server_ends_a_chunked_body_at_its_zero_size_chunk() {
    // A chunk whose data ends in `0\r\n`, followed by its own CRLF: the bytes end in `0\r\n\r\n`
    // with the body still open.
    let first = b"5\r\nab0\r\n\r\n";
    assert_eq!(mock_server::chunked_body_end(first), None);
    assert_eq!(mock_server::chunked_body_end(b"5\r\nab0"), None);

    let mut whole = first.to_vec();
    whole.extend_from_slice(b"0\r\n\r\n");
    assert_eq!(mock_server::chunked_body_end(&whole), Some(whole.len()));

    // A trailer section after the zero-size chunk, and a chunk extension on a size line.
    let trailers = b"3;name=value\r\nabc\r\n0\r\nX-Checksum: 1\r\n";
    assert_eq!(mock_server::chunked_body_end(trailers), None);
    let mut trailers = trailers.to_vec();
    trailers.extend_from_slice(b"\r\n");
    assert_eq!(
        mock_server::chunked_body_end(&trailers),
        Some(trailers.len())
    );
}

/// A chunked request with a trailer section reaches the mock server's join handle whole, framing
/// included, and is answered.
#[test]
fn the_mock_server_reads_a_chunked_request_through_its_trailers() {
    use std::io::{Read, Write};

    let (base, server) = mock_server::serve_no_content();
    let body = b"5\r\nab0\r\n\r\n0\r\nX-Checksum: 1\r\n\r\n";
    let mut stream = std::net::TcpStream::connect(base.strip_prefix("http://").unwrap()).unwrap();
    stream
        .write_all(b"POST /upload HTTP/1.1\r\nHost: mock\r\nTransfer-Encoding: chunked\r\n\r\n")
        .unwrap();
    stream.write_all(body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 204 No Content"),
        "{response}"
    );

    let request = server.join().unwrap();
    assert_eq!(request.line(), "POST /upload HTTP/1.1");
    assert_eq!(request.header("transfer-encoding"), ["chunked"]);
    assert_eq!(request.body, body);
}
