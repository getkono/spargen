//! The dispatch routines every operation of a generated client shares: build URL → attach auth
//! (in the `auth` module) → send → classify status → decode. Sharing happens *within* a generated
//! client (not via a shared crate).
//!
//! URL building and sending are non-generic. The success decoders are generic over the body type
//! and the error classifiers over the operation's error type `E`; each takes the body codec as a
//! closure over the shared `decode_success_with` / `classify_with` routine, so one copy of each
//! is monomorphized per distinct body or error type.

use std::convert::Infallible;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use url::Url;

use crate::client::require_http_base;
use crate::{ClientCore, Error, HttpBackend, HttpBody, ResponseValue, TotalTimeout};

/// A request as the transport seam carries it.
type Request = http::Request<HttpBody>;

/// A response as the transport seam carries it.
type Response = http::Response<HttpBody>;

/// Build a request URL from the base URL, or an optional per-operation server override, and a
/// pre-rendered path plus pre-encoded query fragments. An absolute override replaces the client's
/// base URL; a relative one is joined onto it. Paths compile to static segment concatenation — no
/// runtime regex. Non-generic.
///
/// `query` holds complete `name=value` fragments that the parameter helpers have already
/// percent-encoded. They are installed verbatim rather than through `query_pairs_mut`, which would
/// re-encode the style delimiters and make a `,` that joins two array items indistinguishable from
/// a `,` inside one of them.
pub fn build_url_on(
    core: &ClientCore,
    server: Option<&str>,
    path: &str,
    query: &[String],
) -> Result<Url, Error<Infallible>> {
    let mut url = base_for(core, server)?;
    let base_path = url.path().trim_end_matches('/');
    let request_path = path.trim_start_matches('/');
    let joined = if base_path.is_empty() {
        format!("/{request_path}")
    } else if request_path.is_empty() {
        base_path.to_owned()
    } else {
        format!("{base_path}/{request_path}")
    };
    // `Url::set_path` removes `.` and `..` segments, in every spelling the URL Standard gives
    // them (`%2E`, `.%2E`, ...), so percent-encoding a dot does not protect it. A rendered path
    // value forming such a segment would silently re-target the request (`/users/../keys` is
    // sent to `/keys`), so it is refused instead. A special-scheme URL also splits on `\`.
    // The segment is `Debug`-formatted: `is_dot_segment` ignores tab, LF and CR, so a refused
    // segment may hold them, and written raw they would hide in or break the message's line.
    if let Some(segment) = request_path.split(['/', '\\']).find(|s| is_dot_segment(s)) {
        return Err(Error::request_message(format!(
            "request path contains the dot segment {segment:?}, which URL normalization would \
             remove and so send the request to a different resource"
        )));
    }
    // Otherwise `Url::set_path` leaves `%`, `;`, `=`, `,` and `.` alone, so pre-encoded values
    // and the `matrix`/`label` style prefixes survive it unchanged.
    url.set_path(&joined);
    append_query(&mut url, query);
    Ok(url)
}

/// Whether `segment` is a `.` or `..` path segment under the URL Standard: `.` or `%2E`, or
/// `..`, `.%2E`, `%2E.` or `%2E%2E`, each `%2E` matched ASCII case-insensitively. The URL parser
/// deletes every ASCII tab, LF and CR before it reads a segment, so `.\t.` is `..` to it; they are
/// deleted here first too.
fn is_dot_segment(segment: &str) -> bool {
    let stripped: String;
    let segment = if segment.contains(['\t', '\n', '\r']) {
        stripped = segment
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        stripped.as_str()
    } else {
        segment
    };
    let rest = segment
        .strip_prefix('.')
        .or_else(|| strip_encoded_dot(segment));
    match rest {
        Some("") => true,
        Some(rest) => rest == "." || strip_encoded_dot(rest) == Some(""),
        None => false,
    }
}

/// `segment` without a leading `%2E` (either case), if it starts with one.
fn strip_encoded_dot(segment: &str) -> Option<&str> {
    segment
        .get(..3)
        .filter(|prefix| prefix.eq_ignore_ascii_case("%2e"))
        .map(|_| &segment[3..])
}

