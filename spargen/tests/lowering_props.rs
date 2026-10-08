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
//!   union of every member's `required`;
//! * the intersection laws (#474), read through the `syn`-based [`shape`] oracle: one conjunction
//!   lowers alike in each of its spellings, an `allOf`'s member order changes nothing, an object
//!   meet admits `null` exactly when its members do (and a nullable union refined to nothing but
//!   `null` is the null type, #450), and every written `default` is kept or reported at its
//!   pointer. Where today's output still splits a law, the gap is held exactly and names the open
//!   issue that tracks it (#541, #542, #545), so the law tightens when the issue's fix lands.
//!
//! Every run is also held to the shared `oracles`: each diagnostic names a real location (#454),
//! and each generated union's variants are distinguishable by shape unless a warning says why or an
//! open issue tracks the gap (#402). Each case of the first three properties moved into a
//! referenced file (`oracles::relocate`) also reaches the same verdict, codes and shapes (#446).

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
/// warning says why, or an open issue tracks the gap.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
    /// dispatch fast path; overlapping required-key sets use typed trial matching. A repeated key
    /// set is two inline structs of one structure, which no value can tell apart, so they are one
    /// variant and the merge is reported (#492); a union of one repeated key set is that struct,
    /// not an enum.
    #[test]
    fn closed_object_unions_generate_typed(
        variants in proptest::collection::vec(key_set_strategy(), 2..=4)
    ) {
        let spec = closed_object_union_spec(&variants);
        let generated = generate_module(&spec);
        let (report, source) = &generated;
        prop_assert_ne!(report.outcome(), Outcome::Rejected, "{:#?}", report);
        prop_assert!(!has_code(report, Code::NonDisjointUnion), "{:#?}", report);
        let distinct: BTreeSet<&BTreeSet<usize>> = variants.iter().collect();
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
        if distinct.len() == 1 {
            prop_assert!(source.contains("pub struct U"), "union of one key set is not that struct:\n{source}");
        }
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

// The intersection laws (#474): one conjunction lowers to one shape however it is spelled and in
// whatever order its members come, admits `null` exactly when its members do, and accounts for
// every `default` it was written with.

/// What an object member says about `null`: `type: object` denies it, `type: [object, 'null']`
/// admits it, and a member with no `type` admits it without deciding, as its object keywords bind
/// objects only (#425).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nullability {
    Object,
    ObjectOrNull,
    Untyped,
}

/// One property of an intersected object member.
#[derive(Clone, Copy, Debug)]
struct Prop {
    ty: PropType,
    required: bool,
    /// An index into [`DEFAULT_VALUES`]' row for `ty`: the `default` written on the property.
    default: Option<usize>,
}

/// The `default` values a [`Prop`] may be written with, a row per [`PropType`], each in its own
/// type, and each row in ascending order.
const DEFAULT_VALUES: [(PropType, [&str; 2]); 2] = [
    (PropType::String, ["x", "y"]),
    (PropType::Integer, ["1", "2"]),
];

/// A `default` as the intersection orders the candidates it keeps one of: a number before a string
/// (as JSON values sort), numbers by value, strings by text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DefaultValue {
    Integer(i64),
    String(&'static str),
}

impl DefaultValue {
    fn of(ty: PropType, index: usize) -> Self {
        let text = DEFAULT_VALUES
            .iter()
            .find(|(row, _)| *row == ty)
            .map(|(_, values)| values[index])
            .unwrap();
        match ty {
            PropType::String => DefaultValue::String(text),
            PropType::Integer => DefaultValue::Integer(text.parse().unwrap()),
        }
    }

    /// The value as YAML writes it, and as the rustdoc `Default: `…`.` line shows it.
    fn display(self) -> String {
        match self {
            DefaultValue::Integer(value) => value.to_string(),
            DefaultValue::String(text) => text.to_owned(),
        }
    }

    /// The value as JSON writes it, as the rustdoc `Default (not applied): `…`.` line shows it and
    /// as the wired serde default function's body spells it.
    fn raw(self) -> String {
        match self {
            DefaultValue::Integer(value) => value.to_string(),
            DefaultValue::String(text) => format!("\"{text}\""),
        }
    }
}

/// One object member of an intersection: property index into [`KEYS`] → property.
#[derive(Clone, Debug)]
struct Obj {
    nullability: Nullability,
    props: BTreeMap<usize, Prop>,
}

impl Obj {
    /// The member's keywords as the entries of a YAML flow mapping, without its braces, so they
    /// can stand as a member of their own or beside a `$ref`.
    fn keywords(&self) -> String {
        let mut keywords = Vec::new();
        match self.nullability {
            Nullability::Object => keywords.push("type: object".to_owned()),
            Nullability::ObjectOrNull => keywords.push("type: [object, 'null']".to_owned()),
            Nullability::Untyped => {}
        }
        let required: Vec<&str> = self
            .props
            .iter()
            .filter(|(_, prop)| prop.required)
            .map(|(&key, _)| KEYS[key])
            .collect();
        if !required.is_empty() {
            keywords.push(format!("required: [{}]", required.join(", ")));
        }
        let properties: Vec<String> = self
            .props
            .iter()
            .map(|(&key, prop)| {
                let ty = match prop.ty {
                    PropType::String => "string",
                    PropType::Integer => "integer",
                };
                match prop.default {
                    Some(index) => format!(
                        "{}: {{ type: {ty}, default: {} }}",
                        KEYS[key],
                        DefaultValue::of(prop.ty, index).display()
                    ),
                    None => format!("{}: {{ type: {ty} }}", KEYS[key]),
                }
            })
            .collect();
        keywords.push(format!("properties: {{ {} }}", properties.join(", ")));
        keywords.join(", ")
    }

