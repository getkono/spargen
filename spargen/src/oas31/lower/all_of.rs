//! `allOf` lowering and a `$ref` with sibling keywords: the entry points, and meeting an
//! `allOf` or `$ref` with a union beside or inside it.

use std::collections::HashSet;
use std::ops::Range;

use crate::diag::Provenance;
use crate::ir::{Docs, Ty, TypeKind, UnionVariant};
use crate::oas31::{Schema, SchemaOr};

use super::combine::{schema_has_union, take_refiners, Contribution};
use super::meet::same_ty;
use super::nullability::{
    member_is_null_only, undecided_admits_null, union_branch_admits_null, NullAdmission,
};
use super::refiner::implied_applicator_category;
use super::{HeldUntypedWarning, LowerCtx, MetUnion, Refiner};

/// A union an `allOf` meet met with its other members, for
/// [`LowerCtx::warn_excluded_union_member_branches`].
pub(super) struct MetUnionMember {
    /// The type the union lowered to.
    pub(super) union: crate::ir::TypeId,
    /// The union's schema: the member itself, or a `$ref` member's target body.
    pub(super) schema: Schema,
    /// What the members the union is met with hold `null` to ([`LowerCtx::null_admission`]).
    pub(super) conjunct: NullAdmission,
    /// Whether those members meet in the exact null type, which keeps no branch's non-null values.
    pub(super) conjunct_is_null: bool,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Merge an `allOf` composition (plus the enclosing schema's own sibling
    /// `properties`/`required`/`additionalProperties`) into a single typed [`TypeKind`].
    ///
    /// Members are gathered in a deterministic order — every `allOf` entry in source order, then the
    /// enclosing schema's own object siblings — flattening `$ref` members by *copying* their fields
    /// (the referenced component still exists as its own named type) and recursing into nested
    /// `allOf`. A member that constrains nothing (`true`, `{}`, an annotation) contributes nothing;
    /// a `false` member, and a member that closes a reference cycle back to the schema being
    /// lowered, are `E013`. A `$ref` member's own shape-bearing siblings are a further member. The
    /// gathered members are then combined ([`Self::combine_all_of`]):
    ///
    /// * **no constraining member** → an open, field-less [`Struct`];
    /// * **all object members** → one flattened [`Struct`]: the union of properties in first-seen
    ///   order, recursive typed intersections for properties declared by several members (an empty
    ///   one types the field uninhabited unless some member requires it, which is `E013` — the rule
    ///   `intersect_structs` applies), the union of `required`, and the `additionalProperties`
    ///   policies merged by [`Self::merge_additional`], whose irreconcilable pair is `E013`;
    /// * **all scalar members** → their typed intersection, including numeric narrowing, enum
    ///   narrowing, arrays/objects/unions, and exact nullability; no typed intersection → `E013`;
    ///   a member of untyped `items`/`prefixItems` alone then refines that intersection's arrays
    ///   (with no scalar member, it establishes the array), and beside object members it
    ///   constrains nothing (#607);
    /// * an **object/scalar mix** → `E013`.
    ///
    /// Each `E013` returns `None`, as does a member that fails to lower for its own reason.
    ///
    /// Every path inserts its result type as the *final* graph insert (all member/property/component
    /// types insert first), so an `allOf` used as a component body still satisfies the
    /// [`Self::ensure_component`] last-insert invariant.
    ///
    /// [`Struct`]: crate::ir::Struct
    pub(super) fn lower_all_of(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let mut contributions = Vec::new();
        // The union members gathered as the one scalar they lower to, by where it went, and the
        // ids each inline one's own lowering inserted.
        let mut union_members = Vec::new();
        let mut lowerings = Vec::new();
        let (gathered, held) = self.holding_untyped_warnings(|ctx| {
            ctx.in_composition(|ctx| {
                ctx.gather_all_of_recording_unions(
                    schema,
                    hint,
                    &mut contributions,
                    &mut union_members,
                    &mut lowerings,
                )
            })
        });
        if gathered.is_none() {
            self.report_held_untyped_warnings(held);
            return None;
        }
        // A `$ref` to a union takes `null` from the other members exactly where the same union
        // written inline does ([`Self::lower_all_of_with_union_member`]): lowered as its own
        // component, it denied `null` for want of a decision, and the meet denied it even beside
        // `type: [array, 'null']` (#624). One nested in an inner `allOf` is a member of this
        // merge as well, flattened into it, so it takes `null` alike (#630). So does each of
        // several inline unions, which reach this arm rather than the sole-union one and were
        // each gathered as the non-null scalar it lowers to alone (#631), and so does an inline
        // union nested in an inner `allOf` flattened into this one (#642). Only where `null`
        // can satisfy the merge at all: the own `type`, `enum` or `const` of this schema, or of
        // an `allOf` flattened into it, contributes nothing to the meet yet constrains every
        // value of it, so one that excludes `null` leaves no `null` to take
        // ([`Self::combine_scalar_union_members`] takes it).
        self.combine_scalar_union_members(
            schema,
            hint,
            contributions,
            &union_members,
            held,
            lowerings,
        )
    }

    /// [`Self::combine_all_of`] of `contributions`, where `union_members` records the union
    /// members gathered as one scalar (as [`Self::lower_all_of`] records them) and `held` the
    /// `W001`s the gather held back ([`Self::holding_untyped_warnings`]). Each union member's
    /// `null` is decided here by the union's own count
    /// ([`Self::union_members_take_scalar_null`]), where `null` can satisfy the merge at all, so
    /// the copy the meet hands its `Value` variants is cleared
    /// ([`Self::scalar_meet_value_variants`]), and an inline union member's `W001` is raised for
    /// the variants the meet leaves `Value` alone: met with `type: [string, 'null']`, a branch that
    /// states nothing is a `string` branch, as the sibling spelling lowers it (#660).
    ///
    /// A union whose own lowering already took `null` from a branch that states it
    /// (`[ {}, { type: [string, 'null'] } ]`) holds that branch's `null`, not its `Value`
    /// variant's: `null` matches both branches, so the copy is the second match the `oneOf`
    /// collapse must count to deny `null`, as the sibling spelling does, and it stays. So it does
    /// beside a variant that accepts `null` itself, such as an exact-null branch's `()`
    /// (`[ {}, { const: null } ]`, or a `$ref` to such a component).
    fn combine_scalar_union_members(
        &mut self,
        schema: &Schema,
        hint: &str,
        mut contributions: Vec<Contribution>,
        union_members: &[(usize, &Schema)],
        held: Vec<HeldUntypedWarning>,
        lowerings: Vec<(Provenance, Range<u32>)>,
    ) -> Option<Ty> {
        let mut value_variants = HashSet::new();
        for (slot, _) in union_members {
            let Some(Contribution::Scalar(ty)) = contributions.get(*slot) else {
                continue;
            };
            if ty.nullable {
                continue;
            }
            let Some(TypeKind::Union(union)) = self.graph.get(ty.id).map(|def| &def.kind) else {
                continue;
            };
            let is_value = |variant: &UnionVariant| {
                matches!(
                    self.graph.get(variant.ty.id).map(|def| &def.kind),
                    Some(TypeKind::Any)
                )
            };
            if union
                .variants
                .iter()
                .any(|variant| !is_value(variant) && self.ty_accepts_null(variant.ty))
            {
                continue;
            }
            for variant in union.variants.iter().filter(|variant| is_value(variant)) {
                value_variants.insert(variant.name_hint.clone());
            }
        }
        if self.flattened_keywords_admit_null(schema) {
            self.union_members_take_scalar_null(union_members, &mut contributions);
        }
        self.scalar_meet_value_variants = value_variants;
        let meets_from = self.variant_meets.len();
        let met = self.combine_all_of(schema, hint, &contributions);
        if met.is_some() {
            let unions = self.met_union_members(schema, union_members, &contributions);
            self.warn_excluded_union_member_branches(schema, &unions, meets_from);
        }
        let mut held = held;
        // A met reservation's body is not known yet, so nothing is settled against it.
        if let Some(met) = met.filter(|met| !self.is_reservation(met.id)) {
            let met_union = self.met_union_variants(met);
            for (provenance, lowered) in lowerings {
                // The member's own lowering is an input to the meet, emitted as a public type
                // nothing referred to once the meet replaced it, whatever the member order (#675).
                self.elide_unused_union_lowering(lowered.clone());
                let Some(index) = held
                    .iter()
                    .position(|warning| warning.provenance == provenance)
                else {
                    continue;
                };
                let Some(union_id) = union_members
                    .iter()
                    .find(|(_, member)| member.provenance == provenance)
                    .and_then(|(slot, _)| match contributions.get(*slot) {
                        Some(Contribution::Scalar(ty)) => Some(ty.id),
                        _ => None,
                    })
                else {
                    continue;
                };
                // Where the meet left one branch (no union) the union's variants are gone; otherwise
                // a variant the meet kept is carried at the top or nested under another union
                // member's branches, so the member order does not decide it. A variant the meet
                // carries nowhere was met into another member's branches, which typed it (#675).
                let carries = |hint: &str| {
                    met_union.as_ref().map_or(Some(None), |variants| {
                        variants
                            .iter()
                            .find(|variant| variant.name_hint == hint)
                            .map(Some)
                    })
                };
                // The union's own lowering is an input to the meet, emitted as a public type
                // nothing referred to once the meet replaced it, as the held-back union's lowering
                // is ([`Self::meet_union_with_all_of`]). Where something still names the union,
                // its `Value` variants are still emitted, and its warning stands as raised.
                self.elide_unused_union_lowering(lowered);
                if !self.graph.is_elided(union_id) {
                    continue;
                }
                let mut warning = held.remove(index);
                warning.untyped.retain(|(hint, _)| {
                    carries(hint).flatten().is_some_and(|variant| {
                        matches!(
                            self.graph.get(variant.ty.id).map(|def| &def.kind),
                            Some(TypeKind::Any)
                        )
                    })
                });
                self.report_untyped_one_of(warning);
            }
        }
        self.report_held_untyped_warnings(held);
        met
    }

    /// The union members of an `allOf`'s scalar meet, for
    /// [`Self::warn_excluded_union_member_branches`]: each member in `union_members` that is a
    /// union, inline or as a bare `$ref` to one, with the type it lowered to
    /// in `contributions` and what the other members hold it to.
    fn met_union_members(
        &self,
        schema: &Schema,
        union_members: &[(usize, &Schema)],
        contributions: &[Contribution],
    ) -> Vec<MetUnionMember> {
        union_members
            .iter()
            .filter_map(|&(slot, member)| {
                let Some(Contribution::Scalar(ty)) = contributions.get(slot) else {
                    return None;
                };
                let union = if schema_has_union(member) {
                    member.clone()
                } else {
                    let reference = member.reference.as_deref()?;
                    let target = self.ref_target_body(reference, &member.provenance)?;
                    schema_has_union(&target).then(|| target.into_owned())?
                };
                let conjunct_is_null = contributions.iter().enumerate().any(|(index, other)| {
                    index != slot
                        && matches!(other, Contribution::Scalar(other) if self.is_exact_null(*other))
                });
                Some(MetUnionMember {
                    union: ty.id,
                    schema: union,
                    conjunct: self.null_admission_without(schema, &member.provenance),
                    conjunct_is_null,
                })
            })
            .collect()
    }

    /// Report, as the sibling spelling does (`W011`), each branch of the union members `unions`
    /// that `schema`'s meet excludes (#671): one whose non-null values no meet
    /// [`Self::intersect_union`] recorded since `meets_from` kept — every record for it is
    /// excluded, or the members it is met with meet in the exact null type — and that `null`
    /// does not satisfy beside them either ([`Self::null_admission`]), so it admits no value at
    /// all. A branch met under several branches of another member is excluded only where none of
    /// those meets kept it. A branch that is the exact null type on its own (`const: null`) is
    /// the null decision's, and drops out unreported, as a `{type: 'null'}` member does in every
    /// spelling. The members a variant came from are read from [`Self::union_variant_members`],
    /// so the message names the positions the sibling spelling names.
    pub(super) fn warn_excluded_union_member_branches(
        &mut self,
        schema: &Schema,
        unions: &[MetUnionMember],
        meets_from: usize,
    ) {
        let mut reported = HashSet::new();
        for met in unions {
            let union = met.union;
            let Some(members) = self.union_variant_members.get(&union).cloned() else {
                continue;
            };
            let exact_null: Vec<bool> = match self.graph.get(union).map(|def| &def.kind) {
                Some(TypeKind::Union(lowered)) => lowered
                    .variants
                    .iter()
                    .map(|variant| self.is_exact_null(variant.ty))
                    .collect(),
                // A reservation's body is not known yet, so nothing is reported against it.
                Some(TypeKind::Reserved) => continue,
                _ => continue,
            };
            let branches: Vec<&SchemaOr> = met
                .schema
                .one_of
                .iter()
                .chain(&met.schema.any_of)
                .filter(|branch| !member_is_null_only(branch))
                .collect();
            let meets = &self.variant_meets[meets_from..];
            let mut excluded: Vec<usize> = Vec::new();
            for (variant, variant_members) in members.iter().enumerate() {
                let records: Vec<bool> = meets
                    .iter()
                    .filter(|(id, index, _)| *id == union && *index == variant)
                    .map(|(_, _, kept)| *kept)
                    .collect();
                let non_null_excluded = if met.conjunct_is_null {
                    true
                } else {
                    !records.is_empty() && !records.contains(&true)
                };
                if !non_null_excluded || exact_null.get(variant).copied().unwrap_or(true) {
                    continue;
                }
                for &member in variant_members {
                    let null_survives = branches.get(member).is_some_and(|branch| {
                        self.branch_null_admission(branch)
                            .and(met.conjunct)
                            .matches()
                    });
                    if !null_survives && reported.insert((union, member)) {
                        excluded.push(member);
                    }
                }
            }
            excluded.sort_unstable();
            for member in excluded {
                // W011 case: excluded-union-branch
                crate::diag::Diagnostic::warning(
                    crate::diag::Code::DeclarationHasNoEffect,
                    schema.provenance.clone(),
                )
                .message(format!(
                    "union member {member} cannot satisfy the enclosing schema's own \
                     constraints, so it is not a variant of the generated enum"
                ))
                .emit(self.diags);
            }
        }
    }

    /// Run `lower` with the `W001`s inline `oneOf`s raise for their `serde_json::Value` variants
    /// held back ([`Self::held_untyped_warnings`]), and return them beside its result for
    /// [`Self::combine_scalar_union_members`]. The enclosing hold, if any, is restored.
    fn holding_untyped_warnings<T>(
        &mut self,
        lower: impl FnOnce(&mut Self) -> T,
    ) -> (T, Vec<HeldUntypedWarning>) {
        let enclosing = self.held_untyped_warnings.replace(Vec::new());
        let result = lower(self);
        let held = std::mem::replace(&mut self.held_untyped_warnings, enclosing);
        (result, held.unwrap_or_default())
    }

    /// Report, as raised, the `W001`s `held` back while an `allOf` gathered its members.
    fn report_held_untyped_warnings(&mut self, held: Vec<HeldUntypedWarning>) {
        for warning in held {
            self.report_untyped_one_of(warning);
        }
    }

    /// `met`, an `allOf`'s scalar meet ([`Self::combine_all_of`]), with the `null` the meet handed
    /// the union variants `value_variants` names cleared ([`Self::scalar_meet_value_variants`]):
    /// each lowered to `Value`, the identity of the meet, so the meet gave it the other members'
    /// `null` a second time, and a `oneOf` collapsed after it read that copy as one more branch
    /// `null` matches. The unions the meet nested in `met`'s variants (a second union member met
    /// with each branch of the first) are cleared alike, since types inserted since `mark` are the
    /// meet's own. `met` is returned as it is where nothing is cleared.
    pub(super) fn clear_value_variant_null(
        &mut self,
        met: Ty,
        value_variants: &HashSet<String>,
        mark: u32,
    ) -> Ty {
        if value_variants.is_empty() || met.id.0 < mark {
            return met;
        }
        let Some(def) = self.graph.get(met.id) else {
            return met;
        };
        let TypeKind::Union(union) = &def.kind else {
            return met;
        };
        let hint = def.name_hint.clone();
        let mut union = union.clone();
        let mut changed = false;
        for variant in &mut union.variants {
            let cleared = self.clear_value_variant_null(variant.ty, value_variants, mark);
            if !same_ty(cleared, variant.ty) {
                variant.ty = cleared;
                changed = true;
            }
        }
        for variant in &mut union.variants {
            if variant.ty.nullable && value_variants.contains(&variant.name_hint) {
                variant.ty.nullable = false;
                changed = true;
            }
        }
        if !changed {
            return met;
        }
        let mut ty = self.insert_type(&hint, TypeKind::Union(union), Docs::default(), None);
        ty.nullable = met.nullable;
        ty.boxed = met.boxed;
        ty
    }

    /// Every variant of `met` where it is a union, and of each union nested in those variants:
    /// where a union member meets another union, its variants sit one level down, under the
    /// other's branches, whichever member is written first. `None` where `met` is no union.
    fn met_union_variants(&self, met: Ty) -> Option<Vec<UnionVariant>> {
        let Some(TypeKind::Union(_)) = self.graph.get(met.id).map(|def| &def.kind) else {
            return None;
        };
        let mut seen = HashSet::new();
        let mut pending = vec![met.id];
        let mut variants = Vec::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(TypeKind::Union(union)) = self.graph.get(id).map(|def| &def.kind) {
                for variant in &union.variants {
                    pending.push(variant.ty.id);
                    variants.push(variant.clone());
                }
            }
        }
        Some(variants)
    }

