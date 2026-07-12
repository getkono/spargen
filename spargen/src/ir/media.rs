use super::Ty;

/// A supported request/response media type. Other media types (XML, multipart) are
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

/// A typed response for one status selector. Response headers stay reachable through
/// `ResponseValue::headers`; typed header accessors are not generated.
#[derive(Debug, Clone)]
pub struct Response {
    /// The response body type, if any.
    pub body: Option<Ty>,
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
    /// The success shape of the operation. A single documented success body yields plain `T`
    /// (any bodyless success sibling, e.g. `204`, is not modeled — the common `T`-plus-`204`
    /// shape stays `Plain`). Two or more documented success bodies yield a per-operation success
    /// enum whose entries are sorted into decode precedence (exact code ascending, then range
    /// ascending, then `default` last) and which also carries any documented bodyless success
    /// status as a payload-free unit variant, so no documented status is silently dropped.
    pub fn success(&self) -> SuccessShape {
        // A default with no explicit status entries is the operation's single success body.
        if self.by_status.is_empty() {
            return match self.default.as_ref().and_then(|default| default.body) {
                Some(body) => SuccessShape::Plain(body),
                None => SuccessShape::Unit,
            };
        }

        let mut entries: Vec<(StatusSpec, Option<Ty>)> = Vec::new();
        for (status, response) in &self.by_status {
            if is_success_status(*status) {
                entries.push((*status, response.body));
            }
        }
        finish_shape(
            entries,
            SuccessShape::Unit,
            SuccessShape::Plain,
            |mut entries| {
                entries.sort_by_key(|(status, _)| precedence_key(*status));
                SuccessShape::Enum(entries)
            },
        )
    }

    /// The error shape of the operation. Zero documented error bodies yields `None`; one yields the
    /// typed `E` body; two or more yield a per-operation error enum, sorted into classification
    /// precedence (exact code ascending, then range ascending, then `default` — the `Range(0)`
    /// sentinel — last) and carrying any documented bodyless error status as a unit variant.
    /// `default` contributes here (as `Range(0)`) whenever it is not the sole success source.
    pub fn error(&self) -> ErrorShape {
        let mut entries: Vec<(StatusSpec, Option<Ty>)> = Vec::new();
        for (status, response) in &self.by_status {
            if !is_success_status(*status) {
                entries.push((*status, response.body));
            }
        }
        if let Some(default) = &self.default {
            entries.push((StatusSpec::Range(0), default.body));
        }
        finish_shape(
            entries,
            ErrorShape::None,
            ErrorShape::Single,
            |mut entries| {
                entries.sort_by_key(|(status, _)| precedence_key(*status));
                ErrorShape::Enum(entries)
            },
        )
    }
}

/// Collapse per-status entries into a response shape by counting how many carry a body: zero → the
/// `unit` shape, exactly one → the `single` shape over that lone body (bodyless siblings are not
/// modeled in this common case), two or more → the `multi` shape over all entries (bodied and
/// bodyless alike).
fn finish_shape<S>(
    entries: Vec<(StatusSpec, Option<Ty>)>,
    unit: S,
    single: impl FnOnce(Ty) -> S,
    multi: impl FnOnce(Vec<(StatusSpec, Option<Ty>)>) -> S,
) -> S {
    let mut bodies = entries.iter().filter_map(|(_, body)| *body);
    match (bodies.next(), bodies.next()) {
        (None, _) => unit,
        (Some(ty), None) => single(ty),
        (Some(_), Some(_)) => multi(entries),
    }
}

/// The deterministic decode-precedence sort key for a documented status selector: exact codes
/// first (ascending), then ranges (ascending by leading digit), then the `default` response (the
/// `Range(0)` sentinel) last. Keys are unique within one operation's entries, so the sort is total.
fn precedence_key(status: StatusSpec) -> (u8, u16) {
    match status {
        StatusSpec::Exact(code) => (0, code),
        StatusSpec::Range(0) => (2, 0),
        StatusSpec::Range(prefix) => (1, u16::from(prefix)),
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
    /// Two or more documented success statuses. Generated as a per-operation response enum, one
    /// variant per status — a payload-carrying variant for a bodied status, a unit variant for a
    /// documented bodyless status (e.g. `204`). Entries are pre-sorted into decode precedence
    /// (exact before range; `default` last); decode dispatches by HTTP status in that order.
    Enum(Vec<(StatusSpec, Option<Ty>)>),
}

/// The typed error body `E` of an operation (matrix: Responses).
#[derive(Debug, Clone)]
pub enum ErrorShape {
    /// No documented error body.
    None,
    /// A single documented error body type.
    Single(Ty),
    /// Two or more documented error statuses. Generated as a per-operation error enum, one variant
    /// per status — a payload-carrying variant for a bodied status, a unit variant for a documented
    /// bodyless status. Entries are pre-sorted into classification precedence (exact before range;
    /// `default` — carried as the `Range(0)` sentinel — last); classification dispatches by HTTP
    /// status in that order.
    Enum(Vec<(StatusSpec, Option<Ty>)>),
}

#[cfg(test)]
mod tests {
    use super::{ErrorShape, Response, Responses, StatusSpec, SuccessShape, Ty};
    use crate::ir::TypeId;

    fn ty(id: u32) -> Ty {
        Ty {
            id: TypeId(id),
            nullable: false,
            boxed: false,
        }
    }

    fn resp(body: Option<u32>) -> Response {
        Response { body: body.map(ty) }
    }

    #[test]
    fn success_enum_sorts_exact_before_range_and_keeps_bodyless_unit() {
        // Document order lists the 2XX range BEFORE the exact 200 (and mixes in a bodyless 204).
        // Precedence must reorder to exact-before-range so a real HTTP 200 decodes into its exact
        // variant, not the overlapping range one; the bodyless 204 survives as a payload-free entry.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Range(2), resp(Some(1))),
                (StatusSpec::Exact(200), resp(Some(2))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            default: None,
        };
        match responses.success() {
            SuccessShape::Enum(entries) => {
                let shape: Vec<_> = entries.iter().map(|(s, b)| (*s, b.is_some())).collect();
                assert_eq!(
                    shape,
                    vec![
                        (StatusSpec::Exact(200), true),
                        (StatusSpec::Exact(204), false),
                        (StatusSpec::Range(2), true),
                    ]
                );
                // The exact 200 body (id 2) precedes the range 2XX body (id 1).
                assert_eq!(entries[0].1.unwrap().id, TypeId(2));
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn error_enum_sorts_exact_before_range_before_default() {
        // Document order: range 4XX, then exact 409, then a default — all must reorder to
        // exact < range < default (the `Range(0)` sentinel is last).
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Range(4), resp(Some(1))),
                (StatusSpec::Exact(409), resp(Some(2))),
            ],
            default: Some(resp(Some(3))),
        };
        match responses.error() {
            ErrorShape::Enum(entries) => {
                let specs: Vec<_> = entries.iter().map(|(s, _)| *s).collect();
                assert_eq!(
                    specs,
                    vec![
                        StatusSpec::Exact(409),
                        StatusSpec::Range(4),
                        StatusSpec::Range(0),
                    ]
                );
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn single_bodied_success_with_bodyless_sibling_stays_plain() {
        // The common `T`-plus-`204` case is NOT promoted to an enum; it stays a plain `T`.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            default: None,
        };
        assert!(matches!(responses.success(), SuccessShape::Plain(_)));
    }
}
