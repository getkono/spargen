use indexmap::IndexMap;

use crate::diag::{JsonPointer, Provenance};

use super::Docs;

/// A stable, dense identifier for a type in the [`TypeGraph`]. Ordered so codegen can emit items
/// deterministically regardless of input map ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// The graph of named/derived types the API references. Owns every [`TypeDef`]; a [`Ty`] is a
/// lightweight reference into it.
#[derive(Debug, Clone, Default)]
pub struct TypeGraph {
    defs: IndexMap<TypeId, TypeDef>,
}

impl TypeGraph {
    /// Insert a definition and return its stable id.
    pub fn insert(&mut self, def: TypeDef) -> TypeId {
        let id = TypeId(self.defs.len() as u32);
        self.defs.insert(id, def);
        id
    }

    /// Reserve a dense id backed by a placeholder def, to be replaced via [`fill`](Self::fill)
    /// before lowering finishes.
    ///
    /// Reserving a component's root id *before* its body is lowered lets a `$ref` back-edge
    /// discovered mid-body box a reference to the (not-yet-filled) root, breaking the cycle so a
    /// recursive schema generates a finite Rust type instead of being rejected. Every reserved id
    /// must be filled before it can be emitted; the placeholder is a valid (if meaningless) def so
    /// a leak on an already-failing (rejected) lowering is harmless rather than a sentinel.
    pub fn reserve(&mut self) -> TypeId {
        self.insert(TypeDef {
            name_hint: String::new(),
            kind: TypeKind::Any,
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
        })
    }

    /// Replace the def at an already-present id (typically a [`reserve`](Self::reserve)d
    /// placeholder). The id's position — and therefore [`iter`](Self::iter) order — is preserved.
    pub fn fill(&mut self, id: TypeId, def: TypeDef) {
        debug_assert!(self.defs.contains_key(&id), "fill of an unreserved id");
        self.defs.insert(id, def);
    }

    /// Remove and return the most recently inserted `(id, def)` pair. Used to lift a component
    /// root — always the last def inserted while lowering its body — into its reserved id, which
    /// keeps ids dense (the freed id is immediately reused by the next insert).
    pub fn pop_last(&mut self) -> Option<(TypeId, TypeDef)> {
        self.defs.pop()
    }

    /// The definition for `id`, if present.
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.defs.get(&id)
    }

    /// A mutable borrow of the definition for `id`, for in-place post-lowering adjustments that do
    /// not change ids or insertion order (e.g. suppressing an XML field rename on a shared type).
    pub fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeDef> {
        self.defs.get_mut(&id)
    }

    /// Iterate `(id, def)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.defs.iter().map(|(id, def)| (*id, def))
    }
}

/// A named or structurally-derived type definition.
#[derive(Debug, Clone)]
pub struct TypeDef {
    /// The preferred wire/spec name; the Rust identifier is allocated later by `name`.
    pub name_hint: String,
    /// The type's structure.
    pub kind: TypeKind,
    /// Documentation lowered to rustdoc.
    pub docs: Docs,
    /// Where the type came from.
    pub provenance: Provenance,
}

/// A reference to a type, plus the two shape modifiers that ride on a use site rather than the
/// definition: nullability (from `"null"` in a type array) and boxing (to break `$ref` cycles →
/// `Box`, matrix: Schema shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ty {
    /// The referenced definition.
    pub id: TypeId,
    /// Whether `null` is an accepted value (`Option<T>`).
    pub nullable: bool,
    /// Whether the reference must be boxed to break a type cycle.
    pub boxed: bool,
}

/// The structure of a [`TypeDef`].
///
/// Invariant: a typed schema is never silently degraded to `serde_json::Value`. [`Any`]
/// appears only where the spec itself is untyped (`{}` / `true` schemas) — faithful, not lossy.
///
/// [`Any`]: TypeKind::Any
#[derive(Debug, Clone)]
pub enum TypeKind {
    /// A scalar primitive.
    Primitive(Prim),
    /// An object with named fields.
    Struct(Struct),
    /// A homogeneous scalar `enum`/`const` set.
    Enum(ScalarEnum),
    /// A homogeneous array (`items`).
    Array(Box<Ty>),
    /// A fixed-length heterogeneous tuple (`prefixItems`).
    Tuple(Vec<Ty>),
    /// Raw bytes (`octet-stream` / `contentEncoding: base64`).
    Bytes,
    /// A tagged or structurally-disjoint union (`oneOf`/`anyOf`).
    Union(Union),
    /// An untyped value (`{}` / `true` schema). Faithful representation of an untyped spec node.
    Any,
}

