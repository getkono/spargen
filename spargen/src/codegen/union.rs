//! Union enums: a `oneOf` / `anyOf` model, with the custom serde its lowered strategy selects.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{DisjointFeature, JsonCategory, Ty, Union, UnionMode, UnionStrategy, UnionVariant};
use crate::name::Names;

use super::ty::ty_tokens;

/// Emit the enum for union definition `id`, one boxed payload variant per union variant, with the
/// `Deserialize` / `Serialize` impls of its strategy: by discriminator tag, by a statically
/// disjoint feature, or by trying each variant's schema. `docs` and `deprecated` are the
/// definition's own attributes.
pub(super) fn emit_union(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    union: &Union,
    names: &Names,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
) -> TokenStream {
    match &union.strategy {
        // Strategy A: a discriminator → a custom `Deserialize`/`Serialize` over a buffered
        // `serde_json::Value`. NOT serde `#[serde(tag = ...)]`: internal tagging consumes the tag
        // field out of the buffer, so a variant struct that declares the discriminator as a
        // (usually required) property would fail with "missing field". Instead the WHOLE value is
        // handed to the selected variant (it keeps its own tag field), and on serialize the tag is
        // re-inserted only when the variant did not already write it. No `untagged`, no `Value`
        // degrade.
        UnionStrategy::Discriminated {
            tag_field,
            tags,
            categories,
            default_variant,
            untagged,
            mode,
        } => {
            let variant_defs = union_variant_defs(id, union, names);
            let category_arms =
                union
                    .variants
                    .iter()
                    .zip(categories)
                    .filter_map(|(variant, category)| {
                        let category = category.as_ref()?;
                        let variant_ident = union_variant_ident(names, id, variant);
                        let predicate = json_category_predicate(category);
                        Some(quote! {
                            if #predicate {
                                return serde_json::from_value(value)
                                    .map(#ident::#variant_ident)
                                    .map_err(serde::de::Error::custom);
                            }
                        })
                    });
            let de_arms = union
                .variants
                .iter()
                .zip(tags)
                .filter(|(_, accepted)| !accepted.is_empty())
                .map(|(variant, accepted)| {
                    let variant_ident = union_variant_ident(names, id, variant);
                    // One arm per variant, matching every tag that selects it.
                    quote! {
                        #(#accepted)|* => serde_json::from_value(value)
                            .map(#ident::#variant_ident)
                            .map_err(serde::de::Error::custom),
                    }
                });
            let ser_arms = union.variants.iter().zip(tags).map(|(variant, accepted)| {
                let variant_ident = union_variant_ident(names, id, variant);
                // The first accepted tag is the canonical one written back.
                let tag = match accepted.first() {
                    Some(tag) => quote! { Some(#tag) },
                    None => quote! { None },
                };
                quote! {
                    #ident::#variant_ident(inner) => (
                        serde_json::to_value(inner).map_err(serde::ser::Error::custom)?,
                        #tag,
                    ),
                }
            });
            let missing_tag = format!(
                "missing discriminator field `{tag_field}` for union {}",
                ident.as_str()
            );
            let unknown_tag = format!("unknown discriminator value for union {}", ident.as_str());
            let non_object = format!(
                "tagged variant of union {} did not serialize as an object",
                ident.as_str()
            );
            // OpenAPI 3.2 `defaultMapping`: an absent or unrecognized tag falls back to a
            // named variant instead of failing.
            let fallback = default_variant.map(|index| {
                let variant = &union.variants[index];
                let variant_ident = union_variant_ident(names, id, variant);
                quote! {
                    serde_json::from_value(value)
                        .map(#ident::#variant_ident)
                        .map_err(serde::de::Error::custom)
                }
            });
            let missing_tag_arm = match &fallback {
                Some(fallback) => fallback.clone(),
                None => quote! { Err(serde::de::Error::custom(#missing_tag)) },
            };
            let unknown_tag_arm = match &fallback {
                Some(fallback) => fallback.clone(),
                None => quote! { Err(serde::de::Error::custom(#unknown_tag)) },
            };
            // Variants no tag selects are tried by their own schemas — with the source
            // applicator's semantics, as `Trial` tries them — before the absent or
            // unrecognized tag falls through to `defaultMapping` or the error. A tag that
            // names a tagged variant never reaches them.
            let attempts: Vec<TokenStream> = union
                .variants
                .iter()
                .zip(untagged)
                .filter_map(|(variant, priority)| {
                    let priority = (*priority)?;
                    let variant_ident = union_variant_ident(names, id, variant);
                    let ty = union_variant_ty_tokens(variant.ty, names);
                    Some(trial_attempt_tokens(ident, variant_ident, &ty, priority))
                })
                .collect();
            let untagged_trial = |otherwise: TokenStream| {
                if attempts.is_empty() {
                    return otherwise;
                }
                let valid = trial_match_rule_tokens(*mode);
                quote! {
                    {
                        let mut match_count = 0_usize;
                        let mut selected: Option<(u32, Self)> = None;
                        #(#attempts)*
                        match selected {
                            Some((_, selected)) if #valid => Ok(selected),
                            _ => #otherwise,
                        }
                    }
                }
            };
            let missing_tag_arm = untagged_trial(missing_tag_arm);
            let unknown_tag_arm = untagged_trial(unknown_tag_arm);
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#variant_defs)*
                }

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        let value = serde_json::Value::deserialize(deserializer)?;
                        #(#category_arms)*
                        let tag = value
                            .get(#tag_field)
                            .and_then(serde_json::Value::as_str)
                            .map(std::borrow::ToOwned::to_owned);
                        let Some(tag) = tag else {
                            return #missing_tag_arm;
                        };
                        match tag.as_str() {
                            #(#de_arms)*
                            _ => #unknown_tag_arm,
                        }
                    }
                }

                impl serde::Serialize for #ident {
                    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        let (mut value, tag): (serde_json::Value, Option<&str>) = match self {
                            #(#ser_arms)*
                        };
                        if let Some(tag) = tag {
                            let serde_json::Value::Object(map) = &mut value else {
                                return Err(serde::ser::Error::custom(#non_object));
                            };
                            map.entry(#tag_field.to_owned()).or_insert_with(|| {
                                serde_json::Value::String(tag.to_owned())
                            });
                        }
                        value.serialize(serializer)
                    }
                }
            }
        }
        // Strategy B: no discriminator but statically-disjoint variants → an enum with a custom
        // content-inspecting `Deserialize` (buffer the value, dispatch on the proven feature) and
        // a `Serialize` that emits just the active variant's inner value (no wrapper, no tag).
        UnionStrategy::Disjoint { features } => {
            let variant_defs = union_variant_defs(id, union, names);
            let de_arms = union
                .variants
                .iter()
                .zip(features)
                .map(|(variant, feature)| {
                    let variant_ident = union_variant_ident(names, id, variant);
                    let predicate = match feature {
                        DisjointFeature::JsonType(category) => json_category_predicate(category),
                        DisjointFeature::RequiredKey(key) => {
                            quote! { value.get(#key).is_some() }
                        }
                    };
                    quote! {
                        if #predicate {
                            return serde_json::from_value(value)
                                .map(#ident::#variant_ident)
                                .map_err(serde::de::Error::custom);
                        }
                    }
                });
            let ser_arms = union.variants.iter().map(|variant| {
                let variant_ident = union_variant_ident(names, id, variant);
                quote! { #ident::#variant_ident(inner) => inner.serialize(serializer), }
            });
            let error_message =
                format!("data did not match any variant of union {}", ident.as_str());
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#variant_defs)*
                }

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        let value = serde_json::Value::deserialize(deserializer)?;
                        #(#de_arms)*
                        Err(serde::de::Error::custom(#error_message))
                    }
                }

                impl serde::Serialize for #ident {
                    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        match self {
                            #(#ser_arms)*
                        }
                    }
                }
            }
        }
        UnionStrategy::Trial { mode, priorities } => {
            let variant_defs = union_variant_defs(id, union, names);
            let attempts = union
                .variants
                .iter()
                .zip(priorities)
                .map(|(variant, priority)| {
                    let variant_ident = union_variant_ident(names, id, variant);
                    let ty = union_variant_ty_tokens(variant.ty, names);
                    trial_attempt_tokens(ident, variant_ident, &ty, *priority)
                });
            let ser_arms = union.variants.iter().map(|variant| {
                let variant_ident = union_variant_ident(names, id, variant);
                quote! {
                    #ident::#variant_ident(inner) => {
                        serde_json::to_value(inner).map_err(serde::ser::Error::custom)?
                    }
                }
            });
            let validations = union.variants.iter().map(|variant| {
                let ty = union_variant_ty_tokens(variant.ty, names);
                quote! {
                    if serde_json::from_value::<#ty>(value.clone()).is_ok() {
                        match_count += 1;
                    }
                }
            });
            let expected = match mode {
                UnionMode::OneOf => "exactly one",
                UnionMode::AnyOf => "at least one",
            };
            let de_valid = trial_match_rule_tokens(*mode);
            let ser_valid = de_valid.clone();
            let de_error = format!(
                "data must match {expected} typed variant of union {}",
                ident.as_str()
            );
            let ser_error = format!(
                "serialized value must match {expected} typed variant of union {}",
                ident.as_str()
            );
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#variant_defs)*
                }

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        let value = serde_json::Value::deserialize(deserializer)?;
                        let mut match_count = 0_usize;
                        let mut selected: Option<(u32, Self)> = None;
                        #(#attempts)*
                        if #de_valid {
                            selected
                                .map(|(_, value)| value)
                                .ok_or_else(|| serde::de::Error::custom(#de_error))
                        } else {
                            Err(serde::de::Error::custom(#de_error))
                        }
                    }
                }

                impl serde::Serialize for #ident {
                    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        let value = match self {
                            #(#ser_arms),*
                        };
                        let mut match_count = 0_usize;
                        #(#validations)*
                        if #ser_valid {
                            value.serialize(serializer)
                        } else {
                            Err(serde::ser::Error::custom(#ser_error))
                        }
                    }
                }
            }
        }
    }
}

