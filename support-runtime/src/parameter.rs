//! OpenAPI parameter serialization for every style the specification defines.
//!
//! Generated methods keep parameter values typed until request construction. These helpers use
//! `serde` only as an inspection bridge, then apply the OpenAPI delimiter, `explode`, and
//! percent-encoding rules; they never rely on a generated model implementing [`std::fmt::Display`].
//!
//! The governing invariant is that **delimiters are emitted literally and every data byte is
//! percent-encoded**. That is what makes a `,` that joins two array items distinguishable from a
//! `,` inside one of the values — OpenAPI requires exactly this, and it is why query fragments are
//! assembled here rather than through `url`'s `query_pairs_mut`, which would encode both alike.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// Which characters survive unencoded when a serialized value is placed on the wire.
///
/// The variant is chosen by codegen from the parameter's location, style, and `allowReserved`;
/// the mapping lives in exactly one place in the generator so the rule cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercentEncoding {
    /// Encode every byte outside RFC 3986 `unreserved` (`A-Za-z0-9` and `-._~`). Used for path
    /// values with `allowReserved: false`; guarantees no unescaped `/`, `?`, or `#` can reach the
    /// path and silently re-route the request.
    Unreserved,
    /// As [`Unreserved`](Self::Unreserved) but also encodes `~`, matching the `form-urlencoded`
    /// safe set. Used for query and `style: form` cookie values. A space is always `%20`, never
    /// `+`.
    Form,
    /// RFC 6570 reserved expansion for a query value (`allowReserved: true`): RFC 3986 reserved
    /// characters and well-formed percent-triples pass through, except `#`, `[`, `]`, `&`, `=`,
    /// and `+`, which would break the query string or the `form-urlencoded` pair syntax. A `%`
    /// that does not begin a valid triple is encoded.
    Reserved,
    /// Reserved expansion for a *path* value: as [`Reserved`](Self::Reserved), but `&`, `=`, `+`,
    /// `[`, and `]` pass through while `/`, `?`, and `#` are still encoded — the specification
    /// forbids those unescaped in a path regardless of `allowReserved`.
    ReservedPath,
    /// Pass the value through byte-for-byte. Used for headers, `style: cookie`, and every
    /// `multipart/form-data` value, all of which the specification says must not be percent-encoded.
    Passthrough,
}

/// The delimiter of the non-RFC 6570 query styles. Always emitted percent-encoded, because an
/// unencoded space or `|` is not legal in a query string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    /// `style: spaceDelimited` — `%20`.
    Space,
    /// `style: pipeDelimited` — `%7C`.
    Pipe,
}

impl Delimiter {
    /// The already-encoded delimiter text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Space => "%20",
            Self::Pipe => "%7C",
        }
    }
}

/// A typed parameter could not be represented by OpenAPI's scalar/array/object wire model.
#[derive(Debug)]
pub enum ParameterError {
    /// Serialization of the generated Rust value failed.
    Serialize(serde_json::Error),
    /// A nested array or object appeared where OpenAPI parameter serialization requires a scalar.
    NestedValue,
    /// A style defined only for objects (`deepObject`) received a non-object value.
    ExpectedObject,
    /// A style defined only for arrays and objects (`spaceDelimited`, `pipeDelimited`) received a
    /// scalar or undefined value.
    ExpectedComposite,
}

impl fmt::Display for ParameterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => write!(formatter, "parameter serialization failed: {error}"),
            Self::NestedValue => formatter.write_str(
                "nested arrays and objects are not supported by OpenAPI parameter serialization",
            ),
            Self::ExpectedObject => {
                formatter.write_str("`style: deepObject` requires an object parameter value")
            }
            Self::ExpectedComposite => formatter.write_str(
                "`style: spaceDelimited` and `style: pipeDelimited` require an array or object \
                 parameter value",
            ),
        }
    }
}

impl std::error::Error for ParameterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::NestedValue | Self::ExpectedObject | Self::ExpectedComposite => None,
        }
    }
}