/// A `oneOf`/`anyOf` union lowered to a Rust enum. Never `serde(untagged)` and never degraded to
/// `serde_json::Value`: it is emitted either as an internally-tagged enum (a `discriminator`) or as
/// an enum with a content-inspecting custom `Deserialize`/`Serialize` proven statically disjoint.
#[derive(Debug, Clone)]
pub struct Union {
    /// The variants, in spec (source) order.
    pub variants: Vec<UnionVariant>,
    /// How the union is (de)serialized.
    pub strategy: UnionStrategy,
}

/// One variant of a [`Union`]: a name hint (allocated to a Rust variant identifier by `name`) and
/// the variant's payload type.
#[derive(Debug, Clone)]
pub struct UnionVariant {
    /// The preferred variant name; the Rust identifier is allocated by `name` (keyed by this hint).
    pub name_hint: String,
    /// The variant's payload type.
    pub ty: Ty,
}

/// The (de)serialization strategy of a [`Union`].
#[derive(Debug, Clone)]
pub enum UnionStrategy {
    /// A `discriminator` → a custom `Deserialize`/`Serialize` that reads/writes the tag field on a
    /// buffered `serde_json::Value` (NOT serde's `#[serde(tag = ...)]`, which would consume the tag
    /// out of the buffer and break variants that declare the discriminator as a required property).
    /// Each variant carries the tag value that selects it.
    Discriminated {
        /// The discriminator `propertyName` — the tag field read from / written into the object.
        tag_field: String,
        /// The tag value per variant, parallel to [`Union::variants`].
        tags: Vec<String>,
    },
    /// No discriminator, but the variants were proven statically disjoint → a custom
    /// content-inspecting `Deserialize`/`Serialize`. Each variant carries the feature that
    /// unambiguously selects it.
    Disjoint {
        /// The discriminating feature per variant, parallel to [`Union::variants`].
        features: Vec<DisjointFeature>,
    },
}

/// The statically-proven feature that selects a [`UnionStrategy::Disjoint`] variant when inspecting
/// a buffered `serde_json::Value`.
#[derive(Debug, Clone)]
pub enum DisjointFeature {
    /// The variant occupies a distinct JSON primitive category (dispatch on `Value::is_*`).
    JsonType(JsonCategory),
    /// The variant is an object carrying a required property whose name appears in no other variant
    /// (dispatch on `Value::get(key).is_some()`).
    RequiredKey(String),
}

/// A JSON primitive category for disjointness by JSON type. `number` and `integer` share
/// [`Number`](JsonCategory::Number) — they overlap on the wire, so they are never disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonCategory {
    /// A JSON string.
    String,
    /// A JSON number (integer or floating-point).
    Number,
    /// A JSON boolean.
    Boolean,
    /// A JSON array.
    Array,
    /// A JSON object.
    Object,
}

/// A scalar primitive. Numeric wire types map to fixed Rust scalars; `format`-based type mappings
/// are feature-gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    /// `boolean`.
    Bool,
    /// `string`.
    String,
    /// `format: int32` → `i32`.
    I32,
    /// `format: int64` / unformatted `integer` → `i64`.
    I64,
    /// `number` → `f64`.
    F64,
    /// `format: uuid` → `uuid::Uuid` (feature `uuid`, else `String`).
    Uuid,
    /// `format: date-time` → `time::OffsetDateTime` (feature `time`, else `String`).
    DateTime,
    /// `format: date` → `time::Date` (feature `time`, else `String`).
    Date,
}

/// An object type: named fields plus an `additionalProperties` policy.
#[derive(Debug, Clone)]
pub struct Struct {
    /// The declared properties, in deterministic order.
    pub fields: Vec<Field>,
    /// How unknown properties are handled.
    pub additional: AdditionalProps,
}

/// A single object field.
#[derive(Debug, Clone)]
pub struct Field {
    /// The wire property name.
    pub name: PropertyName,
    /// The field's type.
    pub ty: Ty,
    /// Whether the property is `required`.
    pub required: bool,
    /// `deprecated` → `#[deprecated]`.
    pub deprecated: bool,
    /// `readOnly` annotation (W-class, surfaced in rustdoc).
    pub read_only: bool,
    /// `writeOnly` annotation (W-class, surfaced in rustdoc).
    pub write_only: bool,
    /// The JSON Schema `default` disposition, if the field declared one. `None` when the field has
    /// no `default`.
    pub default: Option<FieldDefault>,
    /// XML representation hints (`xml.name` / `xml.attribute`) applied when the field's owning type
    /// is serialized as XML. Default (no hint) leaves the field's normal wire name and element form.
    pub xml: XmlField,
}

