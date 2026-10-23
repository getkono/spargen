//! Encoding Objects of `multipart` and form bodies: each property's mode, content type and
//! headers.

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{
    AdditionalProps, BodyEncoding, Delimiter, EncodingMode, MediaType, ParamStyle,
    PropertyEncoding, Ty, TypeKind,
};
use crate::oas31::media::{
    classify_media, first_list_element, is_field_name, is_field_value, media_essence,
    media_type_is_well_formed, media_type_with_parameters, ParameterFault,
};
use crate::oas31::{EncodingObject, MediaTypeObject, RefOr, Schema};

use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Resolve the Encoding Objects of a form or multipart request body into a fully-populated
    /// [`BodyEncoding`] — one entry per body property, so the emitted code never has to infer a
    /// default at runtime.
    ///
    /// Returns `None` only when the body is unrepresentable; an encoding that simply has no effect
    /// here is reported as `W011` and dropped.
    pub(super) fn lower_body_encoding(
        &mut self,
        media: MediaType,
        media_name: &str,
        ty: Option<Ty>,
        object: &MediaTypeObject,
    ) -> Option<BodyEncoding> {
        // Encoding diagnostics point at the Media Type Object that declares them, not at the whole
        // request body.
        let at = &object.provenance;
        // Media that is neither form nor multipart is dispositioned once, in
        // `resolve_media_object`, which every Media Type Object passes through.
        if !matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
            return Some(BodyEncoding::default());
        }
        // `prefixEncoding`/`itemEncoding` describe positional parts of an array-shaped body, and
        // the specification scopes both to `multipart`. On multipart spargen generates from an
        // object schema, so there are no positions to encode and the declaration is rejected; on
        // form-urlencoded the specification itself says they do not apply, so they are inert.
        let positional = object
            .prefix_encoding
            .first()
            .map(|(_, at)| ("prefixEncoding", at.clone()))
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = positional {
            if media == MediaType::FormUrlEncoded {
                // W011 case: positional-encoding-form
                Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                    .message(format!(
                        "`{field}` has no effect on `{media_name}`: the specification scopes it to \
                         `multipart` content"
                    ))
                    .emit(self.diags);
            } else {
                // E009 case: positional-encoding
                Diagnostic::error(Code::UnsupportedMediaType, at)
                    .message(format!(
                        "`{field}` describes positional parts of an array-shaped multipart body; \
                         spargen generates `multipart/form-data` from an object schema, which has \
                         no positions"
                    ))
                    .remedy(
                        "use `encoding` keyed by property name, or omit this API segment with \
                         spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let Some(TypeKind::Struct(structure)) =
            ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind)
        else {
            // The body already failed its own shape gate above; don't pile on.
            return Some(BodyEncoding::default());
        };
        let fields: Vec<(String, Ty)> = structure
            .fields
            .iter()
            .map(|field| (field.name.wire.clone(), field.ty))
            .collect();
        let overflow = match &structure.additional {
            AdditionalProps::Typed(ty) => Some(**ty),
            AdditionalProps::Allow | AdditionalProps::Deny => None,
        };
        // Nested encoding describes nested multipart parts (`multipart/mixed` inside a part).
        // Spargen generates one flat level, so a nested field is rejected rather than dropped.
        for (name, encoding) in &object.encoding {
            if let Some((field, at)) = encoding.nested.first() {
                // E009 case: nested-encoding
                Diagnostic::error(Code::UnsupportedMediaType, at.clone())
                    .message(format!(
                        "`encoding.{name}.{field}` describes a nested multipart part, which \
                         spargen does not generate"
                    ))
                    .remedy("flatten the body, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
        }
        // An `encoding` key naming no property has nothing to apply to.
        for name in object.encoding.keys() {
            if !fields.iter().any(|(wire, _)| wire == name) {
                // W011 case: encoding-unknown-property
                Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
                    .message(format!(
                        "`encoding` entry `{name}` names no property of the body schema, so it is \
                         ignored"
                    ))
                    .emit(self.diags);
            }
        }
        let mut properties = Vec::with_capacity(fields.len());
        for (name, field_ty) in fields {
            let declared = object.encoding.get(&name);
            let mode = self.encoding_mode(declared, field_ty, media, &name, at)?;
            let headers = self.encoding_headers(declared, media, &name)?;
            properties.push(PropertyEncoding {
                name,
                mode,
                headers,
            });
        }
        // The members of a typed `additionalProperties` overflow are sent too, as the JSON codec
        // sends them. An Encoding Object is keyed by property name, so none can name them, and
        // they take the default for their value type, under the same rules as an undeclared
        // property (a binary overflow on form-urlencoded is refused, as a binary property is).
        let additional = match overflow {
            Some(overflow_ty) => {
                Some(self.encoding_mode(None, overflow_ty, media, "additionalProperties", at)?)
            }
            None => None,
        };
        Some(BodyEncoding {
            properties,
            additional,
        })
    }

    /// Apply the Encoding Object's mode switch for one property, or `None` once a refusal is
    /// reported.
    ///
    /// Any of `style`, `explode` or `allowReserved` selects style mode
    /// ([`Self::encoding_style_mode`]); otherwise media mode ([`Self::encoding_media_mode`]).
    fn encoding_mode(
        &mut self,
        declared: Option<&EncodingObject>,
        field_ty: Ty,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Option<EncodingMode> {
        // Presence of any RFC 6570 field selects query-style serialization outright, and makes
        // `contentType` inert — the specification is explicit that it is then ignored.
        if let Some(encoding) = declared {
            if encoding.style.is_some()
                || encoding.explode.is_some()
                || encoding.allow_reserved.is_some()
            {
                return self.encoding_style_mode(encoding, field_ty, media, name);
            }
        }
        self.encoding_media_mode(declared, field_ty, media, name, at)
    }

    /// Style mode for one property. It refuses (`E009`) a delimited style with `explode: true`,
    /// and, on multipart, `deepObject` and an object property; `allowReserved` on multipart is
    /// inert (`W011`).
    fn encoding_style_mode(
        &mut self,
        encoding: &EncodingObject,
        field_ty: Ty,
        media: MediaType,
        name: &str,
    ) -> Option<EncodingMode> {
        let style_name = encoding.style.as_deref().unwrap_or("form");
        let style = match style_name {
            "form" => ParamStyle::Form,
            "spaceDelimited" => ParamStyle::Delimited(Delimiter::Space),
            "pipeDelimited" => ParamStyle::Delimited(Delimiter::Pipe),
            "deepObject" => ParamStyle::DeepObject,
            // The document schema enumerates these four and rejects any other as `E011`, so a
            // validated document does not reach this arm; it reports that same code for a path
            // that skips validation.
            _ => {
                Diagnostic::error(Code::InvalidInput, encoding.provenance.clone())
                    .message(format!(
                        "`encoding.{name}.style: {style_name}` is not a form style"
                    ))
                    .emit(self.diags);
                return None;
            }
        };
        let explode = encoding
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form));
        // The specification's own serialization table marks the delimited styles with
        // `explode: true` as *n/a* — undefined. The identical parameter-side construct is
        // already `E010`; without this an Encoding Object could declare it and have the
        // `explode` silently ignored.
        if explode && matches!(style, ParamStyle::Delimited(_)) {
            // E009 case: undefined-rfc6570
            Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}.style: {style_name}` with `explode: true` is \
                     undefined: the specification's serialization table gives no value for \
                     that combination"
                ))
                .remedy("set `explode: false`, or use `style: form`")
                .emit(self.diags);
            return None;
        }
        // `deepObject` builds `name[key]=value` query fragments. A multipart part carries
        // its name in `Content-Disposition` and its value alone, so there is nowhere for
        // that syntax to go and no defined representation to fall back on.
        if media == MediaType::Multipart && style == ParamStyle::DeepObject {
            // E009 case: undefined-rfc6570
            Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}.style: deepObject` is defined only for `in: query`; \
                     it has no `multipart/form-data` part representation"
                ))
                .remedy(
                    "use `style: form`, give the property a `contentType` such as \
                     `application/json`, or omit this API segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // An object property under RFC 6570 serialization: the specification says the
        // Encoding Object applies to the *entire value* for a non-array property, but
        // defines no part representation for an object, so there is nothing to generate.
        if media == MediaType::Multipart
            && matches!(
                self.graph.get(field_ty.id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            )
        {
            // E009 case: undefined-rfc6570
            Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}` selects RFC 6570 serialization for an object \
                     property, which has no defined `multipart/form-data` part \
                     representation"
                ))
                .remedy(
                    "give the property a `contentType` such as `application/json` instead \
                     of `style`/`explode`/`allowReserved`, or omit this API segment with \
                     spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // Multipart part values are never percent-encoded, so `allowReserved` is inert.
        let allow_reserved = encoding.allow_reserved.unwrap_or(false);
        if allow_reserved && media == MediaType::Multipart {
            // W011 case: allow-reserved-multipart
            Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                .message(
                    "`allowReserved` has no effect on `multipart/form-data`: part values \
                     are not percent-encoded",
                )
                .emit(self.diags);
        }
        Some(EncodingMode::Style {
            style,
            explode,
            allow_reserved: allow_reserved && media != MediaType::Multipart,
        })
    }

    /// Media mode for one property: a part's bytes come from the property's lowered type and its
    /// declared `contentType` ([`Self::declared_content_type`]), else the default for the type,
    /// rides on it as the header. A declared `contentType` is refused (`E009`) where it and the
    /// bytes cannot agree: a property rendered as JSON (anything but a scalar or bytes) whose
    /// declared type is not JSON, on multipart (the part's header) and on form-urlencoded (the
    /// field's serialization syntax) alike. A binary property on form-urlencoded is refused
    /// (`E009`) whether or not a `contentType` made it binary.
    fn encoding_media_mode(
        &mut self,
        declared: Option<&EncodingObject>,
        field_ty: Ty,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Option<EncodingMode> {
        let explicit = declared.and_then(|encoding| encoding.content_type.as_deref());
        let content_type = match explicit {
            Some(list) => self.declared_content_type(list, declared, media, name, at)?,
            None => self.default_content_type(field_ty),
        };
        // The declared `contentType` is a wire *header*; how the value is rendered into bytes is
        // decided by the property's own lowered type. That is what lets a part declare
        // `application/sdp` (which spargen has no codec for) over a string property and still be
        // sent correctly, with the declared header attached. Media types are case-insensitive
        // (RFC 9110 § 8.3.1) and `classify_media` matches them so: `Application/JSON` selects the
        // codec `application/json` does, here and in the refusal below.
        let classified = classify_media(media_essence(&content_type)).map(|(codec, _)| codec);
        let codec = match classified {
            Some(codec @ (MediaType::Json | MediaType::Text | MediaType::OctetStream)) => codec,
            _ => self.natural_codec(field_ty),
        };
        // That header-rides rule holds only where the header and the bytes agree. A scalar part
        // is the text the document described and a bytes part is whatever the caller supplies, so
        // either carries any well-formed declaration. Every other property is rendered as JSON
        // whatever it declares, so a declaration that is not JSON (`application/xml` over an
        // object, `text/csv` over an array, or a type with no codec at all) would put JSON under
        // a header naming another syntax — bytes no reader of the document predicts. Refused.
        // `classified` is case-insensitive, so `Application/JSON` is judged as the JSON it is.
        //
        // A form-urlencoded field has no header, so the argument there is not the same one, but it
        // reaches the same rule: its `contentType` is the only statement of the syntax the field's
        // value is serialized in before percent-encoding (the specification's form examples), so
        // it is what a server decodes the field by. Spargen serializes a non-scalar field only as
        // JSON (`FormMode::Json`); XML or any other syntax has no field codec, and `text/plain`
        // over an object or array has no defined rendering (the runtime's `FormMode::Text` refuses
        // a nested value, failing every call). Either way the declaration cannot be honoured, so
        // it is refused rather than sent as JSON or generated to fail.
        if matches!(media, MediaType::Multipart | MediaType::FormUrlEncoded)
            && explicit.is_some()
            && self.natural_codec(field_ty) == MediaType::Json
            && classified != Some(MediaType::Json)
        {
            // E009 case: non-json-structured
            Diagnostic::error(Code::UnsupportedMediaType, encoding_site(declared, at))
                .message(if media == MediaType::Multipart {
                    format!(
                        "property `{name}` declares `contentType: {content_type}`, but it is not \
                         a scalar or binary value, so spargen can send it only as JSON; the \
                         part's bytes would contradict its header"
                    )
                } else {
                    format!(
                        "property `{name}` declares `contentType: {content_type}`, but it is not \
                         a scalar value, so spargen can serialize it into a form field only as \
                         JSON; the field would not be in the syntax the document declares"
                    )
                })
                .remedy(if media == MediaType::Multipart {
                    "declare `application/json` (or a `+json` type), make the property a string \
                     or binary value, or omit this API segment with spargen::omit!"
                } else {
                    "declare `application/json` (or a `+json` type), select RFC 6570 \
                     serialization with `style`/`explode`, make the property a scalar, or omit \
                     this API segment with spargen::omit!"
                })
                .emit(self.diags);
            return None;
        }
        // A form field is a single URL-encoded string; raw bytes have no representation there.
        if media == MediaType::FormUrlEncoded && codec == MediaType::OctetStream {
            // E009 case: form-binary
            Diagnostic::error(Code::UnsupportedMediaType, encoding_site(declared, at))
                // Name what the document wrote: a declared `contentType` is the reason a string
                // property is binary here, while a property whose own schema is binary defaulted to
                // octet-stream and never mentioned a `contentType` at all.
                .message(if explicit.is_some() {
                    format!(
                        "property `{name}` declares `contentType: {content_type}`, which is \
                         binary; a form-urlencoded body cannot carry a binary part"
                    )
                } else {
                    format!(
                        "property `{name}` is binary, which has no \
                         `application/x-www-form-urlencoded` representation"
                    )
                })
                .remedy("send the body as `multipart/form-data`, or encode the value as text")
                .emit(self.diags);
            return None;
        }
        Some(EncodingMode::Media {
            content_type,
            codec,
        })
    }

    /// The media type a declared `contentType` list sends, canonicalized, or `None` once it is
    /// refused (`E009`): a wildcard, not a media type, or a parameter that is not `name=value`
    /// (or, on multipart, one the client cannot send).
    fn declared_content_type(
        &mut self,
        list: &str,
        declared: Option<&EncodingObject>,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Option<String> {
        // `contentType` is a comma-separated list of acceptable types, but a client sends
        // exactly one, so the first element wins. A comma inside a quoted parameter value is
        // part of that element, not a list separator.
        let first = first_list_element(list).to_owned();
        // Only the essence can be a range; a `*` in a parameter value is an ordinary
        // `tchar`.
        if media_essence(&first).contains('*') {
            // E009 case: wildcard-content-type
            Diagnostic::error(Code::UnsupportedMediaType, encoding_site(declared, at))
                .message(format!(
                    "`encoding.{name}.contentType: {first}` is a wildcard; a client must \
                     send one concrete media type"
                ))
                .remedy("name a concrete media type such as `image/png`")
                .emit(self.diags);
            return None;
        }
        // The value is sent verbatim as the part's `Content-Type`, so it is held to the
        // rule a `content` key is: a string that is not a media type at all would
        // otherwise fall through to the natural codec with nothing reported, and
        // fail only when a request is built.
        if !media_type_is_well_formed(media_essence(&first)) {
            // E009 case: content-type-not-media
            Diagnostic::error(Code::UnsupportedMediaType, encoding_site(declared, at))
                .message(format!(
                    "`encoding.{name}.contentType: {first}` is not a media type"
                ))
                .remedy("name a media type such as `text/plain`, as `type/subtype`")
                .emit(self.diags);
            return None;
        }
        // Its parameters are sent too, so they are held to RFC 9110 § 5.6.6: a
        // parameter without `=` (`text/plain; foo`) would otherwise generate with nothing
        // reported and fail only when the part's `Content-Type` is parsed at request time.
        // Only a multipart part sends it (`mime_str`); a form-urlencoded field's
        // `contentType` only picks the codec, so a value the transport could not carry is
        // no reason to refuse it there. A malformed list is refused under either, as the
        // essence rule above is: it is not a media type.
        match media_type_with_parameters(&first) {
            Ok(canonical) => Some(canonical),
            Err(ParameterFault::Unsendable(canonical)) if media != MediaType::Multipart => {
                Some(canonical)
            }
            Err(fault) => {
                let (message, remedy) = match fault {
                    ParameterFault::Malformed => (
                        format!(
                            "`encoding.{name}.contentType: {first}` has a parameter \
                             that is not `name=value` under RFC 9110 § 5.6.6"
                        ),
                        "write each parameter as `name=value`, with a token name and a \
                         token or quoted-string value, such as `text/plain; charset=utf-8`",
                    ),
                    ParameterFault::Unsendable(_) => (
                        format!(
                            "`encoding.{name}.contentType: {first}` has a quoted \
                             parameter value the generated client cannot send: an empty \
                             value, or one holding a `\"` or a tab"
                        ),
                        "quote a non-empty value without `\"` or a tab, or drop the \
                         parameter",
                    ),
                };
                // E009 case: content-type-parameter
                Diagnostic::error(Code::UnsupportedMediaType, encoding_site(declared, at))
                    .message(message)
                    .remedy(remedy)
                    .emit(self.diags);
                None
            }
        }
    }

    /// How a property's value is rendered into bytes, from its lowered type alone.
    fn natural_codec(&self, ty: Ty) -> MediaType {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => MediaType::OctetStream,
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_)) => MediaType::Text,
            // Encodings are lowered per operation, after every component the body reaches is
            // filled, so this is not expected. Were it reached, JSON is the codec that renders any
            // value faithfully, and it agrees with `default_content_type`'s answer for the same
            // placeholder, so the part's header and its bytes cannot disagree.
            Some(TypeKind::Reserved) => MediaType::Json,
            _ => MediaType::Json,
        }
    }

    /// The Encoding Object's default `contentType` for a property, from its lowered type.
    fn default_content_type(&self, ty: Ty) -> String {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => "application/octet-stream".to_owned(),
            Some(TypeKind::Struct(_)) | Some(TypeKind::Union(_)) | Some(TypeKind::Any) => {
                "application/json".to_owned()
            }
            // In 3.1 an array's default follows its item type; 3.2 simplified this to JSON. Both
            // agree that an array of objects is JSON, and spargen sends any array as JSON, which
            // is the 3.2 rule and the only self-consistent reading for a nested array.
            Some(TypeKind::Array(_)) | Some(TypeKind::Tuple(_)) => "application/json".to_owned(),
            Some(TypeKind::Primitive(_)) | Some(TypeKind::Enum(_)) => "text/plain".to_owned(),
            // Not expected, for the reason `natural_codec` states. Were it reached, JSON is what
            // `natural_codec` renders a placeholder as, so it is the header that tells the truth
            // about those bytes; the octet-stream fallback below would not.
            Some(TypeKind::Reserved) => "application/json".to_owned(),
            _ => "application/octet-stream".to_owned(),
        }
    }

    /// The literal extra part headers of one multipart property.
    ///
    /// A Header Object *describes* a header; it carries no value. Only a schema that pins one —
    /// through `const`, or `default` in its absence, read through a `$ref` to the Header Object or
    /// in its schema ([`Self::literal_header_value`]) — gives a client something to send. A
    /// `Content-Type` entry is skipped (`contentType` describes it), `headers` on a body that is
    /// not multipart is inert (`W011`), and so is a header that pins no value (`W011`); a
    /// reference that resolves to nothing is reported where it is resolved (`E004`) and nothing
    /// more is said of that header.
    ///
    /// What is sent is held to RFC 9110: a name that is not a `token`, or a pinned literal that
    /// is not a field value, is refused (`E009`), so `None` is returned once every such header
    /// has been reported.
    fn encoding_headers(
        &mut self,
        declared: Option<&EncodingObject>,
        media: MediaType,
        name: &str,
    ) -> Option<Vec<(String, String)>> {
        let Some(encoding) = declared else {
            return Some(Vec::new());
        };
        if encoding.headers.is_empty() {
            return Some(Vec::new());
        }
        if media != MediaType::Multipart {
            // W011 case: encoding-headers-non-multipart
            Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}.headers` applies only to `multipart` content"
                ))
                .emit(self.diags);
            return Some(Vec::new());
        }
        let mut headers = Vec::new();
        let mut refused = false;
        for (header_name, header) in &encoding.headers {
            // `Content-Type` is described by `contentType`, not here.
            if header_name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            // A `$ref` here, to the Header Object or inside it to its schema, is resolved rather
            // than treated as pinning nothing: the target may well declare the `const` that gives
            // the client something to send, and reporting "pins no value" without looking would
            // name the wrong reason. An unresolvable reference is `E004`, from `resolve_header`
            // or `literal_header_value`, not a warning.
            let schema = match header {
                RefOr::Item(header) => header.schema.clone(),
                RefOr::Ref(_) => match self.resolve_header(header) {
                    Some(resolved) => resolved.schema,
                    // `resolve_header` already reported the unresolvable reference; adding
                    // "pins no value" would name a second, wrong reason for one defect.
                    None => continue,
                },
            };
            let literal = match schema.map(|schema| self.literal_header_value(&schema)) {
                Some(HeaderLiteral::Pinned(value)) => Some(value),
                Some(HeaderLiteral::Unpinned) | None => None,
                // Reported where the reference was resolved, for the same reason as above.
                Some(HeaderLiteral::Unreadable) => continue,
            };
            match literal {
                // The pair is sent verbatim as a part header, so a name or value the HTTP field
                // grammar refuses would otherwise generate and fail, or be dropped, only when a
                // request is built.
                Some(_) if !is_field_name(header_name) => {
                    // E009 case: part-header
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.headers` names `{}`, which is not an HTTP field \
                             name: RFC 9110 § 5.1 requires a token, one or more letters, \
                             digits, or characters among !#$%&'*+-.^_`|~",
                            header_name.escape_debug()
                        ))
                        .remedy("rename the header, or omit this API segment with spargen::omit!")
                        .emit(self.diags);
                    refused = true;
                }
                Some(value) if !is_field_value(&value) => {
                    // E009 case: part-header
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.headers.{header_name}` pins `{}`, which is not an \
                             HTTP field value: RFC 9110 § 5.5 admits no control character and no \
                             leading or trailing whitespace",
                            value.escape_debug()
                        ))
                        .remedy(
                            "pin a value of visible characters, with spaces or tabs only between \
                             them",
                        )
                        .emit(self.diags);
                    refused = true;
                }
                Some(value) => headers.push((header_name.clone(), value)),
                None => {
                    // W011 case: encoding-header-no-value
                    Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.headers.{header_name}` pins no value, so there is \
                             nothing for the client to send"
                        ))
                        .remedy("give the header schema a `const` (or a `default`) value")
                        .emit(self.diags);
                }
            }
        }
        (!refused).then_some(headers)
    }

    /// The literal value a header schema pins: its own `const`, else its own `default`, and where
    /// it states neither, those of the schema a `$ref` in it names (#701). The reference is read
    /// through component aliases and through targets that are themselves a `$ref` beside a sibling
    /// keyword, as [`Self::ref_target_body`] reads it, so `{ $ref: V }` pins what `V` pins.
    ///
    /// This is where the header schema's written reference is resolved, and a reference that
    /// resolves to nothing is reported here (`E004`). That outcome, a chain
    /// [`Self::ref_target_body`] cannot read past it, and a chain of sibling-bearing `$ref`s that
    /// runs past `MAX_SCHEMA_DEPTH` are [`HeaderLiteral::Unreadable`]: the defect is reported
    /// once, not again as "pins no value".
    fn literal_header_value(&mut self, schema: &RefOr<Schema>) -> HeaderLiteral {
        let (mut reference, mut at) = match schema {
            RefOr::Item(schema) => {
                if let Some(literal) = pinned_literal(schema) {
                    return literal;
                }
                let Some(reference) = &schema.reference else {
                    return HeaderLiteral::Unpinned;
                };
                (reference.clone(), schema.provenance.clone())
            }
            RefOr::Ref(reference) => (reference.reference.clone(), reference.provenance.clone()),
        };
        if self.resolver.resolve(&reference, &at, self.diags).is_err() {
            return HeaderLiteral::Unreadable;
        }
        for _ in 0..super::MAX_SCHEMA_DEPTH {
            let Some(target) = self.ref_target_body(&reference, &at) else {
                return HeaderLiteral::Unreadable;
            };
            if let Some(literal) = pinned_literal(&target) {
                return literal;
            }
            let Some(next) = &target.reference else {
                return HeaderLiteral::Unpinned;
            };
            reference.clone_from(next);
            at = target.provenance.clone();
        }
        // A bare alias cycle ends in `ref_target_body`. A chain of `$ref`s with siblings that runs
        // past the bound is a cycle through them, which component lowering rejects (`E013`), or a
        // chain deeper than it reads (`E014`); either way the defect is reported there, not here.
        HeaderLiteral::Unreadable
    }

    /// Report the encoding fields of a Media Type Object that cannot take effect in this position.
    pub(super) fn note_inert_encoding(
        &mut self,
        object: &crate::oas31::MediaTypeObject,
        media_name: &str,
    ) {
        let declared = object
            .encoding
            .first()
            .map(|(_, encoding)| ("encoding", encoding.provenance.clone()))
            .or_else(|| {
                object
                    .prefix_encoding
                    .first()
                    .map(|(_, at)| ("prefixEncoding", at.clone()))
            })
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = declared {
            // W011 case: encoding-on-other-media
            Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                .message(format!(
                    "`{field}` has no effect on `{media_name}`: it applies only to `multipart` \
                     and `application/x-www-form-urlencoded` content"
                ))
                .emit(self.diags);
        }
    }
}

