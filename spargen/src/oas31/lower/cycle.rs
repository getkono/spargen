//! Detecting a `$ref` or union member that closes a cycle back into the schema being lowered, and
//! ordering the bodies whose `allOf` member found its target still being lowered.

use std::collections::HashSet;

use crate::diag::Provenance;
use crate::ir::{Ty, TypeId};
use crate::oas31::{Document, SchemaOr};
use crate::source::SpannedValue;

use super::{LowerCtx, Reservation};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Whether `reference`, written at `at`, closes a reference cycle back through a schema whose
    /// lowering encloses `at`.
    ///
    /// This is a property of the DOCUMENT, not of the lowering: it asks whether the target reaches,
    /// through `$ref`s, a schema that contains `at` along the keywords lowering descends into. That
    /// is the same answer however any map is ordered and whichever end of a cycle lowering entered
    /// first. The predicate it replaced — membership of an in-progress map — was a property of
    /// *when* lowering happened, so mutual recursion rejected or generated according to which entry
    /// the document happened to declare, or which operation happened to reach it, first. Re-ordering
    /// a YAML map is a no-op in OpenAPI.
    ///
    /// Every schema is named by its resolved `(file, pointer)`, for every spelling alike, which is
    /// what makes the answer spelling-independent. An earlier form walked the root document's
    /// `components.schemas` by name, so it could not see a sub-file or remote target at all, and it
    /// matched a sub-file component's name against the root's map — a sub-file `Item` sharing its
    /// name with a root `Item` in a cycle was reported as closing that cycle.
    pub(super) fn ref_closes_a_cycle(&self, reference: &str, at: &Provenance) -> bool {
        let site_file = self.resolver.written_in(at);
        let Some(start) = self
            .resolver
            .schema_reference_identity(reference, site_file)
        else {
            // Not a target this bundle knows; the lowering reports it in its own words.
            return false;
        };
        let mut seen: HashSet<(crate::diag::FileId, crate::diag::JsonPointer)> = HashSet::new();
        let mut stack = vec![start];
        while let Some((file, pointer)) = stack.pop() {
            if file == site_file && lowering_encloses(&pointer, &at.pointer) {
                return true;
            }
            if !seen.insert((file, pointer.clone())) {
                continue;
            }
            let Some(node) = self.resolver.node_at(file, &pointer) else {
                continue;
            };
            let mut references = Vec::new();
            collect_node_refs(node, &mut references);
            stack.extend(
                references.into_iter().filter_map(|reference| {
                    self.resolver.schema_reference_identity(reference, file)
                }),
            );
        }
        false
    }

    /// Whether a union member is a `$ref` that closes a reference cycle back through the component
    /// enclosing the union. Only a member that IS a reference counts: a member with recursive
    /// *fields* lowers fine, exactly as it does on the `allOf` path.
    ///
    /// Every spelling is asked. Asking only `#/components/schemas/…` left the explicit
    /// `./lib.yaml#/…` spelling to the reservation checks after lowering, so with siblings on one
    /// edge of a two-schema cycle it generated when lowering entered at one end and rejected when
    /// it entered at the other.
    pub(super) fn member_closes_a_cycle(
        &self,
        member: &SchemaOr,
        at: &crate::diag::Provenance,
    ) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        member
            .reference
            .as_deref()
            .is_some_and(|reference| self.ref_closes_a_cycle(reference, at))
    }

    /// Whether a union member is a `$ref` to the very schema the union at `at` is. Such a union
    /// resolves to itself, which is `E007` rather than a sibling-intersection question.
    ///
    /// Answered by resolved identity, for every spelling, and also by the reservation the schema at
    /// `at` occupies, which is how the `#/components/schemas/…` spelling was answered before the
    /// document half of the union guard was asked of every spelling.
    pub(super) fn member_is_this_union(
        &self,
        member: &SchemaOr,
        at: &crate::diag::Provenance,
    ) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        let Some(reference) = member.reference.as_deref() else {
            return false;
        };
        let site_file = self.resolver.written_in(at);
        if self
            .resolver
            .schema_reference_identity(reference, site_file)
            .is_some_and(|(file, pointer)| file == site_file && pointer == at.pointer)
        {
            return true;
        }
        let Some(own) = self.reservation_at(at) else {
            return false;
        };
        reference
            .strip_prefix("#/components/schemas/")
            .and_then(|name| self.in_progress.get(name))
            .is_some_and(|&(id, _)| id == own)
    }
}

