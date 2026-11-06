//! What an operation method does around the wire: attach the credentials its `security` selects,
//! and dispatch the response by status into the typed success value or the typed error.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{
    Api, ApiKeyLoc, ErrorShape, HttpScheme, MediaType, Operation, SecurityScheme, SuccessShape, Ty,
};
use crate::name::{Names, OperationBindings};

use super::idents::status_variant_ident;
use super::ty::{boxed_ty_tokens, is_bytes_ty, ty_tokens};

/// The statement that attaches the operation's credentials to `request_binding`: the first
/// `security` alternative whose schemes are all registered, through `support::attach_auth`. Empty
/// for an operation with no `security` requirement.
pub(super) fn attach_auth_tokens(
    operation: &Operation,
    api: &Api,
    request_binding: &crate::name::Ident,
) -> TokenStream {
    if operation.security.is_empty() {
        quote! {}
    } else {
        let alternatives = operation.security.iter().map(|requirement| {
            let schemes = requirement.0.iter().map(|id| {
                let scheme = api
                    .security_schemes
                    .get(id)
                    .expect("security scheme validated during lowering");
                let name = &id.0;
                let kind = match &scheme.kind {
                    // Caller-supplied oauth2/oidc tokens attach as bearer credentials.
                    SecurityScheme::Http(HttpScheme::Bearer)
                    | SecurityScheme::OAuth2
                    | SecurityScheme::OpenIdConnect => quote! { support::AuthKind::Bearer },
                    SecurityScheme::Http(HttpScheme::Basic) => quote! { support::AuthKind::Basic },
                    // Satisfied by the transport's client certificate; nothing to attach.
                    SecurityScheme::MutualTls => quote! { support::AuthKind::MutualTls },
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
            #request_binding = support::attach_auth(
                &self.core,
                #request_binding,
                &[#(#alternatives),*],
            )
                .await
                .map_err(support::Error::widen)?;
        }
    }
}

/// The method's non-success branch, by the operation's [`ErrorShape`]: an unexpected status when
/// no error body is documented, the one-entry classifier for a single body, and a by-status
/// dispatch into `error_ident`'s variants for an enum.
pub(super) fn error_branch_tokens(
    operation: &Operation,
    api: &Api,
    names: &Names,
    error_ident: &crate::name::Ident,
) -> TokenStream {
    let error_shape = operation.responses.error();
    match &error_shape {
        ErrorShape::None => quote! {
            Err(support::unexpected_status::<#error_ident>(&self.core, response).await)
        },
        // A single documented error body, the operation's only error entry: classify against its
        // one-entry status table into the aliased `E` (or `Error::UnexpectedStatus` for any other
        // status). A bodyless entry beside it makes the shape an enum, so the filters below keep
        // exactly that entry; a `default` one maps to `Any`.
        ErrorShape::Single(body_ty) => {
            let documented = operation
                .responses
                .entries()
                .filter(|(status, response)| !status.is_success() && response.body.is_some())
                .map(|(status, _)| runtime_status_spec(status))
                .collect::<Vec<_>>();
            // The raw-bytes classifier builds the newtype through `From<Bytes>` and takes no type
            // argument; every other codec's classifier decodes into the aliased `E`. A streaming
            // error media never reaches here: lowering rejects a bodied stream on the error side
            // (`Responses::stream_outside_single_success`), since a stream cannot be classified as
            // one whole body.
            let classifier =
                match BodyCodec::of(api, *body_ty, operation.responses.single_error_media()) {
                    BodyCodec::Bytes => quote! { classify_error_bytes },
                    BodyCodec::Xml => quote! { classify_error_xml::<#error_ident> },
                    BodyCodec::Text => quote! { classify_error_text::<#error_ident> },
                    BodyCodec::Json => quote! { classify_error::<#error_ident> },
                };
            let classify = quote! {
                support::#classifier(
                    &self.core,
                    response,
                    &[#(#documented),*],
                )
                .await
            };
            quote! {
                Err(#classify)
            }
        }
        // Several documented error entries, at least one bodied (several bodies, or one beside a
        // bodyless entry): dispatch by status in precedence order (exact before range before
        // default), and classify through `support::classify_with` against the matched selector
        // into that enum variant → `Error::Api`; a parse failure → `Error::Decode`; an
        // undocumented status → `Error::UnexpectedStatus` (capped body preserved either way).
        ErrorShape::Enum(entries) => {
            let arms = entries.iter().map(|(spec, ty)| {
                let spec_tokens = runtime_status_spec(*spec);
                let variant = {
                    let variant_ident = status_variant_ident(*spec);
                    quote! { #error_ident::#variant_ident }
                };
                let decode = match ty {
                    // Bodied status: decode into the variant's type.
                    Some(ty) => {
                        let decode =
                            variant_body_decode(operation, api, names, *spec, *ty, variant);
                        quote! { |body| #decode }
                    }
                    // Documented bodyless error status: the unit variant, no body parse.
                    None => quote! { |_| Ok::<_, String>(#variant) },
                };
                quote! {
                    if #spec_tokens.matches(status) {
                        return Err(support::classify_with(
                            &self.core,
                            response,
                            &[#spec_tokens],
                            #decode,
                        )
                        .await);
                    }
                }
            });
            quote! {
                let status = response.status();
                #(#arms)*
                Err(support::unexpected_status::<#error_ident>(&self.core, response).await)
            }
        }
    }
}

/// The method's success branch and, for a streaming success, the statement that keeps a clone of
/// the request for reconnection (`None` otherwise). `success_ty` is [`success_type`]'s rendering
/// and `error_ident` the operation's error type.
pub(super) fn success_decode_tokens(
    operation: &Operation,
    api: &Api,
    names: &Names,
    bindings: &OperationBindings,
    success_ty: &TokenStream,
    error_ident: &crate::name::Ident,
) -> (TokenStream, Option<TokenStream>) {
    let request_binding = &bindings.request;
    let reconnect_request_binding = &bindings.reconnect_request;
    let success_decode = match operation.responses.success() {
        SuccessShape::Unit => quote! {
            let status = response.status();
            let headers = response.headers().clone();
            Ok(support::ResponseValue::new(status, headers, ()))
        },
        // A single success body: decode into the aliased `T` through its selected wire codec.
        SuccessShape::Plain(body_ty) => {
            let decode =
                match BodyCodec::of(api, body_ty, operation.responses.single_success_media()) {
                    BodyCodec::Bytes => {
                        quote! { support::decode_success_bytes(&self.core, response) }
                    }
                    BodyCodec::Xml => {
                        quote! { support::decode_success_xml::<#success_ty>(&self.core, response) }
                    }
                    BodyCodec::Text => {
                        quote! { support::decode_success_text::<#success_ty>(&self.core, response) }
                    }
                    BodyCodec::Json => {
                        quote! { support::decode_success::<#success_ty>(&self.core, response) }
                    }
                };
            quote! {
                #decode.await.map_err(support::Error::widen)
            }
        }
        // Multi-status success: dispatch by status in precedence order (exact code ascending, then
        // range ascending) into the matching variant, and only then read the body. `default` never
        // enters this shape, so there is no `Default` arm. A success status matching no documented
        // variant is an unexpected-status error — there is no untyped fallback. Every body an
        // error keeps goes through a runtime helper that caps it at `max_error_body`.
        SuccessShape::Enum(entries) => {
            let enum_ident = names.operation(&operation.id).success_enum();
            let arms = entries.iter().map(|(spec, ty)| {
                let spec_tokens = runtime_status_spec(*spec);
                let variant_ident = status_variant_ident(*spec);
                match ty {
                    // Bodied status: read the whole body and parse it into the variant's type; a
                    // parse failure is `Decode` with the body capped.
                    Some(ty) => {
                        let decode = variant_body_decode(
                            operation,
                            api,
                            names,
                            *spec,
                            *ty,
                            quote! { #enum_ident::#variant_ident },
                        );
                        quote! {
                            if #spec_tokens.matches(status) {
                                return support::decode_success_with(&self.core, response, |body| {
                                    #decode
                                })
                                .await
                                .map_err(support::Error::widen);
                            }
                        }
                    }
                    // Documented bodyless status (e.g. `204`): the unit variant, body unread.
                    None => quote! {
                        if #spec_tokens.matches(status) {
                            return Ok(support::ResponseValue::new(
                                status,
                                response.headers().clone(),
                                #enum_ident::#variant_ident,
                            ));
                        }
                    },
                }
            });
            quote! {
                let status = response.status();
                #(#arms)*
                Err(support::unexpected_status::<#error_ident>(&self.core, response).await)
            }
        }
    };

    // A streaming success response (`text/event-stream` / `application/x-ndjson`) returns an
    // `EventStream<T>` instead of a `ResponseValue<T>`: on success the whole `response` is handed
    // to the stream with its framing mode, and items are decoded lazily as the caller pulls them.
    // The error path is a whole-body response's (lowering rejects a streaming error body).
    // `success_ty` is the streamed item type `T` — `stream_success` fires only in the
    // single-success-body case, where `success()` is `Plain(T)` and `success_type` renders `T`.
    let stream_framing = operation
        .responses
        .stream_success()
        .map(|(framing, _)| framing);
    let success_decode = match stream_framing {
        Some(framing) => {
            let framing_tokens = match framing {
                crate::ir::Framing::Sse => quote! { support::Framing::Sse },
                crate::ir::Framing::SseEvent => quote! { support::Framing::SseEvent },
                crate::ir::Framing::SseJsonData => quote! { support::Framing::SseJsonData },
                crate::ir::Framing::Ndjson => quote! { support::Framing::Ndjson },
                crate::ir::Framing::JsonSequence => quote! { support::Framing::JsonSequence },
            };
            quote! {
                Ok(support::EventStream::new_reconnectable(
                    response,
                    #framing_tokens,
                    &self.core,
                    #reconnect_request_binding,
                ))
            }
        }
        None => success_decode,
    };
    let reconnect_request_init = stream_framing.map(|_| {
        quote! {
            let #reconnect_request_binding = #request_binding.try_clone();
        }
    });
    (success_decode, reconnect_request_init)
}

/// The expression decoding a by-status variant's already-read `body` into that variant, as a
/// `Result<_, String>` whose error is the decode failure's path: the bytes themselves for a
/// raw-bytes body, otherwise the codec of the media the status's response chose. The success and
/// error dispatch both build their variants' decodes here, so the two sides cannot pick a codec
/// differently.
///
/// Each side evaluates it inside the closure it hands `support::decode_success_with` or
/// `support::classify_with`, where `body` is a `&Bytes`; `variant` is the variant constructor the
/// expression applies to the payload.
fn variant_body_decode(
    operation: &Operation,
    api: &Api,
    names: &Names,
    spec: crate::ir::StatusSpec,
    body_ty: Ty,
    variant: TokenStream,
) -> TokenStream {
    let ty = boxed_ty_tokens(body_ty, names, true);
    let media = response_media_for_spec(&operation.responses, spec);
    let decode = match BodyCodec::of(api, body_ty, media) {
        BodyCodec::Bytes => return quote! { Ok::<_, String>(#variant(Box::new(body.clone()))) },
        BodyCodec::Text => quote! { support::decode_text_body::<#ty>(body) },
        // Only as the lone body beside bodyless entries on its side: a second bodied response
        // beside an XML one is rejected (`xml_in_multi_status`).
        BodyCodec::Xml => quote! { support::decode_xml_body::<#ty>(body) },
        BodyCodec::Json => quote! {
            serde_json::from_slice::<#ty>(body)
                .map_err(|error| error.to_string())
        },
    };
    quote! { #decode.map(#variant) }
}

/// The codec a response body is decoded with, chosen once for the single-body success and error
/// paths and the by-status variants alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyCodec {
    /// A `bytes`-typed body, read raw whatever its media.
    Bytes,
    /// An XML media.
    Xml,
    /// A `text/plain` media.
    Text,
    /// Every other media: JSON.
    Json,
}

impl BodyCodec {
    /// The codec of a body of type `ty` under the response's chosen `media`.
    fn of(api: &Api, ty: Ty, media: Option<MediaType>) -> Self {
        if is_bytes_ty(api, ty) {
            return Self::Bytes;
        }
        match media {
            Some(MediaType::Xml) => Self::Xml,
            Some(MediaType::Text) => Self::Text,
            _ => Self::Json,
        }
    }
}

/// The `T` an operation's success yields: `()` for no body, the body's type for one, and the
/// operation's response enum for several (or one beside a documented bodyless status).
pub(super) fn success_type(operation: &Operation, names: &Names) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Unit => quote! { () },
        SuccessShape::Plain(ty) => ty_tokens(ty, names, true),
        SuccessShape::Enum(_) => {
            let ident = names.operation(&operation.id).success_enum();
            quote! { #ident }
        }
    }
}

/// The runtime `support::StatusSpec` tokens for a documented status selector. `default` maps to
/// `Any`: it is classified last, so it matches exactly the statuses no other entry claimed.
fn runtime_status_spec(spec: crate::ir::StatusSpec) -> TokenStream {
    match spec {
        crate::ir::StatusSpec::Exact(code) => quote! { support::StatusSpec::Exact(#code) },
        crate::ir::StatusSpec::Range(prefix) => quote! { support::StatusSpec::Range(#prefix) },
        crate::ir::StatusSpec::Default => quote! { support::StatusSpec::Any },
    }
}

/// The chosen media type of the response a shape entry was built from (see
/// [`Responses::get`](crate::ir::Responses::get)).
fn response_media_for_spec(
    responses: &crate::ir::Responses,
    spec: crate::ir::StatusSpec,
) -> Option<MediaType> {
    responses.get(spec).and_then(|response| response.media)
}

#[cfg(test)]
mod tests {
    use crate::ir::{MediaType, Response, Responses, StatusSpec, Ty, TypeId};

    /// Each IR status selector maps to the runtime selector that classifies the same statuses,
    /// spelled with the literal's own type, and to the response variant named for it.
    #[test]
    fn each_status_selector_maps_to_its_runtime_spec_and_variant() {
        for (spec, runtime, variant) in [
            (
                StatusSpec::Exact(200),
                quote::quote! { support::StatusSpec::Exact(200u16) },
                "Status200",
            ),
            (
                StatusSpec::Exact(404),
                quote::quote! { support::StatusSpec::Exact(404u16) },
                "Status404",
            ),
            (
                StatusSpec::Range(2),
                quote::quote! { support::StatusSpec::Range(2u8) },
                "Status2xx",
            ),
            (
                StatusSpec::Range(5),
                quote::quote! { support::StatusSpec::Range(5u8) },
                "Status5xx",
            ),
            (
                StatusSpec::Default,
                quote::quote! { support::StatusSpec::Any },
                "Default",
            ),
        ] {
            assert_eq!(
                super::runtime_status_spec(spec).to_string(),
                runtime.to_string(),
                "{spec:?}"
            );
            assert_eq!(
                super::status_variant_ident(spec).to_string(),
                variant,
                "{spec:?}"
            );
        }
    }

    /// Issue #233: each status selector reads only its own response, so a `0XX` entry decodes
    /// under its own codec, not the `default` response's (or the fallback, with no `default`), and
    /// its variant and runtime selector stay distinct from `default`'s.
    #[test]
    fn each_status_selector_reads_its_own_response_media_and_label() {
        let response = |media| Response {
            body: Some(Ty {
                id: TypeId(0),
                nullable: false,
                boxed: false,
            }),
            media: Some(media),
            stream: None,
            accept: None,
            headers: Vec::new(),
        };
        let with_default = Responses {
            by_status: vec![
                (StatusSpec::Range(0), response(MediaType::Xml)),
                (StatusSpec::Exact(404), response(MediaType::Json)),
            ],
            default: Some(response(MediaType::Text)),
        };
        let media = |responses: &Responses, spec| super::response_media_for_spec(responses, spec);
        assert_eq!(
            media(&with_default, StatusSpec::Default),
            Some(MediaType::Text)
        );
        assert_eq!(
            media(&with_default, StatusSpec::Range(0)),
            Some(MediaType::Xml)
        );
        assert_eq!(
            media(&with_default, StatusSpec::Exact(404)),
            Some(MediaType::Json)
        );
        let without_default = Responses {
            default: None,
            ..with_default
        };
        assert_eq!(media(&without_default, StatusSpec::Default), None);
        assert_eq!(
            media(&without_default, StatusSpec::Range(0)),
            Some(MediaType::Xml)
        );

        assert_eq!(
            super::status_variant_ident(StatusSpec::Default).to_string(),
            "Default"
        );
        assert_eq!(
            super::status_variant_ident(StatusSpec::Range(0)).to_string(),
            "Status0xx"
        );
        assert_eq!(
            super::runtime_status_spec(StatusSpec::Default).to_string(),
            quote::quote! { support::StatusSpec::Any }.to_string()
        );
        assert_ne!(
            super::runtime_status_spec(StatusSpec::Range(0)).to_string(),
            super::runtime_status_spec(StatusSpec::Default).to_string()
        );
    }
}
