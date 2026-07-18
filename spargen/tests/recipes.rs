//! Framework round-trip recipes (Issue #30): proves spargen turns the OpenAPI document EMITTED BY a
//! Rust server framework (utoipa, aide, poem-openapi) into a client, i.e. the round-trip
//! `Rust server → OpenAPI → spargen client`.
//!
//! Each case reads a vendored spec under `corpus/recipes/` that mirrors that framework's OUTPUT
//! IDIOMS (see `corpus/recipes/README.md` for provenance and the verified version constants) and
//! asserts spargen's outcome, so the recipes in `docs/recipes.md` stay honest:
//!
//! * `utoipa` (emits OpenAPI 3.1.0) — generates cleanly; exercises `type: [T, null]` nullables,
//!   nullable `$ref` via a `oneOf` `null` member, `allOf`-composed models, and a `discriminator`ed
//!   union across tag-grouped, multi-status operations.
//! * `aide` (emits OpenAPI 3.1.0, schemars schemas) — generates with only validation-only warnings
//!   (`W001`); exercises `anyOf`/`type`-array nullables, an externally-tagged (content-dispatched)
//!   union, a by-JSON-type disjoint union, and an `allOf` flatten.
//! * `poem-openapi` (emits OpenAPI 3.0.0) — REJECTED with `E001`, proving the 3.1.x requirement and
//!   motivating the "upgrade to 3.1" step of its recipe.
//! * the `--carve` escape hatch — a utoipa document carrying one unrepresentable untagged union
//!   (`E007`) is rejected whole without carve, and generates the rest (reporting `W009`) with it.
//!
//! Everything is deterministic and offline: the specs are vendored into the repo and the test only
//! reads local files (no network).

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget, Report};

/// Absolute path to a vendored recipe spec (workspace root is one level up from this crate).
fn recipe_path(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
        .join("recipes")
        .join(name)
}

/// Run `check` on a vendored recipe spec.
fn check(name: &str) -> Report {
    spargen::check(&Config::new(
        recipe_path(name),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ))
}

/// Run `generate` on a vendored recipe spec (optionally with `--carve`), returning the report and
/// the emitted module text (when generation ran). Output goes to a throwaway tempdir.
fn generate(name: &str, carve: bool) -> (Report, Option<String>) {
    let temp = tempfile::tempdir().unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let mut config = Config::new(recipe_path(name), OutputTarget::Module(out.clone()));
    config.carve = carve;
    let report = spargen::generate(&config);
    let text = std::fs::read_to_string(out).ok();
    (report, text)
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics.iter().any(|d| d.code == code)
}

fn error_codes(report: &Report) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.as_str().starts_with('E'))
        .map(|d| d.code.as_str())
        .collect()
}

// --- utoipa: emits OpenAPI 3.1.0, generates cleanly ---------------------------------------------

#[test]
fn utoipa_document_generates_cleanly() {
    // `check` and `generate` must agree (the recipe tells users `check` is a safe pre-flight).
    let checked = check("utoipa.json");
    assert_eq!(checked.outcome, Outcome::Clean, "{:?}", checked.diagnostics);
    assert!(
        error_codes(&checked).is_empty(),
        "no errors expected: {:?}",
        checked.diagnostics
    );

    let (report, text) = generate("utoipa.json", false);
    assert_eq!(
        report.outcome,
        Outcome::Generated,
        "{:?}",
        report.diagnostics
    );
    assert!(
        error_codes(&report).is_empty(),
        "no errors expected: {:?}",
        report.diagnostics
    );
    let text = text.expect("utoipa generation wrote a module");
    // The idiomatic operations (tag-grouped, multi-status, union return) all lower to methods.
    for op in [
        "fn list_pets",
        "fn create_pet",
        "fn get_pet",
        "fn latest_event",
    ] {
        assert!(text.contains(op), "missing operation {op}");
    }
}

// --- aide: emits OpenAPI 3.1.0 (schemars), generates with only validation-only warnings ----------

#[test]
fn aide_document_generates_with_only_validation_warnings() {
    let (report, text) = generate("aide.json", false);
    assert_eq!(
        report.outcome,
        Outcome::Generated,
        "{:?}",
        report.diagnostics
    );
    assert!(
        error_codes(&report).is_empty(),
        "no errors expected: {:?}",
        report.diagnostics
    );
    // schemars emits `minimum`/`format` validation hints that spargen faithfully ignores (W001);
    // any other diagnostic class would be a surprise the recipe should mention.
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code == Code::ValidationKeywordIgnored),
        "only validation-only warnings expected: {:?}",
        report.diagnostics
    );
    let text = text.expect("aide generation wrote a module");
    for op in [
        "fn list_items",
        "fn get_item",
        "fn add_shape",
        "fn get_scalar",
    ] {
        assert!(text.contains(op), "missing operation {op}");
    }
}

// --- poem-openapi: emits OpenAPI 3.0.0, rejected with E001 --------------------------------------

#[test]
fn poem_openapi_document_is_rejected_e001() {
    // poem-openapi pins OPENAPI_VERSION = "3.0.0"; spargen requires 3.1.x/3.2.x, so the document is
    // rejected loudly with E001 (never silently degraded). check/generate agree.
    let checked = check("poem-openapi.json");
    assert_eq!(checked.outcome, Outcome::Rejected);
    assert!(has_code(&checked, Code::UnsupportedOpenApiVersion));

    let (report, _) = generate("poem-openapi.json", false);
    assert_eq!(report.outcome, Outcome::Rejected);
    assert!(
        has_code(&report, Code::UnsupportedOpenApiVersion),
        "E001 expected: {:?}",
        report.diagnostics
    );
}

// --- --carve escape hatch on a framework idiom spargen cannot represent -------------------------

#[test]
fn utoipa_untagged_overlap_is_rejected_but_carves() {
    // Without carve the single unrepresentable untagged union (E007) rejects the WHOLE document.
    let (plain, _) = generate("utoipa-untagged-overlap.json", false);
    assert_eq!(plain.outcome, Outcome::Rejected);
    assert!(
        has_code(&plain, Code::NonDisjointUnion),
        "E007 expected: {:?}",
        plain.diagnostics
    );

    // With `--carve` spargen drops only the offending operation (reported W009) and generates the
    // rest — the escape hatch the recipe documents.
    let (carved, text) = generate("utoipa-untagged-overlap.json", true);
    assert_eq!(
        carved.outcome,
        Outcome::Generated,
        "{:?}",
        carved.diagnostics
    );
    assert!(
        has_code(&carved, Code::OmittedConstruct),
        "W009 expected: {:?}",
        carved.diagnostics
    );
    assert!(
        !has_code(&carved, Code::NonDisjointUnion),
        "the union was carved, not left as an error: {:?}",
        carved.diagnostics
    );
    let text = text.expect("carve generation wrote a module");
    assert!(text.contains("fn ping"), "the clean op survives carve");
    assert!(
        !text.contains("fn measure"),
        "the carved op is absent from the output"
    );
}
