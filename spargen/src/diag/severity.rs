use serde::Serialize;

/// The severity of a [`Diagnostic`](super::Diagnostic). `#[non_exhaustive]`, so a severity can
/// be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
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
