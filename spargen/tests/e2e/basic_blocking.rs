//! The `tests/blocking.rs` that `generated_module_compiles_in_basic_oas31_crate` writes into the
//! `BASIC_SPEC` fixture crate and runs there.

pub(super) const BLOCKING_TEST: &str = r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

// Prove the BlockingClient performs an actual HTTP round-trip: a blocking method drives the async
// dispatch to completion on the owned current-thread runtime and returns the decoded, typed body.
#[test]
fn blocking_method_round_trips_against_a_mock() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let body = r#"{"ok":"yes"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let base = format!("http://{addr}");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_multi().expect("blocking get_multi round-trips");
    assert_eq!(response.status(), 200);
    match response.into_inner() {
        basic_client::GetMultiResponse::Status200(ok) => assert_eq!(ok.ok, "yes"),
        other => panic!("expected Status200, got {other:?}"),
    }

    // The constructors mirror the async client and the inner client is reachable.
    let _ = client.inner();
    let _ = client.core();

    server.join().unwrap();
}

// Selection never falls through past a chosen alternative whose credential fails, so the remedy
// is to unregister it: `without_credential` on the same client reaches the later, fully registered
// alternative over real HTTP, and the request carries that alternative's credential alone.
#[test]
fn without_credential_falls_through_to_a_later_alternative() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_ascii_lowercase();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });
    let failing_bearer = basic_client::Credential::Provider(std::sync::Arc::new(|| {
        Box::pin(async { Err(basic_client::AuthError::new("idp down")) })
            as basic_client::TokenFuture
    }));
    // `getConjunction` is `tenant + bearer`, or `apiKey`, or `mtls + tenant`: all three registered
    // selects the first, whose bearer provider fails before anything is sent — so the mock sees
    // no connection from this call.
    let client = basic_client::BlockingClient::new(&format!("http://{addr}"))
        .unwrap()
        .with_credential("tenant", basic_client::Credential::ApiKey(basic_client::SecretString::from("acme")))
        .with_credential("bearer", failing_bearer)
        .with_credential("apiKey", basic_client::Credential::ApiKey(basic_client::SecretString::from("k3y")));
    match client.get_conjunction() {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialProvider { scheme, .. },
        )) => assert_eq!(scheme, "bearer"),
        other => panic!("expected the selected alternative's provider failure, got {other:?}"),
    }

    let client = client.without_credential("bearer");
    let response = client.get_conjunction().expect("the apiKey alternative is selected and sent");
    assert_eq!(response.status(), 204);
    let request = server.join().unwrap();
    // The second alternative is `apiKey` alone: the still-registered `tenant` belongs only to
    // alternatives that were not selected, so it must not ride along, and neither may a bearer.
    assert!(request.contains("x-api-key: k3y\r\n"), "{request}");
    assert!(!request.contains("x-tenant"), "{request}");
    assert!(!request.contains("authorization"), "{request}");
}

/// A resolver whose lookup never completes. reqwest's `connect_timeout` bounds the whole connector
/// call, name resolution included, so a lookup that hangs is a connect that hangs: it reaches the
/// same `TimedOut` inside the same connect error a blackholed TCP handshake does, without needing a
/// non-routable address that some networks refuse instead of dropping.
struct NeverResolves;

impl reqwest::dns::Resolve for NeverResolves {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(std::future::pending())
    }
}

// A connect that exceeds the client's `connect_timeout` is `TimeoutKind::Connect` — the server never
// accepted a connection — and not `Total`, which says the whole request ran over its budget.
#[test]
fn an_elapsed_connect_timeout_is_classified_as_connect() {
    let http = reqwest::Client::builder()
        .dns_resolver(std::sync::Arc::new(NeverResolves))
        .connect_timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap();
    let client = basic_client::BlockingClient::with_client(http, "http://never-resolves.invalid").unwrap();
    match client.get_multi() {
        Err(basic_client::Error::Timeout(kind)) => {
            assert_eq!(kind, basic_client::TimeoutKind::Connect);
        }
        other => panic!("expected a connect timeout, got {other:?}"),
    }
}

// The total-request budget elapsing is `TimeoutKind::Total`, including when the connection itself
// succeeded — and a connect timeout configured beside it does not change that.
#[test]
fn an_elapsed_total_timeout_is_classified_as_total() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Accept the connection and hold it open without answering, until the client has given up.
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        while stream.read(&mut buf).map(|read| read > 0).unwrap_or(false) {}
    });

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let client = basic_client::BlockingClient::with_client(http, &format!("http://{addr}")).unwrap();
    let outcome = client.get_multi();
    drop(client);
    match outcome {
        Err(basic_client::Error::Timeout(kind)) => {
            assert_eq!(kind, basic_client::TimeoutKind::Total);
        }
        other => panic!("expected a total timeout, got {other:?}"),
    }

    server.join().unwrap();
}

