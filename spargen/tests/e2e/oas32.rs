//! The fixture crate generated from `OAS32_SPEC`, and an omit overlay removing an unsupported
//! operation.

use super::*;

#[test]
fn generated_module_compiles_in_oas32_crate_with_query_method() {
    // OpenAPI 3.2 lowers through the same frontend. This spec exercises the new fixed `QUERY`
    // method (which must emit a real client method) alongside a plain `get`, and must produce a
    // module in an application-owned crate that passes `cargo check` + `cargo clippy -D warnings`.
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, OAS32_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "oas32_client");

    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics()
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let generated_path = out.join("src/lib.rs");
    let mut generated = std::fs::read_to_string(&generated_path).unwrap();
    generated.push_str(
        r#"
#[allow(dead_code)]
fn assert_standard_stream<S: futures_core::Stream<Item = Result<types::AdminEvent, StreamError>>>() {}
#[allow(dead_code)]
fn assert_generated_stream_surface() {
    assert_standard_stream::<EventStream<types::AdminEvent>>();
}
#[allow(dead_code)]
fn assert_reconnect_policy_is_public<P: ReconnectPolicy>() {}
"#,
    );
    std::fs::write(&generated_path, &generated).unwrap();

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(status.success());

    let status = fixture_cargo(&out)
        .args(["clippy", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(status.success());

    // The QUERY operation lowered to a real client method (compile-verified above); prove the
    // method exists in the emitted source so a regression that drops QUERY is caught.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub async fn search_records"),
        "QUERY operation should emit a client method"
    );
    assert!(
        generated.contains("reqwest::Method::from_bytes(b\"QUERY\")"),
        "QUERY method should be built from its token bytes"
    );
    // 3.2 `additionalOperations`: a custom method token also emits a real client method, built
    // from its exact bytes rather than mapped onto a fixed `reqwest::Method` constant.
    assert!(
        generated.contains("pub async fn purge_cache"),
        "an additionalOperations method should emit a client method"
    );
    assert!(
        generated.contains("reqwest::Method::from_bytes(b\"PURGE\")"),
        "a custom method should be built from its token bytes"
    );

    // The OpenAPI 3.2 streaming response recognizes the standard SSE envelope annotation and types
    // the stream as its JSON `data.contentSchema`, not as the envelope object.
    let flat: String = generated.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("pub async fn stream_events"),
        "streaming operation should emit a client method"
    );
    assert!(
        flat.contains("support::EventStream<types::AdminEvent>"),
        "SSE contentSchema must type EventStream as the JSON payload: {flat}"
    );

    // Wire-level 3.2 coverage: `in: querystring`, `style: cookie`, typed server variables, and a
    // typed response-header accessor, all against a real socket. Without this the 3.2 constructs
    // are only compile-verified, and a construct can compile while sending the wrong bytes.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/wire.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn oas32_constructs_reach_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();

        // `in: querystring` with `content: application/x-www-form-urlencoded` serializes the whole
        // object into the query string.
        assert!(request_line.starts_with("GET /records?term="), "{request}");
        assert!(request_line.contains("term=a%20b"), "{request}");
        // `style: cookie` sends the value verbatim — the one cookie style that never encodes.
        assert!(request.contains("cookie: session=a/b\r\n"), "{request}");

        let body = r#"[{"id":"r1"}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Total-Count: 42\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    // A templated server resolves with every variable at its declared default, and the typed enum
    // makes an out-of-enum region unconstructible.
    assert_eq!(oas32_client::servers::default_url(), "https://us.example.com/v1");
    assert_eq!(
        oas32_client::servers::Server0::new()
            .region(oas32_client::servers::Server0Region::Eu)
            .version("v2")
            .url(),
        "https://eu.example.com/v2"
    );

    let params = oas32_client::ListRecordsParams::default()
        .filter(oas32_client::types::Query { term: Some("a b".to_owned()) })
        .session("a/b".to_owned());
    let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    let response = client.list_records(Some(params)).unwrap();

    // Documented response headers are read through a typed accessor, as an explicit second step —
    // a malformed header can never turn a successful call into a failure.
    let headers = oas32_client::ListRecordsStatus200Headers::from_response(&response).unwrap();
    assert_eq!(headers.x_total_count, 42);
    assert_eq!(response.into_inner()[0].id, "r1");

    server.join().unwrap();
}

