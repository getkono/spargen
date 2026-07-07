//! Casing conversions using Unicode-XID-aware segmentation (PRD D9). `heck` is deliberately not
//! used — it is not Unicode-XID-correct, which D9 requires.

/// Convert `raw` to `PascalCase` (for types and variants).
pub fn to_pascal_case(raw: &str) -> String {
    todo!()
}

/// Convert `raw` to `snake_case` (for fields, methods, modules).
pub fn to_snake_case(raw: &str) -> String {
    todo!()
}

/// Convert `raw` to `SHOUTY_SNAKE_CASE` (for constants).
pub fn to_shouty_snake_case(raw: &str) -> String {
    todo!()
}
