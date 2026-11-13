//! The type definitions the [`TypeGraph`](super::TypeGraph) owns: what a [`Ty`] references, each
//! [`TypeKind`], and the union, enum and object shapes those kinds carry.

use crate::diag::Provenance;

use super::Docs;

/// A stable, dense identifier for a type in the [`TypeGraph`](super::TypeGraph). Ordered so codegen can emit items
/// deterministically regardless of input map ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TypeId(pub(crate) u32);

/// A named or structurally-derived type definition.
#[derive(Debug, Clone)]
pub(crate) struct TypeDef {
    /// The preferred wire/spec name; the Rust identifier is allocated later by `name`.
    pub(crate) name_hint: String,
    /// The type's structure.
    pub(crate) kind: TypeKind,
    /// Documentation lowered to rustdoc.
    pub(crate) docs: Docs,
    /// Where the type came from.
    pub(crate) provenance: Provenance,
    /// The document `provenance.pointer` points into, spelled independently of load order: empty
    /// for the root document, otherwise the local path relative to the root document's directory,
    /// or the retrieval URL of a vendored remote document.
    ///
    /// A pointer alone does not identify a definition — two files can each declare
    /// `/components/schemas/Shape` — and the span's `FileId` numbers files in discovery order, so
    /// reordering the document renumbers them. This spelling plus the pointer is the definition's
    /// own identity, which is what naming ranks a contested type name by.
    pub(crate) document: String,
}

/// A reference to a type, plus the two shape modifiers that ride on a use site rather than the
/// definition: nullability (from `"null"` in a type array) and boxing (to break `$ref` cycles →
/// `Box`, matrix: Schema shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ty {
    /// The referenced definition.
    pub(crate) id: TypeId,
    /// Whether `null` is an accepted value (`Option<T>`).
    pub(crate) nullable: bool,
    /// Whether the reference must be boxed to break a type cycle.
    pub(crate) boxed: bool,
}

/// The structure of a [`TypeDef`].
///
/// Invariant: a typed schema is never silently degraded to `serde_json::Value`. [`Any`]
/// appears only where the spec itself is untyped (`{}` / `true` schemas) — faithful, not lossy.
///
/// [`Any`]: TypeKind::Any
#[derive(Debug, Clone)]
pub(crate) enum TypeKind {
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
    /// Binary content, held as `bytes::Bytes`, and how a serde-encoded body spells it.
    Bytes(BytesEncoding),
    /// The exact JSON `null` value (emitted as Rust's unit type, whose serde form is `null`).
    Null,
    /// An uninhabited schema used inside a container when an item intersection is empty. For
    /// example, `array<string> & array<null>` is faithfully `Vec<Never>`: only `[]` is valid.
    Never,
    /// A tagged or structurally-disjoint union (`oneOf`/`anyOf`).
    Union(Union),
    /// An untyped value (`{}` / `true` schema). Faithful representation of an untyped spec node.
    Any,
    /// A **reservation**: an id handed out by [`TypeGraph::reserve`](super::TypeGraph::reserve) whose body has not been lowered
    /// yet, so nothing is known about its shape. It lets a cycle-closing `$ref` box a reference to
    /// a component still being lowered, and [`TypeGraph::fill`](super::TypeGraph::fill) replaces it as soon as the body
    /// finishes; [`check_invariants`](super::check_invariants) reports one that survives a
    /// lowering as `E011`, a frontend bug.
    ///
    /// It is a variant of its own rather than a reuse of [`Self::Any`], because reading a
    /// reservation as `Any` is a plausible, wrong answer with no diagnostic. Sites that run only on
    /// a checked `Api` (codegen, `name`, `surface`, the runtime contract) refuse it with
    /// `unreachable!`; sites in `oas31::lower`, where it is live, answer "not proven". An
    /// exhaustive `match` is held to stating an answer by the compiler, and one with a catch-all
    /// arm names `Reserved` above it, which
    /// `every_type_kind_match_states_its_answer_for_a_reservation` in this module holds.
    Reserved,
}

