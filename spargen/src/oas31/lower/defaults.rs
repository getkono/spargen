//! Schema `default` values: which are representable on the lowered type, and how the rest are
//! documented and reported.

use std::collections::HashMap;

use crate::diag::{Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{
    DefaultValue, FieldDefault, Prim, ScalarRepr, ScalarValue, Ty, TypeGraph, TypeId, TypeKind,
};
use crate::oas31::{RefOr, Schema, SchemaOr};
use crate::source::{Node, Number, SpannedValue};

use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Give a property's `default` its single explicit disposition. Returns `None` when the
    /// property declared no `default`; otherwise a [`FieldDefault`] whose `applied` is set only for
    /// a representable scalar on a plain optional field. A non-representable default emits `W005`.
    pub(super) fn field_default(
        &mut self,
        child: &SchemaOr,
        ty: Ty,
        required: bool,
    ) -> Option<FieldDefault> {
        let SchemaOr::Schema(schema) = child else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let classified = classify_default(raw);
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        let provenance = Provenance::new(schema.provenance.pointer.push("default"), Some(raw.span));
        match representable_default(&classified, kind) {
            Some(value) => {
                let display = default_display(&value);
                // A serde default only fires for an absent field on deserialization, so it is wired
                // only for a plain optional (non-required, non-nullable) scalar. A required field is
                // always present, and a nullable field already carries `Option`; both are documented
                // in rustdoc instead of silently ignored.
                let applied = (!required && !ty.nullable).then_some(value);
                Some(FieldDefault {
                    doc_note: format!("Default: `{display}`."),
                    applied,
                    provenance,
                    also_written: Vec::new(),
                })
            }
            None => {
                Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                    .message(
                        "schema `default` is not a scalar matching the field type; it is \
                         documented in rustdoc but not applied as a deserialization default",
                    )
                    .remedy(
                        "use a scalar default matching the field's own type, or set the value \
                         explicitly at each call site",
                    )
                    .emit(self.diags);
                Some(FieldDefault {
                    doc_note: format!("Default (not applied): `{}`.", raw_display(raw)),
                    applied: None,
                    provenance,
                    also_written: Vec::new(),
                })
            }
        }
    }

    /// Render the rustdoc `Default:` note for a parameter's schema `default`, if it declared one.
    /// Parameter defaults are documented but never serde-wired.
    pub(super) fn param_default_display(
        &self,
        schema: Option<&RefOr<Schema>>,
        ty: Ty,
    ) -> Option<String> {
        let RefOr::Item(schema) = schema? else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        Some(default_display_for(raw, kind))
    }

    /// A `default` in a structural position with no field/parameter/type home of its own —
    /// array `items`, tuple `prefixItems`, `additionalProperties` value, or a request/response body
    /// root — cannot be applied or documented against a named item, so it is reported as `W005`
    /// rather than dropped silently.
    pub(super) fn warn_structural_default_or(&mut self, schema: &SchemaOr, position: &str) {
        if let SchemaOr::Schema(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    pub(super) fn warn_structural_default_ref(&mut self, schema: &RefOr<Schema>, position: &str) {
        if let RefOr::Item(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    fn warn_structural_default(&mut self, schema: &Schema, position: &str) {
        if schema.default.is_some() {
            Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                .message(format!(
                    "schema `default` on {position} has no field to carry it and is not applied"
                ))
                .remedy("move the default onto a named property, or set the value explicitly")
                .emit(self.diags);
        }
    }
}

/// Re-type every applied field `default` against the type its field ends lowering with (#404).
///
/// [`LowerCtx::field_default`] decides a default against the type the declaring property lowers
/// to, but an intersection — `allOf` members repeating the property, or a `$ref` whose sibling
/// `properties` repeat it — then narrows that type: a `string` met with `enum: [a, b]` is the enum,
/// a `number` met with `integer` is the integer. A default the narrowed type still admits becomes a
/// value of it (the enum variant, the integer); one it does not admit is no value of the field, so
/// it is documented as not applied and reported (`W005`) at the `default` that wrote it, naming
/// the type whose field drops it. Running once over the finished graph reaches every meet, and
/// only the types that are emitted: a meet's discarded intermediates are gone or elided by now.
pub(super) fn retype_field_defaults(
    graph: &mut TypeGraph,
    meet_locations: &HashMap<TypeId, Provenance>,
    diags: &mut Diagnostics,
) {
    let mut retyped: Vec<(TypeId, usize, Option<DefaultValue>)> = Vec::new();
    for (id, def) in graph.emitted() {
        let TypeKind::Struct(object) = &def.kind else {
            continue;
        };
        for (index, field) in object.fields.iter().enumerate() {
            let Some(applied) = field.default.as_ref().and_then(|d| d.applied.as_ref()) else {
                continue;
            };
            let kind = graph.get(field.ty.id).map(|target| &target.kind);
            let value = representable_default(&reclassify_default(applied), kind);
            if value.as_ref() != Some(applied) {
                retyped.push((id, index, value));
            }
        }
    }
    for (id, index, value) in retyped {
        let Some(def) = graph.get_mut(id) else {
            continue;
        };
        let located = meet_locations
            .get(&id)
            .unwrap_or(&def.provenance)
            .pointer
            .clone();
        let TypeKind::Struct(object) = &mut def.kind else {
            continue;
        };
        let field = &mut object.fields[index];
        let Some(default) = field.default.as_mut() else {
            continue;
        };
        if value.is_none() {
            let written = default
                .applied
                .as_ref()
                .map(written_default_display)
                .unwrap_or_default();
            // Each `default` that wrote this value is dropped with it, so each is reported (#543).
            for at in std::iter::once(&default.provenance).chain(&default.also_written) {
                Diagnostic::warning(Code::SchemaDefaultNotApplied, at.clone())
                    .message(format!(
                        "schema `default` `{written}` of property `{}` is not a value of the type \
                         an intersection narrows the property to in `{}`; it is documented in \
                         rustdoc there but not applied as a deserialization default",
                        field.name.wire, located
                    ))
                    .remedy(
                        "use a default every intersected schema of the property admits, or set \
                         the value explicitly at each call site",
                    )
                    .emit(diags);
            }
            default.doc_note = format!("Default (not applied): `{written}`.");
        }
        default.applied = value;
    }
}

/// Recover the JSON value a representable default was decided from, so it can be decided again
/// against another type. An integral float is classified as the integer JSON Schema says it is: a
/// `number` field's `3` is carried as `3.0`, and the `integer` it narrows to admits it.
pub(super) fn reclassify_default(value: &DefaultValue) -> RawDefault {
    match value {
        DefaultValue::Bool(value) => RawDefault::Bool(*value),
        DefaultValue::Int(value) => RawDefault::Int(*value),
        DefaultValue::Float(value)
            if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value < i64::MAX as f64 =>
        {
            RawDefault::Int(*value as i64)
        }
        DefaultValue::Float(value) => RawDefault::Float(*value),
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => {
            RawDefault::Str(value.clone())
        }
    }
}

/// Render a representable default as [`raw_display`] renders the JSON it came from.
fn written_default_display(value: &DefaultValue) -> String {
    match value {
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => format!("{value:?}"),
        other => default_display(other),
    }
}

/// A `default` value classified into the scalar kinds that can back a Rust literal, or `Other` for
/// anything (object/array/null) that cannot.
#[derive(PartialEq)]
pub(super) enum RawDefault {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Other,
}

fn classify_default(value: &SpannedValue) -> RawDefault {
    match &value.node {
        Node::Bool(value) => RawDefault::Bool(*value),
        Node::Number(Number::Int(value)) => RawDefault::Int(*value),
        Node::Number(Number::UInt(value)) => {
            i64::try_from(*value).map_or(RawDefault::Float(*value as f64), RawDefault::Int)
        }
        Node::Number(Number::Float(value)) => RawDefault::Float(*value),
        Node::String(value) => RawDefault::Str(value.clone()),
        Node::Null | Node::Array(_) | Node::Object(_) => RawDefault::Other,
    }
}

/// Decide whether a classified `default` is representable against the field's lowered type: a
/// `Primitive` of the matching scalar kind, or a `ScalarEnum` value that is one of its variants.
fn representable_default(raw: &RawDefault, kind: Option<&TypeKind>) -> Option<DefaultValue> {
    let kind = kind?;
    match (raw, kind) {
        (RawDefault::Bool(value), TypeKind::Primitive(Prim::Bool)) => {
            Some(DefaultValue::Bool(*value))
        }
        // Width-check the literal so an out-of-range `int32` default is treated as
        // non-representable (→ W005, rustdoc-only) rather than rendered into code that fails to
        // compile. `i64` fields always fit.
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I32))
            if i32::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I64)) => Some(DefaultValue::Int(*value)),
        (RawDefault::Int(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value as f64))
        }
        (RawDefault::Float(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value))
        }
        (RawDefault::Str(value), TypeKind::Primitive(Prim::String)) => {
            Some(DefaultValue::Str(value.clone()))
        }
        (RawDefault::Str(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::String
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::String(v) if v == value)) =>
        {
            Some(DefaultValue::EnumVariant(value.clone()))
        }
        (RawDefault::Int(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Int
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Int(v) if v == value)) =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Bool(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Bool
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Bool(v) if v == value)) =>
        {
            Some(DefaultValue::Bool(*value))
        }
        // A property whose type is a cycle-closing `$ref` to a component still being lowered sees
        // its placeholder here. No literal can be proved to fit an unknown body, so the default is
        // not representable: it is documented and reported (`W005`) rather than wired. It is also
        // the answer the filled body would get — a cycle closes only through a schema that holds a
        // reference (an object, array, tuple, or union), and none of those takes a literal here.
        (_, TypeKind::Reserved) => None,
        _ => None,
    }
}

