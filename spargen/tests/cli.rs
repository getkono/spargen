//! CLI-surface tests that drive the real `spargen` binary. Generation intentionally has no CLI
//! path: client modules are created from Rust build code or the proc macro.

use std::path::Path;
use std::process::Command;

fn spargen_bin() -> &'static str {
    env!("CARGO_BIN_EXE_spargen")
}

#[test]
fn help_lists_only_non_generation_tools() {
    let output = Command::new(spargen_bin()).arg("--help").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    // The command column, not the whole text: a description may name a command's word ("into
    // `spargen.lock`"), so a substring search passes with the command itself renamed. `help` is
    // clap's own subcommand. Equality also pins that no `generate` command is listed.
    let commands: Vec<&str> = stdout
        .lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        commands,
        ["check", "deps", "lock", "explain", "diff", "help"],
        "help must list exactly the non-generation tools: {stdout}"
    );
}

#[test]
fn generate_is_rejected_as_an_unknown_command() {
    let output = Command::new(spargen_bin())
        .arg("generate")
        .output()
        .unwrap();

    // Clap's own argument errors exit 2; spargen's usage errors exit 3 (`explain` and the config
    // tests pin that side). Only the exit status and that the command is named are asserted, not
    // clap's wording.
    assert_eq!(
        output.status.code(),
        Some(2),
        "generation must not be available through the CLI: {output:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "an unknown command must not print generated code"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("generate"),
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
///    below require the two outputs to differ; identical prose reddens both of those assertions.
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
    assert!(
        json_stderr.starts_with("error: "),
        "a failed lookup must still be reported as an error under `--format json`: {json_stderr}"
    );
}

/// The report `spargen check` gives does not depend on how the spec path is spelled (#220). With
/// the bare `openapi.yaml` the root's directory is empty, so a `$ref` to `./openapi.yaml` resolved
/// to a path that differed from the root's as written, loaded the root again, and `W011` reported
/// the root as shadowing itself. Only a child process can be given its own working directory, which
/// the bare spelling needs; `frontend/` covers the spellings an absolute root can reach.
#[test]
fn check_reports_the_same_for_every_spelling_of_the_spec_path() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n\
         paths:\n  /n:\n    get:\n      operationId: getN\n      responses:\n        '200':\n          \
         description: ok\n          content:\n            application/json: { schema: { $ref: \
         '#/components/schemas/Node' } }\ncomponents:\n  schemas:\n    Node:\n      type: object\n      \
         properties:\n        parent: { $ref: '#/components/schemas/MaybeNode' }\n    MaybeNode:\n      \
         oneOf:\n        - $ref: './openapi.yaml#/components/schemas/Node'\n        - type: 'null'\n",
    )
    .unwrap();
    let absolute = spec.to_str().unwrap().to_owned();
    for spelling in ["openapi.yaml", "./openapi.yaml", absolute.as_str()] {
        let output = Command::new(spargen_bin())
            .args(["check", spelling])
            .current_dir(temp.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{spelling}: stdout {stdout} stderr {stderr}"
        );
        assert!(
            !stdout.contains("W011") && !stderr.contains("W011"),
            "{spelling}: the root cannot shadow itself: stdout {stdout} stderr {stderr}"
        );
        assert!(
            stderr.contains("spargen: clean"),
            "{spelling}: stdout {stdout} stderr {stderr}"
        );
    }
}

// --- Exit codes and output routing ---------------------------------------------------------------
//
// `cli/exit.rs` declares the contract: 0 for success, 1 when error-severity diagnostics were
// reported, 3 for a usage error spargen detects (clap's own argument errors exit 2). Every report a
// subcommand renders as JSON goes to stdout, so it stays pipeable. A human-rendered report goes to
// stderr when it is a rejection, and to stdout when it is the subcommand's result, except for
// `check`: its result is a diagnostic report, so it goes to stderr on success too.

/// Two operations; [`ONE_OPERATION`] removes `getPet`, a breaking (`major`) change.
const TWO_OPERATIONS: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
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

/// [`TWO_OPERATIONS`] without `getPet`; diffed the other way round, its addition is `minor`.
const ONE_OPERATION: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses: { "204": { description: OK } }
"##;

/// A 3.0 document, which every subcommand that lowers a spec rejects with `E001`.
const REJECTED: &str = "openapi: 3.0.3\ninfo: { title: Old, version: 1.0.0 }\npaths: {}\n";

/// Write `contents` to `name` in `dir`, returning its path as the `&str` a command line takes.
fn write_spec(dir: &Path, name: &str, contents: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path.to_str().unwrap().to_owned()
}

/// Run the binary in `dir`, returning its exit status, stdout, and stderr.
fn spargen(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(spargen_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Parse `stdout` as the one JSON value `--format json` prints.
fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|error| panic!("--format json must emit JSON: {error}: {stdout}"))
}

