//! Removing the intermediate types a meet or a discarded union lowering left in the graph.

use std::collections::HashSet;

use crate::ir::{AdditionalProps, Ty, TypeDef, TypeGraph, TypeId, TypeKind};
use crate::oas31::Schema;

use super::combine::Contribution;
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// [Elide](TypeGraph::elide) every type in `lowered` that nothing can name any more. `lowered`
    /// holds the ids a union was lowered into, with its merge held back ([`Self::unmerged_union`]),
    /// before [`Self::meet_union_with_all_of`] or a `$ref`'s target met it. The pre-meet union
    /// itself and the branch types the meet replaced were emitted as public types nothing referred
    /// to (#561). [`Self::meet_ref_union_sibling`] also passes the ids the keywords beside the
    /// union were lowered into, an input to the same meet (#571).
    ///
    /// Unlike the meets' own inserts ([`Self::elide_meet_intermediates`]), these come from lowering
    /// a schema, so a memo may hold one: a component, remote or bundle target a branch `$ref`s and
    /// first lowered here, which every later use of it is handed, or a type in a memoised `allOf`
    /// contribution. So a type stays when the graph outside `lowered` reaches it, which takes in
    /// the re-emitted meet, or a memo holds it, or one of those reaches it. Eliding keeps each id
    /// and name, so no type the output carries is renamed or reordered.
    pub(super) fn elide_unused_union_lowering(&mut self, lowered: std::ops::Range<u32>) {
        if lowered.is_empty() {
            return;
        }
        let inside = |id: &TypeId| lowered.contains(&id.0);
        let mut roots: Vec<TypeId> = self
            .graph
            .emitted()
            .filter(|(id, _)| !inside(id))
            .flat_map(|(_, def)| kind_edges(&def.kind))
            .filter(inside)
            .collect();
        roots.extend(
            self.components
                .values()
                .chain(self.in_progress.values())
                .chain(self.remote_components.values())
                .chain(self.remote_in_progress.values())
                .chain(self.resolved_components.values())
                .chain(self.resolved_in_progress.values())
                .map(|&(id, _)| id)
                .filter(inside),
        );
        for (_, contribution) in self.resolved_contributions.values().flatten() {
            match contribution {
                Contribution::Object { fields, .. } => {
                    roots.extend(fields.iter().map(|field| field.ty.id).filter(inside));
                }
                Contribution::Scalar(ty) => roots.extend(Some(ty.id).filter(inside)),
                Contribution::Refiner { scoped, .. } => roots.extend(
                    [scoped.object, scoped.array]
                        .into_iter()
                        .flatten()
                        .map(|ty| ty.id)
                        .filter(inside),
                ),
            }
        }
        let reached = reachable_types(&self.graph, &roots);
        for id in lowered.map(TypeId) {
            if !reached.contains(&id) && self.graph.get(id).is_some() {
                self.graph.elide(id);
            }
        }
    }

    /// The id the next graph insert takes: every type inserted from here on has an id at or above
    /// it. [`Self::discard_meet_intermediates`] takes it back.
    pub(super) fn graph_mark(&self) -> u32 {
        self.graph.last_id().map_or(0, |id| id.0 + 1)
    }

    /// [`TypeGraph::pop_last`], also forgetting the location [`Self::meet_locations`] recorded for
    /// the popped id. The next insert reuses that id for a type of its own, which would otherwise
    /// inherit the popped meet's location. Every pop in lowering goes through here.
    pub(super) fn pop_last_type(&mut self) -> Option<(TypeId, TypeDef)> {
        let popped = self.graph.pop_last();
        if let Some((id, _)) = &popped {
            self.meet_locations.remove(id);
        }
        popped
    }

    /// Discard every type inserted since `mark` that `kind` does not refer to, directly or
    /// transitively. The caller has just met two or more types inserted before `mark` and is about
    /// to re-emit the meet's result as a new definition of `kind`, so the meets' own inserts are
    /// unused unless that definition reaches them. Each would otherwise be emitted as a public type
    /// nothing refers to: the open or locked copy [`Self::reopened_set`] makes of a `$ref`'d set
    /// (#401), and every intermediate a later meet superseded.
    ///
    /// When `kind` reaches none of them, all are removed (#401). Otherwise only the most recent
    /// could be, since ids are dense, so the unused ones are elided instead, as
    /// [`Self::elide_meet_intermediates`] does.
    pub(super) fn discard_meet_intermediates(&mut self, mark: u32, kind: &TypeKind) {
        let reached = reachable_types(&self.graph, &kind_edges(kind));
        if reached.iter().any(|id| id.0 >= mark) {
            self.elide_unreached(mark, &reached);
            return;
        }
        while self.graph.last_id().is_some_and(|id| id.0 >= mark) {
            self.pop_last_type();
        }
    }

    /// Re-emit `kind`, the result of the meets since `mark`, as `schema`'s own type under `hint`,
    /// first discarding the meets' inserts `kind` does not reach
    /// ([`Self::discard_meet_intermediates`]). The returned type carries no nullability or
    /// indirection of the meet's: each caller settles those itself.
    pub(super) fn reemit_meet(
        &mut self,
        schema: &Schema,
        hint: &str,
        mark: u32,
        kind: TypeKind,
    ) -> Ty {
        self.discard_meet_intermediates(mark, &kind);
        self.insert_schema_type(schema, hint, kind)
    }

    /// [Elide](TypeGraph::elide) every type inserted since `mark` that `kind` does not refer to,
    /// directly or transitively. The caller has just met the properties its members repeat, each
    /// meet replacing the field's type, and is about to emit `kind`, the struct that refers to the
    /// last meet of each property and not to the ones a later member superseded: the open copy
    /// [`Self::reopened_set`] makes of a `$ref`'d set, or the struct an earlier pair of members met
    /// a repeated object property in (#428). Those are interleaved with the inserts the struct
    /// uses, so they cannot be popped; eliding keeps each one's id and name, so no type the output
    /// carries is renamed or reordered.
    ///
    /// Sound because intersecting only reads the graph and inserts into it: it lowers no schema and
    /// fills no memo, so nothing outside the inserts since `mark` refers to them, and an in-place
    /// change of an earlier set's openness ([`Self::reopen_in_place`]) is kept.
    pub(super) fn elide_meet_intermediates(&mut self, mark: u32, kind: &TypeKind) {
        let reached = reachable_types(&self.graph, &kind_edges(kind));
        self.elide_unreached(mark, &reached);
    }

    /// Elide every type inserted since `mark` that is not in `reached`.
    fn elide_unreached(&mut self, mark: u32, reached: &HashSet<TypeId>) {
        let Some(last) = self.graph.last_id() else {
            return;
        };
        for id in (mark..=last.0).map(TypeId) {
            if !reached.contains(&id) {
                self.graph.elide(id);
            }
        }
    }
}

