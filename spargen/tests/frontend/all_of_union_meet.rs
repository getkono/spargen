//! An `allOf` met with a `oneOf`/`anyOf`: a union written as an `allOf` member, beside the `allOf`,
//! or beside a member's own keywords is met with the other conjuncts as a `$ref`-sibling union is,
//! takes its `null` from every conjunct, and leaves no pre-meet union in the output; several unions
//! beside an object are rejected.

use super::*;

/// A union met with an `allOf`'s other members, or with a `$ref`'s target, is lowered on its own
/// first, with its merge held back, and the meet re-emits what it leaves under the schema's own
/// name. The pre-meet union, and the branch types the meet replaced, were left in the output as
/// public types nothing referred to (#561), such as `Umember0`, a trial-matched `oneOf` with a
/// `serde_json::Value` variant, beside the met union `U`. Each spelling of the meet now withholds
/// them: as an `allOf` member, beside an `allOf`, and beside a `$ref` alone or with keywords, where
/// the keywords' own lowered type, `UconstraintConstraint`, is withheld too (#571), and so is the
/// object composition the union was met with, `Ucomposition`, as an `allOf` member and beside an
/// `allOf` (#852). What
/// something can still name stays: a branch the met union refers to, a `$ref`'d component first
/// lowered inside the union, and a `$ref`'d subschema the resolver's memo hands a later `$ref`.
#[test]
fn an_all_of_union_meet_emits_no_union_its_result_does_not_use() {
    /// The types the `types` module declares, in source order.
    fn declared(code: &str) -> Vec<String> {
        types_module(code)
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .map(str::to_owned)
            .collect()
    }
    let refiner = "{ properties: { a: { type: string } } }";
    let b = "B: { type: object, properties: { a: { type: string } } }";
    // Each spelling, with the types it no longer emits and the ones it must keep.
    let cases: [(&str, String, &[&str], &[&str]); 9] = [
        (
            "an allOf member",
            format!("U: {{ allOf: [{{ oneOf: [{{ type: string }}, {{}}] }}, {refiner}] }}"),
            &["Umember0", "Umember0Variant1"],
            &["U", "Umember0Variant0"],
        ),
        (
            "beside an allOf",
            format!("U: {{ oneOf: [{{ type: string }}, {{}}], allOf: [{refiner}] }}"),
            &["Uunion", "UunionVariant1"],
            &["U", "UunionVariant0"],
        ),
        (
            "beside a $ref",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', \
                 oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &["Uconstraint", "UconstraintVariant0", "UconstraintVariant1"],
            &["B", "U"],
        ),
        (
            "beside a $ref and its keywords",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', properties: {{ c: {{ type: integer \
                 }} }}, oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &[
                "Uunion",
                "UunionVariant0",
                "UunionVariant1",
                "UconstraintConstraint",
            ],
            &["B", "U", "UconstraintConstraintc"],
        ),
        (
            "beside a $ref and its typed keywords, met with the target first",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', type: object, properties: {{ c: \
                 {{ type: integer }} }}, oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &[
                "Uunion",
                "UunionVariant0",
                "UunionVariant1",
                "Uconstraint",
                "UreferenceComposition",
            ],
            &["B", "U", "Uconstraintc"],
        ),
        (
            "a $ref'd component first lowered inside the union",
            format!(
                "U: {{ allOf: [{{ oneOf: [{{ $ref: '#/components/schemas/S' }}, {{}}] }}, \
                 {refiner}] }}\n    S: {{ type: object, properties: {{ s: {{ type: string }} }} }}"
            ),
            &["Umember0", "Umember0Variant1"],
            &["U", "S", "Ss"],
        ),
        (
            "a $ref'd subschema the resolver hands a later $ref",
            format!(
                "U: {{ allOf: [{{ oneOf: [{{ type: string }}, {{ $ref: \
                 '#/components/schemas/W/properties/w' }}] }}, {refiner}] }}\n    \
                 W: {{ type: object, properties: {{ w: {{ type: object, properties: \
                 {{ b: {{ type: integer }} }} }} }} }}\n    \
                 X: {{ type: object, properties: {{ x: {{ $ref: \
                 '#/components/schemas/W/properties/w' }} }} }}"
            ),
            &["Umember0"],
            &["U", "Umember0Variant0", "X"],
        ),
        (
            "an allOf member beside an object member",
            format!("{b}\n    U: {{ allOf: [{{ $ref: '#/components/schemas/B' }}, {{ anyOf: [{{}}, {{}}] }}] }}"),
            &["Ucomposition"],
            &["B", "U"],
        ),
        (
            "beside an allOf with an object member",
            format!("{b}\n    U: {{ anyOf: [{{}}, {{}}], allOf: [{{ $ref: '#/components/schemas/B' }}] }}"),
            &["Ucomposition"],
            &["B", "U"],
        ),
    ];
    for (spelling, schemas, withheld, kept) in cases {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    {schemas}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{spelling}: {report:#?}"
        );
        let declared = declared(&code);
        for name in withheld {
            assert!(
                !declared.iter().any(|declared| declared == name),
                "{spelling}: `{name}` is still emitted: {declared:?}"
            );
        }
        for name in kept {
            assert!(
                declared.iter().any(|declared| declared == name),
                "{spelling}: `{name}` is no longer emitted: {declared:?}"
            );
        }
        // The type the resolver first lowered inside the union is the one `X.x` names.
        if let Some((_, rest)) = code.split_once("pub x: Option<") {
            let shared = rest.split('>').next().unwrap_or_default();
            assert!(
                declared.iter().any(|declared| declared == shared),
                "{spelling}: `X.x`'s `{shared}` is not emitted: {declared:?}"
            );
        }
        let found = oracles::indistinguishable_variants(&code);
        assert!(found.is_empty(), "{spelling}: {found:#?}");
    }
}

