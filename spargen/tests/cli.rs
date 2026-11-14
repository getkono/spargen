//! CLI-surface tests that drive the real `spargen` binary. Generation intentionally has no CLI
//! path: client modules are created from Rust build code or the proc macro.

use std::path::Path;
use std::process::Command;

#[allow(
    dead_code,
    reason = "this suite writes specs through `write_spec` and generates through the binary"
)]
#[path = "shared/generate.rs"]
mod generate;

use generate::write_spec;

#[path = "shared/repo.rs"]
mod repo;

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

/// `docs/book/src/cli.md` quotes `spargen --help` in a `text` fence; this holds that copy to what
/// the binary prints, byte for byte, so a changed subcommand or description fails here until the
/// book is regenerated from the real output.
#[test]
fn the_cli_reference_quotes_the_help_output_verbatim() {
    let output = Command::new(spargen_bin()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/book/src/cli.md");
    let page = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let quoted: Vec<&str> = page
        .split("```text\n")
        .skip(1)
        .filter_map(|rest| rest.split_once("```").map(|(body, _)| body))
        .filter(|body| body.contains("Usage: spargen <COMMAND>"))
        .collect();
    assert_eq!(
        quoted.len(),
        1,
        "{} must quote the top-level help in exactly one `text` fence",
        path.display()
    );
    assert_eq!(
        quoted[0],
        stdout,
        "{} quotes stale `spargen --help` output; paste the binary's output",
        path.display()
    );
}

/// The `Option<bool>` keys of `spargen.toml`, as `FileConfig` in `spargen/src/config.rs` declares
/// them: the switches every other surface must spell.
fn file_config_switches() -> Vec<String> {
    let source = repo::read("spargen/src/config.rs");
    let (_, rest) = source
        .split_once("struct FileConfig {\n")
        .expect("spargen/src/config.rs declares `struct FileConfig`");
    let (body, _) = rest
        .split_once("\n}")
        .expect("`FileConfig`'s body is closed");
    body.lines()
        .filter_map(|line| line.trim().strip_suffix(": Option<bool>,"))
        .map(str::to_owned)
        .collect()
}

/// The names in `generate_api!`'s `SWITCHES` array, in `spargen-macro/src/lib.rs`.
fn macro_switches() -> Vec<String> {
    let source = repo::read("spargen-macro/src/lib.rs");
    let (_, rest) = source
        .split_once("const SWITCHES: [&str; ")
        .expect("spargen-macro/src/lib.rs declares `const SWITCHES`");
    let (_, rest) = rest.split_once("= [").expect("`SWITCHES` is an array");
    let (body, _) = rest.split_once("];").expect("`SWITCHES` is closed");
    body.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.trim_matches('"').to_owned())
        .collect()
}