/// How a [`TypeKind::Bytes`] value is written where a serde codec (JSON, XML, a form body, a
/// sequential stream's items) carries it. A whole raw body is the octets themselves under either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BytesEncoding {
    /// `format: binary`, an untyped octet-stream body, or no schema: `bytes::Bytes` itself, which
    /// a serde codec writes as a sequence of numbers.
    Raw,
    /// `contentEncoding: base64`: a string of RFC 4648 base64 text, emitted as a newtype over
    /// `bytes::Bytes` that the embedded `base64` runtime module (de)serializes. A whole non-streaming
    /// body never carries it: lowering takes such a body raw, and `check_invariants` holds that.
    Base64,
}

/// A `oneOf`/`anyOf` union lowered to a Rust enum. Never `serde(untagged)` and never degraded to
/// `serde_json::Value`: it is emitted with content-inspecting custom `Deserialize`/`Serialize`.
/// The one opt-in exception is an open union ([`UnionStrategy::Discriminated`]'s `open`), whose
/// extra explicit `Unknown { tag, value }` variant holds a payload no listed variant claims as a
/// `serde_json::Value`; every listed variant stays typed.
/// Dispatch uses discriminator tags / unique JSON categories, statically disjoint features, or
/// typed trial matching with the source applicator's exact semantics.
#[derive(Debug, Clone)]
pub(crate) struct Union {
    /// The variants, in spec (source) order.
    pub(crate) variants: Vec<UnionVariant>,
    /// How the union is (de)serialized.
    pub(crate) strategy: UnionStrategy,
}

impl Union {
    /// Whether the union carries an `Unknown` variant beyond [`Self::variants`], for a tag value
    /// it does not list ([`UnionStrategy::Discriminated`]'s `open`).
    pub(crate) fn is_open(&self) -> bool {
        matches!(
            self.strategy,
            UnionStrategy::Discriminated { open: true, .. }
        )
    }
}

/// One variant of a [`Union`]: a name hint (allocated to a Rust variant identifier by `name`), the
/// variant's payload type, and what selects it under the union's [`UnionStrategy`].
#[derive(Debug, Clone)]
pub(crate) struct UnionVariant {
    /// The preferred variant name; the Rust identifier is allocated by `name` (keyed by this hint).
    pub(crate) name_hint: String,
    /// The variant's payload type.
    pub(crate) ty: Ty,
    /// What selects this variant, of the same arm as the union's [`UnionStrategy`]: a
    /// [`VariantDispatch::Discriminated`] variant belongs to a [`UnionStrategy::Discriminated`]
    /// union, and so on. `check_invariants` holds every emitted union to that agreement.
    pub(crate) dispatch: VariantDispatch,
}

/// The (de)serialization strategy of a [`Union`]: its union-level settings. What selects each
/// variant is that variant's own [`UnionVariant::dispatch`].
#[derive(Debug, Clone)]
pub(crate) enum UnionStrategy {
    /// A `discriminator` → a custom `Deserialize`/`Serialize` that reads/writes the tag field on a
    /// buffered `serde_json::Value` (NOT serde's `#[serde(tag = ...)]`, which would consume the tag
    /// out of the buffer and break variants that declare the discriminator as a required property).
    /// Object variants carry the tag values that select them. A non-object variant may coexist
    /// when its JSON category is unique (for example an array beside tagged objects).
    Discriminated {
        /// The discriminator `propertyName` — the tag field read from / written into the object.
        tag_field: String,
        /// OpenAPI 3.2 `defaultMapping`: the position in [`Union::variants`] of the variant used
        /// when the tag is absent or unrecognized and no `untagged` variant matches. Without one,
        /// either case is a deserialization error.
        default_variant: Option<usize>,
        /// How the `untagged` variants are matched: the source applicator's semantics.
        mode: UnionMode,
        /// Whether the union is open (`Spec::open_unions`): it carries one more variant, beyond
        /// [`Union::variants`], holding a payload whose tag is a string no variant lists and no
        /// `untagged` variant accepts, which a closed union refuses. Never set beside a
        /// `default_variant`, which already takes such a payload; `check_invariants` holds that.
        open: bool,
    },
    /// No discriminator, but the variants were proven statically disjoint → a custom
    /// content-inspecting `Deserialize`/`Serialize`. Each variant carries the feature that
    /// unambiguously selects it ([`VariantDispatch::Disjoint`]).
    Disjoint,
    /// Variants overlap structurally, so generated serde implementations try each typed payload
    /// against one buffered JSON value. `oneOf` requires exactly one match; `anyOf` selects the
    /// highest-specificity match ([`VariantDispatch::Trial`]), with source order breaking ties.
    Trial {
        /// Whether the source applicator was `oneOf` or `anyOf`.
        mode: UnionMode,
    },
}

