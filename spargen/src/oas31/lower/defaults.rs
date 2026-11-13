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
    /// a representable scalar on a plain optional field: `decodes_required` is whether the field
    /// decodes as required, which a `required` `readOnly` or `writeOnly` one does not. A
    /// non-representable default emits `W005`.
    pub(super) fn field_default(
        &mut self,
        child: &SchemaOr,
        ty: Ty,
        decodes_required: bool,
    ) -> Option<FieldDefault> {
        let SchemaOr::Schema(schema) = child else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let classified = classify_default(raw);
        let kind = self.kind_of(ty.id);
        let provenance = Provenance::new(schema.provenance.pointer.push("default"), Some(raw.span));
        match representable_default(&classified, kind) {
            Some(value) => {
                let display = default_display(&value);
                // A serde default only fires for an absent field on deserialization, so it is wired
                // only for a plain optional (decoded as not required, non-nullable) scalar. A field
                // decoded as required is always present, and a nullable field already carries
                // `Option`; both are documented in rustdoc instead of silently ignored.
                let applied = (!decodes_required && !ty.nullable).then_some(value);
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
        let kind = self.kind_of(ty.id);
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
/// against another type. A float is classified by [`classify_float`]: a `number` field's `3` is
/// carried as `3.0`, and the `integer` it narrows to admits it.
pub(super) fn reclassify_default(value: &DefaultValue) -> RawDefault {
    match value {
        DefaultValue::Bool(value) => RawDefault::Bool(*value),
        DefaultValue::Int(value) => RawDefault::Int(*value),
        DefaultValue::Float(value) => classify_float(*value),
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
        Node::Number(Number::Float(value)) => classify_float(*value),
        Node::String(value) => RawDefault::Str(value.clone()),
        Node::Null | Node::Array(_) | Node::Object(_) => RawDefault::Other,
    }
}

/// Classify a float as JSON Schema compares numbers, by value: an integral float of magnitude
/// below 2^53 is that integer (`3.0` is `3`), so an `integer` field admits it under the same width
/// check as an integer literal. Anything else — a fraction, or a magnitude of 2^53 or more — stays a
/// float. The rule is [`exact_integer`]'s, which an `integer` schema's `enum`/`const` members are
/// read by too.
fn classify_float(value: f64) -> RawDefault {
    exact_integer(value).map_or(RawDefault::Float(value), RawDefault::Int)
}

/// The integer a parsed float names exactly, as JSON Schema compares numbers by value: `Some` for
/// an integral float of magnitude below 2^53 (`3.0` is `3`), `None` for a fraction or a magnitude
/// of 2^53 or more.
pub(super) fn exact_integer(value: f64) -> Option<i64> {
    // At 2^53 and above, consecutive `f64` values are 2 or more apart, so every one of them is
    // integral: `12345678901234567.5` parses to `12345678901234568.0`, and its fraction is gone
    // before this sees it. Below 2^53 every integer is exact, so an integral float there names the
    // integer its literal wrote, to the precision every float is parsed at.
    const EXACT_INTEGER_BOUND: f64 = 9_007_199_254_740_992.0; // 2^53
    (value.fract() == 0.0 && value.abs() < EXACT_INTEGER_BOUND).then_some(value as i64)
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
        // The unsigned widths take the same check, so a negative default, or one past the width,
        // is reported rather than rendered. A literal above `i64::MAX` never classifies as
        // `RawDefault::Int`, so a `uint64` default up there is reported too.
        (RawDefault::Int(value), TypeKind::Primitive(Prim::U8)) if u8::try_from(*value).is_ok() => {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::U16))
            if u16::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::U32))
            if u32::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::U64))
            if u64::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
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
fn default_display(value: &DefaultValue) -> String {
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

#[cfg(test)]
mod tests {
    //! The arms of [`representable_default`] and [`classify_default`] pinned directly: a scalar
    //! `enum` of integers or booleans, and an unsigned literal on either side of `i64::MAX`.

    use super::{classify_default, representable_default, RawDefault};
    use crate::diag::{Diagnostics, FileId};
    use crate::ir::{DefaultValue, Openness, ScalarEnum, ScalarRepr, ScalarValue, TypeKind};
    use crate::source::{Node, Number, SpannedValue};

    fn scalar_enum(repr: ScalarRepr, variants: Vec<ScalarValue>) -> TypeKind {
        TypeKind::Enum(ScalarEnum {
            repr,
            variants,
            width: match repr {
                ScalarRepr::String => crate::ir::Prim::String,
                ScalarRepr::Int => crate::ir::Prim::I64,
                ScalarRepr::Bool => crate::ir::Prim::Bool,
            },
            openness: Openness::Closed,
        })
    }

    fn parsed(yaml: &str) -> SpannedValue {
        let mut diags = Diagnostics::default();
        crate::source::parse_yaml(FileId(0), yaml, &mut diags)
            .unwrap_or_else(|_| panic!("probe does not parse: {yaml}"))
    }

    #[test]
    fn an_integer_enum_takes_exactly_its_member_values() {
        let kind = scalar_enum(
            ScalarRepr::Int,
            vec![ScalarValue::Int(1), ScalarValue::Int(2)],
        );
        assert_eq!(
            representable_default(&RawDefault::Int(2), Some(&kind)),
            Some(DefaultValue::Int(2))
        );
        assert_eq!(
            representable_default(&RawDefault::Int(3), Some(&kind)),
            None
        );
        // The integer arm is held to an integer set: a string set spelling the same digits is no
        // home for an integer literal, and an integer set none for a boolean.
        let digits = scalar_enum(
            ScalarRepr::String,
            vec![ScalarValue::String("2".to_owned())],
        );
        assert_eq!(
            representable_default(&RawDefault::Int(2), Some(&digits)),
            None
        );
        assert_eq!(
            representable_default(&RawDefault::Bool(true), Some(&kind)),
            None
        );
    }

    #[test]
    fn a_boolean_enum_takes_exactly_its_member_values() {
        let kind = scalar_enum(ScalarRepr::Bool, vec![ScalarValue::Bool(true)]);
        assert_eq!(
            representable_default(&RawDefault::Bool(true), Some(&kind)),
            Some(DefaultValue::Bool(true))
        );
        assert_eq!(
            representable_default(&RawDefault::Bool(false), Some(&kind)),
            None
        );
        assert_eq!(
            representable_default(&RawDefault::Int(1), Some(&kind)),
            None
        );
    }

    /// `spec` lowered through the whole frontend from a temporary root document, with every
    /// diagnostic it reported.
    fn lowered(spec: &str) -> (crate::ir::Api, Vec<crate::Diagnostic>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, spec).unwrap();
        let spec = crate::Spec::new(camino::Utf8PathBuf::from_path_buf(path).unwrap());
        let mut diags = Diagnostics::default();
        let lowered = crate::pipeline::lower_frontend(&spec, &mut diags);
        let (api, _) = lowered.unwrap_or_else(|()| panic!("the spec lowers: {:?}", diags.items()));
        (api, diags.items().to_vec())
    }

    /// Each property of the emitted struct `name`, as `(wire name, doc note, applied value)`, the
    /// note and value `None` for a property that declared no `default`.
    fn field_defaults(
        api: &crate::ir::Api,
        name: &str,
    ) -> Vec<(String, Option<String>, Option<DefaultValue>)> {
        let (_, def) = api
            .types
            .emitted()
            .find(|(_, def)| def.name_hint == name)
            .unwrap_or_else(|| panic!("no emitted type `{name}`"));
        let TypeKind::Struct(object) = &def.kind else {
            panic!("`{name}` is no struct");
        };
        object
            .fields
            .iter()
            .map(|field| {
                let default = field.default.as_ref();
                (
                    field.name.wire.clone(),
                    default.map(|default| default.doc_note.clone()),
                    default.and_then(|default| default.applied.clone()),
                )
            })
            .collect()
    }

    /// The messages of the `W005`s in `diagnostics`.
    fn w005(diagnostics: &[crate::Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == crate::diag::Code::SchemaDefaultNotApplied)
            .map(|diagnostic| diagnostic.message.as_str())
            .collect()
    }

    const DEFAULTS_SPEC: &str = r#"
openapi: 3.1.0
info: { title: t, version: '1' }
paths:
  /p:
    post:
      operationId: postP
      parameters:
        - { name: limit, in: query, schema: { type: integer, format: int32, default: 10 } }
        - { name: tags, in: query, schema: { type: array, items: { type: string }, default: [a, b] } }
        - { name: plain, in: query, schema: { type: string } }
      requestBody:
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } }, default: { a: x } }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
        '201':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Narrow' }
