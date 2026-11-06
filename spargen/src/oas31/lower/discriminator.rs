//! A union's Discriminator Object: resolving its `mapping` against the union's members, the
//! standalone-discriminator warning, and the discriminated dispatch strategy.

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    AdditionalProps, JsonCategory, ScalarValue, Ty, TypeId, TypeKind, UnionMode, UnionStrategy,
    VariantDispatch,
};
use crate::name::hint_segment;
use crate::oas31::discriminator::{
    discriminator_entry, discriminator_target_identity, is_schema_component_name,
    resolve_discriminator_targets,
};
use crate::oas31::{Resolver, Schema, SchemaOr};

use super::shape::schema_has_union;
use super::strategy::UnionDispatch;
use super::union::LoweredVariant;
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

    /// Build the discriminated fast path. Objects route by tag; a non-object variant routes by its
    /// unique JSON category. An object variant is selected by every `discriminator.mapping` key
    /// naming its member, in document order, and then by its own `$ref` component name unless a
    /// mapping key claims that value; the first is the tag serialization writes. One left with
    /// neither is selected by the one string its own required tag property admits, where no other
    /// value claims it ([`Self::pinned_tag`]). Every variant
    /// starts with no `untagged` priority: the caller refuses a component variant left with no
    /// tag ([`Self::reject_unselectable_discriminated_variant`]) and decides how any other is
    /// reached.
    pub(super) fn discriminated_strategy(
        &self,
        lowered: &[LoweredVariant],
        tag_field: &str,
        discriminator: &DiscriminatorMembers,
        default_variant: Option<usize>,
        mode: UnionMode,
    ) -> Option<UnionDispatch> {
        let mut tags = Vec::new();
        let mut categories = Vec::new();
        for lowered in lowered {
            let (ty, member) = (lowered.ty, lowered.member());
            if !matches!(self.kind_of(ty.id), Some(TypeKind::Struct(_))) {
                let category = self.json_category(ty)?;
                if category == JsonCategory::Object || categories.contains(&Some(category)) {
                    return None;
                }
                tags.push(Vec::new());
                categories.push(Some(category));
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
            categories.push(None);
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
            .zip(&categories)
            .map(|(accepted, category)| category.is_none() && accepted.is_empty())
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
                        && categories[fallback].is_none()
                        && self.admits_tag(variants[fallback], tag_field, value)
                });
            if !claimed {
                tags[index].push(value.clone());
            }
        }
        Some(UnionDispatch {
            strategy: UnionStrategy::Discriminated {
                tag_field: tag_field.to_owned(),
                default_variant,
                mode,
            },
            variants: tags
                .into_iter()
                .zip(categories)
                .map(|(tags, category)| VariantDispatch::Discriminated {
                    tags,
                    category,
                    untagged: None,
                })
                .collect(),
        })
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
    /// (`{hint}Variant{N}`), so no component or other shared type is touched. Every def that
    /// lowering inserted under that hint is renamed onto `{hint}{Tag}` (the variant's struct,
    /// its nested inline types, and a `…Constrained` meet alike), and the variant is named from
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
            let fresh: Vec<TypeId> = [
                members.member_lowerings.get(member),
                members.meet_inserts.get(member),
            ]
            .into_iter()
            .flatten()
            .flat_map(Clone::clone)
            .map(TypeId)
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
/// inserted.
pub(super) struct PinnedMembers<'m, 's> {
    pub(super) real: &'m [&'s SchemaOr],
    pub(super) member_lowerings: &'m [std::ops::Range<u32>],
    pub(super) meet_inserts: &'m [std::ops::Range<u32>],
}
