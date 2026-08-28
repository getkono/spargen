//! # Subsystem: ir
//! layer-deps: diag
//!
//! The version-agnostic API model: operation set, type graph, auth requirements, media map;
//! provenance (pointer + span) on every node; well-formedness invariants. The IR is the coupling
//! firewall and primary extension seam — it never sees a spec document or Rust tokens, so a new
//! spec-version frontend (`oas32`) lowers into it and touches nothing downstream.

mod auth;
mod invariant;
mod media;
mod operation;
mod types;

use indexmap::IndexMap;

pub use auth::{
    ApiKeyLoc, HttpScheme, SchemeId, SecurityRequirement, SecurityScheme, SecuritySchemeDef,
};
pub use invariant::check_invariants;
pub use media::{
    BodyEncoding, EncodingMode, ErrorShape, Framing, HeaderShape, MediaType, PropertyEncoding,
    RequestBody, Response, ResponseHeader, Responses, StatusSpec, SuccessShape,
};
pub use operation::{
    Delimiter, Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathSegment,
    PathTemplate,
};
pub use types::{
    AdditionalProps, DefaultValue, DisjointFeature, Field, FieldDefault, JsonCategory, Prim,
    PropertyName, ScalarEnum, ScalarRepr, ScalarValue, Struct, Ty, TypeDef, TypeGraph, TypeId,
    TypeKind, Union, UnionMode, UnionStrategy, UnionVariant, XmlField,
};

/// The whole lowered API: the single artifact frontends produce and backends consume.
#[derive(Debug, Clone)]
pub struct Api {
    /// API identity (`info`).
    pub info: Info,
    /// Servers, with variable-substitution metadata retained.
    pub servers: Vec<Server>,
    /// Every operation, in deterministic order.
    pub operations: Vec<Operation>,
    /// The type graph referenced by operations and each other.
    pub types: TypeGraph,
    /// Named security schemes (`components.securitySchemes`).
    pub security_schemes: IndexMap<SchemeId, SecuritySchemeDef>,
}

impl Api {
    /// Whether any operation uses an `application/xml` / `text/xml` request or response body. Drives
    /// the feature-gated `quick-xml` dependency in the synthesized manifest and the conditional
    /// embedding of the XML runtime helpers — both deterministic functions of the API.
    pub fn uses_xml(&self) -> bool {
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
    pub fn uses_time(&self) -> bool {
        self.types.iter().any(|(_, definition)| {
            matches!(
                definition.kind,
                TypeKind::Primitive(Prim::Date | Prim::DateTime)
            )
        })
    }

    /// Whether any operation returns a sequential response as a typed stream. Drives conditional
    /// stream-runtime embedding and the `futures-core` / reqwest `stream` requirements.
    pub fn uses_streams(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.responses.stream_success().is_some())
    }
}

/// API identity, lowered from `info`.
#[derive(Debug, Clone)]
pub struct Info {
    /// `info.title`.
    pub title: String,
    /// `info.version`.
    pub version: String,
    /// `info.description`, if present.
    pub description: Option<String>,
}

/// A server entry (matrix: Document).
#[derive(Debug, Clone)]
pub struct Server {
    /// OpenAPI 3.2 `name`: a stable identity for this host, used to name the generated builder.
    pub name: Option<String>,
    /// The raw, possibly templated server URL.
    pub url: String,
    /// The URL template split into literals and variable references, parsed once here so codegen
    /// and rendering never re-scan the string.
    pub segments: Vec<UrlSegment>,
    /// Declared variables, in source order.
    pub variables: IndexMap<String, ServerVariable>,
    /// `server.description`.
    pub description: Option<String>,
}

/// One piece of a parsed server URL template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlSegment {
    /// Literal text, emitted verbatim.
    Literal(String),
    /// A `{name}` reference to a declared server variable.
    Variable(String),
}

/// A server variable: a closed or open set of substitutions with a default that is actually sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerVariable {
    /// The value used when the caller supplies none.
    pub default: String,
    /// The permitted values, when the document declares a closed set. Empty means free-form.
    pub enum_values: Vec<String>,
    /// `description`, surfaced as rustdoc on the generated setter.
    pub description: Option<String>,
}

/// Documentation carried from a construct's `title`/`summary`/`description`/`deprecated`, lowered
/// to rustdoc so IDE hover shows API docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Docs {
    /// `title`.
    pub title: Option<String>,
    /// `summary`.
    pub summary: Option<String>,
    /// `description`.
    pub description: Option<String>,
    /// Whether the construct is `deprecated` (also drives `#[deprecated]`).
    pub deprecated: bool,
}
