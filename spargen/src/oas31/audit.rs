use std::borrow::Cow;
use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};
use crate::source::SpannedValue;

use super::{
    Document, EncodingObject, HeaderObject, MediaTypeObject, ParameterObject, PathItem, RefOr,
    Reference, RequestBodyObject, Resolver, ResponseObject, Schema, SchemaOr,
};

/// How a Parameter, Request Body, Response or Header Object target is parsed, as lowering parses
/// it.
type ObjectParser<T> = fn(&SpannedValue, &JsonPointer, &mut Diagnostics) -> Option<T>;

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
    /// Every Path Item, Parameter, Request Body, Response, Header and Media Type Object the walk
    /// has audited or followed a reference through, by `(file, pointer)`, so an object that
    /// several references reach, or that the walk reaches as a root component as well, is audited
    /// once and a reference cycle terminates (#495, #715).
    objects: HashSet<SchemaKey>,
    /// The references met in audited positions, followed once the root walk is done
    /// ([`Audit::follow_references`]).
    pending: Vec<(String, Provenance)>,
    diags: &'a mut Diagnostics,
}

/// The per-keyword audit: walks every schema under the root document's components and paths, and
/// every schema a reference met there reaches — through the Path Items, inline or referenced, and
/// the Parameter, Request Body, Response, Header, Media Type and Encoding Objects that hold them —
/// and emits the once-per-site warnings (validation-only keywords), and resolves the references
/// in every subschema lowering never reads ([`Audit::resolve_unlowered`]). Other rejections fire
/// during parsing and lowering.
pub(crate) fn audit(document: &Document, resolver: &Resolver<'_>, diags: &mut Diagnostics) {
    let consumed_content = consumed_sse_content(document, resolver, diags);
    let mut audit = Audit {
        document,
        resolver,
        consumed_content,
        unlowered_walked: HashSet::new(),
        audited: HashSet::new(),
        objects: HashSet::new(),
        pending: Vec::new(),
        diags,
    };
    audit.walk();
    audit.follow_references();
}

impl Audit<'_, '_> {
    fn walk(&mut self) {
        let document = self.document;
        for schema in document.components.schemas.values() {
            self.audit_schema_ref_or(schema);
        }
        for parameter in document.components.parameters.values() {
            self.audit_parameter_ref_or(parameter);
        }
        for body in document.components.request_bodies.values() {
            self.audit_request_body_ref_or(body);
        }
        for response in document.components.responses.values() {
            self.audit_response_ref_or(response);
        }
        for media in document.components.media_types.values() {
            self.audit_media(media);
        }
        for header in document.components.headers.values() {
            self.audit_header_ref_or(header);
        }

        let root = self.resolver.root_id();
        let path_items = JsonPointer::root().push("components").push("pathItems");
        for (name, item) in &document.components.path_items {
            self.audit_path_item(item, (root, path_items.push(name)));
        }
        for (path, item) in &document.paths.items {
            self.audit_path_item(item, (root, JsonPointer::root().push("paths").push(path)));
        }
    }

    /// The Path Item at `key`: each operation's parameters, request body and responses are
    /// audited, and a Path Item `$ref` is followed to the Path Item it names (#715), one hop, as
    /// lowering follows it; a target that is itself a `$ref` is a chain lowering declines.
    fn audit_path_item(&mut self, item: &PathItem, key: SchemaKey) {
        if !self.objects.insert(key) {
            return;
        }
        if let Some(reference) = &item.reference {
            self.follow_path_item(reference);
        }
        for operation in item.operations.values() {
            for parameter in item.parameters.iter().chain(operation.parameters.iter()) {
                self.audit_parameter_ref_or(parameter);
            }
            if let Some(body) = &operation.request_body {
                self.audit_request_body_ref_or(body);
            }
            for response in operation
                .responses
                .by_status
                .values()
                .chain(operation.responses.default.as_ref())
            {
                self.audit_response_ref_or(response);
            }
        }
    }

    /// Audit the Path Item a Path Item `$ref` names, through the input bundle as lowering resolves
    /// it. A miss is lowering's to report in its own words, so the resolver's diagnostics are
    /// discarded, as [`Self::follow_references`] discards them.
    fn follow_path_item(&mut self, reference: &Reference) {
        let Some(target) = self
            .resolver
            .reference_identity(&reference.reference, &reference.provenance)
        else {
            return;
        };
        if self.objects.contains(&target) {
            return;
        }
        let mut discarded = Diagnostics::new(0);
        let Some(resolved) = self.resolver.resolve_path_item(
            &reference.reference,
            self.resolver.written_in(&reference.provenance),
            &reference.provenance,
            &mut discarded,
        ) else {
            return;
        };
        if resolved.reference.is_none() {
            self.audit_path_item(&resolved, target);
        }
    }