/// Resolve the base URL for one request: the client's own, or a per-operation server override,
/// which is refused, as the client's base URL is, unless it resolves to an `http` or `https` URL.
fn base_for(core: &ClientCore, server: Option<&str>) -> Result<Url, Error<Infallible>> {
    match server {
        None => Ok(core.base_url().clone()),
        Some(server) => match Url::parse(server) {
            Ok(absolute) => require_http_base(absolute),
            // A relative override is resolved against the client's base URL, so it keeps that
            // URL's scheme.
            Err(_) => core
                .base_url()
                .join(server)
                .map_err(Error::request_construction),
        },
    }
}

/// Append pre-encoded fragments to a URL, `&`-joined after any non-empty query the URL already
/// carried. The new query is written into one buffer of exactly its length, and an empty `query`
/// leaves the URL as it was.
fn append_query<S: AsRef<str>>(url: &mut Url, query: &[S]) {
    if query.is_empty() {
        return;
    }
    let existing = url.query().filter(|existing| !existing.is_empty());
    let capacity = existing.map_or(0, |existing| existing.len() + 1)
        + query
            .iter()
            .map(|fragment| fragment.as_ref().len())
            .sum::<usize>()
        + (query.len() - 1);
    let mut joined = String::with_capacity(capacity);
    if let Some(existing) = existing {
        joined.push_str(existing);
        joined.push('&');
    }
    for (index, fragment) in query.iter().enumerate() {
        if index > 0 {
            joined.push('&');
        }
        joined.push_str(fragment.as_ref());
    }
    url.set_query(Some(&joined));
}

/// As [`build_url_on`], for a URL whose entire query string is owned by an OpenAPI 3.2
/// `in: querystring` parameter. With or without a query string, it replaces any query embedded in
/// the selected server URL.
pub fn build_url_with_query_string_on(
    core: &ClientCore,
    server: Option<&str>,
    path: &str,
    query: &[String],
    query_string: Option<&str>,
) -> Result<Url, Error<Infallible>> {
    let mut url = build_url_on(core, server, path, &[])?;
    // `in: querystring` owns the complete query. A query embedded in the selected server URL must
    // not leak into that value.
    url.set_query(None);
    append_query(&mut url, query);
    if let Some(query_string) = query_string {
        append_query(&mut url, &[query_string]);
    }
    Ok(url)
}

/// Send a request ([`crate::RequestBuilder`] builds one) through the core's transport
/// [`crate::HttpBackend`], mapping transport/timeout/protocol/redirect failures into the taxonomy.
/// The backend reports failures as a [`crate::TransportError`] carrying its taxonomy class, which
/// picks the variant here. Non-generic.
///
/// The core's [`crate::RequestOptions`] are applied first, as [`send_streaming`] applies them. A
/// request that then carries no [`TotalTimeout`] is given the core's
/// [`crate::ClientConfig::timeout`], read now, which bounds it until its whole response body has
/// been read: a total timeout the request carries itself wins, then one in the options'
/// extensions, then the configured one. The body limits [`crate::RequestOptions::max_body`] and
/// [`crate::ClientConfig::max_success_body`] set are read now too, and travel on the response to
/// the decoder that reads its body. An operation whose success response is a stream sends with
/// [`send_streaming`] instead.
pub async fn send(
    core: &ClientCore,
    request: Request,
) -> Result<http::Response<HttpBody>, Error<Infallible>> {
    execute(core, prepare(core, request, false), None, false).await
}

/// The request [`send`] (or, with `streaming`, [`send_streaming`]) hands the core's backend,
/// without sending it: the core's [`crate::RequestOptions`] headers and extensions added where it
/// lacks them, and then, unless `streaming`, the core's [`crate::ClientConfig::timeout`] as a
/// [`TotalTimeout`] where it still carries none. Non-generic.
///
/// A generated operation prepares its request with it, inserts its own `ops::Operation` into the
/// request's extensions, and hands the request to [`execute`]; its `{method}_request` builder
/// returns the same request unsent.
pub fn prepare(core: &ClientCore, mut request: Request, streaming: bool) -> Request {
    apply_options(core.options(), &mut request);
    if !streaming && request.extensions().get::<TotalTimeout>().is_none() {
        if let Some(timeout) = core.config().timeout {
            request.extensions_mut().insert(TotalTimeout(timeout));
        }
    }
    request
}

