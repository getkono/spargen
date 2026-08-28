//! Integration coverage for omit-profile globbing (bulk omits) and auto-carve (Issue #24). These
//! drive the supported Rust generation API end-to-end, proving that:
//!
//! * a glob omit-path value removes EVERY matching construct (bulk), while exact
//!   rules are unchanged (that half is pinned by `tests/config.rs`);
//! * carve turns a spec that would REJECT into a generate-what-you-can outcome — dropping only
//!   the unsupported islands, reporting each via `W009`, reaching a fixpoint (no infinite loop),
//!   and staying byte-for-byte deterministic.
//!
use std::path::Path;
use std::process::{Command, Output};

use camino::Utf8PathBuf;
use spargen::{Build, CargoIntegration, Code, Outcome, Report, Spec};

fn spargen(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn config(spec: &Path, out: &Path) -> Build {
    build(
        Spec::new(Utf8PathBuf::from_path_buf(spec.to_path_buf()).unwrap()),
        out,
    )
}

/// As [`config`], but with auto-carve on.
fn carving(spec: &Path, out: &Path) -> Build {
    build(
        Spec::new(Utf8PathBuf::from_path_buf(spec.to_path_buf()).unwrap()).carve(true),
        out,
    )
}

/// As [`config`], but with an explicit omit profile.
fn omitting(spec: &Path, out: &Path, omit: spargen::Omit) -> Build {
    build(
        Spec::new(Utf8PathBuf::from_path_buf(spec.to_path_buf()).unwrap()).omit(omit),
        out,
    )
}

fn build(spec: Spec, out: &Path) -> Build {
    spec.build(Utf8PathBuf::from_path_buf(out.to_path_buf()).unwrap())
        // Not a build script; keep `W013` out of the carve reports these tests assert on.
        .cargo(CargoIntegration::Off)
}

fn write_spec(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn w009_count(report: &Report) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::OmittedConstruct)
        .count()
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
    let config = omitting(&spec, &out, spargen::omit! { paths { "/admin/**"; } });
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        !generated.contains("fn list_users"),
        "admin op removed: {generated}"
    );
    assert!(!generated.contains("fn delete_user"), "admin op removed");
    assert!(generated.contains("fn health"), "public op survives");

    assert_eq!(
        w009_count(&report),
        2,
        "one W009 per bulk-removed path: {report:#?}"
    );
}

#[test]
fn exact_omit_path_still_removes_exactly_one() {
    // Regression guard: a rule with NO glob metacharacter behaves exactly as before — only the one
    // exact path is removed, its sibling `/admin/users/{id}` stays.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ADMIN_SPEC);
    let out = temp.path().join("client.rs");
    let config = omitting(&spec, &out, spargen::omit! { paths { "/admin/users"; } });
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("fn list_users"), "exact path removed");
    assert!(
        generated.contains("fn delete_user"),
        "sibling path survives: {generated}"
    );

    assert_eq!(w009_count(&report), 1, "exactly one W009: {report:#?}");
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
    // Baseline: the same spec REJECTS (E006) without carve, so carve changes the outcome.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&config(&spec, &out));
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(report
        .diagnostics
        .iter()
        .any(|d| d.code == Code::DynamicRefRejected));
}

#[test]
fn carve_generates_the_rest_and_reports_the_carved_operation() {
    // (b) Carve on the rejecting spec generates the REST: the good operation is present, the
    // rejecting operation is absent, and it is reported via W009 (never silent). Outcome flips from
    // Rejected to Generated.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ONE_BAD_OP);
    let out = temp.path().join("client.rs");
    let config = carving(&spec, &out);
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert_eq!(
        w009_count(&report),
        1,
        "the carved op is reported once: {report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("get /bad")),
        "W009 names the carved operation: {report:#?}"
    );
    // No un-carvable residual errors leaked as errors.
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == Code::DynamicRefRejected),
        "the rejection was carved, not left as an error: {report:#?}"
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
    let first_config = carving(&spec, &first);
    let second_config = carving(&spec, &second);
    assert_eq!(spargen::generate(&first_config).outcome, Outcome::Generated);
    assert_eq!(
        spargen::generate(&second_config).outcome,
        Outcome::Generated
    );
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
    let config = carving(&spec, &out);
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    // The component `Bad` and the `$dynamicRef` operation are both carved and reported.
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("component schemas Bad")),
        "carved component reported: {report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("get /dynamic")),
        "carved operation reported: {report:#?}"
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == Code::DynamicRefRejected),
        "the dynamic-ref rejection was carved: {report:#?}"
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == Code::AllOfIrreconcilable),
        "the intersection rejection was carved: {report:#?}"
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
    // (d) Carve on a spec that already generates cleanly changes nothing: it generates normally,
    // with no carve W009s.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", CLEAN_SPEC);
    let carved = temp.path().join("carved.rs");
    let plain = temp.path().join("plain.rs");

    let carved_config = carving(&spec, &carved);
    let report = spargen::generate(&carved_config);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert_eq!(
        w009_count(&report),
        0,
        "no constructs carved on a clean spec: {report:#?}"
    );

    // The carved output is identical to a plain (non-carve) generate — carve added nothing.
    assert_eq!(
        spargen::generate(&config(&spec, &plain)).outcome,
        Outcome::Generated
    );
    let carved = std::fs::read_to_string(&carved).unwrap();
    let plain = std::fs::read_to_string(&plain).unwrap();
    let carved_body = carved.splitn(3, '\n').nth(2).unwrap();
    let plain_body = plain.splitn(3, '\n').nth(2).unwrap();
    assert_eq!(
        carved_body, plain_body,
        "carve on a clean spec generates the same module body"
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
    assert!(stdout.contains("\"outcome\":\"clean\""), "{stdout}");
    assert_eq!(
        stdout.matches("W009").count(),
        1,
        "check reports the carved op: {stdout}"
    );
}

