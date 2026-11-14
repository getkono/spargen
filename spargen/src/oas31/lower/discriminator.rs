//! A union's Discriminator Object: resolving its `mapping` against the union's members, the
//! standalone-discriminator warning, and the discriminated dispatch strategy.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    AdditionalProps, JsonCategory, ScalarValue, Ty, TypeId, TypeKind, Union, UnionMode,
    UnionStrategy, UnionVariant, VariantDispatch,
};
use crate::name::hint_segment;
use crate::oas31::discriminator::{
    discriminator_entry, discriminator_target_identity, is_schema_component_name,
    resolve_discriminator_targets,
};
use crate::oas31::{Resolver, Schema, SchemaOr};

use super::shape::schema_has_union;
use super::strategy::{retain_strategy, UnionDispatch};
use super::union::{HeldUntypedWarning, LoweredVariant};
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Resolve every schema a union's Discriminator Object names — each `mapping` value, then
    /// `defaultMapping` — to the union member it denotes.
    ///
    /// Each value is read as [`discriminator_target_identity`] reads it. The two sides are compared
    /// by resolved `file#pointer` ([`crate::oas31::Resolver::schema_reference_identity`]), never by spelling, so
    /// `Cat`, `#/components/schemas/Cat` and `./openapi.yaml#/components/schemas/Cat` all name one
    /// member. Only `$ref` members can be named: the specification excludes inline members from
    /// name mapping.
    ///
    /// Every entry is checked, and each failure is reported at the entry itself: one naming no
    /// schema the loaded description holds is `E004`, like any other reference that cannot be
    /// followed; one naming a schema the union does not list is `E007`, because the tag it describes
    /// has no variant to decode into and the specification requires every possible schema to be
    /// listed beside the discriminator.
    pub(super) fn discriminator_members(
        &mut self,
        discriminator: &crate::oas31::Discriminator,
        members: &[&SchemaOr],
    ) -> Option<DiscriminatorMembers> {
        let member_identities: Vec<_> = members
            .iter()
            .map(|member| match member {
                SchemaOr::Schema(schema) => schema.reference.as_deref().and_then(|reference| {
                    self.resolver.schema_reference_identity(
                        reference,
                        self.resolver.written_in(&schema.provenance),
                    )
                }),
                SchemaOr::Bool(_) => None,
            })
            .collect();
        let mut resolved = DiscriminatorMembers {
            mapping: Vec::new(),
            default: None,
        };
        let mut failed = false;
        for (tag, target) in discriminator.entries() {
            let entry = discriminator_entry(tag);
            let value = &target.value;
            let Some(identity) =
                discriminator_target_identity(self.resolver, self.diags, &entry, target)
            else {
                failed = true;
                continue;
            };
            let Some(member) = member_identities
                .iter()
                .position(|member| member.as_ref() == Some(&identity))
            else {
                let consequence = match tag {
                    Some(tag) => format!("a payload tagged `{tag}` has no variant to decode into"),
                    None => "there is no branch to fall back to".to_owned(),
                };
                // E007 case: unrepresentable-applicators
                Diagnostic::error(Code::NonDisjointUnion, target.provenance.clone())
                    .message(format!(
                        "{entry} names `{value}`, which is not one of this union's `$ref` \
                         members, so {consequence}"
                    ))
                    .remedy(
                        "list the schema as a `$ref` member of the union beside the \
                         discriminator, or remove the entry",
                    )
                    .emit(self.diags);
                failed = true;
                continue;
            };
            match tag {
                Some(tag) => resolved.mapping.push((tag.clone(), member)),
                None => resolved.default = Some(member),
            }
        }
        (!failed).then_some(resolved)
    }

    /// Give a `discriminator` on a schema with no `oneOf`/`anyOf` of its own a disposition (#264).
    ///
    /// spargen dispatches by discriminator only across the members of the union it sits beside,
    /// so here it selects nothing: the schema lowers by its other keywords, and the `allOf`
    /// polymorphism form — children reaching this schema through `allOf`, decoded by the tag into
    /// whichever child it names — is not generated, since no keyword of the parent lists its
    /// children. That is `W011` at the Discriminator Object. Every `mapping` and `defaultMapping`
    /// value is still a reference to a schema, so each is resolved as a union's would be, and one
    /// naming no schema is `E004` at the entry; with no union, there is no membership to check.
    /// Called wherever a schema is lowered or gathered as an `allOf` member by its keywords, and
    /// idempotent there: a repeated report at one site is the same diagnostic, which
    /// [`Diagnostics`] keeps once.
    ///
    /// [`Diagnostics`]: crate::diag::Diagnostics
    pub(super) fn diagnose_standalone_discriminator(&mut self, schema: &Schema) {
        let Some(discriminator) = &schema.discriminator else {
            return;
        };
        if schema_has_union(schema) {
            return;
        }
        resolve_discriminator_targets(self.resolver, self.diags, discriminator);
        // W011 case: standalone-discriminator
        Diagnostic::warning(
            Code::DeclarationHasNoEffect,
            discriminator.provenance.clone(),
        )
        .message(
            "this `discriminator` has no `oneOf` or `anyOf` beside it, so it selects nothing: the \
             schema lowers by its other keywords alone, and the `allOf` polymorphism form, which \
             decodes a payload into whichever child schema its tag names, is not generated",
        )
        .remedy(
            "list every schema the tag can select in a `oneOf` beside the discriminator, or remove \
             the discriminator",
        )
        .emit(self.diags);
    }

    /// Build the discriminated fast path. A variant that can carry the tag property
    /// ([`Self::carries_tag`]: an object, an untyped member, or a union of such) routes by tag,
    /// and its own decoder reads the payload the tag selects; any other variant routes by its JSON
    /// category where no other such variant shares it, and is otherwise tried by its own schema
    /// on a payload no tag selects. A tag-carrying variant is selected by every
    /// `discriminator.mapping` key naming its member, in document order, and then by its own
    /// `$ref` component name unless a mapping key claims that value; the first is the tag
    /// serialization writes. One left with neither is selected by the one string its own
    /// required tag property admits, where no other value claims it ([`Self::pinned_tag`]). Every
    /// tag-carrying variant starts with no `untagged` priority: the caller refuses a component
    /// variant left with no tag ([`Self::reject_unselectable_discriminated_variant`]) and decides
    /// how any other is reached.
    ///
    /// A `mapping` entry naming a variant that cannot carry the tag property describes a tag no
    /// payload of that variant holds, so it selects nothing ([`Self::uncarried_mapping_entries`]).
    pub(super) fn discriminated_strategy(
        &self,
        lowered: &[LoweredVariant],
        tag_field: &str,
        discriminator: &DiscriminatorMembers,
        default_variant: Option<usize>,
        mode: UnionMode,
    ) -> UnionDispatch {
        let carries: Vec<bool> = lowered
            .iter()
            .map(|lowered| self.carries_tag(lowered.ty, &mut HashSet::new()))
            .collect();
        // The JSON category of each variant that cannot carry the tag, which selects it where no
        // other such variant shares it.
        let categories: Vec<Option<JsonCategory>> = lowered
            .iter()
            .zip(&carries)
            .map(|(lowered, carries)| (!carries).then(|| self.json_category(lowered.ty)).flatten())
            .collect();
        let mut tags = Vec::new();
        let mut dispatch_categories = Vec::new();
        let mut untagged = Vec::new();
        for (index, lowered) in lowered.iter().enumerate() {
            let (ty, member) = (lowered.ty, lowered.member());
            if !carries[index] {
                let category = categories[index].filter(|category| {
                    categories
                        .iter()
                        .filter(|other| **other == Some(*category))
                        .count()
                        == 1
                });
                tags.push(Vec::new());
                dispatch_categories.push(category);
                untagged.push(
                    category
                        .is_none()
                        .then(|| self.type_specificity(ty, &mut HashSet::new())),
                );
                continue;
            }
            // Every explicit mapping entry naming this variant's member — already resolved by
            // identity, so its spelling does not matter — selects it. So does its component name,
            // because the specification reads a value as a component name "unless a `mapping` is
            // present for that value": a key equal to it claims it, for this member or another.
            let mut accepted: Vec<String> = discriminator
                .mapping
                .iter()
                .filter(|(_, named)| *named == member)
                .map(|(tag, _)| tag.clone())
                .collect();
            let component = lowered
                .ref_name
                .as_deref()
                .filter(|name| is_schema_component_name(name));
            // A member that is no component — inline, or a deeper pointer — has no implicit value
            // ("inline `oneOf` or `anyOf` subschemas are not considered"), so only a mapping key
            // selects it. With none it keeps no tag at all: anything else would be one spargen
            // made up and no server sends. The caller decides how such a member is reached.
            if let Some(name) = component {
                if !discriminator.mapping.iter().any(|(tag, _)| tag == name) {
                    accepted.push(name.to_owned());
                }
            }
            tags.push(accepted);
            dispatch_categories.push(None);
            untagged.push(None);
        }
        // An object member left with no tag — no component name, no mapping key — may still pin
        // the tag property to exactly one string itself (#1001): a required, non-nullable `const`
        // or one-value `enum`, as an internally tagged Rust enum's OpenAPI output writes every
        // branch. Every payload that member accepts carries that value, so dispatching on it
        // selects exactly what the member's own schema would, and serializing it writes the value
        // the member's field already holds. It is no implicit value the specification assigns,
        // so a value something else claims — a `mapping` key, another member's tag, or another
        // untagged member whose tag property admits the same string (a second pin of it, a plain
        // `type: string`, or no declaration of the property at all), or the 3.2 `defaultMapping`
        // member when its tag property admits it — keeps none of them: the member stays untagged
        // and is reached as one with no such property is. The last two claims matter because an
        // untagged member is tried by its schema, and the `defaultMapping` member decodes the
        // payload, when the tag selects no tagged member, so tagging the pin would send a payload
        // only that other member accepts to the pinned one instead.
        let untagged_objects: Vec<bool> = tags
            .iter()
            .zip(&carries)
            .map(|(accepted, carries)| *carries && accepted.is_empty())
            .collect();
        let variants: Vec<Ty> = lowered.iter().map(|lowered| lowered.ty).collect();
        let pinned: Vec<Option<String>> = variants
            .iter()
            .zip(&untagged_objects)
            .map(|(variant, untagged_object)| {
                untagged_object
                    .then(|| self.pinned_tag(*variant, tag_field))
                    .flatten()
            })
            .collect();
        for (index, value) in pinned.iter().enumerate() {
            let Some(value) = value else {
                continue;
            };
            let claimed = discriminator.mapping.iter().any(|(tag, _)| tag == value)
                || tags.iter().flatten().any(|tag| tag == value)
                || variants.iter().zip(&untagged_objects).enumerate().any(
                    |(other, (variant, untagged_object))| {
                        other != index
                            && *untagged_object
                            && self.admits_tag(*variant, tag_field, value)
                    },
                )
                || default_variant.is_some_and(|fallback| {
                    fallback != index
                        && carries[fallback]
                        && self.admits_tag(variants[fallback], tag_field, value)
                });
            if !claimed {
                tags[index].push(value.clone());
            }
        }
        UnionDispatch {
            strategy: UnionStrategy::Discriminated {
                tag_field: tag_field.to_owned(),
                default_variant,
                mode,
                // `open_unions` opens a union whose unlisted tags have nowhere else to go: a
                // `defaultMapping` variant already takes them.
                open: self.open_unions && default_variant.is_none(),
            },
            variants: tags
                .into_iter()
                .zip(dispatch_categories)
                .zip(untagged)
                .map(
                    |((tags, category), untagged)| VariantDispatch::Discriminated {
                        tags,
                        category,
                        untagged,
                    },
                )
                .collect(),
        }
    }

    /// Whether every payload of `ty` is an object that can hold the discriminator property, so a
    /// tag can select it: a struct, an untyped member (whose objects are among its values), a map
    /// or other object-category type, or a union every variant of which can. A union member is
    /// then decoded by its own `Deserialize`, which applies its own dispatch to the payload the
    /// outer tag selected. `visiting` guards a union cycle, which finds nothing against it.
    fn carries_tag(&self, ty: Ty, visiting: &mut HashSet<TypeId>) -> bool {
        if !visiting.insert(ty.id) {
            return true;
        }
        match self.kind_of(ty.id) {
            Some(TypeKind::Reserved) | None => false,
            Some(TypeKind::Struct(_) | TypeKind::Any) => true,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .all(|variant| self.carries_tag(variant.ty, visiting)),
            Some(_) => self.json_category(ty) == Some(JsonCategory::Object),
        }
    }

    /// Whether `ty` is, or is a union reaching through its variants, a reservation still being
    /// lowered: nothing is known yet of what its payloads are, so neither whether it can carry a
    /// tag nor its JSON category can be decided until lowering has filled it.
    pub(super) fn awaits_reservation(&self, ty: Ty, visiting: &mut HashSet<TypeId>) -> bool {
        if !visiting.insert(ty.id) {
            return false;
        }
        match self.kind_of(ty.id) {
            Some(TypeKind::Reserved) => true,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .any(|variant| self.awaits_reservation(variant.ty, visiting)),
            _ => false,
        }
    }

    /// The `mapping` entries naming a variant that cannot carry the tag property
    /// ([`Self::carries_tag`]), in document order: no payload of that variant holds a tag, so the
    /// entry selects nothing, and the variant is reached by its JSON category or its own schema
    /// ([`Self::discriminated_strategy`]).
    pub(super) fn uncarried_mapping_entries(
        &self,
        lowered: &[LoweredVariant],
        discriminator: &DiscriminatorMembers,
    ) -> Vec<UncarriedTag> {
        discriminator
            .mapping
            .iter()
            .filter(|(_, member)| {
                lowered.iter().any(|lowered| {
                    lowered.members.contains(member)
                        && !self.carries_tag(lowered.ty, &mut HashSet::new())
                })
            })
            .map(|(tag, member)| UncarriedTag {
                tag: tag.clone(),
                member: *member,
            })
            .collect()
    }

    /// Report a `mapping` entry naming a member that cannot carry the tag property
    /// ([`UncarriedTag`]) as `W011` at the entry: the member is still generated and decoded, by
    /// its JSON category or its own schema, and only the entry has no effect.
    pub(super) fn warn_uncarried_tag(
        &mut self,
        schema: &Schema,
        discriminator: &crate::oas31::Discriminator,
        uncarried: &UncarriedTag,
    ) {
        let UncarriedTag { tag, member } = uncarried;
        let (provenance, value) = match discriminator.mapping.get(tag) {
            Some(target) => (target.provenance.clone(), target.value.as_str()),
            None => (schema.provenance.clone(), tag.as_str()),
        };
        let property = &discriminator.property_name;
        // W011 case: uncarried-mapping-entry
        Diagnostic::warning(Code::DeclarationHasNoEffect, provenance)
            .message(format!(
                "`discriminator.mapping` entry `{tag}` names `{value}`, union member {member}, \
                 which cannot carry the discriminator property `{property}`: not every payload it \
                 accepts is an object, so no payload of it holds a tag and the entry selects \
                 nothing; the member is decoded by its JSON category, or by its own schema"
            ))
            .remedy(
                "remove the entry, or make the member an object schema that declares the \
                 discriminator property",
            )
            .emit(self.diags);
    }

    /// The one string an object variant's own `tag_field` admits: the field is required, not
    /// nullable, and its type is a closed string set of exactly one value (a `const`, or an `enum`
    /// of one). `None` for any other variant, including one whose field admits several values,
    /// whose tag the dispatch could not write back without overwriting the one the payload held.
    fn pinned_tag(&self, variant: Ty, tag_field: &str) -> Option<String> {
        let Some(TypeKind::Struct(object)) = self.kind_of(variant.id) else {
            return None;
        };
        let field = object
            .fields
            .iter()
            .find(|field| field.name.wire == tag_field)?;
        if !field.required || field.ty.nullable {
            return None;
        }
        let Some(TypeKind::Enum(set)) = self.kind_of(field.ty.id) else {
            return None;
        };
        match set.variants.as_slice() {
            [ScalarValue::String(value)] if !set.is_open() => Some(value.clone()),
            _ => None,
        }
    }

    /// Name each inline variant its pinned tag selects from that tag, and leave the tag property
    /// out of its struct (#1232).
    ///
    /// A variant qualifies when its member is an inline schema (no `$ref`), its one tag is the
    /// value its own required tag property pins ([`Self::pinned_tag`]), and its payload is a def
    /// that member's lowering or sibling meet inserted under the member's numbered hint
    /// (`{hint}Variant{N}`). Every def that lowering inserted under that hint, outside the body
    /// of a reservation it opened ([`Self::reserved_types`]), is renamed onto `{hint}{Tag}` (the
    /// variant's struct, its nested inline types, and a `…Constrained` meet alike): a component,
    /// remote or bundle target first lowered there keeps its name, and so do its own inline
    /// types, though their names start with that hint (#1245). The variant is named from
    /// the tag itself. A tag that does not start with an ASCII letter, or that another variant's
    /// hint already spells, keeps the numbered name.
    ///
    /// The tag property is recorded for [`Self::drop_pinned_tag_fields`], which drops it once
    /// lowering is done from a struct that ignores unknown properties: it admits one
    /// value, which the union's `Serialize` writes for the variant whenever the struct did not,
    /// and its `Deserialize` hands the whole object, tag included, to the struct, which ignores
    /// the key. A struct that denies unknown properties, or collects them into a typed map, keeps
    /// the field, since the tag would otherwise fail or fill that map.
    pub(super) fn name_pinned_variants(
        &mut self,
        hint: &str,
        tag_field: &str,
        lowered: &mut [LoweredVariant],
        dispatches: &[VariantDispatch],
        members: &PinnedMembers<'_, '_>,
    ) {
        for index in 0..lowered.len() {
            let variant = &lowered[index];
            let Some(VariantDispatch::Discriminated {
                tags,
                category: None,
                ..
            }) = dispatches.get(index)
            else {
                continue;
            };
            let [tag] = tags.as_slice() else {
                continue;
            };
            let member = variant.member();
            let inline = matches!(
                members.real.get(member),
                Some(SchemaOr::Schema(schema)) if schema.reference.is_none()
            );
            if !inline
                || variant.ref_name.is_some()
                || variant.members.len() != 1
                || self.pinned_tag(variant.ty, tag_field).as_deref() != Some(tag.as_str())
                || !tag
                    .chars()
                    .find(char::is_ascii_alphanumeric)
                    .is_some_and(|first| first.is_ascii_alphabetic())
                || lowered.iter().any(|other| other.name_hint == *tag)
            {
                continue;
            }
            let numbered = format!("{hint}Variant{member}");
            let fresh: Vec<TypeId> = members
                .fresh
                .get(member)
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    self.graph
                        .get(*id)
                        .is_some_and(|def| def.name_hint.starts_with(&numbered))
                })
                .collect();
            if !fresh.contains(&variant.ty.id) {
                continue;
            }
            let tagged = format!("{hint}{}", hint_segment(tag));
            let renamed = |name: &mut String| {
                if let Some(rest) = name.strip_prefix(&numbered) {
                    *name = format!("{tagged}{rest}");
                }
            };
            for id in fresh {
                if let Some(def) = self.graph.get_mut(id) {
                    renamed(&mut def.name_hint);
                    // A nested inline union's own variants carry the same prefix.
                    if let TypeKind::Union(union) = &mut def.kind {
                        for nested in &mut union.variants {
                            renamed(&mut nested.name_hint);
                        }
                    }
                }
            }
            self.pinned_tag_fields
                .push((variant.ty.id, tag_field.to_owned()));
            lowered[index].name_hint = tag.clone();
        }
    }

    /// Leave each tag property [`Self::name_pinned_variants`] recorded out of its struct, where
    /// the struct ignores unknown properties. Run once lowering is done: until then the field is
    /// the pin a meet reads, so a tag constraint met with the union later (`allOf: [{$ref:
    /// Union}, {properties: {kind: {const: other}}}]`) still excludes the branch it rules out.
    pub(super) fn drop_pinned_tag_fields(&mut self) {
        for (id, tag_field) in std::mem::take(&mut self.pinned_tag_fields) {
            if let Some(TypeKind::Struct(object)) = self.graph.get_mut(id).map(|def| &mut def.kind)
            {
                if matches!(object.additional, AdditionalProps::Allow) {
                    object.fields.retain(|field| field.name.wire != tag_field);
                }
            }
        }
    }

    /// The ids each real member's own lowering (`member_lowerings`) and its sibling meet
    /// (`meet_inserts`) inserted, indexed by the member's position, leaving out those inside the
    /// body of a reservation that lowering opened ([`Self::reserved_types`]): a component first
    /// lowered there keeps its own name.
    pub(super) fn fresh_member_ids(
        &self,
        member_lowerings: &[std::ops::Range<u32>],
        meet_inserts: &[std::ops::Range<u32>],
    ) -> Vec<Vec<TypeId>> {
        (0..member_lowerings.len().max(meet_inserts.len()))
            .map(|member| {
                [member_lowerings.get(member), meet_inserts.get(member)]
                    .into_iter()
                    .flatten()
                    .flat_map(Clone::clone)
                    .map(TypeId)
                    .filter(|id| !self.reserved_types.contains(id))
                    .collect()
            })
            .collect()
    }

    /// Decide the dispatch of every discriminated union [`Self::union_strategy`] deferred because
    /// a variant reached an open reservation. Run once every reservation is filled, so the
    /// declaration order of the components decides nothing: the union gets the dispatch, the
    /// pinned variant names and the reports it would have had were each variant lowered before
    /// it. Every union definition carrying the union's [`VariantDispatch::Deferred`] markers takes
    /// the decision over the variants it kept: the union itself, and each copy a meet, a collapse
    /// or a re-emit made of it before the decision, exactly as a copy made after it would have
    /// kept the decided dispatch. A refusal is reported here, and the document is rejected.
    pub(super) fn decide_deferred_discriminators(&mut self) {
        for deferred in std::mem::take(&mut self.deferred_discriminators) {
            let DeferredDiscriminator {
                id: deferred_id,
                schema,
                hint,
                mode,
                mut lowered,
                discriminator,
                real,
                fresh,
                reports_untyped,
            } = deferred;
            // Each definition whose every variant is a marker of this union, with the position
            // in the union each of its variants stands for.
            let targets: Vec<(TypeId, Vec<usize>)> = self
                .graph
                .iter()
                .filter_map(|(id, def)| {
                    let TypeKind::Union(union) = &def.kind else {
                        return None;
                    };
                    let members: Option<Vec<usize>> = union
                        .variants
                        .iter()
                        .map(|variant| match variant.dispatch {
                            VariantDispatch::Deferred { union, member } if union == deferred_id => {
                                Some(member)
                            }
                            _ => None,
                        })
                        .collect();
                    members
                        .filter(|members| !members.is_empty())
                        .map(|members| (id, members))
                })
                .collect();
            if targets.is_empty() {
                continue;
            }
            let Some((dispatch, _)) =
                self.union_strategy(&schema, &lowered, mode, Some(&discriminator), false)
            else {
                continue;
            };
            let UnionDispatch {
                strategy,
                variants: dispatches,
            } = dispatch;
            if let UnionStrategy::Discriminated { tag_field, .. } = &strategy {
                let real: Vec<&SchemaOr> = real.iter().collect();
                self.name_pinned_variants(
                    &hint,
                    tag_field,
                    &mut lowered,
                    &dispatches,
                    &PinnedMembers {
                        real: &real,
                        fresh: &fresh,
                    },
                );
            }
            for (id, members) in targets {
                if let Some(TypeKind::Union(union)) =
                    self.graph.get_mut(id).map(|def| &mut def.kind)
                {
                    union.strategy = retain_strategy(&strategy, &members);
                    for (variant, &member) in union.variants.iter_mut().zip(&members) {
                        variant.name_hint.clone_from(&lowered[member].name_hint);
                        variant.dispatch = dispatches[member].clone();
                    }
                }
            }
            if reports_untyped {
                let labels: Vec<usize> = lowered.iter().map(LoweredVariant::member).collect();
                let union = Union {
                    variants: lowered
                        .iter()
                        .zip(dispatches)
                        .map(|(variant, dispatch)| UnionVariant {
                            name_hint: variant.name_hint.clone(),
                            ty: variant.ty,
                            dispatch,
                        })
                        .collect(),
                    strategy,
                };
                let untyped = self.untyped_one_of_variants(&union, &labels);
                self.report_untyped_one_of(HeldUntypedWarning {
                    provenance: schema.provenance.clone(),
                    untyped,
                });
            }
        }
    }

    /// Whether an object variant may accept a payload whose `tag_field` holds `value`, as far as
    /// that one property decides it. `false` only where the property is declared with a closed
    /// enum set that omits `value`, or is undeclared on an object that denies unknown properties;
    /// every other declaration (a plain `string`, an open set, a `$ref` to anything else) is
    /// taken to admit it, so a doubt keeps the pinned value claimed rather than tagged.
    fn admits_tag(&self, variant: Ty, tag_field: &str, value: &str) -> bool {
        let Some(TypeKind::Struct(object)) = self.kind_of(variant.id) else {
            return true;
        };
        let Some(field) = object
            .fields
            .iter()
            .find(|field| field.name.wire == tag_field)
        else {
            return !matches!(object.additional, AdditionalProps::Deny);
        };
        if let Some(TypeKind::Enum(set)) = self.kind_of(field.ty.id) {
            set.is_open()
                || set
                    .variants
                    .iter()
                    .any(|listed| matches!(listed, ScalarValue::String(text) if text == value))
        } else {
            true
        }
    }

    /// A discriminated object member no discriminator value selects: no `mapping` key names it, and
    /// a key equal to its component name claims that value for another member, so the dispatch has
    /// no arm that decodes into it, and the tag it would serialize decodes as that other member.
    /// Reported at the entry that claims the name. A member `defaultMapping` names is still reached
    /// by the fallback and never comes here.
    pub(super) fn reject_unselectable_discriminated_variant<T>(
        &mut self,
        schema: &Schema,
        discriminator: &crate::oas31::Discriminator,
        member: usize,
        implicit: &str,
    ) -> Option<T> {
        let claim = discriminator.mapping.get_key_value(implicit);
        let (provenance, message) = match claim {
            Some((tag, target)) => (
                target.provenance.clone(),
                format!(
                    "`discriminator.mapping` entry `{tag}` claims the component name of union \
                     member {member} for `{}`, and no entry names member {member}, so no \
                     discriminator value selects it",
                    target.value
                ),
            ),
            // The caller passes only a component member left with no tag, and such a member's
            // name is claimed by a `mapping` key, so this arm is not reached; it stays as the
            // refusal for a caller that passes another member.
            None => (
                schema.provenance.clone(),
                format!("no discriminator value selects union member {member}"),
            ),
        };
        // E007 case: unrepresentable-applicators
        Diagnostic::error(Code::NonDisjointUnion, provenance)
            .message(message)
            .remedy(
                "add a `discriminator.mapping` entry naming the member, rename the entry that \
                 claims its component name, or remove the member from the union",
            )
            .emit(self.diags);
        None
    }

    /// Discriminated object members that are no schema component — inline, or a pointer into
    /// another schema — and that no `mapping` entry or `defaultMapping` names. The specification
    /// gives them no implicit value ("inline `oneOf` or `anyOf` subschemas are not considered"),
    /// so no discriminator value selects them. Where another variant carries a tag (`dispatches`),
    /// the caller keeps the tag dispatch for those and tries these by their schemas when the tag
    /// is absent or names no tagged member; otherwise the discriminator dispatches nothing and the
    /// union is decoded by its members' schemas, as one with no discriminator. Either way no tag
    /// is invented for them; this says so at the discriminator.
    pub(super) fn warn_untagged_discriminated_members(
        &mut self,
        discriminator: &crate::oas31::Discriminator,
        members: &[usize],
        dispatches: bool,
    ) {
        let list = members
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let (subject, pronoun) = if members.len() == 1 {
            (
                format!(
                    "union member {list} is no schema component — inline, or a pointer into \
                     another schema — so it has no implicit discriminator value"
                ),
                "it",
            )
        } else {
            (
                format!(
                    "union members {list} are no schema components — inline, or pointers into \
                     another schema — so they have no implicit discriminator value"
                ),
                "them",
            )
        };
        let consequence = if dispatches {
            let (verb, possessive) = if members.len() == 1 {
                ("it is", "its")
            } else {
                ("they are", "their")
            };
            format!(
                "so the tag dispatches only to the members a value names, and {verb} matched by \
                 {possessive} own schema when the tag is absent or names no such member"
            )
        } else {
            "so this `discriminator` dispatches nothing and the union is decoded by its members' \
             schemas"
                .to_owned()
        };
        // W011 case: untagged-discriminated-member
        Diagnostic::warning(
            Code::DeclarationHasNoEffect,
            discriminator.provenance.clone(),
        )
        .message(format!(
            "{subject}, and no `discriminator.mapping` entry names {pronoun}: no discriminator \
             value selects {pronoun}, {consequence}"
        ))
        .remedy(
            "add a `discriminator.mapping` entry naming each such member (a URI reference to it \
             works), move it to a schema component of its own, or remove the discriminator",
        )
        .emit(self.diags);
    }
}

