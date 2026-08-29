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
