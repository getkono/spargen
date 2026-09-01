use indexmap::IndexMap;

use crate::diag::Provenance;
use crate::ir::Method;

use super::Schema;

/// The typed OAS 3.1/3.2 document model. Built by
/// [`parse_document`](super::parse_document) from the span-preserving source tree; every node
/// retains provenance for diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct Document {
    /// Whether the declared OpenAPI feature version is 3.2.x.
    pub(crate) is_oas32: bool,
    /// `info`.
    pub(crate) info: Info,
    /// `servers`.
    pub(crate) servers: Vec<Server>,
    /// `paths`.
    pub(crate) paths: Paths,
    /// `components`.
    pub(crate) components: Components,
    /// Top-level `security`.
    pub(crate) security: Vec<SecurityRequirement>,
    /// Top-level tag metadata, including OpenAPI 3.2 hierarchy fields.
    pub(crate) tags: Vec<Tag>,
    /// Provenance of the document root.
    pub(crate) provenance: Provenance,
}

/// Either an inline item or a `$ref` to one. Resolution is performed by the
/// [`Resolver`](super::Resolver); the frontend keeps refs symbolic until lowering.
#[derive(Debug, Clone)]
pub(crate) enum RefOr<T> {
    /// A `$ref`.
    Ref(Reference),
    /// An inline item.
    Item(T),
}

/// A `$ref` with its provenance, for precise unresolved-ref diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct Reference {
    /// The raw reference string.
    pub(crate) reference: String,
    /// A Reference Object `summary`/`description`, which document the *reference site* rather than
    /// the target. Retained so the override has a disposition instead of vanishing.
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    /// Where the reference occurred, including its source file.
    pub(crate) provenance: Provenance,
}

/// `info`.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    pub(crate) title: String,
    pub(crate) version: String,
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    /// `contact.name`/`url`/`email`, flattened into one displayable line.
    pub(crate) contact: Option<String>,
    /// `license.name` plus its SPDX `identifier` or `url`.
    pub(crate) license: Option<String>,
    /// `externalDocs.url`, with its description when one is given.
    pub(crate) external_docs: Option<String>,
}

/// A `servers` entry.
#[derive(Debug, Clone)]
pub(crate) struct Server {
    pub(crate) name: Option<String>,
    pub(crate) url: String,
    pub(crate) description: Option<String>,
    /// `variables`: substitutions for the `{braces}` in `url`. Unlike a Schema Object `default`,
    /// a Server Variable `default` genuinely changes the wire — the specification says it SHALL be
    /// sent when the caller supplies no alternative.
    pub(crate) variables: IndexMap<String, ServerVariable>,
    pub(crate) provenance: Provenance,
}

/// An OAS Server Variable Object.
#[derive(Debug, Clone)]
pub(crate) struct ServerVariable {
    /// The value sent when the caller supplies none. Required by the document schema.
    pub(crate) default: String,
    /// The closed set of permitted values, if the document declares one.
    pub(crate) enum_values: Vec<String>,
    pub(crate) description: Option<String>,
}

/// `paths`: a map from path template to its item.
#[derive(Debug, Clone, Default)]
pub(crate) struct Paths {
    pub(crate) items: IndexMap<String, PathItem>,
}

/// A `paths` entry: the per-method operations plus path-level shared parameters.
#[derive(Debug, Clone)]
pub(crate) struct PathItem {
    /// A Path Item `$ref`, which replaces this item with the referenced one.
    pub(crate) reference: Option<Reference>,
    /// Structural fields declared alongside a `$ref`. The specification leaves their interaction
    /// with the referenced item *undefined*, so they are rejected rather than guessed at.
    pub(crate) reference_siblings: Vec<String>,
    /// Operations keyed by HTTP method.
    pub(crate) operations: IndexMap<Method, OperationObject>,
    /// Parameters shared across all operations on this path.
    pub(crate) parameters: Vec<RefOr<ParameterObject>>,
    /// `servers`: an alternative base URL for every operation on this path, overriding the
    /// document's.
    pub(crate) servers: Vec<Server>,
    /// `summary`: applies to every operation on this path.
    pub(crate) summary: Option<String>,
    /// `description`: applies to every operation on this path.
    pub(crate) description: Option<String>,
}