    /// The member as a YAML flow mapping.
    fn inline(&self) -> String {
        format!("{{ {} }}", self.keywords())
    }
}

/// The property keys an intersection member draws from: fewer than [`KEYS`], so members repeat a
/// property often and the meet has a field to reconcile.
const MEET_KEYS: usize = 3;

fn prop_strategy() -> impl Strategy<Value = Prop> {
    (
        prop_oneof![Just(PropType::String), Just(PropType::Integer)],
        any::<bool>(),
        proptest::option::of(0usize..2),
    )
        .prop_map(|(ty, required, default)| Prop {
            ty,
            required,
            default,
        })
}

fn nullability_strategy() -> impl Strategy<Value = Nullability> {
    prop_oneof![
        Just(Nullability::Object),
        Just(Nullability::ObjectOrNull),
        Just(Nullability::Untyped),
    ]
}

fn obj_strategy() -> impl Strategy<Value = Obj> {
    (
        nullability_strategy(),
        proptest::collection::btree_map(0usize..MEET_KEYS, prop_strategy(), 1..=MEET_KEYS),
    )
        .prop_map(|(nullability, props)| Obj { nullability, props })
}

/// One way to write the conjunction of an intersection's members as the required property
/// `Holder.p`. Every spelling declares each member as the component `M{id}` too, whether or not it
/// refers to it, so the diagnostics the members' own lowering reports are the same in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// `allOf: [{ $ref: M0 }, { $ref: M1 }, …]`.
    AllOfRefs,
    /// `{ $ref: M0, <M1's keywords> }`: two members only.
    RefSiblings,
    /// `allOf: [<M0>, <M1>, …]`, each member written inline.
    InlineAllOf,
    /// `allOf: [{ $ref: M0 }, …]` beside `oneOf: [{}]`, a union whose one branch admits everything.
    AllOfBesideOneOf,
}

impl Spelling {
    const ALL: [Spelling; 4] = [
        Spelling::AllOfRefs,
        Spelling::RefSiblings,
        Spelling::InlineAllOf,
        Spelling::AllOfBesideOneOf,
    ];

    /// Whether the spelling can write a conjunction of `members` members.
    fn writes(self, members: usize) -> bool {
        self != Spelling::RefSiblings || members == 2
    }

    /// Whether the member at `position` is written as a `$ref` to its component.
    fn refers(self, position: usize) -> bool {
        match self {
            Spelling::AllOfRefs | Spelling::AllOfBesideOneOf => true,
            Spelling::RefSiblings => position == 0,
            Spelling::InlineAllOf => false,
        }
    }

    /// The schema of `Holder.p`, the members in the order given, each with the id of its component.
    fn schema(self, members: &[(usize, &Obj)]) -> String {
        let reference = |id: usize| format!("{{ $ref: '#/components/schemas/M{id}' }}");
        let refs: Vec<String> = members.iter().map(|&(id, _)| reference(id)).collect();
        match self {
            Spelling::AllOfRefs => format!("{{ allOf: [{}] }}", refs.join(", ")),
            Spelling::RefSiblings => format!(
                "{{ $ref: '#/components/schemas/M{}', {} }}",
                members[0].0,
                members[1].1.keywords()
            ),
            Spelling::InlineAllOf => {
                let inline: Vec<String> = members.iter().map(|(_, obj)| obj.inline()).collect();
                format!("{{ allOf: [{}] }}", inline.join(", "))
            }
            Spelling::AllOfBesideOneOf => {
                format!("{{ allOf: [{}], oneOf: [{{}}] }}", refs.join(", "))
            }
        }
    }

    /// The pointer of the schema whose keywords write the member at `position` (component `id`).
    fn member_pointer(self, position: usize, id: usize) -> String {
        if self.refers(position) {
            return format!("/components/schemas/M{id}");
        }
        match self {
            Spelling::RefSiblings => HOLDER_P.to_owned(),
            _ => format!("{HOLDER_P}/allOf/{position}"),
        }
    }
}

/// The pointer of the property every [`Spelling`] writes its conjunction as.
const HOLDER_P: &str = "/components/schemas/Holder/properties/p";

/// The document declaring each member (id = index into `members`) as a component and `Holder.p` as
/// their conjunction, written by `spelling` with the members in `order`.
fn intersection_spec(members: &[Obj], order: &[usize], spelling: Spelling) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n",
    );
    for (id, member) in members.iter().enumerate() {
        spec.push_str(&format!("    M{id}: {}\n", member.inline()));
    }
    let ordered: Vec<(usize, &Obj)> = order.iter().map(|&id| (id, &members[id])).collect();
    spec.push_str(&format!(
        "    Holder:\n      type: object\n      required: [p]\n      properties:\n        p: {}\n",
        spelling.schema(&ordered)
    ));
    spec
}

