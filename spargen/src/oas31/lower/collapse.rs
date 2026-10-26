//! Collapsing a union a meet produced: variants that became indistinguishable, and the `null`
//! a met union keeps or drops.

use crate::diag::{Code, Diagnostic};
use crate::ir::{Docs, Ty, TypeKind, Union, UnionMode, UnionStrategy};
use crate::oas31::Schema;

use super::meet::same_ty;
use super::nullability::non_nullable;
use super::strategy::retain_strategy;
use super::{LowerCtx, MetUnion};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// The one type every variant of `ty` shares, when `ty` is a union of two or more variants that
    /// are all the same type once each one's own `null` is set aside. No non-null value can tell
    /// such variants apart, so the union adds nothing to its common type — and a `oneOf` of them
    /// rejects every value its common type accepts. `structural` also counts variants that are
    /// distinct definitions decoding the same values — one Rust type, or distinct items of one
    /// structure (#492) — as the same ([`Self::same_type_apart_from_null`]), unless a
    /// discriminator tells them apart by tag. Returned with the number of variants that accept
    /// `null`: after the meet each variant keeps its own nullability, such as a branch that states
    /// nothing and takes the conjunct's `null`, so variants of one type need not agree on it.
    fn indistinguishable_union_variant(&self, ty: Ty, structural: bool) -> Option<(Ty, usize)> {
        let Some(TypeKind::Union(union)) = self.graph.get(ty.id).map(|def| &def.kind) else {
            return None;
        };
        let structural =
            structural && !matches!(union.strategy, UnionStrategy::Discriminated { .. });
        let (first, rest) = union.variants.split_first()?;
        let nullable = union
            .variants
            .iter()
            .filter(|variant| variant.ty.nullable)
            .count();
        (!rest.is_empty()
            && rest.iter().all(|variant| {
                same_ty(non_nullable(variant.ty), non_nullable(first.ty))
                    || (structural && self.same_type_apart_from_null(variant.ty, first.ty))
            }))
        .then_some((first.ty, nullable))
    }

    /// Whether two `oneOf` variants decode the same values — one Rust type, or two of one
    /// structure ([`TypeGraph::same_decoded_values`]) — once each one's own `null` is set aside.
    /// No non-null value tells them apart, so they are one variant whatever their nullability; the
    /// callers decide `null` separately, by how many of the merged branches accept it.
    ///
    /// [`TypeGraph::same_decoded_values`]: crate::ir::TypeGraph::same_decoded_values
    fn same_type_apart_from_null(&self, a: Ty, b: Ty) -> bool {
        self.graph
            .same_decoded_values(non_nullable(a), non_nullable(b))
    }

    /// Collapse `met`, the meet of a `oneOf` (`one_of`) or `anyOf` with what the union is
    /// conjoined with, as each of the [`MetUnion`] spellings must: the union is lowered with its
    /// own merge held back ([`Self::unmerged_union`]), because its branches are compared once the
    /// meet has made them what they are. Returns `met` itself where nothing collapses, beside
    /// whether the result is a `oneOf` whose `serde_json::Value` variants the caller reports
    /// ([`Self::warn_untyped_met_variants`]) once nothing else meets it.
    pub(super) fn collapse_met_union(
        &mut self,
        schema: &Schema,
        met: Ty,
        one_of: bool,
        hint: &str,
        spelling: MetUnion,
    ) -> (Ty, bool) {
        // A `oneOf`'s branches are compared by generated type, as the inline merge compares them
        // (#402): two distinct `i64`-alias enums are one Rust type, so no value tells them apart,
        // and two structs or string enums of one structure decode the same values (#492).
        // An `anyOf` keeps the identity comparison it has always had.
        match self.indistinguishable_union_variant(met, one_of) {
            // Every branch of the intersected union is one and the same type: the branches differ
            // only in keywords the lowered shape does not carry, such as a branch of nothing but
            // `required` (#140). Emitting them as a union gives a `oneOf` whose exactly-one check
            // fails on every value, so the position takes that one type and the ignored branch
            // distinctions are reported, not dropped in silence.
            Some((mut common, branches_accepting_null)) => {
                // `branches_accepting_null` counts the branches that accept `null`, and
                // `met.nullable` says whether the union's own `null` member survived the meet. An
                // `anyOf` needs one match, so `null` is valid when any of them admits it. A
                // `oneOf` needs exactly one: its branches are grouped apart from their own `null`,
                // so they need not agree on it, and `null` is valid only when exactly one of the
                // branches and that member accepts it — two put it in two branches, which fails
                // exactly-one.
                common.nullable = if one_of {
                    usize::from(met.nullable) + branches_accepting_null == 1
                } else {
                    met.nullable || branches_accepting_null > 0
                };
                // W001 case: indistinguishable-branches
                Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
                    .message(format!(
                        "{} intersect to a union whose branches differ only in keywords the \
                         generated type does not carry, so which branch a value matches is not \
                         enforced",
                        spelling.subject(false)
                    ))
                    .remedy("keep producer-side validation for the union's branch constraints")
                    .emit(self.diags);
                (common, false)
            }
            // Only some branches share a generated type: they become one variant, as the inline
            // merge makes them, and the others stand. A `serde_json::Value` variant left among them
            // is the caller's to report ([`Self::warn_untyped_met_variants`]), once whatever still
            // meets the union after the collapse has made its branches final (#535). Where every
            // variant stays distinct, the `null` its variants accept is the one decision's to
            // settle once the conjunction is lowered ([`Self::settle_null_admission`]).
            None if one_of => {
                let collapsed = self
                    .merge_intersected_one_of(schema, met, hint, spelling)
                    .unwrap_or(met);
                (collapsed, true)
            }
            None => (met, false),
        }
    }

    /// `met` with every trial union nested in its variants that the meet inserted since `mark`
    /// collapsed as [`Self::collapse_met_union`] collapses `met` itself, innermost first (#998).
    /// A conjunct that is itself a union, such as a `$ref` target of several `type`s, meets the
    /// union branch by branch per variant of its own, so the union the document wrote is nested
    /// once per conjunct variant. With `C: { type: [string, integer] }`, the schema
    /// `{ $ref: C, oneOf: [{ type: [string, integer] }, { type: string }] }` meets to a category
    /// dispatch over a `oneOf` of two strings and an integer. Each nested `oneOf` is compared by
    /// generated type and each nested `anyOf` by identity, as the top-level union is, so a nested
    /// union whose branches the meet left alike is merged with `W001` rather than emitted as
    /// variants no value tells apart. A union the meet did not make (one the conjunct already
    /// held) is left as it was lowered. A union the meet made is collapsed in its own definition
    /// ([`Self::collapse_met_variants`], [`Self::collapse_met_union_in_place`]), so it keeps the
    /// name it would have had uncollapsed.
    pub(super) fn collapse_nested_met_unions(
        &mut self,
        schema: &Schema,
        met: Ty,
        mark: u32,
        spelling: MetUnion,
    ) -> Ty {
        if met.id.0 < mark {
            return met;
        }
        self.collapse_met_variants(met, mark, |ctx, variant| {
            let nested = ctx.collapse_nested_met_unions(schema, variant, mark, spelling);
            let Some(TypeKind::Union(Union {
                strategy: UnionStrategy::Trial { mode, .. },
                ..
            })) = ctx.graph.get(nested.id).map(|def| &def.kind)
            else {
                return nested;
            };
            if nested.id.0 < mark {
                return nested;
            }
            let one_of = matches!(mode, UnionMode::OneOf);
            ctx.collapse_met_union_in_place(schema, nested, one_of, mark, spelling)
        })
    }

    /// `met`, the meet of a `$ref`'s union target with sibling keywords that are not a union, with
    /// the branches that meet left alike merged as [`Self::collapse_met_union`] merges them
    /// (#1014). The target meets the siblings branch by branch, so two branches the target keeps
    /// apart can meet to one generated type: `oneOf: [{ type: [string, integer] }, { type: string
    /// }]` beside `type: string` is two `String` variants, and every string fails exactly-one. A
    /// `oneOf` is compared by generated type and an `anyOf` by identity, read from the met union's
    /// own mode, and unions nested in its variants are collapsed first. Only a trial union the
    /// meet inserted since `mark` is read: a target the siblings left as it was is the target's
    /// own lowering, which merged its alike branches when it was lowered, and a discriminated or
    /// disjoint union tells its variants apart by construction. The target's `serde_json::Value`
    /// branches were reported when it was lowered, so no untyped-variant check is made here.
    pub(super) fn collapse_met_target_union(
        &mut self,
        schema: &Schema,
        met: Ty,
        mark: u32,
        hint: &str,
    ) -> Ty {
        if met.id.0 < mark {
            return met;
        }
        let met = self.collapse_nested_met_unions(schema, met, mark, MetUnion::RefTarget);
        let Some(TypeKind::Union(Union {
            strategy: UnionStrategy::Trial { mode, .. },
            ..
        })) = self.graph.get(met.id).map(|def| &def.kind)
        else {
            return met;
        };
        let one_of = matches!(mode, UnionMode::OneOf);
        let (collapsed, _) = self.collapse_met_union(
            schema,
            met,
            one_of,
            &format!("{hint}ReferenceIntersection"),
            MetUnion::RefTarget,
        );
        collapsed
    }

    /// `met`, the meet an `allOf`'s scalar members leave ([`Self::combine_all_of`]), with every
    /// `oneOf` union that meet produced collapsed as [`Self::collapse_met_union`] collapses the
    /// meet of a union member (#643). The scalar meet intersects a union branch by branch, so an
    /// `anyOf` member met with a `oneOf` member is an `anyOf` of `oneOf`s, each the second union
    /// met with one branch of the first; branches it leaves sharing one generated type are
    /// indistinguishable, and were emitted as such with no diagnostic. A union is collapsed after
    /// its variants are, so an outer `oneOf` compares the variants as they end up.
    ///
    /// Only types inserted since `mark`, which the meet made, are read, and the union members
    /// lowered for the meet with their merge held back ([`Self::scalar_meet_unmerged`]), which a
    /// meet that excluded nothing of them leaves as they are (#678): any other union lowered
    /// before it was already merged when it was lowered
    /// ([`Self::merge_indistinguishable_variants`]). A discriminated or disjoint union is left as
    /// it is, since its variants are told apart by construction, and so is an `anyOf`, which
    /// needs only one match.
    pub(super) fn collapse_met_one_ofs(&mut self, schema: &Schema, met: Ty, mark: u32) -> Ty {
        if met.id.0 < mark && !self.scalar_meet_unmerged.contains(&met.id) {
            return met;
        }
        let ty = self.collapse_met_variants(met, mark, |ctx, variant| {
            ctx.collapse_met_one_ofs(schema, variant, mark)
        });
        let one_of = matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Union(Union {
                strategy: UnionStrategy::Trial {
                    mode: UnionMode::OneOf,
                    ..
                },
                ..
            }))
        );
        if !one_of {
            return ty;
        }
        self.collapse_met_union_in_place(schema, ty, true, mark, MetUnion::AllOfMember)
    }

    /// `met` with each variant's type replaced by what `collapse` makes of it, when `met` is a
    /// union; `met` itself where it is not one or `collapse` changes no variant. A union the meet
    /// inserted since `mark` is rewritten in its own definition, so it keeps its id and the name
    /// its hint allocates: a rebuilt copy under the same hint would contest that name with the
    /// definition it replaced, which nothing emits but naming still allocates, and lose it to a
    /// hash-suffixed spelling. That is sound because the meet only reads the graph and inserts
    /// into it, so nothing outside its own inserts refers to them
    /// ([`Self::elide_meet_intermediates`]). Any other union is rebuilt as a new definition.
    fn collapse_met_variants(
        &mut self,
        met: Ty,
        mark: u32,
        mut collapse: impl FnMut(&mut Self, Ty) -> Ty,
    ) -> Ty {
        let Some(TypeKind::Union(union)) = self.graph.get(met.id).map(|def| &def.kind) else {
            return met;
        };
        let mut union = union.clone();
        let mut changed = false;
        for variant in &mut union.variants {
            let collapsed = collapse(self, variant.ty);
            if !same_ty(collapsed, variant.ty) {
                variant.ty = collapsed;
                changed = true;
            }
        }
        if !changed {
            return met;
        }
        if met.id.0 >= mark {
            if let Some(def) = self.graph.get_mut(met.id) {
                def.kind = TypeKind::Union(union);
            }
            return met;
        }
        let hint = self
            .graph
            .get(met.id)
            .map(|def| def.name_hint.clone())
            .unwrap_or_default();
        let mut ty = self.insert_type(&hint, TypeKind::Union(union), Docs::default(), None);
        ty.nullable = met.nullable;
        ty.boxed = met.boxed;
        ty
    }

    /// [`Self::collapse_met_union`] over `met`, a union nested in a meet's result, under its own
    /// hint. Where `met` is a union the meet inserted since `mark` and the collapse merges only
    /// some of its branches, the merged union is moved into `met`'s own definition, for the reason
    /// [`Self::collapse_met_variants`] rewrites one in place. The untyped-variant check the
    /// collapse returns is the top-level caller's, which reads the union it finally generates.
    fn collapse_met_union_in_place(
        &mut self,
        schema: &Schema,
        met: Ty,
        one_of: bool,
        mark: u32,
        spelling: MetUnion,
    ) -> Ty {
        let hint = self
            .graph
            .get(met.id)
            .map(|def| def.name_hint.clone())
            .unwrap_or_default();
        let inserted_from = self.graph_mark();
        let (collapsed, _) = self.collapse_met_union(schema, met, one_of, &hint, spelling);
        if met.id.0 < mark
            || collapsed.id.0 < inserted_from
            || self.graph.last_id() != Some(collapsed.id)
        {
            return collapsed;
        }
        let Some((_, merged)) = self.pop_last_type() else {
            return collapsed;
        };
        if let Some(def) = self.graph.get_mut(met.id) {
            def.kind = merged.kind;
        }
        Ty {
            id: met.id,
            nullable: collapsed.nullable,
            boxed: collapsed.boxed,
        }
    }

    /// Report, as the inline union reports them ([`Self::warn_untyped_one_of_variants`]), the
    /// `serde_json::Value` variants of `met`, a `oneOf` meet that [`Self::collapse_met_union`]
    /// left a union (#535). Called on the union as it is generated: after the `allOf` refiners,
    /// whose untyped object or array keywords give a branch of no category a type of their own, so
    /// a branch they refine is not reported as accepting every value.
    pub(super) fn warn_untyped_met_variants(
        &mut self,
        schema: &Schema,
        met: Ty,
        spelling: MetUnion,
    ) {
        if let Some(TypeKind::Union(union)) = self.graph.get(met.id).map(|def| def.kind.clone()) {
            let positions: Vec<usize> = (0..union.variants.len()).collect();
            self.warn_untyped_one_of_variants(
                &schema.provenance,
                &union,
                &positions,
                &format!("{} intersect to a `oneOf` whose", spelling.subject(true)),
                "variant",
            );
        }
    }

    /// The `oneOf` union `ty` a `$ref` met its own sibling to, with the variants that lower to one
    /// generated type merged into the first of them, or `None` when no two do — the post-meet
    /// counterpart of [`Self::merge_indistinguishable_variants`] (#402). Called once
    /// [`Self::indistinguishable_union_variant`] has found that not every variant shares one type,
    /// so two or more variants remain. Variants are grouped apart from their own `null`
    /// ([`Self::same_type_apart_from_null`]). `null` is counted across the union's own `null`
    /// member and every branch of every set: where exactly one accepts it, that one keeps it; where
    /// two or more do, `null` is in two branches, which fails exactly-one, so it is then invalid
    /// everywhere in the union.
    fn merge_intersected_one_of(
        &mut self,
        schema: &Schema,
        ty: Ty,
        hint: &str,
        spelling: MetUnion,
    ) -> Option<Ty> {
        let TypeKind::Union(union) = &self.graph.get(ty.id)?.kind else {
            return None;
        };
        if matches!(union.strategy, UnionStrategy::Discriminated { .. }) {
            return None;
        }
        let union = union.clone();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            let shared = groups.iter_mut().find(|group| {
                self.same_type_apart_from_null(union.variants[group[0]].ty, variant.ty)
            });
            match shared {
                Some(group) => group.push(index),
                None => groups.push(vec![index]),
            }
        }
        if groups.len() == union.variants.len() {
            return None;
        }
        // The members of a set need not agree on `null`, and neither need the sets nor the union's
        // own `null` member (`ty.nullable`). `null` is counted across all of them, as the
        // all-collapse path in [`Self::collapse_met_union`] counts it: it stays valid, where it
        // was, only when exactly one source accepts it; two or more put `null` in two branches,
        // which fails exactly-one everywhere in the union.
        let accepting_null = |group: &Vec<usize>| {
            group
                .iter()
                .filter(|index| union.variants[**index].ty.nullable)
                .count()
        };
        let null_sources =
            usize::from(ty.nullable) + groups.iter().map(accepting_null).sum::<usize>();
        let null_twice = null_sources > 1;
        let retained: Vec<usize> = groups.iter().map(|group| group[0]).collect();
        let variants = groups
            .iter()
            .map(|group| {
                let mut variant = union.variants[group[0]].clone();
                variant.ty.nullable = accepting_null(group) == 1 && !null_twice;
                variant
            })
            .collect();
        // W001 case: indistinguishable-branches
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(format!(
                "{} intersect to a union some of whose branches lower to the same generated type \
                 or to identically structured ones, differing only in keywords it does not carry, \
                 so a value matching one matches all \
                 of them and would fail the exactly-one rule: each such set is one variant of the \
                 generated enum, and which of them a value matches is not enforced",
                spelling.subject(true)
            ))
            .remedy("keep producer-side validation for the union's branch constraints")
            .emit(self.diags);
        let strategy = retain_strategy(&union.strategy, &retained);
        let mut merged = self.insert_type(
            hint,
            TypeKind::Union(Union { variants, strategy }),
            Docs::default(),
            None,
        );
        merged.nullable = ty.nullable && !null_twice;
        merged.boxed = ty.boxed;
        Some(merged)
    }
}
