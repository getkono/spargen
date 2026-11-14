use crate::diag::Provenance;

use super::{Api, Docs, MediaType, RequestBody, Responses, SecurityRequirement, Ty};

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

/// A parsed path template. The segments are what lowering checks path parameters against and what
/// a synthesized operation ID is built from; the emitted URL code starts from `raw` and replaces
/// each `{name}` placeholder with its encoded value.
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

/// One API operation. Path parameters become positional method arguments, in URL template order;
/// every other parameter travels in a per-operation `…Params` struct, whose `new` takes the
/// required ones and which derives `Default` where every one is optional.
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
    /// The header parameters the build bound once on the client (`Spec::add_bound_header`), in
    /// declaration order. Lowering takes each out of [`Self::params`]: it is sent from the client's
    /// bound value rather than taken as an argument.
    pub(crate) bound_headers: Vec<BoundHeader>,
}

/// A header parameter an operation declares and the build bound once on the client: the method
/// takes no argument for it, and sends the value the client binds to `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundHeader {
    /// The header name, as the operation declares it.
    pub(crate) name: String,
    /// Whether the operation requires it: with no value bound, a required header fails the call
    /// before anything is sent, and an optional one is left off the request.
    pub(crate) required: bool,
}

impl Api {
    /// The header bindings in `names` that no operation's [`Operation::bound_headers`] holds,
    /// matched in any letter case, in the order given: bindings that bound nothing, for the
    /// caller to report.
    pub(crate) fn unmatched_bindings<'a>(&self, names: &'a [String]) -> Vec<&'a str> {
        names
            .iter()
            .filter(|name| {
                !self
                    .operations
                    .iter()
                    .flat_map(|operation| &operation.bound_headers)
                    .any(|header| header.name.eq_ignore_ascii_case(name))
            })
            .map(String::as_str)
            .collect()
    }

    /// Whether any operation sends a bound header, so the client emits `with_bound_header`.
    pub(crate) fn binds_headers(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| !operation.bound_headers.is_empty())
    }
}

impl Operation {
    /// The request body beside the type it lowered to: the body the operation's method takes an
    /// argument for and sends. Lowering types every body it keeps (`check_invariants` reports one
    /// it did not), so this is `None` exactly when [`Self::request_body`] is.
    pub(crate) fn typed_body(&self) -> Option<(&RequestBody, Ty)> {
        self.request_body
            .as_ref()
            .and_then(|body| body.ty.map(|ty| (body, ty)))
    }

    /// Whether any parameter rides in the `…Params` bundle (see [`Parameter::is_positional`]), so
    /// the method takes one.
    pub(crate) fn takes_params(&self) -> bool {
        self.params.iter().any(|param| !param.is_positional())
    }

    /// Whether the `…Params` bundle holds a required parameter, so the bundle is constructed with
    /// `…Params::new(required…)` and the method takes it by value rather than as an optional
    /// `impl Into<Option<…Params>>`.
    pub(crate) fn params_required(&self) -> bool {
        self.params
            .iter()
            .any(|param| param.required && !param.is_positional())
    }

    /// Whether the `…Params` bundle holds a parameter that shapes the URL, so the operation's URL
    /// accessor takes the bundle after its [`Parameter::is_positional`] arguments.
    pub(crate) fn url_takes_params(&self) -> bool {
        self.params
            .iter()
            .any(|param| !param.is_positional() && param.location.shapes_url())
    }

    /// Where `param`, a path parameter, sits in the URL template: the index of its `{name}`
    /// placeholder among the template's segments, which orders the positional arguments. A name
    /// no segment holds sorts last.
    pub(crate) fn template_position(&self, param: &Parameter) -> usize {
        self.path
            .segments
            .iter()
            .position(|segment| matches!(segment, PathSegment::Param(name) if *name == param.name))
            .unwrap_or(usize::MAX)
    }
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
    /// `deprecated: true`. A parameter's `…Params` field carries a "Deprecated per the spec."
    /// rustdoc note; a path parameter, a positional argument with no attribute slot, gets a
    /// "Parameter `name` is deprecated per the spec." note in its method's rustdoc.
    pub(crate) deprecated: bool,
    /// The rendered `default` value, if the parameter schema declared one. Documented in rustdoc
    /// (never serde-wired: a params struct is only serialized, and a server-side default means the
    /// client may legitimately omit the value).
    pub(crate) default_display: Option<String>,
}

impl Parameter {
    /// Whether this parameter is a positional argument of the operation's method and its URL
    /// accessor: exactly a path parameter. Both take these in URL template order (see
    /// [`Operation::template_position`]), then the `…Params` bundle, which every other parameter
    /// rides in, required or not.
    pub(crate) fn is_positional(&self) -> bool {
        self.location == ParamLoc::Path
    }

    /// Whether this optional parameter's value can be an explicit `null` distinct from absence,
    /// so its `…Params` field is a `support::Nullable<T>`: `Absent` omits it, `Null` sends `null`.
    ///
    /// Only an optional, nullable `content: application/json` parameter qualifies. Its codec
    /// renders `Null` as the text `null`, which a server tells apart from an absent parameter. A
    /// schema-styled (`style`/`explode`) serialization and a `text/*` one have no token for `null`,
    /// so a `Nullable` there would ask the caller for a distinction the wire cannot carry.
    pub(crate) fn sends_explicit_null(&self) -> bool {
        !self.required
            && self.ty.nullable
            && matches!(self.style, ParamStyle::Content(MediaType::Json))
    }
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

impl ParamLoc {
    /// This location as the OpenAPI parameter `in` value spells it.
    pub(crate) fn as_openapi_in(self) -> &'static str {
        match self {
            ParamLoc::Path => "path",
            ParamLoc::Query => "query",
            ParamLoc::QueryString => "querystring",
            ParamLoc::Header => "header",
            ParamLoc::Cookie => "cookie",
        }
    }

    /// Whether a parameter here shapes the request URL: the path, the query, or the whole query
    /// string.
    pub(crate) fn shapes_url(self) -> bool {
        matches!(
            self,
            ParamLoc::Path | ParamLoc::Query | ParamLoc::QueryString
        )
    }
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
