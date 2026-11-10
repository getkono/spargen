//! # Subsystem: ir
//! layer-deps: diag
//!
//! The version-agnostic API model: operation set, type graph, auth requirements, media map;
//! provenance (pointer + span) on every node; well-formedness invariants. The IR is the coupling
//! firewall and primary extension seam — it never sees a spec document or Rust tokens. One
//! frontend, `oas31`, lowers both OpenAPI 3.1 and 3.2 into it, and a frontend for another spec
//! version would lower into it too, touching nothing downstream.

mod auth;
mod graph;
mod invariant;
mod media;
mod operation;
mod responses;
mod types;

use std::collections::BTreeSet;

use indexmap::IndexMap;

pub(crate) use auth::{
    ApiKeyLoc, HttpScheme, SchemeId, SecurityRequirement, SecurityScheme, SecuritySchemeDef,
};
pub(crate) use graph::{Emission, TypeGraph};
pub(crate) use invariant::check_invariants;
pub(crate) use media::{
    BodyEncoding, EncodingMode, Framing, MediaType, PropertyEncoding, RequestBody,
};
pub(crate) use operation::{
    BoundHeader, Delimiter, Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter,
    PathSegment, PathTemplate,
};
pub(crate) use responses::{
    ApiErrorBodyImpl, ErrorShape, HeaderShape, Response, ResponseHeader, Responses, StatusSpec,
    SuccessShape,
};
pub(crate) use types::{
    AdditionalProps, BytesEncoding, DefaultValue, DisjointFeature, Field, FieldDefault,
    JsonCategory, Openness, Prim, PropertyName, ScalarEnum, ScalarRepr, ScalarValue, Struct, Ty,
    TypeDef, TypeId, TypeKind, Union, UnionMode, UnionStrategy, UnionVariant, VariantDispatch,
    XmlField,
};

/// The whole lowered API: the single artifact frontends produce and backends consume.
#[derive(Debug, Clone)]
pub(crate) struct Api {
    /// API identity (`info`).
    pub(crate) info: Info,
    /// Servers, with variable-substitution metadata retained.
    pub(crate) servers: Vec<Server>,
    /// Every operation, in deterministic order.
    pub(crate) operations: Vec<Operation>,
    /// The type graph referenced by operations and each other.
    pub(crate) types: TypeGraph,
    /// Named security schemes (`components.securitySchemes`).
    pub(crate) security_schemes: IndexMap<SchemeId, SecuritySchemeDef>,
}

/// The conditionally embedded runtime modules a generated module carries, decided once by
/// [`Api::runtime_features`] for both the embed (`codegen`) and the dependencies it implies
/// (`runtime_contract`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeFeatures {
    /// The XML codec: [`Api::uses_xml`].
    pub(crate) xml: bool,
    /// The sequential-response `stream` module: [`Api::uses_streams`].
    pub(crate) streams: bool,
    /// The RFC 3339 `Date`/`DateTime` newtypes: [`Api::uses_time`], with the `time` mapping on.
    pub(crate) time: bool,
    /// The `contentEncoding: base64` codec: [`Api::uses_base64`].
    pub(crate) base64: bool,
}

impl Api {
    /// The conditional runtime modules the module generated from this API embeds, with the `time`
    /// mapping on or off as `time_mapping` says. With it off, the date primitives stay `String` and
    /// the newtypes would be dead weight.
    pub(crate) fn runtime_features(&self, time_mapping: bool) -> RuntimeFeatures {
        RuntimeFeatures {
            xml: self.uses_xml(),
            streams: self.uses_streams(),
            time: time_mapping && self.uses_time(),
            base64: self.uses_base64(),
        }
    }