impl From<serde_json::Error> for ParameterError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// Percent-encode `value` under `encoding`.
///
/// Encoding operates on UTF-8 bytes, so one multi-byte character yields one triple per byte.
pub fn encode(value: &str, encoding: PercentEncoding) -> String {
    if encoding == PercentEncoding::Passthrough {
        return value.to_owned();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        // Reserved expansion must let an already-percent-encoded triple through untouched while
        // still encoding a bare `%`; that is a look-ahead, not a character-set test.
        if byte == b'%'
            && matches!(
                encoding,
                PercentEncoding::Reserved | PercentEncoding::ReservedPath
            )
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(bytes[index + 1] as char);
            out.push(bytes[index + 2] as char);
            index += 3;
            continue;
        }
        if keeps(byte, encoding) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(upper_hex(byte >> 4));
            out.push(upper_hex(byte & 0x0f));
        }
        index += 1;
    }
    out
}

/// Whether `byte` is emitted literally under `encoding`.
fn keeps(byte: u8, encoding: PercentEncoding) -> bool {
    let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
    match encoding {
        PercentEncoding::Passthrough => true,
        PercentEncoding::Unreserved => unreserved,
        // `~` is escaped for `form-urlencoded` interoperability.
        PercentEncoding::Form => unreserved && byte != b'~',
        // RFC 3986 reserved, minus the characters that would break query or pair syntax.
        PercentEncoding::Reserved => {
            unreserved
                || matches!(
                    byte,
                    b':' | b'/'
                        | b'?'
                        | b'@'
                        | b'!'
                        | b'$'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b','
                        | b';'
                )
        }
        // As `Reserved`, but a path may carry `&=+[]` and must never carry `/?#`.
        PercentEncoding::ReservedPath => {
            unreserved
                || matches!(
                    byte,
                    b':' | b'@'
                        | b'!'
                        | b'$'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b','
                        | b';'
                        | b'&'
                        | b'='
                        | b'+'
                        | b'['
                        | b']'
                )
        }
    }
}

fn upper_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Serialize one `style: simple` value for a path or header parameter.
///
/// Scalars render directly, arrays are comma-separated, and objects alternate keys and values
/// unless `explode` is true, in which case each member is rendered as `key=value`.
pub fn serialize_simple<T: Serialize>(
    value: &T,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    match serde_json::to_value(value)? {
        Value::Null => Ok(String::new()),
        Value::Array(values) => join_scalars(values.iter(), ",", encoding),
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "{}={}",
                    encode(key, encoding),
                    scalar(value, encoding)?
                ))
            })
            .collect::<Result<Vec<_>, ParameterError>>()
            .map(|parts| parts.join(",")),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(encode(&key, encoding));
                parts.push(scalar(&value, encoding)?);
            }
            Ok(parts.join(","))
        }
        value => scalar(&value, encoding),
    }
}

/// Serialize one `style: matrix` path parameter, including its leading `;` and — where the
/// serialization rules keep it — the parameter name.
pub fn serialize_matrix<T: Serialize>(
    name: &str,
    value: &T,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    let name = encode(name, encoding);
    match serde_json::to_value(value)? {
        // An undefined value still carries the name, with no `=`.
        Value::Null => Ok(format!(";{name}")),
        Value::Array(values) if explode => values
            .iter()
            .map(|value| Ok(format!(";{name}={}", scalar(value, encoding)?)))
            .collect::<Result<Vec<_>, ParameterError>>()
            .map(|parts| parts.concat()),
        Value::Array(values) => Ok(format!(
            ";{name}={}",
            join_scalars(values.iter(), ",", encoding)?
        )),
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    ";{}={}",
                    encode(key, encoding),
                    scalar(value, encoding)?
                ))
            })
            .collect::<Result<Vec<_>, ParameterError>>()
            .map(|parts| parts.concat()),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(encode(&key, encoding));
                parts.push(scalar(&value, encoding)?);
            }
            Ok(format!(";{name}={}", parts.join(",")))
        }
        value => Ok(format!(";{name}={}", scalar(&value, encoding)?)),
    }
}

