//! CLI-surface tests that drive the real `spargen` binary. Generation intentionally has no CLI
//! path: client modules are created from Rust build code or the proc macro.

use std::process::Command;

fn spargen_bin() -> &'static str {
    env!("CARGO_BIN_EXE_spargen")
}

#[test]
fn help_lists_only_non_generation_tools() {
    let output = Command::new(spargen_bin()).arg("--help").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in ["check", "deps", "lock", "diff", "explain"] {
        assert!(
            stdout.contains(command),
            "help must list {command}: {stdout}"
        );
    }
    // The word may legitimately appear in a description ("the dependencies generated output
    // needs"); what must not exist is a `generate` command, which is one of the listed commands.
    let commands: Vec<&str> = stdout
        .lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(
        !commands.contains(&"generate"),
        "help must not expose generation: {commands:?}"
    );
}

#[test]
fn generate_is_rejected_as_an_unknown_command() {
    let output = Command::new(spargen_bin())
        .arg("generate")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "generation must not be available through the CLI"
    );
    assert!(
        output.stdout.is_empty(),
        "an unknown command must not print generated code"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("unrecognized subcommand 'generate'"),
        "stderr must name the unrecognized command: {stderr}"
    );
}

/// The diagnostic code the `explain` cases below resolve.
///
/// Any code exercises the same handler, so the choice is nearly free. The assertions never spell
/// the prose out — they compare the binary's output against [`spargen::explain`] of the *same*
/// code, so both sides move together and rewriting any code's wording, or its title, cannot redden
/// this suite. Two constraints do bind, and they are the only two:
///
/// 1. Both this code and [`OTHER_CODE`] must remain **declared**. `spargen::explain(..).unwrap()`
///    panics on a code no longer in the enum, so removing one breaks these tests — and that is the
///    correct outcome, since the CLI could no longer explain it either.
/// 2. The two must not explain to **identical text**. Both the human and the `--format json` case
///    below require the two outputs to differ; identical prose would make that assertion vacuous.
///
/// Both hold for `E008` and `W005`. Whatever pair is used must satisfy them; a single code cannot,
/// which is why there are two.
const EXPLAINED_CODE: &str = "E008";

/// A second, distinct code. Requiring the two to explain differently is what pins that the handler
/// resolves the code it was *given*: comparing one code against the library would still pass if the
/// handler ignored its argument and resolved a constant that happened to be that code.
const OTHER_CODE: &str = "W005";

#[test]
fn explain_prints_the_explain_text_of_the_requested_code() {
    let explained = Command::new(spargen_bin())
        .args(["explain", EXPLAINED_CODE])
        .output()
        .unwrap();

    assert_eq!(
        explained.status.code(),
        Some(0),
        "explaining a known code must succeed: {}",
        String::from_utf8_lossy(&explained.stderr)
    );
    let stdout = String::from_utf8(explained.stdout).unwrap();
    assert_eq!(
        stdout.trim_end(),
        spargen::explain(EXPLAINED_CODE).unwrap(),
        "human output must be the requested code's explain text"
    );

    let other = Command::new(spargen_bin())
        .args(["explain", OTHER_CODE])
        .output()
        .unwrap();
    let other_stdout = String::from_utf8(other.stdout).unwrap();
    assert_eq!(
        other_stdout.trim_end(),
        spargen::explain(OTHER_CODE).unwrap(),
        "human output must be the requested code's explain text"
    );
    assert_ne!(
        stdout, other_stdout,
        "two distinct codes must not explain to the same text, or this suite could not tell \
         resolution from a constant"
    );
}

