//! Discriminators: mapping values and keys, default mappings, and a discriminator beside no union
//! or beside `allOf`.

use super::*;

#[test]
fn oas32_discriminator_default_mapping_generates_a_fallback_branch() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Cat' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        defaultMapping: Dog
    Cat: { type: object, properties: { kind: { const: cat } } }
    Dog: { type: object, properties: { kind: { type: string } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
}

#[test]
fn e007_discriminator_default_mapping_outside_the_union() {
    // A fallback naming a schema that is not a member describes a branch the generated enum does
    // not have, so it cannot be quietly downgraded to another dispatch strategy.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Cat' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        defaultMapping: Fish
    Cat: { type: object, properties: { kind: { const: cat } } }
    Dog: { type: object, properties: { kind: { type: string } } }
    Fish: { type: object, properties: { kind: { const: fish } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
}

/// A `Pet` union over `Cat` and `Dog` (both declared) plus a declared non-member `Fish`, carrying
/// the given `discriminator` body. `members` replaces the `oneOf` list when a case needs another
/// member shape.
fn discriminated_pet(version: &str, members: &str, discriminator: &str) -> String {
    format!(
        "openapi: {version}\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         paths: {{}}\n\
         components:\n  \
         schemas:\n    \
         Pet:\n      \
         oneOf:\n{members}      \
         discriminator:\n{discriminator}    \
         Cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }} }} }}\n    \
         Dog: {{ type: object, required: [kind, bark], properties: {{ kind: {{ type: string }}, bark: {{ type: boolean }} }} }}\n    \
         Fish: {{ type: object, required: [kind, fins], properties: {{ kind: {{ type: string }}, fins: {{ type: integer }} }} }}\n"
    )
}

const CAT_AND_DOG: &str = "        - { $ref: '#/components/schemas/Cat' }\n        \
                           - { $ref: '#/components/schemas/Dog' }\n";

/// Issue #124: a `discriminator.mapping` value is a reference to a schema, and one naming a schema
/// that does not exist was dropped with no diagnostic — `check` said `clean` while the tag it
/// described had no variant to decode into. It is an unresolved reference like any other, so it is
/// `E004`, reported at the mapping entry itself so the rejection names (and auto-carve can reach)
/// the site that is wrong. The same holds for 3.2 `defaultMapping`, which used to be reported as a
/// membership problem (`E007`) about a schema that was never declared at all.
#[test]
fn a_discriminator_mapping_value_naming_an_undeclared_schema_is_e004_at_the_entry() {
    let cases = [
        (
            "pointer spelling",
            discriminated_pet(
                "3.1.0",
                CAT_AND_DOG,
                "        propertyName: kind\n        mapping:\n          \
                 cat: '#/components/schemas/Cat'\n          \
                 c: '#/components/schemas/MissingC'\n",
            ),
            "/components/schemas/Pet/discriminator/mapping/c",
            "MissingC",
        ),
        (
            "schema-name spelling",
            discriminated_pet(
                "3.1.0",
                CAT_AND_DOG,
                "        propertyName: kind\n        mapping:\n          c: MissingC\n",
            ),
            "/components/schemas/Pet/discriminator/mapping/c",
            "MissingC",
        ),
        (
            "3.2 defaultMapping",
            discriminated_pet(
                "3.2.0",
                CAT_AND_DOG,
                "        propertyName: kind\n        defaultMapping: MissingC\n",
            ),
            "/components/schemas/Pet/discriminator/defaultMapping",
            "MissingC",
        ),
    ];
    for (what, spec, pointer, named) in cases {
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: {report:#?}\n{spec}"
            );
            let e004: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::UnresolvedRef)
                .collect();
            assert!(
                e004.iter()
                    .any(|d| d.pointer.as_str() == pointer && d.message.contains(named)),
                "{what} via {entry}: E004 must sit at `{pointer}` and name `{named}`: \
                 {report:#?}"
            );
            // The schema does not exist, so saying it is "not a member" would send the reader to
            // edit the union rather than to the dangling name.
            assert!(
                !has_code(&report, Code::NonDisjointUnion),
                "{what} via {entry}: {report:#?}"
            );
        }
    }
}

/// The sibling shape issue #124 asked to settle. A mapping value that resolves, but to a schema the
/// `oneOf`/`anyOf` does not list, describes a tag whose payload the generated enum has no variant
/// for. The specification requires every possible schema to be listed explicitly beside the
/// discriminator, so this is the same refusal `defaultMapping` already gets for a non-member —
/// `E007` — now reported at the mapping entry. Checked on the sole-real-member collapse as well as
/// the multi-member union, since that path returns before any discriminated dispatch is built and
/// once dropped the mapping there without looking at it.
#[test]
fn a_discriminator_mapping_value_naming_a_non_member_is_e007_at_the_entry() {
    let fish = "        propertyName: kind\n        mapping:\n          \
                cat: Cat\n          fish: '#/components/schemas/Fish'\n";
    let cases = [
        ("two members", discriminated_pet("3.1.0", CAT_AND_DOG, fish)),
        (
            "one real member beside null",
            discriminated_pet(
                "3.1.0",
                "        - { $ref: '#/components/schemas/Cat' }\n        \
                 - { type: 'null' }\n",
                fish,
            ),
        ),
        (
            "inline member",
            discriminated_pet(
                "3.1.0",
                "        - { $ref: '#/components/schemas/Cat' }\n        \
                 - { type: object, required: [kind, fins], properties: { kind: { type: string }, \
                 fins: { type: integer } } }\n",
                fish,
            ),
        ),
    ];
    for (what, spec) in cases {
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
                    .any(|d| d.code == Code::NonDisjointUnion
                        && d.pointer.as_str()
                            == "/components/schemas/Pet/discriminator/mapping/fish"
                        && d.message.contains("Fish")),
                "{what} via {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnresolvedRef),
                "{what} via {entry}: `Fish` is declared: {report:#?}"
            );
        }
    }
}