/// The set of type ids transitively reachable from `roots` through the type graph's structural
/// edges (struct fields and typed `additionalProperties`, array/tuple elements, union variants).
/// A visited set makes recursive (`$ref`-cycle) types terminate.
pub(super) fn reachable_types(graph: &TypeGraph, roots: &[TypeId]) -> HashSet<TypeId> {
    let mut visited = HashSet::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(def) = graph.get(id) else {
            continue;
        };
        stack.extend(kind_edges(&def.kind));
    }
    visited
}

/// The type ids a definition of `kind` refers to directly: its struct fields and typed
/// `additionalProperties`, array/tuple elements, and union variants.
pub(super) fn kind_edges(kind: &TypeKind) -> Vec<TypeId> {
    match kind {
        TypeKind::Struct(object) => {
            let mut edges: Vec<TypeId> = object.fields.iter().map(|field| field.ty.id).collect();
            if let AdditionalProps::Typed(ty) = &object.additional {
                edges.push(ty.id);
            }
            edges
        }
        TypeKind::Array(ty) => vec![ty.id],
        TypeKind::Tuple(items) => items.iter().map(|ty| ty.id).collect(),
        TypeKind::Union(union) => union.variants.iter().map(|variant| variant.ty.id).collect(),
        // A reservation has no structural edges yet: its body is still being lowered.
        TypeKind::Reserved
        | TypeKind::Primitive(_)
        | TypeKind::Enum(_)
        | TypeKind::Bytes
        | TypeKind::Null
        | TypeKind::Never
        | TypeKind::Any => Vec::new(),
    }
}
