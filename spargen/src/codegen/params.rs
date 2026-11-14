//! Parameter serialization: each location's per-parameter statements an operation method runs,
//! and the `…Params` struct.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, MediaType, Operation, ParamLoc, Ty};
use crate::name::{Names, OperationBindings, OperationNames};

use super::docs::normalize_rustdoc;
use super::operation::takes_into_string;
use super::ty::ty_tokens;

/// The percent-encoding set for one parameter, derived from its location, style, and
/// `allowReserved`.
///
/// Headers and OpenAPI 3.2's `style: cookie` are sent verbatim — the specification says values
/// there must not be percent-encoded, and that escaping is the caller's job. `in: cookie` with
/// `style: form` *does* encode: Appendix D describes that pairing as the way to opt into automatic
/// encoding, with `allowReserved` as its escape hatch. Every other location but the path takes
/// [`query_encoding_tokens`].
fn percent_encoding_tokens(param: &crate::ir::Parameter) -> TokenStream {
    if param.location == ParamLoc::Header || matches!(param.style, crate::ir::ParamStyle::Cookie) {
        encoding_variant_tokens("Passthrough")
    } else if param.location == ParamLoc::Path {
        encoding_variant_tokens(if param.allow_reserved {
            "ReservedPath"
        } else {
            "Unreserved"
        })
    } else {
        query_encoding_tokens(param.allow_reserved)
    }
}

/// The percent-encoding set of a query-style value — a query or `style: form` cookie parameter,
/// a whole-query value, or an RFC 6570-mode form or multipart body property: reserved expansion
/// under `allowReserved`, the `form-urlencoded` set otherwise.
pub(super) fn query_encoding_tokens(allow_reserved: bool) -> TokenStream {
    encoding_variant_tokens(if allow_reserved { "Reserved" } else { "Form" })
}

