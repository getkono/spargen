use indexmap::IndexMap;

use crate::diag::{JsonPointer, Provenance};

use super::Docs;

/// A stable, dense identifier for a type in the [`TypeGraph`]. Ordered so codegen can emit items
/// deterministically regardless of input map ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TypeId(pub(crate) u32);

/// The graph of named/derived types the API references. Owns every [`TypeDef`]; a [`Ty`] is a
/// lightweight reference into it.
#[derive(Debug, Clone, Default)]
pub(crate) struct TypeGraph {
    defs: IndexMap<TypeId, TypeDef>,
}

impl TypeGraph {
    /// Insert a definition and return its stable id.
    pub(crate) fn insert(&mut self, def: TypeDef) -> TypeId {
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
    /// must be filled before it can be emitted.
    ///
    /// The placeholder kind is [`TypeKind::Reserved`] and **not** a legitimate kind. It used to be
    /// `TypeKind::Any`, which every `match` already handled, so a site that read a reservation got a
    /// plausible answer — "untyped value" — instead of a compile error. Four separate sites did
    /// exactly that over three review rounds, each silently: an `allOf` member became a scalar, a
    /// `$ref` applicator with siblings was discarded, an octet use retyped a shared component, and a
    /// `oneOf` variant became the union itself. A dedicated variant makes each of those a compile
    /// error wherever the `match` is exhaustive, so that part of the audit is the compiler's rather
    /// than a reviewer's — see [`TypeKind::Reserved`] for what it does not cover.
    pub(crate) fn reserve(&mut self) -> TypeId {
        self.insert(TypeDef {
            name_hint: String::new(),
            kind: TypeKind::Reserved,
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
        })
    }

    /// Replace the def at an already-present id (typically a [`reserve`](Self::reserve)d
    /// placeholder). The id's position — and therefore [`iter`](Self::iter) order — is preserved.
    pub(crate) fn fill(&mut self, id: TypeId, def: TypeDef) {
        debug_assert!(self.defs.contains_key(&id), "fill of an unreserved id");
        self.defs.insert(id, def);
    }

    /// Remove and return the most recently inserted `(id, def)` pair. Used to lift a component
    /// root — always the last def inserted while lowering its body — into its reserved id, which
    /// keeps ids dense (the freed id is immediately reused by the next insert).
    pub(crate) fn pop_last(&mut self) -> Option<(TypeId, TypeDef)> {
        self.defs.pop()
    }

    /// The id of the most recently inserted definition, if any. Paired with
    /// [`pop_last`](Self::pop_last) to retype a definition nothing else can reference yet.
    pub(crate) fn last_id(&self) -> Option<TypeId> {
        self.defs.last().map(|(id, _)| *id)
    }

    /// The definition for `id`, if present.
    pub(crate) fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.defs.get(&id)
    }

    /// A mutable borrow of the definition for `id`, for in-place post-lowering adjustments that do
    /// not change ids or insertion order (e.g. suppressing an XML field rename on a shared type).
    pub(crate) fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeDef> {
        self.defs.get_mut(&id)
    }