/// `allOf: [{$ref: T}, {oneOf/anyOf: […]}]` describes exactly the instances `{$ref: T,
/// oneOf/anyOf: […]}` does, and so does the union written beside the `allOf`: all three are one
/// conjunction, and must agree (#463). The `allOf`-member spelling used to read the union as a
/// scalar member, so beside `T`'s object it was `E013` "mixes object and scalar members" where the
/// `$ref` spelling generated. Each case is lowered in all three spellings as the component `Pick`,
/// and each must reach the same outcome, report the same codes at `Pick`, and give `Pick` the same
/// shape: branches the meet leaves as one type collapse with `W001` and keep the keyword's `null`
/// rule, a branch the target excludes drops out with `W011` (#671), a union none of whose branches meets the target
/// is `E013`, and a union of one real branch beside `null` is met as that branch.
#[test]
fn an_all_of_union_member_is_met_with_the_other_members_as_a_ref_sibling_union_is() {
    let base =
        "    Base:\n      type: object\n      properties:\n        a: { type: string }\n        \
                b: { type: string }\n";
    let nullable_base =
        "    NB:\n      type: [object, 'null']\n      properties:\n        a: { type: \
                         string }\n        b: { type: string }\n";
    let name = "    Name: { type: string }\n";
    let other =
        "    Other:\n      type: object\n      required: [a]\n      properties:\n        c: \
                 { type: integer }\n";
    // Branches that state nothing, so each meets the target to the target itself. Untyped
    // `required` branches no longer do: each establishes an object that requires its key (#613).
    let keywordless = "[ {}, {} ]";
    let enumerated = "[ { type: string, enum: [red, green] }, { type: integer } ]";
    let single =
        "[ { type: object, required: [a], properties: { c: { type: integer } } }, { type: \
                  'null' } ]";
    let single_ref = "[ { $ref: '#/components/schemas/Other' }, { type: 'null' } ]";
    // (case, target, keyword, branches, the codes reported at `Pick`, `Pick`'s shape). `Fields`
    // carries whether the required `pick` property is `Option`, that is, whether `null` is valid.
    enum Shape {
        Fields(&'static [&'static str], bool),
        Variants(&'static [&'static str]),
        Rejected,
    }
    let cases = [
        (
            "keywordless oneOf",
            "Base",
            "oneOf",
            keywordless,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "keywordless anyOf",
            "Base",
            "anyOf",
            keywordless,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "nullable target, oneOf",
            "NB",
            "oneOf",
            keywordless,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "nullable target, anyOf",
            "NB",
            "anyOf",
            keywordless,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], true),
        ),
        // The branch the target excludes is reported in each spelling, as the sibling spelling
        // reports it (#671).
        (
            "excluded branch, oneOf",
            "Name",
            "oneOf",
            enumerated,
            vec![Code::DeclarationHasNoEffect],
            Shape::Variants(&["Red", "Green"]),
        ),
        (
            "excluded branch, anyOf",
            "Name",
            "anyOf",
            enumerated,
            vec![Code::DeclarationHasNoEffect],
            Shape::Variants(&["Red", "Green"]),
        ),
        (
            "empty meet",
            "Name",
            "oneOf",
            "[ { type: integer } ]",
            vec![Code::AllOfIrreconcilable],
            Shape::Rejected,
        ),
        (
            "single nullable branch",
            "Base",
            "oneOf",
            single,
            vec![],
            Shape::Fields(&["a", "b", "c"], false),
        ),
        // The same with its real branch a `$ref` (#491): met as `Other`, so `Pick` is `Base` and
        // `Other` merged. Beside the nullable target `null` matches the target and exactly one
        // branch, so the `oneOf` admits it.
        (
            "single nullable $ref branch",
            "Base",
            "oneOf",
            single_ref,
            vec![],
            Shape::Fields(&["a", "b", "c"], false),
        ),
        (
            "nullable target, single nullable $ref branch",
            "NB",
            "oneOf",
            single_ref,
            vec![],
            Shape::Fields(&["a", "b", "c"], true),
        ),
    ];
    // A further conjunct `required: [a]`, which every instance satisfies as well, written as an
    // `allOf` member that is either those untyped object keywords alone (a member that refines the
    // union's object branches) or the same keywords nested in an `allOf` of their own (a member
    // combined with the target). `null` satisfies the target, the conjunct and both branches, so
    // the `anyOf` admits it and the `oneOf`, which it matches twice, does not. The `$ref` spelling
    // writes the conjunct's keywords beside the `$ref`, `{$ref: NB, required: [a], oneOf|anyOf:
    // […]}` (#538), and its nested-`allOf` form `{$ref: NB, allOf: [{required: [a]}], …}`, whose
    // untyped `allOf` admits the target's `null` without deciding it (#562).
    let mut conjunct_cases = Vec::new();
    //
    // A typed conjunct, a nullable object with a further property `c`, meets the target before the
    // union rather than refining its branches, and admits `null` of its own accord.
    let ab: &[&str] = &["a", "b"];
    let abc: &[&str] = &["a", "b", "c"];
    for (form, conjunct, fields) in [
        (
            "refiner member",
            ("{ required: [a] }", ", required: [a]"),
            ab,
        ),
        (
            "nested allOf member",
            (
                "{ allOf: [ { required: [a] } ] }",
                ", allOf: [ { required: [a] } ]",
            ),
            ab,
        ),
        (
            "typed member",
            (
                "{ type: [object, 'null'], properties: { c: { type: integer } } }",
                ", type: [object, 'null'], properties: { c: { type: integer } }",
            ),
            abc,
        ),
    ] {
        for (keyword, nullable) in [("oneOf", false), ("anyOf", true)] {
            conjunct_cases.push((
                format!("nullable target, {form}, {keyword}"),
                "NB",
                keyword,
                keywordless,
                vec![Code::ValidationKeywordIgnored],
                Shape::Fields(fields, nullable),
                Some(conjunct),
            ));
        }
    }
    let cases = cases
        .into_iter()
        .map(|(case, target, keyword, branches, codes, shape)| {
            (
                case.to_owned(),
                target,
                keyword,
                branches,
                codes,
                shape,
                None,
            )
        })
        .chain(conjunct_cases);
    for (case, target, keyword, branches, codes, shape, conjunct) in cases {
        let target_ref = format!("'#/components/schemas/{target}'");
        let member = conjunct.map_or_else(String::new, |(member, _)| format!(", {member}"));
        // The conjunct's keywords written beside the `$ref`, as further siblings.
        let keywords = conjunct.map_or("", |(_, keywords)| keywords);
        for (spelling, site) in [
            (
                "$ref sibling",
                format!("{{ $ref: {target_ref}{keywords}, {keyword}: {branches} }}"),
            ),
            (
                "allOf member",
                format!(
                    "{{ allOf: [ {{ $ref: {target_ref} }}{member}, {{ {keyword}: {branches} }} \
                     ] }}"
                ),
            ),
            (
                "beside allOf",
                format!("{{ allOf: [ {{ $ref: {target_ref} }}{member} ], {keyword}: {branches} }}"),
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
{base}{nullable_base}{name}{other}    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
            );
            let label = format!("{case}, {spelling}");
            let (report, code) = generate_with_code(&spec);
            let at_pick: Vec<Code> = report
                .diagnostics()
                .iter()
                .filter(|d| d.pointer.as_str() == "/components/schemas/Pick")
                .map(|d| d.code)
                .collect();
            assert_eq!(at_pick, codes, "{label}: {report:#?}\n{spec}");
            let types = types_module(&code);
            match shape {
                Shape::Rejected => {
                    assert_eq!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                }
                Shape::Fields(fields, nullable) => {
                    assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                    assert_eq!(declared_fields(&types, "Pick"), fields, "{label}: {types}");
                    let pick = field_type(&types, "pub pick")
                        .unwrap_or_else(|| panic!("{label}: no `pick` field: {types}"));
                    assert_eq!(
                        pick.starts_with("Option<"),
                        nullable,
                        "{label}: `pick` is `{pick}`: {types}"
                    );
                }
                Shape::Variants(variants) => {
                    assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                    assert_eq!(enum_variants(&types, "Pick"), variants, "{label}: {types}");
                }
            }
        }
    }
}

