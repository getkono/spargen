use super::{FeatureSet, PackageMeta};

/// Synthesize the `Cargo.toml` for a standalone generated crate.
///
/// Runtime dependencies are exactly the near-universal set — `reqwest` (no default features),
/// `serde`, `serde_json`, `bytes` — plus the feature-gated `uuid`/`time` mappings. No spargen crate
/// ever appears, so the freestanding-output gate holds. The eventual
/// implementation builds this with `toml_edit` for stable formatting.
pub fn synth_cargo_toml(package: &PackageMeta, features: &FeatureSet) -> String {
    let default_features = match (features.uuid, features.time) {
        (true, true) => r#""uuid", "time""#,
        (true, false) => r#""uuid""#,
        (false, true) => r#""time""#,
        (false, false) => "",
    };
    format!(
        r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"
license = "MIT OR Apache-2.0"

[features]
default = [{default_features}]
uuid = ["dep:uuid"]
time = ["dep:time"]

[dependencies]
bytes = "1"
reqwest = {{ version = "0.12", default-features = false, features = ["json"] }}
secrecy = "0.10"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
uuid = {{ version = "1", features = ["serde"], optional = true }}
time = {{ version = "0.3", features = ["serde", "formatting", "parsing"], optional = true }}
"#,
        name = package.name,
        version = package.version,
    )
}
