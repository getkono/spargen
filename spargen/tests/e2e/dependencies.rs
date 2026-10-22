//! The runtime dependency contract: what generated output compiles against, the TLS crates the
//! example gate greps for, the requirement floors Cargo enforces, and the wasm32 target.

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

/// Whether `name` is a TLS crate, by the same rule the `example` gate applies to each example
/// lockfile in `mise.toml` and `ci.yml`: it names `rustls`, `native-tls`, `openssl` or `webpki`,
/// or ends in `-tls`. The two share a rule so that "no TLS crate" there and "a TLS crate" here
/// mean the same set; `is_tls_crate_classifies_exactly_what_the_example_gate_greps` holds the two
/// copies of the rule equal.
fn is_tls_crate(name: &str) -> bool {
    ["rustls", "native-tls", "openssl", "webpki"]
        .iter()
        .any(|family| name.contains(family))
        || name.ends_with("-tls")
}

/// The extended regular expression the `example` gate's lockfile step passes to `grep -En` in
/// `file`, read from the file itself. Exactly one such call must be there: a second would be a
/// second rule this test does not compare, and none means the step moved out from under it.
fn example_gate_tls_pattern(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let patterns: Vec<&str> = text
        .match_indices("grep -En '")
        .map(|(at, call)| {
            let rest = &text[at + call.len()..];
            &rest[..rest.find('\'').expect("the grep pattern's quote closes")]
        })
        .collect();
    assert_eq!(
        patterns.len(),
        1,
        "{file} must carry exactly one `grep -En '…'` (the example gate's TLS-crate rule), found \
         {patterns:?}"
    );
    patterns[0].to_owned()
}

/// The `example` gate's TLS-crate regex (`mise.toml`, and its CI copy in `ci.yml`) and
/// `is_tls_crate` are two spellings of one rule; this runs both over the same names and requires
/// they agree on every one, so neither can drift alone (#411). The regex runs through `grep -E`
/// itself, the engine the gate uses, over lines shaped as `Cargo.lock` writes them, since the
/// regex is anchored on that shape.
///
/// The names are every package in the four committed lockfiles plus edge cases each half of the
/// rule turns on (a family inside a longer name, a `-tls` suffix, `tls` without the hyphen or not
/// at the end). The set must carry names on both sides of the rule, or agreement proves nothing.
#[test]
fn is_tls_crate_classifies_exactly_what_the_example_gate_greps() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut names = std::collections::BTreeSet::new();
    for lock in [
        "Cargo.lock",
        "examples/petstore/Cargo.lock",
        "examples/petstore-macro/Cargo.lock",
        "examples/github-api/Cargo.lock",
    ] {
        let text = std::fs::read_to_string(root.join(lock)).unwrap();
        names.extend(
            text.lines()
                .filter_map(|line| line.strip_prefix("name = \"")?.strip_suffix('"'))
                .map(str::to_owned),
        );
    }
    names.extend(
        [
            "rustls",
            "tokio-rustls",
            "rustls-pki-types",
            "native-tls",
            "tokio-native-tls",
            "openssl",
            "openssl-sys",
            "webpki-roots",
            "rustls-webpki",
            "async-tls",
            "tls",
            "tls-foo",
            "mytls",
            "foo-tlsx",
            "native_tls",
            "serde",
        ]
        .map(str::to_owned),
    );
    let tls: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| is_tls_crate(name))
        .collect();
    assert!(
        !tls.is_empty() && tls.len() < names.len(),
        "the compared names must include TLS crates and non-TLS crates alike"
    );

    let input: String = names
        .iter()
        .map(|name| format!("name = \"{name}\"\n"))
        .collect();
    for file in ["mise.toml", ".github/workflows/ci.yml"] {
        let pattern = example_gate_tls_pattern(file);
        let mut grep = Command::new("grep")
            .args(["-E", &pattern])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            grep.stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = grep.wait_with_output().unwrap();
        // 0 is a match and 1 no match; anything else (2) is grep rejecting the pattern.
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "`grep -E` rejected {file}'s pattern {pattern:?}: {:?}",
            output.status
        );
        let grepped: Vec<&str> = std::str::from_utf8(&output.stdout)
            .unwrap()
            .lines()
            .map(|line| {
                line.strip_prefix("name = \"")
                    .and_then(|line| line.strip_suffix('"'))
                    .unwrap()
            })
            .collect();
        assert_eq!(
            grepped, tls,
            "{file}'s TLS-crate regex {pattern:?} and `is_tls_crate` classify different crates"
        );
    }
}

/// The distinct package names in this workspace's resolved graph, from `Cargo.lock` as committed
/// (`--locked`, as the `deny` gate audits it), with `features` passed to `cargo tree`.
///
/// `cargo tree` is read rather than `cargo metadata` because it resolves features the way
/// cargo-deny does: `cargo metadata`'s package list keeps the target of a weak `dep?/feature`
/// that nothing enables (reqwest's `quinn`, which depends on `rustls`), so it could report a TLS
/// crate cargo-deny never audits (#187). Every edge kind on every target is read, the scope
/// cargo-deny audits by default.
fn workspace_graph_package_names(features: &[&str]) -> std::collections::BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let output = query_cargo(root)
        .args(["tree", "--workspace", "--locked", "--target", "all"])
        .args(["--edges", "normal,build,dev", "--prefix", "none"])
        .args(["--format", "{p}"])
        .args(features)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "`cargo tree {features:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// The `deny` gate audits under `--all-features` because that is what puts a TLS stack in the
/// audited graph, and TLS advisories (RUSTSEC-2026-0285) are found only through one that is
/// there. `the_deny_gate_states_the_feature_scope_it_audits` in `corpus_manifest.rs` pins the
/// flag; this pins the property the flag stands for. Restructuring `remote-fetch`, dropping
/// reqwest's `rustls-tls`, or removing the `rustls` floor would otherwise leave the flag in place
/// and the TLS advisory audit vacuous with every gate green (#227).
///
/// Any TLS crate satisfies it, not `rustls` by name, so a backend swap (to `native-tls`, say)
/// keeps it passing while the audit still sees a TLS stack. The default-features graph is the
/// control: it must contain none, or the matcher proves nothing and the gate comments saying
/// `--all-features` is what brings TLS in are false.
#[test]
fn the_all_features_workspace_graph_carries_a_tls_stack() {
    let tls = |features: &[&str]| -> Vec<String> {
        workspace_graph_package_names(features)
            .into_iter()
            .filter(|name| is_tls_crate(name))
            .collect()
    };

    let audited = tls(&["--all-features"]);
    assert!(
        !audited.is_empty(),
        "the `--all-features` workspace graph the `deny` gate audits carries no TLS crate, so no \
         TLS advisory can fail it"
    );

    let default = tls(&[]);
    assert!(
        default.is_empty(),
        "the default-features workspace graph carries TLS crates {default:?}: `--all-features` is \
         no longer what puts TLS in the audited graph, so this test's control and the `deny` \
         gate's stated rationale no longer hold"
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
