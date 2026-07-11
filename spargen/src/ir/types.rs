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
    /// `deprecated` → `#[deprecated]`.
    pub deprecated: bool,
    /// `readOnly` annotation (W-class, surfaced in rustdoc; PRD D2).
    pub read_only: bool,
    /// `writeOnly` annotation (W-class, surfaced in rustdoc; PRD D2).
    pub write_only: bool,
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

/// A homogeneous scalar enumeration generated from `enum`/`const` over a single scalar kind
/// (PRD D6). Heterogeneous or structured value sets are R-rejected.
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
