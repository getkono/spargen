//! Sibling keywords beside a union: splitting them into the refiners each branch is met with,
//! and intersecting a union with them.

use indexmap::IndexMap;

use crate::ir::{Docs, JsonCategory, Ty, TypeKind, Union, UnionVariant};
use crate::oas31::{JsonType, Schema, ValidationKeywords};

use super::combine::schema_is_object_like;
use super::meet::{same_ty, value_category, NoMeet};
use super::nullability::non_nullable;
use super::shape::schema_has_shape_constraint;
use super::strategy::retain_strategy;
use super::{CategoryMask, LowerCtx, Refiner, ScopeReach, ScopedRefiners, UnionSibling};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower shape-bearing keywords adjacent to `oneOf`/`anyOf` (the schema without its union and
    /// `discriminator`) so every branch is intersected with them. A multi-non-null `type` array is
    /// already expressed by the union members and is removed here. Its `null` only permits
    /// `null`: the union admits it through a branch `null` matches (#574), which a branch whose own
    /// keywords leave `null` undecided takes from the array
    /// ([`Self::branch_takes_permitted_null`]), and which [`Self::lower_type_array`] spells as a
    /// `null` member of the union it synthesizes.
    ///
    /// Answers `Some(None)` when what is left carries no shape constraint, so there is nothing to
    /// meet. Untyped object or array applicators (one [`implied_applicator_category`] answers for)
    /// lower to a [`Refiner::Scoped`] sibling that refines only the branches of its own category,
    /// limited to the categories a removed `type` array names and admitting `null` unless that
    /// array omits it. Any other sibling lowers whole to a [`Refiner::Whole`], which takes back the
    /// removed array's `null`. The answer carries whether the sibling's own keywords say anything
    /// about `null`. `None` where the sibling fails to lower, which has reported why.
    pub(super) fn lower_union_sibling(
        &mut self,
        schema: &Schema,
        hint: &str,
    ) -> Option<Option<UnionSibling>> {
        let mut sibling = schema.clone();
        sibling.one_of.clear();
        sibling.any_of.clear();
        sibling.discriminator = None;

        // Asked of the sibling's OWN keywords, and asked BEFORE the type array is deleted below.
        // `type`, `enum`, `const`, `$ref` and `allOf` each state something about `null` — an `enum`
        // either lists it or does not, a `$ref`'s target carries its own nullability. The object and
        // array applicators (`properties`, `patternProperties`, `required`, `additionalProperties`,
        // `items`, `prefixItems`) state nothing: in 2020-12 they are vacuously satisfied by every
        // instance of another category, `null` included.
        let speaks_about_null = !sibling.types.types.is_empty()
            || sibling.enum_values.is_some()
            || sibling.const_value.is_some()
            || sibling.reference.is_some()
            || !sibling.all_of.is_empty();
        let declared_types_admit_null = sibling.types.types.contains(&JsonType::Null);

        let non_null_types = sibling
            .types
            .types
            .iter()
            .filter(|kind| **kind != JsonType::Null)
            .count();
        // More than one non-null type has no single lowered representation, so the array is dropped
        // for lowering. That is a lowering convenience, not a statement about the document.
        let types_deleted = non_null_types > 1;
        if types_deleted {
            sibling.types.types.clear();
        }
        if !schema_has_shape_constraint(&sibling) {
            return Some(None);
        }
        // Untyped object or array applicators alone name no `type`, so `lower_schema` would lower
        // them to `TypeKind::Any`, which intersects as identity and drops them from every branch
        // in silence (#282). Nor do they establish their category for the whole union, as they do
        // beside a `$ref` to a single schema: `required: [a]` beside `oneOf: [string, Obj]` is
        // vacuous for the strings, and reading it as an object would drop that branch. Each set
        // refines the branches of its own category instead.
        //
        // A multi-type array deleted above still says which categories the union admits, so it
        // goes with them: the branches of the categories it omits are excluded as they would be
        // against it.
        if implied_applicator_category(&sibling).is_some() {
            let admits_null = !types_deleted || declared_types_admit_null;
            let allowed = types_deleted.then(|| CategoryMask::of(&schema.types.types));
            let scoped = self.lower_scoped_refiners(&sibling, admits_null, allowed, hint)?;
            return Some(Some(UnionSibling {
                refiner: Refiner::Scoped(scoped),
                speaks_about_null,
            }));
        }
        let mut ty = self.lower_schema(&sibling, &format!("{hint}Constraint"))?;
        if types_deleted {
            // Restore the one piece of the deleted array that still has a faithful representation.
            // Without this the lowered sibling reads as null-rejecting for an array that admitted
            // null, and the union's acceptance is removed by a deletion the author never wrote.
            ty.nullable = declared_types_admit_null;
        }
        Some(Some(UnionSibling {
            refiner: Refiner::Whole(ty),
            speaks_about_null,
        }))
    }

    /// Lower a sibling of untyped object or array applicators (one [`implied_applicator_category`]
    /// answers for) into the [`ScopedRefiners`] its two halves refine. Each half is the sibling
    /// with the other half's keywords removed and its own category as `type`, with `null` admitted
    /// where `admits_null` says the sibling does not deny it. `allowed` is the categories of a
    /// multi-type array deleted for lowering, where there was one.
    pub(super) fn lower_scoped_refiners(
        &mut self,
        sibling: &Schema,
        admits_null: bool,
        allowed: Option<CategoryMask>,
        hint: &str,
    ) -> Option<ScopedRefiners> {
        // `types` is empty here, so this is exactly the object applicators.
        let object_like = schema_is_object_like(sibling);
        let array_like = sibling.items.is_some() || !sibling.prefix_items.is_empty();
        let category = |kind: JsonType| {
            if admits_null {
                vec![kind, JsonType::Null]
            } else {
                vec![kind]
            }
        };
        let object = if object_like {
            let mut half = sibling.clone();
            half.items = None;
            half.prefix_items.clear();
            half.types.types = category(JsonType::Object);
            Some(self.lower_schema(&half, &format!("{hint}Constraint"))?)
        } else {
            None
        };
        let array = if array_like {
            let mut half = sibling.clone();
            half.clear_object_keywords();
            half.types.types = category(JsonType::Array);
            let name = if object_like {
                "ArrayConstraint"
            } else {
                "Constraint"
            };
            Some(self.lower_schema(&half, &format!("{hint}{name}"))?)
        } else {
            None
        };
        Some(ScopedRefiners {
            object,
            array,
            admits_null,
            allowed,
        })
    }

    /// Meet one union branch with `refiner`. A [`Refiner::Whole`] sibling is intersected with it
    /// ([`Self::intersect_types`]).
    ///
    /// A [`Refiner::Scoped`] one meets an object branch with its object half and an array branch
    /// with its array half, recording in `reach` which half reached one; a branch of another
    /// category (a scalar, an enum, bytes, `null`, or an uninhabited type), or of a category whose
    /// half the sibling lacks, is left as it is, but for a `null` the sibling denies. A nested
    /// union is met branch by branch ([`Self::meet_scoped_refiner_with_union`]). A branch of a
    /// category [`ScopedRefiners::allowed`] omits is excluded: it is the exact null type where
    /// both it and the sibling admit `null`, and [`NoMeet::Empty`] otherwise. A branch that
    /// states no category (`{}`) takes the one the sibling establishes, as an untyped `$ref`
    /// target does, keeping its own `null` only where the sibling admits it; where the sibling
    /// carries both kinds, or `allowed` admits another category too, there is none to establish,
    /// and the meet is [`NoMeet::Unrepresentable`] with `reach.uncategorised` set. A branch whose
    /// definition is missing or still a reservation is [`NoMeet::Unrepresentable`] as well.
    pub(super) fn meet_refiner(
        &mut self,
        branch: Ty,
        refiner: Refiner,
        reach: &mut ScopeReach,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let scoped = match refiner {
            Refiner::Whole(ty) => return self.intersect_types(branch, ty, hint),
            Refiner::Scoped(scoped) => scoped,
        };
        let Some(kind) = self.graph.get(branch.id).map(|def| def.kind.clone()) else {
            return Err(NoMeet::Unrepresentable);
        };
        // A branch of a category the deleted `type` array omits is excluded, as against the array.
        if let (Some(allowed), Some(category)) = (scoped.allowed, value_category(&kind)) {
            if !allowed.admits(category) {
                return self.excluded_branch(branch, scoped.admits_null, hint);
            }
        }
        let half = match &kind {
            TypeKind::Union(union) => {
                return self.meet_scoped_refiner_with_union(branch, union, refiner, hint, reach);
            }
            TypeKind::Struct(_) => {
                reach.object |= scoped.object.is_some();
                scoped.object
            }
            TypeKind::Array(_) | TypeKind::Tuple(_) => {
                reach.array |= scoped.array.is_some();
                scoped.array
            }
            // A branch that states no category (`{}`, or `{required: [x]}` alone) is the untyped
            // target the `$ref` arm meets: there the applicators establish their category, so
            // `properties` beside `anyOf: [{required: [a]}, {required: [b]}]` is an object in
            // every branch. Both kinds at once have no single category to establish, and nor
            // does one kind beside a deleted `type` array that admits another category too.
            TypeKind::Any => {
                let (half, category) = match (scoped.object, scoped.array) {
                    (Some(object), None) => (object, JsonCategory::Object),
                    (None, Some(array)) => (array, JsonCategory::Array),
                    _ => {
                        reach.uncategorised = true;
                        return Err(NoMeet::Unrepresentable);
                    }
                };
                match scoped.allowed {
                    Some(allowed) if allowed.admits_besides(category) => {
                        reach.uncategorised = true;
                        return Err(NoMeet::Unrepresentable);
                    }
                    Some(allowed) if !allowed.admits(category) => {
                        return self.excluded_branch(branch, scoped.admits_null, hint);
                    }
                    _ => {}
                }
                if category == JsonCategory::Object {
                    reach.object = true;
                } else {
                    reach.array = true;
                }
                // The half's `null` is the sibling not denying it, which refines nothing: `Value`
                // is the identity of the meet, so the meet would take the half's `null` as the
                // branch's own. The branch keeps its own answer instead, as an untyped object
                // branch does against the same half (#588).
                let mut met = self.intersect_types(branch, half, hint)?;
                met.nullable = branch.nullable && scoped.admits_null;
                return Ok(met);
            }
            // A placeholder's body is not known yet, so nothing can be said of its category. The
            // callers refuse a reservation before they get here; this keeps the refusal for one
            // that does not.
            TypeKind::Reserved => return Err(NoMeet::Unrepresentable),
            TypeKind::Primitive(_)
            | TypeKind::Enum(_)
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never => None,
        };
        match half {
            Some(half) => self.intersect_types(branch, half, hint),
            None => {
                let mut ty = branch;
                ty.nullable = branch.nullable && scoped.admits_null;
                Ok(ty)
            }
        }
    }

    /// A union branch of a category the sibling's deleted `type` array omits: no value of it
    /// satisfies the sibling but `null`, which is left where both the branch and the sibling admit
    /// it (the exact JSON null type under `hint`), and otherwise the meet is empty.
    fn excluded_branch(
        &mut self,
        branch: Ty,
        sibling_admits_null: bool,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        if branch.nullable && sibling_admits_null {
            Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
        } else {
            Err(NoMeet::Empty)
        }
    }

    /// The `$ref` arm's answer for a `$ref` to the union `union` (its target, `referenced`) whose
    /// siblings are untyped object or array applicators alone: each set refines the target's
    /// branches of its own category, and the rest are kept as they are. Such siblings say nothing
    /// about `null`, so the target's nullability stands. A set that reaches no branch of its
    /// category is vacuous, and `W011`; a branch that states no category with no single one to
    /// establish for it, and a meet that leaves no branch, are `E013`.
    pub(super) fn refine_union_target(
        &mut self,
        schema: &Schema,
        hint: &str,
        referenced: Ty,
        union: &Union,
        sibling: &Schema,
    ) -> Option<Ty> {
        let scoped = self.lower_scoped_refiners(sibling, true, None, hint)?;
        let refiner = Refiner::Scoped(scoped);
        let mark = self.graph_mark();
        let met_hint = format!("{hint}ReferenceIntersection");
        let met = self.meet_scoped_and_report(
            refiner,
            |ctx, reach| {
                ctx.meet_scoped_refiner_with_union(referenced, union, refiner, &met_hint, reach)
            },
            |ctx| {
                ctx.reject_ref_sibling_category(
                    schema,
                    "a branch of this `$ref`'s target union states no JSON category, and the \
                     untyped sibling keywords are both object keywords and array keywords with no \
                     `type` to choose between them, so no single Rust type represents what they \
                     constrain of it",
                )
            },
            schema,
            |keywords| {
                format!(
                    "this `$ref`'s untyped sibling {keywords} constrain only the instances of \
                     their own category, and no branch of its target union has that category, so \
                     they constrain no value the target accepts"
                )
            },
        )?;
        let Ok(met) = met else {
            return self.reject_ref_sibling_intersection(schema);
        };
        let kind = self.graph.get(met.id)?.kind.clone();
        // The meet carries the target's nullability, but for a meet that left only `null`: that
        // is the exact null type, whose one value is `null` already, so it is not wrapped in an
        // `Option` as the `allOf` and inline spellings of the same union are not (#450).
        let nullable = referenced.nullable && !matches!(kind, TypeKind::Null);
        // The met union is re-emitted under this schema's name, so its own def
        // (`…ReferenceIntersection`) is unused; the branches it met stay where `kind` reaches
        // them (#462).
        let mut ty = self.reemit_meet(schema, hint, mark, kind);
        ty.nullable = nullable;
        ty.boxed = met.boxed;
        Some(ty)
    }

    /// Meet `target` with the untyped applicators `refiner` carries: branch by branch where
    /// `target` is a union, and as that one branch otherwise. [`Self::refine_union_target`]'s
    /// meet, for a target that need not be a union.
    ///
    /// That dispatch is [`Self::meet_refiner`]'s for a [`Refiner::Scoped`] `refiner`, which is the
    /// only kind this is given, so its cases are this one's: a union target goes to
    /// [`Self::meet_scoped_refiner_with_union`]; a branch of another category is kept, but one
    /// [`ScopedRefiners::allowed`] omits is excluded; a target that states no category with none
    /// to establish for it, and a reservation, are refused as [`NoMeet::Unrepresentable`]. What
    /// this adds is the narrowing: a target that is not a union is met closed, as a union's meet
    /// closes it for its branches, while a union target's meet reads the enclosing answer itself.
    pub(super) fn meet_scoped_refiner(
        &mut self,
        target: Ty,
        refiner: Refiner,
        hint: &str,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        debug_assert!(matches!(refiner, Refiner::Scoped(_)));
        if matches!(
            self.graph.get(target.id).map(|def| &def.kind),
            Some(TypeKind::Union(_))
        ) {
            return self.meet_refiner(target, refiner, reach, hint);
        }
        self.closed_narrowing(|ctx| ctx.meet_refiner(target, refiner, reach, hint))
    }

    /// Meet with a sibling, `refiner`, and report what the meet found besides its result: the
    /// sequence a union's sole member, a `$ref` to a union and an `allOf`'s untyped members share.
    /// Only a [`Refiner::Scoped`] sibling records anything to report; a [`Refiner::Whole`] one
    /// leaves the meet as it is. `meet` runs with a fresh [`ScopeReach`]. Where it failed on a branch that
    /// states no category with none to establish for it, `reject_uncategorised` reports that
    /// (`E013`) and its `None` is returned. Otherwise each half of `refiner` that reached no branch
    /// of its category is reported (`W011`) at `warned`, worded by `unreached`, and the meet is
    /// returned for the caller to settle.
    pub(super) fn meet_scoped_and_report(
        &mut self,
        refiner: Refiner,
        meet: impl FnOnce(&mut Self, &mut ScopeReach) -> Result<Ty, NoMeet>,
        reject_uncategorised: impl FnOnce(&mut Self) -> Option<Result<Ty, NoMeet>>,
        warned: &Schema,
        unreached: impl Fn(&str) -> String,
    ) -> Option<Result<Ty, NoMeet>> {
        let mut reach = ScopeReach::default();
        let met = meet(self, &mut reach);
        if met.is_err() && reach.uncategorised {
            return reject_uncategorised(self);
        }
        for keywords in unreached_halves(refiner, &reach) {
            self.warn_unreached_union_sibling(warned, unreached(keywords));
        }
        Some(met)
    }

    /// [`Self::meet_scoped_refiner`] for a `target` whose kind is `union`. The meet admits `null`
    /// exactly when `target` and `refiner` both do: [`Self::intersect_union`] builds its result
    /// from the non-null branches alone, and a union's `null` branch is its outer nullability,
    /// so it is carried across here rather than lost with the rebuilt union. For the same reason,
    /// a meet that excludes every non-null branch is not empty while both still admit `null`:
    /// `null` is then the only value left, and the meet is the exact JSON null type, as the inline
    /// sibling spelling of the same union answers (#450).
    fn meet_scoped_refiner_with_union(
        &mut self,
        target: Ty,
        union: &Union,
        refiner: Refiner,
        hint: &str,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        let enclosing = self.narrowing_opens;
        let accepts_null = target.nullable && self.refiner_accepts_null(refiner);
        match self.closed_narrowing(|ctx| {
            ctx.intersect_union(target, union, refiner, hint, enclosing, reach)
        }) {
            Ok(mut ty) => {
                ty.nullable = accepts_null;
                Ok(ty)
            }
            Err(NoMeet::Empty) if accepts_null => {
                Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
            }
            Err(no_meet) => Err(no_meet),
        }
    }

    /// The meet `met` of a union that lowered to `union` with what the union is a conjunct of,
    /// where a union uninhabited on its own keeps its verdict (#615). A union whose only branch is
    /// `false` (inline, or a `$ref` to a `false` component) admits no value, and neither does any
    /// conjunction it is part of, so the conjunction is the uninhabited type the union already is,
    /// as the bare union and its spellings beside untyped object keywords generate. The meet of
    /// [`TypeKind::Never`] with anything is [`NoMeet::Empty`] (but for `null` both sides admit), and
    /// read as an empty meet of two inhabited sides it rejected the same empty set as `E013` beside
    /// a `$ref` or an `allOf` and as `E007` beside `type: object`. Any other answer is `met`.
    pub(super) fn uninhabited_union_meet(
        &self,
        union: Ty,
        met: Result<Ty, NoMeet>,
    ) -> Result<Ty, NoMeet> {
        match met {
            Err(NoMeet::Empty)
                if matches!(
                    self.graph.get(union.id).map(|def| &def.kind),
                    Some(TypeKind::Never)
                ) =>
            {
                Ok(non_nullable(union))
            }
            met => met,
        }
    }

    /// Whether a union branch met with `refiner` may still be `null`, for a union whose every
    /// real branch was excluded: the sibling's own answer.
    pub(super) fn refiner_accepts_null(&self, refiner: Refiner) -> bool {
        match refiner {
            Refiner::Whole(ty) => self.ty_accepts_null(ty),
            Refiner::Scoped(scoped) => scoped.admits_null,
        }
    }

    /// The meet of the union `union` (whose type is `union_ty`) with `other`, branch by branch.
    /// Called with `open_narrowing` out of effect, so every retained branch is closed.
    /// `enclosing_opens` is [`Self::narrowing_opens`] where the meet was asked for: when exactly one
    /// branch survives, the result is no union, and that branch is met again under that answer, so
    /// the meet is the same set whichever order the `allOf` writes the union and the `string` in
    /// (written first, the union narrows to that branch before the `string` opens it).
    ///
    /// Each branch is met through [`Self::meet_refiner`], so a [`Refiner::Scoped`] `other` leaves
    /// the branches of another category as they are and records what it reached in `reach`.
    pub(super) fn intersect_union(
        &mut self,
        union_ty: Ty,
        union: &Union,
        other: Refiner,
        hint: &str,
        enclosing_opens: bool,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        let mut variants = Vec::new();
        let mut retained = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            match self.meet_refiner(variant.ty, other, reach, &format!("{hint}Variant{index}")) {
                Ok(ty) => {
                    variants.push(UnionVariant {
                        name_hint: variant.name_hint.clone(),
                        ty,
                    });
                    retained.push(index);
                }
                // A branch no value of `other` satisfies contributes nothing to the intersection,
                // so dropping it loses no value.
                Err(NoMeet::Empty) => {}
                // A branch that shares values with `other` but has no type for them cannot be
                // dropped without refusing those values, whatever the other branches do.
                Err(NoMeet::Unrepresentable) => return Err(NoMeet::Unrepresentable),
            }
        }
        if variants.len() == 1 {
            if enclosing_opens {
                let branch = union.variants[retained[0]].ty;
                return self.response_narrowing(|ctx| ctx.meet_refiner(branch, other, reach, hint));
            }
            return Ok(variants.remove(0).ty);
        }
        if variants.is_empty() {
            return Err(NoMeet::Empty);
        }
        if variants.len() == union.variants.len()
            && variants
                .iter()
                .zip(&union.variants)
                .all(|(left, right)| same_ty(left.ty, right.ty))
        {
            return Ok(non_nullable(union_ty));
        }
        let strategy = retain_strategy(&union.strategy, &retained);
        Ok(self.insert_type(
            hint,
            TypeKind::Union(Union { variants, strategy }),
            Docs::default(),
            None,
        ))
    }
}

