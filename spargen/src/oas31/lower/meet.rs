//! The meet of two lowered types: intersecting primitives, enums, arrays, maps and structs, and
//! merging the field declarations an intersection combines.

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{
    AdditionalProps, Docs, Field, FieldDefault, JsonCategory, Openness, Prim, ScalarEnum,
    ScalarRepr, ScalarValue, Struct, Ty, TypeId, TypeKind, Union,
};

use super::defaults::reclassify_default;
use super::nullability::{non_nullable, type_accepts_null};
use super::{LowerCtx, Refiner, ScopeReach};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Merge two `additionalProperties` policies for an `allOf` intersection. `Deny` dominates (a
    /// value must satisfy every member, so any member denying unknown keys forbids them outright),
    /// whatever the other side's value schema is. Two typed value schemas merge to their typed
    /// intersection ([`Self::intersect_types`], under `hint`), so they need not lower to the same
    /// type; a typed one beside `Allow` is kept as it is, and two `Allow`s stay `Allow`. Returns
    /// `None` only when the two typed value schemas have no typed intersection, empty or
    /// unrepresentable alike.
    pub(super) fn merge_additional(
        &mut self,
        acc: &AdditionalProps,
        next: &AdditionalProps,
        hint: &str,
    ) -> Option<AdditionalProps> {
        Some(match (acc, next) {
            (AdditionalProps::Deny, _) | (_, AdditionalProps::Deny) => AdditionalProps::Deny,
            (AdditionalProps::Typed(x), AdditionalProps::Typed(y)) => {
                let intersection = self.intersect_types(**x, **y, hint).ok()?;
                AdditionalProps::Typed(Box::new(intersection))
            }
            (AdditionalProps::Typed(x), AdditionalProps::Allow)
            | (AdditionalProps::Allow, AdditionalProps::Typed(x)) => {
                AdditionalProps::Typed(x.clone())
            }
            (AdditionalProps::Allow, AdditionalProps::Allow) => AdditionalProps::Allow,
        })
    }

    /// The type of a [`Field::undeclared`] field `field` once it is also a key another object does
    /// not declare, whose overflow policy is `additional`: a typed value schema there constrains
    /// the key as well, and `true`, `false` and an absent one leave it as it was (`false` closes
    /// the object to the fields the merged type declares, this one included, as in
    /// [`Self::object_body`]).
    pub(super) fn narrow_undeclared(
        &mut self,
        field: Ty,
        additional: &AdditionalProps,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let AdditionalProps::Typed(value) = additional else {
            return Ok(field);
        };
        // An unconstrained field simply takes the value type. A map value dropped its `Box`
        // because the map is the indirection a cycle-closing reference needs; a plain field has
        // none, so it is boxed again exactly when the value's target is still being lowered.
        if matches!(
            self.graph.get(field.id).map(|def| &def.kind),
            Some(TypeKind::Any)
        ) {
            let mut ty = **value;
            ty.boxed = self.is_in_progress_root(ty.id);
            return Ok(ty);
        }
        let mut ty = self.intersect_types(field, **value, hint)?;
        ty.boxed = field.boxed || self.is_in_progress_root(ty.id);
        Ok(ty)
    }

    /// Compute a typed intersection for two already-lowered schemas. Nullability is intersected
    /// independently from the non-null shape; an intersection containing only JSON `null` becomes
    /// [`TypeKind::Null`]. Derived arrays, objects, enums, and narrowed unions are inserted into the
    /// graph so codegen still sees an ordinary, fully typed IR node.
    ///
    /// No typed intersection is one of two answers, and [`NoMeet`] says which: an empty one may be
    /// typed uninhabited where an empty value remains (an array's items, a property no side
    /// requires) or collapse to `null` where both sides admit it, while an unrepresentable one is
    /// never narrowed that way — it reaches a caller that reports it.
    pub(super) fn intersect_types(&mut self, a: Ty, b: Ty, hint: &str) -> Result<Ty, NoMeet> {
        let (Some(a_def), Some(b_def)) = (self.graph.get(a.id), self.graph.get(b.id)) else {
            return Err(NoMeet::Unrepresentable);
        };
        let a_kind = a_def.kind.clone();
        let b_kind = b_def.kind.clone();

        // Fail closed on a reservation, BEFORE nullability is consulted. A `TypeKind::Reserved`
        // operand is a placeholder whose body is still being lowered, so no true statement can be
        // made about the intersection — `is_in_progress_root`'s own documentation says the only
        // safe thing to do with one is refuse to read it. The callers above guard their own paths,
        // but a guard that asks about the *spelling* of a reference rather than its resolved
        // identity lets one through, and the rescue below then converted that unanswerable
        // intersection into a confident wrong answer: `intersect_non_null` found no meet for it
        // (it now refuses by a `Reserved` arm of its own), and the null rescue typed the
        // position as the exact JSON null type. The result was `pub type X = ();` — a client that
        // decodes only `null` for a schema that accepts objects — emitted with no diagnostic,
        // which is the standing invariant's fourth, silent behaviour.
        //
        // Refusing it as `NoMeet::Unrepresentable` hands the refusal to the caller, and that answer
        // is never typed uninhabited or collapsed to `null`: the `Never` fallbacks (an array's
        // items, a property no side requires) take only `NoMeet::Empty`, and so does the null
        // rescue below. So a reservation that slips past a caller-side guard is still rejected.
        //
        // A reservation intersected with ITSELF is exempt: `X ∩ X = X` needs no knowledge of the
        // body, and it is how every ordinary recursive schema composes when two `allOf` members
        // repeat one construct. Refusing it rejected those documents with a false "conflicting
        // types" message. `intersect_non_null` answers it by its identity short-circuit.
        if a.id != b.id
            && (matches!(a_kind, TypeKind::Reserved) || matches!(b_kind, TypeKind::Reserved))
        {
            return Err(NoMeet::Unrepresentable);
        }

        let accepts_null = type_accepts_null(a, &a_kind) && type_accepts_null(b, &b_kind);

        let non_null = if matches!(a_kind, TypeKind::Null) || matches!(b_kind, TypeKind::Null) {
            Err(NoMeet::Empty)
        } else {
            self.intersect_non_null(a, &a_kind, b, &b_kind, hint)
        };

        match non_null {
            Ok(mut ty) => {
                ty.nullable = accepts_null;
                Ok(ty)
            }
            // Only an EMPTY non-null meet leaves exactly `null`. An unrepresentable one still holds
            // the non-null values the two sides share, and `()` would refuse every one of them.
            Err(NoMeet::Empty) if accepts_null => {
                Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
            }
            Err(no_meet) => Err(no_meet),
        }
    }

    fn intersect_non_null(
        &mut self,
        a: Ty,
        a_kind: &TypeKind,
        b: Ty,
        b_kind: &TypeKind,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        if a.id == b.id {
            let mut ty = a;
            ty.nullable = false;
            ty.boxed = a.boxed || b.boxed;
            return Ok(ty);
        }

        match (a_kind, b_kind) {
            // Nothing true can be said about intersecting an unlowered body with anything else
            // (the identical reservation answered above by id). `intersect_types` refuses this
            // before calling here; stating it again means a new caller inherits the refusal rather
            // than reaching the `Any` arms below, which would answer with the placeholder itself.
            (TypeKind::Reserved, _) | (_, TypeKind::Reserved) => Err(NoMeet::Unrepresentable),
            (TypeKind::Any, _) => Ok(non_nullable(b)),
            (_, TypeKind::Any) => Ok(non_nullable(a)),
            (TypeKind::Primitive(left), TypeKind::Primitive(right)) => {
                let Some(primitive) = intersect_primitives(*left, *right) else {
                    return Err(no_meet(a_kind, b_kind));
                };
                if primitive == *left {
                    Ok(non_nullable(a))
                } else if primitive == *right {
                    Ok(non_nullable(b))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Primitive(primitive),
                        Docs::default(),
                        None,
                    ))
                }
            }
            // A set's variants are the values the description lists, open or not: `open` says only
            // that the lowering also holds an unlisted string, because a plain `string` was met
            // (`narrowed_string`). So two sets meet in the values both list, open when either is.
            // Both parts are order-independent (an intersection and a disjunction), so an `allOf`
            // lowers to the same set whichever order its members are written in; keeping the
            // closed side whole instead would admit values the open side's description forbids.
            // Where `open_narrowing` is out of effect (inside a union, which `intersect_union`
            // reaches with a set the response already opened) the meet is closed: two variants
            // that each held an unlisted string would both match it, and the trial union would
            // refuse every value. A locked set (one narrowed against a `uuid` or date string) locks
            // the meet, open side or not, which is again order-independent: the format's domain
            // holds no unlisted string for the open side to keep.
            (TypeKind::Enum(left), TypeKind::Enum(right)) if left.repr == right.repr => {
                let variants: Vec<ScalarValue> = left
                    .variants
                    .iter()
                    .filter(|value| right.variants.contains(value))
                    .cloned()
                    .collect();
                let openness =
                    if left.openness == Openness::Locked || right.openness == Openness::Locked {
                        Openness::Locked
                    } else if self.narrowing_opens && (left.is_open() || right.is_open()) {
                        Openness::Open
                    } else {
                        Openness::Closed
                    };
                if variants.is_empty() {
                    // Both value sets are finite and listed in full, so sharing no value is proof.
                    Err(NoMeet::Empty)
                } else if variants == left.variants && openness == left.openness {
                    Ok(non_nullable(a))
                } else if variants == right.variants && openness == right.openness {
                    Ok(non_nullable(b))
                } else if variants == left.variants {
                    Ok(self.reopened_set(a, left, openness, hint))
                } else if variants == right.variants {
                    Ok(self.reopened_set(b, right, openness, hint))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Enum(ScalarEnum {
                            repr: left.repr,
                            variants,
                            openness,
                        }),
                        Docs::default(),
                        None,
                    ))
                }
            }
            (TypeKind::Enum(enumeration), TypeKind::Primitive(primitive))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Ok(self.narrowed_string(a, enumeration, *primitive, hint))
            }
            (TypeKind::Primitive(primitive), TypeKind::Enum(enumeration))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Ok(self.narrowed_string(b, enumeration, *primitive, hint))
            }
            (TypeKind::Array(left), TypeKind::Array(right)) => {
                let item_hint = format!("{hint}Item");
                let item = match self.intersect_types(**left, **right, &item_hint) {
                    Ok(item) => item,
                    // No item satisfies both, so exactly the empty array satisfies both arrays:
                    // `Vec<Never>` is faithful.
                    Err(NoMeet::Empty) => {
                        self.insert_type(&item_hint, TypeKind::Never, Docs::default(), None)
                    }
                    // Items both sides admit exist, and `Vec<Never>` would refuse every array that
                    // holds one.
                    Err(NoMeet::Unrepresentable) => return Err(NoMeet::Unrepresentable),
                };
                if same_ty(item, **left) {
                    Ok(non_nullable(a))
                } else if same_ty(item, **right) {
                    Ok(non_nullable(b))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Array(Box::new(item)),
                        Docs::default(),
                        None,
                    ))
                }
            }
            // A position with no intersection is unrepresentable rather than empty, whichever way
            // it fails: `prefixItems` does not require the array to reach that position, so an
            // array shorter than it still satisfies both tuples.
            (TypeKind::Tuple(left), TypeKind::Tuple(right)) if left.len() == right.len() => {
                let items = self
                    .intersect_positions(left.iter().copied().zip(right.iter().copied()), hint)?;
                Ok(self.insert_type(hint, TypeKind::Tuple(items), Docs::default(), None))
            }
            // A homogeneous array against a tuple: every tuple position must also satisfy the
            // array's item schema, and the length is the tuple's. So the intersection is the tuple
            // with each position narrowed by the item — `{$ref: Coord, type: array}` over a
            // `prefixItems` `Coord` is `Coord`. A position with no intersection leaves no tuple,
            // and, as for two tuples, that is unrepresentable rather than empty.
            (TypeKind::Array(item), TypeKind::Tuple(positions)) => {
                self.intersect_array_tuple(**item, positions, b, hint)
            }
            (TypeKind::Tuple(positions), TypeKind::Array(item)) => {
                self.intersect_array_tuple(**item, positions, a, hint)
            }
            (TypeKind::Struct(left), TypeKind::Struct(right)) => {
                let location = self
                    .authored_location(a)
                    .or_else(|| self.authored_location(b));
                self.intersect_structs(left, right, hint, location)
            }
            // A union's variants stay closed for the reason `lower_union_closed` gives; a union
            // that narrows to one branch is no union, and `intersect_union` meets that branch where
            // the enclosing position's answer holds.
            (TypeKind::Union(union), _) => self.intersect_with_union(a, union, b, hint),
            (_, TypeKind::Union(union)) => self.intersect_with_union(b, union, a, hint),
            (TypeKind::Bytes, TypeKind::Bytes) => Ok(non_nullable(a)),
            // Binary content (`format: binary` / `contentEncoding: base64`) is a string, so a plain
            // string conjoined with it is the binary content: `{$ref: Data, format: binary}` over a
            // string `Data` lowers exactly as the inline `{type: string, format: binary}` does. Only
            // the unformatted string: `uuid` and the date formats carry a decoded representation of
            // their own that `Bytes` cannot also be, so that pair is unrepresentable (`no_meet`).
            (TypeKind::Bytes, TypeKind::Primitive(Prim::String)) => Ok(non_nullable(a)),
            (TypeKind::Primitive(Prim::String), TypeKind::Bytes) => Ok(non_nullable(b)),
            _ => Err(no_meet(a_kind, b_kind)),
        }
    }

    /// The intersection of a homogeneous array whose items are `item` with the tuple `tuple`, whose
    /// positions are `positions`: the tuple, each position intersected with `item`. Returns the
    /// tuple itself when no position narrowed, and [`NoMeet::Unrepresentable`] when any position
    /// has no intersection.
    fn intersect_array_tuple(
        &mut self,
        item: Ty,
        positions: &[Ty],
        tuple: Ty,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let items =
            self.intersect_positions(positions.iter().map(|position| (*position, item)), hint)?;
        if items
            .iter()
            .zip(positions)
            .all(|(narrowed, position)| same_ty(*narrowed, *position))
        {
            Ok(non_nullable(tuple))
        } else {
            Ok(self.insert_type(hint, TypeKind::Tuple(items), Docs::default(), None))
        }
    }

    /// Merge `field` into `existing`, a property both sides of an intersection carry: the part the
    /// `allOf` merge and [`Self::intersect_structs`] share, each keeping its own policy for a meet
    /// that fails. Where one side carries the name only because it requires it,
    /// [`take_declaration`] keeps the other side's declaration and nothing is met (`None`).
    /// Otherwise the two types are met under `hint`, the field is required where either side
    /// requires it, and a met field that is required drops its applied `default`. A failed meet
    /// leaves `existing.ty` as it was for the caller to settle.
    ///
    /// `pairwise_defaults` merges the two sides' `default`s first (see [`merge_field_default`]),
    /// for a caller that meets exactly two sides; the `allOf` merge gathers every member's before
    /// deciding instead, and passes `false`.
    pub(super) fn merge_repeated_field(
        &mut self,
        existing: &mut Field,
        field: &Field,
        hint: &str,
        pairwise_defaults: bool,
    ) -> Option<Result<(), NoMeet>> {
        if take_declaration(existing, field) {
            return None;
        }
        if pairwise_defaults {
            // Either side's `default` is a default of the merged field, whichever side is the
            // `$ref` (see `merge_field_default`).
            let defaults = existing
                .default
                .take()
                .into_iter()
                .chain(field.default.clone())
                .collect();
            existing.default = merge_field_default(defaults, &field.name.wire, self.diags);
        }
        // A repeated property is an intersection, not an equality assertion: retain the narrower
        // compatible type.
        let intersection = self.intersect_types(existing.ty, field.ty, hint);
        existing.required = existing.required || field.required;
        Some(intersection.map(|met| {
            existing.ty = met;
            if existing.required {
                if let Some(default) = &mut existing.default {
                    default.applied = None;
                }
            }
        }))
    }

    /// The meet of `union_ty`, whose kind is `union`, with `other`, branch by branch, under a
    /// closed narrowing: the answer for a union on either side of [`Self::intersect_non_null`].
    fn intersect_with_union(
        &mut self,
        union_ty: Ty,
        union: &Union,
        other: Ty,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let enclosing = self.narrowing_opens;
        self.closed_narrowing(|ctx| {
            let reach = &mut ScopeReach::default();
            ctx.intersect_union(
                union_ty,
                union,
                Refiner::Whole(other),
                hint,
                enclosing,
                reach,
            )
        })
    }

    /// Meet a tuple's positions pair by pair, position `index` under `{hint}Item{index}`: the
    /// positions of two tuples, or of a tuple and an array's item. A position with no meet, empty or
    /// not, leaves no tuple, and that is [`NoMeet::Unrepresentable`]: `prefixItems` does not
    /// require an array to reach the position, so a shorter array still satisfies both sides.
    fn intersect_positions(
        &mut self,
        pairs: impl Iterator<Item = (Ty, Ty)>,
        hint: &str,
    ) -> Result<Vec<Ty>, NoMeet> {
        pairs
            .enumerate()
            .map(|(index, (left, right))| {
                self.intersect_types(left, right, &format!("{hint}Item{index}"))
                    .ok()
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(NoMeet::Unrepresentable)
    }

    fn intersect_structs(
        &mut self,
        left: &Struct,
        right: &Struct,
        hint: &str,
        location: Option<Provenance>,
    ) -> Result<Ty, NoMeet> {
        let mut fields: IndexMap<String, Field> = left
            .fields
            .iter()
            .cloned()
            .map(|field| (field.name.wire.clone(), field))
            .collect();
        // An unrepresentable property is remembered rather than returned at once: a later
        // required property whose types are disjoint still proves the whole object empty.
        let mut unrepresentable = false;
        // So is an emptied one, though nothing outranks it: every property is still merged, so the
        // `default`s the merge drops are reported (`W005`) beside the verdict, as the `allOf`
        // merge, which reads every member before deciding, reports them (#545).
        let mut empty = false;
        for field in &right.fields {
            match fields.get_mut(&field.name.wire) {
                Some(existing) => {
                    let field_hint = format!("{hint}{}", field.name.wire);
                    match self.merge_repeated_field(existing, field, &field_hint, true) {
                        None | Some(Ok(())) => {}
                        // Mirrors the array arm above, and for the same reason `E013`'s explain
                        // gives for it: a property NEITHER side requires does not empty the
                        // object when its two types cannot meet, because every instance that
                        // omits it still satisfies both sides. The field takes an uninhabited
                        // type, so the instances that remain representable are exactly the valid
                        // ones. Propagating the failure would reject a document that `{}`
                        // satisfies. An applied `default` is no value of the uninhabited type, and
                        // is left for `retype_field_defaults` to report (`W005`) where it was
                        // written and document as not applied (#453).
                        Some(Err(NoMeet::Empty)) if !existing.required => {
                            existing.ty = self.insert_type(
                                &field_hint,
                                TypeKind::Never,
                                Docs::default(),
                                None,
                            );
                        }
                        // Required on one side or the other: every instance must carry a value no
                        // type admits, so the composition really is empty.
                        Some(Err(NoMeet::Empty)) => empty = true,
                        // Values both sides admit exist, so an uninhabited field would refuse every
                        // object carrying one, required or not.
                        Some(Err(NoMeet::Unrepresentable)) => unrepresentable = true,
                    }
                }
                None => {
                    fields.insert(field.name.wire.clone(), field.clone());
                }
            }
        }
        if empty {
            return Err(NoMeet::Empty);
        }
        // A field neither side declares is an undeclared key of the side that does not carry it
        // too, so that side's `additionalProperties` value schema constrains it:
        // `{$ref: Labels, required: [a]}` with string-valued `Labels` makes `a` a string, not an
        // unconstrained value. The field is required, so a value no type admits empties the object.
        for field in fields.values_mut() {
            if !field.undeclared {
                continue;
            }
            let other = if carries_key(&left.fields, &field.name.wire) {
                if carries_key(&right.fields, &field.name.wire) {
                    continue;
                }
                &right.additional
            } else {
                &left.additional
            };
            let field_hint = format!("{hint}{}", field.name.wire);
            match self.narrow_undeclared(field.ty, other, &field_hint) {
                Ok(ty) => field.ty = ty,
                Err(NoMeet::Empty) => return Err(NoMeet::Empty),
                Err(NoMeet::Unrepresentable) => unrepresentable = true,
            }
        }
        if unrepresentable {
            return Err(NoMeet::Unrepresentable);
        }
        // Two additional-value types that do not meet leave the object inhabited (one with no
        // additional key satisfies both), so that failure is never an empty object.
        let additional = self
            .merge_additional(
                &left.additional,
                &right.additional,
                &format!("{hint}Additional"),
            )
            .ok_or(NoMeet::Unrepresentable)?;
        let meet = self.insert_type(
            hint,
            TypeKind::Struct(Struct {
                fields: fields.into_values().collect(),
                additional,
            }),
            Docs::default(),
            None,
        );
        if let Some(location) = location {
            self.meet_locations.insert(meet.id, location);
        }
        Ok(meet)
    }

    /// Whether two lowered value types would emit the *same* Rust type as a shared map value, so
    /// multiple `patternProperties`/`additionalProperties` values can collapse into one typed
    /// overflow map. A bounded structural equivalence:
    ///
    /// * different `nullable` — never the same;
    /// * equal `TypeId` (with equal `nullable`) — a shared `$ref` or the single-entry case;
    /// * otherwise, for distinct ids with equal `nullable`, compare the def kinds structurally but
    ///   only for *leaf* shapes that have no per-inline-schema identity: `Primitive` (same `Prim`),
    ///   `Bytes`, `Null`, `Never`, `Any`, and `Array` (recursing on the element). Composite kinds
    ///   (`Struct`/`Enum`/`Tuple`/`Union`) generate a distinct named Rust type per inline schema,
    ///   so two such inline shapes are treated as heterogeneous (→ `E005`) rather than silently
    ///   merged, and so is a pair either side of which is an unlowered reservation or has no def.
    ///
    /// `boxed` is deliberately ignored: it is a use-site indirection modifier, not part of the map
    /// value's emitted type (the map value is never boxed).
    ///
    /// The `Array` recursion is *not* structurally bounded — array element types can form `$ref`
    /// cycles (`A = [B]`, `B = [A]`) — so a visited-pair guard makes it terminate: an `(a.id, b.id)`
    /// pair already on the comparison stack is a co-recursive back-edge and compares equal (the two
    /// types are being compared identically along the cycle, so they are structurally equal there).
    pub(super) fn same_map_value_type(&self, a: Ty, b: Ty) -> bool {
        self.same_map_value_type_guarded(a, b, &mut Vec::new())
    }

    fn same_map_value_type_guarded(
        &self,
        a: Ty,
        b: Ty,
        visiting: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        if a.nullable != b.nullable {
            return false;
        }
        if a.id == b.id {
            return true;
        }
        let pair = (a.id, b.id);
        if visiting.contains(&pair) {
            // Co-recursive back-edge: the same pair is already being compared further up the stack.
            // Along a cycle the two types are compared identically, so they are structurally equal.
            return true;
        }
        visiting.push(pair);
        let result = match (self.graph.get(a.id), self.graph.get(b.id)) {
            (Some(a_def), Some(b_def)) => match (&a_def.kind, &b_def.kind) {
                (TypeKind::Primitive(x), TypeKind::Primitive(y)) => x == y,
                (TypeKind::Bytes, TypeKind::Bytes) => true,
                (TypeKind::Null, TypeKind::Null) | (TypeKind::Never, TypeKind::Never) => true,
                (TypeKind::Any, TypeKind::Any) => true,
                (TypeKind::Array(x), TypeKind::Array(y)) => {
                    self.same_map_value_type_guarded(**x, **y, visiting)
                }
                // An unlowered body cannot be proven the same value type as anything else, so the
                // pair is heterogeneous and the map is rejected with `E005` rather than merged on
                // a guess. The same reservation on both sides already answered `true` by id.
                (TypeKind::Reserved, _) | (_, TypeKind::Reserved) => false,
                _ => false,
            },
            _ => false,
        };
        visiting.pop();
        result
    }
}

/// Why two lowered types have no typed intersection. The two answers call for different handling,
/// so an intersection never reports one where it may be the other.
///
/// Only [`NoMeet::Empty`] may be typed uninhabited ([`TypeKind::Never`]) or collapsed to the exact
/// JSON `null`: those stand in for the intersection only when no value satisfies both sides.
/// [`NoMeet::Unrepresentable`] is a set of values the generated client would silently refuse, so
/// every caller reports it (`E013`) instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoMeet {
    /// No JSON value satisfies both sides: their value categories are disjoint (a string and an
    /// integer), or their scalar `enum` sets share no value.
    Empty,
    /// The sides may share values, but no single Rust type represents the ones they share — `uuid`
    /// and `contentEncoding: base64` are both annotations on a string, so every string satisfies
    /// both — or nothing can be known yet, because one side is a reservation.
    Unrepresentable,
}

