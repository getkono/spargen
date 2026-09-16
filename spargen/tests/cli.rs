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
/// Any code exercises the same handler, so the choice is governed by one constraint: it must be a
/// code whose title and explain text no open branch is rewriting, or this suite reddens when an
/// unrelated pull request lands. At the time of writing that rules out `E023` — pr#87's entire
/// production change is its explain string, and it is the code the issue behind these tests
/// suggested — along with `E013` and `E007` (pr#112, pr#125) and `E004` and `W011` (pr#112).
/// `E008` and `W005` are touched by none of them. Please do not "simplify" this back to `E023`.
///
/// The assertions never spell the prose out; they compare against [`spargen::explain`], so editing
/// any wording is free and only the delivery path is pinned.
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
    // Parsed rather than substring-matched. `stdout.contains("\"code\"")` is satisfied by explain
    // prose that merely mentions the word, so it would pass against output carrying no such field;
    // a JSON consumer breaks on a renamed or dropped field, which is what these assertions catch.
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
}
