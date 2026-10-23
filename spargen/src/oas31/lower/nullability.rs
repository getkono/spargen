//! Whether a schema, an `allOf` merge, a union branch or a `$ref` target admits, denies, or
//! leaves undecided a `null`.

use std::collections::HashSet;

use crate::diag::{Diagnostics, Provenance};
use crate::ir::{Ty, TypeId, TypeKind, Union, UnionMode, UnionStrategy};
use crate::oas31::{JsonType, RefOr, Schema, SchemaOr};
use crate::source::{is_remote_ref, Node};

use super::combine::Contribution;
use super::refiner::{implied_applicator_category, ImpliedCategory};
use super::shape::{
    ref_siblings_bear_shape, schema_has_shape_constraint, schema_has_union, schema_is_object_like,
};
use super::strategy::retain_strategy;
use super::{memoised_answer, resolved_identity, LowerCtx, MAX_SCHEMA_DEPTH};

/// What a schema says about the JSON `null` instance, read from the schema alone in JSON Schema
/// 2020-12 terms, whatever spelling the constraints take ([`LowerCtx::null_admission`]).
///
/// `null` is valid against a schema or it is not; that is [`Self::matches`]. Spargen also tells
/// apart two ways `null` can be valid, because they lower differently: a schema that *states*
/// `null` (a `type`, `enum` or `const` listing it) generates `Option<T>`, and one whose keywords
/// say nothing about `null` (an untyped object or array, `{}`, a branch of scalar keywords alone)
/// generates the non-null type every other spelling of it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NullAdmission {
    /// `null` fails the schema: a `type`, `enum` or `const` leaves it out, a `false` schema, or a
    /// `oneOf` in which `null` matches no branch or more than one.
    Denies,
    /// `null` satisfies the schema, and nothing in it states `null`.
    Undecided,
    /// `null` satisfies the schema, and some keyword it is held to states `null`.
    Admits,
}

impl NullAdmission {
    /// The conjunction of two constraints held at one location: `null` fails it where it fails
    /// either, and is stated where either states it.
    pub(super) fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Denies, _) | (_, Self::Denies) => Self::Denies,
            (Self::Admits, _) | (_, Self::Admits) => Self::Admits,
            (Self::Undecided, Self::Undecided) => Self::Undecided,
        }
    }

    /// Whether `null` is valid against the schema: a branch `null` matches.
    pub(super) fn matches(self) -> bool {
        self != Self::Denies
    }

    /// A schema's own `type`, `enum` and `const`, in conjunction: each that it states admits
    /// `null` only where it lists it, and one that states nothing decides nothing.
    pub(super) fn of_own_keywords(schema: &Schema) -> Self {
        let stated = |listed: bool| if listed { Self::Admits } else { Self::Denies };
        let type_admission = stated_nullability(schema).map_or(Self::Undecided, stated);
        let enum_admission = schema
            .enum_values
            .as_ref()
            .map_or(Self::Undecided, |values| {
                stated(values.iter().any(|value| matches!(value.node, Node::Null)))
            });
        let const_admission = schema
            .const_value
            .as_ref()
            .map_or(Self::Undecided, |value| {
                stated(matches!(value.node, Node::Null))
            });
        type_admission.and(enum_admission).and(const_admission)
    }

    /// A `oneOf` of branches each read as `branches` yields: `null` satisfies it only where it
    /// matches exactly one branch, which then says whether `null` is stated.
    pub(super) fn one_of(branches: impl IntoIterator<Item = Self>) -> Self {
        let mut matching = branches.into_iter().filter(|branch| branch.matches());
        match (matching.next(), matching.next()) {
            (Some(only), None) => only,
            _ => Self::Denies,
        }
    }

    /// An `anyOf` of branches each read as `branches` yields: `null` satisfies it where it matches
    /// at least one branch, and is stated where a matching branch states it.
    pub(super) fn any_of(branches: impl IntoIterator<Item = Self>) -> Self {
        branches
            .into_iter()
            .fold(Self::Denies, |answer, branch| match (answer, branch) {
                (Self::Admits, _) | (_, Self::Admits) => Self::Admits,
                (Self::Undecided, _) | (_, Self::Undecided) => Self::Undecided,
                (Self::Denies, Self::Denies) => Self::Denies,
            })
    }
}

