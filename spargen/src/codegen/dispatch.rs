//! What an operation method does around the wire: attach the credentials its `security` selects,
//! and dispatch the response by status into the typed success value or the typed error.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{
    Api, ApiKeyLoc, ErrorShape, HttpScheme, MediaType, Operation, SecurityScheme, SuccessShape, Ty,
};
use crate::name::{Names, OperationBindings};

use super::idents::{status_variant_ident, success_enum_ident};
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
            let schemes = requirement.0.iter().map(|(id, _scopes)| {
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
    let error_ty = quote! { #error_ident };
    let error_shape = operation.responses.error();
    match &error_shape {
        ErrorShape::None => quote! {
            Err(support::unexpected_status::<#error_ty>(&self.core, response).await)
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
            // argument; every other codec's classifier decodes into the aliased `E`.
            let classifier = if is_bytes_ty(api, *body_ty) {
                quote! { classify_error_bytes }
            } else {
                match operation.responses.single_error_media() {
                    Some(MediaType::Xml) => quote! { classify_error_xml::<#error_ty> },
                    Some(MediaType::Text) => quote! { classify_error_text::<#error_ty> },
                    // JSON. A streaming error media never reaches this arm: lowering rejects a
                    // bodied stream on the error side (`Responses::stream_outside_single_success`),
                    // since a stream cannot be classified as one whole body.
                    _ => quote! { classify_error::<#error_ty> },
                }
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
        // bodyless entry): read the capped body once, then dispatch by status in
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
                        let decode = variant_body_decode(operation, api, names, *spec, *ty, None);
                        quote! {
                            if #spec_tokens.matches(status) {
                                return Err(match #decode {
                                    Ok(value) => support::Error::Api(support::ResponseValue::new(
                                        status,
                                        headers,
                                        #error_ident::#variant_ident(value),
                                    )),
                                    Err(path) => support::Error::Decode {
                                        status,
                                        headers,
                                        path,
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
    let error_ty = quote! { #error_ident };
    let success_decode = match operation.responses.success() {
        SuccessShape::Unit => quote! {
            let status = response.status();
            let headers = response.headers().clone();
            Ok(support::ResponseValue::new(status, headers, ()))
        },
        // A single success body: decode into the aliased `T` through its selected wire codec.
        SuccessShape::Plain(body_ty) => {
            let decode = if is_bytes_ty(api, body_ty) {
                quote! { support::decode_success_bytes(&self.core, response) }
            } else {
                match operation.responses.single_success_media() {
                    Some(MediaType::Xml) => {
                        quote! { support::decode_success_xml::<#success_ty>(&self.core, response) }
                    }
                    Some(MediaType::Text) => {
                        quote! { support::decode_success_text::<#success_ty>(&self.core, response) }
                    }
                    _ => quote! { support::decode_success::<#success_ty>(&self.core, response) },
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
            let enum_ident = success_enum_ident(operation, names);
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
                            Some(quote! { #enum_ident::#variant_ident }),
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
                Err(support::unexpected_status::<#error_ty>(&self.core, response).await)
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
                crate::ir::Framing::SseEvent => quote! { support::Framing::SseEvent },
                crate::ir::Framing::SseJsonData => quote! { support::Framing::SseJsonData },
                crate::ir::Framing::Ndjson => quote! { support::Framing::Ndjson },
                crate::ir::Framing::JsonSequence => quote! { support::Framing::JsonSequence },
            };
            quote! {
                Ok(support::EventStream::new_reconnectable(
                    response,
                    #framing_tokens,
                    self.core.clone(),
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

/// The expression decoding a by-status variant's already-read `body` into its payload type, as a
/// `Result<_, String>` whose error is the decode failure's path: the bytes themselves for a
/// raw-bytes body, otherwise the codec of the media the status's response chose. The success and
/// error dispatch both build their variants' decodes here, so the two sides cannot pick a codec
/// differently.
///
/// The error side binds `body` to the read `Bytes` and wraps the payload itself, and passes `None`.
/// The success side decodes inside the closure it hands `support::decode_success_with`, where
/// `body` is a `&Bytes`, and passes the variant constructor as `success_variant`, which the
/// expression applies to the payload.
fn variant_body_decode(
    operation: &Operation,
    api: &Api,
    names: &Names,
    spec: crate::ir::StatusSpec,
    body_ty: Ty,
    success_variant: Option<TokenStream>,
) -> TokenStream {
    let ty = boxed_ty_tokens(body_ty, names, true);
    let media = response_media_for_spec(&operation.responses, spec);
    let bytes = if success_variant.is_some() {
        quote! { body }
    } else {
        quote! { &body }
    };
    if is_bytes_ty(api, body_ty) {
        let payload = quote! { Box::new(body.clone()) };
        return match success_variant {
            Some(variant) => quote! { Ok::<_, String>(#variant(#payload)) },
            None => quote! { Ok::<#ty, String>(#payload) },
        };
    }
    let decode = if media == Some(MediaType::Text) {
        quote! { support::decode_text_body::<#ty>(#bytes) }
    } else if media == Some(MediaType::Xml) {
        // Only as the lone body beside bodyless entries on its side: a second bodied response
        // beside an XML one is rejected (`xml_in_multi_status`).
        quote! { support::decode_xml_body::<#ty>(#bytes) }
    } else {
        quote! {
            serde_json::from_slice::<#ty>(#bytes)
                .map_err(|error| error.to_string())
        }
    };
    match success_variant {
        Some(variant) => quote! { #decode.map(#variant) },
        None => decode,
    }
}

/// The `T` an operation's success yields: `()` for no body, the body's type for one, and the
/// operation's response enum for several (or one beside a documented bodyless status).
pub(super) fn success_type(operation: &Operation, names: &Names) -> TokenStream {
    match operation.responses.success() {
        SuccessShape::Unit => quote! { () },
        SuccessShape::Plain(ty) => ty_tokens(ty, names, true),
        SuccessShape::Enum(_) => {
            let ident = success_enum_ident(operation, names);
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

    /// Issue #233: `default` was once the selector `Range(0)`, so a `0XX` entry decoded under the
    /// `default` response's codec (or the fallback, with no `default`), and its variant and
    /// runtime selector collided with `default`'s. Each selector now reads only its own response.
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