    /// [`Self::gather_all_of`] of `schema`, recording in `union_members` each `$ref` member
    /// gathered as the one scalar its target lowers to, and each inline union member gathered as
    /// the one scalar it lowers to (#631), by where it went, for
    /// [`Self::union_members_take_scalar_null`]. A member that is an inline `allOf` with no
    /// union of its own is flattened into the same contributions, as [`Self::gather_member`]
    /// flattens it, and its `$ref` and inline union members are recorded too: written there, they
    /// are members of the merge as much as written in `schema.all_of` (#630, #642).
    fn gather_all_of_recording_unions<'s>(
        &mut self,
        schema: &'s Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
        union_members: &mut Vec<(usize, &'s Schema)>,
        lowerings: &mut Vec<(Provenance, Range<u32>)>,
    ) -> Option<()> {
        for (index, member) in schema.all_of.iter().enumerate() {
            let member_hint = format!("{hint}Member{index}");
            self.gather_member_recording_unions(
                member,
                &member_hint,
                out,
                union_members,
                lowerings,
            )?;
        }
        let mut siblings = schema.clone();
        siblings.all_of.clear();
        self.gather_all_of(&siblings, hint, out)
    }

    /// [`Self::gather_member`] of one `allOf` member, as [`Self::gather_all_of_recording_unions`]
    /// gathers each: a nested inline `allOf` with no union of its own is flattened with its
    /// `$ref` and inline union members recorded, and the member itself is recorded where it is
    /// gathered as one scalar and is a `$ref` or an inline union.
    fn gather_member_recording_unions<'s>(
        &mut self,
        member: &'s SchemaOr,
        hint: &str,
        out: &mut Vec<Contribution>,
        union_members: &mut Vec<(usize, &'s Schema)>,
        lowerings: &mut Vec<(Provenance, Range<u32>)>,
    ) -> Option<()> {
        if let SchemaOr::Schema(nested) = member {
            if nested.reference.is_none() && !nested.all_of.is_empty() && !schema_has_union(nested)
            {
                // The check `gather_member` makes of a member it reads by its keywords.
                self.diagnose_standalone_discriminator(nested);
                return self.gather_all_of_recording_unions(
                    nested,
                    hint,
                    out,
                    union_members,
                    lowerings,
                );
            }
        }
        let slot = out.len();
        let from = self.graph_mark();
        self.gather_member(member, hint, out)?;
        if let SchemaOr::Schema(member) = member {
            if (member.reference.is_some() || schema_has_union(member))
                && out.len() == slot + 1
                && matches!(out[slot], Contribution::Scalar(_))
            {
                union_members.push((slot, member.as_ref()));
                if member.reference.is_none() {
                    lowerings.push((member.provenance.clone(), from..self.graph_mark()));
                }
            } else if member.reference.is_none()
                && out.len() == slot + 1
                && matches!(out[slot], Contribution::Scalar(ty) if matches!(
                    self.graph.get(ty.id).map(|def| &def.kind),
                    Some(TypeKind::Union(_))
                ))
            {
                // An inline member that lowers to a union of its own (a multi-type `type` array)
                // is an input to the meet as a union member is, and is elided alike once the meet
                // replaced it (#675).
                lowerings.push((member.provenance.clone(), from..self.graph_mark()));
            }
        }
        Some(())
    }

