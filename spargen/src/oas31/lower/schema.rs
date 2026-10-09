//! The schema-lowering entry points and dispatch: one Schema Object to one IR type, by its
//! `type`, `enum`, applicators and `$ref`, and the type-graph insertion every lowering ends in.

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{
    Docs, JsonCategory, Openness, Prim, ScalarEnum, ScalarRepr, ScalarValue, Struct, Ty, TypeDef,
    TypeKind,
};
use crate::oas31::{JsonType, RefOr, Schema, SchemaOr, ValidationKeywords};
use crate::source::{is_remote_ref, Node, Number, SpannedValue};

use super::combine::{schema_has_union, sole_union_member};
use super::meet::value_category;
use super::nullability::union_branch_admits_null;
use super::refiner::{implied_applicator_category, split_union_sibling, ImpliedCategory};
use super::shape::schema_has_shape_constraint;
use super::{LowerCtx, MetUnion, MAX_SCHEMA_DEPTH};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    pub(super) fn lower_schema_or(&mut self, schema: &SchemaOr, hint: &str) -> Option<Ty> {
        match schema {
            SchemaOr::Bool(true) => {
                Some(self.insert_type(hint, TypeKind::Any, Docs::default(), None))
            }
            SchemaOr::Bool(false) => {
                Some(self.insert_type(hint, TypeKind::Never, Docs::default(), None))
            }
            SchemaOr::Schema(schema) => self.lower_schema(schema, hint),
        }
    }

    /// Depth-guarded entry to schema lowering. Bounds the `$ref`/allOf/array/object recursion to
    /// [`MAX_SCHEMA_DEPTH`] so a pathologically deep composition rejects with `E014` instead of
    /// exhausting the stack; the counter is decremented on every exit so sibling members (breadth)
    /// never accumulate against the cap.
    pub(super) fn lower_schema(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if self.depth >= MAX_SCHEMA_DEPTH {
            return self.reject_too_deep(&schema.provenance);
        }
        self.depth += 1;
        let result = self.lower_schema_inner(schema, hint);
        self.depth -= 1;
        result
    }

    /// Lower the body of a type whose root id `ensure_component`, `ensure_remote` or
    /// `ensure_resolved` has just reserved, with [`Self::resolved_member_stack`] empty for the
    /// duration and restored after.
    ///
    /// The stack answers "is this expansion inside itself", and a reservation starts a new type:
    /// a member target flattened by an enclosing expansion and met again inside this body is a
    /// recursive *field* of the new type, which the reservation boxes, not a loop of the enclosing
    /// expansion. Re-entering the reserved type itself is refused by its `*_in_progress` entry, so
    /// every loop that crosses this boundary is still caught, as the root document catches it.
    pub(super) fn lower_reserved_body(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let enclosing = std::mem::take(&mut self.resolved_member_stack);
        let lowered = self.lower_schema(schema, hint);
        self.resolved_member_stack = enclosing;
        lowered
    }

    /// The `E014` rejection [`Self::lower_schema`] reports at [`MAX_SCHEMA_DEPTH`], shared with the
    /// one other recursion that does not pass through it: [`Self::gather_ref_target`]'s expansion
    /// of a bundle-`$ref` `allOf` member's target.
    pub(super) fn reject_too_deep<T>(&mut self, provenance: &Provenance) -> Option<T> {
        Diagnostic::error(Code::SchemaNestingTooDeep, provenance.clone())
            .message(format!(
                "schema nesting exceeds the maximum lowering depth of {MAX_SCHEMA_DEPTH} \
                 (a very long `$ref` chain or a pathologically nested schema)"
            ))
            .remedy(
                "flatten the offending schema chain, or omit this API segment with \
                 spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    fn lower_schema_inner(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        // Before any arm can return: a discriminator beside no union is dropped by every one of
        // them (a `$ref` with no other sibling, `allOf`, a type array, a plain object).
        self.diagnose_standalone_discriminator(schema);
        if let Some(value) = schema.boolean {
            let kind = if value {
                TypeKind::Any
            } else {
                TypeKind::Never
            };
            return Some(self.insert_schema_type(schema, hint, kind));
        }

        if let Some(reference) = &schema.reference {
            let referenced = self.ensure_reference(reference, &schema.provenance, hint)?;

            // In JSON Schema 2020-12 `$ref` is an applicator, not a replacement for the containing
            // schema. Intersect every shape-bearing sibling instead of silently discarding it.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                // No shape to compose, so a cycle-closing reference here is the ordinary recursive
                // schema: it boxes and generates. Only the intersection below needs a real target.
                return Some(referenced);
            }
            // Whether this `$ref` closes a reference cycle back through a schema whose lowering
            // encloses it. A target inside the cycle cannot be composed with: its definition
            // depends on the very result being computed, so `intersect_non_null`'s `(Any, _)` arm
            // would return the sibling and silently discard the target.
            //
            // It is asked of the DOCUMENT, for every spelling alike. A lowering-order test — "is the
            // target mid-flight" — gives two byte-identical documents opposite verdicts when only
            // the order of two map entries differs (decision 23), and so does a document test that
            // only one spelling can reach: the sub-file spelling used to fall back to
            // `is_in_progress_root`, so with siblings on one edge of a two-schema cycle the verdict
            // followed which end lowering happened to enter first. `ref_closes_a_cycle` walks
            // resolved identities across every file instead.
            //
            // `is_in_progress_root` stays as a backstop and adds no rejection of its own: a target
            // still being lowered is one whose lowering reached this site, which is a cycle the walk
            // finds. Kept so that a walk which ever missed one reports the recursion, rather than
            // leaving `intersect_types`' fail-closed arm to report it as a failed intersection.
            let back_edge = self.ref_closes_a_cycle(reference, &schema.provenance)
                || self.is_in_progress_root(referenced.id);
            if back_edge {
                // The siblings have nothing yet to intersect with. The `allOf` spelling of the same
                // conjunction has always rejected this rather than composing against a placeholder,
                // and the alternative here is not "compose anyway" but "discard the target", which
                // produces a type accepting documents the description forbids — a recursive `Node`
                // flattened to a one-off struct, or to the sibling's own scalar.
                // The wording is chosen by what the reference RESOLVES TO, not by how it was
                // spelled. It used to branch on whether the string began `#/components/schemas/`,
                // which is a fact about the author's typing: the explicit `./lib.yaml#/…` spelling
                // of a sub-file component was therefore described to the reader as *remote*, which
                // it is not, while the bare spelling of the same target in the same file was
                // described correctly. One reference, two spellings, two different accounts of one
                // fact — the mistake decision 23 removed from the verdict, left standing in the
                // explanation.
                //
                // `is_remote_ref` is the same predicate that routes the lowering a few lines above,
                // so the message and the code path now agree by construction. A genuinely remote
                // target still says so; every local target, however addressed, says the same thing.
                // The local noun is "schema", not "component": a local target need not be a
                // component at all (`./lib.yaml#/bag/Tree`, or `#/bag/Tree` inside a sub-file), and
                // a two-way predicate cannot tell that case apart, so the wording must hold for it.
                return self.reject_ref_sibling_cycle(
                    schema,
                    if is_remote_ref(reference) {
                        "this remote `$ref` closes a reference cycle back to the schema that \
                         encloses it, so its shape-bearing siblings would have to be intersected \
                         with a target whose own definition depends on the result"
                    } else {
                        "this `$ref` closes a reference cycle back to the schema that encloses it, \
                         so its shape-bearing siblings would have to be intersected with a target \
                         whose own definition depends on the result"
                    },
                );
            }
            // A sibling carrying only object or only array applicators names no `type`, and
            // `lower_schema` reaches its object and array arms through `type`, so it would lower to
            // `TypeKind::Any` — which intersects as identity, discarding the keywords with no
            // diagnostic (#140). The applicators establish the category they apply to, as an
            // untyped `properties` already does, and say nothing about `null` (the same reading
            // `lower_union_sibling` takes), so the target's nullability survives the intersection.
            //
            // Against a union target that reading would drop every branch of another category in
            // silence — `oneOf: [string, Obj]` with a `required` sibling would become `Obj` alone
            // and reject the strings the target accepts. There the applicators refine the branches
            // of their own category, as they do beside an inline union (#282).
            let category = implied_applicator_category(&sibling);
            if category.is_some() {
                if let TypeKind::Union(union) = &self.graph.get(referenced.id)?.kind {
                    let union = union.clone();
                    return self.refine_union_target(schema, hint, referenced, &union, &sibling);
                }
            }
            let mut inferred_category = None;
            match category {
                Some(ImpliedCategory::Only(category)) => {
                    sibling.types.types = vec![category, JsonType::Null];
                    inferred_category = Some(category);
                }
                Some(ImpliedCategory::Conflicting) => {
                    return self.reject_ref_sibling_category(
                        schema,
                        "this `$ref`'s untyped sibling keywords are both object keywords \
                         (`properties`, `patternProperties`, `required`, `additionalProperties`) \
                         and array keywords (`items`, `prefixItems`) with no `type` to choose \
                         between them, so no single Rust type represents what they constrain",
                    );
                }
                None => {}
            }
            // An untyped object target admits `null` without deciding it, as the same conjunct
            // written inline as an `allOf` member does (#541), so beside a sibling typed `object`
            // the sibling's own answer about `null` is the meet's. Read before the union-sibling
            // dispatch below, so a sibling carrying a `oneOf`/`anyOf` meets the same target. Only
            // there: what an untyped target means beside another category is not an object meet,
            // and keeps its verdict.
            let referenced = if schema.types.types.contains(&JsonType::Object)
                && matches!(
                    self.graph.get(referenced.id).map(|def| &def.kind),
                    Some(TypeKind::Struct(_))
                )
                && !self.ref_target_decides_null(reference, &schema.provenance)
            {
                Ty {
                    nullable: true,
                    ..referenced
                }
            } else {
                referenced
            };
            let has_union_sibling = !schema.one_of.is_empty() || !schema.any_of.is_empty();
            // Beside a union sibling and no `type`, the untyped target leaves `null` to the
            // union, as the same target written as an `allOf` member beside it does
            // ([`undecided_admits_null`], #586): where a branch admits `null` itself
            // ([`union_branch_admits_null`]), the target admits it, and the meet keeps the
            // union's answer.
            let referenced = if has_union_sibling
                && schema.types.types.is_empty()
                && !referenced.nullable
                && matches!(
                    self.graph.get(referenced.id).map(|def| &def.kind),
                    Some(TypeKind::Struct(_))
                )
                && !self.ref_target_decides_null(reference, &schema.provenance)
                && union_branch_admits_null(schema)
            {
                Ty {
                    nullable: true,
                    ..referenced
                }
            } else {
                referenced
            };
            if has_union_sibling {
                let (keywords, union) = split_union_sibling(&sibling);
                if schema_has_shape_constraint(&keywords) {
                    return self
                        .meet_ref_union_sibling(schema, hint, referenced, &keywords, &union);
                }
            }
            let enclosing_unmerged = std::mem::replace(
                &mut self.unmerged_union,
                has_union_sibling.then(|| schema.provenance.clone()),
            );
            let enclosing_meets_null =
                std::mem::replace(&mut self.unmerged_union_meets_null, referenced.nullable);
            let sibling_mark = self.graph_mark();
            let sibling =
                self.lower_ref_sibling(referenced, &sibling, &format!("{hint}Constraint"));
            self.unmerged_union = enclosing_unmerged;
            self.unmerged_union_meets_null = enclosing_meets_null;
            let untyped_beside_null_member =
                self.untyped_beside_null_member.take() == Some(schema.provenance.clone());
            let stated_nothing_took_null = self.take_stated_nothing_took_null(&schema.provenance);
            let sibling = sibling?;
            let mark = self.graph_mark();
            let Ok(intersection) =
                self.intersect_types(referenced, sibling, &format!("{hint}ReferenceIntersection"))
            else {
                // `$ref` is an applicator: the value must satisfy the target AND these siblings.
                // `intersect_types` fails for two distinct conditions — the intersection is
                // empty, so no value satisfies both, or it is inhabited but has no single Rust type
                // — and this one message covers both, so it must not claim the first. Either way
                // it is reported rather than dropped: dropping would silently delete a body,
                // parameter or property from the generated client.
                return self.reject_ref_sibling_intersection(schema);
            };
            // Only a `$ref` whose own sibling is a `oneOf`/`anyOf` is collapsed. A `$ref` to a union
            // beside a non-union sibling is an intersection this check was never meant for, and it
            // keeps the shape it has always generated.
            let intersection = if has_union_sibling {
                let intersection = self.clear_counted_null(
                    intersection,
                    referenced,
                    stated_nothing_took_null.as_ref(),
                    &format!("{hint}ReferenceIntersection"),
                );
                let (collapsed, untyped_check) = self.collapse_met_union(
                    schema,
                    intersection,
                    !schema.one_of.is_empty(),
                    &format!("{hint}ReferenceIntersection"),
                    MetUnion::RefSibling,
                );
                // Nothing meets the union after the collapse here.
                if untyped_check {
                    self.warn_untyped_met_variants(schema, collapsed, MetUnion::RefSibling);
                }
                // The meet gave the untyped member `null` exactly where it kept the `null`
                // member's, so `null` is in two branches or none.
                let mut collapsed = collapsed;
                if untyped_beside_null_member {
                    collapsed.nullable = false;
                }
                collapsed
            } else {
                intersection
            };
            let kind = self.graph.get(intersection.id)?.kind.clone();
            // The `null` the inferred category carries is there to leave the target's nullability
            // alone, not to satisfy the intersection on its own. Against a nullable target of
            // another category the two share only `null`, and typing that as the exact JSON null
            // would silently replace, say, a nullable string with `()`: the category contradiction
            // is the same empty intersection it is against the non-null target, and is reported
            // the same way. A target that is itself exactly `null` keeps its type, and so does a
            // nullable target OF the inferred category: there no category is contradicted, the
            // object meet is empty, and `null` satisfies both, as it does when the same keywords
            // are an untyped `allOf` member beside the target (#542).
            let target_kind = &self.graph.get(referenced.id)?.kind;
            let same_category = matches!(
                (inferred_category, value_category(target_kind)),
                (Some(JsonType::Object), Some(JsonCategory::Object))
                    | (Some(JsonType::Array), Some(JsonCategory::Array))
            );
            if inferred_category.is_some()
                && !same_category
                && matches!(kind, TypeKind::Null)
                && !matches!(target_kind, TypeKind::Null)
            {
                return self.reject_ref_sibling_category(
                    schema,
                    "this `$ref`'s untyped sibling keywords establish a category its target does \
                     not have, so the only value both accept is `null`; the intersection is empty \
                     but for the target's nullability",
                );
            }
            self.discard_meet_intermediates(mark, &kind);
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = intersection.nullable;
            ty.boxed = intersection.boxed;
            // The sibling's union was lowered with its merge held back, as the `allOf` spellings'
            // is, and the collapse re-emitted what the meet left of it (#561).
            if has_union_sibling {
                self.elide_unused_union_lowering(sibling_mark..mark);
            }
            return Some(ty);
        }

        if !schema.all_of.is_empty() {
            if schema_has_union(schema) {
                return self.lower_all_of_beside_union(schema, hint);
            }
            if let Some(index) = sole_union_member(schema) {
                return self.lower_all_of_with_union_member(schema, hint, index);
            }
            return self.lower_all_of(schema, hint);
        }

        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            return self.lower_union(schema, hint);
        }

        if let Some(enumeration) = &schema.enum_values {
            return self.lower_enum(enumeration, schema, hint);
        }
        if let Some(value) = &schema.const_value {
            return self.lower_enum(std::slice::from_ref(value), schema, hint);
        }

        let non_null_types: Vec<JsonType> = schema
            .types
            .types
            .iter()
            .copied()
            .filter(|ty| *ty != JsonType::Null)
            .collect();
        if non_null_types.len() > 1 {
            return self.lower_type_array(schema, hint, &non_null_types);
        }

        // A binary payload — `contentEncoding: base64` or `format: binary` (the OpenAPI file/upload
        // marker) — lowers to raw `bytes::Bytes` rather than a `String`, so a multipart file part
        // carries bytes and a byte body is not misdecoded as UTF-8. A `"null"` in the type array
        // (`type: [string, 'null']`) makes it nullable exactly as the `oneOf [.., null]` spelling
        // is, so both spellings reach the raw-body gates, and a JSON member becomes
        // `Option<bytes::Bytes>`, rather than the `null` being dropped here.
        if schema.content_encoding.as_deref() == Some("base64")
            || schema.format.as_deref() == Some("binary")
        {
            let mut ty = self.insert_schema_type(schema, hint, TypeKind::Bytes);
            ty.nullable = schema.types.types.contains(&JsonType::Null);
            return Some(ty);
        }

        let nullable = schema.types.types.contains(&JsonType::Null);
        let primary = schema
            .types
            .types
            .iter()
            .find(|ty| **ty != JsonType::Null)
            .copied();

        let mut ty = match primary {
            Some(JsonType::Boolean) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::Bool))
            }
            Some(JsonType::Integer) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("int32") => Prim::I32,
                    _ => Prim::I64,
                }),
            ),
            Some(JsonType::Number) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::F64))
            }
            Some(JsonType::String) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("uuid") => Prim::Uuid,
                    Some("date-time") => Prim::DateTime,
                    Some("date") => Prim::Date,
                    _ => Prim::String,
                }),
            ),
            Some(JsonType::Array) => {
                if !schema.prefix_items.is_empty() {
                    // `items` beside `prefixItems` is the 2020-12 rest-element schema. A Rust tuple
                    // is fixed-length, so a typed remainder is not representable — except
                    // `items: false`, which closes the array at the prefix and *is* a tuple.
                    if let Some(rest) = &schema.items {
                        if !matches!(rest.as_ref(), SchemaOr::Bool(false)) {
                            Diagnostic::error(
                                Code::TupleRestNotRepresentable,
                                schema.provenance.clone(),
                            )
                            .message(
                                "`items` beside `prefixItems` allows a typed variable-length \
                                 remainder, which no single Rust type expresses",
                            )
                            .remedy(
                                "use `items: false` to close the tuple, describe the whole array \
                                 with `items`, or omit this API segment with spargen::omit!",
                            )
                            .emit(self.diags);
                            return None;
                        }
                    }
                    let mut items = Vec::new();
                    for (index, child) in schema.prefix_items.iter().enumerate() {
                        items.push(self.lower_schema_or(child, &format!("{hint}Item{index}"))?);
                        self.warn_structural_default_or(child, "a tuple `prefixItems` entry");
                    }
                    self.insert_schema_type(schema, hint, TypeKind::Tuple(items))
                } else {
                    let mut item = match &schema.items {
                        Some(items) => {
                            let item = self.lower_schema_or(items, &format!("{hint}Item"))?;
                            self.warn_structural_default_or(items, "array `items`");
                            item
                        }
                        None => self.insert_type(
                            &format!("{hint}Item"),
                            TypeKind::Any,
                            Docs::default(),
                            None,
                        ),
                    };
                    // A `Vec` already provides the heap indirection that breaks a `$ref` cycle, so a
                    // back-edge closing through an array never needs its own `Box`.
                    item.boxed = false;
                    self.insert_schema_type(schema, hint, TypeKind::Array(Box::new(item)))
                }
            }
            Some(JsonType::Object) | None
                if !schema.properties.is_empty() || !schema.pattern_properties.is_empty() =>
            {
                self.lower_object(schema, hint)?
            }
            Some(JsonType::Object) => self.lower_object(schema, hint)?,
            Some(JsonType::Null) => self.insert_schema_type(schema, hint, TypeKind::Null),
            None if schema.types.types.contains(&JsonType::Null) => {
                self.insert_schema_type(schema, hint, TypeKind::Null)
            }
            None => self.insert_schema_type(schema, hint, TypeKind::Any),
        };
        ty.nullable = nullable;
        Some(ty)
    }

    fn lower_type_array(
        &mut self,
        schema: &Schema,
        hint: &str,
        non_null_types: &[JsonType],
    ) -> Option<Ty> {
        let mut branches = Vec::with_capacity(non_null_types.len());
        for ty in non_null_types {
            let mut branch = schema.clone();
            branch.types.types = vec![*ty];
            branch.title = None;
            branch.description = None;
            branches.push(SchemaOr::Schema(Box::new(branch)));
        }

        let mut union = schema.clone();
        union.boolean = None;
        union.reference = None;
        union.types.types.clear();
        union.properties.clear();
        union.required.clear();
        union.additional_properties = None;
        union.pattern_properties.clear();
        union.items = None;
        union.prefix_items.clear();
        union.all_of.clear();
        union.one_of.clear();
        union.any_of = branches;
        union.discriminator = None;
        union.enum_values = None;
        union.const_value = None;
        union.format = None;
        union.content_encoding = None;
        union.content_media_type = None;
        union.content_schema = None;
        union.xml = None;
        union.validation = ValidationKeywords::default();
        // The array's `null` is a branch of its own: a union's `null` comes only from a branch
        // `null` matches (#574), and a `null` left in the union's `type` would instead be a
        // sibling every non-null branch is met with.
        if schema.types.types.contains(&JsonType::Null) {
            let mut null = union.clone();
            null.title = None;
            null.description = None;
            null.any_of.clear();
            null.types.types = vec![JsonType::Null];
            union.any_of.push(SchemaOr::Schema(Box::new(null)));
        }
        self.lower_union(&union, hint)
    }

    fn lower_object(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (fields, additional) = self.object_body(schema, hint)?;
        Some(self.insert_schema_type(
            schema,
            hint,
            TypeKind::Struct(Struct { fields, additional }),
        ))
    }

    fn lower_enum(&mut self, values: &[SpannedValue], schema: &Schema, hint: &str) -> Option<Ty> {
        // A `null` member — or `"null"` in the schema's own type array — makes the enum/const
        // nullable: strip the nulls, lower the remaining scalars as the enum, and wrap the result
        // in `Option`. The enum/const branch returns before `lower_schema` computes `nullable`, so
        // the nullability has to be decided here from both sources.
        let has_null = schema.types.types.contains(&JsonType::Null)
            || values.iter().any(|value| matches!(value.node, Node::Null));
        // Declared order is preserved (minus nulls) so double generation stays byte-identical.
        let remainder: Vec<&SpannedValue> = values
            .iter()
            .filter(|value| !matches!(value.node, Node::Null))
            .collect();

        // Only `null` members remained (`enum: [null]` / `const: null`): emit the exact JSON null
        // type (`()`), not a nullable unconstrained value that would also accept non-null content.
        if remainder.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        let mut variants = Vec::new();
        let mut repr = None;
        for value in remainder {
            let scalar = match scalar_value(value) {
                Some(value) => value,
                None => {
                    Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                        .message(non_scalar_enum_message(value))
                        .emit(self.diags);
                    return None;
                }
            };
            let scalar_repr = match scalar {
                ScalarValue::Bool(_) => ScalarRepr::Bool,
                ScalarValue::Int(_) => ScalarRepr::Int,
                ScalarValue::String(_) => ScalarRepr::String,
            };
            if repr
                .replace(scalar_repr)
                .is_some_and(|previous| previous != scalar_repr)
            {
                Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                    .message("enum/const values must all share the same scalar kind")
                    .emit(self.diags);
                return None;
            }
            variants.push(scalar);
        }
        // The enum def is the last graph insert; setting `nullable` afterward is a pure mutate that
        // preserves the component-root last-insert invariant asserted in `ensure_component`.
        let repr = repr.unwrap_or(ScalarRepr::String);
        // A string set whose own schema names a `uuid` or date format is narrowed against that
        // format exactly as an `allOf` member declaring it would narrow it (`narrowed_string`).
        let formatted = matches!(
            schema.format.as_deref(),
            Some("uuid" | "date" | "date-time")
        );
        let openness = if self.open_narrowing && repr == ScalarRepr::String && formatted {
            Openness::Locked
        } else {
            Openness::Closed
        };
        let mut ty = self.insert_schema_type(
            schema,
            hint,
            TypeKind::Enum(ScalarEnum {
                repr,
                variants,
                openness,
            }),
        );
        if self.narrowing_opens && repr == ScalarRepr::String {
            self.open_candidates.insert(ty.id);
        }
        ty.nullable = has_null;
        Some(ty)
    }

    /// Lower a possibly-`$ref` schema. A reference goes through [`Self::ensure_reference`] so
    /// every use site shares one generated type instead of lowering a duplicate.
    pub(super) fn lower_schema_ref(&mut self, schema: &RefOr<Schema>, hint: &str) -> Option<Ty> {
        match schema {
            RefOr::Item(schema) => self.lower_schema(schema, hint),
            RefOr::Ref(reference) => {
                self.ensure_reference(&reference.reference, &reference.provenance, hint)
            }
        }
    }

    pub(super) fn insert_schema_type(&mut self, schema: &Schema, hint: &str, kind: TypeKind) -> Ty {
        self.insert_type(
            hint,
            kind,
            Docs {
                title: schema.title.clone(),
                description: schema.description.clone(),
                deprecated: schema.deprecated,
                ..Docs::default()
            },
            Some(schema.provenance.clone()),
        )
    }

    pub(super) fn insert_type(
        &mut self,
        hint: &str,
        kind: TypeKind,
        docs: Docs,
        provenance: Option<crate::diag::Provenance>,
    ) -> Ty {
        let provenance = provenance.unwrap_or_else(|| self.document.provenance.clone());
        let document = provenance
            .span
            .map(|span| self.resolver.document_key(span.file))
            .unwrap_or_default();
        let id = self.graph.insert(TypeDef {
            name_hint: hint.to_owned(),
            kind,
            docs,
            provenance,
            document,
        });
        Ty {
            id,
            nullable: false,
            boxed: false,
        }
    }
}

