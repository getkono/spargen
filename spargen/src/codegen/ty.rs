//! The Rust type a lowered [`Ty`] or [`Prim`] names, whether a body takes the raw byte codec, and
//! the `reqwest::Method` an operation sends.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, Prim, Ty, TypeKind};
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

/// [`ty_tokens`] with the payload always boxed: the type a union variant or a multi-status
/// response variant carries. Those payloads are uniformly indirect so an API's largest object
/// variant cannot inflate every value of the enum (or trip strict `large_enum_variant` linting);
/// single-body response aliases stay allocation-free. Recursive boxing is a boolean
/// representation flag, so setting it again never produces `Box<Box<T>>`.
pub(super) fn boxed_ty_tokens(ty: Ty, names: &Names, qualified: bool) -> TokenStream {
    ty_tokens(Ty { boxed: true, ..ty }, names, qualified)
}

/// Whether a body is decoded by the raw byte codec, which yields `bytes::Bytes` itself. It reads
/// only the definition's kind, which is sound because `LowerCtx::lower_response` (in
/// `oas31/lower/response.rs`) refuses a nullable `Bytes` body (`E009`): no `Option<bytes::Bytes>`
/// body reaches these decode sites.
pub(super) fn is_bytes_ty(api: &Api, ty: Ty) -> bool {
    matches!(
        api.types.get(ty.id).map(|definition| &definition.kind),
        Some(TypeKind::Bytes)
    )
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
///
/// A token-built method applies `?` to its `from_bytes` result, so the expression is spliced only
/// into a body returning `Result<_, support::Error<_>>`.
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
        // token bytes. `QUERY`, like a validated `additionalOperations` key, is a valid HTTP
        // method token, so `from_bytes` never fails here; its error still maps to a
        // request-construction error rather than an `expect`, so generated code passes
        // `clippy::expect_used`.
        crate::ir::Method::Query => from_token_bytes(&proc_macro2::Literal::byte_string(b"QUERY")),
        crate::ir::Method::Custom(method) => {
            from_token_bytes(&proc_macro2::Literal::byte_string(method.as_bytes()))
        }
    }
}

/// A method built from its token bytes, an `InvalidMethod` mapped through `?`.
fn from_token_bytes(bytes: &proc_macro2::Literal) -> TokenStream {
    quote! {
        reqwest::Method::from_bytes(#bytes).map_err(support::Error::request_construction)?
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::Method;

    /// Each fixed method maps to its `reqwest::Method` constant; `QUERY`, which has none, and an
    /// `additionalOperations` token are built from their exact bytes, the error mapped through `?`
    /// rather than an `expect` or `unwrap`.
    #[test]
    fn reqwest_method_follows_the_table() {
        let custom = |token: &str| Method::Custom(token.to_owned());
        let from_bytes = |token: &str| {
            format!(
                "reqwest :: Method :: from_bytes (b\"{token}\") \
                 . map_err (support :: Error :: request_construction) ?"
            )
        };
        for (method, expected) in [
            (Method::Get, "reqwest :: Method :: GET".to_owned()),
            (Method::Put, "reqwest :: Method :: PUT".to_owned()),
            (Method::Post, "reqwest :: Method :: POST".to_owned()),
            (Method::Delete, "reqwest :: Method :: DELETE".to_owned()),
            (Method::Options, "reqwest :: Method :: OPTIONS".to_owned()),
            (Method::Head, "reqwest :: Method :: HEAD".to_owned()),
            (Method::Patch, "reqwest :: Method :: PATCH".to_owned()),
            (Method::Trace, "reqwest :: Method :: TRACE".to_owned()),
            (Method::Query, from_bytes("QUERY")),
            (custom("PURGE"), from_bytes("PURGE")),
            (custom("copy"), from_bytes("copy")),
        ] {
            assert_eq!(
                super::reqwest_method(&method).to_string(),
                expected,
                "{method:?}"
            );
        }
    }
}
