use super::Ty;

/// A supported request/response media type. Other media types (e.g. XML) are R-rejected in the
/// frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    /// `application/json` (canonical).
    Json,
    /// `application/x-www-form-urlencoded`.
    FormUrlEncoded,
    /// `application/xml` / `text/xml`: a body serialized/deserialized as XML via the runtime's
    /// feature-gated `quick-xml` codec. Lowers to the same struct type `T` as JSON; JSON still wins
    /// when both are offered. Scoped to single-body request/response bodies (see
    /// [`Responses::xml_in_multi_status`]).
    Xml,
    /// `application/octet-stream` (bytes in; bytes or stream out).
    OctetStream,
    /// `text/plain`.
    TextPlain,
    /// `multipart/form-data` (request bodies): an object schema whose properties are the form
    /// parts — binary/bytes properties become file parts, scalars/composites become text parts.
    Multipart,
    /// `text/event-stream` (Server-Sent Events, response bodies): a stream of items decoded from
    /// the event `data:` fields. Lowered to a streaming operation returning `EventStream<T>`.
    EventStream,
    /// `application/x-ndjson` (newline-delimited JSON, response bodies): a stream of items, one per
    /// line. Lowered to a streaming operation returning `EventStream<T>`.
    Ndjson,
}

impl MediaType {
    /// The stream framing for a streaming response media type, or `None` for a non-streaming media.
    pub fn stream_framing(self) -> Option<Framing> {
        match self {
            MediaType::EventStream => Some(Framing::Sse),
            MediaType::Ndjson => Some(Framing::Ndjson),
            _ => None,
        }
    }
}

/// How a streaming response body is framed into typed items. Mirrors the runtime `Framing` enum;
/// codegen maps each variant to its `support::Framing` counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Server-Sent Events (`text/event-stream`).
    Sse,
    /// Newline-delimited JSON (`application/x-ndjson`).
    Ndjson,
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
    /// The chosen body media type, or `None` for a bodyless response. Codegen routes the decode by
    /// this (e.g. an XML body decodes through the `quick-xml` runtime helpers rather than serde_json).
    pub media: Option<MediaType>,
    /// For a streaming response (chosen media `text/event-stream` or `application/x-ndjson`), the
    /// framing of the streamed items; `None` for a whole-body response. The `body` is the item
    /// type `T` when this is `Some`.
    pub stream: Option<Framing>,
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

    /// Whether the operation's success response is a typed *stream* (`text/event-stream` or
    /// `application/x-ndjson`), and if so its framing plus the streamed item type `T`. Streaming is
    /// scoped to the single-success-body case: it fires only when exactly one success response
    /// carries a body and that body was lowered from a streaming media. The generated method then
    /// returns `EventStream<T>` in place of `ResponseValue<T>`. A JSON alternative on the same
    /// response wins during media selection (see `choose_media`), so it never reaches here as a
    /// stream. Multiple bodied success statuses fall back to the normal (non-streaming) shape.
    pub fn stream_success(&self) -> Option<(Framing, Ty)> {
        let responses = self.success_responses();
        let mut bodied = responses
            .into_iter()
            .filter(|response| response.body.is_some());
        match (bodied.next(), bodied.next()) {
            (Some(response), None) => {
                let framing = response.stream?;
                let body = response.body?;
                Some((framing, body))
            }
            _ => None,
        }
    }

    /// The media type of the operation's single bodied success response, when exactly one success
    /// response carries a body (i.e. [`Self::success`] is [`SuccessShape::Plain`]). Codegen uses this
    /// to route the success decode (JSON vs XML). `None` when there is no single bodied success.
    pub fn single_success_media(&self) -> Option<MediaType> {
        let mut bodied = self
            .success_responses()
            .into_iter()
            .filter(|response| response.body.is_some());
        match (bodied.next(), bodied.next()) {
            (Some(response), None) => response.media,
            _ => None,
        }
    }

    /// The media type of the operation's single bodied error response, when exactly one error
    /// response carries a body (i.e. [`Self::error`] is [`ErrorShape::Single`]). Codegen uses this to
    /// route the error-body classification (JSON vs XML). `None` when there is no single bodied error.
    pub fn single_error_media(&self) -> Option<MediaType> {
        let mut bodied = self
            .error_responses()
            .into_iter()
            .filter(|response| response.body.is_some());
        match (bodied.next(), bodied.next()) {
            (Some(response), None) => response.media,
            _ => None,
        }
    }

    /// Whether an XML body appears in a response position that lowers to a *multi-status* enum
    /// (two or more bodied success or error statuses). XML decode is scoped to the single-body
    /// success/error paths, so this exotic combination is rejected cleanly during lowering (narrowed
    /// `E009`) rather than silently mis-decoding an XML body as JSON.
    pub fn xml_in_multi_status(&self) -> bool {
        let is_xml = |response: &&Response| response.media == Some(MediaType::Xml);
        let success_multi = matches!(self.success(), SuccessShape::Enum(_))
            && self.success_responses().iter().any(is_xml);
        let error_multi = matches!(self.error(), ErrorShape::Enum(_))
            && self.error_responses().iter().any(is_xml);
        success_multi || error_multi
    }

    /// The operation's error responses: every non-success explicit status plus the `default`
    /// response (which matches any status). Mirrors the entry set built by [`Self::error`].
    fn error_responses(&self) -> Vec<&Response> {
        let mut responses: Vec<&Response> = self
            .by_status
            .iter()
            .filter(|(status, _)| !is_success_status(*status))
            .map(|(_, response)| response)
            .collect();
        if let Some(default) = &self.default {
            responses.push(default);
        }
        responses
    }

    /// The operation's success responses in document order: the `default` response alone when no
    /// explicit statuses are declared (it is then the sole success), otherwise the 2xx entries.
    /// Mirrors the success/error split used by [`Self::success`].
    fn success_responses(&self) -> Vec<&Response> {
        if self.by_status.is_empty() {
            return self.default.iter().collect();
        }
        self.by_status
            .iter()
            .filter(|(status, _)| is_success_status(*status))
            .map(|(_, response)| response)
            .collect()
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
        Response {
            media: body.map(|_| super::MediaType::Json),
            body: body.map(ty),
            stream: None,
        }
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