/// The well-formed control for the two rejections above: a mapping that names every member, in
/// each spelling the specification allows, generates and dispatches on the mapped tags. The file
/// spelling of a root component is the one that used to miss: matching compared strings, so
/// `./openapi.yaml#/components/schemas/Dog` never equalled `Dog`, and the variant silently decoded
/// on the tag `"Dog"` instead of the `"doggo"` the document gave it.
#[test]
fn a_discriminator_mapping_naming_every_member_in_any_spelling_generates_its_tags() {
    let spec = discriminated_pet(
        "3.1.0",
        "        - { $ref: '#/components/schemas/Cat' }\n        \
         - { $ref: '#/components/schemas/Dog' }\n        \
         - { $ref: '#/components/schemas/Fish' }\n",
        "        propertyName: kind\n        mapping:\n          \
         kitty: Cat\n          \
         doggo: './openapi.yaml#/components/schemas/Dog'\n          \
         fishy: '#/components/schemas/Fish'\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    for tag in ["\"kitty\"", "\"doggo\"", "\"fishy\""] {
        assert!(
            code.contains(tag),
            "the mapped tag {tag} must dispatch:\n{code}"
        );
    }
    // The explicit tag is the one written back; the component name still selects the member
    // (#263), since no mapping key claims it.
    let arms = discriminated_arms(&code, "Pet");
    assert_eq!(
        arms.decode,
        [
            ("Cat", vec!["kitty", "Cat"]),
            ("Dog", vec!["doggo", "Dog"]),
            ("Fish", vec!["fishy", "Fish"]),
        ]
        .map(|(variant, tags)| (
            variant.to_owned(),
            tags.into_iter().map(str::to_owned).collect()
        ))
        .to_vec(),
        "{code}"
    );
    assert_eq!(
        arms.encode,
        [("Cat", "kitty"), ("Dog", "doggo"), ("Fish", "fishy")]
            .map(|(variant, tag)| (variant.to_owned(), Some(tag.to_owned())))
            .to_vec(),
        "{code}"
    );
    let checked = check(&spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(checked.diagnostics().is_empty(), "{checked:#?}");
}

/// The places a `discriminator` can sit with no `oneOf`/`anyOf` of its own, each with the pointer
/// of the Discriminator Object. `mapping` is the body of its `mapping`, spliced in at the depth
/// each placement needs.
fn standalone_discriminators(mapping: &str) -> Vec<(&'static str, String, &'static str)> {
    vec![
        (
            "allOf parent",
            with_schemas(
                "3.1.0",
                &format!(
                    "    Pet:\n      type: object\n      required: [kind]\n      \
                     properties: {{ kind: {{ type: string }} }}\n      \
                     discriminator:\n        propertyName: kind\n        \
                     mapping: {{ {mapping} }}\n    \
                     Kitten:\n      allOf:\n        - $ref: '#/components/schemas/Pet'\n        \
                     - {{ properties: {{ meow: {{ type: boolean }} }} }}\n"
                ),
            ),
            "/components/schemas/Pet/discriminator",
        ),
        (
            "inline allOf member",
            with_schemas(
                "3.1.0",
                &format!(
                    "    Pet:\n      allOf:\n        \
                     - type: object\n          properties: {{ kind: {{ type: string }} }}\n          \
                     discriminator: {{ propertyName: kind, mapping: {{ {mapping} }} }}\n"
                ),
            ),
            "/components/schemas/Pet/allOf/0/discriminator",
        ),
        (
            "beside a $ref",
            with_schemas(
                "3.1.0",
                &format!(
                    "    Pet:\n      $ref: '#/components/schemas/Cat'\n      \
                     discriminator: {{ propertyName: kind, mapping: {{ {mapping} }} }}\n"
                ),
            ),
            "/components/schemas/Pet/discriminator",
        ),
        (
            "on a union's $ref member",
            with_schemas(
                "3.1.0",
                &format!(
                    "    Pet:\n      oneOf:\n        \
                     - $ref: '#/components/schemas/Cat'\n          \
                     discriminator: {{ propertyName: kind, mapping: {{ {mapping} }} }}\n        \
                     - {{ type: string }}\n"
                ),
            ),
            "/components/schemas/Pet/oneOf/0/discriminator",
        ),
        (
            "multi-type array",
            with_schemas(
                "3.1.0",
                &format!(
                    "    Pet:\n      type: [object, string]\n      \
                     discriminator: {{ propertyName: kind, mapping: {{ {mapping} }} }}\n"
                ),
            ),
            "/components/schemas/Pet/discriminator",
        ),
    ]
}

/// Issue #264: a `discriminator` on a schema with no `oneOf`/`anyOf` of its own — the `allOf`
/// polymorphism form above all — was ignored with no diagnostic. spargen dispatches by tag only
/// across a union's members, and no keyword of a parent lists the children that reach it through
/// `allOf`, so the form is not generated: the schema lowers by its other keywords, and the
/// discriminator is acknowledged as `W011` at its own site. A mapping naming declared schemas is
/// otherwise fine — with no union there is no membership to check, so no `E007` either.
#[test]
fn a_discriminator_beside_no_union_is_w011_at_the_discriminator() {
    for (what, spec, pointer) in standalone_discriminators("cat: Cat") {
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{what}: {report:#?}\n{spec}"
        );
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            let w011: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .collect();
            assert_eq!(
                w011.len(),
                1,
                "{what} via {entry}: one W011, at the discriminator: {report:#?}"
            );
            assert_eq!(w011[0].pointer.as_str(), pointer, "{what} via {entry}");
            assert!(
                w011[0].message.contains("`discriminator`"),
                "{what} via {entry}: {:?}",
                w011[0].message
            );
            assert!(
                !has_code(report, Code::UnresolvedRef) && !has_code(report, Code::NonDisjointUnion),
                "{what} via {entry}: every mapping target is declared: {report:#?}"
            );
        }
        assert!(
            !code.contains("\"cat\""),
            "{what}: an inert discriminator dispatches on nothing:\n{code}"
        );
    }
}

