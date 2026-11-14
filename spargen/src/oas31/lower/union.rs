//! `oneOf` / `anyOf` lowering: a union's variants, their null handling, and the merge of
//! variants no strategy could tell apart.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{
    Ty, TypeId, TypeKind, Union, UnionMode, UnionStrategy, UnionVariant, VariantDispatch,
};
use crate::oas31::discriminator::is_schema_component_name;
use crate::oas31::{Schema, SchemaOr};

use super::discriminator::member_component_name;
use super::discriminator::{DeferredDiscriminator, DiscriminatorMembers, PinnedMembers};
use super::meet::NoMeet;
use super::nullability::{member_is_null_only, union_branch_admits_null, NullAdmission};
use super::prune::{kind_edges, reachable_types};
use super::refiner::{
    multi_category_types, union_sibling_is_categories_alone, unreached_halves, unreached_message,
};
use super::shape::ref_siblings_bear_shape;
use super::strategy::UnionDispatch;
use super::{LowerCtx, Refiner, ScopeReach, UnionSibling};

/// A union's members, sorted before any is lowered: what [`LowerCtx::lower_union_with_sibling`]
/// hands the sole-member collapse and the variant path alike.
struct UnionMembers<'s> {
    /// The members that are not null-only, in document order.
    real: Vec<&'s SchemaOr>,
    /// Whether the members are a `oneOf`'s or an `anyOf`'s.
    mode: UnionMode,
    /// How many null-only members there are: each is a branch `null` matches, which a `oneOf`
    /// counts against its exactly-one rule beside the real branches that accept `null` too.
    null_members: usize,
    /// Whether the schema's own keywords permit `null` (a `"null"` in its `type` array, a
    /// `const: null`, an `enum` listing `null`), which supplies no branch of its own.
    null_from_type_array: bool,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower a `oneOf`/`anyOf` union, with `open_narrowing` out of effect.
    ///
    /// Null-only members ([`member_is_null_only`]) are stripped, and each is a branch `null`
    /// matches, so it makes the union `nullable` (`Option<Union>`). A `"null"` in the enclosing
    /// `type` array only *permits* `null`: it reaches a branch whose own keywords leave `null`
    /// undecided, and adds no branch of its own. The schema's sibling keywords
    /// ([`Self::lower_union_sibling`]) are met with every remaining member. Then:
    ///
    /// * with only null-only members, the union is the exact JSON null type;
    /// * with one real member, it collapses to that member's type (`Option<T>` beside a `null`
    ///   member, however many there are) with no enum — a member that is a cycle-closing `$ref`
    ///   stays the boxed reference itself;
    /// * otherwise the variants are represented WITHOUT `serde(untagged)` and without degrading to
    ///   `serde_json::Value`: a `discriminator` dispatches object variants by tag and uniquely
    ///   categorized non-object variants by JSON category; statically disjoint variants dispatch
    ///   by JSON category or unique required key; overlapping variants use typed trial matching
    ///   with exact-one (`oneOf`) or deterministic most-specific (`anyOf`) semantics, including
    ///   serialization revalidation.
    ///
    /// Two warnings are `W001` at the union. An undiscriminated `oneOf`'s variants that decode the
    /// same values — that lower to one generated type, or to identically structured ones — merge
    /// into one variant ([`Self::merge_indistinguishable_variants`]). A trial-matched `oneOf`'s
    /// variants that lower to `serde_json::Value` beside another variant are reported
    /// ([`Self::warn_untyped_one_of_variants`]). Neither is emitted here for a `$ref`'s own `oneOf`
    /// sibling, which the `$ref` arm merges and reports after the meet
    /// ([`Self::collapse_met_union`]), or for an `allOf`'s union member its scalar meet takes; and
    /// while an `allOf` gathers its members, the `Value` report is held for its meet to settle
    /// ([`Self::report_untyped_one_of`]).
    ///
    /// A member the sibling meet excludes drops out with `W011`; where that leaves no branch, the
    /// union is the exact null type if a null-only member remains and the sibling admits `null`,
    /// and `E007` otherwise. A sole member that is uninhabited on its own (`false`) is no branch the
    /// sibling excludes: the union is that uninhabited type whatever the sibling is
    /// ([`Self::uninhabited_union_meet`]). An `anyOf` member the sibling meet narrows to the exact
    /// null type is a branch `null` matches: beside typed members it makes the union `nullable`
    /// and is no variant, and with every member so narrowed the union is the null type. A `oneOf`
    /// in which `null` matches more than one branch admits no `null`.
    ///
    /// The union is rejected (`None`) with `E007` where one node declares both `oneOf` and
    /// `anyOf`; where a member is a direct recursive `$ref` to this union itself; where the
    /// schema's whole body is a union whose only non-null member is a `$ref` closing a cycle back
    /// to that schema; where the sibling meet leaves no branch, as above; and where the
    /// `discriminator` cannot dispatch: a `mapping` or `defaultMapping` entry naming a schema that
    /// is no `$ref` member ([`Self::discriminator_members`]), a `defaultMapping` naming a member
    /// the sibling excluded, or a member no discriminator value selects
    /// ([`Self::reject_unselectable_discriminated_variant`]). It is rejected with `E013` where the
    /// sibling would have to be met with a member that closes a reference cycle (but for one of a
    /// category a sibling that is a multi-category `type` array alone admits, which is kept unmet,
    /// [`Self::cycle_branch_within`]), with a member that states no category for untyped sibling
    /// keywords to establish, or with a member it shares values with that no single Rust type
    /// represents, or where it meets every one of several `oneOf` members, or one beside a `null`
    /// member, in `null` alone, which then matches them all.
    ///
    /// Every variant type inserts before the union def, so the [`TypeKind::Union`] is the final
    /// graph insert (last-insert invariant: `lower_into_reservation`).
    pub(super) fn lower_union(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.lower_union_closed(schema, hint))
    }

    /// [`Self::lower_union`]'s body, run with `open_narrowing` out of effect. A union tells its
    /// variants apart by what each one refuses — a `Trial` `oneOf` requires exactly one to match —
    /// so an open set inside a variant could make two variants accept the same value and fail a
    /// value the closed union decodes.
    fn lower_union_closed(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (members, mode): (Vec<&SchemaOr>, UnionMode) =
            match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
                (false, true) => (schema.one_of.iter().collect(), UnionMode::OneOf),
                (true, false) => (schema.any_of.iter().collect(), UnionMode::AnyOf),
                (false, false) => {
                    return self.reject_unrepresentable_union(
                    schema,
                    "a single schema node declares both `oneOf` and `anyOf`; their intersected \
                     applicator semantics are not representable as one generated union",
                );
                }
                (true, true) => unreachable!("lower_union is called only for a union schema"),
            };
        let sibling_from = self.graph_mark();
        let sibling = self.lower_union_sibling(schema, hint)?;
        let sibling_lowered = sibling_from..self.graph_mark();
        let union = self.lower_union_with_sibling(schema, hint, members, mode, sibling);
        // A multi-category `type` array lowers to the union it synthesizes only to be met with each
        // branch, so that union and its branches are emitted only where a meet kept one of them as
        // it is (#677). A union left one variant is that variant's kind, re-emitted under this
        // schema's name, so the sibling keywords' own lowering (`…Constraint`) is emitted only
        // where that kind reaches it too (#987).
        let left_one_variant = |ctx: &Self, union: Ty| {
            ctx.graph
                .get(union.id)
                .is_some_and(|def| !matches!(def.kind, TypeKind::Union(_)))
        };
        if union.is_some_and(|union| {
            !sibling_lowered.contains(&union.id.0)
                && (multi_category_types(schema).is_some() || left_one_variant(self, union))
        }) {
            self.elide_unused_lowering(sibling_lowered);
        }
        union
    }

    /// Whether `member`, a branch of the union `schema` that closes a reference cycle, is kept
    /// unmet. A sibling that is a multi-category `type` array alone constrains a branch's category
    /// and nothing else, so a cycle-closing branch of a category it admits meets it in itself,
    /// and is kept unmet rather than refused ([`Self::cycle_branch_within`]).
    fn kept_unmet(&self, schema: &Schema, member: &SchemaOr) -> bool {
        union_sibling_is_categories_alone(schema)
            .is_some_and(|allowed| self.cycle_branch_within(member, allowed))
    }

    /// Run `lower` with `unmerged` as [`Self::unmerged_union`]: the union whose merge waits for
    /// its meet, beside whether a conjunct it is met with admits `null`, or `None` for no such
    /// union. The enclosing value is restored.
    pub(super) fn with_unmerged_union<T>(
        &mut self,
        unmerged: Option<(Provenance, bool)>,
        lower: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let enclosing = std::mem::replace(&mut self.unmerged_union, unmerged);
        let lowered = lower(self);
        self.unmerged_union = enclosing;
        lowered
    }

    /// Whether the union written at `provenance` is [`Self::unmerged_union`], whose merge waits
    /// for its meet.
    fn is_unmerged_union(&self, provenance: &Provenance) -> bool {
        self.unmerged_union
            .as_ref()
            .is_some_and(|(unmerged, _)| unmerged == provenance)
    }

    /// Whether the union written at `provenance` is [`Self::unmerged_union`] and a conjunct it is
    /// met with admits `null`.
    fn unmerged_union_meets_null(&self, provenance: &Provenance) -> bool {
        self.unmerged_union
            .as_ref()
            .is_some_and(|(unmerged, meets_null)| unmerged == provenance && *meets_null)
    }

    /// [`Self::lower_union_closed`] past the lowering of its `sibling`, which it meets with every
    /// member of the `oneOf`/`anyOf` `members`: the members are sorted into null-only and real
    /// ones, a single real member collapses ([`Self::lower_sole_union_member`]), and several are
    /// lowered as variants ([`Self::lower_union_variants`]).
    fn lower_union_with_sibling(
        &mut self,
        schema: &Schema,
        hint: &str,
        members: Vec<&SchemaOr>,
        mode: UnionMode,
        sibling: Option<UnionSibling>,
    ) -> Option<Ty> {
        // Two DIFFERENT facts, tracked apart because only one of them can rescue an empty
        // intersection. A null-only MEMBER supplies a branch that `null` validates against. A
        // `"null"` in the enclosing `type` array only *permits* null: `oneOf` still demands exactly
        // one matching member and `anyOf` at least one, so with no null member there is nothing for
        // `null` to match and the schema admits nothing at all. Merging them would make
        // `{type: [integer,'null'], oneOf: [{type: string}]}` — which nothing satisfies —
        // indistinguishable from the same document with a `{type: 'null'}` member, which only
        // `null` satisfies. A `const: null` or an `enum` listing `null` permits it as the `type`
        // array does, where nothing else the schema states refuses it: an untyped branch, which
        // decides nothing about `null`, takes it from either, as it does in the `allOf` spelling
        // (#632).
        let null_from_type_array = NullAdmission::of_own_keywords(schema) == NullAdmission::Admits;
        let mut null_members = 0usize;
        let mut real_members: Vec<&SchemaOr> = Vec::new();
        for member in members {
            if member_is_null_only(member) {
                null_members += 1;
            } else {
                real_members.push(member);
            }
        }

        // Every schema the discriminator names is checked against the members before any path
        // below can return. The collapses do not build a discriminated dispatch at all, so a check
        // made only where one is built dropped a dangling or non-member mapping entry there
        // without looking at it. Resolution here reads identities, not schemas, so it lowers
        // nothing and cannot reorder what the members lower to.
        let discriminator_members = match &schema.discriminator {
            Some(discriminator) => Some(self.discriminator_members(discriminator, &real_members)?),
            None => None,
        };

        // Only null members remained: the exact JSON null type.
        if real_members.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        // The third spelling of the conjunction the `$ref`-sibling and `allOf` arms already guard.
        // A union member that closes a reference cycle resolves to the target's RESERVED id, whose
        // def is the `TypeKind::Reserved` placeholder, so intersecting a variant against it can
        // produce no true answer. Only reachable when there IS a sibling to intersect with: without
        // one, an ordinary recursive union boxes its back-edge and generates, which is what makes a
        // recursive `oneOf` usable at all.
        //
        // This is the DOCUMENT half of the test and it is answered before lowering, for every
        // spelling of the member's `$ref`. The reservation half is applied after each member is
        // lowered, at the two sites below where the member's `Ty` exists; between them the three
        // spellings of one conjunction give one verdict.
        if sibling.is_some() {
            for member in &real_members {
                // A member that is this union's OWN reservation is not an intersection problem:
                // the union is itself, which the collapse and variant paths below reject as `E007`
                // with or without siblings. Left to this guard it would draw `E013` only when
                // siblings are present, so one shape would get two codes.
                if self.member_closes_a_cycle(member, &schema.provenance)
                    && !self.member_is_this_union(member, &schema.provenance)
                    && !self.kept_unmet(schema, member)
                {
                    return self.reject_union_member_cycle(schema);
                }
            }
        }

        // A single real member (the rest were null): `Option<ThatType>`, no enum needed. Re-emit the
        // member's kind as this position's own def so it is the final graph insert, as the allOf
        // single-member collapse does (last-insert invariant: `lower_into_reservation`); a bare
        // `$ref` member would otherwise return an existing id.
        let members = UnionMembers {
            real: real_members,
            mode,
            null_members,
            null_from_type_array,
        };
        if let [member] = members.real[..] {
            return self.lower_sole_union_member(schema, hint, member, &members, sibling);
        }
        self.lower_union_variants(schema, hint, &members, sibling, discriminator_members)
    }

    /// Refuse `member`, a real branch of the union `schema` lowered to `ty`, where it closes a
    /// cycle the union cannot hold; `None` where it is refused. Asked of the sole member and of
    /// every variant alike, right after the member is lowered and before anything is met with it.
    ///
    /// A member whose type is the reservation *this* schema will occupy is the whole union:
    /// `Selfy = Selfy | null` describes nothing a decoder can terminate on, and the emitted
    /// `Deserialize` would open by re-entering itself on the same value, with no base case, and
    /// recurse until the stack is exhausted. It compiles, so nothing downstream can catch it, and
    /// it is `E007` whatever the member count. Asked before the reservation guard below, which
    /// would otherwise answer the narrower question first and hand one shape two codes. The
    /// document-half guard (`member_closes_a_cycle`, in [`Self::lower_union_with_sibling`]) runs
    /// first and would otherwise answer `E013` for this shape whenever the union carries sibling
    /// keywords; it excludes a member that is this union's own reservation
    /// (`member_is_this_union`), so the self-union reaches here and draws `E007` with or without
    /// siblings, and with one member or several.
    /// `a_union_whose_sole_member_is_its_own_reservation_is_rejected` in
    /// `tests/frontend/recursion_reservations.rs` asserts the reported error codes are
    /// **exactly** `[E007]` on both its spellings.
    ///
    /// The test is against *this* schema's own reservation, not against any open one. Asking
    /// `is_in_progress_root(ty.id)` answers the strictly weaker "is the member any open
    /// reservation", which is true of every ordinary recursive schema whose union sits in a
    /// property: `Node.child: {oneOf: [{$ref: Node}, …]}` has a member pointing at `Node`'s
    /// reservation while the union being built is `Nodechild`. That decodes perfectly well — the
    /// member is a *different* type — and rejecting it refuses the most common recursive
    /// construct there is, which `docs/support-matrix.md` lists as supported.
    ///
    /// Then the reservation half of the cycle test, where the union has a sibling to meet the
    /// member with (`has_sibling`). The document half answers only the
    /// `#/components/schemas/…` spelling; a sub-file or remote member reference reaches here
    /// still pointing at a placeholder, which no intersection can compose with. Reported with the
    /// wording the other spellings use, because it is the same fact about the same document.
    /// Without a sibling an ordinary recursive union still boxes its back-edge and generates, and
    /// so does a member of a category a sibling that is a multi-category `type` array alone admits
    /// ([`Self::kept_unmet`]).
    fn check_member_cycle(
        &mut self,
        schema: &Schema,
        member: &SchemaOr,
        ty: Ty,
        has_sibling: bool,
    ) -> Option<()> {
        if self.reservation_at(&schema.provenance) == Some(ty.id) {
            return self.reject_union_member_is_the_union(schema);
        }
        if has_sibling && self.is_in_progress_root(ty.id) && !self.kept_unmet(schema, member) {
            return self.reject_union_member_cycle(schema).map(|_| ());
        }
        Some(())
    }

    /// `met`, a branch's meet with the union's sibling, with an empty meet answered as the exact
    /// null type (inserted under `hint`) where `null_matches`: `null` satisfies the branch and the
    /// sibling alike ([`Self::null_admission`]), so the meet is `null` alone, as the same branch
    /// written inline narrows to (#670, #674). A `$ref` to a union `null` matches through an
    /// untyped branch of its own lowered non-null, which is how the lowered meet comes out empty.
    fn met_or_null(
        &mut self,
        met: Result<Ty, NoMeet>,
        null_matches: bool,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        match met {
            Err(NoMeet::Empty) if null_matches => {
                Ok(self.insert_type(hint, TypeKind::Null, crate::ir::Docs::default(), None))
            }
            met => met,
        }
    }

    /// Whether every one of the variant types `variants` is the exact null type.
    pub(super) fn all_exact_null(&self, variants: impl IntoIterator<Item = Ty>) -> bool {
        variants.into_iter().all(|ty| self.is_exact_null(ty))
    }

    /// [`Self::lower_union_with_sibling`] for a union with one real `member` beside its null-only
    /// ones: the member's type, met with the `sibling`, re-emitted under this schema's own name,
    /// `Option<T>` beside a `null` member, and no enum.
    fn lower_sole_union_member(
        &mut self,
        schema: &Schema,
        hint: &str,
        member: &SchemaOr,
        members: &UnionMembers<'_>,
        sibling: Option<UnionSibling>,
    ) -> Option<Ty> {
        let UnionMembers {
            mode,
            null_members,
            null_from_type_array,
            ..
        } = *members;
        let null_from_member = null_members > 0;
        // The union accepts `null` only through a branch `null` matches, as
        // [`Self::lower_union_variants`] says.
        let mut nullable = null_from_member;
        // The member is lowered under this schema's own hint and then re-emitted below, so its
        // own lowering is a second copy of the same type (#926).
        let member_from = self.graph_mark();
        let mut inner = self.lower_schema_or(member, hint)?;
        // Everything the sibling meet below inserts has an id at or above this mark; the
        // member and the sibling were lowered before it.
        let mark = self.graph_mark();
        // The member's OWN nullability, before the intersection overwrites `inner`. Needed
        // below when the sibling is not entitled to decide.
        let member_nullable = inner.nullable;
        // An untyped object member's non-null struct decides nothing about `null` either
        // (#567), so it takes `null` from a sibling or a meet as `Value` does; so does any
        // other member whose own keywords leave `null` undecided (#574).
        let member_untyped = self.branch_takes_permitted_null(member, inner);
        // Whether `null` satisfies the member met with the sibling, by the one decision
        // ([`Self::null_admission`]): an untyped object or array member, a nested union `null`
        // matches, and a member of scalar keywords alone are branches `null` matches as a
        // member that states it is.
        let null_matches_member = self
            .branch_null_admission(member)
            .and(self.union_sibling_null_admission(schema))
            .matches();
        // A `null` member beside a member `null` matches too puts `null` in two branches,
        // which fails a `oneOf`'s exactly-one rule (#563, #622, #627).
        let null_twice =
            mode == UnionMode::OneOf && null_members + usize::from(null_matches_member) > 1;
        self.check_member_cycle(schema, member, inner, sibling.is_some())?;
        // The member is some *other* type's still-open reservation — the cycle-closing
        // `$ref` of an ordinary recursive schema. Its kind may not be read: cloning a
        // `TypeKind::Reserved` inserts a second reservation nothing will ever `fill`, which
        // `check_invariants` reports as `E011` against a document that is not malformed.
        //
        // A truthful answer exists and needs no def of its own: the member's own `Ty`, boxed so
        // the cycle has a finite size and optional because the `null` member is what collapsed
        // away. That is `Option<Box<T>>` — what `docs/support-matrix.md` promises for this
        // construct, and what the direct `{$ref: T}` spelling already produces.
        //
        // Inserting no def is safe exactly where this union is not itself the body of a type
        // whose root id was reserved before lowering began. Where it is, the caller pops the
        // last insert and lifts it into that reserved root, so returning a foreign id would
        // relocate the wrong def and dangle the component. `ensure_component`, `ensure_resolved`
        // and `ensure_remote` recognise that shape as a nullable alias before they reserve
        // anything, following a member that is a bare alias of the open target to it whichever
        // of the two `components.schemas` declares first (#1189); a body that still reaches here
        // is refused rather than mis-assembled.
        if self.is_reservation(inner.id) {
            if self.reservation_at(&schema.provenance).is_some() {
                return self.reject_self_referential_union(
                    schema,
                    "this schema's whole body is a union whose only non-null member is a \
                         `$ref` that closes a reference cycle, so the schema names no shape of its \
                         own and cannot be given a generated type",
                );
            }
            // A sibling meet is refused above, so here there is no sibling, or one that is a
            // multi-category `type` array alone admitting the target's category, and a target
            // whose keywords decide nothing takes the array's `null` (#574).
            let takes_null = inner.nullable || (null_from_type_array && member_untyped);
            inner.nullable = (takes_null || nullable) && !null_twice;
            inner.boxed = true;
            return Some(inner);
        }
        // An untyped member accepts the `null` the `type` array permits (#574), as the
        // multi-member path's untyped variants do, before the sibling meet so the other
        // sibling keywords can still take it away.
        if null_from_type_array && member_untyped {
            inner.nullable = true;
        }
        if let Some(sibling) = sibling {
            // The null-only MEMBER's branch was stripped out above, BEFORE this intersection,
            // so `inner` carries `nullable: false` and `type_accepts_null` — which reads
            // `Ty::nullable` — cannot see it. Restore exactly that branch. Without it the
            // intersection reports empty for a schema `null` genuinely satisfies, and the
            // `$ref` spelling of the identical instance set, where the target's nullability
            // rides on its own `Ty`, generates while this one rejects.
            //
            // `null_from_type_array` is deliberately NOT restored here: it supplies no branch,
            // and it already reaches the intersection on the sibling side, where it belongs —
            // it can narrow what the result accepts, never create something to accept.
            inner.nullable = inner.nullable || null_from_member;
            let constrained_hint = format!("{hint}Constrained");
            let met = self.meet_scoped_and_report(
                sibling.refiner,
                |ctx, reach| {
                    // A member uninhabited on its own leaves the union so whatever the
                    // sibling is (#615), rather than an empty meet that rejects it below.
                    let met = ctx.meet_union_sibling(
                        schema,
                        inner,
                        sibling.refiner,
                        reach,
                        &constrained_hint,
                    );
                    ctx.uninhabited_union_meet(inner, met)
                },
                |ctx| {
                    ctx.reject_unscoped_union_sibling(
                        schema,
                        "the union's sole non-null member states no JSON category, and the \
                             enclosing schema's untyped sibling keywords settle none for it — they \
                             are both object keywords and array keywords, or its `type` array \
                             admits another category beside theirs — so no single Rust type \
                             represents what they constrain of it",
                    )
                },
                schema,
                unreached_message,
            )?;
            // As the multi-member path narrows such a branch.
            let met = self.met_or_null(met, null_matches_member, &constrained_hint);
            let Ok(constrained) = met else {
                // Neither side admits null and the non-null shapes do not meet, so nothing is
                // left to collapse to. The terminal code matches the multi-variant path below,
                // which rejects with `E007` once every variant has been excluded — but only the
                // terminal code: that path also emits a per-variant `W011`, and this one
                // deliberately does not, because `W011` describes a construct dropped from
                // output that still exists, and here no enum is generated at all.
                //
                // The message says "empty or unrepresentable" for the same reason Site A's
                // does: it covers both, and the sole non-null member is named because there
                // is exactly one, so the author needs no index to find it.
                return self.reject_branchless_union(
                    schema,
                    "the union's sole non-null member and the enclosing schema's own sibling \
                         keywords have an empty or unrepresentable intersection, leaving the union \
                         with no variant",
                );
            };
            // The intersection owns the answer for nullability too — but ONLY where the sibling
            // is entitled to give one, which is a question about the SIBLING. Reading the
            // enclosing schema's `type` instead answered it wrongly in both directions: the two
            // disagree whenever a multi-type array is deleted for lowering, and whenever the
            // sibling speaks through `enum`/`const` rather than `type`. An object applicator
            // denies nothing and must not remove the union's own acceptance.
            nullable = if sibling.speaks_about_null {
                constrained.nullable
            } else {
                member_nullable || null_from_member
            };
            inner = constrained;
        }
        let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
        // The meet's result is re-emitted under this schema's name, so the meet's own inserts
        // (`…Constrained`, and whatever it built on the way) are unused unless `kind` reaches
        // them (#462). Without a sibling nothing was inserted since `mark`.
        let mut ty = self.reemit_meet(schema, hint, mark, kind);
        // The member's own lowering, which the re-emitted def replaced, is unused unless the
        // re-emitted kind reaches it or a memo holds it (#926).
        self.elide_unused_lowering(member_from..mark);
        // The sibling meet narrowed the member to the exact null type beside a `null` member:
        // `null` matches both branches and fails exactly-one, and the member accepts nothing
        // else, so nothing satisfies the schema, as the multi-member path below rejects for
        // the same branches (#632).
        if null_twice && sibling.is_some() && self.is_exact_null(inner) {
            return self.reject_one_of_null_in_every_branch(schema);
        }
        ty.nullable = (inner.nullable || nullable) && !null_twice;
        ty.boxed = inner.boxed;
        Some(ty)
    }

    /// [`Self::lower_union_with_sibling`] for a union with two or more real members: each is
    /// lowered and met with the `sibling` as a variant, the variants a meet excludes, narrows to
    /// `null`, or leaves indistinguishable are dropped or merged, and what remains is one variant's
    /// type or an enum dispatched by [`Self::union_strategy`]. `discriminator_members` is the
    /// discriminator's resolution against the members, where the union has one.
    fn lower_union_variants(
        &mut self,
        schema: &Schema,
        hint: &str,
        members: &UnionMembers<'_>,
        sibling: Option<UnionSibling>,
        discriminator_members: Option<DiscriminatorMembers>,
    ) -> Option<Ty> {
        let UnionMembers {
            real: ref real_members,
            mode,
            null_members,
            ..
        } = *members;
        let null_from_member = null_members > 0;
        // The union accepts `null` only through a branch `null` matches (#574): a `null` member,
        // or a real branch that accepts it after its sibling meet. The `type` array alone adds
        // nothing, so it does not start this; an untyped branch, which decides nothing about
        // `null`, takes the array's `null` below, and the meet keeps it where the other sibling
        // keywords admit it.
        let mut nullable = null_from_member;

        // Whether `null` satisfies a branch met with the sibling, by the one decision
        // ([`Self::null_admission`]): for a meet that lowered empty, and for the branches a
        // `oneOf` counts `null` in.
        let sibling_admission = self.union_sibling_null_admission(schema);
        let MemberLowering {
            mut variants,
            hoisted_null,
            meet_inserts,
            met_into_null,
            own_nulls,
            member_lowerings,
        } = self.lower_variant_members(schema, hint, members, sibling, sibling_admission)?;
        // Hoist a variant's own nullability up to the union.
        nullable = nullable || hoisted_null;
        if variants.is_empty() {
            // The same blind spot as the sole-member site above, on the pre-existing path: every
            // REAL variant is impossible, but a null-only member's branch was stripped out before
            // any of them were intersected, so it is not among the variants that just vanished.
            // When the sibling admits null too, `null` still satisfies the whole schema and the
            // exact JSON null type is the answer — the same type the all-null-members branch above
            // returns for the same reason. Again only the MEMBER-derived flag can rescue: a
            // `"null"` in the enclosing `type` array leaves nothing for `null` to match.
            if null_from_member
                && sibling.is_none_or(|sibling| self.refiner_accepts_null(sibling.refiner))
            {
                return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
            }
            return self.reject_branchless_union(
                schema,
                "union sibling constraints make every variant impossible",
            );
        }
        // A `oneOf` admits `null` only where exactly one branch does (#563): `null` matches a
        // `null` member, and a real branch wherever the one decision says `null` satisfies it met
        // with the sibling ([`Self::null_admission`]) — one that states `null`, one the sibling
        // meet narrowed to the exact null type (#645), an untyped object or array (#622), a nested
        // union `null` matches (#628), and one of scalar keywords alone or that states nothing
        // (#592). Two of them, whether or not any of the variants merge below, put `null` in two
        // branches, which fails exactly-one. Decided here from the branches themselves, before a
        // meet the union is held back for ([`Self::unmerged_union`]); the `null` that meet gives a
        // branch is settled by the one decision after it ([`Self::settle_null_admission`]).
        let null_branches = real_members
            .iter()
            .filter(|member| self.null_survives(member, sibling_admission))
            .count()
            + if sibling_admission.matches() {
                null_members
            } else {
                0
            };
        let null_twice = mode == UnionMode::OneOf && null_branches > 1;
        if null_twice {
            nullable = false;
        }
        // The sibling meet left every one of several `oneOf` branches the exact null type:
        // `type: [string, 'null']` beside untyped `items` branches, which constrain arrays alone,
        // meets each of them in `null` only. `null` then matches every branch, which fails the
        // exactly-one rule, so nothing satisfies the schema, and its `allOf` spelling (the `type`
        // as a member beside the union) is rejected for it (#632). Merged below, the branches
        // would be one `()` variant with `W001`, a type for a schema no value satisfies. The
        // `anyOf` counterpart, which needs one match, is the null type below (#625). A `null`
        // member is one more branch `null` matches, so a single variant met in `null` beside one
        // is the same empty intersection.
        if mode == UnionMode::OneOf
            && sibling.is_some()
            && variants.len() + null_members > 1
            && self.all_exact_null(variants.iter().map(|variant| variant.ty))
        {
            return self.reject_one_of_null_in_every_branch(schema);
        }
        // The meet left only some branches the exact null type: `type: [string, 'null']` beside
        // an untyped `items` branch and a `{type: string}` one. Each such branch is a branch `null`
        // matches, as a `null` member is, so the typed branches are the variants, as the `allOf`
        // spelling lowers them. An `anyOf` hoists `null` to the union's `Option` (#633); a `oneOf`
        // does too where that branch is the only one `null` matches, and otherwise `null` fails
        // its exactly-one rule and the union admits no `null` (#645). Kept, each would be a `()`
        // variant beside the typed ones — merged with `W001` where there are several — so one
        // instance set would have two public types by spelling. Dropped here, before the merge
        // below, so the branches are not reported as indistinguishable variants of an enum that
        // does not have them. The dropped branches' meet inserts (their `…Constrained` aliases)
        // are elided unless a surviving variant reaches them. A branch that is the null type on
        // its own (`const: null`) stays the `()` variant it is with no sibling and in the `allOf`
        // spelling, except where a `oneOf` drops it below (#657).
        if variants
            .iter()
            .any(|variant| met_into_null.contains(&variant.member()))
            && !self.all_exact_null(variants.iter().map(|variant| variant.ty))
        {
            let dropped: Vec<usize> = variants
                .iter()
                .map(LoweredVariant::member)
                .filter(|member| met_into_null.contains(member))
                .collect();
            variants.retain(|variant| !met_into_null.contains(&variant.member()));
            nullable = !null_twice;
            if variants.len() > 1 {
                let roots: Vec<TypeId> = variants.iter().map(|variant| variant.ty.id).collect();
                let reached = reachable_types(&self.graph, &roots);
                let inserts = dropped
                    .iter()
                    .filter_map(|&member| meet_inserts.get(member))
                    .flat_map(|range| range.clone().map(TypeId));
                self.elide_unreached(inserts, &reached);
            }
        }
        // A `oneOf` branch that is the exact null type on its own (`const: null`, `enum: [null]`),
        // where the count above finds another branch `null` matches, puts `null` in two branches.
        // `null` fails exactly-one, and the branch accepts nothing else, so it accepts nothing the
        // union does: it is no variant, and the other branches are, in the sibling spelling and in
        // the `allOf` one, whose union member is lowered on its own. Kept, it would be a `()` arm
        // that decodes the `null` the union refuses (#657). The types its own lowering inserted
        // are elided unless something else names them: a `$ref`'d component stays. Only where
        // another variant is left.
        let is_own_null = |member: usize| own_nulls.iter().any(|(own, _)| *own == member);
        if null_twice
            && !own_nulls.is_empty()
            && variants
                .iter()
                .any(|variant| !is_own_null(variant.member()))
        {
            variants.retain(|variant| !is_own_null(variant.member()));
            self.elide_unused_lowerings(own_nulls.into_iter().map(|(_, lowered)| lowered));
        }
        // A `oneOf` needs exactly one branch to match, and its typed trial matching decides that
        // by which variants decode. Variants that lower to the same generated type decode the same
        // values, so every value one of them accepts fails exactly-one: branches of nothing but
        // `required` beside `type: object` (#402), or bare ones that each lower to
        // `serde_json::Value`. Two inline objects of one structure, or two string enums of one
        // value set, are distinct generated items that decode the same values too (#492). They
        // become one variant — the whole union is that type when every
        // variant shares it, as the `$ref`-sibling collapse answers for the same branches — and
        // the distinctions the generated type does not carry are reported, not dropped in silence.
        // A discriminator tells such variants apart by tag, so it keeps them all; an `anyOf`
        // decodes with any one match, so it does too. A `$ref`'s own `oneOf` sibling is collapsed
        // by the `$ref` arm after the meet instead ([`Self::unmerged_union`]), and an `allOf`'s
        // union member by the `allOf`'s scalar meet ([`Self::scalar_meet_union`], #678).
        let held_for_scalar_meet = self.scalar_meet_union.as_ref() == Some(&schema.provenance);
        if mode == UnionMode::OneOf
            && schema.discriminator.is_none()
            && !self.is_unmerged_union(&schema.provenance)
            && !held_for_scalar_meet
        {
            self.merge_indistinguishable_variants(schema, &mut variants);
        }
        // The sibling meet left every branch the exact null type: `type: [string, 'null']` beside
        // untyped `items` branches, which constrain arrays alone, meets each of them in `null`
        // only. An `anyOf` needs one match, so `null` is the one value the union accepts and the
        // union is the null type, as its `allOf` spelling (the `type` as a member beside the
        // union) is (#625). Kept as an enum it would be a union of `()` variants beside a `String`
        // constraint, which reads as a string-typed union.
        if mode == UnionMode::AnyOf
            && sibling.is_some()
            && variants.len() > 1
            && self.all_exact_null(variants.iter().map(|variant| variant.ty))
        {
            variants.truncate(1);
        }
        if let [variant] = &variants[..] {
            return self.lower_sole_variant(
                schema,
                hint,
                variant.ty,
                nullable,
                meet_inserts,
                member_lowerings,
            );
        }

        let (
            UnionDispatch {
                strategy,
                variants: dispatches,
            },
            deferred,
        ) = self.union_strategy(
            schema,
            &variants,
            mode,
            discriminator_members.as_ref(),
            true,
        )?;
        let fresh = self.fresh_member_ids(&member_lowerings, &meet_inserts);
        // A dispatch waiting on a reservation is decided, with its pinned names, once lowering
        // has filled it ([`Self::decide_deferred_discriminators`]).
        let deferred = deferred.map(|id| DeferredDiscriminator {
            id,
            schema: schema.clone(),
            hint: hint.to_owned(),
            mode,
            lowered: variants.clone(),
            discriminator: discriminator_members
                .expect("only a discriminated union's dispatch is deferred"),
            real: real_members
                .iter()
                .map(|member| (*member).clone())
                .collect(),
            fresh: fresh.clone(),
            reports_untyped: !self.is_unmerged_union(&schema.provenance),
        });
        if let (UnionStrategy::Discriminated { tag_field, .. }, None) = (&strategy, &deferred) {
            self.name_pinned_variants(
                hint,
                tag_field,
                &mut variants,
                &dispatches,
                &PinnedMembers {
                    real: real_members,
                    fresh: &fresh,
                },
            );
        }
        // The member each variant came from, and every member it stands for.
        let labels: Vec<usize> = variants.iter().map(LoweredVariant::member).collect();
        let merged_members: Vec<Vec<usize>> = variants
            .iter()
            .map(|variant| variant.members.clone())
            .collect();
        let union = Union {
            variants: variants
                .into_iter()
                .zip(dispatches)
                .map(|(variant, dispatch)| variant.into_variant(dispatch))
                .collect(),
            strategy,
        };
        // A `$ref`'s own `oneOf` sibling is reported by the `$ref` arm once the meet has made its
        // branches what they are ([`Self::collapse_met_union`]), as its merge is. While an `allOf`
        // gathers its members the report waits for its scalar meet, which may give a `Value`
        // variant a type ([`Self::held_untyped_warnings`], #660).
        if !self.is_unmerged_union(&schema.provenance) {
            let untyped = self.untyped_one_of_variants(&union, &labels);
            self.report_untyped_one_of(HeldUntypedWarning {
                provenance: schema.provenance.clone(),
                untyped,
            });
        }
        let mut ty = self.insert_schema_type(schema, hint, TypeKind::Union(union));
        self.deferred_discriminators.extend(deferred);
        self.union_variant_members.insert(ty.id, merged_members);
        if held_for_scalar_meet {
            self.scalar_meet_unmerged.insert(ty.id);
        }
        ty.nullable = nullable;
        Some(ty)
    }

    /// Whether `null` satisfies the real `member` met with a sibling `null` satisfies as
    /// `sibling_admission` says, by the one decision ([`Self::null_admission`]).
    fn null_survives(&self, member: &SchemaOr, sibling_admission: NullAdmission) -> bool {
        self.branch_null_admission(member)
            .and(sibling_admission)
            .matches()
    }

    /// [`Self::lower_union_variants`]' lowering of each real member as a variant, met with the
    /// `sibling`: a member the meet excludes is no variant (`W011`), and the halves of the sibling
    /// no branch reached are reported once every member is met. `sibling_admission` is what the
    /// sibling holds `null` to. Every variant type inserts before the union def the caller
    /// inserts. `None` where a member is refused.
    fn lower_variant_members(
        &mut self,
        schema: &Schema,
        hint: &str,
        members: &UnionMembers<'_>,
        sibling: Option<UnionSibling>,
        sibling_admission: NullAdmission,
    ) -> Option<MemberLowering> {
        let UnionMembers {
            real: ref real_members,
            mode,
            null_from_type_array,
            ..
        } = *members;
        let mut lowering = MemberLowering::default();
        let mut used_hints: HashSet<String> = HashSet::new();
        let mut reach = ScopeReach::default();
        for (index, member) in real_members.iter().enumerate() {
            let lowered_from = self.graph_mark();
            let (mut ty, ref_name) =
                self.lower_union_variant(member, &format!("{hint}Variant{index}"))?;
            let lowered = lowered_from..self.graph_mark();
            lowering.member_lowerings.push(lowered.clone());
            self.check_member_cycle(schema, member, ty, sibling.is_some())?;
            // A cycle-closing branch a multi-category `type` array alone admits meets that sibling
            // in itself: it is the variant it is without one.
            let sibling = if sibling.is_some()
                && self.is_in_progress_root(ty.id)
                && self.kept_unmet(schema, member)
            {
                None
            } else {
                sibling
            };
            // Held back for a meet with nothing of its own to meet first, an untyped object branch
            // accepts `null` wherever what it is met with does (#567), as an untyped `allOf` member
            // does (#541): its non-null struct decides nothing. It accepts it here, so the meet
            // keeps `null` in that branch exactly where the other conjuncts admit it. Only where a
            // conjunct it is met with admits `null` ([`Self::unmerged_union_meets_null`]): met with
            // untyped objects alone, nothing does, and the branch stays the non-null struct every
            // other spelling of that composition gives it.
            //
            // A branch that states nothing (`true`, `{}`) accepts `null` there too, as it does
            // beside untyped sibling keywords (#592), but `Value` is the identity of the meet, so
            // the meet hands it the conjunct's `null` again. An `anyOf`, which needs one match,
            // hoists it here, and the one decision carries it once on the meet's position
            // ([`Self::settle_null_admission`]). A `oneOf` does not hoist it: the meet's copy is
            // the branch's answer, which the `oneOf` reads after the meet
            // ([`Self::collapse_met_union`]) and the one decision settles (#597).
            if sibling.is_none()
                && self.unmerged_union_meets_null(&schema.provenance)
                && self.branch_takes_conjunct_null(member, ty)
                && (mode == UnionMode::AnyOf || self.branch_leaves_null_undecided(member, ty))
            {
                ty.nullable = true;
            }
            // An untyped branch accepts the `null` the `type` array permits (#574), before the
            // sibling meet so the other sibling keywords can still take it away. Without a sibling
            // to meet (a cycle-closing branch a multi-category `type` array alone admits) nothing
            // else would give it the array's `null`.
            if null_from_type_array && self.branch_takes_permitted_null(member, ty) {
                ty.nullable = true;
            }
            // Untyped sibling keywords are a conjunct that leaves `null` to the union, as an
            // untyped `allOf` member beside it does: they admit `null` where a branch admits it
            // itself ([`union_branch_admits_null`]), and an untyped object branch then takes it
            // from the meet, as it does in that spelling (#586). So does a branch that states
            // nothing and lowers to `Value` (`true`, `{}`), which the meet otherwise leaves as it
            // is (#588).
            if sibling.is_some_and(|sibling| {
                !sibling.speaks_about_null && matches!(sibling.refiner, Refiner::Scoped(_))
            }) && union_branch_admits_null(schema)
                && self.branch_takes_conjunct_null(member, ty)
            {
                ty.nullable = true;
            }
            let mark = self.graph_mark();
            // A member with no sibling to meet (none at all, or a cycle-closing branch kept
            // unmet) inserted nothing for it: an empty range keeps `meet_inserts` aligned with
            // member positions, which the met-into-null drop reads it by (#1226).
            if sibling.is_none() {
                lowering.meet_inserts.push(mark..mark);
            }
            if let Some(sibling) = sibling {
                let null_before = self.is_exact_null(ty);
                let constrained_hint = format!("{hint}Variant{index}Constrained");
                let met = self.meet_union_sibling(
                    schema,
                    ty,
                    sibling.refiner,
                    &mut reach,
                    &constrained_hint,
                );
                // The lowered meet is empty, but `null` satisfies the branch and the sibling
                // alike: the branch is one `null` matches, not one the sibling excludes (#670).
                let narrowed_to_null = matches!(met, Err(NoMeet::Empty))
                    && self.null_survives(member, sibling_admission);
                let met = self.met_or_null(met, narrowed_to_null, &constrained_hint);
                // Every type this branch's meet inserted, the `null` it narrowed to included,
                // elided with the branch once it is dropped.
                lowering.meet_inserts.push(mark..self.graph_mark());
                ty = match met {
                    Ok(null) if narrowed_to_null => {
                        lowering.met_into_null.insert(index);
                        null
                    }
                    // A branch already the exact null type (`const: null`) is narrowed by nothing
                    // the sibling admits `null` beside, so it stays its own `()` def; the meet's
                    // copy (`…Constrained`) would leave that def an unused alias beside it (#653).
                    Ok(intersection) if null_before && self.is_exact_null(intersection) => {
                        for id in (mark..self.graph_mark()).map(TypeId) {
                            self.graph.elide(id);
                        }
                        Ty {
                            id: ty.id,
                            ..intersection
                        }
                    }
                    Ok(intersection) => {
                        if !null_before && self.is_exact_null(intersection) {
                            lowering.met_into_null.insert(index);
                        }
                        intersection
                    }
                    // A branch that is the exact null type on its own (`const: null`), beside a
                    // sibling that denies `null`, accepts nothing: it drops out unreported, as a
                    // `{type: 'null'}` member does, since whether `null` is valid is the one null
                    // decision's to say ([`Self::null_admission`]) and every spelling drops it alike.
                    Err(NoMeet::Empty) if null_before => continue,
                    // The sibling constraints make this branch impossible; JSON Schema simply
                    // removes it from the union's accepted set. Acknowledge it, because a variant
                    // vanishing from the generated enum is otherwise invisible.
                    Err(NoMeet::Empty) => {
                        self.warn_excluded_union_branch(&schema.provenance, index);
                        continue;
                    }
                    // Object and array applicators together beside a branch that states no
                    // category (`{}`) have no single category to establish for it.
                    Err(NoMeet::Unrepresentable) if reach.uncategorised => {
                        let message = format!(
                            "union member {index} states no JSON category, and the enclosing \
                             schema's untyped sibling keywords settle none for it — they are both \
                             object keywords and array keywords, or its `type` array admits \
                             another category beside theirs — so no single Rust type represents \
                             what they constrain of it"
                        );
                        return self.reject_unscoped_union_sibling(schema, &message);
                    }
                    // The branch does admit values the siblings admit, but no Rust type holds
                    // them, so it can be neither kept nor dropped without refusing them.
                    Err(NoMeet::Unrepresentable) => {
                        let message = format!(
                            "union member {index} and the enclosing schema's own sibling keywords \
                             share values that no single Rust type represents, so the member can \
                             be neither generated nor dropped"
                        );
                        return self.reject_unrepresentable_meet(schema, &message);
                    }
                };
            }
            // Hoist a variant's own nullability up to the union: a `null` payload then resolves at the
            // outer `Option<Union>` (→ `None`), and the discriminated/disjoint dispatch below only
            // ever inspects non-null content — otherwise a variant like `{type: [string, null]}`
            // would be categorized `String` yet have no `null` arm in the custom `Deserialize`.
            lowering.hoisted_null = lowering.hoisted_null || ty.nullable;
            if !lowering.met_into_null.contains(&index) && self.is_exact_null(ty) {
                lowering.own_nulls.push((index, lowered));
            }
            ty.nullable = false;
            let base_hint = ref_name
                .clone()
                .unwrap_or_else(|| format!("{hint}Variant{index}"));
            // Keep hints unique so `name` allocates one identifier per variant (the hint keys the
            // per-union variant table).
            let mut name_hint = base_hint.clone();
            let mut disambiguator = 2usize;
            while !used_hints.insert(name_hint.clone()) {
                name_hint = format!("{base_hint}{disambiguator}");
                disambiguator += 1;
            }
            lowering.variants.push(LoweredVariant {
                name_hint,
                ty,
                ref_name,
                members: vec![index],
            });
        }

        debug_assert_eq!(
            lowering.meet_inserts.len(),
            real_members.len(),
            "one meet range per real member, by member position"
        );
        if let Some(sibling) = sibling {
            for keywords in unreached_halves(sibling.refiner, &reach) {
                self.warn_unreached_union_sibling(schema, unreached_message(keywords));
            }
        }
        Some(lowering)
    }

    /// [`Self::lower_union_variants`] left one variant, of type `inner`: the union is that
    /// variant's type, re-emitted under this schema's name, `nullable` where the union admits
    /// `null`. `meet_inserts` and `member_lowerings` are the ids the members' sibling meets and
    /// own lowerings inserted, elided where the result no longer reaches them.
    fn lower_sole_variant(
        &mut self,
        schema: &Schema,
        hint: &str,
        mut inner: Ty,
        nullable: bool,
        meet_inserts: Vec<std::ops::Range<u32>>,
        member_lowerings: Vec<std::ops::Range<u32>>,
    ) -> Option<Ty> {
        // The one variant left is some type's still-open reservation: a `oneOf` that lists
        // the same cycle-closing `$ref` more than once, merged into one (#1152). Its kind may
        // not be read, for the reason [`Self::lower_sole_union_member`] gives, so the
        // position is answered as that path answers its reservation: the member's own `Ty`,
        // boxed so the cycle has a finite size, with no def of its own. Where this union is
        // itself the body of a reserved root, the caller lifts the last insert into that
        // root, so a foreign id cannot be handed back, and the shape is refused as that path
        // refuses it. A body whose members are bare `$ref`s to that one open target, directly
        // or through bare aliases of it, is a nullable alias instead
        // ([`Self::nullable_alias_back_edge`], #1176, #1189), so its outcome does not depend
        // on which of them `components.schemas` declares first.
        if self.is_reservation(inner.id) {
            if self.reservation_at(&schema.provenance).is_some() {
                return self.reject_self_referential_union(
                    schema,
                    "this schema's whole body is a union whose members all lower to one \
                     `$ref` that closes a reference cycle, so the schema names no shape of \
                     its own and cannot be given a generated type",
                );
            }
            // Nothing names whatever the members' own lowerings or meets inserted: the type of
            // a branch the sibling excludes, and the `null` a branch's meet narrowed it to.
            // These two calls keep `TypeGraph::emitted` exact for the elisions lowering runs
            // after this one, which read it for their roots
            // ([`Self::elide_unused_lowering`]). The generated output is the same
            // without them, because `elide_unreachable` withholds every type nothing reaches
            // once lowering ends (#924), so an output fixture holds the types they elide out
            // of the module but does not pin these calls (#1190):
            // `a_one_of_repeating_a_cycle_closing_ref_drops_the_branches_its_sibling_excludes`
            // in `tests/frontend/recursion_reservations.rs`.
            self.elide_unreached(
                meet_inserts.into_iter().flatten().map(TypeId),
                &HashSet::new(),
            );
            self.elide_unused_lowerings(member_lowerings);
            inner.nullable = inner.nullable || nullable;
            inner.boxed = true;
            return Some(inner);
        }
        let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
        // The sole variant is re-emitted under this schema's name, so the meets' inserts —
        // the surviving variant's `…Constrained` def, and whatever an excluded member's meet
        // left — are unused unless `kind` reaches them (#462).
        let reached = reachable_types(&self.graph, &kind_edges(&kind));
        self.elide_unreached(meet_inserts.into_iter().flatten().map(TypeId), &reached);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        // So is the sole variant's own type, whose kind the union now is, unless something
        // else names it, as a union the one decision collapses to one variant leaves it
        // ([`Self::settle_null_admission`]). So is every member's own lowering, the
        // `…Variant<N>` types with whatever they built on the way (`…Item`), whether its
        // member is the surviving variant, one the sibling excluded, or one the merge folded
        // into it (#987).
        self.elide_unused_lowering(inner.id.0..inner.id.0 + 1);
        self.elide_unused_lowerings(member_lowerings);
        ty.nullable = inner.nullable || nullable;
        ty.boxed = inner.boxed;
        Some(ty)
    }

    /// How the enum of `variants` tells them apart: by the discriminator, where `schema` has one
    /// that dispatches them (`discriminator_members` is its resolution against the members),
    /// then by JSON category or a unique required key where they are disjoint, and otherwise by
    /// trial matching under `mode`. Rejected (`E007`) where a `defaultMapping` names a member the
    /// sibling keywords excluded, or a component member's own name selects another member. A
    /// `mapping` entry naming a member that cannot carry the tag property is `W011` at the entry.
    ///
    /// Where `may_defer` holds and a variant of a discriminated union reaches a reservation still
    /// being lowered ([`Self::awaits_reservation`]), nothing about that variant is known yet, so
    /// the decision waits until every reservation is filled: the dispatch returned is a
    /// provisional one, every variant a [`VariantDispatch::Deferred`] marker, and the number
    /// beside it names the deferred union; the caller records the union under that number for
    /// [`Self::decide_deferred_discriminators`]. Deciding it now would
    /// make the order components are declared in choose the dispatch.
    pub(super) fn union_strategy(
        &mut self,
        schema: &Schema,
        lowered: &[LoweredVariant],
        mode: UnionMode,
        discriminator_members: Option<&DiscriminatorMembers>,
        may_defer: bool,
    ) -> Option<(UnionDispatch, Option<usize>)> {
        let types: Vec<Ty> = lowered.iter().map(|variant| variant.ty).collect();
        let types = types.as_slice();
        let dispatch = if let (Some(discriminator), Some(resolved)) =
            (&schema.discriminator, discriminator_members)
        {
            if may_defer
                && types
                    .iter()
                    .any(|ty| self.awaits_reservation(*ty, &mut HashSet::new()))
            {
                let union = self.deferred_discriminator_count;
                self.deferred_discriminator_count += 1;
                let provisional = UnionDispatch {
                    strategy: UnionStrategy::Discriminated {
                        tag_field: discriminator.property_name.clone(),
                        default_variant: None,
                        mode,
                        open: false,
                    },
                    variants: (0..types.len())
                        .map(|member| VariantDispatch::Deferred { union, member })
                        .collect(),
                };
                return Some((provisional, Some(union)));
            }
            // `discriminator_members` already refused a `defaultMapping` naming a non-member. A
            // member the enclosing schema's sibling keywords excluded (`W011`) is the one way left
            // for it to have no variant, and a fallback to a branch the enum does not have cannot
            // be quietly downgraded to another dispatch strategy either.
            let default_variant = match resolved.default {
                None => None,
                Some(member) => {
                    let Some(variant) = lowered
                        .iter()
                        .position(|variant| variant.member() == member)
                    else {
                        return self.reject_unrepresentable_union(
                            schema,
                            "`discriminator.defaultMapping` names a member that the enclosing \
                             schema's own sibling keywords exclude, so there is no branch to fall \
                             back to",
                        );
                    };
                    Some(variant)
                }
            };
            for uncarried in self.uncarried_mapping_entries(lowered, resolved) {
                self.warn_uncarried_tag(schema, discriminator, &uncarried);
            }
            let mut discriminated = Some(self.discriminated_strategy(
                lowered,
                &discriminator.property_name,
                resolved,
                default_variant,
                mode,
            ));
            if let Some(UnionDispatch {
                variants: variant_dispatch,
                ..
            }) = &mut discriminated
            {
                let unselectable: Vec<usize> = variant_dispatch
                    .iter()
                    .enumerate()
                    .filter(|&(index, dispatch)| {
                        // A variant that cannot carry the tag is already tried by its own schema.
                        matches!(
                            dispatch,
                            VariantDispatch::Discriminated {
                                tags,
                                category: None,
                                untagged: None,
                            } if tags.is_empty()
                        ) && default_variant != Some(index)
                    })
                    .map(|(index, _)| index)
                    .collect();
                let component = |index: usize| {
                    lowered[index]
                        .ref_name
                        .as_deref()
                        .filter(|name| is_schema_component_name(name))
                };
                // A component member whose implicit value a mapping key claims for another
                // member: the document routes that member's own name elsewhere, which no
                // dispatch can honour.
                if let Some(&index) = unselectable
                    .iter()
                    .find(|&&index| component(index).is_some())
                {
                    let implicit = component(index).unwrap_or_default().to_owned();
                    return self.reject_unselectable_discriminated_variant(
                        schema,
                        discriminator,
                        lowered[index].member(),
                        &implicit,
                    );
                }
                // A member that is no component — inline, or a pointer into another schema — has
                // no implicit value, and with no mapping entry naming it and no tag property
                // pinning a free value of its own, no tag selects it;
                // inventing a tag for it would be one no server sends. Where no variant carries a
                // tag at all, the discriminator dispatches nothing, and the members' own schemas
                // decode the union, as for one with no discriminator. Otherwise the tagged
                // members keep their dispatch — a tag that names one selects it, exactly as the
                // document says — and the untagged ones are tried by their schemas only when the
                // tag is absent or names no tagged member. Dropping the dispatch for the whole
                // union instead would let an `anyOf` trial pick a tagged member the payload's own
                // tag does not name.
                if !unselectable.is_empty() {
                    let members: Vec<usize> = unselectable
                        .iter()
                        .map(|&index| lowered[index].member())
                        .collect();
                    let dispatches = variant_dispatch.iter().any(|dispatch| {
                        matches!(
                            dispatch,
                            VariantDispatch::Discriminated { tags, .. } if !tags.is_empty()
                        )
                    });
                    self.warn_untagged_discriminated_members(discriminator, &members, dispatches);
                    if dispatches {
                        for &index in &unselectable {
                            if let VariantDispatch::Discriminated { untagged, .. } =
                                &mut variant_dispatch[index]
                            {
                                *untagged =
                                    Some(self.type_specificity(types[index], &mut HashSet::new()));
                            }
                        }
                    } else {
                        discriminated = None;
                    }
                }
            }
            // A tag-selected member that may arrive with no tag has nowhere to go but a
            // `defaultMapping` (#1421).
            if let Some(UnionDispatch {
                variants: variant_dispatch,
                ..
            }) = &discriminated
            {
                if default_variant.is_none()
                    && self.reject_optional_tag_property(discriminator, lowered, variant_dispatch)
                {
                    return None;
                }
            }
            // `None` only where no variant carries a tag, which the `W011` above has said.
            discriminated
                .or_else(|| self.disjoint_strategy(types))
                .unwrap_or_else(|| self.trial_strategy(types, mode))
        } else {
            self.disjoint_strategy(types)
                .unwrap_or_else(|| self.trial_strategy(types, mode))
        };
        Some((dispatch, None))
    }

    /// Report, as `W001` at `provenance`, the variants of a trial-matched `oneOf` (one whose decode
    /// requires exactly one variant to match) that lower to `serde_json::Value` beside another
    /// variant (#535). Such a variant accepts every value, so every value another variant accepts
    /// matches two of them and fails the exactly-one rule. The union is faithful to the document,
    /// which admits only the values no other branch accepts, but the generated enum does not show
    /// that its other variants never decode a value and fail to serialize one, so it is said.
    /// `labels` gives each variant's position for the message, named `noun` after `prefix`.
    ///
    /// An `anyOf` is not reported: its most specific match picks a typed variant for every value
    /// one accepts, so a `serde_json::Value` variant, the faithful lowering of an untyped member,
    /// takes only the rest. Neither is a discriminated or disjoint union, which tells the variant
    /// apart by its tag or its JSON category.
    pub(super) fn warn_untyped_one_of_variants(
        &mut self,
        provenance: &Provenance,
        union: &Union,
        labels: &[usize],
        prefix: &str,
        noun: &str,
    ) {
        let untyped: Vec<usize> = self
            .untyped_one_of_variants(union, labels)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        self.emit_untyped_one_of_warning(provenance, &untyped, prefix, noun);
    }

    /// The variants [`Self::warn_untyped_one_of_variants`] reports of `union`, each variant's hint
    /// beside its label from `labels`: those that lower to `serde_json::Value`, in a trial-matched
    /// `oneOf` of two or more variants. Empty for any other union.
    pub(super) fn untyped_one_of_variants(
        &self,
        union: &Union,
        labels: &[usize],
    ) -> Vec<(String, usize)> {
        if union.variants.len() < 2
            || !matches!(
                union.strategy,
                UnionStrategy::Trial {
                    mode: UnionMode::OneOf,
                    ..
                }
            )
        {
            return Vec::new();
        }
        union
            .variants
            .iter()
            .zip(labels)
            .filter(|(variant, _)| matches!(self.kind_of(variant.ty.id), Some(TypeKind::Any)))
            .map(|(variant, label)| (variant.name_hint.clone(), *label))
            .collect()
    }

    /// Report, as `W011` at `at`, that the union's real member `member` cannot satisfy the
    /// enclosing schema's own constraints and so is no variant of the generated enum.
    pub(super) fn warn_excluded_union_branch(&mut self, at: &Provenance, member: usize) {
        // W011 case: excluded-union-branch
        Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
            .message(format!(
                "union member {member} cannot satisfy the enclosing schema's own constraints, so \
                 it is not a variant of the generated enum"
            ))
            .emit(self.diags);
    }

    /// Report `warning`, an inline `oneOf`'s `W001` for its `serde_json::Value` variants, or hold
    /// it back while an `allOf` gathers its members ([`Self::held_untyped_warnings`]). Nothing is
    /// reported for a warning that names no variant.
    pub(super) fn report_untyped_one_of(&mut self, warning: HeldUntypedWarning) {
        if warning.untyped.is_empty() {
            return;
        }
        if let Some(held) = &mut self.held_untyped_warnings {
            held.push(warning);
            return;
        }
        let labels: Vec<usize> = warning.untyped.iter().map(|(_, label)| *label).collect();
        self.emit_untyped_one_of_warning(&warning.provenance, &labels, "this `oneOf`'s", "member");
    }

    /// Emit the `W001` [`Self::warn_untyped_one_of_variants`] describes for the variants labelled
    /// `untyped`, if there are any.
    fn emit_untyped_one_of_warning(
        &mut self,
        provenance: &Provenance,
        untyped: &[usize],
        prefix: &str,
        noun: &str,
    ) {
        if untyped.is_empty() {
            return;
        }
        let untyped: Vec<String> = untyped.iter().map(ToString::to_string).collect();
        let (noun, verb) = if untyped.len() == 1 {
            (noun.to_owned(), "lowers")
        } else {
            (format!("{noun}s"), "lower")
        };
        // W001 case: untyped-one-of-variant
        Diagnostic::warning(Code::ValidationKeywordIgnored, provenance.clone())
            .message(format!(
                "{prefix} {noun} {} {verb} to `serde_json::Value`, which accepts every value, so \
                 a value any other variant accepts matches two variants and fails the exactly-one \
                 rule: the other variants never decode a value and fail to serialize one, and \
                 only a value no other variant accepts decodes, as `serde_json::Value`",
                untyped.join(", ")
            ))
            .remedy(
                "give the untyped branch the type its values have, or use `anyOf` where a value \
                 may match more than one branch",
            )
            .emit(self.diags);
    }

    /// Merge the `oneOf` variants that decode the same values — that lower to the same generated
    /// type, or to distinct structs or string enums of one structure (#492,
    /// [`TypeGraph::same_decoded_values`]) — into the first of them, which then stands for the
    /// members of every variant folded into it ([`LoweredVariant::members`]), and report the merge
    /// as `W001` at the union. Each variant comes in standing for its own member alone. The
    /// union's `null` is not decided here: [`Self::lower_union_variants`] counts the branches
    /// `null` matches before any merge, across the merged sets and the rest.
    ///
    /// [`TypeGraph::same_decoded_values`]: crate::ir::TypeGraph::same_decoded_values
    fn merge_indistinguishable_variants(
        &mut self,
        schema: &Schema,
        variants: &mut Vec<LoweredVariant>,
    ) {
        let groups = group_alike(variants, |kept, variant| {
            self.graph.same_decoded_values(kept.ty, variant.ty)
        });
        if groups.len() == variants.len() {
            return;
        }
        let merged: Vec<Vec<usize>> = groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|&index| variants[index].member())
                    .collect()
            })
            .collect();
        let slices: Vec<&[usize]> = merged.iter().map(Vec::as_slice).collect();
        self.warn_indistinguishable_branches(&schema.provenance, &slices);
        let mut kept = groups.iter().map(|group| group[0]).peekable();
        let mut merged = merged.into_iter();
        let mut index = 0;
        variants.retain_mut(|variant| {
            let keep = kept.next_if_eq(&index).is_some();
            index += 1;
            if keep {
                variant.members = merged.next().unwrap_or_default();
            }
            keep
        });
    }

    /// Report, as `W001` at the `oneOf` written at `at`, that its real members merged into the
    /// `groups` given: each group the real-member positions one kept variant stands for, in
    /// variant order, with at least one group of more than one. Shared by the merge in
    /// [`Self::merge_indistinguishable_variants`] and the nullable alias that repeats its one
    /// `$ref` ([`Self::nullable_alias_back_edge`]), which answers before any merge is made, so
    /// the two spellings of one merge report it in one wording.
    pub(super) fn warn_indistinguishable_branches(&mut self, at: &Provenance, groups: &[&[usize]]) {
        let merged: Vec<String> = groups
            .iter()
            .filter(|members| members.len() > 1)
            .map(|members| {
                let members: Vec<String> = members.iter().map(usize::to_string).collect();
                format!("members {}", members.join(", "))
            })
            .collect();
        let consequence = if groups.len() == 1 {
            "the union is that one type"
        } else {
            "each such set is one variant of the generated enum"
        };
        // W001 case: indistinguishable-branches
        Diagnostic::warning(Code::ValidationKeywordIgnored, at.clone())
            .message(format!(
                "this `oneOf`'s {} lower to the same generated type or to identically structured \
                 ones, differing at most in keywords it does not carry, so a value matching one \
                 matches all of them and would fail the exactly-one rule: {consequence}, and which \
                 of them a value matches is not enforced",
                merged.join(" and ")
            ))
            .remedy("keep producer-side validation for the union's branch constraints")
            .emit(self.diags);
    }

    /// Lower one union member, returning its type and — when the member is a `$ref` to a component —
    /// that component's name (used to derive the variant name and implicit discriminator tag).
    ///
    /// The name is a fact about how the member is *written*; the type is what it *means*. A bare
    /// component `$ref` means its target, so it is that component's shared type. A `$ref` beside
    /// shape-bearing siblings means the intersection of the two — `$ref` is an applicator in
    /// 2020-12 — so it lowers through `lower_schema_or`, the same `$ref`-sibling intersection every
    /// other position takes (an empty or unrepresentable one is `E013` at the member), and keeps
    /// the component name only for naming. Returning the target here instead discarded the
    /// siblings in silence (#279).
    fn lower_union_variant(
        &mut self,
        member: &SchemaOr,
        hint: &str,
    ) -> Option<(Ty, Option<String>)> {
        if let (Some(name), SchemaOr::Schema(schema)) =
            (member_component_name(member, self.resolver), member)
        {
            let ty = if ref_siblings_bear_shape(schema) {
                self.lower_schema_or(member, hint)?
            } else {
                // The member itself never reaches `lower_schema_inner`, which reports this.
                self.diagnose_standalone_discriminator(schema);
                self.ensure_component(name, schema.reference.as_deref(), &schema.provenance)?
            };
            return Some((ty, Some(name.to_owned())));
        }
        let ty = self.lower_schema_or(member, hint)?;
        Some((ty, None))
    }
}

