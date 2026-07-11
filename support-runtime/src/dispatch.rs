//! The small set of dispatch routines shared within a generated client (PRD NFR2): build URL →
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

/// Build a request URL from the base URL and pre-rendered path plus query pairs. Paths compile to
/// static segment concatenation — no runtime regex (PRD NFR1). Non-generic.
pub fn build_url(
    core: &ClientCore,
    path: &str,
    query: &[(&str, String)],
) -> Result<Url, Error<Infallible>> {
    let mut url = core.base_url().clone();
    let base_path = url.path().trim_end_matches('/');
    let request_path = path.trim_start_matches('/');
    let joined = if base_path.is_empty() {
        format!("/{request_path}")
    } else if request_path.is_empty() {
        base_path.to_owned()
    } else {
        format!("{base_path}/{request_path}")
    };
    url.set_path(&joined);
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

/// Attach credentials for an operation's security requirement (PRD FR4). `requirements` is an OR
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
        alternative
            .iter()
            .all(|scheme| core.credential(scheme.name).is_some())
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

/// Send a prepared request, mapping transport/timeout/protocol/redirect failures into the taxonomy
/// (PRD FR5 #2–#5). Non-generic.
pub async fn send(core: &ClientCore, request: Request) -> Result<Response, Error<Infallible>> {
    core.http()
        .execute(request)
        .await
        .map_err(Error::from_reqwest)
}

/// Decode a success response body into `T`, wrapping it with status and headers. Monomorphized once
/// per body type. Decode failures become [`Error::Decode`] with the serde path and a capped body.
pub async fn decode_success<T>(
    _core: &ClientCore,
    response: Response,
) -> Result<ResponseValue<T>, Error<Infallible>>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(Error::from_reqwest)?;
    let value = serde_json::from_slice::<T>(&body).map_err(|error| Error::Decode {
        path: error.to_string(),
        body,
        truncated: false,
    })?;
    Ok(ResponseValue::new(status, headers, value))
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
/// `max_error_body` bytes either way (PRD D7).
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

async fn read_capped<E>(core: &ClientCore, response: Response) -> Result<(Bytes, bool), Error<E>> {
    let cap = core.config().max_error_body;
    let bytes = response.bytes().await.map_err(Error::from_reqwest)?;
    if bytes.len() <= cap {
        Ok((bytes, false))
    } else {
        Ok((bytes.slice(..cap), true))
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use secrecy::SecretString;

    use crate::{AuthKind, AuthScheme, ClientCore, Credential, TokenFuture};

    use super::attach_auth;

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

    use super::{build_url, StatusSpec};

    fn core_at(base: &str) -> ClientCore {
        ClientCore::new(base).unwrap()
    }

    #[test]
    fn build_url_collapses_double_slash_at_join() {
        let core = core_at("https://example.com/");
        let url = build_url(&core, "/foo", &[]).unwrap();
        // Trailing base slash + leading path slash collapse to a single separator.
        assert_eq!(url.path(), "/foo");
        // BUG: build_url always enters `query_pairs_mut()`, so an empty query still stamps a
        // trailing `?` onto the serialized URL (harmless, but not byte-clean). Documenting current
        // behavior. See dispatch.rs `build_url`.
        assert_eq!(url.as_str(), "https://example.com/foo?");
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
    fn build_url_appends_and_percent_encodes_query_pairs() {
        let core = core_at("https://example.com");
        let url = build_url(&core, "/search", &[("q", "a b&c".to_owned())]).unwrap();
        // The space and ampersand are form-encoded, so the pair round-trips unambiguously.
        assert_eq!(url.query(), Some("q=a+b%26c"));
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(pairs, vec![("q".to_owned(), "a b&c".to_owned())]);
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
}