/// Every long flag the `subcommand`'s `--help` lists.
fn long_flags(subcommand: &str) -> Vec<String> {
    let output = Command::new(spargen_bin())
        .args([subcommand, "--help"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{subcommand} --help");
    String::from_utf8(output.stdout)
        .unwrap()
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| word.starts_with("--"))
        .map(str::to_owned)
        .collect()
}

/// Every boolean `spargen.toml` key is a switch on each surface that configures a spec: a
/// `--X`/`--no-X` pair on every subcommand that reads one, an entry in `generate_api!`'s
/// `SWITCHES`, and a `Spec::with_X` setter; and every `--no-X` flag names such a key. The set is
/// read from `FileConfig`, so a switch added there fails here until each surface spells it.
#[test]
fn every_config_switch_is_spelled_on_every_surface() {
    let switches = file_config_switches();
    assert!(
        switches.iter().any(|switch| switch == "testing"),
        "the reader finds FileConfig's switches: {switches:?}"
    );

    let mut sorted = switches.clone();
    sorted.sort();
    let mut in_macro = macro_switches();
    in_macro.sort();
    assert_eq!(
        in_macro, sorted,
        "generate_api!'s SWITCHES are exactly spargen.toml's boolean keys"
    );

    let setters = repo::read("spargen/src/config.rs");
    for switch in &switches {
        assert!(
            setters.contains(&format!("    pub fn with_{switch}(")),
            "`Spec` has no `with_{switch}` setter for the `{switch}` key"
        );
    }

    for subcommand in ["check", "deps", "lock", "diff"] {
        let flags = long_flags(subcommand);
        for switch in &switches {
            let flag = switch.replace('_', "-");
            for spelled in [format!("--{flag}"), format!("--no-{flag}")] {
                assert!(
                    flags.contains(&spelled),
                    "`spargen {subcommand}` has no `{spelled}` for the `{switch}` key: {flags:?}"
                );
            }
        }
        for negated in flags.iter().filter_map(|flag| flag.strip_prefix("--no-")) {
            assert!(
                switches.contains(&negated.replace('-', "_")),
                "`spargen {subcommand}`'s `--no-{negated}` names no spargen.toml switch"
            );
        }
    }
}

/// The list keys of `spargen.toml` (its `Vec` fields, as `FileConfig` declares them) in the
/// singular, the stem every other surface spells them by: `omit`, `keep`, `bound_header`.
fn file_config_list_stems() -> Vec<String> {
    let source = repo::read("spargen/src/config.rs");
    let (_, rest) = source
        .split_once("struct FileConfig {\n")
        .expect("spargen/src/config.rs declares `struct FileConfig`");
    let (body, _) = rest
        .split_once("\n}")
        .expect("`FileConfig`'s body is closed");
    body.lines()
        .filter_map(|line| {
            let (key, ty) = line.trim().split_once(": ")?;
            ty.starts_with("Vec<")
                .then(|| key.strip_suffix('s').unwrap_or(key).to_owned())
        })
        .collect()
}

/// The names in `generate_api!`'s `LIST_ARGUMENTS` array, in `spargen-macro/src/lib.rs`.
fn macro_list_arguments() -> Vec<String> {
    let source = repo::read("spargen-macro/src/lib.rs");
    let (_, rest) = source
        .split_once("const LIST_ARGUMENTS: [&str; ")
        .expect("spargen-macro/src/lib.rs declares `const LIST_ARGUMENTS`");
    let (_, rest) = rest
        .split_once("= [")
        .expect("`LIST_ARGUMENTS` is an array");
    let (body, _) = rest.split_once("];").expect("`LIST_ARGUMENTS` is closed");
    body.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.trim_matches('"').to_owned())
        .collect()
}

/// Every `pub fn` of `impl Spec` that takes `mut self` and returns `Self`: its chained setters.
fn spec_chained_setters() -> Vec<String> {
    let source = repo::read("spargen/src/config.rs");
    let (_, rest) = source
        .split_once("\nimpl Spec {\n")
        .expect("spargen/src/config.rs declares `impl Spec`");
    let (body, _) = rest.split_once("\n}\n").expect("`impl Spec` is closed");
    body.split("    pub fn ")
        .skip(1)
        .filter_map(|item| {
            let (name, rest) = item.split_once('(')?;
            let (signature, _) = rest.split_once('{')?;
            (signature.contains("mut self") && signature.trim_end().ends_with("-> Self"))
                .then(|| name.to_owned())
        })
        .collect()
}

