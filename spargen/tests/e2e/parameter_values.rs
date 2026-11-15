//! Parameter values that would silently change the request, refused before anything is sent:
//! an empty path value and a non-finite number.

use super::*;

/// A parameter value that would silently change the request is refused before anything is sent
/// (#1405): a path value that renders to nothing would leave an empty segment, re-targeting
/// `DELETE /users/{id}` at the collection `/users/`, and a NaN or an infinity would reach the wire
/// as an undefined value (`ratio=`, or an empty segment again). Each call fails as
/// `RequestConstruction` whose source downcasts to the `ParameterError` naming the cause, while a
/// path value of `0` and a finite number still reach the wire.
#[test]
fn an_empty_path_value_or_a_non_finite_number_is_refused_before_sending() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: Parameter values, version: 1.0.0 }
paths:
  /users/{id}:
    delete:
      operationId: deleteUser
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "204": { description: Deleted }
  /users/{id}/keys:
    get:
      operationId: listKeys
      parameters:
        - { name: id, in: path, required: true, schema: { type: array, items: { type: string } } }
      responses:
        "204": { description: No Content }
  /scores/{value}:
    get:
      operationId: getScore
      parameters:
        - { name: value, in: path, required: true, schema: { type: number } }
        - { name: ratio, in: query, schema: { type: number } }
      responses:
        "204": { description: No Content }
"##,
    );
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "param_values_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    write_fixture_test(
        &out,
        "param_values",
        r##"#![cfg(feature = "blocking")]

mod common;

use param_values_client::{BlockingClient, Error, GetScoreParams, ParameterError};

/// A client against a base no server listens on: a request that reached the transport would
/// fail there instead, so `RequestConstruction` proves nothing was sent.
fn unreachable() -> BlockingClient {
    BlockingClient::new("http://127.0.0.1:9").unwrap()
}

/// The `ParameterError` a refused call's `RequestConstruction` source downcasts to.
fn parameter_error<T: std::fmt::Debug>(
    result: Result<T, Error<std::convert::Infallible>>,
) -> ParameterError {
    match result.expect_err("the value is refused") {
        Error::RequestConstruction(request) => {
            let source = std::error::Error::source(&request).expect("a cause");
            match source.downcast_ref::<ParameterError>() {
                Some(ParameterError::EmptyPathSegment) => ParameterError::EmptyPathSegment,
                Some(ParameterError::NonFiniteNumber) => ParameterError::NonFiniteNumber,
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_empty_path_value_is_refused() {
    assert!(matches!(
        parameter_error(unreachable().delete_user("")),
        ParameterError::EmptyPathSegment
    ));
    assert!(matches!(
        parameter_error(unreachable().list_keys(Vec::new())),
        ParameterError::EmptyPathSegment
    ));
}

#[test]
fn a_non_finite_number_is_refused() {
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            parameter_error(unreachable().get_score(number, GetScoreParams::default())),
            ParameterError::NonFiniteNumber
        ));
        assert!(matches!(
            parameter_error(
                unreachable().get_score(1.5, GetScoreParams::default().ratio(number))
            ),
            ParameterError::NonFiniteNumber
        ));
    }
}

#[test]
fn a_zero_and_a_finite_number_reach_the_wire() {
    let (base, server) = common::serve_no_content();
    BlockingClient::new(&base)
        .unwrap()
        .delete_user("0")
        .expect("the call round-trips");
    assert_eq!(server.join().unwrap().line(), "DELETE /users/0 HTTP/1.1");

    let (base, server) = common::serve_no_content();
    BlockingClient::new(&base)
        .unwrap()
        .get_score(1.5, GetScoreParams::default().ratio(0.25))
        .expect("the call round-trips");
    assert_eq!(
        server.join().unwrap().line(),
        "GET /scores/1.5?ratio=0.25 HTTP/1.1"
    );
}
"##,
    );

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "param_values"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "an empty path value or a non-finite number must be refused before sending"
    );
}