/// Render any `default` for a rustdoc note — nicely when it is representable against `kind`, else
/// as compact JSON. Used by the document-only positions (parameters, component roots) that never
/// serde-wire a default but must still surface it.
pub(super) fn default_display_for(raw: &SpannedValue, kind: Option<&TypeKind>) -> String {
    match representable_default(&classify_default(raw), kind) {
        Some(value) => default_display(&value),
        None => raw_display(raw),
    }
}

/// Render a representable default for its rustdoc `Default:` note.
pub(super) fn default_display(value: &DefaultValue) -> String {
    match value {
        DefaultValue::Bool(value) => value.to_string(),
        DefaultValue::Int(value) => value.to_string(),
        DefaultValue::Float(value) => value.to_string(),
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => value.clone(),
    }
}

/// Render an arbitrary default value as compact JSON-ish text for the rustdoc note of a
/// non-representable (`W005`) default.
fn raw_display(value: &SpannedValue) -> String {
    match &value.node {
        Node::Null => "null".to_owned(),
        Node::Bool(value) => value.to_string(),
        Node::Number(Number::Int(value)) => value.to_string(),
        Node::Number(Number::UInt(value)) => value.to_string(),
        Node::Number(Number::Float(value)) => value.to_string(),
        Node::String(value) => format!("{value:?}"),
        Node::Array(items) => {
            let items = items.iter().map(raw_display).collect::<Vec<_>>().join(", ");
            format!("[{items}]")
        }
        Node::Object(map) => {
            let entries = map
                .iter()
                .map(|(key, value)| format!("{:?}: {}", key.name, raw_display(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{entries}}}")
        }
    }
}