    /// Iterate `(id, def)` pairs in insertion order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.defs.iter().map(|(id, def)| (*id, def))
    }

    /// Whether two references emit the identical Rust type. Sound rather than complete: `true`
    /// means the generated types are one type, because every non-nominal definition is emitted as a
    /// transparent `pub type` alias of its structure. A nominal definition (a struct, string enum,
    /// union, or `Never`) is its own item and matches only itself. Both use-site modifiers must
    /// agree, so callers strip the top-level ones they absorb; nested ones are compared, because a
    /// tuple item keeps its `Box`. Independent of the `uuid`/`time` features: a feature-mapped
    /// primitive never equals `String`, even in a build where it would emit one.
    pub(crate) fn same_generated_type(&self, a: Ty, b: Ty) -> bool {
        self.same_generated_type_guarded(a, b, &mut Vec::new())
    }

    fn same_generated_type_guarded(
        &self,
        a: Ty,
        b: Ty,
        visiting: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        if a.nullable != b.nullable || a.boxed != b.boxed {
            return false;
        }
        if a.id == b.id {
            return true;
        }
        let pair = (a.id, b.id);
        if visiting.contains(&pair) {
            // A `$ref` cycle through containers: this pair is already being compared further up the
            // stack, and along the cycle both sides unfold identically.
            return true;
        }
        let (Some(a_def), Some(b_def)) = (self.get(a.id), self.get(b.id)) else {
            return false;
        };
        visiting.push(pair);
        let same = match (&a_def.kind, &b_def.kind) {
            (TypeKind::Primitive(x), TypeKind::Primitive(y)) => x == y,
            // Integer and boolean enums are `pub type X = i64` / `bool` aliases; a string enum is a
            // real `pub enum`, so it is nominal.
            (TypeKind::Enum(x), TypeKind::Enum(y)) => {
                x.repr == y.repr && x.repr != ScalarRepr::String
            }
            (TypeKind::Enum(scalar), TypeKind::Primitive(prim))
            | (TypeKind::Primitive(prim), TypeKind::Enum(scalar)) => matches!(
                (scalar.repr, prim),
                (ScalarRepr::Int, Prim::I64) | (ScalarRepr::Bool, Prim::Bool)
            ),
            (TypeKind::Array(x), TypeKind::Array(y)) => {
                self.same_generated_type_guarded(**x, **y, visiting)
            }
            (TypeKind::Tuple(xs), TypeKind::Tuple(ys)) => {
                xs.len() == ys.len()
                    && xs
                        .iter()
                        .zip(ys)
                        .all(|(x, y)| self.same_generated_type_guarded(*x, *y, visiting))
            }
            (TypeKind::Bytes, TypeKind::Bytes)
            | (TypeKind::Null, TypeKind::Null)
            | (TypeKind::Any, TypeKind::Any) => true,
            _ => false,
        };
        visiting.pop();
        same
    }
}

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
    /// Raw bytes (`octet-stream` / `contentEncoding: base64`).
    Bytes,
    /// The exact JSON `null` value (emitted as Rust's unit type, whose serde form is `null`).
    Null,
    /// An uninhabited schema used inside a container when an item intersection is empty. For
    /// example, `array<string> & array<null>` is faithfully `Vec<Never>`: only `[]` is valid.
    Never,
    /// A tagged or structurally-disjoint union (`oneOf`/`anyOf`).
    Union(Union),
    /// An untyped value (`{}` / `true` schema). Faithful representation of an untyped spec node.
    Any,
    /// A **reservation**: an id handed out by [`TypeGraph::reserve`] whose body has not been lowered
    /// yet, so nothing is known about its shape.
    ///
    /// This is not a type. It exists so that a cycle-closing `$ref` can box a reference to a
    /// component while that component is still being lowered, and it is replaced by
    /// [`TypeGraph::fill`] as soon as the body finishes. No reservation survives a successful
    /// lowering — `Api::check_invariants` proves it — so no consumer of a finished [`TypeGraph`]
    /// ever observes one.
    ///
    /// It is a variant of its own rather than a reuse of [`Self::Any`] deliberately. Reading a
    /// reservation as `Any` is a silent wrong answer, and every `match` in the crate already handled
    /// `Any`; four sites over three review rounds read one and produced plausible, wrong output with
    /// no diagnostic. Every **exhaustive** `match` on [`TypeKind`] must now state what it does with a
    /// back edge, and the compiler will not let a new one omit it.
    ///
    /// **The guarantee is narrower than it first appears, and the difference is worth stating.** A
    /// dedicated variant turns a read site into a compile error only where the `match` was already
    /// exhaustive. Seven sites are declared that way; **seventeen** others absorb this variant
    /// through a catch-all arm and got no error — **ten** in `oas31::lower`, two each in
    /// `codegen::emit` and `runtime_contract`, and one each in this module, `name` and `surface`.
    ///
    /// Counted, not estimated, and by a stated rule so the figure can be re-derived rather than
    /// re-guessed: a site is every `match` at least one of whose arm patterns names a `TypeKind`
    /// variant, and it is an absorber when one of that match's own arms is a catch-all (`_`, a bare
    /// binding, `Some(_)`, or a `| _` tail). `matches!` and `if let`, which test for one variant
    /// rather than classifying, are not sites. Twenty-four sites satisfy the first rule and seven
    /// do not satisfy the second, which is the same seven the exhaustive count above reaches
    /// independently — the two halves agree, which is what an earlier revision of this paragraph
    /// could not say: it published seventeen over a breakdown that summed to nineteen, in three
    /// places including this shipped doc comment, because it credited `oas31::lower` with twelve.
    /// The headline was the right number all along; the breakdown was not.
    ///
    /// That population includes [`TypeGraph`]'s own `push_ref_member` in `oas31::lower`,
    /// which is the historical origin of the whole defect class and is still shaped exactly the same
    /// way, and it includes `intersect_non_null`, which is the one whose behaviour the new variant
    /// actually changed: its `TypeKind::Any` arms used to absorb the placeholder and return the
    /// other operand, and a placeholder now falls to `_ => None` instead. Its callers guard it, and
    /// the guards live in the *callers*, so a new caller gets no compile error either. Converting
    /// those arms is a change across five subsystems and is filed rather than rushed; until then,
    /// the compile-time audit covers the minority of read sites.
    ///
    /// **Semver.** The breaks are a list, not a pair, and an earlier revision of this paragraph
    /// said "two" where it should have said what follows. Making the placeholder unreadable did not
    /// by itself move a snapshot, but the branch it landed on moves generated output in four ways,
    /// and nothing but this list records them together.
    ///
    /// 1. A `$ref` carrying shape siblings whose target is still being lowered, previously generated
    ///    untyped, now `E013`.
    /// 2. A `oneOf` member that is the union being lowered, previously generated and undecodable,
    ///    now `E007`. Neither shape contains an `allOf` keyword, so neither is covered by the scope
    ///    stated on the earlier footers.
    /// 3. Giving a resolved `$ref` one identity and one type: `snapshot__openapi_boilerplate_surface`
    ///    moved from 35 public types to 25 on the corpus's only multi-file case. Duplicated types
    ///    disappearing is an improvement and is still a Major break, because a consumer naming one
    ///    of them no longer compiles.
    /// 4. Reading a reservation where one may not be read. A union whose only non-null member is a
    ///    cycle-closing `$ref` — the canonical 3.1 spelling of a nullable recursive reference, since
    ///    3.1 removed `nullable: true` — cloned the placeholder's kind into a definition nothing
    ///    would ever `fill`, and seven such documents were rejected with `E011` against input that
    ///    is not malformed. They generate as `Option<Box<T>>`, which the support matrix promises and
    ///    which the direct `{$ref: T}` spelling already produced. Where the same construct cannot be
    ///    answered truthfully it is refused instead: the union that is a component's whole body and
    ///    refers only to itself (`E007`), and a sibling keyword that would have to be intersected
    ///    against an unlowered target (`E013`). The second of those *is* a break — such documents
    ///    generated before, by guessing.
    ///
    ///    The line between the two was first drawn on the `$ref`'s **spelling**, and that was
    ///    wrong: only a member written `#/components/schemas/…` against the root component map was
    ///    recognised as an alias, so the same target addressed by relative file, by a sub-file's
    ///    own sibling reference, or by a whole-file reference was refused by the `E007` above —
    ///    whose sentence the same binary disproves for the one spelling it did recognise. It is
    ///    drawn on the resolved target's identity now, so every spelling of one target gets one
    ///    answer, which is the rule `ensure_resolved` already states for the types themselves. The
    ///    remote spelling had no line at all: it reached neither guard and aborted the process on
    ///    an `assert_eq!` instead.
    ///
    /// **The invariant's own status.** `Api::check_invariants`' reservation arm is the proof that no
    /// consumer of a finished graph observes a placeholder, and it is not a user-facing spec
    /// diagnostic: it reports under `Code::InvalidInput`, whose explain text describes malformed
    /// input, which is never what a surviving reservation means. It carries that code because after
    /// the refusals above no document reaches it, so a code of its own would have no fixture that
    /// could assert it and would fail the test that every declared code is asserted by the suite
    /// owning it. That argument holds only while the arm stays unreachable — an earlier revision of
    /// this record asserted the arm was unreachable and it was reachable seven ways, so the claim is
    /// stated here as a condition rather than as a fact, and the seven documents are fixtures in
    /// `spargen/tests/frontend.rs` precisely so that it cannot quietly stop being true again.
    Reserved,
}

