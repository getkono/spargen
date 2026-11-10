//! How a generated union decides which variant a payload is: disjoint JSON categories, required
//! keys, or trial decoding in specificity order.

use std::collections::HashSet;

use crate::ir::{
    AdditionalProps, DisjointFeature, JsonCategory, Prim, Struct, Ty, TypeId, TypeKind, UnionMode,
    UnionStrategy, VariantDispatch,
};

use super::meet::value_category;
use super::LowerCtx;

/// How the union being lowered tells its variants apart: the union-level strategy, and what
/// selects each variant, in variant order, which lowering moves onto each
/// [`crate::ir::UnionVariant`] as it assembles them.
pub(super) struct UnionDispatch {
    /// The union-level strategy.
    pub(super) strategy: UnionStrategy,
    /// What selects each variant, of `strategy`'s arm, one per variant in order.
    pub(super) variants: Vec<VariantDispatch>,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Build the disjoint fast path for an undiscriminated union. Two proofs are attempted:
    ///
    /// 1. **JSON-type-disjoint**: every variant occupies a distinct JSON primitive category
    ///    (`number` and `integer` share one category, so they never separate).
    /// 2. **Required-key-disjoint**: every variant is a *closed* object (`additionalProperties:
    ///    false`) with at least one required property whose name appears in no other variant. Closed
    ///    is essential — an open object could carry another variant's unique key as an extra field
    ///    and be misrouted, so open-object required-key unions are never provably disjoint.
    ///
    /// `variants` are the variants' types, in order.
    pub(super) fn disjoint_strategy(&self, variants: &[Ty]) -> Option<UnionDispatch> {
        // Proof 1: pairwise-distinct JSON type categories.
        let categories: Option<Vec<JsonCategory>> =
            variants.iter().map(|ty| self.json_category(*ty)).collect();
        if let Some(categories) = categories {
            let all_distinct = categories.iter().enumerate().all(|(i, cat)| {
                categories
                    .iter()
                    .enumerate()
                    .all(|(j, other)| i == j || cat != other)
            });
            if all_distinct {
                return Some(UnionDispatch {
                    strategy: UnionStrategy::Disjoint,
                    variants: categories
                        .into_iter()
                        .map(|category| {
                            VariantDispatch::Disjoint(DisjointFeature::JsonType(category))
                        })
                        .collect(),
                });
            }
        }

        // Proof 2: object variants each carrying a unique required key.
        if let Some(keys) = self.required_key_features(variants) {
            return Some(UnionDispatch {
                strategy: UnionStrategy::Disjoint,
                variants: keys
                    .into_iter()
                    .map(|key| VariantDispatch::Disjoint(DisjointFeature::RequiredKey(key)))
                    .collect(),
            });
        }

        None
    }