/// What [`LowerCtx::clear_variant_nulls`] left of a union.
#[derive(Debug, Clone, Copy)]
enum Settled {
    /// The type keeps its kind: no union, or a union with two or more variants left.
    Kept,
    /// The union lost variants down to one, whose kind it now is; the variant's own `Ty`.
    Collapsed(Ty),
    /// The union lost every variant, and is kept as it was.
    Emptied,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// The single `null` decision for `schema` ([`NullAdmission`]), from the conjunction of every
    /// constraint that applies at its location: its own `type`, `enum` and `const`
    /// ([`NullAdmission::of_own_keywords`]); its `$ref` target, read through
    /// [`Self::ref_target_body`] and met with the `$ref`'s siblings; every `allOf` member, nested
    /// `allOf`s and `$ref`s flattened alike; and its `oneOf` (exactly one branch `null` matches) and
    /// `anyOf` (at least one). Each branch is read on its own: conjoining it with its siblings
    /// first changes neither whether `null` matches it (where a sibling denies `null`, the whole
    /// schema does) nor whether `null` is stated, so the answer is the same however the
    /// constraints are spelled — a `type` beside a union, an `allOf` member in either order, a
    /// nested `allOf`, or a `$ref` to any of them.
    ///
    /// Keywords that never constrain a `null` instance (object, array, string and number
    /// keywords) decide nothing. Neither do `not` and `if`/`then`/`else`, which lowering does not
    /// carry. A `true` schema decides nothing, and `false` denies. A `$ref` target that cannot be
    /// read, a `$ref` loop, or a chain past [`MAX_SCHEMA_DEPTH`] decides nothing.
    ///
    /// One spelling departs from 2020-12: a `type` naming only `null` beside an `allOf` that
    /// refuses `null`, with no union, `$ref`, `enum` or `const` beside them, admits no value, and
    /// is read as admitting `null` instead — the `allOf`'s type or `null` — wherever the walk
    /// reaches it, so a `$ref` to it, an `allOf` of it and a union branch of it agree with it.
    pub(super) fn null_admission(&self, schema: &Schema) -> NullAdmission {
        self.null_admission_within(schema, 0, None)
    }

    /// [`Self::null_admission`] of a union branch or `allOf` member, a boolean schema included.
    pub(super) fn branch_null_admission(&self, branch: &SchemaOr) -> NullAdmission {
        self.branch_null_admission_within(branch, 0, None)
    }

    /// [`Self::null_admission`] of `schema` with the `allOf` member at `member` (nested in inner
    /// `allOf`s or not) read as `true`: what the other members hold that member to.
    pub(super) fn null_admission_without(
        &self,
        schema: &Schema,
        member: &Provenance,
    ) -> NullAdmission {
        self.null_admission_within(schema, 0, Some(member))
    }

    /// Whether [`Self::null_admission`] admits `null` at `schema` only by its one departure: a
    /// `type` naming only `null` beside an `allOf` whose members refuse it, read as the `allOf`'s
    /// type or `null`. That `null` makes the members' type optional and is never the whole type,
    /// so members that share no value leave the schema `E013`.
    pub(super) fn null_admitted_only_by_departure(&self, schema: &Schema) -> bool {
        type_beside_all_of_is_null_only(schema)
            && schema
                .all_of
                .iter()
                .map(|member| self.branch_null_admission(member))
                .fold(NullAdmission::Undecided, NullAdmission::and)
                == NullAdmission::Denies
    }

    /// [`Self::null_admission`] of the keywords a union's branches are met with: `schema` without
    /// its `oneOf`, `anyOf` and `discriminator`.
    pub(super) fn union_sibling_null_admission(&self, schema: &Schema) -> NullAdmission {
        let mut sibling = schema.clone();
        sibling.clear_union_keywords();
        self.null_admission(&sibling)
    }