/// An OAS Operation Object.
#[derive(Debug, Clone)]
pub(crate) struct OperationObject {
    pub(crate) operation_id: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) parameters: Vec<RefOr<ParameterObject>>,
    pub(crate) request_body: Option<RefOr<RequestBodyObject>>,
    pub(crate) responses: ResponsesObject,
    pub(crate) security: Option<Vec<SecurityRequirement>>,
    pub(crate) deprecated: bool,
    pub(crate) tags: Vec<String>,
    /// `servers`: an alternative base URL for this operation, overriding the path item's and the
    /// document's.
    pub(crate) servers: Vec<Server>,
    pub(crate) provenance: Provenance,
}

/// An OAS Parameter Object.
#[derive(Debug, Clone)]
pub(crate) struct ParameterObject {
    pub(crate) name: String,
    /// `in`: `path` / `query` / `header` / `cookie`.
    pub(crate) location: String,
    pub(crate) required: bool,
    pub(crate) deprecated: bool,
    pub(crate) style: Option<String>,
    pub(crate) explode: Option<bool>,
    pub(crate) allow_reserved: bool,
    /// `allowEmptyValue`. Deprecated in OpenAPI 3.2 and inert for a typed client: an absent
    /// optional parameter is simply not sent, so there is no case where a client would choose to
    /// send an empty string instead.
    pub(crate) allow_empty_value: bool,
    /// A schema-typed parameter …
    pub(crate) schema: Option<RefOr<Schema>>,
    /// … or a `content`-typed one (media type → schema).
    pub(crate) content: IndexMap<String, MediaTypeObject>,
    pub(crate) provenance: Provenance,
}

/// An OAS Request Body Object.
#[derive(Debug, Clone)]
pub(crate) struct RequestBodyObject {
    /// Media type → schema.
    pub(crate) content: IndexMap<String, MediaTypeObject>,
    /// `required`, defaulting to `false`. Decides whether the generated method takes the body by
    /// value or as an `Option`.
    pub(crate) required: bool,
    pub(crate) provenance: Provenance,
}

/// An OAS Responses Object: per-status entries keyed by `"200"`, `"2XX"`, or `"default"`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ResponsesObject {
    pub(crate) by_status: IndexMap<String, RefOr<ResponseObject>>,
    pub(crate) default: Option<RefOr<ResponseObject>>,
}

/// An OAS Response Object.
#[derive(Debug, Clone)]
pub(crate) struct ResponseObject {
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    /// Media type → schema.
    pub(crate) content: IndexMap<String, MediaTypeObject>,
    /// Documented response headers, keyed by header name.
    pub(crate) headers: IndexMap<String, RefOr<HeaderObject>>,
    pub(crate) provenance: Provenance,
}

/// Top-level Tag Object metadata.
#[derive(Debug, Clone)]
pub(crate) struct Tag {
    pub(crate) name: String,
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) provenance: Provenance,
}

/// An OAS Media Type Object.
#[derive(Debug, Clone)]
pub(crate) struct MediaTypeObject {
    /// OpenAPI 3.2 allows a Media Type Object itself to be a Reference Object.
    pub(crate) reference: Option<Reference>,
    pub(crate) schema: Option<RefOr<Schema>>,
    /// OpenAPI 3.2 `itemSchema`: the per-item type for a sequential/streaming media
    /// (`text/event-stream`, `application/x-ndjson`). For a streaming response it supplies the
    /// streamed item type `T`; on a non-streaming media it is meaningless and acknowledged (`W010`).
    pub(crate) item_schema: Option<RefOr<Schema>>,
    /// `encoding`: per-property wire encoding, keyed by body-schema property name. Applies only to
    /// `multipart` and `application/x-www-form-urlencoded` content; the specification says it is
    /// ignored elsewhere.
    pub(crate) encoding: IndexMap<String, EncodingObject>,
    /// OpenAPI 3.2 `prefixEncoding`: positional encodings for an array-shaped `multipart` body.
    pub(crate) prefix_encoding: Vec<(EncodingObject, Provenance)>,
    /// OpenAPI 3.2 `itemEncoding`: the encoding applied to every remaining item of an
    /// array-shaped `multipart` body.
    pub(crate) item_encoding: Option<(EncodingObject, Provenance)>,
    pub(crate) provenance: Provenance,
}

/// An OAS Encoding Object: how one property of a form or multipart body reaches the wire.
///
/// The three RFC 6570 fields are `Option` because the specification switches modes on their
/// *presence*, not their value: any one of them explicitly set selects query-style serialization
/// and makes `contentType` inert; all three absent selects media-type serialization.
#[derive(Debug, Clone)]
pub(crate) struct EncodingObject {
    pub(crate) content_type: Option<String>,
    pub(crate) headers: IndexMap<String, RefOr<HeaderObject>>,
    pub(crate) style: Option<String>,
    pub(crate) explode: Option<bool>,
    pub(crate) allow_reserved: Option<bool>,
    /// Nested `encoding`/`prefixEncoding`/`itemEncoding` fields, retained so lowering can reject
    /// them with a message that names the offending field.
    pub(crate) nested: Vec<(String, Provenance)>,
    pub(crate) provenance: Provenance,
}

