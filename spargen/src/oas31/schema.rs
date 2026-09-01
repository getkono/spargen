use indexmap::IndexMap;

use crate::diag::Provenance;
use crate::source::SpannedValue;

/// A JSON Schema 2020-12 node under the default OAS 3.1 dialect.
///
/// Validation-only keywords are retained in [`validation`](Schema::validation) so the disposition
/// [`audit`](fn@super::audit) can W-warn them by pointer; shape keywords drive lowering to the IR.
#[derive(Debug, Clone)]
pub(crate) struct Schema {
    /// A boolean schema when this value appears in an OpenAPI position whose outer model stores a
    /// full `Schema` rather than [`SchemaOr`]. `true` accepts any value; `false` accepts none.
    pub(crate) boolean: Option<bool>,
    /// The `type` set, including type arrays and `"null"`.
    pub(crate) types: TypeSet,
    /// A `$ref`, if this node is a reference.
    pub(crate) reference: Option<String>,
    /// `properties`.
    pub(crate) properties: IndexMap<String, SchemaOr>,
    /// `required`.
    pub(crate) required: Vec<String>,
    /// `additionalProperties`.
    pub(crate) additional_properties: Option<Box<SchemaOr>>,
    /// `patternProperties`: key-regex → value schema. Lowering composes the value schemas into the
    /// object's typed overflow map (the key regex itself is validation-only and surfaced as `W001`).
    pub(crate) pattern_properties: IndexMap<String, SchemaOr>,
    /// `items`.
    pub(crate) items: Option<Box<SchemaOr>>,
    /// `prefixItems`.
    pub(crate) prefix_items: Vec<SchemaOr>,
    /// `allOf`.
    pub(crate) all_of: Vec<SchemaOr>,
    /// `oneOf`.
    pub(crate) one_of: Vec<SchemaOr>,
    /// `anyOf`.
    pub(crate) any_of: Vec<SchemaOr>,
    /// `discriminator`.
    pub(crate) discriminator: Option<Discriminator>,
    /// `$defs`.
    pub(crate) defs: IndexMap<String, SchemaOr>,
    /// Subschemas reached only through validation/applicator keywords that do not change the Rust
    /// storage shape. They are still parsed and audited so nested unsupported constructs cannot
    /// disappear silently.
    pub(crate) validation_children: Vec<(String, SchemaOr)>,
    /// `enum` values (spanned, so non-scalar members can be diagnosed).
    pub(crate) enum_values: Option<Vec<SpannedValue>>,
    /// `const` value.
    pub(crate) const_value: Option<SpannedValue>,
    /// `default` value (spanned, so a non-representable default can be diagnosed by pointer).
    pub(crate) default: Option<SpannedValue>,
    /// `format` (annotation vocabulary; drives feature-gated type mappings).
    pub(crate) format: Option<String>,
    /// `contentEncoding` (e.g. `base64` → bytes).
    pub(crate) content_encoding: Option<String>,
    /// `contentMediaType`, retained so OpenAPI 3.2 SSE `data` fields can declare embedded JSON.
    pub(crate) content_media_type: Option<String>,
    /// `contentSchema`, the schema of string-encoded content. It changes the generated payload
    /// shape only for the recognized OpenAPI 3.2 SSE `data` position.
    pub(crate) content_schema: Option<Box<SchemaOr>>,
    /// The OpenAPI `xml` object, if present — XML representation hints consumed only when the schema
    /// is used as an XML body.
    pub(crate) xml: Option<XmlHints>,
    /// Retained validation-only keywords (W-class).
    pub(crate) validation: ValidationKeywords,
    /// `deprecated`.
    pub(crate) deprecated: bool,
    /// `readOnly` (W-class annotation).
    pub(crate) read_only: bool,
    /// `writeOnly` (W-class annotation).
    pub(crate) write_only: bool,
    /// `title` → rustdoc.
    pub(crate) title: Option<String>,
    /// `description` → rustdoc.
    pub(crate) description: Option<String>,
    /// Where the schema came from.
    pub(crate) provenance: Provenance,
}

/// The OpenAPI `xml` object on a schema. `name`/`attribute` drive XML field renaming; the remaining
/// hints (`namespace`/`prefix`/`wrapped`) are retained only so lowering can warn (`W006`) that they
/// are ignored — quick-xml serde has no faithful representation for them.
#[derive(Debug, Clone, Default)]
pub(crate) struct XmlHints {
    /// `xml.name`: overrides the element/attribute wire name.
    pub(crate) name: Option<String>,
    /// `xml.attribute`: serialize as an XML attribute rather than a child element.
    pub(crate) attribute: bool,
    /// OpenAPI 3.2 `nodeType`.
    pub(crate) node_type: Option<String>,
    /// `xml.namespace`: an XML namespace URI (unsupported → `W006`).
    pub(crate) namespace: Option<String>,
    /// `xml.prefix`: a namespace prefix (unsupported → `W006`).
    pub(crate) prefix: Option<String>,
    /// `xml.wrapped`: wrap an array in an outer element (unsupported → `W006`).
    pub(crate) wrapped: bool,
}

