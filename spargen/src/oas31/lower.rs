use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, BodyEncoding, DefaultValue, Delimiter, DisjointFeature, Docs,
    EncodingMode, Field, FieldDefault, HttpScheme, Info, JsonCategory, MediaType, Operation,
    OperationId, ParamLoc, ParamStyle, Parameter, PathSegment, PathTemplate, Prim,
    PropertyEncoding, PropertyName, RequestBody, Response, ResponseHeader, Responses, ScalarEnum,
    ScalarRepr, ScalarValue, SchemeId, SecurityScheme, SecuritySchemeDef, Server, StatusSpec,
    Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind, Union, UnionMode, UnionStrategy,
    UnionVariant, UrlSegment, XmlField,
};
use crate::name::synth_operation_id;
use crate::source::{is_remote_ref, Node, Number, SpannedValue};

use super::{
    Document, EncodingObject, JsonType, MediaTypeObject, ParameterObject, PathItem, RefOr,
    RequestBodyObject, Resolver, ResponseObject, Schema, SchemaOr, SecurityRequirement,
    ValidationKeywords,
};

/// Maximum schema-lowering recursion depth. Each nested object property, array item,
/// `allOf`/`oneOf`/`anyOf` member, and resolved `$ref` target descends one level through
/// [`LowerCtx::lower_schema`]. Inline nesting is already bounded by the parser's own depth cap, but
/// a chain of components (or remote refs) that each `$ref` the next is parsed shallowly and would
/// otherwise recurse without bound — a long enough chain overflows the stack. This cap stops that
/// descent and rejects with `E014` (`SchemaNestingTooDeep`) instead of crashing. It is far above any
/// real API's nesting depth; the whole frontend runs on a dedicated large-stack thread (see the
/// facade) so lowering this many levels deep is comfortably safe.
const MAX_SCHEMA_DEPTH: u32 = 128;

