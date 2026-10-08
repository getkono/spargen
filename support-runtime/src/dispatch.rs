//! The small set of dispatch routines shared within a generated client: build URL →
//! attach auth → send → classify status → decode. Sharing happens *within* a generated client
//! (not via a shared crate), so per-operation functions stay thin `#[inline]` shims.
//!
//! URL/send/classification are non-generic; only body decode is generic, monomorphized once per
//! distinct body type — the one place monomorphization is unavoidable.

use std::convert::Infallible;

use bytes::Bytes;
use reqwest::header::HeaderValue;
use reqwest::{Request, RequestBuilder, Response, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;

use crate::{AuthKind, AuthScheme, ClientCore, Credential, Error, RequestError, ResponseValue};

/// Build a request URL from the base URL and a pre-rendered path plus pre-encoded query
/// fragments. Paths compile to static segment concatenation — no runtime regex. Non-generic.
///
/// `query` holds complete `name=value` fragments that the parameter helpers have already
/// percent-encoded. They are installed verbatim rather than through `query_pairs_mut`, which would
/// re-encode the style delimiters and make a `,` that joins two array items indistinguishable from
/// a `,` inside one of them.
pub fn build_url(
    core: &ClientCore,
    path: &str,
    query: &[String],
) -> Result<Url, Error<Infallible>> {
    build_url_on(core, None, path, query)
}

/// As [`build_url`], but against an optional per-operation server override. An absolute override
/// replaces the client's base URL; a relative one is joined onto it.
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

/// Resolve the base URL for one request: the client's own, or a per-operation server override.
fn base_for(core: &ClientCore, server: Option<&str>) -> Result<Url, Error<Infallible>> {
    match server {
        None => Ok(core.base_url().clone()),
        Some(server) => match Url::parse(server) {
            Ok(absolute) => Ok(absolute),
            // A relative override is resolved against the client's base URL.
            Err(_) => core
                .base_url()
                .join(server)
                .map_err(Error::request_construction),
        },
    }
}

/// Append pre-encoded fragments to a URL, preserving any query the base URL already carried.
fn append_query(url: &mut Url, query: &[String]) {
    if query.is_empty() {
        return;
    }
    let joined = match url.query() {
        Some(existing) if !existing.is_empty() => format!("{existing}&{}", query.join("&")),
        _ => query.join("&"),
    };
    url.set_query(Some(&joined));
}

/// Build a URL whose entire query string is owned by an OpenAPI 3.2 `in: querystring` parameter.
/// Both forms replace any query embedded in the selected server URL.
pub fn build_url_with_query_string(
    core: &ClientCore,
    path: &str,
    query: &[String],
    query_string: Option<&str>,
) -> Result<Url, Error<Infallible>> {
    build_url_with_query_string_on(core, None, path, query, query_string)
}

/// As [`build_url_with_query_string`], but against an optional per-operation server override.
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
        let joined = match url.query() {
            Some(existing) if !existing.is_empty() => format!("{existing}&{query_string}"),
            _ => query_string.to_owned(),
        };
        url.set_query(Some(&joined));
    }
    Ok(url)
}

/// Attach credentials for an operation's security requirement. `requirements` is an OR
/// of alternatives, each an AND of schemes; the first alternative whose schemes all have a
/// registered credential wins, deterministically. An empty alternative (`{}` in the spec) marks
/// security optional and always satisfies. If no alternative is satisfiable the request fails
/// before it is sent — [`RequestError::MissingCredential`], never a silent 401 — and a registered
/// token provider that fails does too, as [`RequestError::CredentialProvider`].
///
/// **Selection is on registration, not on success, and there is no fall-through.** Once an
/// alternative is chosen, a failure while attaching it — a token provider that errors, or a
/// credential registered under a kind its scheme cannot use — fails the call, even when a later
/// alternative is fully registered and would have succeeded. Falling through would send
/// credentials the caller's registration did not select, silently, on a call they had expressed a
/// different intent for; an error they can see is the better failure.
///
/// A caller who wants the *other* alternative unregisters a scheme of the earlier one with
/// [`ClientCore::remove_credential`] (the generated client's `without_credential`); selection then
/// passes over the earlier alternative and reaches the later one on the next call. Registering the
/// fallback as well is not enough on its own, because selection stops at the first satisfiable
/// alternative and the earlier one stays satisfied until one of its schemes is removed.
pub async fn attach_auth(
    core: &ClientCore,
    request: RequestBuilder,
    requirements: &[&[AuthScheme]],
) -> Result<RequestBuilder, Error<Infallible>> {
    // No requirement means "attach nothing", not "unauthenticated". Without this, an empty slice
    // would fall through the loop below without choosing anything, and the call would fail as
    // `MissingCredential` naming no schemes at all. Generated output never produces an empty slice
    // — `emit.rs` omits the call entirely for an operation with no `security` — but this function
    // is public in the runtime crate and reachable from sibling code in whichever module `include!`s
    // a generated client, so this is a contract, not dead code, and spargen's own runtime tests
    // (which are not embedded) hold it to that.
    if requirements.is_empty() {
        return Ok(request);
    }
    // Selection and reporting are one computation: an alternative is chosen exactly when the list
    // of its unregistered schemes is empty, and that same list is what `MissingCredential`
    // reports for it. So every reported inner list is non-empty by construction, and the outer
    // list is non-empty because `requirements` is.
    let mut alternatives: Vec<Vec<&'static str>> = Vec::with_capacity(requirements.len());
    for alternative in requirements {
        let mut missing = Vec::new();
        let mut registered = Vec::with_capacity(alternative.len());
        for scheme in *alternative {
            match resolve(core, scheme) {
                Resolution::Transport => {}
                Resolution::Registered(credential) => registered.push((scheme, credential)),
                Resolution::Missing => missing.push(scheme.name),
            }
        }
        if missing.is_empty() {
            let mut request = request;
            for (scheme, credential) in registered {
                request = apply_credential(request, scheme, credential).await?;
            }
            return Ok(request);
        }
        alternatives.push(missing);
    }
    Err(Error::RequestConstruction(
        RequestError::MissingCredential { alternatives },
    ))
}