// --- (d) OpenAPI 3.2 constructs are omittable ----------------------------------------------------

/// A 3.2 path whose supported `get` sits beside an operation spargen cannot generate (a response
/// whose only content entry is an unregistered media type). The unsupported operation is reachable
/// only through a 3.2 Path Item field, so it exercises the carve mapping for those fields.
fn oas32_sibling_spec(unsupported: &str) -> String {
    format!(
        r#"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /items:
    get:
      operationId: listItems
      responses:
        "200":
          description: OK
          content:
            application/json: {{ schema: {{ type: string }} }}
{unsupported}
"#
    )
}

#[test]
fn carve_targets_a_query_operation_without_taking_its_path() {
    // `query` is a 3.2 Path Item fixed field. Before `OmitMethod::Query` existed, the carve mapping
    // could not name it and fell back to omitting the whole path — taking the supported `get` with
    // it. Carving must remove the one operation it cannot generate and nothing else.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        &oas32_sibling_spec(
            r#"    query:
      operationId: searchItems
      responses:
        "200":
          description: OK
          content:
            application/sdp: { schema: { type: string } }"#,
        ),
    );
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("fn list_items"),
        "the supported sibling must survive: {generated}"
    );
    assert!(
        !generated.contains("fn search_items"),
        "the unsupported `query` operation must be carved: {generated}"
    );
    assert_eq!(w009_count(&report), 1, "exactly one construct: {report:#?}");
}

#[test]
fn carve_targets_an_additional_operation_without_taking_its_path() {
    // A 3.2 `additionalOperations` method is not a Path Item fixed field, so no `OmitMethod` names
    // it; it is carved as a JSON Pointer instead. Same requirement: the sibling `get` survives.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        &oas32_sibling_spec(
            r#"    additionalOperations:
      PURGE:
        operationId: purgeItems
        responses:
          "200":
            description: OK
            content:
              application/sdp: { schema: { type: string } }"#,
        ),
    );
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("fn list_items"),
        "the supported sibling must survive: {generated}"
    );
    assert!(
        !generated.contains("fn purge_items"),
        "the unsupported `additionalOperations` method must be carved: {generated}"
    );
    assert_eq!(w009_count(&report), 1, "exactly one construct: {report:#?}");
}

#[test]
fn omit_rules_reach_the_component_maps_the_frontend_models() {
    // `components.pathItems` and 3.2's `components.mediaTypes` are both modeled by the frontend and
    // both reachable by `$ref`, so an omit profile has to be able to name them. Neither spelling
    // parsed as a `ComponentKind` before, so both were `E019` "invalid omit rule".
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r#"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      responses:
        "200":
          description: OK
          content:
            application/json: { schema: { type: string } }
components:
  pathItems:
    Unused:
      get:
        operationId: unusedOp
        responses: { "204": { description: No Content } }
  mediaTypes:
    UnusedMedia:
      schema: { type: string }
"#,
    );
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&omitting(
        &spec,
        &out,
        spargen::omit! {
            components {
                path_items { "Unused"; }
                media_types { "UnusedMedia"; }
            }
        },
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert_eq!(w009_count(&report), 2, "one W009 per removal: {report:#?}");
}

#[test]
fn an_omit_profile_does_not_invent_a_paths_requirement() {
    // Both official schemas `require` only `openapi` and `info`, then place `paths`, `components`,
    // and `webhooks` in an `anyOf`. A components-only document is therefore valid OpenAPI, and
    // spargen already accepts it — until any omit profile is attached, at which point the
    // post-omit check demanded `paths` and reported `E020` for a field the profile never touched.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
components:
  schemas:
    Item: { type: object, properties: { id: { type: string } } }
    Legacy: { type: object, properties: { x: { type: string } } }
"#,
    );
    let out = temp.path().join("client.rs");

    // The same document with no profile is already accepted, so the profile is the only difference.
    assert_eq!(
        spargen::generate(&config(&spec, &out)).outcome,
        Outcome::Generated
    );

    let report = spargen::generate(&omitting(
        &spec,
        &out,
        spargen::omit! { components { schemas { "Legacy"; } } },
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == Code::OmitCreatedInvalidDocument),
        "a components-only document is valid OpenAPI: {report:#?}"
    );
}

