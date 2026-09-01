use crate::diag::Provenance;

use super::{Docs, MediaType, RequestBody, Responses, SecurityRequirement, Ty};

/// The `operationId` (or synthesized name); the Rust method name is allocated by `name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OperationId(pub(crate) String);

/// An HTTP method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Method {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
    /// The `QUERY` method, added as a fixed path-item field by OpenAPI 3.2.
    Query,
    /// An extension method declared through OpenAPI 3.2 `additionalOperations`.
    Custom(String),
}

impl Method {
    /// The OAS method token.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Method::Get => "get",
            Method::Put => "put",
            Method::Post => "post",
            Method::Delete => "delete",
            Method::Options => "options",
            Method::Head => "head",
            Method::Patch => "patch",
            Method::Trace => "trace",
            Method::Query => "query",
            Method::Custom(method) => method,
        }
    }
}

/// A parsed path template, split into literal and parameter segments so URL construction is
/// static segment concatenation with no runtime regex.
#[derive(Debug, Clone)]
pub(crate) struct PathTemplate {
    /// The raw template, e.g. `/users/{id}/posts`.
    pub(crate) raw: String,
    /// The parsed segments.
    pub(crate) segments: Vec<PathSegment>,
}

/// One segment of a [`PathTemplate`].
#[derive(Debug, Clone)]
pub(crate) enum PathSegment {
    /// A literal path chunk.
    Literal(String),
    /// A `{name}` placeholder bound to a path parameter.
    Param(String),
}

/// One API operation. Required parameters become positional method arguments; optional ones
/// travel in a per-operation `…Params` struct deriving `Default`.
#[derive(Debug, Clone)]
pub(crate) struct Operation {
    /// The operation identifier.
    pub(crate) id: OperationId,
    /// The HTTP method.
    pub(crate) method: Method,
    /// The path template.
    pub(crate) path: PathTemplate,
    /// Parameters (path/query/header/cookie), in deterministic order.
    pub(crate) params: Vec<Parameter>,
    /// The request body, if any.
    pub(crate) request_body: Option<RequestBody>,
    /// The typed responses (success and error).
    pub(crate) responses: Responses,
    /// Operation-level security requirements (which credentials attach where).
    pub(crate) security: Vec<SecurityRequirement>,
    /// `deprecated` → `#[deprecated]` on the method.
    pub(crate) deprecated: bool,
    /// Documentation lowered from `summary`/`description`.
    pub(crate) docs: Docs,
    /// The base URL this operation is sent to when the Operation or Path Item Object overrides the
    /// document's `servers`, already rendered with each server variable at its declared default.
    ///
    /// `None` means "use the client's base URL". An absolute override replaces that base; a
    /// relative one is joined onto it.
    pub(crate) server: Option<String>,
    /// Where the operation came from.
    pub(crate) provenance: Provenance,
}

/// A single operation parameter (matrix: Parameters). Only S-class styles reach the IR;
/// unsupported styles are rejected in the frontend.
#[derive(Debug, Clone)]
pub(crate) struct Parameter {
    /// The wire parameter name.
    pub(crate) name: String,
    /// Where the parameter is carried.
    pub(crate) location: ParamLoc,
    /// The parameter's type.
    pub(crate) ty: Ty,
    /// Whether the parameter is `required` (always true for path parameters).
    pub(crate) required: bool,
    /// The serialization style.
    pub(crate) style: ParamStyle,
    /// `allowReserved: true` — RFC 6570 reserved expansion. Orthogonal to the style: it selects a
    /// percent-encoding set rather than a delimiter layout, and it is a no-op wherever the
    /// location does not percent-encode at all (headers, `style: cookie`).
    pub(crate) allow_reserved: bool,
    /// Whether array/object values use the style's exploded representation.
    pub(crate) explode: bool,
    /// `deprecated` → `#[deprecated]`.
    pub(crate) deprecated: bool,
    /// The rendered `default` value, if the parameter schema declared one. Documented in rustdoc
    /// (never serde-wired: a params struct is only serialized, and a server-side default means the
    /// client may legitimately omit the value).
    pub(crate) default_display: Option<String>,
}

/// Where a parameter is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamLoc {
    Path,
    Query,
    /// OpenAPI 3.2's whole-query parameter.
    QueryString,
    Header,
    Cookie,
}

/// The serialization style of a parameter (matrix: Parameters → S).
///
/// Which `(style, in)` pairs are legal is enforced by the official document schema before
/// lowering, so this enum only carries what codegen must emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParamStyle {
    /// `style: simple` — path and header. RFC 6570 §3.2.2.
    Simple,
    /// `style: matrix` — path only. Leading `;`. RFC 6570 §3.2.7.
    Matrix,
    /// `style: label` — path only. Leading `.`. RFC 6570 §3.2.5.
    Label,
    /// `style: form` — query and cookie. RFC 6570 §3.2.8.
    Form,
    /// `style: spaceDelimited` / `pipeDelimited` — query, array/object, `explode: false` only.
    Delimited(Delimiter),
    /// `style: deepObject` — query, object only. `explode` has no effect on this style.
    DeepObject,
    /// OpenAPI 3.2 cookie syntax (form-shaped values joined with Cookie delimiters, no escaping).
    Cookie,
    /// A `content`-typed parameter, serialized in the given media type.
    Content(MediaType),
}

/// The delimiter of the non-RFC 6570 query styles. Always emitted percent-encoded, since neither
/// a bare space nor a bare `|` is legal in a query string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimiter {
    /// `style: spaceDelimited`.
    Space,
    /// `style: pipeDelimited`.
    Pipe,
}