/// How one scheme of a security alternative is satisfied, if at all.
enum Resolution<'a> {
    /// Satisfied by the transport itself; nothing is attached to the request.
    Transport,
    /// Satisfied by the credential registered under the scheme's name.
    Registered(&'a Credential),
    /// Not satisfied: the caller still has to register a credential for it.
    Missing,
}

/// The single answer to "does this scheme block its alternative?", read by both the selection
/// and the `MissingCredential` report in [`attach_auth`]. The match is exhaustive over
/// [`AuthKind`], so a new kind has to state how it is satisfied before it compiles.
fn resolve<'a>(core: &'a ClientCore, scheme: &AuthScheme) -> Resolution<'a> {
    match scheme.kind {
        // `mutualTLS` is satisfied by the transport's client certificate, so it never needs a
        // registered credential and never blocks an alternative from being chosen.
        AuthKind::MutualTls => Resolution::Transport,
        AuthKind::Bearer
        | AuthKind::Basic
        | AuthKind::ApiKeyHeader(_)
        | AuthKind::ApiKeyQuery(_)
        | AuthKind::ApiKeyCookie(_) => match core.credential(scheme.name) {
            Some(credential) => Resolution::Registered(credential),
            None => Resolution::Missing,
        },
    }
}

async fn apply_credential(
    request: RequestBuilder,
    scheme: &AuthScheme,
    credential: &Credential,
) -> Result<RequestBuilder, Error<Infallible>> {
    // A provider yields a single secret, usable anywhere a bearer token or apiKey fits. Only a kind
    // that takes a token asks it for one, so a provider registered under a scheme that takes none
    // is the same mismatch whether or not a refresh would have succeeded. `attach_auth` skips
    // `MutualTls` before calling this, so the `MutualTls` term below is never evaluated from there;
    // it is deliberate defence, so a caller that bypasses the skip still never fetches a token.
    let takes_token = !matches!(scheme.kind, AuthKind::Basic | AuthKind::MutualTls);
    let token: Option<SecretString> = match credential {
        Credential::Bearer(secret) | Credential::ApiKey(secret) => Some(secret.clone()),
        Credential::Provider(provider) if takes_token => {
            Some(provider().await.map_err(|source| {
                Error::RequestConstruction(RequestError::CredentialProvider {
                    scheme: scheme.name,
                    source,
                })
            })?)
        }
        Credential::Provider(_) | Credential::Basic { .. } => None,
    };
    match scheme.kind {
        AuthKind::Basic => match credential {
            Credential::Basic { username, password } => {
                Ok(request.basic_auth(username, Some(password.expose_secret())))
            }
            _ => Err(credential_mismatch(scheme.name, "http basic", credential)),
        },
        AuthKind::Bearer => match token {
            Some(token) => Ok(request.bearer_auth(token.expose_secret())),
            None => Err(credential_mismatch(scheme.name, "bearer", credential)),
        },
        AuthKind::ApiKeyHeader(name) => match token {
            Some(token) => Ok(request.header(name, sensitive_value(token.expose_secret())?)),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
        AuthKind::ApiKeyQuery(name) => match token {
            Some(token) => Ok(request.query(&[(name, token.expose_secret())])),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
        // Satisfied by the transport; `attach_auth` never reaches this arm.
        AuthKind::MutualTls => Ok(request),
        AuthKind::ApiKeyCookie(name) => match token {
            Some(token) => {
                let cookie = format!("{name}={}", token.expose_secret());
                Ok(request.header(reqwest::header::COOKIE, sensitive_value(&cookie)?))
            }
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
    }
}

fn sensitive_value(secret: &str) -> Result<HeaderValue, Error<Infallible>> {
    let mut value = HeaderValue::from_str(secret).map_err(Error::request_construction)?;
    value.set_sensitive(true);
    Ok(value)
}

fn credential_mismatch(
    scheme: &'static str,
    required: &'static str,
    credential: &Credential,
) -> Error<Infallible> {
    let registered = match credential {
        Credential::Bearer(_) => "Bearer",
        Credential::Basic { .. } => "Basic",
        Credential::ApiKey(_) => "ApiKey",
        Credential::Provider(_) => "Provider",
    };
    Error::RequestConstruction(RequestError::CredentialMismatch {
        scheme,
        required,
        registered,
    })
}

/// Send a prepared request through the core's transport [`crate::HttpBackend`], mapping
/// transport/timeout/protocol/redirect failures into the taxonomy. The backend reports failures as
/// a [`crate::TransportError`] wrapping the originating `reqwest::Error`; that error is run back
/// through [`Error::from_reqwest`] here, so classification is identical to executing directly on the
/// reqwest client. Non-generic.
pub async fn send(core: &ClientCore, request: Request) -> Result<Response, Error<Infallible>> {
    core.backend()
        .execute(request)
        .await
        .map_err(|error| Error::from_reqwest(error.into_source()))
}

/// Decode a success response body into `T`, wrapping it with status and headers. Monomorphized once
/// per body type. Decode failures become [`Error::Decode`] with the serde path and a body capped
/// at `max_error_body`. A zero-length body is not a JSON value, so it is always [`Error::Decode`],
/// whatever `T` is.
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
/// type. Converting through `Value::String` keeps generated string enums and string formats typed
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
    decode_success_with(core, response, decode_text_body::<T>).await
}

/// The body every structured success decoder shares: read the whole body, run `decode` over it, and
/// wrap the value with status and headers. A decode failure becomes [`Error::Decode`] carrying the
/// decoder's message as its `path` and a body capped at `max_error_body`. Deserialization needs the
/// whole body, so peak memory here is inherent to typed decoding; only what the error *retains* is
/// capped.
pub(crate) async fn decode_success_with<T>(
    core: &ClientCore,
    response: Response,
    decode: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<ResponseValue<T>, Error<Infallible>> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
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
pub async fn decode_success_bytes(
    _core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<Bytes>, Error<Infallible>> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    Ok(ResponseValue::new(status, headers, body))
}

/// Deserialize a raw UTF-8 body through a JSON string value. Exposed to the generated shim so
/// multi-status response variants use exactly the same textual codec as single-body responses.
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
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|error| error.to_string())
}

/// Read a success response body whole, returning its status, headers, and raw bytes so generated
/// code can select the matching per-status variant and decode it. Non-generic: the per-variant
/// `serde_json::from_slice` (and the error taxonomy on failure) stays in the thin generated shim,
/// which owns the status→variant table and its distinct body types.
pub async fn read_success_body(
    response: Response,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Bytes), Error<Infallible>> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    Ok((status, headers, body))
}

