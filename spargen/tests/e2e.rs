use std::process::Command;

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget};

#[test]
fn generates_standalone_crate_for_basic_oas31_api() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Crate {
            dir: Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
            name: "basic_client".to_owned(),
        },
    ));

    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let status = Command::new("cargo")
        .arg("check")
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new("cargo")
        .args(["clippy", "--", "-D", "warnings"])
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    // Issue #19: the synchronous `BlockingClient` is USER-opt-in, so the generated manifest always
    // DECLARES a `blocking` feature wired to an OPTIONAL tokio (`rt` only) — but the default feature
    // set must not enable it, keeping the default dependency set unchanged (no tokio direct dep).
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains(r#"blocking = ["dep:tokio"]"#),
        "manifest must declare the blocking feature: {manifest}"
    );
    assert!(
        manifest.contains(r#"tokio = { version = "1", features = ["rt"], optional = true }"#),
        "tokio must be an optional dependency: {manifest}"
    );
    assert!(
        !manifest.contains(r#"default = ["uuid", "time", "blocking"]"#)
            && manifest.contains(r#"default = ["uuid", "time"]"#),
        "blocking must NOT be in the default feature set: {manifest}"
    );
    // The `BlockingClient` and every blocking method are emitted behind `#[cfg(feature = "blocking")]`
    // so a default build compiles them out entirely — there is no `BlockingClient` without the opt-in.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub struct BlockingClient"),
        "BlockingClient must be emitted"
    );
    assert!(
        generated.contains("#[cfg(feature = \"blocking\")]"),
        "BlockingClient must be feature-gated"
    );

    // A real round-trip driven by a blocking method against a std-thread mock server (the generated
    // crate is not inside an async runtime, so building a `BlockingClient` here is valid). Gated on
    // the `blocking` feature so the default `cargo test` compiles it to nothing.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/blocking.rs"),
        r##"#![cfg(feature = "blocking")]

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
"##,
    )
    .unwrap();

    // Prove the wired serde defaults actually deserialize: an absent optional field with a
    // representable scalar default fills in the default instead of `None`, while a required field
    // (default rustdoc-only) still comes from the payload.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/defaults.rs"),
        r##"
#[test]
fn absent_optional_fields_use_schema_defaults() {
    let settings: basic_client::types::Settings =
        serde_json::from_str(r#"{"retries": 7}"#).unwrap();
    assert_eq!(settings.color.as_deref(), Some("red"));
    assert_eq!(settings.enabled, Some(true));
    assert_eq!(settings.ratio, Some(1.5));
    assert_eq!(settings.retries, 7);
    assert_eq!(settings.mode, Some(basic_client::types::Mode::Auto));
}

#[test]
fn pattern_properties_capture_into_typed_overflow_map() {
    // The declared `host` field is typed; every non-declared property is captured by the flatten
    // `BTreeMap<String, String>` overflow that `patternProperties` lowered to.
    let headers: basic_client::types::Headers =
        serde_json::from_str(r#"{"host": "h", "x-a": "1", "x-b": "2"}"#).unwrap();
    assert_eq!(headers.host.as_deref(), Some("h"));
    assert_eq!(headers.additional.get("x-a").map(String::as_str), Some("1"));
    assert_eq!(headers.additional.get("x-b").map(String::as_str), Some("2"));
}

#[test]
fn null_mixed_enum_field_is_option_of_enum() {
    // The null-mixed `Priority` enum lowered to a real Rust enum used behind `Option`: an absent
    // field and an explicit `null` both deserialize to `None`; a string value to the variant.
    let absent: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n"}"#).unwrap();
    assert_eq!(absent.priority, None);

    let explicit_null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": null}"#).unwrap();
    assert_eq!(explicit_null.priority, None);

    let set: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": "high"}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
}

#[test]
fn component_nullability_propagates_through_ref() {
    // A REQUIRED field referencing the nullable `Priority` component is `Option<Priority>`: the key
    // must be present, but `null` deserializes to `None` and a string to the variant. This only
    // holds if the component's nullability propagated to the `$ref` use site.
    let null_priority: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": null, "history": []}"#).unwrap();
    assert_eq!(null_priority.priority, None);

    // An array of the nullable component is `Vec<Option<Priority>>`: a `null` element is accepted.
    let set: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": "high", "history": ["low", null]}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
    assert_eq!(
        set.history,
        vec![Some(basic_client::types::Priority::Low), None]
    );
}

#[test]
fn all_of_merged_struct_carries_every_member_field() {
    // `Account` merged a `$ref` base (id, required), an inline member (label, required) and a
    // sibling property (owner, optional). Required fields are plain, the optional is `Option`, and a
    // payload carrying all three deserializes into the single flattened struct.
    let account: basic_client::types::Account =
        serde_json::from_str(r#"{"id": "a1", "label": "L", "owner": "o"}"#).unwrap();
    assert_eq!(account.id, "a1");
    assert_eq!(account.label, "L");
    assert_eq!(account.owner.as_deref(), Some("o"));
}

#[test]
fn discriminated_union_round_trips_with_tag() {
    // Cat DECLARES `petType` as a required property — the shape that broke serde internal tagging
    // ("missing field petType"). The custom buffer-to-Value Deserialize hands the WHOLE value to the
    // variant, so Cat's own `pet_type` field is filled, and re-serialization keeps the tag.
    let pet: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "cat", "name": "Whiskers"}"#).unwrap();
    match &pet {
        basic_client::types::Pet::Cat(cat) => {
            assert_eq!(cat.name, "Whiskers");
            assert_eq!(cat.pet_type, "cat");
        }
        other => panic!("expected Cat variant, got {other:?}"),
    }
    let json = serde_json::to_value(&pet).unwrap();
    assert_eq!(json["petType"], "cat");
    assert_eq!(json["name"], "Whiskers");

    // Dog does NOT declare `petType`; the custom Serialize re-inserts the tag it would otherwise
    // lack, and deserialization still routes by the tag.
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "dog", "bark": true}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    let json = serde_json::to_value(&dog).unwrap();
    assert_eq!(json["petType"], "dog");
    assert_eq!(json["bark"], true);
}

#[test]
fn nullable_variant_union_resolves_null_at_option() {
    // A `null` payload resolves at the outer `Option` (variant nullability hoisted to the union),
    // and non-null string/array content routes to the right disjoint variant and re-serializes as a
    // bare value.
    let null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": null}"#).unwrap();
    assert!(null.notes.is_none());

    let text: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": "hi"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&text.notes).unwrap(),
        serde_json::json!("hi")
    );

    let list: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": ["a", "b"]}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&list.notes).unwrap(),
        serde_json::json!(["a", "b"])
    );
}