/// The JSON category every instance of a non-null lowered kind falls in, for a kind confined to
/// one. Unlike [`LowerCtx::json_category`], which picks a union's dispatch and so leaves raw bytes
/// uncategorised, this answers what an instance of a *schema* can be: binary content in a schema is
/// a (base64) JSON string.
pub(super) fn value_category(kind: &TypeKind) -> Option<JsonCategory> {
    match kind {
        TypeKind::Primitive(Prim::Bool) => Some(JsonCategory::Boolean),
        TypeKind::Primitive(Prim::I32 | Prim::I64 | Prim::F64) => Some(JsonCategory::Number),
        TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date)
        | TypeKind::Bytes => Some(JsonCategory::String),
        TypeKind::Enum(enumeration) => Some(match enumeration.repr {
            ScalarRepr::String => JsonCategory::String,
            ScalarRepr::Int => JsonCategory::Number,
            ScalarRepr::Bool => JsonCategory::Boolean,
        }),
        TypeKind::Array(_) | TypeKind::Tuple(_) => Some(JsonCategory::Array),
        TypeKind::Struct(_) => Some(JsonCategory::Object),
        // `null` is intersected before any non-null kind is compared, `Never` has no instance, a
        // union and `Any` span several categories, and a reservation's body is not known yet.
        TypeKind::Null
        | TypeKind::Never
        | TypeKind::Union(_)
        | TypeKind::Any
        | TypeKind::Reserved => None,
    }
}