/// Serialize one `style: label` path parameter, including its leading `.`.
///
/// A serialization that is exactly `.` or `..` is emitted as `%2E`/`%2E%2E`; otherwise URL path
/// normalization would remove it as a dot segment and silently change the request target.
pub fn serialize_label<T: Serialize>(
    value: &T,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    let rendered = match serde_json::to_value(value)? {
        Value::Null => String::new(),
        Value::Array(values) if explode => join_scalars(values.iter(), ".", encoding)?,
        Value::Array(values) => join_scalars(values.iter(), ",", encoding)?,
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "{}={}",
                    encode(key, encoding),
                    scalar(value, encoding)?
                ))
            })
            .collect::<Result<Vec<_>, ParameterError>>()
            .map(|parts| parts.join("."))?,
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(encode(&key, encoding));
                parts.push(scalar(&value, encoding)?);
            }
            parts.join(",")
        }
        value => scalar(&value, encoding)?,
    };
    let labelled = format!(".{rendered}");
    Ok(match labelled.as_str() {
        "." => "%2E".to_owned(),
        ".." => "%2E%2E".to_owned(),
        _ => labelled,
    })
}

/// Serialize one `style: form` value into fully-encoded `name=value` query or cookie fragments.
///
/// With `explode: true`, arrays repeat the parameter name and objects use each property name.
/// With `explode: false`, either shape is flattened into one comma-separated value under `name`.
/// `style: cookie` is exactly this function with [`PercentEncoding::Passthrough`] — the two rows
/// of the specification's serialization table are otherwise identical.
pub fn serialize_form<T: Serialize>(
    name: &str,
    value: &T,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<Vec<String>, ParameterError> {
    let name = encode(name, encoding);
    match serde_json::to_value(value)? {
        Value::Null => Ok(vec![format!("{name}=")]),
        Value::Array(values) if explode => values
            .iter()
            .map(|value| Ok(format!("{name}={}", scalar(value, encoding)?)))
            .collect(),
        Value::Array(values) => Ok(vec![format!(
            "{name}={}",
            join_scalars(values.iter(), ",", encoding)?
        )]),
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "{}={}",
                    encode(key, encoding),
                    scalar(value, encoding)?
                ))
            })
            .collect(),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(encode(&key, encoding));
                parts.push(scalar(&value, encoding)?);
            }
            Ok(vec![format!("{name}={}", parts.join(","))])
        }
        value => Ok(vec![format!("{name}={}", scalar(&value, encoding)?)]),
    }
}

/// Serialize one `style: spaceDelimited` or `style: pipeDelimited` query parameter.
///
/// Both styles are defined only for arrays and objects with `explode: false`; the frontend rejects
/// every other combination, so a scalar here is a generator bug rather than user input.
pub fn serialize_delimited<T: Serialize>(
    name: &str,
    value: &T,
    delimiter: Delimiter,
    encoding: PercentEncoding,
) -> Result<Vec<String>, ParameterError> {
    let name = encode(name, encoding);
    match serde_json::to_value(value)? {
        Value::Array(values) => Ok(vec![format!(
            "{name}={}",
            join_scalars(values.iter(), delimiter.as_str(), encoding)?
        )]),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(encode(&key, encoding));
                parts.push(scalar(&value, encoding)?);
            }
            Ok(vec![format!("{name}={}", parts.join(delimiter.as_str()))])
        }
        _ => Err(ParameterError::ExpectedComposite),
    }
}

/// Serialize one `style: deepObject` query parameter into `name[key]=value` fragments.
///
/// `explode` has no effect on this style. The `[`/`]` are emitted as the literal delimiters
/// `%5B`/`%5D`; a bracket inside a property key encodes to the same triple, an ambiguity the
/// specification itself acknowledges as inherent to the style.
pub fn serialize_deep_object<T: Serialize>(
    name: &str,
    value: &T,
    encoding: PercentEncoding,
) -> Result<Vec<String>, ParameterError> {
    let name = encode(name, encoding);
    match serde_json::to_value(value)? {
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "{name}%5B{}%5D={}",
                    encode(key, encoding),
                    scalar(value, encoding)?
                ))
            })
            .collect(),
        _ => Err(ParameterError::ExpectedObject),
    }
}

/// The resolved wire encoding of one property of a form or multipart request body.
///
/// Codegen emits these as a `&'static [FormProperty]` const, fully resolved — the Encoding
/// Object's defaulting rules run at generation time, so the runtime never has to infer anything.
#[derive(Debug, Clone, Copy)]
pub struct FormProperty {
    /// The wire property name: the form field name, or the multipart part name.
    pub name: &'static str,
    /// How the value is rendered.
    pub mode: FormMode,
}

