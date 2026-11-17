//! Parameter values that would silently change the request, refused before anything is sent:
//! an empty path value and a non-finite number, in a styled parameter, a JSON `content`
//! parameter, or a JSON request body or multipart part.

use super::*;

/// A parameter value that would silently change the request is refused before anything is sent
/// (#1405): a path value that renders to nothing would leave an empty segment, re-targeting
/// `DELETE /users/{id}` at the collection `/users/`, and a NaN or an infinity would reach the wire
/// as an undefined value (`ratio=`, or an empty segment again). Each call fails as
/// `RequestConstruction` whose source downcasts to the `ParameterError` naming the cause, while a
/// path value of `0` and a finite number still reach the wire.
///
/// The JSON paths refuse a non-finite number the same way (#1507): `serde_json` writes one as
/// `null`, so a JSON `content` parameter, a JSON `in: querystring` value, a JSON body, and a
/// JSON-encoded multipart part would each send `null` where the caller passed a number.
#[test]
fn an_empty_path_value_or_a_non_finite_number_is_refused_before_sending() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.2.0
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
  /filters:
    get:
      operationId: getFiltered
      parameters:
        - name: filter
          in: query
          content: { application/json: { schema: { type: number } } }
        - name: X-Ratio
          in: header
          content: { application/json: { schema: { type: number } } }
      responses:
        "204": { description: No Content }
  /search:
    get:
      operationId: search
      parameters:
        - name: q
          in: querystring
          content: { application/json: { schema: { $ref: "#/components/schemas/Ratio" } } }
      responses:
        "204": { description: No Content }
  /ratios:
    post:
      operationId: postRatios
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: array, items: { $ref: "#/components/schemas/Ratio" } }
      responses:
        "204": { description: No Content }
  /uploads:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [meta]
              properties:
                meta: { $ref: "#/components/schemas/Ratio" }
      responses:
        "204": { description: No Content }
  /shapes:
    post:
      operationId: postShape
      parameters:
        - { name: size, in: query, schema: { $ref: "#/components/schemas/Size" } }
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Shape" }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Shape:
      oneOf:
        - $ref: "#/components/schemas/Circle"
        - $ref: "#/components/schemas/Square"
        - $ref: "#/components/schemas/Triangle"
      discriminator:
        propertyName: kind
        mapping:
          circle: "#/components/schemas/Circle"
          square: "#/components/schemas/Square"
          triangle: "#/components/schemas/Triangle"
        defaultMapping: "#/components/schemas/Triangle"
    Tagged:
      oneOf:
        - $ref: "#/components/schemas/Circle"
        - $ref: "#/components/schemas/Square"
      discriminator:
        propertyName: kind
        mapping:
          circle: "#/components/schemas/Circle"
          square: "#/components/schemas/Square"
    Circle:
      type: object
      required: [kind, radius]
      properties:
        kind: { type: string }
        radius: { type: number }
    Square:
      type: object
      required: [kind, side]
      properties:
        kind: { type: string }
        side: { type: number }
    Triangle:
      type: object
      required: [side]
      properties:
        side: { type: number }
    Size:
      oneOf:
        - $ref: "#/components/schemas/Ratio"
        - $ref: "#/components/schemas/Weighted"
    Weighted:
      type: object
      required: [ratio, weight]
      properties:
        ratio: { type: number }
        weight: { type: number }
    Ratio:
      type: object
      required: [ratio]
      properties:
        ratio: { type: number }
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

use param_values_client::types::{
    Circle, Ratio, Shape, Size, Square, Tagged, Triangle, UploadBody,
};
use param_values_client::{
    BlockingClient, Error, GetFilteredParams, GetScoreParams, ParameterError, PostShapeParams,
    SearchParams,
};

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
fn a_non_finite_number_in_a_json_parameter_or_body_is_refused() {
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let ratio = Ratio { ratio: number };
        assert!(matches!(
            parameter_error(unreachable().get_filtered(GetFilteredParams::default().filter(number))),
            ParameterError::NonFiniteNumber
        ));
        assert!(matches!(
            parameter_error(unreachable().get_filtered(GetFilteredParams::default().x_ratio(number))),
            ParameterError::NonFiniteNumber
        ));
        assert!(matches!(
            parameter_error(unreachable().search(Some(SearchParams::default().q(ratio.clone())))),
            ParameterError::NonFiniteNumber
        ));
        // Nested: the second element of the array body.
        assert!(matches!(
            parameter_error(unreachable().post_ratios(vec![Ratio { ratio: 1.0 }, ratio.clone()])),
            ParameterError::NonFiniteNumber
        ));
        assert!(matches!(
            parameter_error(unreachable().upload(UploadBody { meta: ratio })),
            ParameterError::NonFiniteNumber
        ));
    }
}