impl UnionStrategy {
    /// Whether `dispatch` is of this strategy's arm, as every variant of a union using it must be.
    pub(crate) fn admits(&self, dispatch: &VariantDispatch) -> bool {
        matches!(
            (self, dispatch),
            (
                UnionStrategy::Discriminated { .. },
                VariantDispatch::Discriminated { .. }
            ) | (UnionStrategy::Disjoint, VariantDispatch::Disjoint(_))
                | (UnionStrategy::Trial { .. }, VariantDispatch::Trial { .. })
        )
    }
}

/// What selects one [`UnionVariant`], one arm per [`UnionStrategy`] arm.
#[derive(Debug, Clone)]
pub(crate) enum VariantDispatch {
    /// A variant of a [`UnionStrategy::Discriminated`] union.
    Discriminated {
        /// Every tag value that selects the variant, in dispatch order: each `mapping` key naming
        /// the variant's member in document order, then its implicit component name unless a
        /// `mapping` key claims that value, or, for a member with neither, the one string its
        /// required tag property pins, unless something else claims it. Deserialization accepts
        /// every entry; serialization writes the first. Empty for a variant routed by `category`,
        /// for an object variant reached only as the `default_variant` fallback, and for an
        /// `untagged` variant.
        tags: Vec<String>,
        /// The JSON category of a non-object variant, which selects it; `None` for an object
        /// variant.
        category: Option<JsonCategory>,
        /// The trial priority of an object variant that no tag selects and that is not the
        /// `default_variant`; `None` for every other variant. Such a variant (inline, or a pointer
        /// into another schema, that no `mapping` entry names) has no discriminator value, so it
        /// is tried against the buffered value, with the strategy's `mode` semantics, only when
        /// the tag is absent or selects no tagged variant; a tag that does select one dispatches
        /// to it alone. Serialization writes no tag for it.
        untagged: Option<u32>,
    },
    /// A variant of a [`UnionStrategy::Disjoint`] union: the feature that selects it.
    Disjoint(DisjointFeature),
    /// A variant of a [`UnionStrategy::Trial`] union.
    Trial {
        /// The variant's static specificity: of two matches, the higher is selected.
        priority: u32,
    },
}

/// Runtime matching semantics for an overlapping typed union.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnionMode {
    /// Exactly one variant must deserialize successfully.
    OneOf,
    /// One or more variants may match; the most specific typed match is selected.
    AnyOf,
}

/// The statically-proven feature that selects a [`VariantDispatch::Disjoint`] variant when inspecting
/// a buffered `serde_json::Value`.
#[derive(Debug, Clone)]
pub(crate) enum DisjointFeature {
    /// The variant occupies a distinct JSON primitive category (dispatch on `Value::is_*`).
    JsonType(JsonCategory),
    /// The variant is an object carrying a required property whose name appears in no other variant
    /// (dispatch on `Value::get(key).is_some()`).
    RequiredKey(String),
}