/// Lower a typed OpenAPI 3.1 or 3.2 [`Document`] into the version-agnostic [`Api`] IR.
pub(crate) fn lower(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Result<Api, Aborted> {
    let mut security_schemes = lower_security_schemes(document, diags);
    // OpenAPI 3.2 lets a security requirement name a Security Scheme Object by URI instead of by
    // component name. A component name always wins — the specification is explicit that name
    // lookup takes precedence, and flags the resulting hijack risk — so only names that match no
    // declared component are resolved as references. A leading `./` forces the URI reading for a
    // single-segment name that would otherwise collide.
    resolve_external_security_schemes(document, resolver, &mut security_schemes, diags);
    let mut ctx = LowerCtx {
        document,
        resolver,
        diags,
        graph: TypeGraph::default(),
        components: HashMap::new(),
        in_progress: HashMap::new(),
        component_alias_stack: HashSet::new(),
        remote_components: HashMap::new(),
        remote_in_progress: HashMap::new(),
        remote_alias_stack: HashSet::new(),
        depth: 0,
    };

    for name in document.components.schemas.keys() {
        let _ = ctx.ensure_component(name);
    }

    let mut operations = Vec::new();
    let mut operation_ids = HashSet::new();
    for (path, item) in &document.paths.items {
        let Some(item) = resolve_path_item(item, resolver, ctx.diags) else {
            continue;
        };
        let item = &item;
        for (method, operation) in &item.operations {
            let path_template = parse_path_template(path);
            let id = operation
                .operation_id
                .clone()
                .unwrap_or_else(|| synth_operation_id(method, &path_template));
            if !operation_ids.insert(id.clone()) {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message(format!(
                        "operationId `{id}` is not unique within the API description"
                    ))
                    .emit(ctx.diags);
            }

            let mut params = Vec::new();
            let mut merged_parameters: IndexMap<(String, String), ParameterObject> =
                IndexMap::new();
            for parameter in &item.parameters {
                if let Some(parameter) = ctx.resolve_parameter(parameter) {
                    let key = (parameter.location.clone(), parameter.name.clone());
                    if merged_parameters.insert(key, parameter).is_some() {
                        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                            .message("path-item parameters contain a duplicate name/location pair")
                            .emit(ctx.diags);
                    }
                }
            }
            let mut operation_parameter_keys = HashSet::new();
            for parameter in &operation.parameters {
                if let Some(parameter) = ctx.resolve_parameter(parameter) {
                    let key = (parameter.location.clone(), parameter.name.clone());
                    if !operation_parameter_keys.insert(key.clone()) {
                        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                            .message("operation parameters contain a duplicate name/location pair")
                            .emit(ctx.diags);
                    }
                    // Operation-level parameters override the matching path-item parameter.
                    merged_parameters.insert(key, parameter);
                }
            }
            for parameter in merged_parameters.values() {
                if let Some(parameter) = ctx.lower_parameter(parameter) {
                    params.push(parameter);
                }
            }
            let placeholders: HashSet<&str> = path_template
                .segments
                .iter()
                .filter_map(|segment| match segment {
                    PathSegment::Param(name) => Some(name.as_str()),
                    PathSegment::Literal(_) => None,
                })
                .collect();
            let path_parameters: HashSet<&str> = params
                .iter()
                .filter(|parameter| parameter.location == ParamLoc::Path)
                .map(|parameter| parameter.name.as_str())
                .collect();
            if placeholders != path_parameters {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message(format!(
                        "path template parameters {placeholders:?} do not match declared path \
                         parameters {path_parameters:?}"
                    ))
                    .emit(ctx.diags);
            }
            let querystrings = params
                .iter()
                .filter(|parameter| parameter.location == ParamLoc::QueryString)
                .count();
            let named_queries = params
                .iter()
                .any(|parameter| parameter.location == ParamLoc::Query);
            if querystrings > 1 || (querystrings == 1 && named_queries) {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message(
                        "an operation may declare at most one `in: querystring` parameter and may \
                         not combine it with `in: query` parameters",
                    )
                    .emit(ctx.diags);
            }

            let request_body = operation
                .request_body
                .as_ref()
                .and_then(|body| ctx.resolve_request_body(body))
                .and_then(|body| ctx.lower_request_body(&body));

            let responses = ctx.lower_responses(&operation.responses);
            // XML decode is scoped to the single-body success/error paths. An XML body that would
            // land in a multi-status response enum is rejected cleanly (narrowed `E009`) rather than
            // silently decoded as JSON.
            if responses.xml_in_multi_status() {
                Diagnostic::error(Code::UnsupportedMediaType, operation.provenance.clone())
                    .message(
                        "an application/xml (or text/xml) response body is only supported as an \
                         operation's single success or single error body; it cannot participate in \
                         a multi-status response enum",
                    )
                    .remedy(
                        "give the operation a single XML-bodied success/error response, use JSON \
                         for the multi-status responses, or omit this API segment with \
                         spargen::omit!",
                    )
                    .emit(ctx.diags);
            }

            let security: Vec<crate::ir::SecurityRequirement> = operation
                .security
                .as_ref()
                .unwrap_or(&document.security)
                .iter()
                .map(lower_security_requirement)
                .collect();
            // Codegen builds per-operation credential tables from the scheme map, so every
            // referenced scheme must have lowered; an undeclared or unsupported scheme would
            // otherwise silently generate an unauthenticated call.
            for requirement in &security {
                for (scheme, _) in &requirement.0 {
                    if !security_schemes.contains_key(scheme) {
                        Diagnostic::error(
                            Code::UnknownSecurityScheme,
                            operation.provenance.clone(),
                        )
                        .message(format!(
                            "security requirement references undeclared or unsupported \
                             scheme `{}`",
                            scheme.0
                        ))
                        .remedy(
                            "declare the scheme under components.securitySchemes as http \
                             bearer/basic, apiKey, oauth2, or openIdConnect",
                        )
                        .emit(ctx.diags);
                    }
                }
            }

            let mut operation_description = operation.description.clone();
            // A Path Item's `summary`/`description` apply to every operation on the path. They are
            // additional context rather than a replacement, so the operation's own documentation
            // stays first and these follow it.
            for text in [item.summary.as_ref(), item.description.as_ref()]
                .into_iter()
                .flatten()
            {
                append_text(&mut operation_description, text.clone());
            }
            if !operation.tags.is_empty() {
                append_text(
                    &mut operation_description,
                    format!("Tags: {}.", operation.tags.join(", ")),
                );
            }
            for (status, response) in &operation.responses.by_status {
                if let Some(response) = ctx.resolve_response(response) {
                    append_response_docs(&mut operation_description, status, &response);
                }
            }
            if let Some(response) = operation
                .responses
                .default
                .as_ref()
                .and_then(|response| ctx.resolve_response(response))
            {
                append_response_docs(&mut operation_description, "default", &response);
            }

            // Operation `servers` override the path item's, which override the document's. The
            // document's is the client's base URL, so "no override" is the common case.
            let server = if operation.servers.is_empty() {
                lower_server_override(&item.servers, ctx.diags)
            } else {
                lower_server_override(&operation.servers, ctx.diags)
            };

            operations.push(Operation {
                id: OperationId(id),
                method: method.clone(),
                path: path_template,
                params,
                request_body,
                responses,
                security,
                deprecated: operation.deprecated,
                docs: Docs {
                    title: None,
                    summary: operation.summary.clone(),
                    description: operation_description,
                    deprecated: operation.deprecated,
                },
                server,
                provenance: operation.provenance.clone(),
            });
        }
    }

    // `xml.name`/`xml.attribute` become a format-agnostic serde `rename`, so they may only be applied
    // to a schema used *exclusively* as an XML body — otherwise the rename would corrupt the JSON
    // wire format. Suppress (and warn `W006` on) the rename for any shared/non-XML-reachable type.
    gate_xml_field_renames(&mut ctx.graph, &operations, ctx.diags);

    let mut api_description = document.info.summary.clone();
    if let Some(description) = &document.info.description {
        append_text(&mut api_description, description.clone());
    }
    if let Some(contact) = &document.info.contact {
        append_text(&mut api_description, format!("Contact: {contact}."));
    }
    if let Some(license) = &document.info.license {
        append_text(&mut api_description, format!("License: {license}."));
    }
    if let Some(external_docs) = &document.info.external_docs {
        append_text(&mut api_description, format!("See also: {external_docs}."));
    }
    if !document.tags.is_empty() {
        let tags = document
            .tags
            .iter()
            .map(|tag| {
                let mut label = tag.name.clone();
                if let Some(summary) = &tag.summary {
                    label.push_str(": ");
                    label.push_str(summary);
                }
                if let Some(description) = &tag.description {
                    label.push_str(" — ");
                    label.push_str(description);
                }
                if let Some(parent) = &tag.parent {
                    label.push_str(&format!(" (parent: {parent})"));
                }
                if let Some(kind) = &tag.kind {
                    label.push_str(&format!(" [{kind}]"));
                }
                label
            })
            .collect::<Vec<_>>()
            .join("; ");
        append_text(&mut api_description, format!("Tags: {tags}."));
    }
    let servers = document
        .servers
        .iter()
        .filter_map(|server| lower_server(server, ctx.diags))
        .collect();
    let api = Api {
        info: Info {
            title: document.info.title.clone(),
            version: document.info.version.clone(),
            description: api_description,
        },
        servers,
        operations,
        types: ctx.graph,
        security_schemes,
    };
    ctx.diags.result(api)
}

fn append_text(target: &mut Option<String>, text: String) {
    match target {
        Some(target) if !target.is_empty() => {
            target.push_str("\n\n");
            target.push_str(&text);
        }
        Some(target) => *target = text,
        None => *target = Some(text),
    }
}

fn append_response_docs(target: &mut Option<String>, status: &str, response: &ResponseObject) {
    let mut docs = response.summary.clone();
    if let Some(description) = &response.description {
        append_text(&mut docs, description.clone());
    }
    if let Some(docs) = docs {
        append_text(target, format!("Response `{status}`: {docs}"));
    }
}

struct LowerCtx<'a, 'doc> {
    document: &'doc Document,
    resolver: &'a Resolver<'doc>,
    diags: &'a mut Diagnostics,
    graph: TypeGraph,
    /// Lowered components, mapped to their root id and nullability. Nullability is carried so a
    /// `$ref` consumer wraps the type in `Option` when the component itself is nullable (a
    /// `"null"` in its type array, or a `null` enum/const member) — otherwise a null-mixed enum
    /// used via `$ref` would emit a non-`Option` field that rejects a conforming `null` payload.
    components: HashMap<String, (TypeId, bool)>,
    /// Components currently being lowered, mapped to the id reserved for their root and their
    /// nullability (computed at reserve time from the schema). A `$ref` that re-enters a name still
    /// in this map is a cycle-closing back-edge and is boxed against the reserved id, carrying the
    /// same nullability a completed lowering would.
    in_progress: HashMap<String, (TypeId, bool)>,
    /// Guards chains of component aliases (`A -> B -> A`) that do not have a concrete schema body
    /// to enter the normal reserve/box recursion path.
    component_alias_stack: HashSet<String>,
    /// The remote-`$ref` analogue of [`Self::components`], keyed by the absolute `url#fragment`. A
    /// remote ref resolves to a fresh owned schema each call, so — unlike local components — it has
    /// no `document`-level identity; this map gives it one, so repeated remote uses share one
    /// generated type and, together with [`Self::remote_in_progress`], recursion terminates.
    remote_components: HashMap<String, (TypeId, bool)>,
    /// Remote refs currently being lowered (same role as [`Self::in_progress`] for components): a
    /// re-entered `url#fragment` is a cycle-closing back-edge and is boxed against its reserved id.
    remote_in_progress: HashMap<String, (TypeId, bool)>,
    /// Guards a chain of bare-`$ref` (alias) remote documents so an alias cycle terminates instead
    /// of recursing forever; a real (object/enum/…) remote schema uses the reserve/box machinery.
    remote_alias_stack: HashSet<String>,
    /// Current schema-lowering recursion depth, incremented on entry to [`Self::lower_schema`] and
    /// decremented on exit. A `$ref`/allOf/array/object chain that pushes this past
    /// [`MAX_SCHEMA_DEPTH`] is rejected (`E014`) rather than allowed to overflow the stack.
    depth: u32,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    fn ensure_component(&mut self, name: &str) -> Option<Ty> {
        if let Some(&(id, nullable)) = self.components.get(name) {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        if let Some(&(id, nullable)) = self.in_progress.get(name) {
            // Re-entered while still lowering this component: a cycle-closing `$ref` back-edge.
            // Box the reference so the recursive type has a finite size instead of rejecting it;
            // the reserved id will hold the root def once the in-progress body finishes.
            return Some(Ty {
                id,
                nullable,
                boxed: true,
            });
        }
        let component = self.document.components.schemas.get(name)?;
        let RefOr::Item(schema) = component else {
            let reference = match component {
                RefOr::Ref(reference) => reference.clone(),
                RefOr::Item(_) => return None,
            };
            if !self.component_alias_stack.insert(name.to_owned()) {
                Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
                    .message(format!(
                        "schema component alias `{name}` forms a reference cycle"
                    ))
                    .emit(self.diags);
                return None;
            }
            let ty = if let Some(target) = reference.reference.strip_prefix("#/components/schemas/")
            {
                self.ensure_component(target)
            } else if is_remote_ref(&reference.reference) {
                self.ensure_remote(&reference.reference)
            } else {
                self.resolver
                    .resolve(&reference.reference, &reference.provenance, self.diags)
                    .ok()
                    .and_then(|resolved| self.lower_schema(&resolved.schema, name))
            };
            self.component_alias_stack.remove(name);
            if let Some(ty) = ty {
                self.components
                    .insert(name.to_owned(), (ty.id, ty.nullable));
            }
            return ty;
        };
        // Nullability is a pure function of the component's own schema — the same inputs
        // `lower_schema`/`lower_enum` use — so computing it once at reserve time lets every `$ref`
        // consumer (cache hit, back-edge, or fresh) agree on it without waiting for the body to
        // finish. No graph insert happens here, so the last-insert invariant below is preserved.
        let nullable = schema_is_nullable(schema);
        // Reserve the root id before lowering the body so any back-edge encountered mid-body can
        // box a reference to it. The root's def is inserted last (children first) and then lifted
        // into this reserved slot, which keeps ids dense and stable.
        let root_id = self.graph.reserve();
        self.in_progress
            .insert(name.to_owned(), (root_id, nullable));
        let lowered = self.lower_schema(schema, name);
        self.in_progress.remove(name);
        let mut ty = lowered?;
        let (popped_id, mut def) = self.graph.pop_last().expect("component root def");
        // Hard invariant (release too): a component root's def is always the last graph insert
        // during its own body lowering (children insert first). If future lowering (allOf/union
        // wrappers) ever inserts a derived type *after* the root, this fails loudly here instead
        // of silently relocating the wrong def and dangling `components[name]`.
        assert_eq!(
            popped_id, ty.id,
            "component root was not the last inserted def"
        );
        // A `default` on the component schema itself has no field to carry it; document it on the
        // named type's rustdoc rather than dropping it. (A component that is a bare `$ref`+`default`
        // never reaches here — it parses to `RefOr::Ref` and is acknowledged as W005 at parse time
        // — so this only sees inline component schemas.) Pure pop-then-mutate: no graph insert
        // happens here, so the last-insert invariant asserted above still holds.
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        // Use the reserve-time nullability consistently, so a direct return and a later cache hit
        // yield an identical `Ty` (it matches what the body lowering computed).
        ty.nullable = nullable;
        self.components.insert(name.to_owned(), (root_id, nullable));
        Some(ty)
    }

    /// Lower a remote (`http`/`https`) `$ref` to a shared, cycle-safe type — the remote analogue of
    /// [`Self::ensure_component`], keyed by the absolute `url#fragment`. Resolution is hermetic (the
    /// schema comes from the vendored, hash-pinned copy already in the bundle; no network). A remote
    /// ref re-entered while its own body is still lowering — a self- or mutually-recursive vendored
    /// schema — returns a boxed back-edge against the reserved root id, so recursion terminates and
    /// generates a finite (boxed) type instead of overflowing the stack.
    fn ensure_remote(&mut self, reference: &str) -> Option<Ty> {
        if let Some(&(id, nullable)) = self.remote_components.get(reference) {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        if let Some(&(id, nullable)) = self.remote_in_progress.get(reference) {
            return Some(Ty {
                id,
                nullable,
                boxed: true,
            });
        }
        let resolved = self
            .resolver
            .resolve(reference, &self.document.provenance, self.diags)
            .ok()?;
        let schema = resolved.schema.into_owned();

        // A vendored document that is itself a bare `$ref` is an alias with no body to reserve a
        // root for. Chain to its target under a cycle guard (so an alias loop terminates) rather
        // than through the reserve/pop machinery, which assumes the body inserts a fresh root.
        if schema.reference.is_some() {
            if !self.remote_alias_stack.insert(reference.to_owned()) {
                Diagnostic::error(Code::UnresolvedRef, self.document.provenance.clone())
                    .message(format!("remote $ref `{reference}` forms an alias cycle"))
                    .emit(self.diags);
                return None;
            }
            let ty = self.lower_schema(&schema, reference);
            self.remote_alias_stack.remove(reference);
            return ty;
        }

        let nullable = schema_is_nullable(&schema);
        let root_id = self.graph.reserve();
        self.remote_in_progress
            .insert(reference.to_owned(), (root_id, nullable));
        let lowered = self.lower_schema(&schema, reference);
        self.remote_in_progress.remove(reference);
        let mut ty = lowered?;
        let (popped_id, mut def) = self.graph.pop_last().expect("remote root def");
        // Same last-insert invariant as `ensure_component`: the remote type's root is the final
        // graph insert during its own body lowering (children insert first).
        assert_eq!(
            popped_id, ty.id,
            "remote root was not the last inserted def"
        );
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        ty.nullable = nullable;
        self.remote_components
            .insert(reference.to_owned(), (root_id, nullable));
        Some(ty)
    }

    fn lower_schema_or(&mut self, schema: &SchemaOr, hint: &str) -> Option<Ty> {
        match schema {
            SchemaOr::Bool(true) => {
                Some(self.insert_type(hint, TypeKind::Any, Docs::default(), None))
            }
            SchemaOr::Bool(false) => {
                Some(self.insert_type(hint, TypeKind::Never, Docs::default(), None))
            }
            SchemaOr::Schema(schema) => self.lower_schema(schema, hint),
        }
    }

    /// Depth-guarded entry to schema lowering. Bounds the `$ref`/allOf/array/object recursion to
    /// [`MAX_SCHEMA_DEPTH`] so a pathologically deep composition rejects with `E014` instead of
    /// exhausting the stack; the counter is decremented on every exit so sibling members (breadth)
    /// never accumulate against the cap.
    fn lower_schema(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if self.depth >= MAX_SCHEMA_DEPTH {
            Diagnostic::error(Code::SchemaNestingTooDeep, schema.provenance.clone())
                .message(format!(
                    "schema nesting exceeds the maximum lowering depth of {MAX_SCHEMA_DEPTH} \
                     (a very long `$ref` chain or a pathologically nested schema)"
                ))
                .remedy(
                    "flatten the offending schema chain, or omit this API segment with \
                     spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        self.depth += 1;
        let result = self.lower_schema_inner(schema, hint);
        self.depth -= 1;
        result
    }

    fn lower_schema_inner(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if let Some(value) = schema.boolean {
            let kind = if value {
                TypeKind::Any
            } else {
                TypeKind::Never
            };
            return Some(self.insert_schema_type(schema, hint, kind));
        }

        if let Some(reference) = &schema.reference {
            let referenced = if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                self.ensure_component(name)?
                // Remote refs go through the cycle-safe, deduped remote path (keyed by
                // `url#fragment`), mirroring `ensure_component`; a bare relative/other ref falls
                // through to `resolve`, which reports it (E003/E004).
            } else if is_remote_ref(reference) {
                self.ensure_remote(reference)?
            } else {
                let resolved = self
                    .resolver
                    .resolve(reference, &schema.provenance, self.diags)
                    .ok()?;
                self.lower_schema(&resolved.schema, hint)?
            };

            // In JSON Schema 2020-12 `$ref` is an applicator, not a replacement for the containing
            // schema. Intersect every shape-bearing sibling instead of silently discarding it.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                return Some(referenced);
            }
            let sibling = self.lower_schema(&sibling, &format!("{hint}Constraint"))?;
            let intersection =
                self.intersect_types(referenced, sibling, &format!("{hint}ReferenceIntersection"))?;
            let kind = self.graph.get(intersection.id)?.kind.clone();
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = intersection.nullable;
            ty.boxed = intersection.boxed;
            return Some(ty);
        }

        if !schema.all_of.is_empty() {
            return self.lower_all_of(schema, hint);
        }

        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            return self.lower_union(schema, hint);
        }

        if let Some(enumeration) = &schema.enum_values {
            return self.lower_enum(enumeration, schema, hint);
        }
        if let Some(value) = &schema.const_value {
            return self.lower_enum(std::slice::from_ref(value), schema, hint);
        }

        let non_null_types: Vec<JsonType> = schema
            .types
            .types
            .iter()
            .copied()
            .filter(|ty| *ty != JsonType::Null)
            .collect();
        if non_null_types.len() > 1 {
            return self.lower_type_array(schema, hint, &non_null_types);
        }

        // A binary payload — `contentEncoding: base64` or `format: binary` (the OpenAPI file/upload
        // marker) — lowers to raw `bytes::Bytes` rather than a `String`, so a multipart file part
        // carries bytes and a byte body is not misdecoded as UTF-8.
        if schema.content_encoding.as_deref() == Some("base64")
            || schema.format.as_deref() == Some("binary")
        {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Bytes));
        }

        let nullable = schema.types.types.contains(&JsonType::Null);
        let primary = schema
            .types
            .types
            .iter()
            .find(|ty| **ty != JsonType::Null)
            .copied();

        let mut ty = match primary {
            Some(JsonType::Boolean) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::Bool))
            }
            Some(JsonType::Integer) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("int32") => Prim::I32,
                    _ => Prim::I64,
                }),
            ),
            Some(JsonType::Number) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::F64))
            }
            Some(JsonType::String) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("uuid") => Prim::Uuid,
                    Some("date-time") => Prim::DateTime,
                    Some("date") => Prim::Date,
                    _ => Prim::String,
                }),
            ),
            Some(JsonType::Array) => {
                if !schema.prefix_items.is_empty() {
                    // `items` beside `prefixItems` is the 2020-12 rest-element schema. A Rust tuple
                    // is fixed-length, so a typed remainder is not representable — except
                    // `items: false`, which closes the array at the prefix and *is* a tuple.
                    if let Some(rest) = &schema.items {
                        if !matches!(rest.as_ref(), SchemaOr::Bool(false)) {
                            Diagnostic::error(
                                Code::TupleRestNotRepresentable,
                                schema.provenance.clone(),
                            )
                            .message(
                                "`items` beside `prefixItems` allows a typed variable-length \
                                 remainder, which no single Rust type expresses",
                            )
                            .remedy(
                                "use `items: false` to close the tuple, describe the whole array \
                                 with `items`, or omit this API segment with spargen::omit!",
                            )
                            .emit(self.diags);
                            return None;
                        }
                    }
                    let mut items = Vec::new();
                    for (index, child) in schema.prefix_items.iter().enumerate() {
                        items.push(self.lower_schema_or(child, &format!("{hint}Item{index}"))?);
                        self.warn_structural_default_or(child, "a tuple `prefixItems` entry");
                    }
                    self.insert_schema_type(schema, hint, TypeKind::Tuple(items))
                } else {
                    let mut item = match &schema.items {
                        Some(items) => {
                            let item = self.lower_schema_or(items, &format!("{hint}Item"))?;
                            self.warn_structural_default_or(items, "array `items`");
                            item
                        }
                        None => self.insert_type(
                            &format!("{hint}Item"),
                            TypeKind::Any,
                            Docs::default(),
                            None,
                        ),
                    };
                    // A `Vec` already provides the heap indirection that breaks a `$ref` cycle, so a
                    // back-edge closing through an array never needs its own `Box`.
                    item.boxed = false;
                    self.insert_schema_type(schema, hint, TypeKind::Array(Box::new(item)))
                }
            }
            Some(JsonType::Object) | None
                if !schema.properties.is_empty() || !schema.pattern_properties.is_empty() =>
            {
                self.lower_object(schema, hint)?
            }
            Some(JsonType::Object) => self.lower_object(schema, hint)?,
            Some(JsonType::Null) => self.insert_schema_type(schema, hint, TypeKind::Null),
            None if schema.types.types.contains(&JsonType::Null) => {
                self.insert_schema_type(schema, hint, TypeKind::Null)
            }
            None => self.insert_schema_type(schema, hint, TypeKind::Any),
        };
        ty.nullable = nullable;
        Some(ty)
    }

    fn lower_type_array(
        &mut self,
        schema: &Schema,
        hint: &str,
        non_null_types: &[JsonType],
    ) -> Option<Ty> {
        let mut branches = Vec::with_capacity(non_null_types.len());
        for ty in non_null_types {
            let mut branch = schema.clone();
            branch.types.types = vec![*ty];
            branch.title = None;
            branch.description = None;
            branches.push(SchemaOr::Schema(Box::new(branch)));
        }

        let mut union = schema.clone();
        union.boolean = None;
        union.reference = None;
        union.types.types.retain(|ty| *ty == JsonType::Null);
        union.properties.clear();
        union.required.clear();
        union.additional_properties = None;
        union.pattern_properties.clear();
        union.items = None;
        union.prefix_items.clear();
        union.all_of.clear();
        union.one_of.clear();
        union.any_of = branches;
        union.discriminator = None;
        union.enum_values = None;
        union.const_value = None;
        union.format = None;
        union.content_encoding = None;
        union.content_media_type = None;
        union.content_schema = None;
        union.xml = None;
        union.validation = ValidationKeywords::default();
        self.lower_union(&union, hint)
    }

    fn lower_object(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (fields, additional) = self.object_body(schema, hint)?;
        Some(self.insert_schema_type(
            schema,
            hint,
            TypeKind::Struct(Struct { fields, additional }),
        ))
    }

    /// Lower a `oneOf`/`anyOf` union. `null` members are stripped and make the union `nullable`
    /// (`Option<Union>`), exactly like a `"null"` in a type array; a 2-member union whose other
    /// member is null collapses to `Option<TheOtherType>` with no enum. The remaining variants are
    /// represented WITHOUT `serde(untagged)` and without degrading to `serde_json::Value`:
    ///
    /// * a `discriminator` dispatches object variants by tag and uniquely categorized non-object
    ///   variants by JSON category;
    /// * statically disjoint variants dispatch by JSON category or unique required key;
    /// * overlapping variants use typed trial matching with exact-one (`oneOf`) or deterministic
    ///   most-specific (`anyOf`) semantics, including serialization revalidation.
    ///
    /// Every variant type inserts before the union def, so the [`TypeKind::Union`] is the final
    /// graph insert — preserving the [`Self::ensure_component`] last-insert invariant when the union
    /// is a component body.
    fn lower_union(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (members, mode): (Vec<&SchemaOr>, UnionMode) =
            match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
                (false, true) => (schema.one_of.iter().collect(), UnionMode::OneOf),
                (true, false) => (schema.any_of.iter().collect(), UnionMode::AnyOf),
                (false, false) => {
                    return self.reject_union(
                    schema,
                    "a single schema node declares both `oneOf` and `anyOf`; their intersected \
                     applicator semantics are not representable as one generated union",
                );
                }
                (true, true) => unreachable!("lower_union is called only for a union schema"),
            };
        let sibling = self.lower_union_sibling(schema, hint)?;

        // A `"null"` in the enclosing type array, or a null-only member, makes the union nullable.
        let mut nullable = schema.types.types.contains(&JsonType::Null);
        let mut real_members: Vec<&SchemaOr> = Vec::new();
        for member in members {
            if member_is_null_only(member) {
                nullable = true;
            } else {
                real_members.push(member);
            }
        }

        // Only null members remained: the exact JSON null type.
        if real_members.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        // A single real member (the rest were null): `Option<ThatType>`, no enum needed. Re-emit the
        // member's kind as this position's own def so it is the final graph insert — mirroring the
        // allOf single-member collapse — which keeps the `ensure_component` last-insert invariant
        // when the union is a component body (a bare `$ref` member would otherwise return an existing
        // id and leave the popped root mismatched).
        if real_members.len() == 1 {
            let mut inner = self.lower_schema_or(real_members[0], hint)?;
            if let Some(sibling) = sibling {
                inner = self.intersect_types(inner, sibling, &format!("{hint}Constrained"))?;
            }
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = inner.nullable || nullable;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        // Lower every real variant first (their defs — especially `$ref` components — insert before
        // the union def below), recording the `$ref` component name for tag/variant naming.
        let mut variants: Vec<UnionVariant> = Vec::new();
        let mut ref_names: Vec<Option<String>> = Vec::new();
        let mut used_hints: HashSet<String> = HashSet::new();
        for (index, member) in real_members.iter().enumerate() {
            let (mut ty, ref_name) =
                self.lower_union_variant(member, &format!("{hint}Variant{index}"))?;
            if let Some(sibling) = sibling {
                let Some(intersection) =
                    self.intersect_types(ty, sibling, &format!("{hint}Variant{index}Constrained"))
                else {
                    // The sibling constraints make this branch impossible; JSON Schema simply
                    // removes it from the union's accepted set. Acknowledge it, because a variant
                    // vanishing from the generated enum is otherwise invisible.
                    Diagnostic::warning(Code::DeclarationHasNoEffect, schema.provenance.clone())
                        .message(format!(
                            "union member {index} cannot satisfy the enclosing schema's own \
                             constraints, so it is not a variant of the generated enum"
                        ))
                        .emit(self.diags);
                    continue;
                };
                ty = intersection;
            }
            // Hoist a variant's own nullability up to the union: a `null` payload then resolves at the
            // outer `Option<Union>` (→ `None`), and the discriminated/disjoint dispatch below only
            // ever inspects non-null content — otherwise a variant like `{type: [string, null]}`
            // would be categorized `String` yet have no `null` arm in the custom `Deserialize`.
            nullable = nullable || ty.nullable;
            ty.nullable = false;
            let base_hint = ref_name
                .clone()
                .unwrap_or_else(|| format!("{hint}Variant{index}"));
            // Keep hints unique so `name` allocates one identifier per variant (the hint keys the
            // per-union variant table).
            let mut name_hint = base_hint.clone();
            let mut disambiguator = 2usize;
            while !used_hints.insert(name_hint.clone()) {
                name_hint = format!("{base_hint}{disambiguator}");
                disambiguator += 1;
            }
            variants.push(UnionVariant { name_hint, ty });
            ref_names.push(ref_name);
        }

        if variants.is_empty() {
            return self.reject_union(
                schema,
                "union sibling constraints make every variant impossible",
            );
        }
        if variants.len() == 1 {
            let inner = variants[0].ty;
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = inner.nullable || nullable;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        let strategy = if let Some(discriminator) = &schema.discriminator {
            // A `defaultMapping` that names a schema outside this union describes a fallback
            // branch the generated enum does not have, so it cannot be quietly downgraded to
            // another dispatch strategy.
            if let Some(target) = &discriminator.default_mapping {
                let bare = target
                    .strip_prefix("#/components/schemas/")
                    .unwrap_or(target);
                if !ref_names.iter().any(|name| name.as_deref() == Some(bare)) {
                    return self.reject_union(
                        schema,
                        &format!(
                            "`discriminator.defaultMapping` names `{target}`, which is not one of \
                             this union's members, so there is no branch to fall back to"
                        ),
                    );
                }
            }
            self.discriminated_strategy(&variants, &ref_names, discriminator)
                .or_else(|| self.disjoint_strategy(&variants))
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        } else {
            self.disjoint_strategy(&variants)
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        };

        let mut ty =
            self.insert_schema_type(schema, hint, TypeKind::Union(Union { variants, strategy }));
        ty.nullable = nullable;
        Some(ty)
    }

    /// Lower shape-bearing keywords adjacent to `oneOf`/`anyOf` so every branch is intersected with
    /// them. A multi-non-null `type` array is already expressed by the union members and is removed
    /// here (its `null` member is handled by the union's outer nullability).
    fn lower_union_sibling(&mut self, schema: &Schema, hint: &str) -> Option<Option<Ty>> {
        let mut sibling = schema.clone();
        sibling.one_of.clear();
        sibling.any_of.clear();
        sibling.discriminator = None;
        let non_null_types = sibling
            .types
            .types
            .iter()
            .filter(|kind| **kind != JsonType::Null)
            .count();
        if non_null_types > 1 {
            sibling.types.types.clear();
        }
        if !schema_has_shape_constraint(&sibling) {
            return Some(None);
        }
        self.lower_schema(&sibling, &format!("{hint}Constraint"))
            .map(Some)
    }

    /// Lower one union member, returning its type and — when the member is a `$ref` to a component —
    /// that component's name (used to derive the variant name and implicit discriminator tag).
    fn lower_union_variant(
        &mut self,
        member: &SchemaOr,
        hint: &str,
    ) -> Option<(Ty, Option<String>)> {
        if let SchemaOr::Schema(schema) = member {
            if let Some(reference) = &schema.reference {
                if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                    let ty = self.ensure_component(name)?;
                    return Some((ty, Some(name.to_owned())));
                }
            }
        }
        let ty = self.lower_schema_or(member, hint)?;
        Some((ty, None))
    }

    /// Build the discriminated fast path. Objects route by tag; a non-object variant routes by its
    /// unique JSON category. The tag value comes from `discriminator.mapping` (matched by `$ref`)
    /// when present, otherwise from the variant's own `$ref` component name.
    fn discriminated_strategy(
        &self,
        variants: &[UnionVariant],
        ref_names: &[Option<String>],
        discriminator: &super::Discriminator,
    ) -> Option<UnionStrategy> {
        let mut tags = Vec::new();
        let mut categories = Vec::new();
        for (variant, ref_name) in variants.iter().zip(ref_names) {
            if !matches!(
                self.graph.get(variant.ty.id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            ) {
                let category = self.json_category(variant.ty)?;
                if category == JsonCategory::Object || categories.contains(&Some(category)) {
                    return None;
                }
                tags.push(None);
                categories.push(Some(category));
                continue;
            }
            // Prefer an explicit mapping entry that points at this variant's component; fall back to
            // the component name (implicit mapping). A mapping value may be a bare name or a full
            // `#/components/schemas/Name` pointer.
            let tag = ref_name
                .as_ref()
                .and_then(|name| {
                    discriminator
                        .mapping
                        .iter()
                        .find(|(_, target)| {
                            target.as_str() == name
                                || target.strip_prefix("#/components/schemas/") == Some(name)
                        })
                        .map(|(key, _)| key.clone())
                        .or_else(|| Some(name.clone()))
                })
                .unwrap_or_else(|| variant.name_hint.clone());
            tags.push(Some(tag));
            categories.push(None);
        }
        // 3.2 `defaultMapping` names the schema to fall back to when the tag is absent or
        // unrecognized. It must name one of this union's own variants; anything else describes a
        // branch that does not exist.
        let default_variant = match &discriminator.default_mapping {
            None => None,
            Some(target) => {
                let bare = target
                    .strip_prefix("#/components/schemas/")
                    .unwrap_or(target);
                // A fallback naming a non-member is rejected by the caller, which owns the
                // union's provenance; here it simply means there is no discriminated strategy.
                Some(
                    ref_names
                        .iter()
                        .position(|name| name.as_deref() == Some(bare))?,
                )
            }
        };
        Some(UnionStrategy::Discriminated {
            tag_field: discriminator.property_name.clone(),
            tags,
            categories,
            default_variant,
        })
    }

    /// Build the disjoint fast path for an undiscriminated union. Two proofs are attempted:
    ///
    /// 1. **JSON-type-disjoint**: every variant occupies a distinct JSON primitive category
    ///    (`number` and `integer` share one category, so they never separate).
    /// 2. **Required-key-disjoint**: every variant is a *closed* object (`additionalProperties:
    ///    false`) with at least one required property whose name appears in no other variant. Closed
    ///    is essential — an open object could carry another variant's unique key as an extra field
    ///    and be misrouted, so open-object required-key unions are never provably disjoint.
    fn disjoint_strategy(&self, variants: &[UnionVariant]) -> Option<UnionStrategy> {
        // Proof 1: pairwise-distinct JSON type categories.
        let categories: Option<Vec<JsonCategory>> =
            variants.iter().map(|v| self.json_category(v.ty)).collect();
        if let Some(categories) = categories {
            let all_distinct = categories.iter().enumerate().all(|(i, cat)| {
                categories
                    .iter()
                    .enumerate()
                    .all(|(j, other)| i == j || cat != other)
            });
            if all_distinct {
                return Some(UnionStrategy::Disjoint {
                    features: categories
                        .into_iter()
                        .map(DisjointFeature::JsonType)
                        .collect(),
                });
            }
        }

        // Proof 2: object variants each carrying a unique required key.
        if let Some(keys) = self.required_key_features(variants) {
            return Some(UnionStrategy::Disjoint {
                features: keys.into_iter().map(DisjointFeature::RequiredKey).collect(),
            });
        }

        None
    }

    fn trial_strategy(&self, variants: &[UnionVariant], mode: UnionMode) -> UnionStrategy {
        UnionStrategy::Trial {
            mode,
            priorities: variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, &mut HashSet::new()))
                .collect(),
        }
    }

    fn type_specificity(&self, ty: Ty, visiting: &mut HashSet<TypeId>) -> u32 {
        if !visiting.insert(ty.id) {
            return 0;
        }
        let priority = match self.graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Enum(enumeration)) => {
                2_000_u32.saturating_sub(enumeration.variants.len() as u32)
            }
            Some(TypeKind::Null) => 3_000,
            Some(TypeKind::Never) => 4_000,
            Some(TypeKind::Struct(object)) => {
                let required = object.fields.iter().filter(|field| field.required).count() as u32;
                1_000 + required * 20 + object.fields.len() as u32
            }
            Some(TypeKind::Tuple(items)) => 900 + items.len() as u32,
            Some(TypeKind::Array(item)) => 800 + self.type_specificity(**item, visiting) / 10,
            Some(TypeKind::Primitive(Prim::I32)) => 700,
            Some(TypeKind::Primitive(Prim::I64)) => 650,
            Some(TypeKind::Primitive(Prim::Uuid | Prim::DateTime | Prim::Date)) => 600,
            Some(TypeKind::Primitive(Prim::F64 | Prim::String | Prim::Bool) | TypeKind::Bytes) => {
                500
            }
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, visiting))
                .min()
                .unwrap_or(0),
            Some(TypeKind::Any) | None => 0,
        };
        visiting.remove(&ty.id);
        priority
    }

    /// The JSON primitive category a lowered variant type serializes as, or `None` when it cannot be
    /// statically categorized (an untyped `Any`, raw `Bytes`, or a nested union).
    fn json_category(&self, ty: Ty) -> Option<JsonCategory> {
        Some(match &self.graph.get(ty.id)?.kind {
            TypeKind::Primitive(Prim::Bool) => JsonCategory::Boolean,
            TypeKind::Primitive(Prim::I32 | Prim::I64 | Prim::F64) => JsonCategory::Number,
            TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date) => {
                JsonCategory::String
            }
            TypeKind::Struct(_) => JsonCategory::Object,
            TypeKind::Array(_) | TypeKind::Tuple(_) => JsonCategory::Array,
            TypeKind::Enum(enumeration) => match enumeration.repr {
                ScalarRepr::String => JsonCategory::String,
                ScalarRepr::Int => JsonCategory::Number,
                ScalarRepr::Bool => JsonCategory::Boolean,
            },
            TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any
            | TypeKind::Union(_) => return None,
        })
    }

    /// If every variant lowers to a *closed* object (`additionalProperties: false`) with at least
    /// one required property whose name appears in no other variant, return that unique required key
    /// per variant (source order); else `None`. Closed is required for soundness: an open object
    /// could carry another variant's unique key as an extra field, misrouting the payload.
    fn required_key_features(&self, variants: &[UnionVariant]) -> Option<Vec<String>> {
        let structs: Option<Vec<&Struct>> = variants
            .iter()
            .map(|v| match &self.graph.get(v.ty.id)?.kind {
                // Only closed objects are sound discriminators by required-key presence.
                TypeKind::Struct(structure)
                    if matches!(structure.additional, AdditionalProps::Deny) =>
                {
                    Some(structure)
                }
                _ => None,
            })
            .collect();
        let structs = structs?;
        let mut keys = Vec::new();
        for (index, structure) in structs.iter().enumerate() {
            let others: HashSet<&str> = structs
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .flat_map(|(_, s)| s.fields.iter().map(|f| f.name.wire.as_str()))
                .collect();
            let key = structure
                .fields
                .iter()
                .find(|field| field.required && !others.contains(field.name.wire.as_str()))?;
            keys.push(key.name.wire.clone());
        }
        Some(keys)
    }

    fn reject_union<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "add a discriminator, restructure the variants to be disjoint, or omit this API \
                 segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// Lower an object schema's `properties`/`required`/`additionalProperties` into the pieces of a
    /// [`Struct`] *without* inserting the struct itself. Shared by [`Self::lower_object`] and the
    /// `allOf` merge, which collects field/additional pieces from several members before inserting a
    /// single merged struct as the final graph insert (the `ensure_component` last-insert invariant).
    fn object_body(
        &mut self,
        schema: &Schema,
        hint: &str,
    ) -> Option<(Vec<Field>, AdditionalProps)> {
        let required = schema.required.iter().cloned().collect::<HashSet<_>>();
        let mut fields = Vec::new();
        for (name, child) in &schema.properties {
            let ty = self.lower_schema_or(child, &format!("{hint}{name}"))?;
            let is_required = required.contains(name);
            let default = self.field_default(child, ty, is_required);
            let xml = self.field_xml(child);
            let (deprecated, read_only, write_only) = field_flags(child);
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: is_required,
                deprecated,
                read_only,
                write_only,
                default,
                xml,
            });
        }
        let additional = if schema.pattern_properties.is_empty() {
            match &schema.additional_properties {
                Some(schema) => match schema.as_ref() {
                    SchemaOr::Bool(false) => AdditionalProps::Deny,
                    SchemaOr::Bool(true) => AdditionalProps::Allow,
                    schema => {
                        let mut ty = self.lower_schema_or(schema, &format!("{hint}Additional"))?;
                        self.warn_structural_default_or(schema, "an `additionalProperties` value");
                        // A map value lives behind the map's own indirection; a cycle-closing ref
                        // here needs no `Box`.
                        ty.boxed = false;
                        AdditionalProps::Typed(Box::new(ty))
                    }
                },
                None => AdditionalProps::Allow,
            }
        } else {
            self.lower_pattern_additional(schema, hint)?
        };
        Some((fields, additional))
    }

    /// Lower a property's OpenAPI `xml` hints into the field's [`XmlField`].
    ///
    /// `xml.name` and `xml.attribute` are represented (applied as a serde rename at emit time).
    /// The hints that change the XML wire without a faithful quick-xml mapping are recorded here
    /// and dispositioned in [`gate_xml_field_renames`], once it is known whether the owning type is
    /// ever serialized as XML at all. A `$ref` property carries no inline `xml` object here.
    fn field_xml(&mut self, child: &SchemaOr) -> XmlField {
        let SchemaOr::Schema(schema) = child else {
            return XmlField::default();
        };
        let Some(hints) = &schema.xml else {
            return XmlField::default();
        };
        let mut unsupported: Vec<String> = Vec::new();
        if hints.namespace.is_some() {
            unsupported.push("namespace".to_owned());
        }
        if hints.prefix.is_some() {
            unsupported.push("prefix".to_owned());
        }
        if hints.wrapped {
            unsupported.push("wrapped".to_owned());
        }
        // OpenAPI 3.2 replaced the `attribute`/`wrapped` flags with `nodeType`, and gave it a
        // *defaulting table*: a `$ref` node and a `type: array` schema default to `none`, and
        // everything else to `element`. Reading the field as a plain string match misses that,
        // which is how the two spellings of one construct came to disagree — `wrapped: true` was
        // rejected while its exact 3.2 equivalent, `nodeType: element` on an array, was waved
        // through and put unwrapped XML on the wire.
        //
        // `none` on a node that defaults to `none` is the default restated: it is a genuine no-op
        // and takes no disposition. Anywhere else it deletes a node from the wire, so it joins
        // `text`/`cdata` and any token outside the enumeration (the document schema does not
        // validate Schema Objects, so unknown tokens do reach here).
        let is_array = schema.types.types.contains(&JsonType::Array);
        let defaults_to_none = schema.reference.is_some() || is_array;
        let effective =
            hints
                .node_type
                .as_deref()
                .unwrap_or(if defaults_to_none { "none" } else { "element" });
        let node_type_unsupported = match effective {
            "attribute" => false,
            // On an array this is precisely `wrapped: true` — it asks for an element wrapping the
            // list, which is the representation quick-xml does not give us. On a `$ref` it names
            // the element the referenced component already produces.
            "element" => is_array,
            "none" => !defaults_to_none,
            _ => true,
        };
        if node_type_unsupported {
            unsupported.push("nodeType".to_owned());
        }
        XmlField {
            name: hints.name.clone(),
            attribute: hints.attribute,
            unsupported,
        }
    }

    /// Merge an `allOf` composition (plus the enclosing schema's own sibling
    /// `properties`/`required`/`additionalProperties`) into a single typed [`TypeKind`].
    ///
    /// Members are gathered in a deterministic order — every `allOf` entry in source order, then the
    /// enclosing schema's own object siblings — flattening `$ref` members by *copying* their fields
    /// (the referenced component still exists as its own named type) and recursing into nested
    /// `allOf`. The gathered members are then combined:
    ///
    /// * **all object members** → one flattened [`Struct`]: the union of properties in first-seen
    ///   order, recursive typed intersections for properties declared by several members, the union
    ///   of `required`, and a conservatively intersected `additionalProperties` policy;
    /// * **all scalar members** → their typed intersection, including numeric narrowing, enum
    ///   narrowing, arrays/objects/unions, and exact nullability; an empty intersection → `E013`;
    /// * an **object/scalar mix** → `E013`.
    ///
    /// Every path inserts its result type as the *final* graph insert (all member/property/component
    /// types insert first), so an `allOf` used as a component body still satisfies the
    /// [`Self::ensure_component`] last-insert invariant.
    fn lower_all_of(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let mut contributions = Vec::new();
        self.gather_all_of(schema, hint, &mut contributions)?;

        let has_object = contributions
            .iter()
            .any(|c| matches!(c, Contribution::Object { .. }));
        let scalars: Vec<Ty> = contributions
            .iter()
            .filter_map(|c| match c {
                Contribution::Scalar(ty) => Some(*ty),
                Contribution::Object { .. } => None,
            })
            .collect();

        // Object-vs-scalar mix has no single representable type.
        if has_object && !scalars.is_empty() {
            return self.reject_all_of(
                schema,
                "an `allOf` mixes object and scalar members, which cannot form one type",
            );
        }

        // All-scalar allOf: recursively intersect compatible members (for example integer with
        // number, an enum with its underlying scalar, or arrays whose item constraints narrow).
        if !has_object {
            let Some(mut intersection) = scalars.first().copied() else {
                // Only no-constraint members (`true`/`{}`) remained: a faithful open object.
                let ty = self.insert_schema_type(
                    schema,
                    hint,
                    TypeKind::Struct(Struct {
                        fields: Vec::new(),
                        additional: AdditionalProps::Allow,
                    }),
                );
                return Some(self.with_all_of_nullability(schema, ty));
            };
            for (index, member) in scalars.iter().copied().enumerate().skip(1) {
                let Some(merged) = self.intersect_types(
                    intersection,
                    member,
                    &format!("{hint}Intersection{index}"),
                ) else {
                    return self.reject_all_of(
                        schema,
                        "`allOf` scalar members have an empty or unrepresentable intersection",
                    );
                };
                intersection = merged;
            }
            // Re-emit the intersection as the final graph insert so the invariant holds even when
            // the allOf is a component body (the per-member scalar inserts above are left dead —
            // `#[allow(dead_code)]` on the models module — rather than threading a reserved id).
            let kind = self
                .graph
                .get(intersection.id)
                .map(|def| def.kind.clone())?;
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = intersection.nullable;
            return Some(self.with_all_of_nullability(schema, ty));
        }

        // All object members: flatten into one struct. Property union preserves first-seen order.
        let mut fields: IndexMap<String, Field> = IndexMap::new();
        let mut required: Vec<String> = Vec::new();
        let mut additional = AdditionalProps::Allow;
        for contribution in &contributions {
            let Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: member_required,
            } = contribution
            else {
                continue;
            };
            for name in member_required {
                if !required.contains(name) {
                    required.push(name.clone());
                }
            }
            match self.merge_additional(
                &additional,
                member_additional,
                &format!("{hint}Additional"),
            ) {
                Some(merged) => additional = merged,
                None => {
                    return self.reject_all_of(
                        schema,
                        "`allOf` members declare conflicting `additionalProperties`",
                    );
                }
            }
            for field in member_fields {
                match fields.get_mut(&field.name.wire) {
                    Some(existing) => {
                        // A repeated property is an intersection, not an equality assertion: retain
                        // the narrower compatible type and reject only an empty/unrepresentable
                        // intersection.
                        let Some(intersection) = self.intersect_types(
                            existing.ty,
                            field.ty,
                            &format!("{hint}{}Intersection", field.name.wire),
                        ) else {
                            let message = format!(
                                "property `{}` appears in multiple `allOf` members with \
                                 conflicting types",
                                field.name.wire
                            );
                            return self.reject_all_of(schema, &message);
                        };
                        existing.ty = intersection;
                        existing.required = existing.required || field.required;
                    }
                    None => {
                        fields.insert(field.name.wire.clone(), field.clone());
                    }
                }
            }
        }

        // Apply the required union, then keep required fields consistent: a serde default only fires
        // for an absent optional field, so a field promoted to required by another member drops its
        // applied default (it stays documented in rustdoc).
        let mut fields: Vec<Field> = fields.into_values().collect();
        for field in &mut fields {
            if required.contains(&field.name.wire) {
                field.required = true;
            }
            if field.required {
                if let Some(default) = &mut field.default {
                    default.applied = None;
                }
            }
        }

        let ty = self.insert_schema_type(
            schema,
            hint,
            TypeKind::Struct(Struct { fields, additional }),
        );
        Some(self.with_all_of_nullability(schema, ty))
    }

    /// Gather every member of `schema.all_of` (source order) plus the enclosing schema's own object
    /// siblings (last), pushing a [`Contribution`] per constraining member.
    fn gather_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        for (index, member) in schema.all_of.iter().enumerate() {
            self.gather_member(member, &format!("{hint}Member{index}"), out)?;
        }
        // The enclosing schema may carry its own object keywords beside `allOf`; fold them in last.
        if schema_is_object_like(schema) {
            let (member_fields, member_additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: schema.required.clone(),
            });
        }
        Some(())
    }

    fn gather_member(
        &mut self,
        member: &SchemaOr,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        let schema = match member {
            // A `true`/`{}` member imposes no constraint.
            SchemaOr::Bool(true) => return Some(()),
            SchemaOr::Bool(false) => {
                return self
                    .reject_all_of_unit(member_provenance(member), "an `allOf` member is `false`");
            }
            SchemaOr::Schema(schema) => schema.as_ref(),
        };

        if let Some(reference) = &schema.reference {
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                // A `$ref` to a component still being lowered is a direct recursive allOf member
                // whose fields are not yet known — irreconcilable (distinct from a member with
                // recursive *fields*, which lowers fine).
                if self.in_progress.contains_key(name) {
                    return self.reject_all_of_unit(
                        schema.provenance.clone(),
                        "an `allOf` member is a direct recursive `$ref` to the component being \
                         lowered",
                    );
                }
                let ty = self.ensure_component(name)?;
                self.push_ref_member(ty, out);
                return Some(());
            }
            // A remote `$ref` member goes through the cycle-safe remote path, exactly like a
            // component member: a member still being lowered is a direct recursive ref whose fields
            // are not yet known (irreconcilable), otherwise its shared type contributes its fields.
            if is_remote_ref(reference) {
                if self.remote_in_progress.contains_key(reference) {
                    return self.reject_all_of_unit(
                        schema.provenance.clone(),
                        "an `allOf` member is a direct recursive remote `$ref` to the schema being \
                         lowered",
                    );
                }
                let ty = self.ensure_remote(reference)?;
                self.push_ref_member(ty, out);
                return Some(());
            }
            // Non-component refs resolve (or error) exactly as `lower_schema` does; treat the target
            // as an inline member.
            let resolved = self
                .resolver
                .resolve(reference, &schema.provenance, self.diags)
                .ok()?;
            let target = resolved.schema.into_owned();
            return self.gather_inline(&target, hint, out);
        }

        if !schema.all_of.is_empty() {
            // Nested allOf: flatten its members (and its own siblings) into the same accumulator.
            return self.gather_all_of(schema, hint, out);
        }

        self.gather_inline(schema, hint, out)
    }

    /// Turn a resolved `$ref` member's already-lowered type into a contribution: an object component
    /// contributes a *copy* of its fields/`additionalProperties`; any other kind is a scalar member.
    fn push_ref_member(&mut self, ty: Ty, out: &mut Vec<Contribution>) {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Struct(structure)) => {
                let fields = structure.fields.clone();
                let required = fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.name.wire.clone())
                    .collect();
                let additional = structure.additional.clone();
                out.push(Contribution::Object {
                    fields,
                    additional,
                    required,
                });
            }
            _ => out.push(Contribution::Scalar(ty)),
        }
    }

    fn gather_inline(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        if schema_is_object_like(schema) {
            let (fields, additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields,
                additional,
                required: schema.required.clone(),
            });
        } else if schema_imposes_scalar(schema) {
            let ty = self.lower_schema(schema, hint)?;
            out.push(Contribution::Scalar(ty));
        }
        // Otherwise the member is a pure annotation (`{description: ...}`): no constraint.
        Some(())
    }

    /// Merge two `additionalProperties` policies for an `allOf` intersection. `Deny` dominates (a
    /// value must satisfy every member, so any member denying unknown keys forbids them outright);
    /// two typed value schemas must lower to the same type. Returns `None` when irreconcilable.
    fn merge_additional(
        &mut self,
        acc: &AdditionalProps,
        next: &AdditionalProps,
        hint: &str,
    ) -> Option<AdditionalProps> {
        Some(match (acc, next) {
            (AdditionalProps::Deny, _) | (_, AdditionalProps::Deny) => AdditionalProps::Deny,
            (AdditionalProps::Typed(x), AdditionalProps::Typed(y)) => {
                let intersection = self.intersect_types(**x, **y, hint)?;
                AdditionalProps::Typed(Box::new(intersection))
            }
            (AdditionalProps::Typed(x), AdditionalProps::Allow)
            | (AdditionalProps::Allow, AdditionalProps::Typed(x)) => {
                AdditionalProps::Typed(x.clone())
            }
            (AdditionalProps::Allow, AdditionalProps::Allow) => AdditionalProps::Allow,
        })
    }

    /// Apply the enclosing `allOf` schema's own nullability (a `"null"` in its type array) to the
    /// merged type. Set after the final insert — a pure mutate that preserves the last-insert
    /// invariant.
    fn with_all_of_nullability(&self, schema: &Schema, mut ty: Ty) -> Ty {
        if schema.types.types.contains(&JsonType::Null) {
            ty.nullable = true;
        }
        ty
    }

    fn reject_all_of(&mut self, schema: &Schema, message: &str) -> Option<Ty> {
        self.reject_all_of_unit(schema.provenance.clone(), message);
        None
    }

    fn reject_all_of_unit(
        &mut self,
        provenance: crate::diag::Provenance,
        message: &str,
    ) -> Option<()> {
        Diagnostic::error(Code::AllOfIrreconcilable, provenance)
            .message(message.to_owned())
            .remedy(
                "restructure the composition so members agree, or omit this API segment with \
                 spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// Lower the overflow policy for an object that declares `patternProperties`. The generated
    /// struct captures every non-declared property into a single `#[serde(flatten)]` typed map, so
    /// every `patternProperties` value schema — together with a typed `additionalProperties` value,
    /// if any — must lower to the *same emitted Rust type*; otherwise a single map cannot type them.
    ///
    /// Homogeneity is decided by [`Self::same_map_value_type`], a bounded structural equivalence:
    /// same `TypeId` (a shared `$ref`, or the single-entry case) is homogeneous, and distinct inline
    /// leaf shapes (primitives, `Bytes`, `Any`, or arrays thereof) that emit the identical Rust type
    /// collapse to one map — so `{type:string}` under two patterns yields one `BTreeMap<String,
    /// String>`. Distinct inline composites (`Struct`/`Enum`/`Tuple`) stay heterogeneous and are
    /// rejected (`E005`), since two different object shapes cannot share one map value type. The
    /// first collected value type is used as the map's value type. Deterministic (graph lookups by
    /// `TypeId`, source-order collection) and bounded (recurses only through `Array` elements).
    fn lower_pattern_additional(&mut self, schema: &Schema, hint: &str) -> Option<AdditionalProps> {
        // `additionalProperties: false` denies unknown keys, but the flatten map must capture the
        // pattern-matched keys (which are themselves "unknown" to the named fields). Serde cannot do
        // both, so this combination has no faithful representation.
        if matches!(
            schema.additional_properties.as_deref(),
            Some(SchemaOr::Bool(false))
        ) {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties` combined with `additionalProperties: false` cannot be \
                     represented: a flatten map captures pattern values but cannot also deny other \
                     unknown keys",
                )
                .remedy(
                    "drop `additionalProperties: false`, or omit this API segment with \
                     spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        // Collect the value types in deterministic source order: patternProperties entries first
        // (IndexMap preserves source order), then a typed `additionalProperties` value if present.
        let mut value_types: Vec<Ty> = Vec::new();
        for (_pattern, child) in &schema.pattern_properties {
            let ty = self.lower_schema_or(child, &format!("{hint}Value"))?;
            self.warn_structural_default_or(child, "a `patternProperties` value");
            value_types.push(ty);
        }
        if let Some(additional) = schema.additional_properties.as_deref() {
            // `true`/absent leave unknown non-pattern keys unconstrained; the typed map still stands
            // in for the overflow. Only a schema value adds another type that must agree.
            if !matches!(additional, SchemaOr::Bool(_)) {
                let ty = self.lower_schema_or(additional, &format!("{hint}Additional"))?;
                self.warn_structural_default_or(additional, "an `additionalProperties` value");
                value_types.push(ty);
            }
        }

        let first = value_types[0];
        if value_types
            .iter()
            .any(|ty| !self.same_map_value_type(first, *ty))
        {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties`/`additionalProperties` value schemas lower to different \
                     types; a single typed overflow map cannot represent them all",
                )
                .remedy(
                    "make every pattern/additional value the same type (e.g. a shared `$ref` or the \
                     same primitive), or omit this API segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        let mut ty = first;
        // A map value lives behind the map's own indirection; a cycle-closing ref needs no `Box`.
        ty.boxed = false;
        Some(AdditionalProps::Typed(Box::new(ty)))
    }

    /// Compute a typed intersection for two already-lowered schemas. Nullability is intersected
    /// independently from the non-null shape; an intersection containing only JSON `null` becomes
    /// [`TypeKind::Null`]. Derived arrays, objects, enums, and narrowed unions are inserted into the
    /// graph so codegen still sees an ordinary, fully typed IR node.
    fn intersect_types(&mut self, a: Ty, b: Ty, hint: &str) -> Option<Ty> {
        let a_kind = self.graph.get(a.id)?.kind.clone();
        let b_kind = self.graph.get(b.id)?.kind.clone();
        let accepts_null = type_accepts_null(a, &a_kind) && type_accepts_null(b, &b_kind);

        let non_null = if matches!(a_kind, TypeKind::Null) || matches!(b_kind, TypeKind::Null) {
            None
        } else {
            self.intersect_non_null(a, &a_kind, b, &b_kind, hint)
        };

        match non_null {
            Some(mut ty) => {
                ty.nullable = accepts_null;
                Some(ty)
            }
            None if accepts_null => {
                Some(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
            }
            None => None,
        }
    }

    fn intersect_non_null(
        &mut self,
        a: Ty,
        a_kind: &TypeKind,
        b: Ty,
        b_kind: &TypeKind,
        hint: &str,
    ) -> Option<Ty> {
        if a.id == b.id {
            let mut ty = a;
            ty.nullable = false;
            ty.boxed = a.boxed || b.boxed;
            return Some(ty);
        }

        match (a_kind, b_kind) {
            (TypeKind::Any, _) => Some(non_nullable(b)),
            (_, TypeKind::Any) => Some(non_nullable(a)),
            (TypeKind::Primitive(left), TypeKind::Primitive(right)) => {
                let primitive = intersect_primitives(*left, *right)?;
                if primitive == *left {
                    Some(non_nullable(a))
                } else if primitive == *right {
                    Some(non_nullable(b))
                } else {
                    Some(self.insert_type(
                        hint,
                        TypeKind::Primitive(primitive),
                        Docs::default(),
                        None,
                    ))
                }
            }
            (TypeKind::Enum(left), TypeKind::Enum(right)) if left.repr == right.repr => {
                let variants: Vec<ScalarValue> = left
                    .variants
                    .iter()
                    .filter(|value| right.variants.contains(value))
                    .cloned()
                    .collect();
                if variants.is_empty() {
                    None
                } else if variants == left.variants {
                    Some(non_nullable(a))
                } else if variants == right.variants {
                    Some(non_nullable(b))
                } else {
                    Some(self.insert_type(
                        hint,
                        TypeKind::Enum(ScalarEnum {
                            repr: left.repr,
                            variants,
                        }),
                        Docs::default(),
                        None,
                    ))
                }
            }
            (TypeKind::Enum(enumeration), TypeKind::Primitive(primitive))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Some(non_nullable(a))
            }
            (TypeKind::Primitive(primitive), TypeKind::Enum(enumeration))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Some(non_nullable(b))
            }
            (TypeKind::Array(left), TypeKind::Array(right)) => {
                let item_hint = format!("{hint}Item");
                let item = self
                    .intersect_types(**left, **right, &item_hint)
                    .unwrap_or_else(|| {
                        self.insert_type(&item_hint, TypeKind::Never, Docs::default(), None)
                    });
                if same_ty(item, **left) {
                    Some(non_nullable(a))
                } else if same_ty(item, **right) {
                    Some(non_nullable(b))
                } else {
                    Some(self.insert_type(
                        hint,
                        TypeKind::Array(Box::new(item)),
                        Docs::default(),
                        None,
                    ))
                }
            }
            (TypeKind::Tuple(left), TypeKind::Tuple(right)) if left.len() == right.len() => {
                let items = left
                    .iter()
                    .zip(right)
                    .enumerate()
                    .map(|(index, (left, right))| {
                        self.intersect_types(*left, *right, &format!("{hint}Item{index}"))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(self.insert_type(hint, TypeKind::Tuple(items), Docs::default(), None))
            }
            (TypeKind::Struct(left), TypeKind::Struct(right)) => {
                self.intersect_structs(left, right, hint)
            }
            (TypeKind::Union(union), _) => self.intersect_union(a, union, b, hint),
            (_, TypeKind::Union(union)) => self.intersect_union(b, union, a, hint),
            (TypeKind::Bytes, TypeKind::Bytes) => Some(non_nullable(a)),
            _ => None,
        }
    }

    fn intersect_structs(&mut self, left: &Struct, right: &Struct, hint: &str) -> Option<Ty> {
        let mut fields: IndexMap<String, Field> = left
            .fields
            .iter()
            .cloned()
            .map(|field| (field.name.wire.clone(), field))
            .collect();
        for field in &right.fields {
            match fields.get_mut(&field.name.wire) {
                Some(existing) => {
                    existing.ty = self.intersect_types(
                        existing.ty,
                        field.ty,
                        &format!("{hint}{}", field.name.wire),
                    )?;
                    existing.required = existing.required || field.required;
                    if existing.required {
                        if let Some(default) = &mut existing.default {
                            default.applied = None;
                        }
                    }
                }
                None => {
                    fields.insert(field.name.wire.clone(), field.clone());
                }
            }
        }
        let additional = self.merge_additional(
            &left.additional,
            &right.additional,
            &format!("{hint}Additional"),
        )?;
        Some(self.insert_type(
            hint,
            TypeKind::Struct(Struct {
                fields: fields.into_values().collect(),
                additional,
            }),
            Docs::default(),
            None,
        ))
    }

    fn intersect_union(
        &mut self,
        union_ty: Ty,
        union: &Union,
        other: Ty,
        hint: &str,
    ) -> Option<Ty> {
        let mut variants = Vec::new();
        let mut retained = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            if let Some(ty) =
                self.intersect_types(variant.ty, other, &format!("{hint}Variant{index}"))
            {
                variants.push(UnionVariant {
                    name_hint: variant.name_hint.clone(),
                    ty,
                });
                retained.push(index);
            }
        }
        if variants.len() == 1 {
            return variants.into_iter().next().map(|variant| variant.ty);
        }
        if variants.is_empty() {
            return None;
        }
        if variants.len() == union.variants.len()
            && variants
                .iter()
                .zip(&union.variants)
                .all(|(left, right)| same_ty(left.ty, right.ty))
        {
            return Some(non_nullable(union_ty));
        }
        let strategy = match &union.strategy {
            UnionStrategy::Discriminated {
                tag_field,
                tags,
                categories,
                default_variant,
            } => UnionStrategy::Discriminated {
                tag_field: tag_field.clone(),
                tags: retained.iter().map(|index| tags[*index].clone()).collect(),
                categories: retained.iter().map(|index| categories[*index]).collect(),
                // The fallback variant's index moves with the retained set; if the fallback itself
                // was dropped, the union simply has no fallback any more.
                default_variant: default_variant
                    .and_then(|target| retained.iter().position(|index| *index == target)),
            },
            UnionStrategy::Disjoint { features } => UnionStrategy::Disjoint {
                features: retained
                    .iter()
                    .map(|index| features[*index].clone())
                    .collect(),
            },
            UnionStrategy::Trial { mode, priorities } => UnionStrategy::Trial {
                mode: *mode,
                priorities: retained.iter().map(|index| priorities[*index]).collect(),
            },
        };
        Some(self.insert_type(
            hint,
            TypeKind::Union(Union { variants, strategy }),
            Docs::default(),
            None,
        ))
    }

    /// Whether two lowered value types would emit the *same* Rust type as a shared map value, so
    /// multiple `patternProperties`/`additionalProperties` values can collapse into one typed
    /// overflow map. A bounded structural equivalence:
    ///
    /// * equal `TypeId` (with equal `nullable`) — a shared `$ref` or the single-entry case;
    /// * otherwise, for distinct ids with equal `nullable`, compare the def kinds structurally but
    ///   only for *leaf* shapes that have no per-inline-schema identity: `Primitive` (same `Prim`),
    ///   `Bytes`, `Any`, and `Array` (recursing on the element). Composite kinds
    ///   (`Struct`/`Enum`/`Tuple`) generate a distinct named Rust type per inline schema, so two
    ///   such inline shapes are treated as heterogeneous (→ `E005`) rather than silently merged.
    ///
    /// `boxed` is deliberately ignored: it is a use-site indirection modifier, not part of the map
    /// value's emitted type (the map value is never boxed).
    ///
    /// The `Array` recursion is *not* structurally bounded — array element types can form `$ref`
    /// cycles (`A = [B]`, `B = [A]`) — so a visited-pair guard makes it terminate: an `(a.id, b.id)`
    /// pair already on the comparison stack is a co-recursive back-edge and compares equal (the two
    /// types are being compared identically along the cycle, so they are structurally equal there).
    fn same_map_value_type(&self, a: Ty, b: Ty) -> bool {
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
                _ => false,
            },
            _ => false,
        };
        visiting.pop();
        result
    }

    /// Give a property's `default` its single explicit disposition. Returns `None` when the
    /// property declared no `default`; otherwise a [`FieldDefault`] whose `applied` is set only for
    /// a representable scalar on a plain optional field. A non-representable default emits `W005`.
    fn field_default(&mut self, child: &SchemaOr, ty: Ty, required: bool) -> Option<FieldDefault> {
        let SchemaOr::Schema(schema) = child else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let classified = classify_default(raw);
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        match representable_default(&classified, kind) {
            Some(value) => {
                let display = default_display(&value);
                // A serde default only fires for an absent field on deserialization, so it is wired
                // only for a plain optional (non-required, non-nullable) scalar. A required field is
                // always present, and a nullable field already carries `Option`; both are documented
                // in rustdoc instead of silently ignored.
                let applied = (!required && !ty.nullable).then_some(value);
                Some(FieldDefault {
                    doc_note: format!("Default: `{display}`."),
                    applied,
                })
            }
            None => {
                Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                    .message(
                        "schema `default` is not a scalar matching the field type; it is \
                         documented in rustdoc but not applied as a deserialization default",
                    )
                    .remedy(
                        "use a scalar default matching the field's own type, or set the value \
                         explicitly at each call site",
                    )
                    .emit(self.diags);
                Some(FieldDefault {
                    doc_note: format!("Default (not applied): `{}`.", raw_display(raw)),
                    applied: None,
                })
            }
        }
    }

    /// Render the rustdoc `Default:` note for a parameter's schema `default`, if it declared one.
    /// Parameter defaults are documented but never serde-wired.
    fn param_default_display(&self, schema: Option<&RefOr<Schema>>, ty: Ty) -> Option<String> {
        let RefOr::Item(schema) = schema? else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        Some(default_display_for(raw, kind))
    }

    /// A `default` in a structural position with no field/parameter/type home of its own —
    /// array `items`, tuple `prefixItems`, `additionalProperties` value, or a request/response body
    /// root — cannot be applied or documented against a named item, so it is reported as `W005`
    /// rather than dropped silently.
    fn warn_structural_default_or(&mut self, schema: &SchemaOr, position: &str) {
        if let SchemaOr::Schema(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    fn warn_structural_default_ref(&mut self, schema: &RefOr<Schema>, position: &str) {
        if let RefOr::Item(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    fn warn_structural_default(&mut self, schema: &Schema, position: &str) {
        if schema.default.is_some() {
            Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                .message(format!(
                    "schema `default` on {position} has no field to carry it and is not applied"
                ))
                .remedy("move the default onto a named property, or set the value explicitly")
                .emit(self.diags);
        }
    }

    fn lower_enum(&mut self, values: &[SpannedValue], schema: &Schema, hint: &str) -> Option<Ty> {
        // A `null` member — or `"null"` in the schema's own type array — makes the enum/const
        // nullable: strip the nulls, lower the remaining scalars as the enum, and wrap the result
        // in `Option`. The enum/const branch returns before `lower_schema` computes `nullable`, so
        // the nullability has to be decided here from both sources.
        let has_null = schema.types.types.contains(&JsonType::Null)
            || values.iter().any(|value| matches!(value.node, Node::Null));
        // Declared order is preserved (minus nulls) so double generation stays byte-identical.
        let remainder: Vec<&SpannedValue> = values
            .iter()
            .filter(|value| !matches!(value.node, Node::Null))
            .collect();

        // Only `null` members remained (`enum: [null]` / `const: null`): emit the exact JSON null
        // type (`()`), not a nullable unconstrained value that would also accept non-null content.
        if remainder.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        let mut variants = Vec::new();
        let mut repr = None;
        for value in remainder {
            let scalar = match scalar_value(value) {
                Some(value) => value,
                None => {
                    Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                        .message(
                            "enum/const values must be scalars (object/array members are not \
                             representable as enum variants)",
                        )
                        .emit(self.diags);
                    return None;
                }
            };
            let scalar_repr = match scalar {
                ScalarValue::Bool(_) => ScalarRepr::Bool,
                ScalarValue::Int(_) => ScalarRepr::Int,
                ScalarValue::String(_) => ScalarRepr::String,
            };
            if repr
                .replace(scalar_repr)
                .is_some_and(|previous| previous != scalar_repr)
            {
                Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                    .message("enum/const values must all share the same scalar kind")
                    .emit(self.diags);
                return None;
            }
            variants.push(scalar);
        }
        // The enum def is the last graph insert; setting `nullable` afterward is a pure mutate that
        // preserves the component-root last-insert invariant asserted in `ensure_component`.
        let mut ty = self.insert_schema_type(
            schema,
            hint,
            TypeKind::Enum(ScalarEnum {
                repr: repr.unwrap_or(ScalarRepr::String),
                variants,
            }),
        );
        ty.nullable = has_null;
        Some(ty)
    }

    /// A parameter is always rendered to a wire string — path/query/header/cookie interpolation or a
    /// serialized content value — and `bytes::Bytes` (from `format: binary` / `contentEncoding:
    /// base64`) is not `Display` and has no faithful string rendering. `format: binary` on a
    /// parameter is conventionally just an opaque string, so a parameter whose type lowered to raw
    /// bytes is represented as a plain `String` instead — keeping the parameter renderable and
    /// matching the pre-`Bytes` behavior. Body/multipart binary lowering is unaffected.
    fn remap_binary_param(&mut self, ty: Ty, hint: &str) -> Ty {
        if matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Bytes)
        ) {
            let mut remapped = self.insert_type(
                hint,
                TypeKind::Primitive(Prim::String),
                Docs::default(),
                None,
            );
            remapped.nullable = ty.nullable;
            remapped
        } else {
            ty
        }
    }

    fn lower_parameter(&mut self, parameter: &ParameterObject) -> Option<Parameter> {
        let location = match parameter.location.as_str() {
            "path" => ParamLoc::Path,
            "query" => ParamLoc::Query,
            "header" => ParamLoc::Header,
            "cookie" => ParamLoc::Cookie,
            "querystring" => ParamLoc::QueryString,
            _ => {
                Diagnostic::error(Code::InvalidInput, parameter.provenance.clone())
                    .message(format!(
                        "unsupported parameter location `{}`",
                        parameter.location
                    ))
                    .emit(self.diags);
                return None;
            }
        };
        // `Accept`, `Content-Type`, and `Authorization` header parameters "SHALL be ignored": the
        // protocol layer owns those, and emitting a client argument for one would let a caller
        // silently fight the codec or the auth attachment.
        if location == ParamLoc::Header
            && matches!(
                parameter.name.to_ascii_lowercase().as_str(),
                "accept" | "content-type" | "authorization"
            )
        {
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(format!(
                    "header parameter `{}` is ignored: the specification reserves `Accept`, \
                     `Content-Type`, and `Authorization` to the protocol layer",
                    parameter.name
                ))
                .remedy(
                    "remove the parameter; content types follow the operation's media types and \
                     credentials are registered with `Client::with_credential`",
                )
                .emit(self.diags);
            return None;
        }
        if location == ParamLoc::QueryString {
            let Some((media_name, object)) = parameter.content.iter().next() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("`in: querystring` requires one content media type with a schema")
                .emit(self.diags);
                return None;
            };
            let object = self.resolve_media_object(object, media_name)?;
            let media = lower_media_type(media_name, &parameter.provenance, self.diags)?;
            if !matches!(media, MediaType::Json | MediaType::FormUrlEncoded) {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "querystring media type `{media_name}` is not supported; use JSON or \
                         application/x-www-form-urlencoded"
                ))
                .emit(self.diags);
                return None;
            }
            let Some(schema) = object.schema.as_ref() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("querystring content requires a schema for a typed client argument")
                .emit(self.diags);
                return None;
            };
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            if media == MediaType::FormUrlEncoded
                && !matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Struct(_))
                )
            {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("form-urlencoded querystring parameters require an object schema")
                .emit(self.diags);
                return None;
            }
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required,
                style: ParamStyle::Content(media),
                allow_reserved: false,
                explode: true,
                deprecated: parameter.deprecated,
                default_display: self.param_default_display(object.schema.as_ref(), ty),
            });
        }
        let style_name = parameter.style.as_deref().unwrap_or(match location {
            ParamLoc::Path | ParamLoc::Header => "simple",
            ParamLoc::Query | ParamLoc::Cookie => "form",
            ParamLoc::QueryString => unreachable!("querystring returned above"),
        });
        // The legal `(style, in)` pairs are enforced by the official document schema before
        // lowering (`E011`), so an unknown pairing here is a generator bug rather than user input.
        // The arm is kept so a future schema relaxation cannot silently mis-serialize.
        let style = match (location, style_name) {
            (ParamLoc::Path | ParamLoc::Header, "simple") => ParamStyle::Simple,
            (ParamLoc::Path, "matrix") => ParamStyle::Matrix,
            (ParamLoc::Path, "label") => ParamStyle::Label,
            (ParamLoc::Query | ParamLoc::Cookie, "form") => ParamStyle::Form,
            (ParamLoc::Query, "spaceDelimited") => ParamStyle::Delimited(Delimiter::Space),
            (ParamLoc::Query, "pipeDelimited") => ParamStyle::Delimited(Delimiter::Pipe),
            (ParamLoc::Query, "deepObject") => ParamStyle::DeepObject,
            (ParamLoc::Cookie, "cookie") => ParamStyle::Cookie,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "parameter style `{style_name}` is not permitted for `{}` parameters",
                    parameter.location
                ))
                .emit(self.diags);
                return None;
            }
        };
        // `deepObject` ignores `explode` entirely; every other style defaults per the
        // specification (true only for `form` and 3.2's `cookie`).
        let explode = parameter
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form | ParamStyle::Cookie));
        if matches!(style, ParamStyle::Delimited(_)) && parameter.explode == Some(true) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(format!(
                "`style: {style_name}` with `explode: true` has no defined serialization"
            ))
            .remedy("set `explode: false`, which is the default for this style")
            .emit(self.diags);
            return None;
        }
        // Deprecated in 3.2, and inert for a typed client: an absent optional parameter is simply
        // not sent, so there is never a case where the client would send an empty string instead.
        if parameter.allow_empty_value {
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowEmptyValue` has no effect: an optional parameter the caller omits is \
                     not sent at all",
                )
                .remedy("remove `allowEmptyValue`; it is deprecated in OpenAPI 3.2")
                .emit(self.diags);
        }
        // `allowReserved` only means anything where the location percent-encodes at all.
        if parameter.allow_reserved
            && (location == ParamLoc::Header || matches!(style, ParamStyle::Cookie))
        {
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowReserved` has no effect here: this parameter is sent without \
                     percent-encoding",
                )
                .remedy("remove `allowReserved`, or use `style: form` if encoding is wanted")
                .emit(self.diags);
        }
        let ty = if let Some(schema) = &parameter.schema {
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            self.remap_binary_param(ty, &parameter.name)
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let object = self.resolve_media_object(object, media)?;
            let media_name = media.clone();
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
            // A `content` parameter is rendered by its media codec. Only JSON and raw text have a
            // codec that produces a single parameter token; anything else would fall through to
            // `simple` serialization and be sent in the wrong format.
            if !matches!(media, MediaType::Json | MediaType::Text) {
                Diagnostic::error(Code::UnsupportedMediaType, parameter.provenance.clone())
                    .message(format!(
                        "`content` parameter media type `{media_name}` has no single-token \
                         serialization"
                    ))
                    .remedy(
                        "use `application/json` or a `text/*` media type, or describe the \
                         parameter with `schema` and a serialization style",
                    )
                    .emit(self.diags);
                return None;
            }
            let ty = object
                .schema
                .as_ref()
                .and_then(|schema| self.lower_schema_ref(schema, &parameter.name))?;
            let ty = self.remap_binary_param(ty, &parameter.name);
            let default_display = self.param_default_display(object.schema.as_ref(), ty);
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required || location == ParamLoc::Path,
                style: ParamStyle::Content(media),
                allow_reserved: false,
                explode: false,
                deprecated: parameter.deprecated,
                default_display,
            });
        } else {
            self.insert_type(
                &parameter.name,
                TypeKind::Any,
                Docs::default(),
                Some(parameter.provenance.clone()),
            )
        };
        if !parameter_shape_supported(&self.graph, ty) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(
                "simple/form parameter serialization does not support nested arrays or objects",
            )
            .emit(self.diags);
            return None;
        }
        let default_display = self.param_default_display(parameter.schema.as_ref(), ty);
        Some(Parameter {
            name: parameter.name.clone(),
            location,
            ty,
            required: parameter.required || location == ParamLoc::Path,
            style,
            allow_reserved: parameter.allow_reserved,
            explode,
            deprecated: parameter.deprecated,
            default_display,
        })
    }

    fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        let (media_name, object) = choose_media(
            &body.content,
            &body.provenance,
            self.diags,
            media_object_is_opaque,
        )?;
        let object = self.resolve_media_object(object, media_name)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
        // A media *range* describes what a server may return, not what a client sends: `Content-Type`
        // requires a concrete type/subtype (RFC 9110 § 8.3), and a generated request puts its media
        // key on the wire verbatim. Emitting `Content-Type: video/*` would be an undispatchable
        // header, and picking a concrete member of the family would be spargen inventing what the
        // document declined to say — so it is rejected rather than guessed at.
        if classify_media_range(media_essence(media_name)).is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is a media range, which describes a family rather \
                     than the concrete `Content-Type` a request must send"
                ))
                .remedy(
                    "name the concrete media type the request body is sent as, or omit this API \
                     segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // Streaming media is a response-only construct: a `text/event-stream` / `application/x-ndjson`
        // *request* body has no representation here, so it stays rejected (narrowed `E009`) rather
        // than silently degrade. (`choose_media` only picks it when no whole-body alternative exists.)
        if media.stream_framing().is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is only supported for streaming response bodies, \
                     not request bodies"
                ))
                .remedy("send a non-streaming request body, or omit this API segment with spargen::omit!")
                .emit(self.diags);
            return None;
        }
        // A streaming request body is already rejected above, so any `itemSchema` reaching here sits
        // on a non-streaming media where it is meaningless; acknowledge it with `W010` rather than
        // dropping it silently.
        if object.item_schema.is_some() {
            Diagnostic::warning(Code::Oas32ConstructIgnored, body.provenance.clone())
                .message(
                    "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; on this \
                     request body it is not used",
                )
                .emit(self.diags);
        }
        let ty = object
            .schema
            .as_ref()
            .and_then(|schema| self.lower_schema_ref(schema, "RequestBody"));
        if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a request body schema");
        }
        // A `multipart/form-data` body is emitted as a `reqwest::multipart::Form` whose parts are the
        // fields of an object schema. A concrete non-object type (or a multipart body with no schema
        // at all) has no fields to enumerate as parts, so it stays unsupported (`E009`, narrowed)
        // rather than silently degrade. A schema that *failed* to lower for its own reason (`ty` is
        // `None` though a schema was declared) has already emitted that diagnostic — don't pile a
        // misleading "must be an object" E009 on top of it.
        if media == MediaType::Multipart {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "a `multipart/form-data` request body must be an object schema; its \
                         properties are the form parts, so a non-object multipart body is not \
                         representable",
                    )
                    .remedy(
                        "give the multipart body an object schema with a property per form part, \
                         or omit this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
            }
        }
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets("RequestBody", ty, object.schema.is_some(), &body.provenance)
        } else {
            ty
        };
        if let Some(ty) = ty {
            let compatible = match media {
                MediaType::Text => raw_text_type_supported(&self.graph, ty),
                MediaType::OctetStream => matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Bytes)
                ),
                _ => true,
            };
            if !compatible {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "media type `{media_name}` requires a string-like or binary schema that can be sent as a raw body"
                    ))
                    .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
        }
        // A form-urlencoded body is rendered property by property, so it needs properties. Without
        // this gate a non-object body compiled and then failed at runtime inside the form encoder.
        if media == MediaType::FormUrlEncoded {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "an `application/x-www-form-urlencoded` request body must be an object \
                         schema; its properties are the form fields",
                    )
                    .remedy(
                        "give the body an object schema with a property per form field, or omit \
                         this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let encoding = self.lower_body_encoding(media, media_name, ty, &object)?;
        Some(RequestBody {
            media,
            content_type: media_essence(media_name).to_owned(),
            ty,
            required: body.required,
            encoding,
        })
    }

    /// Resolve the Encoding Objects of a form or multipart request body into a fully-populated
    /// [`BodyEncoding`] — one entry per body property, so the emitted code never has to infer a
    /// default at runtime.
    ///
    /// Returns `None` only when the body is unrepresentable; an encoding that simply has no effect
    /// here is reported as `W011` and dropped.
    fn lower_body_encoding(
        &mut self,
        media: MediaType,
        media_name: &str,
        ty: Option<Ty>,
        object: &MediaTypeObject,
    ) -> Option<BodyEncoding> {
        // Encoding diagnostics point at the Media Type Object that declares them, not at the whole
        // request body.
        let at = &object.provenance;
        // Media that is neither form nor multipart is dispositioned once, in
        // `resolve_media_object`, which every Media Type Object passes through.
        if !matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
            return Some(BodyEncoding::default());
        }
        // `prefixEncoding`/`itemEncoding` describe positional parts of an array-shaped body, and
        // the specification scopes both to `multipart`. On multipart spargen generates from an
        // object schema, so there are no positions to encode and the declaration is rejected; on
        // form-urlencoded the specification itself says they do not apply, so they are inert.
        let positional = object
            .prefix_encoding
            .first()
            .map(|(_, at)| ("prefixEncoding", at.clone()))
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = positional {
            if media == MediaType::FormUrlEncoded {
                Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                    .message(format!(
                        "`{field}` has no effect on `{media_name}`: the specification scopes it to \
                         `multipart` content"
                    ))
                    .emit(self.diags);
            } else {
                Diagnostic::error(Code::UnsupportedMediaType, at)
                    .message(format!(
                        "`{field}` describes positional parts of an array-shaped multipart body; \
                         spargen generates `multipart/form-data` from an object schema, which has \
                         no positions"
                    ))
                    .remedy(
                        "use `encoding` keyed by property name, or omit this API segment with \
                         spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let Some(TypeKind::Struct(structure)) =
            ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind)
        else {
            // The body already failed its own shape gate above; don't pile on.
            return Some(BodyEncoding::default());
        };
        let fields: Vec<(String, Ty)> = structure
            .fields
            .iter()
            .map(|field| (field.name.wire.clone(), field.ty))
            .collect();
        // Nested encoding describes nested multipart parts (`multipart/mixed` inside a part).
        // Spargen generates one flat level, so a nested field is rejected rather than dropped.
        for (name, encoding) in &object.encoding {
            if let Some((field, at)) = encoding.nested.first() {
                Diagnostic::error(Code::UnsupportedMediaType, at.clone())
                    .message(format!(
                        "`encoding.{name}.{field}` describes a nested multipart part, which \
                         spargen does not generate"
                    ))
                    .remedy("flatten the body, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
        }
        // An `encoding` key naming no property has nothing to apply to.
        for name in object.encoding.keys() {
            if !fields.iter().any(|(wire, _)| wire == name) {
                Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
                    .message(format!(
                        "`encoding` entry `{name}` names no property of the body schema, so it is \
                         ignored"
                    ))
                    .emit(self.diags);
            }
        }
        let mut properties = Vec::with_capacity(fields.len());
        for (name, field_ty) in fields {
            let declared = object.encoding.get(&name);
            let mode = self.encoding_mode(declared, field_ty, media, &name, at)?;
            let headers = self.encoding_headers(declared, media, &name, at);
            properties.push(PropertyEncoding {
                name,
                mode,
                headers,
            });
        }
        Some(BodyEncoding { properties })
    }

    /// Apply the Encoding Object's mode switch for one property.
    fn encoding_mode(
        &mut self,
        declared: Option<&EncodingObject>,
        field_ty: Ty,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Option<EncodingMode> {
        // Presence of any RFC 6570 field selects query-style serialization outright, and makes
        // `contentType` inert — the specification is explicit that it is then ignored.
        if let Some(encoding) = declared {
            if encoding.style.is_some()
                || encoding.explode.is_some()
                || encoding.allow_reserved.is_some()
            {
                let style_name = encoding.style.as_deref().unwrap_or("form");
                let style = match style_name {
                    "form" => ParamStyle::Form,
                    "spaceDelimited" => ParamStyle::Delimited(Delimiter::Space),
                    "pipeDelimited" => ParamStyle::Delimited(Delimiter::Pipe),
                    "deepObject" => ParamStyle::DeepObject,
                    // The document schema enumerates these four, so this is unreachable for a
                    // validated document.
                    _ => {
                        Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                            .message(format!(
                                "`encoding.{name}.style: {style_name}` is not a form style"
                            ))
                            .emit(self.diags);
                        return None;
                    }
                };
                let explode = encoding
                    .explode
                    .unwrap_or(matches!(style, ParamStyle::Form));
                // The specification's own serialization table marks the delimited styles with
                // `explode: true` as *n/a* — undefined. The identical parameter-side construct is
                // already `E010`; without this an Encoding Object could declare it and have the
                // `explode` silently ignored.
                if explode && matches!(style, ParamStyle::Delimited(_)) {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.style: {style_name}` with `explode: true` is \
                             undefined: the specification's serialization table gives no value for \
                             that combination"
                        ))
                        .remedy("set `explode: false`, or use `style: form`")
                        .emit(self.diags);
                    return None;
                }
                // `deepObject` builds `name[key]=value` query fragments. A multipart part carries
                // its name in `Content-Disposition` and its value alone, so there is nowhere for
                // that syntax to go and no defined representation to fall back on.
                if media == MediaType::Multipart && style == ParamStyle::DeepObject {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.style: deepObject` is defined only for `in: query`; \
                             it has no `multipart/form-data` part representation"
                        ))
                        .remedy(
                            "use `style: form`, give the property a `contentType` such as \
                             `application/json`, or omit this API segment with spargen::omit!",
                        )
                        .emit(self.diags);
                    return None;
                }
                // An object property under RFC 6570 serialization: the specification says the
                // Encoding Object applies to the *entire value* for a non-array property, but
                // defines no part representation for an object, so there is nothing to generate.
                if media == MediaType::Multipart
                    && matches!(
                        self.graph.get(field_ty.id).map(|def| &def.kind),
                        Some(TypeKind::Struct(_))
                    )
                {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}` selects RFC 6570 serialization for an object \
                             property, which has no defined `multipart/form-data` part \
                             representation"
                        ))
                        .remedy(
                            "give the property a `contentType` such as `application/json` instead \
                             of `style`/`explode`/`allowReserved`, or omit this API segment with \
                             spargen::omit!",
                        )
                        .emit(self.diags);
                    return None;
                }
                // Multipart part values are never percent-encoded, so `allowReserved` is inert.
                let allow_reserved = encoding.allow_reserved.unwrap_or(false);
                if allow_reserved && media == MediaType::Multipart {
                    Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                        .message(
                            "`allowReserved` has no effect on `multipart/form-data`: part values \
                             are not percent-encoded",
                        )
                        .emit(self.diags);
                }
                return Some(EncodingMode::Style {
                    style,
                    explode,
                    allow_reserved: allow_reserved && media != MediaType::Multipart,
                });
            }
        }
        let explicit = declared.and_then(|encoding| encoding.content_type.as_deref());
        let content_type = match explicit {
            // `contentType` is a comma-separated list of acceptable types, but a client sends
            // exactly one, so the first element wins.
            Some(list) => {
                let first = list.split(',').next().unwrap_or(list).trim().to_owned();
                if first.contains('*') {
                    Diagnostic::error(
                        Code::UnsupportedMediaType,
                        declared
                            .map(|encoding| encoding.provenance.clone())
                            .unwrap_or_else(|| at.clone()),
                    )
                    .message(format!(
                        "`encoding.{name}.contentType: {first}` is a wildcard; a client must send \
                         one concrete media type"
                    ))
                    .remedy("name a concrete media type such as `image/png`")
                    .emit(self.diags);
                    return None;
                }
                first
            }
            None => self.default_content_type(field_ty),
        };
        // The declared `contentType` is a wire *header*; how the value is rendered into bytes is
        // decided by the property's own lowered type. That is what lets a part declare
        // `application/sdp` (which spargen has no codec for) over a string property and still be
        // sent correctly, with the declared header attached.
        let codec = match classify_media(media_essence(&content_type)).map(|(codec, _)| codec) {
            Some(codec @ (MediaType::Json | MediaType::Text | MediaType::OctetStream)) => codec,
            _ => self.natural_codec(field_ty),
        };
        // A form field is a single URL-encoded string; raw bytes have no representation there.
        if media == MediaType::FormUrlEncoded && codec == MediaType::OctetStream {
            Diagnostic::error(
                Code::UnsupportedMediaType,
                declared
                    .map(|encoding| encoding.provenance.clone())
                    .unwrap_or_else(|| at.clone()),
            )
            .message(format!(
                "property `{name}` declares `contentType: {content_type}`, which is binary; a \
                 form-urlencoded body cannot carry a binary part"
            ))
            .remedy("send the body as `multipart/form-data`, or encode the value as text")
            .emit(self.diags);
            return None;
        }
        Some(EncodingMode::Media {
            content_type,
            codec,
        })
    }

    /// How a property's value is rendered into bytes, from its lowered type alone.
    fn natural_codec(&self, ty: Ty) -> MediaType {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => MediaType::OctetStream,
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_)) => MediaType::Text,
            _ => MediaType::Json,
        }
    }

    /// The Encoding Object's default `contentType` for a property, from its lowered type.
    fn default_content_type(&self, ty: Ty) -> String {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => "application/octet-stream".to_owned(),
            Some(TypeKind::Struct(_)) | Some(TypeKind::Union(_)) | Some(TypeKind::Any) => {
                "application/json".to_owned()
            }
            // In 3.1 an array's default follows its item type; 3.2 simplified this to JSON. Both
            // agree that an array of objects is JSON, and spargen sends any array as JSON, which
            // is the 3.2 rule and the only self-consistent reading for a nested array.
            Some(TypeKind::Array(_)) | Some(TypeKind::Tuple(_)) => "application/json".to_owned(),
            Some(TypeKind::Primitive(_)) | Some(TypeKind::Enum(_)) => "text/plain".to_owned(),
            _ => "application/octet-stream".to_owned(),
        }
    }

    /// The literal extra part headers of one multipart property.
    ///
    /// A Header Object *describes* a header; it carries no value. Only a schema that pins one —
    /// through `const`, or `default` in its absence — gives a client something to send.
    fn encoding_headers(
        &mut self,
        declared: Option<&EncodingObject>,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Vec<(String, String)> {
        let Some(encoding) = declared else {
            return Vec::new();
        };
        if encoding.headers.is_empty() {
            return Vec::new();
        }
        if media != MediaType::Multipart {
            Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}.headers` applies only to `multipart` content"
                ))
                .emit(self.diags);
            return Vec::new();
        }
        let _ = at;
        let mut headers = Vec::new();
        for (header_name, header) in &encoding.headers {
            // `Content-Type` is described by `contentType`, not here.
            if header_name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            // A `$ref` here is resolved rather than treated as pinning nothing: the target may
            // well declare the `const` that gives the client something to send, and reporting
            // "pins no value" without looking would name the wrong reason. An unresolvable
            // reference is `E004` from `resolve_header`, not a warning.
            let literal = match header {
                RefOr::Item(header) => header
                    .schema
                    .as_ref()
                    .and_then(|schema| self.literal_header_value(schema)),
                RefOr::Ref(_) => match self.resolve_header(header) {
                    Some(resolved) => resolved
                        .schema
                        .as_ref()
                        .and_then(|schema| self.literal_header_value(schema)),
                    // `resolve_header` already reported the unresolvable reference; adding
                    // "pins no value" would name a second, wrong reason for one defect.
                    None => continue,
                },
            };
            match literal {
                Some(value) => headers.push((header_name.clone(), value)),
                None => {
                    Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.headers.{header_name}` pins no value, so there is \
                             nothing for the client to send"
                        ))
                        .remedy("give the header schema a `const` (or a `default`) value")
                        .emit(self.diags);
                }
            }
        }
        headers
    }

    /// The literal value a header schema pins, if any.
    fn literal_header_value(&self, schema: &RefOr<Schema>) -> Option<String> {
        let RefOr::Item(schema) = schema else {
            return None;
        };
        let value = schema.const_value.as_ref().or(schema.default.as_ref())?;
        match &value.node {
            crate::source::Node::String(text) => Some(text.clone()),
            crate::source::Node::Bool(value) => Some(value.to_string()),
            crate::source::Node::Number(number) => Some(match number {
                crate::source::Number::Int(value) => value.to_string(),
                crate::source::Number::UInt(value) => value.to_string(),
                crate::source::Number::Float(value) => value.to_string(),
            }),
            _ => None,
        }
    }

    fn lower_responses(&mut self, responses: &super::ResponsesObject) -> Responses {
        let mut by_status = Vec::new();
        for (status, response) in &responses.by_status {
            if let Some(status) = parse_status(status) {
                if let Some(response) = self
                    .resolve_response(response)
                    .and_then(|r| self.lower_response(&r))
                {
                    by_status.push((status, response));
                }
            }
        }
        let default = responses
            .default
            .as_ref()
            .and_then(|response| self.resolve_response(response))
            .and_then(|response| self.lower_response(&response));
        Responses { by_status, default }
    }

    fn lower_response(&mut self, response: &ResponseObject) -> Option<Response> {
        let body = choose_media(
            &response.content,
            &response.provenance,
            self.diags,
            media_object_is_opaque,
        )
        .and_then(
            |(media_name, object)| {
                let object = self.resolve_media_object(object, media_name)?;
                let media = lower_media_type(media_name, &response.provenance, self.diags)?;
                // For a sequential/streaming media (`text/event-stream` / `application/x-ndjson`),
                // OpenAPI 3.2 gives the PER-ITEM type in `itemSchema`; a whole-body `schema` does not
                // apply to a stream, so `itemSchema` is preferred (falling back to `schema` for the
                // pre-3.2 form where the item type was written as `schema`). On a non-streaming media
                // `itemSchema` is meaningless: acknowledge it with `W010` and use `schema`.
                let (ty, stream) = if let Some(framing) = media.stream_framing() {
                    if let Some(item_schema) = object.item_schema.as_ref() {
                        if media == MediaType::EventStream && self.document.is_oas32 {
                            if let Some(json) = super::sse::json_data_schema(
                                item_schema,
                                self.resolver,
                                self.diags,
                            ) {
                                let ty = self.lower_schema_or(&json.schema, "ResponseBody");
                                self.warn_structural_default_or(
                                    &json.schema,
                                    "an SSE JSON data content schema",
                                );
                                (ty, Some(crate::ir::Framing::SseJsonData))
                            } else {
                                (
                                    self.lower_schema_ref(item_schema, "ResponseBody"),
                                    Some(crate::ir::Framing::SseEvent),
                                )
                            }
                        } else {
                            (
                                self.lower_schema_ref(item_schema, "ResponseBody"),
                                Some(framing),
                            )
                        }
                    } else if self.document.is_oas32 && object.schema.is_some() {
                        Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(
                                "in OpenAPI 3.2, `schema` on sequential media describes the \
                                 complete sequence; use `itemSchema` for a streaming client result",
                            )
                            .remedy("replace `schema` with `itemSchema`, or choose a non-sequential response media type")
                            .emit(self.diags);
                        return None;
                    } else {
                        (
                            object
                                .schema
                                .as_ref()
                                .and_then(|schema| self.lower_schema_ref(schema, "ResponseBody")),
                            Some(framing),
                        )
                    }
                } else {
                    if object.item_schema.is_some() {
                        Diagnostic::warning(
                            Code::Oas32ConstructIgnored,
                            response.provenance.clone(),
                        )
                        .message(
                            "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; \
                             on this non-streaming media it is not used",
                        )
                        .emit(self.diags);
                    }
                    (
                        object
                            .schema
                            .as_ref()
                            .and_then(|schema| self.lower_schema_ref(schema, "ResponseBody")),
                        None,
                    )
                };
                if let Some(schema) = object.item_schema.as_ref().filter(|_| stream.is_some()) {
                    self.warn_structural_default_ref(schema, "a response body schema");
                } else if let Some(schema) = object.schema.as_ref() {
                    self.warn_structural_default_ref(schema, "a response body schema");
                }
                if matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
                    Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                        .message(format!(
                            "media type `{media_name}` is supported for request bodies, not response bodies"
                        ))
                        .remedy("document a JSON, XML, textual, binary, or streaming response, or omit this API segment with spargen::omit!")
                        .emit(self.diags);
                    return None;
                }
                let ty = if media == MediaType::OctetStream {
                    self.opaque_octets(
                        "ResponseBody",
                        ty,
                        object.schema.is_some(),
                        &response.provenance,
                    )
                } else {
                    ty
                };
                if let Some(ty) = ty {
                    let compatible = match media {
                        MediaType::Text => raw_text_type_supported(&self.graph, ty),
                        MediaType::OctetStream => matches!(
                            self.graph.get(ty.id).map(|definition| &definition.kind),
                            Some(TypeKind::Bytes)
                        ),
                        _ => true,
                    };
                    if !compatible {
                        Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(format!(
                                "media type `{media_name}` requires a string-like or binary response schema"
                            ))
                            .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                            .emit(self.diags);
                        return None;
                    }
                }
                Some((media, ty, stream))
            },
        );
        // A streaming response media (`text/event-stream` / `application/x-ndjson`) records its
        // framing; the body is then the streamed item type `T`. A whole-body response has no
        // framing. Streaming only takes effect when this is the operation's single success body
        // (see `Responses::stream_success`).
        let headers = self.lower_response_headers(response);
        Some(Response {
            media: body.map(|(media, _, _)| media),
            body: body.and_then(|(_, ty, _)| ty),
            stream: body.and_then(|(_, _, stream)| stream),
            headers,
        })
    }

    /// Lower a response's documented headers into typed accessors.
    ///
    /// A header that cannot be represented is skipped with a diagnostic rather than failing the
    /// whole operation: the body is what the call returns, and refusing an otherwise-generatable
    /// operation over an unreadable header would be a poor trade.
    fn lower_response_headers(&mut self, response: &ResponseObject) -> Vec<ResponseHeader> {
        let mut headers = Vec::new();
        for (name, header) in &response.headers {
            // The specification says a documented `Content-Type` header SHALL be ignored: the
            // media type is already the operation's, and a second source would only disagree.
            if name.eq_ignore_ascii_case("content-type") {
                Diagnostic::warning(Code::DeclarationHasNoEffect, response.provenance.clone())
                    .message(
                        "a documented `Content-Type` response header is ignored; the operation's \
                         media type already determines it",
                    )
                    .emit(self.diags);
                continue;
            }
            let Some(header) = self.resolve_header(header) else {
                continue;
            };
            let header = &header;
            // A Header Object may only use `simple`; the document schema already enforces that.
            let (ty, shape) = if let Some(schema) = &header.schema {
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                let Some(shape) = self.header_shape(ty) else {
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` has a shape `simple` serialization cannot \
                             express, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                };
                (ty, shape)
            } else if let Some((media, object)) = header.content.iter().next() {
                // Resolve first: a header's content may itself be a Reference Object, and reading
                // `schema` off the unresolved shell would find `None` and drop the typed accessor
                // with nothing said.
                let Some(object) = self.resolve_media_object(object, media) else {
                    continue;
                };
                let Some(media) = lower_media_type(media, &header.provenance, self.diags) else {
                    continue;
                };
                // A textual `content` entry describes the field value itself, so it decodes exactly
                // like the `schema:` spelling — the shape gate below is what decides. This is not a
                // rare form: `Content-Range` on a ranged response is routinely documented this way,
                // and refusing it cost a typed accessor for no reason.
                if !matches!(media, MediaType::Json | MediaType::Text) {
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` uses a `content` media type spargen cannot \
                             decode, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                }
                if object.item_schema.is_some() {
                    Diagnostic::warning(Code::Oas32ConstructIgnored, header.provenance.clone())
                        .message(format!(
                            "`itemSchema` has no effect on response header `{name}`: a header \
                             field value is not a sequential media"
                        ))
                        .emit(self.diags);
                }
                let Some(schema) = object.schema.as_ref() else {
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` declares `content` without a schema, so no \
                             typed accessor is generated"
                        ))
                        .remedy("give the content entry a `schema`")
                        .emit(self.diags);
                    continue;
                };
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                if media == MediaType::Json {
                    (ty, crate::ir::HeaderShape::Json)
                } else {
                    // Textual content carries the field value verbatim, so only a scalar schema is
                    // representable: a list or an object under `text/plain` says nothing about how
                    // the value is framed, and `simple` is not that framing.
                    let Some(shape @ crate::ir::HeaderShape::Scalar) = self.header_shape(ty) else {
                        Diagnostic::warning(
                            Code::DeclarationHasNoEffect,
                            header.provenance.clone(),
                        )
                        .message(format!(
                            "response header `{name}` declares a textual `content` schema that \
                                 is not a single value, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                        continue;
                    };
                    (ty, shape)
                }
            } else {
                continue;
            };
            // `Set-Cookie` is the one field RFC 9110 §5.3 exempts from the comma-joined field-list
            // rule, and 3.2 gives it a section of its own saying each value must be kept on its own
            // line. The declared schema therefore describes ONE cookie, and the accessor is a list
            // of them — a schema that is already a list is taken to be that list.
            let (ty, shape) = if name.eq_ignore_ascii_case("set-cookie") {
                let list = match self.graph.get(ty.id).map(|def| &def.kind) {
                    Some(TypeKind::Array(_)) => ty,
                    _ => self.insert_type(
                        &format!("Header{name}"),
                        TypeKind::Array(Box::new(ty)),
                        Docs::default(),
                        Some(header.provenance.clone()),
                    ),
                };
                (list, crate::ir::HeaderShape::SetCookie)
            } else {
                (ty, shape)
            };
            headers.push(ResponseHeader {
                name: name.clone(),
                ty,
                required: header.required,
                explode: header.explode.unwrap_or(false),
                shape,
                deprecated: header.deprecated,
                docs: Docs {
                    description: header.description.clone(),
                    ..Docs::default()
                },
            });
        }
        headers
    }

    /// Resolve a Header Object that may be a `$ref` into `#/components/headers/`.
    fn resolve_header(
        &mut self,
        header: &RefOr<super::HeaderObject>,
    ) -> Option<super::HeaderObject> {
        let mut current = header.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(header) => return Some(header),
                RefOr::Ref(reference) => {
                    if !seen.insert(reference.reference.clone()) {
                        Diagnostic::error(Code::UnresolvedRef, reference.provenance)
                            .message("header reference cycle cannot be resolved")
                            .emit(self.diags);
                        return None;
                    }
                    let alias = reference
                        .reference
                        .strip_prefix("#/components/headers/")
                        .map(|name| self.document.components.headers.get(name).cloned());
                    match alias {
                        Some(Some(target)) => current = target,
                        Some(None) => {
                            Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
                                .message(format!(
                                    "unresolved header reference `{}`",
                                    reference.reference
                                ))
                                .emit(self.diags);
                            return None;
                        }
                        // Not a component alias: a multi-file description may reference a whole
                        // file, which resolves through the input bundle exactly as a Parameter or
                        // Response Object reference already does.
                        None => {
                            let from = reference
                                .provenance
                                .span
                                .map(|span| span.file)
                                .unwrap_or(crate::diag::FileId(0));
                            return match self.resolver.resolve_component(
                                &reference.reference,
                                from,
                                super::deserialize::parse_header_object,
                                self.diags,
                            ) {
                                Some(resolved) => Some(resolved),
                                None => {
                                    Diagnostic::error(
                                        Code::UnresolvedRef,
                                        reference.provenance.clone(),
                                    )
                                    .message(format!(
                                        "unresolved header reference `{}`",
                                        reference.reference
                                    ))
                                    .emit(self.diags);
                                    None
                                }
                            };
                        }
                    }
                }
            }
        }
    }

    /// The `simple` wire shape of a lowered header type, or `None` when it has none.
    fn header_shape(&self, ty: Ty) -> Option<crate::ir::HeaderShape> {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Null) => {
                Some(crate::ir::HeaderShape::Scalar)
            }
            Some(TypeKind::Array(_)) => Some(crate::ir::HeaderShape::Array),
            Some(TypeKind::Struct(_)) => Some(crate::ir::HeaderShape::Object),
            _ => None,
        }
    }

    fn resolve_parameter(&mut self, parameter: &RefOr<ParameterObject>) -> Option<ParameterObject> {
        let mut current = parameter;
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(parameter) => return Some(parameter.clone()),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(reference);
                    if !seen.insert(reference.reference.clone()) {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "parameter",
                            "cycle",
                        );
                    }
                    let Some(name) = reference.reference.strip_prefix("#/components/parameters/")
                    else {
                        // Not a component alias: a multi-file description may reference a whole
                        // file, which resolves through the input bundle like a schema `$ref`.
                        let from = reference
                            .provenance
                            .span
                            .map(|span| span.file)
                            .unwrap_or(crate::diag::FileId(0));
                        return match self.resolver.resolve_component(
                            &reference.reference,
                            from,
                            super::deserialize::parse_parameter,
                            self.diags,
                        ) {
                            Some(resolved) => Some(resolved),
                            None => self.reject_component_alias(
                                &reference.provenance,
                                "parameter",
                                &reference.reference,
                            ),
                        };
                    };
                    let Some(target) = self.document.components.parameters.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "parameter",
                            &reference.reference,
                        );
                    };
                    current = target;
                }
            }
        }
    }

    fn resolve_request_body(
        &mut self,
        body: &RefOr<RequestBodyObject>,
    ) -> Option<RequestBodyObject> {
        let mut current = body;
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(body) => return Some(body.clone()),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(reference);
                    if !seen.insert(reference.reference.clone()) {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "request body",
                            "cycle",
                        );
                    }
                    let Some(name) = reference
                        .reference
                        .strip_prefix("#/components/requestBodies/")
                    else {
                        // Not a component alias: a multi-file description may reference a whole
                        // file, which resolves through the input bundle like a schema `$ref`.
                        let from = reference
                            .provenance
                            .span
                            .map(|span| span.file)
                            .unwrap_or(crate::diag::FileId(0));
                        return match self.resolver.resolve_component(
                            &reference.reference,
                            from,
                            super::deserialize::parse_request_body,
                            self.diags,
                        ) {
                            Some(resolved) => Some(resolved),
                            None => self.reject_component_alias(
                                &reference.provenance,
                                "request body",
                                &reference.reference,
                            ),
                        };
                    };
                    let Some(target) = self.document.components.request_bodies.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "request body",
                            &reference.reference,
                        );
                    };
                    current = target;
                }
            }
        }
    }

    fn resolve_response(&mut self, response: &RefOr<ResponseObject>) -> Option<ResponseObject> {
        let mut current = response;
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(response) => return Some(response.clone()),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(reference);
                    if !seen.insert(reference.reference.clone()) {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "response",
                            "cycle",
                        );
                    }
                    let Some(name) = reference.reference.strip_prefix("#/components/responses/")
                    else {
                        // Not a component alias: a multi-file description may reference a whole
                        // file, which resolves through the input bundle like a schema `$ref`.
                        let from = reference
                            .provenance
                            .span
                            .map(|span| span.file)
                            .unwrap_or(crate::diag::FileId(0));
                        return match self.resolver.resolve_component(
                            &reference.reference,
                            from,
                            super::deserialize::parse_response,
                            self.diags,
                        ) {
                            Some(resolved) => Some(resolved),
                            None => self.reject_component_alias(
                                &reference.provenance,
                                "response",
                                &reference.reference,
                            ),
                        };
                    };
                    let Some(target) = self.document.components.responses.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "response",
                            &reference.reference,
                        );
                    };
                    current = target;
                }
            }
        }
    }

    /// Acknowledge a Reference Object `summary`/`description`.
    ///
    /// These document the *reference site*, not the target. Spargen emits one shared item per
    /// component, so a per-site documentation override has nowhere to land without making two use
    /// sites of the same component disagree. Reported rather than dropped.
    fn note_reference_docs(&mut self, reference: &super::Reference) {
        if reference.summary.is_none() && reference.description.is_none() {
            return;
        }
        Diagnostic::warning(Code::DeclarationHasNoEffect, reference.provenance.clone())
            .message(format!(
                "the `summary`/`description` on the reference to `{}` documents this use site, \
                 but the generated item is shared across every use, so the override is not applied",
                reference.reference
            ))
            .remedy("document the referenced component itself")
            .emit(self.diags);
    }

    fn reject_component_alias<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
        reference: &str,
    ) -> Option<T> {
        Diagnostic::error(Code::UnresolvedRef, provenance.clone())
            .message(format!("unresolved {kind} reference `{reference}`"))
            .emit(self.diags);
        None
    }

    /// Resolve a Media Type Object through any `$ref` hops and give every position-independent
    /// field it can carry a disposition.
    ///
    /// This is the single seam every Media Type Object passes through — request bodies, responses,
    /// parameter content, whole-query-string content, and response header content alike — so a
    /// field that only *sometimes* has an effect is reported once, wherever it appears, instead of
    /// being dispositioned on the request-body path and silently dropped everywhere else.
    fn resolve_media_object(
        &mut self,
        object: &super::MediaTypeObject,
        media_name: &str,
    ) -> Option<super::MediaTypeObject> {
        let mut current = object.clone();
        let mut seen = HashSet::new();
        while let Some(reference) = current.reference.clone() {
            // A Reference Object's own `summary`/`description` documents this use site, which one
            // generated item shared across every use cannot express — the same disposition the
            // Parameter, Response, and Request Body paths already give it.
            self.note_reference_docs(&reference);
            if !seen.insert(reference.reference.clone()) {
                Diagnostic::error(Code::UnresolvedRef, reference.provenance)
                    .message("media type reference cycle cannot be resolved")
                    .emit(self.diags);
                return None;
            }
            let Some(name) = reference.reference.strip_prefix("#/components/mediaTypes/") else {
                // Not a component alias: a multi-file description may reference a whole file,
                // which resolves through the input bundle exactly as a Parameter or Response
                // Object reference already does.
                let from = reference
                    .provenance
                    .span
                    .map(|span| span.file)
                    .unwrap_or(crate::diag::FileId(0));
                let resolved = self.resolver.resolve_component(
                    &reference.reference,
                    from,
                    |value, pointer, diags| {
                        Some(super::deserialize::parse_media_type(value, pointer, diags))
                    },
                    self.diags,
                );
                match resolved {
                    Some(resolved) => {
                        current = resolved;
                        continue;
                    }
                    None => {
                        Diagnostic::error(Code::UnresolvedRef, reference.provenance)
                            .message(format!(
                                "unsupported or unresolved Media Type Object reference `{}`",
                                reference.reference
                            ))
                            .emit(self.diags);
                        return None;
                    }
                }
            };
            let Some(target) = self.document.components.media_types.get(name) else {
                Diagnostic::error(Code::UnresolvedRef, reference.provenance)
                    .message(format!("unresolved Media Type Object component `{name}`"))
                    .emit(self.diags);
                return None;
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

    /// Report the encoding fields of a Media Type Object that cannot take effect in this position.
    fn note_inert_encoding(&mut self, object: &super::MediaTypeObject, media_name: &str) {
        let declared = object
            .encoding
            .first()
            .map(|(_, encoding)| ("encoding", encoding.provenance.clone()))
            .or_else(|| {
                object
                    .prefix_encoding
                    .first()
                    .map(|(_, at)| ("prefixEncoding", at.clone()))
            })
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = declared {
            Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                .message(format!(
                    "`{field}` has no effect on `{media_name}`: it applies only to `multipart` \
                     and `application/x-www-form-urlencoded` content"
                ))
                .emit(self.diags);
        }
    }

    /// Lower a possibly-`$ref` schema. Component refs go through [`Self::ensure_component`] so
    /// every use site shares one generated type instead of lowering a duplicate.
    fn lower_schema_ref(&mut self, schema: &RefOr<Schema>, hint: &str) -> Option<Ty> {
        match schema {
            RefOr::Item(schema) => self.lower_schema(schema, hint),
            RefOr::Ref(reference) => {
                if let Some(name) = reference.reference.strip_prefix("#/components/schemas/") {
                    self.ensure_component(name)
                } else if is_remote_ref(&reference.reference) {
                    self.ensure_remote(&reference.reference)
                } else {
                    let resolved = self
                        .resolver
                        .resolve(&reference.reference, &reference.provenance, self.diags)
                        .ok()?;
                    self.lower_schema(&resolved.schema, hint)
                }
            }
        }
    }

    /// Whether this definition is a named component root — reachable by name from anywhere else in
    /// the document, rather than owned by the single use site that produced it.
    fn is_component_root(&self, id: TypeId) -> bool {
        self.components
            .values()
            .chain(self.in_progress.values())
            .chain(self.remote_components.values())
            .chain(self.remote_in_progress.values())
            .any(|&(root, _)| root == id)
    }

    /// Read an untyped body on a binary media type as raw octets.
    ///
    /// OpenAPI 3.1 aligned Schema Objects with JSON Schema 2020-12 and removed `format: binary`, so
    /// an empty (always-true) Schema Object — or no `schema` at all — is now how a document says
    /// *any octets*: the media type already carries the meaning, and `type: string` would be the
    /// 3.0 spelling the release deliberately retired. Both lower to `Any`, which on
    /// `application/octet-stream` would emit `serde_json::Value` for a byte stream, so the use site
    /// is retyped to `Bytes`.
    ///
    /// `declared_but_unlowerable` is a schema that was written and failed to lower for its own
    /// reason: it has already reported that, and must not be silently rewritten into bytes.
    ///
    /// `provenance` is the body's own, never the document root's: `Scope::alloc` disambiguates
    /// colliding name hints by pointer precisely so that reordering paths renames nothing, and a
    /// root pointer would collapse every `RequestBody`/`ResponseBody` here into arrival order.
    fn opaque_octets(
        &mut self,
        hint: &str,
        ty: Option<Ty>,
        declared: bool,
        provenance: &crate::diag::Provenance,
    ) -> Option<Ty> {
        let Some(ty) = ty else {
            let declared_but_unlowerable = declared;
            return (!declared_but_unlowerable).then(|| {
                self.insert_type(
                    hint,
                    TypeKind::Bytes,
                    Docs::default(),
                    Some(provenance.clone()),
                )
            });
        };
        if !matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Any)
        ) {
            return Some(ty);
        }
        // An inline `{}` is the definition just inserted, and nothing can reference it yet, so it
        // is replaced in place — left behind it would emit a second `pub type … =
        // serde_json::Value` alias and take the name this body wants.
        //
        // Being the last definition is not enough to prove that, though: a *childless* component
        // (`Opaque: {}`) is lifted into its reserved id, which is then the last id as well, and
        // rewriting that would retype the component for every other reference in the document. A
        // named root is therefore left exactly as declared and the use site gets its own type.
        if self.graph.last_id() == Some(ty.id) && !self.is_component_root(ty.id) {
            let (_, definition) = self
                .graph
                .pop_last()
                .expect("a definition was just observed");
            let id = self.graph.insert(TypeDef {
                kind: TypeKind::Bytes,
                ..definition
            });
            debug_assert_eq!(id, ty.id, "popping and reinserting reuses the dense id");
            return Some(Ty { id, ..ty });
        }
        Some(self.insert_type(
            hint,
            TypeKind::Bytes,
            Docs::default(),
            Some(provenance.clone()),
        ))
    }

    fn insert_schema_type(&mut self, schema: &Schema, hint: &str, kind: TypeKind) -> Ty {
        self.insert_type(
            hint,
            kind,
            Docs {
                title: schema.title.clone(),
                description: schema.description.clone(),
                deprecated: schema.deprecated,
                ..Docs::default()
            },
            Some(schema.provenance.clone()),
        )
    }

    fn insert_type(
        &mut self,
        hint: &str,
        kind: TypeKind,
        docs: Docs,
        provenance: Option<crate::diag::Provenance>,
    ) -> Ty {
        let id = self.graph.insert(TypeDef {
            name_hint: hint.to_owned(),
            kind,
            docs,
            provenance: provenance.unwrap_or_else(|| self.document.provenance.clone()),
        });
        Ty {
            id,
            nullable: false,
            boxed: false,
        }
    }
}