    /// Lower a schema carrying `allOf` and `oneOf`/`anyOf` together (issue #419). Both apply to
    /// every instance, so the schema is their conjunction: the union (the schema without its
    /// `allOf`, so the schema's own keywords refine its branches as they do with no `allOf` beside
    /// it) met with the `allOf` members branch by branch. A branch a member excludes drops out,
    /// and a union left with no branch is `E013`. Dispatching to the `allOf` arm alone dropped the
    /// union and its discriminator with no diagnostic.
    ///
    /// The schema's own keywords are the union's siblings only. Folded into the composition as
    /// well, as [`Self::gather_all_of`] folds them beside a bare `allOf`, untyped ones would make
    /// it an object and drop every branch of another category, and every `null` branch.
    ///
    /// Each member of untyped object or array applicators alone (one
    /// [`implied_applicator_category`] answers for) refines the branches of its own category and
    /// leaves the rest, as the same keywords do as siblings of a `$ref` to a union
    /// ([`Self::refine_union_target`]). The other members are combined as an `allOf` and met
    /// with the union through [`Self::intersect_types`], the meet that `$ref` arm applies to a typed
    /// sibling, under the nullability [`Self::combine_all_of`] gives their merge, as the
    /// `allOf`-member spelling ([`Self::lower_all_of_with_union_member`]) takes it. Members that
    /// constrain nothing (`true`, `{}`, an annotation) take part in neither: with none of either
    /// kind left the union alone is the schema's type, under the schema's own hint, as it is with
    /// no `allOf` at all.
    pub(super) fn lower_all_of_beside_union(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let mut composition = schema.clone();
        composition.one_of.clear();
        composition.any_of.clear();
        composition.discriminator = None;
        // Everything `gather_all_of` and `combine_all_of` read beside `all_of`: the fold of the
        // schema's own object keywords and its `null`.
        composition.types = crate::oas31::TypeSet::default();
        composition.clear_object_keywords();
        let (scoped, combined): (Vec<SchemaOr>, Vec<SchemaOr>) =
            schema.all_of.iter().cloned().partition(|member| {
                matches!(member, SchemaOr::Schema(member) if implied_applicator_category(member).is_some())
            });
        composition.all_of = combined;
        let mut union = schema.clone();
        union.all_of.clear();

        let composition_hint = format!("{hint}Composition");
        let mut contributions = Vec::new();
        self.in_composition(|ctx| {
            ctx.gather_all_of(&composition, &composition_hint, &mut contributions)
        })?;
        let nested = take_refiners(&mut contributions);
        if contributions.is_empty() && scoped.is_empty() && nested.is_empty() {
            return self.lower_union(&union, hint);
        }
        let composed = if contributions.is_empty() {
            None
        } else {
            // Under the nullability `combine_all_of` gives it, as the `allOf`-member spelling
            // takes it: an untyped object member admits `null` and decides nothing, so a
            // composition no member decides leaves `null` to the union.
            let composed = self.combine_all_of(&composition, &composition_hint, &contributions)?;
            let has_object = contributions
                .iter()
                .any(|contribution| matches!(contribution, Contribution::Object { .. }));
            Some(undecided_admits_null(
                composed,
                has_object,
                &contributions,
                &union,
                |branch| self.branch_decides_null(branch),
                |branch| self.branch_denies_null(branch),
            ))
        };
        let mut refiners = self.lower_all_of_refiners(&scoped, hint)?;
        refiners.extend(
            nested
                .iter()
                .map(|(member, refiner)| (member.as_ref(), *refiner)),
        );
        self.meet_union_with_all_of(
            schema,
            hint,
            composed,
            refiners,
            &union,
            &format!("{hint}Union"),
            MetUnion::BesideAllOf,
        )
    }