/// The #124 rule, carried to the discriminator #264 found unchecked: a `mapping` or
/// `defaultMapping` value naming no schema is an unresolved reference (`E004`) at the entry,
/// whether or not a union sits beside the discriminator. The `allOf`-parent case is the issue's
/// own reproduction, which `check` used to call clean.
#[test]
fn a_discriminator_beside_no_union_naming_an_undeclared_schema_is_e004_at_the_entry() {
    let mut cases: Vec<_> =
        standalone_discriminators("cat: Cat, c: '#/components/schemas/MissingC'")
            .into_iter()
            .map(|(what, spec, pointer)| (what, spec, format!("{pointer}/mapping/c")))
            .collect();
    cases.push((
        "3.2 defaultMapping",
        with_schemas(
            "3.2.0",
            "    Pet:\n      type: object\n      \
             discriminator: { propertyName: kind, defaultMapping: MissingC }\n",
        ),
        "/components/schemas/Pet/discriminator/defaultMapping".to_owned(),
    ));
    for (what, spec, pointer) in cases {
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
                    .any(|d| d.code == Code::UnresolvedRef
                        && d.pointer.as_str() == pointer
                        && d.message.contains("MissingC")),
                "{what} via {entry}: E004 must sit at `{pointer}` and name `MissingC`: \
                 {report:#?}"
            );
            assert!(
                !has_code(&report, Code::NonDisjointUnion),
                "{what} via {entry}: {report:#?}"
            );
        }
    }
}

/// Issue #419: the `discriminator` beside a union that `allOf` shadowed went unread, so a `mapping`
/// naming no schema was never reported. It belongs to the union, and is checked as the union's own.
#[test]
fn a_discriminator_beside_all_of_and_a_union_has_its_mapping_checked() {
    let spec = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf: [ { description: a pet } ]\n      \
         oneOf: [ { $ref: '#/components/schemas/Cat' } ]\n      \
         discriminator: { propertyName: kind, mapping: { x: Nope } }\n",
    );
    for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: {report:#?}"
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::UnresolvedRef
                    && d.pointer.as_str() == "/components/schemas/Pet/discriminator/mapping/x"
                    && d.message.contains("Nope")),
            "via {entry}: E004 must sit at the mapping entry and name `Nope`: {report:#?}"
        );
    }
}

/// The dispatch a discriminated union `name` emits, read back from the generated source: per
/// decode arm, the variant and every tag its pattern matches, and per encode arm, the variant and
/// the tag serialization writes (`None` for one it writes no tag for).
struct DiscriminatedArms {
    decode: Vec<(String, Vec<String>)>,
    encode: Vec<(String, Option<String>)>,
}

