//! # Subsystem: name
//! layer-deps: ir, diag
//!
//! Deterministic identifier allocation: Rust-conventional casing via Unicode-XID-aware
//! segmentation, keyword escaping, in-scope collision resolution, and `operationId` synthesis
//! (PRD D9). Every allocation is deterministic and injective within its scope, and always yields a
//! valid Rust identifier — property-tested (§7.5).

mod casing;
mod ident;
mod keyword;
mod scope;
mod synth;

use std::collections::HashMap;

use crate::diag::Diagnostics;
use crate::ir::{Api, OperationId, TypeId};

pub use casing::{to_pascal_case, to_shouty_snake_case, to_snake_case};
pub use ident::Ident;
pub use keyword::{escape, IdentRole};
pub use scope::Scope;
pub use synth::synth_operation_id;

/// The identifiers allocated for a whole [`Api`]: one per operation, params struct, type, field,
/// and variant. Codegen looks names up here rather than deriving them, so naming stays in one
/// place and stays deterministic (PRD FR3, D9).
#[derive(Debug, Default)]
pub struct Names {
    /// Method name per operation.
    pub operations: HashMap<OperationId, Ident>,
    /// Optional-parameters `…Params` struct name per operation.
    pub params_structs: HashMap<OperationId, Ident>,
    /// Type name per type.
    pub types: HashMap<TypeId, Ident>,
    /// Field name per `(type, wire property name)`.
    pub fields: HashMap<(TypeId, String), Ident>,
    /// Variant name per `(type, wire variant value)`.
    pub variants: HashMap<(TypeId, String), Ident>,
}

/// Allocate every identifier the API needs, in one deterministic pass (PRD D9). Naming conflicts
/// that cannot be resolved are reported through `diags`.
pub fn allocate(api: &Api, diags: &mut Diagnostics) -> Names {
    todo!()
}
