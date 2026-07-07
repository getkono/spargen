//! Internal token builders. Each produces a deterministically-ordered fragment of the output;
//! [`generate`](super::generate) assembles and formats them.

use proc_macro2::TokenStream;

use crate::ir::{Api, Operation};
use crate::name::Names;

use super::CodegenOptions;

/// Emit the `types` (models) module for every type in the graph, in deterministic order (PRD FR3).
pub(crate) fn emit_models(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    todo!()
}

/// Emit the `Client` struct and its `new` / `with_client` constructors (PRD FR3).
pub(crate) fn emit_client(api: &Api, names: &Names) -> TokenStream {
    todo!()
}

/// Emit one operation method — a thin `#[inline]` shim over the non-generic `support` dispatch
/// routines, so per-operation code stays tiny (PRD NFR2).
pub(crate) fn emit_operation(operation: &Operation, names: &Names) -> TokenStream {
    todo!()
}

/// Emit an operation's optional-parameters `…Params` struct, deriving `Default` (PRD D3).
pub(crate) fn emit_params_struct(operation: &Operation, names: &Names) -> TokenStream {
    todo!()
}

/// Emit an operation's typed error enum (or type alias for a single error body) (PRD FR3, FR5 #6).
pub(crate) fn emit_error_enum(operation: &Operation, names: &Names) -> TokenStream {
    todo!()
}

/// Emit the private `support` module by embedding the freestanding runtime source verbatim, under
/// `#![forbid(unsafe_code)]` (PRD §2.3 rule 3).
pub(crate) fn emit_support() -> TokenStream {
    todo!()
}
