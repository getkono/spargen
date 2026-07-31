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
    /// Signature and request-construction bindings per operation.
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
}

/// Every binding whose spelling is part of an emitted operation's signature or request-building
/// prelude. Codegen consumes these identifiers directly, so it never maintains a parallel list of
/// generator-owned names that must stay synchronized with allocation.
#[derive(Debug)]
pub struct OperationBindings {
    /// Rust identifier for every OpenAPI parameter, in IR order. Required arguments and optional
    /// `…Params` fields occupy separate lexical scopes, but the index identifies the source item.
    pub parameters: Vec<Ident>,
    /// The optional-parameters struct argument, when the operation has optional parameters.
    pub params: Option<Ident>,
    /// The request-body argument, when the operation has a typed request body.
    pub body: Option<Ident>,
    /// Mutable path assembled before URL construction.
    pub request_path: Ident,
    /// Mutable query-pair collection assembled before URL construction.
    pub request_query: Ident,
    /// Fully constructed request URL.
    pub request_url: Ident,
    /// Mutable reqwest request builder.
    pub request_builder: Ident,
    /// Mutable cookie-fragment collection, used only by operations with cookie parameters.
    pub request_cookies: Ident,
}

/// Allocate every identifier the API needs, in one deterministic pass. Naming conflicts
/// that cannot be resolved are reported through `diags`.
pub fn allocate(api: &Api, diags: &mut Diagnostics) -> Names {
    let _ = diags;
    let mut names = Names::default();

    let mut type_scope = Scope::default();
    for (id, def) in api.types.iter() {
        names.types.insert(
            id,
            type_scope.alloc(&def.name_hint, IdentRole::Type, &def.provenance.pointer),
        );
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

        // Fixed signature arguments keep their conventional names. Required wire parameters share
        // that signature scope, while optional parameters are fields (and identically named
        // setters) on their own params struct.
        let mut required_scope = Scope::default();
        let params = operation
            .params
            .iter()
            .any(|parameter| !parameter.required)
            .then(|| required_scope.reserve("params", IdentRole::Param));
        let body = operation
            .request_body
            .as_ref()
            .and_then(|request_body| request_body.ty)
            .map(|_| required_scope.reserve("body", IdentRole::Param));
        let mut optional_scope = Scope::default();
        let mut parameter_names = Vec::with_capacity(operation.params.len());
        for parameter in &operation.params {
            let (scope, role) = if parameter.required {
                (&mut required_scope, IdentRole::Param)
            } else {
                (&mut optional_scope, IdentRole::Field)
            };
            parameter_names.push(scope.alloc(&parameter.name, role, &parameter.provenance.pointer));
        }

        // Internal request-construction locals yield to the complete emitted signature. Allocating
        // them from the same scope preserves wire-derived argument spellings and makes shadowing
        // impossible without a second hard-coded reservation channel in codegen.
        let pointer = &operation.provenance.pointer;
        let bindings = OperationBindings {
            parameters: parameter_names,
            params,
            body,
            request_path: required_scope.alloc("request_path", IdentRole::Param, pointer),
            request_query: required_scope.alloc("request_query", IdentRole::Param, pointer),
            request_url: required_scope.alloc("request_url", IdentRole::Param, pointer),
            request_builder: required_scope.alloc("request_builder", IdentRole::Param, pointer),
            request_cookies: required_scope.alloc("request_cookies", IdentRole::Param, pointer),
        };
        names
            .operation_bindings
            .insert(operation.id.clone(), bindings);
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
