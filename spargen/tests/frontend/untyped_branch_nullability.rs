//! How `null` reaches a union through an untyped applicator branch (`properties`, `items` or
//! `prefixItems` with no `type`), which leaves `null` undecided: the branches a `oneOf` counts
//! `null` against, inline and through a cycle-closing `$ref`, and the `allOf` meets and sibling
//! `type`s beside such a union.

use super::*;

/// An untyped `items` or `prefixItems` union branch, which now lowers to an array (#614), still
/// leaves `null` undecided, as its `Value` did and as the untyped object branch beside it does
/// (#574, #567): it takes the `null` a `type` array or a nullable `$ref` target permits, and
/// counts as a branch `null` matches. Read as a typed array instead, it dropped that `null`, and a
/// required field of the union went from `Option<Pick>` to `Pick`. Each row is pinned in its array
/// and its untyped object spelling, which must agree.
#[test]
fn an_untyped_array_applicator_branch_leaves_null_undecided() {
    let mut mismatches = Vec::new();
    for (array, object, nullable) in [
        (
            "{ type: [array, 'null'], anyOf: [ { items: { type: string } }, { items: { type: \
             integer } } ] }",
            "{ type: [object, 'null'], anyOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] }",
            true,
        ),
        (
            "{ type: [array, 'null'], anyOf: [ { items: { type: string } }, { type: integer } ] }",
            "{ type: [object, 'null'], anyOf: [ { properties: { a: { type: string } } }, { type: \
             integer } ] }",
            true,
        ),
        (
            "{ type: [array, 'null'], oneOf: [ { prefixItems: [ { type: string } ] } ] }",
            "{ type: [object, 'null'], oneOf: [ { required: [a] } ] }",
            true,
        ),
        // `null` matches both untyped branches, which fails the `oneOf`'s exactly-one.
        (
            "{ type: [array, 'null'], oneOf: [ { items: { type: string } }, { items: { type: \
             integer } } ] }",
            "{ type: [object, 'null'], oneOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] }",
            false,
        ),
        (
            "{ type: [array, string, 'null'], oneOf: [ { items: { type: string } }, { type: \
             string } ] }",
            "{ type: [object, string, 'null'], oneOf: [ { properties: { a: { type: string } } }, \
             { type: string } ] }",
            true,
        ),
        // A nullable `$ref` target the union is met with permits `null`, and the untyped branches
        // take it from the meet.
        (
            "{ $ref: '#/components/schemas/NullableArray', anyOf: [ { items: { type: string } }, \
             { items: { type: integer } } ] }",
            "{ $ref: '#/components/schemas/NullableObject', anyOf: [ { properties: { a: { type: \
             string } } }, { properties: { b: { type: integer } } } ] }",
            true,
        ),
        // The same conjunction with the nullable category written as an `allOf` member beside
        // the union member (#621).
        (
            "{ allOf: [ { type: [array, 'null'] }, { anyOf: [ { items: { type: string } }, { \
             items: { type: integer } } ] } ] }",
            "{ allOf: [ { type: [object, 'null'] }, { anyOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
            true,
        ),
        (
            "{ allOf: [ { type: [array, 'null'] }, { oneOf: [ { items: { type: string } }, { \
             items: { type: integer } } ] } ] }",
            "{ allOf: [ { type: [object, 'null'] }, { oneOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
            false,
        ),
        // The same union member written as a `$ref` to a union component (#624). Its object
        // spelling, a `$ref` to an object union beside an object member, is still rejected as an
        // object/scalar mix (#491), so the inline object spelling stands beside it.
        (
            "{ allOf: [ { type: [array, 'null'] }, { $ref: '#/components/schemas/ArrayAnyOf' } ] }",
            "{ allOf: [ { type: [object, 'null'] }, { anyOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
            true,
        ),
        (
            "{ allOf: [ { type: [array, 'null'] }, { $ref: '#/components/schemas/ArrayOneOf' } ] }",
            "{ allOf: [ { type: [object, 'null'] }, { oneOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
            false,
        ),
        // The same `$ref` member nested in an inner `allOf`, which flattens into the outer (#630).
        (
            "{ allOf: [ { type: [array, 'null'] }, { allOf: [ { $ref: \
             '#/components/schemas/ArrayAnyOf' } ] } ] }",
            "{ allOf: [ { type: [array, 'null'] }, { $ref: '#/components/schemas/ArrayAnyOf' } ] }",
            true,
        ),
        (
            "{ allOf: [ { type: array }, { $ref: '#/components/schemas/ArrayAnyOf' } ] }",
            "{ allOf: [ { type: object }, { anyOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] } ] }",
            false,
        ),
        // Two such `$ref` union members, or the same one twice: each leaves `null` undecided, so
        // neither decides it for the other, and the meet stays non-null as the inline spelling
        // does. Beside a member that admits `null`, both take it.
        (
            "{ allOf: [ { $ref: '#/components/schemas/ArrayAnyOf' }, { $ref: \
             '#/components/schemas/ArrayAnyOfB' } ] }",
            "{ allOf: [ { anyOf: [ { properties: { a: { type: string } } }, { properties: { b: { \
             type: integer } } } ] }, { anyOf: [ { properties: { c: { type: string } } }, { \
             properties: { d: { type: integer } } } ] } ] }",
            false,
        ),
        (
            "{ allOf: [ { $ref: '#/components/schemas/ArrayAnyOf' }, { $ref: \
             '#/components/schemas/ArrayAnyOf' } ] }",
            "{ allOf: [ { anyOf: [ { items: { type: string } }, { items: { type: integer } } ] }, \
             { anyOf: [ { items: { type: boolean } }, { items: { type: number } } ] } ] }",
            false,
        ),
        (
            "{ allOf: [ { type: [array, 'null'] }, { $ref: '#/components/schemas/ArrayAnyOf' }, { \
             $ref: '#/components/schemas/ArrayAnyOfB' } ] }",
            "{ allOf: [ { type: [array, 'null'] }, { $ref: '#/components/schemas/ArrayAnyOf' }, { \
             $ref: '#/components/schemas/ArrayAnyOf' } ] }",
            true,
        ),
        (
            "{ allOf: [ { type: array }, { anyOf: [ { items: { type: string } }, { items: { type: \
             integer } } ] } ] }",
            "{ allOf: [ { type: object }, { anyOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] } ] }",
            false,
        ),
    ] {
        for site in [array, object] {
            let spec = with_schemas(
                "3.1.0",
                &format!(
                    "    Pick: {site}\n    NullableArray: {{ type: [array, 'null'] }}\n    \
                     NullableObject: {{ type: [object, 'null'] }}\n    ArrayAnyOf: {{ anyOf: \
                     [ {{ items: {{ type: string }} }}, {{ items: {{ type: integer }} }} ] }}\n    \
                     ArrayOneOf: {{ oneOf: [ {{ items: {{ type: string }} }}, {{ items: {{ type: \
                     integer }} }} ] }}\n    ArrayAnyOfB: {{ anyOf: [ {{ items: {{ type: \
                     boolean }} }}, {{ items: {{ type: number }} }} ] }}\n    Holder:\n      type: \
                     object\n      properties:\n        pick: {{ $ref: \
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
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `oneOf` of an untyped object or array branch (`properties` or `items` with no `type`) and a
/// branch that states `null`: the untyped branch constrains objects or arrays alone, so `null`
/// matches it as well as the `null` branch, which fails the exactly-one rule, and the union admits
/// no `null` (#622), with or without a sibling keyword of the branch's category beside it, and
/// beside a `null` member with or without a typed branch beside the two, and spelled as a `$ref`
/// to a component with no `type`, discriminated or not. A typed
/// object or array branch denies `null`, which then matches the `null` branch alone, and an
/// `anyOf` needs only one match, so both of those stay `Option`.
#[test]
fn a_oneof_untyped_applicator_branch_beside_a_null_branch_admits_no_null() {
    let mut mismatches = Vec::new();
    for (site, nullable) in [
        (
            "{ oneOf: [ { properties: { a: { type: string } } }, { type: [integer, 'null'] } ] }",
            false,
        ),
        (
            "{ maxProperties: 3, oneOf: [ { properties: { a: { type: string } } }, { type: \
             [integer, 'null'] } ] }",
            false,
        ),
        (
            "{ oneOf: [ { items: { type: string } }, { type: [integer, 'null'] } ] }",
            false,
        ),
        (
            "{ minItems: 1, oneOf: [ { items: { type: string } }, { type: [integer, 'null'] } ] }",
            false,
        ),
        (
            "{ oneOf: [ { properties: { a: { type: string } } }, { type: integer }, { type: \
             'null' } ] }",
            false,
        ),
        (
            "{ oneOf: [ { properties: { a: { type: string } } }, { type: 'null' } ] }",
            false,
        ),
        (
            "{ oneOf: [ { items: { type: string } }, { type: 'null' } ] }",
            false,
        ),
        (
            "{ oneOf: [ { type: object, properties: { a: { type: string } } }, { type: [integer, \
             'null'] } ] }",
            true,
        ),
        (
            "{ oneOf: [ { type: array, items: { type: string } }, { type: [integer, 'null'] } ] }",
            true,
        ),
        (
            "{ anyOf: [ { properties: { a: { type: string } } }, { type: [integer, 'null'] } ] }",
            true,
        ),
        (
            "{ anyOf: [ { items: { type: string } }, { type: [integer, 'null'] } ] }",
            true,
        ),
        // A `$ref` branch whose target component has no `type` is the same untyped branch.
        (
            "{ oneOf: [ { $ref: '#/components/schemas/UntypedObject' }, { type: [integer, \
             'null'] } ] }",
            false,
        ),
        (
            "{ oneOf: [ { $ref: '#/components/schemas/UntypedArray' }, { type: [integer, \
             'null'] } ] }",
            false,
        ),
        (
            "{ oneOf: [ { type: 'null' }, { $ref: '#/components/schemas/UntypedObject' } ] }",
            false,
        ),
        (
            "{ oneOf: [ { $ref: '#/components/schemas/UntypedObject' }, { type: integer }, { \
             type: 'null' } ] }",
            false,
        ),
        // `null` carries no tag, so a discriminator does not exempt the count.
        (
            "{ oneOf: [ { $ref: '#/components/schemas/UntypedTagA' }, { $ref: \
             '#/components/schemas/UntypedTagB' }, { type: 'null' } ], discriminator: { \
             propertyName: kind } }",
            false,
        ),
        // A `$ref` to a typed object component denies `null`, as an inline typed branch does.
        (
            "{ oneOf: [ { type: 'null' }, { $ref: '#/components/schemas/TypedObject' } ] }",
            true,
        ),
        (
            "{ anyOf: [ { $ref: '#/components/schemas/UntypedObject' }, { type: [integer, \
             'null'] } ] }",
            true,
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: [pick]\n    \
                 UntypedObject: {{ properties: {{ a: {{ type: string }} }} }}\n    \
                 UntypedArray: {{ items: {{ type: string }} }}\n    \
                 UntypedTagA: {{ required: [kind], properties: {{ kind: {{ type: string }}, a: \
                 {{ type: string }} }} }}\n    \
                 UntypedTagB: {{ required: [kind], properties: {{ kind: {{ type: string }}, b: \
                 {{ type: integer }} }} }}\n    \
                 TypedObject: {{ type: object, properties: {{ a: {{ type: string }} }} }}\n"
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

/// The rows of [`a_oneof_untyped_applicator_branch_beside_a_null_branch_admits_no_null`] whose
/// `$ref` branch names an untyped component, written inside that component, where the `$ref`
/// closes a cycle and lowers to the component's still-open reservation (#627): `Node.next` and
/// `Holder.pick` carry the same union, and `null` is as invalid in one as in the other. A `oneOf`
/// beside a `null` branch admits no `null`, with or without a typed branch beside the two, and so
/// does one whose `$ref` names a component whose `type` lists `null`, which the reservation path
/// counted no more than the untyped one; an `anyOf`, or a `$ref` to a non-null typed component,
/// stays `Option` in both places.
#[test]
fn a_cycle_closing_untyped_ref_branch_beside_a_null_branch_matches_the_same_union_outside_it() {
    let mut mismatches = Vec::new();
    for (target, site, nullable) in [
        (
            "Node",
            "{ oneOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }",
            false,
        ),
        (
            "Node",
            "{ oneOf: [ { type: 'null' }, { $ref: '#/components/schemas/Node' } ] }",
            false,
        ),
        (
            "Node",
            "{ oneOf: [ { $ref: '#/components/schemas/Node' }, { type: integer }, { type: \
             'null' } ] }",
            false,
        ),
        (
            "Node",
            "{ anyOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }",
            true,
        ),
        (
            "TypedNode",
            "{ oneOf: [ { $ref: '#/components/schemas/TypedNode' }, { type: 'null' } ] }",
            true,
        ),
        (
            "TypedNode",
            "{ oneOf: [ { $ref: '#/components/schemas/TypedNode' }, { type: integer }, { type: \
             'null' } ] }",
            true,
        ),
        (
            "NullableNode",
            "{ oneOf: [ { $ref: '#/components/schemas/NullableNode' }, { type: 'null' } ] }",
            false,
        ),
    ] {
        let typed = match target {
            "TypedNode" => "type: object, ",
            "NullableNode" => "type: [object, 'null'], ",
            _ => "",
        };
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    {target}: {{ {typed}properties: {{ a: {{ type: string }}, next: {site} }}, \
                 required: [next] }}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {site}\n      required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let types = types_module(&code);
        for field in ["pub next", "pub pick"] {
            let ty = field_type(&types, field)
                .unwrap_or_else(|| panic!("{site}: no `{field}` field: {types}"));
            if ty.starts_with("Option<") != nullable {
                let validity = if nullable { "valid" } else { "invalid" };
                mismatches.push(format!(
                    "{site}: `{field}` is `{ty}`, but `null` is {validity} here"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// An `allOf` of a scalar member listing `null` beside a union whose untyped `items` branches
/// leave `null` undecided, where the scalar's other category meets no branch: the union takes
/// `null` from that member (#621), so `null` is the one value left and the schema is the null
/// type, as any other null-only `allOf` meet is, where it was `E013` before. Without `null` in
/// the scalar member nothing is left, and that stays `E013`. The union written as a `$ref` to a
/// union component takes `null` the same way (#624).
#[test]
fn an_allof_scalar_meeting_an_untyped_items_union_only_at_null_is_the_null_type() {
    let items_union = "{ anyOf: [ { items: { type: string } }, { items: { type: integer } } ] }";
    let document = |site: &str| {
        with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {site}\n    ItemsUnion: {items_union}\n    Holder:\n      type: \
                 object\n      properties:\n        pick: {{ $ref: '#/components/schemas/Pick' \
                 }}\n      required: [pick]\n"
            ),
        )
    };
    for site in [
        format!("{{ allOf: [ {{ type: [string, 'null'] }}, {items_union} ] }}"),
        format!("{{ allOf: [ {{ type: 'null' }}, {items_union} ] }}"),
        format!("{{ allOf: [ {{ const: null }}, {items_union} ] }}"),
        "{ allOf: [ { type: [integer, 'null'] }, { anyOf: [ { items: { type: string } }, { type: \
         string } ] } ] }"
            .to_owned(),
        "{ allOf: [ { type: [string, 'null'] }, { $ref: '#/components/schemas/ItemsUnion' } ] }"
            .to_owned(),
    ] {
        let (report, code) = generate_with_code(&document(&site));
        assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{site}: {report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types.contains("pub type Pick = ();"),
            "{site}: only `null` satisfies both members, so `Pick` is the null type: {types}"
        );
    }

    for empty in [
        format!("{{ allOf: [ {{ type: string }}, {items_union} ] }}"),
        "{ allOf: [ { type: string }, { $ref: '#/components/schemas/ItemsUnion' } ] }".to_owned(),
    ] {
        let report = generate(&document(&empty));
        assert_eq!(report.outcome(), Outcome::Rejected, "{empty}: {report:#?}");
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "{empty}: {report:#?}"
        );
    }
}

/// A nullable scalar `type` beside an untyped `items` (or untyped object) `anyOf` lowers as its
/// `allOf` spelling does, the `type` written as a member beside the union (#625). Spargen reads an
/// untyped `items` branch as array-only (#614, and the `E013` of the non-null `allOf` spelling in
/// [`an_allof_scalar_meeting_an_untyped_items_union_only_at_null_is_the_null_type`]), so the scalar
/// meets each branch in `null` alone and both spellings are the null type, with a required field
/// of it `pub pick: Pick`. The sibling spelling was an enum of `()` variants beside a `String`
/// constraint.
#[test]
fn a_nullable_scalar_type_beside_an_untyped_union_lowers_as_its_allof_spelling() {
    let mut mismatches = Vec::new();
    for (sibling, all_of) in [
        (
            "{ type: [string, 'null'], anyOf: [ { items: { type: string } }, { items: { type: \
             integer } } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { anyOf: [ { items: { type: string } }, { \
             items: { type: integer } } ] } ] }",
        ),
        (
            "{ type: 'null', anyOf: [ { items: { type: string } }, { items: { type: integer } } \
             ] }",
            "{ allOf: [ { type: 'null' }, { anyOf: [ { items: { type: string } }, { items: { \
             type: integer } } ] } ] }",
        ),
        (
            "{ type: [string, 'null'], anyOf: [ { properties: { a: { type: string } } }, { \
             properties: { b: { type: integer } } } ] }",
            "{ allOf: [ { type: [string, 'null'] }, { anyOf: [ { properties: { a: { type: string \
             } } }, { properties: { b: { type: integer } } } ] } ] }",
        ),
        // One branch excluded (`W011`) and one met in `null`: the sole-variant path, unchanged.
        (
            "{ type: [integer, 'null'], anyOf: [ { items: { type: string } }, { type: string } ] }",
            "{ allOf: [ { type: [integer, 'null'] }, { anyOf: [ { items: { type: string } }, { \
             type: string } ] } ] }",
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
            assert_ne!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
            let types = types_module(&code);
            let pick = field_type(&types, "pub pick")
                .unwrap_or_else(|| panic!("{site}: no `pick` field: {types}"));
            if !types.contains("pub type Pick = ();") || pick != "Pick" {
                mismatches.push(format!(
                    "{site}: `pick` is `{pick}`, and `Pick` is not the null type, but only `null` \
                     satisfies it: {types}"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
