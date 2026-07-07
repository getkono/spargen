use serde::Serialize;

/// Identifies a numbered interpretation (`INT-###`) recording a chosen reading where the
/// normative sources are genuinely ambiguous or in tension (PRD §3.3, §2.3 rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct InterpId(pub u16);

impl std::fmt::Display for InterpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "INT-{:03}", self.0)
    }
}

/// A documented interpretation of the normative specs. The registry is the single source of
/// truth: both the published docs and any diagnostic whose behavior depends on the reading link
/// to it.
#[derive(Debug, Clone, Copy)]
pub struct Interpretation {
    /// The stable identifier.
    pub id: InterpId,
    /// One-line title.
    pub title: &'static str,
    /// The full explanation of the chosen reading.
    pub text: &'static str,
    /// References into the vendored spec texts this interpretation resolves (PRD §3.3).
    pub spec_refs: &'static [&'static str],
}

/// Look up an interpretation by id.
pub fn interpretation(id: InterpId) -> Option<&'static Interpretation> {
    todo!()
}

/// Every interpretation, in stable order — for docs generation and exhaustiveness tests.
pub fn all_interpretations() -> &'static [Interpretation] {
    todo!()
}