components:
  schemas:
    Pet:
      type: object
      required: [req, read_only, write_only]
      properties:
        i32: { type: integer, format: int32, default: 10 }
        wide: { type: integer, format: int32, default: 3000000000 }
        i64: { type: integer, default: 4 }
        int_float: { type: integer, default: 3.0 }
        fraction: { type: integer, default: 2.5 }
        huge: { type: integer, default: 9007199254740992.0 }
        float_from_int: { type: number, default: 2 }
        float: { type: number, default: 1.5 }
        flag: { type: boolean, default: true }
        name: { type: string, default: hi }
        color: { type: string, enum: [red, blue], default: blue }
        off_color: { type: string, enum: [red], default: green }
        req: { type: string, default: r }
        read_only: { type: string, readOnly: true, default: ro }
        write_only: { type: string, writeOnly: true, default: wo }
        maybe: { type: [string, 'null'], default: m }
        obj: { type: object, default: { k: [1, null, 2.5, true, -3, 18446744073709551615] } }
        list: { type: array, items: { type: string, default: x } }
        plain: { type: string }
    Narrow:
      allOf:
        - type: object
          properties:
            kept: { type: string, default: a }
            dropped: { type: string, default: c }
            whole: { type: number, default: 3 }
            relaxed: { type: string, readOnly: true, default: r }
        - type: object
          required: [relaxed]
          properties:
            kept: { enum: [a, b] }
            dropped: { enum: [a, b] }
            whole: { type: integer }
