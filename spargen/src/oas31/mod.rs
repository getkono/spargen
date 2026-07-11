//! # Subsystem: oas31
//! layer-deps: source, ir, diag
//!
//! The OAS 3.1.1 typed document model, structural/meta-schema validation, `$ref` resolution,
//! per-keyword disposition audit, and lowering `SpannedValue` → IR. The only subsystem that knows
//! OpenAPI 3.1 syntax; a future `oas32` sibling would lower into the same IR and
//! touch nothing downstream.
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
pub use schema::{Discriminator, JsonType, Schema, SchemaOr, TypeSet, ValidationKeywords};
