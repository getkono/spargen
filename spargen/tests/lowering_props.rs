//! Issue #34 (layer B) — LOWERING-INVARIANT property tests over the union/allOf lowering, driven
//! end-to-end through `check`/`generate` on synthesized inline specs (the strategy and merge
//! decisions are methods on the private lowering state, so they are exercised via the public
//! frontend rather than called in isolation):
//!
//! * JSON-category unions always lower to a typed enum, including overlapping numeric variants;
//! * closed-object unions always lower to a typed enum, whether required keys prove a fast-path
//!   dispatch or overlapping shapes require typed trial matching;
//! * `allOf` merge reconciles exactly — a property declared with two different types is `E013`, and
//!   an otherwise-consistent merge keeps the union of every member's fields (no field loss) with the
//!   union of every member's `required`.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use proptest::prelude::*;
use spargen::{Code, Config, Outcome, OutputTarget, Report};

/// Run `check` (frontend + lowering, no emit) on an inline spec written into a throwaway tempdir.
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    spargen::check(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
    ))
}

/// Run `generate` to a module and return the report plus the emitted source (when written).
fn generate_module(spec: &str) -> (Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&Config::new(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        OutputTarget::Module(Utf8PathBuf::from_path_buf(out.clone()).unwrap()),
    ));
    let source = std::fs::read_to_string(&out).unwrap_or_default();
    (report, source)
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics.iter().any(|d| d.code == code)
}

// ---------------------------------------------------------------------------
// Property 1: JSON-type-category disjointness is sound.
// ---------------------------------------------------------------------------

/// A JSON primitive category to place in a union variant. `Integer` and `Number` deliberately share
/// the numeric wire category — the lowering must never treat them as disjoint.
#[derive(Clone, Copy, Debug)]
enum Category {
    String,
    Integer,
    Number,
    Boolean,
    Array,
}

impl Category {
    /// The variant's `oneOf` schema line.
    fn schema_line(self) -> &'static str {
        match self {
            Category::String => "        - type: string",
            Category::Integer => "        - type: integer",
            Category::Number => "        - type: number",
            Category::Boolean => "        - type: boolean",
            Category::Array => "        - { type: array, items: { type: string } }",
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

fn category_union_spec(variants: &[Category]) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    U:\n      oneOf:\n",
    );
    for variant in variants {
        spec.push_str(variant.schema_line());
        spec.push('\n');
    }
    spec
}

// ---------------------------------------------------------------------------
// Property 2: required-key disjointness (closed objects) is sound.
// ---------------------------------------------------------------------------

/// The candidate property names for the closed-object variants. Each is a single lowercase letter so
/// its wire name equals its Rust field ident.
const KEYS: [&str; 4] = ["a", "b", "c", "d"];

fn key_set_strategy() -> impl Strategy<Value = BTreeSet<usize>> {
    proptest::collection::btree_set(0usize..KEYS.len(), 1..=KEYS.len())
}

fn closed_object_union_spec(variants: &[BTreeSet<usize>]) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    U:\n      oneOf:\n",
    );
    for keys in variants {
        spec.push_str("        - type: object\n");
        spec.push_str("          additionalProperties: false\n");
        let names: Vec<&str> = keys.iter().map(|&i| KEYS[i]).collect();
        spec.push_str(&format!("          required: [{}]\n", names.join(", ")));
        spec.push_str("          properties:\n");
        for name in &names {
            spec.push_str(&format!("            {name}: {{ type: string }}\n"));
        }
    }
    spec
}

// ---------------------------------------------------------------------------
// Property 3: allOf merge reconciliation.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PropType {
    String,
    Integer,
}

impl PropType {
    fn schema(self) -> &'static str {
        match self {
            PropType::String => "{ type: string }",
            PropType::Integer => "{ type: integer }",
        }
    }
}

/// One `allOf` member: property name → (type, required).
type Member = BTreeMap<usize, (PropType, bool)>;

fn member_strategy() -> impl Strategy<Value = Member> {
    proptest::collection::btree_map(
        0usize..KEYS.len(),
        (
            prop_oneof![Just(PropType::String), Just(PropType::Integer)],
            any::<bool>(),
        ),
        1..=KEYS.len(),
    )
}

