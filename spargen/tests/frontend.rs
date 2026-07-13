//! Per-diagnostic frontend coverage: one minimal inline spec per rejection/warning code, asserting
//! the code fires and the pipeline outcome is what the taxonomy promises. Rejections travel through
//! `generate`; the E013 case also proves `check` runs the same lowering (check/generate parity).

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget, Report};

/// Run `generate` on an inline spec written into a throwaway tempdir, returning the report. The
/// tempdir (and any written output) is discarded once the report — which owns its data — is built.
fn generate(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    let out = temp.path().join("client.rs");
    spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out).unwrap()),
    ))
}

/// As [`generate`], but through the `check` entry point (no codegen/emit).
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    spargen::check(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ))
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics.iter().any(|d| d.code == code)
}

/// A remote-`$ref` spec fixture referencing a single vendored schema, plus a helper to lay it out
/// in a tempdir with a hand-written lock + vendored file (no network) and run `generate`/`check`.
mod remote {
    use super::*;

    // The exact bytes of the vendored remote document and their real SHA-256 (see the module test
    // asserting spargen's own `sha256` matches this). A mismatch here is a pin-drift fixture.
    const GIZMO_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n";
    const GIZMO_SHA256: &str = "6d9d14b78ee36c68c62cfbde1e06186a7ded59991eb2f5b6aa8b4503209d8974";
    const GIZMO_URL: &str = "https://api.example.com/schemas/gizmo.yaml";
    const GIZMO_VENDOR_PATH: &str = "api.example.com/schemas/gizmo.yaml";