/// An `allOf` with a union member takes its `null` from every conjunct, the enclosing schema's own
/// `type` among them, exactly as `{$ref: T, type: …, oneOf|anyOf: […]}` and the union written
/// beside the `allOf` do (#491). The `allOf` path once read nullability from the enclosing `type`
/// alone after the meet (`with_all_of_nullability`), so beside a nullable target the spellings
/// could differ in `Option<_>`. Each case is lowered in the three spellings as `Pick`, held by a
/// required `pick`, and each must make `pick` optional exactly where `null` satisfies the target,
/// the `type`, and the union (`oneOf`: exactly one branch).
#[test]
fn an_all_of_union_member_takes_null_from_every_conjunct_as_the_ref_spelling_does() {
    let any_required = "anyOf: [ { required: [a] }, { required: [b] } ]";
    let any_null_branch = "anyOf: [ { required: [a] }, { type: 'null' }, { required: [b] } ]";
    let one_ref_or_null = "oneOf: [ { $ref: '#/components/schemas/Other' }, { type: 'null' } ]";
    let any_ref_or_null = "anyOf: [ { $ref: '#/components/schemas/Other' }, { type: 'null' } ]";
    // (target, the enclosing `type` keyword or none, the union, whether `pick` is optional).
    let cases = [
        ("NB", Some("[object, 'null']"), any_required, true),
        ("Base", Some("[object, 'null']"), any_required, false),
        ("NB", None, any_null_branch, true),
        ("NB", None, one_ref_or_null, true),
        ("NB", Some("object"), one_ref_or_null, false),
        ("Base", Some("[object, 'null']"), one_ref_or_null, false),
        ("NB", Some("[object, 'null']"), one_ref_or_null, true),
        ("NB", None, any_ref_or_null, true),
    ];
    for (target, types, union, optional) in cases {
        let target_ref = format!("'#/components/schemas/{target}'");
        let types = types.map_or_else(String::new, |types| format!("type: {types}, "));
        for (spelling, site) in [
            (
                "allOf member",
                format!("{{ {types}allOf: [ {{ $ref: {target_ref} }}, {{ {union} }} ] }}"),
            ),
            (
                "$ref sibling",
                format!("{{ $ref: {target_ref}, {types}{union} }}"),
            ),
            (
                "beside allOf",
                format!("{{ {types}allOf: [ {{ $ref: {target_ref} }} ], {union} }}"),
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
    Base:
      type: object
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Other:
      type: object
      required: [a]
      properties:
        c: {{ type: integer }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
            );
            let label = format!("{target}, {types}{union}, {spelling}");
            let (report, code) = generate_with_code(&spec);
            assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
            let types_code = types_module(&code);
            let pick = field_type(&types_code, "pub pick")
                .unwrap_or_else(|| panic!("{label}: no `pick` field: {types_code}"));
            assert_eq!(
                pick.starts_with("Option<"),
                optional,
                "{label}: `pick` is `{pick}`: {types_code}"
            );
        }
    }
}

/// Object members are met with one `oneOf`/`anyOf` union branch by branch (#463); a second union
/// would nest one union's branches under the other's, so an object composition with two or more
/// unions to meet is rejected with `E013`'s `several-unions` case (#491), in every place an
/// `allOf` gathers a union from: a second inline member, a `$ref` member to a union component, a
/// union nested in an inner `allOf` (flattened into the merge), the schema's own object keywords
/// beside two inline unions, and the union written beside the `allOf`. Before #491 each was
/// reported as an object/scalar mix, which names no member the document wrote. The controls
/// keep the rule to what it says: one union beside a `$ref` to a plain object component, a
/// union of one real branch beside `null` (no union once lowered), and two unions with no object
/// member (met as scalars) still generate.
#[test]
fn an_all_of_with_several_union_members_beside_an_object_is_rejected() {
    let one = "{ oneOf: [ { required: [a] }, { required: [b] } ] }";
    let any = "{ anyOf: [ { required: [a] }, { required: [b] } ] }";
    let base = "{ $ref: '#/components/schemas/Base' }";
    let union_ref = "{ $ref: '#/components/schemas/U' }";
    let rejected = [
        (
            "oneOf and anyOf members",
            format!("{{ allOf: [ {base}, {one}, {any} ] }}"),
        ),
        (
            "two oneOf members",
            format!("{{ allOf: [ {base}, {one}, {one} ] }}"),
        ),
        (
            "a $ref member to a union",
            format!("{{ allOf: [ {base}, {one}, {union_ref} ] }}"),
        ),
        (
            "a union nested in an inner allOf",
            format!("{{ allOf: [ {base}, {one}, {{ allOf: [ {any} ] }} ] }}"),
        ),
        (
            "two unions nested in an inner allOf",
            format!("{{ allOf: [ {base}, {{ allOf: [ {one}, {any} ] }} ] }}"),
        ),
        (
            "the schema's own object keywords",
            format!(
                "{{ type: object, properties: {{ a: {{ type: string }} }}, allOf: [ {one}, {any} \
                 ] }}"
            ),
        ),
        (
            "an inline member beside a union beside the allOf",
            format!(
                "{{ allOf: [ {base}, {any} ], oneOf: [ {{ required: [a] }}, {{ required: [b] }} \
                 ] }}"
            ),
        ),
        (
            "a $ref member beside a union beside the allOf",
            format!(
                "{{ allOf: [ {base}, {union_ref} ], oneOf: [ {{ required: [a] }}, {{ required: \
                 [b] }} ] }}"
            ),
        ),
    ];
    let generated = [
        (
            "one union beside a $ref to an object",
            format!("{{ allOf: [ {base}, {one}, {{ $ref: '#/components/schemas/Other' }} ] }}"),
        ),
        (
            "a union of one real branch beside null",
            format!(
                "{{ allOf: [ {base}, {{ oneOf: [ {{ $ref: '#/components/schemas/Other' }}, {{ \
                 type: 'null' }} ] }} ] }}"
            ),
        ),
        (
            "two unions and no object member",
            "{ allOf: [ { oneOf: [ { type: string }, { type: integer } ] }, { anyOf: [ { type: \
             string }, { type: integer } ] } ] }"
                .to_owned(),
        ),
    ];
    let several = "more than one `oneOf`/`anyOf` union";
    for (case, pick, expect_rejected) in rejected
        .iter()
        .map(|(case, pick)| (case, pick, true))
        .chain(generated.iter().map(|(case, pick)| (case, pick, false)))
    {
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
      properties:
        c: {{ type: integer }}
    U:
      anyOf: [ {{ required: [a] }}, {{ required: [b] }} ]
    Pick: {pick}
"##
        );
        let report = generate(&spec);
        let at_pick: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| {
                d.code == Code::AllOfIrreconcilable
                    && d.pointer.as_str() == "/components/schemas/Pick"
            })
            .map(|d| d.message.as_str())
            .collect();
        if expect_rejected {
            assert_eq!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            assert!(
                at_pick.len() == 1 && at_pick[0].contains(several),
                "{case}: {report:#?}"
            );
        } else {
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
        }
    }
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
    let spec = |pick: &str| {
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
    Pick: {pick}
"##
        )
    };
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
    for (case, spellings) in &agreeing {
        let mut expected: Option<Vec<String>> = None;
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
            match &expected {
                None => expected = Some(variants),
                Some(expected) => assert_eq!(
                    &variants,
                    expected,
                    "{case}, `{pick}` must agree with its `$ref` spelling: {}",
                    types_module(&code)
                ),
            }
        }
    }
}

