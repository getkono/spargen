use indexmap::IndexMap;

use crate::diag::Provenance;
use crate::ir::Method;

use super::Schema;

/// The typed OAS 3.1/3.2 document model. Built by
/// [`parse_document`](super::parse_document) from the span-preserving source tree; every node
/// retains provenance for diagnostics.
#[derive(Debug, Clone)]
pub struct Document {
    /// Whether the declared OpenAPI feature version is 3.2.x.
    pub is_oas32: bool,
    /// `info`.
    pub info: Info,
    /// `servers`.
    pub servers: Vec<Server>,
    /// `paths`.
    pub paths: Paths,
    /// `components`.
    pub components: Components,
    /// Top-level `security`.
    pub security: Vec<SecurityRequirement>,
    /// Top-level tag metadata, including OpenAPI 3.2 hierarchy fields.
    pub tags: Vec<Tag>,
    /// Provenance of the document root.
    pub provenance: Provenance,
}

/// Either an inline item or a `$ref` to one. Resolution is performed by the
/// [`Resolver`](super::Resolver); the frontend keeps refs symbolic until lowering.
#[derive(Debug, Clone)]
pub enum RefOr<T> {
    /// A `$ref`.
    Ref(Reference),
    /// An inline item.
    Item(T),
}

/// A `$ref` with its provenance, for precise unresolved-ref diagnostics.
#[derive(Debug, Clone)]
pub struct Reference {
    /// The raw reference string.
    pub reference: String,
    /// Where the reference occurred, including its source file.
    pub provenance: Provenance,
}

/// `info`.
#[derive(Debug, Clone)]
pub struct Info {
    pub title: String,
    pub version: String,
    pub summary: Option<String>,
    pub description: Option<String>,
}

/// A `servers` entry.
#[derive(Debug, Clone)]
pub struct Server {
    pub name: Option<String>,
    pub url: String,
    pub description: Option<String>,
}

/// `paths`: a map from path template to its item.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    pub items: IndexMap<String, PathItem>,
}

/// A `paths` entry: the per-method operations plus path-level shared parameters.
#[derive(Debug, Clone)]
pub struct PathItem {
    /// Operations keyed by HTTP method.
    pub operations: IndexMap<Method, OperationObject>,
    /// Parameters shared across all operations on this path.
    pub parameters: Vec<RefOr<ParameterObject>>,
}

/// An OAS Operation Object.
#[derive(Debug, Clone)]
pub struct OperationObject {
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub parameters: Vec<RefOr<ParameterObject>>,
    pub request_body: Option<RefOr<RequestBodyObject>>,
    pub responses: ResponsesObject,
    pub security: Option<Vec<SecurityRequirement>>,
    pub deprecated: bool,
    pub tags: Vec<String>,
    pub provenance: Provenance,
}

/// An OAS Parameter Object.
#[derive(Debug, Clone)]
pub struct ParameterObject {
    pub name: String,
    /// `in`: `path` / `query` / `header` / `cookie`.
    pub location: String,
    pub required: bool,
    pub deprecated: bool,
    pub style: Option<String>,
    pub explode: Option<bool>,
    pub allow_reserved: bool,
    /// A schema-typed parameter …
    pub schema: Option<RefOr<Schema>>,
    /// … or a `content`-typed one (media type → schema).
    pub content: IndexMap<String, MediaTypeObject>,
    pub provenance: Provenance,
}

/// An OAS Request Body Object.
#[derive(Debug, Clone)]
pub struct RequestBodyObject {
    /// Media type → schema.
    pub content: IndexMap<String, MediaTypeObject>,
    /// `required`, defaulting to `false`. Decides whether the generated method takes the body by
    /// value or as an `Option`.
    pub required: bool,
    pub provenance: Provenance,
}

/// An OAS Responses Object: per-status entries keyed by `"200"`, `"2XX"`, or `"default"`.
#[derive(Debug, Clone, Default)]
pub struct ResponsesObject {
    pub by_status: IndexMap<String, RefOr<ResponseObject>>,
    pub default: Option<RefOr<ResponseObject>>,
}

