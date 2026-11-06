//! Discriminators over inline members: the tag value a member pins on its tag property, a
//! `defaultMapping` member admitting a pinned value, and an untagged member beside tagged ones.

use super::discriminator::{discriminated_arms, discriminated_pet};
use super::*;

/// Issue #1001: an internally tagged Rust enum's OpenAPI output — every branch inline, each pinning
/// the required tag property with a `const` or a one-value `enum` — had no member a discriminator
/// value selected, so it was `W011` and decoded by trial. Each member's pinned value selects it and
/// is the tag it writes, with no diagnostic, in `check` as in `generate`. The value is read from the
/// lowered member, so a pin a `$ref`d property or an `allOf` conjunct supplies counts too.
#[test]
fn an_inline_member_pinning_its_tag_property_is_selected_by_that_value() {
    let spec = |members: &str, discriminator: &str| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n    \
             Config:\n      \
             oneOf:\n{members}      \
             discriminator:\n        \
             propertyName: type\n{discriminator}    \
             Base: {{ type: object, required: [url], properties: {{ url: {{ type: string }} }} }}\n    \
             Email: {{ type: string, const: email }}\n"
        )
    };
    let members = "        - { type: object, required: [type, url], properties: { type: { type: string, const: webhook }, url: { type: string } } }\n        \
                   - { type: object, required: [type, url], properties: { type: { type: string, enum: [apprise] }, url: { type: string } } }\n        \
                   - { type: object, required: [type, to], properties: { type: { $ref: '#/components/schemas/Email' }, to: { type: string } } }\n        \
                   - allOf:\n            \
                       - { $ref: '#/components/schemas/Base' }\n            \
                       - { type: object, required: [type], properties: { type: { const: ntfy } } }\n";
    let spec = spec(members, "");
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let arms = discriminated_arms(&code, "Config");
    let expected = [
        ("Webhook", "webhook"),
        ("Apprise", "apprise"),
        ("Email", "email"),
        ("Ntfy", "ntfy"),
    ];
    assert_eq!(
        arms.decode,
        expected
            .iter()
            .map(|(variant, tag)| ((*variant).to_owned(), vec![(*tag).to_owned()]))
            .collect::<Vec<_>>(),
        "{code}"
    );
    assert_eq!(
        arms.encode,
        expected
            .iter()
            .map(|(variant, tag)| ((*variant).to_owned(), Some((*tag).to_owned())))
            .collect::<Vec<_>>(),
        "{code}"
    );
}

/// Issue #1232: an inline member its pinned tag selects is named from that tag, variant and
/// struct alike (`Config::Webhook(ConfigWebhook)`), with its nested inline types under the same
/// prefix (`ConfigWebhookUrl`), and its struct leaves out the tag property: the property admits
/// one value, which the union writes back. A struct that denies unknown properties keeps the
/// field, since the payload it is handed carries the tag. A member whose pinned tag starts with no
/// ASCII letter, and one whose pin another member claims, keep the numbered name and the field.
#[test]
fn an_inline_member_selected_by_its_pinned_tag_is_named_from_it_and_leaves_the_tag_out() {
    let spec = |members: &str| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n    \
             Config:\n      \
             oneOf:\n{members}      \
             discriminator:\n        \
             propertyName: type\n    \
             Mail: {{ type: object, required: [type, to], properties: {{ type: {{ type: string }}, to: {{ type: string }} }} }}\n"
        )
    };
    let webhook = "        - { type: object, required: [type, url], properties: { type: { const: webhook }, url: { type: string }, retry: { oneOf: [{ type: string }, { type: integer }] } } }\n";
    let members = format!(
        "{webhook}        \
         - {{ type: object, required: [type, port], additionalProperties: false, properties: {{ type: {{ const: apprise }}, port: {{ type: integer }} }} }}\n        \
         - {{ type: object, required: [type, to], properties: {{ type: {{ const: 2fa }}, to: {{ type: string }} }} }}\n        \
         - {{ type: object, required: [type, to], properties: {{ type: {{ const: Mail }}, to: {{ type: string }} }} }}\n        \
         - {{ $ref: '#/components/schemas/Mail' }}\n"
    );
    let (report, code) = generate_with_code(&spec(&members));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let arms = discriminated_arms(&code, "Config");
    assert_eq!(
        arms.encode,
        [
            ("Webhook", Some("webhook")),
            ("Apprise", Some("apprise")),
            ("ConfigVariant2", Some("2fa")),
            ("ConfigVariant3", None),
            ("Mail", Some("Mail")),
        ]
        .iter()
        .map(|(variant, tag)| ((*variant).to_owned(), tag.map(str::to_owned)))
        .collect::<Vec<_>>(),
        "{code}"
    );
    let types = types_module(&code);
    assert!(
        types.contains("Webhook(Box<ConfigWebhook>),")
            && types.contains("Apprise(Box<ConfigApprise>),"),
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "ConfigWebhook"),
        ["url", "retry"],
        "{types}"
    );
    // A nested inline union keeps the tag's prefix, for its type and its own variants.
    assert!(types.contains("pub enum ConfigWebhookRetry {"), "{types}");
    assert_eq!(
        alias_target(&types, "ConfigWebhookUrl").as_deref(),
        Some("String"),
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "ConfigApprise"),
        ["r#type", "port"],
        "a struct denying unknown properties keeps its tag: {types}"
    );
    assert_eq!(
        declared_fields(&types, "ConfigVariant2"),
        ["r#type", "to"],
        "a numbered variant keeps its tag: {types}"
    );
    assert_eq!(
        declared_fields(&types, "ConfigVariant3"),
        ["r#type", "to"],
        "an untagged variant keeps its tag: {types}"
    );
    assert!(!types.contains("ConfigVariant0"), "{types}");
}

