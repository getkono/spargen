//! How `null` reaches a union through its branches: a `"null"` beside a union needs a branch that
//! accepts it, and untyped, stated-nothing (`true`, `{}`), `false` and `$ref` branches decide
//! `null`, or leave it undecided, alike in every spelling of their conjunction.

use super::*;

/// A `"null"` in the `type` array beside a `oneOf`/`anyOf` only permits `null`: the union still
/// needs a branch `null` matches (exactly one for `oneOf`, at least one for `anyOf`), so its
/// branches alone decide whether the position is `Option` (#574). Branches typed `object` beside
/// `type: [object, 'null']` all refuse `null`, so the field is not `Option`, with several branches
/// or one, and where the multi-type array is dropped for lowering with nothing else beside it.
/// A branch that accepts `null` itself, or an untyped one that takes it from the `type` array,
/// keeps it valid.
#[test]
fn a_union_admits_null_only_through_a_branch_that_accepts_it() {
    let typed_objects = "\n            - { type: object, required: [a] }\n            \
                         - { type: object, required: [c], properties: { d: { type: string } } }";
    let props = "properties: { a: { type: string }, c: { type: integer } }";
    for (shape, schema, nullable) in [
        (
            "oneOf of typed objects beside a nullable object type",
            format!("type: [object, 'null']\n          {props}\n          oneOf:{typed_objects}"),
            false,
        ),
        (
            "anyOf of typed objects beside a nullable object type",
            format!("type: [object, 'null']\n          {props}\n          anyOf:{typed_objects}"),
            false,
        ),
        (
            "sole typed branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ type: object, required: [a] }} ]"
            ),
            false,
        ),
        (
            "typed scalars beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             oneOf: [ { type: string }, { type: integer } ]"
                .to_owned(),
            false,
        ),
        (
            "anyOf of typed scalars beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             anyOf: [ { type: string }, { type: integer } ]"
                .to_owned(),
            false,
        ),
        (
            "sole typed scalar beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ { type: string } ]".to_owned(),
            false,
        ),
        (
            "a branch that accepts null itself",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ type: [object, 'null'], required: [a] }}\n            \
                 - {{ type: object, required: [c] }}"
            ),
            true,
        ),
        (
            "an untyped branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ required: [a] }}\n            \
                 - {{ type: object, required: [c], properties: {{ d: {{ type: string }} }} }}"
            ),
            true,
        ),
        (
            "an anyOf of untyped object branches beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 anyOf: [ {{ properties: {{ a: {{ type: string }} }} }}, {{ properties: {{ b: {{ \
                 type: string }} }} }} ]"
            ),
            true,
        ),
        (
            "a sole untyped object branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ properties: {{ b: {{ type: string }} }} }} ]"
            ),
            true,
        ),
        // `null` matches both untyped branches, which fails the `oneOf`'s exactly-one.
        (
            "a oneOf of two untyped object branches beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ properties: {{ a: {{ type: string }} }} }}, {{ properties: {{ b: {{ \
                 type: string }} }} }} ]"
            ),
            false,
        ),
        (
            "an untyped object branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { properties: { a: { type: string } } }, { type: string } ]"
                .to_owned(),
            true,
        ),
        (
            "a sole untyped object branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { properties: { a: { type: string } } } ]"
                .to_owned(),
            true,
        ),
        (
            "a null member beside typed scalars and a dropped multi-type array",
            "type: [string, integer, 'null']\n          \
             oneOf: [ { type: 'null' }, { type: string }, { type: integer } ]"
                .to_owned(),
            true,
        ),
        // A nested union of untyped branches states nothing about `null` either: `null` matches
        // both of its `anyOf`'s branches, so it matches the nested union.
        (
            "an untyped nested anyOf branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          oneOf:\n            \
                 - {{ anyOf: [ {{ required: [a] }}, {{ required: [c] }} ] }}\n            \
                 - {{ type: object, required: [c], properties: {{ d: {{ type: string }} }} }}"
            ),
            true,
        ),
        (
            "a sole untyped nested anyOf branch beside a nullable object type",
            format!(
                "type: [object, 'null']\n          {props}\n          \
                 oneOf: [ {{ anyOf: [ {{ required: [a] }}, {{ required: [c] }} ] }} ]"
            ),
            true,
        ),
        (
            "an untyped nested anyOf branch beside a dropped multi-type array",
            "type: [object, string, 'null']\n          \
             oneOf: [ { anyOf: [ { required: [a] }, { required: [b] } ] }, { type: string } ]"
                .to_owned(),
            true,
        ),
        // `null` matches both branches of the nested `oneOf`, so it matches no branch here.
        (
            "an untyped nested oneOf branch that null matches twice",
            "type: [object, string, 'null']\n          \
             oneOf: [ { oneOf: [ { required: [a] }, { required: [b] } ] }, { type: string } ]"
                .to_owned(),
            false,
        ),
        // A `true` branch lowers to `Value` and takes the array's `null` with no sibling to meet.
        (
            "a true branch beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ true, { type: integer } ]"
                .to_owned(),
            true,
        ),
        (
            "a sole true branch beside a dropped multi-type array",
            "type: [string, integer, 'null']\n          oneOf: [ true ]".to_owned(),
            true,
        ),
        // No union keyword at all: the array's own `null` is the synthesized union's null branch.
        (
            "a plain multi-type array",
            "type: [string, integer, 'null']".to_owned(),
            true,
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      type: object
      properties:
        x:
          {schema}
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{shape}: {report:#?}");
        let types = types_module(&code);
        let x = field_type(&types, "pub x").unwrap_or_else(|| panic!("{shape}: no `x`: {types}"));
        assert_eq!(
            x.starts_with("Option<"),
            nullable,
            "{shape}: `x` is `{x}`, but `null` is {} here: {types}",
            if nullable { "valid" } else { "invalid" }
        );
    }
}