/// Union payloads are uniformly indirect so an API's largest object variant cannot inflate every
/// value of the enum (or trip strict `large_enum_variant` linting). Existing recursive boxing is a
/// boolean representation flag, so setting it again never produces `Box<Box<T>>`.
fn union_variant_ty_tokens(ty: Ty, names: &Names) -> TokenStream {
    ty_tokens(Ty { boxed: true, ..ty }, names, false)
}

/// The identifier `name` allocated for one variant of union `id`.
fn union_variant_ident<'a>(
    names: &'a Names,
    id: crate::ir::TypeId,
    variant: &UnionVariant,
) -> &'a crate::name::Ident {
    names
        .variants
        .get(&(id, variant.name_hint.clone()))
        .expect("union variant name allocated")
}

/// The union enum's variant definitions, one `Variant(Box<T>),` per union variant in source order.
fn union_variant_defs(id: crate::ir::TypeId, union: &Union, names: &Names) -> Vec<TokenStream> {
    union
        .variants
        .iter()
        .map(|variant| {
            let variant_ident = union_variant_ident(names, id, variant);
            let ty = union_variant_ty_tokens(variant.ty, names);
            quote! { #variant_ident(#ty), }
        })
        .collect()
}

/// The test a buffered `value` passes when its JSON type is `category`.
fn json_category_predicate(category: &JsonCategory) -> TokenStream {
    match category {
        JsonCategory::String => quote! { value.is_string() },
        JsonCategory::Number => quote! { value.is_number() },
        JsonCategory::Boolean => quote! { value.is_boolean() },
        JsonCategory::Array => quote! { value.is_array() },
        JsonCategory::Object => quote! { value.is_object() },
    }
}

/// One variant's attempt in a trial decode: decode `value` as the variant's type, count the
/// match, and keep it when its priority beats the one selected so far (the earlier variant wins a
/// tie). The emitted block expects `value`, `match_count`, and `selected: Option<(u32, Self)>` in
/// scope. `Trial` tries every variant through it, and `Discriminated` its untagged variants, so
/// the two strategies share one selection rule.
fn trial_attempt_tokens(
    ident: &crate::name::Ident,
    variant_ident: &crate::name::Ident,
    ty: &TokenStream,
    priority: u32,
) -> TokenStream {
    quote! {
        if let Ok(inner) = serde_json::from_value::<#ty>(value.clone()) {
            match_count += 1;
            let replace = match &selected {
                Some((selected_priority, _)) => #priority > *selected_priority,
                None => true,
            };
            if replace {
                selected = Some((#priority, #ident::#variant_ident(inner)));
            }
        }
    }
}

/// The condition a trial's `match_count` must meet for the union's applicator: exactly one
/// matching variant for `oneOf`, at least one for `anyOf`.
fn trial_match_rule_tokens(mode: UnionMode) -> TokenStream {
    match mode {
        UnionMode::OneOf => quote! { match_count == 1 },
        UnionMode::AnyOf => quote! { match_count >= 1 },
    }
}
