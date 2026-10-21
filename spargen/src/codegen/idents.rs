//! The identifiers an emitted item is declared or referenced by: lookups into the
//! [`Names`] table `name` allocated, and the status-selector variant names derived from a
//! [`StatusSpec`](crate::ir::StatusSpec). Each lookup here is written once, key and failure
//! message included, for the operation, blocking-client, parameter, response and dispatch
//! emitters that read it.

use quote::format_ident;

use crate::ir::Operation;
use crate::name::{Names, OperationBindings};

/// The method identifier of an operation on `Client` and `BlockingClient`, as
/// [`Names::operations`] allocates it.
pub(super) fn operation_method_ident<'a>(
    operation: &Operation,
    names: &'a Names,
) -> &'a crate::name::Ident {
    names
        .operations
        .get(&operation.id)
        .expect("operation name allocated")
}

/// The type name of an operation's optional-parameters `…Params` struct, as
/// [`Names::params_structs`] allocates it.
pub(super) fn params_struct_ident<'a>(
    operation: &Operation,
    names: &'a Names,
) -> &'a crate::name::Ident {
    names
        .params_structs
        .get(&operation.id)
        .expect("params name allocated")
}

/// The type name of an operation's multi-status success response enum, `{Method}Response` as
/// [`Names::success_enums`] allocates it.
pub(super) fn success_enum_ident<'a>(
    operation: &Operation,
    names: &'a Names,
) -> &'a crate::name::Ident {
    names
        .success_enums
        .get(&operation.id)
        .expect("success enum name allocated for an enum-shaped success")
}

/// The type name of an operation's error type, `{Method}Error` or `{Method}OperationError` as
/// [`Names::operation_errors`] allocates it.
pub(super) fn error_type_ident<'a>(
    operation: &Operation,
    names: &'a Names,
) -> &'a crate::name::Ident {
    names
        .operation_errors
        .get(&operation.id)
        .expect("error type name allocated")
}

/// The `PascalCase` variant identifier for a documented status selector: `Status200` for an exact
/// code, `Status2xx` for a range, and `Default` for [`StatusSpec::Default`](crate::ir::StatusSpec)
/// — the label [`crate::name::status_label`] gives the same selector's header struct.
/// Deterministic and, within one enum, unique by construction (each selector appears once).
/// Routed through the `name` escaper for validity.
pub(super) fn status_variant_ident(spec: crate::ir::StatusSpec) -> proc_macro2::Ident {
    let raw = crate::name::status_label(spec);
    format_ident!(
        "{}",
        crate::name::escape(&raw, crate::name::IdentRole::Variant).as_str()
    )
}

/// The local and argument identifiers `name` allocated for one operation method's body.
pub(super) fn operation_bindings<'a>(
    operation: &Operation,
    names: &'a Names,
) -> &'a OperationBindings {
    names
        .operation_bindings
        .get(&operation.id)
        .expect("operation bindings allocated")
}

/// The identifier `name` allocated for `param`: its method argument when it is required, its
/// `…Params` field and setter when it is optional. Looked up rather than escaped here, so two
/// parameters whose names escape alike still get distinct identifiers.
///
/// `param` must be borrowed from `operation.params`; the allocation is positional, and it is found
/// by address so that two parameters sharing a wire name in different locations stay apart.
pub(super) fn param_ident<'a>(
    names: &'a Names,
    operation: &Operation,
    param: &crate::ir::Parameter,
) -> &'a crate::name::Ident {
    let index = operation
        .params
        .iter()
        .position(|candidate| std::ptr::eq(candidate, param))
        .expect("a parameter is borrowed from its own operation");
    &names
        .parameters
        .get(&operation.id)
        .expect("parameter names allocated")[index]
}