    /// Trial matching under `mode` for the variants of types `variants`, each ranked by its
    /// static specificity.
    pub(super) fn trial_strategy(&self, variants: &[Ty], mode: UnionMode) -> UnionDispatch {
        UnionDispatch {
            strategy: UnionStrategy::Trial { mode },
            variants: variants
                .iter()
                .map(|ty| VariantDispatch::Trial {
                    priority: self.type_specificity(*ty, &mut HashSet::new()),
                })
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
            // Integers rank by the values they admit, fewest first; at one width the unsigned type
            // ranks above the signed one.
            Some(TypeKind::Primitive(Prim::U8)) => 780,
            Some(TypeKind::Primitive(Prim::U16)) => 740,
            Some(TypeKind::Primitive(Prim::U32)) => 710,
            Some(TypeKind::Primitive(Prim::I32)) => 700,
            Some(TypeKind::Primitive(Prim::U64)) => 660,
            Some(TypeKind::Primitive(Prim::I64)) => 650,
            Some(TypeKind::Primitive(Prim::Uuid | Prim::DateTime | Prim::Date)) => 600,
            Some(
                TypeKind::Primitive(Prim::F64 | Prim::String | Prim::Bool) | TypeKind::Bytes(_),
            ) => 500,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, visiting))
                .min()
                .unwrap_or(0),
            // A reservation has no body yet, so there is nothing to rank: least specific, alongside
            // `Any` and a missing definition.
            //
            // This arm is live, not defensive: `lower_union`'s guard tests the direct member's id,
            // while this function recurses through `Array` and `Union`, so an array-wrapped back
            // edge — `anyOf: [{type: array, items: {$ref: self}}, …]` — reaches here on a document
            // that generates cleanly, and the value returned is emitted into the client as the
            // trial-match order of an `anyOf`. Ranking it least specific matches what is known
            // about it, which is nothing; `an_array_wrapped_union_back_edge_ranks_least_specific`
            // pins it.
            Some(TypeKind::Reserved) => 0,
            Some(TypeKind::Any) | None => 0,
        };
        visiting.remove(&ty.id);
        priority
    }

    /// The JSON primitive category a lowered variant type serializes as, or `None` when it cannot be
    /// statically categorized: an untyped `Any`, raw `Bytes`, a nested union, the exact JSON
    /// `null`, an uninhabited `Never`, a reservation, or a `TypeId` with no definition.
    ///
    /// This is [`value_category`] but for raw bytes, which a schema's instance holds as a (base64)
    /// string but a union's dispatch does not categorise.
    ///
    /// [`value_category`]: super::meet::value_category
    pub(super) fn json_category(&self, ty: Ty) -> Option<JsonCategory> {
        match &self.graph.get(ty.id)?.kind {
            // A reservation cannot be categorised — its body has not been lowered, so nothing is
            // known about the JSON it serialises as. `value_category` answers the same; the arm is
            // stated here because the answer matters here.
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
            TypeKind::Reserved | TypeKind::Bytes(_) => None,
            kind => value_category(kind),
        }
    }

    /// If every variant lowers to a *closed* object (`additionalProperties: false`) with at least
    /// one required property whose name appears in no other variant, return that unique required key
    /// per variant (source order); else `None`. Closed is required for soundness: an open object
    /// could carry another variant's unique key as an extra field, misrouting the payload.
    fn required_key_features(&self, variants: &[Ty]) -> Option<Vec<String>> {
        let structs: Option<Vec<&Struct>> = variants
            .iter()
            .map(|ty| match &self.graph.get(ty.id)?.kind {
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
/// described), in that order. Each retained variant keeps its own dispatch; only the
/// `defaultMapping` fallback's position moves.
pub(super) fn retain_strategy(strategy: &UnionStrategy, retained: &[usize]) -> UnionStrategy {
    match strategy {
        UnionStrategy::Discriminated {
            tag_field,
            default_variant,
            mode,
            open,
        } => UnionStrategy::Discriminated {
            tag_field: tag_field.clone(),
            mode: *mode,
            // An open union stays open: a dropped variant's tags become unlisted and reach
            // `Unknown` like any other. A closed one whose fallback was dropped stays closed, as
            // `discriminated_strategy` left it.
            open: *open,
            // The fallback variant's index moves with the retained set; if the fallback itself
            // was dropped, the union simply has no fallback any more.
            default_variant: default_variant
                .and_then(|target| retained.iter().position(|index| *index == target)),
        },
        UnionStrategy::Disjoint => UnionStrategy::Disjoint,
        UnionStrategy::Trial { mode } => UnionStrategy::Trial { mode: *mode },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discriminated(default_variant: Option<usize>, open: bool) -> UnionStrategy {
        UnionStrategy::Discriminated {
            tag_field: "kind".to_owned(),
            default_variant,
            mode: UnionMode::OneOf,
            open,
        }
    }

    /// `(default_variant, open)` of a narrowed discriminated strategy.
    fn fallback_and_openness(strategy: &UnionStrategy) -> (Option<usize>, bool) {
        match strategy {
            UnionStrategy::Discriminated {
                default_variant,
                open,
                ..
            } => (*default_variant, *open),
            UnionStrategy::Disjoint | UnionStrategy::Trial { .. } => {
                panic!("a discriminated strategy stays discriminated: {strategy:?}")
            }
        }
    }

    #[test]
    fn a_narrowed_open_union_stays_open() {
        // The dropped variant's tags become unlisted, and reach `Unknown` like any other.
        let narrowed = retain_strategy(&discriminated(None, true), &[0, 2]);
        assert_eq!(fallback_and_openness(&narrowed), (None, true));
        let closed = retain_strategy(&discriminated(None, false), &[0, 2]);
        assert_eq!(fallback_and_openness(&closed), (None, false));
    }

    #[test]
    fn a_closed_union_losing_its_fallback_stays_closed() {
        // The fallback moves with the retained set, and dropping it opens nothing.
        let moved = retain_strategy(&discriminated(Some(2), false), &[0, 2]);
        assert_eq!(fallback_and_openness(&moved), (Some(1), false));
        let dropped = retain_strategy(&discriminated(Some(1), false), &[0, 2]);
        assert_eq!(fallback_and_openness(&dropped), (None, false));
    }
}
