//! The spellings of an object met with `oneOf`/`anyOf` unions agree (#841): a union reached as a
//! `$ref` `allOf` member, as a union of one real branch beside `null`, or through a lowering of its
//! own reaches the outcome its other spellings do, and an `allOf` of unions alone keeps the scalar
//! meet it generated through.

use super::*;

/// The document this module's fixtures read: `Pick` beside the components `Base`, `Other`, the
/// two-branch union `U`, the one-real-branch `NullableOther`, and the `extra` component lines.
fn union_member_document(extra: &str, pick: &str) -> String {
    format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Pick' }} }}
components:
  schemas:
    Base:
      type: object
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Other:
      type: object
      required: [a]
      properties:
        c: {{ type: integer }}
    U:
      anyOf: [ {{ required: [a] }}, {{ required: [b] }} ]
    NullableOther:
      oneOf: [ {{ type: 'null' }}, {{ $ref: '#/components/schemas/Other' }} ]
{extra}    Pick: {pick}
"##
    )
}

/// The spellings of an object meeting unions that reach the meet outside the `allOf` merge agree
/// with the ones it gathers (#841). A second union reached through a lowering of its own — in a
/// `$ref`'s `allOf` sibling beside its union sibling, inline or as a `$ref` to a union component,
/// or in the union member's own `allOf` — is `E013`'s `several-unions` case, as
/// `allOf: [{$ref: Base}, {oneOf: …}, {anyOf: …}]` is; each generated a union whose variants
/// were unions of the met objects. A lone `$ref` member to a union component beside object
/// members, and a union of one real branch beside `null` written next to another union member,
/// are met branch by branch as their `$ref` spellings are, where each was an object/scalar mix.
#[test]
fn an_object_meets_the_unions_of_each_spelling_alike() {
    let branches = "[ { required: [a] }, { required: [b] } ]";
    let one = format!("{{ oneOf: {branches} }}");
    let any = format!("{{ anyOf: {branches} }}");
    let union_ref = "{ $ref: '#/components/schemas/U' }";
    let nullable_other = "{ oneOf: [ { type: 'null' }, { $ref: '#/components/schemas/Other' } ] }";
    let spec = |pick: &str| union_member_document("", pick);
    let rejected = [
        (
            "an inline union in a $ref's allOf sibling",
            format!(
                "{{ $ref: '#/components/schemas/Base', allOf: [ {any} ], oneOf: {} }}",
                branches
            ),
        ),
        (
            "a $ref to a union in a $ref's allOf sibling",
            format!(
                "{{ $ref: '#/components/schemas/Base', allOf: [ {union_ref} ], oneOf: {} }}",
                branches
            ),
        ),
        (
            "a union in the union member's own allOf",
            format!(
                "{{ allOf: [ {{ $ref: '#/components/schemas/Base' }}, {{ allOf: [ {any} ], \
                 oneOf: {} }} ] }}",
                branches
            ),
        ),
        (
            "the allOf spelling of the same conjunction",
            format!("{{ allOf: [ {{ $ref: '#/components/schemas/Base' }}, {one}, {any} ] }}"),
        ),
    ];
    let several = "more than one `oneOf`/`anyOf` union";
    for (case, pick) in &rejected {
        let spec = spec(pick);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{case} via {entry}: {report:#?}"
            );
            let at_pick: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| {
                    d.code == Code::AllOfIrreconcilable
                        && d.pointer.as_str() == "/components/schemas/Pick"
                })
                .map(|d| d.message.as_str())
                .collect();
            assert!(
                at_pick.len() == 1 && at_pick[0].contains(several),
                "{case} via {entry}: {report:#?}"
            );
        }
    }

    // Each group is one conjunction in several spellings, the first its `$ref` spelling: every
    // spelling generates, and `Pick` is the same enum of the union's branches met with the object.
    let base = "{ $ref: '#/components/schemas/Base' }";
    let inline_object =
        "{ type: object, properties: { a: { type: string }, b: { type: string } } }";
    let agreeing: [(&str, Vec<String>); 2] = [
        (
            "a lone $ref member to a union component",
            vec![
                "{ $ref: '#/components/schemas/U', properties: { a: { type: string }, b: { \
                 type: string } } }"
                    .to_owned(),
                format!("{{ allOf: [ {base}, {any} ] }}"),
                format!("{{ allOf: [ {base}, {union_ref} ] }}"),
                format!("{{ allOf: [ {union_ref}, {base} ] }}"),
                format!("{{ allOf: [ {inline_object}, {union_ref} ] }}"),
            ],
        ),
        (
            "a union of one real branch beside null next to a union member",
            vec![
                format!(
                    "{{ $ref: '#/components/schemas/Base', allOf: [ {nullable_other} ], oneOf: \
                     {} }}",
                    branches
                ),
                format!("{{ allOf: [ {base}, {one}, {nullable_other} ] }}"),
                format!("{{ allOf: [ {base}, {nullable_other}, {one} ] }}"),
            ],
        ),
    ];
    // Each variant's payload by its fields and whether each is optional: the variant and payload
    // names follow the spelling, the shape does not.
    let shapes = |code: &str, variants: &[String]| -> Vec<Vec<String>> {
        variants
            .iter()
            .map(|variant| {
                let payload = variant
                    .split_once('(')
                    .map(|(_, rest)| rest.trim_end_matches(')'))
                    .map(|ty| ty.trim_start_matches("Box<").trim_end_matches('>'))
                    .unwrap_or_default();
                let mut lines = code
                    .lines()
                    .map(str::trim_start)
                    .skip_while(|line| !line.starts_with(&format!("pub struct {payload} ")));
                lines.next();
                lines
                    .take_while(|line| !line.starts_with('}'))
                    .filter_map(|line| line.strip_prefix("pub "))
                    // A field's string alias is named after the spelling too; whether it is
                    // required is the shape.
                    .filter_map(|field| field.split_once(": "))
                    .map(|(name, ty)| format!("{name}: {}", ty.starts_with("Option<")))
                    .collect()
            })
            .collect()
    };
    for (case, spellings) in &agreeing {
        let mut expected: Option<Vec<Vec<String>>> = None;
        for pick in spellings {
            let spec = spec(pick);
            let (report, code) = generate_with_code(&spec);
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{case}, `{pick}`: {report:#?}"
            );
            assert_check_agrees(&report, &check(&spec));
            let variants = enum_variants(&code, "Pick");
            assert_eq!(
                variants.len(),
                2,
                "{case}, `{pick}`: `Pick` is the union's two branches met with the object: {}",
                types_module(&code)
            );
            let shape = shapes(&code, &variants);
            assert!(
                shape.iter().all(|fields| !fields.is_empty()),
                "{case}, `{pick}`: each variant is a met object: {}",
                types_module(&code)
            );
            match &expected {
                None => expected = Some(shape),
                Some(expected) => assert_eq!(
                    &shape,
                    expected,
                    "{case}, `{pick}` must agree with its `$ref` spelling: {}",
                    types_module(&code)
                ),
            }
        }
    }
}