#[test]
fn disjoint_union_round_trips_without_wrapper() {
    // A `string` payload deserializes to the string variant and re-serializes as a BARE string —
    // no tag, no wrapper (Issue #9, strategy B custom Serialize).
    let text: basic_client::types::StringOrList =
        serde_json::from_str(r#""hello""#).unwrap();
    assert_eq!(serde_json::to_string(&text).unwrap(), r#""hello""#);

    // An `array` payload deserializes to the array variant and re-serializes as a bare array.
    let list: basic_client::types::StringOrList =
        serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(serde_json::to_string(&list).unwrap(), r#"["a","b"]"#);
}

#[test]
fn multi_status_response_enums_carry_typed_variants() {
    // Issue #10: the two success statuses lowered to a `GetMultiResponse` enum and the two error
    // statuses to a `GetMultiError` enum, each variant carrying that status's typed body. The
    // variants deserialize their bodies (the same `serde_json::from_slice` the generated dispatch
    // runs after selecting by HTTP status), proving the types are real and payload-carrying — not
    // `serde_json::Value`.
    let ok: basic_client::types::MultiOk = serde_json::from_str(r#"{"ok":"yes"}"#).unwrap();
    match basic_client::GetMultiResponse::Status200(ok) {
        basic_client::GetMultiResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        other => panic!("expected Status200, got {other:?}"),
    }
    let created: basic_client::types::MultiCreated =
        serde_json::from_str(r#"{"id":7}"#).unwrap();
    match basic_client::GetMultiResponse::Status201(created) {
        basic_client::GetMultiResponse::Status201(body) => assert_eq!(body.id, 7),
        other => panic!("expected Status201, got {other:?}"),
    }
    // The documented bodyless 204 is a payload-free unit variant (carries no body).
    assert!(matches!(
        basic_client::GetMultiResponse::Status204,
        basic_client::GetMultiResponse::Status204
    ));

    let not_found: basic_client::types::NotFoundError =
        serde_json::from_str(r#"{"reason":"gone"}"#).unwrap();
    match basic_client::GetMultiError::Status404(not_found) {
        basic_client::GetMultiError::Status404(body) => assert_eq!(body.reason, "gone"),
        other => panic!("expected Status404, got {other:?}"),
    }
    let conflict: basic_client::types::ConflictError =
        serde_json::from_str(r#"{"detail":"dup"}"#).unwrap();
    match basic_client::GetMultiError::Status409(conflict) {
        basic_client::GetMultiError::Status409(body) => assert_eq!(body.detail, "dup"),
        other => panic!("expected Status409, got {other:?}"),
    }
}

#[test]
fn multipart_body_struct_has_typed_form_part_fields() {
    // Issue #12: the multipart/form-data body lowered to a typed struct whose fields are the form
    // parts. The binary `file` part is `bytes::Bytes` (its `serde` impls compile only because the
    // synthesized Cargo.toml enabled bytes' `serde` feature), `caption` a required `String`, and the
    // optional `count`/`tags` are `Option`. Constructing the value proves the field types; the
    // generated `upload_file` method (compiled here) builds the `reqwest::multipart::Form` from it,
    // which compiles only with reqwest's `multipart` feature enabled.
    let body = basic_client::types::RequestBody {
        file: bytes::Bytes::from_static(b"hello"),
        caption: "a caption".to_owned(),
        count: Some(3),
        tags: Some(vec!["x".to_owned(), "y".to_owned()]),
    };
    assert_eq!(&body.file[..], b"hello");
    assert_eq!(body.caption, "a caption");
    assert_eq!(body.count, Some(3));
    assert_eq!(body.tags.as_deref(), Some(&["x".to_owned(), "y".to_owned()][..]));
}

#[test]
fn streaming_op_item_type_is_typed_not_json_value() {
    // Issue #14: the SSE `/chat/stream` response schema lowered to a real `ChatChunk` type — the
    // streamed item of the `EventStream<ChatChunk>` the `stream_chat` method returns (that signature
    // and the embedded runtime `EventStream` are compile-verified by this crate's build). The item
    // type is a typed struct, never `serde_json::Value`; deserializing a frame the way the runtime's
    // `next` does proves it.
    let chunk: basic_client::types::ChatChunk =
        serde_json::from_str(r#"{"delta": "hi"}"#).unwrap();
    assert_eq!(chunk.delta, "hi");
}

#[test]
fn xml_body_types_carry_attribute_and_rename() {
    // Issue #13: the XML request/response bodies lowered to typed structs whose serde wire names
    // honor the `xml` hints — `XmlOrder.id` is an attribute (`xml.attribute` → serde `@id`), `sku` a
    // child element, and `XmlReceipt.code` is renamed via `xml.name` to `ReceiptCode`. The generated
    // crate depends on quick-xml (proving the conditional `xml` feature was enabled in its
    // Cargo.toml), so this exercises the same codec the `submit_order` method's `to_xml`/decode use.
    let order = basic_client::types::XmlOrder {
        id: 42,
        sku: "ABC".to_owned(),
    };
    let xml = quick_xml::se::to_string(&order).unwrap();
    assert!(xml.contains("id=\"42\""), "{xml}");
    assert!(xml.contains("<sku>ABC</sku>"), "{xml}");

    let receipt: basic_client::types::XmlReceipt =
        quick_xml::de::from_str("<XmlReceipt><ReceiptCode>OK</ReceiptCode></XmlReceipt>").unwrap();
    assert_eq!(receipt.code, "OK");
    assert_eq!(receipt.note, None);
}

#[test]
fn json_only_schema_with_xml_metadata_keeps_original_json_names() {
    // Issue #13 regression guard: `JsonMeta` carries `xml.attribute`/`xml.name` hints but is used
    // only by a JSON operation, so the format-agnostic serde rename is SUPPRESSED. JSON must use the
    // original `id`/`sku` names — deserializing a normal server payload succeeds and re-serializing
    // produces the same names (never `@id`/`ProductSku`), proving JSON is uncorrupted.
    let parsed: basic_client::types::JsonMeta =
        serde_json::from_str(r#"{"id": 5, "sku": "Z9"}"#).unwrap();
    assert_eq!(parsed.id, 5);
    assert_eq!(parsed.sku, "Z9");
    let back = serde_json::to_string(&parsed).unwrap();
    assert!(back.contains(r#""id":5"#), "{back}");
    assert!(back.contains(r#""sku":"Z9""#), "{back}");
    assert!(!back.contains("@id"), "{back}");
    assert!(!back.contains("ProductSku"), "{back}");
}

#[test]
fn optional_params_construct_via_fluent_setters() {
    // Issue #18: each optional param on a `…Params` struct gets a `#[must_use]` consuming setter
    // named after its field, taking the field's inner `T` (never `Option<T>`) and storing `Some`.
    // `getUser` has an ordinary optional query param (`page` → `Option<i64>`) and a NULLABLE optional
    // one (`filter`, `type: [integer, "null"]` → `Option<i64>`); the setter for the nullable param
    // must still take the bare `i64`. Building via `default().setter(x)` must compile and set fields.
    let params = basic_client::GetUserParams::default()
        .page(2)
        .filter(7);
    assert_eq!(params.page, Some(2));
    assert_eq!(params.filter, Some(7));

    // Back-compat: the struct still derives `Default` and keeps public fields, so the pre-existing
    // struct-literal form is unchanged.
    let literal = basic_client::GetUserParams {
        page: Some(2),
        ..Default::default()
    };
    assert_eq!(literal.filter, None);
}

#[test]
fn generated_support_module_exposes_link_paginator() {
    // Issue #27: the generic Link-header paginator is a runtime helper re-exported at the crate root
    // (`basic_client::LinkPaginator` / `basic_client::next_link`), so a generated client can drive
    // Link/RFC-8288 pagination with no per-operation codegen. Constructing one via
    // `client.core().paginate_links::<T>(url)` compiles under clippy -D warnings, proving the
    // embedded `support::paginate` module is present and wired.
    let client = basic_client::Client::new("https://api.example.com").unwrap();
    let first = reqwest::Url::parse("https://api.example.com/items?page=1").unwrap();
    let pages: basic_client::LinkPaginator<Vec<i64>> = client.core().paginate_links(first);
    assert!(pages.has_next());

    // The pure `rel="next"` header helper is exposed too: no `Link` header → no next page.
    let mut headers = reqwest::header::HeaderMap::new();
    assert!(basic_client::next_link(&headers).is_none());
    headers.insert(
        reqwest::header::LINK,
        r#"<https://api.example.com/items?page=2>; rel="next""#
            .parse()
            .unwrap(),
    );
    assert_eq!(
        basic_client::next_link(&headers).unwrap().as_str(),
        "https://api.example.com/items?page=2"
    );
}

#[test]
fn custom_http_backend_plugs_into_non_generic_client() {
    // Issue #11: the transport seam is re-exported at the crate root
    // (`basic_client::HttpBackend` / `ExecuteFuture` / `ReqwestBackend`), so a consumer can
    // implement their own transport and plug it via `Client::with_backend` WITHOUT `Client`
    // becoming generic. A trivial backend compiles (under clippy -D warnings) and constructs a
    // client. This test only exercises construction, so the transport is never polled — the runtime
    // crate's own tests prove that dispatch actually routes through the installed backend.
    #[derive(Debug)]
    struct TestBackend;
    impl basic_client::HttpBackend for TestBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TestBackend);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();

    // Back-compat: the pre-existing `new` / `with_client` constructors still work and install the
    // default reqwest-backed transport.
    let _default = basic_client::Client::new("https://api.example.com").unwrap();
    let _byo = basic_client::Client::with_client(
        reqwest::Client::new(),
        "https://api.example.com",
    )
    .unwrap();

    // The default backend type is nameable and usable as an `HttpBackend` too.
    let _reqwest_backend: std::sync::Arc<dyn basic_client::HttpBackend> =
        std::sync::Arc::new(basic_client::ReqwestBackend::new(reqwest::Client::new()));
}

#[test]
fn retry_backend_wraps_an_inner_backend() {
    // Issue #17: the retry adapter is re-exported at the crate root (`basic_client::RetryBackend`
    // / `RetryPolicy` / `RetryOutcome` / `exponential_backoff`). A consumer implements a policy
    // that decides retry AND supplies the wait (bring-your-own timing — no tokio in the runtime),
    // wraps their backend in a `RetryBackend`, and installs it via `Client::with_backend`, all
    // without `Client` becoming generic. This construction test compiles under clippy -D warnings;
    // the runtime crate's own tests prove the retry loop actually retries.
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    struct BackoffPolicy;
    impl basic_client::RetryPolicy for BackoffPolicy {
        fn retry<'a>(
            &'a self,
            attempt: u32,
            outcome: &basic_client::RetryOutcome<'_>,
        ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
            if attempt < 3 && outcome.is_transient() {
                // A real policy would await the caller's timer here (e.g. tokio::time::sleep); a
                // ready future keeps this construction test runtime-free.
                let _wait = basic_client::exponential_backoff(
                    attempt,
                    Duration::from_millis(50),
                    Duration::from_secs(2),
                );
                Some(Box::pin(std::future::ready(())))
            } else {
                None
            }
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let retry = basic_client::RetryBackend::new(inner, std::sync::Arc::new(BackoffPolicy));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(retry);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}

#[test]
fn middleware_backend_wraps_an_inner_backend() {
    // Issue #20: the interceptor middleware is re-exported at the crate root
    // (`basic_client::Middleware` / `Next` / `MiddlewareBackend`). A consumer implements a trivial
    // header-injecting middleware, layers it onto a `MiddlewareBackend`, and installs the whole
    // chain via `Client::with_backend` — all without `Client` becoming generic. This construction
    // test compiles under clippy -D warnings; the runtime crate's own tests prove the chain
    // actually observes/modifies/short-circuits and composes in order.
    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    // A middleware that inserts a header on the way in, then proceeds to the rest of the chain via
    // `Next::run`. Modifying the request before `run` and returning `run`'s future directly is the
    // simplest shape; the trait's `'a` ties the borrow of `self`, the `Next`, and the boxed future.
    #[derive(Debug)]
    struct InjectHeader;
    impl basic_client::Middleware for InjectHeader {
        fn handle<'a>(
            &'a self,
            mut request: reqwest::Request,
            next: basic_client::Next<'a>,
        ) -> basic_client::ExecuteFuture<'a> {
            request.headers_mut().insert(
                reqwest::header::HeaderName::from_static("x-generated-mw"),
                reqwest::header::HeaderValue::from_static("on"),
            );
            next.run(request)
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let middleware = basic_client::MiddlewareBackend::new(inner).layer(std::sync::Arc::new(InjectHeader));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(middleware);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}
"##,
    )
    .unwrap();
    let status = Command::new("cargo")
        .arg("test")
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    // Issue #19: the same generated crate must also build and lint clean WITH the `blocking` feature,
    // proving the `BlockingClient` type and its blocking methods compile under clippy -D warnings.
    let status = Command::new("cargo")
        .args(["build", "--features", "blocking"])
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must build with --features blocking"
    );

    let status = Command::new("cargo")
        .args(["clippy", "--features", "blocking", "--", "-D", "warnings"])
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must pass clippy -D warnings with --features blocking"
    );

    // Drive the blocking round-trip test under the feature (it is `#![cfg(feature = "blocking")]`, so
    // it only exists here). This exercises a real HTTP round-trip through a blocking method.
    let status = Command::new("cargo")
        .args(["test", "--features", "blocking", "--test", "blocking"])
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the BlockingClient round-trip must pass with --features blocking"
    );
}

#[test]
fn rejects_openapi_30_without_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        BASIC_SPEC.replace("openapi: 3.1.0", "openapi: 3.0.3"),
    )
    .unwrap();

    let report = spargen::check(&Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ));

    assert_eq!(report.outcome, Outcome::Rejected);
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == Code::UnsupportedOpenApiVersion));
}

#[test]
fn generates_standalone_crate_for_oas32_api_with_query_method() {
    // OpenAPI 3.2 lowers through the same frontend. This spec exercises the new fixed `QUERY`
    // method (which must emit a real client method) alongside a plain `get`, and must produce a
    // standalone crate that passes `cargo check` + `cargo clippy -D warnings`.
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, OAS32_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Crate {
            dir: Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
            name: "oas32_client".to_owned(),
        },
    ));

    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let status = Command::new("cargo")
        .arg("check")
        .current_dir(&out)
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new("cargo")
        .args(["clippy", "--", "-D", "warnings"])
        .current_dir(&out)
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

    // The OpenAPI 3.2 streaming response typed its item via `itemSchema` (no `schema`): the operation
    // must return `EventStream<Event>` — the item type is the typed struct, not dropped to a bodyless
    // `()`. A regression that ignores `itemSchema` would collapse this to `-> ResponseValue<()>`.
    let flat: String = generated.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("pub async fn stream_events"),
        "streaming operation should emit a client method"
    );
    assert!(
        flat.contains("support :: EventStream < types :: Event >")
            || flat.contains("support::EventStream<types::Event>"),
        "itemSchema must type the EventStream item as the typed struct, not be dropped: {flat}"
    );
}

