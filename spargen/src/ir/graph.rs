//! The [`TypeGraph`] that owns every type definition, and the questions asked of it as a whole:
//! which definitions the output carries, which close an alias cycle, which generate the same Rust
//! type, and the item each is emitted as.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;

use super::types::{
    AdditionalProps, BytesEncoding, Prim, ScalarEnum, ScalarRepr, Struct, Ty, TypeDef, TypeId,
    TypeKind, Union,
};
use super::Docs;
use crate::diag::{JsonPointer, Provenance};

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
    /// a field's [`XmlField`](super::XmlField) rename this changes no non-XML wire. Set only after lowering, once
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

    /// The item codegen emits for `def`, the definition `id` names: the one decision codegen and
    /// the API surface both read, so neither can list a definition as an item of another kind.
    ///
    /// Asked only after `check_invariants`, so it panics on a reservation.
    pub(crate) fn emission<'a>(&self, id: TypeId, def: &'a TypeDef) -> Emission<'a> {
        match &def.kind {
            TypeKind::Struct(object) => Emission::Struct(object),
            TypeKind::Enum(enumeration) if enumeration.repr == ScalarRepr::String => {
                Emission::StringEnum(enumeration)
            }
            TypeKind::Enum(enumeration) if enumeration.is_every_boolean() => Emission::BoolAlias,
            TypeKind::Enum(enumeration) => Emission::Checked(enumeration),
            TypeKind::Union(union) => Emission::Union(union),
            TypeKind::Never => Emission::Never,
            TypeKind::Bytes(BytesEncoding::Base64) => Emission::Base64,
            TypeKind::Reserved => unreachable!(
                "a reservation reached emission; `check_invariants` should have rejected it"
            ),
            TypeKind::Array(_) | TypeKind::Tuple(_) if self.closes_alias_cycle(id) => {
                Emission::Newtype
            }
            _ => Emission::Alias,
        }
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
                | TypeKind::Bytes(_)
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
    /// integer or boolean set's checked newtype, union, `Never`, base64 bytes, or an array or tuple
    /// that
    /// [closes an alias cycle](Self::closes_alias_cycle)) is its own item and matches only
    /// itself. Both use-site modifiers must agree, so callers
    /// strip the top-level ones they absorb; nested ones are compared, because a tuple item keeps
    /// its `Box`. Independent of the `uuid`/`time` features: a feature-mapped primitive never
    /// equals `String`, even in a build where it would emit one.
    pub(crate) fn same_generated_type(&self, a: Ty, b: Ty) -> bool {
        self.same_generated_type_guarded(a, b, false, &mut Vec::new())
    }

    /// Whether two references decode exactly the same JSON values: [`same_generated_type`], or two
    /// nominal definitions that are distinct Rust items of one structure (#492). Two structs match
    /// when they declare the same wire names, each either decoded as optional or not
    /// ([`Field::decodes_optional`], which `required` and `readOnly`/`writeOnly` decide), with the
    /// same XML hints, each field's type matches by this same test, and their
    /// `additionalProperties` policies match;
    /// two string enums, or two integer or boolean sets' checked newtypes, match when they are held
    /// in one width, list the same value set and agree on whether they are open; two `Never`
    /// definitions match, since neither decodes any value (#643), so arrays of two such items both
    /// decode only `[]`; two base64 bytes newtypes match, since both decode
    /// the same base64 strings. A union still matches only itself. Sound rather than
    /// complete, like
    /// [`same_generated_type`]: `true` means no value tells the two apart, which is what a `oneOf`'s
    /// exactly-one rule needs, but not that they are one Rust type, so a caller that needs one type
    /// (a response body's) asks [`same_generated_type`] instead. Annotations a decode never reads
    /// — docs, `deprecated`, a field's `default` — are not compared.
    ///
    /// [`same_generated_type`]: Self::same_generated_type
    /// [`Field::decodes_optional`]: crate::ir::Field::decodes_optional
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
            // A boolean set listing both values is a `pub type X = bool` alias. Every other set is
            // nominal — a string enum's `pub enum`, an integer or boolean set's checked newtype —
            // and decodes the same values as another one only when both are held in one width,
            // list the same values, and both or neither admit the unlisted ones.
            (TypeKind::Enum(x), TypeKind::Enum(y))
                if x.is_every_boolean() && y.is_every_boolean() =>
            {
                true
            }
            (TypeKind::Enum(x), TypeKind::Enum(y)) => {
                structural
                    && x.repr == y.repr
                    && x.width == y.width
                    && !x.is_every_boolean()
                    && !y.is_every_boolean()
                    && x.is_open() == y.is_open()
                    && x.variants.iter().all(|value| y.variants.contains(value))
                    && y.variants.iter().all(|value| x.variants.contains(value))
            }
            (TypeKind::Enum(scalar), TypeKind::Primitive(prim))
            | (TypeKind::Primitive(prim), TypeKind::Enum(scalar)) => {
                scalar.is_every_boolean() && *prim == Prim::Bool
            }
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
            // Each base64 definition is its own newtype, but all decode the same strings. Raw and
            // base64 bytes differ on the wire, so they never match.
            (TypeKind::Bytes(BytesEncoding::Base64), TypeKind::Bytes(BytesEncoding::Base64)) => {
                structural
            }
            (TypeKind::Bytes(BytesEncoding::Raw), TypeKind::Bytes(BytesEncoding::Raw))
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
    /// [`same_decoded_values`](Self::same_decoded_values): the same wire names, each decoded as
    /// optional or not alike, with the same XML hints, and a field type that decodes the same
    /// values, and matching `additionalProperties` policies. Field order is not compared; a decode reads by name.
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
                        && other.decodes_optional() == field.decodes_optional()
                        && other.xml.name == field.xml.name
                        && other.xml.attribute == field.xml.attribute
                        && other.xml.unsupported == field.xml.unsupported
                        && self.same_generated_type_guarded(field.ty, other.ty, true, visiting)
                })
            })
    }
}