    /// [`Self::null_admission`] of a union branch or `allOf` member, a boolean schema included,
    /// reached `depth` steps into the walk.
    fn branch_null_admission_within(
        &self,
        branch: &SchemaOr,
        depth: u32,
        except: Option<&Provenance>,
    ) -> NullAdmission {
        match branch {
            SchemaOr::Bool(true) => NullAdmission::Undecided,
            SchemaOr::Bool(false) => NullAdmission::Denies,
            SchemaOr::Schema(schema) => self.null_admission_within(schema, depth, except),
        }
    }

    /// Apply [`Self::null_admission`] of `schema` to `ty`, the type its composition lowered to
    /// (everything inserted since `mark` is that lowering's own): the generated type admits `null`
    /// exactly where the schema states it. A schema with no composition (`allOf`, `oneOf`,
    /// `anyOf`, a `$ref` with shape-bearing siblings) and no `enum` or `const` decides by its own
    /// `type`, which its lowering already read; a bare `$ref` is its target's type, which the
    /// target's own lowering settled; and the exact null type `()` is `null` itself.
    ///
    /// The answer is applied, not re-derived from the lowered shape:
    ///
    /// * where `schema` denies `null`, nothing in the type decodes it: the type is not `Option`, a
    ///   union variant of this lowering's own that is the exact null type accepts nothing the
    ///   schema does and is no variant, and no variant is an `Option`. A union left with one
    ///   variant is that variant's type, and one left with none — the exact null type was all the
    ///   lowering found — admits no value at all, which is rejected (`E013`);
    /// * where `null` satisfies it unstated, the type is not `Option` and no variant is;
    /// * where it admits `null`, the type carries it once: as a `()` variant, which wins over
    ///   every `Option` (a union holding one is not made `Option` and does not stay one, and no
    ///   other variant stays one); as the `Option` of the union, which an `anyOf` of this
    ///   lowering's own also takes from a variant that accepts `null`; or as the `Option` of the
    ///   one merged `oneOf` variant `null` matches. It is made `Option` where none of them does.
    ///
    /// A `type` naming only `null` beside an `allOf` whose members lowered to a non-null type,
    /// which admits no value under 2020-12, is applied as the decision reads it: that type or
    /// `null` (`Option`).
    ///
    /// Only types this lowering inserted (at or above `mark`), and no component among them, are
    /// changed in place.
    pub(super) fn settle_null_admission(
        &mut self,
        schema: &Schema,
        mut ty: Ty,
        mark: u32,
    ) -> Option<Ty> {
        if schema.boolean.is_some() || !schema_is_composed(schema) {
            return Some(ty);
        }
        let admission = self.null_admission(schema);
        // A union of this lowering's own whose every variant is the exact null type accepts `null`
        // alone: it is the null type, as the sibling spelling collapses such an `anyOf` (#625).
        if ty.id.0 >= mark && !self.is_component_def(ty.id) {
            if let Some(TypeKind::Union(union)) = self.graph.get(ty.id).map(|def| &def.kind) {
                if union
                    .variants
                    .iter()
                    .all(|variant| self.is_exact_null(variant.ty))
                {
                    let dropped: Vec<u32> = union.variants.iter().map(|v| v.ty.id.0).collect();
                    if let Some(def) = self.graph.get_mut(ty.id) {
                        def.kind = TypeKind::Null;
                    }
                    self.union_variant_members.remove(&ty.id);
                    for id in dropped.into_iter().filter(|id| *id >= mark) {
                        self.elide_unused_union_lowering(id..id + 1);
                    }
                }
            }
        }
        if self.is_exact_null(ty) {
            if admission == NullAdmission::Denies {
                return self.reject_null_admission_denied(schema);
            }
            // `null` is its one value, so it is no `Option`.
            ty.nullable = false;
            return Some(ty);
        }
        match admission {
            NullAdmission::Admits => {
                if self.has_exact_null_variant(ty) {
                    // A `()` arm carries the `null`, so neither the union nor another variant is
                    // an `Option` as well, as an `allOf` spelling of the same constraints settles
                    // it (Decision 3).
                    ty.nullable = false;
                    self.clear_variant_nulls(ty.id, mark, false);
                } else if ty.nullable || self.any_of_variant_carries_null(ty, mark) {
                    // The union's `Option` carries the `null`. An `anyOf` of this lowering's own
                    // takes it from a variant that accepts it, since one match admits it.
                    ty.nullable = true;
                    self.clear_variant_nulls(ty.id, mark, false);
                } else if !self.carries_null(ty) {
                    ty.nullable = true;
                }
            }
            NullAdmission::Undecided => {
                ty.nullable = false;
                self.clear_variant_nulls(ty.id, mark, false);
            }
            NullAdmission::Denies => {
                ty.nullable = false;
                match self.clear_variant_nulls(ty.id, mark, true) {
                    Settled::Kept => {}
                    Settled::Collapsed(variant) => ty.boxed = variant.boxed,
                    Settled::Emptied => return self.reject_null_admission_denied(schema),
                }
            }
        }
        Some(ty)
    }