#[test]
fn omit_overlay_removes_unsupported_operation() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC_WITH_UNSUPPORTED_OPERATION).unwrap();
    let out = temp.path().join("client.rs");
    let mut config = Config::new(
        Utf8PathBuf::from_path_buf(spec).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out).unwrap()),
    );
    config.omit = spargen::omit! {
        operations {
            post "/upload";
        }
    };

    let report = spargen::generate(&config);

    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == Code::OmittedConstruct));
}

const BASIC_SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Basic
  version: 1.0.0
servers:
  - url: https://example.com/api
paths:
  /users/{id}:
    get:
      operationId: getUser
      security:
        - bearer: []
        - apiKey: []
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
        - name: page
          in: query
          schema:
            type: integer
            default: 1
        # Optional nullable query param (Issue #6): `type: [integer, "null"]` lowers to a nullable
        # `Ty`, which `ty_tokens` renders as `Option<i64>`. The params struct must NOT wrap it again
        # (`Option<Option<i64>>` would not serialize — `Option<i64>: !Display`).
        - name: filter
          in: query
          schema:
            type: [integer, "null"]
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/User"
  # Multi-status responses (Issue #10): TWO success statuses (200/201) with different bodies lower
  # to a typed `GetMultiResponse` enum, and TWO error statuses (404/409) with different bodies to a
  # typed `GetMultiError` enum — no `serde_json::Value`, no `serde(untagged)`. Decode dispatches by
  # HTTP status. Here we compile-verify the enums and construct/deserialize their variants.
  /multi:
    get:
      operationId: getMulti
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        "201":
          description: Created
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiCreated"
        # A documented bodyless success alongside 2+ bodied successes → a payload-free unit variant
        # (Issue #10 follow-up): not silently dropped, decoded without reading a body.
        "204":
          description: No Content
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/NotFoundError"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ConflictError"
  # multipart/form-data request body (Issue #12): the body is an object whose properties are the form
  # parts. `file` is `format: binary` → a `bytes::Bytes` file part; `caption` a required text part;
  # `count` an optional scalar text part; `tags` an optional array → a JSON-encoded text part. The
  # generated method builds a `reqwest::multipart::Form` (compile-verifies the multipart emit AND that
  # the synthesized Cargo.toml enabled reqwest's `multipart` feature and bytes' `serde` feature).
  /upload:
    post:
      operationId: uploadFile
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, caption]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
                count:
                  type: integer
                tags:
                  type: array
                  items:
                    type: string
      responses:
        "204":
          description: No Content
  # Binary in parameter / non-multipart body positions (Issue #12 regression guard): `format: binary`
  # on a param has no faithful byte rendering, so it is represented as `String` (remapped) and stays
  # renderable via `to_string()`; a `format: binary` text/plain body lowers to `bytes::Bytes` and is
  # sent as a raw byte body (`request.body(body.clone())`), never `.to_string()` (`Bytes: !Display`).
  # Compile-verified: without the fixes these positions generate with zero diagnostics yet fail to
  # compile (the forbidden silent non-compile).
  /blob/{token}:
    get:
      operationId: getBlob
      parameters:
        - name: token
          in: path
          required: true
          schema:
            type: string
            format: binary
        - name: cursor
          in: query
          schema:
            type: string
            format: binary
      responses:
        "204":
          description: No Content
  /raw:
    post:
      operationId: postRaw
      requestBody:
        required: true
        content:
          text/plain:
            schema:
              type: string
              format: binary
      responses:
        "204":
          description: No Content
  # XML request + response bodies (Issue #13): both lower to typed structs and are
  # serialized/decoded through the embedded quick-xml codec — compile-verifies that the synthesized
  # Cargo.toml enabled quick-xml (the `xml` feature) and that the embedded `support::xml` helpers
  # (`to_xml`, `decode_success_xml`) compile. `id` carries `xml.attribute` (serde `@id`) and `code`
  # an `xml.name` rename; both are honored, an unsupported `xml.namespace` on `note` warns (W006).
  /xml/order:
    post:
      operationId: submitOrder
      requestBody:
        required: true
        content:
          application/xml:
            schema:
              $ref: "#/components/schemas/XmlOrder"
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema:
                $ref: "#/components/schemas/XmlReceipt"
  # JSON body carrying `xml` metadata (Issue #13 regression guard): the schema has `xml.attribute`
  # and `xml.name` hints but is used only by a JSON operation. The format-agnostic serde rename must
  # NOT be applied (it would corrupt JSON), so `JsonMeta` keeps its `id`/`sku` wire names — the
  # suppression is acknowledged as W006. Round-trip is compile+run verified below.
  /json/meta:
    post:
      operationId: postJsonMeta
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/JsonMeta"
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/JsonMeta"
  # Streaming response (Issue #14): a `text/event-stream` success response lowers to a streaming
  # operation whose method returns `support::EventStream<ChatChunk>` instead of `ResponseValue<T>`.
  # Compile-verifies both the streaming method signature and the embedded runtime `EventStream`
  # (framing + manual async `next`) — with reqwest's `.chunk()` needing no `stream` feature.
  /chat/stream:
    get:
      operationId: streamChat
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema:
                $ref: "#/components/schemas/ChatChunk"
