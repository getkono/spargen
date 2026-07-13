//! Integration coverage for the `spargen.toml` config file and the CLI omit-profile surface
//! (Issue #23). These drive the real `spargen` binary end-to-end, proving the CLI plumbing merges
//! defaults < config file < CLI flags and folds omit rules through the deterministic pipeline —
//! the library `Config` API is untouched. The binary requires the `cli` feature (always on under
//! `cargo test --all-features`).

use std::path::Path;
use std::process::{Command, Output};

/// A spec exercising the feature toggles (a `format: uuid` and a `format: date-time` field) and
/// two operations/paths and a component that omit rules can target.
const SPEC: &str = r##"
openapi: 3.1.0
info: { title: Config Test, version: 1.0.0 }
servers:
  - url: https://example.com/api
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
  /pets/{id}:
    get:
      operationId: getPet
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Pet" }
components:
  schemas:
    Pet:
      type: object
      required: [id]
      properties:
        id: { type: string, format: uuid }
        born: { type: string, format: date-time }
"##;

/// Lay out the spec in a fresh tempdir and return `(tempdir, spec_path)`.
fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    (temp, spec)
}

/// Run `spargen generate <spec> --out <out> [extra...]` in `dir`, returning the process output.
fn generate(dir: &Path, spec: &Path, out: &Path, extra: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_spargen"));
    cmd.current_dir(dir)
        .arg("generate")
        .arg(spec)
        .args(["--out", out.to_str().unwrap()])
        .args(extra);
    cmd.output().unwrap()
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

#[test]
fn config_file_features_off_fall_back_to_string() {
    // (a) `[features] uuid=false, time=false` from an auto-discovered `spargen.toml` take effect:
    // the emitted `Pet` fields fall back to `String` instead of `uuid::Uuid` / `time::OffsetDateTime`.
    let (temp, spec) = workspace();
    write(
        &temp.path().join("spargen.toml"),
        "[features]\nuuid = false\ntime = false\n",
    );
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &[]);
    assert!(output.status.success(), "{output:?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("uuid::Uuid"),
        "uuid feature off ⇒ no uuid::Uuid: {generated}"
    );
    assert!(
        !generated.contains("time::OffsetDateTime"),
        "time feature off ⇒ no time::OffsetDateTime"
    );
}

#[test]
fn defaults_keep_features_on() {
    // Baseline / regression guard: with no config and no flags, the typed mappings stay on.
    let (temp, spec) = workspace();
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &[]);
    assert!(output.status.success(), "{output:?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(generated.contains("uuid::Uuid"), "uuid on by default");
    assert!(
        generated.contains("time::OffsetDateTime"),
        "time on by default"
    );
}

#[test]
fn config_file_omit_rule_removes_operation() {
    // (b) An `[[omit]]` path rule removes the targeted path: `get_pet` is gone from the output and
    // the W009 (construct omitted) diagnostic fires. Uses `--format json` to observe the code.
    let (temp, spec) = workspace();
    write(
        &temp.path().join("spargen.toml"),
        "[[omit]]\npath = \"/pets/{id}\"\n",
    );
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--format", "json"]);
    assert!(output.status.success(), "{output:?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("fn get_pet"),
        "omitted path's operation must be absent: {generated}"
    );
    assert!(
        generated.contains("fn list_pets"),
        "the un-omitted operation is still generated"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("W009"), "omit fires W009: {stdout}");
}

#[test]
fn cli_omit_flags_are_equivalent_to_config() {
    // (c) The `--omit-path` flag alone (no config file) removes the same operation and fires W009 —
    // proving CLI == config equivalence for omit rules.
    let (temp, spec) = workspace();
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--omit-path", "/pets/{id}", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("fn get_pet"), "{generated}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("W009"), "{stdout}");
}

#[test]
fn cli_omit_operation_flag_removes_single_operation() {
    // The `--omit-operation "get /pets"` flag removes exactly that operation (not the whole path),
    // exercising the METHOD /path parse.
    let (temp, spec) = workspace();
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--omit-operation", "get /pets", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("fn list_pets"), "{generated}");
    assert!(generated.contains("fn get_pet"), "other op survives");
}

#[test]
fn cli_flag_overrides_config_value() {
    // (d) The config file turns uuid ON explicitly; the `--no-uuid` CLI flag must WIN, falling the
    // field back to `String`. This pins the precedence CLI > config > default.
    let (temp, spec) = workspace();
    write(
        &temp.path().join("spargen.toml"),
        "[features]\nuuid = true\ntime = true\n",
    );
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--no-uuid"]);
    assert!(output.status.success(), "{output:?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("uuid::Uuid"),
        "--no-uuid overrides config uuid=true: {generated}"
    );
    // The un-overridden `time` config value still holds (still typed).
    assert!(
        generated.contains("time::OffsetDateTime"),
        "time stays on from config"
    );
}

#[test]
fn malformed_config_errors_cleanly_without_panic() {
    // (e) A syntactically invalid `spargen.toml` yields a clean usage error (exit 3) — not a panic
    // (which would be exit 101) and not a silent success.
    let (temp, spec) = workspace();
    write(
        &temp.path().join("spargen.toml"),
        "[features]\nuuid = not_a_bool\n",
    );
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &[]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "malformed config exits with usage status: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid config file"),
        "clear error message: {stderr}"
    );
}

#[test]
fn bad_omit_flag_syntax_errors_cleanly() {
    // Bad omit-flag syntax is a clean usage error too, never a panic.
    let (temp, spec) = workspace();
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--omit-operation", "get"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--omit-operation"), "{stderr}");
}

#[test]
fn explicit_config_path_is_used() {
    // (f) An explicit `--config <path>` (named differently and outside the spec dir) is loaded.
    let (temp, spec) = workspace();
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg = cfg_dir.path().join("my-spargen.toml");
    write(&cfg, "[features]\nuuid = false\n");
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--config", cfg.to_str().unwrap()],
    );
    assert!(output.status.success(), "{output:?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("uuid::Uuid"), "{generated}");
    // `time` was not set in this config ⇒ default on.
    assert!(generated.contains("time::OffsetDateTime"), "{generated}");
}

#[test]
fn missing_explicit_config_path_errors() {
    // An explicit `--config` that does not exist is a clean error (a missing AUTO-discovered file is
    // fine, but an explicitly named one that is absent is not).
    let (temp, spec) = workspace();
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--config", "does-not-exist.toml"],
    );
    assert_eq!(output.status.code(), Some(3), "{output:?}");
}

#[test]
fn check_command_applies_config_omit_rules() {
    // `check` honors the same config-file omit rules (it runs the full frontend), firing W009 and
    // staying non-zero-free.
    let (temp, spec) = workspace();
    write(
        &temp.path().join("spargen.toml"),
        "[[omit]]\npath = \"/pets/{id}\"\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(temp.path())
        .arg("check")
        .arg(&spec)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("W009"), "check applies omit: {stdout}");
}
