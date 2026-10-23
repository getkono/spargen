//! One `null` decision across every spelling of one constraint set (#674): a `type` (with its own
//! `enum` / `const`) and a `oneOf` / `anyOf`, written as siblings, as `allOf` members in either
//! order, nested in an inner `allOf`, or as `$ref`'d components, give one generated nullability,
//! one set of `()` and `Option` arms, and one set of excluded-member `W011`s; and the cases the
//! decision settles (#666, #668, #670, #671, #673, #675, #676) are fixed rows of it.

use super::*;

/// The `type` (and its own `enum` / `const`) one constraint set holds the union to, a `type` array
/// of two non-null categories included (#677), and one naming `object` or `array` beside a scalar
/// category (#827).
const NULL_CONJUNCTS: [&str; 12] = [
    "type: [string, 'null']",
    "type: [integer, 'null']",
    "type: string",
    "type: [string, integer]",
    "type: [string, integer, 'null']",
    "type: [object, string]",
    "type: [array, integer, 'null']",
    "type: [string, 'null'], enum: [a]",
    "type: [string, 'null'], enum: [a, null]",
    "type: [string, 'null'], const: a",
    "enum: [a, null]",
    "const: null",
];

/// The branches a constraint set's union draws from: stated nothing, exact null (inline and as a
/// `$ref` to `Z`), single- and multi-category `type` arrays with and without `null`, scalar
/// keywords alone, untyped `items` / `properties` shapes, a typed one-value enum, and a nested
/// union `null` matches through an untyped branch, inline and as a `$ref` to `U`
/// ([`NULL_COMPONENTS`]).
const NULL_BRANCHES: [&str; 20] = [
    "{}",
    "true",
    "{ type: 'null' }",
    "{ const: null }",
    "{ enum: [null] }",
    "{ type: string }",
    "{ type: integer }",
    "{ type: [string, 'null'] }",
    "{ type: [integer, 'null'] }",
    "{ type: [string, integer] }",
    "{ minLength: 1 }",
    "{ pattern: '^a' }",
    "{ minimum: 1 }",
    "{ not: { type: boolean } }",
    "{ items: { type: string } }",
    "{ properties: { a: { type: string } } }",
    "{ type: string, enum: [a] }",
    "{ $ref: '#/components/schemas/U' }",
    "{ $ref: '#/components/schemas/Z' }",
    "{ oneOf: [ { items: { type: string } }, { type: boolean } ] }",
];

/// The components [`NULL_BRANCHES`] names, beside the `Holder` whose required `pick` names `Pick`.
const NULL_COMPONENTS: &str = "    U: { oneOf: [ { items: { type: string } }, { type: boolean } ] }
    Z: { const: null }
    Holder: { type: object, properties: { pick: { $ref: '#/components/schemas/Pick' } }, required: [pick] }
";

/// One constraint set: a conjunct ([`NULL_CONJUNCTS`]) and a `oneOf` or `anyOf` of branches.
#[derive(Debug, Clone)]
struct NullCase {
    conjunct: &'static str,
    one_of: bool,
    branches: Vec<&'static str>,
}

/// The ways one [`NullCase`] is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NullSpelling {
    /// `{ <conjunct>, oneOf: M }`.
    Sibling,
    /// `{ allOf: [ { <conjunct> }, { oneOf: M } ] }`.
    UnionSecond,
    /// `{ allOf: [ { oneOf: M }, { <conjunct> } ] }`.
    UnionFirst,
    /// `{ allOf: [ { <conjunct> }, { allOf: [ { oneOf: M } ] } ] }`.
    Nested,
    /// `{ allOf: [ { $ref: C }, { $ref: Un } ] }`, the members as components.
    RefMembers,
}

impl NullSpelling {
    const ALL: [NullSpelling; 5] = [
        NullSpelling::Sibling,
        NullSpelling::UnionSecond,
        NullSpelling::UnionFirst,
        NullSpelling::Nested,
        NullSpelling::RefMembers,
    ];
}

