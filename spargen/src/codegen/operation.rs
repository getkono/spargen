//! One async operation method on `Client` with its URL accessor, and the [`OperationSig`] its
//! blocking twin shares.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, MediaType, Operation, ParamLoc, Ty, TypeKind};
use crate::name::{Names, OperationNames};

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
pub(super) fn emit_operation(
    operation: &Operation,
    sig: &OperationSig,
    api: &Api,
    names: &Names,
) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let bindings = &operation_names.bindings;
    let url_binding = &bindings.url;
    let request_binding = &bindings.request;
    let cookies_binding = &bindings.cookies;
    let method_ident = &operation_names.method;
    let error_ident = &operation_names.error;
    let reqwest_method = reqwest_method(&operation.method);
    let OperationSig {
        attrs,
        args,
        success_ty,
        return_ok_ty,
        error_ty,
        ..
    } = sig;
    let decls = args.iter().map(|arg| &arg.decl);
    let normalizations = args.iter().filter_map(|arg| arg.normalize.as_ref());
    let url_building = url_building(operation, operation_names);
    let UrlBuilding {
        statements: url_statements,
        build_url,
    } = &url_building;

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
        success_decode_tokens(operation, api, names, bindings, success_ty, error_ident);
    let url_accessor = emit_operation_url(operation_names, sig, &url_building);
    // A streaming success response is exempt from the client's total timeout, which would cut a
    // long-lived stream off; `send_streaming` is `send` without it.
    let send = if operation.responses.stream_success().is_some() {
        quote! { support::send_streaming }
    } else {
        quote! { support::send }
    };

    quote! {
        #attrs
        #[inline]
        pub async fn #method_ident(
            &self,
            #(#decls),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            #(#normalizations)*
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
            let response = #send(&self.core, #request_binding)
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

/// Emit one operation's URL accessor on `Client`, built by the operation method's own
/// `url_building` statements.
fn emit_operation_url(
    names: &OperationNames,
    sig: &OperationSig,
    url_building: &UrlBuilding,
) -> TokenStream {
    let normalizations = sig.url_args().filter_map(|arg| arg.normalize.as_ref());
    let UrlBuilding {
        statements,
        build_url,
    } = url_building;
    url_accessor_tokens(
        names,
        sig,
        quote! {
            #(#normalizations)*
            #statements
            #build_url
        },
    )
}

/// An operation's URL accessor with `body`, on `Client` and `BlockingClient` alike: its doc, the
/// operation's deprecation, and the [`OperationSig::url_args`] it takes.
pub(super) fn url_accessor_tokens(
    names: &OperationNames,
    sig: &OperationSig,
    body: TokenStream,
) -> TokenStream {
    let url_ident = &names.url_accessor;
    let doc = url_accessor_doc(&names.method);
    let deprecated = &sig.deprecated;
    let decls = sig.url_args().map(|arg| &arg.decl);
    quote! {
        #[doc = #doc]
        #deprecated
        #[inline]
        pub fn #url_ident(
            &self,
            #(#decls),*
        ) -> Result<reqwest::Url, support::Error<std::convert::Infallible>> {
            #body
        }
    }
}

