//! What an operation method does around the wire: attach the credentials its `security` selects,
//! and dispatch the response by status into the typed success value or the typed error.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{
    Api, ApiKeyLoc, ErrorShape, HttpScheme, MediaType, Operation, SecurityScheme, SuccessShape, Ty,
    TypeKind,
};
use crate::name::{Names, OperationBindings};

use super::responses::response_payload_ty_tokens;
use super::runtime::to_pascal;
use super::ty::ty_tokens;

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
    error_ident: &proc_macro2::Ident,
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
        // exactly that entry.
        ErrorShape::Single(body_ty) => {
            let mut documented = operation
                .responses
                .by_status
                .iter()
                .filter(|(status, response)| !status.is_success() && response.body.is_some())
                .map(|(status, _)| runtime_status_spec(*status))
                .collect::<Vec<_>>();
            if operation
                .responses
                .default
                .as_ref()
                .is_some_and(|default| default.body.is_some())
            {
                documented.push(quote! { support::StatusSpec::Any });
            }
            let classify = if is_bytes_ty(api, *body_ty) {
                quote! {
                    support::classify_error_bytes(
                        &self.core,
                        response,
                        &[#(#documented),*],
                    )
                    .await
                }
            } else {
                match operation.responses.single_error_media() {
                    Some(MediaType::Xml) => quote! {
                        support::classify_error_xml::<#error_ty>(
                            &self.core,
                            response,
                            &[#(#documented),*],
                        )
                        .await
                    },
                    Some(MediaType::Text) => quote! {
                        support::classify_error_text::<#error_ty>(
                            &self.core,
                            response,
                            &[#(#documented),*],
                        )
                        .await
                    },
                    // JSON. A streaming error media never reaches this arm: lowering rejects a
                    // bodied stream on the error side (`Responses::stream_outside_single_success`),
                    // since a stream cannot be classified as one whole body.
                    _ => quote! {
                        support::classify_error::<#error_ty>(
                            &self.core,
                            response,
                            &[#(#documented),*],
                        )
                        .await
                    },
                }
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
                        let decode = variant_body_decode(operation, api, names, *spec, *ty);
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
    error_ident: &proc_macro2::Ident,
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
        // Multi-status success: read the body once, then dispatch by status in precedence order
        // (exact code ascending, then range ascending) into the matching variant. `default` never
        // enters this shape, so there is no `Default` arm. A success status matching no documented
        // variant is an unexpected-status error — there is no untyped fallback.
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
                        let decode = variant_body_decode(operation, api, names, *spec, *ty);
                        quote! {
                            if #spec_tokens.matches(status) {
                                // A `match`, not `map_err`: the failure moves the headers into
                                // `Decode`, which a closure capturing them would also do on the
                                // success path that still needs them.
                                let value = match #decode {
                                    Ok(value) => value,
                                    Err(path) => {
                                        return Err(support::Error::<#error_ty>::Decode {
                                            status,
                                            headers,
                                            path,
                                            body,
                                            truncated: false,
                                        });
                                    }
                                };
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
fn variant_body_decode(
    operation: &Operation,
    api: &Api,
    names: &Names,
    spec: crate::ir::StatusSpec,
    body_ty: Ty,
) -> TokenStream {
    let ty = response_payload_ty_tokens(body_ty, names, true);
    let media = response_media_for_spec(&operation.responses, spec);
    if is_bytes_ty(api, body_ty) {
        quote! { Ok::<#ty, String>(Box::new(body.clone())) }
    } else if media == Some(MediaType::Text) {
        quote! { support::decode_text_body::<#ty>(&body) }
    } else if media == Some(MediaType::Xml) {
        // Only as the lone body beside bodyless entries on its side: a second bodied response
        // beside an XML one is rejected (`xml_in_multi_status`).
        quote! { support::decode_xml_body::<#ty>(&body) }
    } else {
        quote! {
            serde_json::from_slice::<#ty>(&body)
                .map_err(|error| error.to_string())
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
pub(super) fn success_enum_ident(method_ident: &crate::name::Ident) -> proc_macro2::Ident {
    format_ident!("{}Response", to_pascal(method_ident.as_str()))
}

/// The `PascalCase` variant identifier for a documented status selector: `Status200` for an exact
/// code, `Status2xx` for a range, and `Default` for [`StatusSpec::Default`](crate::ir::StatusSpec)
/// — the label [`crate::name::status_label`] gives the same selector's header struct.
/// Deterministic and, within one enum, unique by construction (each selector appears once).
/// Routed through the `name` escaper for validity.
pub(super) fn status_variant_ident(spec: crate::ir::StatusSpec) -> proc_macro2::Ident {
    let raw = crate::name::status_label(spec);
    format_ident!(
        "{}",
        crate::name::escape(&raw, crate::name::IdentRole::Variant).as_str()
    )
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

/// The chosen media type of the response a shape entry was built from: the `default` response for
/// [`StatusSpec::Default`](crate::ir::StatusSpec), otherwise the `by_status` entry with exactly
/// that selector.
fn response_media_for_spec(
    responses: &crate::ir::Responses,
    spec: crate::ir::StatusSpec,
) -> Option<MediaType> {
    let response = match spec {
        crate::ir::StatusSpec::Default => responses.default.as_ref(),
        crate::ir::StatusSpec::Exact(_) | crate::ir::StatusSpec::Range(_) => responses
            .by_status
            .iter()
            .find(|(candidate, _)| *candidate == spec)
            .map(|(_, response)| response),
    };
    response.and_then(|response| response.media)
}

/// Whether a body is decoded by the raw byte codec, which yields `bytes::Bytes` itself. It reads
/// only the definition's kind, which is sound because `LowerCtx::lower_response` (in
/// `oas31/lower/response.rs`) refuses a nullable `Bytes` body (`E009`): no `Option<bytes::Bytes>`
/// body reaches these decode sites.
pub(super) fn is_bytes_ty(api: &Api, ty: Ty) -> bool {
    matches!(
        api.types.get(ty.id).map(|definition| &definition.kind),
        Some(TypeKind::Bytes)
    )
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
        assert_eq!(StatusSpec::Default.display_label(), "default");
        assert_eq!(StatusSpec::Range(0).display_label(), "0XX");
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
