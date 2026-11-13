//! The headline invariant (CLAUDE.md): same spargen version + spec + config produces
//! byte-identical output. Generating the same spec into two module paths must yield identical code,
//! the same outcome, and the same diagnostics in the same order. It is held over an inline spec,
//! every generating corpus case, the broad `e2e.rs` spec (through `generate` with default and with
//! non-default options, and through the proc-macro preview), `e2e.rs`'s 3.2 and date specs, and
//! generated union/`allOf` documents; and across the spellings of the spec's path and the
//! directories a description is checked out into.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use proptest::prelude::*;
use spargen::{Outcome, Spec};

#[allow(
    dead_code,
    reason = "this suite reads only a case's `id`, `path` and `expect`"
)]
#[path = "shared/corpus.rs"]
mod corpus;
#[allow(
    dead_code,
    reason = "this suite generates through `generate_to_string` and `write_spec` alone"
)]
#[path = "shared/generate.rs"]
mod generate;
#[path = "lowering_props/strategies.rs"]
mod strategies;

use corpus::{corpus_dir, manifest};
use generate::{generate_to_string, write_spec};
use strategies::{
    category_strategy, key_set_strategy, member_strategy, Category, Member, PropType, KEYS,
};

const SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Determinism
  version: 1.0.0
servers:
  - url: https://example.com/api
paths:
  /users/{id}:
    get:
      operationId: getUser
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/User" }
components:
  schemas:
    User:
      type: object
      required: [id, name]
      properties:
        id: { type: string }
        name: { type: string }
        age: { type: integer }
"##;

/// Generate `spec_path` twice into two output directories and require the same outcome, the same
/// diagnostics in the same order, and a byte-identical module. Same spec path + config, different
/// output dirs, isolating the invariant to generation order; the spellings of the spec's path are
/// [`the_module_does_not_depend_on_the_spec_path_or_where_the_checkout_lives`]'s. Returns the
/// outcome both runs reached.
fn assert_generates_identically(spec_path: &Utf8PathBuf, label: &str) -> Outcome {
    assert_configured_generates_identically(spec_path, label, |spec| spec).0
}

/// [`assert_generates_identically`] with `configure` applied to both runs' [`Spec`], returning the
/// outcome and the module both runs wrote.
fn assert_configured_generates_identically(
    spec_path: &Utf8PathBuf,
    label: &str,
    configure: impl Fn(Spec) -> Spec,
) -> (Outcome, Option<String>) {
    let (report_a, module_a) = generate_to_string(configure(Spec::new(spec_path.clone())));
    let (report_b, module_b) = generate_to_string(configure(Spec::new(spec_path.clone())));
    assert_eq!(
        report_a.outcome(),
        report_b.outcome(),
        "{label}: two runs reached different outcomes"
    );
    assert_eq!(
        format!("{:?}", report_a.diagnostics()),
        format!("{:?}", report_b.diagnostics()),
        "{label}: two runs reported different diagnostics"
    );
    assert!(
        module_a == module_b,
        "{label}: generated module is not deterministic"
    );
    (report_a.outcome(), module_a)
}

/// The inline spec above and two of the specifications `e2e.rs` compiles, each written as its own
/// root document. The OpenAPI 3.2 specification's constructs (`itemSchema` sequential media, the
/// `query` method, `additionalOperations`) take code paths `BASIC_SPEC` does not reach, and the date
/// specification `e2e.rs` round-trips over the wire puts both date formats in every position a
/// value reaches the wire from.
#[test]
fn the_inline_and_e2e_specs_are_byte_identical_across_runs() {
    for (label, text) in [
        ("inline spec", SPEC),
        ("e2e OAS32_SPEC", include_str!("e2e/specs/oas32.yaml")),
        ("e2e DATE_SPEC", include_str!("e2e/specs/date.yaml")),
    ] {
        let src = tempfile::tempdir().unwrap();
        let spec_path = write_spec(src.path(), "openapi.yaml", text);
        assert_eq!(
            assert_generates_identically(&spec_path, label),
            Outcome::Generated,
            "{label}"
        );
    }
}

/// A description split over a root document and a `$ref`'d file in a subdirectory, written into
/// `dir`; returns the root's path.
fn write_split_spec(dir: &std::path::Path) -> Utf8PathBuf {
    std::fs::create_dir_all(dir.join("schemas")).unwrap();
    write_spec(
        &dir.join("schemas"),
        "user.yaml",
        "type: object\nrequired: [id]\nproperties:\n  id: { type: string }\n",
    );
    write_spec(
        dir,
        "openapi.yaml",
        &SPEC.replace(
            "schema: { $ref: \"#/components/schemas/User\" }",
            "schema: { $ref: \"schemas/user.yaml\" }",
        ),
    )
}

