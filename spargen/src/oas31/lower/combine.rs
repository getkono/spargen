//! Flattening an `allOf` into one type: gathering each member's contribution, inline or through
//! a `$ref`, and combining them.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use indexmap::{IndexMap, IndexSet};

use crate::diag::Provenance;
use crate::ir::{AdditionalProps, Docs, Field, FieldDefault, Struct, Ty, TypeKind};
use crate::oas31::{JsonType, Schema, SchemaOr};
use crate::source::is_remote_ref;

use super::all_of::real_union;
use super::meet::{carries_key, drop_applied_default_if_required, merge_field_default, NoMeet};
use super::nullability::{object_all_of_admits_null, stated_nullability, NullAdmission};
use super::refiner::{implied_applicator_category, ImpliedCategory};
use super::shape::{
    schema_has_shape_constraint, schema_has_union, schema_imposes_scalar, schema_is_object_like,
};
use super::{
    all_of_entry_provenance, resolved_hint, resolved_identity, LowerCtx, Refiner, ScopeReach,
    ScopedRefiners, MAX_SCHEMA_DEPTH,
};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Combine the gathered members of an `allOf` into its type; see [`Self::lower_all_of`].
    pub(super) fn combine_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        contributions: &[Contribution],
    ) -> Option<Ty> {
        let mut has_object = contributions
            .iter()
            .any(|c| matches!(c, Contribution::Object { .. }));
        let mut scalars: Vec<Ty> = contributions
            .iter()
            .filter_map(|c| match c {
                Contribution::Scalar(ty) => Some(*ty),
                Contribution::Object { .. } | Contribution::Refiner { .. } => None,
            })
            .collect();
        // Every object member is a `$ref` to a union that lowered to its one object, beside scalar
        // members: each is the union it is, met with them as the same union written inline is,
        // in its place among them (#678). Beside another object member it stays an object.
        if has_object
            && !scalars.is_empty()
            && contributions
                .iter()
                .all(|c| !matches!(c, Contribution::Object { union: None, .. }))
        {
            has_object = false;
            scalars = contributions
                .iter()
                .filter_map(|c| match c {
                    Contribution::Scalar(ty)
                    | Contribution::Object {
                        union: Some(ty), ..
                    } => Some(*ty),
                    Contribution::Object { .. } | Contribution::Refiner { .. } => None,
                })
                .collect();
        }
        // Untyped array applicators constrain only arrays: beside object members they are
        // vacuous, and beside scalars they refine the scalars' meet below.
        let refiners: Vec<Refiner> = contributions
            .iter()
            .filter_map(|c| match c {
                Contribution::Refiner { scoped, .. } => Some(Refiner::Scoped(*scoped)),
                Contribution::Object { .. } | Contribution::Scalar(_) => None,
            })
            .collect();

        // Object-vs-scalar mix has no single representable type.
        if has_object && !scalars.is_empty() {
            return self.reject_all_of_object_scalar_mix(schema);
        }

        // All-scalar allOf: recursively intersect compatible members (for example integer with
        // number, an enum with its underlying scalar, or arrays whose item constraints narrow).
        if !has_object {
            let mark = self.graph_mark();
            // Untyped array applicators with no other constraining member: they establish the
            // category they apply to, as beside a `$ref` to an untyped target and in a union
            // branch that states none, so the meet starts from `Value`, the meet's identity.
            let unconstrained = (scalars.is_empty() && !refiners.is_empty()).then(|| {
                self.insert_type(
                    &format!("{hint}Unconstrained"),
                    TypeKind::Any,
                    Docs::default(),
                    None,
                )
            });
            let Some(mut intersection) = scalars.first().copied().or(unconstrained) else {
                // Only no-constraint members (`true`/`{}`) remained: a faithful open object.
                let ty = self.insert_schema_type(
                    schema,
                    hint,
                    TypeKind::Struct(Struct {
                        fields: Vec::new(),
                        additional: AdditionalProps::Allow,
                    }),
                );
                return Some(ty);
            };
            for (index, member) in scalars.iter().copied().enumerate().skip(1) {
                let merged = match self.intersect_types(
                    intersection,
                    member,
                    &format!("{hint}Intersection{index}"),
                ) {
                    Ok(merged) => merged,
                    // The members share no non-null value, but the one null decision says the
                    // `allOf` states `null` and `null` satisfies every member: a union member whose
                    // branch `null` matches through an untyped branch of its own (a `$ref` to such
                    // a union) lowered non-null on its own. Only `null` is left, the exact null type
                    // the sibling spelling gives the same constraints (#674). A `null` only the
                    // decision's departure admits (a null-only `type` beside the `allOf`) is not
                    // such a `null`: it makes the members' type optional, and members sharing no
                    // value make none, so the schema is `E013`, as before the one decision.
                    Err(NoMeet::Empty)
                        if self.null_admission(schema) == NullAdmission::Admits
                            && !self.null_admitted_only_by_departure(schema) =>
                    {
                        self.discard_meet_intermediates(mark, &TypeKind::Null);
                        return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
                    }
                    Err(_) => return self.reject_all_of_scalars(schema),
                };
                intersection = merged;
            }
            // Under the enclosing narrowing, as the scalar meets above: a refiner is one more
            // member of the same `allOf`. Its array half meets the intersection's arrays (a
            // union's array branches) and leaves every other category as it is; `Value` takes the
            // array category from it.
            for (index, refiner) in refiners.iter().copied().enumerate() {
                let Ok(met) = self.meet_refiner(
                    intersection,
                    refiner,
                    &mut ScopeReach::default(),
                    &format!("{hint}Refined{index}"),
                ) else {
                    return self.reject_all_of_scalars(schema);
                };
                intersection = met;
            }
            // A `oneOf` the meets left with branches of one generated type collapses with `W001`,
            // as the meet of a union member does (#643); an `anyOf` member met with a `oneOf`
            // member left such `oneOf`s nested in its branches.
            intersection = self.collapse_met_one_ofs(schema, intersection, mark);
            // Re-emit the intersection as the final graph insert so the invariant holds even when
            // the allOf is a component body (the per-member scalar inserts above are left dead —
            // `#[allow(dead_code)]` on the models module — rather than threading a reserved id).
            // The meets' own inserts are discarded unless the re-emitted kind reaches them.
            let kind = self
                .graph
                .get(intersection.id)
                .map(|def| def.kind.clone())?;
            let mut ty = self.reemit_meet(schema, hint, mark, kind);
            ty.nullable = intersection.nullable;
            self.elide_unused_member_lowerings(contributions);
            return Some(ty);
        }

        // All object members: flatten into one struct. Property union preserves first-seen order.
        let mut fields: IndexMap<String, Field> = IndexMap::new();
        let mut required: Vec<String> = Vec::new();
        let mut additional = AdditionalProps::Allow;
        // Repeated properties whose types have no common value, in first-seen order.
        let mut uninhabited: IndexSet<String> = IndexSet::new();
        // Every `default` a member writes for each property, as that member wrote it. Which one the
        // merged field keeps is decided once over all of them after the loop (see
        // `merge_field_default`), never pair by pair as members arrive (#577).
        let mut written: IndexMap<String, Vec<FieldDefault>> = IndexMap::new();
        // Every member is lowered already, so what the merge inserts from here on is its meets'.
        let mark = self.graph_mark();
        for contribution in contributions {
            let Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: member_required,
                ..
            } = contribution
            else {
                continue;
            };
            for name in member_required {
                if !required.contains(name) {
                    required.push(name.clone());
                }
            }
            match self.merge_additional(
                &additional,
                member_additional,
                &format!("{hint}Additional"),
            ) {
                Some(merged) => additional = merged,
                None => {
                    // `merge_additional` can only decline by failing to intersect the two value
                    // types, and that has two causes the author has to tell apart. A genuine
                    // conflict is one sentence; a value schema that is a `$ref` back to the type
                    // being lowered is another, and calling it "conflicting" sends the reader
                    // looking for a disagreement that is not in the document — nothing conflicts,
                    // the target's body simply has not been computed yet.
                    let unlowered = [&additional, member_additional].into_iter().any(|policy| {
                        matches!(policy, AdditionalProps::Typed(ty) if self.is_reservation(ty.id))
                    });
                    if unlowered {
                        return self.reject_all_of_cycle(
                            schema.provenance.clone(),
                            "an `allOf` member's `additionalProperties` value schema is a `$ref` \
                             that closes a reference cycle back to the schema being lowered, whose \
                             body is not yet known, so the merged overflow map has no computable \
                             value type",
                        );
                    }
                    return self.reject_all_of_additional(schema);
                }
            }
            for field in member_fields {
                if let Some(default) = &field.default {
                    written
                        .entry(field.name.wire.clone())
                        .or_default()
                        .push(default.clone());
                }
                match fields.get_mut(&field.name.wire) {
                    Some(existing) => {
                        // A field one side carries only because it requires the name is not a
                        // declaration of the property, so it does not intersect with one: the
                        // declaring member supplies the type and the metadata, and the requirement
                        // survives (see `merge_repeated_field`). Every member's `default` is
                        // decided after the loop, so none is merged pair by pair here.
                        let field_hint = format!("{hint}{}Intersection", field.name.wire);
                        match self.merge_repeated_field(existing, field, &field_hint, false) {
                            None | Some(Ok(())) => {}
                            // A reservation's body is not known yet, so the failure here says
                            // nothing about whether the property's types meet; typing the field
                            // uninhabited would be a guess. Refuse it, naming the cycle rather
                            // than a conflict nobody wrote.
                            Some(Err(_))
                                if self.is_reservation(existing.ty.id)
                                    || self.is_reservation(field.ty.id) =>
                            {
                                let message = format!(
                                    "property `{}` repeated across `allOf` members is typed by a \
                                     `$ref` that closes a reference cycle back to the schema \
                                     being lowered, so its intersection cannot be computed",
                                    field.name.wire
                                );
                                return self
                                    .reject_all_of_cycle(schema.provenance.clone(), &message);
                            }
                            // The same rule `intersect_structs` applies to a `$ref` and its
                            // siblings, so the four equivalent spellings of one conjunction agree:
                            // the types cannot meet, but that empties the object only if some
                            // instance must carry the property. Whether one must is not known
                            // until every member's `required` has been read — a later member may
                            // require it without declaring it — so the field takes an uninhabited
                            // type now and the requirement is settled after the loop. A member's
                            // applied `default` is left for `retype_field_defaults`, which finds
                            // it no value of the uninhabited type and reports it (`W005`) where it
                            // was written, documenting it as not applied (#453).
                            Some(Err(NoMeet::Empty)) => {
                                uninhabited.insert(field.name.wire.clone());
                                existing.ty = self.insert_type(
                                    &field_hint,
                                    TypeKind::Never,
                                    Docs::default(),
                                    None,
                                );
                            }
                            // Only an empty meet is uninhabited: these two types share values, and
                            // an uninhabited field would refuse every object carrying one.
                            Some(Err(NoMeet::Unrepresentable)) => {
                                let message = format!(
                                    "property `{}` repeated across `allOf` members has types that \
                                     share values no single Rust type represents",
                                    field.name.wire
                                );
                                return self.reject_unrepresentable_meet(schema, &message);
                            }
                        }
                    }
                    None => {
                        fields.insert(field.name.wire.clone(), field.clone());
                    }
                }
            }
        }

        // Every member's `default` is a default of the merged field, whichever member came first
        // (see `merge_field_default`). This is decided before any verdict below, so the `default`s
        // the merge drops are reported beside it (#545).
        for field in fields.values_mut() {
            let defaults = written.shift_remove(&field.name.wire).unwrap_or_default();
            field.default = merge_field_default(defaults, &field.name.wire, self.diags);
        }

        // A field no member declares is an undeclared key of every member, so each member's
        // `additionalProperties` value schema constrains it, not only the requiring member's own:
        // `allOf: [{$ref: Labels}, {required: [a]}]` with string-valued `Labels` makes `a` a
        // string, not an unconstrained value. The requiring member already applied its own.
        for contribution in contributions {
            let Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                ..
            } = contribution
            else {
                continue;
            };
            for field in fields.values_mut() {
                if !field.undeclared || carries_key(member_fields, &field.name.wire) {
                    continue;
                }
                let field_hint = format!("{hint}{}Intersection", field.name.wire);
                match self.narrow_undeclared(field.ty, member_additional, &field_hint) {
                    Ok(ty) => field.ty = ty,
                    Err(_)
                        if self.is_reservation(field.ty.id)
                            || matches!(member_additional, AdditionalProps::Typed(value) if self.is_reservation(value.id)) =>
                    {
                        let message = format!(
                            "required property `{}`, which no `allOf` member declares, is typed by \
                             an `additionalProperties` value schema that is a `$ref` closing a \
                             reference cycle back to the schema being lowered, so its \
                             intersection cannot be computed",
                            field.name.wire
                        );
                        return self.reject_all_of_cycle(schema.provenance.clone(), &message);
                    }
                    // The field is required, so a value no type admits empties the object meet:
                    // only `null` can be left (see `null_only_all_of`).
                    Err(NoMeet::Empty) => {
                        let name = field.name.wire.clone();
                        return self
                            .null_only_all_of(schema, hint, contributions, mark)
                            .or_else(|| self.reject_all_of_undeclared_required(schema, &name));
                    }
                    Err(NoMeet::Unrepresentable) => {
                        let message = format!(
                            "required property `{}`, which no `allOf` member declares, is typed by \
                             `additionalProperties` value schemas that share values no single Rust \
                             type represents",
                            field.name.wire
                        );
                        return self.reject_unrepresentable_meet(schema, &message);
                    }
                }
            }
        }

        // An uninhabited property that any member requires obliges every object instance to carry
        // a value no type admits: no object satisfies the composition, which leaves only `null`
        // (see `null_only_all_of`), and where `null` does not satisfy it either that is the
        // document error.
        if let Some(name) = uninhabited.iter().find(|name| {
            required.contains(name) || fields.get(*name).is_some_and(|field| field.required)
        }) {
            let name = name.clone();
            return self
                .null_only_all_of(schema, hint, contributions, mark)
                .or_else(|| self.reject_all_of_required_property(schema, &name));
        }

        // Apply the required union, then keep required fields consistent: a serde default only fires
        // for an absent optional field, so a field promoted to required by another member drops its
        // applied default (it stays documented in rustdoc).
        let mut fields: Vec<Field> = fields.into_values().collect();
        for field in &mut fields {
            if required.contains(&field.name.wire) {
                field.required = true;
            }
            drop_applied_default_if_required(field);
        }

        // A property repeated by three or more members is met pair by pair, and each meet replaces
        // the field's type, so the struct refers to the last meet and not to the ones before it.
        let kind = TypeKind::Struct(Struct { fields, additional });
        self.elide_meet_intermediates(mark, &kind);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        // As the all-scalar branch takes its meet's nullability: `null` satisfies the merge when
        // it satisfies every member.
        ty.nullable = object_all_of_admits_null(contributions);
        self.elide_unused_member_lowerings(contributions);
        Some(ty)
    }

    /// Elide what the merge's result leaves unused of each inline member's own lowering that
    /// [`Contribution::Object`]'s `lowered` records: the union of its `type` array's categories
    /// where the merge took it as an object, and its fields where it took it as that union. The
    /// result is inserted already, so whatever it reaches stays.
    fn elide_unused_member_lowerings(&mut self, contributions: &[Contribution]) {
        for contribution in contributions {
            if let Contribution::Object {
                lowered: Some(lowered),
                ..
            } = contribution
            {
                self.elide_unused_union_lowering(lowered.clone());
            }
        }
    }

    /// Gather every member of `schema.all_of` (source order) plus the enclosing schema's own object
    /// siblings (last), pushing a [`Contribution`] per constraining member.
    pub(super) fn gather_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        for (index, member) in schema.all_of.iter().enumerate() {
            self.gather_member(
                member,
                (&schema.provenance, index),
                &format!("{hint}Member{index}"),
                out,
            )?;
        }
        // The enclosing schema may carry its own object keywords beside `allOf`; fold them in last.
        if schema_is_object_like(schema) {
            let (member_fields, member_additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
                union: None,
                lowered: None,
            });
        }
        Some(())
    }

    /// Push the contributions of one `allOf` member, `entry` being where it is written: the
    /// provenance of the schema whose `allOf` holds it, and its index there. A member that is a
    /// boolean schema carries no provenance of its own, so `entry` is what locates it.
    pub(super) fn gather_member(
        &mut self,
        member: &SchemaOr,
        entry: (&Provenance, usize),
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        match member {
            // A `true`/`{}` member imposes no constraint.
            SchemaOr::Bool(true) => Some(()),
            SchemaOr::Bool(false) => {
                let (holder, index) = entry;
                self.reject_all_of_false_member(all_of_entry_provenance(
                    self.resolver,
                    holder,
                    index,
                ))
            }
            SchemaOr::Schema(schema) => self.gather_schema_member(schema, hint, out),
        }
    }

    /// [`Self::gather_member`] of a member that is a schema object, or of a schema gathered as
    /// one: a `$ref` member's siblings, or a `$ref` target expanded in place.
    fn gather_schema_member(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // An inline member, or a non-component target expanded in place, is read by its keywords
        // here rather than lowered through `lower_schema_inner`, which would report this.
        self.diagnose_standalone_discriminator(schema);

        if let Some(reference) = &schema.reference {
            self.gather_ref_target(schema, reference, hint, out)?;
            // `$ref` is an applicator, not a replacement for the member that holds it: the member
            // is the target AND its own shape-bearing siblings, so those are further conjuncts of
            // this same merge, gathered exactly as a separate member carrying them would be.
            // Returning once the target is pushed would delete the siblings' properties and
            // `required` silently, and let a sibling contradicting its target generate as the
            // target alone. The gate is the one `lower_schema_inner` asks of a
            // `$ref`'s siblings, so the two positions agree on what counts as a shape.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                return Some(());
            }
            return self.gather_schema_member(&sibling, &format!("{hint}Constraint"), out);
        }

        if !schema.all_of.is_empty() && !schema_has_union(schema) {
            // Nested allOf: flatten its members (and its own siblings) into the same accumulator.
            // One with a union beside it is that composition met with the union, which only
            // lowering computes, so `gather_inline` lowers it as the scalar it then is.
            return self.gather_all_of(schema, hint, out);
        }

        self.gather_inline(schema, hint, out)
    }

    /// Push the contribution of an `allOf` member's `$ref` target — and only the target: the
    /// member's own siblings are [`Self::gather_member`]'s to gather, after this returns.
    fn gather_ref_target(
        &mut self,
        schema: &Schema,
        reference: &str,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            // A `$ref` to a component still being lowered is a direct recursive allOf member
            // whose fields are not yet known — irreconcilable (distinct from a member with
            // recursive *fields*, which lowers fine).
            if self.in_progress.contains_key(name) {
                return self
                    .reject_all_of_cycle(schema.provenance.clone(), RECURSIVE_COMPONENT_MEMBER);
            }
            let ty = self.ensure_component(name, Some(reference), &schema.provenance)?;
            let decides_null = self.ref_target_decides_null(reference, &schema.provenance);
            // The pre-check above sees root components only. A name the root does not declare
            // is a *sub-file* component, and it reaches its own reservation through
            // `ensure_resolved`, so a direct recursive member there arrives here as a back-edge
            // rather than being caught above; `push_ref_member` refuses to read it.
            return self.push_ref_member(
                RefMember {
                    ty,
                    decides_null,
                    reference,
                    provenance: &schema.provenance,
                    recursive: RECURSIVE_COMPONENT_MEMBER,
                },
                hint,
                out,
            );
        }
        // A remote `$ref` member goes through the cycle-safe remote path, exactly like a
        // component member: a member still being lowered is a direct recursive ref whose fields
        // are not yet known (irreconcilable), otherwise its shared type contributes its fields.
        if is_remote_ref(reference) {
            if self.remote_in_progress.contains_key(reference) {
                return self
                    .reject_all_of_cycle(schema.provenance.clone(), RECURSIVE_REMOTE_MEMBER);
            }
            let ty = self.ensure_remote(reference, &schema.provenance)?;
            let decides_null = self.ref_target_decides_null(reference, &schema.provenance);
            return self.push_ref_member(
                RefMember {
                    ty,
                    decides_null,
                    reference,
                    provenance: &schema.provenance,
                    recursive: RECURSIVE_REMOTE_MEMBER,
                },
                hint,
                out,
            );
        }
        // Non-component refs resolve (or error) exactly as `lower_schema` does; the target is then
        // gathered as an inline member would be (see `gather_resolved_target`).
        let resolved = self
            .resolver
            .resolve(reference, &schema.provenance, self.diags)
            .ok()?;
        let target = resolved.schema.into_owned();
        // This arm inlines rather than referencing a shared type, so there is no `Ty` to test —
        // test the target's identity (`reservation_at`) instead of `is_in_progress_root`. Without
        // this, a member that is the very schema being lowered descends into its own body again
        // and stops only at `MAX_SCHEMA_DEPTH`, reporting a chain length for what is a cycle of
        // length one. Like the component and remote arms above, this one refuses to read an
        // in-progress member.
        if self.reservation_at(&target.provenance).is_some() {
            return self.reject_all_of_cycle(
                schema.provenance.clone(),
                "an `allOf` member is a direct recursive `$ref` to the schema being lowered",
            );
        }
        // Expand the target once per resolved `file#pointer` and replay its contribution at
        // every later use: see `resolved_contributions`. The in-progress test above runs
        // first on every use, so a replay never stands in for a refusal. Only the target is
        // memoised: the member's siblings belong to this use, and `gather_member` adds them.
        let Some(key) = resolved_identity(&target.provenance) else {
            // No span, so no identity to key on — expand un-memoised, as `ensure_resolved`
            // lowers un-deduplicated in the same case. The depth cap still bounds it.
            return self.gather_resolved_target(target, hint, out);
        };
        if self.resolved_contributions.contains_key(&key) {
            self.replay_resolved(&key, out);
            return Some(());
        }
        // The target's expansion follows its own `$ref` and `allOf` members, and nothing on that
        // path reserves a type a re-entry could be boxed against, so a target this expansion is
        // already inside is a loop: reject it rather than recurse to the depth cap. A loop made
        // only of bare aliases is the alias cycle `ensure_resolved` reports; one that passes
        // through a body is a member recursive through its own composition, as the root document
        // reports it.
        if let Some(start) = self
            .resolved_member_stack
            .iter()
            .position(|(open, _)| *open == key)
        {
            if self.resolved_member_stack[start..]
                .iter()
                .all(|&(_, alias)| alias)
            {
                return self.reject_schema_alias_cycle(schema.provenance.clone(), reference);
            }
            return self.reject_all_of_cycle(
                schema.provenance.clone(),
                "an `allOf` member is a recursive `$ref` that reaches itself through its target's \
                 own `$ref` and `allOf` members",
            );
        }
        // Name what the body lowers to for the schema it came from, not for whichever use
        // reached it first — once one expansion serves every use, a per-use hint would make
        // the generated names depend on lowering order. The `Member` suffix keeps it off the
        // hint `ensure_resolved` gives the same target when it is also a direct `$ref`: that
        // lowers a second copy of the body, and two copies on one hint would leave the bare
        // name (`Basemeta`, or a scalar target's own `Code`) to whichever lowering ran first.
        let hint = format!("{}Member", resolved_hint(&target.provenance, hint));
        // The expansion is its own gathering, so what it records holds each nested target once
        // and does not depend on what the use that first reached it had already gathered.
        let mut contributed = Vec::new();
        self.resolved_member_stack
            .push((key.clone(), target.reference.is_some()));
        let (expanded, gathering) =
            self.gathering(|ctx| ctx.gather_resolved_target(target, &hint, &mut contributed));
        self.resolved_member_stack.pop();
        expanded?;
        // A target's member whose `type` names `object` beside another category is met as its
        // object alone, as before #827: the union of its categories is read only for a member
        // written inline in the composition ([`objects_met_as_their_unions`]). Recorded here, it
        // would stay reachable from the memo at every later use, and be emitted unused.
        let mut unread = Vec::new();
        for contribution in &mut contributed {
            if let Contribution::Object {
                union,
                lowered: lowered @ Some(_),
                ..
            } = contribution
            {
                *union = None;
                unread.extend(lowered.take());
            }
        }
        let recorded = contributed
            .into_iter()
            .enumerate()
            .map(|(index, contribution)| (gathering.origins.get(&index).cloned(), contribution))
            .collect();
        self.resolved_contributions.insert(key.clone(), recorded);
        // The fields stay, held by the record; what only the union reached goes.
        self.elide_unused_lowerings(unread);
        self.replay_resolved(&key, out);
        Some(())
    }

    /// Run `gather` as one composition's gathering: the bundle targets it adds are counted once
    /// within it, and not against the gathering it is nested in (see `LowerCtx::gatherings`).
    pub(super) fn in_composition<T>(&mut self, gather: impl FnOnce(&mut Self) -> T) -> T {
        self.gathering(gather).0
    }

    /// [`Self::in_composition`], also returning what the gathering recorded.
    fn gathering<T>(&mut self, gather: impl FnOnce(&mut Self) -> T) -> (T, Gathering) {
        self.gatherings.push(Gathering::default());
        let gathered = gather(self);
        let gathering = self
            .gatherings
            .pop()
            .expect("a gathering pops the frame it pushed");
        (gathered, gathering)
    }

    /// Add bundle target `key`'s recorded contribution to `out`, the innermost gathering's
    /// contributions: nothing where that gathering already holds `key`, and otherwise each
    /// contribution but those of a nested target it already holds. A held target's own nested
    /// targets are held as well, since each was added with it or before it, so a replay never
    /// adds part of a target twice.
    ///
    /// Both lookups are invariants, not fallbacks: the caller replays `key` only once it is
    /// recorded, and every gather runs inside a gathering ([`Self::in_composition`] at each `allOf`
    /// entry point, [`Self::gathering`] around each expansion). Replaying with no gathering open
    /// would append each copy flat, the exponential replay of #616, so it panics instead.
    fn replay_resolved(&mut self, key: &str, out: &mut Vec<Contribution>) {
        let recorded = self
            .resolved_contributions
            .get(key)
            .expect("a bundle target is replayed only once its contribution is recorded");
        let gathering = self
            .gatherings
            .last_mut()
            .expect("a bundle target is replayed only inside an open gathering");
        if gathering.seen.contains(key) {
            return;
        }
        let mut added = Vec::new();
        for (origin, contribution) in recorded {
            let origin = origin.as_deref().unwrap_or(key);
            if origin != key && gathering.seen.contains(origin) {
                continue;
            }
            gathering.origins.insert(out.len(), origin.to_owned());
            out.push(contribution.clone());
            added.push(origin);
        }
        gathering.seen.extend(added.into_iter().map(str::to_owned));
        gathering.seen.insert(key.to_owned());
    }

    /// Expand a bundle-`$ref` `allOf` member's resolved target as [`Self::gather_member`] expands
    /// any member: a target that is itself a `$ref` chains to *its* target (and gathers its own
    /// siblings), one that is an `allOf` flattens its members, and only a plain body is read for
    /// object or scalar keywords. An `allOf` or alias target carries neither kind of keyword, so
    /// reading it as a plain body would take it for a pure annotation and silently drop everything
    /// it constrains (issue #306).
    ///
    /// This recursion does not pass through [`Self::lower_schema`], so it counts against
    /// [`Self::depth`] itself: a long acyclic chain of such targets rejects with `E014` rather than
    /// exhausting the stack. Loops are the caller's to refuse, before this is entered.
    fn gather_resolved_target(
        &mut self,
        target: Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // The target's contribution is memoised and replayed at every later use of it, so what it
        // lowers must not depend on the position that first reached it: it is a `$ref` target,
        // and `open_narrowing` is out of effect there. The merge of its fields with the enclosing
        // members' still happens at the use site.
        self.closed_narrowing(|ctx| ctx.gather_resolved_target_closed(target, hint, out))
    }

    /// [`Self::gather_resolved_target`]'s body, run with `open_narrowing` out of effect.
    fn gather_resolved_target_closed(
        &mut self,
        target: Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        if self.depth >= MAX_SCHEMA_DEPTH {
            return self.reject_too_deep(&target.provenance);
        }
        self.depth += 1;
        let result = self.gather_schema_member(&target, hint, out);
        self.depth -= 1;
        result
    }

    /// Turn a resolved `$ref` member's already-lowered type into a contribution: an object component
    /// contributes a *copy* of its fields/`additionalProperties`; a target of untyped array
    /// applicators alone contributes the [`Contribution::Refiner`] the same member written inline
    /// does, lowered on `hint`; any other lowered kind is a scalar member.
    ///
    /// A member whose body is still being lowered is refused here, with `recursive` as the
    /// message, rather than by each caller: a reservation's kind says nothing about the schema's
    /// shape, and reading it as "not a struct" would make a recursive member a silent scalar. The
    /// guard lives here, so no caller has one to forget.
    ///
    /// `decides_null` is [`Self::ref_target_decides_null`] of the target: an untyped object target,
    /// or one composed of untyped object members alone, admits `null` without deciding the merge's
    /// nullability, as the same member written inline does (issues #541, #565), so its lowered
    /// non-null struct is not recorded as a decision.
    fn push_ref_member(
        &mut self,
        member: RefMember<'_>,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        let RefMember {
            ty,
            decides_null,
            reference,
            provenance,
            recursive,
        } = member;
        // Every id `is_in_progress_root` accepts is still a `Reserved` placeholder — each
        // in-progress map is entered with a fresh `reserve` and left before its `fill` — so this
        // arm is the whole guard against reading one.
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Reserved) => {
                return self.reject_all_of_cycle(provenance.clone(), recursive)
            }
            // Read by the type it lowers to standalone, a target of untyped array applicators
            // alone would deny the merge's other categories as an array (#614), or as `Value` meet
            // the merge as its identity and drop its `items` with no diagnostic (#610). Either way
            // the target is read by its keywords, as an inline member and a sub-file
            // target are (#607): it refines the merge's arrays and constrains nothing else. It is a
            // `$ref` target, so `open_narrowing` is out of effect for it.
            Some(TypeKind::Any | TypeKind::Array(_) | TypeKind::Tuple(_)) => {
                let target = self
                    .ref_target_body(reference, provenance)
                    .filter(|target| {
                        matches!(
                            implied_applicator_category(target),
                            Some(ImpliedCategory::Only(JsonType::Array))
                        )
                    })
                    .map(std::borrow::Cow::into_owned);
                let Some(target) = target else {
                    out.push(Contribution::Scalar(ty));
                    return Some(());
                };
                let scoped = self
                    .closed_narrowing(|ctx| ctx.lower_scoped_refiners(&target, true, None, hint))?;
                out.push(Contribution::Refiner {
                    scoped,
                    member: Box::new(target),
                });
            }
            Some(TypeKind::Struct(structure)) => {
                let fields = structure.fields.clone();
                let required = fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.name.wire.clone())
                    .collect();
                let additional = structure.additional.clone();
                let union = self
                    .ref_target_body(reference, provenance)
                    .is_some_and(|target| schema_has_union(&target))
                    .then_some(ty);
                // A copied field keeps its `undeclared` mark, so one the component carries only
                // for its own `required` still gives way to a later member's declaration.
                out.push(Contribution::Object {
                    fields,
                    additional,
                    required,
                    nullable: decides_null.then_some(ty.nullable),
                    union,
                    lowered: None,
                });
            }
            _ => out.push(Contribution::Scalar(ty)),
        }
        Some(())
    }

    fn gather_inline(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // A member carrying its own `oneOf`/`anyOf` is that union, its object keywords refining the
        // branches as `lower_union` refines them; read as an object by its keywords, the union
        // would be dropped with no diagnostic (issue #419).
        if schema_is_object_like(schema) && !schema_has_union(schema) {
            let from = self.graph_mark();
            let (fields, additional) = self.object_body(schema, hint)?;
            // A `type` naming `object` beside another non-null category admits that category's
            // instances too, so the member also carries the union of its categories it lowers to
            // on its own, which a union met with the merge meets branch by branch: read as an
            // object alone, it dropped every branch of that union that is not an object, and
            // reported each as excluded (#827).
            let union = if names_object_beside_another_category(schema) {
                Some(self.lower_schema(schema, hint)?)
            } else {
                None
            };
            let lowered = union.map(|_| from..self.graph_mark());
            out.push(Contribution::Object {
                fields,
                additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
                union,
                lowered,
            });
        } else if schema_imposes_scalar(schema) {
            let ty = self.lower_schema(schema, hint)?;
            out.push(Contribution::Scalar(ty));
        } else if matches!(
            implied_applicator_category(schema),
            Some(ImpliedCategory::Only(JsonType::Array))
        ) {
            // Untyped array applicators (`items`, `prefixItems`) name no `type`, so neither arm
            // above reads them; this arm keeps them from being dropped as an annotation with no
            // diagnostic (#607). In 2020-12 they constrain only arrays, so they refine the merge's
            // array members and
            // leave the rest, as the same member does beside a union.
            let scoped = self.lower_scoped_refiners(schema, true, None, hint)?;
            out.push(Contribution::Refiner {
                scoped,
                member: Box::new(schema.clone()),
            });
        }
        // Otherwise the member is a pure annotation (`{description: ...}`): no constraint.
        Some(())
    }
}