/// The keyword set of every half of a [`Refiner::Scoped`] sibling that reached no branch of its
/// category, object half first; empty where every half the sibling carries reached one (or the
/// sibling is not scoped). Each entry is reported with a `W011` of its own.
pub(super) fn unreached_halves(refiner: Refiner, reach: &ScopeReach) -> Vec<&'static str> {
    let Refiner::Scoped(scoped) = refiner else {
        return Vec::new();
    };
    let mut halves = Vec::new();
    if scoped.object.is_some() && !reach.object {
        halves.push(
            "object keywords (`properties`, `patternProperties`, `required`, \
             `additionalProperties`)",
        );
    }
    if scoped.array.is_some() && !reach.array {
        halves.push("array keywords (`items`, `prefixItems`)");
    }
    halves
}

/// The message for a scoped sibling half [`unreached_halves`] names.
pub(super) fn unreached_message(keywords: &str) -> String {
    format!(
        "this schema's untyped {keywords} constrain only the instances of their own category, \
         and no branch of its union has that category, so they apply to no value the union \
         accepts"
    )
}

/// The category a schema's object or array applicators imply, for a schema that establishes none
/// of its own. See [`implied_applicator_category`].
pub(super) enum ImpliedCategory {
    /// Only object applicators, or only array applicators: the category they apply to.
    Only(JsonType),
    /// Both kinds, and nothing to choose between them.
    Conflicting,
}