/// The component name a union member is written as — `$ref: '#/components/schemas/<name>'` — or
/// `None`. It names the member's variant and implicit discriminator tag; an explicit
/// `discriminator.mapping` value is matched by resolved target instead
/// ([`LowerCtx::discriminator_members`]). Written in the root document, a name with a
/// raw `/` is a pointer *into* a component, not a component name: it has none to derive a variant
/// or a tag from, exactly as the same pointer written against a relative file has none, and neither
/// has any file reference.
///
/// Written in a sub-file, the same spelling names its variant from the pointer text
/// (`Envelope/properties/cat` → variant `EnvelopePropertiesCat`), so the root-only filter keeps
/// those variant names stable. The name is no component name, so it supplies no implicit
/// discriminator tag ([`is_schema_component_name`]).
pub(super) fn member_component_name<'m>(
    member: &'m SchemaOr,
    resolver: &Resolver<'_>,
) -> Option<&'m str> {
    let SchemaOr::Schema(schema) = member else {
        return None;
    };
    let in_sub_file = resolver.written_in(&schema.provenance) != resolver.root_id();
    schema
        .reference
        .as_deref()?
        .strip_prefix("#/components/schemas/")
        .filter(|name| in_sub_file || !name.contains('/'))
}

/// A union's Discriminator Object resolved against the union's own members: every `mapping`
/// entry's tag with the index (into the union's real, non-null members) of the member it names, in
/// document order, and the member `defaultMapping` names. Built by
/// [`LowerCtx::discriminator_members`], which rejects any entry naming a schema that does not
/// exist or is not a member, so every index here is a member.
pub(super) struct DiscriminatorMembers {
    pub(super) mapping: Vec<(String, usize)>,
    pub(super) default: Option<usize>,
}

