use super::{Framing, MediaType, Ty, TypeGraph};

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
    /// For a streaming response (chosen media whose [`MediaType::stream_framing`] is `Some`), the
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
    /// Per-status responses, in document order. [`Self::success`] and [`Self::error`] sort them
    /// into precedence order (exact before range).
    pub(crate) by_status: Vec<(StatusSpec, Response)>,
    /// The `default` response, if declared.
    pub(crate) default: Option<Response>,
}

impl Responses {
    /// Every documented response with its selector: the `by_status` entries in order, then the
    /// `default` response as [`StatusSpec::Default`]. The one walk over both fields, so every
    /// reader that keys a response by its selector (`name`'s header structs, `codegen`'s emission)
    /// sees the same keys.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (StatusSpec, &Response)> {
        self.by_status
            .iter()
            .map(|(status, response)| (*status, response))
            .chain(
                self.default
                    .as_ref()
                    .map(|response| (StatusSpec::Default, response)),
            )
    }

    /// The response a shape entry was built from: the `default` response for
    /// [`StatusSpec::Default`], otherwise the first `by_status` entry with exactly that selector.
    pub(crate) fn get(&self, status: StatusSpec) -> Option<&Response> {
        match status {
            StatusSpec::Default => self.default.as_ref(),
            StatusSpec::Exact(_) | StatusSpec::Range(_) => self
                .by_status
                .iter()
                .find(|(candidate, _)| *candidate == status)
                .map(|(_, response)| response),
        }
    }

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
    /// a bodyless `304` is plain `T`, and the `304` is on the error side (issues #198 and
    /// #105; the tests below pin both). It is the success source only when `by_status`
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

        let entries: Vec<(StatusSpec, Option<Ty>)> = self
            .entries()
            .filter(|(status, _)| status.is_success())
            .map(|(status, response)| (status, response.body))
            .collect();
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
        has_two_bodied(&self.success_responses())
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
        let response = sole_bodied(self.success_responses())?;
        Some((response.stream?, response.body?))
    }

    /// The media type of the operation's single bodied success response, when exactly one success
    /// response carries a body. Codegen uses this to route the [`SuccessShape::Plain`] decode (a
    /// single body beside a bodyless sibling is an [`SuccessShape::Enum`], routed per status
    /// instead). `None` when there is no single bodied success.
    pub(crate) fn single_success_media(&self) -> Option<MediaType> {
        sole_bodied(self.success_responses())?.media
    }

    /// The media type of the operation's single bodied error response, when exactly one error
    /// response carries a body. Codegen uses this to route the [`ErrorShape::Single`] classification
    /// (a single body beside a bodyless error entry is an [`ErrorShape::Enum`], routed per status
    /// instead). `None` when there is no single bodied error.
    pub(crate) fn single_error_media(&self) -> Option<MediaType> {
        sole_bodied(self.error_responses())?.media
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
        has_two_bodied(&self.error_responses())
    }

    /// Whether a bodied streaming response (media whose [`MediaType::stream_framing`] is `Some`)
    /// sits anywhere but the operation's single bodied success — the one position whose framing
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
        self.entries()
            .filter(|(status, _)| !status.is_success())
            .map(|(_, response)| response)
            .collect()
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
        !self.by_status.iter().any(|(status, _)| status.is_success())
    }

    /// The operation's success responses in document order: the `default` response alone when no
    /// success status is declared (it is then the sole success), otherwise the 2xx entries.
    /// Mirrors the success/error split used by [`Self::success`].
    fn success_responses(&self) -> Vec<&Response> {
        if self.default_is_success_source() {
            return self.default.iter().collect();
        }
        self.entries()
            .filter(|(status, _)| status.is_success())
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
        let entries: Vec<(StatusSpec, Option<Ty>)> = self
            .entries()
            .filter(|(status, _)| !status.is_success())
            .map(|(status, response)| (status, response.body))
            .collect();
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

/// The one response of `responses` that carries a body, when exactly one does.
fn sole_bodied(responses: Vec<&Response>) -> Option<&Response> {
    let mut bodied = responses
        .into_iter()
        .filter(|response| response.body.is_some());
    match (bodied.next(), bodied.next()) {
        (Some(response), None) => Some(response),
        _ => None,
    }
}

/// Whether two or more of `responses` carry a body.
fn has_two_bodied(responses: &[&Response]) -> bool {
    responses
        .iter()
        .filter(|response| response.body.is_some())
        .nth(1)
        .is_some()
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
mod tests;
