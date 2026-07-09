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
    INTERPRETATIONS
        .iter()
        .find(|interpretation| interpretation.id == id)
}

/// Every interpretation, in stable order — for docs generation and exhaustiveness tests.
pub fn all_interpretations() -> &'static [Interpretation] {
    INTERPRETATIONS
}

const INTERPRETATIONS: &[Interpretation] = &[
    Interpretation {
        id: InterpId(1),
        title: "3.1 patch releases share the 3.1 feature set",
        text: "Documents declaring any OpenAPI 3.1 patch version are accepted and interpreted by the active vendored 3.1 reference. Other minor lines are rejected.",
        spec_refs: &["references/3.1.2.md", "references/README.md"],
    },
    Interpretation {
        id: InterpId(2),
        title: "Validation-only JSON Schema keywords are annotations for clients",
        text: "Keywords such as pattern, minimum, and maxLength constrain validation but do not change the static Rust data shape emitted for a client.",
        spec_refs: &["references/3.1.2.md", "docs/prd.md#fr2"],
    },
    Interpretation {
        id: InterpId(3),
        title: "Undiscriminated unions must be order-independent",
        text: "Spargen never emits serde untagged unions for overlapping variants because first-match behavior can silently misparse responses.",
        spec_refs: &["references/3.1.2.md", "docs/prd.md#fr2"],
    },
];
