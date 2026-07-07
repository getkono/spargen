use super::{FeatureSet, PackageMeta};

/// Synthesize the `Cargo.toml` for a standalone generated crate.
///
/// Runtime dependencies are exactly the §2.1 set — `reqwest` (no default features), `serde`,
/// `serde_json`, `bytes` — plus the feature-gated `uuid`/`time` mappings (§6.2). No spargen crate
/// ever appears, so the freestanding-output gate holds (PRD §2.1, DoD #7). The eventual
/// implementation builds this with `toml_edit` for stable formatting.
pub fn synth_cargo_toml(package: &PackageMeta, features: &FeatureSet) -> String {
    todo!()
}