/// Why two non-null kinds that no intersection rule meets do not meet: empty when one side is
/// uninhabited or the two sit in disjoint JSON categories, and otherwise unrepresentable — two
/// strings of different formats, tuples of different lengths — because nothing here proves that
/// no value satisfies both.
pub(super) fn no_meet(left: &TypeKind, right: &TypeKind) -> NoMeet {
    if matches!(left, TypeKind::Never) || matches!(right, TypeKind::Never) {
        return NoMeet::Empty;
    }
    match (value_category(left), value_category(right)) {
        (Some(left), Some(right)) if left != right => NoMeet::Empty,
        _ => NoMeet::Unrepresentable,
    }
}

pub(super) fn same_ty(left: Ty, right: Ty) -> bool {
    left.id == right.id && left.nullable == right.nullable && left.boxed == right.boxed
}

fn intersect_primitives(left: Prim, right: Prim) -> Option<Prim> {
    use Prim::{Bool, Date, DateTime, String, Uuid, F64, I32, I64};
    Some(match (left, right) {
        (Bool, Bool) => Bool,
        (I32, I32 | I64 | F64) | (I64 | F64, I32) => I32,
        (I64, I64 | F64) | (F64, I64) => I64,
        (F64, F64) => F64,
        (String, String) => String,
        (String, formatted @ (Uuid | DateTime | Date))
        | (formatted @ (Uuid | DateTime | Date), String) => formatted,
        (Uuid, Uuid) => Uuid,
        (DateTime, DateTime) => DateTime,
        (Date, Date) => Date,
        _ => return None,
    })
}