fn parameter_shape_supported(graph: &TypeGraph, ty: Ty) -> bool {
    parameter_shape_supported_inner(graph, ty, false, &mut HashSet::new())
}

fn parameter_shape_supported_inner(
    graph: &TypeGraph,
    ty: Ty,
    scalar_only: bool,
    visiting: &mut HashSet<TypeId>,
) -> bool {
    if !visiting.insert(ty.id) {
        return false;
    }
    let Some(definition) = graph.get(ty.id) else {
        visiting.remove(&ty.id);
        return false;
    };
    let supported = match &definition.kind {
        TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Bytes | TypeKind::Null => true,
        TypeKind::Array(item) if !scalar_only => {
            parameter_shape_supported_inner(graph, **item, true, visiting)
        }
        TypeKind::Tuple(items) if !scalar_only => items
            .iter()
            .all(|item| parameter_shape_supported_inner(graph, *item, true, visiting)),
        TypeKind::Struct(object) if !scalar_only => {
            object
                .fields
                .iter()
                .all(|field| parameter_shape_supported_inner(graph, field.ty, true, visiting))
                && match &object.additional {
                    AdditionalProps::Deny | AdditionalProps::Allow => true,
                    AdditionalProps::Typed(value) => {
                        parameter_shape_supported_inner(graph, **value, true, visiting)
                    }
                }
        }
        TypeKind::Union(union) => union.variants.iter().all(|variant| {
            parameter_shape_supported_inner(graph, variant.ty, scalar_only, visiting)
        }),
        TypeKind::Struct(_)
        | TypeKind::Array(_)
        | TypeKind::Tuple(_)
        | TypeKind::Never
        | TypeKind::Any => false,
    };
    visiting.remove(&ty.id);
    supported
}

