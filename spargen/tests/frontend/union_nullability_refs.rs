//! How an `allOf` counts the branches `null` matches in a `oneOf` it reaches only through `$ref`
//! members: a bare `$ref` to the `oneOf` component, to an `allOf` wrapping it, or to a reuse graph
//! of such wrappers, answers as the inline spelling does, and reads each target once.

use super::*;

/// A bare `$ref` member to a `oneOf` component that `null` matches twice (`[ {}, {} ]`, `[ {}, {
/// type: 'null' } ]`), or to an `allOf` wrapping one, inline or as a bare `$ref` member of its own
/// (`U: { allOf: [ { $ref: V } ] }`), denies `null` beside `type: [string, 'null']` in either
/// member order, as the inline spelling does (#673): `pub pick: Pick`, not `Option<Pick>`.
#[test]
fn a_ref_member_to_a_oneof_null_matches_twice_denies_null_beside_a_nullable_type() {
    let mut mismatches = Vec::new();
    for members in ["[ {}, {} ]", "[ {}, { type: 'null' } ]"] {
        let ty = "{ type: [string, 'null'] }";
        let union = "{ $ref: '#/components/schemas/U' }";
        for body in [
            format!("{{ oneOf: {members} }}"),
            format!("{{ allOf: [ {{ oneOf: {members} }} ] }}"),
            format!(
                "{{ allOf: [ {{ $ref: '#/components/schemas/V' }} ] }}\n    V: {{ oneOf: {members} }}"
            ),
        ] {
            for site in [
                format!("{{ allOf: [ {ty}, {union} ] }}"),
                format!("{{ allOf: [ {union}, {ty} ] }}"),
            ] {
                let spec = with_schemas(
                    "3.1.0",
                    &format!(
                        "    U: {body}\n    Pick: {site}\n    Holder:\n      type: object\n      \
                         properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      \
                         required: [pick]\n"
                    ),
                );
                let (report, code) = generate_with_code(&spec);
                let types = types_module(&code);
                if report.outcome() == Outcome::Rejected
                    || field_type(&types, "pub pick").as_deref() != Some("Pick")
                    || !types.contains("pub type Pick = String;")
                {
                    mismatches.push(format!(
                        "U: {body}, Pick: {site}: not `Pick = String` as `Pick`: {report:#?} \
                         {types}"
                    ));
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Reading whether an `allOf` holds a `oneOf` that `null` matches twice (#673) is done once per
/// member target, not once per path. `C<i>` is `allOf: [$ref C<i-1>, $ref C<i-1>]` over `C0: {
/// oneOf: [ {}, {} ] }`, so the paths to it number 2^30; re-reading each target at every `$ref`
/// would not finish at this depth. The answer is still the `oneOf`'s, replayed: `null` fails it,
/// so `Pick` denies the `type` member's `null`.
#[test]
fn a_ref_member_to_a_branching_allof_graph_over_a_oneof_null_matches_twice_reads_each_target_once()
{
    const DEPTH: usize = 30;
    let mut defs = String::from("    C0: { oneOf: [ {}, {} ] }\n");
    for level in 1..=DEPTH {
        let prev = level - 1;
        defs.push_str(&format!(
            "    C{level}: {{ allOf: [ {{ $ref: '#/components/schemas/C{prev}' }}, {{ $ref: \
             '#/components/schemas/C{prev}' }} ] }}\n"
        ));
    }
    let spec = with_schemas(
        "3.1.0",
        &format!(
            "    Pick: {{ allOf: [ {{ type: [string, 'null'] }}, {{ $ref: \
             '#/components/schemas/C{DEPTH}' }} ] }}\n{defs}    Holder:\n      type: object\n      \
             properties:\n        pick: {{ $ref: '#/components/schemas/Pick' }}\n      required: \
             [pick]\n"
        ),
    );
    let (report, code) = generate_with_code(&spec);
    for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    let types = types_module(&code);
    let pick = field_type(&types, "pub pick").unwrap_or_else(|| panic!("no `pick` field: {types}"));
    assert_eq!(pick, "Pick", "{types}");
}