/// One variant of a union [`LowerCtx::lower_union_variants`] is lowering, beside what its
/// dispatch and its reports read of the members it came from. Sibling keywords can exclude a
/// member, and a merge folds several into one variant, so a variant's position is not its
/// member's.
#[derive(Clone)]
pub(super) struct LoweredVariant {
    /// The variant's name hint, as [`UnionVariant::name_hint`] carries it.
    pub(super) name_hint: String,
    /// The variant's payload type, as [`UnionVariant::ty`] carries it.
    pub(super) ty: Ty,
    /// The `$ref` component name the member was written as, which names the variant and is its
    /// implicit discriminator tag ([`LowerCtx::discriminated_strategy`]).
    pub(super) ref_name: Option<String>,
    /// The real members this variant stands for, its own first: a merge folds several into one
    /// ([`LowerCtx::merge_indistinguishable_variants`]).
    pub(super) members: Vec<usize>,
}

impl LoweredVariant {
    /// The real member this variant came from.
    pub(super) fn member(&self) -> usize {
        self.members[0]
    }

    /// The variant as the generated union carries it, selected by `dispatch`.
    fn into_variant(self, dispatch: VariantDispatch) -> UnionVariant {
        UnionVariant {
            name_hint: self.name_hint,
            ty: self.ty,
            dispatch,
        }
    }
}

