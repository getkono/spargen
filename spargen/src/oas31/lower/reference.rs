//! Following a `$ref` to a non-schema component (parameter, request body, response, header,
//! media type, path item) and the rejections an unfollowable one gets.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::oas31::media::media_essence;
use crate::oas31::resolve::reject_undeclared_component;
use crate::oas31::{
    ParameterObject, PathItem, RefOr, Reference, RequestBodyObject, Resolver, ResponseObject,
};
use crate::source::SpannedValue;

use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Resolve a Header Object that may be a `$ref` into `#/components/headers/`.
    pub(super) fn resolve_header(
        &mut self,
        header: &RefOr<crate::oas31::HeaderObject>,
    ) -> Option<crate::oas31::HeaderObject> {
        self.follow_reference_chain(
            header.clone(),
            &ReferenceChain {
                kind: "header",
                cycle_kind: "header",
                prefix: "#/components/headers/",
                split: std::convert::identity,
                declared: |components, name| components.headers.get(name).cloned(),
                follow: |ctx, reference| {
                    ctx.follow_bundle_reference(
                        reference,
                        "header",
                        crate::oas31::deserialize::parse_header_object,
                    )
                },
            },
        )
    }

    pub(super) fn resolve_parameter(
        &mut self,
        parameter: &RefOr<ParameterObject>,
    ) -> Option<ParameterObject> {
        self.follow_reference_chain(
            parameter.clone(),
            &ReferenceChain {
                kind: "parameter",
                cycle_kind: "parameter",
                prefix: "#/components/parameters/",
                split: std::convert::identity,
                declared: |components, name| components.parameters.get(name).cloned(),
                follow: |ctx, reference| {
                    ctx.follow_bundle_reference(
                        reference,
                        "parameter",
                        crate::oas31::deserialize::parse_parameter,
                    )
                },
            },
        )
    }

    pub(super) fn resolve_request_body(
        &mut self,
        body: &RefOr<RequestBodyObject>,
    ) -> Option<RequestBodyObject> {
        self.follow_reference_chain(
            body.clone(),
            &ReferenceChain {
                kind: "request body",
                cycle_kind: "request body",
                prefix: "#/components/requestBodies/",
                split: std::convert::identity,
                declared: |components, name| components.request_bodies.get(name).cloned(),
                follow: |ctx, reference| {
                    ctx.follow_bundle_reference(
                        reference,
                        "request body",
                        crate::oas31::deserialize::parse_request_body,
                    )
                },
            },
        )
    }

    pub(super) fn resolve_response(
        &mut self,
        response: &RefOr<ResponseObject>,
    ) -> Option<ResponseObject> {
        self.follow_reference_chain(
            response.clone(),
            &ReferenceChain {
                kind: "response",
                cycle_kind: "response",
                prefix: "#/components/responses/",
                split: std::convert::identity,
                declared: |components, name| components.responses.get(name).cloned(),
                follow: |ctx, reference| {
                    ctx.follow_bundle_reference(
                        reference,
                        "response",
                        crate::oas31::deserialize::parse_response,
                    )
                },
            },
        )
    }

    /// Follow a Reference Object chain from `start` to the object it ends at, hop by hop.
    ///
    /// Each hop documents its reference site's `summary`/`description` (`W011`), then refuses a
    /// target it has already followed (`E004`, keyed on the `(file, pointer)` each hop resolves
    /// to, so `#/components/<kind>/A` written in two files is two hops), then reads the root's own
    /// `#/components/<kind>/<name>` declaration for a reference written in the root. Anything else
    /// — a whole file, a pointer into one, or a sub-file's own components — resolves through the
    /// input bundle from the file the reference is written in, and may itself be a Reference.
    fn follow_reference_chain<S, T>(
        &mut self,
        start: S,
        chain: &ReferenceChain<S, T>,
    ) -> Option<T> {
        let mut current = start;
        let mut seen = HashSet::new();
        loop {
            let reference = match (chain.split)(current) {
                RefOr::Item(object) => return Some(object),
                RefOr::Ref(reference) => reference,
            };
            self.note_reference_docs(&reference);
            if !seen.insert(self.hop_identity(&reference)) {
                return self.reject_alias_cycle(&reference.provenance, chain.cycle_kind);
            }
            current = match self.root_component_name(&reference, chain.prefix) {
                Some(name) => match (chain.declared)(&self.document.components, name) {
                    Some(target) => target,
                    None => {
                        return self.reject_component_alias(
                            &reference.provenance,
                            chain.kind,
                            &reference.reference,
                        );
                    }
                },
                None => (chain.follow)(self, &reference)?,
            };
        }
    }

    /// One hop of a Parameter, Request Body, Response or Header chain through the input bundle:
    /// the target as written, which is either the object or the next Reference to follow. A miss
    /// is reported here, in the words of the way it failed.
    fn follow_bundle_reference<T>(
        &mut self,
        reference: &crate::oas31::Reference,
        kind: &str,
        parse: fn(&SpannedValue, &crate::diag::JsonPointer, &mut Diagnostics) -> Option<T>,
    ) -> Option<RefOr<T>> {
        let from = self.resolver.written_in(&reference.provenance);
        match self
            .resolver
            .resolve_component_or_ref(&reference.reference, from, parse, self.diags)
        {
            Ok(target) => Some(target),
            Err(miss) => self.reject_unfollowable_reference(
                &reference.provenance,
                kind,
                &reference.reference,
                miss,
            ),
        }
    }

    /// What a reference hop names, for a chain's cycle check: the `(file, pointer)` it resolves to
    /// wherever the bundle can place it, so one relative spelling written in two files is two
    /// targets and two spellings of one target are one; the reference as written otherwise.
    fn hop_identity(
        &self,
        reference: &crate::oas31::Reference,
    ) -> Result<(crate::diag::FileId, crate::diag::JsonPointer), String> {
        self.resolver
            .reference_identity(&reference.reference, &reference.provenance)
            .ok_or_else(|| reference.reference.clone())
    }

    /// The `<name>` of a `#/components/<kind>/<name>` reference (`prefix` is
    /// `#/components/<kind>/`) that addresses the **root** document's component map — one written
    /// in the root document, or with no span to say otherwise.
    ///
    /// A JSON Pointer fragment addresses the document it appears in, so the same spelling written
    /// inside a referenced file names that file's components (#397). Reading the root's map for it
    /// rejected the reference when the root declared no such name and silently substituted the
    /// root's declaration when it did; `None` sends it to the bundle, which resolves it from the
    /// file it is written in.
    fn root_component_name<'r>(
        &self,
        reference: &'r crate::oas31::Reference,
        prefix: &str,
    ) -> Option<&'r str> {
        if self.resolver.written_in(&reference.provenance) != self.resolver.root_id() {
            return None;
        }
        reference.reference.strip_prefix(prefix)
    }

    /// Acknowledge a Reference Object `summary`/`description`.
    ///
    /// These document the *reference site*, not the target. Spargen emits one shared item per
    /// component, so a per-site documentation override has nowhere to land without making two use
    /// sites of the same component disagree. Reported rather than dropped.
    fn note_reference_docs(&mut self, reference: &crate::oas31::Reference) {
        if reference.summary.is_none() && reference.description.is_none() {
            return;
        }
        // W011 case: reference-docs
        Diagnostic::warning(Code::DeclarationHasNoEffect, reference.provenance.clone())
            .message(format!(
                "the `summary`/`description` on the reference to `{}` documents this use site, \
                 but the generated item is shared across every use, so the override is not applied",
                reference.reference
            ))
            .remedy("document the referenced component itself")
            .emit(self.diags);
    }

    /// A reference into `#/components/<kind>/` naming an entry the document does not declare.
    pub(super) fn reject_component_alias<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
        reference: &str,
    ) -> Option<T> {
        reject_undeclared_component(self.diags, provenance, kind, reference);
        None
    }

    /// A chain of `{kind}` reference hops that returns to a reference it already followed.
    fn reject_alias_cycle<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
    ) -> Option<T> {
        // E004 case: cycle
        Diagnostic::error(Code::UnresolvedRef, provenance.clone())
            .message(format!("{kind} reference cycle cannot be resolved"))
            .emit(self.diags);
        None
    }

    /// A schema `$ref` resolved outside the components, whose chain of bare aliases returns to a
    /// target it already passed through: the alias chain `ensure_resolved` follows, and the one an
    /// `allOf` member's expansion walks, report it alike.
    pub(super) fn reject_schema_alias_cycle<T>(
        &mut self,
        provenance: crate::diag::Provenance,
        reference: &str,
    ) -> Option<T> {
        // E004 case: cycle
        Diagnostic::error(Code::UnresolvedRef, provenance)
            .message(format!(
                "schema reference `{reference}` forms an alias cycle"
            ))
            .remedy(
                "give one component in the cycle a schema body, or break the cycle at one of its \
                 references",
            )
            .emit(self.diags);
        None
    }

    /// A reference outside `#/components/<kind>/` that the input bundle could not follow, reported
    /// in the words of the way it failed: the resolver separates a reference it cannot place from
    /// a pointer with nothing at it, and a target that exists but does not parse has already been
    /// rejected by its parser, at the target, so it gets no second diagnostic here — the same
    /// disposition a malformed Path Item or schema target gets.
    fn reject_unfollowable_reference<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
        reference: &str,
        miss: crate::oas31::resolve::ComponentMiss,
    ) -> Option<T> {
        use crate::oas31::resolve::ComponentMiss;
        match miss {
            // The bundle cannot tell a file it does not hold from a fragment form it declines to
            // walk, so the message says both, in the one wording `E004` reserves for that.
            ComponentMiss::Unclassifiable => {
                // E004 case: unsupported-or-unresolved
                Diagnostic::error(Code::UnresolvedRef, provenance.clone())
                    .message(format!(
                        "unsupported or unresolved {kind} reference `{reference}`"
                    ))
                    .emit(self.diags);
            }
            ComponentMiss::AbsentTarget => {
                // E004 case: absent-target
                let mut diagnostic = Diagnostic::error(Code::UnresolvedRef, provenance.clone())
                    .message(format!(
                        "{kind} reference target `{reference}` was not found in the input bundle"
                    ));
                // A bare fragment written in a referenced file addresses that file (#397); when
                // the root declares what it names, that is almost certainly what was meant.
                if let Some(remedy) = self.root_declares_the_fragment(provenance, reference) {
                    diagnostic = diagnostic.remedy(remedy);
                }
                diagnostic.emit(self.diags);
            }
            ComponentMiss::Unparsable => {}
        }
        None
    }

    /// The remedy for a bare `#…` fragment written in a referenced file whose own document holds
    /// nothing at it while the root document does: the fragment addresses the file it is written
    /// in, so the root's declaration is reached only by naming the root document.
    fn root_declares_the_fragment(
        &self,
        provenance: &crate::diag::Provenance,
        reference: &str,
    ) -> Option<String> {
        let root = self.resolver.root_id();
        if self.resolver.written_in(provenance) == root || !reference.starts_with('#') {
            return None;
        }
        let (file, pointer) = self.resolver.reference_identity_from(reference, root)?;
        self.resolver.node_at(file, &pointer)?;
        Some(format!(
            "the root document declares `{reference}`, but a `#` fragment addresses the file it \
             is written in; name the root document before the fragment to reference the root's \
             declaration, or declare it in this file"
        ))
    }

    /// Resolve a Media Type Object through any `$ref` hops and give every position-independent
    /// field it can carry a disposition.
    ///
    /// This is the single seam every Media Type Object passes through — request bodies, responses,
    /// parameter content, whole-query-string content, and response header content alike — so a
    /// field that only *sometimes* has an effect is reported once, wherever it appears, instead of
    /// being dispositioned on the request-body path and silently dropped everywhere else.
    pub(super) fn resolve_media_object(
        &mut self,
        object: &crate::oas31::MediaTypeObject,
        media_name: &str,
    ) -> Option<crate::oas31::MediaTypeObject> {
        // A Media Type Object is not `Reference | Object` but an object that may carry a
        // `$ref` of its own (OpenAPI 3.2), so it ends the chain when it carries none, and a hop
        // through the bundle reads the object rather than another Reference.
        let current = self.follow_reference_chain(
            object.clone(),
            &ReferenceChain {
                kind: "Media Type Object",
                cycle_kind: "media type",
                prefix: "#/components/mediaTypes/",
                split: |media| match media.reference.clone() {
                    Some(reference) => RefOr::Ref(reference),
                    None => RefOr::Item(media),
                },
                declared: |components, name| components.media_types.get(name).cloned(),
                follow: |ctx, reference| {
                    let from = ctx.resolver.written_in(&reference.provenance);
                    let resolved = ctx.resolver.resolve_component(
                        &reference.reference,
                        from,
                        |value, pointer, diags| {
                            Some(crate::oas31::deserialize::parse_media_type(
                                value, pointer, diags,
                            ))
                        },
                        ctx.diags,
                    );
                    match resolved {
                        Ok(resolved) => Some(resolved),
                        Err(miss) => ctx.reject_unfollowable_reference(
                            &reference.provenance,
                            "Media Type Object",
                            &reference.reference,
                            miss,
                        ),
                    }
                },
            },
        )?;
        // Encoding is scoped to form and multipart content. The specification says it is simply
        // ignored elsewhere, so rejecting would refuse valid documents — but ignoring it silently
        // would be the fourth behavior this generator does not have. Acknowledge it instead. The
        // form/multipart case carries on to `lower_body_encoding`, which knows the schema.
        if !matches!(
            media_essence(media_name),
            "multipart/form-data" | "application/x-www-form-urlencoded"
        ) {
            self.note_inert_encoding(&current, media_name);
        }
        Some(current)
    }
}

