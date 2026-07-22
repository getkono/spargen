//! Integration coverage for omit-profile globbing (bulk omits) and auto-carve (Issue #24). These
//! drive the real `spargen` binary end-to-end, proving that:
//!
//! * a glob `--omit-path`/`[[omit]]` value removes EVERY matching construct (bulk), while exact
//!   rules are unchanged (that half is pinned by `tests/config.rs`);
//! * `--carve` turns a spec that would REJECT into a generate-what-you-can outcome — dropping only
//!   the unsupported islands, reporting each via `W009`, reaching a fixpoint (no infinite loop),
//!   and staying byte-for-byte deterministic.
//!
//! The binary requires the `cli` feature (always on under `cargo test --all-features`).

use std::path::Path;
use std::process::{Command, Output};

fn spargen(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

/// `generate <spec> --out <out> [extra…]`, returning the process output.
fn generate(dir: &Path, spec: &Path, out: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "generate",
        spec.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    spargen(dir, &args)
}

fn write_spec(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

/// Count `W009` (construct omitted) diagnostics in a `--format json` report.
fn w009_count(stdout: &str) -> usize {
    stdout.matches("W009").count()
}

// --- (a) Glob / bulk omit -----------------------------------------------------------------------

const ADMIN_SPEC: &str = r#"
openapi: 3.1.0
info: { title: Admin, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /admin/users:
    get: { operationId: listUsers, responses: { "200": { description: OK } } }
  /admin/users/{id}:
    delete:
      operationId: deleteUser
      parameters: [ { name: id, in: path, required: true, schema: { type: string } } ]
      responses: { "204": { description: OK } }
  /public/health:
    get: { operationId: health, responses: { "200": { description: OK } } }
"#;

#[test]
fn glob_omit_path_removes_all_matching_operations() {
    // `--omit-path "/admin/**"` is a GLOB: it removes EVERY path under /admin (both `/admin/users`
    // and `/admin/users/{id}`), leaving only the un-matched public path. Two constructs removed ⇒
    // two W009. The bulk removal is the headline of the globbing half.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ADMIN_SPEC);
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--omit-path", "/admin/**", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("fn list_users"),
        "admin op removed: {generated}"
    );
    assert!(!generated.contains("fn delete_user"), "admin op removed");
    assert!(generated.contains("fn health"), "public op survives");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        w009_count(&stdout),
        2,
        "one W009 per bulk-removed path: {stdout}"
    );
}

#[test]
fn exact_omit_path_still_removes_exactly_one() {
    // Regression guard: a rule with NO glob metacharacter behaves exactly as before — only the one
    // exact path is removed, its sibling `/admin/users/{id}` stays.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ADMIN_SPEC);
    let out = temp.path().join("client.rs");
    let output = generate(
        temp.path(),
        &spec,
        &out,
        &["--omit-path", "/admin/users", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("fn list_users"), "exact path removed");
    assert!(
        generated.contains("fn delete_user"),
        "sibling path survives: {generated}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(w009_count(&stdout), 1, "exactly one W009: {stdout}");
}

// --- (b) Carve generates the rest ---------------------------------------------------------------

/// `/good` is representable; `/bad` returns a dynamic reference (an `E006`).
const ONE_BAD_OP: &str = r##"
openapi: 3.1.0
info: { title: Carve, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
  /bad:
    get:
      operationId: getBad
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $dynamicRef: "#meta" }
components: {}
"##;

#[test]
fn without_carve_a_rejecting_spec_fails() {
    // Baseline: the same spec REJECTS (E006) without `--carve`, so carve is what changes the outcome.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--format", "json"]);
    assert!(!output.status.success(), "rejects without carve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"outcome\":\"Rejected\""), "{stdout}");
    assert!(stdout.contains("E006"), "{stdout}");
}

#[test]
fn carve_generates_the_rest_and_reports_the_carved_operation() {
    // (b) `--carve` on the rejecting spec generates the REST: the good operation is present, the
    // rejecting operation is absent, and it is reported via W009 (never silent). Outcome flips from
    // Rejected to Generated.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--carve", "--format", "json"]);
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"outcome\":\"Generated\""), "{stdout}");
    assert_eq!(
        w009_count(&stdout),
        1,
        "the carved op is reported once: {stdout}"
    );
    assert!(
        stdout.contains("get /bad"),
        "W009 names the carved operation: {stdout}"
    );
    // No un-carvable residual errors leaked as errors.
    assert!(
        !stdout.contains("E006"),
        "the rejection was carved, not left as an error: {stdout}"
    );

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("fn get_good"),
        "the rest is generated: {generated}"
    );
    assert!(
        !generated.contains("fn get_bad"),
        "the rejecting op is absent"
    );
}

#[test]
fn carve_output_is_deterministic() {
    // Determinism: carve produces byte-identical output on a second run (same spec + version).
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let first = temp.path().join("first.rs");
    let second = temp.path().join("second.rs");
    assert!(generate(temp.path(), &spec, &first, &["--carve"])
        .status
        .success());
    assert!(generate(temp.path(), &spec, &second, &["--carve"])
        .status
        .success());
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "carve output must be byte-identical across runs"
    );
}

// --- (c) Fixpoint / termination, including a component cascade -----------------------------------

/// Mixes THREE kinds of rejection so carve must remove constructs of different kinds and iterate to
/// a fixpoint: an incompatible component intersection (`Bad`, `E013`), an operation that returns a
/// `$dynamicRef` (`E006`), and a healthy operation. Omitting the component `Bad` cascades to the
/// operation that referenced it.
const MIXED_REJECTIONS: &str = r##"
openapi: 3.1.0
info: { title: Mixed, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
  /uses-bad:
    get:
      operationId: getUsesBad
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Bad" }
  /dynamic:
    get:
      operationId: getDynamic
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $dynamicRef: "#meta" }
components:
  schemas:
    Bad:
      allOf:
        - { type: string }
        - { type: integer }
"##;

#[test]
fn carve_reaches_a_fixpoint_and_terminates_with_a_component_cascade() {
    // (c) Carve iterates to a fixpoint over MULTIPLE kinds of construct — a component (`Bad`) and an
    // operation (`getDynamic`) — and terminates (no infinite loop; the process returns). The healthy
    // operation is generated; the carved component and operation are each reported via W009; no
    // residual error leaks. This exercises the pointer→construct mapping for both `components/*` and
    // `paths/*` and the round-bounded fixpoint driver.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", MIXED_REJECTIONS);
    let out = temp.path().join("client.rs");
    let output = generate(temp.path(), &spec, &out, &["--carve", "--format", "json"]);
    assert!(
        output.status.success(),
        "carve terminates and generates: {output:?}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"outcome\":\"Generated\""), "{stdout}");
    // The component `Bad` and the `$dynamicRef` operation are both carved and reported.
    assert!(
        stdout.contains("component schemas Bad"),
        "carved component reported: {stdout}"
    );
    assert!(
        stdout.contains("get /dynamic"),
        "carved operation reported: {stdout}"
    );
    assert!(
        !stdout.contains("E006"),
        "the dynamic-ref rejection was carved: {stdout}"
    );
    assert!(
        !stdout.contains("E013"),
        "the intersection rejection was carved: {stdout}"
    );

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("fn get_good"),
        "the healthy op is generated: {generated}"
    );
    assert!(
        !generated.contains("fn get_dynamic"),
        "the dynamic-ref op is absent"
    );
}

// --- (d) Carve is a no-op on a clean spec -------------------------------------------------------

const CLEAN_SPEC: &str = r#"
openapi: 3.1.0
info: { title: Clean, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
"#;

#[test]
fn carve_is_a_noop_on_a_spec_with_no_rejections() {
    // (d) `--carve` on a spec that already generates cleanly changes nothing: it generates normally,
    // with no carve W009s.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", CLEAN_SPEC);
    let carved = temp.path().join("carved.rs");
    let plain = temp.path().join("plain.rs");

    let output = generate(
        temp.path(),
        &spec,
        &carved,
        &["--carve", "--format", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        w009_count(&stdout),
        0,
        "no constructs carved on a clean spec: {stdout}"
    );

    // The carved output is identical to a plain (non-carve) generate — carve added nothing.
    assert!(generate(temp.path(), &spec, &plain, &[]).status.success());
    assert_eq!(
        std::fs::read(&carved).unwrap(),
        std::fs::read(&plain).unwrap(),
        "carve on a clean spec equals a plain generate"
    );
}

#[test]
fn check_command_supports_carve() {
    // `spargen check --carve` audits the carved subset clean (it runs the full frontend), reporting
    // the carved construct via W009 and exiting 0.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let output = spargen(
        temp.path(),
        &[
            "check",
            spec.to_str().unwrap(),
            "--carve",
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"outcome\":\"Clean\""), "{stdout}");
    assert_eq!(
        w009_count(&stdout),
        1,
        "check reports the carved op: {stdout}"
    );
}