/// Issue #419: a schema carrying `allOf` and `oneOf`/`anyOf` together lowered through the `allOf`
/// arm alone, which never read the union, so the union — and a `discriminator` beside it — was
/// dropped and `check` called the document clean. The two are a conjunction: the `allOf`
/// composition is met with the union branch by branch, as a `$ref` is met with a union target, so
/// a branch the composition excludes drops out. Where no branch is left, or a meet has no single
/// type, it is `E013` at the schema.
#[test]
fn a_union_beside_all_of_that_contradicts_it_is_rejected() {
    let cases = [
        (
            "issue reproduction (oneOf)",
            "    Pet:\n      \
             allOf: [ { type: object, properties: { id: { type: integer } } } ]\n      \
             oneOf: [ { type: string } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "anyOf",
            "    Pet:\n      \
             allOf: [ { type: object, properties: { id: { type: integer } } } ]\n      \
             anyOf: [ { type: string } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "scalar allOf against a disjoint union",
            "    Pet:\n      \
             allOf: [ { type: integer } ]\n      \
             oneOf: [ { type: string }, { type: boolean } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "nested allOf member carrying a union",
            "    Pet:\n      allOf:\n        \
             - allOf: [ { type: object, properties: { id: { type: integer } } } ]\n          \
             oneOf: [ { type: string } ]\n",
            "/components/schemas/Pet/allOf/0",
        ),
    ];
    for (what, schemas, pointer) in cases {
        let spec = with_schemas("3.1.0", schemas);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: {report:#?}\n{spec}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.pointer.as_str() == pointer),
                "{what} via {entry}: E013 must sit at `{pointer}`: {report:#?}"
            );
        }
    }
}