fn type_accepts_null(ty: Ty, kind: &TypeKind) -> bool {
    ty.nullable || matches!(kind, TypeKind::Null | TypeKind::Any)
}

fn non_nullable(mut ty: Ty) -> Ty {
    ty.nullable = false;
    ty
}

fn same_ty(left: Ty, right: Ty) -> bool {
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

fn lower_security_requirement(requirement: &SecurityRequirement) -> crate::ir::SecurityRequirement {
    crate::ir::SecurityRequirement(
        requirement
            .0
            .iter()
            .map(|(name, scopes)| (SchemeId(name.clone()), scopes.clone()))
            .collect(),
    )
}

/// Lower one Server Object, parsing its URL template and validating its variables.
///
/// A Server Variable `default` is unlike a Schema Object `default`: the specification says it is
/// actually sent when the caller supplies no alternative, so it changes the wire and must be
/// modeled rather than documented.
/// Resolve the base-URL override an Operation or Path Item Object declares, rendered with every
/// server variable at its declared default.
///
/// The specification defines no way for a client to *choose* among several `servers` entries in
/// this position, so the first is used and the rest are acknowledged as having no effect (`W011`).
/// Variables are substituted with their declared defaults — what the specification says is sent
/// when nothing selects another value. Unlike the document's `servers`, a per-operation override
/// gets no typed builder: there is no constructor to hand a selection to, since the choice is made
/// per call rather than per client.
fn lower_server_override(servers: &[super::Server], diags: &mut Diagnostics) -> Option<String> {
    let (first, rest) = servers.split_first()?;
    for extra in rest {
        Diagnostic::warning(Code::DeclarationHasNoEffect, extra.provenance.clone())
            .message(format!(
                "`servers` entry `{}` past the first has no effect here: the specification \
                 defines no rule for selecting among per-operation servers, so the first is used",
                extra.url
            ))
            .emit(diags);
    }
    lower_server(first, diags).map(|server| render_server_url(&server))
}

/// Render a lowered server URL template with each variable at its declared default.
///
/// `lower_server` has already rejected a template naming an undeclared variable, so a missing
/// entry here can only occur on a document that is already failing.
fn render_server_url(server: &Server) -> String {
    let mut url = String::with_capacity(server.url.len());
    for segment in &server.segments {
        match segment {
            UrlSegment::Literal(text) => url.push_str(text),
            UrlSegment::Variable(name) => {
                if let Some(variable) = server.variables.get(name) {
                    url.push_str(&variable.default);
                }
            }
        }
    }
    url
}

fn lower_server(server: &super::Server, diags: &mut Diagnostics) -> Option<Server> {
    let segments = parse_url_template(&server.url);
    let mut seen: HashSet<&str> = HashSet::new();
    for segment in &segments {
        let UrlSegment::Variable(name) = segment else {
            continue;
        };
        if !seen.insert(name.as_str()) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` appears more than once in `{}`",
                    server.url
                ))
                .emit(diags);
            return None;
        }
        if !server.variables.contains_key(name) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server URL `{}` references undeclared variable `{name}`",
                    server.url
                ))
                .remedy("declare it under the server's `variables`")
                .emit(diags);
            return None;
        }
    }
    for (name, variable) in &server.variables {
        // A default outside its own `enum` would make the no-argument path send an illegal value.
        if !variable.enum_values.is_empty() && !variable.enum_values.contains(&variable.default) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` has default `{}`, which is not one of its declared \
                     `enum` values",
                    variable.default
                ))
                .emit(diags);
            return None;
        }
        if !seen.contains(name.as_str()) {
            Diagnostic::warning(Code::DeclarationHasNoEffect, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` is declared but does not appear in `{}`",
                    server.url
                ))
                .emit(diags);
        }
    }
    let mut docs = server.name.as_ref().map(|name| format!("Server `{name}`."));
    if let Some(description) = &server.description {
        append_text(&mut docs, description.clone());
    }
    Some(Server {
        name: server.name.clone(),
        url: server.url.clone(),
        segments,
        variables: server
            .variables
            .iter()
            .map(|(name, variable)| {
                (
                    name.clone(),
                    crate::ir::ServerVariable {
                        default: variable.default.clone(),
                        enum_values: variable.enum_values.clone(),
                        description: variable.description.clone(),
                    },
                )
            })
            .collect(),
        description: docs,
    })
}

/// Split a server URL template into literals and `{variable}` references.
///
/// An unmatched `{` is kept as literal text: the document schema constrains the template shape, so
/// there is nothing useful to diagnose here that it has not already refused.
fn parse_url_template(url: &str) -> Vec<UrlSegment> {
    let mut segments = Vec::new();
    let mut rest = url;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|at| open + at) else {
            break;
        };
        if open > 0 {
            segments.push(UrlSegment::Literal(rest[..open].to_owned()));
        }
        segments.push(UrlSegment::Variable(rest[open + 1..close].to_owned()));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(UrlSegment::Literal(rest.to_owned()));
    }
    segments
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
fn resolve_path_item(
    item: &PathItem,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Option<PathItem> {
    let Some(reference) = &item.reference else {
        return Some(item.clone());
    };
    if let Some(sibling) = item.reference_siblings.first() {
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
    let from = reference
        .provenance
        .span
        .map(|span| span.file)
        .unwrap_or(crate::diag::FileId(0));
    let mut target =
        resolver.resolve_path_item(&reference.reference, from, &reference.provenance, diags)?;
    // One level of indirection is what the specification requires implementations to support, and
    // a chain would need its own cycle guard.
    if target.reference.is_some() {
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

/// Resolve security requirement names that are URIs rather than declared component names.
fn resolve_external_security_schemes(
    document: &Document,
    resolver: &Resolver,
    schemes: &mut IndexMap<SchemeId, SecuritySchemeDef>,
    diags: &mut Diagnostics,
) {
    let mut wanted: Vec<(String, crate::diag::Provenance)> = Vec::new();
    let mut collect = |requirements: &[SecurityRequirement], at: &crate::diag::Provenance| {
        for requirement in requirements {
            for name in requirement.0.keys() {
                wanted.push((name.clone(), at.clone()));
            }
        }
    };
    collect(&document.security, &document.provenance);
    for item in document.paths.items.values() {
        for operation in item.operations.values() {
            if let Some(security) = &operation.security {
                collect(security, &operation.provenance);
            }
        }
    }
    for (name, at) in wanted {
        if schemes.contains_key(&SchemeId(name.clone())) {
            continue;
        }
        // Only a name that looks like a reference is worth resolving; a plain unknown name is an
        // ordinary undeclared-scheme error, reported at the requirement site.
        let reference = match name.strip_prefix("./") {
            Some(rest) => rest.to_owned(),
            None if name.contains('/') || name.contains('#') || name.contains(':') => name.clone(),
            None => continue,
        };
        let from = at
            .span
            .map(|span| span.file)
            .unwrap_or(crate::diag::FileId(0));
        let Some(object) = resolver.resolve_component(
            &reference,
            from,
            super::deserialize::parse_security_scheme,
            diags,
        ) else {
            continue;
        };
        let mut resolved = IndexMap::new();
        resolved.insert(name.clone(), RefOr::Item(object));
        let mut document = document.clone();
        document.components.security_schemes = resolved;
        for (id, scheme) in lower_security_schemes(&document, diags) {
            schemes.insert(id, scheme);
        }
    }
}

/// Lower `components.securitySchemes`.
///
/// Every declared scheme gets a disposition here rather than only when something references it: a
/// scheme that silently vanished used to surface — if at all — as a confusing `E012` at the
/// requirement site, naming a scheme the document plainly declares.
fn lower_security_schemes(
    document: &Document,
    diags: &mut Diagnostics,
) -> IndexMap<SchemeId, SecuritySchemeDef> {
    let mut schemes = IndexMap::new();
    for (name, scheme) in &document.components.security_schemes {
        let scheme = match scheme {
            RefOr::Item(scheme) => scheme,
            // A `$ref` to another scheme component resolves; anything else is unresolvable.
            RefOr::Ref(reference) => {
                let target = reference
                    .reference
                    .strip_prefix("#/components/securitySchemes/")
                    .and_then(|target| document.components.security_schemes.get(target))
                    .and_then(|target| match target {
                        RefOr::Item(target) => Some(target),
                        RefOr::Ref(_) => None,
                    });
                match target {
                    Some(target) => target,
                    None => {
                        Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
                            .message(format!(
                                "unresolved security scheme reference `{}`",
                                reference.reference
                            ))
                            .remedy(
                                "reference a scheme declared under \
                                 `#/components/securitySchemes/`",
                            )
                            .emit(diags);
                        continue;
                    }
                }
            }
        };
        let lowered = match scheme.scheme_type.as_str() {
            "http" => match scheme.scheme.as_deref() {
                Some("bearer") => SecurityScheme::Http(HttpScheme::Bearer),
                Some("basic") => SecurityScheme::Http(HttpScheme::Basic),
                other => {
                    // `digest`, `negotiate`, and friends need a challenge/response exchange that a
                    // statically-attached credential cannot perform.
                    Diagnostic::error(Code::UnknownSecurityScheme, scheme.provenance.clone())
                        .message(format!(
                            "`http` security scheme `{}` uses authentication scheme `{}`, which \
                             spargen cannot attach",
                            name,
                            other.unwrap_or("<missing>")
                        ))
                        .remedy(
                            "use `bearer` or `basic`, or omit this API segment with \
                             spargen::omit!",
                        )
                        .emit(diags);
                    continue;
                }
            },
            "apiKey" => {
                let location = match scheme.location.as_deref() {
                    Some("header") => ApiKeyLoc::Header,
                    Some("query") => ApiKeyLoc::Query,
                    Some("cookie") => ApiKeyLoc::Cookie,
                    // The document schema requires a valid `in` for `apiKey`.
                    _ => continue,
                };
                SecurityScheme::ApiKey {
                    location,
                    name: scheme.name.clone().unwrap_or_else(|| name.clone()),
                }
            }
            "oauth2" => SecurityScheme::OAuth2,
            "openIdConnect" => SecurityScheme::OpenIdConnect,
            "mutualTLS" => {
                Diagnostic::warning(Code::DeclarationHasNoEffect, scheme.provenance.clone())
                    .message(format!(
                        "`mutualTLS` scheme `{name}` is satisfied by the client certificate on the \
                         injected `reqwest::Client`, so no credential is registered for it"
                    ))
                    .remedy(
                        "configure the certificate on the client passed to `Client::with_client`",
                    )
                    .emit(diags);
                SecurityScheme::MutualTls
            }
            // The document schema closes the `type` enum.
            _ => continue,
        };
        schemes.insert(
            SchemeId(name.clone()),
            SecuritySchemeDef {
                kind: lowered,
                docs: security_scheme_docs(name, scheme),
            },
        );
    }
    schemes
}

