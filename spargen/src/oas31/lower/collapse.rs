//! Collapsing a union a meet produced: variants that became indistinguishable, and the `null`
//! a met union keeps or drops.

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{Docs, Ty, TypeKind, Union, UnionStrategy, UnionVariant};
use crate::oas31::Schema;

use super::meet::same_ty;
use super::nullability::non_nullable;
use super::strategy::retain_strategy;
use super::{LowerCtx, MetUnion, StatedNothingNull};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Where the schema `ty` lowers from was authored: the location [`Self::meet_locations`]
    /// recorded for a meet struct, else the type's own provenance, unless that is the document root
    /// every synthesized type falls back to.
    pub(super) fn authored_location(&self, ty: Ty) -> Option<Provenance> {
        if let Some(location) = self.meet_locations.get(&ty.id) {
            return Some(location.clone());
        }
        let def = self.graph.get(ty.id)?;
        (!def.provenance.pointer.as_str().is_empty()).then(|| def.provenance.clone())
    }

    /// The one type every variant of `ty` shares, when `ty` is a union of two or more variants that
    /// are all the same type. No value can tell such variants apart, so the union adds nothing to
    /// its common type — and a `oneOf` of them rejects every value its common type accepts.
    /// `structural` also counts variants that are distinct definitions decoding the same values —
    /// one Rust type, or distinct items of one structure (#492) — as the same
    /// ([`Self::same_type_apart_from_null`]), unless a discriminator tells them apart by
    /// tag. Returned with the number of variants that accept `null`: after the meet with a `$ref`
    /// target each variant keeps its own nullability, so structurally shared variants need not
    /// agree on it.
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
                same_ty(variant.ty, first.ty)
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
            // meets the union after the collapse has made its branches final (#535).
            None if one_of => {
                let collapsed = match self.merge_intersected_one_of(schema, met, hint, spelling) {
                    Some(merged) => merged,
                    None => self.drop_null_matching_two_branches(met, hint),
                };
                (collapsed, true)
            }
            None => (met, false),
        }
    }

    /// What [`Self::stated_nothing_took_null`] holds for the union lowered at `at`, taken so an
    /// enclosing meet does not read it; `None` for any other union.
    pub(super) fn take_stated_nothing_took_null(
        &mut self,
        at: &Provenance,
    ) -> Option<StatedNothingNull> {
        match self.stated_nothing_took_null.take() {
            Some((provenance, stated)) if provenance == *at => Some(stated),
            _ => None,
        }
    }

    /// `met`, the meet of a union held back for it ([`Self::unmerged_union`]), with the `null` the
    /// meet handed again to its branches that state nothing and lower to `Value` settled as
    /// `stated` says ([`Self::stated_nothing_took_null`]). `Value` is the identity of the meet, so
    /// the meet gave each the conjunct's `null` a second time.
    ///
    /// Where the union counted that `null` ([`StatedNothingNull::Counted`]) the copies in the
    /// branches it names are cleared (#592): one would leave an `anyOf` variant accepting the
    /// `null` its union already holds, and the `oneOf` counts after the meet
    /// ([`Self::collapse_met_union`]) would read it as `null` matching that branch only. Where a
    /// `oneOf` counted the one such branch as the only branch `null` matches
    /// ([`StatedNothingNull::Sole`]) and the meet narrowed the union to a single branch, that
    /// branch is the one: a branch that states nothing meets every conjunct, so it survives every
    /// meet. The position then accepts the `null` the meet gave it (#597), which the union's own
    /// nullability, not hoisted, does not carry. The meet's variants are the held-back union's
    /// only where `conjunct`, what it was met with, is no union itself; otherwise, or where there
    /// is nothing to settle, `met` is returned as it is.
    pub(super) fn clear_counted_null(
        &mut self,
        met: Ty,
        conjunct: Ty,
        stated: Option<&StatedNothingNull>,
        hint: &str,
    ) -> Ty {
        let Some(stated) = stated else {
            return met;
        };
        if matches!(
            self.graph.get(conjunct.id).map(|def| &def.kind),
            Some(TypeKind::Union(_))
        ) {
            return met;
        }
        let met_kind = self.graph.get(met.id).map(|def| &def.kind);
        let hints = match stated {
            StatedNothingNull::Sole => {
                let mut met = met;
                if !matches!(met_kind, Some(TypeKind::Union(_))) {
                    met.nullable = met.nullable || conjunct.nullable;
                }
                return met;
            }
            StatedNothingNull::Counted(hints) => hints,
        };
        let Some(TypeKind::Union(union)) = met_kind else {
            return met;
        };
        let counted =
            |variant: &UnionVariant| variant.ty.nullable && hints.contains(&variant.name_hint);
        if !union.variants.iter().any(counted) {
            return met;
        }
        let mut union = union.clone();
        for variant in &mut union.variants {
            if hints.contains(&variant.name_hint) {
                variant.ty.nullable = false;
            }
        }
        let mut ty = self.insert_type(hint, TypeKind::Union(union), Docs::default(), None);
        ty.nullable = met.nullable;
        ty.boxed = met.boxed;
        ty
    }

    /// `met`, a `oneOf` meet whose variants all stay distinct, with `null` made invalid where two
    /// or more of its sources accept it (#563): the union's own `null` (`met.nullable`, which
    /// carries the branches the union hoisted before the meet) and each variant the meet left
    /// accepting it, such as an untyped branch that took a nullable target's shape. `null` in two
    /// branches fails exactly-one, so no variant keeps it and neither does the union, as
    /// [`Self::merge_intersected_one_of`] answers for the variants it merges.
    fn drop_null_matching_two_branches(&mut self, met: Ty, hint: &str) -> Ty {
        let Some(TypeKind::Union(union)) = self.graph.get(met.id).map(|def| &def.kind) else {
            return met;
        };
        let nullable_variants = union
            .variants
            .iter()
            .filter(|variant| variant.ty.nullable)
            .count();
        // An exact-`null` variant (`const: null`) is a branch `null` matches too, counted beside a
        // variant the meet left accepting it, as the inline union counts it (#586).
        let null_variants = union
            .variants
            .iter()
            .filter(|variant| self.is_exact_null(variant.ty))
            .count();
        if nullable_variants == 0
            || usize::from(met.nullable) + nullable_variants + null_variants < 2
        {
            return met;
        }
        let mut union = union.clone();
        for variant in &mut union.variants {
            variant.ty.nullable = false;
        }
        let mut ty = self.insert_type(hint, TypeKind::Union(union), Docs::default(), None);
        ty.boxed = met.boxed;
        ty
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
