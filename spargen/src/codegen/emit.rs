//! Internal token builders. Each produces a deterministically-ordered fragment of the output;
//! [`generate`](super::generate) assembles and formats them.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{
    AdditionalProps, Api, ErrorShape, Field, MediaType, Operation, ParamLoc, Prim, ScalarRepr,
    ScalarValue, SuccessShape, Ty, TypeDef, TypeKind,
};
use crate::name::Names;

use super::CodegenOptions;

/// Emit the `types` (models) module for every type in the graph, in deterministic order (PRD FR3).
pub(crate) fn emit_models(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    let items = api
        .types
        .iter()
        .map(|(id, def)| emit_type_def(id, def, api, names, options));
    quote! {
        pub mod types {
            use serde::{Deserialize, Serialize};
            use std::collections::BTreeMap;

            #(#items)*
        }
    }
}

/// Emit the `Client` struct and its `new` / `with_client` constructors (PRD FR3).
pub(crate) fn emit_client(api: &Api, names: &Names) -> TokenStream {
    let params = api
        .operations
        .iter()
        .map(|operation| emit_params_struct(operation, names));
    let errors = api
        .operations
        .iter()
        .map(|operation| emit_error_enum(operation, names));
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_operation(operation, names));
    quote! {
        #(#params)*
        #(#errors)*

        pub struct Client {
            core: support::ClientCore,
        }

        impl Client {
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                Ok(Self { core: support::ClientCore::new(base_url)? })
            }

            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                Ok(Self { core: support::ClientCore::with_client(client, base_url)? })
            }

            pub fn core(&self) -> &support::ClientCore {
                &self.core
            }

            #(#methods)*
        }
    }
}

