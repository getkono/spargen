//! `allOf` lowering and a `$ref` with sibling keywords: the entry points, and meeting an
//! `allOf` or `$ref` with a union beside or inside it.

use crate::ir::{Ty, TypeKind};
use crate::oas31::{Schema, SchemaOr};

use super::combine::{schema_has_union, Contribution};
use super::nullability::{undecided_admits_null, union_branch_admits_null};
use super::refiner::implied_applicator_category;
use super::{LowerCtx, MetUnion, Refiner};

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
        self.gather_all_of(schema, hint, &mut contributions)?;
        self.combine_all_of(schema, hint, &contributions)
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
        self.gather_all_of(&composition, &composition_hint, &mut contributions)?;
        if contributions.is_empty() && scoped.is_empty() {
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
        let refiners = self.lower_all_of_refiners(&scoped, hint)?;
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
            self.gather_member(member, &format!("{hint}Member{index}"), &mut contributions)?;
        }
        let mut composition = schema.clone();
        composition.all_of.clear();
        self.gather_all_of(&composition, hint, &mut contributions)?;
        // With no object member, the union meets the other members as the scalar it is, in its
        // place among them, exactly as an `allOf` of scalars always met one: the scalar meet
        // already intersects a union branch by branch, and it keeps the narrowing `open_narrowing`
        // gives each member where it is written.
        let has_object = contributions
            .iter()
            .any(|contribution| matches!(contribution, Contribution::Object { .. }));
        if !has_object && scoped.is_empty() {
            let ty = self.lower_schema(union, &union_hint)?;
            contributions.insert(union_slot, Contribution::Scalar(ty));
            return self.combine_all_of(schema, hint, &contributions);
        }
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
        let refiners = self.lower_all_of_refiners(&scoped, hint)?;
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
    /// keywords beside the union.
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
        let mut meet = lowered;
        if let Some(composed) = composed {
            let Ok(met) = self.intersect_types(composed, meet, &meet_hint) else {
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