/// Issue #419, the inline-member spelling: an `allOf` member carrying object keywords beside its
/// own `oneOf` was read as an object by its keywords and its union dropped. The member is lowered
/// as the union it is, its object keywords refining the branches of their own category, so an
/// explicit `type: object` beside a string-only union leaves no branch and is reported, and untyped
/// `properties` beside it constrain nothing the union accepts.
#[test]
fn an_all_of_member_with_its_own_union_beside_object_keywords_keeps_the_union() {
    let typed = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf:\n        \
         - type: object\n          properties: { id: { type: integer } }\n          \
         oneOf: [ { type: string } ]\n",
    );
    let report = generate(&typed);
    assert_eq!(
        report.outcome(),
        Outcome::Rejected,
        "an object no string is: {report:#?}"
    );
    assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    let untyped = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf:\n        \
         - properties: { id: { type: integer } }\n          \
         oneOf: [ { type: string } ]\n",
    );
    let (report, code) = generate_with_code(&untyped);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    assert!(
        types.contains("pub type Pet = String;"),
        "the member is a string, not a struct with `id`:\n{types}"
    );
    assert!(
        !types.contains("pub struct Pet "),
        "the union must not be dropped for the object keywords:\n{types}"
    );
}

/// Issue #419: where the conjunction has a type, it is the union narrowed by the `allOf`, not the
/// `allOf` alone. An annotation-only `allOf` leaves the discriminated union exactly as the same
/// schema without it generates; a scalar `allOf` narrows the union to the branches it admits.
#[test]
fn a_union_beside_all_of_composes_with_it() {
    let union = "      oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: string } ]\n      \
                 discriminator: { propertyName: kind, mapping: { cat: Cat } }\n";
    let bare = with_schemas("3.1.0", &format!("    Pet:\n{union}"));
    let shadowed = with_schemas(
        "3.1.0",
        &format!("    Pet:\n      allOf: [ {{ description: a pet }} ]\n{union}"),
    );
    let (bare_report, bare_code) = generate_with_code(&bare);
    assert_ne!(bare_report.outcome(), Outcome::Rejected, "{bare_report:#?}");
    assert_eq!(
        enum_variants(&bare_code, "Pet"),
        ["Cat(Box<Cat>)", "PetVariant1(Box<PetVariant1>)"],
        "{bare_code}"
    );
    let (report, code) = generate_with_code(&shadowed);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    assert_eq!(types_module(&code), types_module(&bare_code));
    assert!(
        code.contains("\"cat\""),
        "the discriminator still dispatches:\n{code}"
    );

    // The `allOf` polymorphism parent spelled beside its own union: every branch is met with the
    // shared base, so each variant carries the base's `id` as well as its own fields.
    let based = with_schemas(
        "3.1.0",
        "    Base: { type: object, required: [id], properties: { id: { type: integer } } }\n    \
         Dog: { type: object, required: [kind], properties: { kind: { type: string }, bark: { type: boolean } } }\n    \
         Pet:\n      allOf: [ { $ref: '#/components/schemas/Base' } ]\n      \
         oneOf: [ { $ref: '#/components/schemas/Cat' }, { $ref: '#/components/schemas/Dog' } ]\n      \
         discriminator: { propertyName: kind }\n",
    );
    let (report, code) = generate_with_code(&based);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let variants = enum_variants(&types, "Pet");
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        let payload = variant
            .split_once("(Box<")
            .and_then(|(_, rest)| rest.strip_suffix(">)"))
            .unwrap_or_else(|| panic!("{variant}: {types}"));
        let fields = declared_fields(&types, payload);
        assert!(
            fields.iter().any(|field| field == "id") && fields.iter().any(|field| field == "kind"),
            "{payload} must carry the base's `id` beside its own `kind`: {fields:?}\n{types}"
        );
    }

    let narrowed = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf: [ { type: integer } ]\n      \
         oneOf: [ { type: string }, { type: integer, format: int32 } ]\n",
    );
    let (report, code) = generate_with_code(&narrowed);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    assert!(
        types.contains("pub type Pet = i32;"),
        "only the integer branch meets `type: integer`:\n{types}"
    );
}

