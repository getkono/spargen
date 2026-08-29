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

use crate::{AuthKind, AuthScheme, ClientCore, Credential, Error, ResponseValue};

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
    // `Url::set_path` leaves `%`, `;`, `=`, `,` and `.` alone, so pre-encoded values and the
    // `matrix`/`label` style prefixes survive it unchanged.
    url.set_path(&joined);
    append_query(&mut url, query);
    Ok(url)
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
/// before it is sent — a request-construction error, never a silent 401.
pub async fn attach_auth(
    core: &ClientCore,
    request: RequestBuilder,
    requirements: &[&[AuthScheme]],
) -> Result<RequestBuilder, Error<Infallible>> {
    if requirements.is_empty() {
        return Ok(request);
    }
    let Some(alternative) = requirements.iter().find(|alternative| {
        alternative.iter().all(|scheme| {
            // `mutualTLS` is satisfied by the transport's client certificate, so it never needs a
            // registered credential and never blocks an alternative from being chosen.
            matches!(scheme.kind, AuthKind::MutualTls) || core.credential(scheme.name).is_some()
        })
    }) else {
        let mut names: Vec<&str> = requirements
            .iter()
            .flat_map(|alternative| alternative.iter().map(|scheme| scheme.name))
            .collect();
        names.sort_unstable();
        names.dedup();
        return Err(Error::request_message(format!(
            "no registered credential satisfies the operation's security requirement \
             (schemes: {})",
            names.join(", ")
        )));
    };
    let mut request = request;
    for scheme in *alternative {
        if matches!(scheme.kind, AuthKind::MutualTls) {
            continue;
        }
        // Present by construction: the alternative was selected because every scheme resolves.
        let Some(credential) = core.credential(scheme.name) else {
            continue;
        };
        request = apply_credential(request, scheme, credential).await?;
    }
    Ok(request)
}

async fn apply_credential(
    request: RequestBuilder,
    scheme: &AuthScheme,
    credential: &Credential,
) -> Result<RequestBuilder, Error<Infallible>> {
    // A provider yields a single secret, usable anywhere a bearer token or apiKey fits.
    let token: Option<SecretString> = match credential {
        Credential::Bearer(secret) | Credential::ApiKey(secret) => Some(secret.clone()),
        Credential::Provider(provider) => {
            Some(provider().await.map_err(Error::request_construction)?)
        }
        Credential::Basic { .. } => None,
    };
    match scheme.kind {
        AuthKind::Basic => match credential {
            Credential::Basic { username, password } => {
                Ok(request.basic_auth(username, Some(password.expose_secret())))
            }
            _ => Err(credential_mismatch(scheme.name, "http basic")),
        },
        AuthKind::Bearer => match token {
            Some(token) => Ok(request.bearer_auth(token.expose_secret())),
            None => Err(credential_mismatch(scheme.name, "bearer")),
        },
        AuthKind::ApiKeyHeader(name) => match token {
            Some(token) => Ok(request.header(name, sensitive_value(token.expose_secret())?)),
            None => Err(credential_mismatch(scheme.name, "apiKey")),
        },
        AuthKind::ApiKeyQuery(name) => match token {
            Some(token) => Ok(request.query(&[(name, token.expose_secret())])),
            None => Err(credential_mismatch(scheme.name, "apiKey")),
        },
        // Satisfied by the transport; `attach_auth` never reaches this arm.
        AuthKind::MutualTls => Ok(request),
        AuthKind::ApiKeyCookie(name) => match token {
            Some(token) => {
                let cookie = format!("{name}={}", token.expose_secret());
                Ok(request.header(reqwest::header::COOKIE, sensitive_value(&cookie)?))
            }
            None => Err(credential_mismatch(scheme.name, "apiKey")),
        },
    }
}

fn sensitive_value(secret: &str) -> Result<HeaderValue, Error<Infallible>> {
    let mut value = HeaderValue::from_str(secret).map_err(Error::request_construction)?;
    value.set_sensitive(true);
    Ok(value)
}

