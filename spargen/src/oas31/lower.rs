use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics};
use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, DefaultValue, Docs, Field, FieldDefault, HttpScheme, Info,
    MediaType, Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathSegment, PathTemplate,
    Prim, PropertyName, RequestBody, Response, Responses, ScalarEnum, ScalarRepr, ScalarValue,
    SchemeId, SecurityScheme, Server, StatusSpec, Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
};
use crate::name::synth_operation_id;
use crate::source::{Node, Number, SpannedValue};

use super::{
    Document, JsonType, ParameterObject, RefOr, RequestBodyObject, Resolver, ResponseObject,
    Schema, SchemaOr, SecurityRequirement,
};

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

    fn lower_schema(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if let Some(reference) = &schema.reference {
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                return self.ensure_component(name);
            }
            let resolved = self
                .resolver
                .resolve(reference, &schema.provenance.pointer, self.diags)
                .ok()?;
            return self.lower_schema(resolved.schema, hint);
        }

        if !schema.all_of.is_empty() {
            return self.lower_all_of(schema, hint);
        }

        if (!schema.one_of.is_empty() || !schema.any_of.is_empty())
            && schema.discriminator.is_none()
        {
            Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
                .message("oneOf/anyOf without a discriminator is not statically proven disjoint")
                .remedy("add a discriminator or omit this API segment with spargen::omit!")
                .emit(self.diags);
            return None;
        }

        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
                .message("discriminated oneOf/anyOf lowering is not implemented in this slice")
                .remedy(
                    "omit this API segment with spargen::omit! until union lowering is extended",
                )
                .emit(self.diags);
            return None;
        }

        if let Some(enumeration) = &schema.enum_values {
            return self.lower_enum(enumeration, schema, hint);
        }
        if let Some(value) = &schema.const_value {
            return self.lower_enum(std::slice::from_ref(value), schema, hint);
        }

        if schema.content_encoding.as_deref() == Some("base64") {
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
            Some(JsonType::Null) | None => self.insert_schema_type(schema, hint, TypeKind::Any),
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
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: is_required,
                deprecated: schema.deprecated,
                read_only: schema.read_only,
                write_only: schema.write_only,
                default,
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

    /// Merge an `allOf` composition (plus the enclosing schema's own sibling
    /// `properties`/`required`/`additionalProperties`) into a single typed [`TypeKind`].
    ///
    /// Members are gathered in a deterministic order — every `allOf` entry in source order, then the
    /// enclosing schema's own object siblings — flattening `$ref` members by *copying* their fields
    /// (the referenced component still exists as its own named type) and recursing into nested
    /// `allOf`. The gathered members are then combined:
    ///
    /// * **all object members** → one flattened [`Struct`]: the union of properties in first-seen
    ///   order (a property in two members with the same lowered type deduplicates; with *different*
    ///   lowered types it is irreconcilable → `E013`), the union of `required` (a property required by
    ///   any member is required), and a conservatively merged `additionalProperties` (a member that
    ///   denies unknown keys wins over `allow`/`typed`; two different typed value schemas → `E013`);
    /// * **all scalar members** that lower to the same primitive → collapse to that primitive (a
    ///   validation-only refinement like `{type: string, minLength: 5}` is a scalar member); distinct
    ///   scalars → `E013`;
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

        // All-scalar allOf: every member must lower to the same emitted type; collapse to it rather
        // than synthesizing a struct. `same_map_value_type` is the same bounded structural
        // equivalence used for typed overflow maps (equal leaf shapes, equal `$ref` ids).
        if !has_object {
            let Some(first) = scalars.first().copied() else {
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
            if scalars
                .iter()
                .any(|ty| !self.same_map_value_type(first, *ty))
            {
                return self.reject_all_of(
                    schema,
                    "`allOf` scalar members lower to different types and cannot be reconciled",
                );
            }
            // Re-emit the shared scalar as the final graph insert so the invariant holds even when
            // the allOf is a component body (the per-member scalar inserts above are left dead —
            // `#[allow(dead_code)]` on the models module — rather than threading a reserved id).
            let kind = self.graph.get(first.id).map(|def| def.kind.clone())?;
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = first.nullable;
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
            match self.merge_additional(&additional, member_additional) {
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
                        // Same property in two members: identical lowered types deduplicate; a type
                        // mismatch is irreconcilable (never silently drop or pick one).
                        if !self.same_map_value_type(existing.ty, field.ty) {
                            return self.reject_all_of(
                                schema,
                                "a property appears in multiple `allOf` members with conflicting \
                                 types",
                            );
                        }
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
            // Non-component refs resolve (or error) exactly as `lower_schema` does; treat the target
            // as an inline member.
            let resolved = self
                .resolver
                .resolve(reference, &schema.provenance.pointer, self.diags)
                .ok()?;
            let target = resolved.schema.clone();
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
        &self,
        acc: &AdditionalProps,
        next: &AdditionalProps,
    ) -> Option<AdditionalProps> {
        Some(match (acc, next) {
            (AdditionalProps::Deny, _) | (_, AdditionalProps::Deny) => AdditionalProps::Deny,
            (AdditionalProps::Typed(x), AdditionalProps::Typed(y)) => {
                if self.same_map_value_type(**x, **y) {
                    AdditionalProps::Typed(x.clone())
                } else {
                    return None;
                }
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

        // Only `null` members remained (`enum: [null]` / `const: null`): no scalar variants to
        // lower, so emit a faithful nullable `Any` rather than rejecting.
        if remainder.is_empty() {
            let mut ty = self.insert_schema_type(schema, hint, TypeKind::Any);
            ty.nullable = true;
            return Some(ty);
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

    fn lower_parameter(&mut self, parameter: &ParameterObject) -> Option<Parameter> {
        let location = match parameter.location.as_str() {
            "path" => ParamLoc::Path,
            "query" => ParamLoc::Query,
            "header" => ParamLoc::Header,
            "cookie" => ParamLoc::Cookie,
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
        let style = match style_name {
            "simple" => ParamStyle::Simple,
            "form" => ParamStyle::Form,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!("parameter style `{style_name}` is not supported"))
                .emit(self.diags);
                return None;
            }
        };
        let ty = if let Some(schema) = &parameter.schema {
            self.lower_schema_ref(schema, &parameter.name)?
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
            let ty = object
                .schema
                .as_ref()
                .and_then(|schema| self.lower_schema_ref(schema, &parameter.name))?;
            let default_display = self.param_default_display(object.schema.as_ref(), ty);
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required || location == ParamLoc::Path,
                style: ParamStyle::Content(media),
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
        let default_display = self.param_default_display(parameter.schema.as_ref(), ty);
        Some(Parameter {
            name: parameter.name.clone(),
            location,
            ty,
            required: parameter.required || location == ParamLoc::Path,
            style,
            deprecated: parameter.deprecated,
            default_display,
        })
    }

    fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        let (media_name, object) = choose_media(&body.content, &body.provenance, self.diags)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
        let ty = object
            .schema
            .as_ref()
            .and_then(|schema| self.lower_schema_ref(schema, "RequestBody"));
        if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a request body schema");
        }
        Some(RequestBody { media, ty })
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
                let ty = object
                    .schema
                    .as_ref()
                    .and_then(|schema| self.lower_schema_ref(schema, "ResponseBody"));
                if let Some(schema) = object.schema.as_ref() {
                    self.warn_structural_default_ref(schema, "a response body schema");
                }
                Some((media, ty))
            },
        );
        Some(Response {
            body: body.and_then(|(_, ty)| ty),
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
                } else {
                    let resolved = self
                        .resolver
                        .resolve(
                            &reference.reference,
                            &crate::diag::JsonPointer::root(),
                            self.diags,
                        )
                        .ok()?;
                    self.lower_schema(resolved.schema, hint)
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

fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    Some(match media.split(';').next().unwrap_or(media).trim() {
        "application/json" => MediaType::Json,
        "application/x-www-form-urlencoded" => MediaType::FormUrlEncoded,
        "application/octet-stream" => MediaType::OctetStream,
        "text/plain" => MediaType::TextPlain,
        other => {
            Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
                .message(format!("media type `{other}` is not supported"))
                .emit(diags);
            return None;
        }
    })
}

fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<(&'a str, &'a T)> {
    if content.is_empty() {
        return None;
    }
    for preferred in [
        "application/json",
        "application/x-www-form-urlencoded",
        "application/octet-stream",
        "text/plain",
    ] {
        if let Some(value) = content.get(preferred) {
            return Some((preferred, value));
        }
    }
    let (media, _) = content.first()?;
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
    None
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
    schema.types.types.iter().any(|ty| *ty != JsonType::Null)
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.content_encoding.is_some()
}

/// The provenance of an `allOf` member for diagnostics — the schema's own provenance, or the
/// document root for a bare boolean member that carries none.
fn member_provenance(member: &SchemaOr) -> crate::diag::Provenance {
    match member {
        SchemaOr::Schema(schema) => schema.provenance.clone(),
        SchemaOr::Bool(_) => crate::diag::Provenance::new(crate::diag::JsonPointer::root(), None),
    }
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
