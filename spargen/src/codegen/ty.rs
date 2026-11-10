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
        Some(TypeKind::Bytes(_))
    )
}

/// The Rust type of a primitive, under the `uuid` and `time` mappings `options` selects.
pub(super) fn prim_tokens(prim: Prim, options: &CodegenOptions) -> TokenStream {
    match prim {
        Prim::Bool => quote! { bool },
        Prim::String => quote! { String },
        Prim::I32 => quote! { i32 },
        Prim::I64 => quote! { i64 },
        Prim::U8 => quote! { u8 },
        Prim::U16 => quote! { u16 },
        Prim::U32 => quote! { u32 },
        Prim::U64 => quote! { u64 },
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
    use crate::diag::{JsonPointer, Provenance};
    use crate::ir::{Api, Info, Method, Prim, Ty, TypeDef, TypeGraph, TypeId, TypeKind};
    use crate::name::{Ident, Names};

    use super::CodegenOptions;

    /// An API with no operations whose types are `kinds`, in order, each named `T<index>`.
    fn api_with(kinds: Vec<TypeKind>) -> (Api, Names, Vec<TypeId>) {
        let mut types = TypeGraph::default();
        let mut names = Names::default();
        let ids = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                let id = types.insert(TypeDef {
                    name_hint: format!("T{index}"),
                    kind,
                    docs: Default::default(),
                    provenance: Provenance::new(JsonPointer::root(), None),
                    document: String::new(),
                });
                names.types.insert(id, Ident::new(format!("T{index}")));
                id
            })
            .collect();
        let api = Api {
            info: Info {
                title: "T".to_owned(),
                version: "1.0.0".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: Vec::new(),
            types,
            security_schemes: Default::default(),
        };
        (api, names, ids)
    }

    fn ty(id: TypeId, nullable: bool, boxed: bool) -> Ty {
        Ty {
            id,
            nullable,
            boxed,
        }
    }

    /// `ty_tokens` qualifies, boxes and wraps in `Option` as the `Ty` says, and `boxed_ty_tokens`
    /// boxes whatever the `Ty` says, without boxing twice.
    #[test]
    fn a_type_is_named_qualified_boxed_and_optional_as_its_ty_says() {
        let (_, names, ids) = api_with(vec![TypeKind::Primitive(Prim::String)]);
        let id = ids[0];
        for (ty, qualified, plain, boxed) in [
            (ty(id, false, false), false, "T0", "Box < T0 >"),
            (
                ty(id, false, false),
                true,
                "types :: T0",
                "Box < types :: T0 >",
            ),
            (ty(id, false, true), false, "Box < T0 >", "Box < T0 >"),
            (
                ty(id, true, false),
                true,
                "Option < types :: T0 >",
                "Option < Box < types :: T0 > >",
            ),
            (
                ty(id, true, true),
                false,
                "Option < Box < T0 > >",
                "Option < Box < T0 > >",
            ),
        ] {
            assert_eq!(
                super::ty_tokens(ty, &names, qualified).to_string(),
                plain,
                "{ty:?}"
            );
            assert_eq!(
                super::boxed_ty_tokens(ty, &names, qualified).to_string(),
                boxed,
                "{ty:?}"
            );
        }
    }

    /// Only a `Bytes` definition takes the raw byte codec; a type the graph does not hold does not.
    #[test]
    fn only_a_bytes_definition_is_a_bytes_body() {
        let (api, _, ids) = api_with(vec![
            TypeKind::Bytes(crate::ir::BytesEncoding::Raw),
            TypeKind::Primitive(Prim::String),
        ]);
        assert!(super::is_bytes_ty(&api, ty(ids[0], false, false)));
        assert!(!super::is_bytes_ty(&api, ty(ids[1], false, false)));
        assert!(!super::is_bytes_ty(&api, ty(TypeId(99), false, false)));
    }

    /// The `uuid` and `time` mappings each switch only their own primitives, to the type they
    /// name or back to `String`; every other primitive names itself under any mapping.
    #[test]
    fn prim_tokens_follow_the_uuid_and_time_mappings() {
        for (feature_uuid, feature_time) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let options = CodegenOptions {
                feature_uuid,
                feature_time,
                ..CodegenOptions::default()
            };
            let tokens = |prim| super::prim_tokens(prim, &options).to_string();
            let case = format!("uuid {feature_uuid}, time {feature_time}");
            assert_eq!(tokens(Prim::Bool), "bool", "{case}");
            assert_eq!(tokens(Prim::String), "String", "{case}");
            assert_eq!(tokens(Prim::I32), "i32", "{case}");
            assert_eq!(tokens(Prim::I64), "i64", "{case}");
            assert_eq!(tokens(Prim::U8), "u8", "{case}");
            assert_eq!(tokens(Prim::U16), "u16", "{case}");
            assert_eq!(tokens(Prim::U32), "u32", "{case}");
            assert_eq!(tokens(Prim::U64), "u64", "{case}");
            assert_eq!(tokens(Prim::F64), "f64", "{case}");
            assert_eq!(
                tokens(Prim::Uuid),
                if feature_uuid {
                    "uuid :: Uuid"
                } else {
                    "String"
                },
                "{case}"
            );
            assert_eq!(
                tokens(Prim::DateTime),
                if feature_time { "DateTime" } else { "String" },
                "{case}"
            );
            assert_eq!(
                tokens(Prim::Date),
                if feature_time { "Date" } else { "String" },
                "{case}"
            );
        }
    }

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