/// What [`LowerCtx::lower_variant_members`] made of a union's real members.
#[derive(Default)]
struct MemberLowering {
    /// The variants, in member order: a member the sibling meet excludes is none.
    variants: Vec<LoweredVariant>,
    /// Whether any variant accepted `null` itself, before it was hoisted off the variant: a
    /// `null` payload then resolves at the outer `Option<Union>` (→ `None`), and the
    /// discriminated or disjoint dispatch only ever inspects non-null content.
    hoisted_null: bool,
    /// The ids each member's sibling meet inserted, one range per real member and indexed by its
    /// position, empty for a member met with nothing. They interleave with the members' own
    /// lowered types, which stay, so a re-emit elides the unused ones rather than popping.
    meet_inserts: Vec<std::ops::Range<u32>>,
    /// The members the sibling meet narrowed to the exact null type, each typed before it: an
    /// `anyOf` hoists them to its `Option` beside typed variants (#633). A branch that is the
    /// null type on its own (`const: null`) is not one of them.
    met_into_null: HashSet<usize>,
    /// The members that are the exact null type on their own (`const: null`, `enum: [null]`),
    /// and that no sibling meet narrowed to it, with the ids each one's own lowering inserted: a
    /// `oneOf` drops them where `null` matches another branch too (#657).
    own_nulls: Vec<(usize, std::ops::Range<u32>)>,
    /// The ids each member's own lowering inserted, whether or not it stays a variant: a union
    /// left one variant re-emits that variant's kind, and leaves them unused unless it reaches
    /// them (#987).
    member_lowerings: Vec<std::ops::Range<u32>>,
}

