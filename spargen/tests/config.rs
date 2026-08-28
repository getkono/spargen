//! Integration coverage for `spargen check` config-file and omit-profile plumbing. The CLI is an
//! analysis surface only; generation configuration belongs in Rust build code.

use std::path::Path;
use std::process::{Command, Output};

const SPEC: &str = r##"
openapi: 3.1.0
info: { title: Config Test, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses: { "204": { description: OK } }
  /pets/{id}:
    get:
      operationId: getPet
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses: { "204": { description: OK } }
"##;

fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    (temp, spec)
}

fn check(dir: &Path, spec: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .arg("check")
        .arg(spec)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn auto_discovered_config_applies_omit_rules() {
    let (temp, spec) = workspace();
    std::fs::write(
        temp.path().join("spargen.toml"),
        "[[omit]]\npath = \"/pets/{id}\"\n",
    )
    .unwrap();
    let output = check(temp.path(), &spec, &["--format", "json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("W009"));
}

#[test]
fn cli_omit_flags_apply_to_analysis() {
    let (temp, spec) = workspace();
    let output = check(
        temp.path(),
        &spec,
        &["--omit-operation", "get /pets", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("W009"));
}

#[test]
fn explicit_config_path_is_used() {
    let (temp, spec) = workspace();
    let configs = tempfile::tempdir().unwrap();
    let config = configs.path().join("analysis.toml");
    std::fs::write(&config, "[[omit]]\npath = \"/pets/{id}\"\n").unwrap();
    let output = check(
        temp.path(),
        &spec,
        &["--config", config.to_str().unwrap(), "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("W009"));
}

#[test]
fn malformed_or_missing_config_errors_cleanly() {
    let (temp, spec) = workspace();
    std::fs::write(
        temp.path().join("spargen.toml"),
        "[features]\nuuid = not_a_bool\n",
    )
    .unwrap();
    let malformed = check(temp.path(), &spec, &[]);
    assert_eq!(malformed.status.code(), Some(3), "{malformed:?}");
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("invalid config file"));

    std::fs::remove_file(temp.path().join("spargen.toml")).unwrap();
    let missing = check(temp.path(), &spec, &["--config", "does-not-exist.toml"]);
    assert_eq!(missing.status.code(), Some(3), "{missing:?}");
}

#[test]
fn bad_omit_flag_syntax_errors_cleanly() {
    let (temp, spec) = workspace();
    let output = check(temp.path(), &spec, &["--omit-operation", "get"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--omit-operation"));
}

#[test]
fn the_removed_features_table_names_the_migration() {
    // 0.2 nested these keys under `[features]`. `deny_unknown_fields` alone would only say
    // "unknown field `features`", which does not tell an upgrading user what to do.
    let (temp, spec) = workspace();
    std::fs::write(
        temp.path().join("spargen.toml"),
        "[features]\ncarve = true\n",
    )
    .unwrap();
    let output = check(temp.path(), &spec, &[]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("move its keys"), "{stderr}");
}

#[test]
fn config_file_knobs_reach_the_library() {
    // `carve` in the file must have the same effect as `--carve`: the unsupported operation is
    // carved away (W009) instead of rejecting the run.
    let (temp, spec) = workspace();
    std::fs::write(
        temp.path().join("spargen.toml"),
        "carve = true\nbatch_cap = 7\nuuid = false\ntime = false\n",
    )
    .unwrap();
    let output = check(temp.path(), &spec, &["--format", "json"]);
    assert!(output.status.success(), "{output:?}");
}

fn deps(dir: &Path, spec: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .arg("deps")
        .arg(spec)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn deps_prints_a_pasteable_dependency_block() {
    let (temp, spec) = workspace();
    let output = deps(temp.path(), &spec, &[]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[dependencies]"), "{stdout}");
    // reqwest's own defaults pull in a TLS stack the generated client does not choose, so
    // `default-features = false` is part of the contract the audit enforces.
    assert!(
        stdout.contains(r#"reqwest = { version = ""#)
            && stdout.contains("default-features = false"),
        "{stdout}"
    );
    assert!(stdout.contains(r#"serde = { version = "#), "{stdout}");
    // The blocking client is opt-in, so it is offered commented out under its feature.
    assert!(stdout.contains("# tokio = {"), "{stdout}");
    // This spec has no uuid/time formats, no XML, and no streams — none of those crates belong.
    for absent in ["uuid", "quick-xml", "futures-core"] {
        assert!(
            !stdout.contains(absent),
            "{absent} should be absent: {stdout}"
        );
    }
}

#[test]
fn deps_follows_the_spec_knobs() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        r##"
openapi: 3.1.0
info: { title: Ids, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string, format: uuid }
"##,
    )
    .unwrap();

    let typed = deps(temp.path(), &spec, &[]);
    assert!(typed.status.success(), "{typed:?}");
    assert!(String::from_utf8_lossy(&typed.stdout).contains("uuid = {"));

    // `--no-uuid` falls back to `String`, which needs no `uuid` dependency at all.
    let untyped = deps(temp.path(), &spec, &["--no-uuid"]);
    assert!(untyped.status.success(), "{untyped:?}");
    assert!(!String::from_utf8_lossy(&untyped.stdout).contains("uuid = {"));
}

#[test]
fn deps_reports_a_rejection_instead_of_a_block() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.0.3\ninfo: { title: Old, version: 1.0.0 }\npaths: {}\n",
    )
    .unwrap();
    let output = deps(temp.path(), &spec, &[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("E001"),
        "{output:?}"
    );
}

// --- Cargo-integration dispositions -------------------------------------------------------------
//
// `W012`, `W013`, and `E024` are the facade's own diagnostics rather than the frontend's, so they
// have no inline-spec fixture in `frontend.rs`. `W013` and `E024` are reachable from any plain
// process; `W012` needs a real build script and lives in `e2e.rs` for that reason.

/// Generating outside a build script emits no rebuild triggers and skips the dependency audit.
/// Under `Auto` that is a degradation worth saying out loud, not a silent one.
#[test]
fn w013_generating_outside_a_build_script_reports_the_degradation() {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), SPEC).unwrap();

    let build = spargen::Spec::new(dir.join("openapi.yaml"))
        .build(dir.join("client.rs"))
        .cargo(spargen::CargoIntegration::Auto);
    let report = spargen::generate(&build);

    assert!(report.outcome().is_success(), "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == spargen::Code::CargoIntegrationDegraded),
        "{report:#?}"
    );
}

/// `Required` turns that same degradation into a hard failure, so a build that depends on the
/// audit cannot quietly run without it.
#[test]
fn e024_required_cargo_integration_outside_a_build_script_is_fatal() {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), SPEC).unwrap();

    let build = spargen::Spec::new(dir.join("openapi.yaml"))
        .build(dir.join("client.rs"))
        .cargo(spargen::CargoIntegration::Required);
    let report = spargen::generate(&build);

    assert_eq!(report.outcome(), spargen::Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == spargen::Code::CargoIntegrationRequired),
        "{report:#?}"
    );
    // The failure is announced before anything is written.
    assert!(!dir.join("client.rs").exists(), "{report:#?}");
}

/// `Off` opts out of the whole policy, so neither diagnostic fires.
#[test]
fn cargo_integration_off_reports_neither_degradation_nor_failure() {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), SPEC).unwrap();

    let build = spargen::Spec::new(dir.join("openapi.yaml"))
        .build(dir.join("client.rs"))
        .cargo(spargen::CargoIntegration::Off);
    let report = spargen::generate(&build);

    assert!(report.outcome().is_success(), "{report:#?}");
    assert!(
        !report.diagnostics().iter().any(|d| matches!(
            d.code,
            spargen::Code::CargoIntegrationDegraded | spargen::Code::CargoIntegrationRequired
        )),
        "{report:#?}"
    );
}
