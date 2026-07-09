use indexmap::IndexMap;

use crate::diag::Provenance;

use super::Docs;

/// A stable, dense identifier for a type in the [`TypeGraph`]. Ordered so codegen can emit items
/// deterministically regardless of input map ordering (PRD FR3).
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

    /// The definition for `id`, if present.
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.defs.get(&id)
    }

    /// Iterate `(id, def)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.defs.iter().map(|(id, def)| (*id, def))
    }

    /// The number of type definitions.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
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
/// Invariant (PRD FR2): a typed schema is never silently degraded to `serde_json::Value`. [`Any`]
/// appears only where the spec itself is untyped (`{}` / `true` schemas) — faithful, not lossy.
///
/// [`Any`]: TypeKind::Any
#[derive(Debug, Clone)]
pub enum TypeKind {
    /// A scalar primitive.
    Primitive(Prim),
    /// An object with named fields.
    Struct(Struct),
    /// A homogeneous scalar `enum`/`const` set (PRD D6).
    Enum(ScalarEnum),
    /// A `oneOf`/`anyOf`, either discriminated or statically provably disjoint (PRD D1).
    Union(Union),
    /// A free-form map (`additionalProperties: <schema>`).
    Map(Box<Ty>),
    /// A homogeneous array (`items`).
    Array(Box<Ty>),
    /// A fixed-length heterogeneous tuple (`prefixItems`).
    Tuple(Vec<Ty>),
    /// Raw bytes (`octet-stream` / `contentEncoding: base64`).
    Bytes,
    /// An untyped value (`{}` / `true` schema). Faithful representation of an untyped spec node.
    Any,
}

/// A scalar primitive. Numeric mappings per PRD D5; `format` type mappings per §6.2 (feature-gated).
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
    /// A scalar deserialization default (`default`), applied on missing input only (PRD D8).
    pub default: Option<ScalarDefault>,
    /// `deprecated` → `#[deprecated]`.
    pub deprecated: bool,
    /// `readOnly` annotation (W-class, surfaced in rustdoc; PRD D2).
    pub read_only: bool,
    /// `writeOnly` annotation (W-class, surfaced in rustdoc; PRD D2).
    pub write_only: bool,
    /// Field documentation.
    pub docs: Docs,
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

/// A wire property name. The Rust identifier is allocated separately by `name` (PRD D9); keeping
/// the wire name here means the IR stays language-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyName {
    /// The exact property name as it appears on the wire.
    pub wire: String,
}

/// A scalar value usable as a deserialization `default` (PRD D8). Non-scalar defaults are W-class.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarDefault {
    /// A boolean default.
    Bool(bool),
    /// An integer default.
    Int(i64),
    /// A floating-point default.
    Float(f64),
    /// A string default.
    String(String),
}

/// A homogeneous scalar enumeration generated from `enum`/`const` over a single scalar kind
/// (PRD D6). Heterogeneous or structured value sets are R-rejected.
#[derive(Debug, Clone)]
pub struct ScalarEnum {
    /// The shared scalar kind of every variant.
    pub repr: ScalarRepr,
    /// The variants, in declared order.
    pub variants: Vec<EnumVariant>,
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

/// A single [`ScalarEnum`] variant.
#[derive(Debug, Clone)]
pub struct EnumVariant {
    /// The variant's wire value.
    pub value: ScalarValue,
    /// Variant documentation.
    pub docs: Docs,
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

/// A discriminated or provably-disjoint union lowered from `oneOf`/`anyOf` (PRD D1). serde
/// `untagged` is never emitted — first-match-wins can silently misparse.
#[derive(Debug, Clone)]
pub struct Union {
    /// How variants are distinguished at deserialization time.
    pub tag: UnionTag,
    /// The variant types.
    pub variants: Vec<Ty>,
}

/// How a [`Union`]'s variants are told apart.
#[derive(Debug, Clone)]
pub enum UnionTag {
    /// An explicit `discriminator`: the tag property and its value→variant mapping.
    Discriminator {
        /// The discriminator property name.
        property: String,
        /// Mapping from discriminator value to variant type.
        mapping: IndexMap<String, TypeId>,
    },
    /// No discriminator, but the variant sets are statically provably disjoint, so an
    /// order-independent deserializer can be generated (PRD D1).
    Disjoint,
}