/// `pointer` with the location `spelling` wrote a member at (members in `order`) replaced by the
/// member's component name, so the same diagnostic about the same member reads the same whichever
/// position or spelling wrote it. A pointer into no member is returned unchanged.
fn member_normalised(pointer: &str, order: &[usize], spelling: Spelling) -> String {
    let mut best: Option<(usize, String)> = None;
    for (position, &id) in order.iter().enumerate() {
        let prefix = spelling.member_pointer(position, id);
        let Some(rest) = pointer.strip_prefix(&prefix) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with('/')) {
            continue;
        }
        if best.as_ref().is_none_or(|(len, _)| prefix.len() > *len) {
            best = Some((prefix.len(), format!("M{id}{rest}")));
        }
    }
    best.map_or_else(|| pointer.to_owned(), |(_, normalised)| normalised)
}

/// The deduplicated `(code, member-normalised pointer)` set of a report.
fn normalised_diagnostics(
    report: &Report,
    order: &[usize],
    spelling: Spelling,
) -> BTreeSet<(&'static str, String)> {
    report
        .diagnostics()
        .iter()
        .map(|d| {
            (
                d.code.as_str(),
                member_normalised(d.pointer.as_str(), order, spelling),
            )
        })
        .collect()
}

/// The sorted error codes of a report, duplicates kept.
fn error_codes(report: &Report) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = report.errors().map(|d| d.code.as_str()).collect();
    codes.sort_unstable();
    codes
}

/// The issue tracking a rejected `$ref`-sibling meet reporting fewer warnings than the `allOf`
/// spellings of the same conjunction: beside its `E013`, it stops before the `W005` they report.
const ISSUE_REJECTED_SIBLING_MEET_WARNS_LESS: u32 = 545;

/// Whether `check` or `generate` rejected the document. The two name a success differently
/// (`Clean` against `Generated`), so this is the verdict they must agree on.
fn rejected(report: &Report) -> bool {
    report.outcome() == Outcome::Rejected
}

/// The property keys some member requires with types the members disagree on: no value is both,
/// and every instance must carry one, so the meet is irreconcilable (`E013`).
fn meet_conflicts(members: &[Obj]) -> (BTreeSet<usize>, bool) {
    let mut types: BTreeMap<usize, BTreeSet<PropType>> = BTreeMap::new();
    for member in members {
        for (&key, prop) in &member.props {
            types.entry(key).or_default().insert(prop.ty);
        }
    }
    let conflicts: BTreeSet<usize> = types
        .into_iter()
        .filter(|(_, types)| types.len() > 1)
        .map(|(key, _)| key)
        .collect();
    let irreconcilable = members.iter().any(|member| {
        member
            .props
            .iter()
            .any(|(key, prop)| prop.required && conflicts.contains(key))
    });
    (conflicts, irreconcilable)
}

/// Whether the conjunction of `members` admits `null`: no member denies it, and some member admits
/// it of its own accord. Untyped members decide nothing, so a meet of them alone is the non-null
/// struct an untyped object schema lowers to by itself (#425).
fn meet_admits_null(members: &[Obj]) -> bool {
    !members.iter().any(|m| m.nullability == Nullability::Object)
        && members
            .iter()
            .any(|m| m.nullability == Nullability::ObjectOrNull)
}

/// The issue tracking a `$ref` to an untyped object component denying `null` in an object meet,
/// where the same member written inline admits it without deciding (#425's rule).
const ISSUE_REF_TO_UNTYPED_DENIES_NULL: u32 = 541;

/// The issue tracking an `allOf` of nullable object members whose object meet is irreconcilable
/// (`E013`), where the `$ref`-sibling spelling of the same conjunction lowers it to the null type:
/// no object satisfies every member, and `null` satisfies them all.
const ISSUE_NULL_ONLY_ALL_OF_REJECTED: u32 = 542;

/// Whether `Holder.p` is emitted nullable today: [`meet_admits_null`], except that a member written
/// as a `$ref` to its untyped component denies `null` (#541, [`ISSUE_REF_TO_UNTYPED_DENIES_NULL`]).
/// Where the two disagree the gap is the known one, and this is what the gap emits.
fn emitted_nullable(members: &[Obj], order: &[usize], spelling: Spelling) -> bool {
    let denies = order.iter().enumerate().any(|(position, &id)| {
        let nullability = members[id].nullability;
        nullability == Nullability::Object
            || (nullability == Nullability::Untyped && spelling.refers(position))
    });
    !denies && meet_admits_null(members)
}

/// What a conjunction lowers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lowered {
    /// No value satisfies every member: `E013`.
    Rejected,
    /// Only `null` does: the null type `()`.
    Null,
    /// An object struct, `Option`al exactly when `nullable`.
    Object { nullable: bool },
}

/// What `spelling` lowers the conjunction of `members` (in `order`) to today. An irreconcilable
/// object meet leaves `null` alone when every member admits it, so it is the null type; else it is
/// `E013`. Two known gaps differ from that: [`emitted_nullable`]'s (#541), and only the `$ref`-sibling
/// spelling finding the null type, when its sibling keywords admit `null` of their own accord
/// (`type: [object, 'null']`), the other spellings rejecting it (#542,
/// [`ISSUE_NULL_ONLY_ALL_OF_REJECTED`]).
fn emitted(members: &[Obj], order: &[usize], spelling: Spelling) -> Lowered {
    let nullable = emitted_nullable(members, order, spelling);
    if !meet_conflicts(members).1 {
        return Lowered::Object { nullable };
    }
    if nullable
        && spelling == Spelling::RefSiblings
        && members[order[1]].nullability == Nullability::ObjectOrNull
    {
        Lowered::Null
    } else {
        Lowered::Rejected
    }
}

