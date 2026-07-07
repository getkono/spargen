use super::Ident;

/// The syntactic role an identifier plays, which governs both its casing and how keywords are
/// escaped (PRD D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentRole {
    /// A type name (`PascalCase`).
    Type,
    /// A struct field (`snake_case`).
    Field,
    /// An enum variant (`PascalCase`).
    Variant,
    /// A method name (`snake_case`).
    Method,
    /// A module name (`snake_case`).
    Module,
    /// A function parameter (`snake_case`).
    Param,
}

/// Produce a legal Rust [`Ident`] for `raw` in the given `role`: cased per the role, with Rust
/// keywords escaped as raw identifiers (`r#type`) where legal and via a trailing underscore
/// otherwise, and leading digits / invalid starts repaired (PRD D9).
pub fn escape(raw: &str, role: IdentRole) -> Ident {
    todo!()
}
