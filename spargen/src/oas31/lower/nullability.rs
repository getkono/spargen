//! Whether a schema, an `allOf` merge, a union branch or a `$ref` target admits, denies, or
//! leaves undecided a `null`.

use std::collections::HashSet;

use crate::diag::{Diagnostics, Provenance};
use crate::ir::{Ty, TypeKind};
use crate::oas31::{JsonType, RefOr, Schema, SchemaOr};
use crate::source::{is_remote_ref, Node};

use super::combine::{schema_has_union, schema_is_object_like, Contribution};
use super::refiner::{implied_applicator_category, ImpliedCategory};
use super::shape::schema_has_shape_constraint;
use super::{memoised_answer, memoised_decision, resolved_identity, LowerCtx, MAX_SCHEMA_DEPTH};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Whether some member of `schema`'s object `allOf` decides the merge's nullability, as
    /// [`Self::gather_all_of`] records it in each [`Contribution::Object`]: the schema's own
    /// `type`, a member that states one, a `$ref` member whose target decides it
    /// ([`Self::ref_target_decides_null`]; a bundle target expanded in place is read as its own
    /// members are), and the members of a nested `allOf` or of a `$ref` member's siblings. A
    /// member this cannot read as an object (`enum`, `const`, a union) is taken as deciding, which
    /// keeps the lowered nullability. `depth` bounds the walk; a chain past it decides. Each `$ref`
    /// target is read once per pass and its answer replayed
    /// ([`Self::target_decides_null_memo`], [`Self::resolved_all_of_decides_null_memo`]).
    pub(super) fn all_of_decides_null(&self, schema: &Schema, depth: u32) -> bool {
        if depth >= MAX_SCHEMA_DEPTH
            || stated_nullability(schema).is_some()
            || schema.enum_values.is_some()
            || schema.const_value.is_some()
            || schema_has_union(schema)
        {
            return true;
        }
        if let Some(reference) = &schema.reference {
            let target_decides =
                if reference.starts_with("#/components/schemas/") || is_remote_ref(reference) {
                    self.ref_target_decides_null_within(reference, &schema.provenance, depth + 1)
                } else {
                    match self.resolver.resolve(
                        reference,
                        &schema.provenance,
                        &mut Diagnostics::default(),
                    ) {
                        Ok(resolved) => memoised_decision(
                            &self.resolved_all_of_decides_null_memo,
                            &resolved.schema,
                            || self.all_of_decides_null(&resolved.schema, depth + 1),
                        ),
                        Err(_) => true,
                    }
                };
            let mut sibling = schema.clone();
            sibling.reference = None;
            return target_decides
                || (schema_has_shape_constraint(&sibling)
                    && self.all_of_decides_null(&sibling, depth + 1));
        }
        schema.all_of.iter().any(|member| match member {
            SchemaOr::Schema(member) => self.all_of_decides_null(member, depth + 1),
            SchemaOr::Bool(_) => false,
        })
    }

    /// Whether `ty` is the exact JSON `null` (`()`), as a `const: null` or `enum: [null]` branch
    /// lowers: a branch `null` matches without its `Ty` being nullable.
    pub(super) fn is_exact_null(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Null)
        )
    }

    /// Whether `member`, a union branch lowered to `ty`, is an object or array whose lowered
    /// non-null struct, `Vec` or tuple decides nothing about `null`: no `type`, and nothing else
    /// [`Self::all_of_decides_null`] reads as deciding, so `null` satisfies it wherever the
    /// conjuncts the union is met with admit it. Untyped `items` or `prefixItems` alone lower to an
    /// array (#614) as untyped object applicators lower to a struct (#613), and leave `null`
    /// undecided alike.
    ///
    /// A cycle-closing `$ref` lowers to its target's still-open reservation, whose kind is not
    /// known yet, so it is read from the target body instead: a target whose object or array
    /// applicators establish its category ([`implied_applicator_category`]) is the struct, `Vec`
    /// or tuple it will be filled with, and the same branch leaves `null` undecided inside the
    /// cycle as outside it (#627).
    pub(super) fn branch_leaves_null_undecided(&self, member: &SchemaOr, ty: Ty) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        let undecided_kind = match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Struct(_) | TypeKind::Array(_) | TypeKind::Tuple(_)) => true,
            Some(TypeKind::Reserved) => member.reference.as_deref().is_some_and(|reference| {
                self.ref_target_is_untyped_shape(reference, &member.provenance, 0)
            }),
            _ => false,
        };
        !ty.nullable && undecided_kind && !self.all_of_decides_null(member, 0)
    }

    /// Whether `member`, a union branch lowered to `ty` that hoisted no `null`, is a branch `null`
    /// matches although nothing in it states `null`, as a `oneOf` counts it beside a branch that
    /// does: an untyped object or array ([`Self::branch_leaves_null_undecided`], #622), or a
    /// nested union `null` matches through its own branches ([`Self::union_admits_null_undecided`]),
    /// written inline or as a bare `$ref` to a union component (#628). A nested union is read by
    /// its branches whatever it lowered to, as [`Self::branch_takes_permitted_null`] reads it.
    pub(super) fn branch_matches_null_undecided(&self, member: &SchemaOr, ty: Ty) -> bool {
        if self.branch_leaves_null_undecided(member, ty) {
            return true;
        }
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        if ty.nullable {
            return false;
        }
        if schema_has_union(member) {
            return self.union_admits_null_undecided(member, 0);
        }
        let Some(reference) = member.reference.as_deref() else {
            return false;
        };
        // A `$ref` with siblings of its own is an intersection this does not read.
        let mut sibling = member.as_ref().clone();
        sibling.reference = None;
        !schema_has_shape_constraint(&sibling)
            && self
                .ref_target_body(reference, &member.provenance)
                .is_some_and(|target| {
                    schema_has_union(&target) && self.union_admits_null_undecided(&target, 0)
                })
    }

    /// Whether the body the `$ref` written at `at` names ([`Self::ref_target_body`]) lowers to a
    /// struct, `Vec` or tuple of untyped keywords, as [`Self::branch_leaves_null_undecided`] reads
    /// a cycle-closing reservation ([`Self::schema_is_untyped_shape`]).
    fn ref_target_is_untyped_shape(&self, reference: &str, at: &Provenance, depth: u32) -> bool {
        self.ref_target_body(reference, at)
            .is_some_and(|target| self.schema_is_untyped_shape(&target, depth + 1))
    }

    /// Whether `schema`, which names no `type`, lowers to a struct, `Vec` or tuple: object or array
    /// applicators that establish its category ([`implied_applicator_category`]), or an untyped
    /// `allOf` whose members, and object keywords beside them, merge into one (#627, #636); see
    /// [`Self::untyped_shape_category`]. A shape this cannot read answers `false`, and keeps the
    /// reservation's stated nullability.
    fn schema_is_untyped_shape(&self, schema: &Schema, depth: u32) -> bool {
        self.untyped_shape_category(schema, depth).is_some()
    }

    /// The category of the struct, `Vec` or tuple `schema`, which names no `type`, lowers to, as
    /// [`Self::schema_is_untyped_shape`] reads it: [`JsonType::Object`] or [`JsonType::Array`],
    /// and `None` for a shape this cannot read.
    ///
    /// Untyped object or array applicators are their own category. An untyped `allOf` is read by
    /// its members: one counts when it is untyped object keywords, untyped array applicators
    /// alone, a nested such `allOf`, a `$ref` to any of them, or a pure annotation. The merge is a
    /// struct where a member, or the keywords beside the `allOf`, is an object, untyped array
    /// members being vacuous beside it (#636); an array where every constraining member is one,
    /// since they then establish the array category; and `None` where no member constrains.
    /// Anything else (a scalar keyword, a union, object and array applicators in one member)
    /// answers `None`. `depth` bounds the walk.
    fn untyped_shape_category(&self, schema: &Schema, depth: u32) -> Option<JsonType> {
        if depth >= MAX_SCHEMA_DEPTH {
            return None;
        }
        match implied_applicator_category(schema) {
            Some(ImpliedCategory::Only(category)) => return Some(category),
            Some(ImpliedCategory::Conflicting) => return None,
            None => {}
        }
        let only_all_of = !schema.all_of.is_empty()
            && schema.types.types.is_empty()
            && schema.reference.is_none()
            && schema.enum_values.is_none()
            && schema.const_value.is_none()
            && !schema_has_union(schema)
            && schema.content_encoding.is_none()
            && schema.format.as_deref() != Some("binary")
            && schema.items.is_none()
            && schema.prefix_items.is_empty();
        if !only_all_of {
            return None;
        }
        let mut object = schema_is_object_like(schema);
        let mut array = false;
        for member in &schema.all_of {
            match member {
                SchemaOr::Bool(true) => {}
                SchemaOr::Bool(false) => return None,
                SchemaOr::Schema(member) if !schema_has_shape_constraint(member) => {}
                SchemaOr::Schema(member) => {
                    let category = if let Some(reference) = member.reference.as_deref() {
                        // A `$ref` member with siblings of its own is an intersection this does
                        // not read.
                        let mut sibling = member.as_ref().clone();
                        sibling.reference = None;
                        if schema_has_shape_constraint(&sibling) {
                            None
                        } else {
                            self.ref_target_body(reference, &member.provenance)
                                .and_then(|target| self.untyped_shape_category(&target, depth + 1))
                        }
                    } else {
                        self.untyped_shape_category(member, depth + 1)
                    };
                    match category {
                        Some(JsonType::Object) => object = true,
                        Some(_) => array = true,
                        None => return None,
                    }
                }
            }
        }
        if object {
            Some(JsonType::Object)
        } else if array {
            Some(JsonType::Array)
        } else {
            None
        }
    }

    /// Whether `member`, a union branch lowered to `ty`, accepts `null` wherever a conjunct the
    /// union is met with admits it: an untyped object ([`Self::branch_leaves_null_undecided`]), or
    /// a branch that states nothing and lowers to `Value` (`true`, `{}`), which the meet otherwise
    /// leaves as it is (#588, #592). Either is then counted as a branch `null` matches.
    pub(super) fn branch_takes_conjunct_null(&self, member: &SchemaOr, ty: Ty) -> bool {
        self.branch_leaves_null_undecided(member, ty)
            || (!ty.nullable
                && self
                    .graph
                    .get(ty.id)
                    .is_some_and(|def| matches!(def.kind, TypeKind::Any)))
    }

    /// Whether `member`, a union branch lowered to `ty`, takes the `null` its enclosing `type`
    /// array permits (#574): its own keywords leave `null` undecided, so `null` matches it
    /// wherever the union's other sibling keywords admit it. That is a branch lowering to `Value`,
    /// an untyped object ([`Self::branch_leaves_null_undecided`]), a cycle-closing `$ref` whose
    /// target's struct is still a reservation and whose keywords decide nothing, and a nested
    /// union `null` matches through its own branches ([`Self::union_admits_null_undecided`]).
    pub(super) fn branch_takes_permitted_null(&self, member: &SchemaOr, ty: Ty) -> bool {
        // A nested union's branches decide, whatever it lowered to: one that is `Value` can still
        // be a `oneOf` that `null` matches twice.
        if let SchemaOr::Schema(member) = member {
            if schema_has_union(member) {
                return !ty.nullable && self.union_admits_null_undecided(member, 0);
            }
        }
        if self
            .graph
            .get(ty.id)
            .is_some_and(|def| matches!(def.kind, TypeKind::Any))
            || self.branch_leaves_null_undecided(member, ty)
        {
            return true;
        }
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        !ty.nullable && self.is_reservation(ty.id) && !self.all_of_decides_null(member, 0)
    }

    /// Whether `schema` is a `oneOf`/`anyOf` whose other keywords decide nothing about `null` and
    /// whose branches `null` matches by the union's own rule (exactly one for `oneOf`, at least one
    /// for `anyOf`), counting a branch that states `null`, one that is `true`, one whose keywords
    /// leave `null` undecided, and a nested union that admits it in turn. `depth` bounds the walk.
    fn union_admits_null_undecided(&self, schema: &Schema, depth: u32) -> bool {
        let (members, one_of) = match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
            (false, true) => (&schema.one_of, true),
            (true, false) => (&schema.any_of, false),
            _ => return false,
        };
        if depth >= MAX_SCHEMA_DEPTH {
            return false;
        }
        let mut rest = schema.clone();
        rest.one_of.clear();
        rest.any_of.clear();
        rest.discriminator = None;
        if self.all_of_decides_null(&rest, depth + 1) {
            return false;
        }
        let matching = members
            .iter()
            .filter(|member| match member {
                SchemaOr::Bool(accepts) => *accepts,
                SchemaOr::Schema(member) => {
                    stated_nullability(member) == Some(true)
                        || self.union_admits_null_undecided(member, depth + 1)
                        || (!schema_has_union(member)
                            && !self.all_of_decides_null(member, depth + 1))
                }
            })
            .count();
        if one_of {
            matching == 1
        } else {
            matching > 0
        }
    }

    /// Whether `schema`, the body an `allOf` member names, is a union whose branches leave `null`
    /// undecided yet match it ([`Self::union_admits_null_undecided`]), or an `allOf` wrapping one
    /// ([`Self::all_of_wraps_undecided_union`]). Either admits `null` without deciding it.
    fn admits_null_undecided_through_union(&self, schema: &Schema, depth: u32) -> bool {
        if schema_has_union(schema) {
            self.union_admits_null_undecided(schema, depth)
        } else {
            self.all_of_wraps_undecided_union(schema, depth)
        }
    }

    /// Whether `schema` is an `allOf`, with no `$ref` or union of its own and own keywords that
    /// decide nothing about `null`, whose members include a union that admits `null` undecided
    /// ([`Self::admits_null_undecided_through_union`]), inline or as a bare `$ref` to it, and are
    /// otherwise members that decide nothing ([`Self::all_of_decides_null`]): `Wrap: { allOf:
    /// [ { $ref: U } ] }` admits exactly the values `U` does, so a `$ref` member to `Wrap`, or a
    /// non-component pointer to such an `allOf`, takes `null` as a `$ref` member to `U` does
    /// (#638). `depth` bounds the walk; a chain past it answers `false`. Each `$ref` member's target
    /// is read once per pass and its answer replayed ([`Self::admits_null_undecided_memo`]).
    fn all_of_wraps_undecided_union(&self, schema: &Schema, depth: u32) -> bool {
        if depth >= MAX_SCHEMA_DEPTH
            || schema.all_of.is_empty()
            || schema.reference.is_some()
            || schema_has_union(schema)
        {
            return false;
        }
        let mut own = schema.clone();
        own.all_of.clear();
        if self.all_of_decides_null(&own, depth + 1) {
            return false;
        }
        let mut wraps = false;
        for member in &schema.all_of {
            let member = match member {
                SchemaOr::Bool(true) => continue,
                SchemaOr::Bool(false) => return false,
                SchemaOr::Schema(member) => member.as_ref(),
            };
            let admits = match member.reference.as_deref() {
                Some(reference) => {
                    // A `$ref` member with siblings of its own is an intersection this does not
                    // read.
                    let mut sibling = member.clone();
                    sibling.reference = None;
                    !schema_has_shape_constraint(&sibling)
                        && self
                            .ref_target_body(reference, &member.provenance)
                            .is_some_and(|body| {
                                memoised_answer(
                                    &self.admits_null_undecided_memo,
                                    &body,
                                    false,
                                    || self.admits_null_undecided_through_union(&body, depth + 1),
                                )
                            })
                }
                None => self.admits_null_undecided_through_union(member, depth + 1),
            };
            if admits {
                wraps = true;
            } else if self.all_of_decides_null(member, depth + 1) {
                return false;
            }
        }
        wraps
    }

    /// [`target_decides_null`] of the schema the `$ref` written at `at` lowered to: a root
    /// component first, as [`Self::ensure_component`] chooses it, then a remote document from the
    /// root, as [`Self::ensure_remote`] resolves it, and otherwise the referring file's target.
    /// A target that is a bare alias (a `$ref` with no shape-bearing sibling) is the schema it
    /// names, so the chain is followed to its body. The target was already lowered from the same
    /// resolution, so this only re-reads it; a target that cannot be read keeps its lowered
    /// nullability as the decision, and reports nothing a second time.
    ///
    /// A target body that is an `allOf` decides exactly when some member of its merge does
    /// ([`Self::all_of_decides_null`], issue #565): a component composed of untyped object members
    /// alone lowers to the non-null struct [`object_all_of_admits_null`] gives a merge no member
    /// decides, and that struct is no decision, as the same members written inline make none.
    pub(super) fn ref_target_decides_null(&self, reference: &str, at: &Provenance) -> bool {
        self.ref_target_decides_null_within(reference, at, 0)
    }

    /// [`Self::ref_target_decides_null`] reached `depth` steps into an
    /// [`Self::all_of_decides_null`] walk, which bounds the two together: a target's `allOf` can
    /// name the `$ref` that reached it.
    fn ref_target_decides_null_within(&self, reference: &str, at: &Provenance, depth: u32) -> bool {
        self.ref_target_body(reference, at).is_none_or(|target| {
            if target.all_of.is_empty() {
                target_decides_null(&target)
            } else {
                memoised_decision(&self.target_decides_null_memo, &target, || {
                    self.all_of_decides_null(&target, depth + 1)
                })
            }
        })
    }

    /// The body of the schema the `$ref` written at `at` names, as
    /// [`Self::ref_target_decides_null`] finds it: a root component first, then a remote document
    /// from the root, and otherwise the referring file's target, with a bare alias (a `$ref` with
    /// no shape-bearing sibling) followed to the schema it names. `None` where the chain cannot be
    /// read, which reports nothing a second time.
    pub(super) fn ref_target_body(
        &self,
        reference: &str,
        at: &Provenance,
    ) -> Option<std::borrow::Cow<'_, Schema>> {
        let mut reference = reference.to_owned();
        let mut at = at.clone();
        // Lowering the chain already refused an alias cycle; the bound only keeps this total.
        for _ in 0..MAX_SCHEMA_DEPTH {
            let component = reference
                .strip_prefix("#/components/schemas/")
                .and_then(|name| self.document.components.schemas.get(name));
            let target = match component {
                Some(RefOr::Item(target)) => std::borrow::Cow::Borrowed(target),
                Some(RefOr::Ref(alias)) => {
                    reference.clone_from(&alias.reference);
                    at = alias.provenance.clone();
                    continue;
                }
                None => {
                    let from = if is_remote_ref(&reference) {
                        &self.document.provenance
                    } else {
                        &at
                    };
                    let Ok(resolved) =
                        self.resolver
                            .resolve(&reference, from, &mut Diagnostics::default())
                    else {
                        return None;
                    };
                    resolved.schema
                }
            };
            let Some(next) = &target.reference else {
                return Some(target);
            };
            let mut sibling = target.as_ref().clone();
            sibling.reference = None;
            if schema_has_shape_constraint(&sibling) {
                return Some(target);
            }
            reference.clone_from(next);
            at = target.provenance.clone();
        }
        None
    }

    /// Whether a union branch decides `null` for [`undecided_admits_null`], reading a `$ref`
    /// branch through its target (#594): `false` decides, `true` does not, and a schema branch
    /// decides where [`target_decides_null`] of its own keywords does. A `$ref` branch is its
    /// target met with its own sibling keywords, so it decides where either does; a `$ref` to a
    /// `true` or `{}` component then leaves `null` undecided, as the same branch written inline
    /// does. The target body is read as the inline branch is, not through an `allOf` walk, so a
    /// `$ref` to an `allOf` component decides as the inline `allOf` branch does.
    pub(super) fn branch_decides_null(&self, branch: &SchemaOr) -> bool {
        match branch {
            SchemaOr::Bool(admits) => !admits,
            SchemaOr::Schema(schema) => self.ref_schema_decides_null(schema, 0),
        }
    }

    /// [`Self::branch_decides_null`] of a schema node reached `depth` `$ref` steps from the
    /// branch: a `$ref` node decides `null` where its own sibling keywords do or its target does,
    /// read the same way; any other node is [`target_decides_null`]. A boolean target (a `false`
    /// or `true` component) is read as the inline boolean branch is: `false` decides, `true` does
    /// not. A target that cannot be read, or a chain past the bound, decides, which keeps the
    /// lowered nullability.
    fn ref_schema_decides_null(&self, schema: &Schema, depth: u32) -> bool {
        if let Some(admits) = schema.boolean {
            return !admits;
        }
        let Some(reference) = &schema.reference else {
            return target_decides_null(schema);
        };
        let mut sibling = schema.clone();
        sibling.reference = None;
        target_decides_null(&sibling)
            || depth >= MAX_SCHEMA_DEPTH
            || self
                .ref_target_body(reference, &schema.provenance)
                .is_none_or(|target| self.ref_schema_decides_null(&target, depth + 1))
    }

    /// Whether a union branch denies `null` ([`denies_null`]), reading a `$ref` branch through its
    /// target (#590): the branch is its target met with its own sibling keywords, so it denies
    /// `null` where either does by its own keywords alone. A `$ref` to a non-null object
    /// component then decides nothing beside an untyped branch, as the same branch written
    /// inline does. A target body that is itself a `$ref` with shape siblings is read the same
    /// way, recursively, so `{ $ref: W }` with `W: { $ref: U, type: object }` denies `null` as the
    /// inline `{ $ref: U, type: object }` does. A target that cannot be read, or whose body is an
    /// `allOf` or a union, may admit `null`.
    pub(super) fn branch_denies_null(&self, branch: &SchemaOr) -> bool {
        match branch {
            SchemaOr::Schema(schema) => self.ref_schema_denies_null(schema, 0),
            SchemaOr::Bool(_) => denies_null(branch),
        }
    }

    /// [`Self::branch_denies_null`] of a schema node reached `depth` `$ref` steps from the branch:
    /// a `$ref` node denies `null` where its own sibling keywords do or its target does, read
    /// the same way; any other node is [`schema_denies_null`]. A boolean target is read as the
    /// inline boolean branch is ([`denies_null`]): a `$ref` to a `false` component denies `null`,
    /// so beside an untyped branch it decides nothing, as the inline `false` does, now that
    /// [`Self::branch_decides_null`] reads that target as deciding (#594). Lowering already
    /// refused a `$ref` cycle; the bound only keeps this total.
    fn ref_schema_denies_null(&self, schema: &Schema, depth: u32) -> bool {
        if let Some(admits) = schema.boolean {
            return !admits;
        }
        let Some(reference) = &schema.reference else {
            return schema_denies_null(schema);
        };
        let mut sibling = schema.clone();
        sibling.reference = None;
        schema_denies_null(&sibling)
            || (depth < MAX_SCHEMA_DEPTH
                && self
                    .ref_target_body(reference, &schema.provenance)
                    .is_some_and(|target| self.ref_schema_denies_null(&target, depth + 1)))
    }

    /// Apply the enclosing `allOf` schema's own nullability (a `"null"` in its type array) to the
    /// merged type. Set after the final insert — a pure mutate that preserves the last-insert
    /// invariant.
    pub(super) fn with_all_of_nullability(&self, schema: &Schema, mut ty: Ty) -> Ty {
        if schema.types.types.contains(&JsonType::Null) {
            ty.nullable = true;
        }
        ty
    }

    /// The null type for an object `allOf` whose object meet is empty, when `null` still satisfies
    /// it: every member admits `null` and one decides it ([`object_all_of_admits_null`]). The
    /// schema's own object keywords are one of those members ([`Self::gather_all_of`]), so its
    /// `type` listing `null` counts as their answer, and does not override a member that denies
    /// it: no value satisfies such a schema. Its own `type`, `enum` or `const` that is not an
    /// object keyword ([`schema_is_object_like`]) is no contribution, yet constrains every value
    /// all the same, so one that excludes `null` leaves nothing either. So does such a keyword on
    /// a nested `allOf` member [`Self::gather_member`] flattens into this meet, inline or as a
    /// non-component `$ref`'s resolved target, which equally contributes nothing of its own
    /// ([`Self::flattened_keywords_admit_null`], #569). `None` where `null` is excluded, so the
    /// caller reports the empty composition.
    ///
    /// `intersect_types` collapses an empty non-null meet that admits `null` to the null type, so
    /// the `$ref`-sibling spelling of the same conjunction already lowered to `()`; rejecting it
    /// here split the spellings of one conjunction (#542), as #450 removed for a nullable union
    /// refined to nothing but `null`. The meets inserted since `mark` reach nothing the null type
    /// refers to, so they are discarded, and the null type is the final graph insert.
    ///
    /// [`schema_is_object_like`]: super::combine::schema_is_object_like
    pub(super) fn null_only_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        contributions: &[Contribution],
        mark: u32,
    ) -> Option<Ty> {
        if !object_all_of_admits_null(contributions) || !self.flattened_keywords_admit_null(schema)
        {
            return None;
        }
        self.discard_meet_intermediates(mark, &TypeKind::Null);
        Some(self.insert_schema_type(schema, hint, TypeKind::Null))
    }

    /// [`own_keywords_admit_null`] over an `allOf` schema and every schema
    /// [`Self::gather_member`] flattens into its meet: a member with an `allOf` and no union beside
    /// it, whether inline, as a `$ref`'s siblings, or as the resolved target of a non-component
    /// `$ref` member, which [`Self::gather_ref_target`] expands in place rather than lowering.
    /// Flattening gathers only such a schema's members and object keywords, so its own `type`,
    /// `enum` or `const` reaches no contribution, yet still constrains every value of the meet
    /// (#569). Any other member is lowered or read as a contribution of its own, which carries its
    /// nullability already: a component or remote `$ref` lowers to a type, and a plain body is
    /// read by its keywords.
    pub(super) fn flattened_keywords_admit_null(&self, schema: &Schema) -> bool {
        own_keywords_admit_null(schema)
            && self.flattened_members_admit_null(schema, &mut HashSet::new(), 0)
    }

    /// Whether every schema `schema`'s `allOf` members flatten into its meet admits `null` by its
    /// own keywords: [`Self::flattened_keywords_admit_null`] below the schema itself.
    fn flattened_members_admit_null(
        &self,
        schema: &Schema,
        visited: &mut HashSet<String>,
        depth: u32,
    ) -> bool {
        schema.all_of.iter().all(|member| match member {
            SchemaOr::Schema(member) => self.flattened_member_admits_null(member, visited, depth),
            SchemaOr::Bool(_) => true,
        })
    }

    /// [`Self::flattened_keywords_admit_null`] of one `allOf` member, read the way
    /// [`Self::gather_member`] gathers it. The answer is the conjunction of every flattened
    /// schema's own keywords, so a resolved target already in `visited` adds nothing a second
    /// time: skipping it keeps a target shared along many paths linear. Gathering already refused
    /// a cycle and an over-deep chain, and resolved every target this reaches, so the depth bound
    /// only keeps this total and a target that cannot be read is no constraint here.
    fn flattened_member_admits_null(
        &self,
        member: &Schema,
        visited: &mut HashSet<String>,
        depth: u32,
    ) -> bool {
        if depth >= MAX_SCHEMA_DEPTH {
            return true;
        }
        if let Some(reference) = &member.reference {
            let target_admits = reference.starts_with("#/components/schemas/")
                || is_remote_ref(reference)
                || match self.resolver.resolve(
                    reference,
                    &member.provenance,
                    &mut Diagnostics::default(),
                ) {
                    Ok(resolved) => {
                        let fresh = resolved_identity(&resolved.schema.provenance)
                            .is_none_or(|key| visited.insert(key));
                        !fresh
                            || self.flattened_member_admits_null(
                                &resolved.schema,
                                visited,
                                depth + 1,
                            )
                    }
                    Err(_) => true,
                };
            let mut sibling = member.clone();
            sibling.reference = None;
            return target_admits
                && (!schema_has_shape_constraint(&sibling)
                    || self.flattened_member_admits_null(&sibling, visited, depth + 1));
        }
        if member.all_of.is_empty() || schema_has_union(member) {
            return true;
        }
        own_keywords_admit_null(member)
            && self.flattened_members_admit_null(member, visited, depth + 1)
    }

    /// Whether an already-lowered type admits JSON `null`, resolving its kind out of the graph.
    /// [`type_accepts_null`] needs the kind beside the [`Ty`]; callers outside the intersection
    /// machinery hold only the [`Ty`].
    pub(super) fn ty_accepts_null(&self, ty: Ty) -> bool {
        self.graph
            .get(ty.id)
            .is_some_and(|def| type_accepts_null(ty, &def.kind))
    }

    /// The `union` member of an `allOf` of scalars, lowered on its own to `ty`, as it meets the
    /// other members' `contributions` ([`Self::lower_all_of_with_union_member`]): admitting `null`
    /// where the union's branches leave it undecided and still match it by the union's own rule
    /// ([`Self::union_admits_null_undecided`]), such as untyped `items` branches of an `anyOf`, so
    /// the meet admits `null` exactly where every other member does (#621). Lowered on its own,
    /// such a union denies `null` for want of a decision, and the meet then denied it even beside
    /// `type: [array, 'null']`, where the object spelling's meet keeps it. A member that lowers to
    /// `Value` decides nothing, so with no member of any other type the union keeps the answer it
    /// has on its own. `union` may also be an `allOf` wrapping such a union, which a `$ref` member
    /// names ([`Self::admits_null_undecided_through_union`], #638).
    pub(super) fn union_member_takes_scalar_null<'c>(
        &self,
        mut ty: Ty,
        union: &Schema,
        contributions: impl IntoIterator<Item = &'c Contribution>,
    ) -> Ty {
        let decided = contributions.into_iter().any(|contribution| {
            matches!(contribution, Contribution::Scalar(other)
                if !self
                    .graph
                    .get(other.id)
                    .is_some_and(|def| matches!(def.kind, TypeKind::Any)))
        });
        if decided && !ty.nullable && self.admits_null_undecided_through_union(union, 0) {
            ty.nullable = true;
        }
        ty
    }

    /// [`Self::union_member_takes_scalar_null`] of each `allOf` member in `members` that is a
    /// union, gathered as the scalar `contributions[slot]` it lowered to, met with the other
    /// contributions: a `$ref` to a union (#624), its target read through
    /// [`Self::ref_target_body`], aliases followed, or an inline union (#631). A `$ref` member
    /// that is not a bare `$ref` (its shape-bearing siblings are further members, which the
    /// union's branches never saw), or whose target is neither a union nor an `allOf` wrapping one
    /// ([`Self::all_of_wraps_undecided_union`], #638), keeps its contribution. A wrapping
    /// component is gathered as the one scalar it lowers to, and a wrapping non-component target
    /// as the one scalar its wrapped union lowers to once [`Self::gather_ref_target`] expands it
    /// in place. An inline union's own keywords are its siblings, read by the union's own rule.
    ///
    /// Another such member whose union leaves `null` undecided is not one of the members that
    /// decide it: it is exactly a member this rule would hand `null` to, whether written as a
    /// `$ref` or inline. So `allOf: [ { $ref: A }, { $ref: B } ]` of two such unions, the same
    /// `$ref` twice, or two such unions inline or one of each, stays non-null, and only a member
    /// that does decide `null`, such as `type: [array, 'null']`, gives it to them.
    pub(super) fn union_members_take_scalar_null(
        &self,
        members: &[(usize, &Schema)],
        contributions: &mut [Contribution],
    ) {
        let unions: Vec<_> = members
            .iter()
            .filter_map(|&(slot, member)| {
                let Contribution::Scalar(ty) = contributions.get(slot)? else {
                    return None;
                };
                let target = match member.reference.as_deref() {
                    Some(reference) => {
                        let mut sibling = member.clone();
                        sibling.reference = None;
                        if schema_has_shape_constraint(&sibling) {
                            return None;
                        }
                        self.ref_target_body(reference, &member.provenance)?
                    }
                    None => std::borrow::Cow::Borrowed(member),
                };
                (schema_has_union(&target) || self.all_of_wraps_undecided_union(&target, 0))
                    .then_some((slot, *ty, target))
            })
            .collect();
        let undecided: Vec<usize> = unions
            .iter()
            .filter(|(_, ty, target)| {
                !ty.nullable && self.admits_null_undecided_through_union(target, 0)
            })
            .map(|(slot, _, _)| *slot)
            .collect();
        for (slot, ty, target) in unions {
            let others = contributions
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != slot && !undecided.contains(index))
                .map(|(_, contribution)| contribution);
            let ty = self.union_member_takes_scalar_null(ty, &target, others);
            contributions[slot] = Contribution::Scalar(ty);
        }
    }
}