fn credential_mismatch(scheme: &str, kind: &str) -> Error<Infallible> {
    Error::request_message(format!(
        "the credential registered for security scheme `{scheme}` cannot satisfy its `{kind}` type"
    ))
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
/// at `max_error_body`.
pub async fn decode_success<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    // Deserialization needs the whole body, so peak here is inherent to typed decoding; only what
    // the error *retains* is capped.
    match serde_json::from_slice::<T>(&body) {
        Ok(value) => Ok(ResponseValue::new(status, headers, value)),
        Err(error) => {
            let (body, truncated) = cap_body(body, core.config().max_error_body);
            Err(Error::Decode {
                path: error.to_string(),
                body,
                truncated,
            })
        }
    }
}

/// Decode a raw UTF-8 success body as the JSON string value described by a textual OpenAPI media
/// type. Converting through `Value::String` keeps generated string enums and string formats typed
/// while avoiding JSON's quote requirement on the wire.
pub async fn decode_success_text<T>(
    core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    match decode_text_body::<T>(&body) {
        Ok(value) => Ok(ResponseValue::new(status, headers, value)),
        Err(path) => {
            let (body, truncated) = cap_body(body, core.config().max_error_body);
            Err(Error::Decode {
                path,
                body,
                truncated,
            })
        }
    }
}

