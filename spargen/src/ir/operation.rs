use crate::diag::Provenance;

use super::{Docs, MediaType, RequestBody, Responses, SecurityRequirement, Ty};

/// The `operationId` (or synthesized name — PRD D9); the Rust method name is allocated by `name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationId(pub String);

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

/// A parsed path template, split into literal and parameter segments so URL construction is
/// static segment concatenation with no runtime regex (PRD NFR1).
#[derive(Debug, Clone)]
pub struct PathTemplate {
    /// The raw template, e.g. `/users/{id}/posts`.
    pub raw: String,
    /// The parsed segments.
    pub segments: Vec<PathSegment>,
}

/// One segment of a [`PathTemplate`].
#[derive(Debug, Clone)]
pub enum PathSegment {
    /// A literal path chunk.
    Literal(String),
    /// A `{name}` placeholder bound to a path parameter.
    Param(String),
}

/// One API operation. Required parameters become positional method arguments; optional ones
/// travel in a per-operation `…Params` struct deriving `Default` (PRD FR3, D3).
#[derive(Debug, Clone)]
pub struct Operation {
    /// The operation identifier.
    pub id: OperationId,
    /// The HTTP method.
    pub method: Method,
    /// The path template.
    pub path: PathTemplate,
    /// Parameters (path/query/header/cookie), in deterministic order.
    pub params: Vec<Parameter>,
    /// The request body, if any.
    pub request_body: Option<RequestBody>,
    /// The typed responses (success and error).
    pub responses: Responses,
    /// Operation-level security requirements (which credentials attach where; PRD FR4).
    pub security: Vec<SecurityRequirement>,
    /// `deprecated` → `#[deprecated]` on the method.
    pub deprecated: bool,
    /// Documentation lowered from `summary`/`description`.
    pub docs: Docs,
    /// Where the operation came from.
    pub provenance: Provenance,
}

/// A single operation parameter (matrix: Parameters). Only S-class styles reach the IR;
/// unsupported styles are rejected in the frontend.
#[derive(Debug, Clone)]
pub struct Parameter {
    /// The wire parameter name.
    pub name: String,
    /// Where the parameter is carried.
    pub location: ParamLoc,
    /// The parameter's type.
    pub ty: Ty,
    /// Whether the parameter is `required` (always true for path parameters).
    pub required: bool,
    /// The serialization style.
    pub style: ParamStyle,
    /// The `explode` flag (with the standard style default already applied).
    pub explode: bool,
    /// `deprecated` → `#[deprecated]`.
    pub deprecated: bool,
}

/// Where a parameter is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamLoc {
    Path,
    Query,
    Header,
    Cookie,
}

/// The serialization style of a parameter (matrix: Parameters → S).
#[derive(Debug, Clone)]
pub enum ParamStyle {
    /// `style: simple`.
    Simple,
    /// `style: form`.
    Form,
    /// A `content`-typed parameter, serialized in the given media type (JSON).
    Content(MediaType),
}
