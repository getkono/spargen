use serde::Serialize;

/// The severity of a [`Diagnostic`](super::Diagnostic): an error or a warning, the split
/// [`Report::errors`](crate::Report::errors) and [`Report::warnings`](crate::Report::warnings)
/// partition a report on. The enum is exhaustive, so a caller's `match` names both variants and
/// needs no wildcard arm:
///
/// ```
/// use spargen::Severity;
///
/// fn cargo_directive(severity: Severity) -> &'static str {
///     match severity {
///         Severity::Error => "cargo::error=",
///         Severity::Warning => "cargo::warning=",
///     }
/// }
///
/// assert_eq!(cargo_directive(Severity::Warning), "cargo::warning=");
/// ```
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

impl Severity {
    /// The stable lowercase string form (`error` / `warning`). [`Display`](std::fmt::Display)
    /// writes it, the serialized form (`#[serde(rename_all = "lowercase")]`) spells each variant
    /// the same way, and a [`Diagnostic`](super::Diagnostic)'s rendering opens with it.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
