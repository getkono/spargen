//! The types an operation's responses are read into: the multi-status success enum, the typed
//! error, and the documented response-header structs.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, ErrorShape, Operation, SuccessShape, Ty};
use crate::name::Names;

use super::dispatch::{is_bytes_ty, status_variant_ident, success_enum_ident};
use super::docs::doc_tokens;
use super::runtime::error_type_ident;
use super::ty::ty_tokens;

/// Emit one typed header struct per documented status that declares response headers.
///
/// Reading headers is an explicitly-called second step rather than part of the return type: the
/// body has already been decoded and handed back before a caller opts in, so a malformed or absent
/// header can never turn a successful call into a failed one. Every header also stays reachable
/// raw through `ResponseValue::headers`.
pub(super) fn emit_response_headers(operation: &Operation, names: &Names) -> TokenStream {
    let responses = operation
        .responses
        .by_status
        .iter()
        .map(|(spec, response)| (crate::name::status_label(*spec), response))
        .chain(operation.responses.default.as_ref().map(|response| {
            (
                crate::name::status_label(crate::ir::StatusSpec::Default),
                response,
            )
        }));
    let structs = responses.filter_map(|(label, response)| {
        if response.headers.is_empty() {
            return None;
        }
        let ident = names
            .response_header_structs
            .get(&(operation.id.clone(), label.clone()))?;
        let fields = response.headers.iter().map(|header| {
            let field = names
                .response_header_fields
                .get(&(operation.id.clone(), label.clone(), header.name.clone()))
                .expect("response header field allocated");
            // Header structs live beside `Client`, not inside `types`, so the type path must be
            // qualified — a header whose schema lowered to a named type would not resolve here.
            let ty = ty_tokens(header.ty, names, true);
            // An optional header is absent-able; a required one is documented as always present.
            let ty = if header.required {
                quote! { #ty }
            } else {
                quote! { Option<#ty> }
            };
            let docs = doc_tokens(&header.docs);
            let deprecated = header.deprecated.then(|| quote! { #[deprecated] });
            quote! { #docs #deprecated pub #field: #ty }
        });
        let reads = response.headers.iter().map(|header| {
            let field = names
                .response_header_fields
                .get(&(operation.id.clone(), label.clone(), header.name.clone()))
                .expect("response header field allocated");
            let name = header.name.clone();
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
            #[allow(dead_code)]
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
pub(super) fn emit_response_enum(operation: &Operation, names: &Names) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Enum(entries) => {
            let method_ident = names
                .operations
                .get(&operation.id)
                .expect("operation name allocated");
            let ident = success_enum_ident(method_ident);
            let variants = entries
                .iter()
                .map(|(spec, ty)| response_variant_def(*spec, *ty, names));
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
fn response_variant_def(spec: crate::ir::StatusSpec, ty: Option<Ty>, names: &Names) -> TokenStream {
    let variant_ident = status_variant_ident(spec);
    match ty {
        Some(ty) => {
            let ty = response_payload_ty_tokens(ty, names, true);
            quote! { #variant_ident(#ty), }
        }
        None => quote! { #variant_ident, },
    }
}

/// Emit an operation's typed error type: a payload-carrying enum for several documented error
/// entries of which at least one is bodied (several bodies, or one body beside a bodyless status),
/// a transparent newtype for a lone bodied entry, and the uninhabited alias for no error body.
pub(super) fn emit_error_enum(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let error_ident = error_type_ident(method_ident.as_str());
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
            // without matching the status: an inherent `body()` (one arm per variant, in the
            // enum's own order, so the output is as deterministic as the enum) and the runtime's
            // `ApiErrorBody`, which is what `Error::api_body` needs. `body` names the first bodied
            // status's type, which is the same Rust type as every other variant's. An enum whose
            // bodies are different types gets neither — there is no single body to hand back —
            // and is matched by variant.
            let shared_body = match api_error_body {
                Some(crate::ir::ApiErrorBodyImpl::Body(body)) => Some(body),
                // `Uninhabited` is the bodyless shape's answer, never an enum's.
                Some(crate::ir::ApiErrorBodyImpl::Uninhabited) | None => None,
            };
            let accessor = shared_body.map(|body| {
                let body_ty = ty_tokens(body, names, true);
                let body_arms = entries.iter().map(|(spec, ty)| {
                    let variant_ident = status_variant_ident(*spec);
                    match ty {
                        // A nullable payload is `Option<Box<T>>`: `as_deref` reaches the `T`.
                        Some(ty) if ty.nullable => {
                            quote! { #error_ident::#variant_ident(body) => body.as_deref(), }
                        }
                        // A boxed payload: `as_ref` reaches the `T` inside the `Box`.
                        Some(_) => {
                            quote! { #error_ident::#variant_ident(body) => Some(body.as_ref()), }
                        }
                        None => quote! { #error_ident::#variant_ident => None, },
                    }
                });
                quote! {
                    #[allow(dead_code)]
                    impl #error_ident {
                        /// The documented error body, whichever status carried it; `None` for a
                        /// documented bodyless status. The status is on the `ResponseValue` that
                        /// wraps this value.
                        pub fn body(&self) -> Option<&#body_ty> {
                            match self {
                                #(#body_arms)*
                            }
                        }
                    }

                    impl support::ApiErrorBody for #error_ident {
                        type Body = #body_ty;
                        fn body(&self) -> Option<&#body_ty> {
                            #error_ident::body(self)
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
                #[allow(dead_code)]
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

                #accessor
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
            let body_expr = if body_ty.nullable {
                quote! { self.0.as_ref() }
            } else {
                quote! { Some(&self.0) }
            };
            // The derive is emitted exactly when the decode path actually uses serde. A binary body
            // is classified by `classify_error_bytes`, which builds the newtype through
            // `From<Bytes>` — so deriving `Deserialize` there would demand `bytes/serde` of the
            // consumer for an impl nothing calls, and the runtime dependency contract deliberately
            // does not require that feature for a top-level bytes body. This predicate is the same
            // one the error branch in `emit_operation` dispatches on, so the two cannot drift.
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
                #[allow(dead_code)]
                #[derive(Debug, Clone)]
                #deserialize
                pub struct #error_ident(pub #ty);

                #[allow(dead_code)]
                impl #error_ident {
                    /// Unwrap to the documented error body.
                    pub fn into_inner(self) -> #ty {
                        self.0
                    }
                }

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

                impl From<#ty> for #error_ident {
                    fn from(inner: #ty) -> Self {
                        Self(inner)
                    }
                }

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
                impl support::ApiErrorBody for #error_ident {
                    type Body = #body;
                    fn body(&self) -> Option<&#body> {
                        #body_expr
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

/// Multi-status response payloads are uniformly indirect for the same bounded-enum-size reason as
/// schema unions. Single-body response aliases remain allocation-free.
pub(super) fn response_payload_ty_tokens(ty: Ty, names: &Names, qualified: bool) -> TokenStream {
    ty_tokens(Ty { boxed: true, ..ty }, names, qualified)
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::emit_error_enum;
    use crate::diag::{Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        Api, Docs, Info, MediaType, Method, Operation, OperationId, PathSegment, PathTemplate,
        Prim, Response, Responses, StatusSpec, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
    };

    /// The error type emitted for `get /message`, whose only documented error is a `400` carrying
    /// `body`, a reference to the one `Message` string definition.
    fn single_error_type(body: Ty) -> String {
        let mut types = TypeGraph::default();
        types.insert(TypeDef {
            name_hint: "Message".to_owned(),
            kind: TypeKind::Primitive(Prim::String),
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
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
                by_status: vec![(
                    StatusSpec::Exact(400),
                    Response {
                        body: Some(body),
                        media: Some(MediaType::Json),
                        stream: None,
                        headers: Vec::new(),
                    },
                )],
                default: None,
            },
            security: Vec::new(),
            deprecated: false,
            docs: Docs::default(),
            server: None,
            provenance: Provenance::new(JsonPointer::root(), None),
        };
        let api = Api {
            info: Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: vec![operation],
            types,
            security_schemes: IndexMap::new(),
        };
        let names = crate::name::allocate(&api, &mut Diagnostics::default());
        emit_error_enum(&api.operations[0], &api, &names).to_string()
    }

    /// Lowering never hands `ErrorShape::Single` a boxed body today (components are lowered before
    /// operations, so a response body is never a cycle back-edge), and the single-body arm relies
    /// on that only through its `boxed: false` normalisation. Pin the normalisation itself, so a
    /// lowering change that does box a response body still emits the finite, compiling newtype and
    /// its accessor rather than an arm nobody has compiled.
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