/// `items` grouped first fit by `alike`: each group the positions of the items alike to its
/// first item, which leads it, and the groups in the order of their first items. `alike` is asked
/// of a group's first item and a later item, in that order.
pub(super) fn group_alike<T>(
    items: &[T],
    mut alike: impl FnMut(&T, &T) -> bool,
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|group| alike(&items[group[0]], item))
        {
            Some(group) => group.push(index),
            None => groups.push(vec![index]),
        }
    }
    groups
}

/// A `W001` an inline `oneOf` raised for its variants that lower to `serde_json::Value`
/// ([`LowerCtx::warn_untyped_one_of_variants`]), held back while an `allOf` gathers its members
/// ([`LowerCtx::held_untyped_warnings`]): where the union is a member the `allOf`'s scalar meet
/// replaces, the warning names only the variants the meet carries as `Value` (#660, #675). It is
/// reported as raised where the gathering or the meet fails, where the meet does not replace the
/// union (its lowering is still named, or the meet is a reservation), and for every warning a
/// union other than such a member raised.
#[derive(Clone, Debug)]
pub(super) struct HeldUntypedWarning {
    /// The union the warning is raised at.
    pub(super) provenance: Provenance,
    /// Each `Value` variant's hint, beside its member position for the message.
    pub(super) untyped: Vec<(String, usize)>,
}

