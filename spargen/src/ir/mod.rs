//! # Subsystem: ir
//! layer-deps: diag
//!
//! The version-agnostic API model: operation set, type graph, auth requirements, media map;
//! provenance (pointer + span) on every node; well-formedness invariants. The IR is the coupling
//! firewall and primary extension seam — it never sees a spec document or Rust tokens, so a new
//! spec-version frontend (`oas32`) lowers into it and touches nothing downstream (PRD §2.3 rule 1).

mod auth;
mod dump;
mod invariant;
mod media;
mod operation;
mod types;

use indexmap::IndexMap;

use crate::diag::Provenance;

pub use auth::{
    ApiKeyLoc, HttpScheme, OAuthMeta, OidcMeta, SchemeId, SecurityRequirement, SecurityScheme,
};
pub use dump::dump;
pub use invariant::check_invariants;
pub use media::{
    ErrorShape, HeaderSpec, MediaType, RequestBody, Response, Responses, StatusSpec, SuccessShape,
};
pub use operation::{
    Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathTemplate,
};
pub use types::{
    AdditionalProps, EnumVariant, Field, Prim, PropertyName, ScalarDefault, ScalarEnum, ScalarRepr,
    ScalarValue, Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind, Union, UnionTag,
};

/// The whole lowered API: the single artifact frontends produce and backends consume (PRD §2.3).
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
    /// Provenance of the document root.
    pub provenance: Provenance,
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

/// A server entry, with variable-substitution metadata (matrix: Document → S).
#[derive(Debug, Clone)]
pub struct Server {
    /// The (possibly templated) server URL.
    pub url: String,
    /// `server.description`.
    pub description: Option<String>,
    /// Declared server variables, in source order.
    pub variables: IndexMap<String, ServerVariable>,
}

/// A single `server.variables` entry.
#[derive(Debug, Clone)]
pub struct ServerVariable {
    /// The default substitution value.
    pub default: String,
    /// The allowed values (`enum`), if constrained.
    pub enumeration: Vec<String>,
    /// Human description.
    pub description: Option<String>,
}

/// Documentation carried from a construct's `title`/`summary`/`description`/`deprecated`, lowered
/// to rustdoc so IDE hover shows API docs (PRD FR3).
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
