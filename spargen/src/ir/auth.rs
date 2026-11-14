/// A security-scheme identifier (`components.securitySchemes` key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SchemeId(pub(crate) String);

/// A declared security scheme: how a credential attaches, plus the documentation the specification
/// lets the scheme carry.
///
/// The documentation is rendered onto the scheme's credential setter on `Client` (`with_<scheme>`),
/// where a caller decides what to register and needs to know the token format, the flows that mint
/// one, the OpenID Connect discovery URL, and whether the scheme is deprecated.
#[derive(Debug, Clone)]
pub(crate) struct SecuritySchemeDef {
    /// How a credential for this scheme attaches to a request.
    pub(crate) kind: SecurityScheme,
    /// Ready-to-render rustdoc lines, in display order. Empty when the scheme documents nothing.
    pub(crate) docs: Vec<String>,
}

/// A security scheme (matrix: Security). `http`/`apiKey` attach their credential kinds;
/// `oauth2`/`openIdConnect` attach a caller-supplied token as bearer; `mutualTLS` attaches nothing
/// (`W011`). Scheme documentation travels in [`SecuritySchemeDef::docs`].
#[derive(Debug, Clone)]
pub(crate) enum SecurityScheme {
    /// `http` scheme (`bearer` or `basic`).
    Http(HttpScheme),
    /// `apiKey` scheme carried in a header, query parameter, or cookie.
    ApiKey {
        /// Where the key is sent.
        location: ApiKeyLoc,
        /// The header/query/cookie name.
        name: String,
    },
    /// `oauth2` — the caller-supplied token attaches as a bearer credential; the caller obtains it
    /// through whichever flow the scheme documents.
    OAuth2,
    /// `openIdConnect` — the caller-supplied token attaches as a bearer credential.
    OpenIdConnect,
    /// `mutualTLS` — satisfied by the client certificate on the injected `reqwest::Client`. The
    /// requirement is always satisfiable and the request carries nothing extra.
    MutualTls,
}

/// What a caller registers for a security scheme, which fixes the arguments of the scheme's typed
/// setters on the generated client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialShape {
    /// One secret token: `http bearer`, `apiKey`, `oauth2` and `openIdConnect`. Registered as a
    /// static token, a token provider, or a refreshable token.
    Token,
    /// A username and password: `http basic`.
    Basic,
}

impl SecurityScheme {
    /// The credential a caller registers for this scheme, or `None` for `mutualTLS`, which the
    /// transport's client certificate satisfies without reading a registered credential.
    pub(crate) fn credential_shape(&self) -> Option<CredentialShape> {
        match self {
            SecurityScheme::Http(HttpScheme::Bearer)
            | SecurityScheme::ApiKey { .. }
            | SecurityScheme::OAuth2
            | SecurityScheme::OpenIdConnect => Some(CredentialShape::Token),
            SecurityScheme::Http(HttpScheme::Basic) => Some(CredentialShape::Basic),
            SecurityScheme::MutualTls => None,
        }
    }
}

/// The `http` scheme kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpScheme {
    /// `scheme: bearer`.
    Bearer,
    /// `scheme: basic`.
    Basic,
}

/// Where an `apiKey` is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiKeyLoc {
    Header,
    Query,
    Cookie,
}

/// One operation-level `security` requirement: an AND of schemes. A list of these on an operation
/// is an OR of alternatives. A requirement's scopes are the server's to check against the token it
/// is sent, so the IR keeps only the schemes, which select the credentials to attach.
#[derive(Debug, Clone)]
pub(crate) struct SecurityRequirement(pub(crate) Vec<SchemeId>);