fn discriminated_arms(code: &str, name: &str) -> DiscriminatedArms {
    let de_impl = code
        .split(&format!("impl<'de> serde::Deserialize<'de> for {name} {{"))
        .nth(1)
        .unwrap_or_else(|| panic!("no Deserialize for {name}:\n{code}"));
    // Layout is the formatter's business, so read whitespace-collapsed text.
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let de_body = de_impl
        .split("match tag.as_str() {")
        .nth(1)
        .and_then(|rest| rest.split("_ =>").next())
        .map(flat)
        .unwrap_or_else(|| panic!("no tag dispatch for {name}:\n{code}"));
    let pieces: Vec<&str> = de_body
        .split(&format!("serde_json::from_value(value) .map({name}::"))
        .collect();
    let decode = pieces
        .windows(2)
        .map(|pair| {
            let pattern = pair[0].rsplit_once(" =>").map_or(pair[0], |(head, _)| head);
            let pattern = pattern.rsplit(['}', ',']).next().unwrap_or(pattern);
            let tags = pattern
                .split('|')
                .map(|tag| tag.trim().trim_matches('"').to_owned())
                .filter(|tag| !tag.is_empty())
                .collect();
            let variant = pair[1].split(')').next().unwrap_or_default().to_owned();
            (variant, tags)
        })
        .collect();
    let ser_impl = code
        .split(&format!("impl serde::Serialize for {name} {{"))
        .nth(1)
        .and_then(|rest| rest.split("if let Some(tag) = tag").next())
        .map(flat)
        .unwrap_or_else(|| panic!("no Serialize for {name}:\n{code}"));
    let pieces: Vec<&str> = ser_impl.split("(inner) =>").collect();
    let encode = pieces
        .windows(2)
        .map(|pair| {
            let variant = pair[0]
                .rsplit(&format!("{name}::"))
                .next()
                .unwrap_or_default();
            let tag = pair[1]
                .split_once("?,")
                .and_then(|(_, rest)| rest.split(',').next())
                .unwrap_or_default()
                .trim();
            let tag = tag
                .strip_prefix("Some(")
                .map(|tag| tag.trim_end_matches(')').trim_matches('"').to_owned());
            (variant.trim().to_owned(), tag)
        })
        .collect();
    DiscriminatedArms { decode, encode }
}