/// How one form or multipart property is rendered.
#[derive(Debug, Clone, Copy)]
pub enum FormMode {
    /// The scalar rendered as-is.
    Text,
    /// The value rendered as JSON.
    Json,
    /// RFC 6570 query-style serialization.
    Style {
        /// The form style in use.
        style: FormStyle,
        explode: bool,
        encoding: PercentEncoding,
    },
}

/// The subset of parameter styles an Encoding Object may select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormStyle {
    /// `style: form`.
    Form,
    /// `style: spaceDelimited` / `pipeDelimited`.
    Delimited(Delimiter),
    /// `style: deepObject`.
    DeepObject,
}

/// Serialize a whole `application/x-www-form-urlencoded` request body.
///
/// `value` must serialize to a JSON object. Each property is rendered by its own resolved
/// `FormProperty`, in the order codegen emitted them, so output is deterministic. A property whose
/// value is JSON `null` is omitted entirely rather than sent empty.
pub fn serialize_form_body<T: Serialize>(
    value: &T,
    properties: &[FormProperty],
) -> Result<String, ParameterError> {
    let Value::Object(members) = serde_json::to_value(value)? else {
        return Err(ParameterError::ExpectedObject);
    };
    let mut fragments: Vec<String> = Vec::new();
    for property in properties {
        let Some(member) = members.get(property.name) else {
            continue;
        };
        if member.is_null() {
            continue;
        }
        match property.mode {
            FormMode::Text => fragments.push(format!(
                "{}={}",
                encode(property.name, PercentEncoding::Form),
                scalar(member, PercentEncoding::Form)?
            )),
            FormMode::Json => fragments.push(format!(
                "{}={}",
                encode(property.name, PercentEncoding::Form),
                encode(&serde_json::to_string(member)?, PercentEncoding::Form)
            )),
            FormMode::Style {
                style,
                explode,
                encoding,
            } => fragments.extend(style_fragments(
                property.name,
                member,
                style,
                explode,
                encoding,
            )?),
        }
    }
    Ok(fragments.join("&"))
}

/// The per-part values for one RFC 6570-mode `multipart/form-data` property.
///
/// An array yields one value per item when `explode` is set, all sent under the same part name (the
/// RFC 7578 §4.3 shape the specification points at); otherwise the items are joined by the
/// style's delimiter into a single value. Any other shape yields one value.
///
/// Two rules make this deliberately *not* the query-fragment builders with the `name=` prefix
/// stripped:
///
/// - The part name travels in `Content-Disposition`, so no fragment syntax appears in the value.
/// - "When using RFC6570-style serialization for `multipart/form-data`, URI percent-encoding MUST
///   NOT be applied" — so the delimiters are emitted literally (` `, `|`, `,`), not as the `%20` /
///   `%7C` triples a query string needs, and the data is passed through byte-for-byte.
///
/// Object values never reach here: the specification defines no part representation for them, so
/// the frontend rejects an object-valued RFC 6570 encoding (and `deepObject`, which is a query-only
/// style) rather than guessing at one.
pub fn serialize_multipart_values<T: Serialize>(
    value: &T,
    style: FormStyle,
    explode: bool,
) -> Result<Vec<String>, ParameterError> {
    // Multipart part values are never percent-encoded, so every rendering below is passthrough.
    let raw = PercentEncoding::Passthrough;
    let separator = match style {
        FormStyle::Form => ",",
        FormStyle::Delimited(Delimiter::Space) => " ",
        FormStyle::Delimited(Delimiter::Pipe) => "|",
        // Defined only for `in: query`: its `name[key]=value` syntax has no meaning as a part value.
        FormStyle::DeepObject => return Err(ParameterError::ExpectedObject),
    };
    match serde_json::to_value(value)? {
        Value::Null => Ok(vec![String::new()]),
        Value::Array(values) if explode => values
            .iter()
            .map(|value| scalar(value, raw))
            .collect::<Result<Vec<_>, ParameterError>>(),
        Value::Array(values) => Ok(vec![join_scalars(values.iter(), separator, raw)?]),
        Value::Object(_) => Err(ParameterError::NestedValue),
        value => Ok(vec![scalar(&value, raw)?]),
    }
}

