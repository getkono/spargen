//! # Subsystem: name
//! layer-deps: ir, diag
//!
//! Deterministic identifier allocation: Rust-conventional casing via Unicode-XID-aware
//! segmentation, keyword escaping, in-scope collision resolution, and `operationId` synthesis.
//! Every allocation is deterministic and injective within its scope, and always yields a
//! valid Rust identifier — property-tested.

mod casing;
mod ident;
mod keyword;
mod scope;
mod synth;

use std::collections::HashMap;

use crate::diag::Diagnostics;
use crate::ir::{AdditionalProps, Api, OperationId, ScalarValue, TypeId, TypeKind};

pub use casing::{to_pascal_case, to_snake_case};
pub use ident::Ident;
pub use keyword::{escape, IdentRole};
pub use scope::Scope;
pub use synth::synth_operation_id;

/// The identifiers allocated for a whole [`Api`]: one per operation, params struct, type, field,
/// and variant. Codegen looks names up here rather than deriving them, so naming stays in one
/// place and stays deterministic.
#[derive(Debug, Default)]
pub struct Names {
    /// Method name per operation.
    pub operations: HashMap<OperationId, Ident>,
    /// Optional-parameters `…Params` struct name per operation.
    pub params_structs: HashMap<OperationId, Ident>,
    /// Generator-owned signature and request-building bindings per operation. Required OpenAPI
    /// parameters reserve their natural Rust spellings first, so these identifiers can never
    /// shadow caller-provided values.
    pub operation_bindings: HashMap<OperationId, OperationBindings>,
    /// Type name per type.
    pub types: HashMap<TypeId, Ident>,
    /// Field name per `(type, wire property name)`.
    pub fields: HashMap<(TypeId, String), Ident>,
    /// The synthetic `#[serde(flatten)]` overflow-map field ident per struct that has a typed
    /// `additionalProperties`/`patternProperties` map. Allocated in the struct's field scope
    /// (reserved after the declared fields) so it can never collide with a declared property named
    /// `additional`.
    pub struct_overflow: HashMap<TypeId, Ident>,
    /// Variant name per `(type, wire variant value)`.
    pub variants: HashMap<(TypeId, String), Ident>,
    /// Builder type name per declared server, by index.
    pub servers: Vec<Ident>,
    /// Enum type name per `(server index, variable name)`, for a variable with a closed `enum`.
    pub server_variable_enums: HashMap<(usize, String), Ident>,
    /// Enum variant name per `(server index, variable name, value)`.
    pub server_variable_variants: HashMap<(usize, String, String), Ident>,
    /// Setter/field name per `(server index, variable name)`.
    pub server_variable_fields: HashMap<(usize, String), Ident>,
    /// Header-struct type name per `(operation, status label)`.
    pub response_header_structs: HashMap<(OperationId, String), Ident>,
    /// Field name per `(operation, status label, header name)`.
    pub response_header_fields: HashMap<(OperationId, String, String), Ident>,
}

/// Generator-owned bindings emitted inside one operation method.
#[derive(Debug)]
pub struct OperationBindings {
    /// The optional-parameters struct argument, when one is emitted.
    pub params: Option<Ident>,
    /// The request-body argument, when one is emitted.
    pub body: Option<Ident>,
    /// Mutable path assembled before URL construction.
    pub path: Ident,
    /// Mutable query-pair collection.
    pub query: Ident,
    /// Serialized whole-query value for an `in: querystring` parameter.
    pub raw_query: Ident,
    /// Fully constructed request URL.
    pub url: Ident,
    /// Mutable request builder, then the built request.
    pub request: Ident,
    /// Clone of a streaming request retained for opt-in SSE reconnects.
    pub reconnect_request: Ident,
    /// Mutable cookie-fragment collection.
    pub cookies: Ident,
}