/// The `outcome` and every diagnostic `code` of a JSON report.
fn outcome_and_codes(report: &serde_json::Value) -> (&str, Vec<&str>) {
    let outcome = report["outcome"].as_str().unwrap_or_else(|| {
        panic!("a report carries its outcome: {report}");
    });
    let codes = report["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("a report carries its diagnostics: {report}"))
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap())
        .collect();
    (outcome, codes)
}

#[test]
fn diff_prints_its_report_on_stdout_and_exits_zero_on_a_breaking_change_without_the_gate() {
    let temp = tempfile::tempdir().unwrap();
    let old = write_spec(temp.path(), "old.yaml", TWO_OPERATIONS);
    let new = write_spec(temp.path(), "new.yaml", ONE_OPERATION);

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &old, &new]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(
        stdout.contains("[operation-removed]") && stdout.contains("recommended bump: major"),
        "the report is the result, so it goes to stdout: {stdout}"
    );
    assert!(stderr.is_empty(), "nothing is diagnosed: {stderr}");
}

#[test]
fn diff_exit_code_fails_exactly_a_breaking_change() {
    let temp = tempfile::tempdir().unwrap();
    let two = write_spec(temp.path(), "two.yaml", TWO_OPERATIONS);
    let one = write_spec(temp.path(), "one.yaml", ONE_OPERATION);

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &two, &one, "--exit-code"]);
    assert_eq!(
        status,
        Some(1),
        "a major diff fails the gate: stdout {stdout} stderr {stderr}"
    );
    assert!(
        stdout.contains("recommended bump: major"),
        "a failed gate still prints the report on stdout: {stdout}"
    );
    assert!(
        stderr.is_empty(),
        "a breaking diff is no diagnostic: {stderr}"
    );

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &one, &two, "--exit-code"]);
    assert_eq!(
        status,
        Some(0),
        "a minor diff passes the gate: stdout {stdout} stderr {stderr}"
    );
    assert!(stdout.contains("recommended bump: minor"), "{stdout}");

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &two, &two, "--exit-code"]);
    assert_eq!(
        status,
        Some(0),
        "no change passes the gate: stdout {stdout} stderr {stderr}"
    );
    assert!(stdout.contains("recommended bump: patch"), "{stdout}");
}