#[test]
fn omitting_the_last_root_collection_is_still_rejected() {
    // The other half of the `anyOf`: strip the only surviving member and the document really is
    // invalid, so `E020` must still fire.
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      responses: { "204": { description: No Content } }
"#,
    );
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&omitting(
        &spec,
        &out,
        spargen::omit! { pointers { "/paths"; } },
    ));
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == Code::OmitCreatedInvalidDocument),
        "{report:#?}"
    );
}

#[test]
fn every_omit_spelling_reaches_the_same_rule() {
    // The Rust macro, the CLI flags, `spargen.toml`, and the proc-macro must all name the same
    // construct set. `query` and the two new component maps were each added to only some of them
    // before this was pinned.
    let macro_rules = spargen::omit! {
        operations { query "/items"; get "/other"; }
        components { path_items { "Unused"; } media_types { "UnusedMedia"; } }
    };
    let built = spargen::Omit {
        rules: vec![
            spargen::OmitRule::operation(spargen::OmitMethod::Query, "/items"),
            spargen::OmitRule::operation(spargen::OmitMethod::Get, "/other"),
            spargen::OmitRule::component(spargen::ComponentKind::PathItems, "Unused"),
            spargen::OmitRule::component(spargen::ComponentKind::MediaTypes, "UnusedMedia"),
        ],
    };
    assert_eq!(macro_rules, built);

    // The CLI/`spargen.toml` spellings parse to the same variants.
    assert_eq!(
        "query".parse::<spargen::OmitMethod>().unwrap(),
        spargen::OmitMethod::Query
    );
    for token in ["path_items", "pathItems", "path_item"] {
        assert_eq!(
            token.parse::<spargen::ComponentKind>().unwrap(),
            spargen::ComponentKind::PathItems,
            "{token}"
        );
    }
    for token in ["media_types", "mediaTypes", "media_type"] {
        assert_eq!(
            token.parse::<spargen::ComponentKind>().unwrap(),
            spargen::ComponentKind::MediaTypes,
            "{token}"
        );
    }
}

// --- (c) Literal glob metacharacters in document text -------------------------------------------

/// A path containing `*` is a legal URI path (RFC 3986 lists `*` as a sub-delimiter). Auto-carve
/// builds its rules from literal document text, so before metacharacters were escaped a rejection
/// under `/files/*` produced a *bulk* rule that also carved away every sibling `/files/<x>` — the
/// carve removed operations that generate perfectly well.
#[test]
fn carve_of_a_path_containing_a_metacharacter_does_not_take_its_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let spec = write_spec(
        dir,
        "openapi.yaml",
        r#"
openapi: 3.1.0
info: { title: Files, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /files/*:
    get:
      operationId: getGlobFile
      responses:
        "200":
          description: OK
          content:
            application/vnd.unsupported: { schema: { type: string } }
  /files/keep:
    get: { operationId: getKeptFile, responses: { "204": { description: OK } } }
  /files/alsokeep:
    get: { operationId: getOtherFile, responses: { "204": { description: OK } } }
"#,
    );
    let out = dir.join("client.rs");
    let report = spargen::generate(&carving(&spec, &out));
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    // The one unsupported operation is carved...
    assert!(!code.contains("get_glob_file"), "{code}");
    // ...and both siblings that merely share its first path segment survive.
    assert!(code.contains("get_kept_file"), "{code}");
    assert!(code.contains("get_other_file"), "{code}");
    assert_eq!(w009_count(&report), 1, "{report:#?}");
}

/// The same escape makes such a path addressable by hand: `\*` is a literal `*`, so an exact rule
/// can name it, while an unescaped `*` keeps its bulk meaning.
#[test]
fn an_escaped_metacharacter_is_an_exact_rule_and_an_unescaped_one_is_a_glob() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let spec = write_spec(
        dir,
        "openapi.yaml",
        r#"
openapi: 3.1.0
info: { title: Files, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /files/*:
    get: { operationId: getGlobFile, responses: { "204": { description: OK } } }
  /files/keep:
    get: { operationId: getKeptFile, responses: { "204": { description: OK } } }
"#,
    );

    let exact = dir.join("exact.rs");
    let report = spargen::generate(&omitting(
        &spec,
        &exact,
        spargen::Omit {
            rules: vec![spargen::OmitRule::path(r"/files/\*")],
        },
    ));
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(&exact).unwrap();
    assert!(!code.contains("get_glob_file"), "{code}");
    assert!(code.contains("get_kept_file"), "{code}");

    let bulk = dir.join("bulk.rs");
    let report = spargen::generate(&omitting(
        &spec,
        &bulk,
        spargen::Omit {
            rules: vec![spargen::OmitRule::path("/files/*")],
        },
    ));
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(&bulk).unwrap();
    assert!(!code.contains("get_glob_file"), "{code}");
    assert!(!code.contains("get_kept_file"), "{code}");
}