    fn spec() -> String {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /gizmo:\n\
             \x20   get:\n\
             \x20     operationId: getGizmo\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{GIZMO_URL}\"\n"
        )
    }

    fn lock(sha256: &str) -> String {
        format!(
            "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{sha256}\"\npath = \"{GIZMO_VENDOR_PATH}\"\n"
        )
    }

    /// Write the spec and (optionally) a lock + vendored file into a fresh tempdir, then run the
    /// pipeline. Returns the report and the generated module text (when generation ran).
    fn run(
        with_lock: Option<String>,
        with_vendor: Option<&str>,
        check_only: bool,
    ) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec()).unwrap();
        if let Some(lock) = with_lock {
            std::fs::write(dir.join("spargen.lock"), lock).unwrap();
        }
        if let Some(vendor) = with_vendor {
            let path = dir.join(".spargen/vendor").join(GIZMO_VENDOR_PATH);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vendor).unwrap();
        }
        let out = dir.join("client.rs");
        let config = spargen::Config::new(
            dir.join("openapi.yaml"),
            spargen::OutputTarget::Module(out.clone()),
        );
        let report = if check_only {
            spargen::check(&config)
        } else {
            spargen::generate(&config)
        };
        (report, temp, out)
    }

    #[test]
    fn unpinned_remote_ref_is_e003_with_remedy() {
        // No lock present ⇒ the remote ref is unpinned. This must be rejected with the *narrowed*
        // E003 and an actionable remedy pointing at `spargen lock`.
        let (report, _temp, _out) = run(None, None, false);
        assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics
            .iter()
            .find(|d| d.code == Code::AbsoluteRefUnsupported)
            .expect("E003 fires");
        let remedy = diag.remedy.as_deref().unwrap_or_default();
        assert!(
            remedy.contains("spargen lock"),
            "actionable remedy: {remedy}"
        );
        assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    #[test]
    fn pinned_remote_ref_resolves_hermetically_to_typed_schema() {
        // Lock pins the correct sha256 and the vendored bytes match ⇒ the remote ref resolves with
        // no network and lowers to a typed struct (never `serde_json::Value`).
        let (report, _temp, out) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), false);
        assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AbsoluteRefUnsupported),
            "{report:#?}"
        );
        assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
        let generated = std::fs::read_to_string(&out).expect("module written");
        assert!(
            generated.contains("id"),
            "the vendored schema's field is emitted:\n{generated}"
        );

        // check/generate parity: `check` resolves the same remote ref, also without network.
        let (checked, _temp2, _out2) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), true);
        assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
        assert!(
            !has_code(&checked, Code::AbsoluteRefUnsupported),
            "{checked:#?}"
        );
    }

    #[test]
    fn drifted_vendored_content_is_e021() {
        // The vendored bytes are fine, but the lock pins a different sha256 ⇒ the lock is the source
        // of truth, so the drift is refused (E021) rather than silently used.
        let wrong_sha = "0".repeat(64);
        let (report, _temp, _out) = run(Some(lock(&wrong_sha)), Some(GIZMO_YAML), false);
        assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    #[test]
    fn missing_vendored_file_is_e021() {
        // Lock pins the ref but the vendored copy is absent ⇒ drift (nothing to hash against).
        let (report, _temp, _out) = run(Some(lock(GIZMO_SHA256)), None, false);
        assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    /// Lay out `spec` + `lock` + arbitrary vendored files `(vendor-relative path, bytes)` in a
    /// fresh tempdir, then run the pipeline (no network). Returns the report and generated module
    /// path.
    fn run_layout(
        spec: &str,
        lock: Option<&str>,
        vendor: &[(&str, &str)],
        check_only: bool,
    ) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec).unwrap();
        if let Some(lock) = lock {
            std::fs::write(dir.join("spargen.lock"), lock).unwrap();
        }
        for (rel, content) in vendor {
            let path = dir.join(".spargen/vendor").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        let out = dir.join("client.rs");
        let config = spargen::Config::new(
            dir.join("openapi.yaml"),
            spargen::OutputTarget::Module(out.clone()),
        );
        let report = if check_only {
            spargen::check(&config)
        } else {
            spargen::generate(&config)
        };
        (report, temp, out)
    }

    fn responds_with(url: &str) -> String {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /it:\n\
             \x20   get:\n\
             \x20     operationId: getIt\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{url}\"\n"
        )
    }

    #[test]
    fn self_recursive_remote_schema_generates_boxed_not_stack_overflow() {
        // A vendored remote schema that refers to ITSELF (a linked-list `next`) must terminate at
        // lowering with a boxed back-edge — ordinary OpenAPI — instead of recursing forever.
        const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
        const NODE_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n  next:\n    $ref: \"https://api.example.com/schemas/node.yaml\"\n";
        const NODE_SHA: &str = "926f0bc154b93b63208fb4895964ca3e7f67ae3bd7b5f6882156edcefb08fffb";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \"api.example.com/schemas/node.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/node.yaml", NODE_YAML)];

        let (report, _temp, out) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
        let generated = std::fs::read_to_string(&out).unwrap();
        assert!(
            generated.contains("Box"),
            "recursion is closed with a boxed field:\n{generated}"
        );

        // check/generate parity: a regression that reintroduces the crash must fail here too, and
        // both must return an outcome rather than aborting.
        let (checked, _t2, _o2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);
        assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    }

    #[test]
    fn mutually_recursive_remote_docs_generate_boxed() {
        // a.yaml ↔ b.yaml reference each other across two vendored documents; the cross-doc cycle
        // must terminate (boxed) rather than overflow.
        const A_URL: &str = "https://api.example.com/schemas/a.yaml";
        const A_YAML: &str = "type: object\nproperties:\n  b:\n    $ref: \"https://api.example.com/schemas/b.yaml\"\n";
        const A_SHA: &str = "bb995ec038973f6ca10fd6674a76a516dc29962fcdf061e1ad49717b2f6e2544";
        const B_URL: &str = "https://api.example.com/schemas/b.yaml";
        const B_YAML: &str = "type: object\nproperties:\n  a:\n    $ref: \"https://api.example.com/schemas/a.yaml\"\n";
        const B_SHA: &str = "62d1762eb79467f3a7204c626a7a268647910a218ac4b344a878a6421c300674";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{A_URL}\"\nsha256 = \"{A_SHA}\"\npath = \"api.example.com/schemas/a.yaml\"\n\n[[remote]]\nurl = \"{B_URL}\"\nsha256 = \"{B_SHA}\"\npath = \"api.example.com/schemas/b.yaml\"\n"
        );
        let vendor = [
            ("api.example.com/schemas/a.yaml", A_YAML),
            ("api.example.com/schemas/b.yaml", B_YAML),
        ];
        let (report, _temp, out) = run_layout(&responds_with(A_URL), Some(&lock), &vendor, false);
        assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
        let generated = std::fs::read_to_string(&out).unwrap();
        assert!(
            generated.contains("Box"),
            "cross-doc cycle is boxed:\n{generated}"
        );
    }

    #[test]
    fn traversal_vendor_path_in_lock_is_rejected_without_reading() {
        // A hand-edited lock whose `path` escapes the vendor dir must be rejected at lock-parse
        // time — before any file is opened — rather than reading an arbitrary file.
        for bad_path in ["../../etc/passwd", "/etc/passwd"] {
            let lock = format!(
                "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{GIZMO_SHA256}\"\npath = \"{bad_path}\"\n"
            );
            let (report, _temp, _out) = run(Some(lock), Some(GIZMO_YAML), false);
            assert_eq!(report.outcome, Outcome::Rejected, "{bad_path}: {report:#?}");
            assert!(
                has_code(&report, Code::InvalidInput),
                "{bad_path} rejected at parse: {report:#?}"
            );
            // It never reached resolution, so no drift/unpinned diagnostic fires.
            assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
        }
    }
}

