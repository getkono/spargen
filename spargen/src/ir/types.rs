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
            document: String::new(),
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
            // A reservation's body is unknown, so nothing proves it emits the same type as another
            // definition — including a second reservation, which may fill to a nominal type that
            // matches only itself. `false` is this function's sound answer ("not proven one type"),
            // not a guess; the same reservation on both sides already answered `true` by id above.
            (TypeKind::Reserved, _) | (_, TypeKind::Reserved) => false,
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
    /// **Where the compiler stops, a test takes over.** A dedicated variant turns a read site into
    /// a compile error only where the `match` is exhaustive; a catch-all arm (`_`, a bare binding,
    /// `Some(_)`, or a `| _` tail) absorbs it with no error. So a `match` that classifies a
    /// `TypeKind` either has no catch-all, and the compiler holds it, or names `Reserved` in an
    /// unguarded arm above its catch-all, and
    /// `every_type_kind_match_states_its_answer_for_a_reservation` in this module holds it: that
    /// test walks every `match` in the crate's sources and fails on one that reaches a catch-all
    /// with no stated answer for a reservation. A site is a `match` at least one of whose arm
    /// patterns names a `TypeKind` variant; `matches!` and `if let`, which test for one variant
    /// rather than classify, are not sites, and a catch-all in one position of a tuple pattern
    /// is not seen. The population is therefore the test's to count, not this comment's — an
    /// earlier revision published a hand count here and got its breakdown wrong.
    ///
    /// The answers fall into three kinds. Sites that run only on a checked `Api` — codegen,
    /// `name`, `surface`, `runtime_contract` — refuse with `unreachable!`, as `check_invariants`
    /// rejects a surviving reservation before any of them runs. Sites in `oas31::lower`, where a
    /// reservation really is live, answer "not proven": not the same type, not string-like, no
    /// `simple` shape, no representable default, no required-key discriminator — each routing
    /// to the refusal or warning that answer already had. And `push_ref_member`, the historical
    /// origin of the defect class, refuses an in-progress member itself (`E013`'s `allOf` unit
    /// rejection) rather than trusting each caller to have guarded it, so a new caller inherits
    /// the refusal. `intersect_non_null` likewise answers `Err(NoMeet::Unrepresentable)` for a
    /// reservation in its own first arm instead of relying on `intersect_types`' guard alone.
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
    /// Whether no `properties` entry declares this field: the object carries it only because its
    /// `required` names the key. Such a field has no metadata of its own, and its type is what the
    /// object says of a key it does not declare, so an intersection that meets a declaration of
    /// the property takes that declaration instead, and one that meets another object's
    /// `additionalProperties` value schema narrows the field by it.
    pub(crate) undeclared: bool,
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

#[cfg(test)]
mod tests {
    //! The source audit behind [`TypeKind::Reserved`]'s guarantee.
    //!
    //! A dedicated variant turns a read site into a compile error only where its `match` is
    //! exhaustive; a catch-all arm absorbs it silently. This walks every `match` in the crate's
    //! sources and holds each one that classifies a [`TypeKind`] to stating its answer for a
    //! reservation, so the audit is checked rather than counted by hand.

    use std::path::{Path, PathBuf};

    use proc_macro2::{Delimiter, TokenStream, TokenTree};
    use syn::{Arm, ExprMatch, Pat};

    /// Every `.rs` file under `dir`, recursively, in a stable order.
    fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

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
        let mut files = Vec::new();
        rust_sources(&src, &mut files);

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
}