fn enum_matches_primitive(repr: ScalarRepr, primitive: Prim) -> bool {
    match repr {
        ScalarRepr::String => matches!(
            primitive,
            Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date
        ),
        ScalarRepr::Int => matches!(primitive, Prim::I32 | Prim::I64 | Prim::F64),
        ScalarRepr::Bool => primitive == Prim::Bool,
    }
}

/// Whether one side of an intersection carries a field for the wire name `wire`, declared or only
/// required. A [`Field::undeclared`] field the merged object carries is an undeclared key of each
/// side that does not, so that side's `additionalProperties` value schema narrows it
/// ([`LowerCtx::narrow_undeclared`]).
pub(super) fn carries_key(fields: &[Field], wire: &str) -> bool {
    fields.iter().any(|carried| carried.name.wire == wire)
}

/// Settle a property two sides of an intersection both carry when exactly one side declares it,
/// and report whether it did. A field marked [`Field::undeclared`] is no declaration: it stands
/// for a key its object requires and types it by that object's `additionalProperties` schema,
/// which by the rule every merge here applies (a merged object's `additionalProperties` constrains
/// only the keys no side declares) does not reach a property the other side declares. So the
/// declared field is kept whole — its type, default, flags and `xml` hints — and the requirement
/// is added to it. Two declarations, or two undeclared fields, are left to the caller to intersect.
pub(super) fn take_declaration(existing: &mut Field, other: &Field) -> bool {
    if existing.undeclared == other.undeclared {
        return false;
    }
    let required = existing.required || other.required;
    if existing.undeclared {
        *existing = other.clone();
    }
    existing.required = required;
    if required {
        if let Some(default) = &mut existing.default {
            default.applied = None;
        }
    }
    true
}