/// The supported XML representation hints for a struct field, lowered from the OpenAPI `xml` object.
/// Only `name` (element/attribute rename) and `attribute` (serialize as an XML attribute) are
/// honored; unsupported hints (namespace/prefix/wrapped arrays) are reported as `W006` during
/// lowering and otherwise ignored. Applied via serde `rename` at emit time — attributes use
/// quick-xml's `@name` convention.
#[derive(Debug, Clone, Default)]
pub struct XmlField {
    /// `xml.name`: the wire element (or attribute) name, overriding the property name.
    pub name: Option<String>,
    /// `xml.attribute: true`: serialize this field as an XML attribute (`@name`) rather than a child
    /// element.
    pub attribute: bool,
}

impl XmlField {
    /// The effective serde wire name for this field under XML: the `xml.name` override (or the given
    /// property wire name), prefixed with `@` when the field is an attribute. Returns `None` when no
    /// XML hint applies, so codegen keeps the plain property wire name.
    pub fn wire_override(&self, property_wire: &str) -> Option<String> {
        if self.name.is_none() && !self.attribute {
            return None;
        }
        let base = self.name.as_deref().unwrap_or(property_wire);
        Some(if self.attribute {
            format!("@{base}")
        } else {
            base.to_owned()
        })
    }
}

/// A field's JSON Schema `default` disposition. Every `default` is given exactly one of three
/// dispositions — never silently dropped:
///
/// * a representable scalar wired through serde (`applied` is `Some`), which also documents the
///   value in rustdoc;
/// * a representable scalar on a required (or nullable) field, documented in rustdoc only
///   (`applied` is `None`); or
/// * a non-representable default (object/array/null/heterogeneous or scalar-type mismatch),
///   documented in rustdoc and reported once as `W005` during lowering (`applied` is `None`).
#[derive(Debug, Clone)]
pub struct FieldDefault {
    /// The rustdoc note line describing the default (e.g. ``Default: `active`.``).
    pub doc_note: String,
    /// The scalar to wire through a generated serde default provider, when the default is
    /// representable *and* the field is a plain optional (non-required, non-nullable) scalar.
    pub applied: Option<DefaultValue>,
}

/// A representable scalar `default`, carried so codegen can render it as a correct Rust literal for
/// the field's Rust type.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    /// A boolean literal.
    Bool(bool),
    /// An integer literal (rendered unsuffixed so it infers to the field's `i32`/`i64`).
    Int(i64),
    /// A floating-point literal (rendered with a decimal point).
    Float(f64),
    /// A string literal.
    Str(String),
    /// A string-repr [`ScalarEnum`] variant, identified by its wire value; codegen renders it as
    /// the generated enum variant rather than a raw string.
    EnumVariant(String),
}

/// The `additionalProperties` policy of a [`Struct`] (matrix: Schema shape).
#[derive(Debug, Clone)]
pub enum AdditionalProps {
    /// `additionalProperties: false` → `#[serde(deny_unknown_fields)]`.
    Deny,
    /// `additionalProperties: true` / absent → unknown fields ignored.
    Allow,
    /// `additionalProperties: <schema>` → a typed overflow map.
    Typed(Box<Ty>),
}

/// A wire property name. The Rust identifier is allocated separately by `name`; keeping
/// the wire name here means the IR stays language-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyName {
    /// The exact property name as it appears on the wire.
    pub wire: String,
}

/// A homogeneous scalar enumeration generated from `enum`/`const` over a single scalar kind.
/// Heterogeneous or structured value sets are R-rejected.
#[derive(Debug, Clone)]
pub struct ScalarEnum {
    /// The shared scalar kind of every variant.
    pub repr: ScalarRepr,
    /// The variant wire values, in declared order.
    pub variants: Vec<ScalarValue>,
}

/// The scalar kind backing a [`ScalarEnum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarRepr {
    /// All-string value set.
    String,
    /// All-integer value set.
    Int,
    /// All-boolean value set.
    Bool,
}

/// A concrete scalar value (an `enum` member or `const`).
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    /// A boolean value.
    Bool(bool),
    /// An integer value.
    Int(i64),
    /// A string value.
    String(String),
}
