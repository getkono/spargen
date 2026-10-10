//! How a union counts the branches `null` matches: a `oneOf` admits `null` only where exactly one
//! branch does, including branches whose own keywords leave `null` undecided.

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