/// What a run lowered `Holder.p` to, read from its report and module.
fn lowered(report: &Report, source: &str) -> Result<Lowered, TestCaseError> {
    if rejected(report) {
        return Ok(Lowered::Rejected);
    }
    let shape = holder_p_shape(source)?;
    Ok(if shape == "()" {
        Lowered::Null
    } else {
        Lowered::Object {
            nullable: shape.starts_with("Option<"),
        }
    })
}

/// What a written `default` became in the meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultFate {
    /// The merged field carries it: its rustdoc reads ``Default: `v`.``, and it is wired as a serde
    /// default exactly when the field is optional.
    Kept,
    /// Reported (`W005`) at the `default` that wrote it, and neither applied nor documented there:
    /// another member's different `default` is the one kept.
    Superseded,
    /// Reported (`W005`) at the `default` that wrote it: the field is uninhabited, so no value is
    /// one of its values, and its rustdoc reads ``Default (not applied): `v`.`` for the one kept.
    Uninhabited,
}

/// The reference oracle for the meet's `default`s: for property `key`, the value the merged field
/// keeps (`None` when no member writes one) and the fate of each member's `default`, by member id.
///
/// Of the different `default`s the members write, the one a member could apply as a serde default
/// (on a property that member does not require) is kept over one it could not, and then the least
/// value (#432). Every other value is superseded; a value equal to the kept one is the same
/// `default`. A field whose members' types share no value keeps none of them (#453).
fn default_oracle(
    members: &[Obj],
    key: usize,
    uninhabited: bool,
) -> (Option<DefaultValue>, BTreeMap<usize, DefaultFate>) {
    let written: Vec<(usize, DefaultValue, bool)> = members
        .iter()
        .enumerate()
        .filter_map(|(id, member)| {
            let prop = member.props.get(&key)?;
            let value = DefaultValue::of(prop.ty, prop.default?);
            Some((id, value, prop.required))
        })
        .collect();
    let kept = written
        .iter()
        .map(|&(_, value, required)| (required, value))
        .min()
        .map(|(_, value)| value);
    let fates = written
        .iter()
        .map(|&(id, value, _)| {
            let fate = if uninhabited {
                DefaultFate::Uninhabited
            } else if Some(value) == kept {
                DefaultFate::Kept
            } else {
                DefaultFate::Superseded
            };
            (id, fate)
        })
        .collect();
    (kept, fates)
}

/// The shape oracle over a generated module, read with `syn` rather than line by line, so a type is
/// what the parser says it is however `prettyplease` wraps it.
mod shape {
    use std::collections::BTreeMap;

    use quote::ToTokens;

    /// The item declarations of a generated `types` module, keyed by name.
    pub struct Types {
        items: BTreeMap<String, syn::Item>,
    }

    /// What a struct field is: its wire name and type, its `serde` attributes other than its rename
    /// (a `default` function by its body, not its name), and its rustdoc lines.
    pub struct Field {
        pub wire: String,
        pub ty: syn::Type,
        pub serde: Vec<String>,
        pub docs: Vec<String>,
    }

    impl Types {
        /// The `types` module of the generated module `code`; empty when there is none.
        pub fn read(code: &str) -> Self {
            let mut items = BTreeMap::new();
            let file = syn::parse_file(code).unwrap_or_else(|error| {
                panic!("the generated module does not parse: {error}\n{code}")
            });
            let module = file.items.into_iter().find_map(|item| match item {
                syn::Item::Mod(module) if module.ident == "types" => module.content,
                _ => None,
            });
            for item in module.map(|(_, items)| items).unwrap_or_default() {
                let name = match &item {
                    syn::Item::Type(alias) => alias.ident.to_string(),
                    syn::Item::Struct(item) => item.ident.to_string(),
                    syn::Item::Enum(item) => item.ident.to_string(),
                    syn::Item::Fn(function) => function.sig.ident.to_string(),
                    _ => continue,
                };
                items.insert(name, item);
            }
            Self { items }
        }

        /// The fields of `pub struct name`, or `None` when the module declares no such struct.
        pub fn fields(&self, name: &str) -> Option<Vec<Field>> {
            let Some(syn::Item::Struct(item)) = self.items.get(name) else {
                return None;
            };
            Some(item.fields.iter().map(|field| self.field(field)).collect())
        }

        /// The type of the field `wire` of `pub struct name`.
        pub fn field_type(&self, name: &str, wire: &str) -> Option<syn::Type> {
            self.fields(name)?
                .into_iter()
                .find(|field| field.wire == wire)
                .map(|field| field.ty)
        }