    /// Whether the types the output carries contain a `contentEncoding: base64` definition, each
    /// emitted as a newtype whose serde form the embedded `base64` runtime module writes and reads.
    /// An [elided](TypeGraph::elide) definition is no type of the output, so it counts for nothing.
    pub(crate) fn uses_base64(&self) -> bool {
        self.types.emitted().any(|(_, definition)| {
            matches!(definition.kind, TypeKind::Bytes(BytesEncoding::Base64))
        })
    }

    /// Whether any operation uses an `application/xml` / `text/xml` request or response body. Drives
    /// the `quick-xml` row of the consumer requirements table (`runtime_contract`) and the
    /// conditional embedding of the XML runtime helpers — both deterministic functions of the API.
    pub(crate) fn uses_xml(&self) -> bool {
        self.operations.iter().any(|operation| {
            let request_xml = operation
                .request_body
                .as_ref()
                .is_some_and(|body| body.media == MediaType::Xml);
            let response_xml = operation
                .responses
                .entries()
                .any(|(_, response)| response.media == Some(MediaType::Xml));
            request_xml || response_xml
        })
    }

    /// Whether the types the output carries contain a `format: date-time` or `format: date`
    /// primitive. Drives the conditional embedding of the RFC 3339 `DateTime`/`Date` runtime
    /// newtypes and the `time` requirement. An [elided](TypeGraph::elide) definition is no type of
    /// the output, so it counts for nothing here, as in `check_invariants` (#723).
    ///
    /// This is a property of the API alone; whether those primitives actually *become* the newtypes
    /// additionally depends on the `time` config knob, which callers apply themselves.
    pub(crate) fn uses_time(&self) -> bool {
        self.types.emitted().any(|(_, definition)| {
            matches!(
                definition.kind,
                TypeKind::Primitive(Prim::Date | Prim::DateTime)
            )
        })
    }

    /// Whether `operation`'s success is one raw `bytes` body read whole: the body its method reads
    /// within a default limit, and its `_stream` twin returns as a `ByteStream`.
    pub(crate) fn streams_binary_success(&self, operation: &Operation) -> bool {
        operation.responses.stream_success().is_none()
            && matches!(
                operation.responses.success(),
                SuccessShape::Plain(ty) if self.is_raw_bytes(ty)
            )
    }

    /// Whether `operation`'s typed request body is raw `bytes`, which its method takes as anything
    /// that converts into the runtime's `HttpBody`, in memory or streamed.
    pub(crate) fn sends_binary_body(&self, operation: &Operation) -> bool {
        operation
            .typed_body()
            .is_some_and(|(_, ty)| self.is_raw_bytes(ty))
    }

    /// Whether `ty` is a non-nullable raw `bytes` value (`bytes::Bytes` itself).
    fn is_raw_bytes(&self, ty: Ty) -> bool {
        !ty.nullable
            && matches!(
                self.types.get(ty.id).map(|definition| &definition.kind),
                Some(TypeKind::Bytes(BytesEncoding::Raw))
            )
    }

