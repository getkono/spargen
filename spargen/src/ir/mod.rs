//! # Subsystem: ir
//! layer-deps: diag
//!
//! The version-agnostic API model: operation set, type graph, auth requirements, media map;
//! provenance (pointer + span) on every node; well-formedness invariants. The IR is the coupling
//! firewall and primary extension seam — it never sees a spec document or Rust tokens. One
//! frontend, `oas31`, lowers both OpenAPI 3.1 and 3.2 into it, and a frontend for another spec
//! version would lower into it too, touching nothing downstream.

mod auth;
mod invariant;
mod media;
mod operation;
mod types;

use std::collections::BTreeSet;

use indexmap::IndexMap;

pub(crate) use auth::{
    ApiKeyLoc, HttpScheme, SchemeId, SecurityRequirement, SecurityScheme, SecuritySchemeDef,
};
pub(crate) use invariant::check_invariants;
pub(crate) use media::{
    ApiErrorBodyImpl, BodyEncoding, EncodingMode, ErrorShape, Framing, HeaderShape, MediaType,
    PropertyEncoding, RequestBody, Response, ResponseHeader, Responses, StatusSpec, SuccessShape,
};
pub(crate) use operation::{
    Delimiter, Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathSegment,
    PathTemplate,
};
pub(crate) use types::{
    AdditionalProps, DefaultValue, DisjointFeature, Field, FieldDefault, JsonCategory, Openness,
    Prim, PropertyName, ScalarEnum, ScalarRepr, ScalarValue, Struct, Ty, TypeDef, TypeGraph,
    TypeId, TypeKind, Union, UnionMode, UnionStrategy, UnionVariant, XmlField,
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

impl Api {
    /// Whether any operation uses an `application/xml` / `text/xml` request or response body. Drives
    /// the feature-gated `quick-xml` dependency in the synthesized manifest and the conditional
    /// embedding of the XML runtime helpers — both deterministic functions of the API.
    pub(crate) fn uses_xml(&self) -> bool {
        self.operations.iter().any(|operation| {
            let request_xml = operation
                .request_body
                .as_ref()
                .is_some_and(|body| body.media == MediaType::Xml);
            let response_xml = operation
                .responses
                .by_status
                .iter()
                .map(|(_, response)| response)
                .chain(operation.responses.default.as_ref())
                .any(|response| response.media == Some(MediaType::Xml));
            request_xml || response_xml
        })
    }

    /// Whether the type graph contains a `format: date-time` or `format: date` primitive. Drives
    /// the conditional embedding of the RFC 3339 `DateTime`/`Date` runtime newtypes and the `time`
    /// requirement.
    ///
    /// This is a property of the API alone; whether those primitives actually *become* the newtypes
    /// additionally depends on the `time` config knob, which callers apply themselves.
    pub(crate) fn uses_time(&self) -> bool {
        self.types.iter().any(|(_, definition)| {
            matches!(
                definition.kind,
                TypeKind::Primitive(Prim::Date | Prim::DateTime)
            )
        })
    }

    /// Whether any operation returns a sequential response as a typed stream. Drives conditional
    /// stream-runtime embedding and the `futures-core` / reqwest `stream` requirements.
    pub(crate) fn uses_streams(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.responses.stream_success().is_some())
    }

    /// Whether generated code serializes or deserializes a `bytes` value through serde: a model
    /// field, additional-properties value or union variant containing one, a JSON `content`
    /// parameter containing one, or a serde-encoded body (JSON, form, XML or a sequential stream)
    /// containing one other than a whole non-streaming `bytes` body, which is sent and read raw.
    /// Drives the `serde` feature of the `bytes` requirement.
    pub(crate) fn uses_bytes_serde(&self) -> bool {
        let model_needs_serde =
            self.types
                .iter()
                .any(|(_, definition)| match &definition.kind {
                    TypeKind::Struct(object) => {
                        object.fields.iter().any(|field| {
                            contains_bytes(&self.types, field.ty.id, &mut BTreeSet::new())
                        }) || match &object.additional {
                            AdditionalProps::Typed(ty) => {
                                contains_bytes(&self.types, ty.id, &mut BTreeSet::new())
                            }
                            AdditionalProps::Allow | AdditionalProps::Deny => false,
                        }
                    }
                    TypeKind::Union(union) => union.variants.iter().any(|variant| {
                        contains_bytes(&self.types, variant.ty.id, &mut BTreeSet::new())
                    }),
                    // Requirements are derived only from an `Api` that passed `check_invariants`,
                    // which rejects a surviving reservation. Answering `false` would under-declare
                    // a runtime dependency for a shape nobody computed.
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
                }) || operation.request_body.as_ref().is_some_and(|body| {
                    body.ty
                        .is_some_and(|ty| self.typed_body_needs_bytes_serde(body.media, ty.id))
                }) || operation
                    .responses
                    .by_status
                    .iter()
                    .map(|(_, response)| response)
                    .chain(operation.responses.default.iter())
                    .any(|response| {
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
                    Some(TypeKind::Bytes)
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
        Some(TypeKind::Bytes) => true,
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
        // Unreachable for the reason `Api::uses_bytes_serde` states: only a checked `Api` gets
        // here.
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