/// Execute a [`prepare`]d request through the core's backend, mapping a transport failure into
/// the taxonomy, and replay it once through [`crate::replay_rejected`] where `replay` is the one
/// [`crate::attach_auth_replayable`] returned for it; the replay carries the request's extensions,
/// its `ops::Operation` included. Unless `streaming`, the response carries the body limits
/// [`crate::RequestOptions::max_body`] and [`crate::ClientConfig::max_success_body`] set, read
/// now, to the decoder that reads its body. Non-generic.
pub async fn execute(
    core: &ClientCore,
    request: http::Request<HttpBody>,
    replay: Option<crate::Replay>,
    streaming: bool,
) -> Result<http::Response<HttpBody>, Error<Infallible>> {
    let replay = replay.map(|replay| replay.carrying(request.extensions()));
    let response = execute_once(core, request, streaming).await?;
    crate::replay_rejected(core, response, replay, streaming).await
}

/// [`execute`] with no replay: the one send a replay makes itself.
pub(crate) async fn execute_once(
    core: &ClientCore,
    request: http::Request<HttpBody>,
    streaming: bool,
) -> Result<http::Response<HttpBody>, Error<Infallible>> {
    let head = request.method() == http::Method::HEAD;
    let mut response = send_on(core.backend().as_ref(), request).await?;
    if head {
        response.extensions_mut().insert(HeadResponse);
    }
    if !streaming {
        insert_body_limits(core, &mut response);
    }
    Ok(response)
}

/// Give `response` the body limits a non-streaming call made on `core` reads its body under,
/// unless it carries them already: [`execute`] inserts them at the send, and a generated
/// `decode_{method}` inserts them into a response a caller obtained on its own.
pub fn with_body_limits(core: &ClientCore, mut response: Response) -> Response {
    if response.extensions().get::<BodyLimits>().is_none() {
        insert_body_limits(core, &mut response);
    }
    response
}

/// Insert the body limits `core` sets now into `response`, replacing any it carries.
fn insert_body_limits(core: &ClientCore, response: &mut Response) {
    let max_body = core.options().max_body;
    response.extensions_mut().insert(BodyLimits {
        success: max_body.or(core.config().max_success_body),
        error: max_body,
    });
}

/// [`send`] for a request whose success response is read as a stream (an `EventStream`): the
/// core's [`crate::ClientConfig::timeout`] is not applied, because a stream may rightly stay open
/// indefinitely, and neither is a body limit. A total timeout the request carries itself, or one
/// the core's [`crate::RequestOptions::extensions`] carry, is a deadline the caller asked for and
/// still applies, and the client's connect and read timeouts bound it as they bound every request.
/// Non-generic.
///
/// Both sends add the core's [`crate::RequestOptions`] headers the request does not already
/// carry, and its extensions of a type the request does not already carry, before the backend
/// executes it.
pub async fn send_streaming(
    core: &ClientCore,
    request: Request,
) -> Result<http::Response<HttpBody>, Error<Infallible>> {
    execute(core, prepare(core, request, true), None, true).await
}

/// Add `options`' headers and extensions to `request`, leaving whatever it already carries as it
/// is: a header name it carries keeps exactly its own values, and an extension type it carries its
/// own value. [`crate::EventStream`]'s reconnect applies them the same way to the request it
/// resends.
pub(crate) fn apply_options(
    options: &crate::RequestOptions,
    request: &mut http::Request<HttpBody>,
) {
    let headers = request.headers_mut();
    for name in options.headers.keys() {
        if !headers.contains_key(name) {
            for value in options.headers.get_all(name) {
                headers.append(name.clone(), value.clone());
            }
        }
    }
    if !options.extensions.is_empty() {
        let mut extensions = options.extensions.clone();
        extensions.extend(std::mem::take(request.extensions_mut()));
        *request.extensions_mut() = extensions;
    }
}

/// The body limits [`send`] read from the core, carried on the response it returns so the decoder
/// reading the body applies the ones the call was made under. A response that does not carry them
/// (a streaming operation's, or one a caller built) is read without a limit beyond
/// `max_error_body`.
#[derive(Debug, Clone, Copy)]
struct BodyLimits {
    /// The most bytes a success body may hold.
    success: Option<usize>,
    /// The most bytes an error body may hold, where it is less than `max_error_body`.
    error: Option<usize>,
}

/// The most bytes of a failed `response`'s body its error reads: `max_error_body`, or the error
/// limit of the [`BodyLimits`] it carries where that is smaller.
pub(crate) fn error_body_bound(max_error_body: usize, response: &Response) -> usize {
    response
        .extensions()
        .get::<BodyLimits>()
        .and_then(|limits| limits.error)
        .map_or(max_error_body, |limit| limit.min(max_error_body))
}