    /// Whether any operation returns a sequential response as a typed stream. Drives conditional
    /// stream-runtime embedding and the `futures-core` / reqwest `stream` requirements.
    pub(crate) fn uses_streams(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.responses.stream_success().is_some())
    }

    /// Whether generated code serializes or deserializes a `bytes` value through `bytes`' own serde
    /// impls: a model field, additional-properties value or union variant containing one, a JSON
    /// `content` parameter containing one, or a serde-encoded body (JSON, form, XML or a
    /// sequential stream) containing one other than a whole non-streaming `bytes` body, which is
    /// sent and read raw. Only raw bytes count: a `contentEncoding: base64` value is a newtype
    /// whose serde impls are the embedded base64 codec's ([`Api::uses_base64`]).
    /// Drives the `serde` feature of the `bytes` requirement. Only the models the output carries
    /// count: an [elided](TypeGraph::elide) definition is none of them (#723).
    pub(crate) fn uses_bytes_serde(&self) -> bool {
        let model_needs_serde =
            self.types
                .emitted()
                .any(|(id, definition)| match &definition.kind {
                    // A model whose fields, additional-properties value or variants reach `bytes`:
                    // exactly what `contains_bytes` walks from the model itself.
                    TypeKind::Struct(_) | TypeKind::Union(_) => {
                        contains_bytes(&self.types, id, &mut BTreeSet::new())
                    }
                    // Unreachable on a checked `Api`; see `TypeKind::Reserved`. Answering `false`
                    // would under-declare a runtime dependency for a shape nobody computed.
                    TypeKind::Reserved => unreachable!(
                        "a reservation reached the runtime contract; `check_invariants` should \
                     have rejected it"
                    ),
                    _ => false,
                });
        model_needs_serde
            || self.operations.iter().any(|operation| {
                operation.params.iter().any(|parameter| {
                    matches!(&parameter.style, ParamStyle::Content(MediaType::Json))
                        && contains_bytes(&self.types, parameter.ty.id, &mut BTreeSet::new())
                }) || operation
                    .typed_body()
                    .is_some_and(|(body, ty)| self.typed_body_needs_bytes_serde(body.media, ty.id))
                    || operation.responses.entries().any(|(_, response)| {
                        response.body.is_some_and(|ty| {
                            response.media.is_some_and(|media| {
                                self.typed_body_needs_bytes_serde(media, ty.id)
                            })
                        })
                    })
            })
    }

    fn typed_body_needs_bytes_serde(&self, media: MediaType, id: TypeId) -> bool {
        serde_body_media(media)
            && (media.stream_framing().is_some()
                || !matches!(
                    self.types.get(id).map(|definition| &definition.kind),
                    Some(TypeKind::Bytes(_))
                ))
            && contains_bytes(&self.types, id, &mut BTreeSet::new())
    }
}

fn serde_body_media(media: MediaType) -> bool {
    matches!(
        media,
        MediaType::Json
            | MediaType::FormUrlEncoded
            | MediaType::Xml
            | MediaType::EventStream
            | MediaType::Ndjson
            | MediaType::JsonSequence
    )
}

fn contains_bytes(types: &TypeGraph, id: TypeId, visiting: &mut BTreeSet<TypeId>) -> bool {
    if !visiting.insert(id) {
        return false;
    }
    let contains = match types.get(id).map(|definition| &definition.kind) {
        Some(TypeKind::Bytes(BytesEncoding::Raw)) => true,
        // A base64 newtype's serde impls call the embedded codec over the bytes, never `bytes`'
        // own (#1230).
        Some(TypeKind::Bytes(BytesEncoding::Base64)) => false,
        Some(TypeKind::Struct(object)) => {
            object
                .fields
                .iter()
                .any(|field| contains_bytes(types, field.ty.id, visiting))
                || match &object.additional {
                    AdditionalProps::Typed(ty) => contains_bytes(types, ty.id, visiting),
                    AdditionalProps::Allow | AdditionalProps::Deny => false,
                }
        }
        Some(TypeKind::Array(item)) => contains_bytes(types, item.id, visiting),
        Some(TypeKind::Tuple(items)) => items
            .iter()
            .any(|item| contains_bytes(types, item.id, visiting)),
        Some(TypeKind::Union(union)) => union
            .variants
            .iter()
            .any(|variant| contains_bytes(types, variant.ty.id, visiting)),
        // Unreachable on a checked `Api`; see `TypeKind::Reserved`.
        Some(TypeKind::Reserved) => unreachable!(
            "a reservation reached the runtime contract; `check_invariants` should have rejected it"
        ),
        _ => false,
    };
    visiting.remove(&id);
    contains
}

/// API identity, lowered from `info`.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    /// `info.title`.
    pub(crate) title: String,
    /// `info.version`.
    pub(crate) version: String,
    /// `info.description`, if present.
    pub(crate) description: Option<String>,
}

