//! OpenAPI parameter serialization for the supported `simple` and `form` styles.
//!
//! Generated methods keep parameter values typed until request construction. These helpers use
//! `serde` only as an inspection bridge, then apply the OpenAPI delimiter and `explode` rules;
//! they never rely on a generated model implementing [`std::fmt::Display`].

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// A typed parameter could not be represented by OpenAPI's scalar/array/object wire model.
#[derive(Debug)]
pub enum ParameterError {
    /// Serialization of the generated Rust value failed.
    Serialize(serde_json::Error),
    /// A nested array or object appeared where OpenAPI parameter serialization requires a scalar.
    NestedValue,
}

impl fmt::Display for ParameterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => write!(formatter, "parameter serialization failed: {error}"),
            Self::NestedValue => formatter.write_str(
                "nested arrays and objects are not supported by simple/form parameter serialization",
            ),
        }
    }
}

impl std::error::Error for ParameterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::NestedValue => None,
        }
    }
}

impl From<serde_json::Error> for ParameterError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// Serialize one `style: simple` value for a path or header parameter.
///
/// Scalars render directly, arrays are comma-separated, and objects alternate keys and values
/// unless `explode` is true, in which case each member is rendered as `key=value`.
pub fn serialize_simple<T: Serialize>(value: &T, explode: bool) -> Result<String, ParameterError> {
    match serde_json::to_value(value)? {
        Value::Array(values) => join_scalars(values.iter(), ","),
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| Ok(format!("{key}={}", scalar(value)?)))
            .collect::<Result<Vec<_>, ParameterError>>()
            .map(|parts| parts.join(",")),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(key);
                parts.push(scalar(&value)?);
            }
            Ok(parts.join(","))
        }
        value => scalar(&value),
    }
}

/// Serialize one `style: form` value into query or cookie name/value pairs.
///
/// With `explode: true`, arrays repeat the parameter name and objects use each property name.
/// With `explode: false`, either shape is flattened into one comma-separated value under `name`.
pub fn serialize_form<T: Serialize>(
    name: &str,
    value: &T,
    explode: bool,
) -> Result<Vec<(String, String)>, ParameterError> {
    match serde_json::to_value(value)? {
        Value::Array(values) if explode => values
            .iter()
            .map(|value| Ok((name.to_owned(), scalar(value)?)))
            .collect(),
        Value::Array(values) => Ok(vec![(name.to_owned(), join_scalars(values.iter(), ",")?)]),
        Value::Object(values) if explode => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), scalar(value)?)))
            .collect(),
        Value::Object(values) => {
            let mut parts = Vec::with_capacity(values.len() * 2);
            for (key, value) in values {
                parts.push(key);
                parts.push(scalar(&value)?);
            }
            Ok(vec![(name.to_owned(), parts.join(","))])
        }
        value => Ok(vec![(name.to_owned(), scalar(&value)?)]),
    }
}

fn join_scalars<'a>(
    values: impl Iterator<Item = &'a Value>,
    separator: &str,
) -> Result<String, ParameterError> {
    values
        .map(scalar)
        .collect::<Result<Vec<_>, ParameterError>>()
        .map(|parts| parts.join(separator))
}

fn scalar(value: &Value) -> Result<String, ParameterError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.clone()),
        Value::Array(_) | Value::Object(_) => Err(ParameterError::NestedValue),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Serialize;

    use super::{serialize_form, serialize_simple, ParameterError};

    #[derive(Serialize)]
    struct Coordinates {
        role: String,
        x: i64,
    }

    #[test]
    fn simple_serializes_scalars_arrays_and_objects() {
        assert_eq!(serialize_simple(&42, false).unwrap(), "42");
        assert_eq!(
            serialize_simple(&["red", "blue"], false).unwrap(),
            "red,blue"
        );
        let coordinates = Coordinates {
            role: "admin".to_owned(),
            x: 7,
        };
        assert_eq!(
            serialize_simple(&coordinates, false).unwrap(),
            "role,admin,x,7"
        );
        assert_eq!(
            serialize_simple(&coordinates, true).unwrap(),
            "role=admin,x=7"
        );
    }

    #[test]
    fn form_honors_array_and_object_explode_rules() {
        assert_eq!(
            serialize_form("color", &["red", "blue"], false).unwrap(),
            vec![("color".to_owned(), "red,blue".to_owned())]
        );
        assert_eq!(
            serialize_form("color", &["red", "blue"], true).unwrap(),
            vec![
                ("color".to_owned(), "red".to_owned()),
                ("color".to_owned(), "blue".to_owned()),
            ]
        );

        let values = BTreeMap::from([("role", "admin"), ("x", "7")]);
        assert_eq!(
            serialize_form("id", &values, false).unwrap(),
            vec![("id".to_owned(), "role,admin,x,7".to_owned())]
        );
        assert_eq!(
            serialize_form("id", &values, true).unwrap(),
            vec![
                ("role".to_owned(), "admin".to_owned()),
                ("x".to_owned(), "7".to_owned()),
            ]
        );
    }

    #[test]
    fn nested_values_fail_explicitly() {
        let nested = vec![vec![1, 2]];
        assert!(matches!(
            serialize_simple(&nested, false),
            Err(ParameterError::NestedValue)
        ));
    }
}