pub(super) fn type_accepts_null(ty: Ty, kind: &TypeKind) -> bool {
    ty.nullable || matches!(kind, TypeKind::Null | TypeKind::Any)
}

pub(super) fn non_nullable(mut ty: Ty) -> Ty {
    ty.nullable = false;
    ty
}

/// Whether an object `allOf` merge admits `null` (issue #425): every member that decides its
/// nullability admits it, and at least one decides. An untyped member admits `null` and decides
/// nothing, so a merge of untyped members alone keeps the non-null struct an untyped object
/// schema lowers to on its own; one nullable `$ref` member beside them makes it nullable, as the
/// `$ref`-sibling spelling of the same conjunction does. Only object contributions reach here.
pub(super) fn object_all_of_admits_null(contributions: &[Contribution]) -> bool {
    let mut decided = false;
    for contribution in contributions {
        if let Contribution::Object {
            nullable: Some(admits),
            ..
        } = contribution
        {
            if !admits {
                return false;
            }
            decided = true;
        }
    }
    decided
}

/// An object composition met with a union beside it ([`LowerCtx::lower_all_of_beside_union`],
/// [`LowerCtx::lower_all_of_with_union_member`]), made to admit `null` where no member decides it
/// (issue #541): its members are untyped objects alone — `$ref`s to untyped object components,
/// which [`object_all_of_admits_null`] reads as denying `null` for want of a decision — and the
/// same members written inline are scoped refiners that leave `null` to the union. So the union
/// decides it in either spelling, where it decides it at all: a `union` that states no `type` and
/// whose every branch is untyped decides nothing either, and the meet keeps the non-null answer an
/// `allOf` of untyped members alone gets. Nor does one whose only deciding branches deny `null`
/// beside an untyped branch (#581): the untyped branch leaves `null` undecided, so the inline
/// spelling of the same meet stays non-null. `denies_null` is [`LowerCtx::branch_denies_null`],
/// which reads a `$ref` branch through its target (#590), so a `$ref` to a non-null object
/// component denies `null` there as the same branch written inline does. `decides` is
/// [`LowerCtx::branch_decides_null`], which reads a `$ref` branch through its target (#594), so a
/// `$ref` to a `true` or `{}` component leaves `null` undecided as the inline `true` or `{}`
/// branch does. A composition some member decides, or a scalar one, keeps its nullability.
pub(super) fn undecided_admits_null(
    mut composed: Ty,
    has_object: bool,
    contributions: &[Contribution],
    union: &Schema,
    decides: impl Fn(&SchemaOr) -> bool,
    denies_null: impl Fn(&SchemaOr) -> bool,
) -> Ty {
    let decided = contributions.iter().any(|contribution| {
        matches!(
            contribution,
            Contribution::Object {
                nullable: Some(_),
                ..
            }
        )
    });
    let branches = || union.one_of.iter().chain(&union.any_of);
    // An untyped branch leaves `null` to the union's other branches (#581): beside one, branches
    // that deny `null` (by their own keywords, or a `$ref` branch's target) decide nothing, as
    // the same union met with the same members written inline keeps them non-null.
    let has_undecided = branches().any(|branch| !decides(branch));
    let union_decides = stated_nullability(union).is_some()
        || branches().any(|branch| decides(branch) && !(has_undecided && denies_null(branch)));
    if has_object && !decided && union_decides {
        composed.nullable = true;
    }
    composed
}