/// A cycle-closing `$ref` branch to an untyped object component takes the `null` a dropped
/// multi-type array permits, as an inline untyped branch does (#574): the component's struct is
/// still a reservation when the branch is lowered, so whether `null` matches it is read from the
/// target's own keywords, and `properties` is vacuous on `null`.
#[test]
fn a_cycle_closing_ref_branch_to_an_untyped_object_takes_the_type_arrays_null() {
    for (shape, schema) in [
        (
            "a sole cycle-closing branch",
            "type: [object, array, 'null']\n          oneOf: [ { $ref: '#/components/schemas/Holder' } ]",
        ),
        (
            "a cycle-closing branch beside a typed one",
            "type: [object, string, 'null']\n          \
             oneOf: [ { $ref: '#/components/schemas/Holder' }, { type: string } ]",
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      properties:
        child:
          {schema}
      required: [child]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{shape}: {report:#?}");
        let types = types_module(&code);
        let child = field_type(&types, "pub child")
            .unwrap_or_else(|| panic!("{shape}: no `child`: {types}"));
        assert!(
            child.starts_with("Option<"),
            "{shape}: `child` is `{child}`, but `null` is valid here: {types}"
        );
    }
}

/// Issue #541, beside a union: a `$ref` to an untyped object component `U` admits `null` without
/// deciding it when the meet also carries a `oneOf`/`anyOf`, as the inline untyped member does.
/// The `$ref` with a union sibling (`refUnion`) is met in `meet_ref_union_sibling`, and the
/// `$ref` member beside a union member (`memberRef`) or beside a union on the `allOf` itself
/// (`besideRef`) is combined as a composition; each recorded `U`'s non-null struct as a decision,
/// so it denied the `null` the inline spelling of the same meet admits. The `type: object`
/// controls stay non-null. `refUnion`'s branches state `'null'` themselves: a `$ref`'s union
/// sibling of untyped branches is lowered apart from the sibling's `type`, and denies `null`
/// whatever the target (#567), so only branches that admit it show the target's answer.
#[test]
fn a_ref_to_an_untyped_object_component_admits_null_beside_a_union() {
    const COMPONENTS: &str = r##"
components:
  schemas:
    U: { properties: { u: { type: string } } }
    Holder:
      type: object
      required: [refUnion, inlineUnion, memberRef, memberInline, besideRef, besideInline, refUnionObject, memberRefObject]
      properties:
        refUnion: { $ref: '#/components/schemas/U', type: [object, 'null'], anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        inlineUnion: { type: [object, 'null'], properties: { u: { type: string } }, anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        memberRef: { allOf: [{ $ref: '#/components/schemas/U' }, { type: [object, 'null'], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
        memberInline: { allOf: [{ properties: { u: { type: string } } }, { type: [object, 'null'], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
        besideRef: { type: [object, 'null'], allOf: [{ $ref: '#/components/schemas/U' }], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }
        besideInline: { type: [object, 'null'], allOf: [{ properties: { u: { type: string } } }], anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }
        refUnionObject: { $ref: '#/components/schemas/U', type: object, anyOf: [{ type: [object, 'null'], properties: { a: { type: string } } }, { type: [object, 'null'], properties: { b: { type: string } } }] }
        memberRefObject: { allOf: [{ $ref: '#/components/schemas/U' }, { type: object, anyOf: [{ properties: { a: { type: string } } }, { properties: { b: { type: string } } }] }] }
"##;
    const EXPECTED: [(&str, bool); 8] = [
        ("pub ref_union:", true),
        ("pub inline_union:", true),
        ("pub member_ref:", true),
        ("pub member_inline:", true),
        ("pub beside_ref:", true),
        ("pub beside_inline:", true),
        ("pub ref_union_object:", false),
        ("pub member_ref_object:", false),
    ];
    let root = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
{COMPONENTS}"##
    );
    let (generated, root_code) = generate_with_code(&root);
    let (split_generated, split_checked, split_code) =
        split("./lib.yaml#/components/schemas/Holder", COMPONENTS);
    for (entry, report) in [
        ("root/generate", &generated),
        ("split/generate", &split_generated),
        ("split/check", &split_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for (spelling, code) in [("root", &root_code), ("split", &split_code)] {
        for (field, nullable) in EXPECTED {
            let ty = field_type(code, field)
                .unwrap_or_else(|| panic!("{spelling}: no `{field}` field: {code}"));
            assert_eq!(
                ty.starts_with("Option<"),
                nullable,
                "{spelling}: `{field}` is `{ty}`, but `null` is {} here: {code}",
                if nullable { "valid" } else { "invalid" }
            );
        }
    }
}

/// Issue #581: a union whose only typed branch denies `null` (`type: object`, `type: string`, the
/// `false` schema, or an `enum` or `const` that leaves `null` out), beside an untyped object
/// branch, met with an untyped object composition. The untyped branch leaves `null` undecided, so
/// every spelling of the meet keeps the non-null struct: a `$ref` to the untyped `U` with the
/// union as its sibling, `U` as an `allOf` member beside the union or with the union as a second
/// member, and the same three with `U`'s body written inline. The `allOf` spellings over `U` read
/// the typed branch as the union deciding `null`, made the composition nullable for it, and
/// generated `Option<Pick>` where the others generate `Pick`.
/// A branch that states nothing (`true`, `{}`, annotations alone) leaves `null` undecided as the
/// untyped branch does (#588): the inline spelling, and the untyped sibling keywords `items` or
/// `required` alone, met its `Value` with the sibling's object half, took that half's `null` as
/// the branch's own, and generated `Option<Pick>`.
/// A typed branch that admits `null` (`type: [object, 'null']`) still decides it in the `anyOf`
/// `allOf` spellings, which stay `Option`.
#[test]
fn a_union_of_a_non_null_typed_branch_beside_an_untyped_one_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for (typed, admits) in [
            (
                "{ type: object, properties: { a: { type: string } } }",
                false,
            ),
            ("{ type: string }", false),
            ("false", false),
            ("{ enum: [x, y] }", false),
            ("{ const: x }", false),
            // A branch that states nothing (#588): the `true` schema, `{}`, and a branch of
            // annotations alone accept `null` as the untyped branch beside them does, and decide
            // it no more than it does.
            ("true", false),
            ("{}", false),
            ("{ description: d }", false),
            (
                "{ type: [object, 'null'], properties: { a: { type: string } } }",
                true,
            ),
        ] {
            let branches = format!("[ {typed}, {{ properties: {{ b: {{ type: string }} }} }} ]");
            // The nullable control is pinned in the `anyOf` `allOf` spellings only: every
            // spelling of the nullable rows is pinned by
            // `a_union_of_a_nullable_typed_branch_beside_an_untyped_one_agrees_across_spellings`.
            if admits && keyword == "oneOf" {
                continue;
            }
            let mut sites = vec![
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
            ];
            if !admits {
                sites.push(format!("{{ {u}, {keyword}: {branches} }}"));
                sites.push(format!(
                    "{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                ));
                // Untyped sibling keywords alone leave `null` to the union as `U` does: an
                // array-only `items`, and an object-only `required` that `null` satisfies.
                sites.push(format!(
                    "{{ items: {{ type: string }}, {keyword}: {branches} }}"
                ));
                sites.push(format!("{{ required: [u], {keyword}: {branches} }}"));
            }
            rows.extend(sites.into_iter().map(|site| (site, admits)));
        }
    }
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "undecided" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #590: a union of a `$ref` branch to the non-null object `Y` beside an untyped object
/// branch, met with the untyped object `U`, keeps the non-null struct in all six spellings of the
/// meet, as the same branch written inline does
/// (`a_union_of_a_non_null_typed_branch_beside_an_untyped_one_keeps_null_undecided`): a `$ref` to
/// `U` with the union as its sibling, `U` as an `allOf` member beside the union or with the union
/// as a second member, and the same three with `U`'s body written inline. The `allOf` spellings
/// over `U` read the `$ref` branch as the union deciding `null` without following it to `Y`, made
/// the composition nullable for it, and generated `Option<Pick>`. The branch reached through the
/// alias `Z` of `Y`, a `$ref` to the untyped `U` whose sibling `type: object` denies `null`, and
/// a `$ref` to the component `W` that is that same `$ref`-with-sibling, agree with the plain
/// `$ref` to `Y`. A `$ref` branch to the nullable
/// `N` still decides `null` in the `anyOf` `allOf` spellings, which stay `Option`.
#[test]
fn a_union_of_a_ref_branch_to_a_non_null_object_beside_an_untyped_one_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        // `Y` itself, the alias `Z` of it, the untyped `U` under a sibling `type: object`,
        // which denies `null` by the branch's own keyword whatever its target says, and `W`,
        // the component spelling of that same `$ref`-with-sibling branch.
        for denying in [
            "{ $ref: '#/components/schemas/Y' }",
            "{ $ref: '#/components/schemas/Z' }",
            "{ $ref: '#/components/schemas/U', type: object }",
            "{ $ref: '#/components/schemas/W' }",
        ] {
            let branches = format!("[ {denying}, {{ properties: {{ b: {{ type: string }} }} }} ]");
            rows.extend(
                [
                    format!("{{ {u}, {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                    format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                    format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                ]
                .into_iter()
                .map(|site| (site, false)),
            );
        }
    }
    // The nullable control: a `$ref` branch whose target admits `null` decides it.
    let nullable =
        "[ { $ref: '#/components/schemas/N' }, { properties: { b: { type: string } } } ]";
    rows.push((
        format!("{{ allOf: [ {{ {u} }} ], anyOf: {nullable} }}"),
        true,
    ));
    rows.push((
        format!("{{ allOf: [ {{ {u} }}, {{ anyOf: {nullable} }} ] }}"),
        true,
    ));
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Y: {{ type: object, properties: {{ y: {{ type: string }} }} }}
    Z: {{ $ref: '#/components/schemas/Y' }}
    W: {{ $ref: '#/components/schemas/U', type: object }}
    N: {{ type: [object, 'null'], properties: {{ n: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "undecided" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #586: a union of one typed branch that admits `null` (`type: [object, 'null']`,
/// `type: 'null'`, `enum: [x, null]`, `const: null` or `type: [string, 'null']`) beside an
/// untyped object branch, met with the untyped object `U`, gets one answer per row across all six
/// spellings: a `$ref` to `U` with the union as its sibling, `U` as an `allOf` member beside the
/// union or with the union as a second member, and the same three with `U`'s body written inline,
/// plus the `$ref` spelling with an untyped `required` beside the union, which meets it through
/// the same target, and two untyped sibling keywords alone, an array-only `items`, which does not
/// reach an object branch, and an object-only `required`, which `null` satisfies: both leave
/// `null` to the union as `U` does. `U` decides nothing about `null`, so an `anyOf` admits it through the typed
/// branch. A `oneOf` denies it: the untyped branch beside the typed one is counted as accepting
/// `null` too (#563), so `null` is in two branches and fails exactly-one; a `const: null` branch,
/// whose exact-`null` variant is not a nullable `Ty`, is counted as one of them. The
/// `$ref`-sibling spellings denied `null` in the `anyOf` rows, and the inline spellings admitted
/// it in the `oneOf` row with the nullable object branch. A branch that states nothing (`true`,
/// `{}`) in place of the untyped one gets the same answers (#592): the `$ref` and `allOf`
/// spellings did not count it as taking `null` from the conjunct the union is met with, so an
/// `anyOf` beside `const: null` denied `null` there, and a `oneOf` beside a nullable typed branch
/// counted `null` in one branch alone and admitted it. The `oneOf` rows of `const: null` beside
/// the untyped object branch emit no `()` variant for it either: `null` fails exactly-one, and the
/// branch accepts nothing else (#657). Where `null` is admitted it is carried once: an `anyOf`'s
/// `const: null` branch is `Pick`'s `()` arm, so the field naming it is `Pick`, not `Option<Pick>`
/// (#674's Decision 3); the other rows are `Option<Pick>`.
#[test]
fn a_union_of_a_nullable_typed_branch_beside_an_untyped_one_agrees_across_spellings() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<(String, bool, bool)> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for typed in [
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "{ type: 'null' }",
            "{ enum: [x, null] }",
            "{ const: null }",
            "{ type: [string, 'null'] }",
        ] {
            let admits = keyword == "anyOf";
            // A branch that states nothing (`true`, `{}`) in place of the untyped one accepts
            // `null` wherever the untyped one does (#588, #592), so the answers stand in every
            // spelling.
            for other in ["{ properties: { b: { type: string } } }", "true", "{}"] {
                let branches = format!("[ {typed}, {other} ]");
                // Beside the untyped object branch a `oneOf`'s `const: null` branch is no variant.
                let no_unit =
                    !admits && typed == "{ const: null }" && other.starts_with("{ properties");
                rows.extend(
                    [
                        format!("{{ {u}, {keyword}: {branches} }}"),
                        format!("{{ {u}, required: [u], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                        format!(
                            "{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                        ),
                        format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                        format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                        format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                        format!("{{ required: [u], {keyword}: {branches} }}"),
                    ]
                    .into_iter()
                    .map(|site| (site, admits, no_unit)),
                );
            }
        }
    }
    let mut mismatches = Vec::new();
    for (site, admits, no_unit) in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        // `null` is carried once: by the field's `Option`, or by `Pick`'s `()` arm.
        let unit_arm = types
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("pub type ")?
                    .strip_suffix(" = ();")
            })
            .any(|unit| types.contains(&format!("(Box<{unit}>)")));
        let optional = pick.starts_with("Option<");
        if (optional || unit_arm) != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
        if optional && unit_arm {
            mismatches.push(format!(
                "{site}: `pick` is `{pick}` and `Pick` has a `()` arm: `null` is carried twice"
            ));
        }
        if no_unit && types.contains("= ();") {
            mismatches.push(format!(
                "{site}: the `const: null` branch is still a `()` variant: {types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #592: a union branch that states nothing (`true`, `{}`), met with a conjunct that admits
/// `null` (the nullable object `NB`) through a `$ref` sibling or an `allOf`, takes that `null`
/// once. `Value` is the identity of the meet, so the meet hands the branch the conjunct's `null`
/// after the union has already counted it: an `anyOf` keeps it on the union alone, so the
/// position is `Option` and no variant is, and a `oneOf` whose `null` another branch also matches
/// (`type: 'null'`) keeps it nowhere. A `oneOf` whose only branch `null` matches is that branch
/// keeps it in that variant (`a_ref_sibling_one_of_whose_branches_differ_only_in_nullability_collapses`).
/// A union the meet narrows to the branch alone, beside a `type: string` branch the object
/// excludes, keeps the `anyOf`'s `null` on the position.
#[test]
fn a_stated_nothing_branch_met_with_a_nullable_conjunct_takes_its_null_once() {
    let nb = "$ref: '#/components/schemas/NB'";
    let c = "{ properties: { c: { type: string } } }";
    let rows = [
        (
            format!("{{ {nb}, required: [a], anyOf: [ true, {c} ] }}"),
            true,
        ),
        (
            format!("{{ {nb}, required: [a], anyOf: [ {{}}, {c} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], anyOf: [ true, {c} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }}, {{ anyOf: [ true, {c} ] }} ] }}"),
            true,
        ),
        (format!("{{ {nb}, anyOf: [ true, {c} ] }}"), true),
        (
            format!("{{ {nb}, required: [a], oneOf: [ {{ type: 'null' }}, true, {c} ] }}"),
            false,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ {{ type: 'null' }}, true, {c} ] }}"),
            false,
        ),
        (
            format!("{{ {nb}, anyOf: [ {{}}, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], anyOf: [ true, {{ type: string }} ] }}"),
            true,
        ),
    ];
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
        let union = pick
            .strip_prefix("Option<")
            .and_then(|inner| inner.strip_suffix('>'))
            .unwrap_or(&pick);
        let nullable_variants: Vec<String> = enum_variants(&types, union)
            .into_iter()
            .filter(|variant| variant.contains("(Option<"))
            .collect();
        if !nullable_variants.is_empty() {
            mismatches.push(format!(
                "{site}: `null` is the position's alone, but variants accept it: \
                 {nullable_variants:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #597: a `oneOf` whose only branch `null` matches states nothing (`true`, `{}`), beside a
/// `type: string` branch the nullable object `NB` excludes, met with `NB` through a `$ref`
/// sibling or an `allOf`. The meet narrows the union to that branch alone, and `null` is valid
/// there, since `NB` admits it and exactly one branch matches it, so every spelling generates
/// `Option<Pick>`, as the inline spelling does. Where another branch matches `null` too
/// (`type: 'null'`, `type: [string, 'null']`) it matches two, and where the conjunct (`N`) denies
/// it nothing admits it, so those stay `Pick`.
#[test]
fn a_one_of_narrowed_to_its_stated_nothing_branch_keeps_the_conjuncts_null() {
    let nb = "$ref: '#/components/schemas/NB'";
    let rows = [
        (format!("{{ {nb}, oneOf: [ {{}}, {{ type: string }} ] }}"), true),
        (format!("{{ {nb}, oneOf: [ true, {{ type: string }} ] }}"), true),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ true, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }} ], oneOf: [ {{}}, {{ type: string }} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ {nb} }}, {{ oneOf: [ true, {{ type: string }} ] }} ] }}"),
            true,
        ),
        (
            "{ type: [object, 'null'], properties: { a: { type: string } }, \
             oneOf: [ true, { type: string } ] }"
                .to_owned(),
            true,
        ),
        (
            format!("{{ {nb}, oneOf: [ {{ type: 'null' }}, true, {{ type: string }} ] }}"),
            false,
        ),
        (
            format!(
                "{{ allOf: [ {{ {nb} }} ], oneOf: [ {{ type: 'null' }}, {{}}, {{ type: string }} ] }}"
            ),
            false,
        ),
        (
            format!("{{ {nb}, oneOf: [ {{}}, {{ type: [string, 'null'] }} ] }}"),
            false,
        ),
        (
            "{ $ref: '#/components/schemas/N', oneOf: [ {}, { type: string } ] }".to_owned(),
            false,
        ),
        (
            "{ allOf: [ { $ref: '#/components/schemas/N' } ], oneOf: [ true, { type: string } ] }"
                .to_owned(),
            false,
        ),
    ];
    let mut mismatches = Vec::new();
    for (site, admits) in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    N:
      type: object
      properties:
        a: {{ type: string }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #588's other spellings of a branch that states nothing, met with an untyped object
/// composition: a union of that branch alone (`[ true ]`, `[ {} ]`, `[ $ref ]` to a `true` or `{}`
/// component), and of that branch beside a typed branch that denies `null` (`type: string`,
/// `type: integer`). Nothing here decides `null`, so every spelling keeps the non-null struct, as
/// the `$ref`-sibling spelling over `U` already did. The inline-sibling spellings (`properties`,
/// `items`, `required`) met the branch's `Value` with the sibling's object half, took that half's
/// `null` as the branch's own, and generated `Option<Pick>`; so did the inline `allOf`-member
/// spellings of a union of that branch alone, which collapses to the branch. A `false` branch
/// beside an untyped one denies `null` and so decides nothing either (#581), whether written
/// inline or as a `$ref` to a `false` component.
#[test]
fn a_union_of_a_stated_nothing_branch_alone_or_beside_a_non_null_type_keeps_null_undecided() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let t = "{ $ref: '#/components/schemas/T' }";
    let e = "{ $ref: '#/components/schemas/E' }";
    let mut rows: Vec<String> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for branches in [
            "[ true ]".to_owned(),
            "[ {} ]".to_owned(),
            format!("[ {t} ]"),
            format!("[ {e} ]"),
            "[ { type: string }, true ]".to_owned(),
            "[ { type: string }, {} ]".to_owned(),
            "[ { type: integer }, true ]".to_owned(),
            format!("[ {t}, {{ properties: {{ b: {{ type: string }} }} }} ]"),
            format!("[ {e}, {{ properties: {{ b: {{ type: string }} }} }} ]"),
            "[ { $ref: '#/components/schemas/R' } ]".to_owned(),
            "[ { $ref: '#/components/schemas/V' }, { type: string } ]".to_owned(),
            "[ false, { properties: { b: { type: string } } } ]".to_owned(),
            "[ { $ref: '#/components/schemas/F' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
            "[ { $ref: '#/components/schemas/G' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
            "[ { not: {} } ]".to_owned(),
            "[ { $ref: '#/components/schemas/X' } ]".to_owned(),
            "[ { not: {} }, { properties: { b: { type: string } } } ]".to_owned(),
            "[ { $ref: '#/components/schemas/X' }, { properties: { b: { type: string } } } ]"
                .to_owned(),
        ] {
            // The `allOf` spellings over `U` of a `$ref` branch to the `true` or `{}` component
            // generated `Option<Pick>` until a `$ref` branch was read through its target (#594),
            // as did those of a `$ref` to an alias `R` of `T`, of a `$ref` to the untyped object
            // `V` beside a branch that denies `null` (the inline `V` is undecided, #581), of a
            // `$ref` to the `false` component `F` (or its alias `G`) beside an untyped branch,
            // which denies `null` as the inline `false` does, and of a `$ref` to the `{ not: {} }`
            // component `X`, which states no type and so decides nothing, as inline. A union of a
            // `false` branch alone is uninhabited, so it has no `null` to decide:
            // `a_union_of_a_false_branch_alone_is_uninhabited_in_every_spelling_of_its_conjunction`
            // pins its spellings (#615).
            rows.extend([
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ {u}, {keyword}: {branches} }}"),
                format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                format!("{{ required: [u], {keyword}: {branches} }}"),
            ]);
        }
    }
    let mut mismatches = Vec::new();
    for site in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    T: true
    E: {{}}
    R: {{ $ref: '#/components/schemas/T' }}
    F: false
    G: {{ $ref: '#/components/schemas/F' }}
    X: {{ not: {{}} }}
    V: {{ properties: {{ v: {{ type: string }} }} }}
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") {
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is undecided here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A union whose only branch is `false` admits no value, and so does every conjunction it is part
/// of, however the conjunction is spelled: beside or inside an `allOf` over an object, beside a
/// `$ref` to one (alone, with untyped object keywords, or with `type: object`), beside the same
/// object written inline, beside
/// `type: object`, or alone. Each spelling generates the uninhabited `Pick` with no `E013` or
/// `E007` (#615). The `allOf`-over-`U` and `$ref`-to-`U` spellings were
/// `E013` and the `type: object` ones `E007`, while the inline
/// object spellings and the bare union generated the uninhabited type. The `false` branch is also
/// written as a `$ref` to the `false` component `F` and to its alias `G`, and beside a `null`
/// member under a `type` that denies `null`, which leaves the same empty set.
#[test]
fn a_union_of_a_false_branch_alone_is_uninhabited_in_every_spelling_of_its_conjunction() {
    let u = "$ref: '#/components/schemas/U'";
    let c = "{ properties: { u: { type: string } } }";
    let mut rows: Vec<String> = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for branches in [
            "[ false ]",
            "[ { $ref: '#/components/schemas/F' } ]",
            "[ { $ref: '#/components/schemas/G' } ]",
        ] {
            rows.extend([
                // The `$ref` arm alone, and beside the untyped object keywords that scope a
                // refiner over the union (`meet_ref_union_sibling`).
                format!("{{ {u}, {keyword}: {branches} }}"),
                format!("{{ {u}, required: [u], {keyword}: {branches} }}"),
                format!(
                    "{{ {u}, properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"
                ),
                format!("{{ allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {{ {u} }}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ properties: {{ u: {{ type: string }} }}, {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c} ], {keyword}: {branches} }}"),
                format!("{{ allOf: [ {c}, {{ {keyword}: {branches} }} ] }}"),
                format!("{{ items: {{ type: string }}, {keyword}: {branches} }}"),
                format!("{{ required: [u], {keyword}: {branches} }}"),
                format!("{{ {keyword}: {branches} }}"),
                format!("{{ type: object, {keyword}: {branches} }}"),
                format!("{{ type: object, {u}, {keyword}: {branches} }}"),
                format!("{{ type: object, allOf: [ {{ {u} }} ], {keyword}: {branches} }}"),
            ]);
        }
    }
    rows.push("{ type: object, oneOf: [ false, { type: 'null' } ] }".to_owned());
    rows.push("{ type: object, anyOf: [ false, { type: 'null' } ] }".to_owned());
    let mut mismatches = Vec::new();
    for site in rows {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    F: false
    G: {{ $ref: '#/components/schemas/F' }}
    U: {{ properties: {{ u: {{ type: string }} }} }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        if report.outcome() == Outcome::Rejected
            || has_code(&report, Code::AllOfIrreconcilable)
            || has_code(&report, Code::NonDisjointUnion)
        {
            mismatches.push(format!("{site}: {:?}", codes(&report)));
            continue;
        }
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick");
        if pick.as_deref() != Some("Pick") || !types.contains("pub enum Pick {}") {
            mismatches.push(format!(
                "{site}: `pick` is {pick:?}, not the uninhabited `Pick`: {types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A union of a `$ref` to the `true` component `T` beside an untyped object branch, met with the
/// untyped sibling keyword `items` or `required` alone, generates `Pick`: neither the `T` branch
/// nor the untyped one decides `null`, and the sibling keyword never adds it. `spargen-v0.5.0`
/// generated `Option<Pick>` for every row here; #593 changed them, a released change of a
/// generated field type, so this fixture pins the exact type rather than only "not optional" (#595).
#[test]
fn a_ref_to_a_true_component_beside_an_untyped_object_branch_under_items_or_required_is_not_optional(
) {
    let mut mismatches = Vec::new();
    for keyword in ["anyOf", "oneOf"] {
        for sibling in ["items: { type: string }", "required: [u]"] {
            let site = format!(
                "{{ {sibling}, {keyword}: [ {{ $ref: '#/components/schemas/T' }}, \
                 {{ properties: {{ b: {{ type: string }} }} }} ] }}"
            );
            let spec = format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    T: true
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
            );
            let (report, code) = generate_with_code(&spec);
            assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
            let types = types_module(&code);
            let pick = field_type(&types, "pub pick")
                .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
            if pick != "Pick" {
                mismatches.push(format!("{site}: `pick` is `{pick}`, expected `Pick`"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #934: a `oneOf` member of an `allOf` whose branches `null` matches twice, by the one
/// decision, admits no `null`: here a branch of scalar keywords alone and a `$ref` to a union
/// `null` matches through its untyped `items` branch. So beside a `const: null` member the scalar
/// meet is empty and the schema is rejected (`E013`), in every member order and nested in an inner
/// `allOf`, and the union member's `serde_json::Value` variant is reported as the meet that failed
/// left it (`W001`).
#[test]
fn a_union_member_null_matches_twice_takes_no_null_from_a_null_member() {
    let union = "{ oneOf: [ { minLength: 1 }, { $ref: '#/components/schemas/U' } ] }";
    let mut mismatches = Vec::new();
    for site in [
        format!("{{ allOf: [ {{ const: null }}, {union} ] }}"),
        format!("{{ allOf: [ {union}, {{ const: null }} ] }}"),
        format!("{{ allOf: [ {{ const: null }}, {{ allOf: [ {union} ] }} ] }}"),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    U: {{ oneOf: [ {{ items: {{ type: string }} }}, {{ type: boolean }} ] }}\n    \
                 Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
            ),
        );
        let (report, _) = generate_with_code(&spec);
        let untyped = messages_for(&report, Code::ValidationKeywordIgnored)
            .into_iter()
            .any(|message| message.contains("member 0 lowers to `serde_json::Value`"));
        if report.outcome() != Outcome::Rejected
            || !has_code(&report, Code::AllOfIrreconcilable)
            || !untyped
        {
            mismatches.push(format!(
                "{site}: not `E013` with the member's `W001`: {report:#?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #934: an `anyOf` whose branch states nothing or holds scalar keywords alone, met with a
/// conjunct that admits `null`, carries that `null` once, as the one decision settles it. The
/// meet hands such a branch the conjunct's `null`; an `anyOf` needs one match, so the `null` is
/// the position's `Option` and no variant's, and beside an exact-`null` branch it is that `()`
/// arm alone. The union written as a `$ref`'d component beside the conjunct gives the answer the
/// sibling spelling gives.
#[test]
fn an_any_of_met_with_a_nullable_conjunct_carries_its_null_once() {
    let mut mismatches = Vec::new();
    for (union, optional, unit_arm) in [
        ("anyOf: [ {}, { minLength: 1 } ]", true, false),
        ("anyOf: [ true, { pattern: '^a' } ]", true, false),
        ("anyOf: [ { const: null }, { minLength: 1 } ]", false, true),
        ("anyOf: [ { enum: [null] }, {} ]", false, true),
    ] {
        for (site, extra) in [
            (
                format!("{{ type: [string, 'null'], {union} }}"),
                String::new(),
            ),
            (
                "{ type: [string, 'null'], $ref: '#/components/schemas/Un' }".to_owned(),
                format!("    Un: {{ {union} }}\n"),
            ),
        ] {
            let spec = with_schemas(
                "3.1.0",
                &format!(
                    "    Pick: {site}\n{extra}    Holder:\n      type: object\n      \
                     properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                     required: [pick]\n"
                ),
            );
            let (report, code) = generate_with_code(&spec);
            assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
            let types = types_module(&code);
            let pick = field_type(&types, "pub pick")
                .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
            let variants = enum_variants(&types, "Pick");
            let option_arms: Vec<&String> = variants
                .iter()
                .filter(|variant| variant.contains("(Option<"))
                .collect();
            let has_unit_arm = types.contains("pub type UnVariant0 = ();")
                || types.contains("pub type PickVariant0 = ();");
            if pick.starts_with("Option<") != optional
                || !option_arms.is_empty()
                || has_unit_arm != unit_arm
            {
                mismatches.push(format!(
                    "{site} with {union}: `pick` is `{pick}`, `Option` arms {option_arms:?}, \
                     `()` arm {has_unit_arm}; expected `Option` {optional}, no `Option` arm, `()` \
                     arm {unit_arm}\n{types}"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #934: a `oneOf` counts `null` in every branch the one decision says it satisfies — a
/// branch of scalar keywords alone and one that states nothing among them — so beside such a
/// branch an exact-`null` branch is a second branch `null` matches, and is no variant before the
/// union's branches are compared. `{ minLength: 1 }` and `{ type: integer }` met with
/// `type: [integer, 'null']` are then the union's only branches, which lower to one type, so the
/// union is that type; and `{}` beside `enum: [null]` is the `serde_json::Value` it lowers to,
/// with no `W001` for a second variant the union does not have.
#[test]
fn a_one_of_exact_null_branch_beside_a_branch_null_matches_unstated_is_no_variant() {
    let pick = |body: &str| {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {body}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
            ),
        );
        generate_with_code(&spec)
    };
    let mut mismatches = Vec::new();
    for other in ["{ minLength: 1 }", "{ minimum: 1 }", "true", "{}"] {
        let site = format!(
            "{{ type: [integer, 'null'], oneOf: [ {other}, {{ const: null }}, {{ type: integer }} \
             ] }}"
        );
        let (report, code) = pick(&site);
        let types = types_module(&code);
        let collapsed = messages_for(&report, Code::ValidationKeywordIgnored)
            .into_iter()
            .any(|message| message.contains("members 0, 2") && message.contains("the union is"));
        if report.outcome() == Outcome::Rejected
            || !collapsed
            || field_type(&types, "pub pick").as_deref() != Some("Pick")
            || types.contains("pub enum Pick ")
        {
            mismatches.push(format!(
                "{site}: not `Pick`, one type with members 0 and 2 merged: {report:#?}\n{types}"
            ));
        }
    }
    for exact_null in ["{ enum: [null] }", "{ const: null }"] {
        let site = format!("{{ oneOf: [ {{}}, {exact_null} ] }}");
        let (report, code) = pick(&site);
        let types = types_module(&code);
        let untyped = messages_for(&report, Code::ValidationKeywordIgnored)
            .into_iter()
            .any(|message| message.contains("lowers to `serde_json::Value`"));
        if report.outcome() == Outcome::Rejected
            || untyped
            || !types.contains("pub type Pick = serde_json::Value;")
        {
            mismatches.push(format!(
                "{site}: not `Value` without a `W001`: {report:#?}\n{types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Issue #934: an `anyOf` beside a `$ref` to a nullable conjunct, whose branches the meet leaves
/// as the conjunct itself, collapses to that conjunct with `W001` even though the branches differ
/// in `null`: a branch that states nothing takes the conjunct's `null`, and `{ type: array }`
/// refuses it. The variants are compared with each one's own `null` set aside, so the position is
/// the conjunct, `Option` because a branch admits `null`, and not an enum of two identical
/// `Box<C>` variants.
#[test]
fn a_ref_beside_an_any_of_whose_branches_differ_only_in_null_collapses_to_the_conjunct() {
    let mut mismatches = Vec::new();
    for stated_nothing in ["{}", "true"] {
        let union = format!("anyOf: [ {stated_nothing}, {{ type: array }} ]");
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    C: {{ type: [array, 'null'], items: {{ type: string }} }}\n    Pick: {{ $ref: \
                 '#/components/schemas/C', {union} }}\n    Holder:\n      type: object\n      \
                 properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                 required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let collapsed = messages_for(&report, Code::ValidationKeywordIgnored)
            .into_iter()
            .any(|message| message.contains("differ only in keywords"));
        if report.outcome() == Outcome::Rejected
            || !collapsed
            || !enum_variants(&types, "Pick").is_empty()
            || alias_target(&types, "Pick").as_deref() != Some("Vec<Citem>")
            || field_type(&types, "pub pick").as_deref() != Some("Option<Pick>")
        {
            mismatches.push(format!(
                "{union}: not collapsed to `C`: {report:#?}\n{types}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