        /// The struct `ty` names once its aliases and `Option`s are expanded, if it names one.
        pub fn struct_of(&self, ty: &syn::Type, depth: usize) -> Option<String> {
            let syn::Type::Path(path) = ty else {
                return None;
            };
            let segment = path.path.segments.last()?;
            if segment.ident == "Option" || segment.ident == "Box" {
                let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
                    return None;
                };
                let syn::GenericArgument::Type(inner) = args.args.first()? else {
                    return None;
                };
                return self.struct_of(inner, depth.checked_sub(1)?);
            }
            if path.path.segments.len() != 1 {
                return None;
            }
            let name = segment.ident.to_string();
            match self.items.get(&name)? {
                syn::Item::Type(alias) => self.struct_of(&alias.ty, depth.checked_sub(1)?),
                syn::Item::Struct(_) => Some(name),
                _ => None,
            }
        }

        /// The body of the function `name`, as tokens.
        pub fn function_body(&self, name: &str) -> Option<String> {
            match self.items.get(name)? {
                syn::Item::Fn(function) => Some(function.block.to_token_stream().to_string()),
                _ => None,
            }
        }

        fn field(&self, field: &syn::Field) -> Field {
            let mut wire = field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let mut serde = Vec::new();
            let mut docs = Vec::new();
            for attribute in &field.attrs {
                if attribute.path().is_ident("doc") {
                    if let syn::Meta::NameValue(syn::MetaNameValue {
                        value:
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(text),
                                ..
                            }),
                        ..
                    }) = &attribute.meta
                    {
                        docs.push(text.value().trim().to_owned());
                    }
                    continue;
                }
                if !attribute.path().is_ident("serde") {
                    serde.push(attribute.to_token_stream().to_string());
                    continue;
                }
                attribute
                    .parse_nested_meta(|meta| {
                        let key = meta.path.to_token_stream().to_string();
                        if !meta.input.peek(syn::Token![=]) {
                            serde.push(key);
                            return Ok(());
                        }
                        let value: syn::LitStr = meta.value()?.parse()?;
                        match key.as_str() {
                            "rename" => wire = value.value(),
                            "default" => serde.push(format!(
                                "default = {}",
                                self.function_body(&value.value())
                                    .unwrap_or_else(|| value.value())
                            )),
                            _ => serde.push(format!("{key} = {}", value.value())),
                        }
                        Ok(())
                    })
                    .unwrap_or_else(|error| panic!("unreadable serde attribute: {error}"));
            }
            Field {
                wire,
                ty: field.ty.clone(),
                serde,
                docs,
            }
        }

        /// The shape of `ty`: what it stands for with every name the module declares expanded, so
        /// two modules that lower one schema under different names agree. An alias is its target;
        /// a struct is its fields sorted by wire name, each with its type's shape, its `serde`
        /// attributes and the rustdoc lines that state its `default`; an enum is its variants'
        /// payload shapes, sorted (a unit variant by its wire name). A path of more than one segment
        /// (`serde_json::Value`) is never a declared name. Expansion stops at `depth`, so a
        /// recursive type ends.
        pub fn shape(&self, ty: &syn::Type, depth: usize) -> String {
            match ty {
                syn::Type::Path(path) if path.qself.is_none() => {
                    let segments = &path.path.segments;
                    if segments.len() == 1 && segments[0].arguments.is_none() && depth > 0 {
                        if let Some(shape) = self.expand(&segments[0].ident.to_string(), depth) {
                            return shape;
                        }
                    }
                    let rendered: Vec<String> = segments
                        .iter()
                        .map(|segment| match &segment.arguments {
                            syn::PathArguments::AngleBracketed(args) => {
                                let args: Vec<String> = args
                                    .args
                                    .iter()
                                    .map(|arg| match arg {
                                        syn::GenericArgument::Type(ty) => self.shape(ty, depth),
                                        other => other.to_token_stream().to_string(),
                                    })
                                    .collect();
                                format!("{}<{}>", segment.ident, args.join(", "))
                            }
                            _ => segment.ident.to_string(),
                        })
                        .collect();
                    rendered.join("::")
                }
                syn::Type::Tuple(tuple) => {
                    let elements: Vec<String> =
                        tuple.elems.iter().map(|ty| self.shape(ty, depth)).collect();
                    format!("({})", elements.join(", "))
                }
                other => other.to_token_stream().to_string(),
            }
        }

        /// [`Self::shape`] of the declared name `name`, or `None` when nothing declares it.
        fn expand(&self, name: &str, depth: usize) -> Option<String> {
            let depth = depth - 1;
            Some(match self.items.get(name)? {
                syn::Item::Type(alias) => self.shape(&alias.ty, depth),
                syn::Item::Struct(_) => {
                    let mut fields: Vec<String> = self
                        .fields(name)?
                        .into_iter()
                        .map(|field| {
                            let docs: Vec<&String> = field
                                .docs
                                .iter()
                                .filter(|line| line.starts_with("Default"))
                                .collect();
                            format!(
                                "{}: {} {:?} {:?}",
                                field.wire,
                                self.shape(&field.ty, depth),
                                field.serde,
                                docs
                            )
                        })
                        .collect();
                    fields.sort();
                    format!("struct {{ {} }}", fields.join("; "))
                }
                syn::Item::Enum(item) => {
                    let mut variants: Vec<String> = item
                        .variants
                        .iter()
                        .map(|variant| match &variant.fields {
                            syn::Fields::Unit => {
                                let mut wire = variant.ident.to_string();
                                for attribute in &variant.attrs {
                                    if attribute.path().is_ident("serde") {
                                        let _ = attribute.parse_nested_meta(|meta| {
                                            if meta.path.is_ident("rename") {
                                                let value: syn::LitStr = meta.value()?.parse()?;
                                                wire = value.value();
                                            }
                                            Ok(())
                                        });
                                    }
                                }
                                format!("{wire:?}")
                            }
                            fields => {
                                let payload: Vec<String> = fields
                                    .iter()
                                    .map(|field| self.shape(&field.ty, depth))
                                    .collect();
                                format!("({})", payload.join(", "))
                            }
                        })
                        .collect();
                    variants.sort();
                    format!("enum {{ {} }}", variants.join(", "))
                }
                // A `default` function is read through the field naming it, never as a type.
                _ => return None,
            })
        }
    }

    /// The shape of the field `wire` of `pub struct name` in the generated module `code`, `None`
    /// when no such field was emitted.
    pub fn field(code: &str, name: &str, wire: &str) -> Option<String> {
        let types = Types::read(code);
        let ty = types.field_type(name, wire)?;
        Some(types.shape(&ty, 8))
    }
}