/// The rustdoc of an operation's URL accessor.
fn url_accessor_doc(method_ident: &crate::name::Ident) -> String {
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
/// URL a caller is handed is the URL the request goes to. Built once per operation.
struct UrlBuilding {
    statements: TokenStream,
    build_url: TokenStream,
}

/// Build an operation's [`UrlBuilding`] over the bindings `name` allocated for it.
fn url_building(operation: &Operation, operation_names: &OperationNames) -> UrlBuilding {
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

/// Every operation's [`OperationSig`], in `api.operations` order.
pub(super) fn signatures(api: &Api, names: &Names) -> Vec<OperationSig> {
    api.operations
        .iter()
        .map(|operation| OperationSig::new(operation, api, names))
        .collect()
}

/// One argument of an operation method.
pub(super) struct Arg {
    /// The `name: Type` declaration.
    decl: TokenStream,
    /// The bare name, which the blocking shim forwards. A body argument is already `&T`, so its
    /// name passes the reference straight through.
    forward: TokenStream,
    /// The statement converting an `impl Into<..>` argument to its concrete type once, at the top
    /// of the body, so the rest of the body is written against concrete types: a required
    /// parameter [`takes_into_string`] admits, and the optional-parameters bundle from
    /// `impl Into<Option<…Params>>` to `Option<…Params>`.
    normalize: Option<TokenStream>,
    /// Whether the URL accessor takes it too: a required parameter in a location that shapes the
    /// URL, and the bundle where one of its parameters does. A header, a cookie or the request
    /// body changes nothing in the URL, so the accessor does not ask for one.
    shapes_url: bool,
}

/// An operation method's signature, built once per operation by [`OperationSig::new`] and shared
/// by the async method, its blocking shim, and both of their URL accessors, so every one of them
/// is declared from one source.
pub(super) struct OperationSig {
    /// The method's rustdoc, its required parameters' deprecation and `default` notes, and its
    /// deprecation.
    attrs: TokenStream,
    /// `#[deprecated]` where the operation is, for the method and its URL accessor alike.
    deprecated: Option<TokenStream>,
    /// The required parameters, then the optional-parameters bundle, then the request body.
    args: Vec<Arg>,
    /// The `T` the success yields; see [`success_type`].
    success_ty: TokenStream,
    /// The `Ok` type of the method's `Result`: `EventStream<T>` for a streaming success,
    /// `ResponseValue<T>` for every other.
    return_ok_ty: TokenStream,
    /// The `E` of the method's `support::Error<E>`.
    error_ty: TokenStream,
}

impl OperationSig {
    /// The signature of `operation`'s method.
    pub(super) fn new(operation: &Operation, api: &Api, names: &Names) -> Self {
        let operation_names = names.operation(&operation.id);
        let bindings = &operation_names.bindings;
        let params_ident = &operation_names.params_struct;

        let docs = doc_tokens(&operation.docs);
        // Required parameters are positional method arguments with no attribute slot of their
        // own, so a required parameter's deprecation and `default` are surfaced in the method
        // rustdoc instead, in the order an optional parameter's `…Params` field carries them.
        let param_docs = operation
            .params
            .iter()
            .filter(|param| param.required)
            .flat_map(|param| {
                let deprecated = param
                    .deprecated
                    .then(|| format!("Parameter `{}` is deprecated per the spec.", param.name));
                let default = param
                    .default_display
                    .as_ref()
                    .map(|default| format!("Parameter `{}` default: `{default}`.", param.name));
                deprecated.into_iter().chain(default)
            })
            .map(|note| {
                let note = normalize_rustdoc(&note);
                quote! { #[doc = #note] }
            });
        let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });
        let attrs = quote! {
            #docs
            #(#param_docs)*
            #deprecated
        };

        let mut args = Vec::new();
        for (param, ident) in operation_names
            .params(operation)
            .filter(|(param, _)| param.required)
        {
            // A plain `String` parameter accepts anything that converts, so a call site passes a
            // literal without `.to_owned()`. Narrowed to exactly `String`: every other generated
            // type keeps its concrete position, where `impl Into<T>` would buy nothing and cost
            // inference.
            let ty = ty_tokens(param.ty, names, true);
            let (decl, normalize) = if takes_into_string(api, param.ty) {
                // `Into<#ty>`, not `Into<String>`: a `$ref`'d string component lowers to a
                // transparent `pub type WorkflowId = String`, so the bound is the same one either
                // way — but naming the component keeps the generated signature self-describing.
                (
                    quote! { #ident: impl Into<#ty> },
                    Some(quote! { let #ident: #ty = #ident.into(); }),
                )
            } else {
                (quote! { #ident: #ty }, None)
            };
            args.push(Arg {
                decl,
                forward: quote! { #ident },
                normalize,
                shapes_url: param.is_url_arg(),
            });
        }
        if let Some(params_binding) = &bindings.params {
            // `impl Into<Option<..>>` accepts the bundle directly as well as `None`/`Some(..)`, so
            // a caller who sets optional parameters passes the bundle without wrapping it in
            // `Some`.
            args.push(Arg {
                decl: quote! { #params_binding: impl Into<Option<#params_ident>> },
                forward: quote! { #params_binding },
                normalize: Some(quote! {
                    let #params_binding: Option<#params_ident> = #params_binding.into();
                }),
                shapes_url: operation.url_takes_params(),
            });
        }
        if let Some((body, ty)) = operation.typed_body() {
            let ty = ty_tokens(ty, names, true);
            let body_binding = bindings
                .body
                .as_ref()
                .expect("request body argument allocated");
            // A body the specification marks `required: false` may legitimately be omitted, so
            // the caller says so in the type rather than inventing an empty value.
            let decl = if body.required {
                quote! { #body_binding: &#ty }
            } else {
                quote! { #body_binding: Option<&#ty> }
            };
            args.push(Arg {
                decl,
                forward: quote! { #body_binding },
                normalize: None,
                shapes_url: false,
            });
        }

        let success_ty = success_type(operation, names);
        let return_ok_ty = match operation.responses.stream_success() {
            Some(_) => quote! { support::EventStream<#success_ty> },
            None => quote! { support::ResponseValue<#success_ty> },
        };
        let error_ident = &operation_names.error;
        Self {
            attrs,
            deprecated,
            args,
            success_ty,
            return_ok_ty,
            error_ty: quote! { #error_ident },
        }
    }

    /// The method's argument declarations.
    pub(super) fn decls(&self) -> impl Iterator<Item = &TokenStream> {
        self.args.iter().map(|arg| &arg.decl)
    }

    /// The method's argument names, in declaration order, for a call forwarding them.
    pub(super) fn forwards(&self) -> impl Iterator<Item = &TokenStream> {
        self.args.iter().map(|arg| &arg.forward)
    }

    /// The arguments the URL accessor takes, in declaration order.
    pub(super) fn url_args(&self) -> impl Iterator<Item = &Arg> {
        self.args.iter().filter(|arg| arg.shapes_url)
    }

    /// The method's rustdoc, required-parameter deprecation and `default` notes, and deprecation.
    pub(super) fn attrs(&self) -> &TokenStream {
        &self.attrs
    }

    /// The `Ok` and `E` types of the method's `Result<Ok, support::Error<E>>`.
    pub(super) fn return_tys(&self) -> (&TokenStream, &TokenStream) {
        (&self.return_ok_ty, &self.error_ty)
    }
}

impl Arg {
    /// The name a call forwarding this argument passes.
    pub(super) fn forward(&self) -> &TokenStream {
        &self.forward
    }
}