/// A `oneOf`/`anyOf` union lowered to a Rust enum. Never `serde(untagged)` and never degraded to
/// `serde_json::Value`: it is emitted with content-inspecting custom `Deserialize`/`Serialize`.
/// Dispatch uses discriminator tags / unique JSON categories, statically disjoint features, or
/// typed trial matching with the source applicator's exact semantics.
#[derive(Debug, Clone)]
pub(crate) struct Union {
    /// The variants, in spec (source) order.
    pub(crate) variants: Vec<UnionVariant>,
    /// How the union is (de)serialized.
    pub(crate) strategy: UnionStrategy,
}

/// One variant of a [`Union`]: a name hint (allocated to a Rust variant identifier by `name`) and
/// the variant's payload type.
#[derive(Debug, Clone)]
pub(crate) struct UnionVariant {
    /// The preferred variant name; the Rust identifier is allocated by `name` (keyed by this hint).
    pub(crate) name_hint: String,
    /// The variant's payload type.
    pub(crate) ty: Ty,
}

/// The (de)serialization strategy of a [`Union`].
#[derive(Debug, Clone)]
pub(crate) enum UnionStrategy {
    /// A `discriminator` → a custom `Deserialize`/`Serialize` that reads/writes the tag field on a
    /// buffered `serde_json::Value` (NOT serde's `#[serde(tag = ...)]`, which would consume the tag
    /// out of the buffer and break variants that declare the discriminator as a required property).
    /// Object variants carry the tag value that selects them. A non-object variant may coexist
    /// when its JSON category is unique (for example an array beside tagged objects).
    Discriminated {
        /// The discriminator `propertyName` — the tag field read from / written into the object.
        tag_field: String,
        /// The object tag value per variant, parallel to [`Union::variants`].
        tags: Vec<Option<String>>,
        /// The JSON category per non-object variant, parallel to [`Union::variants`].
        categories: Vec<Option<JsonCategory>>,
        /// OpenAPI 3.2 `defaultMapping`: the variant used when the tag is absent or unrecognized.
        /// Without one, either case is a deserialization error.
        default_variant: Option<usize>,
    },
    /// No discriminator, but the variants were proven statically disjoint → a custom
    /// content-inspecting `Deserialize`/`Serialize`. Each variant carries the feature that
    /// unambiguously selects it.
    Disjoint {
        /// The discriminating feature per variant, parallel to [`Union::variants`].
        features: Vec<DisjointFeature>,
    },
    /// Variants overlap structurally, so generated serde implementations try each typed payload
    /// against one buffered JSON value. `oneOf` requires exactly one match; `anyOf` selects the
    /// highest-specificity match, with source order breaking ties.
    Trial {
        /// Whether the source applicator was `oneOf` or `anyOf`.
        mode: UnionMode,
        /// Static specificity per variant, parallel to [`Union::variants`].
        priorities: Vec<u32>,
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

/// The statically-proven feature that selects a [`UnionStrategy::Disjoint`] variant when inspecting
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
    /// `number` → `f64`.
    F64,
    /// `format: uuid` → `uuid::Uuid` (feature `uuid`, else `String`).
    Uuid,
    /// `format: date-time` → the embedded RFC 3339 `DateTime` newtype over `time::OffsetDateTime`
    /// (feature `time`, else `String`).
    DateTime,
    /// `format: date` → the embedded RFC 3339 `Date` newtype over `time::Date` (feature `time`,
    /// else `String`).
    Date,
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
    /// `deprecated` → `#[deprecated]`.
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
}

/// The supported XML representation hints for a struct field, lowered from the OpenAPI `xml` object.
/// Only `name` (element/attribute rename) and `attribute` (serialize as an XML attribute) are
/// honored; unsupported hints (namespace/prefix/wrapped arrays) are reported as `W006` during
/// lowering and otherwise ignored. Applied via serde `rename` at emit time — attributes use
/// quick-xml's `@name` convention.
#[derive(Debug, Clone, Default)]
pub(crate) struct XmlField {
    /// `xml.name`: the wire element (or attribute) name, overriding the property name.
    pub(crate) name: Option<String>,
    /// `xml.attribute: true`: serialize this field as an XML attribute (`@name`) rather than a child
    /// element.
    pub(crate) attribute: bool,
    /// XML hints that change the wire but have no faithful mapping — `namespace`, `prefix`,
    /// `wrapped`, and the 3.2 node types other than `element`/`attribute`. Their disposition
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