/// Marks a response [`execute`] received for a `HEAD` request, so the reader of its body knows its
/// `Content-Length` describes a representation it does not carry.
#[derive(Debug, Clone, Copy)]
struct HeadResponse;

/// Whether the response `parts` describe carries no body whatever its `Content-Length` says: the
/// answer to a `HEAD` request [`execute`] sent, or a `1xx`, `204` or `304` response (RFC 9110
/// §6.4.1). Its body is still bounded as it is read.
fn is_bodiless(parts: &http::response::Parts) -> bool {
    parts.extensions.get::<HeadResponse>().is_some()
        || parts.status.is_informational()
        || parts.status == http::StatusCode::NO_CONTENT
        || parts.status == http::StatusCode::NOT_MODIFIED
}

/// The body length a response declares in a well-formed `Content-Length`, unless it
/// [`is_bodiless`], whose `Content-Length` declares no body it carries.
pub(crate) fn declared_body_length(parts: &http::response::Parts) -> Option<u64> {
    if is_bodiless(parts) {
        return None;
    }
    declared_length(&parts.headers)
}

/// Whether `headers` declare a `Content-Length` past `limit`. A missing or malformed one declares
/// nothing, and the body is bounded as it is read instead.
fn declares_more_than(headers: &reqwest::header::HeaderMap, limit: usize) -> bool {
    declared_length(headers).is_some_and(|length| exceeds(length, limit))
}

/// The body length `headers` declare in a well-formed `Content-Length`.
fn declared_length(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok())
}

/// Whether `length` bytes are more than `limit`.
pub(crate) fn exceeds(length: u64, limit: usize) -> bool {
    u64::try_from(limit).is_ok_and(|limit| length > limit)
}

/// Read `body`, the body of the response `parts` describe, whole, or fail with
/// [`Error::BodyTooLarge`] once it is known to hold more than `limit` bytes: up front from a
/// `Content-Length` past it, unless the response [`is_bodiless`], or at the chunk that carries it
/// past.
async fn read_within<E>(
    limit: usize,
    parts: &http::response::Parts,
    body: HttpBody,
) -> Result<Bytes, Error<E>> {
    let status = parts.status;
    if !is_bodiless(parts) && declares_more_than(&parts.headers, limit) {
        return Err(Error::BodyTooLarge { limit, status });
    }
    body.collect_within(limit)
        .await
        .map_err(Error::from_transport)?
        .ok_or(Error::BodyTooLarge { limit, status })
}

/// [`send`] through the backend alone, for a caller that holds the backend and a request already
/// on the seam rather than the whole [`ClientCore`]: a streaming response's reconnect.
pub(crate) async fn send_on(
    backend: &dyn HttpBackend,
    request: http::Request<HttpBody>,
) -> Result<Response, Error<Infallible>> {
    backend
        .execute(request)
        .await
        .map_err(Error::from_transport)
}

/// Decode a success response body into `T`, wrapping it with status and headers. Monomorphized once
/// per body type. Decode failures become [`Error::Decode`] with serde's error message and a body
/// capped at `max_error_body`. A zero-length body is not a JSON value, so it is always
/// [`Error::Decode`], whatever `T` is.
pub async fn decode_success<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    decode_success_with(core, response, |body| {
        serde_json::from_slice::<T>(body).map_err(|error| error.to_string())
    })
    .await
}

/// Decode a raw UTF-8 success body as the JSON string value described by a textual OpenAPI media
/// type, or as the JSON scalar it spells where `T` takes no string (see [`decode_text_body`]).
/// Converting through `Value::String` keeps generated string enums and string formats typed
/// while avoiding JSON's quote requirement on the wire. An empty body is decoded like any other
/// (see [`decode_text_body`]): `String` yields `""`, a typed value with no empty member yields
/// [`Error::Decode`].
pub async fn decode_success_text<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    decode_success_with(core, response, |body| decode_text_body::<T>(body)).await
}

