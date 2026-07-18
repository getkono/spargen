use indexmap::IndexMap;

use crate::diag::Provenance;
use crate::source::SpannedValue;

/// A JSON Schema 2020-12 node under the default OAS 3.1 dialect.
///
/// Validation-only keywords are retained in [`validation`](Schema::validation) so the disposition
/// [`audit`](super::audit) can W-warn them by pointer; shape keywords drive lowering to the IR.
#[derive(Debug, Clone)]
pub struct Schema {
    /// The `type` set, including type arrays and `"null"`.
    pub types: TypeSet,
    /// A `$ref`, if this node is a reference.
    pub reference: Option<String>,
    /// `properties`.
    pub properties: IndexMap<String, SchemaOr>,
    /// `required`.
    pub required: Vec<String>,
    /// `additionalProperties`.
    pub additional_properties: Option<Box<SchemaOr>>,
    /// `patternProperties`: key-regex → value schema. Lowering composes the value schemas into the
    /// object's typed overflow map (the key regex itself is validation-only and surfaced as `W001`).
    pub pattern_properties: IndexMap<String, SchemaOr>,
    /// `items`.
    pub items: Option<Box<SchemaOr>>,
    /// `prefixItems`.
    pub prefix_items: Vec<SchemaOr>,
    /// `allOf`.
    pub all_of: Vec<SchemaOr>,
    /// `oneOf`.
    pub one_of: Vec<SchemaOr>,
    /// `anyOf`.
    pub any_of: Vec<SchemaOr>,
    /// `discriminator`.
    pub discriminator: Option<Discriminator>,
    /// `$defs`.
    pub defs: IndexMap<String, SchemaOr>,
    /// `enum` values (spanned, so non-scalar members can be diagnosed).
    pub enum_values: Option<Vec<SpannedValue>>,
    /// `const` value.
    pub const_value: Option<SpannedValue>,
    /// `default` value (spanned, so a non-representable default can be diagnosed by pointer).
    pub default: Option<SpannedValue>,
    /// `format` (annotation vocabulary; drives feature-gated type mappings).
    pub format: Option<String>,
    /// `contentEncoding` (e.g. `base64` → bytes).
    pub content_encoding: Option<String>,
    /// The OpenAPI `xml` object, if present — XML representation hints consumed only when the schema
    /// is used as an XML body.
    pub xml: Option<XmlHints>,
    /// Retained validation-only keywords (W-class).
    pub validation: ValidationKeywords,
    /// `deprecated`.
    pub deprecated: bool,
    /// `readOnly` (W-class annotation).
    pub read_only: bool,
    /// `writeOnly` (W-class annotation).
    pub write_only: bool,
    /// `title` → rustdoc.
    pub title: Option<String>,
    /// `description` → rustdoc.
    pub description: Option<String>,
    /// Where the schema came from.
    pub provenance: Provenance,
}

/// The OpenAPI `xml` object on a schema. `name`/`attribute` drive XML field renaming; the remaining
/// hints (`namespace`/`prefix`/`wrapped`) are retained only so lowering can warn (`W006`) that they
/// are ignored — quick-xml serde has no faithful representation for them.
#[derive(Debug, Clone, Default)]
pub struct XmlHints {
    /// `xml.name`: overrides the element/attribute wire name.
    pub name: Option<String>,
    /// `xml.attribute`: serialize as an XML attribute rather than a child element.
    pub attribute: bool,
    /// `xml.namespace`: an XML namespace URI (unsupported → `W006`).
    pub namespace: Option<String>,
    /// `xml.prefix`: a namespace prefix (unsupported → `W006`).
    pub prefix: Option<String>,
    /// `xml.wrapped`: wrap an array in an outer element (unsupported → `W006`).
    pub wrapped: bool,
}

/// A schema position that may be a boolean schema (`true`/`false`) or a full [`Schema`]. `{}` and
/// `true` are the untyped schemas that faithfully lower to `Any`.
#[derive(Debug, Clone)]
pub enum SchemaOr {
    /// A boolean schema.
    Bool(bool),
    /// A full schema node.
    Schema(Box<Schema>),
}

/// The `type` keyword's value set (a single type or a type array, possibly including `"null"`).
#[derive(Debug, Clone, Default)]
pub struct TypeSet {
    /// The declared JSON Schema types.
    pub types: Vec<JsonType>,
}

/// A JSON Schema primitive type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    Null,
    Boolean,
    Object,
    Array,
    Number,
    Integer,
    String,
}

/// An OAS `discriminator` object, consumed by discriminated-union lowering to build an
/// internally-tagged enum: `property_name` is the serde tag field and `mapping` supplies each
/// variant's tag value (falling back to the variant's `$ref` component name).
#[derive(Debug, Clone)]
pub struct Discriminator {
    /// `propertyName`.
    pub property_name: String,
    /// `mapping`: discriminator value → schema name/`$ref`.
    pub mapping: IndexMap<String, String>,
}

/// The validation-only JSON Schema keywords spargen retains but does not enforce at runtime
/// (W-class). Present so the disposition audit can warn once per site; kept as a
/// representative surface (raw applicator keywords such as `if`/`then`/`else`, `not`,
/// `unevaluated*`, `propertyNames`, and `dependentSchemas`/`dependentRequired` are retained during
/// implementation).
#[derive(Debug, Clone, Default)]
pub struct ValidationKeywords {
    pub pattern: Option<String>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub exclusive_minimum: Option<f64>,
    pub exclusive_maximum: Option<f64>,
    pub multiple_of: Option<f64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
}