/// One `allOf` member's contribution to the merged type: a set of object fields (with its
/// `additionalProperties` policy and its own `required` names) to flatten, a scalar/leaf type, or
/// untyped array applicators that refine the merge's arrays alone.
/// `Clone` so a bundle-`$ref` member's contribution can be recorded once and replayed at every use
/// (see `LowerCtx::resolved_contributions`).
#[derive(Clone)]
pub(super) enum Contribution {
    Object {
        fields: Vec<Field>,
        additional: AdditionalProps,
        required: Vec<String>,
        /// Whether the member admits `null`, where the member decides it: `Some` for a member
        /// that states a `type` (whether it lists `"null"`) or is a `$ref` target that decides it
        /// (its lowered nullability, [`LowerCtx::ref_target_decides_null`]), `None` for an untyped
        /// one, inline ([`stated_nullability`]) or a `$ref` to an untyped object component, or to
        /// one composed of untyped object members alone. An untyped member's
        /// object keywords constrain only objects, so it admits `null` without deciding the
        /// merge's nullability, as an untyped `$ref` sibling leaves its target's alone.
        nullable: Option<bool>,
        /// The type a `$ref` member's target lowered to, where that target is a `oneOf`/`anyOf`
        /// that lowered to one object (its one real branch): beside scalar members alone it is
        /// met with them as the union it is ([`LowerCtx::combine_all_of`]), as the same union
        /// written inline is, rather than mixed with them as an object (#678). For an inline
        /// member whose `type` names `object` beside another non-null category, the union of its
        /// categories it lowers to on its own, met as that union beside scalar members alone and
        /// beside a union the merge is met with ([`objects_met_as_their_unions`], #827). `None`
        /// for every other object member.
        union: Option<Ty>,
        /// The ids the inline member's own lowering inserted, where `union` is the union of its
        /// `type` array's categories: its fields and that union, each elided where the merge
        /// leaves it unused ([`LowerCtx::elide_unused_union_lowering`]). `None` for every other
        /// object member.
        lowered: Option<Range<u32>>,
    },
    Scalar(Ty),
    /// A member of untyped array applicators alone (`items`, `prefixItems`, no `type`): the
    /// halves of a [`Refiner::Scoped`] sibling, with only the array half, met with the merge's
    /// arrays and vacuous for every other category (#607). Where no other member constrains,
    /// they establish the array category, as beside a `$ref` to an untyped target. `member` is the
    /// member as written: beside a union it refines the union's branches instead
    /// ([`take_refiners`]), and a half that reaches none is reported there (`W011`).
    Refiner {
        scoped: ScopedRefiners,
        member: Box<Schema>,
    },
}

