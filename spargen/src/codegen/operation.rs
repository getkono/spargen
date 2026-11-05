//! One async operation method on `Client` with its URL accessor, and the signature pieces its
//! blocking twin shares.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, MediaType, Operation, ParamLoc, Ty, TypeKind};
use crate::name::Names;

use super::body::{accept_tokens, body_send_tokens};
use super::dispatch::{
    attach_auth_tokens, error_branch_tokens, success_decode_tokens, success_type,
};
use super::docs::{doc_tokens, normalize_rustdoc};
use super::params::{
    cookie_param_tokens, json_querystring_tokens, location_statements, param_value_tokens,
    query_param_tokens, querystring_param_tokens,
};
use super::ty::{reqwest_method, ty_tokens};

/// Emit one `#[inline] pub async fn` operation method. Its body is written out per operation: it
/// substitutes the path parameters, serializes the query, `querystring`, header, and cookie
/// parameters, encodes the request body by its media, attaches the selected credentials, and sends
/// the request through `support`, then dispatches the response by status into the operation's
/// typed success value or typed error. The wire work itself — encoding, sending, and decoding —
/// is done by `support` routines, most of them generic over the body or error type.
///
/// The operation's URL accessor (`emit_operation_url`) is emitted after it, from the same
/// URL-building statements.
pub(super) fn emit_operation(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let bindings = &operation_names.bindings;
    let url_binding = &bindings.url;
    let request_binding = &bindings.request;
    let cookies_binding = &bindings.cookies;
    let method_ident = &operation_names.method;
    let error_ident = &operation_names.error;
    let reqwest_method = reqwest_method(&operation.method);
    let success_ty = success_type(operation, names);
    let error_ty = quote! { #error_ident };
    let docs = doc_tokens(&operation.docs);
    // Required parameters are positional method arguments with no attribute slot of their own, so a
    // required parameter's `default` is surfaced in the method rustdoc instead.
    let param_default_docs = param_default_docs_tokens(operation);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });

    // The typed argument list (required params, the optional-params struct, the body); see
    // `operation_args`.
    let (args, _arg_names) = operation_args(operation, api, names);
    let UrlBuilding {
        statements: url_statements,
        build_url,
    } = url_building(operation, names);

    let headers = location_statements(
        operation,
        operation_names,
        ParamLoc::Header,
        |param, value| {
            let name = &param.name;
            let value = param_value_tokens(param, value);
            quote! { #request_binding = #request_binding.header(#name, #value); }
        },
    );
    let has_cookies = operation
        .params
        .iter()
        .any(|param| param.location == ParamLoc::Cookie);
    let cookie_init =
        has_cookies.then(|| quote! { let mut #cookies_binding: Vec<String> = Vec::new(); });
    let cookies = location_statements(
        operation,
        operation_names,
        ParamLoc::Cookie,
        |param, value| cookie_param_tokens(param, &param.name, value, cookies_binding),
    );
    let cookie_attach = has_cookies.then(|| {
        quote! {
            if !#cookies_binding.is_empty() {
                #request_binding = #request_binding.header(
                    reqwest::header::COOKIE,
                    #cookies_binding.join("; "),
                );
            }
        }
    });
    let accept = accept_tokens(operation, bindings);
    let body_send = body_send_tokens(operation, api, names, bindings);
    let attach_auth = attach_auth_tokens(operation, api, request_binding);
    let error_branch = error_branch_tokens(operation, api, names, error_ident);
    let (success_decode, reconnect_request_init) =
        success_decode_tokens(operation, api, names, bindings, &success_ty, error_ident);
    // The return type is shared with the blocking shim so both surfaces stay identical.
    let (return_ok_ty, _) = operation_return_ty(operation, names);

    let arg_normalizations = operation_arg_normalizations(operation, api, names, |_| true, true);
    let url_accessor = emit_operation_url(operation, api, names);

    quote! {
        #docs
        #(#param_default_docs)*
        #deprecated
        #[inline]
        pub async fn #method_ident(
            &self,
            #(#args),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            #(#arg_normalizations)*
            #url_statements
            let #url_binding = #build_url
                .map_err(support::Error::widen)?;
            let mut #request_binding = self.core.http().request(#reqwest_method, #url_binding);
            #(#headers)*
            #cookie_init
            #(#cookies)*
            #cookie_attach
            #accept
            #body_send
            #attach_auth
            let #request_binding = #request_binding
                .build()
                .map_err(support::Error::request_construction)?;
            #reconnect_request_init
            let response = support::send(&self.core, #request_binding)
                .await
                .map_err(support::Error::widen)?;
            if response.status().is_success() {
                #success_decode
            } else {
                #error_branch
            }
        }

        #url_accessor
    }
}