/// `in: querystring` with a JSON `content:` entry owns the whole query string: the serialized
/// value, percent-encoded as one token, so none of its `&`, `=`, or `#` splits or ends the query.
/// An absent optional value sends no query at all.
#[test]
fn a_json_querystring_parameter_is_one_encoded_query() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for expected in [
            "GET /lookup?%7B%22term%22%3A%22a%26b%3Dc%23d%22%7D HTTP/1.1",
            "POST /lookup?%7B%22term%22%3A%22x%20y%22%7D HTTP/1.1",
            "POST /lookup HTTP/1.1",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);
            let request_line = request.lines().next().unwrap();

            assert_eq!(request_line, expected, "{request}");

            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.flush().unwrap();
        }
    });

    let query = |term: &str| oas32_client::types::Query { term: Some(term.to_owned()) };
    let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client.lookup_records(query("a&b=c#d")).unwrap();
    client
        .lookup_records_maybe(Some(
            oas32_client::LookupRecordsMaybeParams::default().filter(query("x y")),
        ))
        .unwrap();
    client.lookup_records_maybe(None).unwrap();

    server.join().unwrap();
}

/// `additionalOperations` on the wire, plus the two constructs its operation is built from: a
/// `components.mediaTypes` reference supplying the request body, and a discriminated union whose
/// `defaultMapping` catches an unrecognized tag.
#[test]
fn oas32_custom_method_and_discriminator_fallback_reach_the_wire() {
    for (tag, expect_unknown) in [("purged", false), ("something-else", true)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);

            // The custom method token travels verbatim; nothing maps it onto a fixed method.
            assert!(
                request.starts_with("PURGE /records/cache "),
                "custom method must reach the wire unchanged: {request}"
            );
            // The body came from the reusable `components.mediaTypes` entry, typed as `Query`.
            assert!(request.contains(r#"{"term":"drop"}"#), "{request}");

            let body = if expect_unknown {
                format!(r#"{{"outcome":"{tag}","detail":"not recognized"}}"#)
            } else {
                r#"{"outcome":"purged","entries":7}"#.to_owned()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
        let outcome = client
            .purge_cache(&oas32_client::types::Query { term: Some("drop".to_owned()) })
            .unwrap()
            .into_inner();

        match outcome {
            // An unmapped discriminator value decodes into the `defaultMapping` branch rather
            // than failing, which is the whole point of the 3.2 field.
            oas32_client::types::CacheOutcome::CacheUnknown(unknown) => {
                assert!(expect_unknown, "mapped tag must not fall back: {unknown:?}");
                assert_eq!(unknown.detail, "not recognized");
            }
            oas32_client::types::CacheOutcome::CachePurged(purged) => {
                assert!(!expect_unknown, "unmapped tag must fall back");
                assert_eq!(purged.entries, 7);
            }
        }

        server.join().unwrap();
    }
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "wire"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the OpenAPI 3.2 wire round-trip must pass with --features blocking"
    );
}

#[test]
fn omit_overlay_removes_unsupported_operation() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC_WITH_UNSUPPORTED_OPERATION).unwrap();
    let out = temp.path().join("client.rs");
    let config = Spec::new(Utf8PathBuf::from_path_buf(spec).unwrap())
        .omit(spargen::omit! {
            operations {
                post "/upload";
            }
        })
        .build(Utf8PathBuf::from_path_buf(out).unwrap())
        .cargo(CargoIntegration::Off);

    let report = spargen::generate(&config);

    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == Code::OmittedConstruct));
}

const SPEC_WITH_UNSUPPORTED_OPERATION: &str = r#"
openapi: 3.1.0
info:
  title: Upload
  version: 1.0.0
paths:
  /health:
    get:
      responses:
        "204":
          description: No Content
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
      responses:
        "204":
          description: No Content
"#;