/// The `support::PercentEncoding` variant named `variant`.
pub(super) fn encoding_variant_tokens(variant: &str) -> TokenStream {
    let ident = proc_macro2::Ident::new(variant, proc_macro2::Span::call_site());
    quote! { support::PercentEncoding::#ident }
}

/// The statements serializing every `location` parameter of `operation`, required ones first and
/// then optional ones, each in declaration order. `serialize` renders one parameter from the
/// expression its value is borrowed through: [`required_param_value`] for a required one, and
/// `value` for an optional one, whose statement runs only when the caller set it in the `…Params`
/// bundle.
pub(super) fn location_statements(
    operation: &Operation,
    names: &OperationNames,
    location: ParamLoc,
    serialize: impl Fn(&crate::ir::Parameter, TokenStream) -> TokenStream,
) -> Vec<TokenStream> {
    let at_location = |required: bool| {
        names
            .params(operation)
            .filter(move |(param, _)| param.required == required && param.location == location)
    };
    let required = at_location(true).map(|(param, ident)| {
        serialize(param, required_param_value(&names.bindings, param, ident))
    });
    let optional = at_location(false).map(|(param, ident)| {
        optional_param_guard(
            operation,
            &names.bindings,
            param,
            ident,
            serialize(param, quote! { value }),
        )
    });
    required.chain(optional).collect()
}

/// The expression a required parameter's value is borrowed through: `&ident`, the method
/// argument, for a path parameter, and `&params.ident`, its field of the `…Params` bundle the
/// method takes by value, for every other one.
pub(super) fn required_param_value(
    bindings: &OperationBindings,
    param: &crate::ir::Parameter,
    ident: &crate::name::Ident,
) -> TokenStream {
    if param.is_positional() {
        return quote! { &#ident };
    }
    let params_binding = params_binding(bindings);
    quote! { &#params_binding.#ident }
}

/// The `…Params` argument's binding, which an operation with a non-path parameter takes.
fn params_binding(bindings: &OperationBindings) -> &crate::name::Ident {
    bindings
        .params
        .as_ref()
        .expect("`…Params` argument allocated")
}

/// The statement that runs `body` with `value` bound to an optional parameter's value, when the
/// caller set `ident` in the `…Params` bundle (and, where every parameter in it is optional and
/// the method takes it as an `Option`, supplied the bundle). A parameter that sends an explicit
/// `null` binds the whole `&Nullable<T>`, which its JSON codec writes as `null` or the value,
/// whenever it is not `Absent`; any other binds the `&T` its `Option` holds.
fn optional_param_guard(
    operation: &Operation,
    bindings: &OperationBindings,
    param: &crate::ir::Parameter,
    ident: &crate::name::Ident,
    body: TokenStream,
) -> TokenStream {
    let params_binding = params_binding(bindings);
    if operation.params_required() {
        // The bundle is always there, so the field is read directly.
        return if param.sends_explicit_null() {
            quote! {
                if !#params_binding.#ident.is_absent() {
                    let value = &#params_binding.#ident;
                    #body
                }
            }
        } else {
            quote! {
                if let Some(value) = #params_binding.#ident.as_ref() {
                    #body
                }
            }
        };
    }
    let set = if param.sends_explicit_null() {
        quote! {
            .filter(|params| !params.#ident.is_absent())
            .map(|params| &params.#ident)
        }
    } else {
        quote! { .and_then(|params| params.#ident.as_ref()) }
    };
    quote! {
        if let Some(value) = #params_binding
            .as_ref()
            #set
        {
            #body
        }
    }
}

/// The runtime `Delimiter` for a non-RFC 6570 query style.
pub(super) fn delimiter_tokens(delimiter: crate::ir::Delimiter) -> TokenStream {
    match delimiter {
        crate::ir::Delimiter::Space => quote! { support::Delimiter::Space },
        crate::ir::Delimiter::Pipe => quote! { support::Delimiter::Pipe },
    }
}

/// Render a parameter value from a borrowed expression: a path or header parameter of any style,
/// or a `content`-typed parameter in any location. Schema-typed parameters use their declared
/// style (`matrix`, `label`, and `simple` for every other); `content`-typed parameters retain
/// their media codec.
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
            _ => {
                let passthrough = encoding_variant_tokens("Passthrough");
                quote! {
                    support::serialize_simple(#value, false, #passthrough)
                        .map_err(support::Error::request_construction)?
                }
            }
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
    let name = &param.name;
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
/// Fragments arrive at `support::build_url_on` (or `build_url_with_query_string_on`, beside a
/// whole-query value) already percent-encoded, so the style's delimiters stay
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
            form_fragments_tokens(query_binding, name, value, param.explode, &encoding)
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
    let encoding = query_encoding_tokens(false);
    quote! {
        support::encode(
            &serde_json::to_string(#value).map_err(support::Error::request_construction)?,
            #encoding,
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
        crate::ir::ParamStyle::Content(MediaType::FormUrlEncoded) => form_fragments_tokens(
            query_binding,
            "",
            value,
            true,
            &query_encoding_tokens(false),
        ),
        _ => quote! {},
    }
}

/// The statement extending `binding` with the `name=value` fragments `support::serialize_form`
/// renders for `value`: a `form`-style query or cookie parameter, or a form-encoded whole-query
/// value (whose empty `name` makes each member its own pair).
fn form_fragments_tokens(
    binding: &crate::name::Ident,
    name: &str,
    value: TokenStream,
    explode: bool,
    encoding: &TokenStream,
) -> TokenStream {
    quote! {
        #binding.extend(
            support::serialize_form(#name, #value, #explode, #encoding)
                .map_err(support::Error::request_construction)?,
        );
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
            form_fragments_tokens(cookies_binding, name, value, param.explode, &encoding)
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

/// The emitted doc of an explicit-null parameter's `Nullable<T>` field: which of its three states
/// sends the JSON `null` and which omits the parameter.
const EXPLICIT_NULL_FIELD_DOC: &str =
    "`Nullable::Null` sends the JSON `null`; `Nullable::Absent` omits the parameter.";

/// The emitted doc of an explicit-null parameter's setter, the same contract seen from the
/// setter's argument.
const EXPLICIT_NULL_SETTER_DOC: &str =
    "`None` sends the JSON `null`; leaving the parameter unset omits it.";

/// Emit an operation's `…Params` struct, which holds every parameter but a path one in a public
/// field, plus an `impl` of fluent `#[must_use]` consuming setters — one per optional param, named
/// after its field. Where every parameter in it is optional, the struct derives `Default`, so
/// callers write `…Params::default().foo(x).bar(y)` instead of a struct literal; where one is
/// required, `…Params::new(required…)` takes the required ones in declaration order, by name at
/// the call site's `.foo(x)` for the rest, so two same-typed required parameters can never trade
/// places unseen behind a positional method argument.
pub(super) fn emit_params_struct(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let ident = &operation_names.params_struct;
    // The setter method reuses the field ident verbatim, so the two can never disagree.
    let bundled: Vec<(&crate::ir::Parameter, &crate::name::Ident)> = operation_names
        .params(operation)
        .filter(|(param, _)| !param.is_positional())
        .collect();
    let optional = bundled.iter().filter(|(param, _)| !param.required);
    let fields = bundled.iter().map(|(param, ident)| {
        let wire = &param.name;
        if param.required {
            // A required parameter is always sent, so its field is its own type, which
            // `ty_tokens` already wraps in `Option` where it is nullable, serialized as `null`.
            let ty = ty_tokens(param.ty, names, true);
            let notes = param_notes(param)
                .into_iter()
                .map(|note| normalize_rustdoc(&note))
                .map(|note| quote! { #[doc = #note] });
            return quote! {
                #(#notes)*
                #[serde(rename = #wire)]
                pub #ident: #ty,
            };
        }
        // Every struct param is optional, so its unset state is omitted. A nullable JSON
        // `content` param is a `support::Nullable` over the non-nullable type, because its codec
        // sends `Null` as the text `null` (#996). Every other param is an `Option`, its `None`
        // omitted: `ty_tokens` already wraps a nullable one (`"null"` in its type array) in
        // `Option`, collapsing absent and `null` to `None`, since its serialization has no token
        // for `null` (see `Parameter::sends_explicit_null`).
        let (ty, skip) = if param.sends_explicit_null() {
            let inner = ty_tokens(
                Ty {
                    nullable: false,
                    ..param.ty
                },
                names,
                true,
            );
            (
                quote! { support::Nullable<#inner> },
                "support::Nullable::is_absent",
            )
        } else if param.ty.nullable {
            (ty_tokens(param.ty, names, true), "Option::is_none")
        } else {
            let inner = ty_tokens(param.ty, names, true);
            (quote! { Option<#inner> }, "Option::is_none")
        };
        let mut notes = param_notes(param);
        if param.sends_explicit_null() {
            notes.push(EXPLICIT_NULL_FIELD_DOC.to_owned());
        }
        let notes = notes
            .iter()
            .map(|note| normalize_rustdoc(note))
            .map(|note| quote! { #[doc = #note] });
        quote! {
            #(#notes)*
            #[serde(rename = #wire, skip_serializing_if = #skip)]
            pub #ident: #ty,
        }
    });
    // Fluent consuming setters, one per optional param, in field order. One whose field is an
    // `Option<T>`, nullable or not, takes the bare `T` by value and stores `Some` of it; by value
    // (not `impl Into<T>`) keeps inference trivial for every generated field type. One that sends
    // an explicit `null` takes `impl Into<Nullable<T>>`, as a nullable model member's setter does,
    // so `value`, `Some(value)` and `None` (that `null`) all fit.
    let setters = optional.clone().map(|(param, ident)| {
        let inner = ty_tokens(
            Ty {
                nullable: false,
                ..param.ty
            },
            names,
            true,
        );
        let doc = format!("Set the `{}` parameter.", param.name);
        if param.sends_explicit_null() {
            quote! {
                #[doc = #doc]
                #[doc = ""]
                #[doc = #EXPLICIT_NULL_SETTER_DOC]
                #[must_use]
                pub fn #ident(mut self, value: impl Into<support::Nullable<#inner>>) -> Self {
                    self.#ident = value.into();
                    self
                }
            }
        } else {
            quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #ident(mut self, value: #inner) -> Self {
                    self.#ident = Some(value);
                    self
                }
            }
        }
    });
    let (derive_default, constructor) = if operation.params_required() {
        (None, Some(params_constructor(&bundled, api, names)))
    } else {
        (Some(quote! { Default, }), None)
    };
    quote! {
        #[allow(dead_code, deprecated)]
        #[derive(Debug, Clone, #derive_default serde::Serialize)]
        pub struct #ident {
            #(#fields)*
        }

        // Setters named after fields can trip `wrong_self_convention` when a param is named
        // `is_*`/`to_*`/etc.; a consuming builder setter is the intended shape, so allow it here.
        #[allow(dead_code, deprecated, clippy::wrong_self_convention)]
        impl #ident {
            #constructor
            #(#setters)*
        }
    }
}

/// The rustdoc notes a `…Params` field carries for its parameter's `deprecated` and `default`.
fn param_notes(param: &crate::ir::Parameter) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    if param.deprecated {
        notes.push("Deprecated per the spec.".to_owned());
    }
    if let Some(default) = &param.default_display {
        notes.push(format!("Default: `{default}`."));
    }
    notes
}

/// `…Params::new`, emitted where a bundled parameter is required: one argument per required
/// parameter, in declaration order and named after its field, and every optional one unset. A
/// plain `String` argument is an `impl Into<String>`, as a path parameter's is.
fn params_constructor(
    bundled: &[(&crate::ir::Parameter, &crate::name::Ident)],
    api: &Api,
    names: &Names,
) -> TokenStream {
    let required = bundled.iter().filter(|(param, _)| param.required);
    let args = required.clone().map(|(param, ident)| {
        let ty = ty_tokens(param.ty, names, true);
        if takes_into_string(api, param.ty) {
            quote! { #ident: impl Into<#ty> }
        } else {
            quote! { #ident: #ty }
        }
    });
    let inits = bundled.iter().map(|(param, ident)| {
        if param.required {
            if takes_into_string(api, param.ty) {
                quote! { #ident: #ident.into(), }
            } else {
                quote! { #ident, }
            }
        } else if param.sends_explicit_null() {
            quote! { #ident: support::Nullable::Absent, }
        } else {
            quote! { #ident: None, }
        }
    });
    let wires: Vec<String> = required
        .map(|(param, _)| format!("`{}`", param.name))
        .collect();
    let doc = normalize_rustdoc(&format!(
        "The parameters with every required one given, in declaration order ({}), and every \
         optional one unset.",
        wires.join(", ")
    ));
    // A description decides how many parameters are required, and a consumer's `clippy.toml`
    // where `too_many_arguments` fires, so `new` allows it whatever its arity.
    quote! {
        #[doc = #doc]
        #[must_use]
        #[allow(clippy::too_many_arguments)]
        pub fn new(#(#args),*) -> Self {
            Self {
                #(#inits)*
            }
        }
    }
}
