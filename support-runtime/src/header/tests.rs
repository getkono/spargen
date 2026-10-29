use super::*;

/// `Set-Cookie` carries unescaped commas (an `Expires=Wed, 09 Jun 2021 …` attribute is the
/// everyday case), so the comma-joined field-list rule cuts its values in half. RFC 9110 §5.3
/// exempts it, and so does this shape: one value per occurrence, never joined.
#[test]
fn set_cookie_is_never_comma_joined() {
    let mut map = HeaderMap::new();
    map.append(
        "set-cookie",
        "lang=en; Expires=Wed, 09 Jun 2021 10:18:14 GMT"
            .parse()
            .unwrap(),
    );
    map.append("set-cookie", "sessionId=38afes7a8".parse().unwrap());
    let cookies: Vec<String> =
        require_header(&map, "set-cookie", HeaderShape::SetCookie, false).unwrap();
    assert_eq!(
        cookies,
        [
            "lang=en; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
            "sessionId=38afes7a8"
        ]
    );
}

/// The same headers under the ordinary list rule are what the defect produced: joined, then
/// split back apart on every comma, yielding four fragments from two cookies.
#[test]
fn the_array_shape_would_have_split_set_cookie_values() {
    let mut map = HeaderMap::new();
    map.append(
        "set-cookie",
        "lang=en; Expires=Wed, 09 Jun 2021 10:18:14 GMT"
            .parse()
            .unwrap(),
    );
    map.append("set-cookie", "sessionId=38afes7a8".parse().unwrap());
    let split: Vec<String> = require_header(&map, "set-cookie", HeaderShape::Array, false).unwrap();
    assert_eq!(
        split.len(),
        3,
        "the joined form fragments the cookie: {split:?}"
    );
}

#[test]
fn a_single_set_cookie_still_yields_a_one_element_list() {
    let mut map = HeaderMap::new();
    map.append("set-cookie", "a=1".parse().unwrap());
    let cookies: Vec<String> =
        require_header(&map, "set-cookie", HeaderShape::SetCookie, false).unwrap();
    assert_eq!(cookies, ["a=1"]);
}

use crate::parameter::{serialize_simple, PercentEncoding};

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            reqwest::header::HeaderName::try_from(*name).unwrap(),
            reqwest::header::HeaderValue::try_from(*value).unwrap(),
        );
    }
    map
}

#[test]
fn absent_optional_header_is_none() {
    let parsed: Option<String> =
        parse_header(&HeaderMap::new(), "x-missing", HeaderShape::Scalar, false).unwrap();
    assert_eq!(parsed, None);
}

#[test]
fn absent_required_header_is_an_error() {
    let parsed =
        require_header::<String>(&HeaderMap::new(), "x-required", HeaderShape::Scalar, false);
    assert!(matches!(parsed, Err(HeaderError::Missing { .. })));
}

#[test]
fn numbers_and_booleans_keep_their_json_type() {
    let map = headers(&[("x-limit", "42"), ("x-ok", "true")]);
    let limit: u32 = require_header(&map, "x-limit", HeaderShape::Scalar, false).unwrap();
    let ok: bool = require_header(&map, "x-ok", HeaderShape::Scalar, false).unwrap();
    assert_eq!(limit, 42);
    assert!(ok);
}

#[test]
fn objects_round_trip_for_both_explode_settings() {
    let value = serde_json::json!({ "a": "1", "b": "2" });
    for explode in [false, true] {
        let written = serialize_simple(&value, explode, PercentEncoding::Passthrough).unwrap();
        let map = headers(&[("x-meta", &written)]);
        let parsed: serde_json::Map<String, Value> =
            require_header(&map, "x-meta", HeaderShape::Object, explode).unwrap();
        assert_eq!(parsed.get("a"), Some(&Value::String("1".to_owned())));
        assert_eq!(parsed.get("b"), Some(&Value::String("2".to_owned())));
    }
}

#[test]
fn repeated_occurrences_join_as_one_field_list() {
    let map = headers(&[("x-tags", "a"), ("x-tags", "b")]);
    let parsed: Vec<String> = require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
    assert_eq!(parsed, ["a", "b"]);
}