impl NullCase {
    fn union(&self) -> String {
        let keyword = if self.one_of { "oneOf" } else { "anyOf" };
        format!("{keyword}: [ {} ]", self.branches.join(", "))
    }

    fn spec(&self, spelling: NullSpelling) -> String {
        let (conjunct, union) = (self.conjunct, self.union());
        let beside = if conjunct.is_empty() {
            String::new()
        } else {
            format!("{conjunct}, ")
        };
        let (pick, extra) = match spelling {
            NullSpelling::Sibling => (format!("{{ {beside}{union} }}"), String::new()),
            NullSpelling::UnionSecond => (
                format!("{{ allOf: [ {{ {conjunct} }}, {{ {union} }} ] }}"),
                String::new(),
            ),
            NullSpelling::UnionFirst => (
                format!("{{ allOf: [ {{ {union} }}, {{ {conjunct} }} ] }}"),
                String::new(),
            ),
            NullSpelling::Nested => (
                format!("{{ allOf: [ {{ {conjunct} }}, {{ allOf: [ {{ {union} }} ] }} ] }}"),
                String::new(),
            ),
            NullSpelling::RefMembers => (
                "{ allOf: [ { $ref: '#/components/schemas/C' }, { $ref: \
                 '#/components/schemas/Un' } ] }"
                    .to_owned(),
                format!("    C: {{ {conjunct} }}\n    Un: {{ {union} }}\n"),
            ),
        };
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    Pick: {pick}\n{extra}{NULL_COMPONENTS}"
        )
    }
}

/// What one spelling's output says about `null`: whether it generated, the `pick` field's
/// `Option`-ness, how many of `Pick`'s arms are the exact null type `()` and how many an `Option`,
/// and the `W011` messages about excluded union members.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NullAnswer {
    generated: bool,
    optional: bool,
    unit_arms: usize,
    option_arms: usize,
    excluded: Vec<String>,
}

fn null_answer(report: &Report, source: &str) -> NullAnswer {
    let mut excluded: Vec<String> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == Code::DeclarationHasNoEffect
                && diagnostic.message.starts_with("union member ")
        })
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    excluded.sort();
    let generated = !rejected(report);
    // A rejected schema has no generated enum for a branch to be missing from.
    if !generated {
        excluded.clear();
    }
    let field = source
        .lines()
        .find_map(|line| line.trim().strip_prefix("pub pick: "))
        .unwrap_or_default();
    let units: BTreeSet<&str> = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub type ")?
                .strip_suffix(" = ();")
        })
        .collect();
    let arms: Vec<&str> = source
        .split_once("pub enum Pick {")
        .and_then(|(_, body)| body.split_once("\n    }"))
        .map(|(body, _)| {
            body.lines()
                .filter_map(|line| line.trim().split_once('(').map(|(_, payload)| payload))
                .collect()
        })
        .unwrap_or_default();
    let is_unit = |payload: &str| {
        payload.starts_with("())")
            || payload
                .strip_prefix("Box<")
                .and_then(|inner| inner.split_once('>'))
                .is_some_and(|(name, _)| units.contains(name))
    };
    NullAnswer {
        generated,
        optional: generated && field.starts_with("Option<"),
        unit_arms: arms.iter().filter(|payload| is_unit(payload)).count(),
        option_arms: arms
            .iter()
            .filter(|payload| payload.starts_with("Option<"))
            .count(),
        excluded,
    }
}

