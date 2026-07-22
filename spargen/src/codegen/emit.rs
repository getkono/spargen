//! Internal token builders. Each produces a deterministically-ordered fragment of the output;
//! [`generate`](super::generate) assembles and formats them.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, DisjointFeature, ErrorShape, Field, HttpScheme, JsonCategory,
    MediaType, Operation, ParamLoc, Prim, ScalarRepr, ScalarValue, SecurityScheme, SuccessShape,
    Ty, TypeDef, TypeKind, UnionMode, UnionStrategy,
};
use crate::name::Names;

use super::CodegenOptions;

/// Emit the `types` (models) module for every type in the graph, in deterministic order.
pub(crate) fn emit_models(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    let items = api
        .types
        .iter()
        .map(|(id, def)| emit_type_def(id, def, api, names, options));
    quote! {
        #[forbid(unsafe_code)]
        #[allow(dead_code, unused_imports)]
        pub mod types {
            use serde::{Deserialize, Serialize};
            use std::collections::BTreeMap;

            #(#items)*
        }
    }
}

/// Emit the `Client` struct and its `new` / `with_client` constructors.
pub(crate) fn emit_client(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    let params = api
        .operations
        .iter()
        .filter(|operation| operation.params.iter().any(|param| !param.required))
        .map(|operation| emit_params_struct(operation, names, options));
    let errors = api
        .operations
        .iter()
        .map(|operation| emit_error_enum(operation, names, options));
    let response_enums = api
        .operations
        .iter()
        .map(|operation| emit_response_enum(operation, names, options));
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_operation(operation, api, names, options));
    let client_docs = client_doc_tokens(api);
    let error_body_cap = options.error_body_cap;
    quote! {
        #(#params)*
        #(#errors)*
        #(#response_enums)*

        #client_docs
        #[allow(dead_code)]
        pub struct Client {
            core: support::ClientCore,
        }

        #[forbid(unsafe_code)]
        #[allow(dead_code, unused_mut, unused_variables, clippy::result_large_err)]
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

            /// Build a client over a caller-supplied transport backend — the injection point for
            /// retry, middleware, or a non-reqwest transport. Requests are still built on a default
            /// `reqwest::Client`; only the execute step goes through the backend.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_backend(backend, base_url)?;
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
/// routines, so per-operation code stays tiny.
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
    let error_ident = format_ident!("{}Error", to_pascal(method_ident.as_str()));
    let reqwest_method = reqwest_method(operation.method);
    let success_ty = success_type(operation, names, options);
    let error_ty = quote! { #error_ident };
    let docs = doc_tokens(&operation.docs);
    // Required parameters are positional method arguments with no attribute slot of their own, so a
    // required parameter's `default` is surfaced in the method rustdoc instead.
    let param_default_docs = param_default_docs_tokens(operation);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });

    // The typed argument list (required params, the optional-params struct, the body) is shared
    // verbatim with the blocking shim so the two signatures can never drift.
    let (args, _arg_names) = operation_args(operation, names, options);

    let path_init = operation.path.raw.clone();
    let path_replacements = operation
        .params
        .iter()
        .filter(|param| param.location == ParamLoc::Path)
        .map(|param| {
            let placeholder = format!("{{{}}}", param.name);
            let ident = param_ident(param, crate::name::IdentRole::Param);
            let value = param_value_tokens(param, quote! { &#ident });
            quote! {
                path = path.replace(#placeholder, &#value);
            }
        });
    let required_query = operation
        .params
        .iter()
        .filter(|param| param.required && param.location == ParamLoc::Query)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Param);
            query_param_tokens(param, &name, quote! { &#ident })
        });
    let optional_query = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Query)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let serialize = query_param_tokens(param, &name, quote! { value });
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    #serialize
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
            let value = param_value_tokens(param, quote! { &#ident });
            quote! { request = request.header(#name, #value); }
        });
    let optional_headers = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Header)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let value = param_value_tokens(param, quote! { value });
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
            cookie_param_tokens(param, &name, quote! { &#ident })
        });
    let optional_cookies = operation
        .params
        .iter()
        .filter(|param| !param.required && param.location == ParamLoc::Cookie)
        .map(|param| {
            let name = param.name.clone();
            let ident = param_ident(param, crate::name::IdentRole::Field);
            let serialize = cookie_param_tokens(param, &name, quote! { value });
            quote! {
                if let Some(value) = params.as_ref().and_then(|params| params.#ident.as_ref()) {
                    #serialize
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
    let body_send = if let Some((ty, media)) = operation
        .request_body
        .as_ref()
        .and_then(|body| body.ty.map(|ty| (ty, body.media)))
    {
        // A raw byte body (`bytes::Bytes`, from `format: binary` / `contentEncoding: base64`) is sent
        // as-is regardless of the declared media — `Bytes` is not `Display`, so it can never go
        // through `.to_string()`. This must be checked before the media match so a `text/plain` (or
        // any) media over a `Bytes` schema does not miscompile.
        if matches!(
            api.types.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Bytes)
        ) {
            quote! { request = request.body(body.clone()); }
        } else {
            match media {
                MediaType::Json => quote! { request = request.json(body); },
                // XML: serialize the typed body to an XML string via the runtime's quick-xml helper
                // and set it as the body with the XML content-type. `to_xml` yields
                // `Error<Infallible>`, widened to the operation's error type.
                MediaType::Xml => quote! {
                    let body = support::to_xml(body).map_err(support::Error::widen)?;
                    request = request
                        .header(reqwest::header::CONTENT_TYPE, "application/xml")
                        .body(body);
                },
                MediaType::FormUrlEncoded => quote! { request = request.form(body); },
                MediaType::TextPlain => quote! { request = request.body(body.to_string()); },
                MediaType::OctetStream => quote! { request = request.body(body.clone()); },
                MediaType::Multipart => emit_multipart_body(ty, api, names),
                // Streaming media are response-only; a streaming request body is rejected during
                // lowering (narrowed `E009`), so this arm is unreachable for any emitted operation.
                MediaType::EventStream | MediaType::Ndjson => quote! {},
            }
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
        // A single documented error body: classify against the documented status table into the
        // aliased `E` (or `Error::UnexpectedStatus` for an undocumented status).
        ErrorShape::Single(_) => {
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
            // An XML error body classifies through the quick-xml runtime helper
            // (`classify_error_xml`); every other media classifies as JSON.
            let classify = if operation.responses.single_error_media() == Some(MediaType::Xml) {
                quote! { support::classify_error_xml }
            } else {
                quote! { support::classify_error }
            };
            quote! {
                Err(
                    #classify::<#error_ty>(
                        &self.core,
                        response,
                        &[#(#documented),*],
                    )
                    .await,
                )
            }
        }
        // Multiple documented error bodies: read the capped body once, then dispatch by status in
        // precedence order (exact before range before default) into the matching enum variant →
        // `Error::Api`; a parse failure → `Error::Decode`; an undocumented status →
        // `Error::UnexpectedStatus` (capped body preserved either way).
        ErrorShape::Enum(entries) => {
            let arms = entries.iter().map(|(spec, ty)| {
                let spec_tokens = runtime_status_spec(*spec);
                let variant_ident = status_variant_ident(*spec);
                match ty {
                    // Bodied status: decode into the variant's type → `Api`, or `Decode` on failure.
                    Some(ty) => {
                        let ty = ty_tokens(*ty, names, options, true);
                        quote! {
                            if #spec_tokens.matches(status) {
                                return Err(match serde_json::from_slice::<#ty>(&body) {
                                    Ok(value) => support::Error::Api(support::ResponseValue::new(
                                        status,
                                        headers,
                                        #error_ident::#variant_ident(value),
                                    )),
                                    Err(error) => support::Error::Decode {
                                        path: error.to_string(),
                                        body,
                                        truncated,
                                    },
                                });
                            }
                        }
                    }
                    // Documented bodyless error status: the unit variant → `Api`, no body parse.
                    None => quote! {
                        if #spec_tokens.matches(status) {
                            return Err(support::Error::Api(support::ResponseValue::new(
                                status,
                                headers,
                                #error_ident::#variant_ident,
                            )));
                        }
                    },
                }
            });
            quote! {
                let (status, headers, body, truncated) =
                    match support::read_error_body::<#error_ty>(&self.core, response).await {
                        Ok(parts) => parts,
                        Err(error) => return Err(error),
                    };
                #(#arms)*
                Err(support::Error::UnexpectedStatus { status, headers, body })
            }
        }
    };
    let success_decode = match operation.responses.success() {
        SuccessShape::Unit => quote! {
            let status = response.status();
            let headers = response.headers().clone();
            Ok(support::ResponseValue::new(status, headers, ()))
        },
        // A single success body: decode into the aliased `T`. An XML body routes through the
        // quick-xml runtime helper (`decode_success_xml`); every other media decodes as JSON.
        SuccessShape::Plain(_)
            if operation.responses.single_success_media() == Some(MediaType::Xml) =>
        {
            quote! {
                support::decode_success_xml::<#success_ty>(&self.core, response)
                    .await
                    .map_err(support::Error::widen)
            }
        }
        SuccessShape::Plain(_) => quote! {
            support::decode_success::<#success_ty>(&self.core, response)
                .await
                .map_err(support::Error::widen)
        },
        // Multi-status success: read the body once, then dispatch by status in precedence order
        // (exact before range before default) into the matching variant. A success status matching
        // no documented variant is an unexpected-status error — there is no untyped fallback.
        SuccessShape::Enum(entries) => {
            let method_ident = names
                .operations
                .get(&operation.id)
                .expect("operation name allocated");
            let enum_ident = success_enum_ident(method_ident);
            let arms = entries.iter().map(|(spec, ty)| {
                let spec_tokens = runtime_status_spec(*spec);
                let variant_ident = status_variant_ident(*spec);
                match ty {
                    // Bodied status: parse the read body into the variant's type.
                    Some(ty) => {
                        let ty = ty_tokens(*ty, names, options, true);
                        quote! {
                            if #spec_tokens.matches(status) {
                                let value = serde_json::from_slice::<#ty>(&body)
                                    .map_err(|error| support::Error::<#error_ty>::Decode {
                                        path: error.to_string(),
                                        body: body.clone(),
                                        truncated: false,
                                    })?;
                                return Ok(support::ResponseValue::new(
                                    status,
                                    headers,
                                    #enum_ident::#variant_ident(value),
                                ));
                            }
                        }
                    }
                    // Documented bodyless status (e.g. `204`): the unit variant, no body parse.
                    None => quote! {
                        if #spec_tokens.matches(status) {
                            return Ok(support::ResponseValue::new(
                                status,
                                headers,
                                #enum_ident::#variant_ident,
                            ));
                        }
                    },
                }
            });
            quote! {
                let (status, headers, body) = support::read_success_body(response)
                    .await
                    .map_err(support::Error::widen)?;
                #(#arms)*
                Err(support::Error::<#error_ty>::UnexpectedStatus { status, headers, body })
            }
        }
    };

    // A streaming success response (`text/event-stream` / `application/x-ndjson`) returns an
    // `EventStream<T>` instead of a `ResponseValue<T>`: on success the whole `response` is handed
    // to the stream with its framing mode, and items are decoded lazily as the caller pulls them.
    // The error path is unchanged (streaming error bodies are out of scope). `success_ty` is the
    // streamed item type `T` — `stream_success` fires only in the single-success-body case, where
    // `success()` is `Plain(T)` and `success_type` renders `T`.
    let stream_framing = operation
        .responses
        .stream_success()
        .map(|(framing, _)| framing);
    let success_decode = match stream_framing {
        Some(framing) => {
            let framing_tokens = match framing {
                crate::ir::Framing::Sse => quote! { support::Framing::Sse },
                crate::ir::Framing::Ndjson => quote! { support::Framing::Ndjson },
            };
            quote! { Ok(support::EventStream::new(response, #framing_tokens)) }
        }
        None => success_decode,
    };
    // The return type is shared with the blocking shim so both surfaces stay identical.
    let (return_ok_ty, _) = operation_return_ty(operation, names, options);

    quote! {
        #docs
        #(#param_default_docs)*
        #deprecated
        #[inline]
        pub async fn #method_ident(
            &self,
            #(#args),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            let mut path = #path_init.to_owned();
            #(#path_replacements)*
            let mut query: Vec<(String, String)> = Vec::new();
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

/// The typed method arguments and their bare forwarding names for an operation. Shared by
/// [`emit_operation`] (the async method) and [`emit_blocking_operation`] (its synchronous shim) so
/// the two signatures are constructed from one source and can never drift. The first vector holds
/// `name: Type` argument declarations; the second holds just the `name`s, in the same order, for the
/// shim's `self.inner.<op>(<names>)` forwarding call. Body args are already `&T`, so the forwarding
/// name (`body`) passes the reference straight through.
fn operation_args(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let params_ident = names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated");
    let mut args = Vec::new();
    let mut forwards = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        let ident = param_ident(param, crate::name::IdentRole::Param);
        let ty = ty_tokens(param.ty, names, options, true);
        args.push(quote! { #ident: #ty });
        forwards.push(quote! { #ident });
    }
    if operation.params.iter().any(|param| !param.required) {
        args.push(quote! { params: Option<#params_ident> });
        forwards.push(quote! { params });
    }
    if let Some(ty) = operation
        .request_body
        .as_ref()
        .and_then(|body| body.ty.map(|ty| ty_tokens(ty, names, options, true)))
    {
        args.push(quote! { body: &#ty });
        forwards.push(quote! { body });
    }
    (args, forwards)
}

/// The `Ok`/`Err` types of an operation's `Result` return: `(return_ok_ty, error_ty)`. A streaming
/// success yields `EventStream<T>`, every other success `ResponseValue<T>`. Shared with the blocking
/// shim so the async and sync return types stay identical.
fn operation_return_ty(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> (TokenStream, TokenStream) {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let error_ident = format_ident!("{}Error", to_pascal(method_ident.as_str()));
    let success_ty = success_type(operation, names, options);
    let return_ok_ty = match operation.responses.stream_success() {
        Some(_) => quote! { support::EventStream<#success_ty> },
        None => quote! { support::ResponseValue<#success_ty> },
    };
    (return_ok_ty, quote! { #error_ident })
}

/// The rustdoc `#[doc = …]` notes carrying required parameters' spec `default`s. Required params are
/// positional arguments with no attribute slot of their own, so their defaults are documented on the
/// method. Shared verbatim between the async method and its blocking shim.
fn param_default_docs_tokens(operation: &Operation) -> Vec<TokenStream> {
    operation
        .params
        .iter()
        .filter(|param| param.required)
        .filter_map(|param| {
            param.default_display.as_ref().map(|default| {
                let note = format!("Parameter `{}` default: `{default}`.", param.name);
                quote! { #[doc = #note] }
            })
        })
        .collect()
}

/// Emit the `BlockingClient`: a synchronous facade, gated on the generated crate's `blocking`
/// feature, that owns the async [`Client`] plus a current-thread tokio runtime and drives each async
/// operation to completion with `block_on`. It reuses the whole async dispatch — every method is a
/// thin shim — so there is zero logic duplication. Constructors mirror the async client's.
///
/// A `BlockingClient` must not be built or used from inside another async runtime (tokio's
/// `block_on` panics when nested); the constructor building its own current-thread runtime is the
/// standard shape for a non-async caller.
pub(crate) fn emit_blocking_client(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_blocking_operation(operation, names, options));
    let doc = "A synchronous client: owns the async `Client` plus a current-thread tokio runtime \
        and `block_on`s each operation. Enable the crate's `blocking` feature to use it.\n\n\
        Must NOT be constructed or called from inside another async runtime — tokio's `block_on` \
        panics when nested. Build one on a plain thread (e.g. `std::thread` or \
        `tokio::task::spawn_blocking`).";
    quote! {
        // In module/include!/macro output, `feature = "blocking"` resolves against the consumer
        // crate. Keep the cfgs inside an ungated lexical lint scope so crates that do not declare
        // that optional feature stay warning-free under `unexpected_cfgs` (including `-D warnings`).
        #[allow(unexpected_cfgs, unused_imports)]
        mod __spargen_blocking {
            use super::*;

            #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
            #[doc = #doc]
            #[allow(dead_code)]
            pub struct BlockingClient {
                inner: Client,
                runtime: support::BlockingRuntime,
            }

            #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
            #[forbid(unsafe_code)]
            #[allow(dead_code, unused_mut, unused_variables, clippy::result_large_err)]
            impl BlockingClient {
            /// Build a blocking client over a fresh default `reqwest::Client`.
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::new(base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client over a caller-supplied `reqwest::Client`.
            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_client(client, base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client over a caller-supplied transport backend.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_backend(backend, base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
                Ok(Self { inner, runtime })
            }

            /// Borrow the wrapped async client.
            pub fn inner(&self) -> &Client {
                &self.inner
            }

            /// Borrow the client's shared core (base URL, credentials, transport).
            pub fn core(&self) -> &support::ClientCore {
                self.inner.core()
            }

            /// Register a credential for a named security scheme (mirrors the async client).
            #[must_use]
            pub fn with_credential(
                mut self,
                scheme: &str,
                credential: support::Credential,
            ) -> Self {
                self.inner = self.inner.with_credential(scheme, credential);
                self
            }

                #(#methods)*
            }
        }

        #[allow(unused_imports)]
        pub use __spargen_blocking::*;
    }
}

/// Emit one blocking operation method: the async method's signature minus `async`, whose body drives
/// the async method to completion on the owned runtime. Same docs, deprecation, argument list, and
/// return types as the async method (all built from the shared signature helpers).
fn emit_blocking_operation(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let docs = doc_tokens(&operation.docs);
    let param_default_docs = param_default_docs_tokens(operation);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });
    let (args, forwards) = operation_args(operation, names, options);
    let (return_ok_ty, error_ty) = operation_return_ty(operation, names, options);
    quote! {
        #docs
        #(#param_default_docs)*
        #deprecated
        #[inline]
        pub fn #method_ident(
            &self,
            #(#args),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            self.runtime.block_on(self.inner.#method_ident(#(#forwards),*))
        }
    }
}

/// Emit the `body_send` for a `multipart/form-data` request body: build a `reqwest::multipart::Form`
/// from the typed body struct, one part per field in declaration order (part order is deterministic).
/// A binary field (`bytes::Bytes`) becomes a file/bytes part; a scalar becomes a text part via
/// `Display`; an object/array/union becomes a JSON-encoded text part. Optional fields (`Option<T>`)
/// only add their part when `Some`. Lowering guarantees a multipart body is an object schema, so a
/// non-struct body cannot reach here (it is rejected as `E009`); the fallback stays a no-op.
fn emit_multipart_body(ty: Ty, api: &Api, names: &Names) -> TokenStream {
    let Some(TypeKind::Struct(object)) = api.types.get(ty.id).map(|def| &def.kind) else {
        return quote! {};
    };
    let parts = object.fields.iter().map(|field| {
        let wire = &field.name.wire;
        let field_ident = names
            .fields
            .get(&(ty.id, field.name.wire.clone()))
            .expect("multipart body field name allocated");
        // A field is accessed as `Option<T>` when it is optional (an extra `Option` wrapper) or the
        // schema itself is nullable (`ty_tokens` already wrapped it) — mirroring `emit_field`.
        let optional = !field.required || field.ty.nullable;
        let kind = api.types.get(field.ty.id).map(|def| &def.kind);
        // `receiver` is the value for method calls (`.to_vec()`/`.to_string()` auto-ref); `reference`
        // is an explicit `&value` for `serde_json::to_string`, which takes `&T`. Splitting them keeps
        // the emitted code free of `clippy::needless_borrow` on the method-call receivers.
        let add_part = |receiver: &TokenStream, reference: &TokenStream| match kind {
            // A binary/bytes property → a file/bytes part carrying the raw bytes.
            Some(TypeKind::Bytes) => quote! {
                form = form.part(#wire, reqwest::multipart::Part::bytes(#receiver.to_vec()));
            },
            // A scalar property → a text part rendered through `Display`.
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_)) => quote! {
                form = form.text(#wire, #receiver.to_string());
            },
            // Any composite (object/array/tuple/union/untyped) property → a JSON-encoded text part.
            _ => quote! {
                form = form.text(
                    #wire,
                    serde_json::to_string(#reference).map_err(support::Error::request_construction)?,
                );
            },
        };
        if optional {
            // `value` is already `&T` from the `if let Some(value) = &body.field` binding.
            let stmt = add_part(&quote! { value }, &quote! { value });
            quote! {
                if let Some(value) = &body.#field_ident {
                    #stmt
                }
            }
        } else {
            add_part(&quote! { body.#field_ident }, &quote! { &body.#field_ident })
        }
    });
    quote! {
        let mut form = reqwest::multipart::Form::new();
        #(#parts)*
        request = request.multipart(form);
    }
}

fn param_ident(param: &crate::ir::Parameter, role: crate::name::IdentRole) -> proc_macro2::Ident {
    escaped_token(&param.name, role)
}

/// Build the `proc_macro2::Ident` for an escaped name, PRESERVING raw escaping: a keyword like
/// `type` escapes to `r#type`, which must become a raw identifier token (`Ident::new_raw`) — NOT a
/// bare `type` (an invalid keyword token that fails to parse). This is the token equivalent of the
/// name subsystem's `Ident` `ToTokens`; use it wherever an escaped param/field name is turned into
/// a `proc_macro2::Ident` directly instead of going through a `name::Ident`.
fn escaped_token(name: &str, role: crate::name::IdentRole) -> proc_macro2::Ident {
    let escaped = crate::name::escape(name, role);
    let span = proc_macro2::Span::call_site();
    match escaped.as_str().strip_prefix("r#") {
        Some(raw) => proc_macro2::Ident::new_raw(raw, span),
        None => proc_macro2::Ident::new(escaped.as_str(), span),
    }
}

/// Render a path/header parameter value from a borrowed expression. Schema-typed parameters use
/// OpenAPI `simple` serialization; `content`-typed parameters retain their media codec.
fn param_value_tokens(param: &crate::ir::Parameter, value: TokenStream) -> TokenStream {
    if let crate::ir::ParamStyle::Content(media) = &param.style {
        return match media {
            MediaType::Json => quote! {
                serde_json::to_string(#value).map_err(support::Error::request_construction)?
            },
            _ => quote! {
                support::serialize_simple(#value, false)
                    .map_err(support::Error::request_construction)?
            },
        };
    }
    let explode = param.explode;
    quote! {
        support::serialize_simple(#value, #explode)
            .map_err(support::Error::request_construction)?
    }
}

/// Emit serialization of one query parameter into the operation's `query` pair vector.
fn query_param_tokens(param: &crate::ir::Parameter, name: &str, value: TokenStream) -> TokenStream {
    match &param.style {
        crate::ir::ParamStyle::Form => {
            let explode = param.explode;
            quote! {
                query.extend(
                    support::serialize_form(#name, #value, #explode)
                        .map_err(support::Error::request_construction)?,
                );
            }
        }
        crate::ir::ParamStyle::Content(_) => {
            let value = param_value_tokens(param, value);
            quote! { query.push((#name.to_owned(), #value)); }
        }
        crate::ir::ParamStyle::Simple => quote! {},
    }
}

/// Emit serialization of one cookie parameter into the operation's cookie fragments.
fn cookie_param_tokens(
    param: &crate::ir::Parameter,
    name: &str,
    value: TokenStream,
) -> TokenStream {
    match &param.style {
        crate::ir::ParamStyle::Form => {
            let explode = param.explode;
            quote! {
                for (name, value) in support::serialize_form(#name, #value, #explode)
                    .map_err(support::Error::request_construction)?
                {
                    cookies.push(format!("{name}={value}"));
                }
            }
        }
        crate::ir::ParamStyle::Content(_) => {
            let value = param_value_tokens(param, value);
            quote! { cookies.push(format!("{}={}", #name, #value)); }
        }
        crate::ir::ParamStyle::Simple => quote! {},
    }
}

/// Turn lowered documentation into `#[doc = …]` attributes so IDE hover shows the API docs.
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

/// Emit an operation's optional-parameters `…Params` struct (deriving `Default`, public fields)
/// plus an `impl` of fluent `#[must_use]` consuming setters — one per optional param, named after
/// its field — so callers can write `…Params::default().foo(x).bar(y)` instead of a struct literal.
pub(crate) fn emit_params_struct(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let ident = names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated");
    let optional: Vec<&crate::ir::Parameter> = operation
        .params
        .iter()
        .filter(|param| !param.required)
        .collect();
    // The setter method reuses the field ident verbatim (same escaping/keyword handling), so
    // build it once per param.
    let field_ident =
        |param: &crate::ir::Parameter| escaped_token(&param.name, crate::name::IdentRole::Field);
    let fields = optional.iter().map(|param| {
        let ident = field_ident(param);
        let wire = &param.name;
        // Every struct param is optional, so the field is always an `Option`. `ty_tokens`
        // already wraps a nullable param (`"null"` in its type array) in `Option`, so only wrap
        // again when it did not — otherwise a nullable optional param becomes `Option<Option<T>>`
        // and the query/header `value.to_string()` serialization would not compile
        // (`Option<T>: !Display`). Absent and `null` both collapse to `None`.
        let ty = if param.ty.nullable {
            ty_tokens(param.ty, names, options, true)
        } else {
            let inner = ty_tokens(param.ty, names, options, true);
            quote! { Option<#inner> }
        };
        let mut notes: Vec<String> = Vec::new();
        if param.deprecated {
            notes.push("Deprecated per the spec.".to_owned());
        }
        if let Some(default) = &param.default_display {
            notes.push(format!("Default: `{default}`."));
        }
        let notes = notes.iter().map(|note| quote! { #[doc = #note] });
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
            options,
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

/// Emit an operation's multi-status success response enum, one payload-carrying variant per
/// documented success status (empty when the operation has zero or one success body). The variant
/// is selected by HTTP status at decode time, so the enum derives only `Debug, Clone` — no
/// whole-enum `Deserialize`, no `serde(untagged)`.
pub(crate) fn emit_response_enum(
    operation: &Operation,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Enum(entries) => {
            let method_ident = names
                .operations
                .get(&operation.id)
                .expect("operation name allocated");
            let ident = success_enum_ident(method_ident);
            let variants = entries
                .iter()
                .map(|(spec, ty)| response_variant_def(*spec, *ty, names, options));
            quote! {
                #[allow(dead_code)]
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#variants)*
                }
            }
        }
        _ => quote! {},
    }
}

/// One response-enum variant definition: a payload-carrying `Status2xx(types::T)` for a bodied
/// status, or a payload-free `Status204` unit variant for a documented bodyless status.
fn response_variant_def(
    spec: crate::ir::StatusSpec,
    ty: Option<Ty>,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let variant_ident = status_variant_ident(spec);
    match ty {
        Some(ty) => {
            let ty = ty_tokens(ty, names, options, true);
            quote! { #variant_ident(#ty), }
        }
        None => quote! { #variant_ident, },
    }
}

/// Emit an operation's typed error enum (or type alias for a single error body).
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
    match operation.responses.error() {
        // Multiple documented error bodies → a payload-carrying enum, one variant per status. The
        // variant is chosen by HTTP status at classification time, so it derives no whole-enum
        // `Deserialize` (and never `serde(untagged)`); each variant's body is decoded on its own.
        ErrorShape::Enum(entries) => {
            let variants = entries
                .iter()
                .map(|(spec, ty)| response_variant_def(*spec, *ty, names, options));
            quote! {
                #[allow(dead_code)]
                #[derive(Debug, Clone)]
                pub enum #error_ident {
                    #(#variants)*
                }
            }
        }
        // A single documented error body: a plain alias to that type.
        ErrorShape::Single(ty) => {
            let ty = ty_tokens(ty, names, options, true);
            quote! {
                #[allow(dead_code)]
                pub type #error_ident = #ty;
            }
        }
        // No documented error body: every non-success status is Error::UnexpectedStatus, and the
        // uninhabited alias makes Error::Api impossible to construct.
        ErrorShape::None => quote! {
            #[allow(dead_code)]
            pub type #error_ident = std::convert::Infallible;
        },
    }
}

/// Emit the private `support` module by embedding the freestanding runtime source verbatim, under
/// `#![forbid(unsafe_code)]`. When `uses_xml` is set (the API has an `application/xml` / `text/xml`
/// body), the feature-gated XML codec module is embedded and its helpers re-exported; otherwise it
/// is omitted entirely, so a non-XML output carries no `quick-xml` reference.
pub(crate) fn emit_support(uses_xml: bool) -> TokenStream {
    let embed = |file: &crate::support::SupportFile| {
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
    };
    let modules = crate::support::runtime_files().iter().map(embed);
    // The XML codec module is embedded only when the API uses an XML body; the generated manifest
    // then carries the `quick-xml` dependency it needs. A non-XML output never references quick-xml.
    let xml_module = uses_xml.then(|| embed(&crate::support::xml_runtime_file()));
    let xml_reexport = uses_xml.then(|| {
        quote! { pub use xml::{classify_error_xml, decode_success_xml, to_xml}; }
    });
    // The blocking facade (`BlockingRuntime`) is embedded unconditionally but gated on the
    // `blocking` feature AND `not(target_arch = "wasm32")` at the module level: the tokio-dependent
    // code compiles only when a consumer opts in on a native target, so a default build carries no
    // tokio reference and a wasm build never pulls tokio even with the feature on (tokio's blocking
    // runtime cannot run on the single-threaded browser). The generated manifest always declares the
    // (user-facing) `blocking` feature wired to an optional, non-wasm tokio dependency.
    let blocking_inner = embed(&crate::support::blocking_runtime_file());
    let blocking_module = quote! {
        #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
        #blocking_inner
    };
    let blocking_reexport = quote! {
        #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
        pub use blocking::BlockingRuntime;
    };
    quote! {
        /// The freestanding runtime embedded verbatim into this output; no spargen crate exists
        /// at runtime.
        #[forbid(unsafe_code)]
        #[allow(dead_code, unexpected_cfgs, unused_imports, clippy::result_large_err)]
        mod support {
            #(#modules)*
            #xml_module
            #blocking_module

            pub use auth::{AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture, TokenProvider};
            pub use client::{ClientConfig, ClientCore};
            pub use dispatch::{attach_auth, build_url, classify_error, decode_success, read_error_body, read_success_body, send, unexpected_status, StatusSpec};
            pub use error::{Error, ProtocolError, RedirectError, RequestError, TimeoutKind, TransportError};
            pub use middleware::{Middleware, MiddlewareBackend, Next};
            pub use parameter::{serialize_form, serialize_simple, ParameterError};
            pub use paginate::{next_link, LinkPaginator};
            pub use response::ResponseValue;
            pub use retry::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};
            pub use stream::{EventStream, Framing};
            pub use transport::{ExecuteFuture, HttpBackend, ReqwestBackend};
            pub use wasm::{MaybeSend, MaybeSync};
            #xml_reexport
            #blocking_reexport
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
            let providers = object
                .fields
                .iter()
                .filter_map(|field| emit_default_provider(id, field, names, options));
            let additional = match &object.additional {
                AdditionalProps::Typed(ty) => {
                    let ty = ty_tokens(**ty, names, options, false);
                    let overflow = names
                        .struct_overflow
                        .get(&id)
                        .expect("overflow field name allocated");
                    quote! { #[serde(flatten)] pub #overflow: BTreeMap<String, #ty>, }
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
                #(#providers)*
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
        TypeKind::Never => {
            let error = format!("no JSON value can inhabit schema {}", ident.as_str());
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone)]
                pub enum #ident {}

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        Err(serde::de::Error::custom(#error))
                    }
                }

                impl serde::Serialize for #ident {
                    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        Err(serde::ser::Error::custom(#error))
                    }
                }
            }
        }
        TypeKind::Union(union) => {
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
                } => {
                    let variant_defs = union.variants.iter().map(|variant| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        let ty = ty_tokens(variant.ty, names, options, false);
                        quote! { #variant_ident(#ty), }
                    });
                    let category_arms =
                        union
                            .variants
                            .iter()
                            .zip(categories)
                            .filter_map(|(variant, category)| {
                                let category = category.as_ref()?;
                                let variant_ident = names
                                    .variants
                                    .get(&(id, variant.name_hint.clone()))
                                    .expect("union variant name allocated");
                                let predicate = match category {
                                    JsonCategory::String => quote! { value.is_string() },
                                    JsonCategory::Number => quote! { value.is_number() },
                                    JsonCategory::Boolean => quote! { value.is_boolean() },
                                    JsonCategory::Array => quote! { value.is_array() },
                                    JsonCategory::Object => quote! { value.is_object() },
                                };
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
                        .filter_map(|(variant, tag)| {
                            let tag = tag.as_ref()?;
                            let variant_ident = names
                                .variants
                                .get(&(id, variant.name_hint.clone()))
                                .expect("union variant name allocated");
                            Some(quote! {
                                #tag => serde_json::from_value(value)
                                    .map(#ident::#variant_ident)
                                    .map_err(serde::de::Error::custom),
                            })
                        });
                    let ser_arms = union.variants.iter().zip(tags).map(|(variant, tag)| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        let tag = match tag {
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
                    let unknown_tag =
                        format!("unknown discriminator value for union {}", ident.as_str());
                    let non_object = format!(
                        "tagged variant of union {} did not serialize as an object",
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
                                #(#category_arms)*
                                let tag = value
                                    .get(#tag_field)
                                    .and_then(serde_json::Value::as_str)
                                    .map(std::borrow::ToOwned::to_owned)
                                    .ok_or_else(|| serde::de::Error::custom(#missing_tag))?;
                                match tag.as_str() {
                                    #(#de_arms)*
                                    _ => Err(serde::de::Error::custom(#unknown_tag)),
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
                    let variant_defs = union.variants.iter().map(|variant| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        let ty = ty_tokens(variant.ty, names, options, false);
                        quote! { #variant_ident(#ty), }
                    });
                    let de_arms = union
                        .variants
                        .iter()
                        .zip(features)
                        .map(|(variant, feature)| {
                            let variant_ident = names
                                .variants
                                .get(&(id, variant.name_hint.clone()))
                                .expect("union variant name allocated");
                            let predicate = match feature {
                                DisjointFeature::JsonType(category) => match category {
                                    JsonCategory::String => quote! { value.is_string() },
                                    JsonCategory::Number => quote! { value.is_number() },
                                    JsonCategory::Boolean => quote! { value.is_boolean() },
                                    JsonCategory::Array => quote! { value.is_array() },
                                    JsonCategory::Object => quote! { value.is_object() },
                                },
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
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
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
                    let variant_defs = union.variants.iter().map(|variant| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        let ty = ty_tokens(variant.ty, names, options, false);
                        quote! { #variant_ident(#ty), }
                    });
                    let attempts = union.variants.iter().zip(priorities).map(
                    |(variant, priority)| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        let ty = ty_tokens(variant.ty, names, options, false);
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
                    },
                );
                    let ser_arms = union.variants.iter().map(|variant| {
                        let variant_ident = names
                            .variants
                            .get(&(id, variant.name_hint.clone()))
                            .expect("union variant name allocated");
                        quote! {
                            #ident::#variant_ident(inner) => {
                                serde_json::to_value(inner).map_err(serde::ser::Error::custom)?
                            }
                        }
                    });
                    let validations = union.variants.iter().map(|variant| {
                        let ty = ty_tokens(variant.ty, names, options, false);
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
                    let de_valid = match mode {
                        UnionMode::OneOf => quote! { match_count == 1 },
                        UnionMode::AnyOf => quote! { match_count >= 1 },
                    };
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
    // An `xml.name`/`xml.attribute` hint overrides the serde wire name for XML bodies (an attribute
    // uses quick-xml's `@name` convention); otherwise the plain property wire name is used. The Rust
    // identifier and the names-table key stay keyed off the original property name, so only the wire
    // string changes.
    let wire = field
        .xml
        .wire_override(&field.name.wire)
        .unwrap_or_else(|| field.name.wire.clone());
    // `ty_tokens` already wraps a nullable type in `Option` (`"null"` in the type array), so only an
    // *optional* non-nullable field needs the extra `Option` here — wrapping a nullable field again
    // would yield `Option<Option<T>>`. A required nullable field stays a single `Option<T>` (present
    // but may be `null`); an optional field of either kind is a single `Option<T>`.
    let mut ty = ty_tokens(field.ty, names, options, false);
    if !field.required && !field.ty.nullable {
        ty = quote! { Option<#ty> };
    }
    // An optional field always deserializes an absent value; when the spec gives a representable
    // scalar default, point serde at a generated provider so the default fills in rather than
    // `None`. Otherwise fall back to `Option::default()` (`None`).
    let serde_default = if field.required {
        quote! {}
    } else if field
        .default
        .as_ref()
        .is_some_and(|default| default.applied.is_some())
    {
        let provider = default_provider_ident(id, ident).to_string();
        quote! { default = #provider, skip_serializing_if = "Option::is_none", }
    } else {
        quote! { default, skip_serializing_if = "Option::is_none", }
    };
    let mut notes: Vec<String> = Vec::new();
    if field.deprecated {
        notes.push("Deprecated per the spec.".to_owned());
    }
    if field.read_only {
        notes.push("Read-only: set by the server; ignored in requests.".to_owned());
    }
    if field.write_only {
        notes.push("Write-only: sent in requests; absent from responses.".to_owned());
    }
    if let Some(default) = &field.default {
        notes.push(default.doc_note.clone());
    }
    let notes = notes.iter().map(|note| quote! { #[doc = #note] });
    quote! {
        #(#notes)*
        #[serde(rename = #wire, #serde_default)]
        pub #ident: #ty,
    }
}

/// The deterministic identifier of a field's generated serde default-provider function. Derived
/// from the owning type's dense id plus the field's Rust identifier, so it is stable across runs
/// and cannot collide with a `PascalCase` type ident or another field's provider.
fn default_provider_ident(
    id: crate::ir::TypeId,
    field_ident: &crate::name::Ident,
) -> proc_macro2::Ident {
    format_ident!(
        "default_{}_{}",
        id.0,
        field_ident.as_str().trim_start_matches("r#")
    )
}

/// Emit a field's serde default-provider function, when its `default` is a representable scalar
/// wired through serde. The function returns `Option<T>` matching the (optional) field's Rust type.
fn emit_default_provider(
    id: crate::ir::TypeId,
    field: &Field,
    names: &Names,
    options: &CodegenOptions,
) -> Option<TokenStream> {
    let applied = field.default.as_ref()?.applied.as_ref()?;
    let field_ident = names
        .fields
        .get(&(id, field.name.wire.clone()))
        .expect("field name allocated");
    let fn_ident = default_provider_ident(id, field_ident);
    let inner_ty = ty_tokens(field.ty, names, options, false);
    let value = default_value_tokens(applied, field.ty, names);
    Some(quote! {
        fn #fn_ident() -> Option<#inner_ty> {
            Some(#value)
        }
    })
}

/// Render a representable default as a Rust literal (or generated enum variant) for the field's
/// Rust type.
fn default_value_tokens(value: &crate::ir::DefaultValue, ty: Ty, names: &Names) -> TokenStream {
    use crate::ir::DefaultValue;
    match value {
        DefaultValue::Bool(value) => quote! { #value },
        DefaultValue::Int(value) => {
            let literal = proc_macro2::Literal::i64_unsuffixed(*value);
            quote! { #literal }
        }
        DefaultValue::Float(value) => {
            let literal = proc_macro2::Literal::f64_unsuffixed(*value);
            quote! { #literal }
        }
        DefaultValue::Str(value) => quote! { #value.to_owned() },
        DefaultValue::EnumVariant(value) => {
            let enum_ident = names.types.get(&ty.id).expect("enum type name allocated");
            let variant_ident = names
                .variants
                .get(&(ty.id, value.clone()))
                .expect("variant name allocated");
            quote! { #enum_ident::#variant_ident }
        }
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
        TypeKind::Null => quote! { () },
        TypeKind::Any => quote! { serde_json::Value },
        TypeKind::Struct(_) | TypeKind::Enum(_) | TypeKind::Never | TypeKind::Union(_) => {
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
        SuccessShape::Enum(_) => {
            let method_ident = names
                .operations
                .get(&operation.id)
                .expect("operation name allocated");
            let ident = success_enum_ident(method_ident);
            quote! { #ident }
        }
    }
}

/// The type name of an operation's multi-status success response enum, derived from the method name
/// (mirroring how the error enum is named `{Method}Error`).
fn success_enum_ident(method_ident: &crate::name::Ident) -> proc_macro2::Ident {
    format_ident!("{}Response", to_pascal(method_ident.as_str()))
}

/// The `PascalCase` variant identifier for a documented status selector: `Status200` for an exact
/// code, `Status2xx` for a range, and `Default` for the `default` response (carried as the
/// `Range(0)` sentinel by [`Responses::error`]). Deterministic and, within one enum, unique by
/// construction (each selector appears once). Routed through the `name` escaper for validity.
fn status_variant_ident(spec: crate::ir::StatusSpec) -> proc_macro2::Ident {
    let raw = match spec {
        crate::ir::StatusSpec::Exact(code) => format!("Status{code}"),
        crate::ir::StatusSpec::Range(0) => "Default".to_owned(),
        crate::ir::StatusSpec::Range(prefix) => format!("Status{prefix}xx"),
    };
    format_ident!(
        "{}",
        crate::name::escape(&raw, crate::name::IdentRole::Variant).as_str()
    )
}

/// The runtime [`support::StatusSpec`] tokens for a documented status selector. The `default`
/// sentinel (`Range(0)`) maps to `Any`, matching how the single-error-body path builds its table.
fn runtime_status_spec(spec: crate::ir::StatusSpec) -> TokenStream {
    match spec {
        crate::ir::StatusSpec::Exact(code) => quote! { support::StatusSpec::Exact(#code) },
        crate::ir::StatusSpec::Range(0) => quote! { support::StatusSpec::Any },
        crate::ir::StatusSpec::Range(prefix) => quote! { support::StatusSpec::Range(#prefix) },
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
        // reqwest has no `QUERY` constant (OpenAPI 3.2's new fixed method), so build it from the
        // token bytes. `QUERY` is a valid HTTP method token, so `from_bytes` never fails here.
        crate::ir::Method::Query => quote! {
            reqwest::Method::from_bytes(b"QUERY").expect("QUERY is a valid HTTP method token")
        },
    }
}

fn to_pascal(value: &str) -> String {
    crate::name::to_pascal_case(value.trim_start_matches("r#"))
}
