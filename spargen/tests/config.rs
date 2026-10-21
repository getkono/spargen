//! Integration coverage for `spargen check` config-file and omit-profile plumbing. The CLI is an
//! analysis surface only; generation configuration belongs in Rust build code.
//!
//! `env!("CARGO_BIN_EXE_spargen")` expands to a path even when the `cli` feature is off and the
//! binary is not built, so every test that spawns it, and each helper only those tests use,
//! carries `#[cfg(feature = "cli")]`. The tests that drive the library run under any feature set.

#[cfg(feature = "cli")]
use std::path::Path;
#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC).unwrap();
    (temp, spec)
}

#[cfg(feature = "cli")]
fn check(dir: &Path, spec: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .arg("check")
        .arg(spec)
        .args(extra)
        .output()
        .unwrap()
}

#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
#[test]
fn an_unknown_omit_token_names_the_same_accepted_values_in_the_file_and_on_the_command_line() {
    // `UnknownOmitToken`'s own message carries the accepted list, so a mistyped token is answered
    // identically wherever it was written.
    const COMPONENTS: &str = "expected one of schemas/responses/parameters/request_bodies/\
                              headers/security_schemes/path_items/media_types";
    const METHODS: &str = "expected one of get/put/post/delete/options/head/patch/trace/query";
    let (temp, spec) = workspace();
    for (config, flags, token, accepted) in [
        (
            "[[omit]]\ncomponent = \"path_itemz\"\nname = \"Foo\"\n",
            &[][..],
            "path_itemz",
            COMPONENTS,
        ),
        (
            "[[omit]]\nmethod = \"qeury\"\npath = \"/pets\"\n",
            &[][..],
            "qeury",
            METHODS,
        ),
        (
            "",
            &["--omit-component", "path_itemz:Foo"][..],
            "path_itemz",
            COMPONENTS,
        ),
        (
            "",
            &["--omit-operation", "qeury /pets"][..],
            "qeury",
            METHODS,
        ),
    ] {
        std::fs::write(temp.path().join("spargen.toml"), config).unwrap();
        let output = check(temp.path(), &spec, flags);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{config}{flags:?}: {output:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("`{token}`; {accepted}\n")),
            "{config}{flags:?}: {stderr}"
        );
    }
}

#[cfg(feature = "cli")]
#[test]
fn bad_omit_flag_syntax_errors_cleanly() {
    let (temp, spec) = workspace();
    let output = check(temp.path(), &spec, &["--omit-operation", "get"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--omit-operation"));

    // Each message is pinned by `bad_omit_flag_syntax_errors` in `cli/config.rs`; here, that every
    // parse failure of either flag reaches the user as a usage error naming what it refused: the
    // whole value for a malformed one, and the one token for an unknown method or kind. Each is
    // matched in its backtick quotes, which the usage example `schema:LegacyPet` never yields.
    for (flag, value, refused) in [
        ("--omit-operation", "get /pets extra", "get /pets extra"),
        ("--omit-operation", "frobnicate /pets", "frobnicate"),
        ("--omit-component", "LegacyPet", "LegacyPet"),
        ("--omit-component", "schema: ", "schema: "),
        ("--omit-component", "bogus:Name", "bogus"),
    ] {
        let output = check(temp.path(), &spec, &[flag, value]);
        assert_eq!(output.status.code(), Some(3), "{flag} {value}: {output:?}");
        assert!(output.stdout.is_empty(), "{flag} {value}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with("error: ") && stderr.contains(&format!("`{refused}`")),
            "{flag} {value}: {stderr}"
        );
    }
}

#[cfg(feature = "cli")]
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

/// One `uuid` body, one `date-time` body, and one operation spargen rejects (`$dynamicRef`, E006),
/// so `carve`, `uuid`, and `time` each have an effect a test can see: the rejection carved away
/// (W009), and the `uuid` and `time` dependencies `deps` prints.
#[cfg(feature = "cli")]
const KNOBS: &str = r##"
openapi: 3.1.0
info: { title: Knobs, version: 1.0.0 }
paths:
  /id:
    get:
      operationId: getId
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string, format: uuid }
  /now:
    get:
      operationId: getNow
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string, format: date-time }
  /bad:
    get:
      operationId: getBad
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $dynamicRef: "#meta" }
components:
  schemas:
    Legacy: { type: object, properties: { id: { type: string } } }
"##;