/// Render the documentation a Security Scheme Object carries into rustdoc lines.
///
/// A caller of `Client::with_credential` needs exactly this to know what to register: the token
/// format, where a token is obtained, and whether the scheme is on its way out. None of it changes
/// a byte on the wire, which is why it is documentation rather than lowered structure.
fn security_scheme_docs(name: &str, scheme: &super::SecuritySchemeObject) -> Vec<String> {
    let mut docs = Vec::new();
    let kind = match scheme.scheme_type.as_str() {
        "http" => match scheme.scheme.as_deref() {
            Some(inner) => format!("`http` (`{inner}`)"),
            None => "`http`".to_owned(),
        },
        other => format!("`{other}`"),
    };
    docs.push(format!("- `{name}` — {kind}."));
    if scheme.deprecated {
        docs.push("  - **Deprecated.**".to_owned());
    }
    if let Some(description) = &scheme.description {
        docs.push(format!("  - {}", description.replace('\n', " ")));
    }
    if let Some(format) = &scheme.bearer_format {
        docs.push(format!("  - Bearer format: `{format}`."));
    }
    if let Some(url) = &scheme.open_id_connect_url {
        docs.push(format!("  - OpenID Connect discovery: <{url}>"));
    }
    if let Some(url) = &scheme.oauth2_metadata_url {
        docs.push(format!("  - OAuth 2 metadata: <{url}>"));
    }
    for flow in &scheme.flows {
        docs.push(format!("  - Flow `{}`:", flow.name));
        for (label, url) in [
            ("authorization", &flow.authorization_url),
            ("token", &flow.token_url),
            ("refresh", &flow.refresh_url),
            ("device authorization", &flow.device_authorization_url),
        ] {
            if let Some(url) = url {
                docs.push(format!("    - {label}: <{url}>"));
            }
        }
        for (scope, description) in &flow.scopes {
            let description = description.replace('\n', " ");
            if description.is_empty() {
                docs.push(format!("    - scope `{scope}`"));
            } else {
                docs.push(format!("    - scope `{scope}` — {description}"));
            }
        }
    }
    docs
}