/// Whether a branch of `union` admits `null` by its own keywords: one with no `$ref`, `allOf` or
/// union of its own whose stated `type`, `enum` or `const` admits it, such as
/// `type: [object, 'null']` or `type: 'null'` (#586). An untyped conjunct leaves `null` to the
/// union, so where such a branch admits it the conjunct does too, as [`undecided_admits_null`]
/// makes an untyped `allOf` composition admit it: the `$ref`-sibling spelling's untyped target
/// and the inline spellings' untyped refiners then admit `null`, and an untyped object branch
/// takes it from the meet as it does in the `allOf` spellings. The `true` schema states nothing,
/// and a branch whose `null` lies behind a `$ref` or a composition is not read here.
pub(super) fn union_branch_admits_null(union: &Schema) -> bool {
    union.one_of.iter().chain(&union.any_of).any(|branch| {
        matches!(branch, SchemaOr::Schema(branch)
            if branch.reference.is_none()
                && branch.all_of.is_empty()
                && !schema_has_union(branch)
                && (stated_nullability(branch).is_some()
                    || branch.enum_values.is_some()
                    || branch.const_value.is_some())
                && own_keywords_admit_null(branch))
    })
}

/// Whether a union branch denies `null` by its own keywords alone: `false`, or a branch with no
/// `$ref`, `allOf` or union of its own whose stated `type`, `enum` or `const` leaves `null` out
/// ([`own_keywords_admit_null`]). A branch this cannot read so may admit `null`; a `$ref` branch
/// is read through its target by [`LowerCtx::branch_denies_null`].
fn denies_null(branch: &SchemaOr) -> bool {
    match branch {
        SchemaOr::Bool(admits) => !admits,
        SchemaOr::Schema(branch) => schema_denies_null(branch),
    }
}

