//! A union's Discriminator Object: resolving its `mapping` against the union's members, the
//! standalone-discriminator warning, and the discriminated dispatch strategy.

use crate::diag::{Code, Diagnostic};
use crate::ir::{JsonCategory, TypeKind, UnionMode, UnionStrategy, UnionVariant};
use crate::oas31::discriminator::{
    discriminator_entry, discriminator_target_identity, is_schema_component_name,
};
use crate::oas31::{Schema, SchemaOr};

use super::{DiscriminatorMembers, LowerCtx};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Resolve every schema a union's Discriminator Object names — each `mapping` value, then
    /// `defaultMapping` — to the union member it denotes.
    ///
    /// A value is a component name or a URI reference. The specification recommends reading a value
    /// that could be either as a name, and a name is exactly a Components Object key, so a value
    /// made only of key characters is `#/components/schemas/<value>` and anything else is a
    /// reference, written relative to the file the discriminator sits in. The two sides are compared
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
        let entries = discriminator
            .mapping
            .iter()
            .map(|(tag, target)| (Some(tag), target))
            .chain(
                discriminator
                    .default_mapping
                    .iter()
                    .map(|target| (None, target)),
            );
        let mut resolved = DiscriminatorMembers {
            mapping: Vec::new(),
            default: None,
        };
        let mut failed = false;
        for (tag, target) in entries {
            let entry = discriminator_entry(tag);
            let value = &target.value;
            let Some(identity) = self.discriminator_target_identity(&entry, target) else {
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

    /// [`discriminator_target_identity`] against this lowering's document and resolver.
    fn discriminator_target_identity(
        &mut self,
        entry: &str,
        target: &crate::oas31::schema::DiscriminatorTarget,
    ) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
        discriminator_target_identity(self.resolver, self.diags, entry, target)
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
        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            return;
        }
        let entries = discriminator
            .mapping
            .iter()
            .map(|(tag, target)| (Some(tag), target))
            .chain(
                discriminator
                    .default_mapping
                    .iter()
                    .map(|target| (None, target)),
            );
        for (tag, target) in entries {
            self.discriminator_target_identity(&discriminator_entry(tag), target);
        }
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
    /// mapping key claims that value; the first is the tag serialization writes. Every variant
    /// starts with no `untagged` priority: the caller refuses a component variant left with no
    /// tag ([`Self::reject_unselectable_discriminated_variant`]) and decides how any other is
    /// reached.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn discriminated_strategy(
        &self,
        variants: &[UnionVariant],
        ref_names: &[Option<String>],
        variant_members: &[usize],
        tag_field: &str,
        discriminator: &DiscriminatorMembers,
        default_variant: Option<usize>,
        mode: UnionMode,
    ) -> Option<UnionStrategy> {
        let mut tags = Vec::new();
        let mut categories = Vec::new();
        for ((variant, ref_name), member) in variants.iter().zip(ref_names).zip(variant_members) {
            if !matches!(
                self.graph.get(variant.ty.id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            ) {
                let category = self.json_category(variant.ty)?;
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
                .filter(|(_, named)| named == member)
                .map(|(tag, _)| tag.clone())
                .collect();
            let component = ref_name
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
        Some(UnionStrategy::Discriminated {
            tag_field: tag_field.to_owned(),
            untagged: vec![None; tags.len()],
            tags,
            categories,
            default_variant,
            mode,
        })
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
/// Written in a sub-file, the same spelling keeps the variant name it has always had. That route
/// resolved through the resolver before same-file deep pointers did in the root, and its members
/// were named from the pointer text (`Envelope/properties/cat` → variant `EnvelopePropertiesCat`).
/// Dropping the name there would rename those variants in documents that generate today; the
/// root-only filter confines the change to what previously rejected. The name is no component
/// name, so it supplies no implicit discriminator tag ([`is_schema_component_name`]).
pub(super) fn member_component_name(member: &SchemaOr, root: crate::diag::FileId) -> Option<&str> {
    let SchemaOr::Schema(schema) = member else {
        return None;
    };
    let in_sub_file = schema.provenance.span.is_some_and(|span| span.file != root);
    schema
        .reference
        .as_deref()?
        .strip_prefix("#/components/schemas/")
        .filter(|name| in_sub_file || !name.contains('/'))
}
