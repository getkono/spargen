use super::{Docs, Ty};

/// A supported request/response media type (PRD §3.1). Other media types (XML, multipart) are
/// R-rejected in the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    /// `application/json` (canonical).
    Json,
    /// `application/x-www-form-urlencoded`.
    FormUrlEncoded,
    /// `application/octet-stream` (bytes in; bytes or stream out).
    OctetStream,
    /// `text/plain`.
    TextPlain,
}

/// A request body (matrix: Bodies).
#[derive(Debug, Clone)]
pub struct RequestBody {
    /// The body media type.
    pub media: MediaType,
    /// The body's type, or `None` for an untyped/byte body.
    pub ty: Option<Ty>,
    /// Whether the body is `required`.
    pub required: bool,
}

/// A response status selector (matrix: Responses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSpec {
    /// An exact status code, e.g. `200`.
    Exact(u16),
    /// A status range by leading digit, e.g. `Range(2)` for `2XX`.
    Range(u8),
}

impl StatusSpec {
    /// Whether the selector covers only success (2xx) statuses.
    pub fn is_success(self) -> bool {
        match self {
            StatusSpec::Exact(code) => (200..300).contains(&code),
            StatusSpec::Range(prefix) => prefix == 2,
        }
    }
}

/// A single response header exposed via `ResponseValue` (matrix: Responses → S).
#[derive(Debug, Clone)]
pub struct HeaderSpec {
    /// The header name.
    pub name: String,
    /// The header's type.
    pub ty: Ty,
    /// Whether the header is `required`.
    pub required: bool,
    /// Header documentation.
    pub docs: Docs,
}

/// A typed response for one status selector.
#[derive(Debug, Clone)]
pub struct Response {
    /// The response body media type, if a body is present.
    pub media: Option<MediaType>,
    /// The response body type, if any.
    pub body: Option<Ty>,
    /// Documented response headers.
    pub headers: Vec<HeaderSpec>,
}

/// The full set of responses for an operation: per-status entries plus an optional `default`.
#[derive(Debug, Clone)]
pub struct Responses {
    /// Per-status responses, most-specific first (exact before range).
    pub by_status: Vec<(StatusSpec, Response)>,
    /// The `default` response, if declared.
    pub default: Option<Response>,
}

impl Responses {
    /// The success shape of the operation, applying per-status precedence: exact code > range
    /// (`2XX`) > `default` (PRD D11). A single success source yields plain `T`; multiple yield a
    /// per-operation success enum.
    pub fn success(&self) -> SuccessShape {
        let mut successes = Vec::new();
        for (status, response) in &self.by_status {
            if is_success_status(*status) {
                if let Some(ty) = response.body {
                    successes.push((*status, ty));
                }
            }
        }

        if successes.is_empty() && self.by_status.is_empty() {
            if let Some(default) = &self.default {
                if let Some(body) = default.body {
                    successes.push((StatusSpec::Range(2), body));
                }
            }
        }

        match successes.as_slice() {
            [] => SuccessShape::Unit,
            [(_, ty)] => SuccessShape::Plain(*ty),
            _ => SuccessShape::Enum(successes),
        }
    }

    /// The error shape of the operation: the typed `E` body, an enum across multiple error
    /// statuses, or none. `default` contributes here unless it is the only success source (D11).
    pub fn error(&self) -> ErrorShape {
        let mut errors = Vec::new();
        for (status, response) in &self.by_status {
            if !is_success_status(*status) {
                if let Some(ty) = response.body {
                    errors.push((*status, ty));
                }
            }
        }

        if let Some(default) = &self.default {
            if let Some(body) = default.body {
                errors.push((StatusSpec::Range(0), body));
            }
        }

        match errors.as_slice() {
            [] => ErrorShape::None,
            [(_, ty)] => ErrorShape::Single(*ty),
            _ => ErrorShape::Enum(errors),
        }
    }
}

fn is_success_status(status: StatusSpec) -> bool {
    status.is_success()
}

/// The success return type of an operation (before wrapping in `ResponseValue<T>`).
#[derive(Debug, Clone)]
pub enum SuccessShape {
    /// No success body.
    Unit,
    /// A single success body type.
    Plain(Ty),
    /// Multiple success statuses → a per-operation success enum.
    Enum(Vec<(StatusSpec, Ty)>),
}

/// The typed error body `E` of an operation (matrix: Responses; PRD FR3, FR5 #6).
#[derive(Debug, Clone)]
pub enum ErrorShape {
    /// No documented error body.
    None,
    /// A single documented error body type.
    Single(Ty),
    /// Multiple documented error statuses → a per-operation error enum.
    Enum(Vec<(StatusSpec, Ty)>),
}