impl LowerCtx<'_, '_> {
    /// Record that an `allOf` member of the body being lowered right now found `target`, the root
    /// id of a reservation still open, and is about to refuse it (`E013`). Unless that body is the
    /// target's own, the next pass opens it first ([`MemberOrder`]), so the member is reached fresh
    /// there whichever of the two `components.schemas` declares first.
    pub(super) fn record_open_member(&mut self, target: TypeId) {
        let Some(holder) = self.open_reservations.last() else {
            return;
        };
        let opened = self.open_reservations.iter().find(|open| {
            let memo = match &open.reservation {
                Reservation::Component(_) => &self.in_progress,
                Reservation::Remote(_) => &self.remote_in_progress,
                Reservation::Resolved(_) => &self.resolved_in_progress,
            };
            memo.get(open.reservation.key())
                .is_some_and(|&(id, _)| id == target)
        });
        let Some(target) = opened else {
            return;
        };
        if target.reservation == holder.reservation {
            return;
        }
        let order = MemberOrder {
            holder: holder.clone(),
            target: target.reservation.clone(),
        };
        self.member_orders.push(order);
    }

    /// Open `opening` from the top level of a pass, as the `$ref` that first reached it did.
    pub(super) fn open(&mut self, opening: &Opening) -> Option<Ty> {
        let root = &self.document.provenance;
        match (&opening.reservation, &opening.via) {
            (Reservation::Component(name), _) => self.ensure_component(name, None, root),
            (Reservation::Remote(reference), _) => self.ensure_remote(reference, root),
            (Reservation::Resolved(_), Some(via)) => {
                self.ensure_resolved(&via.reference, &via.at, &via.hint)
            }
            // `lower_into_reservation` is handed a `Via` for every bundle target it opens.
            (Reservation::Resolved(_), None) => None,
        }
    }
}

/// A reservation, and how the top level of a pass opens it ([`MemberOrder`]).
#[derive(Debug, Clone)]
pub(super) struct Opening {
    pub(super) reservation: Reservation,
    /// How a bundle target was reached: its key is its resolved `file#pointer`, which is no `$ref`
    /// to follow again. `None` for a root component and a remote target, each opened by its key.
    pub(super) via: Option<Via>,
}

/// The `$ref` that opened a bundle target, where it is written, and the hint it was given: what
/// [`LowerCtx::ensure_resolved`] opens it with again.
#[derive(Debug, Clone)]
pub(super) struct Via {
    pub(super) reference: String,
    pub(super) at: Provenance,
    pub(super) hint: String,
}

/// A body to open before `target` (#999). An `allOf` member of the body found `target`'s
/// reservation still open, so its fields were not known yet and the pass refused the member. The
/// target was open only because something lowered before the body reached it: opened first, the
/// body reaches its member fresh, and the member's own way back to the body is an ordinary boxed
/// back-edge.
#[derive(Debug, Clone)]
pub(super) struct MemberOrder {
    holder: Opening,
    target: Reservation,
}

impl MemberOrder {
    /// Whether `orders` lacks this ordering and taking it closes no loop: no chain of `orders`
    /// leads from its target back to its holder. Each body on such a loop is composed of the next,
    /// which no order lowers, so its refusal stands.
    pub(super) fn extends(&self, orders: &[MemberOrder]) -> bool {
        let known = orders.iter().any(|order| {
            order.holder.reservation == self.holder.reservation && order.target == self.target
        });
        !known && !orders_reach(orders, &self.target, &self.holder.reservation)
    }

    /// What a pass opens before it lowers the root components in document order: every holder
    /// `orders` names, each after the holders ordered before it, the root components among them in
    /// document order and the rest by key, so the order is a function of the document and
    /// `orders` alone.
    ///
    /// Every holder goes before every root component, not only before its target: the target
    /// was open because something lowered before the holder reached it, at the top level or
    /// inside any body that leads to it. Opened at the top level, a holder's own members are
    /// reached inside its own frame only, where the one open reservation they can find is its own.
    pub(super) fn opened_first<'o>(
        document: &Document,
        orders: &'o [MemberOrder],
    ) -> Vec<&'o Opening> {
        let mut holders: Vec<&Opening> = Vec::new();
        for order in orders {
            if !holders
                .iter()
                .any(|holder| holder.reservation == order.holder.reservation)
            {
                holders.push(&order.holder);
            }
        }
        holders.sort_by_cached_key(|holder| match &holder.reservation {
            Reservation::Component(name) => (
                0,
                document.components.schemas.get_index_of(name),
                String::new(),
            ),
            other => (1, None, other.key().to_owned()),
        });
        let mut placing = HashSet::new();
        let mut first = Vec::new();
        for holder in &holders {
            place_opening(holder, orders, &holders, &mut placing, &mut first);
        }
        first
    }
}