/// A server entry (matrix: Document).
#[derive(Debug, Clone)]
pub(crate) struct Server {
    /// OpenAPI 3.2 `name`: a stable identity for this host, used to name the generated builder.
    pub(crate) name: Option<String>,
    /// The raw, possibly templated server URL.
    pub(crate) url: String,
    /// The URL template split into literals and variable references, parsed once here so codegen
    /// and rendering never re-scan the string.
    pub(crate) segments: Vec<UrlSegment>,
    /// Declared variables, in source order.
    pub(crate) variables: IndexMap<String, ServerVariable>,
    /// `server.description`.
    pub(crate) description: Option<String>,
}

/// One piece of a parsed server URL template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UrlSegment {
    /// Literal text, emitted verbatim.
    Literal(String),
    /// A `{name}` reference to a declared server variable.
    Variable(String),
}

/// A server variable: a closed or open set of substitutions with a default that is actually sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerVariable {
    /// The value used when the caller supplies none.
    pub(crate) default: String,
    /// The permitted values, when the document declares a closed set. Empty means free-form.
    pub(crate) enum_values: Vec<String>,
    /// `description`, surfaced as rustdoc on the generated setter.
    pub(crate) description: Option<String>,
}

/// Documentation carried from a construct's `title`/`summary`/`description`/`deprecated`, lowered
/// to rustdoc so IDE hover shows API docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Docs {
    /// `title`.
    pub(crate) title: Option<String>,
    /// `summary`.
    pub(crate) summary: Option<String>,
    /// `description`.
    pub(crate) description: Option<String>,
    /// Whether the construct is `deprecated` (also drives `#[deprecated]`).
    pub(crate) deprecated: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        AdditionalProps, Api, BodyEncoding, BoundHeader, Field, Info, MediaType, Method, Operation,
        OperationId, ParamLoc, ParamStyle, Parameter, PathTemplate, Prim, PropertyName,
        RequestBody, Response, Responses, StatusSpec, Struct, Ty, TypeDef, TypeGraph, TypeKind,
        Union, UnionMode, UnionStrategy, UnionVariant, VariantDispatch,
    };
    use crate::diag::{JsonPointer, Provenance};

    /// One row: what it adds to [`api`]'s empty operation, and the predicate's answer.
    type Row = (&'static str, fn(&mut Api), bool);

    /// An API with one bodyless, parameterless `GET /items` and no types, which each row builds on.
    fn api() -> Api {
        Api {
            info: Info {
                title: "T".to_owned(),
                version: "1.0.0".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: vec![Operation {
                id: OperationId("listItems".to_owned()),
                method: Method::Get,
                path: PathTemplate {
                    raw: "/items".to_owned(),
                    segments: Vec::new(),
                },
                params: Vec::new(),
                request_body: None,
                responses: Responses {
                    by_status: Vec::new(),
                    default: None,
                },
                security: Vec::new(),
                deprecated: false,
                docs: Default::default(),
                server: None,
                provenance: Provenance::new(JsonPointer::root(), None),
                bound_headers: Vec::new(),
            }],
            types: TypeGraph::default(),
            security_schemes: Default::default(),
        }
    }

    fn insert(api: &mut Api, kind: TypeKind) -> Ty {
        let id = api.types.insert(TypeDef {
            name_hint: "T".to_owned(),
            kind,
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        Ty {
            id,
            nullable: false,
            boxed: false,
        }
    }

    fn bytes(api: &mut Api) -> Ty {
        insert(api, TypeKind::Bytes(super::BytesEncoding::Raw))
    }

    fn string(api: &mut Api) -> Ty {
        insert(api, TypeKind::Primitive(Prim::String))
    }

    fn object(api: &mut Api, field: Ty, additional: AdditionalProps) -> Ty {
        let field = Field {
            name: PropertyName {
                wire: "field".to_owned(),
            },
            ty: field,
            required: true,
            deprecated: false,
            read_only: false,
            write_only: false,
            default: None,
            xml: Default::default(),
            undeclared: false,
        };
        insert(
            api,
            TypeKind::Struct(Struct {
                fields: vec![field],
                additional,
            }),
        )
    }

    fn request(api: &mut Api, media: MediaType, ty: Ty) {
        api.operations[0].request_body = Some(RequestBody {
            media,
            content_type: String::new(),
            ty: Some(ty),
            required: true,
            encoding: BodyEncoding::default(),
        });
    }

    /// A response under `status` (`None` is `default`), streamed when `media` frames a stream.
    fn respond(api: &mut Api, status: Option<StatusSpec>, media: MediaType, ty: Ty) {
        let response = Response {
            body: Some(ty),
            media: Some(media),
            stream: media.stream_framing(),
            accept: None,
            headers: Vec::new(),
        };
        let responses = &mut api.operations[0].responses;
        match status {
            Some(status) => responses.by_status.push((status, response)),
            None => responses.default = Some(response),
        }
    }

    fn check(rows: &[Row], predicate: fn(&Api) -> bool) {
        for (case, build, expected) in rows {
            let mut api = api();
            build(&mut api);
            assert_eq!(predicate(&api), *expected, "{case}");
        }
    }

    const OK: Option<StatusSpec> = Some(StatusSpec::Exact(200));

    #[test]
    fn uses_xml_follows_the_table() {
        check(
            &[
                ("no body", |_| {}, false),
                (
                    "a JSON request and response",
                    |api| {
                        let ty = string(api);
                        request(api, MediaType::Json, ty);
                        respond(api, OK, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "an XML request body",
                    |api| {
                        let ty = string(api);
                        request(api, MediaType::Xml, ty);
                    },
                    true,
                ),
                (
                    "an XML success body",
                    |api| {
                        let ty = string(api);
                        respond(api, OK, MediaType::Xml, ty);
                    },
                    true,
                ),
                (
                    "an XML error body",
                    |api| {
                        let ty = string(api);
                        respond(api, Some(StatusSpec::Range(4)), MediaType::Xml, ty);
                    },
                    true,
                ),
                (
                    "an XML default response",
                    |api| {
                        let ty = string(api);
                        respond(api, None, MediaType::Xml, ty);
                    },
                    true,
                ),
            ],
            Api::uses_xml,
        );
    }

    #[test]
    fn uses_time_follows_the_table() {
        check(
            &[
                ("no type", |_| {}, false),
                (
                    "a string and a uuid",
                    |api| {
                        string(api);
                        insert(api, TypeKind::Primitive(Prim::Uuid));
                    },
                    false,
                ),
                (
                    "a date-time",
                    |api| {
                        insert(api, TypeKind::Primitive(Prim::DateTime));
                    },
                    true,
                ),
                (
                    "a date, referenced by nothing",
                    |api| {
                        insert(api, TypeKind::Primitive(Prim::Date));
                    },
                    true,
                ),
                (
                    "a date only in an elided definition",
                    |api| {
                        let date = insert(api, TypeKind::Primitive(Prim::Date));
                        api.types.elide(date.id);
                    },
                    false,
                ),
            ],
            Api::uses_time,
        );
    }

    /// `runtime_features` embeds the date newtypes only when the API uses a date primitive *and*
    /// the `time` mapping is on, and takes the other two modules from their predicates whatever
    /// the mapping says.
    #[test]
    fn runtime_features_gate_time_on_the_mapping_and_the_api_alike() {
        let plain = api();
        let mut dated = api();
        insert(&mut dated, TypeKind::Primitive(Prim::DateTime));
        for (case, api, mapping, time) in [
            ("no date, mapping off", &plain, false, false),
            ("no date, mapping on", &plain, true, false),
            ("a date, mapping off", &dated, false, false),
            ("a date, mapping on", &dated, true, true),
        ] {
            let features = api.runtime_features(mapping);
            assert_eq!(features.time, time, "{case}");
            assert!(!features.xml && !features.streams, "{case}");
        }
    }

    #[test]
    fn uses_streams_follows_the_table() {
        check(
            &[
                (
                    "a JSON success",
                    |api| {
                        let ty = string(api);
                        respond(api, OK, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "an event-stream success",
                    |api| {
                        let ty = string(api);
                        respond(api, OK, MediaType::EventStream, ty);
                    },
                    true,
                ),
                (
                    "an NDJSON success under a range",
                    |api| {
                        let ty = string(api);
                        respond(api, Some(StatusSpec::Range(2)), MediaType::Ndjson, ty);
                    },
                    true,
                ),
                (
                    "a stream beside a second bodied success",
                    |api| {
                        let ty = string(api);
                        respond(api, OK, MediaType::EventStream, ty);
                        respond(api, Some(StatusSpec::Exact(201)), MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "a stream on the error side",
                    |api| {
                        let ty = string(api);
                        respond(
                            api,
                            Some(StatusSpec::Exact(500)),
                            MediaType::EventStream,
                            ty,
                        );
                    },
                    false,
                ),
            ],
            Api::uses_streams,
        );
    }

    /// Issue #1263: only a single raw `bytes` success body read whole is streamed by a twin.
    #[test]
    fn streams_binary_success_follows_the_table() {
        check(
            &[
                ("no body", |_| {}, false),
                (
                    "an octet-stream success",
                    |api| {
                        let ty = bytes(api);
                        respond(api, OK, MediaType::OctetStream, ty);
                    },
                    true,
                ),
                (
                    "a raw bytes success under a range",
                    |api| {
                        let ty = bytes(api);
                        respond(api, Some(StatusSpec::Range(2)), MediaType::OctetStream, ty);
                    },
                    true,
                ),
                (
                    "a base64 bytes success",
                    |api| {
                        let ty = insert(api, TypeKind::Bytes(super::BytesEncoding::Base64));
                        respond(api, OK, MediaType::OctetStream, ty);
                    },
                    false,
                ),
                (
                    "a JSON string success",
                    |api| {
                        let ty = string(api);
                        respond(api, OK, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "a bytes success beside a second bodied success",
                    |api| {
                        let ty = bytes(api);
                        respond(api, OK, MediaType::OctetStream, ty);
                        respond(
                            api,
                            Some(StatusSpec::Exact(206)),
                            MediaType::OctetStream,
                            ty,
                        );
                    },
                    false,
                ),
                (
                    "a bytes body on the error side only",
                    |api| {
                        let ty = bytes(api);
                        respond(
                            api,
                            Some(StatusSpec::Exact(500)),
                            MediaType::OctetStream,
                            ty,
                        );
                    },
                    false,
                ),
            ],
            |api| api.streams_binary_success(&api.operations[0]),
        );
    }

    /// Issue #1263: only a raw `bytes` request body is taken as an `HttpBody`.
    #[test]
    fn sends_binary_body_follows_the_table() {
        check(
            &[
                ("no body", |_| {}, false),
                (
                    "an octet-stream request body",
                    |api| {
                        let ty = bytes(api);
                        request(api, MediaType::OctetStream, ty);
                    },
                    true,
                ),
                (
                    "a nullable bytes request body",
                    |api| {
                        let ty = bytes(api);
                        request(
                            api,
                            MediaType::Json,
                            Ty {
                                nullable: true,
                                ..ty
                            },
                        );
                    },
                    false,
                ),
                (
                    "a base64 request body",
                    |api| {
                        let ty = insert(api, TypeKind::Bytes(super::BytesEncoding::Base64));
                        request(api, MediaType::OctetStream, ty);
                    },
                    false,
                ),
                (
                    "a JSON object request body",
                    |api| {
                        let field = string(api);
                        let ty = object(api, field, AdditionalProps::Deny);
                        request(api, MediaType::Json, ty);
                    },
                    false,
                ),
            ],
            |api| api.sends_binary_body(&api.operations[0]),
        );
    }

    /// Each row reaches `bytes` (or does not) through one position `uses_bytes_serde` documents,
    /// and through each kind `contains_bytes` walks, cycles included.
    #[test]
    fn uses_bytes_serde_follows_the_table() {
        check(
            &[
                ("no type", |_| {}, false),
                (
                    "a whole octet-stream request body",
                    |api| {
                        let ty = bytes(api);
                        request(api, MediaType::OctetStream, ty);
                    },
                    false,
                ),
                (
                    "a whole JSON request body, sent raw",
                    |api| {
                        let ty = bytes(api);
                        request(api, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "a whole JSON response body, read raw",
                    |api| {
                        let ty = bytes(api);
                        respond(api, OK, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "a JSON request body over an array of bytes",
                    |api| {
                        let item = bytes(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        request(api, MediaType::Json, ty);
                    },
                    true,
                ),
                (
                    "a text request body over an array of bytes",
                    |api| {
                        let item = bytes(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        request(api, MediaType::Text, ty);
                    },
                    false,
                ),
                (
                    "a form request body over a tuple holding bytes",
                    |api| {
                        let (text, item) = (string(api), bytes(api));
                        let ty = insert(api, TypeKind::Tuple(vec![text, item]));
                        request(api, MediaType::FormUrlEncoded, ty);
                    },
                    true,
                ),
                (
                    "an event stream of whole bytes items",
                    |api| {
                        let ty = bytes(api);
                        respond(api, OK, MediaType::EventStream, ty);
                    },
                    true,
                ),
                (
                    "an XML error body over an array of bytes",
                    |api| {
                        let item = bytes(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        respond(api, None, MediaType::Xml, ty);
                    },
                    true,
                ),
                (
                    "a model field, referenced by nothing",
                    |api| {
                        let item = bytes(api);
                        object(api, item, AdditionalProps::Allow);
                    },
                    true,
                ),
                (
                    "a typed additional-properties value",
                    |api| {
                        let (text, item) = (string(api), bytes(api));
                        object(api, text, AdditionalProps::Typed(Box::new(item)));
                    },
                    true,
                ),
                (
                    "a union variant",
                    |api| {
                        let (text, item) = (string(api), bytes(api));
                        let variant = |ty| UnionVariant {
                            name_hint: "V".to_owned(),
                            ty,
                            dispatch: VariantDispatch::Trial { priority: 0 },
                        };
                        insert(
                            api,
                            TypeKind::Union(Union {
                                variants: vec![variant(text), variant(item)],
                                strategy: UnionStrategy::Trial {
                                    mode: UnionMode::AnyOf,
                                },
                            }),
                        );
                    },
                    true,
                ),
                (
                    "a model holding bytes only in an elided definition",
                    |api| {
                        let item = bytes(api);
                        let model = object(api, item, AdditionalProps::Deny);
                        api.types.elide(model.id);
                    },
                    false,
                ),
                (
                    "a self-referencing model without bytes",
                    |api| {
                        let text = string(api);
                        let model = object(api, text, AdditionalProps::Allow);
                        let TypeKind::Struct(object) =
                            &mut api.types.get_mut(model.id).expect("inserted").kind
                        else {
                            unreachable!()
                        };
                        object.additional = AdditionalProps::Typed(Box::new(model));
                    },
                    false,
                ),
                (
                    "a self-referencing model holding bytes",
                    |api| {
                        let item = bytes(api);
                        let model = object(api, item, AdditionalProps::Allow);
                        let TypeKind::Struct(object) =
                            &mut api.types.get_mut(model.id).expect("inserted").kind
                        else {
                            unreachable!()
                        };
                        object.additional = AdditionalProps::Typed(Box::new(model));
                    },
                    true,
                ),
                (
                    "a JSON content parameter over an array of bytes",
                    |api| {
                        let item = bytes(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        api.operations[0]
                            .params
                            .push(parameter(ty, ParamStyle::Content(MediaType::Json)));
                    },
                    true,
                ),
                (
                    "a form query parameter over an array of bytes",
                    |api| {
                        let item = bytes(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        api.operations[0]
                            .params
                            .push(parameter(ty, ParamStyle::Form));
                    },
                    false,
                ),
                (
                    "a model field of base64 bytes, whose newtype is serialized by the codec",
                    |api| {
                        let item = base64(api);
                        object(api, item, AdditionalProps::Allow);
                    },
                    false,
                ),
                (
                    "a JSON request body over an array of base64 bytes",
                    |api| {
                        let item = base64(api);
                        let ty = insert(api, TypeKind::Array(Box::new(item)));
                        request(api, MediaType::Json, ty);
                    },
                    false,
                ),
                (
                    "an event stream of base64 items",
                    |api| {
                        let ty = base64(api);
                        respond(api, OK, MediaType::EventStream, ty);
                    },
                    false,
                ),
            ],
            Api::uses_bytes_serde,
        );
    }

    /// The base64 codec is embedded exactly when a base64 definition the output carries exists,
    /// wherever it sits; raw bytes and an elided base64 definition do not embed it.
    #[test]
    fn uses_base64_follows_the_table() {
        check(
            &[
                ("no type", |_| {}, false),
                (
                    "raw bytes",
                    |api| {
                        let item = bytes(api);
                        object(api, item, AdditionalProps::Allow);
                    },
                    false,
                ),
                (
                    "a base64 definition referenced by nothing",
                    |api| {
                        base64(api);
                    },
                    true,
                ),
                (
                    "a model field of base64 bytes",
                    |api| {
                        let item = base64(api);
                        object(api, item, AdditionalProps::Allow);
                    },
                    true,
                ),
                (
                    "an elided base64 definition",
                    |api| {
                        let ty = base64(api);
                        api.types.elide(ty.id);
                    },
                    false,
                ),
            ],
            Api::uses_base64,
        );
        let mut api = api();
        base64(&mut api);
        assert!(api.runtime_features(false).base64);
    }

    fn base64(api: &mut Api) -> Ty {
        insert(api, TypeKind::Bytes(super::BytesEncoding::Base64))
    }

    fn parameter(ty: Ty, style: ParamStyle) -> Parameter {
        Parameter {
            name: "filter".to_owned(),
            location: ParamLoc::Query,
            ty,
            required: true,
            style,
            allow_reserved: false,
            explode: false,
            deprecated: false,
            default_display: None,
        }
    }

    /// `unmatched_bindings` returns, in the order given, each binding no operation's bound
    /// headers hold, matched in any letter case, and `binds_headers` holds exactly while some
    /// operation sends a bound header.
    #[test]
    fn unmatched_bindings_are_the_ones_no_operation_bound() {
        let mut api = api();
        assert!(!api.binds_headers());
        let bindings = [
            "x-API-version".to_owned(),
            "X-Missing".to_owned(),
            "X-Tenant".to_owned(),
        ];
        assert_eq!(api.unmatched_bindings(&bindings), bindings.each_ref());

        let mut second = api.operations[0].clone();
        second.id = OperationId("getItem".to_owned());
        second.bound_headers.push(BoundHeader {
            name: "X-Tenant".to_owned(),
            required: false,
        });
        api.operations[0].bound_headers.push(BoundHeader {
            name: "X-Api-Version".to_owned(),
            required: true,
        });
        api.operations.push(second);
        assert!(api.binds_headers());
        assert_eq!(api.unmatched_bindings(&bindings), ["X-Missing"]);
        assert!(api.unmatched_bindings(&[]).is_empty());
    }
}
