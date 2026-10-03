use std::borrow::Cow;
use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};

use super::{Document, MediaTypeObject, RefOr, Resolver, Schema, SchemaOr, ValidationKeywords};

type AnnotationKey = (Option<FileId>, JsonPointer);

/// A schema node's `(file, pointer)` identity, with an unspanned node placed in the root document,
/// as [`Resolver::reference_identity`] places a reference written at one.
type SchemaKey = (FileId, JsonPointer);

/// What one audit pass carries through its walk.
struct Audit<'a, 'doc> {
    document: &'doc Document,
    resolver: &'a Resolver<'doc>,
    /// The `contentSchema` sites an SSE envelope consumes, which lowering reads.
    consumed_content: HashSet<AnnotationKey>,
    /// Every schema node [`Audit::resolve_unlowered`] has already walked, by `(file, pointer)`,
    /// so a subtree reached twice is walked once and a cycle of references terminates.
    unlowered_walked: HashSet<AnnotationKey>,
    /// Every schema node [`Audit::audit_schema`] has audited, so a target that several references
    /// reach, or that the walk reaches as well, is audited once and a reference cycle terminates.
    audited: HashSet<SchemaKey>,
    /// The references met in audited positions, followed once the root walk is done
    /// ([`Audit::follow_references`]).
    pending: Vec<(String, Provenance)>,
    diags: &'a mut Diagnostics,
}

/// The per-keyword audit: walks every reachable schema and emits the once-per-site warnings
/// (validation-only keywords), and resolves the references in every subschema lowering never
/// reads ([`Audit::resolve_unlowered`]). Other rejections fire during parsing and lowering.
pub(crate) fn audit(document: &Document, resolver: &Resolver<'_>, diags: &mut Diagnostics) {
    let consumed_content = consumed_sse_content(document, resolver, diags);
    let mut audit = Audit {
        document,
        resolver,
        consumed_content,
        unlowered_walked: HashSet::new(),
        audited: HashSet::new(),
        pending: Vec::new(),
        diags,
    };
    audit.walk();
    audit.follow_references();
}

impl Audit<'_, '_> {
    fn walk(&mut self) {
        let document = self.document;
        for (name, schema) in &document.components.schemas {
            self.audit_schema_ref_or(
                schema,
                JsonPointer::root()
                    .push("components")
                    .push("schemas")
                    .push(name),
            );
        }

        let components_pointer = JsonPointer::root().push("components");
        for (name, parameter) in &document.components.parameters {
            if let RefOr::Item(parameter) = parameter {
                self.audit_parameter(parameter, components_pointer.push("parameters").push(name));
            }
        }
        for (name, body) in &document.components.request_bodies {
            if let RefOr::Item(body) = body {
                self.audit_content(
                    &body.content,
                    components_pointer
                        .push("requestBodies")
                        .push(name)
                        .push("content"),
                );
            }
        }
        for (name, response) in &document.components.responses {
            if let RefOr::Item(response) = response {
                self.audit_content(
                    &response.content,
                    components_pointer
                        .push("responses")
                        .push(name)
                        .push("content"),
                );
            }
        }
        for (name, media) in &document.components.media_types {
            self.audit_media(media, components_pointer.push("mediaTypes").push(name));
        }

        for (path, item) in &document.paths.items {
            for (method, operation) in &item.operations {
                let op_pointer = JsonPointer::root()
                    .push("paths")
                    .push(path)
                    .push(method.as_str());
                for (index, parameter) in item
                    .parameters
                    .iter()
                    .chain(operation.parameters.iter())
                    .enumerate()
                {
                    if let RefOr::Item(parameter) = parameter {
                        self.audit_parameter(parameter, op_pointer.push("parameters").index(index));
                    }
                }
                if let Some(RefOr::Item(body)) = &operation.request_body {
                    self.audit_content(
                        &body.content,
                        op_pointer.push("requestBody").push("content"),
                    );
                }
                for (status, response) in &operation.responses.by_status {
                    if let RefOr::Item(response) = response {
                        self.audit_content(
                            &response.content,
                            op_pointer.push("responses").push(status).push("content"),
                        );
                    }
                }
                if let Some(RefOr::Item(response)) = &operation.responses.default {
                    self.audit_content(
                        &response.content,
                        op_pointer.push("responses").push("default").push("content"),
                    );
                }
            }
        }
    }

