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

use crate::error::write_escaped;

/// The wire shape a documented header value is parsed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderShape {
    /// A single scalar.
    Scalar,
    /// A comma-separated list.
    Array,
    /// Alternating `key,value` pairs, or `key=value` pairs when `explode` is set. A pair that is
    /// incomplete (a trailing key with no value, or an exploded member without `=`) is dropped
    /// without an error.
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
        /// The underlying serde message, held as serde wrote it; `Display` escapes its control
        /// characters, since the message may quote the server-supplied value.
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
            Self::Parse { name, message } => {
                // `message` is serde's, which quotes the server-supplied value (an unknown enum
                // variant, say) verbatim, and a field value may legally hold a tab; control
                // characters are escaped so the value cannot break or forge the message's line.
                write!(
                    formatter,
                    "response header `{name}` did not match its documented type: "
                )?;
                write_escaped(formatter, message)
            }
        }
    }
}

impl std::error::Error for HeaderError {}

/// Parse one documented response header, or `Ok(None)` when it is absent.
///
/// Repeated occurrences are joined with `,` first, which is the RFC 9110 field-list rule and the
/// same shape a non-exploded array is written in. [`HeaderShape::SetCookie`] is the exception:
/// each occurrence is decoded as one list item and none are joined.
///
/// An occurrence that is not valid UTF-8 is [`HeaderError::NotUtf8`], and a value that does not
/// deserialize into `T` is [`HeaderError::Parse`].
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
    // A `content`-typed header is JSON text, with no `simple` ambiguity to resolve: it is parsed
    // once, and text that is not JSON reports the syntax error rather than reading as `null`.
    if shape == HeaderShape::Json {
        return serde_json::from_str(&raw)
            .map(Some)
            .map_err(|error| HeaderError::Parse {
                name,
                message: error.to_string(),
            });
    }
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
        HeaderShape::Scalar => scalar(raw, scalars),
        // `SetCookie` is handled before any joining happens, and `Json` before any reconstruction,
        // both in `parse_header`.
        HeaderShape::Json | HeaderShape::SetCookie => scalar(raw, scalars),
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
mod tests;
