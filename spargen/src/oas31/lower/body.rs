//! Request Body Objects: choosing the `content` entry a request sends and typing its body.

use std::collections::{BTreeMap, HashSet};

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    AdditionalProps, BytesEncoding, Docs, MediaType, Operation, Prim, RequestBody, Ty, TypeDef,
    TypeGraph, TypeId, TypeKind,
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
    /// Choose the `content` entry a request sends ([`choose_media`]) and lower it, or `None` when
    /// no entry can be chosen or the chosen one is refused. The `W014`s for the entries passed
    /// over are emitted only once the chosen one has lowered. `hint` names an inline body schema
    /// after its operation ([`crate::name::request_body_hint`]).
    pub(super) fn lower_request_body(
        &mut self,
        body: &RequestBodyObject,
        hint: &str,
    ) -> Option<RequestBody> {
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
        let lowered = self.lower_chosen_request_body(body, media_name, object, hint)?;
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

    /// Lower the request body entry [`Self::lower_request_body`] selected, or `None` when it is
    /// refused: by the Media Type Object's or media key's own gates
    /// ([`Self::resolve_media_object`], [`lower_media_type`]), by one of the request-body gates
    /// here (each an `E009`: a media range, streaming media, a non-object form or multipart body,
    /// a raw body whose schema is not raw content or admits `null`), or by its encoding
    /// ([`Self::lower_body_encoding`]). An `itemSchema` on the entry is inert (`W010`). An entry
    /// with no `schema` takes any value its media carries ([`Self::schemaless_request_body`]),
    /// so every body this returns has a type.
    fn lower_chosen_request_body(
        &mut self,
        body: &RequestBodyObject,
        media_name: &str,
        object: &MediaTypeObject,
        hint: &str,
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
        //
        // A structured-suffix range such as `application/*+json` is a range for the same reason,
        // even though the suffix arms classify it as the codec its suffix names.
        if classify_media_range(media_essence(media_name)).is_some()
            || media_essence_is_suffix_range(media_essence(media_name))
        {
            // E009 case: request-range
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
            // E009 case: streaming-request
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
        let mark = self.graph_mark();
        let ty = object
            .schema
            .as_ref()
            .and_then(|schema| self.lower_schema_ref(schema, hint))
            .map(|ty| self.raw_body_bytes(ty, hint, &body.provenance, mark));
        if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a request body schema");
            // A declared schema that failed to lower has already reported why; the body is
            // refused with it rather than kept untyped, so no method silently sends nothing.
            ty?;
        }
        let ty = ty.or_else(|| self.schemaless_request_body(media, hint, &body.provenance));
        // A `multipart/form-data` body is emitted as a runtime `MultipartForm` whose parts are the
        // fields of an object schema. A concrete non-object type (or a multipart body with no schema
        // at all) has no fields to enumerate as parts, so it stays unsupported (`E009`, narrowed)
        // rather than silently degrade.
        if media == MediaType::Multipart && self.form_body_is_not_object(ty) {
            // E009 case: form-body-not-object
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
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets(hint, ty, object.schema.is_some(), &body.provenance)
        } else {
            ty
        };
        if let Some(ty) = ty {
            if !self.raw_body_compatible(media, ty) {
                // E009 case: raw-schema
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "media type `{media_name}` requires a {} schema that can be sent as a raw body",
                        raw_schema_wording(media)
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
                // E009 case: raw-null
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
        if media == MediaType::FormUrlEncoded && self.form_body_is_not_object(ty) {
            // E009 case: form-body-not-object
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
    /// 3.0 spelling the release deliberately retired. An empty schema lowers to `Any`, which on
    /// `application/octet-stream` would emit `serde_json::Value` for a byte stream, and no schema
    /// reaches here as no type at all (`ty` is `None`); either way the use site is typed `Bytes`.
    /// A `ty` that is anything but `Any` is returned unchanged.
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
                    TypeKind::Bytes(BytesEncoding::Raw),
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
                kind: TypeKind::Bytes(BytesEncoding::Raw),
                ..definition
            });
            debug_assert_eq!(id, ty.id, "popping and reinserting reuses the dense id");
            return Some(Ty { id, ..ty });
        }
        Some(self.insert_type(
            hint,
            TypeKind::Bytes(BytesEncoding::Raw),
            Docs::default(),
            Some(provenance.clone()),
        ))
    }

    /// A whole non-streaming body's type, with a `contentEncoding: base64` (or `base64url`)
    /// definition taken as raw
    /// bytes: the emitter sends and decodes a whole `bytes` body verbatim whatever the media, so the
    /// body is `bytes::Bytes` itself rather than the base64 newtype a serde-encoded position takes
    /// (#1230). `check_invariants` holds every non-streaming body to that.
    ///
    /// An inline definition only this body holds is retyped in place, keeping its name; a
    /// component (or any definition lowered before `mark`) keeps its base64 type for its other
    /// uses, and the body takes a raw definition of its own, eliding what this lowering inserted
    /// and nothing else reaches.
    pub(super) fn raw_body_bytes(
        &mut self,
        ty: Ty,
        hint: &str,
        provenance: &crate::diag::Provenance,
        mark: u32,
    ) -> Ty {
        if !matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Bytes(
                BytesEncoding::Base64 | BytesEncoding::Base64Url
            ))
        ) {
            return ty;
        }
        if ty.id.0 >= mark && self.graph.last_id() == Some(ty.id) && !self.is_component_root(ty.id)
        {
            let (_, definition) = self
                .pop_last_type()
                .expect("a definition was just observed");
            let id = self.graph.insert(TypeDef {
                kind: TypeKind::Bytes(BytesEncoding::Raw),
                ..definition
            });
            debug_assert_eq!(id, ty.id, "popping and reinserting reuses the dense id");
            return Ty { id, ..ty };
        }
        self.elide_unused_lowering(mark..self.graph_mark());
        let raw = self.insert_type(
            hint,
            TypeKind::Bytes(BytesEncoding::Raw),
            Docs::default(),
            Some(provenance.clone()),
        );
        Ty {
            nullable: ty.nullable,
            ..raw
        }
    }

    /// Whether `ty`'s definition is raw `bytes::Bytes`, which the emitter sends and decodes
    /// verbatim whatever the media.
    pub(super) fn is_bytes(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Bytes(_))
        )
    }

    /// Whether a lowered body type can travel as `media`'s raw content: under raw text it must be
    /// proved a scalar ([`raw_text_type_supported`]), under `application/octet-stream` it must
    /// be [`TypeKind::Bytes`], and under every other media any type is accepted here. Shared by the
    /// request and response body gates, which refuse with their own `E009` wording.
    pub(super) fn raw_body_compatible(&self, media: MediaType, ty: Ty) -> bool {
        match media {
            MediaType::Text => raw_text_type_supported(&self.graph, ty),
            MediaType::OctetStream => self.is_bytes(ty),
            _ => true,
        }
    }

    /// Whether a `multipart/form-data` or form-urlencoded request body lacks the object schema
    /// whose properties are its parts or fields. `ty` is the lowered body type, `None` only for a
    /// form body declared with no schema: a declared schema that failed to lower has already
    /// refused the body, so no second `E009` is stacked on it.
    fn form_body_is_not_object(&self, ty: Option<Ty>) -> bool {
        !matches!(
            ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
            Some(TypeKind::Struct(_))
        )
    }

    /// The type a request body declared with no `schema` is sent as: any value its media carries.
    /// A JSON or XML body takes any value (`serde_json::Value`, as `{}` does there) and a raw text
    /// body any text (`String`). `None` for the rest: an octet-stream body is typed any octets
    /// (`bytes::Bytes`) by [`Self::opaque_octets`], a form or multipart body has no properties to
    /// send and is refused by its own `E009`, and a streaming request body never reaches here.
    fn schemaless_request_body(
        &mut self,
        media: MediaType,
        hint: &str,
        provenance: &crate::diag::Provenance,
    ) -> Option<Ty> {
        let kind = match media {
            MediaType::Json | MediaType::Xml => TypeKind::Any,
            MediaType::Text => TypeKind::Primitive(Prim::String),
            MediaType::OctetStream
            | MediaType::FormUrlEncoded
            | MediaType::Multipart
            | MediaType::EventStream
            | MediaType::Ndjson
            | MediaType::JsonSequence => return None,
        };
        Some(self.insert_type(hint, kind, Docs::default(), Some(provenance.clone())))
    }

    /// Give every raw `bytes` value a serde codec carries the `String` its wire holds, and report
    /// each as `W001` (#1417).
    ///
    /// `format: binary` is an annotation in OpenAPI 3.1 and 3.2, with no effect on how a value is
    /// encoded (3.2.0.md:1646), so inside a JSON, XML or form body, or a sequential response's
    /// items, the value is a JSON string: `bytes::Bytes`' own serde form, an array of numbers,
    /// would send and expect what no server writes. The bytes are the wire itself in two places
    /// only, which keep raw `bytes::Bytes`:
    ///
    /// - a whole non-streaming body, under any media, which the emitter sends and reads verbatim;
    /// - a part of a `multipart/form-data` body: a field, or the typed `additionalProperties`
    ///   value, of a struct that is a multipart request body and that nothing else carries — no
    ///   other definition's member, and no other body, response or parameter.
    ///
    /// A multipart body struct that a serde codec also carries is one Rust type for both, and its
    /// members take the serde answer: the part is then sent as text.
    ///
    /// A definition no raw position holds is retyped in place, keeping its name; one that a raw
    /// position holds too keeps its bytes, and the serde positions take a `String` copy of it.
    /// Run once every operation is lowered and the unreachable definitions are elided, so every
    /// use of every definition is known.
    pub(super) fn retype_serde_binary(&mut self, operations: &mut [Operation]) {
        let raw = |graph: &TypeGraph, id: TypeId| {
            matches!(
                graph.get(id).map(|definition| &definition.kind),
                Some(TypeKind::Bytes(BytesEncoding::Raw))
            )
        };
        let mut raw_uses: HashSet<TypeId> = HashSet::new();
        let members: HashSet<TypeId> = self
            .graph
            .emitted()
            .flat_map(|(_, definition)| super::prune::kind_edges(&definition.kind))
            .collect();
        let mut carried_elsewhere: HashSet<TypeId> = HashSet::new();
        let mut multipart_bodies: Vec<TypeId> = Vec::new();
        for operation in operations.iter() {
            carried_elsewhere.extend(operation.params.iter().map(|parameter| parameter.ty.id));
            if let Some((body, ty)) = operation.typed_body() {
                if body.media == MediaType::Multipart {
                    multipart_bodies.push(ty.id);
                } else {
                    carried_elsewhere.insert(ty.id);
                    if raw(&self.graph, ty.id) {
                        raw_uses.insert(ty.id);
                    }
                }
            }
            for (_, response) in operation.responses.entries() {
                carried_elsewhere.extend(response.headers.iter().map(|header| header.ty.id));
                if let Some(ty) = response.body {
                    carried_elsewhere.insert(ty.id);
                    if response.stream.is_none() && raw(&self.graph, ty.id) {
                        raw_uses.insert(ty.id);
                    }
                }
            }
        }
        let part_holders: HashSet<TypeId> = multipart_bodies
            .into_iter()
            .filter(|id| !members.contains(id) && !carried_elsewhere.contains(id))
            .collect();

        // Every serde position holding raw bytes, grouped by the definition it holds.
        let mut serde_slots: BTreeMap<TypeId, Vec<SerdeSlot>> = BTreeMap::new();
        let mut hold = |target: TypeId, slot: SerdeSlot| {
            serde_slots.entry(target).or_default().push(slot);
        };
        for (owner, definition) in self.graph.emitted() {
            match &definition.kind {
                TypeKind::Struct(object) => {
                    let parts = part_holders.contains(&owner);
                    for (index, field) in object.fields.iter().enumerate() {
                        if !raw(&self.graph, field.ty.id) {
                        } else if parts {
                            raw_uses.insert(field.ty.id);
                        } else {
                            hold(field.ty.id, SerdeSlot::Field { owner, index });
                        }
                    }
                    if let AdditionalProps::Typed(value) = &object.additional {
                        if !raw(&self.graph, value.id) {
                        } else if parts {
                            raw_uses.insert(value.id);
                        } else {
                            hold(value.id, SerdeSlot::Additional { owner });
                        }
                    }
                }
                TypeKind::Array(item) if raw(&self.graph, item.id) => {
                    hold(item.id, SerdeSlot::Item { owner });
                }
                TypeKind::Tuple(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if raw(&self.graph, item.id) {
                            hold(item.id, SerdeSlot::Position { owner, index });
                        }
                    }
                }
                TypeKind::Union(union) => {
                    for (index, variant) in union.variants.iter().enumerate() {
                        if raw(&self.graph, variant.ty.id) {
                            hold(variant.ty.id, SerdeSlot::Variant { owner, index });
                        }
                    }
                }
                // A reservation is filled before any operation is lowered, so none is left here;
                // the rest hold no member.
                TypeKind::Reserved
                | TypeKind::Array(_)
                | TypeKind::Primitive(_)
                | TypeKind::Enum(_)
                | TypeKind::Bytes(_)
                | TypeKind::Null
                | TypeKind::Never
                | TypeKind::Any => {}
            }
        }
        for (operation, entry) in operations.iter().enumerate() {
            let statuses = entry
                .responses
                .by_status
                .iter()
                .map(|(_, response)| response)
                .enumerate()
                .map(|(index, response)| (Some(index), response));
            let default = entry
                .responses
                .default
                .iter()
                .map(|response| (None, response));
            for (status, response) in statuses.chain(default) {
                if let (Some(_), Some(ty)) = (response.stream, response.body) {
                    if raw(&self.graph, ty.id) {
                        hold(ty.id, SerdeSlot::StreamItem { operation, status });
                    }
                }
            }
        }

        for (target, slots) in serde_slots {
            let definition = self
                .graph
                .get(target)
                .expect("a held definition is in the graph");
            let provenance = definition.provenance.clone();
            let replacement = if raw_uses.contains(&target) {
                let (hint, docs) = (definition.name_hint.clone(), definition.docs.clone());
                self.insert_type(
                    &hint,
                    TypeKind::Primitive(Prim::String),
                    docs,
                    Some(provenance.clone()),
                )
                .id
            } else {
                self.graph
                    .get_mut(target)
                    .expect("a held definition is in the graph")
                    .kind = TypeKind::Primitive(Prim::String);
                target
            };
            for slot in slots {
                let ty = match slot {
                    SerdeSlot::StreamItem { operation, status } => {
                        let responses = &mut operations[operation].responses;
                        let response = match status {
                            Some(index) => &mut responses.by_status[index].1,
                            None => responses
                                .default
                                .as_mut()
                                .expect("a held default response exists"),
                        };
                        response.body.as_mut()
                    }
                    SerdeSlot::Field { owner, index } => match self.kind_mut(owner) {
                        Some(TypeKind::Struct(object)) => Some(&mut object.fields[index].ty),
                        Some(TypeKind::Reserved) | None => None,
                        _ => None,
                    },
                    SerdeSlot::Additional { owner } => match self.kind_mut(owner) {
                        Some(TypeKind::Struct(object)) => match &mut object.additional {
                            AdditionalProps::Typed(value) => Some(&mut **value),
                            AdditionalProps::Allow | AdditionalProps::Deny => None,
                        },
                        Some(TypeKind::Reserved) | None => None,
                        _ => None,
                    },
                    SerdeSlot::Item { owner } => match self.kind_mut(owner) {
                        Some(TypeKind::Array(item)) => Some(&mut **item),
                        Some(TypeKind::Reserved) | None => None,
                        _ => None,
                    },
                    SerdeSlot::Position { owner, index } => match self.kind_mut(owner) {
                        Some(TypeKind::Tuple(items)) => Some(&mut items[index]),
                        Some(TypeKind::Reserved) | None => None,
                        _ => None,
                    },
                    SerdeSlot::Variant { owner, index } => match self.kind_mut(owner) {
                        Some(TypeKind::Union(union)) => Some(&mut union.variants[index].ty),
                        Some(TypeKind::Reserved) | None => None,
                        _ => None,
                    },
                };
                ty.expect("a held slot keeps the shape it was found in").id = replacement;
            }
            // W001 case: binary-format
            Diagnostic::warning(Code::ValidationKeywordIgnored, provenance)
                .message(
                    "`format: binary` is an annotation in OpenAPI 3.1 and 3.2, with no effect on \
                     how a value is encoded, so where a JSON, XML or form body or a stream item \
                     carries this value it is the `String` the wire holds, not raw bytes",
                )
                .remedy(
                    "spell binary content inside a structured body as `contentEncoding: base64` \
                     or `base64url`, which generate typed bytes",
                )
                .emit(self.diags);
        }
    }

    /// The kind of the definition `id` names, for rewriting a member in place.
    fn kind_mut(&mut self, id: TypeId) -> Option<&mut TypeKind> {
        self.graph
            .get_mut(id)
            .map(|definition| &mut definition.kind)
    }
}

