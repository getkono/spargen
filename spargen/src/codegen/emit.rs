//! Internal token builders. Each produces a deterministically-ordered fragment of the output;
//! [`generate`](super::generate) assembles and formats them.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, ErrorShape, Field, HttpScheme, MediaType, Operation, ParamLoc,
    Prim, ScalarRepr, ScalarValue, SecurityScheme, SuccessShape, Ty, TypeDef, TypeKind,
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
pub(crate) fn emit_client(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    let params = api
        .operations
        .iter()
        .map(|operation| emit_params_struct(operation, names, options));
    let errors = api
        .operations
        .iter()
        .map(|operation| emit_error_enum(operation, names, options));
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_operation(operation, api, names, options));
    let client_docs = client_doc_tokens(api);
    let error_body_cap = options.error_body_cap;
    quote! {
        #(#params)*
        #(#errors)*

        #client_docs
        pub struct Client {
            core: support::ClientCore,
        }

        impl Client {
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                Self::with_client(reqwest::Client::new(), base_url)
            }

            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_client(client, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            pub fn core(&self) -> &support::ClientCore {
                &self.core
            }

            /// Register a credential for a named security scheme. Operations whose `security`
            /// requirement cannot be satisfied by the registered credentials fail with a
            /// request-construction error before anything is sent.
            #[must_use]
            pub fn with_credential(
                mut self,
                scheme: &str,
                credential: support::Credential,
            ) -> Self {
                self.core.set_credential(scheme, credential);
                self
            }

            #(#methods)*
        }
    }
}