    /// Clear the `null` the variants of the union `id` decode, where `id` is a union this lowering
    /// inserted (at or above `mark`) and no component: each variant's `Option`, and, where
    /// `drop_exact_nulls`, the variants that are the exact null type, which then accept nothing.
    /// Nested unions of this lowering's own are cleared alike. A union left with one variant
    /// becomes that variant's kind in place; one left with none is [`Settled::Emptied`] and kept.
    fn clear_variant_nulls(&mut self, id: TypeId, mark: u32, drop_exact_nulls: bool) -> Settled {
        if id.0 < mark || self.is_component_def(id) {
            return Settled::Kept;
        }
        let Some(TypeKind::Union(union)) = self.graph.get(id).map(|def| def.kind.clone()) else {
            return Settled::Kept;
        };
        let mut retained = Vec::new();
        let mut variants = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            if drop_exact_nulls && self.is_exact_null(variant.ty) {
                continue;
            }
            let mut variant = variant.clone();
            if let Settled::Collapsed(inner) =
                self.clear_variant_nulls(variant.ty.id, mark, drop_exact_nulls)
            {
                variant.ty.boxed = variant.ty.boxed || inner.boxed;
            }
            variant.ty.nullable = false;
            variants.push(variant);
            retained.push(index);
        }
        let dropped: Vec<TypeId> = union
            .variants
            .iter()
            .enumerate()
            .filter(|(index, _)| !retained.contains(index))
            .map(|(_, variant)| variant.ty.id)
            .collect();
        let settled = match variants.len() {
            0 => return Settled::Emptied,
            // A variant still a reservation has no kind to take yet, so it stays a variant.
            1 if !dropped.is_empty() && !self.is_reservation(variants[0].ty.id) => {
                let variant = variants[0].ty;
                let Some(kind) = self.graph.get(variant.id).map(|def| def.kind.clone()) else {
                    return Settled::Kept;
                };
                if let Some(def) = self.graph.get_mut(id) {
                    def.kind = kind;
                }
                Settled::Collapsed(variant)
            }
            _ => {
                let strategy = retain_strategy(&union.strategy, &retained);
                if let Some(def) = self.graph.get_mut(id) {
                    def.kind = TypeKind::Union(Union { variants, strategy });
                }
                Settled::Kept
            }
        };
        if dropped.is_empty() {
            return settled;
        }
        // The variants' members stay aligned with the variants left; a union no longer has any.
        if let Some(members) = self.union_variant_members.remove(&id) {
            if matches!(settled, Settled::Kept) {
                let members = retained
                    .iter()
                    .filter_map(|&index| members.get(index).cloned())
                    .collect();
                self.union_variant_members.insert(id, members);
            }
        }
        // A dropped variant's type, and a collapsed one's (its kind now the union's own), are
        // emitted unused unless something else names them.
        let collapsed = match settled {
            Settled::Collapsed(variant) => Some(variant.id),
            Settled::Kept | Settled::Emptied => None,
        };
        for unused in dropped.into_iter().chain(collapsed) {
            if unused.0 >= mark {
                self.elide_unused_union_lowering(unused.0..unused.0 + 1);
            }
        }
        settled
    }

    /// Whether `id` is the type of a component, a remote or a resolved target, finished or still
    /// being lowered: a type other uses share, which one use's lowering does not change.
    fn is_component_def(&self, id: TypeId) -> bool {
        self.reservation_roots().any(|component| component == id)
    }

    /// Whether `ty` is an `anyOf` union this lowering inserted (at or above `mark`), no component,
    /// with a variant that accepts `null` ([`Self::carries_null`]).
    fn any_of_variant_carries_null(&self, ty: Ty, mark: u32) -> bool {
        ty.id.0 >= mark
            && !self.is_component_def(ty.id)
            && matches!(
                self.graph.get(ty.id).map(|def| &def.kind),
                Some(TypeKind::Union(Union {
                    strategy: UnionStrategy::Trial {
                        mode: UnionMode::AnyOf,
                        ..
                    },
                    ..
                }))
            )
            && self.carries_null(ty)
    }

    /// Whether `ty` is a union with a variant that is the exact null type (a `()` arm).
    fn has_exact_null_variant(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Union(union))
                if union.variants.iter().any(|variant| self.is_exact_null(variant.ty))
        )
    }

    /// Whether `ty` already accepts `null` without being made nullable: it is, or it is a union
    /// with a variant that decodes `null` — an exact-null (`()`) one, or one a merge left
    /// nullable, where the one merged branch `null` matches is its only source.
    fn carries_null(&self, ty: Ty) -> bool {
        ty.nullable
            || match self.graph.get(ty.id).map(|def| &def.kind) {
                Some(TypeKind::Null) => true,
                Some(TypeKind::Union(union)) => union
                    .variants
                    .iter()
                    .any(|variant| variant.ty.nullable || self.is_exact_null(variant.ty)),
                // A reservation's body is not known yet: its reference's own nullability stands.
                Some(TypeKind::Reserved) => false,
                _ => false,
            }
    }

    /// [`Self::null_admission`] reached `depth` steps into the walk.
    fn null_admission_within(
        &self,
        schema: &Schema,
        depth: u32,
        except: Option<&Provenance>,
    ) -> NullAdmission {
        match schema.boolean {
            Some(true) => return NullAdmission::Undecided,
            Some(false) => return NullAdmission::Denies,
            None => {}
        }
        if depth >= MAX_SCHEMA_DEPTH || except == Some(&schema.provenance) {
            return NullAdmission::Undecided;
        }
        let mut answer = NullAdmission::of_own_keywords(schema);
        if let Some(reference) = &schema.reference {
            let target = self.ref_target_body(reference, &schema.provenance).map_or(
                NullAdmission::Undecided,
                |target| {
                    memoised_answer(
                        &self.null_admission_memo,
                        &target,
                        NullAdmission::Undecided,
                        || self.null_admission_within(&target, depth + 1, None),
                    )
                },
            );
            answer = answer.and(target);
        }
        for member in &schema.all_of {
            answer = answer.and(self.branch_null_admission_within(member, depth + 1, except));
        }
        if !schema.one_of.is_empty() {
            answer =
                answer.and(NullAdmission::one_of(schema.one_of.iter().map(|branch| {
                    self.branch_null_admission_within(branch, depth + 1, except)
                })));
        }
        if !schema.any_of.is_empty() {
            answer =
                answer.and(NullAdmission::any_of(schema.any_of.iter().map(|branch| {
                    self.branch_null_admission_within(branch, depth + 1, except)
                })));
        }
        // A `type` naming only `null` beside an `allOf` that refuses it (`{allOf: [{$ref: User}],
        // type: ['null']}`, GitHub's spelling of a nullable `$ref`) is satisfied by no value under
        // 2020-12, yet a non-null type would refuse the `null` such an API sends, and `E013` would
        // refuse the whole description: it is read as the `allOf`'s type or `null`, as before the
        // one decision, wherever it is reached — the schema itself, and every `$ref`, `allOf`
        // member and union branch that composes it.
        if answer == NullAdmission::Denies && type_beside_all_of_is_null_only(schema) {
            return NullAdmission::Admits;
        }
        answer
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
    /// non-null struct, `Vec` or tuple decides nothing about `null`: the one decision leaves it
    /// [`NullAdmission::Undecided`] ([`Self::null_admission`]), so `null` satisfies it wherever the
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
        !ty.nullable && undecided_kind && self.null_admission(member) == NullAdmission::Undecided
    }

    /// Whether `member`, a union branch lowered to `ty` that hoisted no `null`, is a branch `null`
    /// matches although nothing in it states `null`, as a `oneOf` counts it beside a branch that
    /// does: an untyped object or array ([`Self::branch_leaves_null_undecided`], #622), or a
    /// nested union `null` matches ([`Self::union_matches_uncarried_null`]), written inline or as
    /// a bare `$ref` to a union component (#628).
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
            return self.union_matches_uncarried_null(member, ty);
        }
        let Some(reference) = member.reference.as_deref() else {
            return false;
        };
        // A `$ref` with siblings of its own is an intersection this does not read.
        !ref_siblings_bear_shape(member)
            && self
                .ref_target_body(reference, &member.provenance)
                .is_some_and(|target| {
                    schema_has_union(&target) && self.union_matches_uncarried_null(member, ty)
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
    pub(super) fn untyped_shape_category(&self, schema: &Schema, depth: u32) -> Option<JsonType> {
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
                        if ref_siblings_bear_shape(member) {
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
    /// union `null` matches ([`Self::union_matches_uncarried_null`]).
    pub(super) fn branch_takes_permitted_null(&self, member: &SchemaOr, ty: Ty) -> bool {
        // A nested union is read by the one decision, whatever it lowered to: one that is `Value`
        // can still be a `oneOf` that `null` matches twice.
        if let SchemaOr::Schema(member) = member {
            if schema_has_union(member) {
                return self.union_matches_uncarried_null(member, ty);
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
        !ty.nullable
            && self.is_reservation(ty.id)
            && self.null_admission(member) == NullAdmission::Undecided
    }

    /// Whether `schema`, a union or a `$ref` to one, lowered to `ty`, is one `null` matches by the
    /// one decision ([`Self::null_admission`]) although `ty` does not carry it
    /// ([`Self::carries_null`]): its branches leave `null` undecided, so its lowering is no
    /// `Option` and has no `()` arm, yet `null` satisfies it, as untyped `items` branches of an
    /// `anyOf` do.
    fn union_matches_uncarried_null(&self, schema: &Schema, ty: Ty) -> bool {
        self.null_admission(schema).matches() && !self.carries_null(ty)
    }

    /// Whether `schema` is an `allOf`, with no `$ref` or union of its own, one of whose members is
    /// a union, inline or as a bare `$ref` to it, or such an `allOf` in turn: `Wrap: { allOf:
    /// [ { $ref: U } ] }` admits exactly the values `U` does, so a `$ref` member to `Wrap`, or a
    /// non-component pointer to such an `allOf`, takes `null` as a `$ref` member to `U` does
    /// (#638). `depth` bounds the walk; a chain past it answers `false`. Each `$ref` member's
    /// target is read once per pass and its answer replayed ([`Self::wraps_union_memo`]).
    fn all_of_wraps_union(&self, schema: &Schema, depth: u32) -> bool {
        if depth >= MAX_SCHEMA_DEPTH
            || schema.all_of.is_empty()
            || schema.reference.is_some()
            || schema_has_union(schema)
        {
            return false;
        }
        schema.all_of.iter().any(|member| {
            let SchemaOr::Schema(member) = member else {
                return false;
            };
            match member.reference.as_deref() {
                // A `$ref` member with siblings of its own is an intersection this does not read.
                Some(reference) => {
                    !ref_siblings_bear_shape(member)
                        && self
                            .ref_target_body(reference, &member.provenance)
                            .is_some_and(|body| {
                                schema_has_union(&body)
                                    || memoised_answer(&self.wraps_union_memo, &body, false, || {
                                        self.all_of_wraps_union(&body, depth + 1)
                                    })
                            })
                }
                None => schema_has_union(member) || self.all_of_wraps_union(member, depth + 1),
            }
        })
    }

    /// Whether the schema the `$ref` written at `at` names ([`Self::ref_target_body`]) decides its
    /// own nullability in an `allOf` merge (issue #541): the one decision ([`Self::null_admission`])
    /// of its body admits or denies `null`. A body that leaves it undecided — an untyped object, an
    /// `allOf` of untyped object members alone (#565) — lowers to the non-null struct
    /// [`object_all_of_admits_null`] gives a merge no member decides, and that struct is no
    /// decision, as the same members written inline make none. A target that cannot be read keeps
    /// its lowered nullability as the decision, and reports nothing a second time.
    pub(super) fn ref_target_decides_null(&self, reference: &str, at: &Provenance) -> bool {
        self.ref_target_body(reference, at)
            .is_none_or(|target| self.null_admission(&target) != NullAdmission::Undecided)
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
            if ref_siblings_bear_shape(&target) {
                return Some(target);
            }
            reference.clone_from(next);
            at = target.provenance.clone();
        }
        None
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
    /// the `$ref`-sibling spelling of the same conjunction lowers to `()`; rejecting it here would
    /// split the spellings of one conjunction (#542), as a nullable union refined to nothing but
    /// `null` is not split either (#450). The meets inserted since `mark` reach nothing the null
    /// type refers to, so they are discarded, and the null type is the final graph insert.
    ///
    /// [`schema_is_object_like`]: super::shape::schema_is_object_like
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

    /// [`own_keywords_admit_null`] over an `allOf` schema and every inline schema among its
    /// members: each member, as a `$ref`'s siblings or as the resolved target of a non-component
    /// `$ref` member, and, where [`Self::gather_member`] flattens it into the meet (a member with
    /// an `allOf` and no union beside it, which [`Self::gather_ref_target`] expands in place
    /// rather than lowering when it is a `$ref`'s target), its own members in turn. Flattening
    /// gathers only such a schema's members and object keywords, so its own `type`, `enum` or
    /// `const` reaches no contribution, yet still constrains every value of the meet (#569). A
    /// member lowered as a contribution of its own is read by the same conjunction, since its
    /// lowering does not: `{ type: [string, 'null'], enum: [a] }` lowers to a nullable enum though
    /// its `enum` refuses `null`, and a union member beside it would take that `null` (#649). A
    /// component or remote `$ref` lowers to a type, which carries its nullability already.
    pub(super) fn flattened_keywords_admit_null(&self, schema: &Schema) -> bool {
        own_keywords_admit_null(schema)
            && self.flattened_members_admit_null(schema, &mut HashSet::new(), 0)
    }

    /// Whether every inline schema among `schema`'s `allOf` members admits `null` by its own
    /// keywords, descending into the members [`Self::gather_member`] flattens into the meet:
    /// [`Self::flattened_keywords_admit_null`] below the schema itself.
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

    /// [`Self::flattened_keywords_admit_null`] of one `allOf` member: [`own_keywords_admit_null`]
    /// over the member itself (its siblings, for a `$ref`, and the resolved target of a
    /// non-component `$ref`), whether or not it is flattened, then over its own members where
    /// [`Self::gather_member`] flattens it. The answer is the conjunction of every such inline
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
        own_keywords_admit_null(member)
            && (member.all_of.is_empty()
                || schema_has_union(member)
                || self.flattened_members_admit_null(member, visited, depth + 1))
    }

    /// `composed`, an object composition met with a union beside it
    /// ([`LowerCtx::lower_all_of_beside_union`], [`LowerCtx::lower_all_of_with_union_member`]),
    /// made to admit `null` where no member decides it (issue #541) and the one decision
    /// ([`Self::null_admission`]) admits it for `schema`, the whole conjunction. Its members are
    /// then untyped objects alone — `$ref`s to untyped object components, which
    /// [`object_all_of_admits_null`] reads as denying `null` for want of a decision — and the
    /// same members written inline are scoped refiners that leave `null` to the union, so the
    /// union's answer is the meet's in either spelling. A composition some member decides, or a
    /// scalar one, keeps its nullability.
    pub(super) fn composition_takes_union_null(
        &self,
        mut composed: Ty,
        has_object: bool,
        contributions: &[Contribution],
        schema: &Schema,
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
        if has_object && !decided && self.null_admission(schema) == NullAdmission::Admits {
            composed.nullable = true;
        }
        composed
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
    /// where the union matches it although `ty` does not carry it
    /// ([`Self::union_matches_uncarried_null`]), such as untyped `items` branches of an `anyOf`,
    /// so the meet admits `null` exactly where every other member does (#621). Lowered on its own,
    /// such a union is no `Option` for want of a decision, and read that way the meet would deny
    /// `null` even beside `type: [array, 'null']`, where the object spelling's meet keeps it. A
    /// member that lowers to `Value` decides nothing, so with no member of any other type the union
    /// keeps the answer it has on its own. `union` may also be an `allOf` wrapping such a union,
    /// which a `$ref` member names ([`Self::all_of_wraps_union`], #638).
    fn union_member_takes_scalar_null<'c>(
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
        if decided && self.union_matches_uncarried_null(union, ty) {
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
    /// ([`Self::all_of_wraps_union`], #638), keeps its contribution. A wrapping component is
    /// gathered as the one scalar it lowers to, and a wrapping non-component target as the one
    /// scalar its wrapped union lowers to once [`Self::gather_ref_target`] expands it in place.
    /// An inline union's own keywords are its siblings, read by the one decision with it.
    ///
    /// Another such member that `null` matches without carrying it is not one of the members that
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
                        if ref_siblings_bear_shape(member) {
                            return None;
                        }
                        self.ref_target_body(reference, &member.provenance)?
                    }
                    None => std::borrow::Cow::Borrowed(member),
                };
                (schema_has_union(&target) || self.all_of_wraps_union(&target, 0))
                    .then_some((slot, *ty, target))
            })
            .collect();
        let undecided: Vec<usize> = unions
            .iter()
            .filter(|(_, ty, target)| self.union_matches_uncarried_null(target, *ty))
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

/// Whether `schema` composes constraints whose `null` its own `type` alone does not settle: an
/// `allOf`, a `oneOf` or `anyOf`, a `$ref` with shape-bearing siblings, or an `enum` or `const`.
fn schema_is_composed(schema: &Schema) -> bool {
    let ref_with_siblings = schema.reference.is_some() && ref_siblings_bear_shape(schema);
    ref_with_siblings
        || (schema.reference.is_none()
            && (!schema.all_of.is_empty()
                || schema_has_union(schema)
                || schema.enum_values.is_some()
                || schema.const_value.is_some()))
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

/// Whether a branch of `union` admits `null` by its own keywords: one with no `$ref`, `allOf` or
/// union of its own whose stated `type`, `enum` or `const` admits it, such as
/// `type: [object, 'null']` or `type: 'null'` (#586). An untyped conjunct leaves `null` to the
/// union, so where such a branch admits it the conjunct does too, as
/// [`LowerCtx::composition_takes_union_null`] makes an untyped `allOf` composition admit it: the `$ref`-sibling spelling's untyped target
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

/// Whether a schema's own `type` admits `null`: `None` for an untyped schema, which states no
/// category and so decides nothing about `null` in an `allOf` merge.
pub(super) fn stated_nullability(schema: &Schema) -> Option<bool> {
    (!schema.types.types.is_empty()).then(|| schema.types.types.contains(&JsonType::Null))
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
        && !schema_has_union(schema)
        && schema.all_of.is_empty()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.properties.is_empty()
}

/// Whether `schema` is an `allOf` beside a `type` that names only `null`, and no union or `$ref`:
/// the spelling [`LowerCtx::null_admission`] reads as the `allOf`'s type or `null`.
fn type_beside_all_of_is_null_only(schema: &Schema) -> bool {
    schema.types.types == [JsonType::Null]
        && !schema.all_of.is_empty()
        && schema.reference.is_none()
        && !schema_has_union(schema)
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
}

/// Whether every one of a schema's own `type`, `enum` and `const` that it states admits `null`
/// (vacuously so where it states none): their conjunction, [`NullAdmission::of_own_keywords`],
/// so `{type: string, enum: [null]}` admits no `null`.
fn own_keywords_admit_null(schema: &Schema) -> bool {
    NullAdmission::of_own_keywords(schema).matches()
}