    /// A Parameter position: an inline object is audited, and a Reference Object is followed to
    /// the object it ends at ([`Self::follow_object`]), which is audited in turn.
    fn audit_parameter_ref_or(&mut self, parameter: &RefOr<ParameterObject>) {
        match parameter {
            RefOr::Item(parameter) => self.audit_parameter(parameter),
            RefOr::Ref(reference) => {
                if let Some(parameter) =
                    self.follow_object(reference, super::deserialize::parse_parameter)
                {
                    self.audit_parameter(&parameter);
                }
            }
        }
    }

    /// [`Self::audit_parameter_ref_or`] for a Request Body position.
    fn audit_request_body_ref_or(&mut self, body: &RefOr<RequestBodyObject>) {
        match body {
            RefOr::Item(body) => self.audit_request_body(body),
            RefOr::Ref(reference) => {
                if let Some(body) =
                    self.follow_object(reference, super::deserialize::parse_request_body)
                {
                    self.audit_request_body(&body);
                }
            }
        }
    }

    /// [`Self::audit_parameter_ref_or`] for a Response position.
    fn audit_response_ref_or(&mut self, response: &RefOr<ResponseObject>) {
        match response {
            RefOr::Item(response) => self.audit_response(response),
            RefOr::Ref(reference) => {
                if let Some(response) =
                    self.follow_object(reference, super::deserialize::parse_response)
                {
                    self.audit_response(&response);
                }
            }
        }
    }

    /// [`Self::audit_parameter_ref_or`] for a Header position: a Response or Encoding Object's
    /// `headers` entry, or a `components.headers` one (#715).
    fn audit_header_ref_or(&mut self, header: &RefOr<HeaderObject>) {
        match header {
            RefOr::Item(header) => self.audit_header(header),
            RefOr::Ref(reference) => {
                if let Some(header) =
                    self.follow_object(reference, super::deserialize::parse_header_object)
                {
                    self.audit_header(&header);
                }
            }
        }
    }

    fn audit_headers(&mut self, headers: &indexmap::IndexMap<String, RefOr<HeaderObject>>) {
        for header in headers.values() {
            self.audit_header_ref_or(header);
        }
    }

    /// [`follow_object`], skipping a chain that reaches an object already audited.
    fn follow_object<T>(&self, reference: &Reference, parse: ObjectParser<T>) -> Option<T> {
        follow_object(self.resolver, reference, parse, &self.objects)
    }

    /// Whether the object at `provenance` is met for the first time, claiming it if so.
    fn claim_object(&mut self, provenance: &Provenance) -> bool {
        let key = self.schema_key(provenance);
        self.objects.insert(key)
    }

    fn audit_request_body(&mut self, body: &RequestBodyObject) {
        if self.claim_object(&body.provenance) {
            self.audit_content(&body.content);
        }
    }

    fn audit_response(&mut self, response: &ResponseObject) {
        if self.claim_object(&response.provenance) {
            self.audit_content(&response.content);
            self.audit_headers(&response.headers);
        }
    }

    fn audit_header(&mut self, header: &HeaderObject) {
        if !self.claim_object(&header.provenance) {
            return;
        }
        if let Some(schema) = &header.schema {
            self.audit_schema_ref_or(schema);
        }
        self.audit_content(&header.content);
    }

    /// An Encoding Object, whose `headers` are the schema positions it holds.
    fn audit_encoding(&mut self, encoding: &EncodingObject) {
        self.audit_headers(&encoding.headers);
    }

    fn audit_parameter(&mut self, parameter: &ParameterObject) {
        if !self.claim_object(&parameter.provenance) {
            return;
        }
        if let Some(schema) = &parameter.schema {
            self.audit_schema_ref_or(schema);
        }
        self.audit_content(&parameter.content);
    }

    fn audit_content(&mut self, content: &indexmap::IndexMap<String, MediaTypeObject>) {
        for object in content.values() {
            self.audit_media(object);
        }
    }

    /// A Media Type Object: one that is itself a Reference Object (OpenAPI 3.2) is followed to the
    /// object its chain ends at ([`follow_media`]), whose schemas are what lowering reads.
    fn audit_media(&mut self, media: &MediaTypeObject) {
        if !self.claim_object(&media.provenance) {
            return;
        }
        if media.reference.is_some() {
            if let Some(target) = follow_media(self.resolver, media, &self.objects) {
                self.audit_media(&target);
            }
        }
        if let Some(schema) = &media.schema {
            self.audit_schema_ref_or(schema);
        }
        if let Some(schema) = &media.item_schema {
            self.audit_schema_ref_or(schema);
        }
        let encodings = media
            .encoding
            .values()
            .chain(media.prefix_encoding.iter().map(|(encoding, _)| encoding))
            .chain(media.item_encoding.iter().map(|(encoding, _)| encoding));
        for encoding in encodings {
            self.audit_encoding(encoding);
        }
    }