/// `path` relative to `base`, both canonicalized first: `..` for each segment of `base` past their
/// common prefix, then the rest of `path`.
fn relative_path(base: &std::path::Path, path: &std::path::Path) -> Utf8PathBuf {
    let base = base.canonicalize().unwrap();
    let path = path.canonicalize().unwrap();
    let base: Vec<_> = base.components().collect();
    let path: Vec<_> = path.components().collect();
    let common = base.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut relative = std::path::PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    relative.extend(&path[common..]);
    Utf8PathBuf::from_path_buf(relative).expect("the relative path is UTF-8")
}

/// The module is a function of the description's bytes and its files' layout relative to one
/// another, not of where the checkout lives or how the spec path is spelled (#1347). One split
/// description, named by an absolute path, by an unnormalized absolute path, by a path relative to
/// the working directory, and copied into another directory, generates byte-identical modules, and
/// the provenance header names the spec by its file name alone.
#[test]
fn the_module_does_not_depend_on_the_spec_path_or_where_the_checkout_lives() {
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let here = tempfile::tempdir_in(scratch).unwrap();
    let elsewhere = tempfile::tempdir_in(scratch).unwrap();
    let absolute = write_split_spec(here.path());
    let copied = write_split_spec(elsewhere.path());
    let unnormalized = Utf8PathBuf::from_path_buf(here.path().join("schemas/../openapi.yaml"))
        .expect("the temporary path is UTF-8");
    let relative = relative_path(&std::env::current_dir().unwrap(), absolute.as_std_path());
    assert!(relative.is_relative(), "{relative}");

    let mut modules = Vec::new();
    for (label, path) in [
        ("absolute", &absolute),
        ("unnormalized", &unnormalized),
        ("relative to the working directory", &relative),
        ("copied into another directory", &copied),
    ] {
        let (report, module) = generate_to_string(Spec::new(path.clone()));
        assert_eq!(report.outcome(), Outcome::Generated, "{label}: {report:#?}");
        modules.push((label, module.expect("a generated run wrote its module")));
    }
    let (_, reference) = &modules[0];
    assert!(
        reference.contains("\n// source: openapi.yaml\n"),
        "the header names the spec by its file name: {reference}"
    );
    for (label, module) in &modules[1..] {
        assert!(
            module == reference,
            "{label}: the module differs from the absolute path's"
        );
    }
}

/// Every `expect = "generate"` case in `corpus/manifest.toml`: real-world descriptions carry far
/// more names, types, and operations than an inline spec, so a `HashMap` iteration order leaking
/// into the output has far more places to show.
#[test]
fn every_generating_corpus_case_is_byte_identical_across_runs() {
    let mut generating = 0;
    for case in manifest().cases {
        if case.expect != "generate" {
            continue;
        }
        let path = corpus_dir().join(&case.path);
        assert_eq!(
            assert_generates_identically(&path, &case.id),
            Outcome::Generated,
            "{}: the manifest expects it to generate",
            case.id
        );
        generating += 1;
    }
    assert!(generating > 0, "the manifest names no generating case");
}

/// The broad specification `e2e.rs` compile-checks: the file its `BASIC_SPEC` includes, so the
/// two cannot drift.
fn e2e_basic_spec() -> &'static str {
    include_str!("e2e/specs/basic.yaml")
}

/// The broad spec under the default options and under every option set away from its default: the
/// configuration is part of the invariant's input, so a non-default value must be as deterministic
/// as the default one. Both runs generate, and the two modules must differ, or the options reached
/// nothing this test could hold.
#[test]
fn the_e2e_basic_spec_is_byte_identical_across_runs_under_default_and_non_default_options() {
    let src = tempfile::tempdir().unwrap();
    let spec_path = write_spec(src.path(), "openapi.yaml", e2e_basic_spec());
    let (outcome, module) =
        assert_configured_generates_identically(&spec_path, "e2e BASIC_SPEC, options", |spec| {
            spec.with_uuid(spargen::UuidMapping::String)
                .with_datetime(spargen::DateTimeBackend::String)
                .with_open_narrowing(spargen::Narrowing::Open)
                .with_open_unions(spargen::UnionTags::Open)
                .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
                .with_testing(spargen::TestingModule::Emitted)
                .with_error_body_cap(1024)
                .with_batch_cap(7)
                .with_carve(spargen::Carve::Auto)
        });
    assert_eq!(outcome, Outcome::Generated);
    let (default_outcome, default_module) =
        assert_configured_generates_identically(&spec_path, "e2e BASIC_SPEC", |spec| spec);
    assert_eq!(default_outcome, Outcome::Generated);
    assert_ne!(
        module, default_module,
        "the non-default options must change the emitted module"
    );
}

