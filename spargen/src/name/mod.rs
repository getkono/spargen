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
    /// Rust identifier for every operation parameter, in the IR's parameter order per operation.
    /// Required arguments and optional `…Params` fields have separate lexical scopes but share
    /// this lookup table because the index identifies which scope allocated the name.
    pub parameters: HashMap<OperationId, Vec<Ident>>,
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

        // Required parameters live in the generated method body. Reserve every binding codegen
        // owns before allocating wire-derived argument names, so a parameter can never shadow a
        // value used to construct the request. `body` and `params` are reserved only when that
        // operation emits those arguments, preserving ordinary parameter names otherwise.
        let mut required_scope = Scope::default();
        for binding in [
            "request_path",
            "request_query",
            "request_url",
            "request_builder",
            "request_cookies",
        ] {
            required_scope.reserve(binding, IdentRole::Param);
        }
        if operation.request_body.is_some() {
            required_scope.reserve("body", IdentRole::Param);
        }
        if operation.params.iter().any(|parameter| !parameter.required) {
            required_scope.reserve("params", IdentRole::Param);
        }

        // Optional parameters are fields (and identically named setters) on their own params
        // struct, so allocate them in a separate scope rather than needlessly disambiguating them
        // against required method arguments.
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
        names
            .parameters
            .insert(operation.id.clone(), parameter_names);
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