#[test]
fn explain_json_carries_the_code_and_its_explain_text() {
    let output = Command::new(spargen_bin())
        .args(["explain", EXPLAINED_CODE, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "explaining a known code as JSON must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Parsed, not substring-matched. `stdout.contains("\"code\"")` holds while `code` carries the
    // wrong value, and asserts nothing whatever about `explain` — renaming that field to `detail`
    // leaves it green. A JSON consumer breaks on both, which is what these assertions catch.
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("--format json must emit JSON: {error}: {stdout}"));
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("--format json must emit a JSON object: {stdout}"));

    let mut fields: Vec<&str> = object.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["code", "explain"],
        "the JSON shape is consumer surface: {stdout}"
    );
    assert_eq!(
        object.get("code").and_then(serde_json::Value::as_str),
        Some(EXPLAINED_CODE),
        "`code` must echo the requested code: {stdout}"
    );
    assert_eq!(
        object.get("explain").and_then(serde_json::Value::as_str),
        spargen::explain(EXPLAINED_CODE).ok(),
        "`explain` must carry that code's explain text: {stdout}"
    );

    // A second, distinct code through the same branch: the echo assertions below are what tell
    // resolution from a constant payload.
    let other = Command::new(spargen_bin())
        .args(["explain", OTHER_CODE, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(
        other.status.code(),
        Some(0),
        "explaining a known code as JSON must succeed: {}",
        String::from_utf8_lossy(&other.stderr)
    );
    let other_stdout = String::from_utf8(other.stdout).unwrap();
    let other_value: serde_json::Value = serde_json::from_str(&other_stdout)
        .unwrap_or_else(|error| panic!("--format json must emit JSON: {error}: {other_stdout}"));
    let other_object = other_value
        .as_object()
        .unwrap_or_else(|| panic!("--format json must emit a JSON object: {other_stdout}"));

    assert_eq!(
        other_object.get("code").and_then(serde_json::Value::as_str),
        Some(OTHER_CODE),
        "`code` must echo the requested code: {other_stdout}"
    );
    assert_eq!(
        other_object
            .get("explain")
            .and_then(serde_json::Value::as_str),
        spargen::explain(OTHER_CODE).ok(),
        "`explain` must carry that code's explain text: {other_stdout}"
    );
    assert_ne!(
        object.get("explain"),
        other_object.get("explain"),
        "two distinct codes must not explain to the same text, or this branch could not tell \
         resolution from a constant"
    );
}

#[test]
fn explain_rejects_an_unresolvable_code() {
    // Shaped like a code but assigned to none, so this exercises the lookup rather than the
    // argument parser.
    let output = Command::new(spargen_bin())
        .args(["explain", "E999"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(3),
        "an unresolvable code is a usage error"
    );
    assert!(
        output.stdout.is_empty(),
        "a failed lookup must print nothing on stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("E999"),
        "stderr must name the code it could not resolve: {stderr}"
    );
    assert!(
        stderr.starts_with("error: "),
        "a failed lookup must be reported as an error: {stderr}"
    );

    // The same failure under `--format json`. This is deliberate, not an oversight to be "fixed":
    // an unresolvable code is a *usage* error — the user mistyped an argument — and `--format`
    // governs diagnostic *reports*. `check` and `diff` route their reports through `emit`, which
    // honours the flag; `config_error` renders usage errors as plain text everywhere, and the
    // `Explain` arm renders its own the same way at `run.rs:103` — by convention, not by calling
    // `config_error`, which no `Explain` path reaches. Pinned in both directions so the choice is
    // recorded rather than merely current.
    let as_json = Command::new(spargen_bin())
        .args(["explain", "E999", "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(
        as_json.status.code(),
        Some(3),
        "`--format json` must not change a usage error's exit status"
    );
    assert!(
        as_json.stdout.is_empty(),
        "a failed lookup must print nothing on stdout under `--format json` either — a consumer \
         piping stdout to a parser must not receive the error text: {}",
        String::from_utf8_lossy(&as_json.stdout)
    );
    let json_stderr = String::from_utf8(as_json.stderr).unwrap();
    assert!(
        json_stderr.contains("E999"),
        "stderr must still name the code it could not resolve under `--format json`: {json_stderr}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(json_stderr.trim()).is_err(),
        "a usage error stays plain text under `--format json`, which applies to reports and not to \
         argument errors: {json_stderr}"
    );
}
