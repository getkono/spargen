//! Object schemas: properties to fields, `required`, and `additionalProperties` /
//! `patternProperties`.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic};
use crate::ir::{AdditionalProps, Docs, Field, PropertyName, Ty, TypeKind, XmlField};
use crate::oas31::{Schema, SchemaOr};

use super::combine::undeclared_required;
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower an object schema's `properties`/`required`/`additionalProperties` into the pieces of a
    /// [`Struct`] *without* inserting the struct itself. Shared by [`Self::lower_object`] and the
    /// `allOf` merge, which collects field/additional pieces from several members before inserting a
    /// single merged struct as the final graph insert (the `ensure_component` last-insert invariant).
    ///
    /// [`Struct`]: crate::ir::Struct
    pub(super) fn object_body(
        &mut self,
        schema: &Schema,
        hint: &str,
    ) -> Option<(Vec<Field>, AdditionalProps)> {
        let required = schema.required.iter().cloned().collect::<HashSet<_>>();
        let mut fields = Vec::new();
        for (name, child) in &schema.properties {
            let ty = self.lower_schema_or(child, &format!("{hint}{name}"))?;
            let is_required = required.contains(name);
            let default = self.field_default(child, ty, is_required);
            let xml = self.field_xml(child);
            let (deprecated, read_only, write_only) = field_flags(child);
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: is_required,
                deprecated,
                read_only,
                write_only,
                default,
                xml,
                undeclared: false,
            });
        }
        let additional = if schema.pattern_properties.is_empty() {
            match &schema.additional_properties {
                Some(schema) => match schema.as_ref() {
                    SchemaOr::Bool(false) => AdditionalProps::Deny,
                    SchemaOr::Bool(true) => AdditionalProps::Allow,
                    schema => {
                        let mut ty = self.lower_schema_or(schema, &format!("{hint}Additional"))?;
                        self.warn_structural_default_or(schema, "an `additionalProperties` value");
                        // A map value lives behind the map's own indirection; a cycle-closing ref
                        // here needs no `Box`.
                        ty.boxed = false;
                        AdditionalProps::Typed(Box::new(ty))
                    }
                },
                None => AdditionalProps::Allow,
            }
        } else {
            self.lower_pattern_additional(schema, hint)?
        };
        // A `required` name no `properties` entry declares is still required: the instance must
        // carry that key. Consuming `required` only as a per-property flag dropped such a name,
        // so the generated type accepted and could emit an object without it (#140). It becomes
        // a required field typed by what the object says of an undeclared key: the
        // `additionalProperties` schema when there is one, and nothing at all otherwise.
        // `patternProperties` cannot be matched against the name at generation time, so its
        // value type would be a guess. `additionalProperties: false` is read the way the rest of
        // lowering reads it — it closes the object to the fields the generated type declares, as
        // `deny_unknown_fields` — and this field is one of them; reading it strictly instead
        // (every undeclared key forbidden, so the object is uninhabited) would reject the common
        // `allOf: [{$ref: Base}, {additionalProperties: false, required: [id]}]`, which that same
        // reading generates when `Base` declares `id`.
        for name in undeclared_required(schema) {
            let ty = match (&additional, schema.additional_properties.as_deref()) {
                (AdditionalProps::Typed(ty), Some(SchemaOr::Schema(_))) => {
                    // The map value dropped its `Box` because the map already provides the
                    // indirection a cycle-closing reference needs. A plain field has none, so it
                    // is boxed again exactly when the value closes a cycle: its target is still
                    // being lowered, which is what makes the `ensure_*` paths box it.
                    let mut ty = **ty;
                    ty.boxed = self.is_in_progress_root(ty.id);
                    ty
                }
                _ => self.insert_type(
                    &format!("{hint}{name}"),
                    TypeKind::Any,
                    Docs::default(),
                    None,
                ),
            };
            fields.push(Field {
                name: PropertyName { wire: name },
                ty,
                required: true,
                deprecated: false,
                read_only: false,
                write_only: false,
                default: None,
                xml: XmlField::default(),
                undeclared: true,
            });
        }
        Some((fields, additional))
    }

    /// Lower the overflow policy for an object that declares `patternProperties`. The generated
    /// struct captures every non-declared property into a single `#[serde(flatten)]` typed map, so
    /// every `patternProperties` value schema — together with a typed `additionalProperties` value,
    /// if any — must lower to the *same emitted Rust type*; otherwise a single map cannot type them.
    ///
    /// Homogeneity is decided by [`Self::same_map_value_type`], a bounded structural equivalence:
    /// same `TypeId` (a shared `$ref`, or the single-entry case) is homogeneous, and distinct inline
    /// leaf shapes (primitives, `Bytes`, `Any`, or arrays thereof) that emit the identical Rust type
    /// collapse to one map — so `{type:string}` under two patterns yields one `BTreeMap<String,
    /// String>`. Distinct inline composites (`Struct`/`Enum`/`Tuple`) stay heterogeneous and are
    /// rejected (`E005`), since two different object shapes cannot share one map value type. The
    /// first collected value type is used as the map's value type. Deterministic (graph lookups by
    /// `TypeId`, source-order collection) and bounded (recurses only through `Array` elements).
    fn lower_pattern_additional(&mut self, schema: &Schema, hint: &str) -> Option<AdditionalProps> {
        // `additionalProperties: false` denies unknown keys, but the flatten map must capture the
        // pattern-matched keys (which are themselves "unknown" to the named fields). Serde cannot do
        // both, so this combination has no faithful representation.
        if matches!(
            schema.additional_properties.as_deref(),
            Some(SchemaOr::Bool(false))
        ) {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties` combined with `additionalProperties: false` cannot be \
                     represented: a flatten map captures pattern values but cannot also deny other \
                     unknown keys",
                )
                .remedy(
                    "drop `additionalProperties: false`, or omit this API segment with \
                     spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        // Collect the value types in deterministic source order: patternProperties entries first
        // (IndexMap preserves source order), then a typed `additionalProperties` value if present.
        let mut value_types: Vec<Ty> = Vec::new();
        for (_pattern, child) in &schema.pattern_properties {
            let ty = self.lower_schema_or(child, &format!("{hint}Value"))?;
            self.warn_structural_default_or(child, "a `patternProperties` value");
            value_types.push(ty);
        }
        if let Some(additional) = schema.additional_properties.as_deref() {
            // `true`/absent leave unknown non-pattern keys unconstrained; the typed map still stands
            // in for the overflow. Only a schema value adds another type that must agree.
            if !matches!(additional, SchemaOr::Bool(_)) {
                let ty = self.lower_schema_or(additional, &format!("{hint}Additional"))?;
                self.warn_structural_default_or(additional, "an `additionalProperties` value");
                value_types.push(ty);
            }
        }

        let first = value_types[0];
        if value_types
            .iter()
            .any(|ty| !self.same_map_value_type(first, *ty))
        {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties`/`additionalProperties` value schemas lower to different \
                     types; a single typed overflow map cannot represent them all",
                )
                .remedy(
                    "make every pattern/additional value the same type (e.g. a shared `$ref` or the \
                     same primitive), or omit this API segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        let mut ty = first;
        // A map value lives behind the map's own indirection; a cycle-closing ref needs no `Box`.
        ty.boxed = false;
        Some(AdditionalProps::Typed(Box::new(ty)))
    }
}

/// The `deprecated`/`readOnly`/`writeOnly` annotations of one *property* subschema.
///
/// These are per-property annotations. Reading them from the enclosing object would both ignore a
/// property's own `deprecated: true` and mark every field of a deprecated object as deprecated;
/// an object-level annotation belongs on the type, where it already is.
fn field_flags(child: &SchemaOr) -> (bool, bool, bool) {
    match child {
        // A boolean schema carries no annotations.
        SchemaOr::Bool(_) => (false, false, false),
        SchemaOr::Schema(schema) => (schema.deprecated, schema.read_only, schema.write_only),
    }
}