/// Decode a raw binary success body without attempting JSON deserialization.
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
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, truncated)) => {
            if documented.iter().any(|spec| spec.matches(status)) {
                match serde_json::from_slice::<E>(&body) {
                    Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
                    Err(error) => Error::Decode {
                        path: error.to_string(),
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

/// Classify a documented textual error body, preserving the same cap and taxonomy as JSON errors.
pub async fn classify_error_text<E>(
    core: &ClientCore,
    response: Response,
    documented: &[StatusSpec],
) -> Error<E>
where
    E: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    match read_capped(core, response).await {
        Ok((body, truncated)) => {
            if documented.iter().any(|spec| spec.matches(status)) {
                match decode_text_body::<E>(&body) {
                    Ok(value) => Error::Api(ResponseValue::new(status, headers, value)),
                    Err(path) => Error::Decode {
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
#[cfg(target_arch = "wasm32")]
async fn read_capped<E>(core: &ClientCore, response: Response) -> Result<(Bytes, bool), Error<E>> {
    let cap = core.config().max_error_body;
    let bytes = response.bytes().await.map_err(Error::from_reqwest)?;
    Ok(cap_body(bytes, cap))
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use secrecy::SecretString;

    use crate::{AuthKind, AuthScheme, ClientCore, Credential, TokenFuture};

    use super::attach_auth;
    use bytes::Bytes;

    /// The static-credential paths never actually suspend, so a single poll with a noop waker is
    /// enough — no async runtime needed in the runtime's own test suite.
    fn poll_ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future was not immediately ready"),
        }
    }

    fn core() -> ClientCore {
        ClientCore::new("https://example.com").unwrap()
    }

    fn get(core: &ClientCore) -> reqwest::RequestBuilder {
        core.http()
            .request(reqwest::Method::GET, "https://example.com/op")
    }

    const BEARER: &[AuthScheme] = &[AuthScheme {
        name: "token",
        kind: AuthKind::Bearer,
    }];

    #[test]
    fn attaches_bearer_credential() {
        let mut core = core();
        core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
        let request = poll_ready(attach_auth(&core, get(&core), &[BEARER]))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer t0k"
        );
    }

    #[test]
    fn attaches_provider_token_as_bearer() {
        let mut core = core();
        core.set_credential(
            "token",
            Credential::Provider(Arc::new(|| {
                Box::pin(async { Ok(SecretString::from("fresh")) }) as TokenFuture
            })),
        );
        let request = poll_ready(attach_auth(&core, get(&core), &[BEARER]))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer fresh"
        );
    }

    #[test]
    fn attaches_api_key_query_from_first_satisfiable_alternative() {
        let mut core = core();
        core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
        let request = poll_ready(attach_auth(
            &core,
            get(&core),
            &[
                BEARER,
                &[AuthScheme {
                    name: "key",
                    kind: AuthKind::ApiKeyQuery("api_key"),
                }],
            ],
        ))
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.url().query(), Some("api_key=k3y"));
    }

    #[test]
    fn empty_alternative_marks_security_optional() {
        let core = core();
        let request = poll_ready(attach_auth(&core, get(&core), &[BEARER, &[]]))
            .unwrap()
            .build()
            .unwrap();
        assert!(request.headers().is_empty());
    }

    #[test]
    fn missing_credential_fails_before_send() {
        let core = core();
        let error = poll_ready(attach_auth(&core, get(&core), &[BEARER])).unwrap_err();
        assert!(error.to_string().contains("request construction"));
        let source = std::error::Error::source(&error).unwrap();
        assert!(source.to_string().contains("token"), "{source}");
    }

    #[test]
    fn mismatched_credential_kind_fails() {
        let mut core = core();
        core.set_credential(
            "token",
            Credential::Basic {
                username: "u".to_owned(),
                password: SecretString::from("p"),
            },
        );
        let error = poll_ready(attach_auth(&core, get(&core), &[BEARER])).unwrap_err();
        let source = std::error::Error::source(&error).unwrap();
        assert!(source.to_string().contains("bearer"), "{source}");
    }

    #[test]
    fn api_key_header_is_sensitive() {
        let mut core = core();
        core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
        let request = poll_ready(attach_auth(
            &core,
            get(&core),
            &[&[AuthScheme {
                name: "key",
                kind: AuthKind::ApiKeyHeader("X-Api-Key"),
            }]],
        ))
        .unwrap()
        .build()
        .unwrap();
        let value = &request.headers()["X-Api-Key"];
        assert_eq!(value, "k3y");
        assert!(value.is_sensitive());
    }

    use super::{build_url, build_url_on, build_url_with_query_string, StatusSpec};

    fn core_at(base: &str) -> ClientCore {
        ClientCore::new(base).unwrap()
    }

    #[test]
    fn build_url_collapses_double_slash_at_join() {
        let core = core_at("https://example.com/");
        let url = build_url(&core, "/foo", &[]).unwrap();
        // Trailing base slash + leading path slash collapse to a single separator.
        assert_eq!(url.path(), "/foo");
        // An empty query must not stamp a trailing `?` onto the serialized URL.
        assert_eq!(url.as_str(), "https://example.com/foo");
    }

    #[test]
    fn build_url_preserves_base_path_prefix() {
        let core = core_at("https://example.com/api");
        let url = build_url(&core, "foo", &[]).unwrap();
        assert_eq!(url.path(), "/api/foo");
    }

    #[test]
    fn build_url_empty_path_keeps_base_path() {
        let prefixed = core_at("https://example.com/api");
        assert_eq!(build_url(&prefixed, "", &[]).unwrap().path(), "/api");

        let root = core_at("https://example.com");
        assert_eq!(build_url(&root, "", &[]).unwrap().path(), "/");
    }

    #[test]
    fn build_url_installs_pre_encoded_query_fragments_verbatim() {
        let core = core_at("https://example.com");
        // The caller has already encoded the data and left the style delimiter literal; the two
        // commas here must stay distinguishable all the way onto the wire.
        let url = build_url(
            &core,
            "/search",
            &["q=a%20b%26c".to_owned(), "tags=x%2Cy,z".to_owned()],
        )
        .unwrap();
        assert_eq!(url.query(), Some("q=a%20b%26c&tags=x%2Cy,z"));
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("q".to_owned(), "a b&c".to_owned()),
                ("tags".to_owned(), "x,y,z".to_owned()),
            ]
        );
    }

    #[test]
    fn build_url_appends_to_a_query_already_on_the_base_url() {
        let core = core_at("https://example.com?tenant=acme");
        let url = build_url(&core, "/search", &["q=rust".to_owned()]).unwrap();
        assert_eq!(url.query(), Some("tenant=acme&q=rust"));
    }

    #[test]
    fn build_url_keeps_matrix_and_label_prefixes_in_the_path() {
        let core = core_at("https://example.com");
        // `set_path` must not disturb `;`, `=`, `,` or an existing percent-triple.
        let url = build_url(&core, "/map/;position=B,150,R,100", &[]).unwrap();
        assert_eq!(url.path(), "/map/;position=B,150,R,100");
        let labelled = build_url(&core, "/files/.tar%2Egz", &[]).unwrap();
        assert_eq!(labelled.path(), "/files/.tar%2Egz");
    }

    #[test]
    fn build_url_on_replaces_the_base_with_an_absolute_server_override() {
        let core = core_at("https://example.com/api");
        let url = build_url_on(&core, Some("https://files.example.net/v2"), "/blobs", &[]).unwrap();
        assert_eq!(url.as_str(), "https://files.example.net/v2/blobs");
    }

    #[test]
    fn build_url_on_joins_a_relative_server_override_onto_the_base() {
        let core = core_at("https://example.com/api/");
        let url = build_url_on(&core, Some("../edge/"), "/blobs", &[]).unwrap();
        assert_eq!(url.as_str(), "https://example.com/edge/blobs");
    }

    #[test]
    fn build_url_with_query_string_installs_a_whole_query_verbatim() {
        let core = core_at("https://example.com?stale=server-value");
        // The whole-query value arrives already encoded by the generated method.
        let url = build_url_with_query_string(
            &core,
            "/search",
            &[],
            Some("%7B%22numbers%22%3A%5B1%2C2%5D%7D"),
        )
        .unwrap();
        assert_eq!(url.query(), Some("%7B%22numbers%22%3A%5B1%2C2%5D%7D"));
    }

    #[test]
    fn build_url_replaces_the_server_query_with_a_form_whole_query_string() {
        let core = core_at("https://example.com?stale=server-value");
        let url =
            build_url_with_query_string(&core, "/search", &["term=rust%20api".to_owned()], None)
                .unwrap();
        assert_eq!(url.query(), Some("term=rust%20api"));
    }

    #[test]
    fn status_spec_matches_exact_range_and_any() {
        use reqwest::StatusCode;

        assert!(StatusSpec::Exact(404).matches(StatusCode::NOT_FOUND));
        assert!(!StatusSpec::Exact(404).matches(StatusCode::INTERNAL_SERVER_ERROR));

        assert!(StatusSpec::Range(5).matches(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!StatusSpec::Range(5).matches(StatusCode::NOT_FOUND));

        assert!(StatusSpec::Any.matches(StatusCode::OK));
        assert!(StatusSpec::Any.matches(StatusCode::IM_A_TEAPOT));
    }

    use std::convert::Infallible;

    use super::{
        classify_error_bytes, classify_error_text, decode_success_bytes, decode_success_text,
        decode_text_body, read_error_body, read_success_body,
    };
    use crate::{Error, ResponseValue};

    /// Synthesize an in-memory `reqwest::Response` (no server, no runtime) so the body readers can be
    /// driven with a poll-once noop waker.
    fn json_response(status: u16, body: &str) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(body.to_owned())
                .expect("valid synthetic response"),
        )
    }

    fn raw_response(status: u16, body: impl Into<reqwest::Body>) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(body.into())
                .expect("valid synthetic response"),
        )
    }

    #[derive(serde::Deserialize, Debug, PartialEq)]
    enum TextChoice {
        #[serde(rename = "ready")]
        Ready,
    }

    #[test]
    fn textual_codec_uses_raw_utf8_as_a_typed_json_string() {
        assert_eq!(
            decode_text_body::<String>(b"not quoted").unwrap(),
            "not quoted"
        );
        assert_eq!(
            decode_text_body::<TextChoice>(b"ready").unwrap(),
            TextChoice::Ready
        );
        assert!(decode_text_body::<String>(&[0xff]).is_err());

        let value = poll_ready(decode_success_text::<String>(
            &core(),
            json_response(200, "<p>raw</p>"),
        ))
        .unwrap();
        assert_eq!(value.into_inner(), "<p>raw</p>");
    }

    #[test]
    fn binary_codec_preserves_success_and_documented_error_bytes() {
        let success = poll_ready(decode_success_bytes(
            &core(),
            raw_response(200, bytes::Bytes::from_static(b"\0raw\xff")),
        ))
        .unwrap();
        assert_eq!(&success.into_inner()[..], b"\0raw\xff");

        let error = poll_ready(classify_error_bytes::<bytes::Bytes>(
            &core(),
            raw_response(400, bytes::Bytes::from_static(b"bad\0")),
            &[StatusSpec::Exact(400)],
        ));
        match error {
            Error::Api(response) => assert_eq!(&response.into_inner()[..], b"bad\0"),
            other => panic!("expected raw API error, got {other:?}"),
        }
    }

    #[test]
    fn textual_error_codec_keeps_documented_status_semantics() {
        let error = poll_ready(classify_error_text::<String>(
            &core(),
            json_response(400, "plain failure"),
            &[StatusSpec::Exact(400)],
        ));
        match error {
            Error::Api(response) => assert_eq!(response.into_inner(), "plain failure"),
            other => panic!("expected textual API error, got {other:?}"),
        }
    }

    #[test]
    fn read_success_body_returns_status_and_bytes() {
        let response = json_response(201, r#"{"ok":true}"#);
        let (status, _headers, body) = poll_ready(read_success_body(response)).unwrap();
        assert_eq!(status, reqwest::StatusCode::CREATED);
        assert_eq!(&body[..], br#"{"ok":true}"#);
    }

    #[test]
    fn read_error_body_truncates_at_cap() {
        let mut core = core();
        core.config_mut().max_error_body = 4;
        let response = json_response(500, "0123456789");
        let (status, _headers, body, truncated) =
            poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
        assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(truncated);
        assert_eq!(body.len(), 4);
        assert_eq!(&body[..], b"0123");
    }

    // Stand-ins for a generated multi-status SUCCESS enum: two success statuses, distinct bodies.
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Created {
        id: u32,
    }
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Accepted {
        job: String,
    }
    #[derive(Debug, PartialEq)]
    enum SuccessEnum {
        Status200(Created),
        Status202(Accepted),
    }

    /// Mirror of the generated per-status success dispatch: read once, select the variant whose
    /// selector matches (vec order = precedence), decode into it, else an undocumented-success error.
    fn dispatch_success(
        response: reqwest::Response,
    ) -> Result<ResponseValue<SuccessEnum>, Error<Infallible>> {
        let (status, headers, body) = poll_ready(read_success_body(response))?;
        if StatusSpec::Exact(200).matches(status) {
            let value =
                serde_json::from_slice::<Created>(&body).map_err(|error| Error::Decode {
                    path: error.to_string(),
                    body: body.clone(),
                    truncated: false,
                })?;
            return Ok(ResponseValue::new(
                status,
                headers,
                SuccessEnum::Status200(value),
            ));
        }
        if StatusSpec::Exact(202).matches(status) {
            let value =
                serde_json::from_slice::<Accepted>(&body).map_err(|error| Error::Decode {
                    path: error.to_string(),
                    body: body.clone(),
                    truncated: false,
                })?;
            return Ok(ResponseValue::new(
                status,
                headers,
                SuccessEnum::Status202(value),
            ));
        }
        Err(Error::UnexpectedStatus {
            status,
            headers,
            body,
        })
    }

    #[test]
    fn success_dispatch_selects_variant_per_status() {
        let created = dispatch_success(json_response(200, r#"{"id":7}"#)).unwrap();
        assert_eq!(*created.inner(), SuccessEnum::Status200(Created { id: 7 }));
        let accepted = dispatch_success(json_response(202, r#"{"job":"j"}"#)).unwrap();
        assert_eq!(
            *accepted.inner(),
            SuccessEnum::Status202(Accepted {
                job: "j".to_owned()
            })
        );
    }

    #[test]
    fn success_dispatch_undocumented_status_has_no_untyped_fallback() {
        // 201 is a success status matching no documented variant → an unexpected-status error, never
        // a silent `serde_json::Value`.
        let error = dispatch_success(json_response(201, r#"{"id":1}"#)).unwrap_err();
        assert!(matches!(error, Error::UnexpectedStatus { .. }));
    }

    #[test]
    fn success_dispatch_parse_failure_is_decode() {
        let error = dispatch_success(json_response(200, "not json")).unwrap_err();
        assert!(matches!(error, Error::Decode { .. }));
    }

    // Stand-ins for a generated multi-status ERROR enum: an exact status plus a range that would
    // also cover it — precedence must prefer the exact selector (it is checked first).
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Conflict {
        conflict: String,
    }
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct ClientError {
        message: String,
    }
    #[derive(Debug, PartialEq)]
    enum ApiError {
        Status409(Conflict),
        Status4xx(ClientError),
    }

    /// Mirror of the generated per-status error classification: read capped, select by status (exact
    /// before range), decode → `Api`; a parse failure → `Decode`; an undocumented status →
    /// `UnexpectedStatus`.
    fn dispatch_error(response: reqwest::Response) -> Error<ApiError> {
        let core = core();
        let (status, headers, body, truncated) =
            match poll_ready(read_error_body::<ApiError>(&core, response)) {
                Ok(parts) => parts,
                Err(error) => return error,
            };
        if StatusSpec::Exact(409).matches(status) {
            return match serde_json::from_slice::<Conflict>(&body) {
                Ok(value) => Error::Api(ResponseValue::new(
                    status,
                    headers,
                    ApiError::Status409(value),
                )),
                Err(error) => Error::Decode {
                    path: error.to_string(),
                    body,
                    truncated,
                },
            };
        }
        if StatusSpec::Range(4).matches(status) {
            return match serde_json::from_slice::<ClientError>(&body) {
                Ok(value) => Error::Api(ResponseValue::new(
                    status,
                    headers,
                    ApiError::Status4xx(value),
                )),
                Err(error) => Error::Decode {
                    path: error.to_string(),
                    body,
                    truncated,
                },
            };
        }
        Error::UnexpectedStatus {
            status,
            headers,
            body,
        }
    }

    #[test]
    fn error_dispatch_exact_selector_beats_range() {
        // 409 matches both `Exact(409)` and `Range(4)`; the exact variant wins because it is tried
        // first, preserving spec precedence.
        match dispatch_error(json_response(409, r#"{"conflict":"dup"}"#)) {
            Error::Api(value) => assert_eq!(
                *value.inner(),
                ApiError::Status409(Conflict {
                    conflict: "dup".to_owned()
                })
            ),
            other => panic!("expected Api(Status409), got {other:?}"),
        }
    }

    #[test]
    fn error_dispatch_range_matches_other_4xx() {
        match dispatch_error(json_response(404, r#"{"message":"nope"}"#)) {
            Error::Api(value) => assert_eq!(
                *value.inner(),
                ApiError::Status4xx(ClientError {
                    message: "nope".to_owned()
                })
            ),
            other => panic!("expected Api(Status4xx), got {other:?}"),
        }
    }

    #[test]
    fn error_dispatch_undocumented_status_is_unexpected() {
        let error = dispatch_error(json_response(500, r#"{}"#));
        assert!(matches!(error, Error::UnexpectedStatus { .. }));
    }

    #[test]
    fn error_dispatch_parse_failure_is_decode() {
        let error = dispatch_error(json_response(409, "not json"));
        assert!(matches!(error, Error::Decode { .. }));
    }

    // --- error-body cap fixtures -------------------------------------------------------------
    //
    // `max_error_body` is documented as a cap on RETENTION (README, `config.rs`, and
    // `Error::Decode`'s own field docs). These pin all three ways that contract can be broken:
    // retaining a view into an oversized buffer, reading an unbounded body before capping, and
    // the success-decode paths ignoring the cap outright.

    /// A body far larger than the cap must not leave the original allocation reachable through the
    /// retained `Bytes`. `Bytes::slice` returns a refcounted view whose parent stays alive; a real
    /// copy is detached, which `try_into_mut` (unique ownership) reports through its capacity.
    #[test]
    fn capped_error_body_does_not_retain_the_oversized_allocation() {
        let mut core = core();
        core.config_mut().max_error_body = 8;
        let response = json_response(500, &"x".repeat(64 * 1024));
        let (_status, _headers, body, truncated) =
            poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
        assert!(truncated);
        assert_eq!(body.len(), 8);
        let owned = body
            .try_into_mut()
            .expect("retained body should be uniquely owned");
        assert_eq!(
            owned.capacity(),
            8,
            "retained body still points into the full response allocation"
        );
    }

    /// The cap must bound PEAK memory too: an oversized body has to stop being pulled once enough
    /// bytes are in hand, rather than being buffered whole and trimmed afterwards.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oversized_error_body_stops_being_read_at_the_cap() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// 1 KiB chunks against the 4 KiB cap below: four pulls reach the cap exactly, and the
        /// fifth is the one that proves the remainder is being dropped.
        const PULLS_FOR_4K_CAP: usize = 5;

        // A stream of 1 KiB chunks that counts how many were actually pulled. Always immediately
        // ready, so the poll-once waker is enough.
        struct Counting {
            remaining: usize,
            pulled: Arc<AtomicUsize>,
        }
        impl futures_core::Stream for Counting {
            type Item = Result<Bytes, std::io::Error>;
            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if self.remaining == 0 {
                    return Poll::Ready(None);
                }
                self.remaining -= 1;
                self.pulled.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Some(Ok(Bytes::from_static(&[b'x'; 1024]))))
            }
        }

        let pulled = Arc::new(AtomicUsize::new(0));
        let body = reqwest::Body::wrap_stream(Counting {
            remaining: 1024, // 1 MiB available
            pulled: Arc::clone(&pulled),
        });

        let mut core = core();
        core.config_mut().max_error_body = 4096; // 4 KiB cap

        let (_status, _headers, retained, truncated) = poll_ready(read_error_body::<
            std::convert::Infallible,
        >(
            &core, raw_response(500, body)
        ))
        .unwrap();

        assert!(truncated);
        assert_eq!(retained.len(), 4096);
        let pulled = pulled.load(Ordering::SeqCst);
        // Derived, not fitted: the loop exits at the first pull that puts the buffer past the cap,
        // so 1 KiB chunks against a 4 KiB cap is exactly five. Asserting the tight value is what
        // pins "stops at the first over-cap chunk" rather than merely "stops somewhere early".
        assert_eq!(
            pulled, PULLS_FOR_4K_CAP,
            "read {pulled} KiB chunks for a 4 KiB cap - the whole body was buffered"
        );
    }

    /// An UNDER-cap body must not pin the read buffer either. The incremental read grows a
    /// `BytesMut` by doubling and `freeze` hands it over at full capacity, so 40 KiB arriving in
    /// chunks under the 64 KiB default retains 64 KiB — measured, not hypothetical. Same contract
    /// violation as the oversized case, just bounded by the cap instead of by the server.
    ///
    /// The body has to arrive in MULTIPLE chunks to reproduce: a single-chunk body reserves
    /// exactly once and lands on an exact-size allocation, so it hides the defect.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_under_cap_body_does_not_retain_the_read_buffer() {
        let mut core = core();
        core.config_mut().max_error_body = 64 * 1024;
        // 40 x 1 KiB, all under the cap, so the read runs to EOF and nothing is truncated.
        // `futures-core` is the only stream dependency here and carries no combinators, so the
        // stream is hand-rolled; it is always immediately ready, which the poll-once waker needs.
        struct Chunks(usize);
        impl futures_core::Stream for Chunks {
            type Item = Result<Bytes, std::io::Error>;
            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if self.0 == 0 {
                    return Poll::Ready(None);
                }
                self.0 -= 1;
                Poll::Ready(Some(Ok(Bytes::from_static(&[b'x'; 1024]))))
            }
        }

        let body = reqwest::Body::wrap_stream(Chunks(40));
        let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
            std::convert::Infallible,
        >(
            &core, raw_response(500, body)
        ))
        .unwrap();
        assert!(!truncated);
        assert_eq!(body.len(), 40 * 1024);
        let owned = body
            .try_into_mut()
            .expect("retained body should be uniquely owned");
        assert_eq!(
            owned.capacity(),
            40 * 1024,
            "under-cap body still pins the doubled read buffer"
        );
    }

    /// A body of exactly `cap` bytes is not truncated. This pins `cap_body`'s truncation test:
    /// widening `body.len() > cap` to `>=` reports an exact-fit body as truncated. Verified by
    /// regressing it — that mutation fails this fixture and `a_zero_cap_retains_nothing`, whose
    /// empty body under a zero cap is the same `len == cap` case, and nothing else.
    #[test]
    fn a_body_of_exactly_the_cap_is_not_truncated() {
        let mut core = core();
        core.config_mut().max_error_body = 32;
        let response = json_response(500, &"x".repeat(32));
        let (_status, _headers, body, truncated) =
            poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
        assert!(!truncated, "an exact-fit body must not report truncation");
        assert_eq!(body.len(), 32);
    }

    /// A cap of zero retains nothing and still terminates. This is the only fixture that pins the
    /// read loop's own `<= cap` bound: narrowing it to `<` never enters the loop at all, so an
    /// over-cap body is silently reported as complete. It also covers `cap_body`'s truncation test
    /// from the other side, since an empty body under a zero cap is `len == cap`. Both verified by
    /// regressing each comparison.
    #[test]
    fn a_zero_cap_retains_nothing() {
        let mut core = core();
        core.config_mut().max_error_body = 0;

        let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
            std::convert::Infallible,
        >(
            &core, json_response(500, "body")
        ))
        .unwrap();
        assert!(truncated);
        assert!(body.is_empty());

        // An empty body under a zero cap is the one case that must NOT report truncation: nothing
        // was dropped.
        let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
            std::convert::Infallible,
        >(
            &core, json_response(500, "")
        ))
        .unwrap();
        assert!(!truncated, "nothing was dropped, so nothing was truncated");
        assert!(body.is_empty());
    }

    /// A SUCCESS response that fails to deserialize must also honour the cap: `Error::Decode`
    /// documents its body as retained "up to the configured cap", with `truncated` saying so.
    #[test]
    fn decode_failure_on_success_body_honours_the_cap() {
        let mut core = core();
        core.config_mut().max_error_body = 16;
        // Valid UTF-8, not valid JSON for `Created`, and far larger than the cap.
        let response = json_response(200, &"n".repeat(32 * 1024));
        match poll_ready(super::decode_success::<Created>(&core, response)) {
            Err(Error::Decode {
                body, truncated, ..
            }) => {
                assert!(truncated, "oversized decode body reported as untruncated");
                assert_eq!(body.len(), 16);
            }
            other => panic!("expected a capped Decode error, got {other:?}"),
        }
    }

    /// The textual codec shares the cap.
    #[test]
    fn textual_decode_failure_honours_the_cap() {
        let mut core = core();
        core.config_mut().max_error_body = 16;
        let response = json_response(200, &"n".repeat(32 * 1024));
        match poll_ready(super::decode_success_text::<TextChoice>(&core, response)) {
            Err(Error::Decode {
                body, truncated, ..
            }) => {
                assert!(truncated);
                assert_eq!(body.len(), 16);
            }
            other => panic!("expected a capped Decode error, got {other:?}"),
        }
    }
}