"#;

    /// A property's `default` lowered end to end: applied only for a representable scalar on a
    /// plain optional field, documented either way, and reported as `W005` where it is not
    /// applied; a float default that names an integer exactly read as that integer, below 2^53
    /// only.
    #[test]
    fn a_property_default_is_applied_only_where_representable_and_optional() {
        let (api, _) = lowered(DEFAULTS_SPEC);
        let applied = |note: &str, value: DefaultValue| (Some(note.to_owned()), Some(value));
        let documented = |note: &str| (Some(note.to_owned()), None);
        let expected = [
            ("i32", applied("Default: `10`.", DefaultValue::Int(10))),
            ("wide", documented("Default (not applied): `3000000000`.")),
            ("i64", applied("Default: `4`.", DefaultValue::Int(4))),
            ("int_float", applied("Default: `3`.", DefaultValue::Int(3))),
            ("fraction", documented("Default (not applied): `2.5`.")),
            (
                "huge",
                documented("Default (not applied): `9007199254740992`."),
            ),
            (
                "float_from_int",
                applied("Default: `2`.", DefaultValue::Float(2.0)),
            ),
            (
                "float",
                applied("Default: `1.5`.", DefaultValue::Float(1.5)),
            ),
            (
                "flag",
                applied("Default: `true`.", DefaultValue::Bool(true)),
            ),
            (
                "name",
                applied("Default: `hi`.", DefaultValue::Str("hi".to_owned())),
            ),
            (
                "color",
                applied(
                    "Default: `blue`.",
                    DefaultValue::EnumVariant("blue".to_owned()),
                ),
            ),
            (
                "off_color",
                documented("Default (not applied): `\"green\"`."),
            ),
            ("req", documented("Default: `r`.")),
            // `required` relaxed by `readOnly`/`writeOnly`: the member decodes as optional.
            (
                "read_only",
                applied("Default: `ro`.", DefaultValue::Str("ro".to_owned())),
            ),
            (
                "write_only",
                applied("Default: `wo`.", DefaultValue::Str("wo".to_owned())),
            ),
            ("maybe", documented("Default: `m`.")),
            (
                "obj",
                documented(
                    "Default (not applied): `{\"k\": [1, null, 2.5, true, -3, \
                     18446744073709551615]}`.",
                ),
            ),
            ("list", (None, None)),
            ("plain", (None, None)),
        ];
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(wire, (note, value))| (wire.to_owned(), note, value))
            .collect();
        assert_eq!(field_defaults(&api, "Pet"), expected);
    }

    /// Every `default` lowering cannot apply is reported as `W005`: one a field's type does not
    /// admit, one in a structural position (array `items`, a request body root), and one an
    /// intersection's narrowing drops, named with the value it wrote.
    #[test]
    fn every_default_not_applied_is_reported() {
        let (_, diagnostics) = lowered(DEFAULTS_SPEC);
        let reported = w005(&diagnostics);
        let not_a_field_scalar = reported
            .iter()
            .filter(|message| message.starts_with("schema `default` is not a scalar matching"))
            .count();
        // `wide`, `fraction`, `huge`, `off_color` and `obj`.
        assert_eq!(not_a_field_scalar, 5, "{reported:#?}");
        for position in ["array `items`", "a request body schema"] {
            assert!(
                reported.contains(
                    &format!(
                        "schema `default` on {position} has no field to carry it and is not \
                         applied"
                    )
                    .as_str()
                ),
                "no structural W005 on {position}: {reported:#?}"
            );
        }
        assert!(
            reported.iter().any(
                |message| message.starts_with("schema `default` `\"c\"` of property `dropped`")
            ),
            "no W005 for the narrowed-away default: {reported:#?}"
        );
    }

    /// An intersection that narrows a property's type re-types its applied default: a member of
    /// the narrowed enum becomes its variant, an integral `number` default becomes the integer,
    /// and a value the narrowed type does not admit is documented as not applied.
    #[test]
    fn a_narrowed_property_retypes_its_default() {
        let (api, _) = lowered(DEFAULTS_SPEC);
        assert_eq!(
            field_defaults(&api, "Narrow"),
            [
                (
                    "kept".to_owned(),
                    Some("Default: `a`.".to_owned()),
                    Some(DefaultValue::EnumVariant("a".to_owned())),
                ),
                (
                    "dropped".to_owned(),
                    Some("Default (not applied): `\"c\"`.".to_owned()),
                    None,
                ),
                (
                    "whole".to_owned(),
                    Some("Default: `3`.".to_owned()),
                    Some(DefaultValue::Int(3)),
                ),
                // Required by the other member, but `readOnly`: it decodes as optional, so the
                // requirement leaves its default applied.
                (
                    "relaxed".to_owned(),
                    Some("Default: `r`.".to_owned()),
                    Some(DefaultValue::Str("r".to_owned())),
                ),
            ]
        );
    }

    /// A parameter's `default` is documented, as its value when its type admits it and as JSON
    /// otherwise; a parameter that declares none has no note.
    #[test]
    fn a_parameter_default_is_documented() {
        let (api, _) = lowered(DEFAULTS_SPEC);
        let notes: Vec<(&str, Option<&str>)> = api.operations[0]
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.default_display.as_deref()))
            .collect();
        assert_eq!(
            notes,
            [
                ("limit", Some("10")),
                ("tags", Some("[\"a\", \"b\"]")),
                ("plain", None),
            ]
        );
    }

    #[test]
    fn an_unsigned_literal_is_an_integer_only_within_i64() {
        // The parser yields `UInt` only above `i64::MAX`; that value is no `i64`, so it is carried
        // as the float JSON's number is, never wrapped into a negative integer.
        let above = parsed("18446744073709551615");
        assert_eq!(above.node, Node::Number(Number::UInt(u64::MAX)));
        assert!(
            matches!(classify_default(&above), RawDefault::Float(value) if value == u64::MAX as f64)
        );
        // An unsigned value that fits is the integer it is.
        let within = SpannedValue::new(Node::Number(Number::UInt(5)), above.span);
        assert!(matches!(classify_default(&within), RawDefault::Int(5)));
    }
}
