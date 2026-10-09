//! Request Body Objects: choosing the `content` entry a request sends and typing its body.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    Docs, MediaType, Prim, RequestBody, ScalarRepr, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
};
use crate::oas31::media::{
    classify_media_range, media_essence, media_essence_is_suffix_range, media_object_is_opaque,
    request_media_candidates,
};
use crate::oas31::{MediaTypeObject, RequestBodyObject};

use super::content::{
    alternative_media_ignored, choose_media, lower_media_type, BodyPosition, ChosenMedia,
};
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    pub(super) fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        // A structured-suffix range such as `application/*+json` ranks with the concrete types its
        // suffix covers, so it could win a tie or a rank and then be refused below as a range. While
        // a sibling can be sent, it is withheld from the choice and reported as not selected.
        let (candidates, withheld) = request_media_candidates(&body.content);
        let ChosenMedia {
            media: media_name,
            value: object,
            narrowing,
        } = choose_media(
            &candidates,
            &body.provenance,
            self.diags,
            BodyPosition::Request,
            |object: &&crate::oas31::MediaTypeObject| media_object_is_opaque(object),
        )?;
        let lowered = self.lower_chosen_request_body(body, media_name, object)?;
        // Both `W014`s are emitted only now that every gate above has accepted the selection: a
        // refused one is reported by its `E009` alone, since no method narrows to a body that is
        // not lowered. First the alternatives `choose_media` passed over, then the withheld suffix
        // ranges — always in this order.
        let withheld = alternative_media_ignored(media_name, &withheld, &body.provenance);
        for warning in narrowing.into_iter().chain(withheld) {
            self.diags.emit(warning);
        }
        Some(lowered)
    }

    /// Lower the request body entry [`Self::lower_request_body`] selected, or `None` when one of
    /// the request-body gates (each an `E009`) refuses it.
    fn lower_chosen_request_body(
        &mut self,
        body: &RequestBodyObject,
        media_name: &str,
        object: &MediaTypeObject,
    ) -> Option<RequestBody> {
        let object = self.resolve_media_object(object, media_name)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
        // A media *range* describes what a server may return, not what a client sends: `Content-Type`
        // requires a concrete type/subtype (RFC 9110 § 8.3), and a generated request puts its media
        // key on the wire verbatim. Emitting `Content-Type: video/*` would be an undispatchable
        // header, and picking a concrete member of the family would be spargen inventing what the
        // document declined to say — so it is rejected rather than guessed at. `choose_media`
        // selects a range for a request only once no concrete key beside it classifies, so this
        // fires only when the document offers nothing else spargen can send.
        if classify_media_range(media_essence(media_name)).is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is a media range, which describes a family rather \
                     than the concrete `Content-Type` a request must send"
                ))
                .remedy(
                    "name the concrete media type the request body is sent as, or omit this API \
                     segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // A structured-suffix range such as `application/*+json` is a range for the same reason,
        // even though the suffix arms classify it as the codec its suffix names.
        if media_essence_is_suffix_range(media_essence(media_name)) {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is a media range, which describes a family rather \
                     than the concrete `Content-Type` a request must send"
                ))
                .remedy(
                    "name the concrete media type the request body is sent as, or omit this API \
                     segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // Streaming media is a response-only construct: a `text/event-stream` / `application/x-ndjson`
        // *request* body has no representation here, so it stays rejected (narrowed `E009`) rather
        // than silently degrade. (`choose_media` only picks it when no whole-body alternative exists.)
        if media.stream_framing().is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is only supported for streaming response bodies, \
                     not request bodies"
                ))
                .remedy("send a non-streaming request body, or omit this API segment with spargen::omit!")
                .emit(self.diags);
            return None;
        }
        // A streaming request body is already rejected above, so any `itemSchema` reaching here sits
        // on a non-streaming media where it is meaningless; acknowledge it with `W010` rather than
        // dropping it silently.
        if object.item_schema.is_some() {
            Diagnostic::warning(Code::Oas32ConstructIgnored, body.provenance.clone())
                .message(
                    "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; on this \
                     request body it is not used",
                )
                .emit(self.diags);
        }
        let ty = object
            .schema
            .as_ref()
            .and_then(|schema| self.lower_schema_ref(schema, "RequestBody"));
        if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a request body schema");
        }
        // A `multipart/form-data` body is emitted as a `reqwest::multipart::Form` whose parts are the
        // fields of an object schema. A concrete non-object type (or a multipart body with no schema
        // at all) has no fields to enumerate as parts, so it stays unsupported (`E009`, narrowed)
        // rather than silently degrade. A schema that *failed* to lower for its own reason (`ty` is
        // `None` though a schema was declared) has already emitted that diagnostic — don't pile a
        // misleading "must be an object" E009 on top of it.
        if media == MediaType::Multipart {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "a `multipart/form-data` request body must be an object schema; its \
                         properties are the form parts, so a non-object multipart body is not \
                         representable",
                    )
                    .remedy(
                        "give the multipart body an object schema with a property per form part, \
                         or omit this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets("RequestBody", ty, object.schema.is_some(), &body.provenance)
        } else {
            ty
        };
        if let Some(ty) = ty {
            let compatible = match media {
                MediaType::Text => raw_text_type_supported(&self.graph, ty),
                MediaType::OctetStream => matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Bytes)
                ),
                _ => true,
            };
            if !compatible {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "media type `{media_name}` requires a string-like or binary schema that can be sent as a raw body"
                    ))
                    .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
            // A raw request body — `bytes::Bytes` under any media, which the emitter sends
            // verbatim, or anything under the raw text codec — is the literal content of the
            // request, so a schema admitting `null` would ask the caller for an `Option` whose
            // `None` the wire cannot carry. An absent body is `required: false`, a different
            // construct, so the `null` is refused rather than reinterpreted as one.
            if ty.nullable && (media == MediaType::Text || self.is_bytes(ty)) {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "this `{media_name}` request body is sent as raw content, which has no \
                         wire representation of `null`, but its schema admits `null`"
                    ))
                    .remedy(
                        "remove `null` from the body schema (use `required: false` for a body \
                         that may be omitted), or omit this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        // A form-urlencoded body is rendered property by property, so it needs properties. Without
        // this gate a non-object body compiled and then failed at runtime inside the form encoder.
        if media == MediaType::FormUrlEncoded {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "an `application/x-www-form-urlencoded` request body must be an object \
                         schema; its properties are the form fields",
                    )
                    .remedy(
                        "give the body an object schema with a property per form field, or omit \
                         this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let encoding = self.lower_body_encoding(media, media_name, ty, &object)?;
        Some(RequestBody {
            media,
            content_type: media_essence(media_name).to_owned(),
            ty,
            required: body.required,
            encoding,
        })
    }

    /// Read an untyped body on a binary media type as raw octets.
    ///
    /// OpenAPI 3.1 aligned Schema Objects with JSON Schema 2020-12 and removed `format: binary`, so
    /// an empty (always-true) Schema Object — or no `schema` at all — is now how a document says
    /// *any octets*: the media type already carries the meaning, and `type: string` would be the
    /// 3.0 spelling the release deliberately retired. Both lower to `Any`, which on
    /// `application/octet-stream` would emit `serde_json::Value` for a byte stream, so the use site
    /// is retyped to `Bytes`.
    ///
    /// `declared_but_unlowerable` is a schema that was written and failed to lower for its own
    /// reason: it has already reported that, and must not be silently rewritten into bytes.
    ///
    /// `provenance` is the body's own, never the document root's: `Scope::alloc` disambiguates
    /// colliding name hints by pointer precisely so that reordering paths renames nothing, and a
    /// root pointer would collapse every `RequestBody`/`ResponseBody` here into arrival order.
    pub(super) fn opaque_octets(
        &mut self,
        hint: &str,
        ty: Option<Ty>,
        declared: bool,
        provenance: &crate::diag::Provenance,
    ) -> Option<Ty> {
        let Some(ty) = ty else {
            let declared_but_unlowerable = declared;
            return (!declared_but_unlowerable).then(|| {
                self.insert_type(
                    hint,
                    TypeKind::Bytes,
                    Docs::default(),
                    Some(provenance.clone()),
                )
            });
        };
        if !matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Any)
        ) {
            return Some(ty);
        }
        // An inline `{}` is the definition just inserted, and nothing can reference it yet, so it
        // is replaced in place — left behind it would emit a second `pub type … =
        // serde_json::Value` alias and take the name this body wants.
        //
        // Being the last definition is not enough to prove that, though: a *childless* component
        // (`Opaque: {}`) is lifted into its reserved id, which is then the last id as well, and
        // rewriting that would retype the component for every other reference in the document. A
        // named root is therefore left exactly as declared and the use site gets its own type.
        if self.graph.last_id() == Some(ty.id) && !self.is_component_root(ty.id) {
            let (_, definition) = self
                .pop_last_type()
                .expect("a definition was just observed");
            let id = self.graph.insert(TypeDef {
                kind: TypeKind::Bytes,
                ..definition
            });
            debug_assert_eq!(id, ty.id, "popping and reinserting reuses the dense id");
            return Some(Ty { id, ..ty });
        }
        Some(self.insert_type(
            hint,
            TypeKind::Bytes,
            Docs::default(),
            Some(provenance.clone()),
        ))
    }

    /// Whether `ty`'s definition is raw `bytes::Bytes`, which the emitter sends and decodes
    /// verbatim whatever the media.
    pub(super) fn is_bytes(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Bytes)
        )
    }
}

pub(super) fn raw_text_type_supported(graph: &TypeGraph, ty: Ty) -> bool {
    fn visit(graph: &TypeGraph, ty: Ty, seen: &mut HashSet<TypeId>) -> bool {
        if !seen.insert(ty.id) {
            return true;
        }
        let supported = match graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date))
            | Some(TypeKind::Bytes)
            | Some(TypeKind::Any) => true,
            Some(TypeKind::Enum(enumeration)) => enumeration.repr == ScalarRepr::String,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .all(|variant| visit(graph, variant.ty, seen)),
            // An unlowered body cannot be proved string-like, and this answers "is it proved":
            // no, so the raw text body is refused rather than admitted on the strength of nothing.
            Some(TypeKind::Reserved) => false,
            _ => false,
        };
        seen.remove(&ty.id);
        supported
    }

    visit(graph, ty, &mut HashSet::new())
}