/// One contribution of a recorded bundle-`$ref` target expansion, with the nested bundle target
/// whose own contribution it is: `None` for the expanded target's own (see
/// `LowerCtx::resolved_contributions`).
pub(super) type RecordedContribution = (Option<String>, Contribution);

/// What one composition, or one bundle-`$ref` target expansion, has gathered of bundle targets
/// (see `LowerCtx::gatherings`).
#[derive(Default)]
pub(super) struct Gathering {
    /// The bundle targets whose contribution the gathering already holds.
    seen: HashSet<String>,
    /// The bundle target each replayed contribution came from, by its index in the gathering's
    /// contributions, so the record of an expansion can say which nested target each is.
    origins: HashMap<usize, String>,
}

/// Remove the [`Contribution::Refiner`]s from `contributions`, in order, as the members and
/// refiners a union beside or inside the `allOf` is met with: gathered from a nested `allOf`, or
/// a bundle `$ref`'s target, they refine the union's branches of their category, as the same
/// member written directly in the `allOf` does, rather than establish the array category for
/// the whole composition and drop every other branch.
pub(super) fn take_refiners(contributions: &mut Vec<Contribution>) -> Vec<(Box<Schema>, Refiner)> {
    let mut refiners = Vec::new();
    contributions.retain(|contribution| match contribution {
        Contribution::Refiner { scoped, member } => {
            refiners.push((member.clone(), Refiner::Scoped(*scoped)));
            false
        }
        Contribution::Object { .. } | Contribution::Scalar(_) => true,
    });
    refiners
}