#[test]
fn a_json_content_header_parses_as_json() {
    let map = headers(&[("x-payload", r#"{"id":7}"#)]);
    let parsed: serde_json::Map<String, Value> =
        require_header(&map, "x-payload", HeaderShape::Json, false).unwrap();
    assert_eq!(parsed.get("id"), Some(&Value::Number(7.into())));
}

/// #692: a nullable or optional target accepts `null`, so text that is not JSON at all must
/// not be read as `null`; it is a `Parse` error carrying the syntax error.
#[test]
fn a_malformed_json_content_header_is_a_parse_error_for_a_nullable_target() {
    let map = headers(&[("x-meta", "{bad")]);
    let parsed = parse_header::<Option<serde_json::Map<String, Value>>>(
        &map,
        "x-meta",
        HeaderShape::Json,
        false,
    );
    let Err(HeaderError::Parse { message, .. }) = parsed else {
        panic!("malformed JSON must not deserialize: {parsed:?}");
    };
    assert!(message.contains("line 1 column"), "{message}");
}

/// #692: for a non-nullable target the message is the syntax error, not a type mismatch
/// against a `null` the server never sent.
#[test]
fn a_malformed_json_content_header_reports_the_syntax_error() {
    let map = headers(&[("x-meta", "{bad")]);
    let parsed =
        require_header::<serde_json::Map<String, Value>>(&map, "x-meta", HeaderShape::Json, false);
    let Err(HeaderError::Parse { message, .. }) = parsed else {
        panic!("malformed JSON must not deserialize: {parsed:?}");
    };
    assert!(!message.contains("null"), "{message}");
    assert!(message.contains("line 1 column"), "{message}");
}

/// A JSON-content header that really is `null` still reads as `null`.
#[test]
fn a_json_null_content_header_is_null() {
    let map = headers(&[("x-meta", "null")]);
    let parsed: Option<Option<serde_json::Map<String, Value>>> =
        parse_header(&map, "x-meta", HeaderShape::Json, false).unwrap();
    assert_eq!(parsed, Some(None));
}

#[test]
fn a_type_mismatch_is_a_parse_error() {
    let map = headers(&[("x-limit", "not-a-number")]);
    let parsed = require_header::<u32>(&map, "x-limit", HeaderShape::Scalar, false);
    assert!(
        matches!(parsed, Err(HeaderError::Parse { .. })),
        "{parsed:?}"
    );
}

#[test]
fn non_utf8_bytes_are_reported_as_such() {
    let mut map = HeaderMap::new();
    map.append(
        reqwest::header::HeaderName::try_from("x-raw").unwrap(),
        reqwest::header::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
    );
    let parsed = require_header::<String>(&map, "x-raw", HeaderShape::Scalar, false);
    assert!(
        matches!(parsed, Err(HeaderError::NotUtf8 { .. })),
        "{parsed:?}"
    );
}

/// Each variant's message names the header and says which way it failed; a `Parse` error also
/// carries serde's message, which is the only detail of what the value looked like.
#[test]
fn every_header_error_displays_its_name_and_cause() {
    for (error, expected) in [
        (
            HeaderError::Missing { name: "x-rate" },
            "required response header `x-rate` is absent",
        ),
        (
            HeaderError::NotUtf8 { name: "x-rate" },
            "response header `x-rate` is not valid UTF-8",
        ),
        (
            HeaderError::Parse {
                name: "x-rate",
                message: "invalid digit".to_owned(),
            },
            "response header `x-rate` did not match its documented type: invalid digit",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
    }
}

/// A documented string enum, the target whose serde message quotes the offending input.
#[derive(Debug, serde::Deserialize)]
enum Documented {
    Listed,
}

const SHAPES: [HeaderShape; 5] = [
    HeaderShape::Scalar,
    HeaderShape::Array,
    HeaderShape::Object,
    HeaderShape::Json,
    HeaderShape::SetCookie,
];

/// Every message along `error`'s `source()` chain.
fn chain(error: &HeaderError) -> Vec<String> {
    let mut messages = Vec::new();
    let mut next: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = next {
        messages.push(current.to_string());
        next = current.source();
    }
    messages
}

/// Parse `raw` as each target, under `shape` and `explode`, and return the messages of every
/// error any of them reports.
fn messages(raw: &[u8], shape: HeaderShape, explode: bool) -> Vec<String> {
    let mut map = HeaderMap::new();
    map.append(
        "x-value",
        reqwest::header::HeaderValue::from_bytes(raw).unwrap(),
    );
    let mut out = Vec::new();
    if let Err(error) = parse_header::<Documented>(&map, "x-value", shape, explode) {
        out.extend(chain(&error));
    }
    if let Err(error) = parse_header::<Vec<Documented>>(&map, "x-value", shape, explode) {
        out.extend(chain(&error));
    }
    if let Err(error) = parse_header::<u32>(&map, "x-value", shape, explode) {
        out.extend(chain(&error));
    }
    out
}

/// #521: a field value may legally carry a horizontal tab, and serde's message quotes it; the
/// `Parse` message shows it escaped and stays on one line, while the field keeps serde's text.
#[test]
fn a_parse_error_escapes_the_values_tab() {
    let map = headers(&[("x-value", "\t")]);
    let error = parse_header::<Documented>(&map, "x-value", HeaderShape::Scalar, false)
        .expect_err("a tab is no documented variant");
    assert_eq!(
        error.to_string(),
        r"response header `x-value` did not match its documented type: unknown variant `\t`, expected `Listed`"
    );
    let HeaderError::Parse { message, .. } = error else {
        unreachable!()
    };
    assert_eq!(message, "unknown variant `\t`, expected `Listed`");
}

fn field_value() -> impl proptest::strategy::Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        proptest::prop_oneof![
            0x21u8..=0x7e,
            proptest::strategy::Just(b'\t'),
            0x80u8..=0xff
        ],
        0..8,
    )
}

proptest::proptest! {
    /// Over field values drawn from visible ASCII, tab, and obs-text, every shape, and both
    /// `explode` settings, a header either parses or fails with no control character in any
    /// message of its `source()` chain.
    #[test]
    fn no_header_error_message_carries_a_control_character(
        raw in field_value(),
        shape in proptest::sample::select(SHAPES.to_vec()),
        explode in proptest::bool::ANY,
    ) {
        for message in messages(&raw, shape, explode) {
            proptest::prop_assert!(
                !message.chars().any(char::is_control),
                "{:?} {:?} {}: {:?}",
                raw,
                shape,
                explode,
                message
            );
        }
    }
}

/// Header text a `simple` token can hold verbatim: visible ASCII, which `HeaderValue` accepts,
/// without `,`, which `simple` gives no escape for. `excluded` names further bytes to keep out.
fn token(excluded: &'static [u8]) -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::strategy::Strategy;
    proptest::collection::vec(0x21u8..=0x7e, 0..6).prop_map(move |bytes| {
        bytes
            .into_iter()
            .filter(|byte| *byte != b',' && !excluded.contains(byte))
            .map(char::from)
            .collect()
    })
}

/// Write `value` with `serialize_simple` under `explode`, then read it back with
/// `require_header` under `shape` and the same `explode`.
fn round_trip<T: serde::Serialize + DeserializeOwned>(
    value: &T,
    shape: HeaderShape,
    explode: bool,
) -> Result<T, HeaderError> {
    let written = serialize_simple(value, explode, PercentEncoding::Passthrough).unwrap();
    require_header(
        &headers(&[("x-value", &written)]),
        "x-value",
        shape,
        explode,
    )
}

proptest::proptest! {
    /// The module's stated law: parsing is the inverse of `serialize_simple`, for every value
    /// `simple` can carry losslessly — a scalar of each JSON type, a non-empty array whose
    /// strings hold no `,`, and an object whose keys and values hold neither `,` nor `=` —
    /// under both `explode` settings. A string spelling a number or a boolean stays a string.
    #[test]
    fn require_header_inverts_serialize_simple(
        text in token(b""),
        unsigned in proptest::num::u64::ANY,
        signed in proptest::num::i64::ANY,
        flag in proptest::bool::ANY,
        texts in proptest::collection::vec(token(b""), 1..5),
        numbers in proptest::collection::vec(proptest::num::i64::ANY, 1..5),
        members in proptest::collection::btree_map(token(b"="), token(b"="), 0..5),
        explode in proptest::bool::ANY,
    ) {
        proptest::prop_assert_eq!(round_trip(&text, HeaderShape::Scalar, explode).unwrap(), text);
        proptest::prop_assert_eq!(
            round_trip(&unsigned, HeaderShape::Scalar, explode).unwrap(),
            unsigned
        );
        proptest::prop_assert_eq!(round_trip(&signed, HeaderShape::Scalar, explode).unwrap(), signed);
        proptest::prop_assert_eq!(round_trip(&flag, HeaderShape::Scalar, explode).unwrap(), flag);
        proptest::prop_assert_eq!(round_trip(&texts, HeaderShape::Array, explode).unwrap(), texts);
        proptest::prop_assert_eq!(
            round_trip(&numbers, HeaderShape::Array, explode).unwrap(),
            numbers
        );
        proptest::prop_assert_eq!(
            round_trip(&members, HeaderShape::Object, explode).unwrap(),
            members
        );
    }
}
