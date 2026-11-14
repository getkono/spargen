//! Union enums: a `oneOf` / `anyOf` model, with the custom serde its lowered strategy selects.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{
    DisjointFeature, JsonCategory, Ty, TypeGraph, TypeKind, Union, UnionMode, UnionStrategy,
    UnionVariant, VariantDispatch,
};
use crate::name::Names;

use super::ty::boxed_ty_tokens;

/// Emit the enum for union definition `id`, one boxed payload variant per union variant, with the
/// `Deserialize` / `Serialize` impls of its strategy: by discriminator (a unique JSON category for
/// a non-object variant, then the tag, then a trial of the variants no tag selects, then the
/// `defaultMapping` variant, or for an open union the `Unknown` variant), by a statically disjoint
/// feature, or by trying each variant's schema. `docs` and `deprecated` are the definition's own
/// attributes; `types` is read for which variants are themselves open unions, whose `Unknown` a
/// trial takes only when no variant matches.
pub(super) fn emit_union(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    union: &Union,
    types: &TypeGraph,
    names: &Names,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
) -> TokenStream {
    match &union.strategy {
        // Strategy A: a discriminator → a custom `Deserialize`/`Serialize` over a buffered
        // `serde_json::Value`. NOT serde `#[serde(tag = ...)]`: internal tagging consumes the tag
        // field out of the buffer, so a variant struct that declares the discriminator as a
        // (usually required) property would fail with "missing field". Instead the WHOLE value is
        // handed to the selected variant (it keeps its own tag field), and on serialize a tagged
        // variant's first tag is inserted only when the variant did not already write it; a tag
        // it wrote that would not select it again is a serialization error. A variant selected
        // by category or trial is written with no tag. No serde `untagged`, no `Value` degrade.
        UnionStrategy::Discriminated {
            tag_field,
            default_variant,
            mode,
            open,
        } => {
            let decl = union_enum_decl(id, ident, union, names, &docs, &deprecated);
            let unknown = open.then(|| open_variant_ident(names, id));
            let category_arms = union.variants.iter().filter_map(|variant| {
                let category = discriminated_dispatch(variant).category?;
                let variant_ident = union_variant_ident(names, id, variant);
                let predicate = json_category_predicate(&category);
                let decode = decode_variant_tokens(ident, variant_ident);
                Some(quote! {
                    if #predicate {
                        return #decode;
                    }
                })
            });
            let de_arms = union
                .variants
                .iter()
                .map(|variant| (variant, discriminated_dispatch(variant).tags))
                .filter(|(_, accepted)| !accepted.is_empty())
                .map(|(variant, accepted)| {
                    let variant_ident = union_variant_ident(names, id, variant);
                    let decode = decode_variant_tokens(ident, variant_ident);
                    // One arm per variant, matching every tag that selects it.
                    quote! {
                        #(#accepted)|* => #decode,
                    }
                });
            let ser_arms = union.variants.iter().map(|variant| {
                let accepted = discriminated_dispatch(variant).tags;
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
            // An open union writes an unknown payload back as it arrived, its own tag kept; the
            // tag is inserted only into a value constructed without it.
            let unknown_ser_arm = unknown.map(|unknown| {
                quote! {
                    #ident::#unknown { tag, value } => (value.clone(), Some(tag.as_str())),
                }
            });
            // A tag the variant already wrote is kept only if decoding it selects that variant
            // again: one of its own tags, or, for the `defaultMapping` variant, any value no other
            // variant's tag claims, so long as no untagged variant takes the payload first
            // (`fallback_claim_check`). Any other held value is refused, never written out (#1344).
            let refusal_arms = union.variants.iter().enumerate().map(|(index, variant)| {
                let variant_ident = union_variant_ident(names, id, variant);
                let refusal = format!(
                    " does not select variant {}::{}",
                    ident.as_str(),
                    variant_ident.as_str()
                );
                if *default_variant == Some(index) {
                    let others = union
                        .variants
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != index)
                        .flat_map(|(_, other)| discriminated_dispatch(other).tags);
                    // A non-string tag reads as an absent one, which also falls back here.
                    let others: Vec<_> = others.collect();
                    if others.is_empty() {
                        return quote! { #ident::#variant_ident(_) => None, };
                    }
                    quote! {
                        #ident::#variant_ident(_) => match held.as_str() {
                            #(Some(#others))|* => Some(#refusal),
                            _ => None,
                        },
                    }
                } else {
                    let accepted = discriminated_dispatch(variant).tags;
                    if accepted.is_empty() {
                        quote! { #ident::#variant_ident(_) => None, }
                    } else {
                        quote! {
                            #ident::#variant_ident(_) => match held.as_str() {
                                #(Some(#accepted))|* => None,
                                _ => Some(#refusal),
                            },
                        }
                    }
                }
            });
            let unknown_refusal_arm = unknown.map(|unknown| {
                quote! { #ident::#unknown { .. } => None, }
            });
            let held_tag = format!("discriminator `{tag_field}` value ");
            // Where no variant has a tag to hold, only `Unknown` reaches the check, and it keeps
            // whatever it holds.
            let tag_write = if default_variant.is_some()
                || union
                    .variants
                    .iter()
                    .any(|variant| !discriminated_dispatch(variant).tags.is_empty())
            {
                quote! {
                    if let Some(held) = map.get(#tag_field) {
                        let refusal: Option<&str> = match self {
                            #(#refusal_arms)*
                            #unknown_refusal_arm
                        };
                        if let Some(refusal) = refusal {
                            return Err(serde::ser::Error::custom(format_args!(
                                "{}{}{}",
                                #held_tag,
                                held,
                                refusal
                            )));
                        }
                    } else {
                        map.insert(
                            #tag_field.to_owned(),
                            serde_json::Value::String(tag.to_owned()),
                        );
                    }
                }
            } else {
                quote! {
                    map.entry(#tag_field.to_owned()).or_insert_with(|| {
                        serde_json::Value::String(tag.to_owned())
                    });
                }
            };
            // A variant with no tag of its own (trial-selected, or a `defaultMapping` fallback
            // without one) writes none, so one it holds is checked too: a tag another variant
            // claims would route the payload there on decode, and is refused. Its other values,
            // and a non-string, decode as an unrecognized or absent tag would.
            let claimed: Vec<&String> = union
                .variants
                .iter()
                .flat_map(|variant| discriminated_dispatch(variant).tags)
                .collect();
            let tagless_arms: Vec<TokenStream> = union
                .variants
                .iter()
                .filter(|variant| {
                    let dispatch = discriminated_dispatch(variant);
                    dispatch.tags.is_empty() && dispatch.category.is_none()
                })
                .map(|variant| {
                    let variant_ident = union_variant_ident(names, id, variant);
                    let refusal = format!(
                        " does not select variant {}::{}",
                        ident.as_str(),
                        variant_ident.as_str()
                    );
                    quote! { #ident::#variant_ident(_) => Some(#refusal), }
                })
                .collect();
            let tagless_check = (!claimed.is_empty() && !tagless_arms.is_empty()).then(|| {
                quote! {
                    let refusal: Option<&str> = match self {
                        #(#tagless_arms)*
                        _ => None,
                    };
                    if let (Some(refusal), Some(held)) = (refusal, value.get(#tag_field)) {
                        if matches!(held.as_str(), #(Some(#claimed))|*) {
                            return Err(serde::ser::Error::custom(format_args!(
                                "{}{}{}",
                                #held_tag,
                                held,
                                refusal
                            )));
                        }
                    }
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
                decode_variant_tokens(ident, union_variant_ident(names, id, variant))
            });
            let missing_tag_arm = match &fallback {
                Some(fallback) => fallback.clone(),
                None => quote! { Err(serde::de::Error::custom(#missing_tag)) },
            };
            // Variants no tag selects are tried by their own schemas — with the source
            // applicator's semantics, as `Trial` tries them — before the absent or
            // unrecognized tag falls through to `defaultMapping` or the error. A tag that
            // names a tagged variant never reaches them.
            let attempts: Vec<TokenStream> = union
                .variants
                .iter()
                .filter_map(|variant| {
                    let priority = discriminated_dispatch(variant).untagged?;
                    let variant_ident = union_variant_ident(names, id, variant);
                    let ty = boxed_ty_tokens(variant.ty, names, false);
                    let unknown = open_member_unknown_test(types, names, variant.ty);
                    let guard =
                        trial_guard_tokens(untagged_category(types, variant.ty), variant.ty);
                    Some(trial_attempt_tokens(
                        ident,
                        variant_ident,
                        &ty,
                        priority,
                        guard.as_ref(),
                        unknown.as_ref(),
                    ))
                })
                .collect();
            // `open_unions`: an unrecognized tag is kept, with the payload, in `Unknown`, but only
            // for a payload no untagged variant matched: a `oneOf` payload two of them take is a
            // decode failure, not an unknown kind. Lowering never opens a union that has a
            // fallback.
            let unknown_tag_arm = match (&fallback, unknown) {
                (Some(fallback), _) => fallback.clone(),
                (None, Some(unknown)) if attempts.is_empty() => {
                    quote! { Ok(#ident::#unknown { tag, value }) }
                }
                (None, Some(unknown)) => {
                    let ambiguous = format!(
                        "data matched more than one untagged variant of union {}",
                        ident.as_str()
                    );
                    quote! {
                        if match_count == 0 {
                            Ok(#ident::#unknown { tag, value })
                        } else {
                            Err(serde::de::Error::custom(#ambiguous))
                        }
                    }
                }
                (None, None) => quote! { Err(serde::de::Error::custom(#unknown_tag)) },
            };
            let fallback_member = union.variants.iter().any(|variant| {
                discriminated_dispatch(variant).untagged.is_some()
                    && open_member_unknown_test(types, names, variant.ty).is_some()
            });
            let untagged_trial = |otherwise: TokenStream| {
                if attempts.is_empty() {
                    return otherwise;
                }
                let valid = trial_match_rule_tokens(*mode);
                let fallback_decl = fallback_member.then(unknown_fallback_decl_tokens);
                let otherwise = if fallback_member {
                    quote! {
                        match unknown_fallback {
                            Some(fallback) if match_count == 0 => Ok(fallback),
                            _ => #otherwise,
                        }
                    }
                } else {
                    otherwise
                };
                quote! {
                    {
                        let mut match_count = 0_usize;
                        let mut selected: Option<(u32, Self)> = None;
                        #fallback_decl
                        #(#attempts)*
                        match selected {
                            Some((_, selected)) if #valid => Ok(selected),
                            _ => #otherwise,
                        }
                    }
                }
            };
            // The `defaultMapping` variant's payload reaches the fallback on decode only when its
            // tag is absent, unclaimed or no string, and then only after the untagged variants'
            // trial has taken nothing: a payload the trial would take is refused (#1376), naming
            // the variant it would decode as. A tag a tagged variant claims routes by tag instead
            // (its own) or was refused above (another's).
            let fallback_claim_check =
                default_variant
                    .filter(|_| !attempts.is_empty())
                    .map(|index| {
                        let fallback_ident = union_variant_ident(names, id, &union.variants[index]);
                        let reaches_trial = if claimed.is_empty() {
                            quote! { matches!(self, #ident::#fallback_ident(_)) }
                        } else {
                            quote! {
                                matches!(self, #ident::#fallback_ident(_))
                                    && !matches!(
                                        value.get(#tag_field).and_then(serde_json::Value::as_str),
                                        #(Some(#claimed))|*
                                    )
                            }
                        };
                        let valid = trial_match_rule_tokens(*mode);
                        let fallback_decl = fallback_member.then(unknown_fallback_decl_tokens);
                        let unmatched = if fallback_member {
                            quote! {
                                if match_count == 0 {
                                    unknown_fallback
                                } else {
                                    None
                                }
                            }
                        } else {
                            quote! { None }
                        };
                        let claimant_names = union.variants.iter().map(|variant| {
                            let variant_ident = union_variant_ident(names, id, variant);
                            let name = format!("{}::{}", ident.as_str(), variant_ident.as_str());
                            quote! { #ident::#variant_ident(_) => #name, }
                        });
                        // Lowering never opens a union with a fallback; the arm keeps the match
                        // exhaustive regardless.
                        let unknown_name = unknown.map(|unknown| {
                            let name = format!("{}::{}", ident.as_str(), unknown.as_str());
                            quote! { #ident::#unknown { .. } => #name, }
                        });
                        let refusal = format!(
                            "payload of variant {}::{} decodes as variant ",
                            ident.as_str(),
                            fallback_ident.as_str()
                        );
                        quote! {
                            if #reaches_trial {
                                let mut match_count = 0_usize;
                                let mut selected: Option<(u32, Self)> = None;
                                #fallback_decl
                                #(#attempts)*
                                let claimant = match selected {
                                    Some((_, selected)) if #valid => Some(selected),
                                    _ => #unmatched,
                                };
                                if let Some(claimant) = claimant {
                                    let claimant = match claimant {
                                        #(#claimant_names)*
                                        #unknown_name
                                    };
                                    return Err(serde::ser::Error::custom(format_args!(
                                        "{}{}",
                                        #refusal,
                                        claimant
                                    )));
                                }
                            }
                        }
                    });
            let missing_tag_arm = untagged_trial(missing_tag_arm);
            let unknown_tag_arm = untagged_trial(unknown_tag_arm);
            quote! {
                #decl

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
                            #unknown_ser_arm
                        };
                        if let Some(tag) = tag {
                            let serde_json::Value::Object(map) = &mut value else {
                                return Err(serde::ser::Error::custom(#non_object));
                            };
                            #tag_write
                        }
                        #tagless_check
                        #fallback_claim_check
                        value.serialize(serializer)
                    }
                }
            }
        }
        // Strategy B: no discriminator but statically-disjoint variants → an enum with a custom
        // content-inspecting `Deserialize` (buffer the value, dispatch on the proven feature) and
        // a `Serialize` that emits just the active variant's inner value (no wrapper, no tag).
        UnionStrategy::Disjoint => {
            let decl = union_enum_decl(id, ident, union, names, &docs, &deprecated);
            let de_arms = union.variants.iter().map(|variant| {
                let variant_ident = union_variant_ident(names, id, variant);
                let predicate = match &variant.dispatch {
                    VariantDispatch::Disjoint(DisjointFeature::JsonType(category)) => {
                        json_category_predicate(category)
                    }
                    VariantDispatch::Disjoint(DisjointFeature::RequiredKey(key)) => {
                        quote! { value.get(#key).is_some() }
                    }
                    VariantDispatch::Discriminated { .. }
                    | VariantDispatch::Trial { .. }
                    | VariantDispatch::Deferred { .. } => {
                        unreachable!("{MISMATCHED_DISPATCH}")
                    }
                };
                let decode = decode_variant_tokens(ident, variant_ident);
                quote! {
                    if #predicate {
                        return #decode;
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
                #decl

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
        UnionStrategy::Trial { mode } => {
            let decl = union_enum_decl(id, ident, union, names, &docs, &deprecated);
            let attempts = union.variants.iter().map(|variant| {
                let (priority, category) = trial_dispatch(variant);
                let variant_ident = union_variant_ident(names, id, variant);
                let ty = boxed_ty_tokens(variant.ty, names, false);
                let unknown = open_member_unknown_test(types, names, variant.ty);
                let guard = trial_guard_tokens(category, variant.ty);
                trial_attempt_tokens(
                    ident,
                    variant_ident,
                    &ty,
                    priority,
                    guard.as_ref(),
                    unknown.as_ref(),
                )
            });
            let ser_arms = union.variants.iter().map(|variant| {
                let variant_ident = union_variant_ident(names, id, variant);
                quote! {
                    #ident::#variant_ident(inner) => {
                        serde_json::to_value(inner).map_err(serde::ser::Error::custom)?
                    }
                }
            });
            let fallback_member = union
                .variants
                .iter()
                .any(|variant| open_member_unknown_test(types, names, variant.ty).is_some());
            // A value an open member decodes only as its `Unknown` is held to the same rule on
            // serialize as on decode: it counts only when no variant matches outright. A variant
            // is tried on the written value under the same category guard as on decode.
            let validations = union.variants.iter().map(|variant| {
                let ty = boxed_ty_tokens(variant.ty, names, false);
                let guard = trial_guard_tokens(trial_dispatch(variant).1, variant.ty);
                match open_member_unknown_test(types, names, variant.ty) {
                    Some(unknown) => {
                        let (pattern, attempt) = guarded_decode_tokens(&ty, guard.as_ref());
                        quote! {
                            if let #pattern = #attempt {
                                if #unknown {
                                    unknown_match = true;
                                } else {
                                    match_count += 1;
                                }
                            }
                        }
                    }
                    None => {
                        let guard = guard.map(|guard| quote! { #guard && });
                        quote! {
                            if #guard serde_json::from_value::<#ty>(value.clone()).is_ok() {
                                match_count += 1;
                            }
                        }
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
            let fallback_decl = fallback_member.then(unknown_fallback_decl_tokens);
            let fallback_arm = fallback_member.then(|| {
                quote! {
                    else if match_count == 0 {
                        unknown_fallback.ok_or_else(|| serde::de::Error::custom(#de_error))
                    }
                }
            });
            let unknown_match_decl =
                fallback_member.then(|| quote! { let mut unknown_match = false; });
            let ser_valid = if fallback_member {
                quote! { #ser_valid || (match_count == 0 && unknown_match) }
            } else {
                ser_valid
            };
            quote! {
                #decl

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        let value = serde_json::Value::deserialize(deserializer)?;
                        let mut match_count = 0_usize;
                        let mut selected: Option<(u32, Self)> = None;
                        #fallback_decl
                        #(#attempts)*
                        if #de_valid {
                            selected
                                .map(|(_, value)| value)
                                .ok_or_else(|| serde::de::Error::custom(#de_error))
                        } #fallback_arm else {
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
                        #unknown_match_decl
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

/// Why a union variant's dispatch is always of its union's strategy arm: lowering assigns them
/// together, and `check_invariants` refuses an API where they disagree before codegen runs.
const MISMATCHED_DISPATCH: &str =
    "check_invariants holds every union variant's dispatch to its union's strategy";

/// What selects one variant of a [`UnionStrategy::Discriminated`] union, borrowed from its
/// [`VariantDispatch::Discriminated`].
struct Discriminated<'a> {
    /// The tag values that select the variant; the first is the one serialization writes.
    tags: &'a [String],
    /// The JSON category that selects a non-object variant.
    category: Option<JsonCategory>,
    /// The trial priority of an object variant no tag selects.
    untagged: Option<u32>,
}

/// `variant`'s dispatch, as a variant of a [`UnionStrategy::Discriminated`] union carries it.
fn discriminated_dispatch(variant: &UnionVariant) -> Discriminated<'_> {
    match &variant.dispatch {
        VariantDispatch::Discriminated {
            tags,
            category,
            untagged,
        } => Discriminated {
            tags,
            category: *category,
            untagged: *untagged,
        },
        VariantDispatch::Disjoint(_)
        | VariantDispatch::Trial { .. }
        | VariantDispatch::Deferred { .. } => {
            unreachable!("{MISMATCHED_DISPATCH}")
        }
    }
}

/// The union enum's declaration, `docs` and `deprecated` first, one variant per
/// [`union_variant_defs`] entry, then, for an open union, the `Unknown { tag, value }` variant.
/// Every strategy declares the same enum and differs only in the serde impls beside it.
fn union_enum_decl(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    union: &Union,
    names: &Names,
    docs: &TokenStream,
    deprecated: &Option<TokenStream>,
) -> TokenStream {
    let variant_defs = union_variant_defs(id, union, names);
    let unknown = match &union.strategy {
        UnionStrategy::Discriminated {
            tag_field,
            open: true,
            ..
        } => {
            let unknown = open_variant_ident(names, id);
            let doc = format!(
                "A payload whose `{tag_field}` names a kind this description does not list. \
                 Decoding produces it only for a value no other variant takes."
            );
            let tag_doc = format!("The `{tag_field}` value the payload carries.");
            Some(quote! {
                #[doc = #doc]
                #unknown {
                    #[doc = #tag_doc]
                    tag: String,
                    /// The whole payload, as it arrived.
                    value: serde_json::Value,
                },
            })
        }
        _ => None,
    };
    quote! {
        #docs
        #deprecated
        #[derive(Debug, Clone, PartialEq)]
        pub enum #ident {
            #(#variant_defs)*
            #unknown
        }
    }
}

/// The identifier `name` allocated for open union `id`'s `Unknown` variant.
fn open_variant_ident(names: &Names, id: crate::ir::TypeId) -> &crate::name::Ident {
    names
        .open_variants
        .get(&id)
        .expect("open union variant name allocated")
}

/// The expression decoding the buffered `value` into `variant_ident`'s payload, as the
/// `Result<#ident, D::Error>` a `Deserialize` impl returns.
fn decode_variant_tokens(
    ident: &crate::name::Ident,
    variant_ident: &crate::name::Ident,
) -> TokenStream {
    quote! {
        serde_json::from_value(value)
            .map(#ident::#variant_ident)
            .map_err(serde::de::Error::custom)
    }
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
            let ty = boxed_ty_tokens(variant.ty, names, false);
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

/// The `(priority, category)` of `variant`, as a variant of a [`UnionStrategy::Trial`] union
/// carries them.
fn trial_dispatch(variant: &UnionVariant) -> (u32, Option<JsonCategory>) {
    match variant.dispatch {
        VariantDispatch::Trial { priority, category } => (priority, category),
        VariantDispatch::Discriminated { .. }
        | VariantDispatch::Disjoint(_)
        | VariantDispatch::Deferred { .. } => {
            unreachable!("{MISMATCHED_DISPATCH}")
        }
    }
}

/// The JSON category of an untagged variant of a [`UnionStrategy::Discriminated`] union: `Object`
/// for an object struct; `None`, trying it on every payload, for any other type (a variant that
/// cannot carry the tag property and shares its JSON category, say), a reservation included.
fn untagged_category(types: &TypeGraph, ty: Ty) -> Option<JsonCategory> {
    let definition = types.get(ty.id)?;
    matches!(definition.kind, TypeKind::Struct(_)).then_some(JsonCategory::Object)
}

/// The test a buffered `value` passes before a trial tries a variant of type `ty` whose values all
/// serialize as `category`: that category's predicate, or `null` beside it when `ty` is nullable.
/// `None` for an uncategorised variant, which is tried on every payload. serde_json's `Value`
/// deserializer is more lenient than the schema: it reads an array into a derived struct field by
/// field, and a one-key object into a unit enum variant, so an unguarded attempt would take a
/// payload of another category.
fn trial_guard_tokens(category: Option<JsonCategory>, ty: Ty) -> Option<TokenStream> {
    let predicate = json_category_predicate(&category?);
    Some(if ty.nullable {
        quote! { (value.is_null() || #predicate) }
    } else {
        predicate
    })
}

/// The `if let` pattern and scrutinee that decode `value` as `ty` into `inner`, only when `guard`
/// ([`trial_guard_tokens`]) holds where there is one.
fn guarded_decode_tokens(
    ty: &TokenStream,
    guard: Option<&TokenStream>,
) -> (TokenStream, TokenStream) {
    let decode = quote! { serde_json::from_value::<#ty>(value.clone()) };
    match guard {
        Some(guard) => (
            quote! { Some(Ok(inner)) },
            quote! { #guard.then(|| #decode) },
        ),
        None => (quote! { Ok(inner) }, decode),
    }
}

/// One variant's attempt in a trial decode: decode `value` as the variant's type, count the
/// match, and keep it when its priority beats the one selected so far (the earlier variant wins a
/// tie). The emitted block expects `value`, `match_count`, and `selected: Option<(u32, Self)>` in
/// scope. `Trial` tries every variant through it, and `Discriminated` its untagged variants, so
/// the two strategies share one selection rule. `guard` ([`trial_guard_tokens`]) is the category
/// test the payload must pass before the variant is tried at all.
///
/// `unknown` is [`open_member_unknown_test`] for a variant that is itself an open union: a value
/// it decodes only as its `Unknown` is not counted as a match but kept, the first such one, in
/// `unknown_fallback` ([`unknown_fallback_decl_tokens`]), which the trial takes only when no
/// variant matched. Counting it would let an open member's catch-all overlap every other object
/// variant, and fail a `oneOf` that decodes with the member closed.
fn trial_attempt_tokens(
    ident: &crate::name::Ident,
    variant_ident: &crate::name::Ident,
    ty: &TokenStream,
    priority: u32,
    guard: Option<&TokenStream>,
    unknown: Option<&TokenStream>,
) -> TokenStream {
    let (pattern, attempt) = guarded_decode_tokens(ty, guard);
    let count = quote! {
        match_count += 1;
        let replace = match &selected {
            Some((selected_priority, _)) => #priority > *selected_priority,
            None => true,
        };
        if replace {
            selected = Some((#priority, #ident::#variant_ident(inner)));
        }
    };
    match unknown {
        Some(unknown) => quote! {
            if let #pattern = #attempt {
                if #unknown {
                    if unknown_fallback.is_none() {
                        unknown_fallback = Some(#ident::#variant_ident(inner));
                    }
                } else {
                    #count
                }
            }
        },
        None => quote! {
            if let #pattern = #attempt {
                #count
            }
        },
    }
}

/// The declaration of the `unknown_fallback` a trial with an open-union variant keeps
/// ([`trial_attempt_tokens`]).
fn unknown_fallback_decl_tokens() -> TokenStream {
    quote! { let mut unknown_fallback: Option<Self> = None; }
}

/// For a union variant of type `ty` that is itself an open union, the test that its decoded
/// payload `inner` (`Box<T>`, or `Option<Box<T>>` when nullable) is that union's `Unknown`;
/// `None` for any other variant type.
fn open_member_unknown_test(types: &TypeGraph, names: &Names, ty: Ty) -> Option<TokenStream> {
    let TypeKind::Union(union) = &types.get(ty.id)?.kind else {
        return None;
    };
    if !union.is_open() {
        return None;
    }
    let member = names.types.get(&ty.id).expect("type name allocated");
    let unknown = open_variant_ident(names, ty.id);
    Some(if ty.nullable {
        quote! { matches!(inner.as_deref(), Some(#member::#unknown { .. })) }
    } else {
        quote! { matches!(*inner, #member::#unknown { .. }) }
    })
}

/// The condition a trial's `match_count` must meet for the union's applicator: exactly one
/// matching variant for `oneOf`, at least one for `anyOf`.
fn trial_match_rule_tokens(mode: UnionMode) -> TokenStream {
    match mode {
        UnionMode::OneOf => quote! { match_count == 1 },
        UnionMode::AnyOf => quote! { match_count >= 1 },
    }
}