/// One kind of Reference Object chain [`LowerCtx::follow_reference_chain`] follows: a link `S`
/// is either the object `T` the chain ends at or a Reference to the next link.
struct ReferenceChain<S, T> {
    /// The object's name in an undeclared-component or unfollowable-reference rejection.
    kind: &'static str,
    /// The object's name in a reference-cycle rejection.
    cycle_kind: &'static str,
    /// `#/components/<kind>/`, the root component map this kind of reference may name.
    prefix: &'static str,
    /// The object a link ends the chain at, or the Reference it holds instead.
    split: fn(S) -> RefOr<T>,
    /// The root document's own declaration of `name` in this kind's component map.
    declared: fn(&crate::oas31::Components, &str) -> Option<S>,
    /// One hop through the input bundle, reporting a miss itself.
    follow: fn(&mut LowerCtx<'_, '_>, &Reference) -> Option<S>,
}

/// Resolve a Path Item `$ref`.
///
/// Unlike a Reference Object, the specification leaves the behavior of fields declared *alongside*
/// a Path Item `$ref` undefined. Guessing either way ships a client that calls a different set of
/// endpoints than the document describes, so a structural sibling is rejected; `summary` and
/// `description` are documentation and cannot change the wire, so they are applied.
///
/// Applying them is what makes them "allowed" rather than silently dropped. A Path Item resolves
/// to exactly one generated construct per path, so unlike a Reference Object — whose target is a
/// component shared across every use site, and whose per-site docs therefore have nowhere to land
/// (`W011`) — a Path Item reference site has a unique home for its documentation.
pub(super) fn resolve_path_item(
    item: &PathItem,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Option<PathItem> {
    let Some(reference) = &item.reference else {
        return Some(item.clone());
    };
    if let Some(sibling) = item.reference_siblings.first() {
        // E016 case: path-item-ref-siblings
        Diagnostic::error(Code::SpecUndefinedBehavior, reference.provenance.clone())
            .message(format!(
                "a Path Item `$ref` declared alongside `{sibling}` has undefined behavior, so \
                 there is no correct client to generate"
            ))
            .remedy("move the sibling fields into the referenced Path Item, or drop the `$ref`")
            .emit(diags);
        return None;
    }
    // Relative refs inside the referenced item resolve against the file that declared the `$ref`.
    let from = resolver.written_in(&reference.provenance);
    let mut target =
        resolver.resolve_path_item(&reference.reference, from, &reference.provenance, diags)?;
    // One level of indirection is what the specification requires implementations to support, and
    // a chain would need its own cycle guard.
    if target.reference.is_some() {
        // E004 case: declined-hop
        Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
            .message(format!(
                "Path Item `$ref` `{}` resolves to another Path Item `$ref`; chained Path Item \
                 references are not resolved",
                reference.reference
            ))
            .emit(diags);
        return None;
    }
    // The reference site documents *this* path, so its `summary`/`description` override the
    // referenced item's own. Each is overridden independently: declaring only one at the reference
    // site keeps the other from the target rather than blanking it.
    if reference.summary.is_some() {
        target.summary = reference.summary.clone();
    }
    if reference.description.is_some() {
        target.description = reference.description.clone();
    }
    Some(target)
}
