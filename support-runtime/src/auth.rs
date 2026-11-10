use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use bytes::Bytes;
use reqwest::header::HeaderValue;
use reqwest::RequestBuilder;

use crate::wasm::{BoxFuture, MaybeSend, MaybeSync};
use crate::{ClientCore, Error, HttpBody, RequestError};

// Secret handling is delegated to `secrecy` rather than hand-rolled: it zeroizes on drop and
// redacts `Debug`. Re-exported so generated code and consumers use one vocabulary.
pub use secrecy::{ExposeSecret, SecretString};

/// A failure from an async token provider.
#[derive(Debug)]
pub struct AuthError {
    message: String,
}

impl AuthError {
    /// Build an authentication-provider failure from a displayable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthError {}

/// The future returned by a [`TokenProvider`]. `Send` off `wasm32` and not on it, as the `wasm`
/// module explains.
pub type TokenFuture = BoxFuture<'static, Result<SecretString, AuthError>>;

/// An async callback that yields a fresh credential, for rotating tokens. `Send + Sync` on native
/// (the provider is shared across tasks); vacuous on `wasm32`, matching [`TokenFuture`].
#[cfg(not(target_arch = "wasm32"))]
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture + Send + Sync>;
/// An async callback that yields a fresh credential (the wasm variant: no `Send + Sync`).
#[cfg(target_arch = "wasm32")]
pub type TokenProvider = Arc<dyn Fn() -> TokenFuture>;

/// Decides whether a failed response rejected the token a [`Credential::Refreshable`] attached
/// (for example a 401 whose problem `code` is `expired_token`), rather than refusing the call for
/// some other reason, such as a wrong password. It reads the response with its body buffered as
/// the operation's error reads it: whole when it holds at most `ClientConfig::max_error_body`
/// bytes, and otherwise only up to the first chunk past that cap.
/// `Send + Sync` on native; vacuous on `wasm32`, matching [`TokenProvider`].
#[cfg(not(target_arch = "wasm32"))]
pub type RejectionCheck = Arc<dyn Fn(&http::Response<Bytes>) -> bool + Send + Sync>;
/// Decides whether a failed response rejected the token it carried (the wasm variant: no
/// `Send + Sync`).
#[cfg(target_arch = "wasm32")]
pub type RejectionCheck = Arc<dyn Fn(&http::Response<Bytes>) -> bool>;

/// An async callback that, given the token a rejected call carried, yields a fresh one, for a
/// [`Credential::Refreshable`]. `Send + Sync` on native; vacuous on `wasm32`, matching
/// [`TokenProvider`].
#[cfg(not(target_arch = "wasm32"))]
pub type TokenRefresher = Arc<dyn Fn(SecretString) -> TokenFuture + Send + Sync>;
/// An async callback that, given a rejected token, yields a fresh one (the wasm variant: no
/// `Send + Sync`).
#[cfg(target_arch = "wasm32")]
pub type TokenRefresher = Arc<dyn Fn(SecretString) -> TokenFuture>;

/// A [`TokenProvider`] from an async closure, without boxing its future by hand:
/// `token_provider(|| async { Ok(SecretString::from("t0k")) })`.
pub fn token_provider<F, Fut>(provider: F) -> TokenProvider
where
    F: Fn() -> Fut + MaybeSend + MaybeSync + 'static,
    Fut: Future<Output = Result<SecretString, AuthError>> + MaybeSend + 'static,
{
    Arc::new(move || -> TokenFuture { Box::pin(provider()) })
}

/// A per-scheme credential supplied at client construction: a static secret or a token provider
/// for rotation.
///
/// Registration checks nothing: a credential is stored under whatever `securitySchemes` key it is
/// given, and is matched against that scheme's [`AuthKind`] only when an operation that requires
/// the scheme is called. Every failure below is therefore an error of that call, raised as a
/// request-construction error before anything is sent — never a 401 from the server:
///
/// - no alternative of the operation's requirement has every scheme registered:
///   [`super::RequestError::MissingCredential`];
/// - the selected alternative has a credential of a variant its scheme cannot carry:
///   [`super::RequestError::CredentialMismatch`];
/// - the selected alternative's [`Credential::Provider`], or the token provider of its
///   [`Credential::Refreshable`], fails: [`super::RequestError::CredentialProvider`].
///
/// Which variant a scheme accepts:
///
/// - `http basic` ([`AuthKind::Basic`]) accepts only [`Credential::Basic`];
/// - every scheme that carries a token — `http bearer`, `oauth2` and `openIdConnect` (all three
///   [`AuthKind::Bearer`]) and `apiKey` in a header, query or cookie — accepts [`Credential::Bearer`],
///   [`Credential::ApiKey`], [`Credential::Provider`] and [`Credential::Refreshable`], and rejects
///   [`Credential::Basic`];
/// - `mutualTLS` ([`AuthKind::MutualTls`]) is satisfied by the transport and never reads a
///   registered credential, so one registered under it is ignored.
#[derive(Clone)]
pub enum Credential {
    /// `Authorization: Bearer <token>`.
    Bearer(SecretString),
    /// HTTP basic auth.
    Basic {
        /// The username.
        username: String,
        /// The password.
        password: SecretString,
    },
    /// An `apiKey` value.
    ApiKey(SecretString),
    /// A rotating token supplied on demand.
    ///
    /// The provider yields one secret, so it stands in for [`Credential::Bearer`] or
    /// [`Credential::ApiKey`]: it is accepted under every scheme that carries a token (`http
    /// bearer`, `oauth2`, `openIdConnect`, and `apiKey` in a header, query or cookie) and is asked
    /// for a fresh token each time a call attaches it. A provider that fails fails that call as
    /// [`super::RequestError::CredentialProvider`], with its [`AuthError`] as the source.
    ///
    /// It is **not** accepted under an `http basic` scheme, which needs a username and password
    /// ([`Credential::Basic`]). There the call fails as
    /// [`super::RequestError::CredentialMismatch`] with `required: "http basic"` and
    /// `registered: "Provider"`, and the provider is never called. Under a `mutualTLS` scheme it is
    /// ignored and never called, since the transport's client certificate satisfies the scheme.
    Provider(TokenProvider),
    /// A token the runtime refreshes and replays once when the server rejects it.
    ///
    /// It attaches like [`Credential::Provider`]: `token` is asked for a token each time a call
    /// attaches it, under the same schemes, with the same failures. What it adds is the reactive
    /// case. When the call's response is not a success and `is_rejection` reads it as a rejection
    /// of the token, `on_rejected` is given the token that was sent and yields a fresh one, and the
    /// call is sent once more with it. `token` is not asked again for that replay; a refresh that
    /// should outlive it is `on_rejected`'s to store where `token` reads it.
    ///
    /// The replay happens at most once per call, and only when the request's body could be cloned
    /// before it was sent (a streaming upload cannot be). The call then returns the replay's
    /// response, unless the replay is rejected again. In every other case — a body that cannot be
    /// cloned, `on_rejected` failing, the replay failing to send, or the replay rejected again —
    /// the call returns the original response, which decodes as the operation's typed error as
    /// it would have without a refresh. A streaming response's reconnect resends the token the
    /// first request carried.
    Refreshable {
        /// Yields the token each call attaches.
        token: TokenProvider,
        /// Whether a failed response means the token it carried is stale.
        is_rejection: RejectionCheck,
        /// Given the stale token, yields a fresh one.
        on_rejected: TokenRefresher,
    },
}

/// How a security scheme carries its credential on the wire. Generated code builds these as
/// static tables from the spec's `securitySchemes`; `oauth2`/`openIdConnect` schemes attach their
/// caller-supplied token as a bearer credential.
#[derive(Debug, Clone, Copy)]
pub enum AuthKind {
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// `Authorization: Basic <base64>`.
    Basic,
    /// An `apiKey` sent as the named request header.
    ApiKeyHeader(&'static str),
    /// An `apiKey` sent as the named query parameter.
    ApiKeyQuery(&'static str),
    /// An `apiKey` sent as the named cookie.
    ApiKeyCookie(&'static str),
    /// `mutualTLS`. Satisfied by the client certificate configured on the injected
    /// `reqwest::Client`, so the request carries nothing extra and any credential registered under
    /// its scheme is never read.
    MutualTls,
}

/// One scheme reference inside an operation's security requirement: the `securitySchemes` key the
/// credential is registered under, plus how it is carried.
#[derive(Debug, Clone, Copy)]
pub struct AuthScheme {
    /// The `components.securitySchemes` key.
    pub name: &'static str,
    /// The wire carrier.
    pub kind: AuthKind,
}

impl Credential {
    /// The variant's name, as `Debug` renders it and as
    /// [`super::RequestError::CredentialMismatch`] reports it in `registered`.
    fn kind_name(&self) -> &'static str {
        match self {
            Credential::Bearer(_) => "Bearer",
            Credential::Basic { .. } => "Basic",
            Credential::ApiKey(_) => "ApiKey",
            Credential::Provider(_) => "Provider",
            Credential::Refreshable { .. } => "Refreshable",
        }
    }

    /// A [`Credential::Refreshable`] from closures: `token` as for [`token_provider`],
    /// `is_rejection` reading a failed response, and `on_rejected` an async closure given the
    /// rejected token.
    pub fn refreshable<P, R, Fut>(token: TokenProvider, is_rejection: P, on_rejected: R) -> Self
    where
        P: Fn(&http::Response<Bytes>) -> bool + MaybeSend + MaybeSync + 'static,
        R: Fn(SecretString) -> Fut + MaybeSend + MaybeSync + 'static,
        Fut: Future<Output = Result<SecretString, AuthError>> + MaybeSend + 'static,
    {
        Credential::Refreshable {
            token,
            is_rejection: Arc::new(is_rejection),
            on_rejected: Arc::new(move |stale| -> TokenFuture { Box::pin(on_rejected(stale)) }),
        }
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secrets are redacted throughout.
        write!(f, "Credential::{}(***)", self.kind_name())
    }
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
    let Some(chosen) = select(core, requirements)? else {
        return Ok(request);
    };
    let mut request = request;
    for scheme in chosen {
        if let Resolution::Registered(credential) = resolve(core, scheme) {
            (request, _) = apply_credential(request, scheme, credential, None).await?;
        }
    }
    Ok(request)
}

/// What a call needs to send itself once more after a [`Credential::Refreshable`] it carried was
/// rejected: a clone of the request taken before any credential was attached, the security
/// alternative that was selected, and the token each refreshable credential of it attached.
/// [`attach_auth_replayable`] builds it and [`replay_rejected`] spends it.
pub struct Replay {
    /// The request as it stood before [`attach_auth_replayable`] attached anything.
    unauthenticated: RequestBuilder,
    /// The selected alternative's schemes, in declared order.
    alternative: Vec<AuthScheme>,
    /// Each refreshable scheme of `alternative`, with the token it attached.
    sent: Vec<(&'static str, SecretString)>,
    /// The extensions of the request as it was sent, which the replay carries too: empty until
    /// [`crate::execute`] records them with [`Self::carrying`].
    extensions: http::Extensions,
}

impl Replay {
    /// This replay, carrying `extensions` (those of the request as it was sent, an operation's
    /// `ops::Operation` among them) onto the request it sends.
    pub(crate) fn carrying(mut self, extensions: &http::Extensions) -> Self {
        self.extensions = extensions.clone();
        self
    }
}

impl std::fmt::Debug for Replay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The request carries no credential yet, but its headers may hold other secrets, and the
        // tokens are secrets: neither is printed.
        let schemes: Vec<&str> = self.alternative.iter().map(|scheme| scheme.name).collect();
        f.debug_struct("Replay")
            .field("alternative", &schemes)
            .finish_non_exhaustive()
    }
}

/// [`attach_auth`], also returning the [`Replay`] [`replay_rejected`] needs when the selected
/// alternative holds a [`Credential::Refreshable`] under a scheme that takes a token. There is no
/// `Replay` when it holds none, or when the request's body cannot be cloned (a streaming upload),
/// so a call that cannot be replayed costs nothing extra. Selection and every failure are
/// [`attach_auth`]'s.
pub async fn attach_auth_replayable(
    core: &ClientCore,
    request: RequestBuilder,
    requirements: &[&[AuthScheme]],
) -> Result<(RequestBuilder, Option<Replay>), Error<Infallible>> {
    let Some(chosen) = select(core, requirements)? else {
        return Ok((request, None));
    };
    let refreshable = chosen.iter().any(|scheme| {
        takes_token(scheme)
            && matches!(
                resolve(core, scheme),
                Resolution::Registered(Credential::Refreshable { .. })
            )
    });
    let unauthenticated = if refreshable {
        request.try_clone()
    } else {
        None
    };
    let mut request = request;
    let mut sent = Vec::new();
    for scheme in chosen {
        if let Resolution::Registered(credential) = resolve(core, scheme) {
            let attached;
            (request, attached) = apply_credential(request, scheme, credential, None).await?;
            if let (Credential::Refreshable { .. }, Some(token)) = (credential, attached) {
                sent.push((scheme.name, token));
            }
        }
    }
    let replay = unauthenticated.map(|unauthenticated| Replay {
        unauthenticated,
        alternative: chosen.to_vec(),
        sent,
        extensions: http::Extensions::new(),
    });
    Ok((request, replay))
}

/// Replay a call once if a [`Credential::Refreshable`] it carried was rejected, given the
/// response the call received and the [`Replay`] [`attach_auth_replayable`] returned for it.
/// `streaming` sends the replay as [`crate::send_streaming`] would, and otherwise as
/// [`crate::send`].
///
/// A success, or a call with no `Replay`, is returned as it came. Otherwise the response's body is
/// read into memory as the operation's error would read it, up to the first chunk past
/// `ClientConfig::max_error_body`, and each refreshable credential whose `is_rejection` holds for it
/// is handed the token it sent to its `on_rejected`. When none holds, the response is returned.
/// When one does, the request is signed again — a refreshed scheme with its fresh token, another
/// refreshable scheme with the token it sent, any other credential attached as it was the first
/// time — and sent once more. Its response is returned unless it is rejected again.
///
/// A refresh that fails, a credential that can no longer attach, a replay that fails to send or
/// whose failed body cannot be read, and a replay rejected again each return the original response,
/// so the operation's typed error decodes from it as if no refresh had been tried. The one error
/// returned is a failure to read the original response's body, which reading it to classify the
/// error would have met as well.
pub async fn replay_rejected(
    core: &ClientCore,
    response: http::Response<HttpBody>,
    replay: Option<Replay>,
    streaming: bool,
) -> Result<http::Response<HttpBody>, Error<Infallible>> {
    let Some(replay) = replay else {
        return Ok(response);
    };
    if response.status().is_success() {
        return Ok(response);
    }
    let original = buffered(core, response)
        .await
        .map_err(Error::from_transport)?;
    let mut tokens = Vec::with_capacity(replay.sent.len());
    let mut refreshed = Vec::new();
    for (name, sent) in replay.sent {
        let token = match core.credential(name) {
            Some(Credential::Refreshable {
                is_rejection,
                on_rejected,
                ..
            }) if is_rejection(&original) => match on_rejected(sent).await {
                Ok(fresh) => {
                    refreshed.push(name);
                    fresh
                }
                Err(_) => return Ok(unbuffered(original)),
            },
            _ => sent,
        };
        tokens.push((name, token));
    }
    if refreshed.is_empty() {
        return Ok(unbuffered(original));
    }
    let mut request = replay.unauthenticated;
    for scheme in &replay.alternative {
        let Resolution::Registered(credential) = resolve(core, scheme) else {
            continue;
        };
        let supplied = tokens
            .iter()
            .find(|(name, _)| *name == scheme.name)
            .map(|(_, token)| token);
        match apply_credential(request, scheme, credential, supplied).await {
            Ok((signed, _)) => request = signed,
            Err(_) => return Ok(unbuffered(original)),
        }
    }
    let Ok(request) = request.build() else {
        return Ok(unbuffered(original));
    };
    let Ok(mut request) = crate::prepare(core, request, streaming) else {
        return Ok(unbuffered(original));
    };
    request.extensions_mut().extend(replay.extensions);
    let sent = crate::dispatch::execute_once(core, request, streaming).await;
    let Ok(replayed) = sent else {
        return Ok(unbuffered(original));
    };
    if replayed.status().is_success() {
        return Ok(replayed);
    }
    let Ok(replayed) = buffered(core, replayed).await else {
        return Ok(unbuffered(original));
    };
    let rejected_again = refreshed.iter().any(|name| {
        matches!(
            core.credential(name),
            Some(Credential::Refreshable { is_rejection, .. }) if is_rejection(&replayed)
        )
    });
    Ok(unbuffered(if rejected_again { original } else { replayed }))
}

/// A failed `response` with its body read into memory for a [`RejectionCheck`] to read, as the
/// error classifier reads it: up to the first chunk past the client's `max_error_body`, the rest
/// abandoned. A body that held more keeps more than `max_error_body` bytes, so the operation's
/// error classifies it as truncated, as it would have the whole body.
async fn buffered(
    core: &ClientCore,
    response: http::Response<HttpBody>,
) -> Result<http::Response<Bytes>, crate::TransportError> {
    let (parts, body) = response.into_parts();
    let body = crate::dispatch::read_prefix(core.config().max_error_body, body).await?;
    Ok(http::Response::from_parts(parts, body))
}

/// A [`buffered`] response back on the transport seam, its body in memory.
fn unbuffered(response: http::Response<Bytes>) -> http::Response<HttpBody> {
    response.map(HttpBody::from)
}

/// The security alternative [`attach_auth`] attaches: `None` for an operation with no
/// requirement, and otherwise the first alternative with no scheme missing, or the
/// [`RequestError::MissingCredential`] that reports each alternative's missing schemes.
fn select<'r>(
    core: &ClientCore,
    requirements: &[&'r [AuthScheme]],
) -> Result<Option<&'r [AuthScheme]>, Error<Infallible>> {
    // No requirement attaches nothing, rather than failing as a `MissingCredential` naming no
    // scheme.
    if requirements.is_empty() {
        return Ok(None);
    }
    // Selection and reporting read the one predicate `is_missing`: an alternative is chosen
    // exactly when none of its schemes is missing, and its missing schemes are what
    // `MissingCredential` reports for it. So every reported inner list is non-empty by
    // construction, and the outer list is non-empty because `requirements` is. The report is
    // built only once no alternative was chosen.
    let is_missing = |scheme: &AuthScheme| matches!(resolve(core, scheme), Resolution::Missing);
    let Some(chosen) = requirements
        .iter()
        .find(|alternative| !alternative.iter().any(is_missing))
    else {
        let alternatives = requirements
            .iter()
            .map(|alternative| {
                alternative
                    .iter()
                    .filter(|scheme| is_missing(scheme))
                    .map(|scheme| scheme.name)
                    .collect()
            })
            .collect();
        return Err(Error::RequestConstruction(
            RequestError::MissingCredential { alternatives },
        ));
    };
    Ok(Some(*chosen))
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

/// Whether `scheme` carries a token, and so asks a provider for one. `attach_auth` skips
/// `MutualTls` before reaching [`apply_credential`], so its term is never evaluated from there; it
/// is deliberate defence, so a caller that bypasses the skip still never fetches a token.
fn takes_token(scheme: &AuthScheme) -> bool {
    !matches!(scheme.kind, AuthKind::Basic | AuthKind::MutualTls)
}

/// Attach `credential` under `scheme`, returning the request and the token a provider (a
/// [`Credential::Provider`] or a [`Credential::Refreshable`]'s `token`) yielded for it. `supplied`
/// stands in for that provider: a replay passes the token it already holds, and the provider is
/// not asked.
async fn apply_credential(
    request: RequestBuilder,
    scheme: &AuthScheme,
    credential: &Credential,
    supplied: Option<&SecretString>,
) -> Result<(RequestBuilder, Option<SecretString>), Error<Infallible>> {
    // A provider yields a single secret, usable anywhere a bearer token or apiKey fits. Only a kind
    // that takes a token asks it for one, so a provider registered under a scheme that takes none
    // is the same mismatch whether or not a refresh would have succeeded.
    let takes_token = takes_token(scheme);
    // A static secret is borrowed; only a provider's fresh token is owned, here.
    let mut fetched: Option<SecretString> = None;
    let token: Option<&SecretString> = match credential {
        Credential::Bearer(secret) | Credential::ApiKey(secret) => Some(secret),
        Credential::Provider(provider)
        | Credential::Refreshable {
            token: provider, ..
        } if takes_token => {
            let token = match supplied {
                Some(token) => token.clone(),
                None => provider().await.map_err(|source| {
                    Error::RequestConstruction(RequestError::CredentialProvider {
                        scheme: scheme.name,
                        source,
                    })
                })?,
            };
            Some(&*fetched.insert(token))
        }
        Credential::Provider(_) | Credential::Refreshable { .. } | Credential::Basic { .. } => None,
    };
    let request = attach_token(request, scheme, credential, token)?;
    Ok((request, fetched))
}

/// Install `token` (or `credential`'s username and password, for `http basic`) where `scheme`
/// carries it, or report the mismatch of a credential its scheme cannot carry.
fn attach_token(
    request: RequestBuilder,
    scheme: &AuthScheme,
    credential: &Credential,
    token: Option<&SecretString>,
) -> Result<RequestBuilder, Error<Infallible>> {
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
            Some(token) => append_query_pair(request, name, token.expose_secret()),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
        // Satisfied by the transport; `attach_auth` never reaches this arm.
        AuthKind::MutualTls => Ok(request),
        AuthKind::ApiKeyCookie(name) => match token {
            Some(token) => append_cookie(request, name, token.expose_secret()),
            None => Err(credential_mismatch(scheme.name, "apiKey", credential)),
        },
    }
}

/// Append `name=value` to the request URL's query, `application/x-www-form-urlencoded` as reqwest's
/// `RequestBuilder::query` encodes it, after whatever query the URL already carries. Written
/// against the built request because `query` needs reqwest's `query` feature, which would add
/// `serde_urlencoded` to every consumer's graph. A builder that already failed reports that
/// failure here, as the request-construction error `build` would have raised.
fn append_query_pair(
    request: RequestBuilder,
    name: &str,
    value: &str,
) -> Result<RequestBuilder, Error<Infallible>> {
    let (client, request) = request.build_split();
    let mut request = request.map_err(Error::request_construction)?;
    request.url_mut().query_pairs_mut().append_pair(name, value);
    Ok(RequestBuilder::from_parts(client, request))
}

/// Add `name=value` to the request's one `Cookie` header. RFC 6265 §5.4 allows a user agent a
/// single `Cookie` header, and `RequestBuilder::header` appends a second, so the cookie joins any
/// value the request already carries (the operation's cookie parameters, or an earlier cookie
/// apiKey of the same alternative) after a `; `. The merged value carries the secret, so it is
/// marked sensitive. A builder that already failed reports that failure here, as the
/// request-construction error `build` would have raised.
fn append_cookie(
    request: RequestBuilder,
    name: &str,
    value: &str,
) -> Result<RequestBuilder, Error<Infallible>> {
    let (client, request) = request.build_split();
    let mut request = request.map_err(Error::request_construction)?;
    let headers = request.headers_mut();
    let mut cookie = Vec::new();
    for existing in headers.get_all(reqwest::header::COOKIE) {
        cookie.extend_from_slice(existing.as_bytes());
        cookie.extend_from_slice(b"; ");
    }
    cookie.extend_from_slice(name.as_bytes());
    cookie.push(b'=');
    cookie.extend_from_slice(value.as_bytes());
    let mut merged = HeaderValue::from_bytes(&cookie).map_err(Error::request_construction)?;
    merged.set_sensitive(true);
    headers.insert(reqwest::header::COOKIE, merged);
    Ok(RequestBuilder::from_parts(client, request))
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
    Error::RequestConstruction(RequestError::CredentialMismatch {
        scheme,
        required,
        registered: credential.kind_name(),
    })
}

#[cfg(test)]
mod tests;