/// Allocate every identifier the API needs, in one deterministic pass. Naming conflicts
/// that cannot be resolved are reported through `diags`.
pub fn allocate(api: &Api, diags: &mut Diagnostics) -> Names {
    let _ = diags;
    let mut names = Names::default();

    // Servers live in their own module, so they get their own scopes and can never collide with a
    // generated model or operation name.
    let mut server_scope = Scope::default();
    let mut server_enum_scope = Scope::default();
    for (index, server) in api.servers.iter().enumerate() {
        let hint = server
            .name
            .clone()
            .unwrap_or_else(|| format!("server{index}"));
        let pointer = crate::diag::JsonPointer::root();
        names
            .servers
            .push(server_scope.alloc(&hint, IdentRole::Type, &pointer));
        let mut field_scope = Scope::default();
        for (variable_name, variable) in &server.variables {
            names.server_variable_fields.insert(
                (index, variable_name.clone()),
                field_scope.alloc(variable_name, IdentRole::Field, &pointer),
            );
            if variable.enum_values.is_empty() {
                continue;
            }
            names.server_variable_enums.insert(
                (index, variable_name.clone()),
                server_enum_scope.alloc(
                    &format!("{hint} {variable_name}"),
                    IdentRole::Type,
                    &pointer,
                ),
            );
            let mut variant_scope = Scope::default();
            for value in &variable.enum_values {
                names.server_variable_variants.insert(
                    (index, variable_name.clone(), value.clone()),
                    variant_scope.alloc(value, IdentRole::Variant, &pointer),
                );
            }
        }
    }

    let mut type_scope = Scope::default();
    for (id, def) in api.types.iter() {
        names.types.insert(
            id,
            type_scope.alloc(&def.name_hint, IdentRole::Type, &def.provenance.pointer),
        );
    }

    // Response-header structs live in the same scope as the other per-operation types, so a
    // documented header can never collide with a generated model.
    for operation in &api.operations {
        let responses = operation
            .responses
            .by_status
            .iter()
            .map(|(spec, response)| (status_label(Some(*spec)), response))
            .chain(
                operation
                    .responses
                    .default
                    .as_ref()
                    .map(|response| (status_label(None), response)),
            );
        for (label, response) in responses {
            if response.headers.is_empty() {
                continue;
            }
            let hint = format!("{} {label} headers", operation.id.0);
            let pointer = crate::diag::JsonPointer::root();
            names.response_header_structs.insert(
                (operation.id.clone(), label.clone()),
                type_scope.alloc(&hint, IdentRole::Type, &pointer),
            );
            let mut field_scope = Scope::default();
            for header in &response.headers {
                names.response_header_fields.insert(
                    (operation.id.clone(), label.clone(), header.name.clone()),
                    field_scope.alloc(&header.name, IdentRole::Field, &pointer),
                );
            }
        }
    }

    let mut operation_scope = Scope::default();
    let mut params_scope = Scope::default();
    for operation in &api.operations {
        names.operations.insert(
            operation.id.clone(),
            operation_scope.alloc(
                &operation.id.0,
                IdentRole::Method,
                &operation.provenance.pointer,
            ),
        );
        names.params_structs.insert(
            operation.id.clone(),
            params_scope.alloc(
                &format!("{} params", operation.id.0),
                IdentRole::Type,
                &operation.provenance.pointer,
            ),
        );

        // Required parameters are fixed by the generated public surface. Reserve their natural
        // spellings, then allocate every generator-owned binding in the same lexical scope so the
        // implementation yields on collision without renaming ordinary arguments.
        let mut binding_scope = Scope::default();
        for parameter in operation
            .params
            .iter()
            .filter(|parameter| parameter.required)
        {
            binding_scope.reserve(&parameter.name, IdentRole::Param);
        }
        let pointer = &operation.provenance.pointer;
        let params = operation
            .params
            .iter()
            .any(|parameter| !parameter.required)
            .then(|| binding_scope.alloc("params", IdentRole::Param, pointer));
        let body = operation
            .request_body
            .as_ref()
            .and_then(|request_body| request_body.ty)
            .map(|_| binding_scope.alloc("body", IdentRole::Param, pointer));
        names.operation_bindings.insert(
            operation.id.clone(),
            OperationBindings {
                params,
                body,
                path: binding_scope.alloc("path", IdentRole::Param, pointer),
                query: binding_scope.alloc("query", IdentRole::Param, pointer),
                raw_query: binding_scope.alloc("raw_query", IdentRole::Param, pointer),
                url: binding_scope.alloc("url", IdentRole::Param, pointer),
                request: binding_scope.alloc("request", IdentRole::Param, pointer),
                reconnect_request: binding_scope.alloc(
                    "reconnect_request",
                    IdentRole::Param,
                    pointer,
                ),
                cookies: binding_scope.alloc("cookies", IdentRole::Param, pointer),
            },
        );
    }

    for (id, def) in api.types.iter() {
        match &def.kind {
            TypeKind::Struct(object) => {
                let mut scope = Scope::default();
                for field in &object.fields {
                    names.fields.insert(
                        (id, field.name.wire.clone()),
                        scope.alloc(&field.name.wire, IdentRole::Field, &def.provenance.pointer),
                    );
                }
                // The flatten overflow field shares the struct's field scope, so it is disambiguated
                // against any declared property (e.g. one named `additional`) instead of emitting a
                // second literal `additional` field that would fail to compile.
                if matches!(object.additional, AdditionalProps::Typed(_)) {
                    names.struct_overflow.insert(
                        id,
                        scope.alloc("additional", IdentRole::Field, &def.provenance.pointer),
                    );
                }
            }
            TypeKind::Enum(enumeration) => {
                let mut scope = Scope::default();
                for variant in &enumeration.variants {
                    let value = match variant {
                        ScalarValue::Bool(value) => value.to_string(),
                        ScalarValue::Int(value) => value.to_string(),
                        ScalarValue::String(value) => value.clone(),
                    };
                    names.variants.insert(
                        (id, value.clone()),
                        scope.alloc(&value, IdentRole::Variant, &def.provenance.pointer),
                    );
                }
            }
            TypeKind::Union(union) => {
                // Union variants share the scalar-enum `variants` table, keyed by `(TypeId, hint)`.
                // A type id is either an enum or a union, so the two never collide; hints are made
                // unique per union at lowering time, keeping this allocation injective in scope.
                let mut scope = Scope::default();
                for variant in &union.variants {
                    names.variants.insert(
                        (id, variant.name_hint.clone()),
                        scope.alloc(
                            &variant.name_hint,
                            IdentRole::Variant,
                            &def.provenance.pointer,
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    names
}

/// The stable label for one documented status, shared by naming and codegen so a header struct and
/// its response variant always agree.
pub fn status_label(spec: Option<crate::ir::StatusSpec>) -> String {
    match spec {
        Some(crate::ir::StatusSpec::Exact(code)) => format!("Status{code}"),
        Some(crate::ir::StatusSpec::Range(0)) | None => "Default".to_owned(),
        Some(crate::ir::StatusSpec::Range(prefix)) => format!("Status{prefix}xx"),
    }
}
