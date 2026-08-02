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