#[test]
fn the_macro_tracks_the_config_file_it_applied() {
    // `generate_api!` re-expands only when a file it `include_bytes!`s changes, so the
    // `spargen.toml` it applied must be among them, or an edit to that file alone leaves the
    // previous client in place (#686).
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec_path = root.join("openapi.yaml");
    std::fs::write(&spec_path, SPEC).unwrap();
    let config = root.join("spargen.toml");
    std::fs::write(&config, "batch_cap = 7\n").unwrap();

    let spec = spargen::Spec::new(&spec_path)
        .discover_config_file()
        .unwrap();
    let preview = spargen::__private::preview(&spec);
    assert_eq!(
        preview.report.outcome(),
        spargen::Outcome::Generated,
        "{:#?}",
        preview.report
    );
    assert_eq!(preview.source_files, [spec_path, config]);
}

/// Each `[[omit]]` entry here sets a well-spelled key that the kind its other keys select does
/// not use: the key, and the kind its presence dispatch picks.
const FOREIGN_OMIT_KEYS: &[(&str, &str, &str)] = &[
    ("path = \"/pets\"\nfile = \"extra.yaml\"\n", "file", "path"),
    ("path = \"/pets\"\nname = \"Pet\"\n", "name", "path"),
    (
        "method = \"get\"\npath = \"/pets\"\nfile = \"extra.yaml\"\n",
        "file",
        "operation",
    ),
    (
        "method = \"get\"\npath = \"/pets\"\nname = \"Pet\"\n",
        "name",
        "operation",
    ),
    (
        "component = \"schema\"\nname = \"Pet\"\nmethod = \"get\"\n",
        "method",
        "component",
    ),
    (
        "component = \"schema\"\nname = \"Pet\"\npath = \"/pets\"\n",
        "path",
        "component",
    ),
    (
        "component = \"schema\"\nname = \"Pet\"\nfile = \"extra.yaml\"\n",
        "file",
        "component",
    ),
    (
        "pointer = \"/components/schemas/Pet\"\nname = \"Pet\"\n",
        "name",
        "pointer",
    ),
];

#[test]
fn an_omit_entry_with_a_key_its_kind_does_not_use_is_rejected() {
    // Presence dispatch once dropped these keys silently, so `file` beside `path` omitted the
    // root document's path rather than failing (#730).
    let temp = tempfile::tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let config = root.join("spargen.toml");
    for (entry, key, kind) in FOREIGN_OMIT_KEYS {
        std::fs::write(&config, format!("[[omit]]\n{entry}")).unwrap();
        let error = spargen::Spec::new(root.join("openapi.yaml"))
            .config_file(&config)
            .expect_err(&format!("`{key}` beside a {kind} rule must not load"))
            .to_string();
        assert!(
            error.contains(&format!(
                "omit rule #1: `{key}` does not apply to a {kind} rule"
            )),
            "{entry}: {error}"
        );
    }
}

#[cfg(feature = "cli")]
fn knobs_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, KNOBS).unwrap();
    (temp, spec)
}

