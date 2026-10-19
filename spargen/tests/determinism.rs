//! The headline invariant (CLAUDE.md): same spargen version + spec + config produces
//! byte-identical output. Generating the same spec into two module paths must yield identical code,
//! the same outcome, and the same diagnostics in the same order. It is held over an inline spec,
//! every generating corpus case, the broad `e2e.rs` spec, and generated union/`allOf` documents.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use proptest::prelude::*;
use spargen::{Outcome, Spec};

#[path = "shared/corpus.rs"]
mod corpus;
#[path = "shared/generate.rs"]
mod generate;
#[path = "lowering_props/strategies.rs"]
mod strategies;

use corpus::{corpus_dir, manifest};
use generate::generate_to_string;
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
/// output dirs: the provenance header (which records the source path) is held constant, isolating
/// the invariant to generation order. Returns the outcome both runs reached.
fn assert_generates_identically(spec_path: &Utf8PathBuf, label: &str) -> Outcome {
    let (report_a, module_a) = generate_to_string(Spec::new(spec_path.clone()));
    let (report_b, module_b) = generate_to_string(Spec::new(spec_path.clone()));
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
    report_a.outcome()
}

/// Write `text` to `openapi.yaml` in a fresh directory, returning the directory and the path.
fn write_spec(text: &str) -> (tempfile::TempDir, Utf8PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(src.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, text).unwrap();
    (src, spec_path)
}

#[test]
fn two_runs_produce_byte_identical_output() {
    let (_src, spec_path) = write_spec(SPEC);
    assert_eq!(
        assert_generates_identically(&spec_path, "inline spec"),
        Outcome::Generated
    );
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

/// The broad specification `e2e.rs` compile-checks, read out of that file so the two cannot drift.
fn e2e_basic_spec() -> &'static str {
    const E2E: &str = include_str!("e2e.rs");
    const OPEN: &str = "const BASIC_SPEC: &str = r##\"";
    let start = E2E.find(OPEN).expect("e2e.rs declares `BASIC_SPEC`") + OPEN.len();
    // The first `"##` after the opening delimiter is the one that closes the raw string.
    let len = E2E[start..]
        .find("\"##")
        .expect("`BASIC_SPEC` is a closed raw string");
    &E2E[start..start + len]
}

#[test]
fn the_e2e_basic_spec_is_byte_identical_across_runs() {
    let (_src, spec_path) = write_spec(e2e_basic_spec());
    assert_eq!(
        assert_generates_identically(&spec_path, "e2e BASIC_SPEC"),
        Outcome::Generated
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
        let (_src, spec_path) = write_spec(&text);
        assert_generates_identically(&spec_path, &text);
    }
}