/// An `allOf` whose members are all unions, here `$ref`s to a two-branch union and to a union of
/// one real branch beside `null`, with no keyword of its own, meets no object member with them:
/// its unions meet as the scalar members they generated as before #841, which reads the `$ref`
/// union member as its target's union only beside an object member. The generated names are the
/// public API, so they are pinned in both orders.
#[test]
fn an_allof_of_unions_alone_keeps_its_scalar_meet() {
    let union = "{ $ref: '#/components/schemas/U' }";
    let nullable = "{ $ref: '#/components/schemas/NullableOther' }";
    for pick in [
        format!("{{ allOf: [ {union}, {nullable} ] }}"),
        format!("{{ allOf: [ {nullable}, {union} ] }}"),
    ] {
        let (report, code) = generate_with_code(&union_member_document("", &pick));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "`{pick}`: {report:#?}"
        );
        assert_eq!(
            enum_variants(&code, "Pick"),
            [
                "Uvariant0(Box<PickIntersection1Variant0>)",
                "Uvariant1(Box<PickIntersection1Variant1>)",
            ],
            "`{pick}`: {}",
            types_module(&code)
        );
    }
}

/// A union component that reaches itself through an `allOf` member of one of its branches,
/// reached in turn as an `allOf` member beside an object, is one recursion: its body, expanded as
/// that member's union (#841), leaves the `$ref` back to it to the ordinary gather, which refuses
/// it at the component's reservation with `E013` alone. Expanded again at each level, it walked to
/// the lowering depth limit and added an `E014` beside it.
#[test]
fn a_recursive_union_component_met_as_an_allof_member_is_one_cycle() {
    let extra = "    U3:\n      anyOf: [ { required: [a] }, { required: [b], properties: { \
                 child: { allOf: [ { $ref: '#/components/schemas/Base' }, { $ref: \
                 '#/components/schemas/U3' } ] } } } ]\n";
    let pick = "{ allOf: [ { $ref: '#/components/schemas/Base' }, { $ref: \
                '#/components/schemas/U3' } ] }";
    let spec = union_member_document(extra, pick);
    for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert_eq!(
            codes(&report),
            ["E013"],
            "{entry}: the recursion is reported once, as the cycle it is: {report:#?}"
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.pointer.as_str()
                    == "/components/schemas/U3/anyOf/1/properties/child/allOf/1"),
            "{entry}: {report:#?}"
        );
    }
}
