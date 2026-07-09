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

/// The support-matrix class of an OpenAPI / JSON-Schema construct (PRD FR2). Every keyword has
/// exactly one disposition — there is no fourth, undefined behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    /// **S** — faithfully represented in generated types/behavior.
    Supported,
    /// **W** — ignored with a warning; affects validation, not the static shape of data.
    Warned,
    /// **R** — rejected; affects data shape or wire behavior in a way spargen does not represent.
    Rejected,
}

impl Disposition {
    /// The single-letter code used in the published support matrix (`S` / `W` / `R`).
    pub fn letter(self) -> char {
        match self {
            Disposition::Supported => 'S',
            Disposition::Warned => 'W',
            Disposition::Rejected => 'R',
        }
    }

    /// The severity a diagnostic for this disposition carries, if any (`Rejected` → error,
    /// `Warned` → warning, `Supported` → none).
    pub fn severity(self) -> Option<Severity> {
        match self {
            Disposition::Supported => None,
            Disposition::Warned => Some(Severity::Warning),
            Disposition::Rejected => Some(Severity::Error),
        }
    }
}