/// An OAS Header Object, as it appears under `encoding.headers` and `response.headers`.
///
/// A Header Object *describes* a header rather than carrying a value, so only a schema that pins
/// one — through `const`, or `default` in its absence — gives a client something to send.
#[derive(Debug, Clone)]
pub(crate) struct HeaderObject {
    pub(crate) description: Option<String>,
    pub(crate) required: bool,
    pub(crate) deprecated: bool,
    /// `explode` for the `simple` style. A Header Object may only use `simple`, which the
    /// document schema already enforces, so the style itself is not modeled.
    pub(crate) explode: Option<bool>,
    pub(crate) schema: Option<RefOr<Schema>>,
    /// A `content`-typed header, as an alternative to `schema`.
    pub(crate) content: IndexMap<String, MediaTypeObject>,
    pub(crate) provenance: Provenance,
}

/// `components`. Only the maps spargen consumes are modeled.
#[derive(Debug, Clone, Default)]
pub(crate) struct Components {
    pub(crate) schemas: IndexMap<String, RefOr<Schema>>,
    pub(crate) responses: IndexMap<String, RefOr<ResponseObject>>,
    pub(crate) parameters: IndexMap<String, RefOr<ParameterObject>>,
    pub(crate) request_bodies: IndexMap<String, RefOr<RequestBodyObject>>,
    pub(crate) media_types: IndexMap<String, MediaTypeObject>,
    pub(crate) security_schemes: IndexMap<String, RefOr<SecuritySchemeObject>>,
    /// Reusable Path Items, referenced by a Path Item `$ref`.
    pub(crate) path_items: IndexMap<String, PathItem>,
    /// Reusable Header Objects, referenced from `response.headers` and `encoding.headers`.
    pub(crate) headers: IndexMap<String, RefOr<HeaderObject>>,
}

/// An OAS Security Scheme Object.
#[derive(Debug, Clone)]
pub(crate) struct SecuritySchemeObject {
    /// `type`: `http` / `apiKey` / `oauth2` / `openIdConnect` / `mutualTLS`.
    pub(crate) scheme_type: String,
    /// `scheme` (for `http`).
    pub(crate) scheme: Option<String>,
    /// `in` (for `apiKey`).
    pub(crate) location: Option<String>,
    /// `name` (for `apiKey`).
    pub(crate) name: Option<String>,
    /// `description`.
    pub(crate) description: Option<String>,
    /// `bearerFormat` (for `http` `bearer`) — a hint such as `JWT`.
    pub(crate) bearer_format: Option<String>,
    /// `openIdConnectUrl` (for `openIdConnect`).
    pub(crate) open_id_connect_url: Option<String>,
    /// OpenAPI 3.2 `oauth2MetadataUrl` (for `oauth2`).
    pub(crate) oauth2_metadata_url: Option<String>,
    /// `deprecated`.
    pub(crate) deprecated: bool,
    /// `flows` (for `oauth2`), in source order.
    pub(crate) flows: Vec<OAuthFlow>,
    pub(crate) provenance: Provenance,
}

/// One entry of an OAuth Flows Object. Documentation only: spargen attaches a caller-supplied
/// token as a bearer credential rather than driving a flow, so these fields describe *where* a
/// caller obtains that token.
#[derive(Debug, Clone)]
pub(crate) struct OAuthFlow {
    /// The flow name: `implicit`, `password`, `clientCredentials`, `authorizationCode`, or the
    /// OpenAPI 3.2 `deviceAuthorization`.
    pub(crate) name: String,
    /// `authorizationUrl`.
    pub(crate) authorization_url: Option<String>,
    /// `tokenUrl`.
    pub(crate) token_url: Option<String>,
    /// `refreshUrl`.
    pub(crate) refresh_url: Option<String>,
    /// OpenAPI 3.2 `deviceAuthorizationUrl`.
    pub(crate) device_authorization_url: Option<String>,
    /// `scopes`: name → description, in source order.
    pub(crate) scopes: Vec<(String, String)>,
}

/// A `security` requirement: scheme name → required scopes.
#[derive(Debug, Clone)]
pub(crate) struct SecurityRequirement(pub(crate) IndexMap<String, Vec<String>>);