/// Every list key of `spargen.toml` is spelled by one stem on each surface that configures a
/// spec: the key in the singular is an entry of `generate_api!`'s `LIST_ARGUMENTS`, the prefix of
/// a `Spec::add_<stem>…` setter, and a `--<stem>` or `--<stem>-…` flag of every subcommand that
/// reads one. Every chained `Spec` setter is a `with_` setter, an `add_` setter of such a stem, or
/// `clear_omissions`, so a list setter cannot keep a spelling of its own.
#[test]
fn every_list_knob_is_spelled_on_every_surface() {
    let stems = file_config_list_stems();
    assert!(
        ["omit", "keep", "bound_header"]
            .iter()
            .all(|stem| stems.iter().any(|found| found == stem)),
        "the reader finds FileConfig's list keys: {stems:?}"
    );

    let mut sorted = stems.clone();
    sorted.sort();
    let mut in_macro = macro_list_arguments();
    in_macro.sort();
    assert_eq!(
        in_macro, sorted,
        "generate_api!'s LIST_ARGUMENTS are exactly spargen.toml's list keys in the singular"
    );

    let setters = spec_chained_setters();
    assert!(
        setters.iter().any(|setter| setter == "with_uuid"),
        "the reader finds Spec's chained setters: {setters:?}"
    );
    for stem in &stems {
        assert!(
            setters
                .iter()
                .any(|setter| setter.starts_with(&format!("add_{stem}"))),
            "`Spec` has no `add_{stem}…` setter for the `{stem}` list: {setters:?}"
        );
    }
    for setter in &setters {
        let named = setter.starts_with("with_")
            || setter == "clear_omissions"
            || setter.strip_prefix("add_").is_some_and(|rest| {
                stems
                    .iter()
                    .any(|stem| rest == stem || rest.starts_with(&format!("{stem}_")))
            });
        assert!(
            named,
            "`Spec::{setter}` is neither a `with_` setter nor an `add_` setter of a spargen.toml \
             list key ({stems:?})"
        );
    }

    for subcommand in ["check", "deps", "lock", "diff"] {
        let flags = long_flags(subcommand);
        for stem in &stems {
            let flag = format!("--{}", stem.replace('_', "-"));
            assert!(
                flags
                    .iter()
                    .any(|given| *given == flag || given.starts_with(&format!("{flag}-"))),
                "`spargen {subcommand}` has no `{flag}` flag for the `{stem}` list: {flags:?}"
            );
        }
    }
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
    // `Command::Explain` arm's error branch in `cli/run.rs` renders its own the same way — by
    // convention, not by calling `config_error`, which no `Explain` path reaches. Pinned in both
    // directions so the choice is recorded rather than merely current.
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
    let absolute = spec_arg(
        temp.path(),
        "openapi.yaml",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n\
         paths:\n  /n:\n    get:\n      operationId: getN\n      responses:\n        '200':\n          \
         description: ok\n          content:\n            application/json: { schema: { $ref: \
         '#/components/schemas/Node' } }\ncomponents:\n  schemas:\n    Node:\n      type: object\n      \
         properties:\n        parent: { $ref: '#/components/schemas/MaybeNode' }\n    MaybeNode:\n      \
         oneOf:\n        - $ref: './openapi.yaml#/components/schemas/Node'\n        - type: 'null'\n",
    );
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
// reported, 3 for a usage error spargen detects (clap's own argument errors exit 2). The one
// document a subcommand renders as JSON, its result or its rejection, goes to stdout, so it stays
// pipeable. A human-rendered report goes to stderr when it is a rejection, and to stdout when it
// is the subcommand's result, except for `check`: its result is a diagnostic report, so it goes
// to stderr on success too. The warnings of a successful `deps` or `diff` are a second report
// beside that document, so they go to stderr in both formats and stdout keeps the one document
// (`deps_and_diff_print_the_warnings_of_a_successful_run_on_stderr`).

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

/// [`ONE_OPERATION`] with a validation-only `minLength`, which lowers with one `W001`.
const WARNED: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - { name: q, in: query, schema: { type: string, minLength: 1 } }
      responses: { "204": { description: OK } }
"##;

/// A 3.0 document, which every subcommand that lowers a spec rejects with `E001`.
const REJECTED: &str = "openapi: 3.0.3\ninfo: { title: Old, version: 1.0.0 }\npaths: {}\n";

/// [`write_spec`], returning the path as the `String` a command line takes.
fn spec_arg(dir: &Path, name: &str, contents: &str) -> String {
    write_spec(dir, name, contents).into_string()
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
    let old = spec_arg(temp.path(), "old.yaml", TWO_OPERATIONS);
    let new = spec_arg(temp.path(), "new.yaml", ONE_OPERATION);

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
    let two = spec_arg(temp.path(), "two.yaml", TWO_OPERATIONS);
    let one = spec_arg(temp.path(), "one.yaml", ONE_OPERATION);

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
    let old = spec_arg(temp.path(), "old.yaml", TWO_OPERATIONS);
    let new = spec_arg(temp.path(), "new.yaml", ONE_OPERATION);

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

/// One operation whose 200 body is a `uuid` string.
const UUID_BODY: &str = r##"
openapi: 3.1.0
info: { title: Pets, version: 1.0.0 }
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
"##;

/// `diff` reads the `spargen.toml` beside each spec on its own, so the same spec under a `uuid`
/// mapping only the new side's config changes is a breaking change the gate fails on.
#[test]
fn diff_classifies_a_setting_only_one_sides_config_file_changes() {
    let temp = tempfile::tempdir().unwrap();
    let old_dir = temp.path().join("old");
    let new_dir = temp.path().join("new");
    std::fs::create_dir(&old_dir).unwrap();
    std::fs::create_dir(&new_dir).unwrap();
    std::fs::write(new_dir.join("spargen.toml"), "uuid = false\n").unwrap();
    let old = spec_arg(&old_dir, "openapi.yaml", UUID_BODY);
    let new = spec_arg(&new_dir, "openapi.yaml", UUID_BODY);

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &old, &new, "--exit-code"]);
    assert_eq!(
        status,
        Some(1),
        "a retyped success body fails the gate: stdout {stdout} stderr {stderr}"
    );
    assert!(
        stdout.contains("[success-type-changed] GET /pets: success type `Uuid` -> `String`")
            && stdout.contains("recommended bump: major"),
        "{stdout}"
    );
    assert!(stderr.is_empty(), "{stderr}");

    // The new side's config alone emitting the `testing` module is additive, which the gate passes.
    std::fs::write(new_dir.join("spargen.toml"), "testing = true\n").unwrap();
    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &old, &new, "--exit-code"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(
        stdout.contains("[testing-module-added] testing: `testing` module added")
            && stdout.contains("recommended bump: minor"),
        "{stdout}"
    );
}