/// Issue #263: a member several discriminator values name dispatched on only the first. Every
/// `mapping` key naming it selects it, and so does its component name, which no key claims;
/// serialization writes the first key.
#[test]
fn every_discriminator_value_naming_a_member_selects_it() {
    let spec = discriminated_pet(
        "3.1.0",
        CAT_AND_DOG,
        "        propertyName: kind\n        mapping:\n          \
         a: Cat\n          \
         d: Dog\n          \
         a2: '#/components/schemas/Cat'\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let arms = discriminated_arms(&code, "Pet");
    assert_eq!(
        arms.decode,
        vec![
            (
                "Cat".to_owned(),
                vec!["a".to_owned(), "a2".to_owned(), "Cat".to_owned()]
            ),
            ("Dog".to_owned(), vec!["d".to_owned(), "Dog".to_owned()]),
        ],
        "{code}"
    );
    assert_eq!(
        arms.encode,
        vec![
            ("Cat".to_owned(), Some("a".to_owned())),
            ("Dog".to_owned(), Some("d".to_owned())),
        ],
        "{code}"
    );
    let checked = check(&spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(checked.diagnostics().is_empty(), "{checked:#?}");
}

/// A component name is read as one only "unless a `mapping` is present for that value": a key
/// equal to `Cat` that names `Dog` makes the value `Cat` select `Dog`, and takes it away from
/// `Cat`, which keeps the key that names it.
#[test]
fn a_mapping_key_equal_to_a_component_name_claims_that_value() {
    let spec = discriminated_pet(
        "3.1.0",
        CAT_AND_DOG,
        "        propertyName: kind\n        mapping:\n          \
         Cat: Dog\n          \
         kitty: Cat\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let arms = discriminated_arms(&code, "Pet");
    assert_eq!(
        arms.decode,
        vec![
            ("Cat".to_owned(), vec!["kitty".to_owned()]),
            ("Dog".to_owned(), vec!["Cat".to_owned(), "Dog".to_owned()]),
        ],
        "{code}"
    );
    assert_eq!(
        arms.encode,
        vec![
            ("Cat".to_owned(), Some("kitty".to_owned())),
            ("Dog".to_owned(), Some("Cat".to_owned())),
        ],
        "{code}"
    );
}

/// The claim above leaves a member nothing to be selected by when no key names it: the generated
/// decoder would have no arm for it, and the tag it serializes would decode as the other member.
/// That is refused (`E007`) at the entry that claims the name, in `check` as in `generate` —
/// unless `defaultMapping` names the member, whose fallback still reaches it.
#[test]
fn a_member_no_discriminator_value_selects_is_e007_at_the_claiming_entry() {
    let spec = discriminated_pet(
        "3.1.0",
        CAT_AND_DOG,
        "        propertyName: kind\n        mapping:\n          \
         Cat: Dog\n",
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let e007: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::NonDisjointUnion)
            .collect();
        assert_eq!(e007.len(), 1, "{report:#?}");
        assert_eq!(
            e007[0].pointer.as_str(),
            "/components/schemas/Pet/discriminator/mapping/Cat",
            "{report:#?}"
        );
        assert!(
            e007[0]
                .message
                .contains("no discriminator value selects it"),
            "{report:#?}"
        );
    }

    let fallback = discriminated_pet(
        "3.2.0",
        CAT_AND_DOG,
        "        propertyName: kind\n        mapping:\n          \
         Cat: Dog\n        \
         defaultMapping: Cat\n",
    );
    let (report, code) = generate_with_code(&fallback);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let arms = discriminated_arms(&code, "Pet");
    assert_eq!(
        arms.decode,
        vec![("Dog".to_owned(), vec!["Cat".to_owned(), "Dog".to_owned()])],
        "{code}"
    );
    assert_eq!(
        arms.encode,
        vec![
            ("Cat".to_owned(), None),
            ("Dog".to_owned(), Some("Cat".to_owned())),
        ],
        "{code}"
    );
    let checked = check(&fallback);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

/// Issue #403: a member that is no schema component — inline, or a pointer into another schema —
/// has no implicit discriminator value ("inline `oneOf` or `anyOf` subschemas are not considered").
/// With no `mapping` entry naming it, the generated client dispatched on a tag spargen made up —
/// the pointer text `Envelope/properties/cat`, or the hint `PetVariant1` — which no server sends,
/// and serialized that tag into the payload. No discriminator value selects such a member: `W011`
/// at the discriminator, in `check` as in `generate`. Beside a tagged member the tag dispatch stays
/// for the tagged one and the untagged one takes no tag; with no tagged member at all the union is
/// decoded by its members' schemas with no tag dispatch. A `mapping` entry naming it, or
/// `defaultMapping` falling back to it, reaches it with no warning.
#[test]
fn a_member_with_no_component_name_and_no_mapping_entry_is_w011_and_takes_no_tag() {
    let spec = |members: &str, discriminator: &str, version: &str| {
        format!(
            "openapi: {version}\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n    \
             Pet:\n      \
             oneOf:\n{members}      \
             discriminator:\n        \
             propertyName: kind\n{discriminator}    \
             Envelope:\n      \
             type: object\n      \
             properties:\n        \
             cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }}, purr: {{ type: string }} }} }}\n    \
             Dog: {{ type: object, required: [kind, bark], properties: {{ kind: {{ type: string }}, bark: {{ type: boolean }} }} }}\n"
        )
    };
    let deep = "        - { $ref: '#/components/schemas/Envelope/properties/cat' }\n        \
                - { $ref: '#/components/schemas/Dog' }\n";
    let inline = "        - { $ref: '#/components/schemas/Dog' }\n        \
                  - { type: object, required: [kind, fins], properties: { kind: { type: string }, fins: { type: integer } } }\n";
    // GitHub's `POST /repos/{owner}/{repo}/check-runs` body: two inline members, each pinning the
    // discriminating property with an `enum` the discriminator never reads.
    let both_inline = "        - { type: object, required: [kind, done], properties: { kind: { enum: [completed] }, done: { type: boolean } } }\n        \
                       - { type: object, properties: { kind: { enum: [queued, in_progress] } } }\n";
    // Beside the tagged `Dog`, the untagged member keeps Dog's dispatch (`dispatches`); with no
    // tagged member at all, the discriminator dispatches nothing.
    for (what, members, subject, dispatches) in [
        (
            "deep pointer",
            deep,
            "union member 0 is no schema component",
            true,
        ),
        (
            "inline",
            inline,
            "union member 1 is no schema component",
            true,
        ),
        (
            "both inline",
            both_inline,
            "union members 0, 1 are no schema components",
            false,
        ),
    ] {
        let spec = spec(members, "", "3.1.0");
        let (generated, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", generated), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{what} {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::NonDisjointUnion),
                "{what} {entry}: {report:#?}"
            );
            let w011: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .collect();
            assert_eq!(w011.len(), 1, "{what} {entry}: {report:#?}");
            assert_eq!(
                w011[0].pointer.as_str(),
                "/components/schemas/Pet/discriminator",
                "{what} {entry}: {report:#?}"
            );
            assert!(
                w011[0].message.contains(subject),
                "{what} {entry}: {report:#?}"
            );
            let consequence = if dispatches {
                "the tag dispatches only to the members a value names"
            } else {
                "this `discriminator` dispatches nothing"
            };
            assert!(
                w011[0].message.contains(consequence),
                "{what} {entry}: {report:#?}"
            );
        }
        if dispatches {
            // `Dog` keeps its implicit tag both ways; the untagged member is selected by no tag
            // and written with none.
            let arms = discriminated_arms(&code, "Pet");
            assert_eq!(
                arms.decode,
                vec![("Dog".to_owned(), vec!["Dog".to_owned()])],
                "{what}\n{code}"
            );
            let written: Vec<_> = arms
                .encode
                .iter()
                .filter_map(|(_, tag)| tag.clone())
                .collect();
            assert_eq!(written, ["Dog"], "{what}\n{code}");
            assert_eq!(arms.encode.len(), 2, "{what}\n{code}");
        } else {
            let de_impl = code
                .split("impl<'de> serde::Deserialize<'de> for Pet {")
                .nth(1)
                .unwrap_or_else(|| panic!("{what}: no Deserialize for Pet\n{code}"));
            let de_impl = de_impl.split("\nimpl").next().unwrap_or(de_impl);
            assert!(!de_impl.contains("match tag"), "{what}\n{de_impl}");
        }
        // Either way no tag is invented: neither the pointer text nor a variant hint.
        for invented in ["\"Envelope/properties/cat\"", "\"PetVariant"] {
            assert!(!code.contains(invented), "{what}: {invented}\n{code}");
        }
    }

    // A mapping entry naming the deep-pointer member gives it the declared tag and nothing else.
    let mapped = spec(
        deep,
        "        mapping:\n          \
         cat: '#/components/schemas/Envelope/properties/cat'\n",
        "3.1.0",
    );
    let (report, code) = generate_with_code(&mapped);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let tags: Vec<Vec<String>> = discriminated_arms(&code, "Pet")
        .decode
        .into_iter()
        .map(|(_, tags)| tags)
        .collect();
    assert_eq!(
        tags,
        [vec!["cat".to_owned()], vec!["Dog".to_owned()]],
        "{code}"
    );

    // `defaultMapping` falling back to it reaches it with no tag of its own.
    let fallback = spec(
        deep,
        "        defaultMapping: '#/components/schemas/Envelope/properties/cat'\n",
        "3.2.0",
    );
    let (report, code) = generate_with_code(&fallback);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let tags: Vec<Vec<String>> = discriminated_arms(&code, "Pet")
        .decode
        .into_iter()
        .map(|(_, tags)| tags)
        .collect();
    assert_eq!(tags, [vec!["Dog".to_owned()]], "{code}");
    assert!(!code.contains("\"Envelope/properties/cat\""), "{code}");
    let checked = check(&fallback);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

/// An untagged member beside tagged ones must not cost the tagged ones their dispatch. Dropping the
/// discriminator for the whole union made `anyOf [Cat, Dog, inline]` decode by priority, then source
/// order, so `{"kind": "Dog", …}` that `Cat` also accepts decoded as `Cat` against its own tag, and
/// `Dog` lost the tag serialization re-inserts. The tag still selects `Cat` and `Dog`, which still
/// write it; the inline member is tried by its schema, with the applicator's semantics, only when
/// the tag is absent or names neither. (`e2e.rs` drives the decoded values.)
#[test]
fn an_untagged_discriminated_member_keeps_the_tag_dispatch_of_the_tagged_ones() {
    let members = "        - { $ref: '#/components/schemas/Cat' }\n        \
                   - { $ref: '#/components/schemas/Dog' }\n        \
                   - { type: object, required: [kind, fins], properties: { kind: { type: string }, fins: { type: integer } } }\n";
    for (applicator, valid) in [("oneOf", "match_count == 1"), ("anyOf", "match_count >= 1")] {
        let spec = discriminated_pet("3.1.0", members, "        propertyName: kind\n").replacen(
            "oneOf:",
            &format!("{applicator}:"),
            1,
        );
        let (generated, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", generated), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{applicator} {entry}: {report:#?}"
            );
            let w011: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .collect();
            assert_eq!(w011.len(), 1, "{applicator} {entry}: {report:#?}");
            assert!(
                w011[0]
                    .message
                    .contains("union member 2 is no schema component")
                    && w011[0]
                        .message
                        .contains("the tag dispatches only to the members a value names"),
                "{applicator} {entry}: {report:#?}"
            );
        }
        let arms = discriminated_arms(&code, "Pet");
        assert_eq!(
            arms.decode,
            vec![
                ("Cat".to_owned(), vec!["Cat".to_owned()]),
                ("Dog".to_owned(), vec!["Dog".to_owned()]),
            ],
            "{applicator}\n{code}"
        );
        assert_eq!(
            arms.encode,
            vec![
                ("Cat".to_owned(), Some("Cat".to_owned())),
                ("Dog".to_owned(), Some("Dog".to_owned())),
                ("PetVariant2".to_owned(), None),
            ],
            "{applicator}\n{code}"
        );
        // The untagged member is the only one tried, after the tag finds no arm, and with the
        // applicator's own match rule.
        let de_impl = code
            .split("impl<'de> serde::Deserialize<'de> for Pet {")
            .nth(1)
            .and_then(|rest| rest.split("impl serde::Serialize for Pet").next())
            .unwrap_or_else(|| panic!("no Deserialize for Pet:\n{code}"));
        let flat = de_impl.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            flat.matches("serde_json::from_value::<").count(),
            2,
            "{applicator}: one attempt in each of the absent and unknown tag paths\n{flat}"
        );
        assert!(flat.contains(valid), "{applicator}\n{flat}");
        let tried = flat
            .split("match tag.as_str() {")
            .nth(1)
            .and_then(|rest| rest.split("_ =>").nth(1))
            .unwrap_or_else(|| panic!("{applicator}: no unknown-tag arm\n{flat}"));
        assert!(
            tried.contains("Pet::PetVariant2(inner)") && !tried.contains("Pet::Cat(inner)"),
            "{applicator}\n{flat}"
        );
    }
}

/// A Discriminator Object whose fields have the wrong shape was read leniently and the bad part
/// thrown away: a missing `propertyName` became the empty tag field, a non-string mapping value
/// vanished from the map, a non-object `mapping` became no mapping at all. Each is a malformed
/// document (`E011`), reported where the bad field sits.
#[test]
fn a_malformed_discriminator_object_is_e011_at_the_field() {
    let cases = [
        (
            "missing propertyName",
            "        mapping: { cat: Cat }\n",
            "/components/schemas/Pet/discriminator",
        ),
        (
            "non-string propertyName",
            "        propertyName: 5\n",
            "/components/schemas/Pet/discriminator/propertyName",
        ),
        (
            "non-string mapping value",
            "        propertyName: kind\n        mapping: { cat: 5 }\n",
            "/components/schemas/Pet/discriminator/mapping/cat",
        ),
        (
            "non-object mapping",
            "        propertyName: kind\n        mapping: [Cat]\n",
            "/components/schemas/Pet/discriminator/mapping",
        ),
        (
            "non-string defaultMapping",
            "        propertyName: kind\n        defaultMapping: 5\n",
            "/components/schemas/Pet/discriminator/defaultMapping",
        ),
    ];
    for (what, discriminator, pointer) in cases {
        let spec = discriminated_pet("3.2.0", CAT_AND_DOG, discriminator);
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
                    .any(|d| d.code == Code::InvalidInput && d.pointer.as_str() == pointer),
                "{what} via {entry}: E011 must sit at `{pointer}`: {report:#?}"
            );
        }
    }
}