/// Append `opening` to `first` after every holder `orders` opens before it, those in the order
/// `holders` ranks them; `placing` holds what is placed already, or being placed.
fn place_opening<'o>(
    opening: &'o Opening,
    orders: &[MemberOrder],
    holders: &[&'o Opening],
    placing: &mut HashSet<&'o Reservation>,
    first: &mut Vec<&'o Opening>,
) {
    if !placing.insert(&opening.reservation) {
        return;
    }
    for holder in holders {
        let before = orders.iter().any(|order| {
            order.holder.reservation == holder.reservation && order.target == opening.reservation
        });
        if before {
            place_opening(holder, orders, holders, placing, first);
        }
    }
    first.push(opening);
}

/// Whether a chain of `orders`, each from its holder to its target, leads from `from` to `to`,
/// `from` itself included.
fn orders_reach(orders: &[MemberOrder], from: &Reservation, to: &Reservation) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(at) = stack.pop() {
        if at == to {
            return true;
        }
        if !seen.insert(at) {
            continue;
        }
        stack.extend(
            orders
                .iter()
                .filter(|order| &order.holder.reservation == at)
                .map(|order| &order.target),
        );
    }
    false
}

/// The keywords whose value maps member names to subschemas that lowering descends into.
const NAMED_MEMBER_KEYWORDS: [&str; 2] = ["properties", "patternProperties"];

/// The keywords whose value is an array of subschemas that lowering descends into.
const INDEXED_MEMBER_KEYWORDS: [&str; 4] = ["prefixItems", "allOf", "oneOf", "anyOf"];

/// The keywords whose value is one subschema that lowering descends into.
const SINGLE_SUBSCHEMA_KEYWORDS: [&str; 2] = ["additionalProperties", "items"];

/// Whether lowering the schema at `frame` descends into the schema at `site`, both pointers into
/// one file: `site` is `frame` itself, or lies below it along only the keywords
/// [`collect_node_refs`] walks.
///
/// Lexical containment alone is not enough. A whole-file target (`./lib.yaml`, pointer `""`)
/// contains every pointer in that file, but lowering it as a schema never enters its `components`,
/// so a `$ref` there is not inside that frame and cannot be handed its placeholder.
fn lowering_encloses(frame: &crate::diag::JsonPointer, site: &crate::diag::JsonPointer) -> bool {
    let Some(rest) = site.as_str().strip_prefix(frame.as_str()) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(rest) = rest.strip_prefix('/') else {
        // `/components/schemas/Ab` is not below `/components/schemas/A`.
        return false;
    };
    let mut tokens = rest.split('/');
    while let Some(keyword) = tokens.next() {
        if NAMED_MEMBER_KEYWORDS.contains(&keyword) || INDEXED_MEMBER_KEYWORDS.contains(&keyword) {
            // Followed by a member name or an index.
            if tokens.next().is_none() {
                return false;
            }
        } else if !SINGLE_SUBSCHEMA_KEYWORDS.contains(&keyword) {
            return false;
        }
    }
    true
}

/// Push every `$ref` string this raw schema subtree carries onto `out`, its own included.
///
/// Every spelling counts — a root component, a sub-file pointer, a whole file, a remote URL — since
/// the cycle predicate resolves each to its `(file, pointer)` identity before comparing anything.
///
/// The keywords walked here are exactly the ones `lower_schema_inner` descends into. `$defs` and
/// the validation-only applicators — `not`, `if`/`then`/`else`, `contains`, `propertyNames`,
/// `unevaluated*`, `dependentSchemas` — are deliberately NOT walked: lowering never enters them, so
/// a `$ref` reachable only that way can never put a component mid-flight and can never yield the
/// placeholder this predicate exists to detect. Nor is `contentSchema`: lowering a schema clears
/// it, and the one `contentSchema` that is lowered — the `data` property's in a 3.2
/// `text/event-stream` envelope — is lowered as the stream's item type on its own, never inside
/// the envelope's frame. Counting them made an unreferenced `$defs` entry —
/// zero emitted bytes, not one instance added or removed — flip a document into a hard rejection
/// whose message asserted a dependence that does not exist. [`lowering_encloses`] reads the same
/// three keyword sets, so the two halves of the predicate agree on what "inside" means.
fn collect_node_refs<'v>(node: &'v SpannedValue, out: &mut Vec<&'v str>) {
    // A boolean schema, or anything that is not a schema object, carries no reference.
    let Some(object) = node.as_object() else {
        return;
    };
    if let Some(reference) = object.get("$ref").and_then(SpannedValue::as_str) {
        out.push(reference);
    }
    for keyword in NAMED_MEMBER_KEYWORDS {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_object) {
            for (_, child) in members.iter() {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in INDEXED_MEMBER_KEYWORDS {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_array) {
            for child in members {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in SINGLE_SUBSCHEMA_KEYWORDS {
        if let Some(child) = object.get(keyword) {
            collect_node_refs(child, out);
        }
    }
}