#[test]
fn diff_exits_one_when_either_spec_fails_to_lower_with_or_without_the_gate() {
    let temp = tempfile::tempdir().unwrap();
    let good = spec_arg(temp.path(), "good.yaml", ONE_OPERATION);
    let rejected = spec_arg(temp.path(), "rejected.yaml", REJECTED);

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
    let spec = spec_arg(temp.path(), "openapi.yaml", REJECTED);

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
    // The path as the bundle loaded it: the `$ref` resolved against the directory of the spec
    // path given on the command line.
    assert_eq!(
        location, "./schemas.yaml:7:11",
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
        Some("./schemas.yaml"),
        "the JSON diagnostic names the referenced file: {stdout}"
    );
    assert_eq!(diagnostic["span"]["start"]["line"], 7, "{stdout}");
    assert_eq!(diagnostic["span"]["start"]["col"], 11, "{stdout}");
}

#[test]
fn deps_json_carries_the_dependencies_or_the_rejection_on_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let good = spec_arg(temp.path(), "good.yaml", ONE_OPERATION);
    let rejected = spec_arg(temp.path(), "rejected.yaml", REJECTED);

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

/// `deps` and `diff` over a spec that lowers with a warning still succeed, print their result on
/// stdout exactly as for a clean spec, and print the warning on stderr in the requested format
/// (#1043).
#[test]
fn deps_and_diff_print_the_warnings_of_a_successful_run_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let warned = spec_arg(temp.path(), "warned.yaml", WARNED);
    let clean = spec_arg(temp.path(), "clean.yaml", ONE_OPERATION);

    let (status, stdout, stderr) = spargen(temp.path(), &["deps", &warned]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stdout.contains("reqwest"), "{stdout}");
    let (_, clean_stdout, _) = spargen(temp.path(), &["deps", &clean]);
    assert_eq!(
        stdout, clean_stdout,
        "the warning leaves the printed block as the clean spec's"
    );
    assert!(
        stderr.contains("spargen: clean") && stderr.contains("warning[W001]"),
        "{stderr}"
    );

    let (status, stdout, stderr) = spargen(temp.path(), &["deps", &warned, "--format", "json"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(json(&stdout)["dependencies"].is_array(), "{stdout}");
    assert_eq!(outcome_and_codes(&json(&stderr)), ("clean", vec!["W001"]));

    let (status, stdout, stderr) = spargen(temp.path(), &["diff", &clean, &warned]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stdout.contains("recommended bump:"), "{stdout}");
    assert!(
        stderr.starts_with("new spec:\nspargen: clean\n") && stderr.contains("warning[W001]"),
        "only the side that warned is reported: {stderr}"
    );

    let (status, stdout, stderr) =
        spargen(temp.path(), &["diff", &clean, &warned, "--format", "json"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(json(&stdout)["bump"].is_string(), "{stdout}");
    let sides = json(&stderr);
    assert_eq!(outcome_and_codes(&sides["old"]), ("clean", vec![]));
    assert_eq!(outcome_and_codes(&sides["new"]), ("clean", vec!["W001"]));
}

#[test]
fn lock_reports_what_it_wrote_and_exits_zero() {
    let temp = tempfile::tempdir().unwrap();
    let spec = spec_arg(temp.path(), "openapi.yaml", ONE_OPERATION);
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

/// A spec flag `lock` does not read is named in a warning on stderr, in each format, and the lock
/// still runs: stdout carries the same one report.
#[test]
fn lock_warns_on_the_spec_flags_it_ignores() {
    let temp = tempfile::tempdir().unwrap();
    let spec = spec_arg(temp.path(), "openapi.yaml", ONE_OPERATION);
    let lock = temp.path().join("spargen.lock");
    let ignored = ["--carve", "--uuid", "--omit-path", "/pets"];

    let args: Vec<&str> = ["lock", spec.as_str()].into_iter().chain(ignored).collect();
    let (status, stdout, stderr) = spargen(temp.path(), &args);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert_eq!(
        stderr,
        "warning: `spargen lock` ignores --carve, --uuid, --omit-path: it reads only --config \
         and --batch-cap, and fetches every remote $ref the spec reaches\n"
    );
    assert!(stdout.contains(lock.to_str().unwrap()), "{stdout}");
    assert!(lock.is_file(), "the warning does not stop the lock");

    std::fs::remove_file(&lock).unwrap();
    let args: Vec<&str> = ["lock", spec.as_str(), "--format", "json"]
        .into_iter()
        .chain(ignored)
        .collect();
    let (status, stdout, stderr) = spargen(temp.path(), &args);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    let warning = json(&stderr);
    assert_eq!(
        warning["ignored"],
        serde_json::json!(["--carve", "--uuid", "--omit-path"]),
        "{stderr}"
    );
    assert!(warning["warning"].is_string(), "{stderr}");
    assert_eq!(
        json(&stdout)["lock_path"],
        lock.to_str().unwrap(),
        "{stdout}"
    );

    // The two flags it reads warn about nothing.
    let (status, stdout, stderr) = spargen(
        temp.path(),
        &["lock", &spec, "--batch-cap", "5", "--config", "none.toml"],
    );
    assert_eq!(status, Some(3), "stdout {stdout} stderr {stderr}");
    assert!(
        !stderr.contains("warning"),
        "`--config` and `--batch-cap` are read: {stderr}"
    );
    let (status, stdout, stderr) = spargen(temp.path(), &["lock", &spec, "--batch-cap", "5"]);
    assert_eq!(status, Some(0), "stdout {stdout} stderr {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
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

/// Every subcommand that takes a spec resolves its configuration before it reads the spec, and a
/// configuration it cannot read or apply is a usage error: exit 3, nothing on stdout, and the
/// error on stderr. `diff` resolves each side's discovered `spargen.toml` on its own, so a
/// malformed one beside either spec fails the same way.
#[test]
fn every_spec_subcommand_exits_three_on_an_unreadable_config() {
    let temp = tempfile::tempdir().unwrap();
    let spec = spec_arg(temp.path(), "openapi.yaml", ONE_OPERATION);
    let assert_usage = |args: &[&str], names: &str| {
        let (status, stdout, stderr) = spargen(temp.path(), args);
        assert_eq!(status, Some(3), "{args:?}: stdout {stdout} stderr {stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.starts_with("error: ") && stderr.contains(names),
            "{args:?}: the error names {names}: {stderr}"
        );
    };

    for subcommand in ["check", "deps", "lock"] {
        assert_usage(
            &[subcommand, &spec, "--config", "missing.toml"],
            "missing.toml",
        );
    }
    // A read failure's I/O error is the `ConfigError`'s source, written once after its message.
    let (_, _, stderr) = spargen(temp.path(), &["check", &spec, "--config", "missing.toml"]);
    let io = std::fs::read("missing.toml-spargen-1328").unwrap_err();
    assert_eq!(
        stderr.trim_end(),
        format!("error: cannot read config file `missing.toml`: {io}"),
    );
    assert_usage(
        &["diff", &spec, &spec, "--config", "missing.toml"],
        "missing.toml",
    );

    for side in ["old", "new"] {
        let dir = temp.path().join(side);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("spargen.toml"), "uuid = not_a_bool\n").unwrap();
        let configured = spec_arg(&dir, "openapi.yaml", ONE_OPERATION);
        let args = if side == "old" {
            ["diff", &configured, &spec]
        } else {
            ["diff", &spec, &configured]
        };
        assert_usage(&args, &format!("{side}/spargen.toml"));
    }
    assert!(
        !temp.path().join("spargen.lock").exists(),
        "a lock that failed on its config wrote nothing"
    );
}

/// `--format json` does not reshape a usage error: a configuration that cannot be read and an
/// unknown `explain` code each print the one human `error:` line on stderr, as without it, exit
/// `3`, and print nothing on stdout.
#[test]
fn a_usage_error_under_format_json_is_the_human_error_line() {
    let temp = tempfile::tempdir().unwrap();
    let spec = spec_arg(temp.path(), "openapi.yaml", ONE_OPERATION);
    for args in [
        vec!["check", spec.as_str(), "--config", "missing.toml"],
        vec!["explain", "E999"],
    ] {
        let (human_status, human_stdout, human_stderr) = spargen(temp.path(), &args);
        let json_args: Vec<&str> = args.iter().copied().chain(["--format", "json"]).collect();
        let (status, stdout, stderr) = spargen(temp.path(), &json_args);
        assert_eq!(
            status,
            Some(3),
            "{json_args:?}: stdout {stdout} stderr {stderr}"
        );
        assert!(stdout.is_empty(), "{json_args:?}: {stdout}");
        assert!(stderr.starts_with("error: "), "{json_args:?}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{json_args:?}: {stderr}");
        assert!(
            serde_json::from_str::<serde_json::Value>(&stderr).is_err(),
            "{json_args:?}: {stderr}"
        );
        assert_eq!(
            (status, stdout, stderr),
            (human_status, human_stdout, human_stderr),
            "{json_args:?}"
        );
    }
}

/// A command line clap itself rejects, here one missing a required argument, exits with clap's
/// own status 2 rather than spargen's usage status 3, and prints nothing on stdout.
#[test]
fn a_missing_required_argument_exits_two() {
    let temp = tempfile::tempdir().unwrap();
    let spec = spec_arg(temp.path(), "openapi.yaml", ONE_OPERATION);
    for args in [
        &["check"][..],
        &["deps"],
        &["lock"],
        &["diff", &spec],
        &["explain"],
    ] {
        let (status, stdout, stderr) = spargen(temp.path(), args);
        assert_eq!(status, Some(2), "{args:?}: stdout {stdout} stderr {stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("required"),
            "{args:?}: clap names the missing argument: {stderr}"
        );
    }
}

/// `check`'s human report is its only output and goes to stderr whatever the outcome, so stdout
/// stays empty for a clean, a warned and a rejected spec alike.
#[test]
fn check_prints_its_human_report_only_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    for (contents, status, first_line, code) in [
        (ONE_OPERATION, 0, "spargen: clean", None),
        (WARNED, 0, "spargen: clean", Some("warning[W001]")),
        (REJECTED, 1, "spargen: rejected", Some("E001")),
    ] {
        let spec = spec_arg(temp.path(), "openapi.yaml", contents);
        let (actual, stdout, stderr) = spargen(temp.path(), &["check", &spec]);
        assert_eq!(actual, Some(status), "stdout {stdout} stderr {stderr}");
        assert!(stdout.is_empty(), "check's report goes to stderr: {stdout}");
        assert_eq!(stderr.lines().next(), Some(first_line), "{stderr}");
        if let Some(code) = code {
            assert!(stderr.contains(code), "{stderr}");
        }
    }
}

/// A `deps` the spec rejects prints no dependency block: its human rejection goes to stderr alone.
#[test]
fn deps_prints_a_human_rejection_only_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let rejected = spec_arg(temp.path(), "rejected.yaml", REJECTED);

    let (status, stdout, stderr) = spargen(temp.path(), &["deps", &rejected]);
    assert_eq!(status, Some(1), "stdout {stdout} stderr {stderr}");
    assert!(
        stdout.is_empty(),
        "a rejection prints no dependency block: {stdout}"
    );
    assert_eq!(stderr.lines().next(), Some("spargen: rejected"), "{stderr}");
    assert!(stderr.contains("E001"), "{stderr}");
}
