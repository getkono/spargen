//! The runtime dependency contract: what generated output compiles against, the requirement
//! floors Cargo enforces, and the wasm32 target.

use super::*;

/// A spec whose only binary payload is one documented error body. `ErrorShape::Single` over a
/// `TypeKind::Bytes` is the shape where generated code and the dependency contract can most easily
/// disagree: the body never travels through serde (it is classified by `classify_error_bytes`), so
/// the contract does not require `bytes/serde` — and codegen must not emit anything that does.
const BYTES_ERROR_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200": { description: ok }
        "404":
          description: raw failure
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
"#;

/// Generated output must compile against exactly the `[dependencies]` block `spargen deps` prints
/// for the same spec — not against a superset. Building the manifest from `requirements()` rather
/// than a fixed fat manifest is the point: a fat manifest hides a missing feature.
#[test]
fn generated_output_compiles_against_exactly_the_dependencies_it_asks_for() {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, BYTES_ERROR_SPEC).unwrap();
    let spec = Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap());

    let requirements = spargen::requirements(&spec).expect("spec lowers");
    let out = temp.path().join("client");
    std::fs::create_dir_all(out.join("src")).unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        format!(
            "[package]\nname = \"deps_exact\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{}",
            requirements.manifest_block()
        ),
    )
    .unwrap();

    let report = spargen::generate(
        &spec
            .clone()
            .build(Utf8PathBuf::from_path_buf(out.join("src/lib.rs")).unwrap())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(
        status.success(),
        "generated output must compile against the block `spargen deps` prints"
    );
}

#[test]
fn cargo_build_rejects_a_runtime_requirement_below_the_supported_floor() {
    let temp = tempfile::tempdir().unwrap();
    let crate_dir = temp.path().join("consumer");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let spargen_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "unsupported-runtime-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
bytes = "1.12.0"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"

[build-dependencies]
spargen = {{ path = {:?}, default-features = false }}

[workspace]
"#,
            spargen_path
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("build.rs"),
        r#"fn main() {
    let build = spargen::Spec::new("openapi.yaml").build("src/generated.rs");
    let report = spargen::generate(&build);
    for diagnostic in report.diagnostics() {
        eprintln!("{}: {}", diagnostic.code.as_str(), diagnostic.message);
    }
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
}
"#,
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        "include!(\"generated.rs\");\n",
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("openapi.yaml"),
        r#"openapi: 3.1.0
info: { title: Minimal, version: 1.0.0 }
paths: {}
"#,
    )
    .unwrap();

    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        !output.status.success(),
        "unsupported floor unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E023"), "{stderr}");
    assert!(stderr.contains(">=1.12.1, <2.0.0"), "{stderr}");
    assert!(
        !crate_dir.join("src/generated.rs").exists(),
        "a rejected runtime contract must not write generated output"
    );
}

/// The blocking client's `tokio` table is evaluated for the target Cargo is building, read from the
/// `TARGET`/`CARGO_CFG_*` a real build script receives. This is the only test that proves that
/// mapping matches what Cargo actually sets.
#[test]
fn cargo_build_evaluates_the_tokio_table_for_the_target_being_built() {
    let temp = tempfile::tempdir().unwrap();
    let crate_dir = temp.path().join("consumer");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let spargen_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = |tokio_tables: &str| {
        format!(
            r#"[package]
name = "target-table-consumer"
version = "0.0.0"
edition = "2021"

[features]
blocking = ["dep:tokio"]

[dependencies]
bytes = "1.12.1"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"

{tokio_tables}
[build-dependencies]
spargen = {{ path = {spargen_path:?}, default-features = false }}

[workspace]
"#
        )
    };
    const TOKIO: &str = r#"tokio = { version = "1.53.1", features = ["rt"], optional = true }"#;
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&format!(
            "[target.'cfg(unix)'.dependencies]\n{TOKIO}\n\n\
             [target.'cfg(windows)'.dependencies]\n{TOKIO}\n"
        )),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("build.rs"),
        r#"fn main() {
    let build = spargen::Spec::new("openapi.yaml").build("src/generated.rs");
    let report = spargen::generate(&build);
    for diagnostic in report.diagnostics() {
        eprintln!("{}: {}", diagnostic.code.as_str(), diagnostic.message);
    }
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
}
"#,
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        "include!(\"generated.rs\");\n",
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("openapi.yaml"),
        r#"openapi: 3.1.0
info: { title: Minimal, version: 1.0.0 }
paths: {}
"#,
    )
    .unwrap();

    // One table per OS family: Cargo applies exactly one of them on any unix or windows host.
    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        output.status.success(),
        "a tokio table applying to the build target must pass the audit:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // A native-only table that applies to no host this test runs on.
    std::fs::remove_file(crate_dir.join("src/generated.rs")).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&format!(
            "[target.'cfg(target_os = \"none\")'.dependencies]\n{TOKIO}\n"
        )),
    )
    .unwrap();
    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        !output.status.success(),
        "a tokio table that does not apply to the build target unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E023"), "{stderr}");
    assert!(stderr.contains("does not apply to"), "{stderr}");
    assert!(
        !crate_dir.join("src/generated.rs").exists(),
        "a rejected runtime contract must not write generated output"
    );
}

/// THE gate: a generated client must compile for `wasm32-unknown-unknown` (the browser,
/// via reqwest's `fetch` backend), where reqwest's client/request/response and fetch futures are
/// `!Send`. The `BASIC_SPEC` exercises the wasm-sensitive surface — the transport seam, the auth
/// token provider, and a streaming `EventStream` operation — so `cargo check --target
/// wasm32-unknown-unknown` succeeding proves the conditional `MaybeSend`/`MaybeSync` bounds, the
/// `cfg`-gated boxed-future aliases, the wasm `EventStream` buffer path, and the target-gated
/// manifest (native-only tokio, wasm-gated `BlockingClient`) all hold together.
#[test]
fn generated_crate_compiles_for_wasm32_browser_target() {
    if !wasm32_target_installed() {
        eprintln!(
            "skipping wasm32 gate: target `wasm32-unknown-unknown` is not installed (rustup target add wasm32-unknown-unknown)"
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("wasm_client");

    let report = generate_fixture_crate(&spec, &out, "wasm_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    // Default features: the client, transport seam, middleware/retry helpers, auth token provider,
    // and streaming `EventStream` must all compile against reqwest's `!Send` wasm `fetch` backend.
    let status = fixture_cargo(&out)
        .args(["check", "--target", "wasm32-unknown-unknown"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must `cargo check --target wasm32-unknown-unknown`"
    );

    // With the opt-in `blocking` feature enabled, a wasm build must STILL compile: the tokio-backed
    // `BlockingClient` and the tokio dependency are both gated off wasm, so the browser build never
    // pulls a runtime it cannot run.
    let status = fixture_cargo(&out)
        .args([
            "check",
            "--target",
            "wasm32-unknown-unknown",
            "--features",
            "blocking",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "wasm build must compile even with the `blocking` feature enabled (no tokio pulled)"
    );
}
