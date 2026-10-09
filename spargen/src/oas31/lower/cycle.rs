//! Detecting a `$ref` or union member that closes a cycle back into the schema being lowered.

use std::collections::HashSet;

use crate::diag::Provenance;
use crate::oas31::SchemaOr;
use crate::source::SpannedValue;

use super::LowerCtx;

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
        let site_file = at
            .span
            .map_or_else(|| self.resolver.root_id(), |span| span.file);
        let Some(start) = self.schema_ref_identity(reference, site_file) else {
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
                references
                    .into_iter()
                    .filter_map(|reference| self.schema_ref_identity(reference, file)),
            );
        }
        false
    }

    /// The `(file, pointer)` a schema `$ref` written in `from` lowers to, with the lowering's own
    /// precedence: `#/components/schemas/<name>` is the ROOT document's component whenever the root
    /// declares `name`, from whichever file it is written in (see [`Self::ensure_component`]), and
    /// every other reference resolves against the file it is written in.
    fn schema_ref_identity(
        &self,
        reference: &str,
        from: crate::diag::FileId,
    ) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            if self.document.components.schemas.contains_key(name) {
                return Some((
                    self.resolver.root_id(),
                    crate::diag::JsonPointer::from(format!("/components/schemas/{name}")),
                ));
            }
        }
        self.resolver.reference_identity_from(reference, from)
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
        let site_file = at
            .span
            .map_or_else(|| self.resolver.root_id(), |span| span.file);
        if self
            .schema_ref_identity(reference, site_file)
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
        match keyword {
            // Each of these is followed by a member name or an index.
            "properties" | "patternProperties" | "prefixItems" | "allOf" | "oneOf" | "anyOf" => {
                if tokens.next().is_none() {
                    return false;
                }
            }
            "additionalProperties" | "items" | "contentSchema" => {}
            _ => return false,
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
/// placeholder this predicate exists to detect. Counting them made an unreferenced `$defs` entry —
/// zero emitted bytes, not one instance added or removed — flip a document into a hard rejection
/// whose message asserted a dependence that does not exist. [`lowering_encloses`] accepts exactly
/// the same keywords, so the two halves of the predicate agree on what "inside" means.
fn collect_node_refs<'v>(node: &'v SpannedValue, out: &mut Vec<&'v str>) {
    // A boolean schema, or anything that is not a schema object, carries no reference.
    let Some(object) = node.as_object() else {
        return;
    };
    if let Some(reference) = object.get("$ref").and_then(SpannedValue::as_str) {
        out.push(reference);
    }
    for keyword in ["properties", "patternProperties"] {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_object) {
            for (_, child) in members.iter() {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in ["prefixItems", "allOf", "oneOf", "anyOf"] {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_array) {
            for child in members {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in ["additionalProperties", "items", "contentSchema"] {
        if let Some(child) = object.get(keyword) {
            collect_node_refs(child, out);
        }
    }
}