/// Where a refusal of one property's encoding points: the property's own Encoding Object when
/// the document wrote one, else `at`, the body the property belongs to.
fn encoding_site(declared: Option<&EncodingObject>, at: &Provenance) -> Provenance {
    declared.map_or_else(|| at.clone(), |encoding| encoding.provenance.clone())
}

/// What an encoding header's schema pins, as [`LowerCtx::literal_header_value`] reads it.
enum HeaderLiteral {
    /// The scalar literal sent as the part header's value.
    Pinned(String),
    /// No `const` or `default`, or one that is not a scalar: nothing to send (`W011`).
    Unpinned,
    /// A `$ref` the schema is read through could not be followed; its defect is reported where
    /// the reference is resolved, so nothing more is said here.
    Unreadable,
}

/// The literal `schema` itself pins, if it states a `const` or a `default`: `const` first, and a
/// non-scalar value as [`HeaderLiteral::Unpinned`]. `None` where it states neither.
fn pinned_literal(schema: &Schema) -> Option<HeaderLiteral> {
    let value = schema.const_value.as_ref().or(schema.default.as_ref())?;
    Some(match &value.node {
        crate::source::Node::String(text) => HeaderLiteral::Pinned(text.clone()),
        crate::source::Node::Bool(value) => HeaderLiteral::Pinned(value.to_string()),
        crate::source::Node::Number(number) => HeaderLiteral::Pinned(match number {
            crate::source::Number::Int(value) => value.to_string(),
            crate::source::Number::UInt(value) => value.to_string(),
            crate::source::Number::Float(value) => value.to_string(),
        }),
        _ => HeaderLiteral::Unpinned,
    })
}