components:
  securitySchemes:
    bearer:
      type: http
      scheme: bearer
    apiKey:
      type: apiKey
      in: header
      name: X-Api-Key
  schemas:
    User:
      type: object
      required: [id, name]
      properties:
        id:
          type: string
        name:
          type: string
        tree:
          $ref: "#/components/schemas/TreeNode"
        category:
          $ref: "#/components/schemas/Category"
        dict:
          $ref: "#/components/schemas/Dict"
        priority:
          $ref: "#/components/schemas/Priority"
        # Discriminated union (Issue #9): an internally-tagged enum over object `$ref` variants.
        pet:
          $ref: "#/components/schemas/Pet"
        # Undiscriminated but provably-disjoint union (string vs array JSON category): an enum with a
        # content-inspecting custom Deserialize/Serialize — no wrapper on the wire.
        alias:
          $ref: "#/components/schemas/StringOrList"
        # Nullable union variant (Issue #9 fix 2): the string variant is `{type: [string, null]}`;
        # its nullability is HOISTED to the union so this field is `Option<...>` and a `null` payload
        # resolves to `None` rather than erroring in the custom Deserialize.
        notes:
          $ref: "#/components/schemas/StringListOrNull"
    # Discriminated union: `petType` selects the object variant. Cat DECLARES `petType` as a required
    # property (the shape that broke serde internal tagging — "missing field petType"); the custom
    # buffer-to-Value Deserialize hands the WHOLE value to the variant, so Cat keeps its own tag.
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
    # Dog does NOT declare `petType`; on serialize the custom Serialize re-inserts the tag.
    Dog:
      type: object
      required: [bark]
      properties:
        bark:
          type: boolean
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
    # Disjoint by JSON type category: a bare string or a list of strings. Serializes WITHOUT any tag
    # or wrapper — the active variant's inner value is emitted directly.
    StringOrList:
      oneOf:
        - type: string
        - type: array
          items:
            type: string
    # Nullable-variant union: the string member is nullable, hoisted to make the whole union nullable.
    StringListOrNull:
      oneOf:
        - type: [string, "null"]
        - type: array
          items:
            type: string
    # Self-recursive: `parent` is a direct back-edge (→ Option<Box<TreeNode>>) and `children`
    # recurses through an array (→ Vec<TreeNode>; the Vec supplies the indirection). Without
    # boxing the direct `parent` back-edge the
    # generated struct would have infinite size and fail to compile.
    TreeNode:
      type: object
      required: [value]
      properties:
        value:
          type: string
        parent:
          $ref: "#/components/schemas/TreeNode"
        children:
          type: array
          items:
            $ref: "#/components/schemas/TreeNode"
    # Mutually recursive: Category <-> Item. One of the two edges in the cycle is boxed.
    Category:
      type: object
      required: [name]
      properties:
        name:
          type: string
        item:
          $ref: "#/components/schemas/Item"
    Item:
      type: object
      required: [label]
      properties:
        label:
          type: string
        category:
          $ref: "#/components/schemas/Category"
    # Self-recursive through additionalProperties (→ BTreeMap<String, Dict>; the map supplies
    # the indirection).
    Dict:
      type: object
      additionalProperties:
        $ref: "#/components/schemas/Dict"
    # Null-mixed enum (Issue #6): the `null` member is stripped and the remaining homogeneous string
    # scalars lower as a real Rust enum; the `"null"` in the type array makes every use nullable, so
    # a field of this type is emitted as `Option<Priority>`. An absent or `null` value deserializes
    # to `None`; a string value to the matching variant.
    Priority:
      type: [string, "null"]
      enum: [low, medium, high, null]
    # Propagation of component nullability through `$ref` (Issue #6): a REQUIRED field whose type is
    # the nullable `Priority` component must still be `Option<Priority>` (present, but may be `null`),
    # and an array of the component must be `Vec<Option<Priority>>` (a null element is accepted).
    # Before propagation these emitted `Priority` / `Vec<Priority>` and rejected a conforming `null`.
    Ticket:
      type: object
      required: [priority, history]
      properties:
        priority:
          $ref: "#/components/schemas/Priority"
        history:
          type: array
          items:
            $ref: "#/components/schemas/Priority"
    # `default` on the component schema itself → documented on the generated `Mode` type.
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
    # Exercises schema `default`: representable scalar defaults on optional fields are wired via
    # generated serde providers; a required field's default is rustdoc-only.
    Settings:
      type: object
      required: [retries]
      properties:
        color:
          type: string
          default: red
        enabled:
          type: boolean
          default: true
        ratio:
          type: number
          default: 1.5
        retries:
          type: integer
          default: 3
        # Out-of-range for i32: must NOT be serde-wired (rustdoc-only, W005). If a regression wired
        # `Some(5000000000)` into `Option<i32>`, the generated crate's `cargo check` would fail.
        wide:
          type: integer
          format: int32
          default: 5000000000
        mode:
          $ref: "#/components/schemas/Mode"
          default: auto
    # `patternProperties` composed with an explicit property: the declared `host` field plus a typed
    # overflow map (`#[serde(flatten)] BTreeMap<String, String>`) for the pattern-matched keys. The
    # key regex is validation-only (W001) and not enforced by the map.
    Headers:
      type: object
      properties:
        host:
          type: string
      patternProperties:
        "^x-": { type: string }
    # Object-ness comes *only* from `patternProperties` (no `type`, no `properties`): still a struct
    # with empty fields and a typed overflow map, not an untyped `Any`.
    Tags:
      patternProperties:
        "^tag-": { type: string }
    # A declared property literally named `additional` alongside a typed overflow map: the synthetic
    # flatten field must be allocated in the field scope and disambiguated, or two `pub additional:`
    # fields would collide and the generated crate would fail to compile.
    Bag:
      type: object
      properties:
        additional:
          type: string
      patternProperties:
        "^x-": { type: integer }
    # allOf merge (Issue #8): `Account` flattens a `$ref` base (id, required), an inline member
    # (label, required) and the enclosing schema's own sibling property (owner, optional) into ONE
    # struct. All fields must be present and correctly typed in the generated `Account` type.
    AccountBase:
      type: object
      required: [id]
      properties:
        id:
          type: string
    Account:
      type: object
      properties:
        owner:
          type: string
      allOf:
        - $ref: "#/components/schemas/AccountBase"
        - type: object
          required: [label]
          properties:
            label:
              type: string
    # Distinct bodies for the multi-status `getMulti` operation (Issue #10).
    MultiOk:
      type: object
      required: [ok]
      properties:
        ok:
          type: string
    MultiCreated:
      type: object
      required: [id]
      properties:
        id:
          type: integer
    NotFoundError:
      type: object
      required: [reason]
      properties:
        reason:
          type: string
    ConflictError:
      type: object
      required: [detail]
      properties:
        detail:
          type: string
    # Streamed item type for the `/chat/stream` SSE operation (Issue #14).
    ChatChunk:
      type: object
      required: [delta]
      properties:
        delta:
          type: string
    # XML request body (Issue #13): `id` is an XML attribute (serde `@id`), `sku` a plain element.
    XmlOrder:
      type: object
      required: [id, sku]
      properties:
        id:
          type: integer
          xml: { attribute: true }
        sku:
          type: string
    # XML response body (Issue #13): `code` renamed via `xml.name`; `note` carries an unsupported
    # `xml.namespace` hint (→ W006, still generates).
    XmlReceipt:
      type: object
      required: [code]
      properties:
        code:
          type: string
          xml: { name: "ReceiptCode" }
        note:
          type: string
          xml: { namespace: "urn:example:receipt" }
    # JSON-only schema carrying `xml` metadata (Issue #13 regression guard): the same hint shapes as
    # `XmlOrder`, but reachable only from a JSON body — the rename must be suppressed so JSON is
    # correct.
    JsonMeta:
      type: object
      required: [id, sku]
      properties:
        id:
          type: integer
          xml: { attribute: true }
        sku:
          type: string
          xml: { name: "ProductSku" }
"##;

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

const OAS32_SPEC: &str = r##"
openapi: 3.2.0
info:
  title: Records
  version: 1.0.0
servers:
  - url: https://example.com/api
paths:
  /records:
    get:
      operationId: listRecords
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: "#/components/schemas/Record"
    query:
      operationId: searchRecords
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/Query"
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: "#/components/schemas/Record"
  /events:
    get:
      operationId: streamEvents
      responses:
        "200":
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/Event"
components:
  schemas:
    Record:
      type: object
      required: [id]
      properties:
        id: { type: string }
        name: { type: string }
    Query:
      type: object
      properties:
        term: { type: string }
    Event:
      type: object
      required: [seq]
      properties:
        seq: { type: integer }
        payload: { type: string }
"##;
