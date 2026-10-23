use std::collections::{BTreeMap, BTreeSet};

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
    /// The definitions [`elide`](Self::elide) withheld from the output.
    elided: BTreeSet<TypeId>,
    /// The XML element name a struct serializes as at the root of an XML body, from the root
    /// `xml.name` of the schema it was lowered from or, for a component, its component name (see
    /// [`set_xml_root_name`](Self::set_xml_root_name)).
    xml_root_names: BTreeMap<TypeId, String>,
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
    /// The placeholder kind is [`TypeKind::Reserved`], a variant of its own rather than a
    /// legitimate kind, so an exhaustive `match` that reads a reservation must state its answer for
    /// one. [`TypeKind::Reserved`] states the rule for a `match` with a catch-all arm.
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
        let popped = self.defs.pop();
        if let Some((id, _)) = &popped {
            // The freed id is reused by the next insert, which is a definition of its own.
            self.elided.remove(id);
        }
        popped
    }

    /// Withhold the definition at `id` from the output: codegen emits no item for it, and the API
    /// surface lists no type for it. It keeps its id and its place in [`iter`](Self::iter), so
    /// naming still allocates it a name and every other definition's name and emission order is
    /// the one it had before. Only a definition nothing in the API refers to may be elided;
    /// `check_invariants` reports a reference to one as a missing type.
    ///
    /// This is how a definition that is not the most recent one is dropped, where
    /// [`pop_last`](Self::pop_last) cannot reach it without renumbering the ones after it: the
    /// intermediate a struct meet made for a property a later member narrowed again (#428).
    pub(crate) fn elide(&mut self, id: TypeId) {
        debug_assert!(self.defs.contains_key(&id), "elide of an absent id");
        self.elided.insert(id);
    }

    /// Name the XML element the struct at `id` serializes as when it is the root of an XML body:
    /// codegen emits it as a serde container `rename`, which quick-xml takes as the root element's
    /// name. Every other serde format the generated client uses ignores a struct's name, so unlike
    /// a field's [`XmlField`] rename this changes no non-XML wire. Set only after lowering, once
    /// ids are final.
    pub(crate) fn set_xml_root_name(&mut self, id: TypeId, name: String) {
        debug_assert!(
            matches!(self.get(id).map(|def| &def.kind), Some(TypeKind::Struct(_))),
            "an XML root name on a definition that is not a struct"
        );
        self.xml_root_names.insert(id, name);
    }

    /// The XML root element name [`set_xml_root_name`](Self::set_xml_root_name) gave the struct at
    /// `id`, if any.
    pub(crate) fn xml_root_name(&self, id: TypeId) -> Option<&str> {
        self.xml_root_names.get(&id).map(String::as_str)
    }

    /// Whether the definition at `id` is withheld from the output (see [`elide`](Self::elide)).
    pub(crate) fn is_elided(&self, id: TypeId) -> bool {
        self.elided.contains(&id)
    }

    /// Iterate the `(id, def)` pairs the output carries, in insertion order: every definition but
    /// the [`elide`](Self::elide)d ones.
    pub(crate) fn emitted(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.iter().filter(|(id, _)| !self.is_elided(*id))
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

    /// Iterate `(id, def)` pairs in insertion order, [`elide`](Self::elide)d ones included. Naming
    /// allocates over this, so eliding a definition renames nothing; codegen, the API surface, and
    /// the post-lowering passes that report on a type read [`emitted`](Self::emitted) instead.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.defs.iter().map(|(id, def)| (*id, def))
    }

    /// Whether the definition at `id` reaches itself again through definitions that are emitted as
    /// aliases alone: an array or tuple whose items name `id`, or name another array or tuple that
    /// does, whatever `Option`/`Box` wraps each reference. rustc expands a type alias in place, so
    /// such a cycle of aliases has no finite expansion and is rejected (`E0391`); a reference that
    /// passes through a struct, enum, or union stops there, because those are nominal. Every member
    /// of the cycle answers `true`, independently of which one is visited first.
    ///
    /// Such a definition is emitted as a nominal `#[serde(transparent)]` newtype rather than an
    /// alias (#648), so every reader that asks which Rust type a definition is — codegen, the
    /// [`same_generated_type`](Self::same_generated_type) comparison, and the API surface — reads
    /// it here, and none of them can disagree about which definitions are nominal.
    ///
    /// Asked only after `check_invariants`, so it panics if the walk meets a reservation; a reader
    /// that can run earlier asks [`alias_cycle`](Self::alias_cycle), which answers that unknown.
    pub(crate) fn closes_alias_cycle(&self, id: TypeId) -> bool {
        self.alias_cycle(id).unwrap_or_else(|| {
            unreachable!(
                "an alias cycle was asked of a reservation; `check_invariants` should have \
                 rejected it"
            )
        })
    }

    /// Whether `id`'s alias walk reaches a reservation — `id` itself, or an array or tuple item
    /// along the way — so a `false` from [`same_generated_type`](Self::same_generated_type) for it
    /// is not final: it may answer `true` once the reservation is filled.
    pub(crate) fn reaches_reservation(&self, id: TypeId) -> bool {
        self.alias_cycle(id).is_none()
    }

    /// [`closes_alias_cycle`](Self::closes_alias_cycle), or `None` where the walk reaches a
    /// reservation — `id` itself, or an array or tuple item along the way — whose body is unknown,
    /// so whether it closes the cycle is unknown too.
    fn alias_cycle(&self, id: TypeId) -> Option<bool> {
        /// The references an alias's expansion contains, none for a definition that is not an
        /// alias of a container (a nominal type, or a leaf alias such as a primitive), or `None`
        /// for a reservation, whose expansion is not known yet.
        fn alias_items(kind: &TypeKind) -> Option<&[Ty]> {
            match kind {
                TypeKind::Array(item) => Some(std::slice::from_ref(&**item)),
                TypeKind::Tuple(items) => Some(items),
                TypeKind::Primitive(_)
                | TypeKind::Struct(_)
                | TypeKind::Enum(_)
                | TypeKind::Bytes
                | TypeKind::Null
                | TypeKind::Never
                | TypeKind::Union(_)
                | TypeKind::Any => Some(&[]),
                TypeKind::Reserved => None,
            }
        }
        let Some(def) = self.get(id) else {
            return Some(false);
        };
        let mut pending: Vec<TypeId> = alias_items(&def.kind)?.iter().map(|ty| ty.id).collect();
        let mut seen = BTreeSet::new();
        while let Some(next) = pending.pop() {
            if next == id {
                return Some(true);
            }
            if !seen.insert(next) {
                continue;
            }
            if let Some(def) = self.get(next) {
                pending.extend(alias_items(&def.kind)?.iter().map(|ty| ty.id));
            }
        }
        Some(false)
    }

    /// Whether two references emit the identical Rust type. Sound rather than complete: `true`
    /// means the generated types are one type, because every non-nominal definition is emitted as a
    /// transparent `pub type` alias of its structure. A nominal definition (a struct, string enum,
    /// union, `Never`, or an array or tuple that [closes an alias cycle](Self::closes_alias_cycle))
    /// is its own item and matches only itself. Both use-site modifiers must agree, so callers
    /// strip the top-level ones they absorb; nested ones are compared, because a tuple item keeps
    /// its `Box`. Independent of the `uuid`/`time` features: a feature-mapped primitive never
    /// equals `String`, even in a build where it would emit one.
    pub(crate) fn same_generated_type(&self, a: Ty, b: Ty) -> bool {
        self.same_generated_type_guarded(a, b, false, &mut Vec::new())
    }

    /// Whether two references decode exactly the same JSON values: [`same_generated_type`], or two
    /// nominal definitions that are distinct Rust items of one structure (#492). Two structs match
    /// when they declare the same wire names with the same `required` flags and XML hints, each
    /// field's type matches by this same test, and their `additionalProperties` policies match;
    /// two string enums match when they list the same value set and agree on whether they are
    /// open; two `Never` definitions match, since neither decodes any value (#643), so arrays of
    /// two such items both decode only `[]`. A union still matches only itself. Sound rather than
    /// complete, like
    /// [`same_generated_type`]: `true` means no value tells the two apart, which is what a `oneOf`'s
    /// exactly-one rule needs, but not that they are one Rust type, so a caller that needs one type
    /// (a response body's) asks [`same_generated_type`] instead. Annotations a decode never reads
    /// — docs, `deprecated`, `readOnly`/`writeOnly`, a field's `default` — are not compared.
    ///
    /// [`same_generated_type`]: Self::same_generated_type
    pub(crate) fn same_decoded_values(&self, a: Ty, b: Ty) -> bool {
        self.same_generated_type_guarded(a, b, true, &mut Vec::new())
    }

    /// [`same_generated_type`](Self::same_generated_type) when `structural` is false, and
    /// [`same_decoded_values`](Self::same_decoded_values) when it is true.
    fn same_generated_type_guarded(
        &self,
        a: Ty,
        b: Ty,
        structural: bool,
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
        // A cycle-member array or tuple is emitted as its own newtype, so as a Rust type it matches
        // only itself, which the id comparison above already answered. Its transparent newtype
        // decodes exactly what its structure does, so `structural` still compares that. A side
        // whose walk reaches a reservation (itself, or an array or tuple item) may fill to such a
        // newtype, so nothing proves it one type with the other: `false`, as the `Reserved` arm
        // below answers, rather than a panic.
        if !structural
            && (self.alias_cycle(a.id) != Some(false) || self.alias_cycle(b.id) != Some(false))
        {
            return false;
        }
        visiting.push(pair);
        let same = match (&a_def.kind, &b_def.kind) {
            (TypeKind::Primitive(x), TypeKind::Primitive(y)) => x == y,
            // Two distinct struct items decode the same values only when their structure does.
            (TypeKind::Struct(x), TypeKind::Struct(y)) if structural => {
                self.same_struct_shape(x, y, visiting)
            }
            // Integer and boolean enums are `pub type X = i64` / `bool` aliases; a string enum is a
            // real `pub enum`, so it is nominal, and decodes the same values as another one only
            // when both list the same values and both or neither admit the unlisted strings.
            (TypeKind::Enum(x), TypeKind::Enum(y))
                if x.repr == ScalarRepr::String && y.repr == ScalarRepr::String =>
            {
                structural
                    && x.is_open() == y.is_open()
                    && x.variants.iter().all(|value| y.variants.contains(value))
                    && y.variants.iter().all(|value| x.variants.contains(value))
            }
            (TypeKind::Enum(x), TypeKind::Enum(y)) => {
                x.repr == y.repr && x.repr != ScalarRepr::String
            }
            (TypeKind::Enum(scalar), TypeKind::Primitive(prim))
            | (TypeKind::Primitive(prim), TypeKind::Enum(scalar)) => matches!(
                (scalar.repr, prim),
                (ScalarRepr::Int, Prim::I64) | (ScalarRepr::Bool, Prim::Bool)
            ),
            (TypeKind::Array(x), TypeKind::Array(y)) => {
                self.same_generated_type_guarded(**x, **y, structural, visiting)
            }
            (TypeKind::Tuple(xs), TypeKind::Tuple(ys)) => {
                xs.len() == ys.len()
                    && xs.iter().zip(ys).all(|(x, y)| {
                        self.same_generated_type_guarded(*x, *y, structural, visiting)
                    })
            }
            // Each `Never` is its own empty `pub enum`, but neither decodes a value.
            (TypeKind::Never, TypeKind::Never) => structural,
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

    /// Whether two structs decode the same values, for
    /// [`same_decoded_values`](Self::same_decoded_values): the same wire names, each with the same
    /// `required` flag, XML hints, and a field type that decodes the same values, and matching
    /// `additionalProperties` policies. Field order is not compared; a decode reads by name.
    fn same_struct_shape(
        &self,
        x: &Struct,
        y: &Struct,
        visiting: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        let additional = match (&x.additional, &y.additional) {
            (AdditionalProps::Deny, AdditionalProps::Deny)
            | (AdditionalProps::Allow, AdditionalProps::Allow) => true,
            (AdditionalProps::Typed(x), AdditionalProps::Typed(y)) => {
                self.same_generated_type_guarded(**x, **y, true, visiting)
            }
            _ => false,
        };
        additional
            && x.fields.len() == y.fields.len()
            && x.fields.iter().all(|field| {
                y.fields.iter().any(|other| {
                    other.name == field.name
                        && other.required == field.required
                        && other.xml.name == field.xml.name
                        && other.xml.attribute == field.xml.attribute
                        && other.xml.unsupported == field.xml.unsupported
                        && self.same_generated_type_guarded(field.ty, other.ty, true, visiting)
                })
            })
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
    /// lowering — [`check_invariants`](super::check_invariants) reports a surviving one — so no
    /// consumer of a finished [`TypeGraph`] observes one.
    ///
    /// It is a variant of its own rather than a reuse of [`Self::Any`], because reading a
    /// reservation as `Any` is a plausible, wrong answer with no diagnostic. Every **exhaustive**
    /// `match` on [`TypeKind`] states what it does with a back edge, and the compiler holds a new
    /// one to the same.
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
    /// is not seen. The population is the test's to count.
    ///
    /// The answers fall into three kinds. Sites that run only on a checked `Api` — codegen,
    /// `name`, `surface`, `Api::uses_bytes_serde` — refuse with `unreachable!`, as
    /// `check_invariants` rejects a surviving reservation before any of them runs. Sites in
    /// `oas31::lower`, where a reservation really is live, answer "not proven": not the same type,
    /// not string-like, no `simple` shape, no representable default, no required-key
    /// discriminator — each routing to the refusal or warning that answer already had. And
    /// `push_ref_member` refuses an in-progress member
    /// itself (`E013`'s `allOf` unit rejection) rather than trusting each caller to have guarded
    /// it, so a new caller inherits the refusal. `intersect_non_null` likewise answers
    /// `Err(NoMeet::Unrepresentable)` for a reservation in its own first arm instead of relying
    /// on `intersect_types`' guard alone.
    ///
    /// **The invariant's own status.** The reservation arm of
    /// [`check_invariants`](super::check_invariants) reports a surviving reservation as `E011`
    /// (`Code::InvalidInput`): a frontend bug, not a spec problem. It shares that code because the
    /// refusals above keep every document from reaching it, so a code of its own would have no
    /// fixture to assert it. That holds only while the arm stays unreachable; the documents that
    /// would reach it without those refusals are fixtures in `spargen/tests/frontend/`.
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
    /// Object variants carry the tag values that select them. A non-object variant may coexist
    /// when its JSON category is unique (for example an array beside tagged objects).
    Discriminated {
        /// The discriminator `propertyName` — the tag field read from / written into the object.
        tag_field: String,
        /// Every tag value that selects each variant, parallel to [`Union::variants`], in dispatch
        /// order: each `mapping` key naming the variant's member in document order, then its
        /// implicit component name unless a `mapping` key claims that value. Deserialization
        /// accepts every entry; serialization writes the first. Empty for a variant routed by
        /// `categories`, for an object variant reached only as the `default_variant` fallback, and
        /// for an `untagged` variant.
        tags: Vec<Vec<String>>,
        /// The JSON category per non-object variant, parallel to [`Union::variants`].
        categories: Vec<Option<JsonCategory>>,
        /// OpenAPI 3.2 `defaultMapping`: the variant used when the tag is absent or unrecognized
        /// and no `untagged` variant matches. Without one, either case is a deserialization error.
        default_variant: Option<usize>,
        /// The trial priority of each object variant that no tag selects and that is not the
        /// `default_variant`, parallel to [`Union::variants`]; `None` for every other variant. Such
        /// a variant (inline, or a pointer into another schema, that no `mapping` entry names) has
        /// no discriminator value, so it is tried against the buffered value, with `mode`'s
        /// semantics, only when the tag is absent or selects no tagged variant; a tag that does
        /// select one dispatches to it alone. Serialization writes no tag for it.
        untagged: Vec<Option<u32>>,
        /// How the `untagged` variants are matched: the source applicator's semantics.
        mode: UnionMode,
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

#[cfg(test)]
mod tests {
    //! The source audit behind [`TypeKind::Reserved`]'s guarantee.
    //!
    //! A dedicated variant turns a read site into a compile error only where its `match` is
    //! exhaustive; a catch-all arm absorbs it silently. This walks every `match` in the crate's
    //! sources and holds each one that classifies a [`TypeKind`] to stating its answer for a
    //! reservation, so the audit is checked rather than counted by hand. The graph's elision
    //! bookkeeping is pinned here too.

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

    fn primitive(hint: &str) -> super::TypeDef {
        super::TypeDef {
            name_hint: hint.to_owned(),
            kind: super::TypeKind::Primitive(super::Prim::String),
            docs: super::Docs::default(),
            provenance: crate::diag::Provenance::new(crate::diag::JsonPointer::root(), None),
            document: String::new(),
        }
    }

    /// Eliding withholds a definition from [`super::TypeGraph::emitted`] and from nothing else: it
    /// keeps its id and its place in `iter`, which naming allocates over, so the definitions
    /// around it keep their ids, order, and names (#428).
    #[test]
    fn an_elided_definition_keeps_its_place_and_leaves_the_output() {
        let mut graph = super::TypeGraph::default();
        let ids: Vec<_> = ["A", "B", "C"]
            .into_iter()
            .map(|hint| graph.insert(primitive(hint)))
            .collect();
        graph.elide(ids[1]);
        let hints = |defs: Vec<(super::TypeId, &super::TypeDef)>| {
            defs.into_iter()
                .map(|(id, def)| (id, def.name_hint.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            hints(graph.iter().collect()),
            vec![
                (ids[0], "A".to_owned()),
                (ids[1], "B".to_owned()),
                (ids[2], "C".to_owned()),
            ]
        );
        assert_eq!(
            hints(graph.emitted().collect()),
            vec![(ids[0], "A".to_owned()), (ids[2], "C".to_owned())]
        );
        assert!(graph.is_elided(ids[1]) && !graph.is_elided(ids[0]) && !graph.is_elided(ids[2]));
    }

    /// An id `pop_last` frees is reused by the next insert, which is a new definition: popping an
    /// elided one clears its elision, so the definition that takes the id is emitted.
    #[test]
    fn popping_an_elided_definition_frees_its_id_unelided() {
        let mut graph = super::TypeGraph::default();
        graph.insert(primitive("A"));
        let popped = graph.insert(primitive("B"));
        graph.elide(popped);
        graph.pop_last();
        let reused = graph.insert(primitive("C"));
        assert_eq!(reused, popped);
        assert!(!graph.is_elided(reused));
        assert_eq!(graph.emitted().count(), 2);
    }

    /// Two string enums listing one value set decode the same values only when they agree on
    /// openness (#492): an open set also decodes every unlisted string, so it never merges with a
    /// closed one, while `Locked` is closed and merges with `Closed`. Pinned here rather than in
    /// `frontend/` because every `oneOf` branch is lowered closed, so no document reaches a
    /// union with an open and a closed branch.
    #[test]
    fn string_enums_of_one_value_set_decode_the_same_values_only_at_one_openness() {
        use super::{Openness, ScalarEnum, ScalarRepr, ScalarValue, Ty, TypeKind};
        let mut graph = super::TypeGraph::default();
        let mut set = |openness: Openness, values: [&str; 2]| {
            let mut def = primitive("E");
            def.kind = TypeKind::Enum(ScalarEnum {
                repr: ScalarRepr::String,
                variants: values
                    .into_iter()
                    .map(|value| ScalarValue::String(value.to_owned()))
                    .collect(),
                openness,
            });
            Ty {
                id: graph.insert(def),
                nullable: false,
                boxed: false,
            }
        };
        let closed = set(Openness::Closed, ["x", "y"]);
        let reordered = set(Openness::Closed, ["y", "x"]);
        let locked = set(Openness::Locked, ["x", "y"]);
        let open = set(Openness::Open, ["x", "y"]);
        let open_reordered = set(Openness::Open, ["y", "x"]);
        assert!(graph.same_decoded_values(closed, reordered));
        assert!(graph.same_decoded_values(closed, locked));
        assert!(graph.same_decoded_values(open, open_reordered));
        assert!(!graph.same_decoded_values(closed, open));
        assert!(!graph.same_decoded_values(open, closed));
        assert!(!graph.same_decoded_values(locked, open));
        // Two distinct string enums are never one generated type, whatever their values.
        assert!(!graph.same_generated_type(closed, reordered));
    }

    /// Two distinct `Never` definitions decode the same values, none (#643), and so do arrays of
    /// them, which decode only `[]`; each is still its own nominal Rust item.
    #[test]
    fn two_never_definitions_decode_the_same_values_but_are_distinct_types() {
        use super::{Ty, TypeKind};
        let mut graph = super::TypeGraph::default();
        let mut insert = |kind: TypeKind| {
            let mut def = primitive("N");
            def.kind = kind;
            Ty {
                id: graph.insert(def),
                nullable: false,
                boxed: false,
            }
        };
        let first = insert(TypeKind::Never);
        let second = insert(TypeKind::Never);
        let first_array = insert(TypeKind::Array(Box::new(first)));
        let second_array = insert(TypeKind::Array(Box::new(second)));
        let string = insert(TypeKind::Primitive(super::Prim::String));
        assert!(graph.same_decoded_values(first, second));
        assert!(graph.same_decoded_values(first_array, second_array));
        assert!(!graph.same_decoded_values(first, string));
        assert!(!graph.same_generated_type(first, second));
        assert!(!graph.same_generated_type(first_array, second_array));
    }
}
