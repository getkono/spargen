//! Schema Object keywords whose value is held to a JSON shape: `type`, `required`, the six count
//! keywords (`minLength`, `maxLength`, `minItems`, `maxItems`, `minProperties`,
//! `maxProperties`), and the other validation keywords `parse_validation` reads (`pattern`, the
//! numeric bounds, `multipleOf`, `uniqueItems`). One of the wrong shape is `E011` at the keyword
//! (or the array element) that holds it; dropping it would silently widen the schema. The one
//! exception is OpenAPI 3.0's boolean `exclusiveMinimum`/`exclusiveMaximum`, which is `W001`.

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer};
use crate::source::{Node, Number, SpannedMap, SpannedValue};

use super::super::{JsonType, TypeSet, ValidationKeywords};
use super::{expect_array, expect_bool, expect_string, provenance};

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

/// The JSON Schema applicator keywords whose value is a single subschema, which spargen parses
/// into `Schema::validation_children` and does not enforce.
pub(super) const SUBSCHEMA_KEYWORDS: [&str; 8] = [
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
    "unevaluatedProperties",
    "unevaluatedItems",
];

/// A Schema Object's [`ValidationKeywords`]. `other` is set by any of [`SUBSCHEMA_KEYWORDS`],
/// `dependentSchemas`, `dependentRequired`, `minContains`, `maxContains`, or an OpenAPI 3.0
/// boolean `exclusiveMinimum`/`exclusiveMaximum` (see [`exclusive_bound`]).
pub(super) fn parse_validation(
    map: &SpannedMap,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> ValidationKeywords {
    let pattern = map
        .get("pattern")
        .and_then(|value| expect_string(value, &pointer.push("pattern"), diags))
        .map(str::to_owned);
    let minimum = number(map, "minimum", pointer, diags);
    let maximum = number(map, "maximum", pointer, diags);
    let (exclusive_minimum, boolean_minimum) =
        exclusive_bound(map, "exclusiveMinimum", pointer, diags);
    let (exclusive_maximum, boolean_maximum) =
        exclusive_bound(map, "exclusiveMaximum", pointer, diags);
    let multiple_of = number(map, "multipleOf", pointer, diags);
    let unique_items = map
        .get("uniqueItems")
        .and_then(|value| expect_bool(value, &pointer.push("uniqueItems"), diags))
        .unwrap_or(false);
    let mut bound = |keyword: &str| count(map, keyword, pointer, diags);
    ValidationKeywords {
        pattern,
        minimum,
        maximum,
        exclusive_minimum,
        exclusive_maximum,
        multiple_of,
        min_length: bound("minLength"),
        max_length: bound("maxLength"),
        min_items: bound("minItems"),
        max_items: bound("maxItems"),
        unique_items,
        min_properties: bound("minProperties"),
        max_properties: bound("maxProperties"),
        other: boolean_minimum
            || boolean_maximum
            || SUBSCHEMA_KEYWORDS
                .iter()
                .chain(&[
                    "minContains",
                    "maxContains",
                    "dependentSchemas",
                    "dependentRequired",
                ])
                .any(|keyword| map.get(keyword).is_some()),
    }
}

/// A numeric keyword (`minimum`, `multipleOf`, ...) of the schema at `pointer`: absent, or a
/// number, or `E011` at the keyword, which used to read as no constraint at all.
fn number(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<f64> {
    let value = map.get(keyword)?;
    let found = number_f64(value);
    if found.is_none() {
        Diagnostic::error(
            Code::InvalidInput,
            provenance(&pointer.push(keyword), value),
        )
        .message(format!("`{keyword}` must be a number"))
        .emit(diags);
    }
    found
}

/// `exclusiveMinimum` or `exclusiveMaximum`: the bound when it is a number, as JSON Schema 2020-12
/// writes it, and `true` in the second place when it is a boolean, OpenAPI 3.0's modifier of
/// `minimum`/`maximum`, which real-world documents labelled 3.1 still carry. Either reading
/// constrains only validation, which spargen does not enforce and the Rust shape does not
/// carry, so the boolean is reported with the other validation-only keywords (`W001`) rather
/// than rejected. Any other value is `E011` at the keyword.
fn exclusive_bound(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> (Option<f64>, bool) {
    match map.get(keyword) {
        Some(value) if value.as_bool().is_some() => (None, true),
        _ => (number(map, keyword, pointer, diags), false),
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