/// Suppress `xml.name`/`xml.attribute` renames on any type that is not XML-dedicated, warning `W006`.
///
/// A serde `rename` applies to every serde format, so honoring an `xml.name`/`xml.attribute` hint on
/// a struct field also rewrites that field's JSON wire name. That is only safe when the owning type
/// is used *exclusively* as an XML body. This walks the type graph from each operation's bodies and
/// parameters, partitions types into XML-reachable and non-XML-reachable, and for any struct that
/// carries an appliable XML hint but is *not* (XML-reachable AND NOT non-XML-reachable), clears the
/// hint (restoring the property's normal wire name so JSON stays correct) and emits one `W006` — so
/// the ignored hint is never silent. XML-dedicated types keep their hints.
fn gate_xml_field_renames(
    graph: &mut TypeGraph,
    operations: &[Operation],
    diags: &mut Diagnostics,
) {
    // Cheap guard: nothing to gate (and nothing to warn) unless some field carries an XML hint.
    let any_hint = graph.iter().any(|(_, def)| {
        matches!(&def.kind, TypeKind::Struct(object)
        if object.fields.iter().any(|field| {
            field.xml.name.is_some()
                || field.xml.attribute
                || !field.xml.unsupported.is_empty()
        }))
    });
    if !any_hint {
        return;
    }

    let mut xml_roots: Vec<TypeId> = Vec::new();
    let mut non_xml_roots: Vec<TypeId> = Vec::new();
    for operation in operations {
        if let Some(body) = &operation.request_body {
            if let Some(ty) = body.ty {
                if body.media == MediaType::Xml {
                    xml_roots.push(ty.id);
                } else {
                    non_xml_roots.push(ty.id);
                }
            }
        }
        let responses = operation
            .responses
            .by_status
            .iter()
            .map(|(_, response)| response)
            .chain(operation.responses.default.as_ref());
        for response in responses {
            if let Some(ty) = response.body {
                if response.media == Some(MediaType::Xml) {
                    xml_roots.push(ty.id);
                } else {
                    non_xml_roots.push(ty.id);
                }
            }
        }
        for param in &operation.params {
            non_xml_roots.push(param.ty.id);
        }
    }

    let xml_reachable = reachable_types(graph, &xml_roots);
    let non_xml_reachable = reachable_types(graph, &non_xml_roots);

    // A hint that changes the XML wire cannot be waved through on a type that is actually
    // serialized as XML: ignoring `wrapped`, a namespace, or a text/cdata node emits structurally
    // different XML while reporting success, which is exactly the silent fourth behavior the
    // contract forbids. On a type never serialized as XML the same hint genuinely has no effect,
    // so it stays a warning and the document is not refused for it.
    let mut unsupported_reports: Vec<(bool, Provenance, String)> = Vec::new();
    for (id, def) in graph.iter() {
        let TypeKind::Struct(object) = &def.kind else {
            continue;
        };
        for field in &object.fields {
            if field.xml.unsupported.is_empty() {
                continue;
            }
            unsupported_reports.push((
                xml_reachable.contains(&id),
                def.provenance.clone(),
                format!(
                    "`{}` on property `{}`",
                    field.xml.unsupported.join("`, `"),
                    field.name.wire
                ),
            ));
        }
    }
    for (serialized_as_xml, provenance, what) in unsupported_reports {
        if serialized_as_xml {
            Diagnostic::error(Code::UnsupportedMediaType, provenance)
                .message(format!(
                    "unsupported XML hint(s) {what}: this type is serialized as XML, and ignoring \
                     the hint would put structurally different XML on the wire"
                ))
                .remedy(
                    "remove the hint, model the wrapper element explicitly as a nested object, or \
                     omit this API segment with spargen::omit!",
                )
                .emit(diags);
        } else {
            Diagnostic::warning(Code::XmlHintIgnored, provenance)
                .message(format!(
                    "unsupported XML hint(s) {what} ignored; this type is never serialized as XML, \
                     so the hint has no effect"
                ))
                .emit(diags);
        }
    }

    let to_suppress: Vec<TypeId> = graph
        .iter()
        .filter_map(|(id, def)| {
            let TypeKind::Struct(object) = &def.kind else {
                return None;
            };
            let has_apply_hint = object
                .fields
                .iter()
                .any(|field| field.xml.name.is_some() || field.xml.attribute);
            let dedicated = xml_reachable.contains(&id) && !non_xml_reachable.contains(&id);
            (has_apply_hint && !dedicated).then_some(id)
        })
        .collect();

    for id in to_suppress {
        let Some(def) = graph.get_mut(id) else {
            continue;
        };
        let provenance = def.provenance.clone();
        if let TypeKind::Struct(object) = &mut def.kind {
            for field in &mut object.fields {
                field.xml = XmlField::default();
            }
        }
        Diagnostic::warning(Code::XmlHintIgnored, provenance)
            .message(
                "`xml.name`/`xml.attribute` not applied: this schema is used as a non-XML (e.g. \
                 JSON) body — or is not used as an XML body — where the format-agnostic serde rename \
                 would corrupt the wire format; the field keeps its normal wire name",
            )
            .remedy(
                "use a schema dedicated to the XML body if the rename is required, or accept the \
                 property's normal wire name",
            )
            .emit(diags);
    }
}