    /// Lower an `allOf` exactly one of whose members is an inline `oneOf`/`anyOf` (#463), on a
    /// schema with no union of its own. The member applies to every instance as the others do, so
    /// the schema is the union met branch by branch with the rest of the composition, as
    /// `{$ref: A, oneOf: […]}` meets its union with `A` and as the same union written beside the
    /// `allOf` is met ([`Self::lower_all_of_beside_union`]): a branch the composition excludes
    /// drops out, a union left with no branch is `E013`, and branches the meet leaves sharing one
    /// generated type collapse with `W001` ([`Self::collapse_met_union`]). A member whose union
    /// lowers to a single type (one real branch beside a `null` one) is met as that type. Read
    /// as an ordinary scalar member, the union made every object composition an object/scalar mix.
    ///
    /// The member's own keywords stay its union's siblings. The other members, and the
    /// schema's own keywords, are gathered as [`Self::gather_all_of`] gathers them, under the hints
    /// it gives them; members of untyped object or array applicators alone refine the union's
    /// branches of their own category, as beside the `allOf`. Where no member is an object and
    /// none refines one, the union is combined as the scalar member it always was.
    pub(super) fn lower_all_of_with_union_member(
        &mut self,
        schema: &Schema,
        hint: &str,
        union_index: usize,
    ) -> Option<Ty> {
        let SchemaOr::Schema(union) = &schema.all_of[union_index] else {
            return self.lower_all_of(schema, hint);
        };
        let union_hint = format!("{hint}Member{union_index}");
        let mut scoped = Vec::new();
        let mut contributions = Vec::new();
        // Where the union's contribution goes when it is combined as a scalar member below.
        let mut union_slot = 0;
        // The members gathered as one scalar, by where it went, as [`Self::lower_all_of`] records
        // them: each `$ref` member, and each inline union nested in an inner `allOf`.
        let mut union_members = Vec::new();
        // The ids each inline union member's own lowering inserted, the union's included below.
        let mut lowerings = Vec::new();
        let (gathered, held) = self.holding_untyped_warnings(|ctx| {
            ctx.in_composition(|ctx| {
                for (index, member) in schema.all_of.iter().enumerate() {
                    if index == union_index {
                        union_slot = contributions.len();
                        continue;
                    }
                    if matches!(member, SchemaOr::Schema(member) if implied_applicator_category(member).is_some())
                    {
                        scoped.push(member.clone());
                        continue;
                    }
                    ctx.gather_member_recording_unions(
                        member,
                        &format!("{hint}Member{index}"),
                        &mut contributions,
                        &mut union_members,
                        &mut lowerings,
                    )?;
                }
                let mut composition = schema.clone();
                composition.all_of.clear();
                ctx.gather_all_of(&composition, hint, &mut contributions)
            })
        });
        if gathered.is_none() {
            self.report_held_untyped_warnings(held);
            return None;
        }
        // With no object member, the union meets the other members as the scalar it is, in its
        // place among them, exactly as an `allOf` of scalars always met one: the scalar meet
        // already intersects a union branch by branch, and it keeps the narrowing `open_narrowing`
        // gives each member where it is written. A refiner gathered from a nested `allOf` or a
        // sub-file target refines the union's branches below, as the same member written here does,
        // so a half that reaches none of them is reported (`W011`) rather than met silently. A
        // union whose branches leave `null` undecided takes it from the other members there, as
        // the object meet below gives it (#621), and so does a `$ref` member to such a union, or
        // an inline one nested in an inner `allOf`, by the rule [`Self::lower_all_of`] applies to
        // it: neither decides `null` for the other (#631, #642).
        let has_object = contributions
            .iter()
            .any(|contribution| matches!(contribution, Contribution::Object { .. }));
        let has_refiner = contributions
            .iter()
            .any(|contribution| matches!(contribution, Contribution::Refiner { .. }));
        if !has_object && !has_refiner && scoped.is_empty() {
            // Its `W001` is held with the members', for the meet to settle (#660).
            let from = self.graph_mark();
            let (lowered, union_held) =
                self.holding_untyped_warnings(|ctx| ctx.lower_schema(union, &union_hint));
            lowerings.push((union.provenance.clone(), from..self.graph_mark()));
            let mut held = held;
            held.extend(union_held);
            let Some(ty) = lowered else {
                self.report_held_untyped_warnings(held);
                return None;
            };
            contributions.insert(union_slot, Contribution::Scalar(ty));
            for (slot, _) in &mut union_members {
                if *slot >= union_slot {
                    *slot += 1;
                }
            }
            union_members.push((union_slot, union.as_ref()));
            // The union members take `null` there, where it can satisfy the merge.
            return self.combine_scalar_union_members(
                schema,
                hint,
                contributions,
                &union_members,
                held,
                lowerings,
            );
        }
        // The union is met below with its merge held back, so its own report waits for that meet
        // ([`Self::warn_untyped_met_variants`]); the members' stand as raised.
        self.report_held_untyped_warnings(held);
        let nested = take_refiners(&mut contributions);
        let composed = if contributions.is_empty() {
            None
        } else {
            let composed =
                self.combine_all_of(schema, &format!("{hint}Composition"), &contributions)?;
            Some(undecided_admits_null(
                composed,
                has_object,
                &contributions,
                union,
                |branch| self.branch_decides_null(branch),
                |branch| self.branch_denies_null(branch),
            ))
        };
        let mut refiners = self.lower_all_of_refiners(&scoped, hint)?;
        refiners.extend(
            nested
                .iter()
                .map(|(member, refiner)| (member.as_ref(), *refiner)),
        );
        let mut ty = self.meet_union_with_all_of(
            schema,
            hint,
            composed,
            refiners,
            union,
            &union_hint,
            MetUnion::AllOfMember,
        )?;
        // As the other `allOf` arms apply it after their merge.
        ty = self.with_all_of_nullability(schema, ty);
        Some(ty)
    }