#[test]
fn e002_unsupported_dialect() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/not-the-base
paths: {}
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedDialect));
}

#[test]
fn oas32_document_with_compatible_constructs_generates() {
    // OpenAPI 3.2 is a compatible superset of 3.1: a 3.2 document using only 3.1-compatible
    // constructs lowers through the same frontend and generates — no `E001`, no warnings.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedOpenApiVersion),
        "{report:#?}"
    );
    // check/generate parity: the same acceptance is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedOpenApiVersion),
        "{checked:#?}"
    );
}

#[test]
fn oas30_document_still_rejected_e001() {
    // Widening to accept 3.2 must not accept 3.0: it uses different schema semantics and stays
    // rejected with `E001`.
    let report = generate(
        r##"
openapi: 3.0.0
info: { title: T, version: 1.0.0 }
paths: {}
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::UnsupportedOpenApiVersion),
        "{report:#?}"
    );
}

#[test]
fn oas32_base_dialect_accepted() {
    // The OAS 3.2 base dialect string is accepted alongside the 3.1 one — both are the JSON Schema
    // 2020-12-based OAS dialect. No `E002`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://spec.openapis.org/oas/3.2/dialect/base
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnsupportedDialect), "{report:#?}");
}

#[test]
fn oas32_query_method_operation_generates() {
    // The OpenAPI 3.2 fixed `QUERY` path-item method is fully supported: it lowers to an operation
    // and generates a client method like any other verb — no warning, no rejection.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    query:
      operationId: searchItems
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn oas32_self_warns_w010_and_generates() {
    // `$self` sets the document base URI for reference resolution; it does not change locally
    // generated code, so it is acknowledged with `W010` and generation still succeeds.
    let spec = r##"
openapi: 3.2.0
$self: https://api.example.com/openapi.yaml
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    // check/generate parity.
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::Oas32ConstructIgnored),
        "{checked:#?}"
    );
}

#[test]
fn oas32_additional_operations_warns_w010_and_generates() {
    // `additionalOperations` declares custom HTTP methods spargen does not generate; the fixed `get`
    // still generates while the custom method is acknowledged with `W010`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        '200': { description: ok }
    additionalOperations:
      COPY:
        operationId: copyPets
        responses:
          '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
}

