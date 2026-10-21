//! One async operation method on `Client`, and the signature pieces its blocking twin shares.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, MediaType, Operation, ParamLoc, Ty, TypeKind};
use crate::name::Names;

use super::body::body_send_tokens;
use super::dispatch::{
    attach_auth_tokens, error_branch_tokens, success_decode_tokens, success_type,
};
use super::docs::{doc_tokens, normalize_rustdoc};
use super::idents::{
    error_type_ident, operation_bindings, operation_method_ident, param_ident, params_struct_ident,
};
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
pub(super) fn emit_operation(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let bindings = operation_bindings(operation, names);
    let path_binding = &bindings.path;
    let query_binding = &bindings.query;
    let raw_query_binding = &bindings.raw_query;
    let url_binding = &bindings.url;
    let request_binding = &bindings.request;
    let cookies_binding = &bindings.cookies;
    let method_ident = operation_method_ident(operation, names);
    let error_ident = error_type_ident(operation, names);
    let reqwest_method = reqwest_method(&operation.method);
    let success_ty = success_type(operation, names);
    let error_ty = quote! { #error_ident };
    let docs = doc_tokens(&operation.docs);
    // Required parameters are positional method arguments with no attribute slot of their own, so a
    // required parameter's `default` is surfaced in the method rustdoc instead.
    let param_default_docs = param_default_docs_tokens(operation);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });

    // The typed argument list (required params, the optional-params struct, the body) is shared
    // verbatim with the blocking shim so the two signatures can never drift.
    let (args, _arg_names) = operation_args(operation, api, names);

    let path_init = operation.path.raw.clone();
    let path_replacements = operation
        .params
        .iter()
        .filter(|param| param.location == ParamLoc::Path)
        .map(|param| {
            let placeholder = format!("{{{}}}", param.name);
            let ident = param_ident(names, operation, param);
            let value = param_value_tokens(param, quote! { &#ident });
            quote! {
                #path_binding = #path_binding.replace(#placeholder, &#value);
            }
        });
    let query = location_statements(
        operation,
        names,
        bindings,
        ParamLoc::Query,
        |param, value| query_param_tokens(param, &param.name, value, query_binding),
    );
    let uses_querystring = operation
        .params
        .iter()
        .any(|parameter| parameter.location == ParamLoc::QueryString);
    // Lowering admits at most one `in: querystring` parameter per operation, so this is that one.
    let json_querystring = operation.params.iter().find(|parameter| {
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
        Some(parameter) if parameter.required => {
            let ident = param_ident(names, operation, parameter);
            let encoded = json_querystring_tokens(quote! { &#ident });
            quote! { let #raw_query_binding: Option<String> = Some(#encoded); }
        }
        Some(_) => quote! { let mut #raw_query_binding: Option<String> = None; },
        None => quote! { let #raw_query_binding: Option<String> = None; },
    });
    let querystring = location_statements(
        operation,
        names,
        bindings,
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
    let headers = location_statements(
        operation,
        names,
        bindings,
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
        names,
        bindings,
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
    let body_send = body_send_tokens(operation, api, names, bindings);
    let attach_auth = attach_auth_tokens(operation, api, request_binding);
    let error_branch = error_branch_tokens(operation, api, names, error_ident);
    let (success_decode, reconnect_request_init) =
        success_decode_tokens(operation, api, names, bindings, &success_ty, error_ident);
    // The return type is shared with the blocking shim so both surfaces stay identical.
    let (return_ok_ty, _) = operation_return_ty(operation, names);

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

    let arg_normalizations = operation_arg_normalizations(operation, api, names);

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
            let mut #path_binding = #path_init.to_owned();
            #(#path_replacements)*
            let mut #query_binding: Vec<String> = Vec::new();
            #raw_query_init
            #(#query)*
            #(#querystring)*
            let #url_binding = #build_url
                .map_err(support::Error::widen)?;
            let mut #request_binding = self.core.http().request(#reqwest_method, #url_binding);
            #(#headers)*
            #cookie_init
            #(#cookies)*
            #cookie_attach
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
fn operation_arg_normalizations(
    operation: &Operation,
    api: &Api,
    names: &Names,
) -> Vec<TokenStream> {
    let bindings = operation_bindings(operation, names);
    let params_ident = params_struct_ident(operation, names);
    let mut statements = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        if takes_into_string(api, param.ty) {
            let ident = param_ident(names, operation, param);
            let ty = ty_tokens(param.ty, names, true);
            statements.push(quote! { let #ident: #ty = #ident.into(); });
        }
    }
    if let Some(params_binding) = &bindings.params {
        statements.push(quote! {
            let #params_binding: Option<#params_ident> = #params_binding.into();
        });
    }
    statements
}

/// The typed method arguments and their bare forwarding names for an operation. Shared by
/// [`emit_operation`] (the async method) and
/// `blocking::emit_blocking_operation` (its synchronous shim, private to that module) so
/// the two signatures are constructed from one source and can never drift. The first vector holds
/// `name: Type` argument declarations; the second holds just the `name`s, in the same order, for the
/// shim's `self.inner.<op>(<names>)` forwarding call. Body args are already `&T`, so the forwarding
/// name (`body`) passes the reference straight through.
pub(super) fn operation_args(
    operation: &Operation,
    api: &Api,
    names: &Names,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let bindings = operation_bindings(operation, names);
    let params_ident = params_struct_ident(operation, names);
    let mut args = Vec::new();
    let mut forwards = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        let ident = param_ident(names, operation, param);
        // A plain `String` parameter accepts anything that converts, so a call site passes a
        // literal without `.to_owned()`. Narrowed to exactly `String`: every other generated type
        // keeps its concrete position, where `impl Into<T>` would buy nothing and cost inference.
        let ty = ty_tokens(param.ty, names, true);
        if takes_into_string(api, param.ty) {
            // `Into<#ty>`, not `Into<String>`: a `$ref`'d string component lowers to a transparent
            // `pub type WorkflowId = String`, so the bound is the same one either way — but naming
            // the component keeps the generated signature as self-describing as it was.
            args.push(quote! { #ident: impl Into<#ty> });
        } else {
            args.push(quote! { #ident: #ty });
        }
        forwards.push(quote! { #ident });
    }
    if let Some(params_binding) = &bindings.params {
        // `impl Into<Option<..>>` accepts the bundle directly as well as `None`/`Some(..)`, so a
        // caller who sets optional parameters no longer wraps them in `Some`.
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
/// shim so the async and sync return types stay identical.
pub(super) fn operation_return_ty(
    operation: &Operation,
    names: &Names,
) -> (TokenStream, TokenStream) {
    let error_ident = error_type_ident(operation, names);
    let success_ty = success_type(operation, names);
    let return_ok_ty = match operation.responses.stream_success() {
        Some(_) => quote! { support::EventStream<#success_ty> },
        None => quote! { support::ResponseValue<#success_ty> },
    };
    (return_ok_ty, quote! { #error_ident })
}

/// The rustdoc `#[doc = …]` notes carrying required parameters' spec `default`s. Required params are
/// positional arguments with no attribute slot of their own, so their defaults are documented on the
/// method. Shared verbatim between the async method and its blocking shim.
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
