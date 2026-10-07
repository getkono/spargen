use super::{ParamStyle, Ty, TypeGraph};

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
    /// [`Responses::xml_in_multi_status`]).
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
    /// `application/x-ndjson` (newline-delimited JSON, response bodies): a stream of items, one per
    /// line. Lowered to a streaming operation returning `EventStream<T>`.
    Ndjson,
    /// RFC 7464 JSON Text Sequences and `+json-seq` media.
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
    /// any body whose declared schema failed to lower. Not every such failure is reported today
    /// (see #107 and #109). An octet-stream body without a schema lowers to `bytes::Bytes`; the
    /// reference may still be nullable, which the emitter does not handle yet (#104).
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

/// A response status selector (matrix: Responses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusSpec {
    /// An exact status code, e.g. `200`.
    Exact(u16),
    /// A status range by leading digit, e.g. `Range(2)` for `2XX`.
    Range(u8),
    /// The `default` response, which covers every status no other selector documents. Lowering
    /// stores that response in [`Responses::default`], and [`Responses::error`] offers it to the
    /// error shape under this selector, classified last.
    Default,
}

impl StatusSpec {
    /// Whether the selector covers only success (2xx) statuses: an exact code in `200..=299`, or
    /// the `2XX` range. Every other selector — `1XX`, `3XX`, `4XX`, `5XX`, and each exact code in
    /// them — is on the error side of the generated shape, `304 Not Modified` included (issue
    /// #198). That is RFC 9110's own classification (§15.3 "Successful 2xx"; 304 is in §15.4
    /// "Redirection 3xx"), and it is the only one the generated dispatch can honour: an emitted
    /// method enters its success branch on the transport's `StatusCode::is_success()`, which is
    /// exactly 2xx, so a status this predicate put on the success side would be a variant no
    /// response could reach. A documented bodyless `304` therefore reaches the caller from the
    /// error side: as a unit variant where that side documents any error body (the `304` is then
    /// one more entry of the error enum; see `Responses::error`), and as `UnexpectedStatus`
    /// carrying the status where it documents none. A documented
    /// `301`/`302`/`303`/`307`/`308` reaches the error side only when the injected client's
    /// redirect policy does not follow it. `default` is not a success selector: it documents
    /// statuses of either class, and [`Responses::success`] decides separately when it is the
    /// success source.
    pub(crate) fn is_success(self) -> bool {
        match self {
            StatusSpec::Exact(code) => (200..300).contains(&code),
            StatusSpec::Range(prefix) => prefix == 2,
            StatusSpec::Default => false,
        }
    }

    /// The selector as it reads in prose: `404`, `5XX`, or `default` — the spelling of its
    /// Responses Object key. The generated error enum's `Display` and `spargen diff`'s response
    /// labels both name a status this way.
    pub(crate) fn display_label(self) -> String {
        match self {
            StatusSpec::Exact(code) => code.to_string(),
            StatusSpec::Range(prefix) => format!("{prefix}XX"),
            StatusSpec::Default => "default".to_owned(),
        }
    }
}

/// A typed response for one status selector. Documented headers get typed accessors; every header
/// stays reachable raw through `ResponseValue::headers`.
#[derive(Debug, Clone)]
pub(crate) struct Response {
    /// The response body type, if any.
    pub(crate) body: Option<Ty>,
    /// The chosen body media type, or `None` for a bodyless response. Codegen routes the decode by
    /// this (e.g. XML/text/binary bodies use their own codecs rather than serde_json).
    pub(crate) media: Option<MediaType>,
    /// For a streaming response (chosen media `text/event-stream` or `application/x-ndjson`), the
    /// framing of the streamed items; `None` for a whole-body response. The `body` is the item
    /// type `T` when this is `Some`. Lowering records it in every response position, and rejects
    /// a bodied stream anywhere but the operation's single bodied success (see
    /// [`Responses::stream_outside_single_success`]).
    pub(crate) stream: Option<Framing>,
    /// Documented response headers, in source order. A `Content-Type` entry is dropped during
    /// lowering, because the specification says it is ignored.
    pub(crate) headers: Vec<ResponseHeader>,
}

/// One documented response header.
#[derive(Debug, Clone)]
pub(crate) struct ResponseHeader {
    /// The header name, as declared.
    pub(crate) name: String,
    /// The value type.
    pub(crate) ty: Ty,
    /// Whether the header is documented as always present.
    pub(crate) required: bool,
    /// `explode` for the `simple` style; the default is `false`.
    pub(crate) explode: bool,
    /// The wire shape, derived from `ty` — `simple` is lossy, so the shape cannot be recovered
    /// from the text and must travel with it.
    pub(crate) shape: HeaderShape,
    /// `deprecated` → `#[deprecated]` on the accessor field.
    pub(crate) deprecated: bool,
    /// Documentation carried onto the generated field.
    pub(crate) docs: super::Docs,
}

/// The wire shape of a documented header value. Mirrors the runtime enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderShape {
    /// A single scalar.
    Scalar,
    /// A comma-separated list.
    Array,
    /// Alternating `key,value` pairs, or `key=value` when exploded.
    Object,
    /// A `content`-typed header carrying JSON.
    Json,
    /// `Set-Cookie`: one value per occurrence, never comma-joined (RFC 9110 §5.3 exempts it from
    /// the field-list rule), decoded into a list of the declared per-line type.
    SetCookie,
}

/// The full set of responses for an operation: per-status entries plus an optional `default`.
#[derive(Debug, Clone)]
pub(crate) struct Responses {
    /// Per-status responses, most-specific first (exact before range).
    pub(crate) by_status: Vec<(StatusSpec, Response)>,
    /// The `default` response, if declared.
    pub(crate) default: Option<Response>,
}