#[test]
fn oas32_querystring_param_warns_w010_and_generates() {
    // An `in: querystring` parameter treats the whole query string as one value; spargen skips it
    // with `W010` and still generates the rest of the operation.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: q
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    // The `querystring` param must NOT be rejected as an invalid location.
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn oas32_stream_item_schema_types_the_stream_not_dropped() {
    // OpenAPI 3.2 gives a sequential/streaming media its per-item type in `itemSchema` (not
    // `schema`). A `text/event-stream` response typed only via `itemSchema` must lower to a typed
    // streaming body — the operation still generates, the item type is NOT dropped to a bodyless
    // `()`, and no `itemSchema` warning fires (on streaming media it IS used).
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        '200':
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/Event"
components:
  schemas:
    Event:
      type: object
      required: [seq]
      properties:
        seq: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    // check/generate parity.
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::Oas32ConstructIgnored),
        "{checked:#?}"
    );
}

#[test]
fn oas32_item_schema_on_non_streaming_media_warns_w010() {
    // `itemSchema` is only meaningful for sequential/streaming media. On a plain JSON response it is
    // acknowledged with `W010` (not silently dropped) and generation still succeeds via `schema`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: string }
              itemSchema: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
}

#[test]
fn pattern_properties_lowers_to_typed_map_with_w001() {
    // A representable `patternProperties` now GENERATES a typed overflow map instead of being
    // rejected. Two inline `{type: string}` value schemas under different patterns collapse to one
    // `BTreeMap<String, String>` (bounded structural equivalence over leaf primitives). The key
    // regexes are validation-only and acknowledged as `W001`, never silently dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^x-": { type: string }
        "^y-": { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    // check/generate parity: the same disposition is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "{checked:#?}"
    );
}

#[test]
fn pattern_properties_cyclic_array_values_terminate() {
    // Mutually-recursive array value schemas (`A = [B]`, `B = [A]`) form a cycle in the structural
    // homogeneity comparison. The visited-pair guard must terminate (return an outcome, never abort
    // with a stack overflow) and, since both patterns lower to the same array type, GENERATE one
    // typed overflow map. The check/generate parity assertion catches a regression that reintroduces
    // the crash.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: array, items: { $ref: "#/components/schemas/B" } }
    B: { type: array, items: { $ref: "#/components/schemas/A" } }
    Thing:
      type: object
      patternProperties:
        "^a-": { $ref: "#/components/schemas/A" }
        "^b-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e005_pattern_properties_heterogeneous_rejected() {
    // Two pattern value schemas that lower to different types cannot share one typed map → E005.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: string }
    B: { type: integer }
    Thing:
      type: object
      patternProperties:
        "^s-": { $ref: "#/components/schemas/A" }
        "^i-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
    // check/generate parity: the rejection fires in `check` too.
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::PatternPropertiesRejected));
}

#[test]
fn e005_pattern_properties_with_deny_rejected() {
    // `patternProperties` + `additionalProperties: false` cannot be faithfully represented → E005.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      additionalProperties: false
      patternProperties:
        "^x-": { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
}

#[test]
fn e006_dynamic_ref_rejected() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      $dynamicRef: "#meta"
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DynamicRefRejected));
}

#[test]
fn e007_non_disjoint_union() {
    // `integer | number` share the JSON numeric category (they overlap on the wire), so the union is
    // NOT provably disjoint → E007 (narrowed). A payload `1` could match either variant.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: integer
        - type: number
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
    // check/generate parity.
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion));
}

#[test]
fn e007_overlapping_required_keys_rejected() {
    // Two object variants that share their only required key are not disjoint by key presence, and
    // both are the Object JSON category → no proof holds → E007.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: object
          required: [kind]
          properties: { kind: { type: string }, a: { type: string } }
        - type: object
          required: [kind]
          properties: { kind: { type: string }, b: { type: string } }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
}

