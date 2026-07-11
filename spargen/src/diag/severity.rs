use serde::Serialize;

/// The severity of a [`Diagnostic`](super::Diagnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Generation fails.
    Error,
    /// Generation proceeds; the construct affects runtime validation but not the static shape.
    Warning,
}
