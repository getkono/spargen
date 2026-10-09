//! How a generated union decides which variant a payload is: disjoint JSON categories, required
//! keys, or trial decoding in specificity order.

use std::collections::HashSet;

use crate::ir::{
    AdditionalProps, DisjointFeature, JsonCategory, Prim, ScalarRepr, Struct, Ty, TypeId, TypeKind,
    UnionMode, UnionStrategy, UnionVariant,
};

use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Build the disjoint fast path for an undiscriminated union. Two proofs are attempted:
    ///
    /// 1. **JSON-type-disjoint**: every variant occupies a distinct JSON primitive category
    ///    (`number` and `integer` share one category, so they never separate).
    /// 2. **Required-key-disjoint**: every variant is a *closed* object (`additionalProperties:
    ///    false`) with at least one required property whose name appears in no other variant. Closed
    ///    is essential — an open object could carry another variant's unique key as an extra field
    ///    and be misrouted, so open-object required-key unions are never provably disjoint.
    pub(super) fn disjoint_strategy(&self, variants: &[UnionVariant]) -> Option<UnionStrategy> {
        // Proof 1: pairwise-distinct JSON type categories.
        let categories: Option<Vec<JsonCategory>> =
            variants.iter().map(|v| self.json_category(v.ty)).collect();
        if let Some(categories) = categories {
            let all_distinct = categories.iter().enumerate().all(|(i, cat)| {
                categories
                    .iter()
                    .enumerate()
                    .all(|(j, other)| i == j || cat != other)
            });
            if all_distinct {
                return Some(UnionStrategy::Disjoint {
                    features: categories
                        .into_iter()
                        .map(DisjointFeature::JsonType)
                        .collect(),
                });
            }
        }

        // Proof 2: object variants each carrying a unique required key.
        if let Some(keys) = self.required_key_features(variants) {
            return Some(UnionStrategy::Disjoint {
                features: keys.into_iter().map(DisjointFeature::RequiredKey).collect(),
            });
        }

        None
    }

    pub(super) fn trial_strategy(
        &self,
        variants: &[UnionVariant],
        mode: UnionMode,
    ) -> UnionStrategy {
        UnionStrategy::Trial {
            mode,
            priorities: variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, &mut HashSet::new()))
                .collect(),
        }
    }

    pub(super) fn type_specificity(&self, ty: Ty, visiting: &mut HashSet<TypeId>) -> u32 {
        if !visiting.insert(ty.id) {
            return 0;
        }
        let priority = match self.graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Enum(enumeration)) => {
                2_000_u32.saturating_sub(enumeration.variants.len() as u32)
            }
            Some(TypeKind::Null) => 3_000,
            Some(TypeKind::Never) => 4_000,
            Some(TypeKind::Struct(object)) => {
                let required = object.fields.iter().filter(|field| field.required).count() as u32;
                1_000 + required * 20 + object.fields.len() as u32
            }
            Some(TypeKind::Tuple(items)) => 900 + items.len() as u32,
            Some(TypeKind::Array(item)) => 800 + self.type_specificity(**item, visiting) / 10,
            Some(TypeKind::Primitive(Prim::I32)) => 700,
            Some(TypeKind::Primitive(Prim::I64)) => 650,
            Some(TypeKind::Primitive(Prim::Uuid | Prim::DateTime | Prim::Date)) => 600,
            Some(TypeKind::Primitive(Prim::F64 | Prim::String | Prim::Bool) | TypeKind::Bytes) => {
                500
            }
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, visiting))
                .min()
                .unwrap_or(0),
            // A reservation has no body yet, so there is nothing to rank: least specific, alongside
            // `Any` and a missing definition.
            //
            // This arm is **live**, not defensive. An earlier comment here claimed a union holding a
            // reservation was rejected before ranking, and named a function that has never existed.
            // Neither half was true: `lower_union`'s guard tests the *direct* member's id, while this
            // function recurses through `Array` and `Union`, so an array-wrapped back edge —
            // `anyOf: [{type: array, items: {$ref: self}}, …]` — reaches here on a document that
            // generates cleanly, and the value returned is emitted into the client as the trial-match
            // order of an `anyOf`. Ranking it least specific is the answer that matches what is known
            // about it, which is nothing; it is pinned by
            // `an_array_wrapped_union_back_edge_ranks_least_specific`.
            Some(TypeKind::Reserved) => 0,
            Some(TypeKind::Any) | None => 0,
        };
        visiting.remove(&ty.id);
        priority
    }

    /// The JSON primitive category a lowered variant type serializes as, or `None` when it cannot be
    /// statically categorized (an untyped `Any`, raw `Bytes`, or a nested union).
    pub(super) fn json_category(&self, ty: Ty) -> Option<JsonCategory> {
        Some(match &self.graph.get(ty.id)?.kind {
            TypeKind::Primitive(Prim::Bool) => JsonCategory::Boolean,
            TypeKind::Primitive(Prim::I32 | Prim::I64 | Prim::F64) => JsonCategory::Number,
            TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date) => {
                JsonCategory::String
            }
            TypeKind::Struct(_) => JsonCategory::Object,
            TypeKind::Array(_) | TypeKind::Tuple(_) => JsonCategory::Array,
            TypeKind::Enum(enumeration) => match enumeration.repr {
                ScalarRepr::String => JsonCategory::String,
                ScalarRepr::Int => JsonCategory::Number,
                ScalarRepr::Bool => JsonCategory::Boolean,
            },
            // A reservation cannot be categorised — its body has not been lowered, so nothing is
            // known about the JSON it serialises as. Uncategorisable, exactly like the others here.
            //
            // This arm is **live** on documents that generate cleanly. `lower_union` refuses a
            // member that is *this* union's own reservation, but not one that is another open
            // component's: `Tree: {type: array, items: {oneOf: [{$ref: Tree}, {type: string}]}}`
            // lowers the items union while `Tree` is still reserved, and both `disjoint_strategy`
            // and `discriminated_strategy` ask for the back edge's category. Guessing one (a
            // reservation is usually an object) would emit a disjoint `Deserialize` that routes the
            // back edge by `value.is_object()`, and a `Tree` — an array — would then match no
            // variant at runtime. `None` sends the union to trial matching, which decodes it.
            // Pinned by `a_union_back_edge_to_an_open_component_is_not_categorised`.
            TypeKind::Reserved
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any
            | TypeKind::Union(_) => return None,
        })
    }

    /// If every variant lowers to a *closed* object (`additionalProperties: false`) with at least
    /// one required property whose name appears in no other variant, return that unique required key
    /// per variant (source order); else `None`. Closed is required for soundness: an open object
    /// could carry another variant's unique key as an extra field, misrouting the payload.
    fn required_key_features(&self, variants: &[UnionVariant]) -> Option<Vec<String>> {
        let structs: Option<Vec<&Struct>> = variants
            .iter()
            .map(|v| match &self.graph.get(v.ty.id)?.kind {
                // Only closed objects are sound discriminators by required-key presence.
                TypeKind::Struct(structure)
                    if matches!(structure.additional, AdditionalProps::Deny) =>
                {
                    Some(structure)
                }
                // A reservation's fields are not known yet, so no required key can be proven
                // unique to it: not a sound discriminator, like any non-closed variant.
                TypeKind::Reserved => None,
                _ => None,
            })
            .collect();
        let structs = structs?;
        let mut keys = Vec::new();
        for (index, structure) in structs.iter().enumerate() {
            let others: HashSet<&str> = structs
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .flat_map(|(_, s)| s.fields.iter().map(|f| f.name.wire.as_str()))
                .collect();
            let key = structure
                .fields
                .iter()
                .find(|field| field.required && !others.contains(field.name.wire.as_str()))?;
            keys.push(key.name.wire.clone());
        }
        Some(keys)
    }
}

/// `strategy` restricted to the variants at `retained` (ascending positions into the union it
/// described), in that order.
pub(super) fn retain_strategy(strategy: &UnionStrategy, retained: &[usize]) -> UnionStrategy {
    match strategy {
        UnionStrategy::Discriminated {
            tag_field,
            tags,
            categories,
            default_variant,
            untagged,
            mode,
        } => UnionStrategy::Discriminated {
            tag_field: tag_field.clone(),
            tags: retained.iter().map(|index| tags[*index].clone()).collect(),
            categories: retained.iter().map(|index| categories[*index]).collect(),
            untagged: retained.iter().map(|index| untagged[*index]).collect(),
            mode: *mode,
            // The fallback variant's index moves with the retained set; if the fallback itself
            // was dropped, the union simply has no fallback any more.
            default_variant: default_variant
                .and_then(|target| retained.iter().position(|index| *index == target)),
        },
        UnionStrategy::Disjoint { features } => UnionStrategy::Disjoint {
            features: retained
                .iter()
                .map(|index| features[*index].clone())
                .collect(),
        },
        UnionStrategy::Trial { mode, priorities } => UnionStrategy::Trial {
            mode: *mode,
            priorities: retained.iter().map(|index| priorities[*index]).collect(),
        },
    }
}