/// The set of type ids transitively reachable from `roots` through the type graph's structural
/// edges (struct fields and typed `additionalProperties`, array/tuple elements, union variants).
/// A visited set makes recursive (`$ref`-cycle) types terminate.
fn reachable_types(graph: &TypeGraph, roots: &[TypeId]) -> HashSet<TypeId> {
    let mut visited = HashSet::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(def) = graph.get(id) else {
            continue;
        };
        match &def.kind {
            TypeKind::Struct(object) => {
                for field in &object.fields {
                    stack.push(field.ty.id);
                }
                if let AdditionalProps::Typed(ty) = &object.additional {
                    stack.push(ty.id);
                }
            }
            TypeKind::Array(ty) => stack.push(ty.id),
            TypeKind::Tuple(items) => stack.extend(items.iter().map(|ty| ty.id)),
            TypeKind::Union(union) => {
                stack.extend(union.variants.iter().map(|variant| variant.ty.id))
            }
            TypeKind::Primitive(_)
            | TypeKind::Enum(_)
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any => {}
        }
    }
    visited
}

fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    let essence = media_essence(media);
    match classify_media(essence) {
        Some((media, _)) => Some(media),
        None => {
            Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
                .message(format!("media type `{essence}` is not supported"))
                .emit(diags);
            None
        }
    }
}

