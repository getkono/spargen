//! Flattening an `allOf` into one type: gathering each member's contribution, inline or through
//! a `$ref`, and combining them.

use std::collections::HashSet;

use indexmap::{IndexMap, IndexSet};

use crate::diag::Provenance;
use crate::ir::{AdditionalProps, Docs, Field, FieldDefault, Struct, Ty, TypeKind};
use crate::oas31::{JsonType, Schema, SchemaOr};
use crate::source::is_remote_ref;

use super::meet::{carries_key, merge_field_default, NoMeet};
use super::nullability::{object_all_of_admits_null, stated_nullability};
use super::shape::{schema_has_shape_constraint, schema_imposes_scalar};
use super::{member_provenance, resolved_hint, resolved_identity, LowerCtx, MAX_SCHEMA_DEPTH};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Combine the gathered members of an `allOf` into its type; see [`Self::lower_all_of`].
    pub(super) fn combine_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        contributions: &[Contribution],
    ) -> Option<Ty> {
        let has_object = contributions
            .iter()
            .any(|c| matches!(c, Contribution::Object { .. }));
        let scalars: Vec<Ty> = contributions
            .iter()
            .filter_map(|c| match c {
                Contribution::Scalar(ty) => Some(*ty),
                Contribution::Object { .. } => None,
            })
            .collect();

        // Object-vs-scalar mix has no single representable type.
        if has_object && !scalars.is_empty() {
            return self.reject_all_of_object_scalar_mix(schema);
        }

        // All-scalar allOf: recursively intersect compatible members (for example integer with
        // number, an enum with its underlying scalar, or arrays whose item constraints narrow).
        if !has_object {
            let Some(mut intersection) = scalars.first().copied() else {
                // Only no-constraint members (`true`/`{}`) remained: a faithful open object.
                let ty = self.insert_schema_type(
                    schema,
                    hint,
                    TypeKind::Struct(Struct {
                        fields: Vec::new(),
                        additional: AdditionalProps::Allow,
                    }),
                );
                return Some(self.with_all_of_nullability(schema, ty));
            };
            let mark = self.graph_mark();
            for (index, member) in scalars.iter().copied().enumerate().skip(1) {
                let Ok(merged) = self.intersect_types(
                    intersection,
                    member,
                    &format!("{hint}Intersection{index}"),
                ) else {
                    return self.reject_all_of_scalars(schema);
                };
                intersection = merged;
            }
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
            return Some(self.with_all_of_nullability(schema, ty));
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
            if field.required {
                if let Some(default) = &mut field.default {
                    default.applied = None;
                }
            }
        }

        // A property repeated by three or more members is met pair by pair, and each meet replaces
        // the field's type, so the struct refers to the last meet and not to the ones before it.
        let kind = TypeKind::Struct(Struct { fields, additional });
        self.elide_meet_intermediates(mark, &kind);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        // As the all-scalar branch takes its meet's nullability: `null` satisfies the merge when
        // it satisfies every member.
        ty.nullable = object_all_of_admits_null(contributions);
        Some(self.with_all_of_nullability(schema, ty))
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
            self.gather_member(member, &format!("{hint}Member{index}"), out)?;
        }
        // The enclosing schema may carry its own object keywords beside `allOf`; fold them in last.
        if schema_is_object_like(schema) {
            let (member_fields, member_additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
            });
        }
        Some(())
    }

    pub(super) fn gather_member(
        &mut self,
        member: &SchemaOr,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        let schema = match member {
            // A `true`/`{}` member imposes no constraint.
            SchemaOr::Bool(true) => return Some(()),
            SchemaOr::Bool(false) => {
                return self.reject_all_of_false_member(member_provenance(member));
            }
            SchemaOr::Schema(schema) => schema.as_ref(),
        };
        // An inline member, or a non-component target expanded in place, is read by its keywords
        // here rather than lowered through `lower_schema_inner`, which would report this.
        self.diagnose_standalone_discriminator(schema);

        if let Some(reference) = &schema.reference {
            self.gather_ref_target(schema, reference, hint, out)?;
            // `$ref` is an applicator, not a replacement for the member that holds it: the member
            // is the target AND its own shape-bearing siblings, so those are further conjuncts of
            // this same merge, gathered exactly as a separate member carrying them would be. Every
            // arm above used to return once the target was pushed, which silently deleted the
            // siblings' properties and `required`, and let a sibling contradicting its target
            // generate as the target alone. The gate is the one `lower_schema_inner` asks of a
            // `$ref`'s siblings, so the two positions agree on what counts as a shape.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                return Some(());
            }
            return self.gather_member(
                &SchemaOr::Schema(Box::new(sibling)),
                &format!("{hint}Constraint"),
                out,
            );
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
                ty,
                decides_null,
                &schema.provenance,
                RECURSIVE_COMPONENT_MEMBER,
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
            let ty = self.ensure_remote(reference)?;
            let decides_null = self.ref_target_decides_null(reference, &schema.provenance);
            return self.push_ref_member(
                ty,
                decides_null,
                &schema.provenance,
                RECURSIVE_REMOTE_MEMBER,
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
        // test the target instead. Without this, a member that is the very schema being lowered
        // descends into its own body again and stops only at `MAX_SCHEMA_DEPTH`, reporting a
        // chain length for what is a cycle of length one. The component and remote arms above
        // refuse to read an in-progress member; this one now does too.
        if self.resolved_target_in_progress(&target.provenance) {
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
        if let Some(recorded) = self.resolved_contributions.get(&key) {
            out.extend(recorded.iter().cloned());
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
        let mut contributed = Vec::new();
        self.resolved_member_stack
            .push((key.clone(), target.reference.is_some()));
        let expanded = self.gather_resolved_target(target, &hint, &mut contributed);
        self.resolved_member_stack.pop();
        expanded?;
        self.resolved_contributions.insert(key, contributed.clone());
        out.extend(contributed);
        Some(())
    }

    /// Expand a bundle-`$ref` `allOf` member's resolved target as [`Self::gather_member`] expands
    /// any member: a target that is itself a `$ref` chains to *its* target (and gathers its own
    /// siblings), one that is an `allOf` flattens its members, and only a plain body is read for
    /// object or scalar keywords. Reading every target as a plain body took an `allOf` or alias
    /// target, which carries neither kind of keyword, for a pure annotation, and silently dropped
    /// everything it constrains (issue #306).
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
        let result = self.gather_member(&SchemaOr::Schema(Box::new(target)), hint, out);
        self.depth -= 1;
        result
    }

    /// Turn a resolved `$ref` member's already-lowered type into a contribution: an object component
    /// contributes a *copy* of its fields/`additionalProperties`; any other lowered kind is a
    /// scalar member.
    ///
    /// A member whose body is still being lowered is refused here, with `recursive` as the
    /// message, rather than by each caller: a reservation's kind says nothing about the schema's
    /// shape, and reading it as "not a struct" is exactly how a recursive member once became a
    /// silent scalar. A caller cannot forget the guard because it no longer holds it.
    ///
    /// `decides_null` is [`Self::ref_target_decides_null`] of the target: an untyped object target,
    /// or one composed of untyped object members alone, admits `null` without deciding the merge's
    /// nullability, as the same member written inline does (issues #541, #565), so its lowered
    /// non-null struct is not recorded as a decision.
    fn push_ref_member(
        &mut self,
        ty: Ty,
        decides_null: bool,
        provenance: &Provenance,
        recursive: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // Every id `is_in_progress_root` accepts is still a `Reserved` placeholder — each
        // in-progress map is entered with a fresh `reserve` and left before its `fill` — so this
        // arm is the whole guard the callers used to hold.
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Reserved) => {
                return self.reject_all_of_cycle(provenance.clone(), recursive)
            }
            Some(TypeKind::Struct(structure)) => {
                let fields = structure.fields.clone();
                let required = fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.name.wire.clone())
                    .collect();
                let additional = structure.additional.clone();
                // A copied field keeps its `undeclared` mark, so one the component carries only
                // for its own `required` still gives way to a later member's declaration.
                out.push(Contribution::Object {
                    fields,
                    additional,
                    required,
                    nullable: decides_null.then_some(ty.nullable),
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
        // branches as `lower_union` refines them; read as an object by its keywords, the union was
        // dropped with no diagnostic (issue #419).
        if schema_is_object_like(schema) && !schema_has_union(schema) {
            let (fields, additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields,
                additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
            });
        } else if schema_imposes_scalar(schema) {
            let ty = self.lower_schema(schema, hint)?;
            out.push(Contribution::Scalar(ty));
        }
        // Otherwise the member is a pure annotation (`{description: ...}`): no constraint.
        Some(())
    }
}

/// One `allOf` member's contribution to the merged type: either a set of object fields (with its
/// `additionalProperties` policy and its own `required` names) to flatten, or a scalar/leaf type.
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
    },
    Scalar(Ty),
}