/// What [`LowerCtx::name_pinned_variants`] reads of a union's real members, each indexed by the
/// member's position: the member itself, and the ids its own lowering and its sibling meet
/// inserted outside the body of a reservation they opened ([`LowerCtx::fresh_member_ids`]).
pub(super) struct PinnedMembers<'m, 's> {
    pub(super) real: &'m [&'s SchemaOr],
    pub(super) fresh: &'m [Vec<TypeId>],
}

/// A `mapping` entry naming a union member that cannot carry the discriminator property
/// ([`LowerCtx::discriminated_strategy`]): the entry's tag, and the member it names.
pub(super) struct UncarriedTag {
    pub(super) tag: String,
    pub(super) member: usize,
}

/// A discriminated union whose dispatch waits for lowering to fill a reservation one of its
/// variants reaches ([`LowerCtx::awaits_reservation`]): everything [`LowerCtx::union_strategy`]
/// and [`LowerCtx::name_pinned_variants`] read, held until
/// [`LowerCtx::decide_deferred_discriminators`] decides it. Until then the union, and every copy
/// a meet, a collapse or a re-emit makes of it, carries [`VariantDispatch::Deferred`] markers,
/// which no strategy admits, so a copy the decision missed is an `E011` invariant failure rather
/// than an emitted dispatch.
pub(super) struct DeferredDiscriminator {
    /// The number [`LowerCtx::union_strategy`] gave the union, which its variants' markers carry.
    pub(super) id: usize,
    /// The union schema, whose discriminator is decided and at whose provenance it reports.
    pub(super) schema: Schema,
    pub(super) hint: String,
    pub(super) mode: UnionMode,
    pub(super) lowered: Vec<LoweredVariant>,
    pub(super) discriminator: DiscriminatorMembers,
    /// The union's real members, as [`PinnedMembers::real`] reads them.
    pub(super) real: Vec<SchemaOr>,
    /// [`PinnedMembers::fresh`], as it was when the union was lowered.
    pub(super) fresh: Vec<Vec<TypeId>>,
    /// Whether the union reports its untyped `oneOf` variants itself
    /// ([`LowerCtx::report_untyped_one_of`]).
    pub(super) reports_untyped: bool,
}