/// `opaque` answers, without lowering anything, whether an entry's body constrains nothing — the
/// proof that an ignored alternative would decode exactly like the selection.
fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
    opaque: impl Fn(&T) -> bool,
) -> Option<(&'a str, &'a T)> {
    if content.is_empty() {
        return None;
    }
    let mut selected: Option<(u8, usize, &str, &T, MediaType)> = None;
    for (source_index, (media, value)) in content.iter().enumerate() {
        let Some((classified, rank)) = classify_media(media_essence(media)) else {
            continue;
        };
        let candidate = (rank, source_index, media.as_str(), value, classified);
        if selected
            .as_ref()
            .is_none_or(|current| (rank, source_index) < (current.0, current.1))
        {
            selected = Some(candidate);
        }
    }
    if let Some((_, _, media, value, classified)) = selected {
        // A generated method sends and decodes exactly one media type, so the alternatives are not
        // generated. That narrows the documented surface — a server that also accepts XML will only
        // ever be sent JSON — so it is reported rather than dropped in silence.
        //
        // An alternative that decodes to the very same thing narrows nothing, though. Two entries
        // decode identically when they share a codec and both constrain nothing: a ranged media
        // response offering `video/*`, `audio/*` and `application/octet-stream` is `bytes::Bytes`
        // three times over, as is `image/jpeg` beside `image/png`, and `text/plain` beside
        // `text/csv` is `String` twice. Saying a narrowing happened there is noise.
        //
        // This is decided before anything is lowered, so type identity is proved structurally
        // rather than by comparing lowered ids: within one codec, a body that constrains nothing
        // has exactly one representation, so codec identity plus proven emptiness is that proof.
        // Distinct codecs are never merged even when they happen to agree — `application/xml` and
        // `application/json` both lower to the same struct, and a client that sends only JSON to a
        // server offering both has still narrowed what the document promised.
        //
        // "Constrains nothing" has to be proved, not assumed from the media type: an
        // octet-classified alternative carrying an object schema would be *rejected* by the octet
        // gate, not turned into bytes, so suppressing it would be the silent fourth behavior
        // nothing is allowed.
        //
        // The selection's own body has to reach that shared representation too. For every codec
        // but octet-stream it must therefore constrain nothing itself — a `text/plain` body
        // carrying a string enum lowers to a typed value rather than to `String`, so an opaque
        // `text/csv` beside it really is a narrowing. Octet-stream is the exception, and it is one
        // because of the gate rather than for convenience: the gate admits only bodies that
        // collapse to `bytes::Bytes`, so every entry that survives it already shares the one
        // representation.
        let selection_is_canonical = classified == MediaType::OctetStream || opaque(value);
        let ignored: Vec<&str> = content
            .iter()
            .filter(|(candidate, _)| candidate.as_str() != media)
            .filter(|(candidate, candidate_value)| {
                !selection_is_canonical
                    || !opaque(candidate_value)
                    || classify_media(media_essence(candidate)).map(|(media, _)| media)
                        != Some(classified)
            })
            .map(|(candidate, _)| candidate.as_str())
            .collect();
        if !ignored.is_empty() {
            Diagnostic::warning(Code::AlternativeMediaIgnored, provenance.clone())
                .message(format!(
                    "`{media}` is generated; the alternative media type(s) `{}` are not",
                    ignored.join("`, `")
                ))
                .remedy(
                    "remove the alternatives, or omit this API segment with spargen::omit! and \
                     hand-write the call",
                )
                .emit(diags);
        }
        return Some((media, value));
    }
    let (media, _) = content.first()?;
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
    None
}

/// Whether a Media Type Object constrains nothing: no `schema` at all, or one that says nothing
/// about the value. On a binary media that is exactly `bytes::Bytes`.
///
/// A `$ref` is never taken as opaque — proving it would mean resolving it here, and answering
/// "unknown" as "not opaque" only costs a warning that was already being reported. That holds for
/// both places a reference can appear: a `schema: {$ref: …}`, and a 3.2 Media Type Object that is
/// *itself* a Reference Object, which parses with `schema: None` and would otherwise take the
/// no-schema arm and be called opaque on the strength of a field the `$ref` spelling never sets.
fn media_object_is_opaque(object: &MediaTypeObject) -> bool {
    if object.reference.is_some() {
        return false;
    }
    match &object.schema {
        None => true,
        Some(RefOr::Item(schema)) => schema.constrains_nothing(),
        Some(RefOr::Ref(_)) => false,
    }
}

fn media_essence(media: &str) -> &str {
    media.split(';').next().unwrap_or(media).trim()
}

/// Classify a content type into its wire codec and deterministic preference rank. Structured JSON
/// suffixes use the JSON codec; textual types use raw UTF-8 except for the two streaming framings.
/// GitHub's documented octocat representation is a textual vendor media type. Concrete members of
/// the `image`, `audio`, and `video` families are opaque octets, like the ranges naming them.
fn classify_media(essence: &str) -> Option<(MediaType, u8)> {
    if let Some(range) = classify_media_range(essence) {
        return Some(range);
    }
    let classified = match essence {
        "application/json" => (MediaType::Json, 0),
        media if media.starts_with("application/") && media.ends_with("+json") => {
            (MediaType::Json, 0)
        }
        "application/xml" | "text/xml" => (MediaType::Xml, 1),
        "multipart/form-data" => (MediaType::Multipart, 2),
        "application/x-www-form-urlencoded" => (MediaType::FormUrlEncoded, 3),
        "application/octet-stream" => (MediaType::OctetStream, 4),
        // Rank 5 is a concrete member of a binary family (`classify_binary_family`): the same
        // codec as octet-stream, ranked just below it so octet-stream wins whenever both are
        // listed.
        "text/event-stream" => (MediaType::EventStream, 7),
        "application/x-ndjson" | "application/jsonl" => (MediaType::Ndjson, 7),
        "application/json-seq" => (MediaType::JsonSequence, 7),
        media if media.starts_with("application/") && media.ends_with("+json-seq") => {
            (MediaType::JsonSequence, 7)
        }
        "application/octocat-stream" => (MediaType::Text, 6),
        media if media.starts_with("text/") => (MediaType::Text, 6),
        _ => return classify_binary_family(essence),
    };
    Some(classified)
}

/// Classify a media **range** — `type/*`, or `*/*` — which the specification permits as a `content`
/// key and which describes a whole family rather than one type.
///
/// `text/*` is the family read as raw UTF-8; every other family, `*/*` included, is opaque octets,
/// which is the only honest reading of "whatever this server detected". A range ranks below every
/// concrete media type, so a concrete sibling always outranks it.
///
/// The type before the slash must be present — `/*` names no family and stays unsupported — and is
/// matched case-insensitively, because media types are (RFC 9110 § 8.3.1) and reading `TEXT/*` as
/// binary would be silently wrong rather than loudly unsupported.
fn classify_media_range(essence: &str) -> Option<(MediaType, u8)> {
    let family = essence
        .strip_suffix("/*")
        .filter(|family| !family.is_empty())?;
    Some(if family.eq_ignore_ascii_case("text") {
        (MediaType::Text, 8)
    } else {
        (MediaType::OctetStream, 9)
    })
}

/// Classify a concrete member of a family whose every subtype is an opaque payload — `image/jpeg`,
/// `audio/mpeg`, `video/mp4`. RFC 6838 registers `image`, `audio`, and `video` as top-level types
/// for non-textual data, so bytes is the only faithful reading of any member, exactly as it is for
/// the family's range (`image/*`); the octet gate still demands a schema that collapses to
/// `bytes::Bytes`. It ranks just *below* `application/octet-stream`: the two decode identically,
/// but octet-stream is the generic spelling every document that generated before the family rule
/// existed was already selecting, so it keeps winning whenever both are listed — the selection, and
/// with it a request's wire `Content-Type`, never changes for a document that already generated.
///
/// `application/*` is deliberately not a family here: it mixes binary (`application/pdf`) with
/// textual (`application/sdp`, `application/sql`) subtypes, and reading SDP as bytes would be
/// silently wrong rather than loudly unsupported. For the same reason a subtype carrying an RFC
/// 6838 structured-syntax suffix (`image/svg+xml`, or any `+suffix`) is not claimed: the suffix
/// says the payload is a text syntax, so reading SVG as bytes would be silently wrong, and it stays
/// unsupported until a codec for the suffix exists in this position. The family is matched
/// case-insensitively for the same reason the range is (RFC 9110 § 8.3.1): `IMAGE/*` and
/// `IMAGE/JPEG` must agree.
fn classify_binary_family(essence: &str) -> Option<(MediaType, u8)> {
    let (family, subtype) = essence.split_once('/')?;
    if subtype.is_empty() || subtype.contains('*') || subtype.contains('+') {
        return None;
    }
    ["image", "audio", "video"]
        .iter()
        .any(|binary| family.eq_ignore_ascii_case(binary))
        .then_some((MediaType::OctetStream, 5))
}

fn raw_text_type_supported(graph: &TypeGraph, ty: Ty) -> bool {
    fn visit(graph: &TypeGraph, ty: Ty, seen: &mut HashSet<TypeId>) -> bool {
        if !seen.insert(ty.id) {
            return true;
        }
        let supported = match graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date))
            | Some(TypeKind::Bytes)
            | Some(TypeKind::Any) => true,
            Some(TypeKind::Enum(enumeration)) => enumeration.repr == ScalarRepr::String,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .all(|variant| visit(graph, variant.ty, seen)),
            _ => false,
        };
        seen.remove(&ty.id);
        supported
    }

    visit(graph, ty, &mut HashSet::new())
}

fn parse_status(status: &str) -> Option<StatusSpec> {
    if let Some(prefix) = status.strip_suffix("XX") {
        return Some(StatusSpec::Range(prefix.parse().ok()?));
    }
    Some(StatusSpec::Exact(status.parse().ok()?))
}

fn parse_path_template(path: &str) -> PathTemplate {
    let mut segments = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let (literal, after_literal) = rest.split_at(open);
        if !literal.is_empty() {
            segments.push(PathSegment::Literal(literal.to_owned()));
        }
        if let Some(close) = after_literal.find('}') {
            let name = &after_literal[1..close];
            segments.push(PathSegment::Param(name.to_owned()));
            rest = &after_literal[close + 1..];
        } else {
            rest = after_literal;
            break;
        }
    }
    if !rest.is_empty() {
        segments.push(PathSegment::Literal(rest.to_owned()));
    }
    PathTemplate {
        raw: path.to_owned(),
        segments,
    }
}

/// A `default` value classified into the scalar kinds that can back a Rust literal, or `Other` for
/// anything (object/array/null) that cannot.
enum RawDefault {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Other,
}

fn classify_default(value: &SpannedValue) -> RawDefault {
    match &value.node {
        Node::Bool(value) => RawDefault::Bool(*value),
        Node::Number(Number::Int(value)) => RawDefault::Int(*value),
        Node::Number(Number::UInt(value)) => {
            i64::try_from(*value).map_or(RawDefault::Float(*value as f64), RawDefault::Int)
        }
        Node::Number(Number::Float(value)) => RawDefault::Float(*value),
        Node::String(value) => RawDefault::Str(value.clone()),
        Node::Null | Node::Array(_) | Node::Object(_) => RawDefault::Other,
    }
}

/// Decide whether a classified `default` is representable against the field's lowered type: a
/// `Primitive` of the matching scalar kind, or a `ScalarEnum` value that is one of its variants.
/// The `deprecated`/`readOnly`/`writeOnly` annotations of one *property* subschema.
///
/// These are per-property annotations. Reading them from the enclosing object would both ignore a
/// property's own `deprecated: true` and mark every field of a deprecated object as deprecated;
/// an object-level annotation belongs on the type, where it already is.
fn field_flags(child: &SchemaOr) -> (bool, bool, bool) {
    match child {
        // A boolean schema carries no annotations.
        SchemaOr::Bool(_) => (false, false, false),
        SchemaOr::Schema(schema) => (schema.deprecated, schema.read_only, schema.write_only),
    }
}

fn representable_default(raw: &RawDefault, kind: Option<&TypeKind>) -> Option<DefaultValue> {
    let kind = kind?;
    match (raw, kind) {
        (RawDefault::Bool(value), TypeKind::Primitive(Prim::Bool)) => {
            Some(DefaultValue::Bool(*value))
        }
        // Width-check the literal so an out-of-range `int32` default is treated as
        // non-representable (→ W005, rustdoc-only) rather than rendered into code that fails to
        // compile. `i64` fields always fit.
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I32))
            if i32::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I64)) => Some(DefaultValue::Int(*value)),
        (RawDefault::Int(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value as f64))
        }
        (RawDefault::Float(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value))
        }
        (RawDefault::Str(value), TypeKind::Primitive(Prim::String)) => {
            Some(DefaultValue::Str(value.clone()))
        }
        (RawDefault::Str(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::String
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::String(v) if v == value)) =>
        {
            Some(DefaultValue::EnumVariant(value.clone()))
        }
        (RawDefault::Int(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Int
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Int(v) if v == value)) =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Bool(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Bool
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Bool(v) if v == value)) =>
        {
            Some(DefaultValue::Bool(*value))
        }
        _ => None,
    }
}

/// Render any `default` for a rustdoc note — nicely when it is representable against `kind`, else
/// as compact JSON. Used by the document-only positions (parameters, component roots) that never
/// serde-wire a default but must still surface it.
fn default_display_for(raw: &SpannedValue, kind: Option<&TypeKind>) -> String {
    match representable_default(&classify_default(raw), kind) {
        Some(value) => default_display(&value),
        None => raw_display(raw),
    }
}

/// Render a representable default for its rustdoc `Default:` note.
fn default_display(value: &DefaultValue) -> String {
    match value {
        DefaultValue::Bool(value) => value.to_string(),
        DefaultValue::Int(value) => value.to_string(),
        DefaultValue::Float(value) => value.to_string(),
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => value.clone(),
    }
}

/// Render an arbitrary default value as compact JSON-ish text for the rustdoc note of a
/// non-representable (`W005`) default.
fn raw_display(value: &SpannedValue) -> String {
    match &value.node {
        Node::Null => "null".to_owned(),
        Node::Bool(value) => value.to_string(),
        Node::Number(Number::Int(value)) => value.to_string(),
        Node::Number(Number::UInt(value)) => value.to_string(),
        Node::Number(Number::Float(value)) => value.to_string(),
        Node::String(value) => format!("{value:?}"),
        Node::Array(items) => {
            let items = items.iter().map(raw_display).collect::<Vec<_>>().join(", ");
            format!("[{items}]")
        }
        Node::Object(map) => {
            let entries = map
                .iter()
                .map(|(key, value)| format!("{:?}: {}", key.name, raw_display(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{entries}}}")
        }
    }
}

/// Append a note as a trailing rustdoc paragraph on a type's [`Docs`], used to surface a
/// component-root `default` on the generated named type.
fn append_doc_note(docs: &mut Docs, note: String) {
    match &mut docs.description {
        Some(description) => {
            description.push_str("\n\n");
            description.push_str(&note);
        }
        None => docs.description = Some(note),
    }
}

/// Whether a schema accepts `null`: a `"null"` member of its type array, or a `null` `enum` member
/// or `const`. Computed at component reserve time so `$ref` consumers wrap the type in `Option`,
/// and it agrees with the `nullable` that [`LowerCtx::lower_schema`]/[`LowerCtx::lower_enum`]
/// compute from the same schema.
/// One `allOf` member's contribution to the merged type: either a set of object fields (with its
/// `additionalProperties` policy and its own `required` names) to flatten, or a scalar/leaf type.
enum Contribution {
    Object {
        fields: Vec<Field>,
        additional: AdditionalProps,
        required: Vec<String>,
    },
    Scalar(Ty),
}

/// Whether a schema constrains object shape — declared/pattern properties, an `additionalProperties`
/// policy, a `required` set, or an explicit `object` type — and so contributes fields to an `allOf`
/// merge rather than a scalar.
fn schema_is_object_like(schema: &Schema) -> bool {
    !schema.properties.is_empty()
        || !schema.pattern_properties.is_empty()
        || schema.additional_properties.is_some()
        || !schema.required.is_empty()
        || schema.types.types.contains(&JsonType::Object)
}

/// Whether a non-object schema still imposes a scalar/leaf constraint (a non-null primitive type,
/// an `enum`/`const`, or `contentEncoding`) — as opposed to a pure annotation member (`{}` /
/// `{description: ...}`) that constrains nothing.
fn schema_imposes_scalar(schema: &Schema) -> bool {
    !schema.types.types.is_empty()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary")
        || !schema.one_of.is_empty()
        || !schema.any_of.is_empty()
}

fn schema_has_shape_constraint(schema: &Schema) -> bool {
    !schema.types.types.is_empty()
        || schema_is_object_like(schema)
        || schema.items.is_some()
        || !schema.prefix_items.is_empty()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary")
        || schema.reference.is_some()
        || !schema.all_of.is_empty()
}

/// The provenance of an `allOf` member for diagnostics — the schema's own provenance, or the
/// document root for a bare boolean member that carries none.
fn member_provenance(member: &SchemaOr) -> crate::diag::Provenance {
    match member {
        SchemaOr::Schema(schema) => schema.provenance.clone(),
        SchemaOr::Bool(_) => crate::diag::Provenance::new(crate::diag::JsonPointer::root(), None),
    }
}

/// Whether a union member is a null-only schema (`{type: "null"}`) — stripped from the union and
/// folded into its nullability, exactly like a `"null"` in a type array. A bare `$ref` member is
/// never null-only here (it names a component with its own shape); only an inline `type: null`
/// node with no other constraints counts.
fn member_is_null_only(member: &SchemaOr) -> bool {
    let SchemaOr::Schema(schema) = member else {
        return false;
    };
    schema.reference.is_none()
        && schema.types.types == [JsonType::Null]
        && schema.one_of.is_empty()
        && schema.any_of.is_empty()
        && schema.all_of.is_empty()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.properties.is_empty()
}

fn schema_is_nullable(schema: &Schema) -> bool {
    schema.types.types.contains(&JsonType::Null)
        || schema
            .enum_values
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| matches!(value.node, Node::Null)))
        || schema
            .const_value
            .as_ref()
            .is_some_and(|value| matches!(value.node, Node::Null))
}

fn scalar_value(value: &SpannedValue) -> Option<ScalarValue> {
    match &value.node {
        Node::Bool(value) => Some(ScalarValue::Bool(*value)),
        Node::Number(Number::Int(value)) => Some(ScalarValue::Int(*value)),
        Node::Number(Number::UInt(value)) => i64::try_from(*value).ok().map(ScalarValue::Int),
        Node::String(value) => Some(ScalarValue::String(value.clone())),
        _ => None,
    }
}
