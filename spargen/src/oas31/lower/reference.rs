//! Following a `$ref` to a non-schema component (parameter, request body, response, header,
//! media type, path item) and the rejections an unfollowable one gets.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::oas31::media::media_essence;
use crate::oas31::resolve::reject_undeclared_component;
use crate::oas31::{ParameterObject, PathItem, RefOr, RequestBodyObject, Resolver, ResponseObject};
use crate::source::SpannedValue;

use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Resolve a Header Object that may be a `$ref` into `#/components/headers/`.
    pub(super) fn resolve_header(
        &mut self,
        header: &RefOr<crate::oas31::HeaderObject>,
    ) -> Option<crate::oas31::HeaderObject> {
        let mut current = header.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(header) => return Some(header),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "header");
                    }
                    let alias = self
                        .root_component_name(&reference, "#/components/headers/")
                        .map(|name| self.document.components.headers.get(name).cloned());
                    match alias {
                        Some(Some(target)) => current = target,
                        Some(None) => {
                            return self.reject_component_alias(
                                &reference.provenance,
                                "header",
                                &reference.reference,
                            );
                        }
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle exactly as a Parameter or Response Object reference already
                        // does — and may itself be a Reference, followed from the file it is
                        // written in.
                        None => {
                            current = self.follow_bundle_reference(
                                &reference,
                                "header",
                                crate::oas31::deserialize::parse_header_object,
                            )?;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn resolve_parameter(
        &mut self,
        parameter: &RefOr<ParameterObject>,
    ) -> Option<ParameterObject> {
        let mut current = parameter.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(parameter) => return Some(parameter),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "parameter");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/parameters/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "parameter",
                            crate::oas31::deserialize::parse_parameter,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.parameters.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "parameter",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
        }
    }

    pub(super) fn resolve_request_body(
        &mut self,
        body: &RefOr<RequestBodyObject>,
    ) -> Option<RequestBodyObject> {
        let mut current = body.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(body) => return Some(body),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "request body");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/requestBodies/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "request body",
                            crate::oas31::deserialize::parse_request_body,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.request_bodies.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "request body",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
        }
    }

    pub(super) fn resolve_response(
        &mut self,
        response: &RefOr<ResponseObject>,
    ) -> Option<ResponseObject> {
        let mut current = response.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(response) => return Some(response),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "response");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/responses/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "response",
                            crate::oas31::deserialize::parse_response,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.responses.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "response",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
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
        let mut current = object.clone();
        let mut seen = HashSet::new();
        while let Some(reference) = current.reference.clone() {
            // A Reference Object's own `summary`/`description` documents this use site, which one
            // generated item shared across every use cannot express — the same disposition the
            // Parameter, Response, and Request Body paths already give it.
            self.note_reference_docs(&reference);
            // Keyed on the target each hop resolves to, as the Parameter, Response, Request Body
            // and Header chains are: `#/components/mediaTypes/A` written in two files is two hops.
            if !seen.insert(self.hop_identity(&reference)) {
                return self.reject_alias_cycle(&reference.provenance, "media type");
            }
            let Some(name) = self.root_component_name(&reference, "#/components/mediaTypes/")
            else {
                // Not a root component alias: a multi-file description may reference a whole
                // file, or a sub-file's own components, which resolve through the input bundle
                // exactly as a Parameter or Response Object reference already does.
                let from = self.resolver.written_in(&reference.provenance);
                let resolved = self.resolver.resolve_component(
                    &reference.reference,
                    from,
                    |value, pointer, diags| {
                        Some(crate::oas31::deserialize::parse_media_type(
                            value, pointer, diags,
                        ))
                    },
                    self.diags,
                );
                match resolved {
                    Ok(resolved) => {
                        current = resolved;
                        continue;
                    }
                    Err(miss) => {
                        return self.reject_unfollowable_reference(
                            &reference.provenance,
                            "Media Type Object",
                            &reference.reference,
                            miss,
                        );
                    }
                }
            };
            let Some(target) = self.document.components.media_types.get(name) else {
                return self.reject_component_alias(
                    &reference.provenance,
                    "Media Type Object",
                    &reference.reference,
                );
            };
            current = target.clone();
        }
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