/// Read a non-success response body capped at `max_error_body`, returning its status, headers, the
/// (capped) bytes, and whether they were truncated. Generated code for a multi-status error enum
/// picks the documented variant by status and decodes it (→ [`Error::Api`], or [`Error::Decode`] on
/// parse failure); a status matching no documented selector becomes [`Error::UnexpectedStatus`].
/// The `E` parameter only threads the taxonomy through a transport failure while reading.
pub async fn read_error_body<E>(
    core: &ClientCore,
    response: Response,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Bytes, bool), Error<E>> {
    let status = response.status();
    let headers = response.headers().clone();
    let (body, truncated) = read_capped(core, response).await?;
    Ok((status, headers, body, truncated))
}

/// A status selector an operation documents as an error response. Generated code passes these as
/// static tables so classification distinguishes documented from undocumented statuses.
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
pub async fn classify_error_text<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    classify_with(core, response, documented, decode_text_body::<E>).await
}

/// The body every structured error classifier shares: read the body capped at `max_error_body`; a
/// status a `documented` selector matches runs `decode` over it ([`Error::Api`], or
/// [`Error::Decode`] carrying the decoder's message as its `path` on failure); any other status is
/// [`Error::UnexpectedStatus`] with the raw body preserved.
pub(crate) async fn classify_with<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
    decode: impl FnOnce(&[u8]) -> Result<E, String>,
) -> Error<E> {
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, truncated)) => {
            if documented.iter().any(|spec| spec.matches(status)) {
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
            } else {
                Error::UnexpectedStatus {
                    status,
                    headers,
                    body,
                }
            }
        }
        Err(error) => error,
    }
}