/// Issue #419: an `allOf` beside the schema's own `oneOf`/`anyOf` excludes only the branches it
/// constrains away. The schema's own keywords beside both are the union's siblings, refining its
/// branches of their own category, and are not also an object member of the composition; an
/// untyped `allOf` member is refined into the union the same way, as a `$ref` to a union refines
/// with untyped siblings. Either, read as an object of the composition, dropped every string and
/// `null` branch, so the generated client refused values the description accepts. A typed member
/// that admits `null` keeps the union's `null` branch.
#[test]
fn a_union_beside_all_of_keeps_the_branches_and_null_the_composition_admits() {
    let mixed = [
        (
            "the schema's own `required` beside an annotation-only allOf",
            "    Pet:\n      required: [kind]\n      allOf: [ { description: a pet } ]\n      \
             oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' } ]\n",
        ),
        (
            "an untyped `required` allOf member",
            "    Pet:\n      allOf: [ { required: [kind] } ]\n      \
             oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' } ]\n",
        ),
    ];
    for (what, schemas) in mixed {
        let spec = with_schemas("3.1.0", schemas);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        assert!(
            !types.contains("pub struct Pet "),
            "{what}: the string branch must survive:\n{types}"
        );
        let variants = enum_variants(&types, "Pet");
        assert_eq!(variants.len(), 2, "{what}: {variants:?}\n{types}");
        assert!(
            variants.iter().any(|variant| {
                variant
                    .split_once("(Box<")
                    .and_then(|(_, rest)| rest.strip_suffix(">)"))
                    .is_some_and(|payload| types.contains(&format!("pub type {payload} = String;")))
            }),
            "{what}: one variant is the string branch: {variants:?}\n{types}"
        );
    }

    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let nullable = [
        (
            "the schema's own `properties` beside an annotation-only allOf",
            "    Pet:\n      properties: { name: { type: string } }\n      \
             allOf: [ { description: a pet } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
        (
            "an untyped `properties` allOf member",
            "    Pet:\n      allOf: [ { properties: { name: { type: string } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
        (
            "a typed allOf member admitting null",
            "    Pet:\n      \
             allOf: [ { type: [object, 'null'], properties: { name: { type: string } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
    ];
    for (what, schemas) in nullable {
        let spec = with_schemas("3.1.0", &format!("{owner}{schemas}"));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert!(
            p.starts_with("Option<"),
            "{what}: `null` satisfies both, so `p` is nullable, not `{p}`:\n{types}"
        );
        let fields = declared_fields(&types, "Pet");
        assert!(
            fields.iter().any(|field| field == "name")
                && fields.iter().any(|field| field == "kind"),
            "{what}: the object branch is `Cat` refined by `name`: {fields:?}\n{types}"
        );
    }
}

/// Issue #419: a union of two or more non-null branches plus `null` stays a nullable union when
/// an untyped `allOf` member refines it, as it does when the same keywords sit beside a `$ref`
/// to it or beside the union itself. The untyped keywords say nothing about `null`, so the union's
/// `null` branch survives the refinement. The branch-by-branch meet built a new union without it,
/// and a required property reaching the schema was typed `Pet`, refusing the `null` the
/// description accepts.
#[test]
fn an_untyped_all_of_member_keeps_a_multi_branch_unions_null() {
    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let union =
        "oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    let cases = [
        (
            "an untyped `required` allOf member",
            format!("    Pet:\n      allOf: [ {{ required: [kind] }} ]\n      {union}\n"),
        ),
        (
            "an untyped `properties` allOf member",
            format!(
                "    Pet:\n      allOf: [ {{ properties: {{ name: {{ type: string }} }} }} ]\n      \
                 {union}\n"
            ),
        ),
        (
            "two untyped allOf members",
            format!(
                "    Pet:\n      allOf: [ {{ required: [kind] }}, \
                 {{ properties: {{ name: {{ type: string }} }} }} ]\n      {union}\n"
            ),
        ),
        (
            "the schema's own `required` beside an annotation-only allOf",
            format!(
                "    Pet:\n      required: [kind]\n      allOf: [ {{ description: a pet }} ]\n      \
                 {union}\n"
            ),
        ),
        (
            "the same keywords beside a `$ref` to the union",
            format!(
                "    Pet:\n      $ref: '#/components/schemas/U'\n      required: [kind]\n    \
                 U:\n      {union}\n"
            ),
        ),
    ];
    for (what, schemas) in cases {
        let spec = with_schemas("3.1.0", &format!("{owner}{schemas}"));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert_eq!(
            p, "Option<Pet>",
            "{what}: `null` satisfies both, so `p` is nullable:\n{types}"
        );
        let variants = enum_variants(&types, "Pet");
        assert_eq!(
            variants.len(),
            2,
            "{what}: both non-null branches survive: {variants:?}\n{types}"
        );
    }
}

/// Issue #419, the `$ref` spelling of an `allOf` member beside a union (decision 10): the member
/// is never a scoped refiner, so the union meets the target's lowered component type. A target
/// holding only `required` lowers to an object as one holding `properties` does (#613), so the
/// meet keeps only the object branch and the string branch drops, reported (`W011`) as the
/// sibling spelling reports a branch its siblings exclude (#671, #674). Neither
/// target states a `type`, so each admits `null` without deciding it (#541), and the `null` branch
/// survives as well: `p` is `Option<Pet>`. This pins today's behaviour of both, so a change to
/// either is a visible diff.
#[test]
fn a_ref_all_of_member_beside_a_union_meets_its_targets_lowered_type() {
    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let with_string =
        "oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    let without_string = "oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    // (target, union, `p`'s type, `Pet`'s variant count: 0 where it is not an enum).
    let cases = [
        (
            "    HasKind:\n      required: [kind]\n",
            with_string,
            "Option<Pet>",
            0,
        ),
        (
            "    HasKind:\n      required: [kind]\n",
            without_string,
            "Option<Pet>",
            0,
        ),
        (
            "    HasName:\n      properties: { name: { type: string } }\n",
            with_string,
            "Option<Pet>",
            0,
        ),
        (
            "    HasName:\n      properties: { name: { type: string } }\n",
            without_string,
            "Option<Pet>",
            0,
        ),
    ];
    for (target, union, p_type, variant_count) in cases {
        let name = target.trim().split(':').next().unwrap_or_default();
        let what = format!("{name} beside {union}");
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "{owner}{target}    Pet:\n      \
                 allOf: [ {{ $ref: '#/components/schemas/{name}' }} ]\n      {union}\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        // The string branch the target excludes is reported as the sibling spelling reports it.
        let excluded: Vec<&str> = report
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        let expected: &[&str] =
            if union == with_string {
                &["union member 0 cannot satisfy the enclosing schema's own constraints, so it is \
               not a variant of the generated enum"]
            } else {
                &[]
            };
        assert_eq!(excluded, expected, "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert_eq!(p, p_type, "{what}:\n{types}");
        let variants = enum_variants(&types, "Pet");
        assert_eq!(
            variants.len(),
            variant_count,
            "{what}: {variants:?}\n{types}"
        );
        assert!(
            types.contains("pub struct Pet {"),
            "{what}: the meet keeps only the object branch:\n{types}"
        );
        let checked = check(&spec);
        let checked_messages: Vec<&str> = checked
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert_eq!(checked_messages, expected, "{what}: {checked:#?}");
    }
}

/// Issue #419: an untyped `allOf` member beside a union is reported as the same keywords are
/// beside a `$ref` to it. Reaching no branch of its category it constrains nothing the union
/// accepts (`W011` at the member, and the union generates); object and array keywords together
/// against a branch that states no category are `E013` at the member; and a meet with the one
/// object branch that leaves no value is `E013` at the schema carrying both keywords.
#[test]
fn an_untyped_all_of_member_beside_a_union_is_reported_as_a_ref_sibling_is() {
    let unreached = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf: [ { required: [kind] } ]\n      \
         oneOf: [ { type: string }, { type: integer } ]\n",
    );
    for (entry, report) in [
        ("generate", generate(&unreached)),
        ("check", check(&unreached)),
    ] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: {report:#?}"
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect
                    && d.pointer.as_str() == "/components/schemas/Pet/allOf/0"),
            "via {entry}: W011 must sit at the member: {report:#?}"
        );
    }

    let rejected = [
        (
            "both kinds against a branch that states no category",
            "    Pet:\n      allOf: [ { required: [kind], items: { type: string } } ]\n      \
             oneOf: [ { type: string }, { description: anything } ]\n",
            "/components/schemas/Pet/allOf/0",
        ),
        (
            "a property type the object branch contradicts",
            "    Pet:\n      \
             allOf: [ { required: [kind], properties: { kind: { type: integer } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' } ]\n",
            "/components/schemas/Pet",
        ),
    ];
    for (what, schemas, pointer) in rejected {
        let spec = with_schemas("3.1.0", schemas);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: {report:#?}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.pointer.as_str() == pointer),
                "{what} via {entry}: E013 must sit at `{pointer}`: {report:#?}"
            );
        }
    }
}