    /// A schema position that may hold a bare Reference Object: an inline schema is audited, and
    /// a reference is queued to be followed ([`Self::follow_references`]).
    fn audit_schema_ref_or(&mut self, schema: &RefOr<Schema>) {
        match schema {
            RefOr::Item(schema) => self.audit_schema(schema),
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
            if self.resolver.declares_root_schema(&reference) {
                continue;
            }
            match self.resolver.reference_identity(&reference, &at) {
                Some(target) if !self.audited.contains(&target) => {}
                _ => continue,
            }
            let mut discarded = Diagnostics::new(0);
            if let Ok(resolved) = self.resolver.resolve(&reference, &at, &mut discarded) {
                self.audit_schema(&resolved.schema);
            }
        }
    }

    fn schema_key(&self, provenance: &Provenance) -> SchemaKey {
        (
            self.resolver.written_in(provenance),
            provenance.pointer.clone(),
        )
    }

    fn audit_schema(&mut self, schema: &Schema) {
        if !self.audited.insert(self.schema_key(&schema.provenance)) {
            return;
        }
        if let Some(reference) = &schema.reference {
            self.pending
                .push((reference.clone(), schema.provenance.clone()));
        }
        let diags = &mut *self.diags;
        if !schema.validation.constrains_nothing() {
            // W001 case: validation-keyword
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
            // W001 case: content-annotation
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
            // W001 case: validation-keyword
            Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
                .message(
                    "`patternProperties` key patterns are not enforced: the generated typed map \
                 captures all non-declared properties, not only pattern-matching keys",
                )
                .remedy("keep producer-side validation for the key pattern")
                .emit(diags);
        }

        let lowered = schema
            .properties
            .values()
            .chain(schema.additional_properties.as_deref())
            .chain(schema.pattern_properties.values())
            .chain(schema.items.as_deref())
            .chain(&schema.prefix_items)
            .chain(&schema.all_of)
            .chain(&schema.one_of)
            .chain(&schema.any_of);
        for child in lowered {
            self.audit_schema_or(child);
        }
        // Lowering reads a `$defs` entry only when a reference names it, so an unreferenced one is
        // reached by nothing else; resolving a referenced one here as well reports nothing lowering
        // would not.
        let unlowered = schema
            .defs
            .values()
            .chain(schema.validation_children.iter().map(|(_, child)| child));
        for child in unlowered {
            self.audit_schema_or(child);
            self.resolve_unlowered_or(child);
        }
        if let Some(child) = schema.content_schema.as_deref() {
            self.audit_schema_or(child);
            if !content_consumed {
                self.resolve_unlowered_or(child);
            }
        }
    }