/// The body every structured success decoder shares: read the whole body, run `decode` over it, and
/// wrap the value with status and headers. A decode failure becomes [`Error::Decode`] carrying the
/// decoder's message as its `path` and a body capped at `max_error_body`. Deserialization needs the
/// whole body, so peak memory here is inherent to typed decoding; only what the error *retains* is
/// capped. A response [`send`] returned is read within the success-body limit it was sent under
/// ([`crate::RequestOptions::max_body`], else [`crate::ClientConfig::max_success_body`]), and one
/// past it is [`Error::BodyTooLarge`], before `decode` runs.
///
/// Generated code for a multi-status success enum calls it from the arm whose selector matched,
/// with a `decode` that builds that arm's variant from the `Bytes` it receives. An arm whose variant
/// holds a raw binary body reads it through [`decode_success_bytes`] instead, within the binary
/// default where the call set no limit.
pub async fn decode_success_with<T>(
    core: &ClientCore,
    response: Response,
    decode: impl FnOnce(&Bytes) -> Result<T, String>,
) -> Result<ResponseValue<T>, Error<Infallible>> {
    decode_success_within(core, response, None, decode).await
}

/// [`decode_success_with`], reading the body within `default_limit` where the call set no
/// success-body limit of its own.
async fn decode_success_within<T>(
    core: &ClientCore,
    response: Response,
    default_limit: Option<usize>,
    decode: impl FnOnce(&Bytes) -> Result<T, String>,
) -> Result<ResponseValue<T>, Error<Infallible>> {
    let (parts, body) = response.into_parts();
    let limit = parts
        .extensions
        .get::<BodyLimits>()
        .and_then(|limits| limits.success)
        .or(default_limit);
    let body = match limit {
        Some(limit) => read_within(limit, &parts, body).await?,
        None => body.collect().await.map_err(Error::from_transport)?,
    };
    let (status, headers) = (parts.status, parts.headers);
    match decode(&body) {
        Ok(value) => Ok(ResponseValue::new(status, headers, value)),
        Err(path) => {
            let (body, truncated) = cap_body(body, core.config().max_error_body);
            Err(Error::Decode {
                status,
                headers,
                path,
                body,
                truncated,
            })
        }
    }
}

/// Decode a raw binary success body without attempting JSON deserialization. Every byte sequence
/// is a valid binary body, so a zero-length one is an empty `Bytes`, never an error.
///
/// The body is read within the success-body limit the call was made under, as
/// [`decode_success_with`] reads one, and where the call set none, within
/// [`crate::ClientConfig::DEFAULT_MAX_BINARY_BODY`]: a binary body is never read whole without a
/// bound. One past it is [`Error::BodyTooLarge`]. A generated method reads with this its one raw
/// binary success body and each raw binary status of its success enum; its `_stream` twin hands
/// each such body over unread instead.
pub async fn decode_success_bytes(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<Bytes>, Error<Infallible>> {
    // The decoder cannot fail, so `Error::Decode` is unreachable and the body is never capped.
    decode_success_within(
        core,
        response,
        Some(crate::ClientConfig::DEFAULT_MAX_BINARY_BODY),
        |body| Ok(body.clone()),
    )
    .await
}

/// Deserialize a raw UTF-8 body through a JSON string value, or, where `T` does not take the
/// string, through the JSON number, boolean or `null` the text spells, as a header value is read:
/// so `42` decodes into an integer and `true` into a boolean, while a `String` still reads `42` as
/// the text it is. Text spelling no scalar (a JSON array or object included) is offered as the
/// string again, and fails as that. Exposed to the generated shim so multi-status response
/// variants use exactly the same textual codec as single-body responses.
///
/// An empty body is the zero-length text and is not special-cased: it becomes
/// `Value::String("")`, so whether it decodes is `T`'s decision. `String` decodes it to `""` by
/// design (as do `format: uuid`/`date-time`/`date`, which lower to `String` with the `uuid`/`time`
/// features off), and so do an untyped schema's `Value` and a string enum with an empty variant; a
/// string enum without one, `uuid::Uuid`, and the RFC 3339 `DateTime` / `Date` newtypes reject it
/// with a decode failure. This is unlike the JSON and XML codecs, where an empty body is not a
/// document and always fails. A documented bodyless status beside a documented body is a unit
/// variant of the response enum, on the success side and the error side alike, and is not decoded
/// here.
pub fn decode_text_body<T>(body: &[u8]) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let text = std::str::from_utf8(body).map_err(|error| error.to_string())?;
    if let Ok(value) = serde_json::from_value(serde_json::Value::String(text.to_owned())) {
        return Ok(value);
    }
    serde_json::from_value(crate::header::json_scalar(text)).map_err(|error| error.to_string())
}