/// Whether `location` shapes the request URL: the path, the query, or the whole query string.
fn shapes_url(location: ParamLoc) -> bool {
    matches!(
        location,
        ParamLoc::Path | ParamLoc::Query | ParamLoc::QueryString
    )
}

/// Whether an operation's optional-parameters bundle holds a parameter that shapes its URL, so the
/// URL accessor takes the bundle as well.
fn url_takes_params(operation: &Operation) -> bool {
    operation
        .params
        .iter()
        .any(|param| !param.required && shapes_url(param.location))
}

/// The arguments of an operation's URL accessor, and their bare forwarding names: the operation's
/// own [`operation_args`] narrowed to the required parameters that [`shapes_url`] admits, and the
/// optional-parameters bundle where [`url_takes_params`] holds. A header, a cookie or the request
/// body changes nothing in the URL, so the accessor does not ask for one.
pub(super) fn operation_url_args(
    operation: &Operation,
    api: &Api,
    names: &Names,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let (args, forwards) = operation_args(operation, api, names);
    let required = operation.params.iter().filter(|param| param.required);
    let mut keep: Vec<bool> = required.map(|param| shapes_url(param.location)).collect();
    if names.operation(&operation.id).bindings.params.is_some() {
        keep.push(url_takes_params(operation));
    }
    // The body argument, last when present, is never kept.
    keep.resize(args.len(), false);
    let kept = |items: Vec<TokenStream>| -> Vec<TokenStream> {
        items
            .into_iter()
            .zip(&keep)
            .filter_map(|(item, keep)| keep.then_some(item))
            .collect()
    };
    (kept(args), kept(forwards))
}

/// Emit one operation's URL accessor on `Client`, built by the same [`url_building`] statements as
/// the operation method and documented by [`url_accessor_doc`].
fn emit_operation_url(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let url_ident = &operation_names.url_accessor;
    let method_ident = &operation_names.method;
    let (args, _) = operation_url_args(operation, api, names);
    let normalizations = operation_arg_normalizations(
        operation,
        api,
        names,
        shapes_url,
        url_takes_params(operation),
    );
    let UrlBuilding {
        statements,
        build_url,
    } = url_building(operation, names);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });
    let doc = url_accessor_doc(method_ident);
    quote! {
        #[doc = #doc]
        #deprecated
        #[inline]
        pub fn #url_ident(
            &self,
            #(#args),*
        ) -> Result<reqwest::Url, support::Error<std::convert::Infallible>> {
            #(#normalizations)*
            #statements
            #build_url
        }
    }
}

/// The rustdoc of an operation's URL accessor, on `Client` and `BlockingClient` alike.
pub(super) fn url_accessor_doc(method_ident: &crate::name::Ident) -> String {
    let method = method_ident.unraw();
    normalize_rustdoc(&format!(
        "The URL `{method}` sends its request to, less any `apiKey` query credential, with the \
         same path and query encoding, resolved against the client's base URL or the operation's \
         server override. Nothing is sent and no credential is attached: this is for a URL handed \
         to something that fetches it itself."
    ))
}

/// The statements that assemble an operation's path and query into its bindings, and the
/// `support::build_url_on` (or `build_url_with_query_string_on`) call that resolves them into a
/// `Result<Url, Error<Infallible>>`. Shared by the operation method and its URL accessor, so the
/// URL a caller is handed is the URL the request goes to.
struct UrlBuilding {
    statements: TokenStream,
    build_url: TokenStream,
}