/// Whether a schema constrains object shape — declared/pattern properties, an `additionalProperties`
/// policy, a `required` set, or an explicit `object` type — and so contributes fields to an `allOf`
/// merge rather than a scalar.
pub(super) fn schema_is_object_like(schema: &Schema) -> bool {
    !schema.properties.is_empty()
        || !schema.pattern_properties.is_empty()
        || schema.additional_properties.is_some()
        || !schema.required.is_empty()
        || schema.types.types.contains(&JsonType::Object)
}

/// Whether a schema carries a `oneOf` or an `anyOf` of its own.
pub(super) fn schema_has_union(schema: &Schema) -> bool {
    !schema.one_of.is_empty() || !schema.any_of.is_empty()
}

/// The index of the one `allOf` member that is an inline `oneOf`/`anyOf`, where exactly one is
/// ([`LowerCtx::lower_all_of_with_union_member`]). A member that is a `$ref` is its target first,
/// and an `allOf` with several union members is an ordinary `allOf`: its unions meet as scalar
/// members, and beside object members they are `E013`.
pub(super) fn sole_union_member(schema: &Schema) -> Option<usize> {
    let mut unions = schema.all_of.iter().enumerate().filter(|(_, member)| {
        matches!(member, SchemaOr::Schema(member) if member.reference.is_none() && schema_has_union(member))
    });
    let (index, _) = unions.next()?;
    unions.next().is_none().then_some(index)
}

/// The `required` names a schema's own `properties` do not declare, deduplicated, in source order.
/// [`LowerCtx::object_body`] carries each as a required field of its own, marked
/// [`Field::undeclared`].
pub(super) fn undeclared_required(schema: &Schema) -> Vec<String> {
    let mut seen: HashSet<&str> = schema.properties.keys().map(String::as_str).collect();
    schema
        .required
        .iter()
        .filter(|name| seen.insert(name.as_str()))
        .cloned()
        .collect()
}

/// The `E013` cycle wording for an `allOf` member that is a `$ref` to a root component still being
/// lowered: refused before the component is read, and by [`LowerCtx::push_ref_member`] for the
/// sub-file component that reaches its reservation the other way.
const RECURSIVE_COMPONENT_MEMBER: &str =
    "an `allOf` member is a direct recursive `$ref` to the component being lowered";

/// [`RECURSIVE_COMPONENT_MEMBER`] for a remote `$ref` member.
const RECURSIVE_REMOTE_MEMBER: &str =
    "an `allOf` member is a direct recursive remote `$ref` to the schema being lowered";
