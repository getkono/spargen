/// A security-scheme identifier (`components.securitySchemes` key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemeId(pub String);

/// A declared security scheme: how a credential attaches, plus the documentation the specification
/// lets the scheme carry.
///
/// The documentation is rendered onto `Client::with_credential`, which is where a caller decides
/// what to register and needs to know the token format, the flows that mint one, and whether the
/// scheme is deprecated.
#[derive(Debug, Clone)]
pub struct SecuritySchemeDef {
    /// How a credential for this scheme attaches to a request.
    pub kind: SecurityScheme,
    /// Ready-to-render rustdoc lines, in display order. Empty when the scheme documents nothing.
    pub docs: Vec<String>,
}

/// A security scheme (matrix: Security). `http`/`apiKey` are fully plumbed; `oauth2`/`oidc` are
/// W-class — the scheme metadata is retained but tokens are supplied by the caller.
#[derive(Debug, Clone)]
pub enum SecurityScheme {
    /// `http` scheme (`bearer` or `basic`).
    Http(HttpScheme),
    /// `apiKey` scheme carried in a header, query parameter, or cookie.
    ApiKey {
        /// Where the key is sent.
        location: ApiKeyLoc,
        /// The header/query/cookie name.
        name: String,
    },
    /// `oauth2` — flows are not implemented; the caller-supplied token attaches as a bearer
    /// credential.
    OAuth2,
    /// `openIdConnect` — the caller-supplied token attaches as a bearer credential.
    OpenIdConnect,
    /// `mutualTLS` — satisfied by the client certificate on the injected `reqwest::Client`. The
    /// requirement is always satisfiable and the request carries nothing extra.
    MutualTls,
}

/// The `http` scheme kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpScheme {
    /// `scheme: bearer`.
    Bearer,
    /// `scheme: basic`.
    Basic,
}

/// Where an `apiKey` is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyLoc {
    Header,
    Query,
    Cookie,
}

/// One operation-level `security` requirement: an AND of schemes (each with its required scopes).
/// A list of these on an operation is an OR of alternatives.
#[derive(Debug, Clone)]
pub struct SecurityRequirement(pub Vec<(SchemeId, Vec<String>)>);
