//! The request body an operation method sends, encoded by its media type.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, MediaType, Operation, Ty, TypeKind};
use crate::name::{Names, OperationBindings};

use super::params::delimiter_tokens;

/// The statements that encode the operation's request body onto the request and set its
/// `Content-Type`, by the body's media type; empty when the operation sends no typed body.
pub(super) fn body_send_tokens(
    operation: &Operation,
    api: &Api,
    names: &Names,
    bindings: &OperationBindings,
) -> TokenStream {
    let request_binding = &bindings.request;
    // A body the specification marks `required: false` arrives as `Option<&T>`, so the whole send
    // block runs only when the caller supplied one. Only a body that lowered to a type takes an
    // argument at all, so an untyped body is never wrapped.
    let optional_body_binding = operation
        .request_body
        .as_ref()
        .filter(|body| !body.required && body.ty.is_some())
        .and(bindings.body.as_ref());
    let body_send = if let Some((ty, media, encoding)) = operation
        .request_body
        .as_ref()
        .and_then(|body| body.ty.map(|ty| (ty, body.media, body.encoding.clone())))
    {
        let body_binding = bindings
            .body
            .as_ref()
            .expect("request body argument allocated");
        let content_type = &operation
            .request_body
            .as_ref()
            .expect("request body exists")
            .content_type;
        // A raw byte body (`bytes::Bytes`, from `format: binary` / `contentEncoding: base64`) is sent
        // as-is regardless of the declared media — `Bytes` is not `Display`, so it can never go
        // through `.to_string()`. This must be checked before the media match so a `text/plain` (or
        // any) media over a `Bytes` schema does not miscompile. This one raw-bytes send is shared
        // with the octet-stream arm below, so no path sends raw bytes without `Content-Type`.
        let raw_bytes_send = quote! {
            #request_binding = #request_binding
                .header(reqwest::header::CONTENT_TYPE, #content_type)
                .body(#body_binding.clone());
        };
        if matches!(
            api.types.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Bytes)
        ) {
            raw_bytes_send
        } else {
            match media {
                MediaType::Json => {
                    quote! { #request_binding = #request_binding.json(#body_binding); }
                }
                // XML: serialize the typed body to an XML string via the runtime's quick-xml helper
                // and set it as the body with the XML content-type. `to_xml` yields
                // `Error<Infallible>`, widened to the operation's error type.
                MediaType::Xml => quote! {
                    let #body_binding = support::to_xml(#body_binding)
                        .map_err(support::Error::widen)?;
                    #request_binding = #request_binding
                        .header(reqwest::header::CONTENT_TYPE, "application/xml")
                        .body(#body_binding);
                },
                MediaType::FormUrlEncoded => {
                    // Rendered property by property through the resolved Encoding Object rather
                    // than `RequestBuilder::form`, whose encoder rejects arrays and objects at
                    // runtime and cannot express a per-property content type.
                    let properties = form_properties_tokens(&encoding);
                    quote! {
                        const FORM_PROPERTIES: &[support::FormProperty] = &[#(#properties),*];
                        let #body_binding = support::serialize_form_body(
                            #body_binding,
                            FORM_PROPERTIES,
                        )
                        .map_err(support::Error::request_construction)?;
                        #request_binding = #request_binding
                            .header(
                                reqwest::header::CONTENT_TYPE,
                                "application/x-www-form-urlencoded",
                            )
                            .body(#body_binding);
                    }
                }
                MediaType::Text => quote! {
                    #request_binding = #request_binding
                        .header(reqwest::header::CONTENT_TYPE, #content_type)
                        .body(#body_binding.to_string());
                },
                // An octet-stream request body's type definition always has kind `TypeKind::Bytes`
                // (the gate in `LowerCtx::lower_request_body` in `oas31/lower/body.rs`, checked
                // again by `ir::check_invariants`), so the `Bytes` branch above takes it and this
                // arm is never reached. It sends the same tokens, so even a looser gate cannot drop the
                // header. Both also refuse a nullable body, which `.body(..)` could not accept.
                MediaType::OctetStream => raw_bytes_send,
                MediaType::Multipart => {
                    emit_multipart_body(ty, api, names, request_binding, body_binding, &encoding)
                }
                // Streaming media are response-only; a streaming request body is rejected during
                // lowering (narrowed `E009`), so this arm is unreachable for any emitted operation.
                MediaType::EventStream | MediaType::Ndjson | MediaType::JsonSequence => quote! {},
            }
        }
    } else {
        quote! {}
    };
    match optional_body_binding {
        None => body_send,
        Some(body_binding) => quote! {
            if let Some(#body_binding) = #body_binding {
                #body_send
            }
        },
    }
}

/// Render one resolved [`BodyEncoding`](crate::ir::BodyEncoding) as `support::FormProperty`
/// const entries.
fn form_properties_tokens(encoding: &crate::ir::BodyEncoding) -> Vec<TokenStream> {
    encoding
        .properties
        .iter()
        .map(|property| {
            let name = property.name.clone();
            let mode = form_mode_tokens(&property.mode);
            quote! { support::FormProperty { name: #name, mode: #mode } }
        })
        .collect()
}

/// Render one property's encoding mode.
fn form_mode_tokens(mode: &crate::ir::EncodingMode) -> TokenStream {
    match mode {
        crate::ir::EncodingMode::Media { codec, .. } => match codec {
            MediaType::Json => quote! { support::FormMode::Json },
            _ => quote! { support::FormMode::Text },
        },
        crate::ir::EncodingMode::Style {
            style,
            explode,
            allow_reserved,
        } => {
            let style = form_style_tokens(style);
            let encoding = if *allow_reserved {
                quote! { support::PercentEncoding::Reserved }
            } else {
                quote! { support::PercentEncoding::Form }
            };
            quote! {
                support::FormMode::Style {
                    style: #style,
                    explode: #explode,
                    encoding: #encoding,
                }
            }
        }
    }
}

/// The runtime `FormStyle` of an RFC 6570-mode form or multipart property: `spaceDelimited` /
/// `pipeDelimited` and `deepObject` keep their style, and every other style serializes as `form`.
fn form_style_tokens(style: &crate::ir::ParamStyle) -> TokenStream {
    match style {
        crate::ir::ParamStyle::Delimited(delimiter) => {
            let delimiter = delimiter_tokens(*delimiter);
            quote! { support::FormStyle::Delimited(#delimiter) }
        }
        crate::ir::ParamStyle::DeepObject => quote! { support::FormStyle::DeepObject },
        _ => quote! { support::FormStyle::Form },
    }
}

/// Emit the body send for a `multipart/form-data` request body: build a `reqwest::multipart::Form`
/// from the typed body struct, one part per field in declaration order (part order is
/// deterministic), and attach it to the request.
///
/// A property in RFC 6570 mode (its Encoding Object sets `style`, `explode`, or `allowReserved`)
/// is style-serialized, one plain text part per value. Otherwise the property's own lowered type
/// decides how its value becomes bytes — the only thing the generator can know for a media type it
/// has no codec for: a binary field (`bytes::Bytes`) becomes a bytes part, a scalar a text part via
/// `Display`, and anything composite a JSON-encoded text part; and that part carries the
/// Content-Type its Encoding Object resolves to — explicit, or defaulted from the property's type
/// by the specification's table — and any header the Encoding Object pins to a literal value. An
/// optional or nullable field adds its part only when it is `Some`. Lowering guarantees a multipart
/// body is an object schema (anything else is rejected as `E009`), so the non-struct fallback
/// stays a no-op.
fn emit_multipart_body(
    ty: Ty,
    api: &Api,
    names: &Names,
    request_binding: &crate::name::Ident,
    body_binding: &crate::name::Ident,
    encoding: &crate::ir::BodyEncoding,
) -> TokenStream {
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
        let property = encoding
            .properties
            .iter()
            .find(|property| property.name == field.name.wire);
        let headers = property
            .map(|property| property.headers.as_slice())
            .unwrap_or(&[]);
        let extra_headers = (!headers.is_empty()).then(|| {
            let inserts = headers.iter().map(|(name, value)| {
                quote! {
                    if let (Ok(name), Ok(value)) = (
                        reqwest::header::HeaderName::try_from(#name),
                        reqwest::header::HeaderValue::try_from(#value),
                    ) {
                        part_headers.insert(name, value);
                    }
                }
            });
            quote! {
                let mut part_headers = reqwest::header::HeaderMap::new();
                #(#inserts)*
                part = part.headers(part_headers);
            }
        });
        // `receiver` is the value for method calls (`.to_vec()`/`.to_string()` auto-ref); `reference`
        // is an explicit `&value` for `serde_json::to_string`, which takes `&T`. Splitting them keeps
        // the emitted code free of `clippy::needless_borrow` on the method-call receivers.
        let add_part = |receiver: &TokenStream, reference: &TokenStream| {
            let build = match &property.map(|property| &property.mode) {
                // RFC 6570 mode: the part value is style-serialized, and an array becomes one part
                // per item under the same name. `contentType` is inert in this mode.
                Some(crate::ir::EncodingMode::Style {
                    style,
                    explode,
                    ..
                }) => {
                    let style = form_style_tokens(style);
                    return quote! {
                        for value in support::serialize_multipart_values(#reference, #style, #explode)
                            .map_err(support::Error::request_construction)?
                        {
                            form = form.part(#wire, reqwest::multipart::Part::text(value));
                        }
                    };
                }
                _ => match kind {
                    // A binary/bytes property → a file/bytes part carrying the raw bytes.
                    Some(TypeKind::Bytes) => quote! {
                        let mut part = reqwest::multipart::Part::bytes(#receiver.to_vec());
                    },
                    // A scalar property → a text part rendered through `Display`.
                    Some(TypeKind::Primitive(_) | TypeKind::Enum(_)) => quote! {
                        let mut part = reqwest::multipart::Part::text(#receiver.to_string());
                    },
                    // Codegen sees only a checked `Api`; JSON-encoding a reservation would pick a
                    // part shape for a body nobody lowered.
                    Some(TypeKind::Reserved) => unreachable!(
                        "a reservation reached codegen; `check_invariants` should have rejected it"
                    ),
                    // Any composite property → a JSON-encoded text part.
                    _ => quote! {
                        let mut part = reqwest::multipart::Part::text(
                            serde_json::to_string(#reference)
                                .map_err(support::Error::request_construction)?,
                        );
                    },
                },
            };
            let content_type = match property.map(|property| &property.mode) {
                Some(crate::ir::EncodingMode::Media { content_type, .. }) => {
                    Some(content_type.clone())
                }
                _ => None,
            };
            // `mime_str` consumes the part, so a malformed content type from the spec surfaces as
            // a request-construction error rather than being silently dropped.
            let set_type = content_type.map(|content_type| {
                quote! {
                    part = part
                        .mime_str(#content_type)
                        .map_err(support::Error::request_construction)?;
                }
            });
            quote! {
                #build
                #set_type
                #extra_headers
                form = form.part(#wire, part);
            }
        };
        if optional {
            // `value` is already `&T` from the `if let Some(value) = &body.field` binding.
            let stmt = add_part(&quote! { value }, &quote! { value });
            quote! {
                if let Some(value) = &#body_binding.#field_ident {
                    #stmt
                }
            }
        } else {
            add_part(
                &quote! { #body_binding.#field_ident },
                &quote! { &#body_binding.#field_ident },
            )
        }
    });
    quote! {
        let mut form = reqwest::multipart::Form::new();
        #(#parts)*
        #request_binding = #request_binding.multipart(form);
    }
}