/// Classify a documented raw-byte error body without passing it through a structured decoder.
///
/// Retains at most `max_error_body` bytes either way, and drops the truncation flag: neither
/// [`Error::Api`] nor [`Error::UnexpectedStatus`] has a field for it, so a body cut at the cap is
/// indistinguishable from one that was exactly that long.
pub async fn classify_error_bytes<E: From<Bytes>>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E> {
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, _truncated)) => {
            if documented.iter().any(|spec| spec.matches(status)) {
                Error::Api(ResponseValue::new(status, headers, E::from(body)))
            } else {
                Error::UnexpectedStatus {
                    status,
                    headers,
                    body,
                }
            }
        }
        Err(error) => error,
    }
}

/// Wrap a non-success response as [`Error::UnexpectedStatus`] (#7) for operations that document no
/// error body at all, retaining at most `max_error_body` bytes.
pub async fn unexpected_status<E>(core: &ClientCore, response: Response) -> Error<E> {
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, _truncated)) => Error::UnexpectedStatus {
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
/// buffer at its doubled capacity; and `response.bytes()` can return a view sharing the transport's
/// read buffer. Retention is documented as bounded by the cap, so the only way to mean it is to
/// detach every time. The cost is one copy of at most `cap` bytes, on error paths only.
pub(crate) fn cap_body(body: Bytes, cap: usize) -> (Bytes, bool) {
    let retained = body.len().min(cap);
    (Bytes::copy_from_slice(&body[..retained]), body.len() > cap)
}

/// Read a response body, retaining at most `max_error_body` bytes.
///
/// The body is pulled incrementally and abandoned at the first chunk that carries it past the cap
/// (the loop extends before it re-tests), so peak memory is a
/// function of the cap rather than of whatever the server chose to send — `reqwest` imposes no
/// response size limit of its own, so without this a hostile or malfunctioning peer could force an
/// arbitrarily large allocation on an error path whose contents are mostly discarded. What matters
/// is that peak stops depending on the body's size; it is not `cap` exactly. The loop extends
/// before it re-tests, so one transport chunk rides on top — hyper buffers up to a few hundred KiB
/// — and `BytesMut` grows by doubling while `cap_body` allocates the retained copy alongside it. A
/// small cap is therefore dominated by the chunk, not by the cap.
///
/// Abandoning the body early forgoes reuse of that connection, which is the right trade for a
/// response already too large to retain. The threshold is the cap itself, with no drain allowance
/// on top: an allowance would be a second bound that nothing declares and no caller can set, and
/// the cap is already the one number a consumer chose. A deployment that would rather keep the
/// connection can raise the cap, which says so directly.
///
/// Uses `Response::chunk`, which — unlike `bytes_stream` — is not behind reqwest's `stream`
/// feature. Generated clients enable `stream` only for APIs with sequential responses, so reading
/// incrementally here must not depend on it.
#[cfg(not(target_arch = "wasm32"))]
async fn read_capped<E>(
    core: &ClientCore,
    mut response: Response,
) -> Result<(Bytes, bool), Error<E>> {
    let cap = core.config().max_error_body;
    // Pre-size to the cap, but do not honour an enormous configured cap up front — the point is to
    // avoid large speculative allocations.
    let mut buffered = bytes::BytesMut::with_capacity(cap.min(16 * 1024));
    while buffered.len() <= cap {
        // One byte past the cap is enough to know the remainder is being dropped.
        match response.chunk().await.map_err(Error::from_reqwest)? {
            Some(chunk) => buffered.extend_from_slice(&chunk),
            None => break,
        }
    }
    // `freeze` hands the buffer over at its doubled capacity, so an under-cap body would pin up to
    // twice its length. `cap_body` copies unconditionally, which is what settles that here.
    Ok(cap_body(buffered.freeze(), cap))
}

/// The `wasm32` counterpart. reqwest's `fetch` backend exposes no `chunk`, so the body arrives
/// whole and only *retention* can be bounded here, not peak memory.
// Mutation testing: replacing this body with `Ok((Default::default(), _))` survives the native
// suite and is declared rather than killed. The function is compiled only for `wasm32`, so a
// native test binary holds the native `read_capped` above in its place and no native test can
// reach the mutated code; on `wasm32` the runtime is compile-checked, not tested.
#[cfg(target_arch = "wasm32")]
async fn read_capped<E>(core: &ClientCore, response: Response) -> Result<(Bytes, bool), Error<E>> {
    let cap = core.config().max_error_body;
    let bytes = response.bytes().await.map_err(Error::from_reqwest)?;
    Ok(cap_body(bytes, cap))
}

#[cfg(test)]
mod tests;
