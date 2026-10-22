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
use serde_json::{Map, Value};

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
    /// A value that must be an object was not one. Three emitters return it: a `deepObject` query
    /// parameter over a non-object value ([`serialize_deep_object`]), a form-urlencoded body that
    /// does not serialize to a JSON object ([`serialize_form_body`]), and a multipart part asked for
    /// the query-only `deepObject` style ([`serialize_multipart_values`]).
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
        Value::Object(values) if explode => Ok(object_pairs(&values, encoding)?.join(",")),
        Value::Object(values) => object_flat(&values, ",", encoding),
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
        Value::Object(values) if explode => Ok(object_pairs(&values, encoding)?
            .iter()
            .map(|pair| format!(";{pair}"))
            .collect()),
        Value::Object(values) => Ok(format!(";{name}={}", object_flat(&values, ",", encoding)?)),
        value => Ok(format!(";{name}={}", scalar(&value, encoding)?)),
    }
}

/// Serialize one `style: label` path parameter, including its leading `.`.
///
/// A serialization that is exactly `.` or `..` is returned as it is. Where it forms a whole path
/// segment, `Url::set_path` would remove it in any spelling (`%2E` included), so the URL builder
/// refuses that path rather than let it re-target the request.
pub fn serialize_label<T: Serialize>(
    value: &T,
    explode: bool,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    let rendered = match serde_json::to_value(value)? {
        Value::Null => String::new(),
        Value::Array(values) if explode => join_scalars(values.iter(), ".", encoding)?,
        Value::Array(values) => join_scalars(values.iter(), ",", encoding)?,
        Value::Object(values) if explode => object_pairs(&values, encoding)?.join("."),
        Value::Object(values) => object_flat(&values, ",", encoding)?,
        value => scalar(&value, encoding)?,
    };
    Ok(format!(".{rendered}"))
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
        Value::Object(values) if explode => object_pairs(&values, encoding),
        Value::Object(values) => Ok(vec![format!(
            "{name}={}",
            object_flat(&values, ",", encoding)?
        )]),
        value => Ok(vec![format!("{name}={}", scalar(&value, encoding)?)]),
    }
}

/// Serialize one `style: spaceDelimited` or `style: pipeDelimited` query parameter.
///
/// Both styles are defined only for arrays and objects with `explode: false`, and any other value
/// is refused with [`ParameterError::ExpectedComposite`].
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
        Value::Object(values) => Ok(vec![format!(
            "{name}={}",
            object_flat(&values, delimiter.as_str(), encoding)?
        )]),
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
///
/// `additional` is the body's typed `additionalProperties` overflow: the resolved mode of its
/// members (the default for their type, since no Encoding Object can name them) and how many
/// members the overflow map holds, which a generated struct serializes after its declared fields.
/// When it is `Some`, those members follow the declared ones, ordered by key, rendered in that
/// mode; when it is `None`, members `properties` does not name are not sent. An overflow member
/// whose key repeats a declared property's name is never sent: the declared property's own value
/// is, or nothing when that property is absent, as in a multipart body.
///
/// A `value` that does not serialize to a JSON object is refused with
/// [`ParameterError::ExpectedObject`].
pub fn serialize_form_body<T: Serialize>(
    value: &T,
    properties: &[FormProperty],
    additional: Option<(FormMode, usize)>,
) -> Result<String, ParameterError> {
    // Serialized to text and read back member by member, a repeated key kept: a
    // `serde_json::Value` map keeps one entry per key, so a flattened overflow member would
    // overwrite the declared field of the same name, or stand in for it when that field is
    // skipped as `None`.
    let text = serde_json::to_vec(value)?;
    if text.first() != Some(&b'{') {
        return Err(ParameterError::ExpectedObject);
    }
    let OrderedMembers(members) = serde_json::from_slice(&text)?;
    let overflow_len = additional.map_or(0, |(_, len)| len);
    let (declared, overflow) = members.split_at(members.len().saturating_sub(overflow_len));
    let mut fragments: Vec<String> = Vec::new();
    for property in properties {
        if let Some((_, member)) = declared.iter().find(|(name, _)| name == property.name) {
            push_form_member(&mut fragments, property.name, member, property.mode)?;
        }
    }
    if let Some((mode, _)) = additional {
        // A generated body's overflow is a `BTreeMap`, so its members arrive ordered by key.
        for (name, member) in overflow {
            if !properties.iter().any(|property| property.name == name) {
                push_form_member(&mut fragments, name, member, mode)?;
            }
        }
    }
    Ok(fragments.join("&"))
}

/// A JSON object's members in the order they were written, a repeated key kept each time it
/// appears.
struct OrderedMembers(Vec<(String, Value)>);

impl<'de> serde::Deserialize<'de> for OrderedMembers {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MembersVisitor;
        impl<'de> serde::de::Visitor<'de> for MembersVisitor {
            type Value = OrderedMembers;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<OrderedMembers, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry::<String, Value>()? {
                    members.push(member);
                }
                Ok(OrderedMembers(members))
            }
        }
        deserializer.deserialize_map(MembersVisitor)
    }
}

/// Render one member of a form body in `mode`; a JSON `null` member adds nothing.
fn push_form_member(
    fragments: &mut Vec<String>,
    name: &str,
    member: &Value,
    mode: FormMode,
) -> Result<(), ParameterError> {
    if member.is_null() {
        return Ok(());
    }
    match mode {
        FormMode::Text => fragments.push(format!(
            "{}={}",
            encode(name, PercentEncoding::Form),
            scalar(member, PercentEncoding::Form)?
        )),
        FormMode::Json => fragments.push(format!(
            "{}={}",
            encode(name, PercentEncoding::Form),
            encode(&serde_json::to_string(member)?, PercentEncoding::Form)
        )),
        FormMode::Style {
            style,
            explode,
            encoding,
        } => fragments.extend(style_fragments(name, member, style, explode, encoding)?),
    }
    Ok(())
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
/// style) rather than guessing at one. A `deepObject` `style` is refused with
/// [`ParameterError::ExpectedObject`].
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

/// An object's members as `key,value` alternating in one `separator`-joined value: the
/// `explode: false` shape every style but `deepObject` shares.
fn object_flat(
    values: &Map<String, Value>,
    separator: &str,
    encoding: PercentEncoding,
) -> Result<String, ParameterError> {
    let mut parts = Vec::with_capacity(values.len() * 2);
    for (key, value) in values {
        parts.push(encode(key, encoding));
        parts.push(scalar(value, encoding)?);
    }
    Ok(parts.join(separator))
}

/// An object's members as one `key=value` fragment each: the `explode: true` shape, which each
/// style then prefixes or joins.
fn object_pairs(
    values: &Map<String, Value>,
    encoding: PercentEncoding,
) -> Result<Vec<String>, ParameterError> {
    values
        .iter()
        .map(|(key, value)| {
            Ok(format!(
                "{}={}",
                encode(key, encoding),
                scalar(value, encoding)?
            ))
        })
        .collect()
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
mod tests;