/// A JSON primitive category for disjointness by JSON type. `number` and `integer` share
/// [`Number`](JsonCategory::Number) — they overlap on the wire, so they are never disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonCategory {
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
pub(crate) enum Prim {
    /// `boolean`.
    Bool,
    /// `string`.
    String,
    /// `format: int32` → `i32`.
    I32,
    /// `format: int64` / unformatted `integer` → `i64`.
    I64,
    /// `format: uint8` → `u8`.
    U8,
    /// `format: uint16` → `u16`.
    U16,
    /// `format: uint32` → `u32`.
    U32,
    /// `format: uint64` → `u64`.
    U64,
    /// `number` → `f64`.
    F64,
    /// `format: uuid` → `uuid::Uuid` (feature `uuid`, else `String`).
    Uuid,
    /// `format: date-time` → the embedded RFC 3339 `DateTime` newtype over the selected
    /// [`DateTimeBackend`]'s instant type, or `String` under [`DateTimeBackend::String`].
    DateTime,
    /// `format: date` → the embedded RFC 3339 `Date` newtype over the selected
    /// [`DateTimeBackend`]'s calendar-date type, or `String` under [`DateTimeBackend::String`].
    Date,
}

/// The crate `format: date-time` and `format: date` map to, chosen with
/// [`Spec::datetime`](crate::Spec::datetime).
///
/// Every typed backend emits an embedded `DateTime` newtype for `date-time` and a `Date` newtype
/// for `date`, each with spargen's own RFC 3339 `Serialize`/`Deserialize`, `Display` and `FromStr`.
/// `Date`, and the `Time` and `Chrono` backends' `DateTime`, also `Deref` to the wrapped value and
/// convert with `From` in both directions; the `Jiff` backend's `DateTime` has the surface the last
/// paragraph states. The wrapped type, and so the crate the dependency audit (`E023`) requires,
/// differs by backend. Every backend reads the same strings (RFC 3339 §5.6's `date-time`, every year from
/// `0000` to `9999`) and keeps the offset a value was written with.
///
/// | backend | `DateTime` wraps | `Date` wraps | required crate |
/// | --- | --- | --- | --- |
/// | [`Time`](Self::Time) (default) | `time::OffsetDateTime` | `time::Date` | `time` |
/// | [`Jiff`](Self::Jiff) | `jiff::civil::DateTime` and `jiff::tz::Offset` | `jiff::civil::Date` | `jiff` |
/// | [`Chrono`](Self::Chrono) | `chrono::DateTime<chrono::FixedOffset>` | `chrono::NaiveDate` | `chrono` |
/// | [`String`](Self::String) | — (`String`) | — (`String`) | none |
///
/// The `Jiff` backend's `DateTime` holds two values, because `jiff::Timestamp` ends before year
/// 9999 does: it reads them through `datetime()` and `offset()` rather than `Deref`, converts from a
/// `jiff::Timestamp`, and back to one through the fallible `to_timestamp()`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DateTimeBackend {
    /// The `time` crate: `time::OffsetDateTime` and `time::Date`.
    #[default]
    Time,
    /// The `jiff` crate: a `jiff::civil::DateTime` at a `jiff::tz::Offset`, and `jiff::civil::Date`.
    Jiff,
    /// The `chrono` crate: `chrono::DateTime<chrono::FixedOffset>` and `chrono::NaiveDate`.
    Chrono,
    /// No date-time crate: both formats stay `String`, a deliberate, documented loss of typing.
    String,
}

impl DateTimeBackend {
    /// Every backend, in declaration order.
    pub const ALL: &'static [Self] = &[Self::Time, Self::Jiff, Self::Chrono, Self::String];

    /// The spelling `spargen.toml`'s `datetime` key, the CLI's `--datetime` flag and
    /// `generate_api!`'s `datetime` argument take: `time`, `jiff`, `chrono` or `string`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Jiff => "jiff",
            Self::Chrono => "chrono",
            Self::String => "string",
        }
    }

    /// Whether the date formats map to the embedded `DateTime`/`Date` newtypes rather than
    /// `String`.
    pub(crate) fn is_typed(self) -> bool {
        self != Self::String
    }
}

impl std::fmt::Display for DateTimeBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for DateTimeBackend {
    type Err = String;

    /// Reads the spelling [`as_str`](Self::as_str) writes; anything else is an error naming the
    /// four it accepts.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|backend| backend.as_str() == text)
            .ok_or_else(|| {
                format!("unknown date-time backend `{text}`; expected time, jiff, chrono or string")
            })
    }
}

