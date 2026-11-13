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

/// The same headers under the ordinary list rule, which `SetCookie` exists to avoid: joined,
/// then split back apart on every comma, so two cookies come back as three fragments, the first
/// cut inside its `Expires` date and the second losing the space after that comma.
#[test]
fn the_array_shape_splits_set_cookie_values_on_every_comma() {
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
        split,
        [
            "lang=en; Expires=Wed",
            "09 Jun 2021 10:18:14 GMT",
            "sessionId=38afes7a8"
        ]
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

/// The header with `name` holding `raw`'s bytes, which need not be visible ASCII.
fn raw_header(name: &'static str, raw: &[u8]) -> HeaderMap {
    let mut map = HeaderMap::new();
    map.append(name, reqwest::header::HeaderValue::from_bytes(raw).unwrap());
    map
}

/// #1331: a field value is bytes, and `HeaderValue::to_str` accepts visible ASCII only; a value
/// that is valid UTF-8 beyond ASCII is text, read as such in every shape that reads text.
#[test]
fn a_non_ascii_utf8_value_is_read_as_text() {
    let map = raw_header("x-title", "Café".as_bytes());
    let title: String = require_header(&map, "x-title", HeaderShape::Scalar, false).unwrap();
    assert_eq!(title, "Café");

    let map = raw_header("x-tags", "Café,thé".as_bytes());
    let tags: Vec<String> = require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
    assert_eq!(tags, ["Café", "thé"]);

    let map = raw_header("x-meta", "nom=Café".as_bytes());
    let meta: std::collections::BTreeMap<String, String> =
        require_header(&map, "x-meta", HeaderShape::Object, true).unwrap();
    assert_eq!(meta["nom"], "Café");

    let map = raw_header("x-payload", r#"{"nom":"Café"}"#.as_bytes());
    let payload: std::collections::BTreeMap<String, String> =
        require_header(&map, "x-payload", HeaderShape::Json, false).unwrap();
    assert_eq!(payload["nom"], "Café");

    let map = raw_header("set-cookie", "nom=Café".as_bytes());
    let cookies: Vec<String> =
        require_header(&map, "set-cookie", HeaderShape::SetCookie, false).unwrap();
    assert_eq!(cookies, ["nom=Café"]);
}

/// A documented string enum with two members, for list elements that must match exactly.
#[derive(Debug, PartialEq, serde::Deserialize)]
enum Tag {
    #[serde(rename = "a")]
    A,
    #[serde(rename = "b")]
    B,
}

/// #1331: RFC 9110 §5.6.1 lets a list carry optional whitespace (SP or HTAB) around each comma,
/// and a proxy folding repeated fields writes `, `; each element is read without it, as a string,
/// an enum member, an integer and an object member alike.
#[test]
fn list_elements_drop_the_optional_whitespace_around_commas() {
    for raw in ["a, b", "a ,b", "a \t, \tb", "a,\tb"] {
        let map = headers(&[("x-tags", raw)]);
        let strings: Vec<String> =
            require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
        assert_eq!(strings, ["a", "b"], "{raw:?}");
        let tags: Vec<Tag> = require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
        assert_eq!(tags, [Tag::A, Tag::B], "{raw:?}");
    }

    let map = headers(&[("x-ids", "1, 2 ,3")]);
    let ids: Vec<i64> = require_header(&map, "x-ids", HeaderShape::Array, false).unwrap();
    assert_eq!(ids, [1, 2, 3]);

    let map = headers(&[("x-meta", "k=v, k2=v2")]);
    let exploded: std::collections::BTreeMap<String, String> =
        require_header(&map, "x-meta", HeaderShape::Object, true).unwrap();
    assert_eq!(exploded["k"], "v");
    assert_eq!(exploded["k2"], "v2");

    let map = headers(&[("x-meta", "k, 1 ,k2,\t2")]);
    let unexploded: std::collections::BTreeMap<String, u8> =
        require_header(&map, "x-meta", HeaderShape::Object, false).unwrap();
    assert_eq!(unexploded["k"], 1);
    assert_eq!(unexploded["k2"], 2);
}

/// #1331: RFC 9110 §5.6.1 has a recipient ignore an empty list element, as `a,,b`, a leading or
/// trailing comma, or an empty repeated occurrence produce; a list skips them rather than reading
/// an empty string, which no enum member or integer matches.
#[test]
fn empty_list_elements_are_skipped() {
    for (raw, expected) in [
        ("a,,b", &["a", "b"][..]),
        (",a,", &["a"]),
        ("a, ,b", &["a", "b"]),
        ("", &[]),
        (" , ", &[]),
    ] {
        let map = headers(&[("x-tags", raw)]);
        let strings: Vec<String> =
            require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
        assert_eq!(strings, expected, "{raw:?}");
    }

    let map = headers(&[("x-tags", "a"), ("x-tags", ""), ("x-tags", "b")]);
    let tags: Vec<Tag> = require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
    assert_eq!(tags, [Tag::A, Tag::B]);

    let map = headers(&[("x-ids", "1,,2,")]);
    let ids: Vec<i64> = require_header(&map, "x-ids", HeaderShape::Array, false).unwrap();
    assert_eq!(ids, [1, 2]);

    let map = headers(&[("x-meta", "k=v,,k2=v2,")]);
    let exploded: std::collections::BTreeMap<String, String> =
        require_header(&map, "x-meta", HeaderShape::Object, true).unwrap();
    assert_eq!(exploded.len(), 2);
    assert_eq!(exploded["k2"], "v2");

    let map = headers(&[("x-meta", ",k,,v,k2, ,v2")]);
    let unexploded: std::collections::BTreeMap<String, String> =
        require_header(&map, "x-meta", HeaderShape::Object, false).unwrap();
    assert_eq!(unexploded.len(), 2);
    assert_eq!(unexploded["k"], "v");
    assert_eq!(unexploded["k2"], "v2");
}

/// A scalar is one value, not a list: its commas and the whitespace beside them are text.
#[test]
fn a_scalar_keeps_its_commas_and_their_whitespace() {
    let map = headers(&[("x-title", "a, b ,c")]);
    let title: String = require_header(&map, "x-title", HeaderShape::Scalar, false).unwrap();
    assert_eq!(title, "a, b ,c");
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

/// As [`token`], but never empty: the list rule skips an empty element, so only a non-empty
/// one survives as a list element or an unexploded object's key or value.
fn element(excluded: &'static [u8]) -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::strategy::Strategy;
    token(excluded).prop_filter("a list element is non-empty", |text| !text.is_empty())
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

/// Every finite `f64`: normal and subnormal values of either sign, and both zeros.
fn finite() -> proptest::num::f64::Any {
    use proptest::num::f64;
    f64::POSITIVE | f64::NEGATIVE | f64::NORMAL | f64::SUBNORMAL | f64::ZERO
}

proptest::proptest! {
    /// The module's stated law: parsing is the inverse of `serialize_simple`, for every value
    /// `simple` can carry losslessly — a scalar of each JSON type, an array of up to four
    /// elements whose strings are non-empty and hold no `,`, and an object of up to four members
    /// whose keys and values are non-empty and hold neither `,` nor `=` — under both `explode`
    /// settings. A string spelling a number or a
    /// boolean stays a string. A finite float, alone or in an array, reads back as itself.
    #[test]
    fn require_header_inverts_serialize_simple(
        text in token(b""),
        unsigned in proptest::num::u64::ANY,
        signed in proptest::num::i64::ANY,
        real in finite(),
        flag in proptest::bool::ANY,
        texts in proptest::collection::vec(element(b""), 0..5),
        numbers in proptest::collection::vec(proptest::num::i64::ANY, 0..5),
        reals in proptest::collection::vec(finite(), 0..5),
        members in proptest::collection::btree_map(element(b"="), element(b"="), 0..5),
        explode in proptest::bool::ANY,
    ) {
        proptest::prop_assert_eq!(round_trip(&text, HeaderShape::Scalar, explode).unwrap(), text);
        proptest::prop_assert_eq!(
            round_trip(&unsigned, HeaderShape::Scalar, explode).unwrap(),
            unsigned
        );
        proptest::prop_assert_eq!(round_trip(&signed, HeaderShape::Scalar, explode).unwrap(), signed);
        proptest::prop_assert_eq!(
            round_trip(&real, HeaderShape::Scalar, explode).unwrap(),
            real
        );
        proptest::prop_assert_eq!(round_trip(&flag, HeaderShape::Scalar, explode).unwrap(), flag);
        proptest::prop_assert_eq!(round_trip(&texts, HeaderShape::Array, explode).unwrap(), texts);
        proptest::prop_assert_eq!(
            round_trip(&numbers, HeaderShape::Array, explode).unwrap(),
            numbers
        );
        proptest::prop_assert_eq!(
            round_trip(&reals, HeaderShape::Array, explode).unwrap(),
            reals
        );
        proptest::prop_assert_eq!(
            round_trip(&members, HeaderShape::Object, explode).unwrap(),
            members
        );
    }
}

/// Why the law above holds only for non-empty strings in a list: `simple` writes an empty array
/// and an array of one empty string alike, as the empty value, and the list rule skips an empty
/// element, so both read back as the former.
#[test]
fn an_array_of_one_empty_string_reads_back_empty() {
    for explode in [false, true] {
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            round_trip(&empty, HeaderShape::Array, explode).unwrap(),
            empty
        );
        assert_eq!(
            round_trip(&vec![String::new()], HeaderShape::Array, explode).unwrap(),
            empty
        );
    }
}

/// A float whose shortest text `serde_json`, without `float_roundtrip`, reads as its neighbour
/// (#1142) reads back as itself from a header, alone, in an array, and as an object member, and
/// so does that text with JSON whitespace around it.
#[test]
fn a_float_serde_json_misreads_reads_back_exactly() {
    let value = -3.009_224_478_676_051_8e292_f64;
    let text = "-3.0092244786760518e+292";
    assert_eq!(serde_json::to_string(&value).unwrap(), text);
    assert_eq!(text.parse::<f64>().unwrap(), value);

    let read: f64 = require_header(
        &headers(&[("x-value", text)]),
        "x-value",
        HeaderShape::Scalar,
        false,
    )
    .unwrap();
    assert_eq!(read, value);
    let read: Vec<f64> = require_header(
        &headers(&[("x-value", &format!("{text},{text}"))]),
        "x-value",
        HeaderShape::Array,
        false,
    )
    .unwrap();
    assert_eq!(read, [value, value]);
    let read: std::collections::BTreeMap<String, f64> = require_header(
        &headers(&[("x-value", &format!("a={text}"))]),
        "x-value",
        HeaderShape::Object,
        true,
    )
    .unwrap();
    assert_eq!(read["a"], value);
    assert_eq!(
        scalar(&format!("\t{text} "), Scalars::Json),
        Value::Number(serde_json::Number::from_f64(value).unwrap())
    );
}

/// The unexploded `form`, `matrix` and `label` renderings are `simple`'s value behind a prefix
/// (`name=`, `;name=`, `.`), so with the prefix removed each decodes back to its value through
/// `require_header`, scalars, arrays and objects alike.
#[test]
fn a_form_rendering_decodes_back_to_its_value() {
    use crate::parameter::{serialize_form, serialize_label, serialize_matrix};

    /// Each rendering of `value`, with its prefix removed.
    fn unprefixed<T: serde::Serialize>(value: &T) -> [String; 3] {
        let p = PercentEncoding::Passthrough;
        let [form] = <[String; 1]>::try_from(serialize_form("v", value, false, p).unwrap())
            .expect("an unexploded form value is one fragment");
        let matrix = serialize_matrix("v", value, false, p).unwrap();
        let label = serialize_label(value, false, p).unwrap();
        [
            form.strip_prefix("v=")
                .expect("form keeps the name")
                .to_owned(),
            matrix
                .strip_prefix(";v=")
                .expect("matrix keeps the name")
                .to_owned(),
            label
                .strip_prefix('.')
                .expect("label opens with a dot")
                .to_owned(),
        ]
    }

    fn decodes<T: serde::Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: T,
        shape: HeaderShape,
    ) {
        for (style, written) in ["form", "matrix", "label"].iter().zip(unprefixed(&value)) {
            let read: T =
                require_header(&headers(&[("x-value", &written)]), "x-value", shape, false)
                    .unwrap();
            assert_eq!(read, value, "{style}: {written:?}");
        }
    }

    decodes("blue".to_owned(), HeaderShape::Scalar);
    decodes(-1.5f64, HeaderShape::Scalar);
    decodes(true, HeaderShape::Scalar);
    decodes(
        vec!["blue".to_owned(), "black".to_owned()],
        HeaderShape::Array,
    );
    decodes(vec![3i64, -4], HeaderShape::Array);
    decodes(
        std::collections::BTreeMap::from([("R".to_owned(), 100u8), ("G".to_owned(), 200)]),
        HeaderShape::Object,
    );
}