#[cfg(feature = "cli")]
#[test]
fn config_file_knobs_reach_the_library() {
    // `batch_cap` from the file is pinned by `batch_cap_from_the_config_file_truncates_the_reported_batch`.
    let (temp, spec) = knobs_workspace();

    // Without the file the `$dynamicRef` operation rejects the run, and the dependency block (with
    // the rejection carved by the flag) carries both `uuid` and `time`.
    let rejected = check(temp.path(), &spec, &["--format", "json"]);
    assert_eq!(rejected.status.code(), Some(1), "{rejected:?}");
    assert!(String::from_utf8_lossy(&rejected.stdout).contains("\"code\":\"E006\""));
    let typed = deps(temp.path(), &spec, &["--carve"]);
    assert!(typed.status.success(), "{typed:?}");
    let typed = String::from_utf8_lossy(&typed.stdout);
    assert!(
        typed.contains("uuid = {") && typed.contains("time = {"),
        "{typed}"
    );

    std::fs::write(
        temp.path().join("spargen.toml"),
        "carve = true\nuuid = false\ntime = false\n",
    )
    .unwrap();
    // `carve` in the file has the same effect as `--carve`: the unsupported operation is carved
    // away (W009) instead of rejecting the run.
    let carved = check(temp.path(), &spec, &["--format", "json"]);
    assert!(carved.status.success(), "{carved:?}");
    let carved = String::from_utf8_lossy(&carved.stdout);
    assert!(omits_pointer(&carved, "/paths/~1bad/get"), "{carved}");
    // `uuid = false` and `time = false` map both formats to `String`, which needs neither crate.
    let untyped = deps(temp.path(), &spec, &[]);
    assert!(untyped.status.success(), "{untyped:?}");
    let untyped = String::from_utf8_lossy(&untyped.stdout);
    assert!(
        !untyped.contains("uuid = {") && !untyped.contains("time = {"),
        "{untyped}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn no_time_maps_dates_to_strings_and_drops_the_time_dependency() {
    let (temp, spec) = knobs_workspace();
    let typed = deps(temp.path(), &spec, &["--carve"]);
    assert!(typed.status.success(), "{typed:?}");
    let typed = String::from_utf8_lossy(&typed.stdout);
    assert!(typed.contains("time = {"), "{typed}");

    let untyped = deps(temp.path(), &spec, &["--carve", "--no-time"]);
    assert!(untyped.status.success(), "{untyped:?}");
    let untyped = String::from_utf8_lossy(&untyped.stdout);
    assert!(!untyped.contains("time = {"), "{untyped}");
    assert!(
        untyped.contains("uuid = {"),
        "`--no-time` leaves `uuid` alone: {untyped}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn error_body_cap_is_accepted_and_changes_no_analysis_output() {
    // The cap only reaches emitted code (`max_error_body`), and these subcommands emit none. What
    // the binary can show is that the flag parses on each subcommand that reads a spec, leaves its
    // output byte-identical, and that clap refuses a non-number.
    let (temp, spec) = workspace();
    let spec = spec.to_str().unwrap();
    for command in [&["check", spec][..], &["deps", spec], &["diff", spec, spec]] {
        let run = |extra: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_spargen"))
                .current_dir(temp.path())
                .args(command)
                .args(extra)
                .args(["--format", "json"])
                .output()
                .unwrap()
        };
        let plain = run(&[]);
        let capped = run(&["--error-body-cap", "7"]);
        assert!(capped.status.success(), "{command:?}: {capped:?}");
        assert_eq!(
            (capped.stdout, capped.stderr),
            (plain.stdout, plain.stderr),
            "{command:?}"
        );
    }

    let refused = check(temp.path(), Path::new(spec), &["--error-body-cap", "lots"]);
    assert_eq!(
        refused.status.code(),
        Some(2),
        "clap's own argument errors exit 2: {refused:?}"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--error-body-cap"),
        "{refused:?}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn omit_component_and_omit_pointer_flags_apply_to_analysis() {
    let (temp, spec) = knobs_workspace();
    let carve = ["--carve", "--format", "json"];

    let component = check(
        temp.path(),
        &spec,
        &[&["--omit-component", "schema:Legacy"][..], &carve].concat(),
    );
    assert!(component.status.success(), "{component:?}");
    let component = String::from_utf8_lossy(&component.stdout);
    assert!(
        omits_pointer(&component, "/components/schemas/Legacy"),
        "{component}"
    );

    // A bare pointer, an empty `file#` (both the root document), and the root named by file.
    for pointer in ["/paths/~1id", "#/paths/~1id", "openapi.yaml#/paths/~1id"] {
        let omitted = check(
            temp.path(),
            &spec,
            &[&["--omit-pointer", pointer][..], &carve].concat(),
        );
        assert!(omitted.status.success(), "{pointer}: {omitted:?}");
        let omitted = String::from_utf8_lossy(&omitted.stdout);
        assert!(
            omits_pointer(&omitted, "/paths/~1id"),
            "{pointer}: {omitted}"
        );
    }
}

/// A response body that narrows a plain `string` to two values: closed, an enum of exactly those;
/// under `open_narrowing`, an open enum with an `Other(String)` catch-all, which `diff` reports as
/// a variant added.
#[cfg(feature = "cli")]
const NARROWED: &str = r##"
openapi: 3.1.0
info: { title: Narrowed, version: 1.0.0 }
paths:
  /status:
    get:
      operationId: getStatus
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                allOf:
                  - type: string
                  - enum: [up, down]
"##;

#[cfg(feature = "cli")]
fn diff(dir: &Path, old: &Path, new: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .arg("diff")
        .arg(old)
        .arg(new)
        .args(extra)
        .output()
        .unwrap()
}

#[cfg(feature = "cli")]
#[test]
fn open_narrowing_is_a_flag_and_a_config_key() {
    // What it changes in emitted code is pinned by
    // `open_narrowing_decodes_an_unlisted_problem_type_and_keeps_it_typed` in `e2e.rs`. Here the
    // change is read off `diff`, which discovers each spec's `spargen.toml` beside it: the same
    // document in two directories, only the new one's config opening the narrowing.
    let temp = tempfile::tempdir().unwrap();
    let (closed_dir, open_dir) = (temp.path().join("closed"), temp.path().join("open"));
    std::fs::create_dir_all(&closed_dir).unwrap();
    std::fs::create_dir_all(&open_dir).unwrap();
    let (closed, open) = (
        closed_dir.join("openapi.yaml"),
        open_dir.join("openapi.yaml"),
    );
    std::fs::write(&closed, NARROWED).unwrap();
    std::fs::write(&open, NARROWED).unwrap();

    let same = diff(temp.path(), &closed, &open, &["--format", "json"]);
    assert!(same.status.success(), "{same:?}");
    assert_eq!(
        String::from_utf8_lossy(&same.stdout).trim_end(),
        r#"{"changes":[],"bump":"patch"}"#,
        "without a config the two sides are the same surface"
    );

    // The key, read from the new spec's directory only: if the old side read it too, or neither
    // did, the surfaces would still match.
    std::fs::write(open_dir.join("spargen.toml"), "open_narrowing = true\n").unwrap();
    let key = diff(temp.path(), &closed, &open, &["--format", "json"]);
    assert!(key.status.success(), "{key:?}");
    let key = String::from_utf8_lossy(&key.stdout);
    assert!(
        key.contains(r#""kind":"variant-added""#) && key.contains("Other(String)"),
        "the new side's config opens its narrowing: {key}"
    );

    // The flag applies to both sides, so it opens the old one too and the surfaces match again.
    let flag = diff(
        temp.path(),
        &closed,
        &open,
        &["--open-narrowing", "--format", "json"],
    );
    assert!(flag.status.success(), "{flag:?}");
    assert_eq!(
        String::from_utf8_lossy(&flag.stdout).trim_end(),
        r#"{"changes":[],"bump":"patch"}"#,
        "the flag opens the old side's narrowing as well"
    );

    std::fs::write(open_dir.join("spargen.toml"), "open_narrowing = 1\n").unwrap();
    let mistyped = check(temp.path(), &open, &[]);
    assert_eq!(mistyped.status.code(), Some(3), "{mistyped:?}");
}

// --- Precedence ---------------------------------------------------------------------------------
//
// `spargen/src/cli/config.rs` states the contract: "defaults < `spargen.toml` < CLI flags", an
// explicit `--config` replaces auto-discovery, and omit flags are *unioned* with the file's rules
// rather than replacing them. Every test above uses a single config source, so none of that was
// exercised — including `explicit_config_path_is_used`, which writes its config into a separate
// directory with no competing `spargen.toml` for it to beat.

/// A JSON-pointer needle that matches the whole pointer value, so `/paths/~1pets` cannot be
/// satisfied by an occurrence of `/paths/~1pets~1{id}`.
#[cfg(feature = "cli")]
fn omits_pointer(stdout: &str, pointer: &str) -> bool {
    stdout.contains(&format!("\"pointer\":\"{pointer}\""))
}

#[cfg(feature = "cli")]
#[test]
fn an_explicit_config_replaces_the_discovered_one_rather_than_merging_with_it() {
    let (temp, spec) = workspace();
    // Discoverable beside the spec, and it omits a *different* path than the explicit file.
    std::fs::write(
        temp.path().join("spargen.toml"),
        "[[omit]]\npath = \"/pets\"\n",
    )
    .unwrap();
    let configs = tempfile::tempdir().unwrap();
    let explicit = configs.path().join("analysis.toml");
    std::fs::write(&explicit, "[[omit]]\npath = \"/pets/{id}\"\n").unwrap();

    let output = check(
        temp.path(),
        &spec,
        &["--config", explicit.to_str().unwrap(), "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        omits_pointer(&stdout, "/paths/~1pets~1{id}"),
        "the explicit config's rule must apply: {stdout}"
    );
    assert!(
        !omits_pointer(&stdout, "/paths/~1pets"),
        "the discovered config must not also apply when --config is given: {stdout}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn a_cli_omit_flag_is_unioned_with_the_config_file_rules() {
    // Flags are applied last so they win on scalar knobs, but omit rules accumulate — a flag adds
    // to the profile rather than discarding what the file declared.
    let (temp, spec) = workspace();
    std::fs::write(
        temp.path().join("spargen.toml"),
        "[[omit]]\npath = \"/pets/{id}\"\n",
    )
    .unwrap();

    let output = check(
        temp.path(),
        &spec,
        &["--omit-path", "/pets", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        omits_pointer(&stdout, "/paths/~1pets~1{id}"),
        "the config file's rule must survive the flag: {stdout}"
    );
    assert!(
        omits_pointer(&stdout, "/paths/~1pets"),
        "the flag's rule must apply too: {stdout}"
    );
}

#[cfg(feature = "cli")]
const TWO_WARNINGS: &str = r##"
openapi: 3.1.0
info: { title: Batch, version: 1.0.0 }
paths: {}
components:
  schemas:
    First: { type: string, minLength: 3 }
    Second: { type: string, maxLength: 9 }
"##;

#[cfg(feature = "cli")]
#[test]
fn batch_cap_from_the_config_file_truncates_the_reported_batch() {
    // `config_file_knobs_reach_the_library` sets four knobs and asserts only that the process
    // exits zero, which a config file that was never read would also satisfy. `batch_cap` has an
    // observable effect: the report says it truncated.
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, TWO_WARNINGS).unwrap();

    let uncapped = check(temp.path(), &spec, &["--format", "json"]);
    assert!(uncapped.status.success(), "{uncapped:?}");
    let uncapped = String::from_utf8_lossy(&uncapped.stdout);
    assert_eq!(
        uncapped.matches("\"code\":\"W001\"").count(),
        2,
        "the fixture must produce two diagnostics to cap: {uncapped}"
    );
    assert!(uncapped.contains("\"truncated\":false"), "{uncapped}");

    std::fs::write(temp.path().join("spargen.toml"), "batch_cap = 1\n").unwrap();
    let capped = check(temp.path(), &spec, &["--format", "json"]);
    assert!(capped.status.success(), "{capped:?}");
    let capped = String::from_utf8_lossy(&capped.stdout);
    assert_eq!(
        capped.matches("\"code\":\"W001\"").count(),
        1,
        "batch_cap from the file must limit the batch: {capped}"
    );
    assert!(
        capped.contains("\"truncated\":true"),
        "a capped batch must say it was truncated: {capped}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn a_cli_flag_beats_the_config_file_on_a_scalar_knob() {
    // "defaults < `spargen.toml` < CLI flags": the file's cap admits both diagnostics, and the flag
    // given beside it caps the batch at one.
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, TWO_WARNINGS).unwrap();
    std::fs::write(temp.path().join("spargen.toml"), "batch_cap = 2\n").unwrap();

    let file = check(temp.path(), &spec, &["--format", "json"]);
    assert!(file.status.success(), "{file:?}");
    let file = String::from_utf8_lossy(&file.stdout);
    assert_eq!(
        file.matches("\"code\":\"W001\"").count(),
        2,
        "the file's cap admits both: {file}"
    );
    assert!(file.contains("\"truncated\":false"), "{file}");

    let flag = check(
        temp.path(),
        &spec,
        &["--batch-cap", "1", "--format", "json"],
    );
    assert!(flag.status.success(), "{flag:?}");
    let flag = String::from_utf8_lossy(&flag.stdout);
    assert_eq!(
        flag.matches("\"code\":\"W001\"").count(),
        1,
        "the flag must win over the file: {flag}"
    );
    assert!(flag.contains("\"truncated\":true"), "{flag}");
}

#[cfg(feature = "cli")]
fn deps(dir: &Path, spec: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .arg("deps")
        .arg(spec)
        .args(extra)
        .output()
        .unwrap()
}

#[cfg(feature = "cli")]
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
    // ...together with the feature wiring the audit requires of a package that opts in.
    assert!(
        stdout.contains("# [features]\n# blocking = [\"dep:tokio\"]\n"),
        "{stdout}"
    );
    // This spec has no uuid/time formats, no XML, and no streams — none of those crates belong.
    for absent in ["uuid", "quick-xml", "futures-core"] {
        assert!(
            !stdout.contains(absent),
            "{absent} should be absent: {stdout}"
        );
    }
}

#[cfg(feature = "cli")]
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

#[cfg(feature = "cli")]
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
// have no inline-spec fixture in `frontend/`. `W013` and `E024` are reachable from any plain
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