#[test]
fn typed_parameters_follow_openapi_wire_rules() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        let request_line = request.lines().next().unwrap();
        assert!(request_line.starts_with("GET /params/1,2?"), "{request}");
        assert!(request_line.contains("workflow_id=build.yml"), "{request}");
        assert!(request_line.contains("labels=bug&labels=api"), "{request}");
        // A non-exploded array joins with a literal `,`: the delimiter must stay distinguishable
        // from a comma inside a value, which `%2C` would not be.
        assert!(request_line.contains("compact=one,two"), "{request}");
        assert!(request.contains("x-flags: fast,safe\r\n"), "{request}");
        assert!(request.contains("cookie: session=a; session=b\r\n"), "{request}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let workflow_id: basic_client::types::WorkflowId =
        serde_json::from_str(r#""build.yml""#).unwrap();
    let params = basic_client::SerializeParamsParams::default()
        .labels(vec!["bug".to_owned(), "api".to_owned()])
        .compact(vec!["one".to_owned(), "two".to_owned()])
        .session(vec!["a".to_owned(), "b".to_owned()]);
    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .serialize_params(
            vec![1, 2],
            workflow_id,
            vec!["fast".to_owned(), "safe".to_owned()],
            Some(params),
        )
        .unwrap();

    server.join().unwrap();
}

/// The full RFC 6570 style table on one request line, plus path-value encoding.
///
/// The invariant: a style's delimiters are emitted literally and every data byte is percent-encoded,
/// so a joining `,` stays distinguishable from a `,` inside a value — and a path value can never
/// change the route it is spliced into.
#[test]
fn every_parameter_style_serializes_onto_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap().to_owned();
        let (target, _) = request_line
            .trim_start_matches("GET ")
            .split_once(" HTTP/1.1")
            .unwrap();
        let (path, query) = target.split_once('?').unwrap();

        // matrix, `explode: false`: `;name=v1,v2` — the `;`/`=`/`,` are structure, so the `,`
        // inside the value `a,b` must be `%2C` or the two are indistinguishable.
        // label, `explode: false`: a single `.` prefix and comma-joined members (`.x,y`);
        // `explode: true` would be `.x.y`.
        // The `raw` segment carries a `/`, `?`, `#`, and a stray `%`: all four must be encoded, or
        // the request would address a different route entirely.
        assert_eq!(
            path,
            "/styles/;matrix=one,a%2Cb/.x,y/a%2Fb%3Fc%23d%25e",
            "{request_line}"
        );

        let pairs: Vec<&str> = query.split('&').collect();
        // spaceDelimited/pipeDelimited join with a literal `%20`/`%7C`; RFC 6570 has no bare-space
        // or bare-pipe form, so the delimiter is the encoded triple and data bytes are encoded too.
        assert!(pairs.contains(&"space=one%20a%20b"), "{query}");
        assert!(pairs.contains(&"pipe=one%7Ca%7Cb"), "{query}");
        // deepObject: one `name[property]=value` pair per member, brackets literal.
        assert!(pairs.contains(&"deep%5Bkind%5D=wide"), "{query}");
        assert!(pairs.contains(&"deep%5Blimit%5D=3"), "{query}");
        // `allowReserved: true` is the one place a `/` in a query value survives unencoded.
        assert!(pairs.contains(&"reserved=a/b?c"), "{query}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let params = basic_client::SerializeStylesParams::default()
        .space(vec!["one".to_owned(), "a b".to_owned()])
        .pipe(vec!["one".to_owned(), "a|b".to_owned()])
        .deep(basic_client::types::DeepFilter {
            kind: "wide".to_owned(),
            limit: Some(3),
        })
        .reserved("a/b?c".to_owned());
    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .serialize_styles(
            vec!["one".to_owned(), "a,b".to_owned()],
            vec!["x".to_owned(), "y".to_owned()],
            "a/b?c#d%e".to_owned(),
            Some(params),
        )
        .unwrap();

    server.join().unwrap();
}

/// `content:`-typed path and header parameters on the wire.
///
/// A path value is rendered through its media codec and then percent-encoded as one opaque segment,
/// exactly as a schema-typed one is: neither the text value's `/` nor the JSON string member's
/// `/`, `?`, or `#` may leave the segment it is spliced into, so the request stays under the
/// client's base path. Header values are sent verbatim, as every header value is.
#[test]
fn content_typed_path_and_header_parameters_reach_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();

        assert_eq!(
            request_line,
            "GET /v1/content-params/x%2F..%2F..%2Fadmin/\
             %7B%22kind%22%3A%22a%2F..%2Fb%3Fc%23d%22%2C%22limit%22%3A3%7D HTTP/1.1",
            "{request}"
        );
        assert!(request.contains("x-text: x/../../admin\r\n"), "{request}");
        assert!(
            request.contains("x-json: {\"kind\":\"a/../b?c#d\",\"limit\":3}\r\n"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let filter = || basic_client::types::DeepFilter {
        kind: "a/../b?c#d".to_owned(),
        limit: Some(3),
    };
    let client = basic_client::BlockingClient::new(&format!("http://{addr}/v1")).unwrap();
    client
        .serialize_content_params(
            "x/../../admin".to_owned(),
            filter(),
            "x/../../admin".to_owned(),
            filter(),
        )
        .unwrap();

    server.join().unwrap();
}

/// The exact bytes of an `application/x-www-form-urlencoded` body built from an Encoding Object.
///
/// A property that declares `style` switches to RFC 6570 mode (its `contentType` becomes inert);
/// one that does not stays in media-type mode and is rendered by its declared content type.
#[test]
fn form_urlencoded_body_bytes_follow_the_encoding_object() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");

        assert!(
            request.contains("content-type: application/x-www-form-urlencoded\r\n"),
            "{request}"
        );
        // `name` is a plain text property; `tags` declared `pipeDelimited` so it joins with `%7C`;
        // `blob` declared `application/json` and is therefore a JSON document, form-encoded.
        // A space is `%20`, not `+`: the specification defers to query-parameter serialization
        // (RFC 6570), and every urlencoded parser decodes `%20` and `+` alike.
        assert_eq!(
            body,
            "name=Ada%20Lovelace&tags=a%7Cb&blob=%7B%22kind%22%3A%22wide%22%2C%22limit%22%3A3%7D",
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .submit_form(&basic_client::types::RequestBody75618f63 {
            name: "Ada Lovelace".to_owned(),
            tags: vec!["a".to_owned(), "b".to_owned()],
            blob: basic_client::types::DeepFilter {
                kind: "wide".to_owned(),
                limit: Some(3),
            },
        })
        .unwrap();

    server.join().unwrap();
}

/// Every multipart part carries a resolved `Content-Type`: the specification's defaulting table
/// picks `application/octet-stream` for a binary property, `text/plain` for a scalar, and
/// `application/json` for an object or array. A header an Encoding Object pins rides on its own
/// part beside that `Content-Type`.
#[test]
fn multipart_parts_carry_their_resolved_content_types() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(
            request.contains("content-type: multipart/form-data; boundary="),
            "{request}"
        );
        for (part, content_type) in [
            ("file", "application/octet-stream"),
            ("caption", "text/plain"),
            ("tags", "application/json"),
        ] {
            let name = format!("name=\"{part}\"");
            let position = request.find(&name).unwrap_or_else(|| panic!("{request}"));
            let rest = &request[position..];
            let header = rest.split("\r\n\r\n").next().unwrap();
            assert!(
                header.contains(&format!("Content-Type: {content_type}")),
                "part `{part}` must declare {content_type}: {header}"
            );
            // The one pinned encoding header rides on its own part and on no other.
            assert_eq!(
                header.contains("x-part-kind: caption text"),
                part == "caption",
                "part `{part}`: {header}"
            );
        }

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .upload_file(&basic_client::types::RequestBodyE12d70b5 {
            file: bytes::Bytes::from_static(b"\x00\x01binary"),
            caption: "a caption".to_owned(),
            count: None,
            tags: Some(vec!["x".to_owned()]),
        })
        .unwrap();

    server.join().unwrap();
}

/// The members of a typed `additionalProperties` overflow reach the wire on form-urlencoded and
/// multipart bodies, as they do on JSON (#697): after the declared properties, ordered by key, in
/// their type's default encoding — a text form field for a string, a JSON part under
/// `application/json` for an integer array. An overflow key repeating a declared name is not sent
/// a second time, and on both media the declared property's value is the one sent.
#[test]
fn additional_properties_reach_form_and_multipart_bodies() {
    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .submit_form_additional(&basic_client::types::RequestBody3c5761a3 {
            a: "x".to_owned(),
            additional: [
                ("z".to_owned(), "last".to_owned()),
                ("b c".to_owned(), "y&z".to_owned()),
                ("a".to_owned(), "overflow".to_owned()),
            ]
            .into_iter()
            .collect(),
        })
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /forms/additional HTTP/1.1"), "{request}");
    assert!(request.ends_with("\r\n\r\na=x&b%20c=y%26z&z=last"), "{request}");

    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .upload_additional(&basic_client::types::RequestBody1654af29 {
            caption: "c".to_owned(),
            additional: [
                ("scores".to_owned(), vec![1, 2]),
                ("caption".to_owned(), vec![9]),
            ]
            .into_iter()
            .collect(),
        })
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /upload/additional HTTP/1.1"), "{request}");
    assert_eq!(request.matches("name=\"caption\"").count(), 1, "{request}");
    let caption = &request[request.find("name=\"caption\"").unwrap()..];
    let (_, caption) = caption.split_once("\r\n\r\n").unwrap();
    assert!(caption.starts_with("c\r\n"), "{caption}");
    let position = request
        .find("name=\"scores\"")
        .unwrap_or_else(|| panic!("{request}"));
    let part = &request[position..];
    let (header, value) = part.split_once("\r\n\r\n").unwrap();
    assert!(header.contains("Content-Type: application/json"), "{header}");
    assert!(value.starts_with("[1,2]\r\n"), "{value}");
}

/// A raw byte request body is sent with the `Content-Type` its media key names: the octet gate
/// admits only `bytes::Bytes`, and a `Bytes` body is emitted with the header before the media
/// arms are consulted. This pins that wire fact directly, on the request the server actually reads,
/// for `application/octet-stream` and for a concrete binary family member (`image/png`), which
/// must go out under its own name rather than the generic one.
#[test]
fn a_byte_request_body_declares_its_content_type() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(request.starts_with("PUT /ranged HTTP/1.1"), "{request}");
        assert!(
            request.contains("content-type: application/octet-stream"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\n\
                  Content-Range: bytes 0-2/3\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .put_ranged(&bytes::Bytes::from_static(b"abc"))
        .unwrap();

    server.join().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(request.starts_with("PUT /artwork/poster HTTP/1.1"), "{request}");
        assert!(request.contains("content-type: image/png"), "{request}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .put_artwork("poster", &bytes::Bytes::from_static(b"\x89PNG"))
        .unwrap();

    server.join().unwrap();
}

/// Serve one request with a `204`, handing the whole request (head and `Content-Length` body) back
/// through the join handle.
fn capture_one_request() -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let read = stream.read(&mut buf).unwrap();
            request.extend_from_slice(&buf[..read]);
            let text = String::from_utf8_lossy(&request).into_owned();
            let Some(head_end) = text.find("\r\n\r\n") else {
                assert_ne!(read, 0, "connection closed before the request head: {text}");
                continue;
            };
            let length = text[..head_end]
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            if request.len() >= head_end + 4 + length || read == 0 {
                break;
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{addr}"), server)
}

/// The `content-type` header lines of a captured request.
fn content_type_lines(request: &str) -> Vec<&str> {
    let head = request.split("\r\n\r\n").next().unwrap();
    head.lines()
        .filter(|line| line.to_ascii_lowercase().starts_with("content-type:"))
        .collect()
}

/// A JSON or XML request body goes out under the `Content-Type` its media key declares, exactly
/// once, and is still encoded by its codec: an `application/merge-patch+json` body as JSON, a
/// `text/xml` body as XML, and the canonical `application/json` / `application/xml` keys unchanged.
#[test]
fn a_json_or_xml_request_body_declares_its_content_type() {
    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .patch_merge_document(
            "d1",
            &basic_client::types::JsonMeta {
                id: 7,
                sku: "S".to_owned(),
                note: None,
            },
        )
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.starts_with("PATCH /merge-patch/d1 HTTP/1.1"), "{request}");
    assert_eq!(
        content_type_lines(&request),
        ["content-type: application/merge-patch+json"],
        "{request}"
    );
    assert!(request.ends_with(r#"{"id":7,"sku":"S"}"#), "{request}");

    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .post_json_meta(&basic_client::types::JsonMeta {
            id: 7,
            sku: "S".to_owned(),
            note: None,
        })
        .unwrap_err();
    let request = server.join().unwrap();
    assert_eq!(
        content_type_lines(&request),
        ["content-type: application/json"],
        "{request}"
    );

    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .submit_legacy_order(&basic_client::types::XmlOrder {
            id: 42,
            sku: "ABC".to_owned(),
        })
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /xml/legacy-order HTTP/1.1"), "{request}");
    assert_eq!(content_type_lines(&request), ["content-type: text/xml"], "{request}");
    assert!(request.contains("<sku>ABC</sku>"), "{request}");

    let (base, server) = capture_one_request();
    let client = basic_client::BlockingClient::new(&base).unwrap();
    client
        .submit_order(&basic_client::types::XmlOrder {
            id: 42,
            sku: "ABC".to_owned(),
        })
        .unwrap_err();
    let request = server.join().unwrap();
    assert_eq!(
        content_type_lines(&request),
        ["content-type: application/xml"],
        "{request}"
    );
}

#[test]
fn required_path_query_parameter_is_not_shadowed_by_codegen_local() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert_eq!(
            request.lines().next(),
            Some("GET /files?path=%2Ftmp%2Fexample.txt HTTP/1.1"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .read_file("/tmp/example.txt".to_owned())
        .expect("read_file sends the caller-provided path query value");

    server.join().unwrap();
}

fn serve_once(content_type: &str, status: &str, body: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let content_type = content_type.to_owned();
    let status = status.to_owned();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{addr}"), server)
}

#[test]
fn textual_vendor_and_binary_responses_use_raw_wire_codecs() {
    let (base, server) = serve_once("text/html", "200 OK", b"<p>Hello</p>");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.render_html().unwrap().into_inner(), "<p>Hello</p>");
    server.join().unwrap();

    let (base, server) = serve_once(
        "application/octocat-stream",
        "200 OK",
        b" /\\_/\\\n( o.o )",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.get_octocat().unwrap().into_inner(), " /\\_/\\\n( o.o )");
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "200 OK", b"\0raw\xff");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.download_raw().unwrap().into_inner().as_ref(), b"\0raw\xff");
    server.join().unwrap();
}

#[test]
#[allow(deprecated)]
fn textual_documented_errors_decode_without_json_quotes() {
    let (base, server) = serve_once("text/plain", "400 Bad Request", b"plain failure");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_text_error().unwrap_err() {
        // `.0` unwraps the documented-error newtype; the same value is one `Deref` away, and
        // `String::from(..)` converts. The newtype is what makes `Error<E>` a `std::error::Error`.
        basic_client::Error::Api(response) => {
            assert_eq!(response.into_inner().0, "plain failure")
        }
        other => panic!("expected typed textual API error, got {other:?}"),
    }
    server.join().unwrap();
}

#[test]
fn multi_status_dispatch_uses_each_status_media_codec() {
    let (base, server) = serve_once("text/plain", "200 OK", b"plain success");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap().into_inner() {
        basic_client::GetRawMultiResponse::Status200(body) => {
            assert_eq!(body.as_str(), "plain success")
        }
        other => panic!("expected text success variant, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "201 Created", b"raw success");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap().into_inner() {
        basic_client::GetRawMultiResponse::Status201(body) => {
            assert_eq!(&body[..], b"raw success")
        }
        other => panic!("expected binary success variant, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "409 Conflict", b"raw failure");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetRawMultiError::Status409(body) => {
                assert_eq!(&body[..], b"raw failure")
            }
            other => panic!("expected binary error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #115: with no success status declared, `default` documents a 2xx, so its body is decoded
// as the success value rather than discarded behind `Ok(())`; a non-2xx it covers is still the
// error enum's `Default` variant, and the declared `404` keeps its own.
#[test]
fn default_types_a_2xx_when_no_success_status_is_declared() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"kind":"ok","limit":3}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let body = client.get_no_success().unwrap().into_inner();
    assert_eq!(body.kind, "ok");
    assert_eq!(body.limit, Some(3));
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", br#"{"kind":"boom"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_no_success().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetNoSuccessError::Default(body) => assert_eq!(body.kind, "boom"),
            other => panic!("expected the default error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("text/plain", "404 Not Found", b"gone");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_no_success().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetNoSuccessError::Status404(body) => assert_eq!(body.as_str(), "gone"),
            other => panic!("expected the 404 error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();
}

// `getMultiDefault` documents 200 and 201 with distinct bodies plus a bodied `default`. The
// matches below are exhaustive with no wildcard, so a `default` routed into the success enum
// fails to compile here; the undocumented 202 proves it is no success fallback at run time either.
#[test]
fn default_beside_multiple_bodied_successes_stays_on_the_error_side() {
    let (base, server) = serve_once("application/json", "201 Created", br#"{"id":7}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap().into_inner() {
        basic_client::GetMultiDefaultResponse::Status200(body) => {
            panic!("201 decoded as the 200 variant: {body:?}")
        }
        basic_client::GetMultiDefaultResponse::Status201(body) => assert_eq!(body.id, 7),
    }
    server.join().unwrap();

    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;
    let (base, server) = serve_once("application/json", "202 Accepted", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 202);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 2xx must not decode as `default`, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetMultiDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #151: beside a declared success status, `default` satisfies no undeclared 2xx in any of
// the three success shapes. The enum shape is pinned above (`getMultiDefault`'s 202 is
// `UnexpectedStatus`); here the plain and unit shapes take an undeclared 201 as their one success,
// and a `default`-shaped 201 body is never decoded as `Problem`.
#[test]
fn an_undeclared_2xx_is_never_decoded_through_default_beside_a_declared_success() {
    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;

    // Plain: the 201 is the single success body, `MultiOk`.
    let (base, server) = serve_once("application/json", "201 Created", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_plain_default().expect("any 2xx is the plain success");
    assert_eq!(response.status(), 201);
    assert_eq!(response.into_inner().ok, "yes");
    server.join().unwrap();

    // A `Problem` body on that 201 is a malformed `MultiOk`, not a `default` decode.
    let (base, server) = serve_once("application/json", "201 Created", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_plain_default().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, .. } => {
            assert_eq!(status, 201);
            assert_eq!(headers.get("content-type").unwrap(), "application/json");
            assert_eq!(&body[..], problem);
        }
        other => panic!("a 2xx must decode as the declared success, got {other:?}"),
    }
    server.join().unwrap();

    // Unit: the 201 is `()`, its `Problem` body discarded rather than decoded.
    let (base, server) = serve_once("application/json", "201 Created", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_unit_default().expect("any 2xx is the unit success");
    assert_eq!(response.status(), 201);
    let () = response.into_inner();
    server.join().unwrap();

    // Both still classify a non-2xx through `default`.
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_plain_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetPlainDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_unit_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetUnitDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();
}

// Issues #127 and #204: a bodyless error entry beside exactly one error body is its own unit
// variant — a bodyless `default`, `403`, or `304` alike — where the single-body newtype dropped it,
// so a documented status arrived as `Error::UnexpectedStatus` or, under a bodied `default`, had its
// empty body decoded as that model. Every match is exhaustive with no wildcard, so a variant
// appearing or vanishing fails to compile.
#[test]
fn a_bodyless_error_entry_beside_one_error_body_is_its_own_variant() {
    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;
    let (base, server) = serve_once("application/json", "404 Not Found", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 404);
            match response.into_inner() {
                basic_client::GetBodylessDefaultError::Status404(body) => {
                    assert_eq!(body.title, "t")
                }
                basic_client::GetBodylessDefaultError::Default => {
                    panic!("a 404 took the bodyless `default`")
                }
            }
        }
        other => panic!("expected the typed 404 error body, got {other:?}"),
    }
    server.join().unwrap();

    // A `500` is the documented bodyless `default`: `Api`, and its body is never parsed as the
    // `404`'s `Problem`.
    for body in [problem, b"".as_slice()] {
        let (base, server) = serve_once("application/json", "500 Internal Server Error", body);
        let client = basic_client::BlockingClient::new(&base).unwrap();
        match client.get_bodyless_default().unwrap_err() {
            basic_client::Error::Api(response) => {
                assert_eq!(response.status(), 500);
                match response.into_inner() {
                    basic_client::GetBodylessDefaultError::Default => {}
                    basic_client::GetBodylessDefaultError::Status404(body) => {
                        panic!("a 500 decoded as the documented 404: {body:?}")
                    }
                }
            }
            other => panic!("expected the unit `Default` error variant, got {other:?}"),
        }
        server.join().unwrap();
    }

    // The issue's repro: a documented bodyless `403` is `Api(Status403)`, not `UnexpectedStatus`.
    let (base, server) = serve_once("application/json", "403 Forbidden", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 403);
            match response.into_inner() {
                basic_client::GetBodylessSiblingError::Status403 => {}
                basic_client::GetBodylessSiblingError::Status404(body) => {
                    panic!("a 403 decoded as the 404: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Status403` error variant, got {other:?}"),
    }
    server.join().unwrap();
    // The bodied `404` still decodes, and an undocumented `500` is still unexpected.
    let (base, server) = serve_once("application/json", "404 Not Found", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetBodylessSiblingError::Status404(body) => assert_eq!(body.title, "t"),
            basic_client::GetBodylessSiblingError::Status403 => panic!("a 404 took the 403"),
        },
        other => panic!("expected the typed 404 error body, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 500 must stay unexpected, got {other:?}"),
    }
    server.join().unwrap();

    // The same shape with an XML error body: the `404` arm decodes XML, the `403` reads nothing.
    let (base, server) = serve_once(
        "application/xml",
        "404 Not Found",
        b"<XmlReceipt><ReceiptCode>A1</ReceiptCode></XmlReceipt>",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetXmlBodylessSiblingError::Status404(body) => {
                assert_eq!(body.code, "A1")
            }
            basic_client::GetXmlBodylessSiblingError::Status403 => panic!("a 404 took the 403"),
        },
        other => panic!("expected the typed XML 404 error body, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/xml", "403 Forbidden", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetXmlBodylessSiblingError::Status403 => {}
            basic_client::GetXmlBodylessSiblingError::Status404(body) => {
                panic!("a 403 decoded as the 404: {body:?}")
            }
        },
        other => panic!("expected the unit `Status403` error variant, got {other:?}"),
    }
    server.join().unwrap();

    // A bodyless `304` beside a bodied `default`: its own variant, the empty body never decoded.
    let (base, server) = serve_once("application/json", "304 Not Modified", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_conditional().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 304);
            match response.into_inner() {
                basic_client::GetConditionalError::Status304 => {}
                basic_client::GetConditionalError::Default(body) => {
                    panic!("a 304 decoded as the `default` body: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Status304` error variant, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_conditional().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetConditionalError::Default(body) => assert_eq!(body.title, "t"),
            basic_client::GetConditionalError::Status304 => panic!("a 500 took the 304"),
        },
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_default_multi().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            match response.into_inner() {
                basic_client::GetBodylessDefaultMultiError::Default => {}
                basic_client::GetBodylessDefaultMultiError::Status404(body)
                | basic_client::GetBodylessDefaultMultiError::Status409(body) => {
                    panic!("a 500 decoded as a documented status: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Default` error variant, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #121: `getMaybeEmpty` documents a bodied 200 and a bodyless 204. The 204 is its own unit
// variant, read without parsing the empty body, where a plain `MultiOk` returned `Error::Decode`.
// The matches are exhaustive with no wildcard, so a variant appearing or vanishing fails to compile.
#[test]
fn a_bodyless_success_beside_one_body_is_its_own_variant() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_maybe_empty().unwrap().into_inner() {
        basic_client::GetMaybeEmptyResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        basic_client::GetMaybeEmptyResponse::Status204 => panic!("a 200 decoded as the 204"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "204 No Content", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_maybe_empty().expect("a documented 204 is a success");
    assert_eq!(response.status(), 204);
    match response.into_inner() {
        basic_client::GetMaybeEmptyResponse::Status204 => {}
        basic_client::GetMaybeEmptyResponse::Status200(body) => {
            panic!("a 204 decoded as the 200: {body:?}")
        }
    }
    server.join().unwrap();

    // An undocumented 2xx matches neither variant: preserved raw, never decoded as `MultiOk`.
    let (base, server) = serve_once("application/json", "202 Accepted", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_maybe_empty().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, .. } => assert_eq!(status, 202),
        other => panic!("expected an unexpected-status error, got {other:?}"),
    }
    server.join().unwrap();

    // The XML variant of the same shape decodes its one body through the XML codec.
    let (base, server) = serve_once(
        "application/xml",
        "200 OK",
        b"<XmlReceipt><ReceiptCode>A1</ReceiptCode></XmlReceipt>",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_maybe_empty().unwrap().into_inner() {
        basic_client::GetXmlMaybeEmptyResponse::Status200(body) => assert_eq!(body.code, "A1"),
        basic_client::GetXmlMaybeEmptyResponse::Status204 => panic!("a 200 decoded as the 204"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/xml", "204 No Content", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_maybe_empty().unwrap().into_inner() {
        basic_client::GetXmlMaybeEmptyResponse::Status204 => {}
        basic_client::GetXmlMaybeEmptyResponse::Status200(body) => {
            panic!("a 204 decoded as the 200: {body:?}")
        }
    }
    server.join().unwrap();
}

// `getRanged` documents an exact 200 and an overlapping 2XX range with different bodies. A 200
// matches both and must take the exact arm; a 202 matches only the range. Executed, not just
// ordered in the emitted text.
#[test]
fn success_dispatch_takes_the_exact_arm_before_an_overlapping_range() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap().into_inner() {
        basic_client::GetRangedResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        basic_client::GetRangedResponse::Status2xx(body) => {
            panic!("a 200 took the range arm: {body:?}")
        }
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "202 Accepted", br#"{"id":7}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_ranged().unwrap();
    assert_eq!(response.status(), 202);
    match response.into_inner() {
        basic_client::GetRangedResponse::Status2xx(body) => assert_eq!(body.id, 7),
        basic_client::GetRangedResponse::Status200(body) => {
            panic!("a 202 took the exact 200 arm: {body:?}")
        }
    }
    server.join().unwrap();

    // A body the matched arm cannot parse is `Error::Decode` at that status, with the headers
    // (#268) and the body kept.
    let (base, server) = serve_once("text/plain", "202 Accepted", b"not json");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, .. } => {
            assert_eq!(status, 202);
            assert_eq!(headers.get("content-type").unwrap(), "text/plain");
            assert_eq!(&body[..], b"not json");
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #689: a multi-success operation retains at most `max_error_body` bytes (64 KiB by
// default) of a body it keeps in an error, as every single-success and error path does, and says
// so through `truncated`. Before, it read through an uncapped helper and kept the whole body.
static OVER_CAP: [u8; 64 * 1024 + 4096] = [b'x'; 64 * 1024 + 4096];

#[test]
fn a_multi_success_operation_caps_the_body_it_keeps_in_an_error() {
    let cap = 64 * 1024;

    // A matched arm whose body does not decode: read whole (decoding needs it), retained capped.
    let (base, server) = serve_once("text/plain", "202 Accepted", &OVER_CAP);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap_err() {
        basic_client::Error::Decode { status, body, truncated, .. } => {
            assert_eq!(status, 202);
            assert_eq!(body.len(), cap);
            assert!(truncated, "a body cut at the cap must say so");
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();

    // An undocumented success status matches no arm: read capped, retained capped.
    let (base, server) = serve_once("text/plain", "202 Accepted", &OVER_CAP);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_maybe_empty().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 202);
            assert_eq!(body.len(), cap);
        }
        other => panic!("expected an unexpected-status error, got {other:?}"),
    }
    server.join().unwrap();

    // A body under the cap is kept whole and reported untruncated.
    let (base, server) = serve_once("text/plain", "202 Accepted", b"not json");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap_err() {
        basic_client::Error::Decode { body, truncated, .. } => {
            assert_eq!(&body[..], b"not json");
            assert!(!truncated);
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #128: the error-side counterpart of the test above, driven through the emitted dispatch
// rather than a hand-written stand-in. `getErrorRanged` documents an exact 409 and an overlapping
// 4XX range, declared range first. A 409 matches both and must take the exact arm; a 404 matches
// only the range; a 500 matches neither; and a body the matched arm cannot parse is `Decode`. The
// matches are exhaustive with no wildcard, so a variant appearing or vanishing fails to compile.
#[test]
fn error_dispatch_takes_the_exact_arm_before_an_overlapping_range() {
    let (base, server) = serve_once("application/json", "409 Conflict", br#"{"detail":"dup"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 409);
            match response.into_inner() {
                basic_client::GetErrorRangedError::Status409(body) => assert_eq!(body.detail, "dup"),
                basic_client::GetErrorRangedError::Status4xx(body) => {
                    panic!("a 409 took the range arm: {body:?}")
                }
            }
        }
        other => panic!("expected a typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "404 Not Found", br#"{"reason":"gone"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 404);
            match response.into_inner() {
                basic_client::GetErrorRangedError::Status4xx(body) => assert_eq!(body.reason, "gone"),
                basic_client::GetErrorRangedError::Status409(body) => {
                    panic!("a 404 took the exact 409 arm: {body:?}")
                }
            }
        }
        other => panic!("expected a typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let problem: &'static [u8] = br#"{"reason":"boom","detail":"boom"}"#;
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 500 must not decode as a documented body, got {other:?}"),
    }
    server.join().unwrap();

    // Issue #268: a documented status whose body does not decode (here an HTML page, as a proxy
    // in front of the server would send) keeps its status and headers on `Decode`.
    let (base, server) = serve_once("text/html", "409 Conflict", b"not json");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, truncated, .. } => {
            assert_eq!(status, 409);
            assert_eq!(headers.get("content-type").unwrap(), "text/html");
            assert_eq!(&body[..], b"not json");
            assert!(!truncated);
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();
}
"##;