impl Responses {
    /// The success shape of the operation. No bodied success entry yields `()`. One bodied entry
    /// alone yields plain `T`. Anything more yields a per-operation success enum, sorted into decode
    /// precedence (exact code ascending, then range ascending), that carries each bodyless success
    /// *entry* as a payload-free unit variant: two or more bodied entries, or one bodied entry
    /// beside a documented bodyless one — the common `T`-plus-`204` shape — since a documented
    /// `204` has no `T` to decode, and a plain `T` would read its empty body as a malformed `T`.
    /// The one exception is a *streaming* single body (see [`Self::stream_success`]): an empty
    /// body is a well-formed empty stream, so that shape stays `Plain` and its bodyless sibling
    /// decodes to a stream that yields nothing.
    ///
    /// The entries are the lowered success statuses of `by_status`, not everything the document
    /// declares; `default` is never among them. A success status is a 2xx one as
    /// [`StatusSpec::is_success`] decides it, so a documented `3XX` — `304 Not Modified` included —
    /// is never an entry here and never promotes a lone body to the enum: `200` with a body beside
    /// a bodyless `304` is plain `T`, and the `304` is on the error side (issue #198; the
    /// tests below pin both, since issue #105 exists because this comment was once false). It is the success source only when `by_status`
    /// documents no success status at all (see [`Self::default_is_success_source`]) — the early
    /// return that bypasses the count above — and is offered to the error shape as
    /// [`StatusSpec::Default`] whenever it is declared, subject there to the same entry rule (see
    /// [`Self::error`]).
    ///
    /// The two `default` claims above, pinned through the public pipeline (this item is private, so
    /// the example drives `spargen::generate` and reads the emitted client):
    ///
    /// ```
    /// const PET: &str = "{description: pet, content: {application/json: {schema: {$ref: '#/components/schemas/Pet'}}}}";
    /// const PROBLEM: &str = "{description: problem, content: {application/json: {schema: {$ref: '#/components/schemas/Problem'}}}}";
    ///
    /// /// The emitted client for one `GET /x` (`getX`) declaring `responses`.
    /// fn generated(responses: &[(&str, &str)]) -> String {
    ///     let responses: String = responses
    ///         .iter()
    ///         .map(|(key, response)| format!("        '{key}': {response}\n"))
    ///         .collect();
    ///     let dir = tempfile::tempdir().unwrap();
    ///     let spec = dir.path().join("openapi.yaml");
    ///     let out = dir.path().join("client.rs");
    ///     std::fs::write(&spec, format!(
    ///         "openapi: 3.1.0\ninfo: {{title: t, version: '1'}}\npaths:\n  /x:\n    get:\n      \
    ///          operationId: getX\n      responses:\n{responses}components:\n  schemas:\n    \
    ///          Pet: {{type: object, properties: {{name: {{type: string}}}}}}\n    \
    ///          Problem: {{type: object, properties: {{detail: {{type: string}}}}}}\n"
    ///     )).unwrap();
    ///     let build = spargen::Spec::new(camino::Utf8PathBuf::from_path_buf(spec).unwrap())
    ///         .build(camino::Utf8PathBuf::from_path_buf(out.clone()).unwrap())
    ///         .cargo(spargen::CargoIntegration::Off);
    ///     spargen::generate(&build).expect_success();
    ///     std::fs::read_to_string(out).unwrap()
    /// }
    ///
    /// /// The variant names of the emitted `pub enum {name}`.
    /// fn variants(code: &str, name: &str) -> Vec<String> {
    ///     let head = format!("pub enum {name} {{\n");
    ///     let start = code.find(&head).expect(&head) + head.len();
    ///     let body = &code[start..start + code[start..].find("\n}").unwrap()];
    ///     body.lines()
    ///         .map(|line| line.trim().split(['(', ',']).next().unwrap().to_owned())
    ///         .collect()
    /// }
    ///
    /// /// The success type `get_x` returns inside `ResponseValue<_>`.
    /// fn success_type(code: &str) -> &str {
    ///     let head = "Result<support::ResponseValue<";
    ///     let start = code.find(head).expect(head) + head.len();
    ///     &code[start..start + code[start..].find(">, support::Error<").unwrap()]
    /// }
    ///
    /// // `default` is never a success entry: beside a declared success status it enters the error
    /// // enum alone, even where the success side is an enum it could have joined.
    /// let code = generated(&[("200", PET), ("204", "{description: none}"), ("404", PROBLEM), ("default", PROBLEM)]);
    /// assert_eq!(variants(&code, "GetXResponse"), ["Status200", "Status204"]);
    /// assert_eq!(variants(&code, "GetXError"), ["Status404", "Default"]);
    /// assert!(!code.contains("GetXResponse::Default"));
    ///
    /// // `default` is the success source exactly when `by_status` declares no success status:
    /// // empty, or only non-2xx statuses such as `404`.
    /// for responses in [&[("default", PROBLEM)][..], &[("404", PET), ("default", PROBLEM)]] {
    ///     assert_eq!(success_type(&generated(responses)), "types::Problem");
    /// }
    /// // One declared success status, even a bodyless one, takes that role from it.
    /// let code = generated(&[("204", "{description: none}"), ("default", PROBLEM)]);
    /// assert_eq!(success_type(&code), "()");
    /// ```
    pub(crate) fn success(&self) -> SuccessShape {
        // With no success status documented, `default` is what documents every 2xx, so it is the
        // operation's single success body.
        if self.default_is_success_source() {
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
        let into_enum = |mut entries: Vec<(StatusSpec, Option<Ty>)>| {
            entries.sort_by_key(|(status, _)| precedence_key(*status));
            SuccessShape::Enum(entries)
        };
        // One body beside a documented bodyless status: two outcomes a plain `T` cannot tell
        // apart, so the bodyless one gets its own unit variant — unless the body is a stream,
        // which an empty body satisfies as-is.
        let bodied = entries.iter().filter(|(_, body)| body.is_some()).count();
        if bodied == 1 && entries.len() > 1 && self.stream_success().is_none() {
            return into_enum(entries);
        }
        finish_shape(entries, SuccessShape::Unit, SuccessShape::Plain, into_enum)
    }

    /// Whether two or more success responses carry a body — the shapes whose success enum decodes
    /// more than one body. Distinct from [`SuccessShape::Enum`], which a single body beside a
    /// bodyless sibling also yields; the XML and streaming limits below are about decoding a
    /// second body, not about the enum.
    fn multiple_success_bodies(&self) -> bool {
        self.success_responses()
            .iter()
            .filter(|response| response.body.is_some())
            .nth(1)
            .is_some()
    }

    /// Whether the operation's success response is a typed *stream*, and if so its framing plus the
    /// streamed item type `T`. A stream is a body lowered from a sequential media — any media whose
    /// [`MediaType::stream_framing`] is `Some`: Server-Sent Events (`text/event-stream`), JSON Lines
    /// (`application/x-ndjson`, `application/jsonl`), or JSON Text Sequences
    /// (`application/json-seq`, `application/*+json-seq`). The returned framing may be any
    /// [`Framing`] variant: an OpenAPI 3.2 `itemSchema` on `text/event-stream` yields
    /// [`Framing::SseEvent`] or [`Framing::SseJsonData`] rather than [`Framing::Sse`]. Streaming is
    /// scoped to the single-success-body case: it fires only when exactly one success response
    /// carries a body and that body was lowered from a streaming media. The generated method then
    /// returns `EventStream<T>` in place of `ResponseValue<T>`. Media selection (`choose_media`)
    /// ranks JSON above every streaming media, so a response that also offers a JSON alternative
    /// lowers to that JSON body and never reaches here as a stream. A streaming body in any other position — beside a second bodied success, or on the
    /// error side — is rejected during lowering (see [`Self::stream_outside_single_success`]), so
    /// no generated operation decodes a stream as a whole body.
    pub(crate) fn stream_success(&self) -> Option<(Framing, Ty)> {
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
    /// response carries a body. Codegen uses this to route the [`SuccessShape::Plain`] decode (a
    /// single body beside a bodyless sibling is an [`SuccessShape::Enum`], routed per status
    /// instead). `None` when there is no single bodied success.
    pub(crate) fn single_success_media(&self) -> Option<MediaType> {
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
    /// response carries a body. Codegen uses this to route the [`ErrorShape::Single`] classification
    /// (a single body beside a bodyless error entry is an [`ErrorShape::Enum`], routed per status
    /// instead). `None` when there is no single bodied error.
    pub(crate) fn single_error_media(&self) -> Option<MediaType> {
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
    /// with two or more bodied success or error statuses. XML decode is scoped to the single-body
    /// success/error paths — a lone XML body beside a bodyless sibling, on either side, is still
    /// that one body, decoded as XML by its enum arm — so this exotic combination is rejected
    /// cleanly during lowering (narrowed `E009`) rather than silently mis-decoding an XML body as
    /// JSON.
    pub(crate) fn xml_in_multi_status(&self) -> bool {
        let is_xml = |response: &&Response| response.media == Some(MediaType::Xml);
        let success_multi =
            self.multiple_success_bodies() && self.success_responses().iter().any(is_xml);
        let error_multi = self.multiple_error_bodies() && self.error_responses().iter().any(is_xml);
        success_multi || error_multi
    }

    /// Whether two or more error responses (the `default` included) carry a body — the error
    /// shapes whose enum decodes more than one body. Distinct from [`ErrorShape::Enum`], which a
    /// single body beside a bodyless error entry also yields.
    fn multiple_error_bodies(&self) -> bool {
        self.error_responses()
            .iter()
            .filter(|response| response.body.is_some())
            .nth(1)
            .is_some()
    }

    /// Whether a bodied streaming response (`text/event-stream` / `application/x-ndjson`) sits
    /// anywhere but the operation's single bodied success — the one position whose framing
    /// [`Self::stream_success`] consumes. Every other position decodes a whole body: an error
    /// response (an explicit non-2xx status, or a `default`, which is offered to the error side
    /// even when it is also the sole success source) is classified into `E`, and a success enum
    /// decodes each arm whole. A stream there would be read as one JSON document, so lowering
    /// rejects it (narrowed `E009`), as it rejects a streaming request body. A bodyless streaming
    /// response is never read, so it is not counted.
    pub(crate) fn stream_outside_single_success(&self) -> bool {
        let is_bodied_stream =
            |response: &&Response| response.stream.is_some() && response.body.is_some();
        let error_stream = self.error_responses().iter().any(is_bodied_stream);
        let success_multi_stream =
            self.multiple_success_bodies() && self.success_responses().iter().any(is_bodied_stream);
        error_stream || success_multi_stream
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

    /// Whether `default` is the operation's success source: exactly when `by_status` documents no
    /// success status — it is empty, or holds only non-2xx statuses such as `404`. The
    /// specification defines `default` as the documentation of every status not declared
    /// explicitly, so with no success status declared it is what documents a 2xx response. While
    /// any success status is declared, `default` stays on the error side alone, and satisfies no
    /// undeclared 2xx in any [`SuccessShape`] (issue #151). That is deliberate: the specification's
    /// own example pairs a `200` with a `default` "for others (implying an error)", so its body is
    /// in practice an error model — often one whose optional fields match any object, which would
    /// type a success silently as an error body — and one response object would then generate a
    /// success variant and an error variant at once. `Unit` and `Plain` name no status, so they take
    /// an undeclared 2xx as their one success; only `Enum` can tell it apart, and surfaces it as
    /// `UnexpectedStatus` with the body preserved, rather than guess.
    fn default_is_success_source(&self) -> bool {
        !self
            .by_status
            .iter()
            .any(|(status, _)| is_success_status(*status))
    }

    /// The operation's success responses in document order: the `default` response alone when no
    /// success status is declared (it is then the sole success), otherwise the 2xx entries.
    /// Mirrors the success/error split used by [`Self::success`].
    fn success_responses(&self) -> Vec<&Response> {
        if self.default_is_success_source() {
            return self.default.iter().collect();
        }
        self.by_status
            .iter()
            .filter(|(status, _)| is_success_status(*status))
            .map(|(_, response)| response)
            .collect()
    }

    /// The error shape of the operation. No bodied error entry yields `None`; one bodied entry
    /// *alone* yields the typed `E` body; anything more yields a per-operation error enum, sorted
    /// into classification precedence (exact code ascending, then range ascending, then
    /// [`StatusSpec::Default`] last) and carrying each bodyless error *entry* as a unit
    /// variant: two or more bodied entries, or one bodied entry beside a documented bodyless one
    /// (a bodied `404` beside a bodyless `403`, `5XX`, or `default`). A newtype over the one body
    /// has no way to represent the bodyless status, which would otherwise be classified as
    /// `UnexpectedStatus` — or, where the body's selector covers it (a `3XX` or `default` beside a
    /// bodyless `304`), have its empty body decoded as that model (issue #204). This is the
    /// success side's rule (issue #121) without its streaming exception, since lowering rejects a
    /// bodied stream on the error side.
    ///
    /// The entries are the lowered non-success statuses of `by_status` plus the
    /// [`StatusSpec::Default`] entry a declared `default` contributes, not everything the document
    /// declares: the count and the variants follow what lowering kept. `default` is *offered*
    /// here whenever it is declared — including when it is also the operation's sole success source (see
    /// [`Self::success`]), which then types both sides with that one body, as the specification
    /// does: `default` documents every undeclared status, of either class. (A bodied *streaming*
    /// `default` cannot be typed on both sides — the success side would stream and this side
    /// decode it whole — so lowering rejects it; see [`Self::stream_outside_single_success`].) With
    /// no bodied entry at all the shape is `None`, whatever bodyless entries are declared: every
    /// non-success status is then `UnexpectedStatus`, retaining at most `max_error_body` bytes of
    /// the body (see [`ErrorShape::None`]).
    pub(crate) fn error(&self) -> ErrorShape {
        let mut entries: Vec<(StatusSpec, Option<Ty>)> = Vec::new();
        for (status, response) in &self.by_status {
            if !is_success_status(*status) {
                entries.push((*status, response.body));
            }
        }
        if let Some(default) = &self.default {
            entries.push((StatusSpec::Default, default.body));
        }
        let into_enum = |mut entries: Vec<(StatusSpec, Option<Ty>)>| {
            entries.sort_by_key(|(status, _)| precedence_key(*status));
            ErrorShape::Enum(entries)
        };
        // One body beside a documented bodyless entry: two outcomes a newtype over the body cannot
        // tell apart, so the bodyless one gets its own unit variant.
        let bodied = entries.iter().filter(|(_, body)| body.is_some()).count();
        if bodied == 1 && entries.len() > 1 {
            return into_enum(entries);
        }
        finish_shape(entries, ErrorShape::None, ErrorShape::Single, into_enum)
    }
}

/// Collapse per-status entries into a response shape by counting how many carry a body: zero → the
/// `unit` shape, exactly one → the `single` shape over that lone body, two or more → the `multi`
/// shape over all entries (bodied and bodyless alike). Bodyless siblings of a lone body are not
/// modeled here: [`Responses::error`] promotes that case to its enum before calling this, so the
/// error side reaches `single` only with its one entry, and [`Responses::success`] does the same
/// except for a streaming body, which an empty sibling response satisfies as-is.
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

/// The deterministic decode-precedence sort key for a lowered status selector: exact codes first
/// (ascending), then ranges (ascending by leading digit), then [`StatusSpec::Default`] last. The
/// key is injective over selectors, so the sort is total exactly when the selectors within one
/// operation's entries are unique — which is a property of lowering, not of the document's map
/// keys: distinct keys stay distinct selectors only because the frontend admits no `Responses` key
/// outside the specification's grammar (on which parsing is injective), and lowering stores
/// `default` in its own field, so it contributes at most one entry.
fn precedence_key(status: StatusSpec) -> (u8, u16) {
    match status {
        StatusSpec::Exact(code) => (0, code),
        StatusSpec::Range(prefix) => (1, u16::from(prefix)),
        StatusSpec::Default => (2, 0),
    }
}

fn is_success_status(status: StatusSpec) -> bool {
    status.is_success()
}

/// The success return type of an operation (before wrapping in `ResponseValue<T>`). Generated code
/// enters the success branch on the raw transport status alone, and only [`SuccessShape::Enum`]
/// carries a status set to compare it against — [`SuccessShape::Unit`] and [`SuccessShape::Plain`]
/// name no status, so they draw no distinction between a documented 2xx and any other. `default`
/// reaches the success side only when `by_status` documents no success status; while it holds any
/// success entry, `default` is not among them and is no success fallback for a 2xx that matches
/// none of them — which is a fact about the lowered entries, not about what the document declares.
#[derive(Debug, Clone)]
pub(crate) enum SuccessShape {
    /// No success body.
    Unit,
    /// A single success body type.
    Plain(Ty),
    /// Two or more success `by_status` entries of which at least one carries a body: several
    /// bodies, or a single non-streaming body beside a documented bodyless status (a bodied `200`
    /// beside a bodyless `204` is `Status200(T)` and `Status204`, since a plain `T` would decode
    /// the `204`'s empty body as a malformed `T`). Generated as a per-operation
    /// response enum, one variant per entry — a payload-carrying variant for a bodied status, a
    /// unit variant for a bodyless one (e.g. `204`). Entries are the lowered *success* statuses
    /// only, not everything the document declares — `default` is never among them — pre-sorted
    /// into decode precedence (exact before range); decode dispatches by HTTP status in that
    /// order and rejects any other 2xx as `Error::UnexpectedStatus`.
    Enum(Vec<(StatusSpec, Option<Ty>)>),
}

/// The typed error body `E` of an operation (matrix: Responses).
#[derive(Debug, Clone)]
pub(crate) enum ErrorShape {
    /// Every lowered error entry is bodyless, or there is none: the entries are the non-success
    /// `by_status` statuses plus the [`StatusSpec::Default`] entry a present `default` contributes,
    /// which is a fact about the lowered entries, not about what the document declares. Every
    /// non-success status is then `UnexpectedStatus` with its status and headers, retaining at most
    /// `max_error_body` bytes of the body (silently truncated past that cap); a failed body read
    /// returns the read error instead.
    None,
    /// The body type of the operation's only lowered error entry, which carries it. Every other
    /// shape with a bodied error entry is an [`ErrorShape::Enum`].
    Single(Ty),
    /// Two or more entries of which at least one carries a body, counted over the non-success
    /// `by_status` entries plus the [`StatusSpec::Default`] entry a present `default` contributes:
    /// several bodies, or a single body beside a documented bodyless entry (a bodied `404` beside a
    /// bodyless `403` is `Status404(E)` and `Status403`, since a newtype over the `404` body has
    /// nowhere to put the `403`). Generated as a per-operation error enum, one variant per entry — a
    /// payload-carrying variant for a bodied status, a unit variant for a bodyless one. Entries are
    /// pre-sorted into classification precedence (exact before range; [`StatusSpec::Default`]
    /// last); classification dispatches by HTTP status in that order.
    Enum(Vec<(StatusSpec, Option<Ty>)>),
}

/// How an operation's generated error type implements the runtime's `ApiErrorBody`. Decided here
/// once, so codegen (which emits the impl) and `surface` (which reports gaining or losing it)
/// cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiErrorBodyImpl {
    /// The uninhabited `Infallible` shape: `Body = Infallible`, and a body is never present.
    Uninhabited,
    /// `Body` is this type, unboxed and non-nullable: the `&T` the accessor hands back.
    Body(Ty),
}

impl ErrorShape {
    /// How the generated error type implements `ApiErrorBody`, or `None` when it does not: an enum
    /// whose bodied statuses carry different generated types has no single body to hand back.
    /// Multi-status payloads are uniformly boxed at emission and a variant's nullability is
    /// absorbed by the accessor, so each body is compared bare; `Body` is the first bodied status's
    /// type in classification precedence, which names the same Rust type as every other.
    pub(crate) fn api_error_body(&self, types: &TypeGraph) -> Option<ApiErrorBodyImpl> {
        let bare = |ty: Ty| Ty {
            nullable: false,
            boxed: false,
            ..ty
        };
        match self {
            ErrorShape::None => Some(ApiErrorBodyImpl::Uninhabited),
            ErrorShape::Single(ty) => Some(ApiErrorBodyImpl::Body(bare(*ty))),
            ErrorShape::Enum(entries) => {
                let mut bodies = entries.iter().filter_map(|(_, body)| body.map(bare));
                let first = bodies.next()?;
                bodies
                    .all(|body| types.same_generated_type(first, body))
                    .then_some(ApiErrorBodyImpl::Body(first))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiErrorBodyImpl, ErrorShape, Response, Responses, StatusSpec, SuccessShape, Ty};
    use crate::diag::{JsonPointer, Provenance};
    use crate::ir::{
        AdditionalProps, Docs, Openness, Prim, ScalarEnum, ScalarRepr, ScalarValue, Struct,
        TypeDef, TypeGraph, TypeId, TypeKind,
    };

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
            headers: Vec::new(),
        }
    }

    fn stream_resp(body: Option<u32>) -> Response {
        Response {
            media: Some(super::MediaType::EventStream),
            body: body.map(ty),
            stream: Some(super::Framing::Sse),
            headers: Vec::new(),
        }
    }

    #[test]
    fn a_bodied_stream_is_admitted_only_as_the_single_bodied_success() {
        let case = |by_status: Vec<(StatusSpec, Response)>, default: Option<Response>| {
            Responses { by_status, default }.stream_outside_single_success()
        };
        // Supported: the single bodied success, beside a bodyless success and whole-body errors.
        assert!(!case(
            vec![
                (StatusSpec::Exact(200), stream_resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
                (StatusSpec::Exact(404), resp(Some(2))),
            ],
            Some(resp(Some(3))),
        ));
        // A bodyless stream is never read, wherever it sits.
        assert!(!case(
            vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Range(4), stream_resp(None)),
            ],
            None,
        ));
        // A lone streaming `default` is the success source and also offered to the error side.
        assert!(case(Vec::new(), Some(stream_resp(Some(1)))));
        // So is one beside only non-2xx statuses.
        assert!(case(
            vec![(StatusSpec::Exact(404), resp(None))],
            Some(stream_resp(Some(1))),
        ));
        // A streaming error status, and a streaming `default` beside a declared success.
        assert!(case(
            vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Range(4), stream_resp(Some(2))),
            ],
            None,
        ));
        assert!(case(
            vec![(StatusSpec::Exact(200), resp(Some(1)))],
            Some(stream_resp(Some(2))),
        ));
        // A stream in a multi-status success enum.
        assert!(case(
            vec![
                (StatusSpec::Exact(200), stream_resp(Some(1))),
                (StatusSpec::Exact(202), resp(Some(2))),
            ],
            None,
        ));
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
        // exact < range < default. A `Range(0)`, which the frontend never admits, is included to
        // pin that a range prefix of `0` is an ordinary range here, sorted among the ranges and
        // distinct from the `default` entry (issue #233: it once *was* the `default` entry).
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Range(4), resp(Some(1))),
                (StatusSpec::Exact(409), resp(Some(2))),
                (StatusSpec::Range(0), resp(Some(4))),
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
                        StatusSpec::Range(0),
                        StatusSpec::Range(4),
                        StatusSpec::Default,
                    ]
                );
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn success_enum_sorts_exact_codes_ascending_whatever_the_document_order() {
        // Issue #138: a key that orders only by class (exact, range, default) is a stable sort
        // that keeps document order within each class, so only exacts listed out of order can
        // observe "exact code ascending"; this case states it on purpose rather than leaving it
        // to a fixture that happens to list a `204` before a `200`. (The success side can hold at most one
        // range — `2XX` is the only success selector the frontend admits — so range order is
        // pinned on the error side below.)
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Range(2), resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
                (StatusSpec::Exact(201), resp(Some(2))),
                (StatusSpec::Exact(200), resp(Some(3))),
            ],
            default: None,
        };
        match responses.success() {
            SuccessShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![
                    StatusSpec::Exact(200),
                    StatusSpec::Exact(201),
                    StatusSpec::Exact(204),
                    StatusSpec::Range(2),
                ]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn error_enum_sorts_exact_codes_then_ranges_ascending_whatever_the_document_order() {
        // Issue #138: exacts and ranges both in descending document order, `default` first. The
        // error side is where two ranges share one sort (`4XX` and `5XX` are both error
        // selectors), so this is the only place "range ascending" is observable.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Range(5), resp(Some(1))),
                (StatusSpec::Range(4), resp(Some(2))),
                (StatusSpec::Exact(409), resp(None)),
                (StatusSpec::Exact(404), resp(Some(3))),
            ],
            default: Some(resp(Some(4))),
        };
        match responses.error() {
            ErrorShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![
                    StatusSpec::Exact(404),
                    StatusSpec::Exact(409),
                    StatusSpec::Range(4),
                    StatusSpec::Range(5),
                    StatusSpec::Default,
                ]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn single_bodied_success_with_bodyless_sibling_is_an_enum() {
        // Issue #121: the common `T`-plus-`204` case is two documented outcomes. A plain `T` would
        // decode the `204`'s empty body as a malformed `T`, so the bodyless status gets its own
        // unit variant — whether the sibling is exact or a range, and on either side of the body.
        for (by_status, expected) in [
            (
                vec![
                    (StatusSpec::Exact(204), resp(None)),
                    (StatusSpec::Exact(200), resp(Some(1))),
                ],
                vec![
                    (StatusSpec::Exact(200), true),
                    (StatusSpec::Exact(204), false),
                ],
            ),
            (
                vec![
                    (StatusSpec::Range(2), resp(Some(1))),
                    (StatusSpec::Exact(204), resp(None)),
                ],
                vec![
                    (StatusSpec::Exact(204), false),
                    (StatusSpec::Range(2), true),
                ],
            ),
            (
                vec![
                    (StatusSpec::Exact(200), resp(Some(1))),
                    (StatusSpec::Range(2), resp(None)),
                ],
                vec![
                    (StatusSpec::Exact(200), true),
                    (StatusSpec::Range(2), false),
                ],
            ),
        ] {
            let responses = Responses {
                by_status,
                default: Some(resp(Some(2))),
            };
            match responses.success() {
                SuccessShape::Enum(entries) => {
                    let shape: Vec<_> = entries.iter().map(|(s, b)| (*s, b.is_some())).collect();
                    assert_eq!(shape, expected);
                }
                other => panic!("expected Enum, got {other:?}"),
            }
            // Still one body: the XML and streaming limits on a *second* body do not apply.
            assert!(!responses.multiple_success_bodies());
        }

        // A lone bodied success, and a bodied success beside only error statuses, stay plain.
        for by_status in [
            vec![(StatusSpec::Exact(200), resp(Some(1)))],
            vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(404), resp(None)),
            ],
        ] {
            let responses = Responses {
                by_status,
                default: Some(resp(None)),
            };
            assert!(matches!(responses.success(), SuccessShape::Plain(_)));
        }
    }

    #[test]
    fn every_bodyless_entry_beside_one_body_is_its_own_unit_variant() {
        // Issue #211: "each bodyless entry" in the plural. Two bodyless entries beside one body
        // are two unit variants on either side, neither merged into the other nor dropped, and
        // they sort among the bodied entry by precedence like any other entry.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(205), resp(None)),
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
                (StatusSpec::Range(5), resp(None)),
                (StatusSpec::Exact(404), resp(Some(2))),
                (StatusSpec::Exact(403), resp(None)),
            ],
            default: None,
        };
        match responses.success() {
            SuccessShape::Enum(entries) => {
                let shape: Vec<_> = entries
                    .iter()
                    .map(|(status, body)| (*status, body.map(|body| body.id.0)))
                    .collect();
                assert_eq!(
                    shape,
                    vec![
                        (StatusSpec::Exact(200), Some(1)),
                        (StatusSpec::Exact(204), None),
                        (StatusSpec::Exact(205), None),
                    ]
                );
            }
            other => panic!("expected Enum, got {other:?}"),
        }
        assert!(!responses.multiple_success_bodies());
        assert_eq!(
            shape(responses.error()),
            Shape::Enum(vec![
                (StatusSpec::Exact(403), None),
                (StatusSpec::Exact(404), Some(2)),
                (StatusSpec::Range(5), None),
            ])
        );
        assert!(!responses.multiple_error_bodies());
    }

    #[test]
    fn a_streaming_body_beside_a_bodyless_sibling_stays_plain() {
        // An empty body is a well-formed empty stream, so the `204` needs no variant of its own;
        // the stream stays the operation's single success body and `EventStream<T>`.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), stream_resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            default: None,
        };
        assert!(matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
        assert!(responses.stream_success().is_some());
        assert!(!responses.stream_outside_single_success());
    }

    #[test]
    fn a_lone_xml_body_beside_a_bodyless_sibling_is_not_a_multi_body_xml_enum() {
        let xml = |body: Option<u32>| Response {
            media: body.map(|_| super::MediaType::Xml),
            ..resp(body)
        };
        // One XML body beside a bodyless `204`: an enum, but still one body to decode as XML.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), xml(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            default: None,
        };
        assert!(matches!(responses.success(), SuccessShape::Enum(_)));
        assert!(!responses.xml_in_multi_status());
        // A second bodied success beside the XML one is still the rejected combination.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), xml(Some(1))),
                (StatusSpec::Exact(201), resp(Some(2))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            default: None,
        };
        assert!(responses.xml_in_multi_status());
    }

    #[test]
    fn only_2xx_selectors_are_success_and_every_other_range_prefix_is_error() {
        // Issue #198: the predicate that splits every generated shape. The emitted method enters
        // its success branch on the transport's `StatusCode::is_success()` — exactly 200..=299 —
        // so this must agree with it at every code RFC 9110 defines a class for, and on every range.
        for code in 100..=599u16 {
            assert_eq!(
                StatusSpec::Exact(code).is_success(),
                (200..=299).contains(&code),
                "exact status {code}",
            );
        }
        for (prefix, success) in [(1, false), (2, true), (3, false), (4, false), (5, false)] {
            assert_eq!(
                StatusSpec::Range(prefix).is_success(),
                success,
                "range {prefix}XX"
            );
        }
        // `default` is never a success selector; it reaches the success side only through
        // `default_is_success_source`.
        assert!(!StatusSpec::Default.is_success());
    }

    /// The one prose spelling of a selector, which the generated error `Display` and
    /// `spargen diff`'s labels share: the Responses Object key, `XX` upper-case.
    #[test]
    fn a_selector_displays_as_its_responses_object_key() {
        assert_eq!(StatusSpec::Exact(404).display_label(), "404");
        assert_eq!(StatusSpec::Range(5).display_label(), "5XX");
        assert_eq!(StatusSpec::Range(0).display_label(), "0XX");
        assert_eq!(StatusSpec::Default.display_label(), "default");
    }

    #[test]
    fn a_documented_304_or_3xx_is_on_the_error_side_and_promotes_no_success_enum() {
        // A bodied `200` beside a bodyless `304`: the `304` is no success entry, so the lone body
        // stays plain `T` (a bodyless 2xx sibling would have made it an enum), and the `304` is an
        // error-side entry — a unit variant of the error enum beside one bodied error or several.
        let single = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(304), resp(None)),
                (StatusSpec::Exact(404), resp(Some(2))),
            ],
            default: None,
        };
        assert!(matches!(single.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
        assert_eq!(
            shape(single.error()),
            Shape::Enum(vec![
                (StatusSpec::Exact(304), None),
                (StatusSpec::Exact(404), Some(2)),
            ])
        );

        let multi = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Range(3), resp(None)),
                (StatusSpec::Exact(304), resp(None)),
                (StatusSpec::Exact(404), resp(Some(2))),
                (StatusSpec::Range(5), resp(Some(3))),
            ],
            default: None,
        };
        assert!(matches!(multi.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
        match multi.error() {
            ErrorShape::Enum(entries) => {
                assert_eq!(
                    statuses(&entries),
                    vec![
                        StatusSpec::Exact(304),
                        StatusSpec::Exact(404),
                        StatusSpec::Range(3),
                        StatusSpec::Range(5),
                    ],
                );
                assert!(entries[0].1.is_none() && entries[2].1.is_none());
            }
            other => panic!("expected an error enum, got {other:?}"),
        }

        // Only a `304` and a `default` documented: no success status is declared, so `default`
        // is the success source, and the `304` does not claim it. On the error side the bodyless
        // `304` is its own unit variant ahead of `default` (issue #204): as a `Single(default)` it
        // was dropped, and a real `304` matched `default` and had its empty body decoded.
        let conditional = Responses {
            by_status: vec![(StatusSpec::Exact(304), resp(None))],
            default: Some(resp(Some(1))),
        };
        assert!(matches!(conditional.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
        assert_eq!(
            shape(conditional.error()),
            Shape::Enum(vec![
                (StatusSpec::Exact(304), None),
                (StatusSpec::Default, Some(1)),
            ])
        );
    }

    /// The statuses of an enum shape, in the order it holds them.
    fn statuses(entries: &[(StatusSpec, Option<Ty>)]) -> Vec<StatusSpec> {
        entries.iter().map(|(status, _)| *status).collect()
    }

    #[test]
    fn default_beside_multiple_bodied_successes_types_only_the_error_side() {
        // Two bodied successes and a bodied `default`: the success enum holds exactly the two 2xx
        // entries — `default` is no success variant and no fallback for another 2xx — while the
        // error side is the `default` body alone, so it is `Single`, not an enum.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(201), resp(Some(2))),
                (StatusSpec::Exact(200), resp(Some(1))),
            ],
            default: Some(resp(Some(3))),
        };
        match responses.success() {
            SuccessShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![StatusSpec::Exact(200), StatusSpec::Exact(201)]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }
        match responses.error() {
            ErrorShape::Single(body) => assert_eq!(body.id, TypeId(3)),
            other => panic!("expected Single, got {other:?}"),
        }
        assert_eq!(responses.single_success_media(), None);
        assert_eq!(responses.single_error_media(), Some(super::MediaType::Json));

        // With a bodied error status beside it, `default` becomes the last error variant — and
        // still never a success one.
        let responses = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(201), resp(Some(2))),
                (StatusSpec::Exact(404), resp(Some(4))),
            ],
            default: Some(resp(Some(3))),
        };
        match responses.success() {
            SuccessShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![StatusSpec::Exact(200), StatusSpec::Exact(201)]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }
        match responses.error() {
            ErrorShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![StatusSpec::Exact(404), StatusSpec::Default]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn a_bodyless_default_is_the_trailing_unit_error_variant_beside_any_error_body() {
        // A bodyless `default` beside one bodied error makes the shape an enum (issue #204), as it
        // does beside two, and is its trailing unit variant either way.
        let single = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(404), resp(Some(2))),
            ],
            default: Some(resp(None)),
        };
        assert_eq!(
            shape(single.error()),
            Shape::Enum(vec![
                (StatusSpec::Exact(404), Some(2)),
                (StatusSpec::Default, None),
            ])
        );

        let multi = Responses {
            by_status: vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Exact(404), resp(Some(2))),
                (StatusSpec::Exact(409), resp(Some(3))),
            ],
            default: Some(resp(None)),
        };
        match multi.error() {
            ErrorShape::Enum(entries) => {
                assert_eq!(
                    statuses(&entries),
                    vec![
                        StatusSpec::Exact(404),
                        StatusSpec::Exact(409),
                        StatusSpec::Default,
                    ]
                );
                assert!(
                    entries[2].1.is_none(),
                    "the bodyless default is a unit variant"
                );
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    /// An error shape reduced to comparable data: bodies by type id, enum entries in order.
    #[derive(Debug, PartialEq)]
    enum Shape {
        None,
        Single(u32),
        Enum(Vec<(StatusSpec, Option<u32>)>),
    }

    fn shape(error: ErrorShape) -> Shape {
        match error {
            ErrorShape::None => Shape::None,
            ErrorShape::Single(body) => Shape::Single(body.id.0),
            ErrorShape::Enum(entries) => Shape::Enum(
                entries
                    .into_iter()
                    .map(|(status, body)| (status, body.map(|body| body.id.0)))
                    .collect(),
            ),
        }
    }

    #[test]
    fn the_error_shape_grid_over_bodied_errors_and_default_body_presence() {
        // Issue #127: every cell of bodied non-default errors (0 / 1 / 2) x `default` (absent /
        // bodied / bodyless), each with and without a bodyless error status beside them. With no
        // body the shape is `None`; a lone bodied entry is `Single`; anything else is an `Enum`
        // with every declared error entry a variant, so a bodyless `default` or `403` beside one
        // body is a unit variant rather than dropped (issue #204). `codegen`'s classification must
        // agree cell for cell (`e2e.rs`, `a_bodyless_error_entry_beside_one_error_body_is_its_own_variant`).
        const DEFAULT: u32 = 9;
        let (e404, e409) = (StatusSpec::Exact(404), StatusSpec::Exact(409));
        let (e403, any) = (StatusSpec::Exact(403), StatusSpec::Default);
        for bodyless_sibling in [false, true] {
            let sibling = || bodyless_sibling.then_some((e403, None));
            // The shape one body takes: `Single` alone, an enum beside the bodyless `403`.
            let lone = |status: StatusSpec, body: u32| {
                if bodyless_sibling {
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(status, Some(body))])
                            .collect(),
                    )
                } else {
                    Shape::Single(body)
                }
            };
            let cells: [(usize, Option<Option<u32>>, Shape); 9] = [
                (0, None, Shape::None),
                (0, Some(Some(DEFAULT)), lone(any, DEFAULT)),
                (0, Some(None), Shape::None),
                (1, None, lone(e404, 4)),
                (
                    1,
                    Some(Some(DEFAULT)),
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(e404, Some(4)), (any, Some(DEFAULT))])
                            .collect(),
                    ),
                ),
                (
                    1,
                    Some(None),
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(e404, Some(4)), (any, None)])
                            .collect(),
                    ),
                ),
                (
                    2,
                    None,
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(e404, Some(4)), (e409, Some(5))])
                            .collect(),
                    ),
                ),
                (
                    2,
                    Some(Some(DEFAULT)),
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(e404, Some(4)), (e409, Some(5)), (any, Some(DEFAULT))])
                            .collect(),
                    ),
                ),
                (
                    2,
                    Some(None),
                    Shape::Enum(
                        sibling()
                            .into_iter()
                            .chain([(e404, Some(4)), (e409, Some(5)), (any, None)])
                            .collect(),
                    ),
                ),
            ];
            for (bodied_errors, default, expected) in cells {
                // Document order is deliberately not precedence order: 409 before 404, and the
                // bodyless `403` last, so the enum cells also pin the sort.
                let mut by_status = vec![(StatusSpec::Exact(200), resp(Some(1)))];
                by_status.extend(
                    [(e409, resp(Some(5))), (e404, resp(Some(4)))]
                        .into_iter()
                        .skip(2 - bodied_errors),
                );
                if bodyless_sibling {
                    by_status.push((e403, resp(None)));
                }
                let responses = Responses {
                    by_status,
                    default: default.map(resp),
                };
                assert_eq!(
                    shape(responses.error()),
                    expected,
                    "{bodied_errors} bodied error(s), default {default:?}, \
                     bodyless 403: {bodyless_sibling}"
                );
            }
        }
    }

    #[test]
    fn a_default_with_no_explicit_status_is_both_the_success_and_the_error_body() {
        // `by_status` empty: the early return makes `default` the sole success source, and the
        // error side is still offered it as `StatusSpec::Default`, so one body types both sides.
        let bodied = Responses {
            by_status: Vec::new(),
            default: Some(resp(Some(7))),
        };
        assert!(matches!(bodied.success(), SuccessShape::Plain(body) if body.id == TypeId(7)));
        assert!(matches!(bodied.error(), ErrorShape::Single(body) if body.id == TypeId(7)));
        assert_eq!(bodied.single_success_media(), Some(super::MediaType::Json));
        assert_eq!(bodied.single_error_media(), Some(super::MediaType::Json));

        // A bodyless sole `default` types neither side.
        let bodyless = Responses {
            by_status: Vec::new(),
            default: Some(resp(None)),
        };
        assert!(matches!(bodyless.success(), SuccessShape::Unit));
        assert!(matches!(bodyless.error(), ErrorShape::None));

        // And no responses at all is `Unit` / `None`.
        let empty = Responses {
            by_status: Vec::new(),
            default: None,
        };
        assert!(matches!(empty.success(), SuccessShape::Unit));
        assert!(matches!(empty.error(), ErrorShape::None));
    }

    #[test]
    fn default_documents_the_success_side_when_no_success_status_is_declared() {
        // `404` plus a bodied `default` (issue #115): `default` is the only documentation of a 2xx,
        // so it is the success body — not `Unit`, which would accept any 2xx as `()` and discard
        // the body — and it stays the error side's catch-all beside the `404`.
        let responses = Responses {
            by_status: vec![(StatusSpec::Exact(404), resp(Some(1)))],
            default: Some(resp(Some(2))),
        };
        assert!(matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(2)));
        assert_eq!(
            responses.single_success_media(),
            Some(super::MediaType::Json)
        );
        match responses.error() {
            ErrorShape::Enum(entries) => assert_eq!(
                statuses(&entries),
                vec![StatusSpec::Exact(404), StatusSpec::Default]
            ),
            other => panic!("expected Enum, got {other:?}"),
        }

        // Every non-2xx status class counts as "no success declared", ranges included.
        for status in [
            StatusSpec::Exact(304),
            StatusSpec::Range(4),
            StatusSpec::Range(5),
        ] {
            let responses = Responses {
                by_status: vec![(status, resp(None))],
                default: Some(resp(Some(2))),
            };
            assert!(
                matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(2)),
                "{status:?}"
            );
        }

        // A bodyless `default` documents a bodyless 2xx: `Unit` is then the documented shape.
        let responses = Responses {
            by_status: vec![(StatusSpec::Exact(404), resp(Some(1)))],
            default: Some(resp(None)),
        };
        assert!(matches!(responses.success(), SuccessShape::Unit));
        assert_eq!(responses.single_success_media(), None);
    }

    #[test]
    fn a_declared_success_status_keeps_default_off_the_success_side() {
        // Any 2xx entry, bodied or not, exact or range, is the success documentation; `default`
        // then stays on the error side alone.
        for (status, body) in [
            (StatusSpec::Exact(204), None),
            (StatusSpec::Range(2), None),
            (StatusSpec::Exact(200), Some(1)),
        ] {
            let responses = Responses {
                by_status: vec![
                    (status, resp(body)),
                    (StatusSpec::Exact(404), resp(Some(3))),
                ],
                default: Some(resp(Some(2))),
            };
            match (body, responses.success()) {
                (None, SuccessShape::Unit) => {}
                (Some(_), SuccessShape::Plain(ty)) => assert_eq!(ty.id, TypeId(1)),
                (_, other) => panic!("{status:?}: unexpected success shape {other:?}"),
            }
            assert_eq!(
                responses.single_success_media(),
                body.map(|_| super::MediaType::Json)
            );
        }
    }

    /// A graph whose ids are the positions of `kinds`.
    fn graph(kinds: Vec<TypeKind>) -> TypeGraph {
        let mut graph = TypeGraph::default();
        for kind in kinds {
            graph.insert(TypeDef {
                name_hint: String::new(),
                kind,
                docs: Docs::default(),
                provenance: Provenance::new(JsonPointer::root(), None),
                document: String::new(),
            });
        }
        graph
    }

    /// A multi-status error shape over exact statuses, in the order given.
    fn error_enum(entries: Vec<(u16, Option<Ty>)>) -> ErrorShape {
        ErrorShape::Enum(
            entries
                .into_iter()
                .map(|(code, body)| (StatusSpec::Exact(code), body))
                .collect(),
        )
    }

    fn object() -> TypeKind {
        TypeKind::Struct(Struct {
            fields: Vec::new(),
            additional: AdditionalProps::Allow,
        })
    }

    fn int_enum() -> TypeKind {
        TypeKind::Enum(ScalarEnum {
            repr: ScalarRepr::Int,
            variants: vec![ScalarValue::Int(1)],
            openness: Openness::Closed,
        })
    }

    fn two_bodies(graph: &TypeGraph, a: Ty, b: Ty) -> Option<ApiErrorBodyImpl> {
        error_enum(vec![(404, Some(a)), (409, Some(b))]).api_error_body(graph)
    }

    #[test]
    fn bodies_referencing_one_definition_share_it() {
        let graph = graph(vec![object()]);
        assert_eq!(
            two_bodies(&graph, ty(0), ty(0)),
            Some(ApiErrorBodyImpl::Body(ty(0)))
        );
    }

    #[test]
    fn distinct_definitions_generating_one_rust_type_share_it() {
        // Two string schemas (a `$ref` component and an inline one) are both `String`.
        let strings = graph(vec![
            TypeKind::Primitive(Prim::String),
            TypeKind::Primitive(Prim::String),
        ]);
        assert_eq!(
            two_bodies(&strings, ty(0), ty(1)),
            Some(ApiErrorBodyImpl::Body(ty(0)))
        );
        // An integer enum is a `pub type X = i64` alias, so it is an `i64` body, either way round.
        let ints = graph(vec![int_enum(), TypeKind::Primitive(Prim::I64)]);
        assert!(two_bodies(&ints, ty(0), ty(1)).is_some());
        assert!(two_bodies(&ints, ty(1), ty(0)).is_some());
        // Containers compare their items, and a `$ref` cycle between arrays still terminates.
        let arrays = graph(vec![
            TypeKind::Primitive(Prim::String),
            TypeKind::Primitive(Prim::String),
            TypeKind::Array(Box::new(ty(0))),
            TypeKind::Array(Box::new(ty(1))),
            TypeKind::Array(Box::new(ty(5))),
            TypeKind::Array(Box::new(ty(4))),
        ]);
        assert!(two_bodies(&arrays, ty(2), ty(3)).is_some());
        assert!(two_bodies(&arrays, ty(4), ty(5)).is_some());
    }

    #[test]
    fn different_rust_types_share_nothing() {
        let scalars = graph(vec![
            TypeKind::Primitive(Prim::I32),
            TypeKind::Primitive(Prim::I64),
        ]);
        assert_eq!(two_bodies(&scalars, ty(0), ty(1)), None);
        // Each struct (or `Never`) definition is its own nominal item, however alike.
        let nominal = graph(vec![object(), object(), TypeKind::Never, TypeKind::Never]);
        assert_eq!(two_bodies(&nominal, ty(0), ty(1)), None);
        assert_eq!(two_bodies(&nominal, ty(2), ty(3)), None);
        // Tuple items keep their `Box`, so an item boxed on one side is a different Rust type.
        let boxed_item = Ty {
            boxed: true,
            ..ty(0)
        };
        let tuples = graph(vec![
            TypeKind::Primitive(Prim::String),
            TypeKind::Tuple(vec![ty(0)]),
            TypeKind::Tuple(vec![boxed_item]),
            TypeKind::Tuple(vec![ty(0)]),
        ]);
        assert_eq!(two_bodies(&tuples, ty(1), ty(2)), None);
        assert!(two_bodies(&tuples, ty(1), ty(3)).is_some());
    }

    #[test]
    fn per_status_nullability_and_bodyless_statuses_do_not_split_the_body() {
        let graph = graph(vec![object()]);
        let nullable = Ty {
            nullable: true,
            ..ty(0)
        };
        let shape = error_enum(vec![(401, None), (404, Some(nullable)), (409, Some(ty(0)))]);
        // `Body` is the bare definition: the per-variant `Option` is absorbed by the accessor.
        assert_eq!(
            shape.api_error_body(&graph),
            Some(ApiErrorBodyImpl::Body(ty(0)))
        );
    }

    #[test]
    fn the_single_and_uninhabited_shapes_always_implement_it() {
        let graph = graph(vec![object()]);
        assert_eq!(
            ErrorShape::None.api_error_body(&graph),
            Some(ApiErrorBodyImpl::Uninhabited)
        );
        let nullable = Ty {
            nullable: true,
            ..ty(0)
        };
        assert_eq!(
            ErrorShape::Single(nullable).api_error_body(&graph),
            Some(ApiErrorBodyImpl::Body(ty(0)))
        );
    }

    /// `api_error_body` bares its inputs before comparing, so it never reaches the nullability
    /// check itself; call `same_generated_type` directly. `T` and `Option<T>` over one definition
    /// are different Rust types, at the top level and as an array item.
    #[test]
    fn nullability_distinguishes_otherwise_identical_types() {
        let nullable = Ty {
            nullable: true,
            ..ty(0)
        };
        let graph = graph(vec![
            TypeKind::Primitive(Prim::String),
            TypeKind::Array(Box::new(ty(0))),
            TypeKind::Array(Box::new(nullable)),
        ]);
        assert!(!graph.same_generated_type(ty(0), nullable));
        assert!(!graph.same_generated_type(nullable, ty(0)));
        assert!(graph.same_generated_type(nullable, nullable));
        // `Vec<T>` against `Vec<Option<T>>`.
        assert!(!graph.same_generated_type(ty(1), ty(2)));
    }

    /// The rules [`Responses::success`] and [`Responses::error`] follow, checked over random
    /// status sets rather than chosen examples (issue #477). Each case draws unique exact codes
    /// (biased toward 2xx) and ranges `1XX`..`5XX`, each bodied or bodyless and streaming or not,
    /// plus an absent, bodied, or bodyless `default`. Every body carries its own type id — the
    /// `by_status` position plus one, or [`DEFAULT_BODY`] — so a reduced shape says which response
    /// each body came from. The oracles are written from the documented contract, not from the
    /// implementation: a model of both shapes, the partition of declared statuses between the two
    /// sides, where `default` lands, and independence from `by_status` insertion order.
    mod partition_props {
        use super::{resp, shape, stream_resp, Shape};
        use crate::ir::media::{Response, Responses, StatusSpec, SuccessShape};
        use proptest::prelude::*;

        /// The body type id a bodied `default` carries, distinct from every `by_status` id.
        const DEFAULT_BODY: u32 = 1000;

        fn success_shape(success: SuccessShape) -> Shape {
            match success {
                SuccessShape::Unit => Shape::None,
                SuccessShape::Plain(body) => Shape::Single(body.id.0),
                SuccessShape::Enum(entries) => Shape::Enum(
                    entries
                        .into_iter()
                        .map(|(status, body)| (status, body.map(|body| body.id.0)))
                        .collect(),
                ),
            }
        }

        /// A 2xx selector, stated from RFC 9110's classes rather than through `is_success`.
        fn is_2xx(status: StatusSpec) -> bool {
            matches!(status, StatusSpec::Exact(200..=299) | StatusSpec::Range(2))
        }

        /// Decode precedence, built by concatenation rather than a sort key: exact codes
        /// ascending, then ranges ascending, then `default`.
        fn in_precedence(
            entries: Vec<(StatusSpec, Option<u32>)>,
        ) -> Vec<(StatusSpec, Option<u32>)> {
            let mut exact: Vec<_> = entries
                .iter()
                .filter_map(|&(status, body)| match status {
                    StatusSpec::Exact(code) => Some((code, body)),
                    _ => None,
                })
                .collect();
            exact.sort_unstable_by_key(|&(code, _)| code);
            let mut ranges: Vec<_> = entries
                .iter()
                .filter_map(|&(status, body)| match status {
                    StatusSpec::Range(prefix) => Some((prefix, body)),
                    _ => None,
                })
                .collect();
            ranges.sort_unstable_by_key(|&(prefix, _)| prefix);
            let defaults = entries
                .iter()
                .filter(|(status, _)| *status == StatusSpec::Default)
                .copied();
            exact
                .into_iter()
                .map(|(code, body)| (StatusSpec::Exact(code), body))
                .chain(
                    ranges
                        .into_iter()
                        .map(|(prefix, body)| (StatusSpec::Range(prefix), body)),
                )
                .chain(defaults)
                .collect()
        }

        /// The documented success shape: with no 2xx declared, `default`'s body (or `()`);
        /// otherwise the 2xx entries by body count — none is `()`, one alone (or one streaming
        /// body beside bodyless siblings) is plain, anything else is the sorted enum.
        fn expected_success(responses: &Responses) -> Shape {
            let entries: Vec<(StatusSpec, Option<u32>, bool)> = responses
                .by_status
                .iter()
                .filter(|(status, _)| is_2xx(*status))
                .map(|(status, response)| {
                    (
                        *status,
                        response.body.map(|body| body.id.0),
                        response.stream.is_some(),
                    )
                })
                .collect();
            if entries.is_empty() {
                return match responses.default.as_ref().and_then(|default| default.body) {
                    Some(body) => Shape::Single(body.id.0),
                    None => Shape::None,
                };
            }
            let bodied: Vec<_> = entries
                .iter()
                .filter(|(_, body, _)| body.is_some())
                .collect();
            match bodied.as_slice() {
                [] => Shape::None,
                [(_, Some(body), _)] if entries.len() == 1 => Shape::Single(*body),
                [(_, Some(body), true)] => Shape::Single(*body),
                _ => Shape::Enum(in_precedence(
                    entries
                        .into_iter()
                        .map(|(status, body, _)| (status, body))
                        .collect(),
                )),
            }
        }

        /// The documented error shape: the non-2xx entries plus a declared `default`, by body
        /// count — none is `None`, one entry carrying the only body is `Single`, anything else is
        /// the sorted enum.
        fn expected_error(responses: &Responses) -> Shape {
            let mut entries: Vec<(StatusSpec, Option<u32>)> = responses
                .by_status
                .iter()
                .filter(|(status, _)| !is_2xx(*status))
                .map(|(status, response)| (*status, response.body.map(|body| body.id.0)))
                .collect();
            if let Some(default) = &responses.default {
                entries.push((StatusSpec::Default, default.body.map(|body| body.id.0)));
            }
            match entries.as_slice() {
                [] => Shape::None,
                [(_, Some(body))] => Shape::Single(*body),
                _ if entries.iter().all(|(_, body)| body.is_none()) => Shape::None,
                _ => Shape::Enum(in_precedence(entries)),
            }
        }

        /// The body type ids a reduced shape carries.
        fn bodies(shape: &Shape) -> Vec<u32> {
            match shape {
                Shape::None => Vec::new(),
                Shape::Single(body) => vec![*body],
                Shape::Enum(entries) => entries.iter().filter_map(|(_, body)| *body).collect(),
            }
        }

        fn response(id: u32, bodied: bool, streaming: bool) -> Response {
            let body = bodied.then_some(id);
            if streaming {
                stream_resp(body)
            } else {
                resp(body)
            }
        }

        /// A case in document order, the same `by_status` entries in a shuffled order, and its
        /// `default`.
        fn arb_case() -> impl Strategy<Value = (Responses, Responses)> {
            let exact =
                proptest::collection::btree_set(prop_oneof![200u16..=206, 100u16..=599], 0..6);
            let ranges = proptest::collection::btree_set(1u8..=5, 0..3);
            (exact, ranges)
                .prop_map(|(exact, ranges)| {
                    exact
                        .into_iter()
                        .map(StatusSpec::Exact)
                        .chain(ranges.into_iter().map(StatusSpec::Range))
                        .collect::<Vec<_>>()
                })
                .prop_flat_map(|statuses| {
                    let flags =
                        proptest::collection::vec((any::<bool>(), any::<bool>()), statuses.len());
                    (
                        Just(statuses),
                        flags,
                        proptest::option::of((any::<bool>(), any::<bool>())),
                    )
                })
                .prop_flat_map(|(statuses, flags, default)| {
                    let by_status: Vec<(StatusSpec, Response)> = statuses
                        .into_iter()
                        .zip(flags)
                        .zip(1u32..)
                        .map(|((status, (bodied, streaming)), id)| {
                            (status, response(id, bodied, streaming))
                        })
                        .collect();
                    let default = default
                        .map(|(bodied, streaming)| response(DEFAULT_BODY, bodied, streaming));
                    (
                        Just(by_status.clone()),
                        Just(by_status).prop_shuffle(),
                        Just(default),
                    )
                })
                .prop_map(|(by_status, shuffled, default)| {
                    (
                        Responses {
                            by_status,
                            default: default.clone(),
                        },
                        Responses {
                            by_status: shuffled,
                            default,
                        },
                    )
                })
        }

        proptest! {
            /// The body count picks unit, single, or enum on each side, and an enum lists that
            /// side's entries in decode precedence.
            #[test]
            fn the_body_count_picks_each_sides_shape((responses, _) in arb_case()) {
                prop_assert_eq!(success_shape(responses.success()), expected_success(&responses));
                prop_assert_eq!(shape(responses.error()), expected_error(&responses));
            }

            /// Success and error partition the declared statuses: every `by_status` body reaches
            /// exactly one side, the success side exactly when its status is 2xx, and an enum on
            /// either side names only that side's statuses.
            #[test]
            fn success_and_error_partition_the_declared_statuses((responses, _) in arb_case()) {
                let success = success_shape(responses.success());
                let error = shape(responses.error());
                let (success_bodies, error_bodies) = (bodies(&success), bodies(&error));
                for ((status, response), id) in responses.by_status.iter().zip(1u32..) {
                    if response.body.is_none() {
                        continue;
                    }
                    prop_assert_eq!(success_bodies.contains(&id), is_2xx(*status));
                    prop_assert_eq!(error_bodies.contains(&id), !is_2xx(*status));
                }
                if let Shape::Enum(entries) = &success {
                    prop_assert!(entries.iter().all(|(status, _)| is_2xx(*status)));
                }
                if let Shape::Enum(entries) = &error {
                    prop_assert!(entries.iter().all(|(status, _)| !is_2xx(*status)));
                }
            }

            /// `default` is on the success side exactly when no 2xx status is declared, and on
            /// the error side whenever it is declared, as the last entry of an error enum.
            #[test]
            fn default_is_the_success_source_iff_no_2xx_is_declared((responses, _) in arb_case()) {
                let success = success_shape(responses.success());
                let error = shape(responses.error());
                let no_2xx = !responses.by_status.iter().any(|(status, _)| is_2xx(*status));
                let bodied_default = responses
                    .default
                    .as_ref()
                    .is_some_and(|default| default.body.is_some());
                prop_assert_eq!(
                    bodies(&success).contains(&DEFAULT_BODY),
                    bodied_default && no_2xx
                );
                prop_assert_eq!(bodies(&error).contains(&DEFAULT_BODY), bodied_default);
                if let Shape::Enum(entries) = &success {
                    prop_assert!(entries.iter().all(|(status, _)| *status != StatusSpec::Default));
                }
                if let Shape::Enum(entries) = &error {
                    let defaults: Vec<usize> = entries
                        .iter()
                        .enumerate()
                        .filter(|(_, (status, _))| *status == StatusSpec::Default)
                        .map(|(index, _)| index)
                        .collect();
                    let expected: Vec<usize> = if responses.default.is_some() {
                        vec![entries.len() - 1]
                    } else {
                        Vec::new()
                    };
                    prop_assert_eq!(defaults, expected);
                }
            }

            /// Neither shape depends on the order lowering inserted `by_status` entries in.
            #[test]
            fn the_shapes_do_not_depend_on_insertion_order((responses, shuffled) in arb_case()) {
                prop_assert_eq!(
                    success_shape(responses.success()),
                    success_shape(shuffled.success())
                );
                prop_assert_eq!(shape(responses.error()), shape(shuffled.error()));
            }
        }
    }
}