/// The pinned value of #1001 selects a member only where it is the one value every payload the
/// member accepts carries, and nothing else claims it. A tag property that is optional, nullable,
/// or admits several values pins nothing; a value a `mapping` key, another member's tag, or another
/// untagged member whose tag property admits it (a second pin, a plain string, or no declaration of
/// the property on an open object) claims is kept by none of the inline members. Each such member is the untagged one
/// `W011` names, exactly as before, while the member that does pin a free value keeps its dispatch.
#[test]
fn a_tag_property_that_pins_no_free_value_leaves_the_member_untagged() {
    let union = |second: &str, mapping: &str| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n    \
             Config:\n      \
             oneOf:\n        \
             - {{ type: object, required: [type, url], properties: {{ type: {{ const: webhook }}, url: {{ type: string }} }} }}\n        \
             - {second}\n        \
             - {{ $ref: '#/components/schemas/Mail' }}\n      \
             discriminator:\n        \
             propertyName: type\n{mapping}    \
             Mail: {{ type: object, required: [type, to], properties: {{ type: {{ type: string }}, to: {{ type: string }} }} }}\n"
        )
    };
    for (what, second, mapping, tagged) in [
        (
            "optional",
            "{ type: object, required: [port], properties: { type: { const: apprise }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
        (
            "nullable",
            "{ type: object, required: [type, port], properties: { type: { type: [string, 'null'], enum: [apprise, null] }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
        (
            "several values",
            "{ type: object, required: [type, port], properties: { type: { enum: [apprise, gotify] }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
        (
            "a component's tag",
            "{ type: object, required: [type, port], properties: { type: { const: Mail }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
        (
            "a mapping key",
            "{ type: object, required: [type, port], properties: { type: { const: apprise }, port: { type: integer } } }",
            "        mapping:\n          apprise: Mail\n",
            &[("Mail", "apprise|Mail"), ("Webhook", "webhook")][..],
        ),
        (
            "another member's pin",
            "{ type: object, required: [type, port], properties: { type: { const: webhook }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail")][..],
        ),
        // An untagged member is tried by its schema when the tag selects no tagged member, so
        // one whose tag property also admits `webhook` claims it: `{type: webhook, port: 1}`
        // decodes as that member, not as the one pinning `webhook`.
        (
            "a plain string beside it",
            "{ type: object, required: [type, port], properties: { type: { type: string }, port: { type: integer } } }",
            "",
            &[("Mail", "Mail")][..],
        ),
        (
            "an undeclared tag property",
            "{ type: object, required: [port], properties: { port: { type: integer } } }",
            "",
            &[("Mail", "Mail")][..],
        ),
        // A closed object that does not declare the tag property refuses every payload carrying
        // it, so it claims nothing.
        (
            "an undeclared tag property on a closed object",
            "{ type: object, required: [port], properties: { port: { type: integer } }, additionalProperties: false }",
            "",
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
    ] {
        let spec = union(second, mapping);
        let (report, code) = generate_with_code(&spec);
        let untagged = if matches!(
            what,
            "another member's pin" | "a plain string beside it" | "an undeclared tag property"
        ) {
            "union members 0, 1 are no schema components"
        } else {
            "union member 1 is no schema component"
        };
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let w011: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .collect();
        assert_eq!(w011.len(), 1, "{what}: {report:#?}");
        assert!(w011[0].message.contains(untagged), "{what}: {report:#?}");
        let arms = discriminated_arms(&code, "Config");
        let mut decoded: Vec<_> = arms
            .decode
            .iter()
            .map(|(variant, tags)| (variant.as_str(), tags.join("|")))
            .collect();
        decoded.sort();
        let mut expected: Vec<_> = tagged
            .iter()
            .map(|(variant, tag)| (*variant, (*tag).to_owned()))
            .collect();
        expected.sort();
        assert_eq!(decoded, expected, "{what}\n{code}");
    }
}

/// The 3.2 `defaultMapping` member decodes every payload whose tag selects no tagged member, so it
/// claims a pinned value its own tag property admits, as an untagged member does: with `Mail`'s
/// `type` a plain string, `{type: webhook, to: x}` is valid only for `Mail` and reached it through
/// the fallback, and tagging the inline `webhook` pin would route it there and refuse it. That
/// member stays untagged (`W011`). A `Mail` whose tag property is a closed set without `webhook`
/// refuses that payload anyway, so the pin is free and selects its member with no diagnostic.
#[test]
fn a_default_mapping_member_admitting_a_pinned_value_claims_it() {
    let spec = |mail_tag: &str| {
        format!(
            "openapi: 3.2.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n    \
             Config:\n      \
             oneOf:\n        \
             - {{ type: object, required: [type, url], properties: {{ type: {{ const: webhook }}, url: {{ type: string }} }} }}\n        \
             - {{ $ref: '#/components/schemas/Mail' }}\n      \
             discriminator:\n        \
             propertyName: type\n        \
             defaultMapping: Mail\n    \
             Mail: {{ type: object, required: [type, to], properties: {{ type: {mail_tag}, to: {{ type: string }} }} }}\n"
        )
    };
    for (what, mail_tag, warned, tagged) in [
        (
            "a plain string",
            "{ type: string }",
            true,
            &[("Mail", "Mail")][..],
        ),
        (
            "a closed set without the pin",
            "{ enum: [Mail] }",
            false,
            &[("Mail", "Mail"), ("Webhook", "webhook")][..],
        ),
    ] {
        let spec = spec(mail_tag);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let w011: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .collect();
        if warned {
            assert_eq!(w011.len(), 1, "{what}: {report:#?}");
            assert!(
                w011[0]
                    .message
                    .contains("union member 0 is no schema component"),
                "{what}: {report:#?}"
            );
        } else {
            assert!(report.diagnostics().is_empty(), "{what}: {report:#?}");
        }
        let arms = discriminated_arms(&code, "Config");
        let mut decoded: Vec<_> = arms
            .decode
            .iter()
            .map(|(variant, tags)| (variant.as_str(), tags.join("|")))
            .collect();
        decoded.sort();
        let mut expected: Vec<_> = tagged
            .iter()
            .map(|(variant, tag)| (*variant, (*tag).to_owned()))
            .collect();
        expected.sort();
        assert_eq!(decoded, expected, "{what}\n{code}");
    }
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
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{applicator}: {report:#?}"
        );
        let w011: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .collect();
        assert_eq!(w011.len(), 1, "{applicator}: {report:#?}");
        assert!(
            w011[0]
                .message
                .contains("union member 2 is no schema component")
                && w011[0]
                    .message
                    .contains("the tag dispatches only to the members a value names"),
            "{applicator}: {report:#?}"
        );
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
