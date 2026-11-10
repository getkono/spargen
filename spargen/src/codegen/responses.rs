//! The types an operation's responses are read into: the multi-status success enum, the typed
//! error, and the documented response-header structs.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, ErrorShape, Operation, StatusSpec, SuccessShape, Ty, TypeGraph};
use crate::name::Names;

use super::docs::doc_tokens;
use super::idents::status_variant_ident;
use super::ty::{boxed_ty_tokens, is_bytes_ty, ty_tokens};

/// Emit one typed header struct per documented status that declares response headers.
///
/// Reading headers is an explicitly-called second step rather than part of the return type: the
/// body has already been decoded and handed back before a caller opts in, so a malformed or absent
/// header can never turn a successful call into a failed one. Every header also stays reachable
/// raw through `ResponseValue::headers`.
pub(super) fn emit_response_headers(
    operation: &Operation,
    names: &Names,
    options: &super::CodegenOptions,
) -> TokenStream {
    let responses = operation
        .responses
        .entries()
        .map(|(spec, response)| (crate::name::status_label(spec), response));
    let structs = responses.filter_map(|(label, response)| {
        if response.headers.is_empty() {
            return None;
        }
        let header_names = names.operation(&operation.id).response_headers(&label);
        let ident = &header_names.ty;
        let field_ident = |header: &crate::ir::ResponseHeader| header_names.field(&header.name);
        let fields = response.headers.iter().map(|header| {
            let field = field_ident(header);
            // Header structs live beside `Client`, not inside `types`, so the type path must be
            // qualified — a header whose schema lowered to a named type would not resolve here.
            let mut ty = ty_tokens(header.ty, names, true);
            // An optional header is absent-able; a required one is documented as always present.
            if !header.required {
                ty = quote! { Option<#ty> };
            }
            let docs = doc_tokens(&header.docs, options);
            let deprecated = header.deprecated.then(|| quote! { #[deprecated] });
            quote! { #docs #deprecated pub #field: #ty }
        });
        let reads = response.headers.iter().map(|header| {
            let field = field_ident(header);
            let name = &header.name;
            let shape = match header.shape {
                crate::ir::HeaderShape::Scalar => quote! { support::HeaderShape::Scalar },
                crate::ir::HeaderShape::Array => quote! { support::HeaderShape::Array },
                crate::ir::HeaderShape::Object => quote! { support::HeaderShape::Object },
                crate::ir::HeaderShape::Json => quote! { support::HeaderShape::Json },
                crate::ir::HeaderShape::SetCookie => quote! { support::HeaderShape::SetCookie },
            };
            let explode = header.explode;
            let call = if header.required {
                quote! { support::require_header(headers, #name, #shape, #explode)? }
            } else {
                quote! { support::parse_header(headers, #name, #shape, #explode)? }
            };
            quote! { #field: #call }
        });
        let doc = format!(
            "Documented response headers for `{}` `{label}`.",
            operation.id.0
        );
        Some(quote! {
            #[doc = #doc]
            #[allow(dead_code, deprecated)]
            #[derive(Debug, Clone)]
            pub struct #ident {
                #(#fields),*
            }

            #[allow(dead_code, deprecated)]
            impl #ident {
                /// Read the documented headers out of a raw header map.
                pub fn from_headers(
                    headers: &reqwest::header::HeaderMap,
                ) -> Result<Self, support::HeaderError> {
                    Ok(Self { #(#reads),* })
                }

                /// Read the documented headers out of a returned response value.
                pub fn from_response<T>(
                    response: &support::ResponseValue<T>,
                ) -> Result<Self, support::HeaderError> {
                    Self::from_headers(response.headers())
                }
            }
        })
    });
    quote! { #(#structs)* }
}

/// Emit an operation's multi-status success response enum, one variant per documented success
/// status (empty unless [`Responses::success`](crate::ir::Responses::success) is an enum: two or
/// more success bodies, or one beside a documented bodyless status). The variant
/// is selected by HTTP status at decode time, so the enum derives only `Debug, Clone` — no
/// whole-enum `Deserialize`, no `serde(untagged)`.
///
/// Its accessors are generated exhaustively, one arm per variant, so a newly documented status
/// updates them rather than every caller's `match`: `status()` when every variant is an exact
/// status, and `body()`/`into_body()` when every bodied variant carries one generated type.
pub(super) fn emit_response_enum(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Enum(entries) => {
            let ident = names.operation(&operation.id).success_enum();
            let variants = entries
                .iter()
                .map(|(spec, ty)| response_variant_def(*spec, *ty, names));
            let status = status_method(&entries);
            let body = shared_body(&entries, &api.types).map(|body| {
                // Every variant carrying a non-null body hands one back unconditionally; a
                // bodyless or nullable variant makes the body optional.
                let always = entries
                    .iter()
                    .all(|(_, ty)| ty.is_some_and(|ty| !ty.nullable));
                body_methods(&entries, body, always, names)
            });
            let accessors = (status.is_some() || body.is_some()).then(|| {
                quote! {
                    #[allow(dead_code, deprecated)]
                    impl #ident {
                        #status
                        #body
                    }
                }
            });
            quote! {
                #[allow(dead_code, deprecated)]
                #[derive(Debug, Clone)]
                pub enum #ident {
                    #(#variants)*
                }

                #accessors
            }
        }
        _ => quote! {},
    }
}

/// The one generated type every bodied entry carries, bare (unboxed, non-nullable, since a
/// variant's box and nullability are absorbed by its accessor arm), or `None` when two bodied
/// entries generate different types. The first bodied entry's type names the same Rust type as
/// every other's. This is [`ErrorShape::api_error_body`]'s rule, applied to either side's entries.
fn shared_body(entries: &[(StatusSpec, Option<Ty>)], types: &TypeGraph) -> Option<Ty> {
    let bare = |ty: Ty| Ty {
        nullable: false,
        boxed: false,
        ..ty
    };
    let mut bodies = entries.iter().filter_map(|(_, body)| body.map(bare));
    let first = bodies.next()?;
    bodies
        .all(|body| types.same_generated_type(first, body))
        .then_some(first)
}

/// A status enum's `status()`: one arm per variant answering the exact status it was dispatched
/// on, or `None` when any variant is a range or `default`, whose value alone does not say which
/// status of the several it covers was received. Each code is a [`status_const`].
fn status_method(entries: &[(StatusSpec, Option<Ty>)]) -> Option<TokenStream> {
    let arms = entries
        .iter()
        .map(|(spec, ty)| {
            let StatusSpec::Exact(code) = *spec else {
                return None;
            };
            let variant_ident = status_variant_ident(*spec);
            let pattern = match ty {
                Some(_) => quote! { Self::#variant_ident(_) },
                None => quote! { Self::#variant_ident },
            };
            let status = status_const(code);
            Some(quote! { #pattern => #status, })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(quote! {
        /// The HTTP status this value was dispatched on: each variant names exactly one.
        pub fn status(&self) -> reqwest::StatusCode {
            match self {
                #(#arms)*
            }
        }
    })
}

/// A documented exact status as a `StatusCode` expression evaluated at compile time. The
/// responses-key grammar admits only `100..=599`, all of which `from_u16` accepts, so the `panic!`
/// arm is unreachable, and a code outside it would fail the consumer's build, never a call.
fn status_const(code: u16) -> TokenStream {
    quote! {
        const {
            match reqwest::StatusCode::from_u16(#code) {
                Ok(status) => status,
                Err(_) => panic!("a documented status is a valid status code"),
            }
        }
    }
}

/// A status enum's `body()` and `into_body()` over the one type `body` its bodied variants share,
/// one arm per variant in the enum's own order. With `always`, every variant carries a non-null
/// body and the accessors return it bare; otherwise they return `Option`, `None` for a documented
/// bodyless status or a `null` payload.
fn body_methods(
    entries: &[(StatusSpec, Option<Ty>)],
    body: Ty,
    always: bool,
    names: &Names,
) -> TokenStream {
    let body_ty = ty_tokens(body, names, true);
    let wrap = |expr: TokenStream| {
        if always {
            expr
        } else {
            quote! { Some(#expr) }
        }
    };
    let arms = entries.iter().map(|(spec, ty)| {
        let variant_ident = status_variant_ident(*spec);
        match ty {
            // A nullable payload is `Option<Box<T>>`, reachable only where the body is optional.
            Some(ty) if ty.nullable => (
                quote! { Self::#variant_ident(body) => body.as_deref(), },
                quote! { Self::#variant_ident(body) => body.map(|body| *body), },
            ),
            // A boxed payload: `as_ref` borrows the `T` inside the `Box`, `*` moves it out.
            Some(_) => {
                let borrowed = wrap(quote! { body.as_ref() });
                let owned = wrap(quote! { *body });
                (
                    quote! { Self::#variant_ident(body) => #borrowed, },
                    quote! { Self::#variant_ident(body) => #owned, },
                )
            }
            None => (
                quote! { Self::#variant_ident => None, },
                quote! { Self::#variant_ident => None, },
            ),
        }
    });
    let (borrow_arms, own_arms): (Vec<_>, Vec<_>) = arms.unzip();
    let (borrowed_ty, owned_ty) = if always {
        (quote! { &#body_ty }, quote! { #body_ty })
    } else {
        (quote! { Option<&#body_ty> }, quote! { Option<#body_ty> })
    };
    let (borrow_doc, own_doc) = if always {
        (
            " The body this value carries, whichever status carried it.",
            " The body this value carries, moved out, whichever status carried it.",
        )
    } else {
        (
            " The body this value carries, whichever status carried it; `None` for a documented \
             bodyless status or a `null` body.",
            " The body this value carries, moved out, whichever status carried it; `None` for a \
             documented bodyless status or a `null` body.",
        )
    };
    quote! {
        #[doc = #borrow_doc]
        pub fn body(&self) -> #borrowed_ty {
            match self {
                #(#borrow_arms)*
            }
        }

        #[doc = #own_doc]
        pub fn into_body(self) -> #owned_ty {
            match self {
                #(#own_arms)*
            }
        }
    }
}

/// One response-enum variant definition: a payload-carrying `Status2xx(types::T)` for a bodied
/// status, or a payload-free `Status204` unit variant for a documented bodyless status.
fn response_variant_def(spec: crate::ir::StatusSpec, ty: Option<Ty>, names: &Names) -> TokenStream {
    let variant_ident = status_variant_ident(spec);
    match ty {
        Some(ty) => {
            let ty = boxed_ty_tokens(ty, names, true);
            quote! { #variant_ident(#ty), }
        }
        None => quote! { #variant_ident, },
    }
}

/// Emit an operation's typed error type: a payload-carrying enum for several documented error
/// entries of which at least one is bodied (several bodies, or one body beside a bodyless status),
/// a transparent newtype for a lone bodied entry, and the uninhabited alias for no error body.
pub(super) fn emit_error_enum(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let error_ident = &names.operation(&operation.id).error;
    let shape = operation.responses.error();
    let api_error_body = shape.api_error_body(&api.types);
    match shape {
        // Several documented error entries → a payload-carrying enum, one variant per status. The
        // variant is chosen by HTTP status at classification time, so it derives no whole-enum
        // `Deserialize` (and never `serde(untagged)`); each variant's body is decoded on its own.
        ErrorShape::Enum(entries) => {
            let variants = entries
                .iter()
                .map(|(spec, ty)| response_variant_def(*spec, *ty, names));
            // `Display` names the status the variant was dispatched on. The body is deliberately
            // not rendered: it is an arbitrary decoded model with no `Display` of its own, and it
            // is already reachable on the variant.
            let display_arms = entries.iter().map(|(spec, ty)| {
                let variant_ident = status_variant_ident(*spec);
                let label = format!("documented `{}` error response", spec.display_label());
                match ty {
                    Some(_) => quote! { #error_ident::#variant_ident(_) => #label, },
                    None => quote! { #error_ident::#variant_ident => #label, },
                }
            });
            // When every bodied variant carries one generated body type, the body is reachable
            // without matching the status: an inherent `body()` and `into_body()` (one arm per
            // variant, in the enum's own order, so the output is as deterministic as the enum) and
            // the runtime's `ApiErrorBody`, which is what `Error::api_body` and
            // `Error::into_api_body` need. `body` names the first bodied
            // status's type, which is the same Rust type as every other variant's. An enum whose
            // bodies are different types gets neither — there is no single body to hand back —
            // and is matched by variant.
            let shared_body = match api_error_body {
                Some(crate::ir::ApiErrorBodyImpl::Body(body)) => Some(body),
                // `Uninhabited` is the bodyless shape's answer, never an enum's.
                Some(crate::ir::ApiErrorBodyImpl::Uninhabited) | None => None,
            };
            // An error body is always optional: `ApiErrorBody` answers `Option` on every shape.
            let accessors = shared_body.map(|body| body_methods(&entries, body, false, names));
            let status = status_method(&entries);
            let inherent = (status.is_some() || accessors.is_some()).then(|| {
                quote! {
                    #[allow(dead_code, deprecated)]
                    impl #error_ident {
                        #status
                        #accessors
                    }
                }
            });
            let body_impl = shared_body.map(|body| {
                let body_ty = ty_tokens(body, names, true);
                quote! {
                    #[allow(deprecated)]
                    impl support::ApiErrorBody for #error_ident {
                        type Body = #body_ty;
                        fn body(&self) -> Option<&#body_ty> {
                            #error_ident::body(self)
                        }
                        fn into_body(self) -> Option<#body_ty> {
                            #error_ident::into_body(self)
                        }
                    }
                }
            });
            // Every enum reads its problem-details members, whichever body type each status
            // carries: `Error::problem` is the reader generic over operations, and the only one an
            // enum whose statuses carry different body types has.
            let problem_arms = entries.iter().map(|(spec, ty)| {
                let variant_ident = status_variant_ident(*spec);
                match ty {
                    // A raw-bytes body has no JSON members, and serializing `Bytes` would demand
                    // the `bytes/serde` feature a top-level bytes body is decoded without.
                    Some(ty) if is_bytes_ty(api, *ty) => {
                        quote! { #error_ident::#variant_ident(_) => None, }
                    }
                    Some(_) => quote! {
                        #error_ident::#variant_ident(body) => support::ProblemDetails::of(body),
                    },
                    None => quote! { #error_ident::#variant_ident => None, },
                }
            });
            quote! {
                #[allow(dead_code, deprecated)]
                #[derive(Debug, Clone)]
                pub enum #error_ident {
                    #(#variants)*
                }

                impl std::fmt::Display for #error_ident {
                    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(match self {
                            #(#display_arms)*
                        })
                    }
                }

                impl std::error::Error for #error_ident {}

                impl support::ApiErrorProblem for #error_ident {
                    fn problem(&self) -> Option<support::ProblemDetails> {
                        match self {
                            #(#problem_arms)*
                        }
                    }
                }

                #inherent
                #body_impl
            }
        }
        // A single documented error body: a transparent newtype over that type.
        //
        // A newtype rather than an alias, because `Error<E>` is only `Display`/`std::error::Error`
        // when `E` is, and an alias cannot carry those impls: the aliased type may be foreign
        // (`String`, `bytes::Bytes`), where the orphan rule forbids implementing them, and even a
        // local model type is shared with success bodies that are not errors. `serde(transparent)`
        // keeps the wire representation identical, and `Deref` plus `From` in both directions keep
        // the inner value one step away.
        ErrorShape::Single(body_ty) => {
            // A response body is never a cycle back-edge: every component is lowered before any
            // operation, and the in-progress set is cleared when a component finishes, so `boxed`
            // is always false here. Normalised once, so the payload type and the accessor agree,
            // and a top-level newtype field never needs a `Box` to be finite anyway.
            let body_ty = Ty {
                boxed: false,
                ..body_ty
            };
            let ty = ty_tokens(body_ty, names, true);
            let body = ty_tokens(
                Ty {
                    nullable: false,
                    ..body_ty
                },
                names,
                true,
            );
            let (body_expr, into_body_expr) = if body_ty.nullable {
                (quote! { self.0.as_ref() }, quote! { self.0 })
            } else {
                (quote! { Some(&self.0) }, quote! { Some(self.0) })
            };
            // The newtype's one entry is the operation's only error entry, read by the same filter
            // `Responses::error` builds the shape from. An exact status answers `status()`; a range
            // or `default` does not say which of its statuses was received.
            let mut error_statuses = operation
                .responses
                .entries()
                .map(|(spec, _)| spec)
                .filter(|spec| !spec.is_success());
            let status = match (error_statuses.next(), error_statuses.next()) {
                (Some(StatusSpec::Exact(code)), None) => {
                    let status = status_const(code);
                    Some(quote! {
                        /// The HTTP status this value was dispatched on: the operation's one
                        /// documented error status.
                        pub fn status(&self) -> reqwest::StatusCode {
                            #status
                        }
                    })
                }
                _ => None,
            };
            // The derive is emitted exactly when the decode path actually uses serde. A binary body
            // is classified by `classify_error_bytes`, which builds the newtype through
            // `From<Bytes>` — so deriving `Deserialize` there would demand `bytes/serde` of the
            // consumer for an impl nothing calls, and the runtime dependency contract deliberately
            // does not require that feature for a top-level bytes body. This predicate is the same
            // one `dispatch::error_branch_tokens` dispatches on, so the two cannot drift.
            let deserialize = (!is_bytes_ty(api, body_ty))
                .then(|| quote! { #[derive(serde::Deserialize)] #[serde(transparent)] });
            // As on the enum shape: a raw-bytes body has no JSON members to read.
            let problem_expr = if is_bytes_ty(api, body_ty) {
                quote! { None }
            } else {
                quote! { support::ProblemDetails::of(&self.0) }
            };
            let doc = format!(
                "The documented error body of this operation, wrapped so `Error<{error_ident}>` \
                 is a `std::error::Error`. Derefs and converts to the inner type."
            );
            quote! {
                #[doc = #doc]
                #[allow(dead_code, deprecated)]
                #[derive(Debug, Clone)]
                #deserialize
                pub struct #error_ident(pub #ty);

                #[allow(dead_code, deprecated)]
                impl #error_ident {
                    /// Unwrap to the documented error body.
                    pub fn into_inner(self) -> #ty {
                        self.0
                    }

                    #status
                }

                #[allow(deprecated)]
                impl std::ops::Deref for #error_ident {
                    type Target = #ty;
                    fn deref(&self) -> &Self::Target {
                        &self.0
                    }
                }

                impl std::ops::DerefMut for #error_ident {
                    fn deref_mut(&mut self) -> &mut Self::Target {
                        &mut self.0
                    }
                }

                #[allow(deprecated)]
                impl From<#ty> for #error_ident {
                    fn from(inner: #ty) -> Self {
                        Self(inner)
                    }
                }

                #[allow(deprecated)]
                impl From<#error_ident> for #ty {
                    fn from(outer: #error_ident) -> Self {
                        outer.0
                    }
                }

                impl std::fmt::Display for #error_ident {
                    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("documented error response")
                    }
                }

                impl std::error::Error for #error_ident {}

                // The one body is the inner value, so `Error::api_body` reaches it too. `Body`
                // is the bare definition — unboxed, non-nullable — exactly as on the enum shape,
                // so one bound covers both shapes of the same schema, and a `null` body answers
                // `None` here as a nullable enum variant does.
                #[allow(deprecated)]
                impl support::ApiErrorBody for #error_ident {
                    type Body = #body;
                    fn body(&self) -> Option<&#body> {
                        #body_expr
                    }
                    fn into_body(self) -> Option<#body> {
                        #into_body_expr
                    }
                }

                impl support::ApiErrorProblem for #error_ident {
                    fn problem(&self) -> Option<support::ProblemDetails> {
                        #problem_expr
                    }
                }
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

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::{emit_error_enum, emit_response_enum};
    use crate::diag::{JsonPointer, Provenance};
    use crate::ir::{
        Api, Docs, Info, MediaType, Method, Operation, OperationId, PathSegment, PathTemplate,
        Prim, Response, Responses, StatusSpec, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
    };

    /// The error type emitted for `get /message`, whose only documented error is a `400` carrying
    /// `body`, a reference to the one `Message` string definition.
    fn single_error_type(body: Ty) -> String {
        let api = message_api(vec![(StatusSpec::Exact(400), Some(body))]);
        let names = crate::name::allocate(&api);
        emit_error_enum(&api.operations[0], &api, &names).to_string()
    }

    /// The success enum and the error type emitted for `get /message` documenting `entries`.
    /// `TypeId(0)` is a `Message` string and `TypeId(1)` a distinct `Count` integer.
    fn emitted(entries: &[(StatusSpec, Option<Ty>)]) -> (String, String) {
        let api = message_api(entries.to_vec());
        let names = crate::name::allocate(&api);
        let operation = &api.operations[0];
        (
            emit_response_enum(operation, &api, &names).to_string(),
            emit_error_enum(operation, &api, &names).to_string(),
        )
    }

    fn body(id: u32, nullable: bool) -> Option<Ty> {
        Some(Ty {
            id: TypeId(id),
            nullable,
            boxed: false,
        })
    }

    /// An API whose one operation, `get /message`, documents `entries`, each bodied entry as JSON.
    fn message_api(entries: Vec<(StatusSpec, Option<Ty>)>) -> Api {
        let mut types = TypeGraph::default();
        types.insert(TypeDef {
            name_hint: "Message".to_owned(),
            kind: TypeKind::Primitive(Prim::String),
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        types.insert(TypeDef {
            name_hint: "Count".to_owned(),
            kind: TypeKind::Primitive(Prim::I64),
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        let response = |body: Option<Ty>| Response {
            body,
            media: body.map(|_| MediaType::Json),
            stream: None,
            accept: None,
            headers: Vec::new(),
        };
        let (default, by_status): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|(spec, _)| *spec == StatusSpec::Default);
        let operation = Operation {
            id: OperationId("getMessage".to_owned()),
            method: Method::Get,
            path: PathTemplate {
                raw: "/message".to_owned(),
                segments: vec![PathSegment::Literal("/message".to_owned())],
            },
            params: Vec::new(),
            request_body: None,
            responses: Responses {
                by_status: by_status
                    .into_iter()
                    .map(|(spec, body)| (spec, response(body)))
                    .collect(),
                default: default.into_iter().next().map(|(_, body)| response(body)),
            },
            security: Vec::new(),
            deprecated: false,
            docs: Docs::default(),
            server: None,
            provenance: Provenance::new(JsonPointer::root(), None),
        };
        Api {
            info: Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: vec![operation],
            types,
            security_schemes: IndexMap::new(),
        }
    }

    /// `status()` exists exactly when every variant names one exact status: a range or `default`
    /// variant covers several, so its value alone cannot answer it (#1257). The single-body
    /// newtype follows its one entry's selector the same way.
    #[test]
    fn status_is_emitted_exactly_when_every_variant_is_an_exact_status() {
        let status = "pub fn status (& self) -> reqwest :: StatusCode";
        for (second, expected) in [
            (StatusSpec::Exact(206), true),
            (StatusSpec::Range(2), false),
        ] {
            let (success, _) = emitted(&[(StatusSpec::Exact(200), body(0, false)), (second, None)]);
            assert_eq!(success.contains(status), expected, "{second:?}: {success}");
        }
        for (second, expected) in [
            (StatusSpec::Exact(409), true),
            (StatusSpec::Range(4), false),
            (StatusSpec::Default, false),
        ] {
            let (_, error) = emitted(&[(StatusSpec::Exact(404), body(0, false)), (second, None)]);
            assert_eq!(error.contains(status), expected, "{second:?}: {error}");
        }
        for (only, expected) in [
            (StatusSpec::Exact(400), true),
            (StatusSpec::Range(4), false),
            (StatusSpec::Default, false),
        ] {
            let (_, newtype) = emitted(&[(StatusSpec::Exact(200), None), (only, body(0, false))]);
            assert!(newtype.contains("pub struct"), "{only:?}: {newtype}");
            assert_eq!(newtype.contains(status), expected, "{only:?}: {newtype}");
        }
    }

    /// A success enum's `body()`/`into_body()`: bare when every variant carries one non-null type,
    /// `Option` beside a bodyless or nullable variant, and absent when two variants carry different
    /// types (#1257).
    #[test]
    fn success_body_accessors_follow_the_shared_body_type() {
        let ok = StatusSpec::Exact(200);
        let partial = StatusSpec::Exact(206);
        let bare = "pub fn into_body (self) -> types :: Message";
        let optional = "pub fn into_body (self) -> Option < types :: Message >";
        // The `206`'s body beside a non-null `Message` on the `200`, and the accessor it yields.
        let cases = [
            (body(0, false), Some(bare)),
            (None, Some(optional)),
            (body(0, true), Some(optional)),
            (body(1, false), None),
        ];
        for (second, expected) in cases {
            let entries = [(ok, body(0, false)), (partial, second)];
            let (success, _) = emitted(&entries);
            assert!(success.contains("pub enum"), "{entries:?}: {success}");
            match expected {
                Some(signature) => assert!(success.contains(signature), "{entries:?}: {success}"),
                None => assert!(!success.contains("fn into_body"), "{entries:?}: {success}"),
            }
        }
    }

    /// An error enum's `into_body()` sits beside its `body()`, inherent and on `ApiErrorBody`, and
    /// neither exists when its statuses carry different body types (#1257).
    #[test]
    fn error_into_body_accompanies_body_on_the_enum_and_the_trait() {
        let shared = [
            (StatusSpec::Exact(404), body(0, false)),
            (StatusSpec::Exact(409), body(0, true)),
        ];
        let (_, error) = emitted(&shared);
        assert!(
            error.contains("pub fn into_body (self) -> Option < types :: Message >"),
            "{error}"
        );
        assert!(
            error.contains("fn into_body (self) -> Option < types :: Message > { GetMessageError :: into_body (self) }"),
            "{error}"
        );
        let distinct = [
            (StatusSpec::Exact(404), body(0, false)),
            (StatusSpec::Exact(409), body(1, false)),
        ];
        let (_, error) = emitted(&distinct);
        assert!(!error.contains("into_body"), "{error}");
        assert!(!error.contains("ApiErrorBody"), "{error}");
    }

    /// The single-body arm's `boxed: false` normalisation (see its comment on why lowering hands it
    /// an unboxed body) is pinned here directly, so a lowering change that does box a response body
    /// still emits the finite, compiling newtype and its accessor.
    #[test]
    fn a_boxed_single_error_body_emits_the_unboxed_newtype() {
        for nullable in [false, true] {
            let unboxed = single_error_type(Ty {
                id: TypeId(0),
                nullable,
                boxed: false,
            });
            let boxed = single_error_type(Ty {
                id: TypeId(0),
                nullable,
                boxed: true,
            });
            assert!(unboxed.contains("ApiErrorBody"), "{unboxed}");
            assert_eq!(boxed, unboxed, "boxing changed the emitted error type");
            assert!(!boxed.contains("Box"), "{boxed}");
        }
    }
}