/// A union serializes its variant through `serde_json::Value`, which would turn the number into
/// `null` before the check saw it: the check reaches the variant itself, in a discriminated-union
/// JSON body and in a trial-union styled parameter alike.
#[test]
fn a_non_finite_number_inside_a_union_is_refused() {
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let circle = Shape::Circle(Box::new(Circle { kind: "circle".to_owned(), radius: number }));
        assert!(matches!(
            parameter_error(unreachable().post_shape(PostShapeParams::default(), circle.clone())),
            ParameterError::NonFiniteNumber
        ));
        let square = Shape::Square(Box::new(Square { kind: "square".to_owned(), side: 1.0 }));
        assert!(matches!(
            parameter_error(
                unreachable().post_shape(
                    PostShapeParams::default().size(Size::Ratio(Box::new(Ratio { ratio: number }))),
                    square.clone(),
                )
            ),
            ParameterError::NonFiniteNumber
        ));
    }
}

/// A discriminated union holding a non-finite number keeps its tag rules (#1516): a held tag
/// that would not select the variant is refused, never written out, and a variant that holds no
/// tag still has its own written, as for a finite number.
#[test]
fn a_union_holding_a_non_finite_number_keeps_its_tag_rules() {
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let foreign = Shape::Circle(Box::new(Circle { kind: "square".to_owned(), radius: number }));
        let error = serde_json::to_string(&foreign).expect_err("a foreign tag is refused");
        assert!(
            error.to_string().contains("does not select variant Shape::Circle"),
            "{error}"
        );
        assert!(unreachable().post_shape(PostShapeParams::default(), &foreign).is_err());

        let triangle = Shape::Triangle(Box::new(Triangle { side: number }));
        assert_eq!(
            serde_json::to_value(&triangle).unwrap(),
            serde_json::json!({ "kind": "triangle", "side": null })
        );
        assert!(matches!(
            parameter_error(unreachable().post_shape(PostShapeParams::default(), &triangle)),
            ParameterError::NonFiniteNumber
        ));
    }
}

/// A discriminated union named `Tagged` compiles and writes its non-finite variant through the
/// direct path: the serializer's own helper type does not shadow the union's name.
#[test]
fn a_union_named_tagged_writes_a_non_finite_variant() {
    let square = Tagged::Square(Box::new(Square { kind: "square".to_owned(), side: f64::NAN }));
    assert_eq!(
        serde_json::to_value(&square).unwrap(),
        serde_json::json!({ "kind": "square", "side": null })
    );
}

#[test]
fn a_finite_number_inside_a_union_reaches_the_wire() {
    let (base, server) = common::serve_no_content();
    BlockingClient::new(&base)
        .unwrap()
        .post_shape(
            PostShapeParams::default().size(Size::Ratio(Box::new(Ratio { ratio: 2.5 }))),
            Shape::Circle(Box::new(Circle { kind: "circle".to_owned(), radius: 0.5 })),
        )
        .expect("the call round-trips");
    let request = server.join().unwrap();
    assert_eq!(request.line(), "POST /shapes?ratio=2.5 HTTP/1.1");
    assert!(
        request.text().ends_with(r#"{"kind":"circle","radius":0.5}"#),
        "{}",
        request.text()
    );
}

#[test]
fn a_finite_number_in_a_json_parameter_or_body_reaches_the_wire() {
    let (base, server) = common::serve(vec![common::no_content(); 3]);
    let client = BlockingClient::new(&base).unwrap();
    client
        .get_filtered(GetFilteredParams::default().filter(0.5))
        .expect("the call round-trips");
    client
        .search(Some(SearchParams::default().q(Ratio { ratio: 0.25 })))
        .expect("the call round-trips");
    client
        .post_ratios(vec![Ratio { ratio: 2.0 }])
        .expect("the call round-trips");
    let requests = server.join().unwrap();
    assert_eq!(requests[0].line(), "GET /filters?filter=0.5 HTTP/1.1");
    assert_eq!(
        requests[1].line(),
        "GET /search?%7B%22ratio%22%3A0.25%7D HTTP/1.1"
    );
    assert!(
        requests[2].text().ends_with(r#"[{"ratio":2.0}]"#),
        "{}",
        requests[2].text()
    );
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