/// The shape of `Holder.p` in `source`, which must have been emitted.
fn holder_p_shape(source: &str) -> Result<String, TestCaseError> {
    shape::field(source, "Holder", "p")
        .ok_or_else(|| TestCaseError::fail(format!("`Holder.p` was not emitted:\n{source}")))
}

/// [`holder_p_shape`] without the `Option` that makes it nullable, so two shapes that differ only
/// in admitting `null` compare equal.
fn non_null(shape: &str) -> &str {
    shape
        .strip_prefix("Option<")
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or(shape)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// One conjunction of two members lowers to one shape however it is written: as `allOf` of
    /// `$ref`s, as a `$ref` with the other member's keywords beside it, as an inline `allOf`, and as
    /// an `allOf` beside `oneOf: [{}]`. Each spelling reaches the same verdict through `check` and
    /// `generate`, with the same code multiset, and gives `Holder.p` the same shape. Each spelling
    /// lowers to what [`emitted`] says, and two known gaps may still split them: a member's `$ref`
    /// to its untyped component deciding `null` differently (#541), and the null type only the
    /// `$ref`-sibling spelling finds (#542). Spellings the gaps split are compared with neither.
    #[test]
    fn every_spelling_of_one_conjunction_lowers_alike(
        members in proptest::collection::vec(obj_strategy(), 2)
    ) {
        let order = [0, 1];
        let mut seen: Vec<(Spelling, Lowered, Report, Option<String>)> = Vec::new();
        for spelling in Spelling::ALL {
            let spec = intersection_spec(&members, &order, spelling);
            let (report, source) = generate_module(&spec);
            let checked = check(&spec);
            prop_assert_eq!(rejected(&checked), rejected(&report), "{:?}: {}", spelling, spec);
            prop_assert_eq!(codes(&checked), codes(&report), "{:?}: {}", spelling, spec);
            let lowered = lowered(&report, &source)?;
            prop_assert_eq!(
                lowered,
                emitted(&members, &order, spelling),
                "{:?}: {:#?}\n{}\n(known gaps: #{}, #{})",
                spelling,
                report,
                spec,
                ISSUE_REF_TO_UNTYPED_DENIES_NULL,
                ISSUE_NULL_ONLY_ALL_OF_REJECTED
            );
            let shape = (!rejected(&report)).then(|| holder_p_shape(&source)).transpose()?;
            for (at, lowered_at, report_at, shape_at) in &seen {
                // Only a nullability gap may split two object spellings, so they still agree on the
                // rest of the shape; a split verdict leaves nothing to compare.
                let same_kind = match (lowered, *lowered_at) {
                    (Lowered::Object { .. }, Lowered::Object { .. }) => true,
                    (kind, kind_at) => kind == kind_at,
                };
                if !same_kind {
                    continue;
                }
                let (compared, compared_at) = if lowered == Lowered::Rejected
                    && (spelling == Spelling::RefSiblings || *at == Spelling::RefSiblings)
                {
                    (error_codes(&report), error_codes(report_at))
                } else {
                    (codes(&report), codes(report_at))
                };
                prop_assert_eq!(
                    &compared,
                    &compared_at,
                    "{:?} and {:?} (known gap #{} compares only errors):\n{}",
                    spelling,
                    at,
                    ISSUE_REJECTED_SIBLING_MEET_WARNS_LESS,
                    spec
                );
                if let (Some(shape), Some(shape_at)) = (&shape, shape_at) {
                    prop_assert_eq!(
                        non_null(shape),
                        non_null(shape_at),
                        "{:?} and {:?} lower `Holder.p` to different shapes:\n{}",
                        spelling,
                        at,
                        spec
                    );
                }
            }
            seen.push((spelling, lowered, report, shape));
        }
    }

    /// The order of an `allOf`'s members changes nothing: the same shape for `Holder.p`, the same
    /// `W005` presence, and the same diagnostics about the same members, in each spelling that
    /// writes the members in order.
    #[test]
    fn all_of_member_order_changes_nothing(
        (members, order) in proptest::collection::vec(obj_strategy(), 2..=3).prop_flat_map(|members| {
            let ids: Vec<usize> = (0..members.len()).collect();
            (Just(members), Just(ids).prop_shuffle())
        })
    ) {
        let identity: Vec<usize> = (0..members.len()).collect();
        for spelling in [Spelling::AllOfRefs, Spelling::InlineAllOf, Spelling::AllOfBesideOneOf] {
            let in_order = intersection_spec(&members, &identity, spelling);
            let shuffled = intersection_spec(&members, &order, spelling);
            let (report, source) = generate_module(&in_order);
            let (moved, moved_source) = generate_module(&shuffled);
            prop_assert_eq!(moved.outcome(), report.outcome(), "{:?}:\n{}\n{}", spelling, in_order, shuffled);
            prop_assert_eq!(
                has_code(&moved, Code::SchemaDefaultNotApplied),
                has_code(&report, Code::SchemaDefaultNotApplied),
                "{:?}:\n{}\n{}",
                spelling,
                in_order,
                shuffled
            );
            prop_assert_eq!(
                normalised_diagnostics(&moved, &order, spelling),
                normalised_diagnostics(&report, &identity, spelling),
                "{:?}:\n{}\n{}",
                spelling,
                in_order,
                shuffled
            );
            if report.outcome() != Outcome::Rejected {
                prop_assert_eq!(
                    holder_p_shape(&moved_source)?,
                    holder_p_shape(&source)?,
                    "{:?}: reordering the members changed `Holder.p`:\n{}\n{}",
                    spelling,
                    in_order,
                    shuffled
                );
            }
        }
    }

    /// An object meet admits `null` exactly when every member does and one of them decides it
    /// ([`meet_admits_null`]), in every spelling, for every order of its members. Where a member is
    /// a `$ref` to its untyped component the known gap [`emitted_nullable`] names is what is held.
    #[test]
    fn an_object_meet_admits_null_exactly_when_its_members_do(
        (members, order) in proptest::collection::vec(
            (nullability_strategy(), proptest::collection::btree_set(0usize..MEET_KEYS, 1..=MEET_KEYS))
                .prop_map(|(nullability, keys)| Obj {
                    nullability,
                    props: keys
                        .into_iter()
                        .map(|key| (key, Prop { ty: PropType::String, required: false, default: None }))
                        .collect(),
                }),
            2..=3,
        )
        .prop_flat_map(|members| {
            let ids: Vec<usize> = (0..members.len()).collect();
            (Just(members), Just(ids).prop_shuffle())
        })
    ) {
        for spelling in Spelling::ALL.into_iter().filter(|s| s.writes(members.len())) {
            let spec = intersection_spec(&members, &order, spelling);
            let (report, source) = generate_module(&spec);
            prop_assert_eq!(report.outcome(), Outcome::Generated, "{:?}: {:#?}\n{}", spelling, report, spec);
            let shape = holder_p_shape(&source)?;
            let expected = emitted_nullable(&members, &order, spelling);
            prop_assert_eq!(
                shape.starts_with("Option<"),
                expected,
                "{:?}: `Holder.p` is `{}`, but `null` is {} here (meet rule: {}, known gap #{}):\n{}",
                spelling,
                shape,
                if expected { "valid" } else { "invalid" },
                meet_admits_null(&members),
                ISSUE_REF_TO_UNTYPED_DENIES_NULL,
                spec
            );
        }
    }

    /// A nullable union of objects (#450) refined by untyped object keywords: where the refiner
    /// excludes every object branch only `null` is left, and the schema is the null type `()`,
    /// never `E013`; without the `null` branch nothing is left, which is a rejection; a refiner
    /// that excludes no branch keeps the union. Each holds in every spelling of the refinement.
    #[test]
    fn a_refined_nullable_union_keeps_exactly_what_the_refiner_admits(
        refined in prop_oneof![Just("integer"), Just("boolean"), Just("string")],
        nullable in any::<bool>(),
        spelling in 0usize..3,
    ) {
        let union = if nullable {
            "[{ $ref: '#/components/schemas/Cat' }, { $ref: '#/components/schemas/Dog' }, { type: 'null' }]"
        } else {
            "[{ $ref: '#/components/schemas/Cat' }, { $ref: '#/components/schemas/Dog' }]"
        };
        let refiner = format!("properties: {{ kind: {{ type: {refined} }} }}");
        let pet = match spelling {
            0 => format!("    Pet:\n      allOf: [{{ {refiner} }}]\n      oneOf: {union}\n"),
            1 => format!("    U:\n      oneOf: {union}\n    Pet:\n      $ref: '#/components/schemas/U'\n      {refiner}\n"),
            _ => format!("    Pet:\n      {refiner}\n      oneOf: {union}\n"),
        };
        let spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  schemas:\n    \
             Cat:\n      type: object\n      required: [kind]\n      properties: {{ kind: {{ type: string }} }}\n    \
             Dog:\n      type: object\n      required: [kind, bark]\n      properties: {{ kind: {{ type: string }}, bark: {{ type: boolean }} }}\n\
             {pet}    Holder:\n      type: object\n      required: [p]\n      properties:\n        p: {{ $ref: '#/components/schemas/Pet' }}\n"
        );
        let excluded = refined != "string";
        let (report, source) = generate_module(&spec);
        let checked = check(&spec);
        prop_assert_eq!(rejected(&checked), rejected(&report), "{}", spec);
        if excluded && !nullable {
            // Which code says so is the spelling's: `E013` beside an `allOf`, `E007` for the
            // union's own siblings.
            prop_assert_eq!(report.outcome(), Outcome::Rejected, "{:#?}\n{}", report, spec);
            return Ok(());
        }
        prop_assert_ne!(report.outcome(), Outcome::Rejected, "{:#?}\n{}", report, spec);
        prop_assert!(!has_code(&report, Code::AllOfIrreconcilable), "{:#?}\n{}", report, spec);
        let shape = holder_p_shape(&source)?;
        prop_assert_eq!(shape == "()", excluded, "`Holder.p` is `{}`:\n{}", shape, spec);
    }

    /// Every `default` a member writes on a property is accounted for by the meet, exactly as
    /// [`default_oracle`] says: kept (the merged field's rustdoc reads ``Default: `v`.``, wired as a
    /// serde default exactly when the field is optional), or reported (`W005`) at exactly the
    /// pointer that wrote it, and then documented as not applied when the field is uninhabited and
    /// not at all when another member's `default` was kept. No other `default` pointer is reported.
    #[test]
    fn every_written_default_is_kept_or_reported(
        (members, order) in proptest::collection::vec(obj_strategy(), 2..=3).prop_flat_map(|members| {
            let ids: Vec<usize> = (0..members.len()).collect();
            (Just(members), Just(ids).prop_shuffle())
        })
    ) {
        let (conflicts, _) = meet_conflicts(&members);
        for spelling in Spelling::ALL.into_iter().filter(|s| s.writes(members.len())) {
            let spec = intersection_spec(&members, &order, spelling);
            let (report, source) = generate_module(&spec);
            let expected = emitted(&members, &order, spelling);
            prop_assert_eq!(lowered(&report, &source)?, expected, "{:?}: {:#?}\n{}", spelling, report, spec);
            match expected {
                Lowered::Rejected => {
                    prop_assert!(has_code(&report, Code::AllOfIrreconcilable), "{:?}: {:#?}\n{}", spelling, report, spec);
                    continue;
                }
                // The null type has no field to carry a `default`; what its members' `default`s
                // report is #542's to settle, as only the gap's spelling reaches it today.
                Lowered::Null => continue,
                Lowered::Object { .. } => {}
            }

            let reported: BTreeSet<String> = normalised_diagnostics(&report, &order, spelling)
                .into_iter()
                .filter(|(code, _)| *code == Code::SchemaDefaultNotApplied.as_str())
                .map(|(_, pointer)| pointer)
                .collect();
            let types = shape::Types::read(&source);
            let holder_p = types.field_type("Holder", "p");
            let meet = holder_p.as_ref().and_then(|ty| types.struct_of(ty, 8));
            let meet = meet.ok_or_else(|| TestCaseError::fail(format!("{spelling:?}: no merged struct:\n{source}")))?;
            let fields = types.fields(&meet).unwrap();

            let mut expected_reported = BTreeSet::new();
            let keys: BTreeSet<usize> = members.iter().flat_map(|m| m.props.keys().copied()).collect();
            for key in keys {
                let uninhabited = conflicts.contains(&key);
                let (kept, fates) = default_oracle(&members, key, uninhabited);
                for (&id, fate) in &fates {
                    if *fate != DefaultFate::Kept {
                        expected_reported.insert(format!("M{id}/properties/{}/default", KEYS[key]));
                    }
                }
                let field = fields
                    .iter()
                    .find(|field| field.wire == KEYS[key])
                    .ok_or_else(|| TestCaseError::fail(format!("{spelling:?}: no `{}` field in `{meet}`:\n{source}", KEYS[key])))?;
                let notes: Vec<&String> = field.docs.iter().filter(|line| line.starts_with("Default")).collect();
                let wired = field.serde.iter().find(|attr| attr.starts_with("default = "));
                let optional = !members.iter().any(|m| m.props.get(&key).is_some_and(|p| p.required));
                let expected_note = kept.map(|value| {
                    if uninhabited {
                        format!("Default (not applied): `{}`.", value.raw())
                    } else {
                        format!("Default: `{}`.", value.display())
                    }
                });
                prop_assert_eq!(
                    notes,
                    expected_note.iter().collect::<Vec<_>>(),
                    "{:?}: `{}.{}` documents the wrong default:\n{}\n{}",
                    spelling, meet, KEYS[key], spec, source
                );
                let expected_wired = kept.filter(|_| optional && !uninhabited);
                prop_assert_eq!(
                    wired.is_some(),
                    expected_wired.is_some(),
                    "{:?}: `{}.{}` wires {:?}, but the oracle keeps {:?}:\n{}\n{}",
                    spelling, meet, KEYS[key], wired, expected_wired, spec, source
                );
                if let (Some(wired), Some(value)) = (wired, expected_wired) {
                    let literal = value.raw();
                    prop_assert!(
                        wired.contains(&format!("Some ({literal}")),
                        "{:?}: `{}.{}` wires `{}`, not {}:\n{}",
                        spelling, meet, KEYS[key], wired, literal, spec
                    );
                }
            }
            prop_assert_eq!(
                reported,
                expected_reported,
                "{:?}: the reported `default`s disagree with the oracle:\n{}\n{:#?}",
                spelling, spec, report
            );
        }
    }
}