    fn audit_parameter(&mut self, parameter: &super::ParameterObject, pointer: JsonPointer) {
        if let Some(schema) = &parameter.schema {
            self.audit_schema_ref_or(schema, pointer.push("schema"));
        }
        self.audit_content(&parameter.content, pointer.push("content"));
    }

    fn audit_content(
        &mut self,
        content: &indexmap::IndexMap<String, MediaTypeObject>,
        pointer: JsonPointer,
    ) {
        for (media, object) in content {
            self.audit_media(object, pointer.push(media));
        }
    }

    fn audit_media(&mut self, media: &MediaTypeObject, pointer: JsonPointer) {
        if let Some(schema) = &media.schema {
            self.audit_schema_ref_or(schema, pointer.push("schema"));
        }
        if let Some(schema) = &media.item_schema {
            self.audit_schema_ref_or(schema, pointer.push("itemSchema"));
        }
    }

    /// A schema position that may hold a bare Reference Object: an inline schema is audited, and
    /// a reference is queued to be followed ([`Self::follow_references`]).
    fn audit_schema_ref_or(&mut self, schema: &RefOr<Schema>, pointer: JsonPointer) {
        match schema {
            RefOr::Item(schema) => self.audit_schema(schema, pointer),
            RefOr::Ref(reference) => self
                .pending
                .push((reference.reference.clone(), reference.provenance.clone())),
        }
    }

    /// Audit every schema a queued reference reaches that the walk has not (#446).
    ///
    /// The walk starts only from positions written in the root document, so a schema in a
    /// referenced sub-file or vendored remote document, which lowering reads when a `$ref` reaches
    /// it, was never audited: no `W001` for its validation-only keywords and no resolution of the
    /// references under its unlowered keywords. Each reference met in an audited position is
    /// followed to its `(file, pointer)` target, which is parsed and audited in turn, so the
    /// target's own references are queued too. [`Self::audit_schema`] audits each target once.
    ///
    /// A miss is not reported here: the reference sits either in a position lowering reads, which
    /// reports it in its own words, or under a keyword lowering never reads, which
    /// [`Self::resolve_unlowered`] reports. So the resolver's diagnostics are discarded.
    fn follow_references(&mut self) {
        while let Some((reference, at)) = self.pending.pop() {
            // As lowering reads it: a bare `#/components/schemas/<name>` the root declares is that
            // root component wherever the reference is written, and the walk has audited every
            // root component already (or queued the reference it is).
            if reference
                .strip_prefix("#/components/schemas/")
                .is_some_and(|name| self.document.components.schemas.contains_key(name))
            {
                continue;
            }
            match self.resolver.reference_identity(&reference, &at) {
                Some(target) if !self.audited.contains(&target) => {}
                _ => continue,
            }
            let mut discarded = Diagnostics::new(0);
            if let Ok(resolved) = self.resolver.resolve(&reference, &at, &mut discarded) {
                let pointer = resolved.schema.provenance.pointer.clone();
                self.audit_schema(&resolved.schema, pointer);
            }
        }
    }

    fn schema_key(&self, provenance: &Provenance) -> SchemaKey {
        (
            provenance
                .span
                .map_or_else(|| self.resolver.root_id(), |span| span.file),
            provenance.pointer.clone(),
        )
    }