/// An object type: named fields plus an `additionalProperties` policy.
#[derive(Debug, Clone)]
pub(crate) struct Struct {
    /// The declared properties, in deterministic order.
    pub(crate) fields: Vec<Field>,
    /// How unknown properties are handled.
    pub(crate) additional: AdditionalProps,
}

/// A single object field.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// The wire property name.
    pub(crate) name: PropertyName,
    /// The field's type.
    pub(crate) ty: Ty,
    /// Whether the property is `required`.
    pub(crate) required: bool,
    /// `deprecated: true`, emitted as a "Deprecated per the spec." rustdoc note on the field.
    pub(crate) deprecated: bool,
    /// `readOnly` annotation (W-class, surfaced in rustdoc).
    pub(crate) read_only: bool,
    /// `writeOnly` annotation (W-class, surfaced in rustdoc).
    pub(crate) write_only: bool,
    /// The JSON Schema `default` disposition, if the field declared one. `None` when the field has
    /// no `default`.
    pub(crate) default: Option<FieldDefault>,
    /// XML representation hints (`xml.name` / `xml.attribute`) applied when the field's owning type
    /// is serialized as XML. Default (no hint) leaves the field's normal wire name and element form.
    pub(crate) xml: XmlField,
    /// Whether no `properties` entry declares this field: the object carries it only because its
    /// `required` names the key. Such a field has no metadata of its own, and its type is what the
    /// object says of a key it does not declare, so an intersection that meets a declaration of
    /// the property takes that declaration instead, and one that meets another object's
    /// `additionalProperties` value schema narrows the field by it.
    pub(crate) undeclared: bool,
}

/// The XML representation hints for a struct field, lowered from the property's OpenAPI `xml`
/// object and, for an array property, its `items`' one. Only `name` (element/attribute rename) and
/// `attribute` (serialize as an XML attribute) are honored, applied via serde `rename` at emit
/// time — attributes use quick-xml's `@name` convention. Every other hint is kept in `unsupported`
/// and reported after lowering, as `E009` on a type serialized as XML and `W006` elsewhere.
#[derive(Debug, Clone, Default)]
pub(crate) struct XmlField {
    /// The wire element (or attribute) name, overriding the property name: the property's
    /// `xml.name`, or for an array property, whose items are emitted directly under the parent,
    /// its `items`' `xml.name`, or else the XML root name of the struct its items resolve to; under
    /// OpenAPI 3.2, also the XML root name of the struct a [`Self::by_reference`] field resolves to.
    pub(crate) name: Option<String>,
    /// Whether the property's schema — or, for an array property, its `items` — is a `$ref` whose
    /// node type is `none`, as OpenAPI 3.2 defaults it: the node names no element, and the
    /// referenced schema's own element stands in its place. An explicit `xml.nodeType` other than
    /// `none` on that node leaves this `false`.
    pub(crate) by_reference: bool,
    /// Whether that same node is a `$ref` with an explicit `xml.nodeType: element`: under OpenAPI
    /// 3.2 the reference is then an element of its own, named after the property, so the field
    /// takes no name from the struct it resolves to, not even an array item's root `xml.name`.
    pub(crate) element_reference: bool,
    /// `xml.attribute: true`: serialize this field as an XML attribute (`@name`) rather than a child
    /// element.
    pub(crate) attribute: bool,
    /// XML hints that change the wire but have no faithful mapping — `namespace`, `prefix`,
    /// `wrapped`, and the 3.2 node types other than `element`/`attribute` — each spelled as the
    /// key it was written under, with an `items.` prefix for one on the items. Their disposition
    /// depends on whether the owning type is ever serialized as XML, which is only known once the
    /// whole type graph exists, so it is decided after lowering.
    pub(crate) unsupported: Vec<String>,
}