/// The category a schema's applicators establish when it names no `type` and carries no other
/// keyword that lowers to a shape of its own: `enum`, `const`, `allOf`, `oneOf`, `anyOf`,
/// `contentEncoding`, `format: binary`, or a `$ref`. Asked of a `$ref`'s siblings, of a union's
/// siblings ([`LowerCtx::lower_union_sibling`]), of an `allOf`'s members beside a union, and of
/// the keywords beside a `$ref` and a union together: beside a single schema the applicators
/// establish this category for it, and beside a union they refine only its branches of this
/// category.
///
/// The object applicators are `properties`, `patternProperties`, `required` and
/// `additionalProperties`; the array applicators are `items` and `prefixItems`.
/// [`ImpliedCategory::Conflicting`] when the schema carries both kinds. `None` when it carries
/// neither, or already names or implies its shape some other way.
pub(super) fn implied_applicator_category(schema: &Schema) -> Option<ImpliedCategory> {
    let establishes_elsewhere = !schema.types.types.is_empty()
        || schema.reference.is_some()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || !schema.all_of.is_empty()
        || !schema.one_of.is_empty()
        || !schema.any_of.is_empty()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary");
    if establishes_elsewhere {
        return None;
    }
    // `types` is empty here, so this is exactly the object applicators.
    let object = schema_is_object_like(schema);
    let array = schema.items.is_some() || !schema.prefix_items.is_empty();
    match (object, array) {
        (true, false) => Some(ImpliedCategory::Only(JsonType::Object)),
        (false, true) => Some(ImpliedCategory::Only(JsonType::Array)),
        (true, true) => Some(ImpliedCategory::Conflicting),
        (false, false) => None,
    }
}

