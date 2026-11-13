//! Binary `_stream` twins of multi-status successes (#1306): an operation whose success enum has a
//! raw binary arm gets a twin returning its own success enum, whose binary arms carry a
//! `ByteStream`, compiled and linted, and driven over the `testing` module's `MockBackend` past the
//! body limit its buffered method reads within.

use super::*;

/// A ranged download (`200` + `206` octets), a download beside a bodyless `204`, and a JSON `200`
/// beside a `206` of octets.
const TWINS_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Twins, version: 1.0.0 }
paths:
  /range:
    get:
      operationId: getRange
      responses:
        "200":
          description: the whole blob
          content:
            application/octet-stream: { schema: {} }
        "206":
          description: part of the blob
          content:
            application/octet-stream: { schema: {} }
  /maybe:
    get:
      operationId: getMaybe
      responses:
        "200":
          description: the blob
          content:
            application/octet-stream: { schema: {} }
        "204": { description: no blob }
  /mixed:
    get:
      operationId: getMixed
      responses:
        "200":
          description: a listing
          content:
            application/json:
              schema: { type: array, items: { type: string } }
        "206":
          description: part of the blob
          content:
            application/octet-stream: { schema: {} }
"##;

#[test]
fn a_success_enum_with_a_binary_arm_streams_it_through_its_twin() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "twins.yaml", TWINS_SPEC);
    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "twins_client", "2021", |spec| {
        spec.with_testing(spargen::TestingModule::Emitted)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    for item in [
        "pub enum GetRangeStreamResponse",
        "pub enum GetMaybeStreamResponse",
        "pub enum GetMixedStreamResponse",
        "pub async fn get_range_stream(",
        "pub async fn get_range_stream_request(",
        "pub async fn decode_get_range_stream(",
    ] {
        assert!(generated.contains(item), "`{item}` is emitted: {generated}");
    }

    write_fixture_test(&out, "twins", TWINS_TEST);
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a success enum's twin must pass clippy -D warnings"
    );
    let status = fixture_cargo(&out)
        .args(["test", "--test", "twins"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each binary arm must stream through the twin past the buffered limit"
    );
}

const TWINS_TEST: &str = r##"use std::future::Future;
use std::sync::Arc;

use twins_client::testing;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a call over the mock never waits"),
    }
}

const BODY: &[u8] = b"abcdefgh";

/// A client over `mock` whose buffered reads stop at four bytes, half of `BODY`.
fn limited(mock: testing::MockBackend) -> twins_client::Client {
    twins_client::Client::with_backend(Arc::new(mock), "http://mock")
        .unwrap()
        .with_options(twins_client::RequestOptions {
            max_body: Some(4),
            ..twins_client::RequestOptions::default()
        })
}

fn too_large<T: std::fmt::Debug, E: std::fmt::Debug>(
    result: Result<T, twins_client::Error<E>>,
    expected: u16,
) {
    match result {
        Err(twins_client::Error::BodyTooLarge { limit, status }) => {
            assert_eq!((limit, status.as_u16()), (4, expected));
        }
        other => panic!("expected BodyTooLarge, got {other:?}"),
    }
}

#[test]
fn a_ranged_download_streams_both_statuses() {
    for status in [200u16, 206] {
        let mock = testing::MockBackend::new().on_get_range(move |_| match status {
            200 => testing::get_range::Reply::status_200(BODY),
            _ => testing::get_range::Reply::status_206(BODY),
        });
        let client = limited(mock);
        too_large(block_on(client.get_range()), status);

        let value = block_on(client.get_range_stream()).unwrap();
        assert_eq!(value.status(), status);
        let streamed = value.into_inner();
        assert_eq!(streamed.status(), status);
        // Every bodied variant is a stream, so the body is reachable without matching.
        match &streamed {
            twins_client::GetRangeStreamResponse::Status200(_) => assert_eq!(status, 200),
            twins_client::GetRangeStreamResponse::Status206(_) => assert_eq!(status, 206),
        }
        let stream: twins_client::ByteStream = streamed.into_body();
        assert_eq!(&block_on(stream.collect_bounded(BODY.len())).unwrap()[..], BODY);
    }
}

#[test]
fn a_download_beside_a_bodyless_status_streams_its_body() {
    let mock = testing::MockBackend::new().on_get_maybe(|_| testing::get_maybe::Reply::status_200(BODY));
    let client = limited(mock);
    too_large(block_on(client.get_maybe()), 200);
    let value = block_on(client.get_maybe_stream()).unwrap().into_inner();
    let stream = value.into_body().expect("a 200 carries the body");
    assert_eq!(&block_on(stream.collect_bounded(BODY.len())).unwrap()[..], BODY);

    let mock = testing::MockBackend::new().on_get_maybe(|_| testing::get_maybe::Reply::status_204());
    let value = block_on(limited(mock).get_maybe_stream()).unwrap().into_inner();
    assert!(matches!(value, twins_client::GetMaybeStreamResponse::Status204));
    assert!(value.into_body().is_none());
}

#[test]
fn a_twin_reads_its_other_arms_within_the_client_limits() {
    let mock = testing::MockBackend::new().on_get_mixed(|_| testing::get_mixed::Reply::status_206(BODY));
    let client = limited(mock);
    too_large(block_on(client.get_mixed()), 206);
    match block_on(client.get_mixed_stream()).unwrap().into_inner() {
        twins_client::GetMixedStreamResponse::Status206(stream) => {
            assert_eq!(&block_on(stream.collect_bounded(BODY.len())).unwrap()[..], BODY);
        }
        other => panic!("expected the streamed 206, got {other:?}"),
    }

    // The JSON arm is read whole, under the same limit the method reads it within: `[]` fits in
    // it, a listing of one entry does not.
    let listing: Vec<String> = Vec::new();
    let mock = testing::MockBackend::new().on_get_mixed({
        let listing = listing.clone();
        move |_| testing::get_mixed::Reply::status_200(&listing)
    });
    match block_on(limited(mock).get_mixed_stream()).unwrap().into_inner() {
        twins_client::GetMixedStreamResponse::Status200(body) => assert_eq!(*body, listing),
        other => panic!("expected the decoded 200, got {other:?}"),
    }
    let long = vec!["a long entry".to_owned()];
    let mock = testing::MockBackend::new().on_get_mixed({
        let long = long.clone();
        move |_| testing::get_mixed::Reply::status_200(&long)
    });
    too_large(block_on(limited(mock).get_mixed_stream()), 200);
}
"##;