/// Read a non-success response body capped at `max_error_body`, returning its status, headers, the
/// (capped) bytes, and whether they were truncated: the read step of [`classify_with`]. The `E`
/// parameter only threads the taxonomy through a transport failure while reading.
pub(crate) async fn read_error_body<E>(
    core: &ClientCore,
    response: Response,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Bytes, bool), Error<E>> {
    read_error_body_capped(core.config().max_error_body, response).await
}

/// [`read_error_body`] under an explicit `max_error_body` cap, for a caller that holds the cap
/// rather than the whole [`ClientCore`].
async fn read_error_body_capped<E>(
    max_error_body: usize,
    response: Response,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Bytes, bool), Error<E>> {
    let (parts, body) = response.into_parts();
    // A per-call `max_body` below the retention cap bounds the read itself: a body past it is
    // refused rather than truncated. At or above the cap, the capped read below never reaches it.
    let limit = parts
        .extensions
        .get::<BodyLimits>()
        .and_then(|limits| limits.error)
        .filter(|limit| *limit < max_error_body);
    if let Some(limit) = limit {
        let body = read_within(limit, &parts, body).await?;
        // Within `limit`, so under the cap: `cap_body` only detaches the retained copy.
        let (body, truncated) = cap_body(body, max_error_body);
        return Ok((parts.status, parts.headers, body, truncated));
    }
    let (body, truncated) = read_capped(max_error_body, body).await?;
    Ok((parts.status, parts.headers, body, truncated))
}

/// A status selector an operation documents a response under. Generated code passes these as
/// static tables so classification distinguishes documented from undocumented statuses, and
/// matches them to pick a multi-status response's variant on either side.
#[derive(Debug, Clone, Copy)]
pub enum StatusSpec {
    /// An exact status code, e.g. `404`.
    Exact(u16),
    /// A status range by leading digit, e.g. `Range(5)` for `5XX`.
    Range(u8),
    /// The `default` response — matches any status.
    Any,
}

impl StatusSpec {
    /// Whether the selector covers the given status.
    pub fn matches(self, status: reqwest::StatusCode) -> bool {
        match self {
            StatusSpec::Exact(code) => status.as_u16() == code,
            StatusSpec::Range(prefix) => status.as_u16() / 100 == u16::from(prefix),
            StatusSpec::Any => true,
        }
    }
}

/// Classify a non-success response: a documented status parses into the operation's typed error
/// body ([`Error::Api`], #6, falling back to [`Error::Decode`] on parse failure); an undocumented
/// status becomes [`Error::UnexpectedStatus`] (#7) with the raw body preserved. Retains at most
/// `max_error_body` bytes either way.
pub async fn classify_error<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    classify_with(core, response, documented, |body| {
        serde_json::from_slice::<E>(body).map_err(|error| error.to_string())
    })
    .await
}

/// Classify a documented textual error body, preserving the same cap and taxonomy as JSON errors.
/// A documented body that is not UTF-8, or whose text `E` rejects, is [`Error::Decode`]; an empty
/// body is decoded like any other, as [`decode_text_body`] describes.
pub async fn classify_error_text<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    classify_with(core, response, documented, |body| {
        decode_text_body::<E>(body)
    })
    .await
}

/// The body every error classifier shares: read the body capped at `max_error_body`; a status a
/// `documented` selector matches runs `decode` over it ([`Error::Api`], or [`Error::Decode`]
/// carrying the decoder's message as its `path` on failure); any other status is
/// [`Error::UnexpectedStatus`] with the raw body preserved. `decode` receives the `Bytes` itself so
/// a raw binary error body keeps it without a copy.
///
/// Generated code for a multi-status error enum calls it from the arm whose selector matched, with
/// that selector as `documented` and a `decode` that builds that arm's variant, as the success side
/// calls [`decode_success_with`].
pub async fn classify_with<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
    decode: impl FnOnce(&Bytes) -> Result<E, String>,
) -> Error<E> {
    let (status, headers, body, truncated) = match read_error_body(core, response).await {
        Ok(parts) => parts,
        Err(error) => return error,
    };
    if !documented.iter().any(|spec| spec.matches(status)) {
        return Error::UnexpectedStatus {
            status,
            headers,
            body,
        };
    }
    match decode(&body) {
        Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
        Err(path) => Error::Decode {
            status,
            headers,
            path,
            body,
            truncated,
        },
    }
}