fn style_fragments(
    name: &str,
    value: &Value,
    style: FormStyle,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<Vec<String>, ParameterError> {
    match style {
        FormStyle::Form => serialize_form(name, value, explode, encoding),
        FormStyle::Delimited(delimiter) => serialize_delimited(name, value, delimiter, encoding),
        FormStyle::DeepObject => serialize_deep_object(name, value, encoding),
    }
}

fn join_scalars<'a>(
    values: impl Iterator<Item = &'a Value>,
    separator: &str,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    values
        .map(|value| scalar(value, encoding))
        .collect::<Result<Vec<_>, ParameterError>>()
        .map(|parts| parts.join(separator))
}

fn scalar(value: &Value, encoding: PercentEncoding) -> Result<String, ParameterError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(encode(value, encoding)),
        Value::Array(_) | Value::Object(_) => Err(ParameterError::NestedValue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specification's own worked example: `color` as a string, an array, and an object.
    ///
    /// `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature is deliberately not
    /// enabled anywhere in the runtime dependency set), so object members serialize in sorted key
    /// order — `B`, `G`, `R` — where the specification's table happens to print them in `R`, `G`,
    /// `B` order. OpenAPI does not fix object member order for parameter serialization, and sorted
    /// order is what keeps generated output deterministic, so the expectations below are the
    /// specification's values in sorted order.
    fn object() -> Value {
        serde_json::json!({ "R": 100, "G": 200, "B": 150 })
    }

    fn array() -> Value {
        serde_json::json!(["blue", "black", "brown"])
    }

    fn string() -> Value {
        serde_json::json!("blue")
    }

    fn undefined() -> Value {
        Value::Null
    }

    const U: PercentEncoding = PercentEncoding::Unreserved;
    const F: PercentEncoding = PercentEncoding::Form;
    const P: PercentEncoding = PercentEncoding::Passthrough;

    #[test]
    fn matrix_table_rows() {
        assert_eq!(
            serialize_matrix("color", &undefined(), false, U).unwrap(),
            ";color"
        );
        assert_eq!(
            serialize_matrix("color", &string(), false, U).unwrap(),
            ";color=blue"
        );
        assert_eq!(
            serialize_matrix("color", &array(), false, U).unwrap(),
            ";color=blue,black,brown"
        );
        assert_eq!(
            serialize_matrix("color", &object(), false, U).unwrap(),
            ";color=B,150,G,200,R,100"
        );
        assert_eq!(
            serialize_matrix("color", &undefined(), true, U).unwrap(),
            ";color"
        );
        assert_eq!(
            serialize_matrix("color", &string(), true, U).unwrap(),
            ";color=blue"
        );
        assert_eq!(
            serialize_matrix("color", &array(), true, U).unwrap(),
            ";color=blue;color=black;color=brown"
        );
        assert_eq!(
            serialize_matrix("color", &object(), true, U).unwrap(),
            ";B=150;G=200;R=100"
        );
    }

    #[test]
    fn label_table_rows() {
        // The undefined row is `.`, which must not survive as a removable dot segment.
        assert_eq!(serialize_label(&undefined(), false, U).unwrap(), "%2E");
        assert_eq!(serialize_label(&string(), false, U).unwrap(), ".blue");
        assert_eq!(
            serialize_label(&array(), false, U).unwrap(),
            ".blue,black,brown"
        );
        assert_eq!(
            serialize_label(&object(), false, U).unwrap(),
            ".B,150,G,200,R,100"
        );
        assert_eq!(serialize_label(&undefined(), true, U).unwrap(), "%2E");
        assert_eq!(serialize_label(&string(), true, U).unwrap(), ".blue");
        assert_eq!(
            serialize_label(&array(), true, U).unwrap(),
            ".blue.black.brown"
        );
        assert_eq!(
            serialize_label(&object(), true, U).unwrap(),
            ".B=150.G=200.R=100"
        );
    }

    #[test]
    fn simple_table_rows() {
        assert_eq!(serialize_simple(&undefined(), false, U).unwrap(), "");
        assert_eq!(serialize_simple(&string(), false, U).unwrap(), "blue");
        assert_eq!(
            serialize_simple(&array(), false, U).unwrap(),
            "blue,black,brown"
        );
        assert_eq!(
            serialize_simple(&object(), false, U).unwrap(),
            "B,150,G,200,R,100"
        );
        assert_eq!(serialize_simple(&undefined(), true, U).unwrap(), "");
        assert_eq!(serialize_simple(&string(), true, U).unwrap(), "blue");
        assert_eq!(
            serialize_simple(&array(), true, U).unwrap(),
            "blue,black,brown"
        );
        assert_eq!(
            serialize_simple(&object(), true, U).unwrap(),
            "B=150,G=200,R=100"
        );
    }

    #[test]
    fn form_table_rows() {
        assert_eq!(
            serialize_form("color", &undefined(), false, F).unwrap(),
            ["color="]
        );
        assert_eq!(
            serialize_form("color", &string(), false, F).unwrap(),
            ["color=blue"]
        );
        assert_eq!(
            serialize_form("color", &array(), false, F).unwrap(),
            ["color=blue,black,brown"]
        );
        assert_eq!(
            serialize_form("color", &object(), false, F).unwrap(),
            ["color=B,150,G,200,R,100"]
        );
        assert_eq!(
            serialize_form("color", &undefined(), true, F).unwrap(),
            ["color="]
        );
        assert_eq!(
            serialize_form("color", &string(), true, F).unwrap(),
            ["color=blue"]
        );
        assert_eq!(
            serialize_form("color", &array(), true, F).unwrap(),
            ["color=blue", "color=black", "color=brown"]
        );
        assert_eq!(
            serialize_form("color", &object(), true, F).unwrap(),
            ["B=150", "G=200", "R=100"]
        );
    }

    #[test]
    fn cookie_table_rows() {
        // `style: cookie` is `form` with no escaping; the caller joins the fragments with "; ".
        assert_eq!(
            serialize_form("color", &string(), false, P).unwrap(),
            ["color=blue"]
        );
        assert_eq!(
            serialize_form("color", &array(), false, P).unwrap(),
            ["color=blue,black,brown"]
        );
        assert_eq!(
            serialize_form("color", &array(), true, P).unwrap(),
            ["color=blue", "color=black", "color=brown"]
        );
        assert_eq!(
            serialize_form("color", &object(), true, P).unwrap(),
            ["B=150", "G=200", "R=100"]
        );
        // Passthrough really does pass reserved bytes through untouched.
        assert_eq!(
            serialize_form("color", &serde_json::json!("a b,c"), false, P).unwrap(),
            ["color=a b,c"]
        );
    }

    #[test]
    fn space_delimited_table_rows() {
        assert_eq!(
            serialize_delimited("color", &array(), Delimiter::Space, F).unwrap(),
            ["color=blue%20black%20brown"]
        );
        assert_eq!(
            serialize_delimited("color", &object(), Delimiter::Space, F).unwrap(),
            ["color=B%20150%20G%20200%20R%20100"]
        );
    }

    #[test]
    fn pipe_delimited_table_rows() {
        assert_eq!(
            serialize_delimited("color", &array(), Delimiter::Pipe, F).unwrap(),
            ["color=blue%7Cblack%7Cbrown"]
        );
        assert_eq!(
            serialize_delimited("color", &object(), Delimiter::Pipe, F).unwrap(),
            ["color=B%7C150%7CG%7C200%7CR%7C100"]
        );
    }

    #[test]
    fn delimited_styles_reject_scalars() {
        assert!(matches!(
            serialize_delimited("color", &string(), Delimiter::Pipe, F),
            Err(ParameterError::ExpectedComposite)
        ));
    }

    #[test]
    fn deep_object_table_row() {
        assert_eq!(
            serialize_deep_object("color", &object(), F).unwrap(),
            ["color%5BB%5D=150", "color%5BG%5D=200", "color%5BR%5D=100"]
        );
    }

    #[test]
    fn deep_object_rejects_non_objects() {
        assert!(matches!(
            serialize_deep_object("color", &array(), F),
            Err(ParameterError::ExpectedObject)
        ));
    }

    #[test]
    fn delimiters_are_distinguishable_from_data() {
        // The whole point of encoding here rather than through `url`: a comma that joins two items
        // and a comma inside one item must not look alike on the wire.
        let values = serde_json::json!(["a,b", "c"]);
        assert_eq!(
            serialize_form("color", &values, false, F).unwrap(),
            ["color=a%2Cb,c"]
        );
        let piped = serialize_delimited(
            "color",
            &serde_json::json!(["a|b", "c"]),
            Delimiter::Pipe,
            F,
        )
        .unwrap();
        assert_eq!(piped, ["color=a%7Cb%7Cc"]);
        let spaced = serialize_delimited(
            "color",
            &serde_json::json!(["a b", "c"]),
            Delimiter::Space,
            F,
        )
        .unwrap();
        assert_eq!(spaced, ["color=a%20b%20c"]);
    }

    #[test]
    fn percent_encoding_sets() {
        let raw = "a/b?c#d e%f~g,h|i[j]k&l=m+n";
        assert_eq!(
            encode(raw, PercentEncoding::Unreserved),
            "a%2Fb%3Fc%23d%20e%25f~g%2Ch%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
        );
        assert_eq!(
            encode(raw, PercentEncoding::Form),
            "a%2Fb%3Fc%23d%20e%25f%7Eg%2Ch%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
        );
        assert_eq!(
            encode(raw, PercentEncoding::Reserved),
            "a/b?c%23d%20e%25f~g,h%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
        );
        assert_eq!(
            encode(raw, PercentEncoding::ReservedPath),
            "a%2Fb%3Fc%23d%20e%25f~g,h%7Ci[j]k&l=m+n"
        );
        assert_eq!(encode(raw, PercentEncoding::Passthrough), raw);
    }

    #[test]
    fn reserved_expansion_preserves_triples_and_encodes_bare_percent() {
        assert_eq!(encode("%41", PercentEncoding::Reserved), "%41");
        assert_eq!(encode("%zz", PercentEncoding::Reserved), "%25zz");
        assert_eq!(encode("100%", PercentEncoding::Reserved), "100%25");
        // Without reserved expansion a triple is re-encoded, because the value is opaque data.
        assert_eq!(encode("%41", PercentEncoding::Unreserved), "%2541");
    }

    #[test]
    fn path_values_never_carry_unescaped_slash_question_or_hash() {
        for encoding in [PercentEncoding::Unreserved, PercentEncoding::ReservedPath] {
            let encoded = encode("a/b?c#d", encoding);
            assert!(
                !encoded.contains('/'),
                "{encoding:?} leaked a slash: {encoded}"
            );
            assert!(
                !encoded.contains('?'),
                "{encoding:?} leaked a question mark: {encoded}"
            );
            assert!(
                !encoded.contains('#'),
                "{encoding:?} leaked a hash: {encoded}"
            );
        }
    }

    #[test]
    fn multibyte_values_encode_per_utf8_byte() {
        assert_eq!(encode("é", PercentEncoding::Unreserved), "%C3%A9");
        assert_eq!(encode("🦀", PercentEncoding::Form), "%F0%9F%A6%80");
    }

    #[test]
    fn serialize_form_body_covers_both_encoding_modes() {
        // Media-type mode renders each property by its resolved content type; style mode uses
        // query-style serialization. Both appear in one body here.
        let body = serde_json::json!({
            "id": "f81d4fae",
            "address": { "city": "Somewhere", "zip": "99999+1234" },
            "tags": ["a", "b"]
        });
        let properties = [
            FormProperty {
                name: "id",
                mode: FormMode::Text,
            },
            FormProperty {
                name: "address",
                mode: FormMode::Json,
            },
            FormProperty {
                name: "tags",
                mode: FormMode::Style {
                    style: FormStyle::Form,
                    explode: true,
                    encoding: PercentEncoding::Form,
                },
            },
        ];
        assert_eq!(
            serialize_form_body(&body, &properties).unwrap(),
            "id=f81d4fae\
             &address=%7B%22city%22%3A%22Somewhere%22%2C%22zip%22%3A%2299999%2B1234%22%7D\
             &tags=a&tags=b"
        );
    }

    #[test]
    fn form_body_omits_null_properties() {
        let body = serde_json::json!({ "a": "x", "b": null });
        let properties = [
            FormProperty {
                name: "a",
                mode: FormMode::Text,
            },
            FormProperty {
                name: "b",
                mode: FormMode::Text,
            },
        ];
        assert_eq!(serialize_form_body(&body, &properties).unwrap(), "a=x");
    }

    #[test]
    fn form_body_emits_properties_in_the_declared_order() {
        // Order is codegen's, not the JSON object's, so output stays deterministic.
        let body = serde_json::json!({ "a": "1", "b": "2" });
        let properties = [
            FormProperty {
                name: "b",
                mode: FormMode::Text,
            },
            FormProperty {
                name: "a",
                mode: FormMode::Text,
            },
        ];
        assert_eq!(serialize_form_body(&body, &properties).unwrap(), "b=2&a=1");
    }

    #[test]
    fn form_body_rejects_a_non_object() {
        let properties: [FormProperty; 0] = [];
        assert!(matches!(
            serialize_form_body(&serde_json::json!("scalar"), &properties),
            Err(ParameterError::ExpectedObject)
        ));
    }

    #[test]
    fn serialize_multipart_values_yields_one_value_per_array_item() {
        // The part name travels in `Content-Disposition`, so the values carry no `name=` prefix,
        // and multipart values are never percent-encoded.
        let values = serde_json::json!(["a b", "c,d"]);
        assert_eq!(
            serialize_multipart_values(&values, FormStyle::Form, true).unwrap(),
            ["a b", "c,d"]
        );
        assert_eq!(
            serialize_multipart_values(&serde_json::json!("solo"), FormStyle::Form, false).unwrap(),
            ["solo"]
        );
    }

    /// The specification is explicit: "When using RFC6570-style serialization for
    /// `multipart/form-data`, URI percent-encoding MUST NOT be applied." Building these values out
    /// of the query fragment builders put `%20`/`%7C` triples into part bodies instead of the
    /// literal delimiters.
    #[test]
    fn multipart_delimiters_are_literal_not_percent_encoded() {
        let values = array();
        assert_eq!(
            serialize_multipart_values(&values, FormStyle::Delimited(Delimiter::Space), false)
                .unwrap(),
            ["blue black brown"]
        );
        assert_eq!(
            serialize_multipart_values(&values, FormStyle::Delimited(Delimiter::Pipe), false)
                .unwrap(),
            ["blue|black|brown"]
        );
        assert_eq!(
            serialize_multipart_values(&values, FormStyle::Form, false).unwrap(),
            ["blue,black,brown"]
        );
    }

    /// Data bytes are passed through untouched too — a part value is not a query fragment.
    #[test]
    fn multipart_values_are_never_percent_encoded() {
        let values = serde_json::json!(["a b&c", "d=e"]);
        assert_eq!(
            serialize_multipart_values(&values, FormStyle::Form, true).unwrap(),
            ["a b&c", "d=e"]
        );
    }

    /// An object has no defined part representation. Reusing the query builders silently produced
    /// one part per member holding only the *value* — the keys were dropped on the floor — so this
    /// now fails loudly. The frontend rejects the construct before generation, making this the
    /// belt to that braces.
    #[test]
    fn multipart_rejects_object_values_rather_than_dropping_their_keys() {
        assert!(matches!(
            serialize_multipart_values(&object(), FormStyle::Form, true),
            Err(ParameterError::NestedValue)
        ));
        assert!(matches!(
            serialize_multipart_values(&object(), FormStyle::Form, false),
            Err(ParameterError::NestedValue)
        ));
        // `deepObject` is defined only for `in: query`.
        assert!(matches!(
            serialize_multipart_values(&object(), FormStyle::DeepObject, false),
            Err(ParameterError::ExpectedObject)
        ));
    }

    #[test]
    fn nested_values_are_rejected() {
        let nested = serde_json::json!([["a"]]);
        assert!(matches!(
            serialize_simple(&nested, false, U),
            Err(ParameterError::NestedValue)
        ));
    }
}
