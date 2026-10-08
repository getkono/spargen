//! The Rust type a lowered [`Ty`] or [`Prim`] names, and the `reqwest::Method` an operation sends.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Prim, Ty};
use crate::name::Names;

use super::CodegenOptions;

/// The type `ty` names: its allocated identifier, `types::`-qualified when `qualified` (every
/// position outside the `types` module), wrapped in `Box` when boxed and `Option` when nullable.
pub(super) fn ty_tokens(ty: Ty, names: &Names, qualified: bool) -> TokenStream {
    let ident = names.types.get(&ty.id).expect("type name allocated");
    let mut tokens = if qualified {
        quote! { types::#ident }
    } else {
        quote! { #ident }
    };
    if ty.boxed {
        tokens = quote! { Box<#tokens> };
    }
    if ty.nullable {
        tokens = quote! { Option<#tokens> };
    }
    tokens
}

/// The Rust type of a primitive, under the `uuid` and `time` mappings `options` selects.
pub(super) fn prim_tokens(prim: Prim, options: &CodegenOptions) -> TokenStream {
    match prim {
        Prim::Bool => quote! { bool },
        Prim::String => quote! { String },
        Prim::I32 => quote! { i32 },
        Prim::I64 => quote! { i64 },
        Prim::F64 => quote! { f64 },
        Prim::Uuid if options.feature_uuid => quote! { uuid::Uuid },
        // The embedded newtypes, not `time`'s own types: OpenAPI fixes these to RFC 3339, which is
        // not what `time`'s `Serialize`/`Display` produce. Named bare so one spelling works at the
        // generated root (via the prelude re-export) and inside `types` (via its `use super::`).
        Prim::DateTime if options.feature_time => quote! { DateTime },
        Prim::Date if options.feature_time => quote! { Date },
        Prim::Uuid | Prim::DateTime | Prim::Date => quote! { String },
    }
}

/// The `reqwest::Method` an operation's request is built with.
pub(super) fn reqwest_method(method: &crate::ir::Method) -> TokenStream {
    match method {
        crate::ir::Method::Get => quote! { reqwest::Method::GET },
        crate::ir::Method::Put => quote! { reqwest::Method::PUT },
        crate::ir::Method::Post => quote! { reqwest::Method::POST },
        crate::ir::Method::Delete => quote! { reqwest::Method::DELETE },
        crate::ir::Method::Options => quote! { reqwest::Method::OPTIONS },
        crate::ir::Method::Head => quote! { reqwest::Method::HEAD },
        crate::ir::Method::Patch => quote! { reqwest::Method::PATCH },
        crate::ir::Method::Trace => quote! { reqwest::Method::TRACE },
        // reqwest has no `QUERY` constant (OpenAPI 3.2's new fixed method), so build it from the
        // token bytes. `QUERY` is a valid HTTP method token, so `from_bytes` never fails here.
        crate::ir::Method::Query => quote! {
            reqwest::Method::from_bytes(b"QUERY").expect("QUERY is a valid HTTP method token")
        },
        crate::ir::Method::Custom(method) => {
            let bytes = proc_macro2::Literal::byte_string(method.as_bytes());
            quote! {
                reqwest::Method::from_bytes(#bytes)
                    .expect("validated additionalOperations key is a valid HTTP method token")
            }
        }
    }
}
