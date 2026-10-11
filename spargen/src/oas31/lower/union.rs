//! `oneOf` / `anyOf` lowering: a union's variants, their null handling, and the merge of
//! variants no strategy could tell apart.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{Ty, TypeId, TypeKind, Union, UnionMode, UnionStrategy, UnionVariant};
use crate::oas31::discriminator::is_schema_component_name;
use crate::oas31::{Schema, SchemaOr};

use super::discriminator::member_component_name;
use super::meet::NoMeet;
use super::nullability::{
    member_is_null_only, own_keywords_admit_null, schema_is_nullable, union_branch_admits_null,
};
use super::prune::{kind_edges, reachable_types};
use super::refiner::{unreached_halves, unreached_message};
use super::shape::schema_has_shape_constraint;
use super::{LowerCtx, Refiner, ScopeReach, StatedNothingNull};

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
    ///   serialization revalidation. A `oneOf`'s variants that lower to one generated type merge
    ///   into one variant, with `W001`.
    ///
    /// A member the sibling meet excludes drops out with `W011`; where that leaves no branch, the
    /// union is the exact null type if a null-only member remains and the sibling admits `null`,
    /// and `E007` otherwise. A sole member that is uninhabited on its own (`false`) is no branch the
    /// sibling excludes: the union is that uninhabited type whatever the sibling is
    /// ([`Self::uninhabited_union_meet`]). An `anyOf` member the sibling meet narrows to the exact
    /// null type is a branch `null` matches: beside typed members it makes the union `nullable`
    /// and is no variant, and with every member so narrowed the union is the null type. A `oneOf`
    /// in which `null` matches more than one branch admits no `null`. The union is rejected
    /// (`None`) with `E007` where one node declares both `oneOf` and `anyOf` or a member is this union itself, and with `E013` where the sibling would have to
    /// be met with a member that closes a reference cycle, with a member that states no category
    /// for untyped sibling keywords to establish, or with a member it shares values with that no
    /// single Rust type represents, or where it meets every one of several `oneOf` members, or one
    /// beside a `null` member, in `null` alone, which then matches them all.
    ///
    /// Every variant type inserts before the union def, so the [`TypeKind::Union`] is the final
    /// graph insert — preserving the [`Self::ensure_component`] last-insert invariant when the union
    /// is a component body.
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
        let sibling = self.lower_union_sibling(schema, hint)?;

        // Two DIFFERENT facts, tracked apart because only one of them can rescue an empty
        // intersection. A null-only MEMBER supplies a branch that `null` validates against. A
        // `"null"` in the enclosing `type` array only *permits* null: `oneOf` still demands exactly
        // one matching member and `anyOf` at least one, so with no null member there is nothing for
        // `null` to match and the schema admits nothing at all. Merging them made
        // `{type: [integer,'null'], oneOf: [{type: string}]}` — which nothing satisfies —
        // indistinguishable from the same document with a `{type: 'null'}` member, which only
        // `null` satisfies. A `const: null` or an `enum` listing `null` permits it as the `type`
        // array does, where nothing else the schema states refuses it: an untyped branch, which
        // decides nothing about `null`, takes it from either, as it does in the `allOf` spelling
        // (#632).
        let null_from_type_array = schema_is_nullable(schema) && own_keywords_admit_null(schema);
        let mut null_from_member = false;
        // How many null-only members there are: each is a branch `null` matches, which a `oneOf`
        // counts against its exactly-one rule beside the real branches that accept `null` too.
        let mut null_members = 0usize;
        let mut real_members: Vec<&SchemaOr> = Vec::new();
        for member in members {
            if member_is_null_only(member) {
                null_from_member = true;
                null_members += 1;
            } else {
                real_members.push(member);
            }
        }
        // The union accepts `null` only through a branch `null` matches (#574): a `null` member,
        // or a real branch that accepts it after its sibling meet. The `type` array alone adds
        // nothing, so it does not start this; an untyped branch, which decides nothing about
        // `null`, takes the array's `null` below, and the meet keeps it where the other sibling
        // keywords admit it.
        let mut nullable = null_from_member;

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
        // spellings of one conjunction give one verdict, which they did not before.
        if sibling.is_some() {
            for member in &real_members {
                // A member that is this union's OWN reservation is not an intersection problem:
                // the union is itself, which the collapse and variant paths below reject as `E007`
                // with or without siblings. Left to this guard it would draw `E013` only when
                // siblings are present, so one shape would get two codes.
                if self.member_closes_a_cycle(member, &schema.provenance)
                    && !self.member_is_this_union(member, &schema.provenance)
                {
                    return self.reject_union_member_cycle(schema);
                }
            }
        }

        // A single real member (the rest were null): `Option<ThatType>`, no enum needed. Re-emit the
        // member's kind as this position's own def so it is the final graph insert — mirroring the
        // allOf single-member collapse — which keeps the `ensure_component` last-insert invariant
        // when the union is a component body (a bare `$ref` member would otherwise return an existing
        // id and leave the popped root mismatched).
        if real_members.len() == 1 {
            let mut inner = self.lower_schema_or(real_members[0], hint)?;
            // Everything the sibling meet below inserts has an id at or above this mark; the
            // member and the sibling were lowered before it.
            let mark = self.graph_mark();
            // The member's OWN nullability, before the intersection overwrites `inner`. Needed
            // below when the sibling is not entitled to decide.
            let member_nullable = inner.nullable;
            // Whether `null` is in the member's branch, for the `oneOf` count below: its own
            // nullability, which a sibling meet can only take away, or — once a sibling meets it —
            // the meet's answer for an untyped member, which takes `null` from a sibling that
            // accepts it (`Value` met with `type: [object, 'null']` is a nullable object), as each
            // variant of the multi-member path below is counted after its own meet. Without a
            // meet a member lowering to `Value` is not counted, as that path does not count one
            // either.
            let mut member_takes_null = member_nullable;
            // An untyped object member's non-null struct decides nothing about `null` either
            // (#567), so it takes `null` from a sibling or a meet as `Value` does; so does any
            // other member whose own keywords leave `null` undecided (#574).
            let member_untyped = self.branch_takes_permitted_null(real_members[0], inner);
            // An untyped object or array member constrains objects or arrays alone, so `null`
            // matches it beside a `null` member, as the multi-member path counts it (#622), and so
            // does a nested union `null` matches through its own branches (#628).
            let member_leaves_null_undecided =
                self.branch_matches_null_undecided(real_members[0], inner);
            // The sole member is the reservation *this* schema will occupy, so the union is the
            // whole of itself: `Selfy = Selfy | null` describes nothing a decoder can terminate on,
            // exactly as a direct recursive member does in a multi-member union. That path already
            // refuses it, and this is the same shape written with fewer members beside it, so it
            // gets the same code and the same wording rather than a second code chosen by member
            // count. Asked before the reservation guards below, which would otherwise answer the
            // narrower question first and hand one shape two codes again.
            //
            // The document-half guard above (`member_closes_a_cycle`) runs before this collapse
            // and would otherwise answer `E013` for this shape whenever the union carries sibling
            // keywords. It excludes a member that is this union's own reservation
            // (`member_is_this_union`), so the self-union reaches here and draws `E007` with or
            // without siblings, and with one member or several.
            // `a_union_whose_sole_member_is_its_own_reservation_is_rejected` in
            // `tests/frontend/recursion.rs` asserts the reported error codes are **exactly** `[E007]` on both its spellings.
            if self.reservation_at(&schema.provenance) == Some(inner.id) {
                return self.reject_union_member_is_the_union(schema);
            }
            // The reservation half of the cycle test, on the sole real member. The document half
            // above answers only the `#/components/schemas/…` spelling; a sub-file or remote member
            // reference reaches here still pointing at a placeholder, and the intersection below
            // cannot compose with one. Reported with the same wording the other two spellings use,
            // because it is the same fact about the same document.
            if sibling.is_some() && self.is_in_progress_root(inner.id) {
                return self.reject_union_member_cycle(schema);
            }
            // The member is some *other* type's still-open reservation — the cycle-closing
            // `$ref` of an ordinary recursive schema. Its kind may not be read: cloning a
            // `TypeKind::Reserved` inserts a second reservation nothing will ever `fill`, which
            // `check_invariants` reports as `E011` against a document that is not malformed, and
            // which at `2aa5ada` (where the placeholder was `TypeKind::Any`) cloned as
            // `serde_json::Value` instead — a typed schema silently degraded.
            //
            // A truthful answer exists and needs no def of its own: the member's own `Ty`, boxed so
            // the cycle has a finite size and optional because the `null` member is what collapsed
            // away. That is `Option<Box<T>>` — what `docs/support-matrix.md` promises for this
            // construct, and what the direct `{$ref: T}` spelling already produces.
            //
            // Inserting no def is safe exactly where this union is not itself the body of a type
            // whose root id was reserved before lowering began. Where it is, the caller pops the
            // last insert and lifts it into that reserved root, so returning a foreign id would
            // relocate the wrong def and dangle the component. `ensure_component` recognises that
            // shape as a nullable alias before it reserves anything, so the root-component spelling
            // never arrives here; a sub-file or remote body reaching it is refused rather than
            // mis-assembled.
            if self.is_reservation(inner.id) {
                if self.reservation_at(&schema.provenance).is_some() {
                    return self.reject_self_referential_union(
                        schema,
                        "this schema's whole body is a union whose only non-null member is a \
                         `$ref` that closes a reference cycle, so the schema names no shape of its \
                         own and cannot be given a generated type",
                    );
                }
                // A sibling meet is refused above, so here the `type` array was dropped for
                // lowering, and a target whose keywords decide nothing takes its `null` (#574).
                let takes_null = inner.nullable || (null_from_type_array && member_untyped);
                // A `null` member beside a member that accepts `null`, or whose untyped target
                // leaves it undecided, puts `null` in two branches, as the collapse below counts
                // it for a member that is no reservation (#627).
                let null_twice = mode == UnionMode::OneOf
                    && null_members + usize::from(takes_null || member_leaves_null_undecided) > 1;
                inner.nullable = (takes_null || nullable) && !null_twice;
                inner.boxed = true;
                return Some(inner);
            }
            // An untyped member accepts the `null` the `type` array permits (#574), as the
            // multi-member path's untyped variants do, before the sibling meet so the other
            // sibling keywords can still take it away. Without a sibling to meet (a multi-type
            // array dropped for lowering) it is then a branch `null` matches; with one, the meet
            // below recounts it.
            if null_from_type_array && member_untyped {
                inner.nullable = true;
                member_takes_null = true;
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
                        let met =
                            ctx.meet_refiner(inner, sibling.refiner, reach, &constrained_hint);
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
                // The `null` member was folded into `inner` before the meet, but an untyped member
                // accepts `null` already, so folding it changed nothing the meet saw: the meet's
                // nullability is the member's own after the meet.
                member_takes_null = member_nullable || (member_untyped && constrained.nullable);
                inner = constrained;
            }
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            // The meet's result is re-emitted under this schema's name, so the meet's own inserts
            // (`…Constrained`, and whatever it built on the way) are unused unless `kind` reaches
            // them (#462). Without a sibling nothing was inserted since `mark`.
            let mut ty = self.reemit_meet(schema, hint, mark, kind);
            // A `null` member beside a member that accepts `null` itself puts `null` in two
            // branches, which fails a `oneOf`'s exactly-one rule (#563), counted after the meet
            // like the multi-member path's variants. An untyped object or array member is such a
            // branch whether or not it took `null` from anything (#622).
            let null_twice = mode == UnionMode::OneOf
                && null_members + usize::from(member_takes_null || member_leaves_null_undecided)
                    > 1;
            // The sibling meet narrowed the member to the exact null type beside a `null` member:
            // `null` matches both branches and fails exactly-one, and the member accepts nothing
            // else, so nothing satisfies the schema, as the multi-member path below rejects for
            // the same branches (#632).
            if null_twice && sibling.is_some() && self.is_exact_null(inner) {
                return self.reject_one_of_null_in_every_branch(schema);
            }
            // Held back for a meet with no sibling here, an untyped member's `null` is not settled
            // yet: the caller counts it after the meet.
            if mode == UnionMode::OneOf
                && null_members > 0
                && member_untyped
                && sibling.is_none()
                && self.unmerged_union.as_ref() == Some(&schema.provenance)
            {
                self.untyped_beside_null_member = Some(schema.provenance.clone());
            }
            ty.nullable = (inner.nullable || nullable) && !null_twice;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        // Lower every real variant first (their defs — especially `$ref` components — insert before
        // the union def below), recording the `$ref` component name for tag/variant naming.
        let mut variants: Vec<UnionVariant> = Vec::new();
        let mut ref_names: Vec<Option<String>> = Vec::new();
        // The real member each variant came from: sibling keywords can exclude a member, so a
        // variant's position is not its member's.
        let mut variant_members: Vec<usize> = Vec::new();
        // How many variants accepted `null` before their nullability was hoisted to the union.
        let mut nullable_variants = 0usize;
        // How many variants are the exact JSON `null` (`const: null`, `enum: [null]`), and whether
        // an untyped branch took `null` from what the union is met with (#567, #586): each such
        // variant is a branch `null` matches beside it, so a `oneOf` counts them together (#563).
        let mut null_variants = 0usize;
        let mut null_from_conjunct = false;
        // The variant hints of the branches that state nothing and took the conjunct's `null`, and
        // how many of them a `oneOf` counts without hoisting (#592).
        let mut stated_nothing_hints: Vec<String> = Vec::new();
        let mut stated_nothing_nulls = 0usize;
        // How many variants are an untyped object or array branch that still leaves `null`
        // undecided after its meet ([`Self::branch_leaves_null_undecided`]): its keywords constrain
        // objects or arrays alone, so `null` matches it, and a `oneOf` counts it beside a branch
        // that states `null` (#622). A nested union `null` matches through such branches of its
        // own is counted alike ([`Self::branch_matches_null_undecided`], #628). Counted for
        // exactly-one alone: such a branch on its own is the non-null struct, array or enum every
        // other spelling gives it, so it hoists nothing.
        let mut undecided_nulls = 0usize;
        let mut used_hints: HashSet<String> = HashSet::new();
        let mut reach = ScopeReach::default();
        // The ids each member's sibling meet inserted. They interleave with the members' own
        // lowered types, which stay, so a re-emit below elides the unused ones rather than popping.
        let mut meet_inserts: Vec<std::ops::Range<u32>> = Vec::new();
        // The members the sibling meet narrowed to the exact null type, each typed before it: an
        // `anyOf` hoists them to its `Option` beside typed variants (#633). A branch that is the
        // null type on its own (`const: null`) is not one of them.
        let mut met_into_null: HashSet<usize> = HashSet::new();
        // How many variants are such a branch: each is one branch `null` matches (#645).
        let mut met_nulls = 0usize;
        for (index, member) in real_members.iter().enumerate() {
            let (mut ty, ref_name) =
                self.lower_union_variant(member, &format!("{hint}Variant{index}"))?;
            let mut took_conjunct_null = false;
            // A variant that is a back-edge to *this* union — the member's type is the very
            // reservation this schema will occupy — is the whole union, so it constrains nothing and
            // cannot be decoded: the emitted `Deserialize` opens by re-entering itself on the same
            // value, with no base case, and recurses until the stack is exhausted. It compiles, so
            // nothing downstream can catch it; refusing here is the only place it can be caught.
            //
            // The test is against *this* schema's own reservation, not against any open one. Asking
            // `is_in_progress_root(ty.id)` answers the strictly weaker "is the member any open
            // reservation", which is true of every ordinary recursive schema whose union sits in a
            // property: `Node.child: {oneOf: [{$ref: Node}, …]}` has a member pointing at `Node`'s
            // reservation while the union being built is `Nodechild`. That decodes perfectly well —
            // the member is a *different* type — and rejecting it refuses the most common recursive
            // construct there is, which `docs/support-matrix.md` lists as supported.
            if self.reservation_at(&schema.provenance) == Some(ty.id) {
                return self.reject_union_member_is_the_union(schema);
            }
            // Read before anything below gives the branch a conjunct's `null`, which would hide
            // that its own keywords decide nothing. A nested union `null` matches through its own
            // branches is such a branch too (#628).
            let leaves_null_undecided = self.branch_matches_null_undecided(member, ty);
            // The reservation half of the cycle test again, on a multi-variant union. Same fact,
            // same wording, same place in the order: before anything tries to intersect against the
            // placeholder. Guarded on there being a sibling at all, so an ordinary recursive
            // `oneOf` still boxes its back-edge and generates.
            if sibling.is_some() && self.is_in_progress_root(ty.id) {
                return self.reject_union_member_cycle(schema);
            }
            // Held back for a meet with nothing of its own to meet first, an untyped object branch
            // accepts `null` wherever what it is met with does (#567), as an untyped `allOf` member
            // does (#541): its non-null struct decides nothing. Counted as accepting it here, so
            // the meet keeps `null` in that branch exactly where the other conjuncts admit it, and
            // a `oneOf` two such branches share rejects it below. Only where a conjunct it is met
            // with admits `null` ([`Self::unmerged_union_meets_null`]): met with untyped objects
            // alone, nothing does, and the branch stays the non-null struct every other spelling
            // of that composition gives it.
            //
            // A branch that states nothing (`true`, `{}`) accepts `null` there too, as it does
            // beside untyped sibling keywords (#592), but `Value` is the identity of the meet, so
            // the meet hands it the conjunct's `null` again. An `anyOf`, which needs one match,
            // hoists it here, and the caller clears the meet's copy
            // ([`Self::stated_nothing_took_null`]). A `oneOf` counts it here without hoisting it:
            // where that puts `null` in two branches the caller clears the meet's copy too, and
            // where it is the one branch `null` matches the meet's copy is its answer, as the
            // `oneOf` counts after the meet read it ([`Self::collapse_met_union`]), and as the
            // caller keeps on the position where the meet narrows the union to it (#597).
            let mut stated_nothing_took_null = false;
            if sibling.is_none()
                && self.unmerged_union.as_ref() == Some(&schema.provenance)
                && self.unmerged_union_meets_null
                && self.branch_takes_conjunct_null(member, ty)
            {
                stated_nothing_took_null = !self.branch_leaves_null_undecided(member, ty);
                if stated_nothing_took_null && mode == UnionMode::OneOf {
                    stated_nothing_nulls += 1;
                    null_from_conjunct = true;
                } else {
                    ty.nullable = true;
                    took_conjunct_null = true;
                }
            }
            // An untyped branch accepts the `null` the `type` array permits (#574), before the
            // sibling meet so the other sibling keywords can still take it away, and is then
            // counted as a branch `null` matches. Without a sibling to meet (a multi-type array
            // dropped for lowering) nothing else would give it the array's `null`.
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
                took_conjunct_null = true;
            }
            if let Some(sibling) = sibling {
                let mark = self.graph_mark();
                let null_before = self.is_exact_null(ty);
                let met = self.meet_refiner(
                    ty,
                    sibling.refiner,
                    &mut reach,
                    &format!("{hint}Variant{index}Constrained"),
                );
                meet_inserts.push(mark..self.graph_mark());
                ty = match met {
                    Ok(intersection) => {
                        if !null_before && self.is_exact_null(intersection) {
                            met_into_null.insert(index);
                        }
                        intersection
                    }
                    // The sibling constraints make this branch impossible; JSON Schema simply
                    // removes it from the union's accepted set. Acknowledge it, because a variant
                    // vanishing from the generated enum is otherwise invisible.
                    Err(NoMeet::Empty) => {
                        // W011 case: excluded-union-branch
                        Diagnostic::warning(
                            Code::DeclarationHasNoEffect,
                            schema.provenance.clone(),
                        )
                        .message(format!(
                            "union member {index} cannot satisfy the enclosing schema's own \
                             constraints, so it is not a variant of the generated enum"
                        ))
                        .emit(self.diags);
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
            nullable = nullable || ty.nullable;
            // A branch the meet narrowed to the exact null type is counted once, as itself, rather
            // than again as an undecided or nullable branch (#645).
            if met_into_null.contains(&index) {
                met_nulls += 1;
            } else {
                nullable_variants += usize::from(ty.nullable);
                // A branch already counted as stating nothing (a nested union lowering to `Value`
                // that took the conjunct's `null`) is one branch `null` matches, not two.
                undecided_nulls +=
                    usize::from(leaves_null_undecided && !ty.nullable && !stated_nothing_took_null);
                null_variants += usize::from(self.is_exact_null(ty));
            }
            null_from_conjunct = null_from_conjunct || (took_conjunct_null && ty.nullable);
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
            if stated_nothing_took_null {
                stated_nothing_hints.push(name_hint.clone());
            }
            variants.push(UnionVariant { name_hint, ty });
            ref_names.push(ref_name);
            variant_members.push(index);
        }

        if let Some(sibling) = sibling {
            for keywords in unreached_halves(sibling.refiner, &reach) {
                self.warn_unreached_union_sibling(schema, unreached_message(keywords));
            }
        }
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
        // A `oneOf` admits `null` only where exactly one branch does: the `null` members and the
        // variants that accepted it before it was hoisted are counted together, whether or not any
        // of the variants merge below, since two put `null` in two branches, which fails
        // exactly-one (#563). Decided here, before a meet the union is held back for
        // ([`Self::unmerged_union`]), because the hoist leaves nothing downstream able to count
        // them. That meet removes `null` from a typed branch only; an untyped branch it gives
        // `null` is counted after it ([`Self::drop_null_matching_two_branches`]).
        // An exact-`null` variant is counted only beside a branch that took `null` from a conjunct
        // (#586): elsewhere it stays the variant `null` decodes to, as it always has. A branch that
        // states nothing is counted where it took `null` from the conjunct (#592), and the meet's
        // copy of that `null` is cleared where the count puts it in two branches, or where an
        // `anyOf` hoisted it. An untyped object or array branch that hoisted nothing is counted
        // too, since `null` matches it all the same (#622), and so is a nested union that hoisted
        // nothing and that `null` matches through its own branches (#628). A branch the sibling
        // meet narrowed to the exact null type is one branch `null` matches wherever it is, as a
        // `null` member is (#645).
        let null_variants = if null_from_conjunct { null_variants } else { 0 };
        let null_twice = mode == UnionMode::OneOf
            && null_members
                + nullable_variants
                + null_variants
                + stated_nothing_nulls
                + undecided_nulls
                + met_nulls
                > 1;
        if null_twice {
            nullable = false;
        }
        // Otherwise a `oneOf` that counted such a branch counted it as the only branch `null`
        // matches, and the meet's copy of that `null` is its answer.
        if !stated_nothing_hints.is_empty() && (mode == UnionMode::AnyOf || null_twice) {
            self.stated_nothing_took_null = Some((
                schema.provenance.clone(),
                StatedNothingNull::Counted(stated_nothing_hints),
            ));
        } else if stated_nothing_nulls > 0 {
            self.stated_nothing_took_null =
                Some((schema.provenance.clone(), StatedNothingNull::Sole));
        }
        // The sibling meet left every one of several `oneOf` branches the exact null type:
        // `type: [string, 'null']` beside untyped `items` branches, which constrain arrays alone,
        // meets each of them in `null` only. `null` then matches every branch, which fails the
        // exactly-one rule, so nothing satisfies the schema, and its `allOf` spelling (the `type`
        // as a member beside the union) is rejected for it (#632). Merged below, the branches
        // were one `()` variant with `W001`, a type for a schema no value satisfies. The `anyOf`
        // counterpart, which needs one match, is the null type below (#625). A `null` member is
        // one more branch `null` matches, so a single variant met in `null` beside one is the
        // same empty intersection.
        if mode == UnionMode::OneOf
            && sibling.is_some()
            && variants.len() + null_members > 1
            && variants
                .iter()
                .all(|variant| self.is_exact_null(variant.ty))
        {
            return self.reject_one_of_null_in_every_branch(schema);
        }
        // The meet left only some branches the exact null type: `type: [string, 'null']` beside
        // an untyped `items` branch and a `{type: string}` one. Each such branch is a branch `null`
        // matches, as a `null` member is, so the typed branches are the variants, as the `allOf`
        // spelling lowers them. An `anyOf` hoists `null` to the union's `Option` (#633); a `oneOf`
        // does too where that branch is the only one `null` matches, and otherwise `null` fails
        // its exactly-one rule and the union admits no `null` (#645). Kept, each was a `()`
        // variant beside the typed ones — merged with `W001` where there were several — so one
        // instance set had two public types by spelling. Dropped here, before the merge below, so
        // the branches are not reported as indistinguishable variants of an enum that no longer
        // has them. The dropped branches' meet inserts (their `…Constrained` aliases) are elided
        // unless a surviving variant reaches them. A branch that is the null type on its own
        // (`const: null`) stays the `()` variant it is with no sibling and in the `allOf` spelling.
        if variant_members
            .iter()
            .any(|member| met_into_null.contains(member))
            && !variants
                .iter()
                .all(|variant| self.is_exact_null(variant.ty))
        {
            let mut dropped: Vec<usize> = Vec::new();
            let entries = std::mem::take(&mut variants)
                .into_iter()
                .zip(std::mem::take(&mut ref_names))
                .zip(std::mem::take(&mut variant_members));
            for ((variant, ref_name), member) in entries {
                if met_into_null.contains(&member) {
                    dropped.push(member);
                } else {
                    variants.push(variant);
                    ref_names.push(ref_name);
                    variant_members.push(member);
                }
            }
            nullable = !null_twice;
            if variants.len() > 1 {
                let roots: Vec<TypeId> = variants.iter().map(|variant| variant.ty.id).collect();
                let reached = reachable_types(&self.graph, &roots);
                for member in dropped {
                    let Some(range) = meet_inserts.get(member) else {
                        continue;
                    };
                    for id in range.clone().map(TypeId) {
                        if !reached.contains(&id) {
                            self.graph.elide(id);
                        }
                    }
                }
            }
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
        // by the `$ref` arm after the meet instead ([`Self::unmerged_union`]).
        if mode == UnionMode::OneOf
            && schema.discriminator.is_none()
            && self.unmerged_union.as_ref() != Some(&schema.provenance)
        {
            self.merge_indistinguishable_variants(
                schema,
                &mut variants,
                &mut ref_names,
                &mut variant_members,
            );
        }
        // The sibling meet left every branch the exact null type: `type: [string, 'null']` beside
        // untyped `items` branches, which constrain arrays alone, meets each of them in `null`
        // only. An `anyOf` needs one match, so `null` is the one value the union accepts and the
        // union is the null type, as its `allOf` spelling (the `type` as a member beside the
        // union) is (#625). Kept as an enum it was a union of `()` variants beside a `String`
        // constraint, which read as a string-typed union.
        if mode == UnionMode::AnyOf
            && sibling.is_some()
            && variants.len() > 1
            && variants
                .iter()
                .all(|variant| self.is_exact_null(variant.ty))
        {
            variants.truncate(1);
            ref_names.truncate(1);
            variant_members.truncate(1);
        }
        if variants.len() == 1 {
            let inner = variants[0].ty;
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            // The sole variant is re-emitted under this schema's name, so the meets' inserts —
            // the surviving variant's `…Constrained` def, and whatever an excluded member's meet
            // left — are unused unless `kind` reaches them (#462).
            let reached = reachable_types(&self.graph, &kind_edges(&kind));
            for id in meet_inserts.into_iter().flatten().map(TypeId) {
                if !reached.contains(&id) {
                    self.graph.elide(id);
                }
            }
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = inner.nullable || nullable;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        let strategy = if let (Some(discriminator), Some(resolved)) =
            (&schema.discriminator, &discriminator_members)
        {
            // `discriminator_members` already refused a `defaultMapping` naming a non-member. A
            // member the enclosing schema's sibling keywords excluded (`W011`) is the one way left
            // for it to have no variant, and a fallback to a branch the enum does not have cannot
            // be quietly downgraded to another dispatch strategy either.
            let default_variant = match resolved.default {
                None => None,
                Some(member) => {
                    let Some(variant) = variant_members.iter().position(|&m| m == member) else {
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
            let mut discriminated = self.discriminated_strategy(
                &variants,
                &ref_names,
                &variant_members,
                &discriminator.property_name,
                resolved,
                default_variant,
                mode,
            );
            if let Some(UnionStrategy::Discriminated {
                tags,
                categories,
                default_variant,
                untagged,
                ..
            }) = &mut discriminated
            {
                let unselectable: Vec<usize> = (0..variants.len())
                    .filter(|&index| {
                        categories[index].is_none()
                            && tags[index].is_empty()
                            && *default_variant != Some(index)
                    })
                    .collect();
                let component = |index: usize| {
                    ref_names[index]
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
                        variant_members[index],
                        &implicit,
                    );
                }
                // A member that is no component — inline, or a pointer into another schema — has
                // no implicit value, and with no mapping entry naming it no tag selects it;
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
                        .map(|&index| variant_members[index])
                        .collect();
                    let dispatches = tags.iter().any(|accepted| !accepted.is_empty());
                    self.warn_untagged_discriminated_members(discriminator, &members, dispatches);
                    if dispatches {
                        for &index in &unselectable {
                            untagged[index] = Some(
                                self.type_specificity(variants[index].ty, &mut HashSet::new()),
                            );
                        }
                    } else {
                        discriminated = None;
                    }
                }
            }
            discriminated
                .or_else(|| self.disjoint_strategy(&variants))
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        } else {
            self.disjoint_strategy(&variants)
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        };

        let union = Union { variants, strategy };
        // A `$ref`'s own `oneOf` sibling is reported by the `$ref` arm once the meet has made its
        // branches what they are ([`Self::collapse_met_union`]), as its merge is.
        if self.unmerged_union.as_ref() != Some(&schema.provenance) {
            self.warn_untyped_one_of_variants(
                &schema.provenance,
                &union,
                &variant_members,
                "this `oneOf`'s",
                "member",
            );
        }
        let mut ty = self.insert_schema_type(schema, hint, TypeKind::Union(union));
        ty.nullable = nullable;
        Some(ty)
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
        if union.variants.len() < 2
            || !matches!(
                union.strategy,
                UnionStrategy::Trial {
                    mode: UnionMode::OneOf,
                    ..
                }
            )
        {
            return;
        }
        let untyped: Vec<String> = union
            .variants
            .iter()
            .zip(labels)
            .filter(|(variant, _)| {
                matches!(
                    self.graph.get(variant.ty.id).map(|def| &def.kind),
                    Some(TypeKind::Any)
                )
            })
            .map(|(_, label)| label.to_string())
            .collect();
        if untyped.is_empty() {
            return;
        }
        let (noun, verb) = if untyped.len() == 1 {
            (noun.to_owned(), "lowers")
        } else {
            (format!("{noun}s"), "lower")
        };
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
    /// [`TypeGraph::same_decoded_values`]) — into the first of them, keeping `ref_names` and
    /// `variant_members` aligned with `variants`, and report the merge as
    /// `W001` at the union. The union's `null` is not decided here: [`Self::lower_union_closed`]
    /// counts the branches that accept it before any merge, across the merged sets and the rest.
    ///
    /// [`TypeGraph::same_decoded_values`]: crate::ir::TypeGraph::same_decoded_values
    fn merge_indistinguishable_variants(
        &mut self,
        schema: &Schema,
        variants: &mut Vec<UnionVariant>,
        ref_names: &mut Vec<Option<String>>,
        variant_members: &mut Vec<usize>,
    ) {
        // Each kept variant, with the members merged into it.
        let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
        for (index, variant) in variants.iter().enumerate() {
            let shared = groups.iter_mut().find(|(kept, _)| {
                self.graph
                    .same_decoded_values(variants[*kept].ty, variant.ty)
            });
            match shared {
                Some((_, members)) => members.push(variant_members[index]),
                None => groups.push((index, vec![variant_members[index]])),
            }
        }
        if groups.len() == variants.len() {
            return;
        }
        let merged: Vec<String> = groups
            .iter()
            .filter(|(_, members)| members.len() > 1)
            .map(|(_, members)| {
                let members: Vec<String> = members.iter().map(usize::to_string).collect();
                format!("members {}", members.join(", "))
            })
            .collect();
        let consequence = if groups.len() == 1 {
            "the union is that one type"
        } else {
            "each such set is one variant of the generated enum"
        };
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(format!(
                "this `oneOf`'s {} lower to the same generated type or to identically structured \
                 ones, differing at most in keywords it does not carry, so a value matching one \
                 matches all of them and would fail the exactly-one rule: {consequence}, and which \
                 of them a value matches is not enforced",
                merged.join(" and ")
            ))
            .remedy("keep producer-side validation for the union's branch constraints")
            .emit(self.diags);
        fn keep<T>(items: &mut Vec<T>, kept: &HashSet<usize>) {
            *items = std::mem::take(items)
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| kept.contains(&index).then_some(item))
                .collect();
        }
        let kept: HashSet<usize> = groups.iter().map(|(kept, _)| *kept).collect();
        keep(variants, &kept);
        keep(ref_names, &kept);
        keep(variant_members, &kept);
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
        let root = self.resolver.root_id();
        if let (Some(name), SchemaOr::Schema(schema)) =
            (member_component_name(member, root), member)
        {
            let mut sibling = schema.as_ref().clone();
            sibling.reference = None;
            let ty = if schema_has_shape_constraint(&sibling) {
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
