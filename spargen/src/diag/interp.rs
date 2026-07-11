use serde::Serialize;

/// Identifies a numbered interpretation (`INT-###`) recording a chosen reading where the
/// normative sources are genuinely ambiguous or in tension. The prose behind each id
/// lives in the published documentation; diagnostics carry only the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct InterpId(pub u16);

impl std::fmt::Display for InterpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "INT-{:03}", self.0)
    }
}
