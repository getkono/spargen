//! The status-selector variant names derived from a [`StatusSpec`](crate::ir::StatusSpec), read by
//! the response and dispatch emitters. Every other identifier an emitted item is declared or
//! referenced by is looked up in the [`Names`](crate::name::Names) table `name` allocated.

use quote::format_ident;

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
