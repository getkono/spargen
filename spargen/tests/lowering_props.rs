//! LOWERING-INVARIANT property tests over the union/allOf lowering, driven
//! end-to-end through `check`/`generate` on synthesized inline specs (the strategy and merge
//! decisions are methods on the private lowering state, so they are exercised via the public
//! frontend rather than called in isolation):
//!
//! * JSON-category unions always lower to a typed enum, including overlapping numeric variants,
//!   with repeated categories — which lower to one generated type, so a `oneOf` could decode none
//!   of their values — merged into one variant and reported (`W001`);
//! * closed-object unions always lower to a typed enum, whether required keys prove a fast-path
//!   dispatch or overlapping shapes require typed trial matching;
//! * `allOf` merge reconciles exactly — a property declared with two different types is `E013`, and
//!   an otherwise-consistent merge keeps the union of every member's fields (no field loss) with the
//!   union of every member's `required`.
//!
//! Every run is also held to the shared `oracles`: each diagnostic names a real location (#454),
//! each generated union's variants are distinguishable by shape unless a warning says why or an
//! open issue tracks the gap (#402), and each case moved into a referenced file
//! (`oracles::relocate`) reaches the same verdict, codes and shapes (#446).

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use proptest::prelude::*;
use spargen::{CargoIntegration, Code, Outcome, Report, Spec};

mod oracles;

/// Run `check` (frontend + lowering, no emit) on an inline spec written into a throwaway tempdir.
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, spec).unwrap();
    check_at(&spec_path)
}

/// `check` on the root document at `root`, held to [`oracles::location_violations`].
fn check_at(root: &Utf8Path) -> Report {
    let report = spargen::check(&Spec::new(root));
    assert_located(&report, root);
    report
}

/// Run `generate` to a module and return the report plus the emitted source (when written).
fn generate_module(spec: &str) -> (Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, spec).unwrap();
    generate_at(&spec_path)
}

/// `generate` on the root document at `root`, writing the module beside it, held to
/// [`oracles::location_violations`] and, when it generates, to
/// [`oracles::indistinguishable_variants`]: a union's variants are told apart by shape, or a
/// warning says why, or an open issue tracks the gap (#492 for the equal nominal variants a
/// repeated closed-object key set lowers to).
fn generate_at(root: &Utf8Path) -> (Report, String) {
    let out = root.with_file_name("client.rs");
    let report = spargen::generate(
        &Spec::new(root)
            .build(out.clone())
            .cargo(CargoIntegration::Off),
    );
    assert_located(&report, root);
    let source = std::fs::read_to_string(&out).unwrap_or_default();
    if report.outcome() == Outcome::Generated {
        let unexplained = oracles::unexplained_variants(&report, &source);
        assert!(
            unexplained.is_empty(),
            "a union's variants cannot be told apart and no warning says why: {unexplained:#?}"
        );
    }
    (report, source)
}

/// Fail unless every diagnostic in `report` names a real location (#454).
fn assert_located(report: &Report, root: &Utf8Path) {
    let text = std::fs::read(root).unwrap_or_default();
    let unlocated = oracles::unknown(oracles::location_violations(report.diagnostics(), &text));
    assert!(
        unlocated.is_empty(),
        "diagnostics with no real location: {unlocated:#?}"
    );
}

/// The sorted diagnostic codes of a report, duplicates kept, so a `W001` count is compared too.
fn codes(report: &Report) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    codes.sort_unstable();
    codes
}

/// Moving `spec`'s schemas into a referenced file ([`oracles::relocate`]) changes nothing (#446):
/// `check` reaches the same verdict with the same code multiset, and when `inline` (the module
/// `spec` generated, `None` when the case only checks) was generated, `generate` gives every
/// schema the same [`oracles::shape`].
fn assert_relocation_changes_nothing(
    spec: &str,
    inline: Option<&(Report, String)>,
) -> Result<(), TestCaseError> {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let root = oracles::write_relocated(&dir, spec).expect("every case declares schemas");
    let inline_checked = check(spec);
    let moved_checked = check_at(&root);
    prop_assert_eq!(moved_checked.outcome(), inline_checked.outcome());
    prop_assert_eq!(codes(&moved_checked), codes(&inline_checked));
    if let Some((report, source)) = inline {
        let (moved, moved_source) = generate_at(&root);
        prop_assert_eq!(moved.outcome(), report.outcome());
        prop_assert_eq!(codes(&moved), codes(report));
        for schema in oracles::schema_names(spec) {
            prop_assert_eq!(
                oracles::shape(&moved_source, &schema),
                oracles::shape(source, &schema),
                "relocated, `{}` lowers to another shape",
                schema
            );
        }
    }
    Ok(())
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics().iter().any(|d| d.code == code)
}

// JSON-type-category disjointness is sound.

/// A JSON primitive category to place in a union variant. `Integer` and `Number` deliberately share
/// the numeric wire category — the lowering must never treat them as disjoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

// Required-key disjointness (closed objects) is sound.

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

// The allOf merge reconciles member constraints exactly.

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

/// Every property name declared with two different types across members. `string` and `integer`
/// share no value, so such a property can only be absent.
fn type_conflicts(members: &[Member]) -> BTreeSet<usize> {
    let mut seen: BTreeMap<usize, PropType> = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    for member in members {
        for (&name, (ty, _)) in member {
            match seen.get(&name) {
                Some(existing) if existing != ty => {
                    conflicts.insert(name);
                }
                _ => {
                    seen.insert(name, *ty);
                }
            }
        }
    }
    conflicts
}