    fn audit_schema(&mut self, schema: &Schema, pointer: JsonPointer) {
        if !self.audited.insert(self.schema_key(&schema.provenance)) {
            return;
        }
        if let Some(reference) = &schema.reference {
            self.pending
                .push((reference.clone(), schema.provenance.clone()));
        }
        let diags = &mut *self.diags;
        if has_validation_keywords(&schema.validation) {
            Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
                .message("validation-only schema keywords are not enforced at runtime")
                .remedy("keep producer-side validation for these constraints")
                .emit(diags);
        }

        let content_consumed = self
            .consumed_content
            .contains(&annotation_key(&schema.provenance));
        if (schema.content_media_type.is_some() || schema.content_schema.is_some())
            && !content_consumed
        {
            Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(
                "`contentMediaType`/`contentSchema` are decoded only on an OpenAPI 3.2 SSE \
                 envelope's string `data` property",
            )
            .remedy(
                "place the annotations on the `data` property of a text/event-stream itemSchema, \
                 or decode the string content in application code",
            )
            .emit(diags);
        }

        // A `patternProperties` key regex is a validation-only constraint: the generated typed overflow
        // map captures every non-declared property regardless of the pattern, so the key regex is not
        // enforced. Acknowledge it as `W001` (never silent) — the value schemas still lower.
        if !schema.pattern_properties.is_empty() {
            Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
                .message(
                    "`patternProperties` key patterns are not enforced: the generated typed map \
                 captures all non-declared properties, not only pattern-matching keys",
                )
                .remedy("keep producer-side validation for the key pattern")
                .emit(diags);
        }

        for (name, child) in &schema.properties {
            self.audit_schema_or(child, pointer.push("properties").push(name));
        }
        if let Some(child) = &schema.additional_properties {
            self.audit_schema_or(child, pointer.push("additionalProperties"));
        }
        for (pattern, child) in &schema.pattern_properties {
            self.audit_schema_or(child, pointer.push("patternProperties").push(pattern));
        }
        if let Some(child) = &schema.items {
            self.audit_schema_or(child, pointer.push("items"));
        }
        for (index, child) in schema.prefix_items.iter().enumerate() {
            self.audit_schema_or(child, pointer.push("prefixItems").index(index));
        }
        for (index, child) in schema.all_of.iter().enumerate() {
            self.audit_schema_or(child, pointer.push("allOf").index(index));
        }
        for (index, child) in schema.one_of.iter().enumerate() {
            self.audit_schema_or(child, pointer.push("oneOf").index(index));
        }
        for (index, child) in schema.any_of.iter().enumerate() {
            self.audit_schema_or(child, pointer.push("anyOf").index(index));
        }
        // Lowering reads a `$defs` entry only when a reference names it, so an unreferenced one is
        // reached by nothing else; resolving a referenced one here as well reports nothing lowering
        // would not.
        for (name, child) in &schema.defs {
            self.audit_schema_or(child, pointer.push("$defs").push(name));
            self.resolve_unlowered_or(child);
        }
        for (keyword, child) in &schema.validation_children {
            self.audit_schema_or(child, pointer.push(keyword));
            self.resolve_unlowered_or(child);
        }
        if let Some(child) = schema.content_schema.as_deref() {
            self.audit_schema_or(child, pointer.push("contentSchema"));
            if !content_consumed {
                self.resolve_unlowered_or(child);
            }
        }
    }

    fn audit_schema_or(&mut self, schema: &SchemaOr, pointer: JsonPointer) {
        if let SchemaOr::Schema(schema) = schema {
            self.audit_schema(schema, pointer);
        }
    }

    fn resolve_unlowered_or(&mut self, schema: &SchemaOr) {
        if let SchemaOr::Schema(schema) = schema {
            self.resolve_unlowered(schema);
        }
    }

    /// Resolve every reference in `schema`, a subschema lowering never reads (#424): one under
    /// `not`, `if`/`then`/`else`, `contains`, `propertyNames`, `unevaluated*` or
    /// `dependentSchemas`, an unreferenced `$defs` entry, or a `contentSchema` no SSE envelope
    /// consumes. The subschema still lowers to nothing, but a reference naming nothing is a broken
    /// document wherever it sits, so it is `E004` here as it is in a position lowering reads.
    ///
    /// Every keyword of the subtree is walked, since none of it is lowered, and a reference is
    /// read the way lowering reads one: a `#/components/schemas/<name>` the root declares is that
    /// component, which lowering reaches on its own, and anything else goes to the resolver, which
    /// reports a miss itself. A target the resolver parses is walked in turn — it is in this
    /// subtree's position, and lowering reads it only if something else names it. Each
    /// `discriminator`'s `mapping` and `defaultMapping` values are resolved as a lowered one's are.
    fn resolve_unlowered(&mut self, schema: &Schema) {
        if !self
            .unlowered_walked
            .insert(annotation_key(&schema.provenance))
        {
            return;
        }
        if let Some(reference) = &schema.reference {
            self.resolve_unlowered_ref(reference, &schema.provenance);
        }
        if let Some(discriminator) = &schema.discriminator {
            self.resolve_unlowered_discriminator(discriminator);
        }
        let children = schema
            .properties
            .values()
            .chain(schema.additional_properties.as_deref())
            .chain(schema.pattern_properties.values())
            .chain(schema.items.as_deref())
            .chain(&schema.prefix_items)
            .chain(&schema.all_of)
            .chain(&schema.one_of)
            .chain(&schema.any_of)
            .chain(schema.defs.values())
            .chain(schema.validation_children.iter().map(|(_, child)| child))
            .chain(schema.content_schema.as_deref());
        for child in children {
            self.resolve_unlowered_or(child);
        }
    }

