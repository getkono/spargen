//! Which categories a union's branches may take: a multi-category `type` array beside a
//! `oneOf`/`anyOf` restricts them to the categories it names, in the sibling spelling as in the
//! `allOf` one (#677).

use super::*;

/// A multi-category `type` array beside a `oneOf`/`anyOf` with no other keyword restricts the
/// union's branches to the categories it names, in the sibling spelling as in the `allOf` one: a
/// branch of a category it omits is excluded with `W011`, and an `items` branch beside
/// `[string, integer, 'null']` meets it in `null` alone, so `Pick` is `()`. The union the array
/// synthesizes for the meet is no type of the output.
#[test]
fn a_multi_category_type_array_beside_a_union_excludes_the_branches_it_omits() {
    let excluded = |index: usize| {
        format!(
            "union member {index} cannot satisfy the enclosing schema's own constraints, so it \
             is not a variant of the generated enum"
        )
    };
    let rows: [(&str, &str, &str, Vec<String>, &str); 4] = [
        (
            "type: [string, integer]",
            "oneOf",
            "{ type: boolean }, { type: string }",
            vec![excluded(0)],
            "String",
        ),
        (
            "type: [string, integer, 'null']",
            "anyOf",
            "{ items: { type: string } }",
            Vec::new(),
            "()",
        ),
        (
            "type: [boolean, integer]",
            "oneOf",
            "{ type: string }, { type: boolean }",
            vec![excluded(0)],
            "bool",
        ),
        (
            "type: [string, integer]",
            "anyOf",
            "{ type: string }, { type: array, items: { type: string } }",
            vec![excluded(1)],
            "String",
        ),
    ];
    let mut mismatches = Vec::new();
    for (conjunct, keyword, branches, excluded, expected) in &rows {
        for site in [
            format!("{{ {conjunct}, {keyword}: [ {branches} ] }}"),
            format!("{{ allOf: [ {{ {conjunct} }}, {{ {keyword}: [ {branches} ] }} ] }}"),
        ] {
            let spec = with_schemas("3.1.0", &format!("    Pick: {site}\n"));
            let (report, code) = generate_with_code(&spec);
            let types = types_module(&code);
            let pick = alias_target(&types, "Pick");
            if pick.as_deref() != Some(*expected) {
                mismatches.push(format!("{site}: `Pick` is {pick:?}, not `{expected}`"));
            }
            let warned: Vec<String> = messages_for(&report, Code::DeclarationHasNoEffect)
                .into_iter()
                .filter(|message| message.starts_with("union member "))
                .map(str::to_owned)
                .collect();
            if &warned != excluded {
                mismatches.push(format!("{site}: excluded {warned:?}, not {excluded:?}"));
            }
            if types.contains("PickConstraint") {
                mismatches.push(format!(
                    "{site}: the synthesized union is emitted:\n{types}"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A cycle-closing `$ref` branch beside a multi-category `type` array alone: one of a category the
/// array admits meets it in itself and generates, as
/// `a_cycle_closing_ref_branch_to_an_untyped_object_takes_the_type_arrays_null` pins; one the
/// array omits, or whose target's category cannot be read, cannot be met with a target that is
/// still a reservation, and is `E013`, as in the `allOf` spelling.
#[test]
fn a_cycle_closing_branch_a_multi_category_type_array_omits_is_rejected() {
    for (shape, holder, child) in [
        (
            "an object target beside an array without `object`",
            "properties: { child: { %CHILD% } }",
            "type: [string, integer, 'null'], oneOf: [ { $ref: '#/components/schemas/Holder' }, \
             { type: string } ]",
        ),
        (
            "the same set in the `allOf` spelling",
            "properties: { child: { %CHILD% } }",
            "allOf: [ { type: [string, integer, 'null'] }, { oneOf: [ { $ref: \
             '#/components/schemas/Holder' }, { type: string } ] } ]",
        ),
        (
            "a target of object and array keywords alike",
            "items: { type: string }, properties: { child: { %CHILD% } }",
            "type: [object, array, 'null'], oneOf: [ { $ref: '#/components/schemas/Holder' }, { \
             type: integer } ]",
        ),
    ] {
        let holder = holder.replace("%CHILD%", child);
        let spec = with_schemas(
            "3.1.0",
            &format!("    Holder: {{ {holder}, required: [child] }}\n"),
        );
        let report = generate(&spec);
        assert_eq!(codes(&report), ["E013"], "{shape}: {report:#?}");
    }
}