fn all_of_spec(members: &[Member]) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    Merged:\n      allOf:\n",
    );
    for member in members {
        spec.push_str("        - type: object\n");
        let required: Vec<&str> = member
            .iter()
            .filter(|(_, (_, req))| *req)
            .map(|(&i, _)| KEYS[i])
            .collect();
        if !required.is_empty() {
            spec.push_str(&format!("          required: [{}]\n", required.join(", ")));
        }
        spec.push_str("          properties:\n");
        for (&i, (ty, _)) in member {
            spec.push_str(&format!("            {}: {}\n", KEYS[i], ty.schema()));
        }
    }
    spec
}

/// `true` when some property name is declared with two different types across members — the
/// irreconcilable case (`E013`).
fn has_type_conflict(members: &[Member]) -> bool {
    let mut seen: BTreeMap<usize, PropType> = BTreeMap::new();
    for member in members {
        for (&name, (ty, _)) in member {
            match seen.get(&name) {
                Some(existing) if existing != ty => return true,
                _ => {
                    seen.insert(name, *ty);
                }
            }
        }
    }
    false
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// Every JSON-category combination lowers to a typed enum. Pairwise-disjoint variants can use a
    /// direct dispatch fast path; repeated categories and `integer | number` use typed trial matching.
    #[test]
    fn json_category_unions_generate_typed(
        variants in proptest::collection::vec(category_strategy(), 2..=5)
    ) {
        let (report, source) = generate_module(&category_union_spec(&variants));
        prop_assert_ne!(report.outcome, Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(&report, Code::NonDisjointUnion), "{:#?}", report);
        prop_assert!(source.contains("pub enum U"), "union was not emitted as a typed enum:\n{source}");
    }

    /// Every closed-object combination lowers to a typed enum. Unique required keys select a direct
    /// dispatch fast path; overlapping required-key sets use typed trial matching.
    #[test]
    fn closed_object_unions_generate_typed(
        variants in proptest::collection::vec(key_set_strategy(), 2..=4)
    ) {
        let (report, source) = generate_module(&closed_object_union_spec(&variants));
        prop_assert_ne!(report.outcome, Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(&report, Code::NonDisjointUnion), "{:#?}", report);
        prop_assert!(source.contains("pub enum U"), "union was not emitted as a typed enum:\n{source}");
    }

    /// A conflicting property type across members is `E013`; otherwise the merge succeeds keeping the
    /// UNION of every member's fields (no field loss) and the UNION of every member's `required`.
    #[test]
    fn all_of_merge_reconciles(
        members in proptest::collection::vec(member_strategy(), 2..=3)
    ) {
        let spec = all_of_spec(&members);

        if has_type_conflict(&members) {
            let report = check(&spec);
            prop_assert_eq!(report.outcome, Outcome::Rejected, "{:#?}", report);
            prop_assert!(has_code(&report, Code::AllOfIrreconcilable), "{:#?}", report);
            return Ok(());
        }

        // No conflict: the merge must succeed and preserve every member's fields.
        let (report, source) = generate_module(&spec);
        prop_assert_ne!(report.outcome, Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(&report, Code::AllOfIrreconcilable), "{:#?}", report);

        // Expected field set = union of member properties; required = union of member required flags.
        let mut required_union: BTreeSet<usize> = BTreeSet::new();
        let mut field_union: BTreeSet<usize> = BTreeSet::new();
        for member in &members {
            for (&name, (_, req)) in member {
                field_union.insert(name);
                if *req {
                    required_union.insert(name);
                }
            }
        }

        for &name in &field_union {
            let ident = KEYS[name];
            let needle = format!("pub {ident}:");
            let line = source
                .lines()
                .find(|l| l.trim_start().starts_with(&needle));
            prop_assert!(line.is_some(), "merged struct dropped field `{ident}`:\n{source}");
            let is_optional = line.unwrap().contains("Option");
            // A field required by ANY member must be plain; a field required by none is `Option`.
            prop_assert_eq!(
                is_optional,
                !required_union.contains(&name),
                "field `{}` optionality disagrees with the required union:\n{}",
                ident,
                source
            );
        }
    }
}