/// The hidden proc-macro bridge renders the broad spec deterministically and writes nothing: after
/// two previews the spec's directory holds the spec alone.
#[test]
fn the_macro_preview_is_byte_identical_across_runs_and_writes_nothing() {
    let src = tempfile::tempdir().unwrap();
    let spec_path = write_spec(src.path(), "openapi.yaml", e2e_basic_spec());
    let config = Spec::new(spec_path);
    let preview = spargen::__private::preview(&config);
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        preview.report
    );
    let contents = preview.contents.expect("generated module");
    let again = spargen::__private::preview(&config);
    assert_eq!(again.contents.as_deref(), Some(contents.as_str()));
    let written: Vec<_> = std::fs::read_dir(src.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        written,
        ["openapi.yaml"],
        "the macro preview must not write beside the spec"
    );
}

// Generated specs: the union and `allOf` shapes `lowering_props.rs` drives, drawn from its own
// strategies and combined into one document behind one operation.

impl Category {
    /// The branch's schema as a YAML flow mapping.
    fn schema(self) -> &'static str {
        match self {
            Category::String => "{ type: string }",
            Category::Integer => "{ type: integer }",
            Category::Number => "{ type: number }",
            Category::Boolean => "{ type: boolean }",
            Category::Array => "{ type: array, items: { type: string } }",
            Category::Object => {
                "{ type: object, required: [o], properties: { o: { type: object, required: [p], properties: { p: { type: string } } } } }"
            }
            Category::Null => "{ type: 'null' }",
        }
    }
}

/// A document holding a JSON-category union `U`, a closed-object union `C`, and an `allOf` merge
/// `Merged`, all reached from one operation's response.
fn composite_spec(
    categories: &[Category],
    closed: &[BTreeSet<usize>],
    members: &[Member],
) -> String {
    const HEAD: &str = r##"openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /things:
    get:
      operationId: getThings
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: object
                properties:
                  u: { $ref: "#/components/schemas/U" }
                  c: { $ref: "#/components/schemas/C" }
                  m: { $ref: "#/components/schemas/Merged" }
components:
  schemas:
    U:
      oneOf:
"##;
    let mut spec = String::from(HEAD);
    for category in categories {
        spec.push_str(&format!("        - {}\n", category.schema()));
    }
    spec.push_str("    C:\n      oneOf:\n");
    for keys in closed {
        let names: Vec<&str> = keys.iter().map(|&i| KEYS[i]).collect();
        spec.push_str("        - type: object\n          additionalProperties: false\n");
        spec.push_str(&format!("          required: [{}]\n", names.join(", ")));
        spec.push_str("          properties:\n");
        for name in &names {
            spec.push_str(&format!("            {name}: {{ type: string }}\n"));
        }
    }
    spec.push_str("    Merged:\n      allOf:\n");
    for member in members {
        spec.push_str("        - type: object\n");
        let required: Vec<&str> = member
            .iter()
            .filter(|(_, (_, required))| *required)
            .map(|(&i, _)| KEYS[i])
            .collect();
        if !required.is_empty() {
            spec.push_str(&format!("          required: [{}]\n", required.join(", ")));
        }
        spec.push_str("          properties:\n");
        for (&i, (ty, _)) in member {
            let ty = match ty {
                PropType::String => "string",
                PropType::Integer => "integer",
            };
            spec.push_str(&format!("            {}: {{ type: {ty} }}\n", KEYS[i]));
        }
    }
    spec
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Generated documents of every union and `allOf` shape: a rejection must be the same
    /// rejection twice, and a generated module the same bytes twice.
    #[test]
    fn generated_union_and_all_of_specs_are_byte_identical_across_runs(
        categories in proptest::collection::vec(category_strategy(), 2..=5),
        closed in proptest::collection::vec(key_set_strategy(), 2..=4),
        members in proptest::collection::vec(member_strategy(), 2..=4),
    ) {
        let text = composite_spec(&categories, &closed, &members);
        let src = tempfile::tempdir().unwrap();
        let spec_path = write_spec(src.path(), "openapi.yaml", &text);
        assert_generates_identically(&spec_path, &text);
    }
}