/// [`denies_null`] of a schema node: one with no `$ref`, `allOf` or union of its own whose stated
/// `type`, `enum` or `const` leaves `null` out.
fn schema_denies_null(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.all_of.is_empty()
        && !schema_has_union(schema)
        && !own_keywords_admit_null(schema)
}

/// Whether a schema's own `type` admits `null`: `None` for an untyped schema, which states no
/// category and so decides nothing about `null` in an `allOf` merge.
pub(super) fn stated_nullability(schema: &Schema) -> Option<bool> {
    (!schema.types.types.is_empty()).then(|| schema.types.types.contains(&JsonType::Null))
}

/// Whether a `$ref` member's target decides its own nullability in an `allOf` merge (issue #541).
/// A plain untyped body — no `type`, no `enum` or `const`, no `$ref` and no composition
/// — is the inline untyped member written as a component: it admits `null` and decides nothing, as
/// [`stated_nullability`] reads the inline spelling. Anything else keeps its lowered nullability as
/// the decision it was before, except that [`LowerCtx::ref_target_decides_null`] reads a `$ref`
/// target's `allOf` body through its members (issue #565).
fn target_decides_null(schema: &Schema) -> bool {
    stated_nullability(schema).is_some()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.reference.is_some()
        || !schema.all_of.is_empty()
        || schema_has_union(schema)
}