#[test]
fn diff_json_goes_to_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let old = write_spec(temp.path(), "old.yaml", TWO_OPERATIONS);
    let new = write_spec(temp.path(), "new.yaml", ONE_OPERATION);

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &old, &new, "--format", "json"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    let report = json(&stdout);
    assert_eq!(report["bump"], "major", "{stdout}");
    let kinds: Vec<&str> = report["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("the report lists its changes: {stdout}"))
        .iter()
        .map(|change| change["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["operation-removed"], "{stdout}");
}

#[test]
fn diff_exits_one_when_either_spec_fails_to_lower_with_or_without_the_gate() {
    let temp = tempfile::tempdir().unwrap();
    let good = write_spec(temp.path(), "good.yaml", ONE_OPERATION);
    let rejected = write_spec(temp.path(), "rejected.yaml", REJECTED);

    for (old, new, side) in [(&rejected, &good, "old"), (&good, &rejected, "new")] {
        for gate in [&[][..], &["--exit-code"][..]] {
            let mut args = vec!["diff", old.as_str(), new.as_str()];
            args.extend_from_slice(gate);
            let (status, stdout, stderr) = spargen(temp.path(), &args);
            assert_eq!(
                status,
                Some(1),
                "a {side} spec that fails to lower is an error ({gate:?}): stderr {stderr}"
            );
            assert!(
                stdout.is_empty(),
                "a rejection is a diagnostic, so it goes to stderr: {stdout}"
            );
            assert!(stderr.contains("E001"), "{side}: {stderr}");
        }

        let (status, stdout, stderr) = spargen(
            temp.path(),
            &["diff", old.as_str(), new.as_str(), "--format", "json"],
        );
        assert_eq!(status, Some(1), "{side}: stdout {stdout} stderr {stderr}");
        assert!(
            stderr.is_empty(),
            "JSON goes to stdout, rejection or not: {stderr}"
        );
        let rejection = json(&stdout);
        let (outcome, codes) = outcome_and_codes(&rejection[side]);
        assert_eq!(
            (outcome, codes),
            ("rejected", vec!["E001"]),
            "the rejection is keyed by the side that failed: {stdout}"
        );
    }
}

#[test]
fn check_exits_one_on_a_rejected_spec() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", REJECTED);

    let (status, stdout, stderr) = spargen(temp.path(), &["check", &spec]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(
        stdout.is_empty(),
        "check's human report goes to stderr: {stdout}"
    );
    assert!(stderr.contains("E001"), "{stderr}");

    let (status, stdout, stderr) = spargen(temp.path(), &["check", &spec, "--format", "json"]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "JSON goes to stdout: {stderr}");
    assert_eq!(
        outcome_and_codes(&json(&stdout)),
        ("rejected", vec!["E001"]),
        "{stdout}"
    );
}

/// A diagnostic raised inside a `$ref`'d file names that file, not only a line and column the
/// reader cannot attach to any document of the bundle (#727): the human rendering prints
/// `--> <path>:<line>:<col>`, and the JSON report carries the same path beside the span.
#[test]
fn check_names_the_referenced_file_a_diagnostic_lies_in() {
    let temp = tempfile::tempdir().unwrap();
    write_spec(
        temp.path(),
        "openapi.yaml",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
         schemas:\n    Pet:\n      $ref: './schemas.yaml#/Pet'\n",
    );
    // The mixed-kind enum (`E008`) sits on line 7, column 11, of the referenced file.
    write_spec(
        temp.path(),
        "schemas.yaml",
        "# padding\n\nPet:\n  type: object\n  properties:\n    kind:\n      enum: [1, \"a\"]\n",
    );

    let (status, stdout, stderr) = spargen(temp.path(), &["check", "openapi.yaml"]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(stderr.contains("E008"), "{stderr}");
    let location = stderr
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("--> "))
        .unwrap_or_else(|| panic!("the diagnostic carries a location: {stderr}"));
    assert_eq!(
        location, "schemas.yaml:7:11",
        "the location names the referenced file: {stderr}"
    );

    let (status, stdout, stderr) =
        spargen(temp.path(), &["check", "openapi.yaml", "--format", "json"]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    let report = json(&stdout);
    assert_eq!(
        outcome_and_codes(&report),
        ("rejected", vec!["E008"]),
        "{stdout}"
    );
    let diagnostic = &report["diagnostics"][0];
    assert_eq!(
        diagnostic["file"].as_str(),
        Some("schemas.yaml"),
        "the JSON diagnostic names the referenced file: {stdout}"
    );
    assert_eq!(diagnostic["span"]["start"]["line"], 7, "{stdout}");
    assert_eq!(diagnostic["span"]["start"]["col"], 11, "{stdout}");
}

#[test]
fn deps_json_carries_the_dependencies_or_the_rejection_on_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let good = write_spec(temp.path(), "good.yaml", ONE_OPERATION);
    let rejected = write_spec(temp.path(), "rejected.yaml", REJECTED);

    let (status, stdout, stderr) = spargen(temp.path(), &["deps", &good, "--format", "json"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    let block = json(&stdout);
    let reqwest = block["dependencies"]
        .as_array()
        .unwrap_or_else(|| panic!("the block lists its dependencies: {stdout}"))
        .iter()
        .find(|dependency| dependency["name"] == "reqwest")
        .unwrap_or_else(|| panic!("generated output always needs reqwest: {stdout}"));
    assert_eq!(
        reqwest["no_default_features"], true,
        "reqwest's default TLS stack is never chosen for the consumer: {stdout}"
    );

    let (status, stdout, stderr) = spargen(temp.path(), &["deps", &rejected, "--format", "json"]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(
        stderr.is_empty(),
        "JSON goes to stdout, rejection or not: {stderr}"
    );
    assert_eq!(
        outcome_and_codes(&json(&stdout)),
        ("rejected", vec!["E001"]),
        "{stdout}"
    );
}

#[test]
fn lock_reports_what_it_wrote_and_exits_zero() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_OPERATION);
    let lock = temp.path().join("spargen.lock");

    let (status, stdout, stderr) = spargen(temp.path(), &["lock", &spec]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        stdout.contains(lock.to_str().unwrap()),
        "the report is the result, so it goes to stdout and names the lock file: {stdout}"
    );
    assert!(lock.is_file(), "the lock file is written beside the spec");

    std::fs::remove_file(&lock).unwrap();
    let (status, stdout, stderr) = spargen(temp.path(), &["lock", &spec, "--format", "json"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    let report = json(&stdout);
    assert_eq!(
        report["refs"],
        serde_json::json!([]),
        "a spec with no remote $ref pins nothing: {stdout}"
    );
    assert_eq!(report["lock_path"], lock.to_str().unwrap(), "{stdout}");
    assert!(lock.is_file(), "the lock file is written under JSON too");
}

#[test]
fn lock_exits_one_on_a_spec_it_cannot_read() {
    // No network: an unreadable root fails before anything is fetched. The refused-fetch arm is
    // `vendor_remote.rs`'s.
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.yaml");
    let missing = missing.to_str().unwrap();

    let (status, stdout, stderr) = spargen(temp.path(), &["lock", missing]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(
        stdout.is_empty(),
        "a failure is a diagnostic, so it goes to stderr: {stdout}"
    );
    assert!(
        stderr.contains("missing.yaml"),
        "stderr names the file: {stderr}"
    );

    let (status, stdout, stderr) = spargen(temp.path(), &["lock", missing, "--format", "json"]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(
        stderr.is_empty(),
        "JSON goes to stdout, failure or not: {stderr}"
    );
    let report = json(&stdout);
    let (outcome, codes) = outcome_and_codes(&report);
    assert_eq!(outcome, "rejected", "{stdout}");
    assert!(
        !codes.is_empty(),
        "a failure names its diagnostic: {stdout}"
    );
    assert!(
        !temp.path().join("spargen.lock").exists(),
        "a failed lock writes no lock file"
    );
}