/// Classify a documented raw-byte error body without passing it through a structured decoder.
///
/// Retains at most `max_error_body` bytes either way, as `ClientConfig::max_error_body` documents.
pub async fn classify_error_bytes<E: From<Bytes>>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E> {
    // The decoder cannot fail, so `Error::Decode` is unreachable.
    classify_with(core, response, documented, |body| Ok(E::from(body.clone()))).await
}

/// Wrap a non-success response as [`Error::UnexpectedStatus`] (#7) for operations that document no
/// error body at all, retaining at most `max_error_body` bytes.
pub async fn unexpected_status<E>(core: &ClientCore, response: Response) -> Error<E> {
    unexpected_status_capped(core.config().max_error_body, response).await
}

/// [`unexpected_status`] under an explicit `max_error_body` cap, for a caller that holds the cap
/// rather than the whole [`ClientCore`]: a streaming response's reconnect. With no selector
/// documented, [`classify_with`] would answer every status this way.
pub(crate) async fn unexpected_status_capped<E>(
    max_error_body: usize,
    response: Response,
) -> Error<E> {
    match read_error_body_capped(max_error_body, response).await {
        Ok((status, headers, body, _truncated)) => Error::UnexpectedStatus {
            status,
            headers,
            body,
        },
        Err(error) => error,
    }
}

/// Truncate a body to the retention cap, **copying** the retained prefix rather than slicing it.
/// Returns the retained bytes and whether any were dropped.
///
/// The copy is unconditional, including when nothing is truncated, because a `Bytes` does not
/// reveal how much memory stands behind it and every source here can hand over more than it holds:
/// `Bytes::slice` is a refcounted view onto the whole original; `BytesMut::freeze` hands over the
/// buffer at its doubled capacity; and a body read whole can be a view sharing the transport's
/// read buffer. Retention is documented as bounded by the cap, so the only way to mean it is to
/// detach every time. The cost is one copy of at most `cap` bytes, on error paths only.
pub(crate) fn cap_body(body: Bytes, cap: usize) -> (Bytes, bool) {
    let retained = body.len().min(cap);
    (Bytes::copy_from_slice(&body[..retained]), body.len() > cap)
}

/// Read a response body, retaining at most `max_error_body` bytes.
///
/// The body is pulled incrementally and abandoned at the first chunk that carries it past the cap,
/// so peak memory depends on the cap (plus one transport chunk, and the retained copy `cap_body`
/// makes) rather than on whatever the server chose to send; `reqwest` imposes no response size
/// limit of its own. Abandoning the body forgoes reuse of that connection; a deployment that would
/// rather keep it raises the cap. A body the backend delivers whole arrives as one chunk, so there
/// only *retention* is bounded, not peak memory. `ReqwestBackend` delivers every `fetch` body on
/// `wasm32` whole, including an error body of a streaming operation and of a reconnect, because
/// only `EventStream` reads a body chunk by chunk there.
async fn read_capped<E>(cap: usize, body: HttpBody) -> Result<(Bytes, bool), Error<E>> {
    let prefix = read_prefix(cap, body)
        .await
        .map_err(Error::from_transport)?;
    // `read_prefix`'s `freeze` hands the buffer over at its doubled capacity, so an under-cap body
    // would pin up to twice its length. `cap_body` copies unconditionally, which is what settles
    // that here.
    Ok(cap_body(prefix, cap))
}

/// The bytes of `body` up to the first chunk that carries it past `cap`, or all of it when it
/// ends first: [`read_capped`]'s read step, before the retention copy. More than `cap` bytes come
/// back exactly when the body held more, so a later [`read_capped`] over them reports the same
/// truncation the whole body would have. Peak memory is the cap plus one transport chunk.
pub(crate) async fn read_prefix(
    cap: usize,
    mut body: HttpBody,
) -> Result<Bytes, crate::TransportError> {
    // Pre-size to the cap, but do not honour an enormous configured cap up front — the point is to
    // avoid large speculative allocations.
    let mut buffered = bytes::BytesMut::with_capacity(cap.min(16 * 1024));
    while buffered.len() <= cap {
        // One byte past the cap is enough to know the remainder is being dropped.
        match body.chunk().await? {
            Some(chunk) => buffered.extend_from_slice(&chunk),
            None => break,
        }
    }
    Ok(buffered.freeze())
}

#[cfg(test)]
mod tests;