/// Whether a union member is a null-only schema (`{type: "null"}`) — stripped from the union and
/// folded into its nullability as a branch `null` matches. A `$ref` member, with or without
/// siblings, is never null-only here (it names a schema with its own shape), and neither is a
/// boolean member.
///
/// The member must be inline, its `type` exactly `null`, and carry no `oneOf`, `anyOf`, `allOf`,
/// `enum`, `const` or `properties`. Nothing else it carries is read: every other keyword
/// (`required`, `items`, `additionalProperties`, `format`, a validation keyword, an annotation)
/// either is vacuous for a `null` instance in 2020-12 or constrains nothing the generated type
/// carries, so `{type: "null", required: [a]}` counts as null-only too.
pub(super) fn member_is_null_only(member: &SchemaOr) -> bool {
    let SchemaOr::Schema(schema) = member else {
        return false;
    };
    schema.reference.is_none()
        && schema.types.types == [JsonType::Null]
        && schema.one_of.is_empty()
        && schema.any_of.is_empty()
        && schema.all_of.is_empty()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.properties.is_empty()
}

/// Whether a schema accepts `null`: a `"null"` member of its type array, or a `null` `enum` member
/// or `const`. Computed at component reserve time so `$ref` consumers wrap the type in `Option`,
/// and it agrees with the `nullable` that [`LowerCtx::lower_schema`]/[`LowerCtx::lower_enum`]
/// compute from the same schema.
pub(super) fn schema_is_nullable(schema: &Schema) -> bool {
    schema.types.types.contains(&JsonType::Null)
        || schema
            .enum_values
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| matches!(value.node, Node::Null)))
        || schema
            .const_value
            .as_ref()
            .is_some_and(|value| matches!(value.node, Node::Null))
}

/// Whether every one of a schema's own `type`, `enum` and `const` that it states admits `null`
/// (vacuously so where it states none). Unlike [`schema_is_nullable`], which asks whether any of
/// them lists `null`, this is their conjunction: `{type: string, enum: [null]}` admits no `null`.
pub(super) fn own_keywords_admit_null(schema: &Schema) -> bool {
    let type_admits = stated_nullability(schema).unwrap_or(true);
    let enum_admits = schema
        .enum_values
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| matches!(value.node, Node::Null)));
    let const_admits = schema
        .const_value
        .as_ref()
        .is_none_or(|value| matches!(value.node, Node::Null));
    type_admits && enum_admits && const_admits
}