/// The `(name, type)` of every field `pub struct Merged` declares, in source order — empty when no
/// such struct was emitted. Serde attribute lines between the fields are skipped.
fn merged_fields(source: &str) -> Vec<(String, String)> {
    let mut lines = source
        .lines()
        .map(str::trim_start)
        .skip_while(|line| *line != "pub struct Merged {");
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|rest| rest.split_once(':'))
        .map(|(name, ty)| (name.to_owned(), ty.trim().trim_end_matches(',').to_owned()))
        .collect()
}

/// `true` when a conflicting property is required by some member — the irreconcilable case
/// (`E013`): every instance must carry a value no type admits. A conflict on a property no member
/// requires leaves the objects that omit it valid, so it is typed uninhabited instead.
fn has_required_type_conflict(members: &[Member]) -> bool {
    let conflicts = type_conflicts(members);
    members.iter().any(|member| {
        member
            .iter()
            .any(|(name, (_, required))| *required && conflicts.contains(name))
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// Every JSON-category combination lowers to a typed enum. Pairwise-disjoint variants can use a
    /// direct dispatch fast path; `integer | number` uses typed trial matching. A repeated category
    /// lowers to the same generated type twice, which no value can tell apart, so it is one variant
    /// and the merge is reported; a union of one repeated category is that type, not an enum.
    #[test]
    fn json_category_unions_generate_typed(
        variants in proptest::collection::vec(category_strategy(), 2..=5)
    ) {
        let spec = category_union_spec(&variants);
        let generated = generate_module(&spec);
        let (report, source) = &generated;
        prop_assert_ne!(report.outcome(), Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(report, Code::NonDisjointUnion), "{:#?}", report);
        let distinct: BTreeSet<Category> = variants.iter().copied().collect();
        prop_assert_eq!(
            has_code(report, Code::ValidationKeywordIgnored),
            distinct.len() < variants.len(),
            "{:#?}",
            report
        );
        prop_assert_eq!(
            source.contains("pub enum U"),
            distinct.len() > 1,
            "union of {:?} emitted as the wrong shape:\n{}",
            variants,
            source
        );
        assert_relocation_changes_nothing(&spec, Some(&generated))?;
    }

    /// Every closed-object combination lowers to a typed enum. Unique required keys select a direct
    /// dispatch fast path; overlapping required-key sets use typed trial matching.
    #[test]
    fn closed_object_unions_generate_typed(
        variants in proptest::collection::vec(key_set_strategy(), 2..=4)
    ) {
        let spec = closed_object_union_spec(&variants);
        let generated = generate_module(&spec);
        let (report, source) = &generated;
        prop_assert_ne!(report.outcome(), Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(report, Code::NonDisjointUnion), "{:#?}", report);
        prop_assert!(source.contains("pub enum U"), "union was not emitted as a typed enum:\n{source}");
        assert_relocation_changes_nothing(&spec, Some(&generated))?;
    }

    /// A conflicting property type across members that some member requires is `E013`; otherwise the
    /// merge succeeds keeping the UNION of every member's fields (no field loss) and the UNION of
    /// every member's `required`, and an optional conflicting property is typed uninhabited.
    #[test]
    fn all_of_merge_reconciles(
        members in proptest::collection::vec(member_strategy(), 2..=3)
    ) {
        let spec = all_of_spec(&members);

        if has_required_type_conflict(&members) {
            let report = check(&spec);
            prop_assert_eq!(report.outcome(), Outcome::Rejected, "{:#?}", report);
            prop_assert!(has_code(&report, Code::AllOfIrreconcilable), "{:#?}", report);
            return assert_relocation_changes_nothing(&spec, None);
        }

        // No conflict: the merge must succeed and preserve every member's fields.
        let generated = generate_module(&spec);
        assert_relocation_changes_nothing(&spec, Some(&generated))?;
        let (report, source) = generated;
        prop_assert_ne!(report.outcome(), Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(&report, Code::AllOfIrreconcilable), "{:#?}", report);

        // Every oracle below reads `pub struct Merged` itself: a field line or an uninhabited type
        // found anywhere else in the module (a member's own struct, the embedded runtime) says
        // nothing about what the merge produced.
        let merged = merged_fields(&source);
        prop_assert!(!merged.is_empty(), "no `pub struct Merged` was emitted:\n{}", source);
        let conflicts = type_conflicts(&members);

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

        let expected: Vec<&str> = field_union.iter().map(|&name| KEYS[name]).collect();
        let mut declared: Vec<&str> = merged.iter().map(|(field, _)| field.as_str()).collect();
        declared.sort_unstable();
        prop_assert_eq!(
            &declared,
            &expected,
            "merged struct fields disagree with the union of member properties:\n{}",
            source
        );

        for &name in &field_union {
            let ident = KEYS[name];
            let ty = &merged.iter().find(|(field, _)| field == ident).unwrap().1;
            // A field required by ANY member must be plain; a field required by none is `Option`.
            let inner = ty.strip_prefix("Option<").and_then(|rest| rest.strip_suffix('>'));
            prop_assert_eq!(
                inner.is_some(),
                !required_union.contains(&name),
                "field `{}: {}` optionality disagrees with the required union:\n{}",
                ident,
                ty,
                source
            );
            // An optional conflict is an uninhabited type, never a silently widened or dropped
            // one; a property with no conflict is never uninhabited.
            let uninhabited = source.contains(&format!("pub enum {} {{}}", inner.unwrap_or(ty)));
            prop_assert_eq!(
                uninhabited,
                conflicts.contains(&name),
                "field `{}: {}` is uninhabited exactly when its members' types conflict:\n{}",
                ident,
                ty,
                source
            );
        }
    }
}
