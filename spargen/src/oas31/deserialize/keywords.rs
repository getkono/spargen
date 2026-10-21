//! Schema Object keywords whose value is held to a JSON shape: `type`, `required`, and the
//! validation keywords. A count keyword, `type`, or `required` of the wrong shape is `E011` at the
//! keyword (or the array element) that holds it; dropping it would silently widen the schema.

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer};
use crate::source::{Node, Number, SpannedMap, SpannedValue};

use super::super::{JsonType, TypeSet, ValidationKeywords};
use super::{expect_array, expect_string, provenance, string};

/// A Schema Object's `required`: an array of property names, or `E011` at the keyword when it is
/// not an array and at the element when one is not a string. Either used to be dropped, leaving
/// the property optional.
pub(super) fn parse_required(
    value: Option<&SpannedValue>,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<String> {
    value
        .and_then(|value| expect_array(value, pointer, diags))
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, name)| expect_string(name, &pointer.index(index), diags))
        .map(str::to_owned)
        .collect()
}

/// A Schema Object's `type`: one JSON type name or an array of them. A value of any other shape,
/// an element that is not a string, and a name that is no JSON type are each `E011` where they
/// stand (the keyword, or the array element), since dropping one would widen the schema.
pub(super) fn parse_type_set(
    value: Option<&SpannedValue>,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> TypeSet {
    let mut types = Vec::new();
    let Some(value) = value else {
        return TypeSet { types };
    };
    match &value.node {
        Node::String(_) => types.extend(json_type(value, pointer, diags)),
        Node::Array(values) => {
            for (index, element) in values.iter().enumerate() {
                types.extend(json_type(element, &pointer.index(index), diags));
            }
        }
        _ => {
            Diagnostic::error(Code::InvalidInput, provenance(pointer, value))
                .message("expected a JSON type name or an array of them")
                .emit(diags);
        }
    }
    TypeSet { types }
}

/// The JSON type `value` names, or `E011` at `pointer` when it is not a string naming one.
fn json_type(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<JsonType> {
    let name = expect_string(value, pointer, diags)?;
    let found = parse_json_type(name);
    if found.is_none() {
        Diagnostic::error(Code::InvalidInput, provenance(pointer, value))
            .message(format!(
                "`{name}` is not a JSON type; expected one of `null`, `boolean`, `object`, \
                 `array`, `number`, `integer`, `string`"
            ))
            .emit(diags);
    }
    found
}

fn parse_json_type(value: &str) -> Option<JsonType> {
    Some(match value {
        "null" => JsonType::Null,
        "boolean" => JsonType::Boolean,
        "object" => JsonType::Object,
        "array" => JsonType::Array,
        "number" => JsonType::Number,
        "integer" => JsonType::Integer,
        "string" => JsonType::String,
        _ => return None,
    })
}

pub(super) fn parse_validation(
    map: &SpannedMap,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> ValidationKeywords {
    let mut bound = |keyword: &str| count(map, keyword, pointer, diags);
    ValidationKeywords {
        pattern: map.get("pattern").and_then(string).map(str::to_owned),
        minimum: map.get("minimum").and_then(number_f64),
        maximum: map.get("maximum").and_then(number_f64),
        exclusive_minimum: map.get("exclusiveMinimum").and_then(number_f64),
        exclusive_maximum: map.get("exclusiveMaximum").and_then(number_f64),
        multiple_of: map.get("multipleOf").and_then(number_f64),
        min_length: bound("minLength"),
        max_length: bound("maxLength"),
        min_items: bound("minItems"),
        max_items: bound("maxItems"),
        unique_items: map
            .get("uniqueItems")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        min_properties: bound("minProperties"),
        max_properties: bound("maxProperties"),
        other: [
            "not",
            "if",
            "then",
            "else",
            "contains",
            "minContains",
            "maxContains",
            "dependentSchemas",
            "dependentRequired",
            "propertyNames",
            "unevaluatedProperties",
            "unevaluatedItems",
        ]
        .iter()
        .any(|keyword| map.get(keyword).is_some()),
    }
}

/// A count keyword (`minLength`, `maxItems`, ...) of the schema at `pointer`: absent, or a
/// non-negative integer, or `E011` at the keyword, which used to read as no bound at all.
fn count(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<u64> {
    let value = map.get(keyword)?;
    let found = number_u64(value);
    if found.is_none() {
        Diagnostic::error(
            Code::InvalidInput,
            provenance(&pointer.push(keyword), value),
        )
        .message(format!("`{keyword}` must be a non-negative integer"))
        .emit(diags);
    }
    found
}

fn number_f64(value: &SpannedValue) -> Option<f64> {
    match &value.node {
        Node::Number(Number::Float(value)) => Some(*value),
        Node::Number(Number::Int(value)) => Some(*value as f64),
        Node::Number(Number::UInt(value)) => Some(*value as f64),
        _ => None,
    }
}

/// A non-negative integer. A float with no fractional part counts, as JSON Schema 2020-12 reads
/// `5.0` as the integer 5 (core, section 4.2.2).
fn number_u64(value: &SpannedValue) -> Option<u64> {
    match &value.node {
        Node::Number(Number::UInt(value)) => Some(*value),
        Node::Number(Number::Int(value)) => u64::try_from(*value).ok(),
        // `u64::MAX as f64` rounds up to 2^64, which no `u64` holds, so the bound is exclusive.
        Node::Number(Number::Float(value))
            if value.fract() == 0.0 && *value >= 0.0 && *value < u64::MAX as f64 =>
        {
            Some(*value as u64)
        }
        _ => None,
    }
}