/// A position in which a serde codec carries a value: a member of the definition `owner`, or the
/// item type of the sequential response `status` (`None` for `default`) of operation `operation`.
#[derive(Debug, Clone, Copy)]
enum SerdeSlot {
    Field {
        owner: TypeId,
        index: usize,
    },
    Additional {
        owner: TypeId,
    },
    Item {
        owner: TypeId,
    },
    Position {
        owner: TypeId,
        index: usize,
    },
    Variant {
        owner: TypeId,
        index: usize,
    },
    StreamItem {
        operation: usize,
        status: Option<usize>,
    },
}

/// The schemas `media`'s raw codec carries, as the `E009` raw-schema message names them.
pub(super) fn raw_schema_wording(media: MediaType) -> &'static str {
    match media {
        MediaType::Text => "scalar (string, number, integer or boolean) or binary",
        _ => "string-like or binary",
    }
}

/// Whether `ty` is proved to be a scalar the raw text codec carries both ways: the runtime renders
/// a request body through `serialize_simple` and reads a response through `decode_text_body`, so
/// a string, number, integer or boolean (a primitive, an enum, or a union of them), raw bytes, and
/// the untyped `{}` schema travel as their bare text. An object, array or map has no text form.
fn raw_text_type_supported(graph: &TypeGraph, ty: Ty) -> bool {
    fn visit(graph: &TypeGraph, ty: Ty, seen: &mut HashSet<TypeId>) -> bool {
        if !seen.insert(ty.id) {
            return true;
        }
        let supported = match graph.get(ty.id).map(|definition| &definition.kind) {
            // Every primitive and every enum's value set is a JSON scalar.
            Some(TypeKind::Primitive(_))
            | Some(TypeKind::Enum(_))
            | Some(TypeKind::Bytes(_))
            | Some(TypeKind::Any) => true,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .all(|variant| visit(graph, variant.ty, seen)),
            // An unlowered body cannot be proved a scalar, and this answers "is it proved":
            // no, so the raw text body is refused rather than admitted on the strength of nothing.
            Some(TypeKind::Reserved) => false,
            _ => false,
        };
        seen.remove(&ty.id);
        supported
    }

    visit(graph, ty, &mut HashSet::new())
}
