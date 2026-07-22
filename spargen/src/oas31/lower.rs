use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics};
use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, DefaultValue, DisjointFeature, Docs, Field, FieldDefault,
    HttpScheme, Info, JsonCategory, MediaType, Operation, OperationId, ParamLoc, ParamStyle,
    Parameter, PathSegment, PathTemplate, Prim, PropertyName, RequestBody, Response, Responses,
    ScalarEnum, ScalarRepr, ScalarValue, SchemeId, SecurityScheme, Server, StatusSpec, Struct, Ty,
    TypeDef, TypeGraph, TypeId, TypeKind, Union, UnionMode, UnionStrategy, UnionVariant, XmlField,
};
use crate::name::synth_operation_id;
use crate::source::{is_remote_ref, Node, Number, SpannedValue};

use super::{
    Document, JsonType, ParameterObject, RefOr, RequestBodyObject, Resolver, ResponseObject,
    Schema, SchemaOr, SecurityRequirement,
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

/// Lower the typed 3.1.1 [`Document`] into the version-agnostic [`Api`] IR.
pub fn lower(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Result<Api, Aborted> {
    let security_schemes = lower_security_schemes(document);
    let mut ctx = LowerCtx {
        document,
        resolver,
        diags,
        graph: TypeGraph::default(),
        components: HashMap::new(),
        in_progress: HashMap::new(),
        remote_components: HashMap::new(),
        remote_in_progress: HashMap::new(),
        remote_alias_stack: HashSet::new(),
        depth: 0,
    };

    for (name, schema) in &document.components.schemas {
        if matches!(schema, RefOr::Item(_)) {
            let _ = ctx.ensure_component(name);
        }
    }

    let mut operations = Vec::new();
    for (path, item) in &document.paths.items {
        for (method, operation) in &item.operations {
            let path_template = parse_path_template(path);
            let id = operation
                .operation_id
                .clone()
                .unwrap_or_else(|| synth_operation_id(*method, &path_template));

            let mut params = Vec::new();
            for parameter in item.parameters.iter().chain(operation.parameters.iter()) {
                if let Some(parameter) = ctx.resolve_parameter(parameter) {
                    if let Some(parameter) = ctx.lower_parameter(&parameter) {
                        params.push(parameter);
                    }
                }
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

            operations.push(Operation {
                id: OperationId(id),
                method: *method,
                path: path_template,
                params,
                request_body,
                responses,
                security,
                deprecated: operation.deprecated,
                docs: Docs {
                    title: None,
                    summary: operation.summary.clone(),
                    description: operation.description.clone(),
                    deprecated: operation.deprecated,
                },
                provenance: operation.provenance.clone(),
            });
        }
    }

    // `xml.name`/`xml.attribute` become a format-agnostic serde `rename`, so they may only be applied
    // to a schema used *exclusively* as an XML body — otherwise the rename would corrupt the JSON
    // wire format. Suppress (and warn `W006` on) the rename for any shared/non-XML-reachable type.
    gate_xml_field_renames(&mut ctx.graph, &operations, ctx.diags);

    let api = Api {
        info: Info {
            title: document.info.title.clone(),
            version: document.info.version.clone(),
            description: document
                .info
                .description
                .clone()
                .or(document.info.summary.clone()),
        },
        servers: document
            .servers
            .iter()
            .map(|server| Server {
                url: server.url.clone(),
                description: server.description.clone(),
            })
            .collect(),
        operations,
        types: ctx.graph,
        security_schemes,
    };
    ctx.diags.into_result(api)
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
        let RefOr::Item(schema) = self.document.components.schemas.get(name)? else {
            return None;
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
            .resolve(reference, &crate::diag::JsonPointer::root(), self.diags)
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
                Diagnostic::error(Code::InvalidInput, self.document.provenance.clone())
                    .message(
                        "boolean false schemas are not representable in generated client types",
                    )
                    .emit(self.diags);
                None
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
        if let Some(reference) = &schema.reference {
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                return self.ensure_component(name);
            }
            // Remote refs go through the cycle-safe, deduped remote path (keyed by `url#fragment`),
            // mirroring `ensure_component`; a bare relative/other ref falls through to `resolve`,
            // which reports it (E003/E004).
            if is_remote_ref(reference) {
                return self.ensure_remote(reference);
            }
            let resolved = self
                .resolver
                .resolve(reference, &schema.provenance.pointer, self.diags)
                .ok()?;
            return self.lower_schema(&resolved.schema, hint);
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
                    // removes it from the union's accepted set.
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
        Some(UnionStrategy::Discriminated {
            tag_field: discriminator.property_name.clone(),
            tags,
            categories,
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
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: is_required,
                deprecated: schema.deprecated,
                read_only: schema.read_only,
                write_only: schema.write_only,
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

    /// Lower a property's OpenAPI `xml` hints into the field's [`XmlField`]. Only `xml.name` and
    /// `xml.attribute` are represented (applied as a serde rename at emit time); the unsupported
    /// `xml.namespace`/`xml.prefix`/`xml.wrapped` hints are warned once as `W006` and otherwise
    /// ignored — never silently honored. A `$ref` property carries no inline `xml` object here.
    fn field_xml(&mut self, child: &SchemaOr) -> XmlField {
        let SchemaOr::Schema(schema) = child else {
            return XmlField::default();
        };
        let Some(hints) = &schema.xml else {
            return XmlField::default();
        };
        let mut unsupported: Vec<&str> = Vec::new();
        if hints.namespace.is_some() {
            unsupported.push("namespace");
        }
        if hints.prefix.is_some() {
            unsupported.push("prefix");
        }
        if hints.wrapped {
            unsupported.push("wrapped");
        }
        if !unsupported.is_empty() {
            Diagnostic::warning(Code::XmlHintIgnored, schema.provenance.clone())
                .message(format!(
                    "unsupported XML hint(s) `{}` ignored; only `xml.name` and `xml.attribute` are \
                     honored",
                    unsupported.join("`, `")
                ))
                .remedy(
                    "remove the unsupported xml hint, or accept that the field serializes by its \
                     local name without a namespace/prefix/array wrapper",
                )
                .emit(self.diags);
        }
        XmlField {
            name: hints.name.clone(),
            attribute: hints.attribute,
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
                .resolve(reference, &schema.provenance.pointer, self.diags)
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
            } => UnionStrategy::Discriminated {
                tag_field: tag_field.clone(),
                tags: retained.iter().map(|index| tags[*index].clone()).collect(),
                categories: retained.iter().map(|index| categories[*index]).collect(),
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
            // OpenAPI 3.2's `in: querystring` treats the whole URL query string as a single
            // `content`-typed value. spargen does not model that; acknowledge it with `W010` and
            // skip the parameter so the rest of the operation still generates the compatible subset.
            "querystring" => {
                Diagnostic::warning(Code::Oas32ConstructIgnored, parameter.provenance.clone())
                    .message(
                        "`in: querystring` (OpenAPI 3.2) treats the entire query string as one \
                         value; this parameter is not generated",
                    )
                    .emit(self.diags);
                return None;
            }
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
        let style_name = parameter.style.as_deref().unwrap_or(match location {
            ParamLoc::Path | ParamLoc::Header => "simple",
            ParamLoc::Query | ParamLoc::Cookie => "form",
        });
        let style = match (location, style_name) {
            (ParamLoc::Path | ParamLoc::Header, "simple") => ParamStyle::Simple,
            (ParamLoc::Query | ParamLoc::Cookie, "form") => ParamStyle::Form,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "parameter style `{style_name}` is not supported for `{}` parameters",
                    parameter.location
                ))
                .emit(self.diags);
                return None;
            }
        };
        if parameter.allow_reserved {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message("`allowReserved: true` parameter encoding is not supported")
            .emit(self.diags);
            return None;
        }
        let explode = parameter
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form));
        let ty = if let Some(schema) = &parameter.schema {
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            self.remap_binary_param(ty, &parameter.name)
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
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
            explode,
            deprecated: parameter.deprecated,
            default_display,
        })
    }

    fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        let (media_name, object) = choose_media(&body.content, &body.provenance, self.diags)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
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
        Some(RequestBody {
            media,
            content_type: media_essence(media_name).to_owned(),
            ty,
        })
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
        let body = choose_media(&response.content, &response.provenance, self.diags).and_then(
            |(media_name, object)| {
                let media = lower_media_type(media_name, &response.provenance, self.diags)?;
                // For a sequential/streaming media (`text/event-stream` / `application/x-ndjson`),
                // OpenAPI 3.2 gives the PER-ITEM type in `itemSchema`; a whole-body `schema` does not
                // apply to a stream, so `itemSchema` is preferred (falling back to `schema` for the
                // pre-3.2 form where the item type was written as `schema`). On a non-streaming media
                // `itemSchema` is meaningless: acknowledge it with `W010` and use `schema`.
                let schema_source = if media.stream_framing().is_some() {
                    object.item_schema.as_ref().or(object.schema.as_ref())
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
                    object.schema.as_ref()
                };
                let ty =
                    schema_source.and_then(|schema| self.lower_schema_ref(schema, "ResponseBody"));
                if let Some(schema) = schema_source {
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
                Some((media, ty))
            },
        );
        // A streaming response media (`text/event-stream` / `application/x-ndjson`) records its
        // framing; the body is then the streamed item type `T`. A whole-body response has no
        // framing. Streaming only takes effect when this is the operation's single success body
        // (see `Responses::stream_success`).
        let stream = body.and_then(|(media, _)| media.stream_framing());
        Some(Response {
            media: body.map(|(media, _)| media),
            body: body.and_then(|(_, ty)| ty),
            stream,
        })
    }

    fn resolve_parameter(&self, parameter: &RefOr<ParameterObject>) -> Option<ParameterObject> {
        match parameter {
            RefOr::Item(parameter) => Some(parameter.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/parameters/")
                .and_then(|name| self.document.components.parameters.get(name))
                .and_then(|parameter| match parameter {
                    RefOr::Item(parameter) => Some(parameter.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn resolve_request_body(&self, body: &RefOr<RequestBodyObject>) -> Option<RequestBodyObject> {
        match body {
            RefOr::Item(body) => Some(body.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/requestBodies/")
                .and_then(|name| self.document.components.request_bodies.get(name))
                .and_then(|body| match body {
                    RefOr::Item(body) => Some(body.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn resolve_response(&self, response: &RefOr<ResponseObject>) -> Option<ResponseObject> {
        match response {
            RefOr::Item(response) => Some(response.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/responses/")
                .and_then(|name| self.document.components.responses.get(name))
                .and_then(|response| match response {
                    RefOr::Item(response) => Some(response.clone()),
                    RefOr::Ref(_) => None,
                }),
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
                        .resolve(
                            &reference.reference,
                            &crate::diag::JsonPointer::root(),
                            self.diags,
                        )
                        .ok()?;
                    self.lower_schema(&resolved.schema, hint)
                }
            }
        }
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

fn lower_security_schemes(document: &Document) -> IndexMap<SchemeId, SecurityScheme> {
    let mut schemes = IndexMap::new();
    for (name, scheme) in &document.components.security_schemes {
        let RefOr::Item(scheme) = scheme else {
            continue;
        };
        let lowered = match scheme.scheme_type.as_str() {
            "http" => match scheme.scheme.as_deref() {
                Some("bearer") => SecurityScheme::Http(HttpScheme::Bearer),
                Some("basic") => SecurityScheme::Http(HttpScheme::Basic),
                _ => continue,
            },
            "apiKey" => {
                let location = match scheme.location.as_deref() {
                    Some("header") => ApiKeyLoc::Header,
                    Some("query") => ApiKeyLoc::Query,
                    Some("cookie") => ApiKeyLoc::Cookie,
                    _ => continue,
                };
                SecurityScheme::ApiKey {
                    location,
                    name: scheme.name.clone().unwrap_or_else(|| name.clone()),
                }
            }
            "oauth2" => SecurityScheme::OAuth2,
            "openIdConnect" => SecurityScheme::OpenIdConnect,
            _ => continue,
        };
        schemes.insert(SchemeId(name.clone()), lowered);
    }
    schemes
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
            if object.fields.iter().any(|field| field.xml.name.is_some() || field.xml.attribute))
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

fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<(&'a str, &'a T)> {
    if content.is_empty() {
        return None;
    }
    let mut selected: Option<(u8, usize, &str, &T)> = None;
    for (source_index, (media, value)) in content.iter().enumerate() {
        let Some((_, rank)) = classify_media(media_essence(media)) else {
            continue;
        };
        let candidate = (rank, source_index, media.as_str(), value);
        if selected
            .as_ref()
            .is_none_or(|current| (rank, source_index) < (current.0, current.1))
        {
            selected = Some(candidate);
        }
    }
    if let Some((_, _, media, value)) = selected {
        return Some((media, value));
    }
    let (media, _) = content.first()?;
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
    None
}

fn media_essence(media: &str) -> &str {
    media.split(';').next().unwrap_or(media).trim()
}

/// Classify a content type into its wire codec and deterministic preference rank. Structured JSON
/// suffixes use the JSON codec; textual types use raw UTF-8 except for the two streaming framings.
/// GitHub's documented octocat representation is a textual vendor media type.
fn classify_media(essence: &str) -> Option<(MediaType, u8)> {
    let classified = match essence {
        "application/json" => (MediaType::Json, 0),
        media if media.starts_with("application/") && media.ends_with("+json") => {
            (MediaType::Json, 0)
        }
        "application/xml" | "text/xml" => (MediaType::Xml, 1),
        "multipart/form-data" => (MediaType::Multipart, 2),
        "application/x-www-form-urlencoded" => (MediaType::FormUrlEncoded, 3),
        "application/octet-stream" => (MediaType::OctetStream, 4),
        "text/event-stream" => (MediaType::EventStream, 6),
        "application/x-ndjson" => (MediaType::Ndjson, 6),
        "application/octocat-stream" => (MediaType::Text, 5),
        media if media.starts_with("text/") => (MediaType::Text, 5),
        _ => return None,
    };
    Some(classified)
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