/// A union member written as a pointer into a component (`#/components/schemas/Envelope/properties/
/// payload`) is a member like any other, and a `mapping` value naming it — in the same-file
/// spelling or the root document's own relative-file spelling — resolves to the same `file#pointer`
/// and supplies its tag. Matching by component name alone could not name such a member at all, so
/// its variant would take an invented tag on the wire; matching by resolved target gives it the
/// one the document declares.
#[test]
fn a_discriminator_mapping_value_spelled_as_a_deep_pointer_names_its_member() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Envelope/properties/payload' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        mapping:
          payload: 'TARGET'
          dog: '#/components/schemas/Dog'
    Envelope:
      type: object
      properties:
        payload:
          type: object
          properties: { kind: { type: string }, id: { type: string } }
          required: [kind]
    Dog:
      type: object
      properties: { kind: { type: string }, bark: { type: string } }
      required: [kind]
"##;
    for target in [
        "#/components/schemas/Envelope/properties/payload",
        "./openapi.yaml#/components/schemas/Envelope/properties/payload",
    ] {
        let spec = spec.replace("TARGET", target);
        let (generated, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", generated), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target} {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().is_empty(),
                "{target} {entry}: {report:#?}"
            );
        }
        // The pointer member has no component name to be selected by as well; `Dog` does (#263).
        let tags: Vec<Vec<String>> = discriminated_arms(&code, "Pet")
            .decode
            .into_iter()
            .map(|(_, tags)| tags)
            .collect();
        assert_eq!(
            tags,
            [vec!["payload"], vec!["dog", "Dog"]]
                .map(|tags| tags.into_iter().map(str::to_owned).collect::<Vec<_>>()),
            "{target}\n{code}"
        );
    }
}

