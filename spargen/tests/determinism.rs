//! The headline invariant (CLAUDE.md): same spargen version + spec + config produces
//! byte-identical output. Generating the same spec into two module paths must yield identical code,
//! the same outcome, and the same diagnostics in the same order. It is held over an inline spec,
//! every generating corpus case, the broad `e2e.rs` spec, and generated union/`allOf` documents.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use proptest::prelude::*;
use spargen::{CargoIntegration, Outcome, Report, Spec};

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

/// One `generate` run: its outcome, its diagnostics rendered in emission order, and the module it
/// wrote (when it wrote one).
fn generate_module(spec_path: &Utf8PathBuf, path: &std::path::Path) -> (Report, Option<Vec<u8>>) {
    let report = spargen::generate(
        &Spec::new(spec_path.clone())
            .build(Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap())
            .cargo(CargoIntegration::Off),
    );
    let module = (report.outcome() == Outcome::Generated)
        .then(|| std::fs::read(path).expect("a `Generated` run wrote its module"));
    (report, module)
}

/// Generate `spec_path` twice into two output directories and require the same outcome, the same
/// diagnostics in the same order, and a byte-identical module. Same spec path + config, different
/// output dirs: the provenance header (which records the source path) is held constant, isolating
/// the invariant to generation order. Returns the outcome both runs reached.
fn assert_generates_identically(spec_path: &Utf8PathBuf, label: &str) -> Outcome {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (report_a, module_a) = generate_module(spec_path, &a.path().join("api.rs"));
    let (report_b, module_b) = generate_module(spec_path, &b.path().join("api.rs"));
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

/// The workspace root, one level above this crate.
fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every `expect = "generate"` case in `corpus/manifest.toml`: real-world descriptions carry far
/// more names, types, and operations than an inline spec, so a `HashMap` iteration order leaking
/// into the output has far more places to show.
#[test]
fn every_generating_corpus_case_is_byte_identical_across_runs() {
    let corpus = workspace_root().join("corpus");
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(corpus.join("manifest.toml")).unwrap())
            .expect("corpus/manifest.toml must parse");
    let cases = manifest["case"]
        .as_array()
        .expect("the manifest is an array of `[[case]]` tables");
    let mut generating = 0;
    for case in cases {
        if case["expect"].as_str() != Some("generate") {
            continue;
        }
        let id = case["id"].as_str().expect("every case has an `id`");
        let path = corpus.join(case["path"].as_str().expect("every case has a `path`"));
        assert_eq!(
            assert_generates_identically(&path, id),
            Outcome::Generated,
            "{id}: the manifest expects it to generate"
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

// Generated specs: the union and `allOf` shapes `lowering_props.rs` drives, combined into one
// document behind one operation. The generators are restated here rather than shared, because each
// integration test is its own crate.

/// A JSON primitive category for a `oneOf` branch. `Integer` and `Number` share the numeric wire
/// category, so some generated unions take typed trial matching rather than direct dispatch.
#[derive(Clone, Copy, Debug)]
enum Category {
    String,
    Integer,
    Number,
    Boolean,
    Array,
}

impl Category {
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

fn category_strategy() -> impl Strategy<Value = Category> {
    prop_oneof![
        Just(Category::String),
        Just(Category::Integer),
        Just(Category::Number),
        Just(Category::Boolean),
        Just(Category::Array),
    ]
}

const KEYS: [&str; 4] = ["a", "b", "c", "d"];

/// One `allOf` member: property index → (`true` for an integer, else a string; required).
type Member = BTreeMap<usize, (bool, bool)>;

fn member_strategy() -> impl Strategy<Value = Member> {
    proptest::collection::btree_map(
        0usize..KEYS.len(),
        (any::<bool>(), any::<bool>()),
        1..=KEYS.len(),
    )
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
        for (&i, (integer, _)) in member {
            let ty = if *integer { "integer" } else { "string" };
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
        closed in proptest::collection::vec(
            proptest::collection::btree_set(0usize..KEYS.len(), 1..=KEYS.len()),
            2..=4,
        ),
        members in proptest::collection::vec(member_strategy(), 2..=4),
    ) {
        let text = composite_spec(&categories, &closed, &members);
        let (_src, spec_path) = write_spec(&text);
        assert_generates_identically(&spec_path, &text);
    }
}