    fn resolve_unlowered_ref(&mut self, reference: &str, at: &Provenance) {
        let from_root = at
            .span
            .is_none_or(|span| span.file == self.resolver.root_id());
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            let components = &self.document.components.schemas;
            if components.contains_key(name) {
                return;
            }
            // As lowering reads it: from the root, a name whose first segment is no declared
            // component is a missing component, and one whose first segment is declared is a
            // pointer into that component's body, which the resolver walks.
            let into_a_declared_component = name
                .split_once('/')
                .is_some_and(|(root, _)| components.contains_key(root));
            if from_root && !into_a_declared_component {
                super::lower::reject_undeclared_component(self.diags, at, "schema", reference);
                return;
            }
        }
        if self
            .resolver
            .reference_identity(reference, at)
            .is_some_and(|(file, pointer)| self.unlowered_walked.contains(&(Some(file), pointer)))
        {
            return;
        }
        if let Ok(resolved) = self.resolver.resolve(reference, at, self.diags) {
            if let Cow::Owned(target) = resolved.schema {
                self.resolve_unlowered(&target);
            }
        }
    }

    fn resolve_unlowered_discriminator(&mut self, discriminator: &super::Discriminator) {
        let entries = discriminator
            .mapping
            .iter()
            .map(|(tag, target)| (Some(tag), target))
            .chain(
                discriminator
                    .default_mapping
                    .iter()
                    .map(|target| (None, target)),
            );
        for (tag, target) in entries {
            super::lower::discriminator_target_identity(
                self.document,
                self.resolver,
                self.diags,
                &super::lower::discriminator_entry(tag),
                target,
            );
        }
    }
}

fn consumed_sse_content(
    document: &Document,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
) -> HashSet<AnnotationKey> {
    let mut consumed = HashSet::new();
    let mut inspect = |content: &indexmap::IndexMap<String, MediaTypeObject>| {
        for (media_name, media) in content {
            if media_name
                .split(';')
                .next()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                let media = resolve_media(document, media);
                if let Some(item) = media.and_then(|media| media.item_schema.as_ref()) {
                    if let Some(json) = super::sse::json_data_schema(item, resolver, diags) {
                        consumed.insert(annotation_key(&json.annotation_site));
                    }
                }
            }
        }
    };
    for response in document.components.responses.values() {
        if let RefOr::Item(response) = response {
            inspect(&response.content);
        }
    }
    for item in document.paths.items.values() {
        for operation in item.operations.values() {
            for response in operation.responses.by_status.values() {
                if let Some(response) = resolve_response(document, response) {
                    inspect(&response.content);
                }
            }
            if let Some(response) = operation
                .responses
                .default
                .as_ref()
                .and_then(|response| resolve_response(document, response))
            {
                inspect(&response.content);
            }
        }
    }
    consumed
}

fn resolve_media<'a>(
    document: &'a Document,
    media: &'a MediaTypeObject,
) -> Option<&'a MediaTypeObject> {
    let mut current = media;
    let mut seen = HashSet::new();
    while let Some(reference) = current.reference.as_ref() {
        let name = reference
            .reference
            .strip_prefix("#/components/mediaTypes/")?;
        if !seen.insert(name) {
            return None;
        }
        current = document.components.media_types.get(name)?;
    }
    Some(current)
}

fn resolve_response<'a>(
    document: &'a Document,
    response: &'a RefOr<super::ResponseObject>,
) -> Option<&'a super::ResponseObject> {
    let mut current = response;
    let mut seen = HashSet::new();
    loop {
        match current {
            RefOr::Item(response) => return Some(response),
            RefOr::Ref(reference) => {
                let name = reference
                    .reference
                    .strip_prefix("#/components/responses/")?;
                if !seen.insert(name) {
                    return None;
                }
                current = document.components.responses.get(name)?;
            }
        }
    }
}

fn annotation_key(provenance: &Provenance) -> AnnotationKey {
    (
        provenance.span.map(|span| span.file),
        provenance.pointer.clone(),
    )
}

fn has_validation_keywords(validation: &ValidationKeywords) -> bool {
    validation.pattern.is_some()
        || validation.minimum.is_some()
        || validation.maximum.is_some()
        || validation.exclusive_minimum.is_some()
        || validation.exclusive_maximum.is_some()
        || validation.multiple_of.is_some()
        || validation.min_length.is_some()
        || validation.max_length.is_some()
        || validation.min_items.is_some()
        || validation.max_items.is_some()
        || validation.unique_items
        || validation.min_properties.is_some()
        || validation.max_properties.is_some()
        || validation.other
}