/// A `mapping` value with no `/` or `#` in it is read as a component name, as the specification
/// recommends for a value that is also a legal relative reference (`cat.yaml`). Read that way it
/// names a schema the description does not hold even when a member is the file `$ref: 'cat.yaml'`,
/// so it is `E004` at the entry — as is a name nothing declares (`Ghost`) — while a declared
/// component that is not a member (`Bird`, in either spelling) is `E007` at the entry. Before
/// either rejection the member's tag was silently replaced by an invented one. `./cat.yaml` is the
/// unambiguous file spelling, and it names the file member.
#[test]
fn a_discriminator_mapping_value_naming_no_member_is_rejected_at_the_entry() {
    let root = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /pet:
    get:
      operationId: getPet
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: 'MEMBER' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        mapping:
          meow: 'TARGET'
          dog: Dog
    Cat:
      type: object
      properties: { kind: { type: string }, purr: { type: string } }
      required: [kind]
    Bird:
      type: object
      properties: { kind: { type: string }, wing: { type: string } }
      required: [kind]
    Dog:
      type: object
      properties: { kind: { type: string }, bark: { type: string } }
      required: [kind]
"##;
    let cat = "type: object\nproperties: { kind: { type: string }, purr: { type: string } }\n\
               required: [kind]\n";
    let run = |member: &str, target: &str| {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            root.replace("MEMBER", member).replace("TARGET", target),
        )
        .unwrap();
        std::fs::write(dir.join("cat.yaml"), cat).unwrap();
        let generated = run_generate(&build(dir.join("openapi.yaml"), dir.join("client.rs")));
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
        let code = std::fs::read_to_string(dir.join("client.rs")).unwrap_or_default();
        (generated, checked, code)
    };
    let rejected = [
        ("cat.yaml", "cat.yaml", Code::UnresolvedRef),
        ("#/components/schemas/Cat", "Ghost", Code::UnresolvedRef),
        ("#/components/schemas/Cat", "Bird", Code::NonDisjointUnion),
        (
            "#/components/schemas/Cat",
            "#/components/schemas/Bird",
            Code::NonDisjointUnion,
        ),
    ];
    for (member, target, code) in rejected {
        let (generated, checked, _) = run(member, target);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{member} / {target} {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| d.code == code
                    && d.pointer.as_str() == "/components/schemas/Pet/discriminator/mapping/meow"
                    && d.message.contains(&format!("`{target}`"))),
                "{member} / {target} {entry}: {code:?} must sit at the entry: {report:#?}"
            );
        }
    }

    // The member's own name, bare or as a pointer, and the file member's unambiguous spelling are
    // matched to their member and supply its tag.
    for (member, target) in [
        ("#/components/schemas/Cat", "Cat"),
        ("#/components/schemas/Cat", "#/components/schemas/Cat"),
        ("cat.yaml", "./cat.yaml"),
    ] {
        let (generated, checked, code) = run(member, target);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{member} / {target} {entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::NonDisjointUnion) && !has_code(report, Code::UnresolvedRef),
                "{member} / {target} {entry}: {report:#?}"
            );
        }
        // The mapped tag leads the member's arm, so it is the one serialization writes; a
        // component member also keeps its name (#263), a file member has none to keep.
        let expected = match member {
            "cat.yaml" => vec!["meow".to_owned()],
            _ => vec!["meow".to_owned(), "Cat".to_owned()],
        };
        let arms = discriminated_arms(&code, "Pet");
        assert!(
            arms.decode.iter().any(|(_, tags)| *tags == expected),
            "{member} / {target}: the mapped tag must dispatch\n{code}"
        );
    }
}

