//! Responses Objects: each status's body and headers.

use crate::diag::{Code, Diagnostic};
use crate::ir::{Docs, MediaType, Response, ResponseHeader, Responses, StatusSpec, Ty, TypeKind};
use crate::oas31::media::{is_cbor, media_essence, media_object_is_opaque};
use crate::oas31::{MediaTypeObject, ResponseObject};

use super::content::{choose_media, lower_media_type, BodyPosition, ChosenMedia};
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    pub(super) fn lower_responses(
        &mut self,
        responses: &crate::oas31::ResponsesObject,
    ) -> Responses {
        let mut by_status = Vec::new();
        for (status, response) in &responses.by_status {
            if let Some(status) = parse_status(status) {
                if let Some(response) = self
                    .resolve_response(response)
                    .map(|r| self.lower_response(&r))
                {
                    by_status.push((status, response));
                }
            }
        }
        let default = responses
            .default
            .as_ref()
            .and_then(|response| self.resolve_response(response))
            .map(|response| self.lower_response(&response));
        Responses { by_status, default }
    }

    fn lower_response(&mut self, response: &ResponseObject) -> Response {
        let body = choose_media(
            &response.content,
            &response.provenance,
            self.diags,
            BodyPosition::Response,
            media_object_is_opaque,
        )
        .and_then(
            |ChosenMedia {
                 media: media_name,
                 value: object,
                 narrowing,
             }| {
                let lowered = self.response_narrowing(|ctx| {
                    ctx.lower_chosen_response_body(response, media_name, object)
                })?;
                // `W014` is emitted only once the gates in `lower_chosen_response_body` have
                // accepted the selection; a refused one is reported by its `E009` alone.
                if let Some(narrowing) = narrowing {
                    self.diags.emit(narrowing);
                }
                // A CBOR body is read as opaque octets, so its key is announced in `Accept`: the
                // media type is the only place the response's representation is stated.
                let essence = media_essence(media_name);
                let accept = is_cbor(essence).then(|| essence.to_owned());
                Some((lowered, accept))
            },
        );
        let (body, accept) = match body {
            Some((body, accept)) => (Some(body), accept),
            None => (None, None),
        };
        // A streaming response media (`text/event-stream` / `application/x-ndjson`) records its
        // framing; the body is then the streamed item type `T`. A whole-body response has no
        // framing. Framing is recorded in every response position; streaming only takes effect when
        // this is the operation's single success body (see `Responses::stream_success`), and a
        // bodied stream anywhere else rejects the operation (`Responses::stream_outside_single_success`).
        let headers = self.lower_response_headers(response);
        Response {
            media: body.map(|(media, _, _)| media),
            body: body.and_then(|(_, ty, _)| ty),
            stream: body.and_then(|(_, _, stream)| stream),
            accept,
            headers,
        }
    }

    /// Lower the response body entry [`Self::lower_response`] selected into its media, type, and
    /// framing, or `None` when one of the response-body gates (each an `E009`) refuses it.
    fn lower_chosen_response_body(
        &mut self,
        response: &ResponseObject,
        media_name: &str,
        object: &MediaTypeObject,
    ) -> Option<(MediaType, Option<Ty>, Option<crate::ir::Framing>)> {
        let object = self.resolve_media_object(object, media_name)?;
        let media = lower_media_type(media_name, &response.provenance, self.diags)?;
        // The whole-body `schema`, lowered only on the paths that read it.
        let lower_whole_body = |ctx: &mut Self| {
            object
                .schema
                .as_ref()
                .and_then(|schema| ctx.lower_schema_ref(schema, "ResponseBody"))
        };
        // For a sequential/streaming media (`text/event-stream` / `application/x-ndjson`),
        // OpenAPI 3.2 gives the PER-ITEM type in `itemSchema`; a whole-body `schema` does not
        // apply to a stream, so `itemSchema` is preferred (falling back to `schema` for the
        // pre-3.2 form where the item type was written as `schema`). On a non-streaming media
        // `itemSchema` is meaningless: acknowledge it with `W010` and use `schema`.
        let (ty, stream) = if let Some(framing) = media.stream_framing() {
            if let Some(item_schema) = object.item_schema.as_ref() {
                if media == MediaType::EventStream && self.document.is_oas32 {
                    if let Some(json) =
                        crate::oas31::sse::json_data_schema(item_schema, self.resolver, self.diags)
                    {
                        let ty = self.lower_schema_or(&json.schema, "ResponseBody");
                        self.warn_structural_default_or(
                            &json.schema,
                            "an SSE JSON data content schema",
                        );
                        (ty, Some(crate::ir::Framing::SseJsonData))
                    } else {
                        (
                            self.lower_schema_ref(item_schema, "ResponseBody"),
                            Some(crate::ir::Framing::SseEvent),
                        )
                    }
                } else {
                    (
                        self.lower_schema_ref(item_schema, "ResponseBody"),
                        Some(framing),
                    )
                }
            } else if self.document.is_oas32 && object.schema.is_some() {
                // E009 case: sequence-schema
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(
                                "in OpenAPI 3.2, `schema` on sequential media describes the \
                                 complete sequence; use `itemSchema` for a streaming client result",
                            )
                            .remedy("replace `schema` with `itemSchema`, or choose a non-sequential response media type")
                            .emit(self.diags);
                return None;
            } else {
                (lower_whole_body(self), Some(framing))
            }
        } else {
            if object.item_schema.is_some() {
                Diagnostic::warning(Code::Oas32ConstructIgnored, response.provenance.clone())
                    .message(
                        "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; \
                             on this non-streaming media it is not used",
                    )
                    .emit(self.diags);
            }
            (lower_whole_body(self), None)
        };
        if let Some(schema) = object.item_schema.as_ref().filter(|_| stream.is_some()) {
            self.warn_structural_default_ref(schema, "a response body schema");
        } else if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a response body schema");
        }
        if matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
            // E009 case: form-response
            Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                        .message(format!(
                            "media type `{media_name}` is supported for request bodies, not response bodies"
                        ))
                        .remedy("document a JSON, XML, textual, binary, or streaming response, or omit this API segment with spargen::omit!")
                        .emit(self.diags);
            return None;
        }
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets(
                "ResponseBody",
                ty,
                object.schema.is_some(),
                &response.provenance,
            )
        } else {
            ty
        };
        if let Some(ty) = ty {
            if !self.raw_body_compatible(media, ty) {
                // E009 case: raw-schema
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(format!(
                                "media type `{media_name}` requires a string-like or binary response schema"
                            ))
                            .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                            .emit(self.diags);
                return None;
            }
            // A `bytes::Bytes` response is decoded as the raw octets of the body under any
            // media, so `null` is never what arrives, and the byte decoder has no `Option`
            // to build. The raw *text* codec decodes through serde and builds
            // `Option<String>` soundly, so it is not refused here, and neither is a
            // streamed item, which is framed and decoded element by element.
            if stream.is_none() && ty.nullable && self.is_bytes(ty) {
                // E009 case: raw-null
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                    .message(format!(
                        "this `{media_name}` response body is read as raw bytes, whose \
                                 content has no wire representation of `null`, but its schema \
                                 admits `null`"
                    ))
                    .remedy(
                        "remove `null` from the response body schema, or omit this API \
                                 segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        Some((media, ty, stream))
    }

    /// Lower a response's documented headers into typed accessors.
    ///
    /// A header with no typed accessor is skipped, and what is reported depends on why. A
    /// documented `Content-Type`, a shape `simple` serialization cannot express, a `content` media
    /// type that is not JSON or text, a `content` entry without a schema, and a textual one that
    /// is not a single value are warnings (`W011`): the body is what the call returns, and
    /// refusing an otherwise-generatable operation over an unreadable header would be a poor trade.
    /// A defect in the document itself still rejects it, with the error reported where it is
    /// found: a reference that resolves to nothing, a `content` key spargen does not classify
    /// (`E009`), or a schema that fails to lower.
    ///
    /// A header skipped after its schema was lowered has the types that lowering inserted
    /// [elided](Self::elide_unused_union_lowering) where nothing else reaches them, so they are
    /// not emitted as public types nothing refers to (#927); a component the header `$ref`s stays,
    /// since its memo holds it.
    fn lower_response_headers(&mut self, response: &ResponseObject) -> Vec<ResponseHeader> {
        let mut headers = Vec::new();
        for (name, header) in &response.headers {
            // The specification says a documented `Content-Type` header SHALL be ignored: the
            // media type is already the operation's, and a second source would only disagree.
            if name.eq_ignore_ascii_case("content-type") {
                // W011 case: response-content-type
                Diagnostic::warning(Code::DeclarationHasNoEffect, response.provenance.clone())
                    .message(
                        "a documented `Content-Type` response header is ignored; the operation's \
                         media type already determines it",
                    )
                    .emit(self.diags);
                continue;
            }
            let Some(header) = self.resolve_header(header) else {
                continue;
            };
            let header = &header;
            // A Header Object may only use `simple`; the document schema already enforces that.
            let (ty, shape) = if let Some(schema) = &header.schema {
                let mark = self.graph_mark();
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                let Some(shape) = self.header_shape(ty) else {
                    let lowered = mark..self.graph_mark();
                    self.elide_unused_union_lowering(lowered);
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` has a shape `simple` serialization cannot \
                             express, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                };
                (ty, shape)
            } else if let Some((media, object)) = header.content.iter().next() {
                // Resolve first: a header's content may itself be a Reference Object, and reading
                // `schema` off the unresolved shell would find `None` and drop the typed accessor
                // with nothing said.
                let Some(object) = self.resolve_media_object(object, media) else {
                    continue;
                };
                let Some(media) = lower_media_type(media, &header.provenance, self.diags) else {
                    continue;
                };
                // A textual `content` entry describes the field value itself, so it decodes exactly
                // like the `schema:` spelling — the shape gate below is what decides. This is not a
                // rare form: `Content-Range` on a ranged response is routinely documented this way,
                // and refusing it cost a typed accessor for no reason.
                if !matches!(media, MediaType::Json | MediaType::Text) {
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` uses a `content` media type spargen cannot \
                             decode, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                }
                if object.item_schema.is_some() {
                    Diagnostic::warning(Code::Oas32ConstructIgnored, header.provenance.clone())
                        .message(format!(
                            "`itemSchema` has no effect on response header `{name}`: a header \
                             field value is not a sequential media"
                        ))
                        .emit(self.diags);
                }
                let Some(schema) = object.schema.as_ref() else {
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` declares `content` without a schema, so no \
                             typed accessor is generated"
                        ))
                        .remedy("give the content entry a `schema`")
                        .emit(self.diags);
                    continue;
                };
                let mark = self.graph_mark();
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                if media == MediaType::Json {
                    (ty, crate::ir::HeaderShape::Json)
                } else {
                    // Textual content carries the field value verbatim, so only a scalar schema is
                    // representable: a list or an object under `text/plain` says nothing about how
                    // the value is framed, and `simple` is not that framing.
                    let Some(shape @ crate::ir::HeaderShape::Scalar) = self.header_shape(ty) else {
                        let lowered = mark..self.graph_mark();
                        self.elide_unused_union_lowering(lowered);
                        // W011 case: response-header-untyped
                        Diagnostic::warning(
                            Code::DeclarationHasNoEffect,
                            header.provenance.clone(),
                        )
                        .message(format!(
                            "response header `{name}` declares a textual `content` schema that \
                                 is not a single value, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                        continue;
                    };
                    (ty, shape)
                }
            } else {
                continue;
            };
            // `Set-Cookie` is the one field RFC 9110 §5.3 exempts from the comma-joined field-list
            // rule, and 3.2 gives it a section of its own saying each value must be kept on its own
            // line. The declared schema therefore describes ONE cookie, and the accessor is a list
            // of them — a schema that is already a list is taken to be that list.
            let (ty, shape) = if name.eq_ignore_ascii_case("set-cookie") {
                let already_list = match self.graph.get(ty.id).map(|def| &def.kind) {
                    Some(TypeKind::Array(_)) => true,
                    // `header_shape` refused a reservation above, so none reaches here; were one
                    // to, it is not known to be a list, and the declared schema is one cookie.
                    Some(TypeKind::Reserved) => false,
                    _ => false,
                };
                let list = if already_list {
                    ty
                } else {
                    self.insert_type(
                        &format!("Header{name}"),
                        TypeKind::Array(Box::new(ty)),
                        Docs::default(),
                        Some(header.provenance.clone()),
                    )
                };
                (list, crate::ir::HeaderShape::SetCookie)
            } else {
                (ty, shape)
            };
            headers.push(ResponseHeader {
                name: name.clone(),
                ty,
                required: header.required,
                explode: header.explode.unwrap_or(false),
                shape,
                deprecated: header.deprecated,
                docs: Docs {
                    description: header.description.clone(),
                    ..Docs::default()
                },
            });
        }
        headers
    }

    /// The `simple` wire shape of a lowered header type, or `None` when it has none.
    fn header_shape(&self, ty: Ty) -> Option<crate::ir::HeaderShape> {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Null) => {
                Some(crate::ir::HeaderShape::Scalar)
            }
            Some(TypeKind::Array(_)) => Some(crate::ir::HeaderShape::Array),
            Some(TypeKind::Struct(_)) => Some(crate::ir::HeaderShape::Object),
            // Headers are lowered per operation, after every component they reach is filled, so
            // this is not expected. Were it reached, an unknown body has no provable `simple`
            // shape: `None` sends the header to the callers' `W011` (no accessor, warned) instead
            // of guessing one.
            Some(TypeKind::Reserved) => None,
            _ => None,
        }
    }
}

fn parse_status(status: &str) -> Option<StatusSpec> {
    if let Some(prefix) = status.strip_suffix("XX") {
        return Some(StatusSpec::Range(prefix.parse().ok()?));
    }
    Some(StatusSpec::Exact(status.parse().ok()?))
}