/// A union member of the `allOf` written beside a `oneOf` is met before its indistinguishable
/// branches merge, as the bare `allOf` spelling meets it (#678): the branch the outer union's
/// `integer` excludes is reported (`W011`), the branches the meet leaves sharing one generated type
/// collapse with one `W001` at the schema, and the member's own unmerged lowering, an input to the
/// meet, is not emitted as a public union whose indistinguishable variants reject every value.
/// Written inline, or as a `$ref` to a component that merged them, alike; the component keeps its
/// own merged type.
#[test]
fn an_all_of_union_member_beside_a_union_is_met_before_its_branches_merge() {
    let member = "{ oneOf: [ { type: string }, { type: string, maxLength: 3 } ] }";
    for (spelling, written) in [
        ("inline", member),
        ("a $ref", "{ $ref: '#/components/schemas/M' }"),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Pick: {{ oneOf: [ {{ type: string }}, {{ type: integer }} ], allOf: [ {{ type: \
                 [string, 'null'] }}, {written} ] }}\n    M: {member}\n    Holder: {{ type: \
                 object, properties: {{ pick: {{ $ref: '#/components/schemas/Pick' }} }}, \
                 required: [pick] }}\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{spelling}: {report:#?}"
        );
        let at = |code: Code, pointer: &str| {
            report
                .diagnostics()
                .iter()
                .filter(|d| d.code == code && d.pointer.as_str() == pointer)
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
        };
        assert!(
            at(Code::DeclarationHasNoEffect, "/components/schemas/Pick")
                .iter()
                .any(|message| message.starts_with("union member 1 cannot satisfy")),
            "{spelling}: the excluded `integer` branch is reported: {report:#?}"
        );
        assert!(
            at(Code::ValidationKeywordIgnored, "/components/schemas/Pick")
                .iter()
                .any(|message| message.contains("intersect to a union whose branches differ only")),
            "{spelling}: the meet's collapse is reported at the schema: {report:#?}"
        );
        assert!(
            at(
                Code::ValidationKeywordIgnored,
                "/components/schemas/Pick/allOf/1"
            )
            .is_empty(),
            "{spelling}: the member is not merged before the meet: {report:#?}"
        );
        assert_eq!(
            alias_target(&code, "Pick").as_deref(),
            Some("String"),
            "{code}"
        );
        assert_eq!(
            alias_target(&code, "M").as_deref(),
            Some("String"),
            "{code}"
        );
        let types = types_module(&code);
        assert!(
            !types.contains("PickCompositionMember1"),
            "{spelling}: the member's unmerged lowering is emitted unused:\n{types}"
        );
    }
}