/// Split the siblings of a `$ref` (`sibling`, the reference already stripped) that carry a
/// `oneOf`/`anyOf` into the keywords beside the union and the union itself, as separate conjuncts
/// ([`LowerCtx::meet_ref_union_sibling`]). The union keeps its `discriminator` and the sibling's
/// provenance, and nothing else: every other keyword, annotations included, stays with the
/// keywords, so each is lowered, and reported, once. The struct literal is exhaustive, so a field
/// added to [`Schema`] has to be placed here.
pub(super) fn split_union_sibling(sibling: &Schema) -> (Schema, Schema) {
    let mut keywords = sibling.clone();
    let one_of = std::mem::take(&mut keywords.one_of);
    let any_of = std::mem::take(&mut keywords.any_of);
    let discriminator = keywords.discriminator.take();
    let union = Schema {
        boolean: None,
        types: crate::oas31::TypeSet::default(),
        reference: None,
        properties: IndexMap::new(),
        required: Vec::new(),
        additional_properties: None,
        pattern_properties: IndexMap::new(),
        items: None,
        prefix_items: Vec::new(),
        all_of: Vec::new(),
        one_of,
        any_of,
        discriminator,
        defs: IndexMap::new(),
        validation_children: Vec::new(),
        enum_values: None,
        const_value: None,
        default: None,
        format: None,
        content_encoding: None,
        content_media_type: None,
        content_schema: None,
        xml: None,
        validation: ValidationKeywords::default(),
        deprecated: false,
        read_only: false,
        write_only: false,
        title: None,
        description: None,
        provenance: sibling.provenance.clone(),
    };
    (keywords, union)
}
