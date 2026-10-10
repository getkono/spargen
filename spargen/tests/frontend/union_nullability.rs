//! How a union counts the branches `null` matches: a `oneOf` admits `null` only where exactly one
//! branch does, including branches whose own keywords leave `null` undecided; and where an `allOf`
//! hands `null` to a `$ref` union member, or to one of several inline union members, whose
//! branches leave it undecided.

use super::*;

/// A `oneOf` whose branch is itself a union that `null` matches through an untyped inner branch
/// (`properties` or `items` with no `type`), beside a branch that states `null`: `null` matches
/// both, which fails the exactly-one rule, and the union admits no `null` (#628). That holds on
/// the multi-variant path (a nullable scalar branch, or a `null` branch beside a typed one) and on
/// the sole-member path (a `null` branch alone beside it), for an inner `oneOf` or `anyOf`, and for
/// the inner union written as a `$ref` to a union component. An inner union whose branches decide
/// `null` (a typed object), or that `null` matches twice and so not at all, leaves `null` to the
/// outer `null` branch alone, and an outer `anyOf` needs only one match: those stay `Option`.
#[test]
fn a_oneof_nested_union_branch_admitting_null_beside_a_null_branch_admits_no_null() {
    let inner = "{ oneOf: [ { properties: { a: { type: string } } }, { type: string } ] }";
    let mut mismatches = Vec::new();
    for (site, nullable) in [
        (
            format!("{{ oneOf: [ {inner}, {{ type: [integer, 'null'] }} ] }}"),
            false,
        ),
        (
            format!("{{ oneOf: [ {inner}, {{ type: integer }}, {{ type: 'null' }} ] }}"),
            false,
        ),
        (
            format!("{{ oneOf: [ {inner}, {{ type: 'null' }} ] }}"),
            false,
        ),
        (
            format!("{{ oneOf: [ {{ type: 'null' }}, {inner} ] }}"),
            false,
        ),
        (
            "{ oneOf: [ { anyOf: [ { items: { type: string } }, { type: string } ] }, { type: \
             [integer, 'null'] } ] }"
                .to_owned(),
            false,
        ),
        (
            "{ oneOf: [ { anyOf: [ { items: { type: string } }, { type: string } ] }, { type: \
             'null' } ] }"
                .to_owned(),
            false,
        ),
        // The inner union written as a `$ref` to a union component is the same branch.
        (
            "{ oneOf: [ { $ref: '#/components/schemas/Inner' }, { type: [integer, 'null'] } ] }"
                .to_owned(),
            false,
        ),
        (
            "{ oneOf: [ { $ref: '#/components/schemas/Inner' }, { type: 'null' } ] }".to_owned(),
            false,
        ),
        // A typed inner object denies `null`, so it matches the outer `null` branch alone.
        (
            "{ oneOf: [ { oneOf: [ { type: object, properties: { a: { type: string } } }, { \
             type: string } ] }, { type: [integer, 'null'] } ] }"
                .to_owned(),
            true,
        ),
        (
            "{ oneOf: [ { oneOf: [ { type: object, properties: { a: { type: string } } }, { \
             type: string } ] }, { type: 'null' } ] }"
                .to_owned(),
            true,
        ),
        // `null` matches both untyped inner branches, so the inner `oneOf` rejects it.
        (
            "{ oneOf: [ { oneOf: [ { properties: { a: { type: string } } }, { items: { type: \
             string } } ] }, { type: [integer, 'null'] } ] }"
                .to_owned(),
            true,
        ),
        (
            format!("{{ anyOf: [ {inner}, {{ type: [integer, 'null'] }} ] }}"),
            true,
        ),
        (
            format!("{{ anyOf: [ {inner}, {{ type: 'null' }} ] }}"),
            true,
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    Inner: {inner}\n    Holder:\n      type: object\n      \
                 properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                 required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != nullable {
            let validity = if nullable { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `oneOf` branch that is a nested union lowering to `Value` (`{anyOf: [true]}`,
/// `{oneOf: [{}]}`), met with a conjunct that admits `null` (an `allOf` member or a `$ref` sibling
/// naming `type: [object, 'null']`), takes the conjunct's `null` as a branch that states nothing,
/// and is counted once for it: beside a branch that denies `null` it is the one branch `null`
/// matches, so the union keeps `null`; beside a branch that states `null` too, `null` matches two
/// branches and the union admits none.
#[test]
fn a_oneof_nested_value_union_branch_met_with_a_null_conjunct_counts_null_once() {
    let mut mismatches = Vec::new();
    for branch in ["{ anyOf: [ true ] }", "{ oneOf: [ {} ] }"] {
        for (others, nullable) in [
            ("{ type: integer }", true),
            ("{ type: [integer, 'null'] }", false),
        ] {
            for site in [
                format!(
                    "{{ allOf: [ {{ $ref: '#/components/schemas/NB' }} ], oneOf: [ {branch}, \
                     {others} ] }}"
                ),
                format!("{{ $ref: '#/components/schemas/NB', oneOf: [ {branch}, {others} ] }}"),
            ] {
                let spec = with_schemas(
                    "3.1.0",
                    &format!(
                        "    Pick: {site}\n    NB: {{ type: [object, 'null'] }}\n    Holder:\n      \
                         type: object\n      properties:\n        pick: {{ $ref: \
                         '#/components/schemas/Pick' }}\n      required: [pick]\n"
                    ),
                );
                let (report, code) = generate_with_code(&spec);
                assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
                let types = types_module(&code);
                let pick = field_type(&types, "pub pick")
                    .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
                if pick.starts_with("Option<") != nullable {
                    let validity = if nullable { "valid" } else { "invalid" };
                    mismatches.push(format!(
                        "{site}: `pick` is `{pick}`, but `null` is {validity} here"
                    ));
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Two or more inline union members of an `allOf` beside a member that admits `null` take it as
/// the same members written as `$ref`s to union components do (#624, #631): each union's untyped
/// `items` branches leave `null` undecided, so the merge admits it where the other member does.
/// Two such unions alone, inline or one of each spelling, decide nothing for each other and stay
/// non-null; and the union's own `type`, or the `allOf` schema's, excluding `null` withholds it.
#[test]
fn several_inline_union_allof_members_take_null_from_a_member_admitting_it() {
    let any_of_a = "{ anyOf: [ { items: { type: string } }, { items: { type: integer } } ] }";
    let any_of_b = "{ anyOf: [ { items: { type: boolean } }, { items: { type: number } } ] }";
    let typed_b =
        "{ type: array, anyOf: [ { items: { type: boolean } }, { items: { type: number } } ] }";
    let union_ref = "{ $ref: '#/components/schemas/ArrayAnyOf' }";
    let mut mismatches = Vec::new();
    for (site, nullable) in [
        (
            format!("{{ allOf: [ {{ type: [array, 'null'] }}, {any_of_a}, {any_of_b} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {any_of_a}, {{ type: [array, 'null'] }}, {any_of_b} ] }}"),
            true,
        ),
        (
            format!("{{ allOf: [ {{ type: [array, 'null'] }}, {any_of_a}, {union_ref} ] }}"),
            true,
        ),
        (format!("{{ allOf: [ {any_of_a}, {any_of_b} ] }}"), false),
        (format!("{{ allOf: [ {any_of_a}, {union_ref} ] }}"), false),
        (
            format!("{{ allOf: [ {{ type: [array, 'null'] }}, {any_of_a}, {typed_b} ] }}"),
            false,
        ),
        (
            format!(
                "{{ type: array, allOf: [ {{ type: [array, 'null'] }}, {any_of_a}, {any_of_b} ] }}"
            ),
            false,
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    ArrayAnyOf: {any_of_b}\n    Holder:\n      type: object\n      \
                 properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                 required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != nullable {
            let validity = if nullable { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// An `allOf` `$ref` member to a union whose branches leave `null` undecided takes `null` from a
/// member that admits it (#624, #630) only where `null` can satisfy the merge at all. The own
/// `type`, `enum` or `const` of the `allOf` schema, or of an inner `allOf` flattened into it,
/// contributes nothing to the meet, yet constrains every value of it: one that excludes `null`
/// leaves the meet non-null, wherever the `$ref` member sits. Own keywords that admit `null` leave
/// it to the members, so the nested `$ref` still takes it there.
#[test]
fn flattened_own_keywords_excluding_null_withhold_it_from_a_ref_union_member() {
    let union = "{ $ref: '#/components/schemas/ArrayAnyOf' }";
    let mut mismatches = Vec::new();
    for (site, nullable) in [
        (
            format!(
                "{{ allOf: [ {{ type: [array, 'null'] }}, {{ type: array, allOf: [ {union} ] }} \
                 ] }}"
            ),
            false,
        ),
        (
            format!(
                "{{ allOf: [ {{ type: [array, 'null'] }}, {{ enum: [[1]], allOf: [ {union} ] }} \
                 ] }}"
            ),
            false,
        ),
        (
            format!("{{ type: array, allOf: [ {{ type: [array, 'null'] }}, {union} ] }}"),
            false,
        ),
        (
            format!(
                "{{ allOf: [ {{ type: [array, 'null'] }}, {union}, {{ type: array, allOf: [ {{ \
                 items: {{}} }} ] }} ] }}"
            ),
            false,
        ),
        (
            format!(
                "{{ allOf: [ {{ type: [array, 'null'] }}, {{ type: [array, 'null'], allOf: [ \
                 {union} ] }} ] }}"
            ),
            true,
        ),
        (
            format!("{{ type: [array, 'null'], allOf: [ {{ type: [array, 'null'] }}, {union} ] }}"),
            true,
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    ArrayAnyOf: {{ anyOf: [ {{ items: {{ type: string }} }}, \
                 {{ items: {{ type: integer }} }} ] }}\n    Holder:\n      type: object\n      \
                 properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                 required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != nullable {
            let validity = if nullable { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A nullable scalar `type` beside an untyped `items` (or untyped object) `oneOf` is rejected as
/// its `allOf` spelling is, the `type` written as a member beside the union (#632). Spargen reads an
/// untyped `items` branch as array-only (#614), so the scalar meets each branch in `null` alone:
/// `null` then matches every branch, which fails the `oneOf`'s exactly-one rule, and no value
/// satisfies the schema. The sibling spelling merged the `()` variants with `W001` and generated the
/// null type for it. A `null` member beside a branch met in `null` alone is the same empty
/// intersection, and `const: null` or `enum: [null]` meets an untyped branch in `null` as
/// `type: 'null'` does, where it excluded every branch with `E007` instead. With one branch excluded
/// (`W011`) and one met in `null`, `null` matches that branch alone, and both spellings stay the
/// null type, as an `anyOf` met in `null` is.
#[test]
fn a_nullable_scalar_type_beside_an_untyped_oneof_is_rejected_as_its_allof_spelling() {
    let mut mismatches = Vec::new();
    for (sibling, all_of, satisfiable) in [
        (
            "{ type: [string, 'null'], oneOf: [ { items: { type: string } }, { items: { type: \
             integer } } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { oneOf: [ { items: { type: string } }, { \
             items: { type: integer } } ] } ] }",
            false,
        ),
        (
            "{ type: 'null', oneOf: [ { items: { type: string } }, { items: { type: integer } } \
             ] }",
            "{ allOf: [ { type: 'null' }, { oneOf: [ { items: { type: string } }, { items: { \
             type: integer } } ] } ] }",
            false,
        ),
        (
            "{ type: [string, 'null'], oneOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { oneOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
            false,
        ),
        (
            "{ type: [integer, 'null'], oneOf: [ { items: { type: string } }, { type: string } ] }",
            "{ allOf: [ { type: [integer, 'null'] }, { oneOf: [ { items: { type: string } }, { \
             type: string } ] } ] }",
            true,
        ),
        // A `null` member is a branch `null` matches beside the one the meet narrows to `null`.
        (
            "{ type: [string, 'null'], oneOf: [ { type: 'null' }, { items: { type: string } } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { oneOf: [ { type: 'null' }, { items: { \
             type: string } } ] } ] }",
            false,
        ),
        (
            "{ type: [string, 'null'], oneOf: [ { type: 'null' }, { items: { type: string } }, { \
             type: integer } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { oneOf: [ { type: 'null' }, { items: { \
             type: string } }, { type: integer } ] } ] }",
            false,
        ),
        // `const: null` and `enum: [null]` permit `null` to an untyped branch as `type: 'null'`
        // does.
        (
            "{ const: null, oneOf: [ { items: { type: string } }, { items: { type: integer } } ] }",
            "{ allOf: [ { const: null }, { oneOf: [ { items: { type: string } }, { items: { \
             type: integer } } ] } ] }",
            false,
        ),
        (
            "{ enum: [null], oneOf: [ { items: { type: string } }, { items: { type: integer } } \
             ] }",
            "{ allOf: [ { enum: [null] }, { oneOf: [ { items: { type: string } }, { items: { \
             type: integer } } ] } ] }",
            false,
        ),
        (
            "{ const: null, oneOf: [ { items: { type: string } }, { type: string } ] }",
            "{ allOf: [ { const: null }, { oneOf: [ { items: { type: string } }, { type: string \
             } ] } ] }",
            true,
        ),
        (
            "{ const: null, anyOf: [ { items: { type: string } }, { items: { type: integer } } ] }",
            "{ allOf: [ { const: null }, { anyOf: [ { items: { type: string } }, { items: { \
             type: integer } } ] } ] }",
            true,
        ),
    ] {
        for site in [sibling, all_of] {
            let spec = with_schemas(
                "3.1.0",
                &format!(
                    "    Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                     pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n"
                ),
            );
            let (report, code) = generate_with_code(&spec);
            if satisfiable {
                let types = types_module(&code);
                let pick = field_type(&types, "pub pick");
                if report.outcome() == Outcome::Rejected
                    || !types.contains("pub type Pick = ();")
                    || pick.as_deref() != Some("Pick")
                {
                    mismatches.push(format!(
                        "{site}: only `null` satisfies it, but it is not the null type: \
                         {report:#?} {types}"
                    ));
                }
            } else if report.outcome() != Outcome::Rejected
                || !has_code(&report, Code::AllOfIrreconcilable)
            {
                mismatches.push(format!(
                    "{site}: nothing satisfies it, but it is not rejected with `E013`: {report:#?}"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