#[test]
fn string_integer_union_generates() {
    // `string | integer` occupy distinct JSON type categories (string vs number) → provably disjoint
    // → GENERATES (this replaced the old, incorrect E007 fixture, which asserted rejection here).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn discriminated_union_with_mapping_generates() {
    // A `discriminator` with an explicit mapping over object `$ref` variants → an internally-tagged
    // enum. Generates without E007. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Dog:
      type: object
      required: [bark]
      properties: { bark: { type: boolean } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_discriminated_non_object_variant_rejected() {
    // A discriminated variant that is not an object (a primitive) cannot be internally tagged → E007.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - type: string
      discriminator:
        propertyName: petType
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
}

#[test]
fn disjoint_string_array_union_generates() {
    // `string | string[]` occupy distinct JSON type categories (string vs array) → provably disjoint
    // (ollama's dominant shape). Generates without E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn required_key_disjoint_objects_generate() {
    // Two CLOSED object variants (`additionalProperties: false`) each with a unique required key
    // (`a` / `b`) → provably disjoint by key presence → GENERATES with a content-inspecting custom
    // Deserialize. Closed is required for soundness (see `e007_open_object_required_key_rejected`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      additionalProperties: false
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      additionalProperties: false
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_open_object_required_key_rejected() {
    // OPEN object variants (default `additionalProperties`) are NOT provably disjoint by required
    // key: a payload for B could carry A's key `a` as an extra field and be misrouted → E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn nullable_variant_hoists_to_option() {
    // A variant that is itself nullable (`{type: [string, "null"]}`) has its nullability HOISTED to
    // the union: the union becomes `Option<Enum>` and the string/array variants stay disjoint. A
    // `null` payload resolves at the outer `Option`, so the custom Deserialize only sees non-null.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn nullable_union_collapses_to_option() {
    // A 2-member union where one member is `{type: "null"}` strips the null and collapses to
    // `Option<String>` — no enum, no E007. Generates.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: "null"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e008_non_scalar_enum() {
    // Mixed scalar kinds with no null are genuinely unrepresentable: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      enum: ["a", 1]
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn e008_stays_for_object_member_enum() {
    // Object (or array) enum members have no scalar-variant representation: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Structured:
      enum: [{ a: 1 }]
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn null_mixed_scalar_enum_generates() {
    // A `null` member is stripped; the remaining homogeneous string scalars lower as a nullable
    // enum (`Option<Enum>`). No E008, and generation succeeds. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Severity:
      type: [string, "null"]
      enum: [low, medium, high, null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::NonScalarEnum), "{checked:#?}");
}

#[test]
fn all_null_enum_generates_as_nullable() {
    // A value set of only `null` has no scalar remainder: it lowers to a faithful nullable untyped
    // value rather than being rejected.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nothing:
      enum: [null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e009_unsupported_media_type() {
    // A genuinely unsupported media (`application/pdf`) still rejects with E009 — the narrowing only
    // added JSON-adjacent/XML/streaming media, not arbitrary binary content types.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/pdf:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn xml_request_body_generates() {
    // Issue #13: an `application/xml` request body lowers to a typed struct and generates (no E009);
    // it is serialized through the runtime's quick-xml codec. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [name]
              properties:
                name: { type: string }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn xml_response_body_generates() {
    // Issue #13: a `text/xml` response body lowers to a typed struct and generates (no E009); it is
    // decoded through the runtime's quick-xml codec rather than serde_json.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            text/xml:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn json_alternative_wins_over_xml_on_same_body() {
    // When a body offers both JSON and XML, media selection deterministically prefers JSON, so the
    // API does not use XML at all — generation succeeds with no E009.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema: { type: object }
          application/json:
            schema: { type: object, required: [id], properties: { id: { type: string } } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
}

#[test]
fn w006_unsupported_xml_hint_warns_but_generates() {
    // Issue #13: an unsupported `xml` hint (namespace/prefix/wrapped) is acknowledged with W006 and
    // generation still succeeds — never silently honored or dropped. The `xml.name`/`xml.attribute`
    // hints on sibling fields are honored (no warning).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id]
              properties:
                id:
                  type: string
                  xml: { attribute: true, name: "Id" }
                tags:
                  type: array
                  items: { type: string }
                  xml: { wrapped: true, namespace: "urn:example" }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    let checked = check(spec);
    assert!(has_code(&checked, Code::XmlHintIgnored), "{checked:#?}");
}

#[test]
fn json_only_schema_with_xml_hints_suppresses_rename_and_warns_w006() {
    // Issue #13 regression guard: a schema carrying `xml.name`/`xml.attribute` but reachable only
    // from a JSON body must NOT have the format-agnostic serde rename applied (it would corrupt
    // JSON). The suppression is acknowledged with W006 (never silent), and generation still succeeds.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    let checked = check(spec);
    assert!(has_code(&checked, Code::XmlHintIgnored), "{checked:#?}");
}

#[test]
fn xml_dedicated_schema_applies_hints_without_w006() {
    // A schema used *exclusively* as an XML body is XML-dedicated, so `xml.name`/`xml.attribute` are
    // honored — no suppression, and with no unsupported (namespace/prefix/wrapped) hint present, no
    // W006 fires at all.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn schema_shared_by_json_and_xml_ops_suppresses_rename_and_warns_w006() {
    // A component referenced by BOTH a JSON operation and an XML operation is non-dedicated (it is
    // non-XML-reachable), so the rename is suppressed to keep JSON correct, with W006.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /json:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
  /xml:
    post:
      requestBody:
        content:
          application/xml:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Shared:
      type: object
      required: [id]
      properties:
        id: { type: integer, xml: { attribute: true } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn xml_body_in_multi_status_enum_is_rejected() {
    // Issue #13: XML decode is scoped to single-body success/error. An XML body that would land in a
    // multi-status success enum (two bodied success statuses) is rejected cleanly with narrowed E009
    // rather than silently decoded as JSON.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema: { type: object, required: [a], properties: { a: { type: string } } }
        "201":
          description: Created
          content:
            application/json:
              schema: { type: object, required: [b], properties: { b: { type: string } } }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    // check/generate parity: the same rejection is reached without emitting.
    let checked = check(spec);
    assert_eq!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn sse_response_body_generates() {
    // Issue #14: a `text/event-stream` (SSE) success response is now a typed stream, not `E009`. It
    // generates without the code firing, and check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn ndjson_response_body_generates() {
    // Issue #14: an `application/x-ndjson` success response is a typed stream, not `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /lines:
    get:
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson:
              schema: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn json_alternative_wins_over_stream_media_on_same_response() {
    // When a response offers BOTH a whole-body (JSON) and a streaming alternative, media selection
    // deterministically picks JSON — the operation is a normal `ResponseValue<T>`, not a stream —
    // and generation succeeds with no `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /both:
    get:
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { type: object }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
}

#[test]
fn e009_streaming_request_body_rejected() {
    // Streaming media is response-only: a `text/event-stream` REQUEST body has no representation and
    // stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /push:
    post:
      requestBody:
        content:
          text/event-stream:
            schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn multipart_form_data_request_body_generates() {
    // A `multipart/form-data` request body whose schema is an object (a file part + a text part) is
    // now supported: it generates without E009 firing. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn e009_multipart_non_object_body_rejected() {
    // A `multipart/form-data` body whose schema is NOT an object has no properties to enumerate as
    // form parts, so it stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn binary_format_in_param_and_text_body_positions_generate() {
    // Regression guard for `format: binary` → `bytes::Bytes` in positions rendered as strings: a
    // binary PATH param, a binary QUERY param, and a `text/plain` body of `format: binary` must all
    // generate cleanly (the e2e suite compile-verifies they do not silently miscompile). `Bytes` is
    // not `Display`; params are remapped to `String` and a Bytes body is sent raw, never `.to_string`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob/{token}:
    get:
      parameters:
        - name: token
          in: path
          required: true
          schema: { type: string, format: binary }
        - name: cursor
          in: query
          schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
  /raw:
    post:
      requestBody:
        required: true
        content:
          text/plain:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome, Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e010_unsupported_parameter_style() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: deepObject
          schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedParameterStyle));
}

#[test]
fn e012_unknown_security_scheme() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - undeclared: []
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnknownSecurityScheme));
}

/// A single-member `allOf` now MERGES into one typed struct instead of being rejected (E013 is
/// repurposed to mean "irreconcilable composition"). Generation succeeds with no E013.
#[test]
fn all_of_single_member_merges_into_struct() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Composed:
      allOf:
        - type: object
          properties:
            a: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf: [{$ref: Base}, {properties: {extra}}]` flattens the referenced component's fields plus
/// the inline member's fields into one struct.
#[test]
fn all_of_ref_plus_inline_members_merge() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
        - type: object
          properties:
            extra: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A nested `allOf` (an `allOf` member that itself has an `allOf`) flattens recursively into one
/// struct.
#[test]
fn all_of_nested_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nested:
      allOf:
        - allOf:
            - type: object
              properties:
                a: { type: string }
        - type: object
          properties:
            b: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf` beside the enclosing schema's own sibling `properties`: both sets of fields merge.
#[test]
fn all_of_beside_sibling_properties_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Sibling:
      type: object
      properties:
        own: { type: string }
      allOf:
        - type: object
          properties:
            base: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A property declared with different lowered types in two `allOf` members is irreconcilable → E013.
const ALL_OF_CONFLICT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Conflict:
      allOf:
        - type: object
          properties:
            x: { type: string }
        - type: object
          properties:
            x: { type: integer }
"##;

#[test]
fn e013_all_of_conflicting_property_types_rejected() {
    let report = generate(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Mixing an object member with a scalar member has no single representable type → E013.
#[test]
fn e013_all_of_object_scalar_mix_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      allOf:
        - type: object
          properties:
            a: { type: string }
        - type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// `check` must run the same lowering as `generate`, so an irreconcilable `allOf` rejects
/// identically through both entry points (check/generate parity).
#[test]
fn e013_check_generate_parity() {
    let report = check(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// A self-referential component (`Node.next -> Node`) once recursed forever, then was rejected as
/// E014. It must now generate: the cycle-closing `$ref` is boxed so the recursive type is finite.
#[test]
fn self_recursive_ref_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Node:
      type: object
      properties:
        next:
          $ref: "#/components/schemas/Node"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "recursive schema must not raise an error: {report:#?}"
    );
}

/// Mutually-recursive components (`A -> B -> A`, including recursion through an array) must also
/// generate: exactly one back-edge in the cycle is boxed.
#[test]
fn mutually_recursive_refs_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      properties:
        b:
          $ref: "#/components/schemas/B"
    B:
      type: object
      properties:
        children:
          type: array
          items:
            $ref: "#/components/schemas/A"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "mutually-recursive schemas must not raise an error: {report:#?}"
    );
}

#[test]
fn w001_validation_keyword_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
components:
  schemas:
    Age:
      type: integer
      minimum: 0
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ValidationKeywordIgnored));
}

#[test]
fn w002_server_initiated_flow_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
webhooks:
  newThing:
    post:
      responses:
        "200": { description: OK }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ServerInitiatedFlowIgnored));
}

const W005_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        count:
          type: integer
          default: "not-a-number"
        meta:
          type: object
          default: { a: 1 }
"##;

#[test]
fn w005_schema_default_not_applied_still_generates() {
    let report = generate(W005_SPEC);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// `check` runs the same lowering as `generate`, so the W005 disposition fires identically.
#[test]
fn w005_check_generate_parity() {
    let report = check(W005_SPEC);
    assert_eq!(report.outcome, Outcome::Clean, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// A representable scalar default on an optional field is applied via serde and must not raise
/// W005 (or any error): generation succeeds and the field is documented with its default.
#[test]
fn representable_scalar_default_applies_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        color:
          type: string
          default: "red"
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A parameter `default` is documented in rustdoc (never serde-wired) — generation is clean and
/// must NOT raise W005 (parameters always have a documentation home).
#[test]
fn parameter_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      parameters:
        - name: per_page
          in: query
          schema: { type: integer, default: 30 }
        - name: sort
          in: query
          required: true
          schema: { type: string, default: name }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` on a component schema itself (here an enum) is documented on the generated named
/// type — generation is clean, with no W005 and no double-handling.
#[test]
fn component_root_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` in a structural position with no field home — array `items` and an
/// `additionalProperties` value — is non-silent: it fires W005 and still generates.
#[test]
fn structural_defaults_fire_w005_and_still_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Tags:
      type: array
      items: { type: string, default: hi }
    Counts:
      type: object
      additionalProperties: { type: integer, default: 5 }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// An out-of-range integer default for the field's width (`int32` here) is NOT representable: it
/// must fire W005 and stay rustdoc-only, never rendered into a literal that fails to compile.
#[test]
fn out_of_range_int_default_fires_w005_and_is_not_wired() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        big:
          type: integer
          format: int32
          default: 5000000000
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// A component that is a bare `$ref` with a sibling `default` drops the default when the reference
/// resolves; it must be acknowledged with W005 rather than lost silently, and still generate.
#[test]
fn component_root_ref_with_default_fires_w005_and_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Bar:
      type: string
    Alias:
      $ref: "#/components/schemas/Bar"
      default: aliased
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

#[test]
fn multi_status_success_bodies_generate_a_typed_enum_without_w003() {
    // Two success statuses with DIFFERENT bodies used to degrade to `serde_json::Value` (W003).
    // W003 is retired: the success type is now a typed per-operation response enum, generated with
    // no diagnostic at all.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyA" }
        "201":
          description: Created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyB" }
components:
  schemas:
    BodyA:
      type: object
      properties:
        a: { type: string }
    BodyB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    // No diagnostics at all — the retired W003 must not fire under any code.
    assert!(report.diagnostics.is_empty(), "{report:#?}");
}

#[test]
fn multi_status_error_bodies_generate_a_typed_enum_without_w003() {
    // Two error statuses with DIFFERENT bodies likewise generate a typed error enum, no W003.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string }
        "404":
          description: Not Found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrA" }
        "409":
          description: Conflict
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrB" }
components:
  schemas:
    ErrA:
      type: object
      properties:
        a: { type: string }
    ErrB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics.is_empty(), "{report:#?}");
}

#[test]
fn multi_status_enum_precedence_emits_exact_arm_before_range_and_a_bodyless_unit_variant() {
    // Both classes list a RANGE before an overlapping EXACT in document order (and mix in a bodyless
    // 204). The emitter must reorder to exact-before-range so a real 200/409 dispatches to its exact
    // variant, and the bodyless 204 must appear as a payload-free unit variant — never a silent drop.
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      operationId: getP
      responses:
        "2XX":
          description: RangeOk
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeOk" } } }
        "200":
          description: ExactOk
          content: { application/json: { schema: { $ref: "#/components/schemas/ExactOk" } } }
        "204":
          description: No Content
        "4XX":
          description: RangeErr
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeErr" } } }
        "409":
          description: Conflict
          content: { application/json: { schema: { $ref: "#/components/schemas/Conflict" } } }
components:
  schemas:
    RangeOk: { type: object, properties: { r: { type: string } } }
    ExactOk: { type: object, properties: { e: { type: string } } }
    RangeErr: { type: object, properties: { x: { type: string } } }
    Conflict: { type: object, properties: { c: { type: string } } }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out.clone()).unwrap()),
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics.is_empty(), "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    // Success dispatch: the exact 200 arm is emitted (and thus checked) before the 2XX range arm.
    let exact_200 = code.find("Exact(200u16)").expect("exact 200 selector");
    let range_2xx = code.find("Range(2u8)").expect("2XX range selector");
    assert!(
        exact_200 < range_2xx,
        "exact 200 must precede the 2XX range in the emitted decode chain"
    );
    // Error classification: the exact 409 arm precedes the 4XX range arm.
    let exact_409 = code.find("Exact(409u16)").expect("exact 409 selector");
    let range_4xx = code.find("Range(4u8)").expect("4XX range selector");
    assert!(
        exact_409 < range_4xx,
        "exact 409 must precede the 4XX range in the emitted classification chain"
    );
    // The bodyless 204 is a payload-free unit variant, not dropped and not a `serde_json::Value`.
    assert!(
        code.contains("Status204,"),
        "bodyless 204 must emit a unit variant"
    );
}