impl XmlField {
    /// The effective serde wire name for this field under XML: the `xml.name` override (or the given
    /// property wire name), prefixed with `@` when the field is an attribute. Returns `None` when no
    /// XML hint applies, so codegen keeps the plain property wire name.
    pub(crate) fn wire_override(&self, property_wire: &str) -> Option<String> {
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
pub(crate) struct FieldDefault {
    /// The rustdoc note line describing the default (e.g. ``Default: `active`.``).
    pub(crate) doc_note: String,
    /// The scalar to wire through a generated serde default provider, when the default is
    /// representable *and* the field is a plain optional (non-required, non-nullable) scalar.
    pub(crate) applied: Option<DefaultValue>,
    /// The `default` keyword that declared the value, so a disposition decided after the field's
    /// own lowering — an intersection that narrows the field's type to one the value is not a
    /// member of — is reported where the value was written.
    pub(crate) provenance: crate::diag::Provenance,
    /// The other `default` keywords an intersection merged into this one because they wrote the
    /// same value (#432), ordered by location. They are one value for choosing what the merged
    /// field keeps, but each is still a `default` someone wrote: when the merge or a later
    /// re-typing drops this default, `W005` is reported at every one of them as well as at
    /// [`Self::provenance`] (#543), so the report does not depend on the members' order.
    pub(crate) also_written: Vec<crate::diag::Provenance>,
}

/// A representable scalar `default`, carried so codegen can render it as a correct Rust literal for
/// the field's Rust type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DefaultValue {
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
pub(crate) enum AdditionalProps {
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
pub(crate) struct PropertyName {
    /// The exact property name as it appears on the wire.
    pub(crate) wire: String,
}

/// A homogeneous scalar enumeration generated from `enum`/`const` over a single scalar kind.
/// Heterogeneous or structured value sets are R-rejected.
#[derive(Debug, Clone)]
pub(crate) struct ScalarEnum {
    /// The shared scalar kind of every variant.
    pub(crate) repr: ScalarRepr,
    /// The variant wire values, in declared order.
    pub(crate) variants: Vec<ScalarValue>,
    /// Whether the set is open, closed, or closed for good. Only an [`Openness::Open`] set emits
    /// a variant beyond the listed values. Always [`Openness::Closed`] for an integer or boolean
    /// set.
    pub(crate) openness: Openness,
}

impl ScalarEnum {
    /// Whether the set is [`Openness::Open`]: it emits a variant holding any unlisted string.
    pub(crate) fn is_open(&self) -> bool {
        self.openness == Openness::Open
    }
}

/// How a [`ScalarEnum`]'s value set relates to the strings beyond it. Only `open_narrowing`
/// produces anything but [`Openness::Closed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Openness {
    /// The listed values and nothing else. A plain `string` it narrows inside a response body's
    /// own schema opens it under `open_narrowing`.
    Closed,
    /// A string enum whose values name the members the description lists, beside one more
    /// variant that holds any other string: a string `enum`/`const` that narrows a property
    /// another `allOf` member (or the `$ref` it sits beside) declares as a plain `string`, inside
    /// a response body's own schema. The open set's domain is that wider declaration's, so it is
    /// still exactly what the description admits there, minus the narrowing.
    Open,
    /// The listed values and nothing else, narrowed against a `uuid`, `date`, or `date-time`
    /// string (in its own schema, or in a schema it met). That format admits no arbitrary string,
    /// so no plain `string` it later meets opens it, and an open set meeting it closes: the set
    /// stays closed whichever order an `allOf` lists its members in.
    Locked,
}

/// The scalar kind backing a [`ScalarEnum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarRepr {
    /// All-string value set.
    String,
    /// All-integer value set.
    Int,
    /// All-boolean value set.
    Bool,
}

/// A concrete scalar value (an `enum` member or `const`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScalarValue {
    /// A boolean value.
    Bool(bool),
    /// An integer value.
    Int(i64),
    /// A string value.
    String(String),
}