impl Schema {
    /// Whether this schema constrains a value at all — neither its storage shape nor its
    /// validation.
    ///
    /// `{}` and `true` constrain nothing. Annotations (`default`, `title`, `description`,
    /// `deprecated`, `readOnly`/`writeOnly`) do not either: they are documented rather than
    /// enforced, and two entries differing only in prose decode identically. A *validation*
    /// keyword does count, even though it never changes the storage type — see
    /// [`ValidationKeywords::constrains_nothing`].
    ///
    /// This is what lets a caller tell "an alternative that decodes to exactly the same thing"
    /// from one that does not, before any of it has been lowered.
    ///
    /// The destructure is exhaustive on purpose: a field added to [`Schema`] fails to compile here
    /// rather than silently letting a constrained schema pass as unconstrained.
    pub(crate) fn constrains_nothing(&self) -> bool {
        let Schema {
            boolean,
            types,
            reference,
            properties,
            required,
            additional_properties,
            pattern_properties,
            items,
            prefix_items,
            all_of,
            one_of,
            any_of,
            discriminator,
            defs,
            validation_children,
            enum_values,
            const_value,
            format,
            content_encoding,
            content_media_type,
            content_schema,
            xml,
            validation,
            // Annotation-only: documented or reported, never typed.
            default: _,
            deprecated: _,
            read_only: _,
            write_only: _,
            title: _,
            description: _,
            provenance: _,
        } = self;
        // `false` is the always-false schema — the one boolean that does constrain, to nothing.
        !matches!(boolean, Some(false))
            && types.types.is_empty()
            && reference.is_none()
            && properties.is_empty()
            && required.is_empty()
            && additional_properties.is_none()
            && pattern_properties.is_empty()
            && items.is_none()
            && prefix_items.is_empty()
            && all_of.is_empty()
            && one_of.is_empty()
            && any_of.is_empty()
            && discriminator.is_none()
            && defs.is_empty()
            && validation_children.is_empty()
            && enum_values.is_none()
            && const_value.is_none()
            && format.is_none()
            && content_encoding.is_none()
            && content_media_type.is_none()
            && content_schema.is_none()
            && xml.is_none()
            // A validation keyword does not change the storage type, but it is a constraint the
            // document stated, so an entry carrying one is not interchangeable with one that
            // says nothing.
            && validation.constrains_nothing()
    }
}

/// A schema position that may be a boolean schema (`true`/`false`) or a full [`Schema`]. `{}` and
/// `true` are the untyped schemas that faithfully lower to `Any`.
#[derive(Debug, Clone)]
pub(crate) enum SchemaOr {
    /// A boolean schema.
    Bool(bool),
    /// A full schema node.
    Schema(Box<Schema>),
}

/// The `type` keyword's value set (a single type or a type array, possibly including `"null"`).
#[derive(Debug, Clone, Default)]
pub(crate) struct TypeSet {
    /// The declared JSON Schema types.
    pub(crate) types: Vec<JsonType>,
}

/// A JSON Schema primitive type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonType {
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
pub(crate) struct Discriminator {
    /// `propertyName`.
    pub(crate) property_name: String,
    /// `mapping`: discriminator value → schema name/`$ref`.
    pub(crate) mapping: IndexMap<String, String>,
    /// OpenAPI 3.2 `defaultMapping`: the schema to use when the discriminating property is absent
    /// or carries a value with no mapping.
    pub(crate) default_mapping: Option<String>,
}

/// The validation-only JSON Schema keywords spargen retains but does not enforce at runtime
/// (W-class). Present so the disposition audit can warn once per site; kept as a
/// representative surface (raw applicator keywords such as `if`/`then`/`else`, `not`,
/// `unevaluated*`, `propertyNames`, and `dependentSchemas`/`dependentRequired` are retained during
/// implementation).
#[derive(Debug, Clone, Default)]
pub(crate) struct ValidationKeywords {
    pub(crate) pattern: Option<String>,
    pub(crate) minimum: Option<f64>,
    pub(crate) maximum: Option<f64>,
    pub(crate) exclusive_minimum: Option<f64>,
    pub(crate) exclusive_maximum: Option<f64>,
    pub(crate) multiple_of: Option<f64>,
    pub(crate) min_length: Option<u64>,
    pub(crate) max_length: Option<u64>,
    pub(crate) min_items: Option<u64>,
    pub(crate) max_items: Option<u64>,
    pub(crate) unique_items: bool,
    pub(crate) min_properties: Option<u64>,
    pub(crate) max_properties: Option<u64>,
    /// Another JSON Schema validation/applicator keyword is present (`not`, `if`/`then`/`else`,
    /// `contains`, `dependent*`, `unevaluated*`, `propertyNames`, or content validation).
    pub(crate) other: bool,
}

impl ValidationKeywords {
    /// Whether no validation keyword is present at all.
    ///
    /// These keywords never change a body's storage type, so they are invisible to
    /// [`Schema::constrains_nothing`]'s question. They are still something the document
    /// *said*: a media alternative carrying `maxLength: 10` is not interchangeable with one that
    /// says nothing, even though both decode to the same Rust type. Proving two entries decode
    /// identically therefore requires this on top of the shape check.
    ///
    /// Destructured exhaustively so a new keyword fails to compile rather than silently passing.
    pub(crate) fn constrains_nothing(&self) -> bool {
        let ValidationKeywords {
            pattern,
            minimum,
            maximum,
            exclusive_minimum,
            exclusive_maximum,
            multiple_of,
            min_length,
            max_length,
            min_items,
            max_items,
            unique_items,
            min_properties,
            max_properties,
            other,
        } = self;
        pattern.is_none()
            && minimum.is_none()
            && maximum.is_none()
            && exclusive_minimum.is_none()
            && exclusive_maximum.is_none()
            && multiple_of.is_none()
            && min_length.is_none()
            && max_length.is_none()
            && min_items.is_none()
            && max_items.is_none()
            && !*unique_items
            && min_properties.is_none()
            && max_properties.is_none()
            && !*other
    }
}
