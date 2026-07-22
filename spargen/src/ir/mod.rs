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

pub use auth::{ApiKeyLoc, HttpScheme, SchemeId, SecurityRequirement, SecurityScheme};
pub use invariant::check_invariants;
pub use media::{
    ErrorShape, Framing, MediaType, RequestBody, Response, Responses, StatusSpec, SuccessShape,
};
pub use operation::{
    Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathSegment, PathTemplate,
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
    pub security_schemes: IndexMap<SchemeId, SecurityScheme>,
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
    /// The (possibly templated) server URL.
    pub url: String,
    /// `server.description`.
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