/// Take each inline object member of `contributions` whose `type` names `object` beside another
/// non-null category as the union of its categories it lowered to ([`Contribution::Object`]'s
/// `union`), where no other object member keeps the merge an object, and return the ids each
/// one's own lowering inserted, for the caller to elide once its result no longer uses them. A
/// union met with the merge then meets that member branch by branch, as the sibling spelling
/// meets the same `type` array (#827): read as an object, it dropped every branch of the union
/// that is not an object.
pub(super) fn objects_met_as_their_unions(contributions: &mut [Contribution]) -> Vec<Range<u32>> {
    let keeps_object = contributions
        .iter()
        .any(|contribution| matches!(contribution, Contribution::Object { lowered: None, .. }));
    if keeps_object {
        return Vec::new();
    }
    let mut lowerings = Vec::new();
    for contribution in contributions.iter_mut() {
        if let Contribution::Object {
            union: Some(union),
            lowered: Some(lowered),
            ..
        } = contribution
        {
            lowerings.push(lowered.clone());
            *contribution = Contribution::Scalar(*union);
        }
    }
    lowerings
}

/// Whether `schema`'s own `type` names `object` and at least one other category besides `null`:
/// a multi-category `type` array, which lowers to a union of its categories rather than to an
/// object ([`LowerCtx::gather_inline`]).
fn names_object_beside_another_category(schema: &Schema) -> bool {
    let types = &schema.types.types;
    types.contains(&JsonType::Object)
        && types
            .iter()
            .any(|ty| !matches!(ty, JsonType::Object | JsonType::Null))
}

