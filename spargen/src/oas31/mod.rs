//! # Subsystem: oas31
//! layer-deps: source, ir, name, diag
//!
//! The OAS 3.1.x / 3.2.x typed document model, structural/meta-schema validation, `$ref`
//! resolution, per-keyword disposition audit, and lowering `SpannedValue` → IR. The subsystem that
//! models OpenAPI 3.1/3.2 syntax; `compat` walks the document's layout to apply omit rules, and
//! `source` reads Security Requirement keys while it collects the references the bundle loads.
//! OpenAPI 3.2 is a compatible extension of 3.1 and retains the same JSON Schema 2020-12 dialect,
//! so both versions deliberately share this frontend and lower into the same IR.
//!
//! Frontend flow: [`MetaSchemaValidator::validate`] (the root document, and every file it reaches
//! through a `$ref` in an OpenAPI position) → [`parse_document`] → [`audit`](fn@audit) →
//! [`lower`](fn@lower), with [`Resolver`] resolving `$ref`s throughout.

mod audit;
mod deserialize;
mod discriminator;
mod document;
mod lower;
mod media;
mod metaschema;
mod resolve;
mod schema;
mod sse;

pub(crate) use audit::audit;
pub(crate) use deserialize::parse_document;
pub(crate) use document::{
    Components, Document, EncodingObject, HeaderObject, Info, MediaTypeObject, OAuthFlow,
    OperationObject, ParameterObject, PathItem, Paths, RefOr, Reference, RequestBodyObject,
    ResponseObject, ResponsesObject, SecurityRequirement, SecuritySchemeObject, Server,
    ServerVariable, Tag,
};
pub(crate) use lower::{lower, LowerOptions};
pub(crate) use metaschema::MetaSchemaValidator;
pub(crate) use resolve::Resolver;
pub(crate) use schema::{
    is_xml_name, Discriminator, JsonType, Schema, SchemaOr, TypeSet, ValidationKeywords, XmlHints,
};