    fn audit_schema_or(&mut self, schema: &SchemaOr) {
        if let SchemaOr::Schema(schema) = schema {
            self.audit_schema(schema);
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
            super::discriminator::resolve_discriminator_targets(
                self.resolver,
                self.diags,
                discriminator,
            );
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
        if self.resolver.declares_root_schema(reference) {
            return;
        }
        let from_root = self.resolver.written_in(at) == self.resolver.root_id();
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            let components = &self.document.components.schemas;
            // As lowering reads it: from the root, a name whose first segment is no declared
            // component is a missing component, and one whose first segment is declared is a
            // pointer into that component's body, which the resolver walks.
            let into_a_declared_component = name
                .split_once('/')
                .is_some_and(|(root, _)| components.contains_key(root));
            if from_root && !into_a_declared_component {
                super::resolve::reject_undeclared_component(self.diags, at, "schema", reference);
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
}

fn consumed_sse_content(
    document: &Document,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
) -> HashSet<AnnotationKey> {
    let mut consumed = HashSet::new();
    let mut inspect = |content: &indexmap::IndexMap<String, MediaTypeObject>| {
        for (media_name, media) in content {
            // The classification lowering selects a body by, so a key it consumes here is one
            // lowering reads as a stream, in whatever letter case it is spelled (#709).
            let classified = super::media::classify_media(super::media::media_essence(media_name));
            if classified.is_some_and(|(media, _)| media == crate::ir::MediaType::EventStream) {
                let media = follow_media(resolver, media, &HashSet::new());
                if let Some(item) = media.as_ref().and_then(|media| media.item_schema.as_ref()) {
                    if let Some(json) = super::sse::json_data_schema(item, resolver, diags) {
                        consumed.insert(annotation_key(&json.annotation_site));
                    }
                }
            }
        }
    };
    // Every response lowering reads, followed as lowering follows it — into another file as well
    // (#495), since the walk audits a response written there too.
    let mut inspect_response = |response: &RefOr<ResponseObject>| match response {
        RefOr::Item(response) => inspect(&response.content),
        RefOr::Ref(reference) => {
            if let Some(response) = follow_object(
                resolver,
                reference,
                super::deserialize::parse_response,
                &HashSet::new(),
            ) {
                inspect(&response.content);
            }
        }
    };
    for response in document.components.responses.values() {
        inspect_response(response);
    }
    // Every Path Item the walk audits (#715): the root's, inline in `paths` or in
    // `components.pathItems`, and the one each Path Item `$ref` among them names, followed one hop
    // as lowering follows it. A target reached twice is inspected twice, which consumes nothing
    // new.
    let roots = document
        .components
        .path_items
        .values()
        .chain(document.paths.items.values());
    let mut items: Vec<Cow<'_, PathItem>> = Vec::new();
    for item in roots {
        if let Some(reference) = &item.reference {
            let mut discarded = Diagnostics::new(0);
            if let Some(target) = resolver.resolve_path_item(
                &reference.reference,
                resolver.written_in(&reference.provenance),
                &reference.provenance,
                &mut discarded,
            ) {
                items.push(Cow::Owned(target));
            }
        }
        items.push(Cow::Borrowed(item));
    }
    for item in &items {
        for operation in item.operations.values() {
            for response in operation.responses.by_status.values() {
                inspect_response(response);
            }
            if let Some(response) = &operation.responses.default {
                inspect_response(response);
            }
        }
    }
    consumed
}

/// Follow a Reference Object, hop by hop, to the object its chain ends at, as lowering does
/// (#495): through the input bundle, so a reference into another file — a whole file, or a
/// pointer into one — reaches the object written there, and a root `#/components/<kind>/<name>`
/// reaches the root's declaration, which is the same node. `hop` reads one hop's target as
/// written: the object, or the next Reference Object.
///
/// `None` when a hop targets an object in `done`, the chain cycles, or a hop misses. A miss is
/// not reported here: lowering follows the same chain and reports it in its own words, so the
/// resolver's diagnostics are discarded, as [`Audit::follow_references`] discards them. That
/// silence, and stopping at an object in `done`, are why this is not lowering's
/// `follow_reference_chain`, which reports each hop's documentation and every miss.
fn follow_chain<T>(
    resolver: &Resolver<'_>,
    reference: &Reference,
    done: &HashSet<SchemaKey>,
    hop: impl Fn(&Reference, &mut Diagnostics) -> Option<RefOr<T>>,
) -> Option<T> {
    let mut reference = Cow::Borrowed(reference);
    let mut seen = HashSet::new();
    loop {
        let target = resolver.reference_identity(&reference.reference, &reference.provenance)?;
        if done.contains(&target) || !seen.insert(target) {
            return None;
        }
        match hop(&reference, &mut Diagnostics::new(0))? {
            RefOr::Item(object) => return Some(object),
            RefOr::Ref(next) => reference = Cow::Owned(next),
        }
    }
}

/// [`follow_chain`] for a Parameter, Request Body, Response or Header Reference Object, each hop
/// of which may be a Reference Object or the object `parse` reads.
fn follow_object<T>(
    resolver: &Resolver<'_>,
    reference: &Reference,
    parse: ObjectParser<T>,
    done: &HashSet<SchemaKey>,
) -> Option<T> {
    follow_chain(resolver, reference, done, |reference, discarded| {
        resolver
            .resolve_component_or_ref(
                &reference.reference,
                resolver.written_in(&reference.provenance),
                parse,
                discarded,
            )
            .ok()
    })
}

/// [`follow_chain`] for a Media Type Object, which OpenAPI 3.2 allows to be a Reference Object:
/// the object its chain ends at, or `media` itself when it is no reference.
fn follow_media<'a>(
    resolver: &Resolver<'_>,
    media: &'a MediaTypeObject,
    done: &HashSet<SchemaKey>,
) -> Option<Cow<'a, MediaTypeObject>> {
    let Some(reference) = &media.reference else {
        return Some(Cow::Borrowed(media));
    };
    follow_chain(resolver, reference, done, |reference, discarded| {
        let target = resolver
            .resolve_component(
                &reference.reference,
                resolver.written_in(&reference.provenance),
                |value, pointer, diags| {
                    Some(super::deserialize::parse_media_type(value, pointer, diags))
                },
                discarded,
            )
            .ok()?;
        Some(match target.reference.clone() {
            Some(next) => RefOr::Ref(next),
            None => RefOr::Item(target),
        })
    })
    .map(Cow::Owned)
}

fn annotation_key(provenance: &Provenance) -> AnnotationKey {
    (
        provenance.span.map(|span| span.file),
        provenance.pointer.clone(),
    )
}