/// Emit one operation method — a thin `#[inline]` shim over the non-generic `support` dispatch
/// routines, so per-operation code stays tiny (PRD NFR2).
pub(crate) fn emit_operation(operation: &Operation, names: &Names) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let params_ident = names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated");
    let error_ident = format_ident!("{}Error", to_pascal(method_ident.as_str()));
    let reqwest_method = reqwest_method(operation.method);
    let success_ty = success_type(operation, names);
    let error_ty = quote! { #error_ident };

    let required_params = operation
        .params
        .iter()
        .filter(|param| param.required)
        .map(|param| {
            let ident = format_ident!(
                "{}",
                crate::name::escape(&param.name, crate::name::IdentRole::Param)
                    .as_str()
                    .trim_start_matches("r#")
            );
            let ty = ty_tokens(param.ty, names, &CodegenOptions::default(), true);
            quote! { #ident: #ty }
        })
        .collect::<Vec<_>>();
    let has_optional = operation.params.iter().any(|param| !param.required);
    let params_arg = has_optional.then(|| quote! { params: Option<#params_ident> });
    let body_arg = operation.request_body.as_ref().and_then(|body| {
        body.ty.map(|ty| {
            let ty = ty_tokens(ty, names, &CodegenOptions::default(), true);
            quote! { body: &#ty }
        })
    });

    let path_init = operation.path.raw.clone();
    let path_replacements = operation
        .params
        .iter()
        .filter(|param| param.location == ParamLoc::Path)
        .map(|param| {
            let placeholder = format!("{{{}}}", param.name);
            let ident = format_ident!(
                "{}",
                crate::name::escape(&param.name, crate::name::IdentRole::Param)
                    .as_str()
                    .trim_start_matches("r#")
            );
            quote! {
                path = path.replace(#placeholder, &#ident.to_string());
            }
        });
    let required_query = operation
        .params
        .iter()
        .filter(|param| param.required && param.location == ParamLoc::Query)
        .map(|param| {
            let name = param.name.clone();
            let ident = format_ident!(
                "{}",
                crate::name::escape(&param.name, crate::name::IdentRole::Param)
                    .as_str()
                    .trim_start_matches("r#")
            );
            quote! { query.push((#name, #ident.to_string())); }
        });
    let optional_query = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Query)
        .map(|param| {
            let name = param.name.clone();
            let ident = format_ident!(
                "{}",
                crate::name::escape(&param.name, crate::name::IdentRole::Field)
                    .as_str()
                    .trim_start_matches("r#")
            );
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    query.push((#name, value.to_string()));
                }
            }
        });
    let body_send = if operation
        .request_body
        .as_ref()
        .and_then(|body| body.ty)
        .is_some()
    {
        match operation
            .request_body
            .as_ref()
            .map(|body| body.media)
            .unwrap_or(MediaType::Json)
        {
            MediaType::Json | MediaType::FormUrlEncoded => {
                quote! { let request = request.json(body); }
            }
            MediaType::TextPlain | MediaType::OctetStream => {
                quote! { let request = request.body(body.to_string()); }
            }
        }
    } else {
        quote! { let request = request; }
    };
    let success_decode = match operation.responses.success() {
        SuccessShape::Unit => quote! {
            let status = response.status();
            let headers = response.headers().clone();
            Ok(support::ResponseValue::new(status, headers, ()))
        },
        _ => quote! {
            support::decode_success::<#success_ty>(&self.core, response)
                .await
                .map_err(|error| error.map_api(|never| match never {}))
        },
    };

    let args = required_params
        .into_iter()
        .chain(params_arg)
        .chain(body_arg)
        .collect::<Vec<_>>();
    quote! {
        #[inline]
        pub async fn #method_ident(
            &self,
            #(#args),*
        ) -> Result<support::ResponseValue<#success_ty>, support::Error<#error_ty>> {
            let mut path = #path_init.to_owned();
            #(#path_replacements)*
            let mut query: Vec<(&str, String)> = Vec::new();
            #(#required_query)*
            #(#optional_query)*
            let url = support::build_url(&self.core, &path, &query)
                .map_err(|error| error.map_api(|never| match never {}))?;
            let request = self.core.http().request(#reqwest_method, url);
            #body_send
            let request = request.build().map_err(support::Error::request_construction)?;
            let response = support::send(&self.core, request)
                .await
                .map_err(|error| error.map_api(|never| match never {}))?;
            if response.status().is_success() {
                #success_decode
            } else {
                Err(support::classify_error::<#error_ty>(&self.core, response).await)
            }
        }
    }
}

/// Emit an operation's optional-parameters `…Params` struct, deriving `Default` (PRD D3).
pub(crate) fn emit_params_struct(operation: &Operation, names: &Names) -> TokenStream {
    let ident = names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated");
    let fields = operation
        .params
        .iter()
        .filter(|param| !param.required)
        .map(|param| {
            let ident = format_ident!(
                "{}",
                crate::name::escape(&param.name, crate::name::IdentRole::Field)
                    .as_str()
                    .trim_start_matches("r#")
            );
            let wire = &param.name;
            let ty = ty_tokens(param.ty, names, &CodegenOptions::default(), true);
            quote! {
                #[serde(rename = #wire, skip_serializing_if = "Option::is_none")]
                pub #ident: Option<#ty>,
            }
        });
    quote! {
        #[derive(Debug, Clone, Default, serde::Serialize)]
        pub struct #ident {
            #(#fields)*
        }
    }
}

/// Emit an operation's typed error enum (or type alias for a single error body) (PRD FR3, FR5 #6).
pub(crate) fn emit_error_enum(operation: &Operation, names: &Names) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let error_ident = format_ident!("{}Error", to_pascal(method_ident.as_str()));
    let error_ty = match operation.responses.error() {
        ErrorShape::Single(ty) => ty_tokens(ty, names, &CodegenOptions::default(), true),
        ErrorShape::Enum(_) | ErrorShape::None => quote! { serde_json::Value },
    };
    quote! {
        pub type #error_ident = #error_ty;
    }
}

/// Emit the private `support` module by embedding the freestanding runtime source verbatim, under
/// `#![forbid(unsafe_code)]` (PRD §2.3 rule 3).
pub(crate) fn emit_support() -> TokenStream {
    let modules = crate::support::runtime_files().iter().map(|file| {
        let stem = file.name.trim_end_matches(".rs");
        let ident = format_ident!("{}", stem);
        let source = file.contents.replace("crate::", "super::");
        let tokens: TokenStream = source
            .parse()
            .expect("embedded support runtime parses as Rust tokens");
        quote! {
            mod #ident {
                #tokens
            }
        }
    });
    quote! {
        mod support {
            #(#modules)*

            pub use auth::{AuthError, Credential, SecretString, TokenFuture, TokenProvider};
            pub use client::{ClientConfig, ClientCore};
            pub use dispatch::{build_url, classify_error, decode_success, send};
            pub use error::{Error, ProtocolError, RedirectError, RequestError, TimeoutKind, TransportError};
            pub use response::ResponseValue;
        }
    }
}

fn emit_type_def(
    id: crate::ir::TypeId,
    def: &TypeDef,
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let ident = names.types.get(&id).expect("type name allocated");
    match &def.kind {
        TypeKind::Struct(object) => {
            let deny_unknown = matches!(object.additional, AdditionalProps::Deny)
                .then(|| quote! { #[serde(deny_unknown_fields)] });
            let fields = object
                .fields
                .iter()
                .map(|field| emit_field(id, field, names, options));
            let additional = match &object.additional {
                AdditionalProps::Typed(ty) => {
                    let ty = ty_tokens(**ty, names, options, false);
                    quote! { #[serde(flatten)] pub additional: BTreeMap<String, #ty>, }
                }
                AdditionalProps::Allow | AdditionalProps::Deny => quote! {},
            };
            quote! {
                #[derive(Debug, Clone, Serialize, Deserialize)]
                #deny_unknown
                pub struct #ident {
                    #(#fields)*
                    #additional
                }
            }
        }
        TypeKind::Enum(enumeration) if enumeration.repr == ScalarRepr::String => {
            let variants = enumeration.variants.iter().map(|variant| {
                let value = match &variant.value {
                    ScalarValue::String(value) => value,
                    _ => unreachable!("string repr has string variants"),
                };
                let ident = names
                    .variants
                    .get(&(id, value.clone()))
                    .expect("variant name allocated");
                quote! { #[serde(rename = #value)] #ident, }
            });
            quote! {
                #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
                pub enum #ident {
                    #(#variants)*
                }
            }
        }
        TypeKind::Enum(enumeration) => {
            let ty = match enumeration.repr {
                ScalarRepr::String => quote! { String },
                ScalarRepr::Int => quote! { i64 },
                ScalarRepr::Bool => quote! { bool },
            };
            quote! { pub type #ident = #ty; }
        }
        _ => {
            let ty = type_kind_tokens(&def.kind, api, names, options);
            quote! { pub type #ident = #ty; }
        }
    }
}

fn emit_field(
    id: crate::ir::TypeId,
    field: &Field,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let ident = names
        .fields
        .get(&(id, field.name.wire.clone()))
        .expect("field name allocated");
    let wire = &field.name.wire;
    let mut ty = ty_tokens(field.ty, names, options, false);
    if !field.required || field.ty.nullable {
        ty = quote! { Option<#ty> };
    }
    let default =
        (!field.required).then(|| quote! { default, skip_serializing_if = "Option::is_none", });
    quote! {
        #[serde(rename = #wire, #default)]
        pub #ident: #ty,
    }
}

fn type_kind_tokens(
    kind: &TypeKind,
    _api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    match kind {
        TypeKind::Primitive(prim) => prim_tokens(*prim, options),
        TypeKind::Map(ty) => {
            let ty = ty_tokens(**ty, names, options, false);
            quote! { BTreeMap<String, #ty> }
        }
        TypeKind::Array(ty) => {
            let ty = ty_tokens(**ty, names, options, false);
            quote! { Vec<#ty> }
        }
        TypeKind::Tuple(items) => {
            let items = items.iter().map(|ty| ty_tokens(*ty, names, options, false));
            quote! { (#(#items),*) }
        }
        TypeKind::Bytes => quote! { bytes::Bytes },
        TypeKind::Any | TypeKind::Union(_) => quote! { serde_json::Value },
        TypeKind::Struct(_) | TypeKind::Enum(_) => {
            unreachable!("named definitions emitted separately")
        }
    }
}

fn ty_tokens(ty: Ty, names: &Names, _options: &CodegenOptions, qualified: bool) -> TokenStream {
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

fn prim_tokens(prim: Prim, options: &CodegenOptions) -> TokenStream {
    match prim {
        Prim::Bool => quote! { bool },
        Prim::String => quote! { String },
        Prim::I32 => quote! { i32 },
        Prim::I64 => quote! { i64 },
        Prim::F64 => quote! { f64 },
        Prim::Uuid if options.feature_uuid => quote! { uuid::Uuid },
        Prim::DateTime if options.feature_time => quote! { time::OffsetDateTime },
        Prim::Date if options.feature_time => quote! { time::Date },
        Prim::Uuid | Prim::DateTime | Prim::Date => quote! { String },
    }
}

fn success_type(operation: &Operation, names: &Names) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Unit => quote! { () },
        SuccessShape::Plain(ty) => ty_tokens(ty, names, &CodegenOptions::default(), true),
        SuccessShape::Enum(_) => quote! { serde_json::Value },
    }
}

fn reqwest_method(method: crate::ir::Method) -> TokenStream {
    match method {
        crate::ir::Method::Get => quote! { reqwest::Method::GET },
        crate::ir::Method::Put => quote! { reqwest::Method::PUT },
        crate::ir::Method::Post => quote! { reqwest::Method::POST },
        crate::ir::Method::Delete => quote! { reqwest::Method::DELETE },
        crate::ir::Method::Options => quote! { reqwest::Method::OPTIONS },
        crate::ir::Method::Head => quote! { reqwest::Method::HEAD },
        crate::ir::Method::Patch => quote! { reqwest::Method::PATCH },
        crate::ir::Method::Trace => quote! { reqwest::Method::TRACE },
    }
}

fn to_pascal(value: &str) -> String {
    crate::name::to_pascal_case(value.trim_start_matches("r#"))
}