impl ScalarValue {
    /// The value as its JSON text spells it, a string without its quotes: the key `name`
    /// allocates an enum variant under and the label the API surface lists it by.
    pub(crate) fn wire(&self) -> String {
        match self {
            ScalarValue::Bool(value) => value.to_string(),
            ScalarValue::Int(value) => value.to_string(),
            ScalarValue::String(value) => value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The source audit behind [`TypeKind::Reserved`]'s guarantee.
    //!
    //! A dedicated variant turns a read site into a compile error only where its `match` is
    //! exhaustive; a catch-all arm absorbs it silently. This walks every `match` in the crate's
    //! sources and holds each one that classifies a [`TypeKind`] to stating its answer for a
    //! reservation, so the audit is checked rather than counted by hand.

    use std::path::Path;

    use proc_macro2::{Delimiter, TokenStream, TokenTree};
    use syn::{Arm, ExprMatch, Pat};

    use crate::test_support::rust_sources;

    /// Every `match` expression in `tokens`, at any nesting depth, re-parsed on its own.
    ///
    /// A `match` scrutinee cannot contain a struct literal, so its arms are the first top-level
    /// brace group after the keyword; a brace nested in the scrutinee sits inside a paren group.
    /// The body of a `quote!`-family invocation is emitted code, not this crate's, and is skipped.
    fn collect_matches(tokens: TokenStream, out: &mut Vec<ExprMatch>) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for (at, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(group) => {
                    const EMITTING: [&str; 3] = ["quote", "quote_spanned", "parse_quote"];
                    let emitting = |tree: &TokenTree| match tree {
                        TokenTree::Ident(name) => EMITTING.iter().any(|emitter| name == emitter),
                        _ => false,
                    };
                    let quoted = at >= 2
                        && matches!(&trees[at - 1], TokenTree::Punct(p) if p.as_char() == '!')
                        && emitting(&trees[at - 2]);
                    if !quoted {
                        collect_matches(group.stream(), out);
                    }
                }
                TokenTree::Ident(ident) if ident == "match" => {
                    let arms = trees[at + 1..].iter().position(
                        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
                    );
                    let Some(arms) = arms else { continue };
                    let expr: TokenStream = trees[at..=at + 1 + arms].iter().cloned().collect();
                    let parsed = syn::parse2::<ExprMatch>(expr.clone())
                        .unwrap_or_else(|e| panic!("`{expr}` does not parse as a match: {e}"));
                    out.push(parsed);
                }
                TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }

    /// Whether `path` is `TypeKind::<variant>` (any variant when `variant` is `None`).
    fn is_kind_path(path: &syn::Path, variant: Option<&str>) -> bool {
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() >= 2
            && segments[segments.len() - 2].ident == "TypeKind"
            && variant.is_none_or(|v| segments[segments.len() - 1].ident == v)
    }

    /// Whether `pat` names a `TypeKind` variant (a specific one when `variant` is `Some`).
    fn names_kind(pat: &Pat, variant: Option<&str>) -> bool {
        match pat {
            Pat::Ident(p) => p
                .subpat
                .as_ref()
                .is_some_and(|(_, sub)| names_kind(sub, variant)),
            Pat::Or(p) => p.cases.iter().any(|c| names_kind(c, variant)),
            Pat::Paren(p) => names_kind(&p.pat, variant),
            Pat::Reference(p) => names_kind(&p.pat, variant),
            Pat::Slice(p) => p.elems.iter().any(|e| names_kind(e, variant)),
            Pat::Tuple(p) => p.elems.iter().any(|e| names_kind(e, variant)),
            Pat::Type(p) => names_kind(&p.pat, variant),
            Pat::Path(p) => is_kind_path(&p.path, variant),
            Pat::Struct(p) => {
                is_kind_path(&p.path, variant)
                    || p.fields.iter().any(|f| names_kind(&f.pat, variant))
            }
            Pat::TupleStruct(p) => {
                is_kind_path(&p.path, variant) || p.elems.iter().any(|e| names_kind(e, variant))
            }
            _ => false,
        }
    }

    /// Whether `pat` is a catch-all: `_`, a bare binding, `..`, `Some(_)`, a tuple of those, or an
    /// or-pattern with such a case. It is what absorbs a variant the arms above it never named.
    fn is_catch_all(pat: &Pat) -> bool {
        match pat {
            Pat::Wild(_) | Pat::Rest(_) => true,
            Pat::Ident(p) => match &p.subpat {
                Some((_, sub)) => is_catch_all(sub),
                None => p
                    .ident
                    .to_string()
                    .starts_with(|c: char| c.is_lowercase() || c == '_'),
            },
            Pat::Or(p) => p.cases.iter().any(is_catch_all),
            Pat::Paren(p) => is_catch_all(&p.pat),
            Pat::Reference(p) => is_catch_all(&p.pat),
            Pat::Tuple(p) => p.elems.iter().all(is_catch_all),
            Pat::TupleStruct(p) => p.path.is_ident("Some") && p.elems.iter().all(is_catch_all),
            _ => false,
        }
    }

    /// Whether `arm` answers for a reservation unconditionally.
    fn answers_reserved(arm: &Arm) -> bool {
        arm.guard.is_none() && names_kind(&arm.pat, Some("Reserved"))
    }

    /// Every `match` that classifies a `TypeKind` and carries a catch-all arm names
    /// `TypeKind::Reserved` in an unguarded arm above that catch-all, so no site reads a
    /// reservation through a wildcard it never decided on.
    ///
    /// A site is a `match` at least one of whose arm patterns names a `TypeKind` variant;
    /// `matches!` and `if let`, which test for one variant rather than classify, are not sites. A
    /// catch-all in one position of a tuple pattern (`(TypeKind::Array(a), _)`) is not seen by
    /// this rule; only a whole arm that matches anything is.
    #[test]
    fn every_type_kind_match_states_its_answer_for_a_reservation() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = rust_sources(&src);

        let mut sites = 0usize;
        let mut absorbers = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
            let tokens: TokenStream = text
                .parse()
                .unwrap_or_else(|e| panic!("tokenizing {}: {e}", file.display()));
            let mut found = Vec::new();
            collect_matches(tokens, &mut found);
            for site in found {
                if !site.arms.iter().any(|arm| names_kind(&arm.pat, None)) {
                    continue;
                }
                sites += 1;
                let Some(catch_all) = site.arms.iter().position(|arm| is_catch_all(&arm.pat))
                else {
                    continue;
                };
                if !site.arms[..catch_all].iter().any(answers_reserved) {
                    let expr = &site.expr;
                    absorbers.push(format!(
                        "{}: match {}",
                        file.strip_prefix(&src).unwrap_or(file).display(),
                        quote::quote!(#expr)
                    ));
                }
            }
        }

        // A scanner that silently found nothing would pass vacuously; the crate has dozens.
        assert!(
            sites >= 20,
            "found only {sites} `TypeKind` match sites; the scanner is broken"
        );
        assert!(
            absorbers.is_empty(),
            "these `match`es classify a `TypeKind` and reach a catch-all arm without an \
             unguarded `TypeKind::Reserved` arm above it, so a reservation falls through \
             undecided:\n{}",
            absorbers.join("\n")
        );
    }

    /// Each backend's spelling reads back through `FromStr` and through the config file's serde
    /// form, the default is `Time`, only `String` is untyped, and an unknown spelling names the
    /// four accepted ones.
    #[test]
    fn datetime_backend_spellings_round_trip() {
        use super::DateTimeBackend;

        #[derive(serde::Deserialize)]
        struct File {
            datetime: DateTimeBackend,
        }

        assert_eq!(DateTimeBackend::default(), DateTimeBackend::Time);
        let spellings: Vec<&str> = DateTimeBackend::ALL.iter().map(|b| b.as_str()).collect();
        assert_eq!(spellings, ["time", "jiff", "chrono", "string"]);
        for &backend in DateTimeBackend::ALL {
            assert_eq!(backend.as_str().parse::<DateTimeBackend>(), Ok(backend));
            assert_eq!(backend.to_string(), backend.as_str());
            let file: File = toml::from_str(&format!("datetime = \"{backend}\"")).unwrap();
            assert_eq!(file.datetime, backend);
            assert_eq!(backend.is_typed(), backend != DateTimeBackend::String);
        }
        let error = "Jiff".parse::<DateTimeBackend>().unwrap_err();
        assert_eq!(
            error,
            "unknown date-time backend `Jiff`; expected time, jiff, chrono or string"
        );
    }
}