/// An OAS Response Object.
#[derive(Debug, Clone)]
pub struct ResponseObject {
    pub summary: Option<String>,
    pub description: Option<String>,
    /// Media type → schema.
    pub content: IndexMap<String, MediaTypeObject>,
    pub provenance: Provenance,
}

/// Top-level Tag Object metadata.
#[derive(Debug, Clone)]
pub struct Tag {
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub parent: Option<String>,
    pub kind: Option<String>,
    pub provenance: Provenance,
}

/// An OAS Media Type Object.
#[derive(Debug, Clone)]
pub struct MediaTypeObject {
    /// OpenAPI 3.2 allows a Media Type Object itself to be a Reference Object.
    pub reference: Option<Reference>,
    pub schema: Option<RefOr<Schema>>,
    /// OpenAPI 3.2 `itemSchema`: the per-item type for a sequential/streaming media
    /// (`text/event-stream`, `application/x-ndjson`). For a streaming response it supplies the
    /// streamed item type `T`; on a non-streaming media it is meaningless and acknowledged (`W010`).
    pub item_schema: Option<RefOr<Schema>>,
    /// `encoding`: per-property wire encoding, keyed by body-schema property name. Applies only to
    /// `multipart` and `application/x-www-form-urlencoded` content; the specification says it is
    /// ignored elsewhere.
    pub encoding: IndexMap<String, EncodingObject>,
    /// OpenAPI 3.2 `prefixEncoding`: positional encodings for an array-shaped `multipart` body.
    pub prefix_encoding: Vec<(EncodingObject, Provenance)>,
    /// OpenAPI 3.2 `itemEncoding`: the encoding applied to every remaining item of an
    /// array-shaped `multipart` body.
    pub item_encoding: Option<(EncodingObject, Provenance)>,
    pub provenance: Provenance,
}

/// An OAS Encoding Object: how one property of a form or multipart body reaches the wire.
///
/// The three RFC 6570 fields are `Option` because the specification switches modes on their
/// *presence*, not their value: any one of them explicitly set selects query-style serialization
/// and makes `contentType` inert; all three absent selects media-type serialization.
#[derive(Debug, Clone)]
pub struct EncodingObject {
    pub content_type: Option<String>,
    pub headers: IndexMap<String, RefOr<HeaderObject>>,
    pub style: Option<String>,
    pub explode: Option<bool>,
    pub allow_reserved: Option<bool>,
    /// Nested `encoding`/`prefixEncoding`/`itemEncoding` fields, retained so lowering can reject
    /// them with a message that names the offending field.
    pub nested: Vec<(String, Provenance)>,
    pub provenance: Provenance,
}

/// An OAS Header Object, as it appears under `encoding.headers` and `response.headers`.
///
/// A Header Object *describes* a header rather than carrying a value, so only a schema that pins
/// one — through `const`, or `default` in its absence — gives a client something to send.
#[derive(Debug, Clone)]
pub struct HeaderObject {
    pub schema: Option<RefOr<Schema>>,
}

/// `components`. Only the maps spargen consumes are modeled.
#[derive(Debug, Clone, Default)]
pub struct Components {
    pub schemas: IndexMap<String, RefOr<Schema>>,
    pub responses: IndexMap<String, RefOr<ResponseObject>>,
    pub parameters: IndexMap<String, RefOr<ParameterObject>>,
    pub request_bodies: IndexMap<String, RefOr<RequestBodyObject>>,
    pub media_types: IndexMap<String, MediaTypeObject>,
    pub security_schemes: IndexMap<String, RefOr<SecuritySchemeObject>>,
}

/// An OAS Security Scheme Object.
#[derive(Debug, Clone)]
pub struct SecuritySchemeObject {
    /// `type`: `http` / `apiKey` / `oauth2` / `openIdConnect`.
    pub scheme_type: String,
    /// `scheme` (for `http`).
    pub scheme: Option<String>,
    /// `in` (for `apiKey`).
    pub location: Option<String>,
    /// `name` (for `apiKey`).
    pub name: Option<String>,
}

/// A `security` requirement: scheme name → required scopes.
#[derive(Debug, Clone)]
pub struct SecurityRequirement(pub IndexMap<String, Vec<String>>);