impl LowerCtx<'_, '_> {
    /// The index of the one `allOf` member that is a union
    /// ([`LowerCtx::lower_all_of_with_union_member`]): an inline `oneOf`/`anyOf`, or a `$ref`
    /// with no shape-bearing siblings to a root component that is one and is not being lowered
    /// already, which is met as its
    /// target's union (#841). Where several members are unions and exactly one of them has two or
    /// more non-null branches ([`real_union`]), it is that one: the others each lower to their one
    /// real branch, and are met with the object members as that branch (#841). Otherwise an
    /// `allOf` with several union members is an ordinary `allOf`: its unions meet as scalar
    /// members, and beside object members they are `E013`
    /// ([`LowerCtx::reject_all_of_several_unions`]).
    pub(super) fn sole_union_member(&self, schema: &Schema) -> Option<usize> {
        let unions: Vec<(usize, bool)> = schema
            .all_of
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                let SchemaOr::Schema(member) = member else {
                    return None;
                };
                let Some(reference) = member.reference.as_deref() else {
                    return schema_has_union(member).then(|| (index, real_union(member)));
                };
                // A component still being lowered is a recursive member, which the gather
                // refuses ([`LowerCtx::gather_ref_target`]); its body is not expanded here.
                let name = reference.strip_prefix("#/components/schemas/")?;
                if self.in_progress.contains_key(name) {
                    return None;
                }
                let mut siblings = member.as_ref().clone();
                siblings.reference = None;
                if schema_has_shape_constraint(&siblings) {
                    return None;
                }
                let target = self.ref_target_body(reference, &member.provenance)?;
                schema_has_union(&target).then(|| (index, real_union(&target)))
            })
            .collect();
        if let [(index, _)] = unions.as_slice() {
            return Some(*index);
        }
        let mut real = unions.iter().filter(|(_, real)| *real);
        let (index, _) = real.next()?;
        real.next().is_none().then_some(*index)
    }
}

/// The `E013` cycle wording for an `allOf` member that is a `$ref` to a root component still being
/// lowered: refused before the component is read, and by [`LowerCtx::push_ref_member`] for the
/// sub-file component that reaches its reservation the other way.
const RECURSIVE_COMPONENT_MEMBER: &str =
    "an `allOf` member is a direct recursive `$ref` to the component being lowered";

/// [`RECURSIVE_COMPONENT_MEMBER`] for a remote `$ref` member.
const RECURSIVE_REMOTE_MEMBER: &str =
    "an `allOf` member is a direct recursive remote `$ref` to the schema being lowered";

/// A component or remote `$ref` `allOf` member, its target already lowered, as
/// [`LowerCtx::push_ref_member`] reads it.
struct RefMember<'r> {
    /// The target's shared lowered type.
    ty: Ty,
    /// [`LowerCtx::ref_target_decides_null`] of the target.
    decides_null: bool,
    /// The `$ref` as written, read for the target's own keywords where its lowered type is `Value`.
    reference: &'r str,
    /// Where the `$ref` is written.
    provenance: &'r Provenance,
    /// The `E013` wording for a target still being lowered.
    recursive: &'r str,
}