fn scalar_value(value: &SpannedValue) -> Option<ScalarValue> {
    match &value.node {
        Node::Bool(value) => Some(ScalarValue::Bool(*value)),
        Node::Number(Number::Int(value)) => Some(ScalarValue::Int(*value)),
        Node::Number(Number::UInt(value)) => i64::try_from(*value).ok().map(ScalarValue::Int),
        Node::String(value) => Some(ScalarValue::String(value.clone())),
        _ => None,
    }
}

/// The `E008` message for an enum/const member `scalar_value` rejected, naming the reason that
/// member is not representable: an object/array member, a float, and an integer above `i64::MAX`
/// fail for different reasons, and a message blaming the wrong one misdirects the fix.
fn non_scalar_enum_message(value: &SpannedValue) -> String {
    match &value.node {
        Node::Number(Number::Float(float)) => format!(
            // `Debug`, so `1.0` reads as the float it is rather than `Display`'s `1`.
            "enum/const value {float:?} is a floating-point number, which has no Rust enum \
             discriminant (only string, integer, and boolean members are representable as enum \
             variants)"
        ),
        Node::Number(Number::UInt(uint)) => format!(
            "enum/const value {uint} exceeds i64::MAX, so it is not representable as an integer \
             enum variant"
        ),
        _ => "enum/const values must be scalars (object/array members are not representable as \
              enum variants)"
            .to_owned(),
    }
}