/// Emit one operation method — a thin `#[inline]` shim over the non-generic `support` dispatch
/// routines, so per-operation code stays tiny (PRD NFR2).
pub(crate) fn emit_operation(
    operation: &Operation,
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
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
    let success_ty = success_type(operation, names, options);
    let error_ty = quote! { #error_ident };
    let docs = doc_tokens(&operation.docs);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });

    let required_params = operation
        .params
        .iter()
        .filter(|param| param.required)
        .map(|param| {
            let ident = param_ident(param, crate::name::IdentRole::Param);
            let ty = ty_tokens(param.ty, names, options, true);
            quote! { #ident: #ty }
        })
        .collect::<Vec<_>>();
    let has_optional = operation.params.iter().any(|param| !param.required);
    let params_arg = has_optional.then(|| quote! { params: Option<#params_ident> });
    let body_arg = operation.request_body.as_ref().and_then(|body| {
        body.ty.map(|ty| {
            let ty = ty_tokens(ty, names, options, true);
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
            let ident = param_ident(param, crate::name::IdentRole::Param);
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
            let ident = param_ident(param, crate::name::IdentRole::Param);
            let value = param_value_tokens(param, api, options, quote! { #ident });
            quote! { query.push((#name, #value)); }
        });
    let optional_query = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Query)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let value = param_value_tokens(param, api, options, quote! { value });
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    query.push((#name, #value));
                }
            }
        });
    let required_headers = operation
        .params
        .iter()
        .filter(|param| param.required && param.location == ParamLoc::Header)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Param);
            let value = param_value_tokens(param, api, options, quote! { #ident });
            quote! { request = request.header(#name, #value); }
        });
    let optional_headers = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Header)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let value = param_value_tokens(param, api, options, quote! { value });
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    request = request.header(#name, #value);
                }
            }
        });
    let has_cookies = operation
        .params
        .iter()
        .any(|param| param.location == ParamLoc::Cookie);
    let cookie_init = has_cookies.then(|| quote! { let mut cookies: Vec<String> = Vec::new(); });
    let required_cookies = operation
        .params
        .iter()
        .filter(|param| param.required && param.location == ParamLoc::Cookie)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Param);
            let value = param_value_tokens(param, api, options, quote! { #ident });
            quote! { cookies.push(format!("{}={}", #name, #value)); }
        });
    let optional_cookies = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Cookie)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let value = param_value_tokens(param, api, options, quote! { value });
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    cookies.push(format!("{}={}", #name, #value));
                }
            }
        });
    let cookie_attach = has_cookies.then(|| {
        quote! {
            if !cookies.is_empty() {
                request = request.header(reqwest::header::COOKIE, cookies.join("; "));
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
            MediaType::Json => quote! { request = request.json(body); },
            MediaType::FormUrlEncoded => quote! { request = request.form(body); },
            MediaType::TextPlain => quote! { request = request.body(body.to_string()); },
            MediaType::OctetStream => quote! { request = request.body(body.clone()); },
        }
    } else {
        quote! {}
    };
    let attach_auth = if operation.security.is_empty() {
        quote! {}
    } else {
        let alternatives = operation.security.iter().map(|requirement| {
            let schemes = requirement.0.iter().map(|(id, _scopes)| {
                let scheme = api
                    .security_schemes
                    .get(id)
                    .expect("security scheme validated during lowering");
                let name = &id.0;
                let kind = match scheme {
                    // Caller-supplied oauth2/oidc tokens attach as bearer credentials.
                    SecurityScheme::Http(HttpScheme::Bearer)
                    | SecurityScheme::OAuth2
                    | SecurityScheme::OpenIdConnect => quote! { support::AuthKind::Bearer },
                    SecurityScheme::Http(HttpScheme::Basic) => quote! { support::AuthKind::Basic },
                    SecurityScheme::ApiKey { location, name } => match location {
                        ApiKeyLoc::Header => quote! { support::AuthKind::ApiKeyHeader(#name) },
                        ApiKeyLoc::Query => quote! { support::AuthKind::ApiKeyQuery(#name) },
                        ApiKeyLoc::Cookie => quote! { support::AuthKind::ApiKeyCookie(#name) },
                    },
                };
                quote! { support::AuthScheme { name: #name, kind: #kind } }
            });
            quote! { &[#(#schemes),*][..] }
        });
        quote! {
            request = support::attach_auth(&self.core, request, &[#(#alternatives),*])
                .await
                .map_err(support::Error::widen)?;
        }
    };
    let error_shape = operation.responses.error();
    let error_branch = match &error_shape {
        ErrorShape::None => quote! {
            Err(support::unexpected_status::<#error_ty>(&self.core, response).await)
        },
        _ => {
            let mut documented = operation
                .responses
                .by_status
                .iter()
                .filter(|(status, response)| !status.is_success() && response.body.is_some())
                .map(|(status, _)| match status {
                    crate::ir::StatusSpec::Exact(code) => {
                        quote! { support::StatusSpec::Exact(#code) }
                    }
                    crate::ir::StatusSpec::Range(prefix) => {
                        quote! { support::StatusSpec::Range(#prefix) }
                    }
                })
                .collect::<Vec<_>>();
            if operation
                .responses
                .default
                .as_ref()
                .is_some_and(|default| default.body.is_some())
            {
                documented.push(quote! { support::StatusSpec::Any });
            }
            quote! {
                Err(
                    support::classify_error::<#error_ty>(
                        &self.core,
                        response,
                        &[#(#documented),*],
                    )
                    .await,
                )
            }
        }
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
                .map_err(support::Error::widen)
        },
    };

    let args = required_params
        .into_iter()
        .chain(params_arg)
        .chain(body_arg)
        .collect::<Vec<_>>();
    quote! {
        #docs
        #deprecated
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
                .map_err(support::Error::widen)?;
            let mut request = self.core.http().request(#reqwest_method, url);
            #(#required_headers)*
            #(#optional_headers)*
            #cookie_init
            #(#required_cookies)*
            #(#optional_cookies)*
            #cookie_attach
            #body_send
            #attach_auth
            let request = request.build().map_err(support::Error::request_construction)?;
            let response = support::send(&self.core, request)
                .await
                .map_err(support::Error::widen)?;
            if response.status().is_success() {
                #success_decode
            } else {
                #error_branch
            }
        }
    }
}

fn param_ident(param: &crate::ir::Parameter, role: crate::name::IdentRole) -> proc_macro2::Ident {
    format_ident!(
        "{}",
        crate::name::escape(&param.name, role)
            .as_str()
            .trim_start_matches("r#")
    )
}

/// Render a parameter value expression to its wire string: JSON for `content`-typed parameters,
/// RFC 3339 for `time` types, `Display` otherwise.
fn param_value_tokens(
    param: &crate::ir::Parameter,
    api: &Api,
    options: &CodegenOptions,
    value: TokenStream,
) -> TokenStream {
    if let crate::ir::ParamStyle::Content(media) = &param.style {
        return match media {
            MediaType::Json => quote! {
                serde_json::to_string(&#value).map_err(support::Error::request_construction)?
            },
            _ => quote! { #value.to_string() },
        };
    }
    let kind = api.types.get(param.ty.id).map(|def| &def.kind);
    match kind {
        Some(TypeKind::Primitive(Prim::DateTime | Prim::Date)) if options.feature_time => quote! {
            #value
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(support::Error::request_construction)?
        },
        _ => quote! { #value.to_string() },
    }
}

/// Turn lowered documentation into `#[doc = …]` attributes so IDE hover shows the API docs
/// (PRD FR3).
fn doc_tokens(docs: &crate::ir::Docs) -> TokenStream {
    let mut paragraphs: Vec<&str> = Vec::new();
    if let Some(summary) = &docs.summary {
        paragraphs.push(summary);
    }
    if let Some(description) = &docs.description {
        if docs.summary.as_deref() != Some(description.as_str()) {
            paragraphs.push(description);
        }
    }
    if paragraphs.is_empty() {
        if let Some(title) = &docs.title {
            paragraphs.push(title);
        }
    }
    if paragraphs.is_empty() {
        return quote! {};
    }
    let text = paragraphs.join("\n\n");
    quote! { #[doc = #text] }
}

/// Document the generated `Client` with the API identity and its declared servers.
fn client_doc_tokens(api: &Api) -> TokenStream {
    let mut text = format!("Client for {} v{}.", api.info.title, api.info.version);
    if let Some(description) = &api.info.description {
        text.push_str("\n\n");
        text.push_str(description);
    }
    if !api.servers.is_empty() {
        text.push_str("\n\nServers declared by the spec:");
        for server in &api.servers {
            text.push_str("\n- `");
            text.push_str(&server.url);
            text.push('`');
            if let Some(description) = &server.description {
                text.push_str(" — ");
                text.push_str(description);
            }
        }
    }
    quote! { #[doc = #text] }
}

/// Emit an operation's optional-parameters `…Params` struct, deriving `Default` (PRD D3).
pub(crate) fn emit_params_struct(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
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
            let ty = ty_tokens(param.ty, names, options, true);
            let note = param
                .deprecated
                .then(|| quote! { #[doc = "Deprecated per the spec."] });
            quote! {
                #note
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
pub(crate) fn emit_error_enum(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let error_ident = format_ident!("{}Error", to_pascal(method_ident.as_str()));
    let error_ty = match operation.responses.error() {
        ErrorShape::Single(ty) => ty_tokens(ty, names, options, true),
        // Multiple documented error bodies fall back to Value (reported as W003 by `generate`).
        ErrorShape::Enum => quote! { serde_json::Value },
        // No documented error body: every non-success status is Error::UnexpectedStatus, and the
        // uninhabited alias makes Error::Api impossible to construct.
        ErrorShape::None => quote! { std::convert::Infallible },
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
        // Each runtime file keeps its `#[cfg(test)]` module last; strip it at embed time — the
        // runtime is tested in the support-runtime crate, and test-only `crate::` imports would
        // not survive the module renesting.
        let source = file
            .contents
            .split("#[cfg(test)]")
            .next()
            .expect("split yields at least one part")
            .replace("crate::", "super::");
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

            pub use auth::{AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture, TokenProvider};
            pub use client::{ClientConfig, ClientCore};
            pub use dispatch::{attach_auth, build_url, classify_error, decode_success, send, unexpected_status, StatusSpec};
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
    let docs = doc_tokens(&def.docs);
    let deprecated = def.docs.deprecated.then(|| quote! { #[deprecated] });
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
                #docs
                #deprecated
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
                let value = match variant {
                    ScalarValue::String(value) => value,
                    _ => unreachable!("string repr has string variants"),
                };
                let ident = names
                    .variants
                    .get(&(id, value.clone()))
                    .expect("variant name allocated");
                quote! { #[serde(rename = #value)] #ident, }
            });
            let display_arms = enumeration.variants.iter().map(|variant| {
                let value = match variant {
                    ScalarValue::String(value) => value,
                    _ => unreachable!("string repr has string variants"),
                };
                let variant_ident = names
                    .variants
                    .get(&(id, value.clone()))
                    .expect("variant name allocated");
                quote! { #ident::#variant_ident => #value, }
            });
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
                pub enum #ident {
                    #(#variants)*
                }

                impl std::fmt::Display for #ident {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(match self {
                            #(#display_arms)*
                        })
                    }
                }
            }
        }
        TypeKind::Enum(enumeration) => {
            let ty = match enumeration.repr {
                ScalarRepr::String => quote! { String },
                ScalarRepr::Int => quote! { i64 },
                ScalarRepr::Bool => quote! { bool },
            };
            quote! { #docs pub type #ident = #ty; }
        }
        _ => {
            let ty = type_kind_tokens(&def.kind, api, names, options);
            quote! { #docs pub type #ident = #ty; }
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
    let mut notes: Vec<&str> = Vec::new();
    if field.deprecated {
        notes.push("Deprecated per the spec.");
    }
    if field.read_only {
        notes.push("Read-only: set by the server; ignored in requests.");
    }
    if field.write_only {
        notes.push("Write-only: sent in requests; absent from responses.");
    }
    let notes = notes.iter().map(|note| quote! { #[doc = #note] });
    quote! {
        #(#notes)*
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
        TypeKind::Array(ty) => {
            let ty = ty_tokens(**ty, names, options, false);
            quote! { Vec<#ty> }
        }
        TypeKind::Tuple(items) => {
            let items = items.iter().map(|ty| ty_tokens(*ty, names, options, false));
            quote! { (#(#items),*) }
        }
        TypeKind::Bytes => quote! { bytes::Bytes },
        TypeKind::Any => quote! { serde_json::Value },
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

fn success_type(operation: &Operation, names: &Names, options: &CodegenOptions) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Unit => quote! { () },
        SuccessShape::Plain(ty) => ty_tokens(ty, names, options, true),
        SuccessShape::Enum => quote! { serde_json::Value },
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