/// The `default` the merged field of an intersection keeps for a repeated property, given every
/// `default` its sides write for it (#432). `allOf` is commutative, and so is this merge: the
/// choice is made once over all of them — an applicable default before one that cannot be
/// applied, then the lesser rustdoc note, then the lesser `default` location — so it is the same
/// in every order of the sides, and every other different value is reported (`W005`) at the
/// `default` that wrote it, since the field cannot carry it. Nothing is reported before every
/// side is read: a pairwise fold would report a value one pair ranked lower even where a later
/// side's equal value is the one kept (#577). Two defaults of one value (`3` and `3.0` alike) are
/// one default for choosing what to keep, but not for accounting: the kept one carries the
/// others' pointers in [`FieldDefault::also_written`], and whichever drop reports it reports
/// every pointer that wrote the value (#543). The kept default is then decided against the merged
/// field as every other is: a requirement drops its application at the caller, and
/// [`retype_field_defaults`] re-types it against the narrowed type, which for an empty meet leaves
/// it unapplied and reports it as `W005` (#453).
///
/// [`retype_field_defaults`]: super::defaults::retype_field_defaults
pub(super) fn merge_field_default(
    written: Vec<FieldDefault>,
    property: &str,
    diags: &mut Diagnostics,
) -> Option<FieldDefault> {
    let rank = |default: &FieldDefault| {
        (
            default.applied.is_none(),
            default.doc_note.clone(),
            provenance_rank(&default.provenance),
        )
    };
    let mut written = written;
    written.sort_by_cached_key(rank);
    let mut written = written.into_iter();
    let mut kept = written.next()?;
    let mut dropped: Vec<FieldDefault> = Vec::new();
    for other in written {
        let same_value = match (&kept.applied, &other.applied) {
            (Some(left), Some(right)) => reclassify_default(left) == reclassify_default(right),
            _ => kept.doc_note == other.doc_note,
        };
        if same_value {
            // The equal value another side wrote is merged, not forgotten: its pointers ride on
            // the kept default, so a later drop reports each of them (#543).
            kept.also_written.push(other.provenance);
            kept.also_written.extend(other.also_written);
        } else {
            dropped.push(other);
        }
    }
    kept.also_written.sort_by_key(provenance_rank);
    // Every pointer that wrote a dropped value is reported, not only the one the merge reached
    // first, so which `default`s are reported does not depend on the sides' order (#543).
    for loser in &dropped {
        for at in std::iter::once(&loser.provenance).chain(&loser.also_written) {
            Diagnostic::warning(Code::SchemaDefaultNotApplied, at.clone())
                .message(format!(
                    "schema `default` of property `{property}` differs from the `default` another \
                     intersected schema declares for it at `{}`, which the merged field keeps; \
                     this one is neither applied nor documented there",
                    kept.provenance.pointer
                ))
                .remedy(
                    "declare one default for the property, or the same default on every \
                     intersected schema that declares it",
                )
                .emit(diags);
        }
    }
    Some(kept)
}

/// The total order [`merge_field_default`] breaks ties by and keeps [`FieldDefault::also_written`]
/// in: the `default`'s pointer, then its source span.
fn provenance_rank(provenance: &Provenance) -> (String, Option<(u32, usize, usize)>) {
    (
        provenance.pointer.to_string(),
        provenance
            .span
            .map(|span| (span.file.0, span.start.offset, span.end.offset)),
    )
}
