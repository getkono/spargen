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
    for command in ["check", "lock", "diff", "explain"] {
        assert!(
            stdout.contains(command),
            "help must list {command}: {stdout}"
        );
    }
    assert!(
        !stdout.contains("generate"),
        "help must not expose generation: {stdout}"
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
        "stderr must identify the removed command: {stderr}"
    );
}