/// Every spelling of `case` gives one [`NullAnswer`], through `generate` and `check` alike; the
/// answer is returned.
fn assert_null_parity(case: &NullCase) -> Result<NullAnswer, TestCaseError> {
    let mut seen: Option<(NullSpelling, NullAnswer, String)> = None;
    for spelling in NullSpelling::ALL {
        let spec = case.spec(spelling);
        let (report, source) = generate_module(&spec);
        let checked = check(&spec);
        prop_assert_eq!(
            rejected(&checked),
            rejected(&report),
            "{:?}: {}",
            spelling,
            spec
        );
        prop_assert_eq!(codes(&checked), codes(&report), "{:?}: {}", spelling, spec);
        let answer = null_answer(&report, &source);
        // `null` is carried once: a `()` arm, or the `Option` of the field naming `Pick`.
        prop_assert!(
            !(answer.optional && answer.unit_arms > 0),
            "{:?}: `Option<Pick>` beside a `()` arm: {}\n{}",
            spelling,
            spec,
            source
        );
        if let Some((at, first, first_spec)) = &seen {
            prop_assert_eq!(
                &answer,
                first,
                "{:?} and {:?} disagree about `null`:\n{}\n{}\n{:#?}\n{}",
                spelling,
                at,
                spec,
                first_spec,
                report,
                source
            );
        } else {
            seen = Some((spelling, answer, spec));
        }
    }
    Ok(seen.expect("a spelling ran").1)
}

