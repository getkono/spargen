use super::{FeatureSet, PackageMeta};

/// Synthesize the `Cargo.toml` for a standalone generated crate.
///
/// Runtime dependencies are exactly the near-universal set — `reqwest` (no default features),
/// `serde`, `serde_json`, `bytes`, `secrecy` — plus the feature-gated `uuid`/`time` mappings. No
/// spargen crate ever appears, so the freestanding-output gate holds. The eventual
/// implementation builds this with `toml_edit` for stable formatting.
///
/// The `blocking` feature is always DECLARED (it is user-opt-in, not spec-driven) and wires up an
/// OPTIONAL `tokio` (`rt` only) that is pulled in solely when the consumer enables the feature — so
/// a default build carries no tokio direct dependency and no `BlockingClient`.
///
/// ## `wasm32` (browser / `fetch`) support
///
/// The generated client compiles for `wasm32-unknown-unknown`, where reqwest auto-selects its
/// `fetch` backend. `reqwest` carries `default-features = false`, so no native TLS/tokio backend is
/// pulled on any target. `tokio` is placed under a `cfg(not(target_arch = "wasm32"))` target table:
/// its `rt` runtime cannot run on the single-threaded browser, and the `BlockingClient` it powers is
/// `cfg`-gated off wasm to match — so even `--features blocking` on wasm pulls no tokio. The
/// conditional `Send`/`Sync` bounds in the embedded runtime (`support::MaybeSend`/`MaybeSync`) make
/// the transport seam and its helpers compile against reqwest's `!Send` `fetch` futures.
pub fn synth_cargo_toml(package: &PackageMeta, features: &FeatureSet) -> String {
    let default_features = match (features.uuid, features.time) {
        (true, true) => r#""uuid", "time""#,
        (true, false) => r#""uuid""#,
        (false, true) => r#""time""#,
        (false, false) => "",
    };
    // reqwest's `multipart` feature is added only when a `multipart/form-data` body is emitted, and
    // `bytes`'s `serde` feature only when a `bytes::Bytes` struct field is emitted — both derived
    // from the API so the manifest is deterministic and minimal.
    let bytes_dep = if features.bytes_serde {
        r#"bytes = { version = "1", features = ["serde"] }"#
    } else {
        r#"bytes = "1""#
    };
    let reqwest_features = if features.multipart {
        r#""json", "multipart""#
    } else {
        r#""json""#
    };
    // `quick-xml` is pulled in only when an XML body is emitted — the embedded `support::xml` module
    // (serialize/decode) is conditional on the same flag, so a non-XML client carries no quick-xml.
    let xml_dep = if features.xml {
        "\nquick-xml = { version = \"0.37\", features = [\"serialize\"] }"
    } else {
        ""
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
blocking = ["dep:tokio"]

[dependencies]
{bytes_dep}
reqwest = {{ version = "0.12", default-features = false, features = [{reqwest_features}] }}
secrecy = "0.10"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"{xml_dep}
uuid = {{ version = "1", features = ["serde"], optional = true }}
time = {{ version = "0.3", features = ["serde", "formatting", "parsing"], optional = true }}

# The blocking client owns a current-thread tokio runtime, which cannot run on the single-threaded
# browser — so tokio is a NATIVE-only optional dependency. Enabling `blocking` on `wasm32` pulls no
# tokio (the dep is absent there) and the `BlockingClient` is `cfg`-gated off wasm to match.
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = {{ version = "1", features = ["rt"], optional = true }}
"#,
        name = package.name,
        version = package.version,
    )
}
