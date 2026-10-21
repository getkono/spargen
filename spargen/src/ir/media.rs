use super::{ParamStyle, Ty};

/// The wire codec selected for a supported request/response media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaType {
    /// `application/json` (canonical).
    Json,
    /// `application/x-www-form-urlencoded`.
    FormUrlEncoded,
    /// `application/xml` / `text/xml`: a body serialized/deserialized as XML via the runtime's
    /// feature-gated `quick-xml` codec. Lowers to the same struct type `T` as JSON; JSON still wins
    /// when both are offered. Scoped to single-body request/response bodies (see
    /// [`Responses::xml_in_multi_status`](super::Responses::xml_in_multi_status)).
    Xml,
    /// `application/octet-stream` (raw bytes).
    OctetStream,
    /// A raw UTF-8 textual representation (`text/*`, plus explicitly supported textual vendor
    /// media such as GitHub's `application/octocat-stream`).
    Text,
    /// `multipart/form-data` (request bodies): an object schema whose properties are the form
    /// parts — binary/bytes properties become file parts, scalars/composites become text parts.
    Multipart,
    /// `text/event-stream` (Server-Sent Events, response bodies): a stream of items decoded from
    /// the event `data:` fields. Lowered to a streaming operation returning `EventStream<T>`.
    EventStream,
    /// JSON Lines (`application/x-ndjson`, `application/jsonl`; response bodies): a stream of
    /// items, one per line. Lowered to a streaming operation returning `EventStream<T>`.
    Ndjson,
    /// RFC 7464 JSON Text Sequences (`application/json-seq`, `application/*+json-seq`).
    JsonSequence,
}

impl MediaType {
    /// The stream framing for a streaming response media type, or `None` for a non-streaming media.
    pub(crate) fn stream_framing(self) -> Option<Framing> {
        match self {
            MediaType::EventStream => Some(Framing::Sse),
            MediaType::Ndjson => Some(Framing::Ndjson),
            MediaType::JsonSequence => Some(Framing::JsonSequence),
            _ => None,
        }
    }
}

/// How a streaming response body is framed into typed items. Mirrors the runtime `Framing` enum;
/// codegen maps each variant to its `support::Framing` counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    /// Server-Sent Events (`text/event-stream`).
    Sse,
    /// Standards-compliant SSE events converted to JSON objects before schema deserialization.
    SseEvent,
    /// OpenAPI 3.2 SSE whose envelope `data` string contains JSON described by `contentSchema`.
    /// The runtime parses the envelope metadata but yields the decoded JSON payload directly.
    SseJsonData,
    /// Newline-delimited JSON (`application/x-ndjson`).
    Ndjson,
    /// RFC 7464 records separated by ASCII RS (`0x1E`).
    JsonSequence,
}

/// A request body (matrix: Bodies).
#[derive(Debug, Clone)]
pub(crate) struct RequestBody {
    /// The body media type.
    pub(crate) media: MediaType,
    /// The selected content type essence, preserved for the emitted `Content-Type` header.
    pub(crate) content_type: String,
    /// The body's type, or `None` for a non-octet-stream body declared without a schema, or for
    /// a body whose declared schema failed to lower. An octet-stream body lowers to a non-nullable
    /// `bytes::Bytes`, which [`check_invariants`](super::check_invariants) holds it to.
    pub(crate) ty: Option<Ty>,
    /// Whether the body is `required`. A required body is a plain argument; an optional one is
    /// passed as `Option<&T>` and omitted from the request when absent.
    pub(crate) required: bool,
    /// Per-property wire encoding for a form or multipart body. Always fully resolved — one entry
    /// per body property, in field order — so the runtime never has to infer a default.
    pub(crate) encoding: BodyEncoding,
}

/// Per-property wire encoding for an `application/x-www-form-urlencoded` or
/// `multipart/form-data` request body (matrix: Media → Encoding Object).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BodyEncoding {
    /// One entry per body-schema property, in the body struct's field order.
    pub(crate) properties: Vec<PropertyEncoding>,
    /// The mode of the members of the body struct's typed `additionalProperties` overflow map,
    /// which no Encoding Object can name: the default for the overflow's value type. `None` when
    /// the body struct has no typed overflow.
    pub(crate) additional: Option<EncodingMode>,
}

/// How one property of a form or multipart body is rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertyEncoding {
    /// The wire property name — the form field name, or the multipart part name.
    pub(crate) name: String,
    pub(crate) mode: EncodingMode,
    /// Literal extra part headers (multipart only), with `Content-Type` already removed because
    /// the specification describes it separately.
    pub(crate) headers: Vec<(String, String)>,
}

/// The Encoding Object's mode switch.
///
/// The specification keys this on *presence*: any explicit `style`/`explode`/`allowReserved`
/// selects RFC 6570 query-style serialization and makes `contentType` inert, while all three
/// absent selects media-type serialization under `contentType` (explicit or defaulted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EncodingMode {
    /// Media-type mode: the value is rendered in `content_type` by `codec`.
    Media {
        /// The single concrete media type this property is sent as.
        content_type: String,
        /// The codec that renders it.
        codec: MediaType,
    },
    /// RFC 6570 query-style mode.
    Style {
        /// Restricted by the document schema to form/spaceDelimited/pipeDelimited/deepObject.
        style: ParamStyle,
        explode: bool,
        allow_reserved: bool,
    },
}