fn null_case_strategy() -> impl Strategy<Value = NullCase> {
    (
        proptest::sample::select(NULL_CONJUNCTS.to_vec()),
        any::<bool>(),
        proptest::collection::vec(proptest::sample::select(NULL_BRANCHES.to_vec()), 1..=3),
    )
        .prop_map(|(conjunct, one_of, branches)| NullCase {
            conjunct,
            one_of,
            branches,
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// One constraint set (#674) — a conjunct's `type`, `enum` and `const` and a `oneOf` or
    /// `anyOf` of branches — decides `null` once, however it is written: the conjunct beside the
    /// union, the union as an `allOf` member after the conjunct or before it, the union nested in
    /// an inner `allOf`, and both as `$ref`'d components. Every spelling gives the same verdict,
    /// the same `Option`-ness of the field naming it, the same `()` and `Option` arms, and the same
    /// excluded-member `W011`s, through `generate` and `check` alike.
    #[test]
    fn every_spelling_of_one_constraint_set_decides_null_alike(case in null_case_strategy()) {
        assert_null_parity(&case)?;
    }
}

/// The cases #674 supersedes, as fixed rows of [`assert_null_parity`]: each constraint set gives
/// one answer in every spelling, and that answer is the one JSON Schema 2020-12 gives. `null`
/// matching two `oneOf` branches fails it, so the field is `Pick` with no `()` arm: a `const: null`
/// branch beside a nullable typed one or a `null` member (#668), beside a branch of scalar keywords
/// or one that states nothing and a single-category `type` array (#670, #668's stated-nothing
/// counterpart), beside a `$ref` to a union `null` matches through an untyped branch, which is
/// narrowed to `null` rather than excluded (#670), and two branches that state nothing (#673). A
/// branch the conjunct excludes is reported (`W011`) in every spelling (#671), one of a category a
/// multi-category `type` array omits included (#677), and one of two branches that lower to one
/// generated type (#678). A `$ref` to a union of one untyped object branch is met as that union
/// beside a scalar conjunct, as the inline union is (#678). An exact-null branch beside one that
/// refuses `null`, under a multi-category `type` array listing `null`, is the `()` arm in every
/// spelling, the conjunct-first ones included (#818).
#[test]
fn the_cases_one_null_decision_settles_agree_in_every_spelling() {
    let excluded_1 = vec![
        "union member 1 cannot satisfy the enclosing schema's own constraints, so it is not a \
         variant of the generated enum"
            .to_owned(),
    ];
    let pick = |optional: bool, excluded: Vec<String>| NullAnswer {
        generated: true,
        optional,
        unit_arms: 0,
        option_arms: 0,
        excluded,
    };
    let mut rows: Vec<(&str, &str, Vec<&str>, NullAnswer)> = vec![
        // #668
        (
            "#668",
            "",
            vec!["{ const: null }", "{ type: [string, 'null'] }"],
            pick(false, Vec::new()),
        ),
        (
            "#668",
            "",
            vec!["{ type: 'null' }", "{ const: null }", "{ type: string }"],
            pick(false, Vec::new()),
        ),
        // #670, the `$ref` branch `null` satisfies through its `items` branch.
        (
            "#670",
            "type: [string, 'null']",
            vec![
                "{ $ref: '#/components/schemas/U' }",
                "{ const: null }",
                "{ type: string }",
            ],
            pick(false, Vec::new()),
        ),
        // #671
        (
            "#671",
            "type: [string, 'null']",
            vec!["{}", "{ type: integer }", "{ type: string, enum: [a] }"],
            pick(true, excluded_1.clone()),
        ),
        // #678, two branches that lower to one generated type, one of which the conjunct
        // excludes: reported before the meet merges what it leaves.
        (
            "#678",
            "type: [integer, 'null']",
            vec!["{ type: [string, 'null'] }", "{ type: string }"],
            pick(false, excluded_1.clone()),
        ),
        // #673
        (
            "#673",
            "type: [string, 'null']",
            vec!["{}", "{}"],
            pick(false, Vec::new()),
        ),
        // #677, a branch of a category the multi-category conjunct omits.
        (
            "#677",
            "type: [string, integer]",
            vec!["{ type: integer }", "{ type: boolean }", "{ type: string }"],
            pick(false, excluded_1),
        ),
        (
            "#673",
            "type: [string, 'null']",
            vec!["{}", "{ type: 'null' }"],
            pick(false, Vec::new()),
        ),
    ];
    // #670, a branch of scalar keywords or one that states nothing beside `const: null` and a
    // single-category `type` array; #668's stated-nothing counterpart beside `type: [string,
    // 'null']` alone.
    for (conjunct, typed) in [
        ("type: [integer, 'null']", "{ type: integer }"),
        ("type: [string, 'null']", "{ type: string }"),
    ] {
        for other in [
            "{ minLength: 1 }",
            "{ pattern: '^a' }",
            "{ minimum: 1 }",
            "{ not: { type: boolean } }",
            "true",
            "{}",
        ] {
            rows.push((
                "#670",
                conjunct,
                vec![other, "{ const: null }", typed],
                pick(false, Vec::new()),
            ));
        }
    }
    for branch in ["{ const: null }", "{ enum: [null] }"] {
        rows.push((
            "#668",
            "type: [string, 'null']",
            vec!["{}", branch],
            pick(false, Vec::new()),
        ));
    }
    // An `anyOf` whose exact-null branch carries the `null` its conjunct admits, beside a `$ref` to
    // a union `null` also matches: `null` is carried once, by the `()` arm, so the field is no
    // `Option` in any spelling (Decision 3).
    let mut any_of_rows: Vec<(&str, &str, Vec<&str>, NullAnswer)> = [
        "type: [string, 'null'], enum: [a, null]",
        "type: [string, 'null']",
    ]
    .into_iter()
    .map(|conjunct| {
        (
            "#674",
            conjunct,
            vec![
                "{ type: string }",
                "{ $ref: '#/components/schemas/U' }",
                "{ const: null }",
            ],
            NullAnswer {
                unit_arms: 1,
                ..pick(false, Vec::new())
            },
        )
    })
    .collect();
    // #818, an exact-null branch beside a branch that refuses `null`, held to a multi-category
    // `type` array listing `null`: the branch's `null` is the `()` arm in every spelling, the ones
    // that list the conjunct before the union included, and never an `Option` around `Pick`.
    for (one_of, branches) in [
        (true, vec!["{ type: integer }", "{ const: null }"]),
        (
            true,
            vec!["{ type: string, enum: [a] }", "{ enum: [null] }"],
        ),
        (
            true,
            vec!["{ type: integer }", "{ $ref: '#/components/schemas/Z' }"],
        ),
        (false, vec!["{ type: string }", "{ const: null }"]),
    ] {
        let row = (
            "#818",
            "type: [string, integer, 'null']",
            branches,
            NullAnswer {
                unit_arms: 1,
                ..pick(false, Vec::new())
            },
        );
        if one_of {
            rows.push(row);
        } else {
            any_of_rows.push(row);
        }
    }
    // #678, a union of one untyped object branch beside a scalar conjunct: the `$ref`'d union is
    // met as the union it is, not mixed with the conjunct as the object it lowers to, so only
    // `null` is left in every spelling.
    any_of_rows.push((
        "#678",
        "type: [integer, 'null']",
        vec!["{ properties: { a: { type: string } } }"],
        pick(false, Vec::new()),
    ));
    let mut mismatches = Vec::new();
    let rows = rows
        .into_iter()
        .map(|row| (true, row))
        .chain(any_of_rows.into_iter().map(|row| (false, row)));
    for (one_of, (issue, conjunct, branches, expected)) in rows {
        let case = NullCase {
            conjunct,
            one_of,
            branches,
        };
        match assert_null_parity(&case) {
            Ok(answer) if answer == expected => {}
            Ok(answer) => mismatches.push(format!(
                "{issue} {conjunct} {}: {answer:?}, not {expected:?}",
                case.union()
            )),
            Err(error) => mismatches.push(format!("{issue} {}: {error}", case.union())),
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `type` array listing `null` beside an `enum` or `const` that leaves it out admits no `null`,
/// since every one of the three must hold (#666): a standalone `Pick` written so is the
/// non-optional field type the same set without the `null` gets, through `generate` and `check`.
#[test]
fn a_type_array_null_an_enum_refuses_gives_a_standalone_enum_no_option() {
    let holder = "    Holder: { type: object, properties: { pick: { $ref: \
                  '#/components/schemas/Pick' } }, required: [pick] }\n";
    let field = |pick: &str| {
        let spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    Pick: {pick}\n{holder}"
        );
        let (report, source) = generate_module(&spec);
        let checked = check(&spec);
        assert_eq!(rejected(&report), rejected(&checked), "{pick}");
        assert_eq!(codes(&report), codes(&checked), "{pick}");
        source
            .lines()
            .find_map(|line| line.trim().strip_prefix("pub pick: ").map(str::to_owned))
            .unwrap_or_else(|| panic!("{pick}: no `pick` field:\n{source}"))
    };
    for (lenient, strict) in [
        (
            "{ type: [string, 'null'], enum: [a] }",
            "{ type: string, enum: [a] }",
        ),
        (
            "{ type: [string, 'null'], const: a }",
            "{ type: string, const: a }",
        ),
        (
            "{ type: string, enum: [a, null] }",
            "{ type: string, enum: [a] }",
        ),
        (
            "{ type: [integer, 'null'], enum: [1] }",
            "{ type: integer, enum: [1] }",
        ),
    ] {
        assert_eq!(field(lenient), field(strict), "{lenient} against {strict}");
        assert_eq!(field(lenient), "Pick,", "{lenient}");
    }
    // `null` listed by all three keeps the `Option`.
    assert_eq!(
        field("{ type: [string, 'null'], enum: [a, null] }"),
        "Option<Pick>,"
    );
}

/// The member order of an `allOf` decides nothing about `null` (#675, #676): a union member
/// whose own branches take `null` from one that states it, beside a `type` member that is a
/// union itself or beside a second union member, and the exact-null two-union shape, give one
/// field type in every order, nested in an inner `allOf` or not. `null` matches two branches of the
/// null-stating union, so it fails the `oneOf` there and the field is `Pick`. A union member
/// beside a `type` member that is a union itself also reports one set of `W001`s in every order
/// (#678).
#[test]
fn the_member_order_of_an_all_of_decides_nothing_about_null() {
    let shapes: [(&str, Vec<&str>); 3] = [
        (
            "#675",
            vec![
                "{ type: [string, integer, 'null'] }",
                "{ oneOf: [ {}, { type: [string, 'null'] } ] }",
            ],
        ),
        (
            "#675",
            vec![
                "{ type: [string, 'null'] }",
                "{ oneOf: [ {}, { type: string, enum: [b] } ] }",
                "{ oneOf: [ {}, { type: [string, 'null'] } ] }",
            ],
        ),
        (
            "#676",
            vec![
                "{ type: [string, 'null'] }",
                "{ oneOf: [ {}, { const: null } ] }",
                "{ oneOf: [ {}, { type: string, enum: [a] } ] }",
            ],
        ),
    ];
    let mut mismatches = Vec::new();
    for (issue, members) in shapes {
        let mut sites = Vec::new();
        let orders: Vec<Vec<usize>> = if members.len() == 2 {
            vec![vec![0, 1], vec![1, 0]]
        } else {
            vec![
                vec![0, 1, 2],
                vec![0, 2, 1],
                vec![1, 0, 2],
                vec![1, 2, 0],
                vec![2, 0, 1],
                vec![2, 1, 0],
            ]
        };
        for order in &orders {
            let written: Vec<&str> = order.iter().map(|&index| members[index]).collect();
            sites.push(format!("{{ allOf: [ {} ] }}", written.join(", ")));
        }
        // The unions nested in an inner `allOf` beside the first member.
        sites.push(format!(
            "{{ allOf: [ {}, {{ allOf: [ {} ] }} ] }}",
            members[0],
            members[1..].join(", ")
        ));
        let mut answers = Vec::new();
        // The `W001`s each site reports, by message: a `type` member that is a union of its own
        // beside one union member is met alike in every order, so it collapses alike (#678).
        let mut collapses: Vec<(String, Vec<String>)> = Vec::new();
        for site in &sites {
            let spec = format!(
                "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
                 schemas:\n    Pick: {site}\n{NULL_COMPONENTS}"
            );
            let (report, source) = generate_module(&spec);
            let checked = check(&spec);
            if rejected(&report) != rejected(&checked) || codes(&report) != codes(&checked) {
                mismatches.push(format!("{issue} {site}: `check` and `generate` disagree"));
            }
            // A union member the meet replaced leaves no standalone type nothing names, and no
            // `Value` alias.
            for unused in unreferenced_member_types(&source) {
                mismatches.push(format!("{issue} {site}: `{unused}` is emitted unused"));
            }
            if source.contains("= serde_json::Value;") {
                mismatches.push(format!("{issue} {site}: a `Value` alias:\n{source}"));
            }
            answers.push((site.clone(), null_answer(&report, &source)));
            let mut collapsed: Vec<String> = report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == Code::ValidationKeywordIgnored)
                .map(|diagnostic| diagnostic.message.clone())
                .collect();
            collapsed.sort();
            collapses.push((site.clone(), collapsed));
        }
        // Two union members still nest by their own order, so only the one-union shape is held.
        if members.len() == 2 {
            if let Some((first_site, first)) = collapses.first() {
                for (site, collapsed) in &collapses[1..] {
                    if collapsed != first {
                        mismatches.push(format!(
                            "{issue} {site}: `W001`s {collapsed:?}, where {first_site} reports \
                             {first:?}"
                        ));
                    }
                }
            }
        }
        for (site, answer) in &answers {
            if !answer.generated || answer.optional || answer.unit_arms + answer.option_arms > 0 {
                mismatches.push(format!("{issue} {site}: {answer:?}, not `pub pick: Pick`"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// The `PickMember…` types `source` declares that nothing else in it names.
fn unreferenced_member_types(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line
                .strip_prefix("pub enum ")
                .or_else(|| line.strip_prefix("pub type "))?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            name.starts_with("PickMember").then_some(name)
        })
        .filter(|name| {
            ![
                format!("Box<{name}>"),
                format!("Option<{name}>"),
                format!("Vec<{name}>"),
                format!("= {name};"),
                format!(": {name},"),
            ]
            .iter()
            .any(|use_site| source.contains(use_site.as_str()))
        })
        .collect()
}
