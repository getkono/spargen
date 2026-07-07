/// A security-scheme identifier (`components.securitySchemes` key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemeId(pub String);

/// A security scheme (matrix: Security). `http`/`apiKey` are fully plumbed; `oauth2`/`oidc` are
/// W-class — the scheme metadata is retained but tokens are supplied by the caller (PRD FR4, §5.5).
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
    /// `oauth2` — flow metadata only; token supplied by the caller.
    OAuth2(OAuthMeta),
    /// `openIdConnect` — metadata only; token supplied by the caller.
    OpenIdConnect(OidcMeta),
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

/// Retained `oauth2` flow metadata (flows are not implemented; PRD §5.5).
#[derive(Debug, Clone)]
pub struct OAuthMeta {
    /// The declared flow kinds (`authorizationCode`, `clientCredentials`, …).
    pub flows: Vec<String>,
}

/// Retained `openIdConnect` metadata.
#[derive(Debug, Clone)]
pub struct OidcMeta {
    /// The `openIdConnectUrl`.
    pub openid_connect_url: String,
}

/// One operation-level `security` requirement: an AND of schemes (each with its required scopes).
/// A list of these on an operation is an OR of alternatives.
#[derive(Debug, Clone)]
pub struct SecurityRequirement(pub Vec<(SchemeId, Vec<String>)>);