#[cfg(test)]
mod tests {
    use super::group_alike;

    /// Each item joins the first group whose first item it is alike to, asked in that order, and
    /// otherwise opens a group of its own; nothing alike leaves every item alone, and every item
    /// alike to the first leaves one group.
    #[test]
    fn group_alike_groups_first_fit_by_each_groups_first_item() {
        let same_parity = |kept: &u32, item: &u32| kept % 2 == item % 2;
        assert_eq!(
            group_alike(&[1, 2, 3, 4, 6, 5], same_parity),
            vec![vec![0, 2, 5], vec![1, 3, 4]]
        );
        assert_eq!(
            group_alike(&[1, 2, 3], |_: &u32, _: &u32| false),
            vec![vec![0], vec![1], vec![2]]
        );
        assert_eq!(
            group_alike(&[1, 2, 3], |_: &u32, _: &u32| true),
            vec![vec![0, 1, 2]]
        );
        assert!(group_alike(&[] as &[u32], |_: &u32, _: &u32| true).is_empty());
        // Asked of a group's first item, then the later one: `kept < item` groups 3 with 1 and
        // leaves 0 alone, where the reverse order would have grouped 0 with 1 instead.
        let mut asked = Vec::new();
        let groups = group_alike(&[1, 3, 0], |kept: &u32, item: &u32| {
            asked.push((*kept, *item));
            kept < item
        });
        assert_eq!(groups, vec![vec![0, 1], vec![2]]);
        assert_eq!(asked, vec![(1, 3), (1, 0)]);
    }
}
