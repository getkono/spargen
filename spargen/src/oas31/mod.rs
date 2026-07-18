//! # Subsystem: oas31
//! layer-deps: source, ir, diag
//!
//! The OAS 3.1.x / 3.2.x typed document model, structural/meta-schema validation, `$ref`
//! resolution, per-keyword disposition audit, and lowering `SpannedValue` → IR. The only subsystem
//! that knows OpenAPI 3.1/3.2 syntax. OpenAPI 3.2 is a compatible superset of 3.1 (same JSON Schema
//! 2020-12 dialect), so it is accepted through this same frontend and lowers into the same IR: the
//! `QUERY` method is fully supported, and 3.2-only constructs that are not lowered (`$self`,
//! `additionalOperations`, `in: querystring`) are acknowledged with `W010` rather than dropped.
//!
//! Frontend flow: [`parse_document`] → [`MetaSchemaValidator::validate`] + [`audit`] → [`lower`],
//! with [`Resolver`] resolving `$ref`s throughout.

mod audit;
mod deserialize;
mod document;
mod lower;
mod metaschema;
mod resolve;
mod schema;

pub use audit::audit;
pub use deserialize::parse_document;
pub use document::{
    Components, Document, Info, MediaTypeObject, OperationObject, ParameterObject, PathItem, Paths,
    RefOr, Reference, RequestBodyObject, ResponseObject, ResponsesObject, SecurityRequirement,
    SecuritySchemeObject, Server,
};
pub use lower::lower;
pub use metaschema::MetaSchemaValidator;
pub use resolve::Resolver;
pub use schema::{
    Discriminator, JsonType, Schema, SchemaOr, TypeSet, ValidationKeywords, XmlHints,
};