/// Build an operation's [`UrlBuilding`] over the bindings `name` allocated for it.
fn url_building(operation: &Operation, names: &Names) -> UrlBuilding {
    let operation_names = names.operation(&operation.id);
    let bindings = &operation_names.bindings;
    let path_binding = &bindings.path;
    let query_binding = &bindings.query;
    let raw_query_binding = &bindings.raw_query;
    let path_init = operation.path.raw.clone();
    let path_replacements = operation_names
        .params(operation)
        .filter(|(param, _)| param.location == ParamLoc::Path)
        .map(|(param, ident)| {
            let placeholder = format!("{{{}}}", param.name);
            let value = param_value_tokens(param, quote! { &#ident });
            quote! {
                #path_binding = #path_binding.replace(#placeholder, &#value);
            }
        });
    let query = location_statements(
        operation,
        operation_names,
        ParamLoc::Query,
        |param, value| query_param_tokens(param, &param.name, value, query_binding),
    );
    let uses_querystring = operation
        .params
        .iter()
        .any(|parameter| parameter.location == ParamLoc::QueryString);
    // Lowering admits at most one `in: querystring` parameter per operation, so this is that one.
    let json_querystring = operation_names.params(operation).find(|(parameter, _)| {
        parameter.location == ParamLoc::QueryString
            && matches!(
                &parameter.style,
                crate::ir::ParamStyle::Content(MediaType::Json)
            )
    });
    // Only a JSON whole-query value sets the raw query. A required one always does, so it is the
    // binding's initializer rather than an assignment: a `None` it always overwrites would be an
    // `unused_assignments` warning in every consumer's build.
    let raw_query_init = uses_querystring.then(|| match json_querystring {
        Some((parameter, ident)) if parameter.required => {
            let encoded = json_querystring_tokens(quote! { &#ident });
            quote! { let #raw_query_binding: Option<String> = Some(#encoded); }
        }
        Some(_) => quote! { let mut #raw_query_binding: Option<String> = None; },
        None => quote! { let #raw_query_binding: Option<String> = None; },
    });
    let querystring = location_statements(
        operation,
        operation_names,
        ParamLoc::QueryString,
        |parameter, value| {
            // A required JSON whole-query value is already the raw query's initializer.
            let initializes_raw_query = parameter.required
                && matches!(
                    &parameter.style,
                    crate::ir::ParamStyle::Content(MediaType::Json)
                );
            if initializes_raw_query {
                return quote! {};
            }
            querystring_param_tokens(parameter, value, query_binding, raw_query_binding)
        },
    );

    // An Operation or Path Item Object may override the document's `servers`. The runtime's
    // `*_on` entry points take that override: absolute, it replaces the client's base URL;
    // relative, it is joined onto it. `None` keeps the client's base.
    let server_override = match &operation.server {
        Some(server) => quote! { Some(#server) },
        None => quote! { None },
    };
    let build_url = if uses_querystring {
        quote! {
            support::build_url_with_query_string_on(
                &self.core,
                #server_override,
                &#path_binding,
                &#query_binding,
                #raw_query_binding.as_deref(),
            )
        }
    } else {
        quote! {
            support::build_url_on(&self.core, #server_override, &#path_binding, &#query_binding)
        }
    };

    UrlBuilding {
        statements: quote! {
            let mut #path_binding = #path_init.to_owned();
            #(#path_replacements)*
            let mut #query_binding: Vec<String> = Vec::new();
            #raw_query_init
            #(#query)*
            #(#querystring)*
        },
        build_url,
    }
}

/// Whether a required parameter is emitted as `impl Into<String>` rather than its concrete type.
///
/// Exactly a non-nullable, unboxed [`Prim::String`](crate::ir::Prim::String). `Option<String>`
/// and `Box<String>` positions stay concrete: `Into` would make `None` ambiguous at the call site
/// for no gain. A `uuid`, `date` or `date-time` primitive stays concrete too, including where its
/// feature is off and it renders as `String`.
fn takes_into_string(api: &Api, ty: Ty) -> bool {
    !ty.nullable
        && !ty.boxed
        && matches!(
            api.types.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Primitive(crate::ir::Prim::String))
        )
}

/// Convert every `impl Into<..>` argument to its concrete type once, at the top of the method body:
/// each required parameter [`takes_into_string`] admits, and the optional-parameters bundle from
/// `impl Into<Option<…Params>>` to `Option<…Params>`. The rest of the emitted body is written
/// against concrete types.
///
/// Only the arguments the method takes are converted: a required parameter whose location
/// `takes_required` admits, and the bundle where `takes_bundle` holds.
fn operation_arg_normalizations(
    operation: &Operation,
    api: &Api,
    names: &Names,
    takes_required: impl Fn(ParamLoc) -> bool,
    takes_bundle: bool,
) -> Vec<TokenStream> {
    let operation_names = names.operation(&operation.id);
    let bindings = &operation_names.bindings;
    let params_ident = &operation_names.params_struct;
    let mut statements = Vec::new();
    let required = operation_names
        .params(operation)
        .filter(|(param, _)| param.required && takes_required(param.location));
    for (param, ident) in required {
        if takes_into_string(api, param.ty) {
            let ty = ty_tokens(param.ty, names, true);
            statements.push(quote! { let #ident: #ty = #ident.into(); });
        }
    }
    if let Some(params_binding) = bindings.params.as_ref().filter(|_| takes_bundle) {
        statements.push(quote! {
            let #params_binding: Option<#params_ident> = #params_binding.into();
        });
    }
    statements
}

/// The typed method arguments and their bare forwarding names for an operation. Shared by
/// [`emit_operation`] (the async method) and `blocking::emit_blocking_operation` (its synchronous
/// shim), as [`operation_return_ty`] and [`param_default_docs_tokens`] are, so the two signatures
/// are built from one source. The first vector holds `name: Type` argument declarations; the
/// second holds just the `name`s, in the same order, for the shim's `self.inner.<op>(<names>)`
/// forwarding call. Body args are already `&T`, so the forwarding name (`body`) passes the
/// reference straight through.
pub(super) fn operation_args(
    operation: &Operation,
    api: &Api,
    names: &Names,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let operation_names = names.operation(&operation.id);
    let bindings = &operation_names.bindings;
    let params_ident = &operation_names.params_struct;
    let mut args = Vec::new();
    let mut forwards = Vec::new();
    for (param, ident) in operation_names
        .params(operation)
        .filter(|(param, _)| param.required)
    {
        // A plain `String` parameter accepts anything that converts, so a call site passes a
        // literal without `.to_owned()`. Narrowed to exactly `String`: every other generated type
        // keeps its concrete position, where `impl Into<T>` would buy nothing and cost inference.
        let ty = ty_tokens(param.ty, names, true);
        if takes_into_string(api, param.ty) {
            // `Into<#ty>`, not `Into<String>`: a `$ref`'d string component lowers to a transparent
            // `pub type WorkflowId = String`, so the bound is the same one either way — but naming
            // the component keeps the generated signature self-describing.
            args.push(quote! { #ident: impl Into<#ty> });
        } else {
            args.push(quote! { #ident: #ty });
        }
        forwards.push(quote! { #ident });
    }
    if let Some(params_binding) = &bindings.params {
        // `impl Into<Option<..>>` accepts the bundle directly as well as `None`/`Some(..)`, so a
        // caller who sets optional parameters passes the bundle without wrapping it in `Some`.
        args.push(quote! { #params_binding: impl Into<Option<#params_ident>> });
        forwards.push(quote! { #params_binding });
    }
    if let Some((ty, required)) = operation.request_body.as_ref().and_then(|body| {
        body.ty
            .map(|ty| (ty_tokens(ty, names, true), body.required))
    }) {
        let body_binding = bindings
            .body
            .as_ref()
            .expect("request body argument allocated");
        // A body the specification marks `required: false` may legitimately be omitted, so the
        // caller says so in the type rather than inventing an empty value.
        if required {
            args.push(quote! { #body_binding: &#ty });
        } else {
            args.push(quote! { #body_binding: Option<&#ty> });
        }
        forwards.push(quote! { #body_binding });
    }
    (args, forwards)
}

/// The `Ok`/`Err` types of an operation's `Result` return: `(return_ok_ty, error_ty)`. A streaming
/// success yields `EventStream<T>`, every other success `ResponseValue<T>`. Shared with the blocking
/// shim (see [`operation_args`]).
pub(super) fn operation_return_ty(
    operation: &Operation,
    names: &Names,
) -> (TokenStream, TokenStream) {
    let error_ident = &names.operation(&operation.id).error;
    let success_ty = success_type(operation, names);
    let return_ok_ty = match operation.responses.stream_success() {
        Some(_) => quote! { support::EventStream<#success_ty> },
        None => quote! { support::ResponseValue<#success_ty> },
    };
    (return_ok_ty, quote! { #error_ident })
}

/// The rustdoc `#[doc = …]` notes carrying required parameters' spec `default`s. Required params are
/// positional arguments with no attribute slot of their own, so their defaults are documented on the
/// method. Shared with the blocking shim (see [`operation_args`]).
pub(super) fn param_default_docs_tokens(operation: &Operation) -> Vec<TokenStream> {
    operation
        .params
        .iter()
        .filter(|param| param.required)
        .filter_map(|param| {
            param.default_display.as_ref().map(|default| {
                let note =
                    normalize_rustdoc(&format!("Parameter `{}` default: `{default}`.", param.name));
                quote! { #[doc = #note] }
            })
        })
        .collect()
}
