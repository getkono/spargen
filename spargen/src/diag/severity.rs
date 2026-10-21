use serde::Serialize;

/// The severity of a [`Diagnostic`](super::Diagnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Generation fails.
    Error,
    /// Generation proceeds; the diagnostic reports a construct the generated client does not
    /// carry as declared (an ignored keyword, an omitted or dropped construct), or a step of the
    /// run that was skipped or degraded.
    Warning,
}