/// The item a definition is emitted as, by [`TypeGraph::emission`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Emission<'a> {
    /// A `pub struct` with one field per property.
    Struct(&'a Struct),
    /// A `pub enum` with one unit variant per listed string (plus a catch-all when open).
    StringEnum(&'a ScalarEnum),
    /// A `pub struct X(i32);` checked newtype over an integer or boolean set's
    /// [width](ScalarEnum::width), with an associated `const` per listed value, whose `TryFrom`
    /// and decode refuse any other value (accept it, for an open set).
    Checked(&'a ScalarEnum),
    /// `pub type X = bool;`: a boolean set listing both values, which narrows nothing.
    BoolAlias,
    /// A `pub enum` with one variant per union member.
    Union(&'a Union),
    /// `pub struct X(pub T);`, `#[serde(transparent)]`: an array or tuple that
    /// [closes an alias cycle](TypeGraph::closes_alias_cycle), which an alias cannot express.
    Newtype,
    /// The empty `pub enum X {}` of an uninhabited schema.
    Never,
    /// `pub struct X(pub bytes::Bytes);` of a `contentEncoding: base64` schema, which serde
    /// writes and reads as a base64 string rather than `bytes::Bytes`' own sequence of numbers.
    Base64,
    /// `pub type X = …;` naming the type the definition lowered to.
    Alias,
}

#[cfg(test)]
mod tests {
    fn primitive(hint: &str) -> crate::ir::TypeDef {
        crate::ir::TypeDef {
            name_hint: hint.to_owned(),
            kind: crate::ir::TypeKind::Primitive(crate::ir::Prim::String),
            docs: crate::ir::Docs::default(),
            provenance: crate::diag::Provenance::new(crate::diag::JsonPointer::root(), None),
            document: String::new(),
        }
    }

    /// Eliding withholds a definition from [`crate::ir::TypeGraph::emitted`] and from nothing else: it
    /// keeps its id and its place in `iter`, which naming allocates over, so the definitions
    /// around it keep their ids, order, and names (#428).
    #[test]
    fn an_elided_definition_keeps_its_place_and_leaves_the_output() {
        let mut graph = crate::ir::TypeGraph::default();
        let ids: Vec<_> = ["A", "B", "C"]
            .into_iter()
            .map(|hint| graph.insert(primitive(hint)))
            .collect();
        graph.elide(ids[1]);
        let hints = |defs: Vec<(crate::ir::TypeId, &crate::ir::TypeDef)>| {
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
        let mut graph = crate::ir::TypeGraph::default();
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
        use crate::ir::{Openness, ScalarEnum, ScalarRepr, ScalarValue, Ty, TypeKind};
        let mut graph = crate::ir::TypeGraph::default();
        let mut set = |openness: Openness, values: [&str; 2]| {
            let mut def = primitive("E");
            def.kind = TypeKind::Enum(ScalarEnum {
                repr: ScalarRepr::String,
                variants: values
                    .into_iter()
                    .map(|value| ScalarValue::String(value.to_owned()))
                    .collect(),
                width: crate::ir::Prim::String,
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

    /// An integer or boolean set is a checked newtype (#1327): nominal, so it is no other type,
    /// and decodes the values of another set only where both are held in one width and list one
    /// value set. A boolean set listing both values is the `bool` alias, one type with `bool`.
    #[test]
    fn integer_and_boolean_sets_are_nominal_unless_they_list_every_boolean() {
        use crate::ir::{Openness, Prim, ScalarEnum, ScalarRepr, ScalarValue, Ty, TypeKind};
        let mut graph = crate::ir::TypeGraph::default();
        let mut insert = |kind: TypeKind| {
            let mut def = primitive("S");
            def.kind = kind;
            Ty {
                id: graph.insert(def),
                nullable: false,
                boxed: false,
            }
        };
        let set = |repr: ScalarRepr, width: Prim, variants: Vec<ScalarValue>| {
            TypeKind::Enum(ScalarEnum {
                repr,
                variants,
                width,
                openness: Openness::Closed,
            })
        };
        let ints = |values: &[i64]| values.iter().copied().map(ScalarValue::Int).collect();
        let small = insert(set(ScalarRepr::Int, Prim::I64, ints(&[1, 2])));
        let reordered = insert(set(ScalarRepr::Int, Prim::I64, ints(&[2, 1])));
        let narrow = insert(set(ScalarRepr::Int, Prim::I32, ints(&[1, 2])));
        let wide = insert(TypeKind::Primitive(Prim::I64));
        let bools = |values: &[bool]| values.iter().copied().map(ScalarValue::Bool).collect();
        let only_true = insert(set(ScalarRepr::Bool, Prim::Bool, bools(&[true])));
        let both = insert(set(ScalarRepr::Bool, Prim::Bool, bools(&[true, false])));
        let both_reordered = insert(set(ScalarRepr::Bool, Prim::Bool, bools(&[false, true])));
        let boolean = insert(TypeKind::Primitive(Prim::Bool));

        assert!(graph.same_decoded_values(small, reordered));
        assert!(!graph.same_decoded_values(small, narrow));
        assert!(!graph.same_generated_type(small, reordered));
        assert!(!graph.same_generated_type(small, wide));
        assert!(!graph.same_decoded_values(small, wide));
        assert!(graph.same_generated_type(both, boolean));
        assert!(graph.same_generated_type(both, both_reordered));
        assert!(!graph.same_generated_type(only_true, boolean));
        assert!(!graph.same_decoded_values(only_true, both));
        let crate::ir::Emission::Checked(_) =
            graph.emission(only_true.id, graph.get(only_true.id).expect("inserted"))
        else {
            panic!("a one-value boolean set is a checked newtype");
        };
        assert!(matches!(
            graph.emission(both.id, graph.get(both.id).expect("inserted")),
            crate::ir::Emission::BoolAlias
        ));
    }

    /// Two distinct `Never` definitions decode the same values, none (#643), and so do arrays of
    /// them, which decode only `[]`; each is still its own nominal Rust item.
    #[test]
    fn two_never_definitions_decode_the_same_values_but_are_distinct_types() {
        use crate::ir::{Ty, TypeKind};
        let mut graph = crate::ir::TypeGraph::default();
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
        let string = insert(TypeKind::Primitive(crate::ir::Prim::String));
        assert!(graph.same_decoded_values(first, second));
        assert!(graph.same_decoded_values(first_array, second_array));
        assert!(!graph.same_decoded_values(first, string));
        assert!(!graph.same_generated_type(first, second));
        assert!(!graph.same_generated_type(first_array, second_array));
    }

    /// A `required` `readOnly` or `writeOnly` member decodes as absent when missing (#1326), so a
    /// struct requiring it decodes the values one leaving it optional does, and not those of one
    /// requiring it plainly.
    #[test]
    fn a_relaxed_required_member_decodes_as_an_optional_one_does() {
        use crate::ir::{AdditionalProps, Field, PropertyName, Struct, Ty, TypeKind};
        let mut graph = crate::ir::TypeGraph::default();
        let id = graph.insert(primitive("S"));
        let id = Ty {
            id,
            nullable: false,
            boxed: false,
        };
        let mut object = |required: bool, read_only: bool, write_only: bool| {
            let mut def = primitive("O");
            def.kind = TypeKind::Struct(Struct {
                fields: vec![Field {
                    name: PropertyName {
                        wire: "id".to_owned(),
                    },
                    ty: id,
                    required,
                    deprecated: false,
                    read_only,
                    write_only,
                    default: None,
                    xml: Default::default(),
                    undeclared: false,
                }],
                additional: AdditionalProps::Allow,
            });
            Ty {
                id: graph.insert(def),
                nullable: false,
                boxed: false,
            }
        };
        let optional = object(false, false, false);
        let required = object(true, false, false);
        let required_read_only = object(true, true, false);
        let required_write_only = object(true, false, true);
        let optional_read_only = object(false, true, false);
        assert!(graph.same_decoded_values(required_read_only, optional));
        assert!(graph.same_decoded_values(required_write_only, optional));
        assert!(graph.same_decoded_values(optional_read_only, optional));
        assert!(!graph.same_decoded_values(required_read_only, required));
        assert!(!graph.same_decoded_values(required_write_only, required));
    }
}