/// A union declared in a sub-file whose members are that file's own deep pointers
/// (`#/components/schemas/Envelope/properties/cat`) generated before the root document's same-file
/// deep pointers resolved, and it keeps the output it had: each member is named from its pointer
/// text, and a `mapping` value spelled the same way resolves, relative to the sub-file it is
/// written in, to that member and supplies its tag. A deep pointer is no schema component, so
/// without that `mapping` the members have no tag at all (issue #403): the discriminator
/// dispatches nothing (`W011`) rather than on the pointer text, which no server sends, and the
/// members keep their names.
#[test]
fn a_sub_file_union_of_deep_pointer_members_keeps_its_names_and_mapping() {
    let root = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /pet:
    get:
      operationId: getPet
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: './lib.yaml#/components/schemas/Pet' }
"##;
    let lib = r##"
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Envelope/properties/cat' }
        - { $ref: '#/components/schemas/Envelope/properties/dog' }
      discriminator:
        propertyName: kind
MAPPING
    Envelope:
      type: object
      properties:
        cat:
          type: object
          properties: { kind: { type: string }, purr: { type: string } }
          required: [kind]
        dog:
          type: object
          properties: { kind: { type: string }, bark: { type: string } }
          required: [kind]
"##;
    let mapping = "        mapping:\n          \
                   meow: '#/components/schemas/Envelope/properties/cat'\n          \
                   woof: '#/components/schemas/Envelope/properties/dog'";
    let write = |with: &str| {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), root).unwrap();
        std::fs::write(dir.join("lib.yaml"), lib.replace("MAPPING", with)).unwrap();
        let generated = run_generate(&build(dir.join("openapi.yaml"), dir.join("client.rs")));
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
        (temp, dir, generated, checked)
    };

    let (_temp, dir, generated, checked) = write(mapping);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::NonDisjointUnion),
            "{entry}: {report:#?}"
        );
    }
    let code = std::fs::read_to_string(dir.join("client.rs")).unwrap();
    for expected in [
        "EnvelopePropertiesCat(Box<Cat>)",
        "EnvelopePropertiesDog(Box<Dog>)",
        "\"meow\" => {",
        "\"woof\" => {",
    ] {
        assert!(code.contains(expected), "{expected}\n{code}");
    }
    assert!(!code.contains("\"Envelope/properties/cat\""), "{code}");

    let (_temp, dir, generated, checked) = write("");
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::NonDisjointUnion),
            "{entry}: {report:#?}"
        );
        let w011: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .collect();
        assert_eq!(w011.len(), 1, "{entry}: {report:#?}");
        assert!(
            w011[0]
                .message
                .contains("union members 0, 1 are no schema components"),
            "{entry}: {report:#?}"
        );
    }
    let code = std::fs::read_to_string(dir.join("client.rs")).unwrap();
    for expected in [
        "EnvelopePropertiesCat(Box<Cat>)",
        "EnvelopePropertiesDog(Box<Dog>)",
    ] {
        assert!(code.contains(expected), "{expected}\n{code}");
    }
    assert!(!code.contains("\"Envelope/properties/cat\""), "{code}");
    assert!(!code.contains("\"Envelope/properties/dog\""), "{code}");
}

#[test]
fn discriminated_union_with_mapping_generates() {
    // A `discriminator` with an explicit mapping over object `$ref` variants → an internally-tagged
    // enum. Generates without E007. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Dog:
      type: object
      required: [bark]
      properties: { bark: { type: boolean } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn discriminated_union_with_unique_non_object_category_generates() {
    // A non-object variant dispatches by JSON category while object variants dispatch by tag.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - type: string
      discriminator:
        propertyName: petType
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}
