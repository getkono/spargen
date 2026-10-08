//! Parameter serialization: each location's per-parameter statements an operation method runs,
//! and the optional-parameters `…Params` struct.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{MediaType, Operation, ParamLoc, Ty};
use crate::name::Names;

use super::docs::normalize_rustdoc;
use super::operation::param_ident;
use super::ty::ty_tokens;

/// The percent-encoding set for one parameter, derived from its location, style, and
/// `allowReserved`.
///
/// This is the single place that mapping exists. Headers and OpenAPI 3.2's `style: cookie` are
/// sent verbatim — the specification says values there must not be percent-encoded, and that
/// escaping is the caller's job. `in: cookie` with `style: form` *does* encode: Appendix D
/// describes that pairing as the way to opt into automatic encoding, with `allowReserved` as its
/// escape hatch.
fn percent_encoding_tokens(param: &crate::ir::Parameter) -> TokenStream {
    let variant = if param.location == ParamLoc::Header
        || matches!(param.style, crate::ir::ParamStyle::Cookie)
    {
        "Passthrough"
    } else if param.location == ParamLoc::Path {
        if param.allow_reserved {
            "ReservedPath"
        } else {
            "Unreserved"
        }
    } else if param.allow_reserved {
        "Reserved"
    } else {
        "Form"
    };
    let ident = proc_macro2::Ident::new(variant, proc_macro2::Span::call_site());
    quote! { support::PercentEncoding::#ident }
}

/// The runtime `Delimiter` for a non-RFC 6570 query style.
pub(super) fn delimiter_tokens(delimiter: crate::ir::Delimiter) -> TokenStream {
    match delimiter {
        crate::ir::Delimiter::Space => quote! { support::Delimiter::Space },
        crate::ir::Delimiter::Pipe => quote! { support::Delimiter::Pipe },
    }
}

/// Render a path/header parameter value from a borrowed expression. Schema-typed parameters use
/// their declared OpenAPI style; `content`-typed parameters retain their media codec.
///
/// Path values are percent-encoded here, at serialization time, whether schema- or
/// `content`-typed. Splicing a raw value into the path template would let a value containing `/`,
/// `?`, or `#` silently re-target the request.
///
/// Every other location's `content` value is returned as the codec rendered it: a header or
/// cookie is sent verbatim, and the query call site encodes the rendered value itself, so encoding
/// it here too would encode it twice.
pub(super) fn param_value_tokens(param: &crate::ir::Parameter, value: TokenStream) -> TokenStream {
    if let crate::ir::ParamStyle::Content(media) = &param.style {
        let rendered = match media {
            MediaType::Json => quote! {
                serde_json::to_string(#value).map_err(support::Error::request_construction)?
            },
            // Text is the only other media a `content` parameter may carry; lowering rejects the
            // rest, so this arm never sees a codec it cannot render.
            _ => quote! {
                support::serialize_simple(#value, false, support::PercentEncoding::Passthrough)
                    .map_err(support::Error::request_construction)?
            },
        };
        if param.location != ParamLoc::Path {
            return rendered;
        }
        // The rendered representation is one opaque path segment value: every byte the path's
        // encoding set does not admit is escaped, which for every path set includes `/`, `?`, `#`.
        let encoding = percent_encoding_tokens(param);
        return quote! { support::encode(&#rendered, #encoding) };
    }
    let explode = param.explode;
    let encoding = percent_encoding_tokens(param);
    let name = param.name.clone();
    match &param.style {
        crate::ir::ParamStyle::Matrix => quote! {
            support::serialize_matrix(#name, #value, #explode, #encoding)
                .map_err(support::Error::request_construction)?
        },
        crate::ir::ParamStyle::Label => quote! {
            support::serialize_label(#value, #explode, #encoding)
                .map_err(support::Error::request_construction)?
        },
        _ => quote! {
            support::serialize_simple(#value, #explode, #encoding)
                .map_err(support::Error::request_construction)?
        },
    }
}

/// Emit serialization of one query parameter into the operation's fragment vector.
///
/// Fragments arrive at `build_url` already percent-encoded, so the style's delimiters stay
/// literal and remain distinguishable from the same character inside a value.
pub(super) fn query_param_tokens(
    param: &crate::ir::Parameter,
    name: &str,
    value: TokenStream,
    query_binding: &crate::name::Ident,
) -> TokenStream {
    let encoding = percent_encoding_tokens(param);
    match &param.style {
        crate::ir::ParamStyle::Form | crate::ir::ParamStyle::Cookie => {
            let explode = param.explode;
            quote! {
                #query_binding.extend(
                    support::serialize_form(#name, #value, #explode, #encoding)
                        .map_err(support::Error::request_construction)?,
                );
            }
        }
        crate::ir::ParamStyle::Delimited(delimiter) => {
            let delimiter = delimiter_tokens(*delimiter);
            quote! {
                #query_binding.extend(
                    support::serialize_delimited(#name, #value, #delimiter, #encoding)
                        .map_err(support::Error::request_construction)?,
                );
            }
        }
        crate::ir::ParamStyle::DeepObject => quote! {
            #query_binding.extend(
                support::serialize_deep_object(#name, #value, #encoding)
                    .map_err(support::Error::request_construction)?,
            );
        },
        crate::ir::ParamStyle::Content(_) => {
            let rendered = param_value_tokens(param, value);
            quote! {
                #query_binding.push(format!(
                    "{}={}",
                    support::encode(#name, #encoding),
                    support::encode(&#rendered, #encoding),
                ));
            }
        }
        crate::ir::ParamStyle::Simple
        | crate::ir::ParamStyle::Matrix
        | crate::ir::ParamStyle::Label => quote! {},
    }
}

/// A JSON whole-query value: one opaque, fully-encoded token.
pub(super) fn json_querystring_tokens(value: TokenStream) -> TokenStream {
    quote! {
        support::encode(
            &serde_json::to_string(#value).map_err(support::Error::request_construction)?,
            support::PercentEncoding::Form,
        )
    }
}

/// Emit serialization of an optional `in: querystring` value, or a required one that is not JSON
/// (a required JSON value initializes the raw query directly).
pub(super) fn querystring_param_tokens(
    param: &crate::ir::Parameter,
    value: TokenStream,
    query_binding: &crate::name::Ident,
    raw_query_binding: &crate::name::Ident,
) -> TokenStream {
    match &param.style {
        crate::ir::ParamStyle::Content(MediaType::Json) => {
            let encoded = json_querystring_tokens(value);
            quote! { #raw_query_binding = Some(#encoded); }
        }
        crate::ir::ParamStyle::Content(MediaType::FormUrlEncoded) => quote! {
            #query_binding.extend(
                support::serialize_form("", #value, true, support::PercentEncoding::Form)
                    .map_err(support::Error::request_construction)?,
            );
        },
        _ => quote! {},
    }
}

/// Emit serialization of one cookie parameter into the operation's cookie fragments.
///
/// `serialize_form` already returns `name=value` fragments; the caller joins them with `"; "`.
pub(super) fn cookie_param_tokens(
    param: &crate::ir::Parameter,
    name: &str,
    value: TokenStream,
    cookies_binding: &crate::name::Ident,
) -> TokenStream {
    let encoding = percent_encoding_tokens(param);
    match &param.style {
        crate::ir::ParamStyle::Form | crate::ir::ParamStyle::Cookie => {
            let explode = param.explode;
            quote! {
                #cookies_binding.extend(
                    support::serialize_form(#name, #value, #explode, #encoding)
                        .map_err(support::Error::request_construction)?,
                );
            }
        }
        crate::ir::ParamStyle::Content(_) => {
            let rendered = param_value_tokens(param, value);
            quote! { #cookies_binding.push(format!("{}={}", #name, #rendered)); }
        }
        crate::ir::ParamStyle::Simple
        | crate::ir::ParamStyle::Matrix
        | crate::ir::ParamStyle::Label
        | crate::ir::ParamStyle::Delimited(_)
        | crate::ir::ParamStyle::DeepObject => quote! {},
    }
}

/// Emit an operation's optional-parameters `…Params` struct (deriving `Default`, public fields)
/// plus an `impl` of fluent `#[must_use]` consuming setters — one per optional param, named after
/// its field — so callers can write `…Params::default().foo(x).bar(y)` instead of a struct literal.
pub(super) fn emit_params_struct(operation: &Operation, names: &Names) -> TokenStream {
    let ident = names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated");
    let optional: Vec<&crate::ir::Parameter> = operation
        .params
        .iter()
        .filter(|param| !param.required)
        .collect();
    // The setter method reuses the field ident verbatim, so the two can never disagree.
    let field_ident = |param: &crate::ir::Parameter| param_ident(names, operation, param);
    let fields = optional.iter().map(|param| {
        let ident = field_ident(param);
        let wire = &param.name;
        // Every struct param is optional, so the field is always an `Option`. `ty_tokens`
        // already wraps a nullable param (`"null"` in its type array) in `Option`, so only wrap
        // again when it did not — otherwise a nullable optional param becomes `Option<Option<T>>`
        // and the query/header `value.to_string()` serialization would not compile
        // (`Option<T>: !Display`). Absent and `null` both collapse to `None`.
        let ty = if param.ty.nullable {
            ty_tokens(param.ty, names, true)
        } else {
            let inner = ty_tokens(param.ty, names, true);
            quote! { Option<#inner> }
        };
        let mut notes: Vec<String> = Vec::new();
        if param.deprecated {
            notes.push("Deprecated per the spec.".to_owned());
        }
        if let Some(default) = &param.default_display {
            notes.push(format!("Default: `{default}`."));
        }
        let notes = notes
            .iter()
            .map(|note| normalize_rustdoc(note))
            .map(|note| quote! { #[doc = #note] });
        quote! {
            #(#notes)*
            #[serde(rename = #wire, skip_serializing_if = "Option::is_none")]
            pub #ident: #ty,
        }
    });
    // Fluent consuming setters, one per optional param, in field order. Each takes the field's
    // inner `T` by value (never the `Option` wrapper) and stores `Some(T)`: a nullable optional
    // param's field is `Option<T>` for the same reason an ordinary optional param's is, so both
    // accept `T`. `T`-by-value (not `impl Into<T>`) keeps inference/coherence trivial for every
    // generated field type.
    let setters = optional.iter().map(|param| {
        let ident = field_ident(param);
        let inner = ty_tokens(
            Ty {
                nullable: false,
                ..param.ty
            },
            names,
            true,
        );
        let doc = format!("Set the `{}` parameter.", param.name);
        quote! {
            #[doc = #doc]
            #[must_use]
            pub fn #ident(mut self, value: #inner) -> Self {
                self.#ident = Some(value);
                self
            }
        }
    });
    quote! {
        #[allow(dead_code)]
        #[derive(Debug, Clone, Default, serde::Serialize)]
        pub struct #ident {
            #(#fields)*
        }

        // Setters named after fields can trip `wrong_self_convention` when a param is named
        // `is_*`/`to_*`/etc.; a consuming builder setter is the intended shape, so allow it here.
        #[allow(dead_code, clippy::wrong_self_convention)]
        impl #ident {
            #(#setters)*
        }
    }
}