/// An `allOf` member whose `type` names `object` beside another category admits that category's
/// instances too, so a union met with it keeps every branch of a category the array names, as the
/// sibling spelling `{type: [object, string], oneOf: [...]}` does (#827). Read as an object member,
/// it dropped the `string` branch as excluded (`W011`) and collapsed `Pick` to an empty struct
/// that refuses every string. A branch of a category the array omits is still reported, and an
/// object member beside the array keeps the composition an object, so there the `string` branch
/// is excluded, and the array beside an object member alone still merges into that object. An
/// untyped object member (`properties` or `required` alone) constrains objects alone, so beside it
/// the `string` branch stays and the object branch is refined. Beside scalar members alone the
/// array generates their meet, and a bundle `$ref` target's array is read as its object alone.
#[test]
fn an_all_of_type_array_naming_object_keeps_the_union_branches_of_its_categories() {
    let excluded = |report: &Report| -> Vec<String> {
        messages_for(report, Code::DeclarationHasNoEffect)
            .into_iter()
            .filter(|message| message.starts_with("union member "))
            .map(str::to_owned)
            .collect()
    };
    let union = "oneOf: [ { type: object }, { type: string } ]";
    let wider = "oneOf: [ { type: object }, { type: string }, { type: integer } ]";
    let array = "type: [object, string]";
    // Each spelling, with the union branches the meet excludes.
    let cases: [(&str, String, &[&str]); 8] = [
        (
            "the sibling spelling",
            format!("{{ {array}, {union} }}"),
            &[],
        ),
        (
            "an allOf member",
            format!("{{ allOf: [ {{ {array} }}, {{ {union} }} ] }}"),
            &[],
        ),
        (
            "an allOf member nested in an inner allOf",
            format!("{{ allOf: [ {{ {array} }}, {{ allOf: [ {{ {union} }} ] }} ] }}"),
            &[],
        ),
        (
            "beside an allOf",
            format!("{{ {union}, allOf: [ {{ {array} }} ] }}"),
            &[],
        ),
        (
            "an allOf member with object keywords, a branch of an omitted category beside it",
            format!(
                "{{ allOf: [ {{ {array}, properties: {{ a: {{ type: string }} }} }}, {{ {wider} \
                 }} ] }}"
            ),
            &["union member 2"],
        ),
        // An untyped object member constrains objects alone, so every string still satisfies
        // it: it refines the union's object branch and leaves the `string` branch.
        (
            "an allOf member beside an untyped object member",
            format!(
                "{{ allOf: [ {{ {array} }}, {{ properties: {{ a: {{ type: string }} }} }}, {{ \
                 {union} }} ] }}"
            ),
            &[],
        ),
        (
            "beside an allOf of the array and an untyped object member",
            format!(
                "{{ {union}, allOf: [ {{ {array} }}, {{ properties: {{ a: {{ type: string }} \
                 }} }} ] }}"
            ),
            &[],
        ),
        (
            "an allOf member beside an object member",
            format!(
                "{{ allOf: [ {{ {array} }}, {{ type: object, properties: {{ a: {{ type: string \
                 }} }} }}, {{ {union} }} ] }}"
            ),
            &["union member 1"],
        ),
    ];
    for (spelling, pick, expected) in cases {
        let (report, code) =
            generate_with_code(&with_schemas("3.1.0", &format!("    Pick: {pick}\n")));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{spelling}: {report:#?}"
        );
        let excluded = excluded(&report);
        assert_eq!(excluded.len(), expected.len(), "{spelling}: {excluded:#?}");
        for (message, member) in excluded.iter().zip(expected) {
            assert!(message.starts_with(member), "{spelling}: {message}");
        }
        let variants = enum_variants(&code, "Pick");
        if expected == ["union member 1"] {
            assert!(
                variants.is_empty(),
                "{spelling}: an object composition:\n{code}"
            );
            assert_eq!(
                declared_fields(&code, "Pick"),
                ["a"],
                "{spelling}: the object member's field is kept:\n{code}"
            );
        } else {
            assert_eq!(variants.len(), 2, "{spelling}: {variants:#?}\n{code}");
            assert!(
                variants.iter().any(|variant| {
                    let payload = variant.split_once("(Box<").map(|(_, rest)| rest);
                    payload
                        .and_then(|rest| rest.strip_suffix(">)"))
                        .is_some_and(|name| alias_target(&code, name).as_deref() == Some("String"))
                }),
                "{spelling}: the `string` branch is a variant: {variants:#?}\n{code}"
            );
            if spelling.contains("untyped") {
                assert!(
                    variants.iter().any(|variant| {
                        variant
                            .split_once("(Box<")
                            .and_then(|(_, rest)| rest.strip_suffix(">)"))
                            .is_some_and(|name| declared_fields(&code, name) == ["a"])
                    }),
                    "{spelling}: the untyped member refines the object branch: {variants:#?}\n{code}"
                );
            }
        }
        assert_no_untyped_value(&code);
    }
    // An untyped `required` alone constrains objects alone too, so the `string` branch stays.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        &format!("    Pick: {{ allOf: [ {{ {array} }}, {{ required: [a] }}, {{ {union} }} ] }}\n"),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(excluded(&report).is_empty(), "{report:#?}");
    assert_eq!(enum_variants(&code, "Pick").len(), 2, "{code}");
    // Beside scalar members alone, the array is the union of its categories met with them, which
    // generates their meet: rejected as an object/scalar mix (`E013`) before #827.
    for scalar in ["{ type: string }", "{ type: string, minLength: 2 }"] {
        let (report, code) = generate_with_code(&with_schemas(
            "3.1.0",
            &format!("    Pick: {{ allOf: [ {{ {array} }}, {scalar} ] }}\n"),
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{scalar}: {report:#?}"
        );
        assert_eq!(
            alias_target(&code, "Pick").as_deref(),
            Some("String"),
            "{scalar}:\n{code}"
        );
    }
    // A bundle target's array is read as its object alone, as before #827, so the union of its
    // categories, which the target's memoised contribution would keep reachable, is not emitted.
    for other in ["{ $ref: '#/components/schemas/Cat' }", "{ type: string }"] {
        let mut spec = with_schemas(
            "3.1.0",
            &format!("    Pick: {{ allOf: [ {{ $ref: '#/x-defs/p' }}, {other} ] }}\n"),
        );
        spec.push_str(&format!(
            "x-defs:\n  p: {{ {array}, properties: {{ a: {{ type: string }} }} }}\n"
        ));
        let (report, code) = generate_with_code(&spec);
        assert!(
            !types_module(&code).contains("pub enum PMember"),
            "{other}: the target's union is emitted unused: {report:#?}\n{code}"
        );
    }
    let mut spec = with_schemas(
        "3.1.0",
        "    Pick: { allOf: [ { $ref: '#/x-defs/p' }, { $ref: '#/components/schemas/Cat' } ] }\n",
    );
    spec.push_str(&format!(
        "x-defs:\n  p: {{ {array}, properties: {{ a: {{ type: string }} }} }}\n"
    ));
    let (report, code) = generate_with_code(&spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(declared_fields(&code, "Pick"), ["a", "kind"], "{code}");
    // Beside an object member alone, the array still merges into that object, and the union of
    // its categories it was also lowered to is not emitted unused.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        &format!(
            "    Pick: {{ allOf: [ {{ $ref: '#/components/schemas/Cat' }}, {{ {array} }} ] }}\n"
        ),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(declared_fields(&code, "Pick"), ["kind"], "{code}");
    assert!(
        !types_module(&code).contains("pub enum PickMember1"),
        "the member's own union is emitted unused:\n{code}"
    );
}