    /// Lower the `allOf` members of untyped object or array applicators alone, which refine a
    /// union's branches of their own category ([`Self::lower_all_of_beside_union`]).
    fn lower_all_of_refiners<'s>(
        &mut self,
        scoped: &'s [SchemaOr],
        hint: &str,
    ) -> Option<Vec<(&'s Schema, Refiner)>> {
        let mut refiners = Vec::new();
        for (index, member) in scoped.iter().enumerate() {
            let SchemaOr::Schema(member) = member else {
                continue;
            };
            let scoped =
                self.lower_scoped_refiners(member, true, None, &format!("{hint}Member{index}"))?;
            refiners.push((member.as_ref(), Refiner::Scoped(scoped)));
        }
        Some(refiners)
    }

    /// Lower a `$ref`'s shape-bearing `sibling` keywords (the reference stripped) to be met with
    /// its `referenced` target. An `allOf` there that no member decides `null` for — untyped
    /// object keywords alone, or `$ref`s to untyped object components
    /// ([`Self::all_of_decides_null`]) — lowers on its own to the non-null struct
    /// [`object_all_of_admits_null`] gives it, and that struct, met with a nullable object target,
    /// denied the target's `null`. Its members constrain only objects, so it admits `null` without
    /// deciding it, as the same keywords written beside the `$ref` and the same `allOf` nested in
    /// an `allOf` beside the target do (#562): the target's answer about `null` is the meet's.
    /// Only beside an object target: beside one of another category the two meet in nothing but
    /// `null`, which the `allOf` spelling reports as an object/scalar mix, so that keeps the
    /// verdict it had.
    ///
    /// [`object_all_of_admits_null`]: super::nullability::object_all_of_admits_null
    pub(super) fn lower_ref_sibling(
        &mut self,
        referenced: Ty,
        sibling: &Schema,
        hint: &str,
    ) -> Option<Ty> {
        let mut ty = self.lower_schema(sibling, hint)?;
        let is_struct = |ctx: &Self, id| {
            matches!(
                ctx.graph.get(id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            )
        };
        if !sibling.all_of.is_empty()
            && !schema_has_union(sibling)
            && is_struct(self, ty.id)
            && is_struct(self, referenced.id)
            && !self.all_of_decides_null(sibling, 0)
        {
            ty.nullable = true;
        }
        Some(ty)
    }

    /// Lower `{$ref: T, <keywords>, oneOf|anyOf: […]}`, a `$ref` whose siblings are a union and
    /// shape-bearing `keywords` beside it ([`split_union_sibling`]), as the two `allOf` spellings of
    /// the same conjunction are lowered (#538): `keywords` meet the target, the union is met with
    /// that composition, and what the meet leaves sharing one generated type collapses with `W001`
    /// ([`Self::meet_union_with_all_of`]). Untyped object or array `keywords` refine the union's
    /// branches of their own category after the collapse, as such an `allOf` member does.
    ///
    /// Lowered as one schema, the sibling met `keywords` with each branch before the target, and
    /// the union hoisted every branch's `null` to itself, so the collapse could no longer count
    /// how many branches accept `null`: a `oneOf` that `null` matches twice kept it.
    ///
    /// [`split_union_sibling`]: super::refiner::split_union_sibling
    pub(super) fn meet_ref_union_sibling(
        &mut self,
        schema: &Schema,
        hint: &str,
        referenced: Ty,
        keywords: &Schema,
        union: &Schema,
    ) -> Option<Ty> {
        let keywords_hint = format!("{hint}Constraint");
        let keywords_mark = self.graph_mark();
        let (composed, refiners) = if implied_applicator_category(keywords).is_some() {
            let scoped = self.lower_scoped_refiners(keywords, true, None, &keywords_hint)?;
            (referenced, vec![(keywords, Refiner::Scoped(scoped))])
        } else {
            let keywords = self.lower_ref_sibling(referenced, keywords, &keywords_hint)?;
            let Ok(composed) =
                self.intersect_types(referenced, keywords, &format!("{hint}ReferenceComposition"))
            else {
                return self.reject_ref_sibling_intersection(schema);
            };
            (composed, Vec::new())
        };
        let keywords_lowered = keywords_mark..self.graph_mark();
        let ty = self.meet_union_with_all_of(
            schema,
            hint,
            Some(composed),
            refiners,
            union,
            &format!("{hint}Union"),
            MetUnion::RefSibling,
        )?;
        // The keywords' own type is an input to the meet, lowered before any mark it takes, so it
        // was emitted as a public type nothing referred to (#571). The same rule as the union's
        // lowering withholds it: whatever the met type, a memo, or another schema names stays.
        self.elide_unused_union_lowering(keywords_lowered);
        Some(ty)
    }

    /// Lower `union`, with its own merge held back, meet it with an `allOf`'s `composed` members,
    /// collapse what that meet leaves sharing one generated type, and then meet the result with
    /// each of its `refiners`; the result is inserted as `schema`'s type under `hint`. The meet
    /// shared by [`Self::lower_all_of_beside_union`], [`Self::lower_all_of_with_union_member`] and
    /// [`Self::meet_ref_union_sibling`], whose `composed` is the `$ref` target met with the
    /// keywords beside the union. A union uninhabited on its own (a lone `false` branch) makes the
    /// whole meet that uninhabited type ([`Self::uninhabited_union_meet`]), not `E013`.
    #[allow(clippy::too_many_arguments)]
    fn meet_union_with_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        composed: Option<Ty>,
        refiners: Vec<(&Schema, Refiner)>,
        union: &Schema,
        union_hint: &str,
        spelling: MetUnion,
    ) -> Option<Ty> {
        // How the refiner diagnostics name the union and the keywords that refine it.
        let (union_is, beside, refining) = match spelling {
            MetUnion::AllOfMember => (
                "the union member of this `allOf`",
                "the union member",
                "this `allOf` member's",
            ),
            MetUnion::BesideAllOf => (
                "the union beside this `allOf`",
                "the union beside the `allOf`",
                "this `allOf` member's",
            ),
            MetUnion::RefSibling => (
                "the union beside this `$ref`",
                "the union beside the `$ref`",
                "this `$ref`'s sibling",
            ),
        };
        let meet_hint = format!("{hint}{}", spelling.meet_suffix());
        let enclosing_unmerged = self.unmerged_union.replace(union.provenance.clone());
        // With no composition, only untyped refiners meet the union: they leave `null` to it, as
        // an untyped composition does ([`undecided_admits_null`]), so they admit it where a branch
        // admits it itself ([`union_branch_admits_null`]), and its untyped branches take it there
        // (#586).
        let enclosing_meets_null = std::mem::replace(
            &mut self.unmerged_union_meets_null,
            composed.map_or_else(
                || union_branch_admits_null(union),
                |composed| composed.nullable,
            ),
        );
        let union_mark = self.graph_mark();
        let lowered = self.lower_schema(union, union_hint);
        self.unmerged_union = enclosing_unmerged;
        self.unmerged_union_meets_null = enclosing_meets_null;
        let untyped_beside_null_member =
            self.untyped_beside_null_member.take() == Some(union.provenance.clone());
        let stated_nothing_took_null = self.take_stated_nothing_took_null(&union.provenance);
        let lowered = lowered?;
        let mark = self.graph_mark();
        let meets_from = self.variant_meets.len();
        let mut meet = lowered;
        if let Some(composed) = composed {
            // A union uninhabited on its own (a lone `false` branch) leaves the conjunction so
            // (#615), rather than an empty meet that rejects it.
            let met = self.intersect_types(composed, meet, &meet_hint);
            let Ok(met) = self.uninhabited_union_meet(lowered, met) else {
                return self.reject_all_of_union_meet(schema, spelling);
            };
            meet = self.clear_counted_null(
                met,
                composed,
                stated_nothing_took_null.as_ref(),
                &meet_hint,
            );
        }
        // Collapsed before the refiners, which meet each branch apart and so would give branches
        // the composition left as one type distinct definitions of the same shape: the refiners
        // constrain every branch of their category alike, so refining the collapsed type admits
        // the same values.
        let (collapsed, untyped_check) =
            self.collapse_met_union(schema, meet, !union.one_of.is_empty(), &meet_hint, spelling);
        meet = collapsed;
        for (index, (member, refiner)) in refiners.into_iter().enumerate() {
            let refined_hint = format!("{hint}Refined{index}");
            let met = self.meet_scoped_and_report(
                refiner,
                |ctx, reach| ctx.meet_scoped_refiner(meet, refiner, &refined_hint, reach),
                |ctx| {
                    let message = format!(
                        "a branch of {union_is} states no JSON category, and {refining} untyped \
                         keywords are both object keywords and array keywords with no `type` to \
                         choose between them, so no single Rust type represents what they \
                         constrain of it"
                    );
                    ctx.reject_unscoped_union_sibling(member, &message)
                },
                member,
                |keywords| {
                    format!(
                        "{refining} untyped {keywords} constrain only the instances of their own \
                         category, and no branch of {beside} has that category, so they apply to \
                         no value the union accepts"
                    )
                },
            )?;
            let Ok(met) = met else {
                return self.reject_all_of_union_meet(schema, spelling);
            };
            meet = met;
        }
        // A branch the composition excludes is reported as the sibling spelling reports it (#671).
        let conjunct = match spelling {
            MetUnion::AllOfMember => self.null_admission_without(schema, &union.provenance),
            MetUnion::BesideAllOf | MetUnion::RefSibling => {
                self.union_sibling_null_admission(schema)
            }
        };
        let met_union = MetUnionMember {
            union: lowered.id,
            schema: union.clone(),
            conjunct,
            conjunct_is_null: composed.is_some_and(|composed| self.is_exact_null(composed)),
        };
        self.warn_excluded_union_member_branches(schema, &[met_union], meets_from);
        // After the refiners, which give an untyped branch of their category a type (#535).
        if untyped_check {
            self.warn_untyped_met_variants(schema, meet, spelling);
        }
        let kind = self.graph.get(meet.id)?.kind.clone();
        let mut ty = self.reemit_meet(schema, hint, mark, kind);
        // The meets gave the untyped member `null` exactly where they kept the `null` member's, so
        // `null` is in two branches or none.
        ty.nullable = meet.nullable && !untyped_beside_null_member;
        ty.boxed = meet.boxed;
        self.elide_unused_union_lowering(union_mark..mark);
        Some(ty)
    }
}
