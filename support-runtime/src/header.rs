//! Typed access to documented response headers.
//!
//! Parsing is the inverse of [`crate::parameter::serialize_simple`]: OpenAPI's `simple` style is a
//! lossy string encoding, so the shape a value was written in cannot be recovered from the text
//! alone. Codegen supplies it, because it is a static function of the documented schema.
//!
//! Header parsing is an explicitly-called second step rather than part of an operation's return
//! type. The body is already decoded and returned before a caller opts in, so a malformed or
//! missing header can never turn a successful call into a failed one.

use std::fmt;

use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The wire shape a documented header value is parsed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderShape {
    /// A single scalar.
    Scalar,
    /// A comma-separated list.
    Array,
    /// Alternating `key,value` pairs, or `key=value` pairs when `explode` is set.
    Object,
    /// A `content`-typed header carrying JSON.
    Json,
    /// `Set-Cookie`: one value per occurrence, never joined, decoded into a list.
    ///
    /// RFC 9110 §5.3 exempts this one field from the rule that lets a repeated header be folded
    /// into a comma-separated line: its values may contain unescaped commas, so it is
    /// one-value-per-line only. Joining and re-splitting on `,` would cut cookie values in half —
    /// an `Expires=Wed, 09 Jun 2021 …` attribute is enough to trigger it.
    SetCookie,
}

/// Why a documented response header could not be read.
#[derive(Debug)]
pub enum HeaderError {
    /// A `required` documented header was absent from the response.
    Missing {
        /// The header name.
        name: &'static str,
    },
    /// The header bytes are not valid UTF-8, so there is no text to parse.
    NotUtf8 {
        /// The header name.
        name: &'static str,
    },
    /// The value did not deserialize into the documented type.
    Parse {
        /// The header name.
        name: &'static str,
        /// The underlying serde message.
        message: String,
    },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { name } => {
                write!(formatter, "required response header `{name}` is absent")
            }
            Self::NotUtf8 { name } => {
                write!(formatter, "response header `{name}` is not valid UTF-8")
            }
            Self::Parse { name, message } => write!(
                formatter,
                "response header `{name}` did not match its documented type: {message}"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Parse one documented response header, or `Ok(None)` when it is absent.
///
/// Repeated occurrences are joined with `,` first, which is the RFC 9110 field-list rule and the
/// same shape a non-exploded array is written in.
pub fn parse_header<T: DeserializeOwned>(
    headers: &HeaderMap,
    name: &'static str,
    shape: HeaderShape,
    explode: bool,
) -> Result<Option<T>, HeaderError> {
    let mut values = headers.get_all(name).iter().peekable();
    if values.peek().is_none() {
        return Ok(None);
    }
    let mut parts: Vec<&str> = Vec::new();
    for value in values {
        parts.push(value.to_str().map_err(|_| HeaderError::NotUtf8 { name })?);
    }
    // `Set-Cookie` is decoded per occurrence and never joined; see [`HeaderShape::SetCookie`].
    if shape == HeaderShape::SetCookie {
        let lines =
            |scalars| Value::Array(parts.iter().map(|part| scalar(part, scalars)).collect());
        if let Ok(value) = serde_json::from_value(lines(Scalars::Text)) {
            return Ok(Some(value));
        }
        return serde_json::from_value(lines(Scalars::Json))
            .map(Some)
            .map_err(|error| HeaderError::Parse {
                name,
                message: error.to_string(),
            });
    }
    let raw = parts.join(",");
    // `simple` is plain text, so a token like `1` is ambiguous between the string "1" and the
    // number 1 — the encoding cannot tell them apart. The target type can: try the text as-is
    // first, and fall back to its JSON reading only if that does not deserialize. The error
    // reported is the JSON-flavored one, which is the more informative of the two.
    let textual = reconstruct(&raw, shape, explode, Scalars::Text);
    if let Ok(value) = serde_json::from_value(textual) {
        return Ok(Some(value));
    }
    let typed = reconstruct(&raw, shape, explode, Scalars::Json);
    serde_json::from_value(typed)
        .map(Some)
        .map_err(|error| HeaderError::Parse {
            name,
            message: error.to_string(),
        })
}

/// How a bare header token is interpreted when rebuilding a JSON value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalars {
    /// Every token is a string.
    Text,
    /// A token that reads as a JSON number, boolean, or null takes that type.
    Json,
}

/// As [`parse_header`], but an absent header is [`HeaderError::Missing`].
pub fn require_header<T: DeserializeOwned>(
    headers: &HeaderMap,
    name: &'static str,
    shape: HeaderShape,
    explode: bool,
) -> Result<T, HeaderError> {
    parse_header(headers, name, shape, explode)?.ok_or(HeaderError::Missing { name })
}

/// Rebuild a JSON value from `simple`-style header text.
fn reconstruct(raw: &str, shape: HeaderShape, explode: bool, scalars: Scalars) -> Value {
    match shape {
        HeaderShape::Json => serde_json::from_str(raw).unwrap_or(Value::Null),
        // Handled before any joining happens, in `parse_header`.
        HeaderShape::SetCookie | HeaderShape::Scalar => scalar(raw, scalars),
        HeaderShape::Array => {
            Value::Array(raw.split(',').map(|part| scalar(part, scalars)).collect())
        }
        HeaderShape::Object => {
            let mut members = serde_json::Map::new();
            if explode {
                for pair in raw.split(',') {
                    if let Some((key, value)) = pair.split_once('=') {
                        members.insert(key.to_owned(), scalar(value, scalars));
                    }
                }
            } else {
                let parts: Vec<&str> = raw.split(',').collect();
                for pair in parts.chunks(2) {
                    if let [key, value] = pair {
                        members.insert((*key).to_owned(), scalar(value, scalars));
                    }
                }
            }
            Value::Object(members)
        }
    }
}

/// Interpret one header token.
fn scalar(raw: &str, scalars: Scalars) -> Value {
    if scalars == Scalars::Text {
        return Value::String(raw.to_owned());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ (Value::Number(_) | Value::Bool(_) | Value::Null)) => value,
        _ => Value::String(raw.to_owned()),
    }
}

#[cfg(test)]
mod tests {
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
        let split: Vec<String> =
            require_header(&map, "set-cookie", HeaderShape::Array, false).unwrap();
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
    fn scalars_round_trip_through_simple_serialization() {
        let written = serialize_simple(&"abc", false, PercentEncoding::Passthrough).unwrap();
        let map = headers(&[("etag", &written)]);
        let parsed: String = require_header(&map, "etag", HeaderShape::Scalar, false).unwrap();
        assert_eq!(parsed, "abc");
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
    fn arrays_round_trip_through_simple_serialization() {
        let values = vec!["a".to_owned(), "b".to_owned()];
        let written = serialize_simple(&values, false, PercentEncoding::Passthrough).unwrap();
        let map = headers(&[("x-tags", &written)]);
        let parsed: Vec<String> =
            require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
        assert_eq!(parsed, values);
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
        let parsed: Vec<String> =
            require_header(&map, "x-tags", HeaderShape::Array, false).unwrap();
        assert_eq!(parsed, ["a", "b"]);
    }

    #[test]
    fn a_json_content_header_parses_as_json() {
        let map = headers(&[("x-payload", r#"{"id":7}"#)]);
        let parsed: serde_json::Map<String, Value> =
            require_header(&map, "x-payload", HeaderShape::Json, false).unwrap();
        assert_eq!(parsed.get("id"), Some(&Value::Number(7.into())));
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
}
