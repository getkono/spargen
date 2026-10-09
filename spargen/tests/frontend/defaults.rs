//! `default` values: when one is applied, when `W005` reports it, and how intersections merge or
//! drop them.

use super::*;

#[test]
fn w005_schema_default_not_applied_still_generates() {
    let report = generate(W005_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// A representable scalar default on an optional field is applied via serde and must not raise
/// W005 (or any error): generation succeeds and the field is documented with its default.
#[test]
fn representable_scalar_default_applies_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        color:
          type: string
          default: "red"
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// Issue #404: an intersection that narrows a property's type re-types the `default` a member
/// declared for the wider type. Every spelling of the meet is held — `allOf` members over `$ref`s,
/// a `$ref` with sibling `properties`, and two object-typed properties met inside an `allOf` — and
/// so is a `number` narrowed to `integer`. A default the narrowed type still admits is applied as a value
/// of it (an enum variant, not the string it was written as); one it no longer admits is reported
/// at the `default` that wrote it (`W005`) rather than wired into code that cannot compile.
const NARROWED_DEFAULT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      properties:
        valid: { type: string, default: a }
        bad: { type: string, default: zzz }
        ratio: { type: number, default: 3 }
        fraction: { type: number, default: 2.5 }
    Narrow:
      type: object
      properties:
        valid: { enum: [a, b] }
        bad: { enum: [a, b] }
        ratio: { type: integer }
        fraction: { type: integer }
    Both:
      allOf:
        - $ref: '#/components/schemas/Base'
        - $ref: '#/components/schemas/Narrow'
    Nested:
      type: object
      properties:
        inner:
          type: object
          properties:
            bad: { type: string, default: zzz }
    NestedNarrow:
      type: object
      properties:
        inner:
          type: object
          properties:
            bad: { enum: [a, b] }
    NestedBoth:
      allOf:
        - $ref: '#/components/schemas/Nested'
        - $ref: '#/components/schemas/NestedNarrow'
    Sibling:
      $ref: '#/components/schemas/Base'
      properties:
        valid: { enum: [a, b] }
        bad: { enum: [a, b] }
"##;

#[test]
fn a_default_an_intersection_narrows_away_is_reported_not_applied() {
    for (entry, report) in [
        ("generate", generate(NARROWED_DEFAULT_SPEC)),
        ("check", check(NARROWED_DEFAULT_SPEC)),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let mut pointers: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::SchemaDefaultNotApplied)
            .map(|d| d.pointer.as_str())
            .collect();
        pointers.sort_unstable();
        pointers.dedup();
        // `bad` and `fraction` lose their defaults; `valid` and `ratio` keep theirs, re-typed. The
        // nested `bad` is narrowed by the meet of two object-typed properties, not by flattening.
        assert_eq!(
            pointers,
            [
                "/components/schemas/Base/properties/bad/default",
                "/components/schemas/Base/properties/fraction/default",
                "/components/schemas/Nested/properties/inner/properties/bad/default",
            ],
            "{entry}: {report:#?}"
        );
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        for narrowed in ["/components/schemas/Both", "/components/schemas/Sibling"] {
            assert!(
                messages.iter().any(|message| message.contains(narrowed)),
                "{entry}: `{narrowed}` drops `bad`'s default and must say so: {messages:#?}"
            );
        }
    }

    let (report, code) = generate_with_code(NARROWED_DEFAULT_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    // The string literal survives only where the field is still a string: `Base` itself.
    assert_eq!(
        code.matches("Some(\"a\".to_owned())").count(),
        1,
        "only `Base.valid` is still a `String`: {code}"
    );
}

/// Issue #453: a repeated property whose types share no value, and that no side requires, takes an
/// uninhabited type, so no value of it is a value of the `default` one side wrote. That default is
/// dropped like one an intersection narrows the property away from: reported at the `default`
/// that wrote it (`W005`), naming the type whose field drops it, and documented as not applied.
/// Both spellings of the meet are held — `allOf` members, and a `$ref` with sibling `properties`.
const UNINHABITED_DEFAULT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      allOf:
        - type: object
          properties:
            a: { type: string, default: z }
        - type: object
          properties:
            a: { type: integer }
    Base:
      type: object
      properties:
        a: { type: string, default: z }
    Sibling:
      $ref: '#/components/schemas/Base'
      properties:
        a: { type: integer }
"##;

#[test]
fn a_default_an_uninhabited_meet_drops_is_reported_not_applied() {
    for (entry, report) in [
        ("generate", generate(UNINHABITED_DEFAULT_SPEC)),
        ("check", check(UNINHABITED_DEFAULT_SPEC)),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let mut pointers: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::SchemaDefaultNotApplied)
            .map(|d| d.pointer.as_str())
            .collect();
        pointers.sort_unstable();
        pointers.dedup();
        assert_eq!(
            pointers,
            [
                "/components/schemas/Base/properties/a/default",
                "/components/schemas/Pet/allOf/0/properties/a/default",
            ],
            "{entry}: {report:#?}"
        );
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        for dropping in ["/components/schemas/Pet", "/components/schemas/Sibling"] {
            assert!(
                messages.iter().any(|message| message.contains(dropping)),
                "{entry}: `{dropping}` drops `a`'s default and must say so: {messages:#?}"
            );
        }
    }

    let (report, code) = generate_with_code(UNINHABITED_DEFAULT_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    for name in ["Pet", "Sibling"] {
        let start = code
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("`{name}` is emitted: {code}"));
        let body = &code[start..start + code[start..].find('}').unwrap()];
        assert!(
            body.contains("Default (not applied): `\"z\"`.") && !body.contains("default = \""),
            "`{name}.a` documents `z` as not applied and wires no default: {body}"
        );
    }
    // `Base` itself is no intersection: its `a` is still a string, and applies `z`.
    assert_eq!(
        code.matches("Some(\"z\".to_owned())").count(),
        1,
        "only `Base` applies `z`: {code}"
    );
}

/// Issue #545: a meet a required property empties also repeats an optional property `c` whose
/// types share no value, and each side gives `c` a different `default`. The `allOf` spellings merge
/// every property before deciding the meet is empty, so beside the `E013` they report the
/// `default` the merge drops (`W005`); the `$ref`-sibling spelling stopped at the required
/// property and reported `E013` alone. Every spelling now reports the same diagnostics, the `W005`
/// at the `default` `M1`'s side wrote, whether the meet is rejected or, where both sides admit
/// `null`, the null type (#542).
#[test]
fn an_emptied_ref_sibling_meet_reports_the_default_it_drops() {
    let document = |ty: &str, p: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    M0:\n      type: {ty}\n      properties: {{ a: {{ type: string }}, \
             c: {{ type: integer, default: 1 }} }}\n    M1:\n      type: {ty}\n      \
             required: [a]\n      properties: {{ a: {{ type: integer }}, \
             c: {{ type: string, default: x }} }}\n    Holder:\n      type: object\n      \
             required: [p]\n      properties:\n        p: {p}\n"
        )
    };
    for (ty, null_only) in [("object", false), ("[object, 'null']", true)] {
        let m0 = format!(
            "{{ type: {ty}, properties: {{ a: {{ type: string }}, \
             c: {{ type: integer, default: 1 }} }} }}"
        );
        let m1 = format!(
            "type: {ty}, required: [a], properties: {{ a: {{ type: integer }}, \
             c: {{ type: string, default: x }} }}"
        );
        for (what, p, dropped) in [
            (
                "an `allOf` of `$ref`s",
                "{ allOf: [{ $ref: '#/components/schemas/M0' }, \
                 { $ref: '#/components/schemas/M1' }] }"
                    .to_owned(),
                "/components/schemas/M1/properties/c/default",
            ),
            (
                "an inline `allOf`",
                format!("{{ allOf: [{m0}, {{ {m1} }}] }}"),
                "/components/schemas/Holder/properties/p/allOf/1/properties/c/default",
            ),
            (
                "a `$ref` beside the keywords",
                format!("{{ $ref: '#/components/schemas/M0', {m1} }}"),
                "/components/schemas/Holder/properties/p/properties/c/default",
            ),
        ] {
            let spec = document(ty, &p);
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome() == Outcome::Rejected,
                    !null_only,
                    "{ty} {what} {entry}: {report:#?}"
                );
                let mut seen: Vec<(&str, &str)> = report
                    .diagnostics()
                    .iter()
                    .map(|d| (d.code.as_str(), d.pointer.as_str()))
                    .collect();
                seen.sort_unstable();
                let mut expected = vec![(Code::SchemaDefaultNotApplied.as_str(), dropped)];
                if !null_only {
                    expected.push((
                        Code::AllOfIrreconcilable.as_str(),
                        "/components/schemas/Holder/properties/p",
                    ));
                }
                expected.sort_unstable();
                assert_eq!(seen, expected, "{ty} {what} {entry}: {report:#?}");
            }
        }
    }
}

/// Issue #432: an intersection whose sides repeat a property merges the `default` either side
/// declares, whichever side comes first. `allOf: [Narrow, Base]` wires `Base`'s default exactly as
/// `allOf: [Base, Narrow]` does, and so does a `$ref` whose sibling `properties` declare it. Two
/// sides that declare different defaults keep the same one in either order, and the one the merged
/// field cannot carry is reported (`W005`) at the `default` that wrote it.
const MERGED_DEFAULT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Narrow:
      type: object
      properties:
        p: { type: string }
    Base:
      type: object
      properties:
        p: { type: string, default: hello }
    Other:
      type: object
      properties:
        p: { type: string, default: world }
    Combined:
      allOf:
        - $ref: '#/components/schemas/Narrow'
        - $ref: '#/components/schemas/Base'
    Reversed:
      allOf:
        - $ref: '#/components/schemas/Base'
        - $ref: '#/components/schemas/Narrow'
    Sib:
      $ref: '#/components/schemas/Narrow'
      properties:
        p: { type: string, default: hello }
    Clash:
      allOf:
        - $ref: '#/components/schemas/Base'
        - $ref: '#/components/schemas/Other'
    ClashReversed:
      allOf:
        - $ref: '#/components/schemas/Other'
        - $ref: '#/components/schemas/Base'
    ClashSibling:
      $ref: '#/components/schemas/Other'
      properties:
        p: { type: string, default: hello }
"##;

#[test]
fn an_intersection_merges_a_default_either_side_declares() {
    for (entry, report) in [
        ("generate", generate(MERGED_DEFAULT_SPEC)),
        ("check", check(MERGED_DEFAULT_SPEC)),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let mut pointers: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::SchemaDefaultNotApplied)
            .map(|d| d.pointer.as_str())
            .collect();
        pointers.sort_unstable();
        pointers.dedup();
        // Only the conflicts report, and in every order at the default they do not keep: `hello`
        // is kept over `world`, so `Other`'s is the one reported.
        assert_eq!(
            pointers,
            ["/components/schemas/Other/properties/p/default"],
            "{entry}: {report:#?}"
        );
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        for kept in [
            "/components/schemas/Base/properties/p/default",
            "/components/schemas/ClashSibling/properties/p/default",
        ] {
            assert!(
                messages.iter().any(|message| message.contains(kept)),
                "{entry}: the report names the default `{kept}` kept instead: {messages:#?}"
            );
        }
    }

    let (report, code) = generate_with_code(MERGED_DEFAULT_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    for name in [
        "Base",
        "Combined",
        "Reversed",
        "Sib",
        "Clash",
        "ClashReversed",
        "ClashSibling",
    ] {
        let start = code
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("`{name}` is emitted: {code}"));
        let body = &code[start..start + code[start..].find('}').unwrap()];
        assert!(
            body.contains("Default: `hello`.") && body.contains("default = \"default_"),
            "`{name}.p` documents and applies `hello`: {body}"
        );
    }
    assert_eq!(
        code.matches("Some(\"world\".to_owned())").count(),
        1,
        "only `Other` itself applies `world`: {code}"
    );
}

/// Issue #432: of two different defaults an intersection's sides declare, the one the side can
/// apply as a serde default is kept, whatever its value and in either order. `NullP` declares `p`
/// nullable and `ReqP` declares it required, so neither applies its `aaa`; `PlainP` declares it
/// optional and applies `zzz`. Were the values compared first, `aaa` would be kept. The nullable
/// meet narrows to a plain optional `String`, so the kept `zzz` is wired; the required meet keeps
/// the requirement, so `zzz` is only documented.
const APPLICABLE_DEFAULT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    PlainP:
      type: object
      properties:
        p: { type: string, default: zzz }
    NullP:
      type: object
      properties:
        p: { type: [string, 'null'], default: aaa }
    ReqP:
      type: object
      required: [p]
      properties:
        p: { type: string, default: aaa }
    NullClash:
      allOf:
        - $ref: '#/components/schemas/NullP'
        - $ref: '#/components/schemas/PlainP'
    NullClashReversed:
      allOf:
        - $ref: '#/components/schemas/PlainP'
        - $ref: '#/components/schemas/NullP'
    ReqClash:
      allOf:
        - $ref: '#/components/schemas/ReqP'
        - $ref: '#/components/schemas/PlainP'
    ReqClashReversed:
      allOf:
        - $ref: '#/components/schemas/PlainP'
        - $ref: '#/components/schemas/ReqP'
"##;

#[test]
fn a_conflicting_intersection_default_keeps_the_one_its_side_can_apply() {
    for (entry, report) in [
        ("generate", generate(APPLICABLE_DEFAULT_SPEC)),
        ("check", check(APPLICABLE_DEFAULT_SPEC)),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let mut pointers: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::SchemaDefaultNotApplied)
            .map(|d| d.pointer.as_str())
            .collect();
        pointers.sort_unstable();
        pointers.dedup();
        // In both orders the unapplied `aaa` is the dropped one, never `PlainP`'s `zzz`.
        assert_eq!(
            pointers,
            [
                "/components/schemas/NullP/properties/p/default",
                "/components/schemas/ReqP/properties/p/default",
            ],
            "{entry}: {report:#?}"
        );
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        assert!(
            messages
                .iter()
                .all(|message| message.contains("/components/schemas/PlainP/properties/p/default")),
            "{entry}: every report names `PlainP`'s default as the one kept: {messages:#?}"
        );
    }

    let (report, code) = generate_with_code(APPLICABLE_DEFAULT_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let body = |name: &str| {
        let start = code
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("`{name}` is emitted: {code}"));
        code[start..start + code[start..].find('}').unwrap()].to_owned()
    };
    for name in ["NullClash", "NullClashReversed"] {
        let body = body(name);
        assert!(
            body.contains("Default: `zzz`.")
                && !body.contains("Default: `aaa`.")
                && body.contains("default = \"default_"),
            "`{name}.p` documents and applies `zzz`: {body}"
        );
    }
    for name in ["ReqClash", "ReqClashReversed"] {
        let body = body(name);
        assert!(
            body.contains("Default: `zzz`.") && !body.contains("Default: `aaa`."),
            "`{name}.p` documents `zzz`: {body}"
        );
    }
}

/// Issue #432: an `integer` default `3` and a `number` default `3` are one default, not a
/// conflict, though the two sides classify it as an integer and as a float. The intersection keeps
/// it without a `W005`, in either order.
const NUMERIC_DEFAULT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    IntP:
      type: object
      properties:
        p: { type: integer, default: 3 }
    NumP:
      type: object
      properties:
        p: { type: number, default: 3 }
    Both:
      allOf:
        - $ref: '#/components/schemas/IntP'
        - $ref: '#/components/schemas/NumP'
    BothReversed:
      allOf:
        - $ref: '#/components/schemas/NumP'
        - $ref: '#/components/schemas/IntP'
"##;

#[test]
fn an_integer_and_a_number_default_of_equal_value_are_one_default() {
    for (entry, report) in [
        ("generate", generate(NUMERIC_DEFAULT_SPEC)),
        ("check", check(NUMERIC_DEFAULT_SPEC)),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::SchemaDefaultNotApplied),
            "{entry}: `3` and `3` do not conflict: {report:#?}"
        );
    }

    let (report, code) = generate_with_code(NUMERIC_DEFAULT_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    for name in ["Both", "BothReversed"] {
        let start = code
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("`{name}` is emitted: {code}"));
        let body = &code[start..start + code[start..].find('}').unwrap()];
        assert!(
            body.contains("default = \"default_"),
            "`{name}.p` applies the merged default: {body}"
        );
    }
}

/// Issue #543: equal `default`s several intersected members write are one value for choosing what
/// the merged field keeps, not for accounting. When the meet drops that value — the field is
/// uninhabited, or another member's different `default` is kept — `W005` is reported at every
/// pointer that wrote it, in every member order, not only at the one the merge reached first.
#[test]
fn a_dropped_default_several_members_write_alike_is_reported_at_each_pointer() {
    fn spec(third: &str, order: [usize; 3]) -> String {
        let members = [
            "{ type: object, properties: { a: { type: string, default: y } } }".to_owned(),
            "{ properties: { a: { type: string, default: y } } }".to_owned(),
            format!("{{ properties: {{ a: {third} }} }}"),
        ];
        let mut spec = String::from(
            "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
             schemas:\n",
        );
        for (id, member) in members.iter().enumerate() {
            spec.push_str(&format!("    M{id}: {member}\n"));
        }
        let refs: Vec<String> = order
            .iter()
            .map(|id| format!("{{ $ref: '#/components/schemas/M{id}' }}"))
            .collect();
        spec.push_str(&format!(
            "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
             p: {{ allOf: [{}] }}\n",
            refs.join(", ")
        ));
        spec
    }

    // An uninhabited `a` (`string` meets `integer`) drops `y`; a kept `x` supersedes it.
    for third in ["{ type: integer }", "{ type: string, default: x }"] {
        for order in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
            let spec = spec(third, order);
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
                let mut pointers: Vec<&str> = report
                    .diagnostics()
                    .iter()
                    .filter(|d| d.code == Code::SchemaDefaultNotApplied)
                    .map(|d| d.pointer.as_str())
                    .collect();
                pointers.sort_unstable();
                pointers.dedup();
                assert_eq!(
                    pointers,
                    [
                        "/components/schemas/M0/properties/a/default",
                        "/components/schemas/M1/properties/a/default",
                    ],
                    "{entry}, {third}, order {order:?}: {report:#?}\n{spec}"
                );
            }
        }
    }
}

/// Issue #577: which of the intersected members' `default`s the merged field keeps is decided once
/// over every member, not pair by pair. A pairwise fold reported a `default` as superseded as soon
/// as one pair ranked it lower, before a later member's equal value outranked the one that beat
/// it: here M0's `x` beat M1's `y` (both are required, so neither is applied), then M2's applied
/// `y` beat `x`, and the field kept `y` while M1's `y` stood reported as differing from it. In
/// every member order, `W005` is reported exactly at M0's superseded `x` and at M1's `c`, whose
/// `string`/`integer` field is uninhabited.
#[test]
fn a_default_equal_to_the_one_the_meet_keeps_is_never_reported_in_any_member_order() {
    fn spec(order: [usize; 3]) -> String {
        let members = [
            "{ type: object, required: [a], properties: { a: { type: string, default: x }, \
             c: { type: string } } }",
            "{ type: [object, 'null'], required: [a], properties: { a: { type: string, \
             default: y }, c: { type: integer, default: 1 } } }",
            "{ properties: { a: { type: string, default: y } } }",
        ];
        let mut spec = String::from(
            "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
             schemas:\n",
        );
        for (id, member) in members.iter().enumerate() {
            spec.push_str(&format!("    M{id}: {member}\n"));
        }
        let refs: Vec<String> = order
            .iter()
            .map(|id| format!("{{ $ref: '#/components/schemas/M{id}' }}"))
            .collect();
        spec.push_str(&format!(
            "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
             p: {{ allOf: [{}] }}\n",
            refs.join(", ")
        ));
        spec
    }

    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in orders {
        let spec = spec(order);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
            let mut pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::SchemaDefaultNotApplied)
                .map(|d| d.pointer.as_str())
                .collect();
            pointers.sort_unstable();
            assert_eq!(
                pointers,
                [
                    "/components/schemas/M0/properties/a/default",
                    "/components/schemas/M1/properties/c/default",
                ],
                "{entry}, order {order:?}: {report:#?}\n{spec}"
            );
            if let Some(superseded) = report
                .diagnostics()
                .iter()
                .find(|d| d.pointer.as_str() == "/components/schemas/M0/properties/a/default")
            {
                assert!(
                    superseded.message.contains("M2/properties/a/default"),
                    "{entry}, order {order:?}: the kept `y` is the applicable one M2 writes: \
                     {superseded:#?}"
                );
            }
        }
    }
}

/// A parameter `default` is documented in rustdoc (never serde-wired) — generation is clean and
/// must NOT raise W005 (parameters always have a documentation home).
#[test]
fn parameter_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      parameters:
        - name: per_page
          in: query
          schema: { type: integer, default: 30 }
        - name: sort
          in: query
          required: true
          schema: { type: string, default: name }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` on a component schema itself (here an enum) is documented on the generated named
/// type — generation is clean, with no W005 and no double-handling.
#[test]
fn component_root_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` in a structural position with no field home — array `items` and an
/// `additionalProperties` value — is non-silent: it fires W005 and still generates.
#[test]
fn structural_defaults_fire_w005_and_still_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Tags:
      type: array
      items: { type: string, default: hi }
    Counts:
      type: object
      additionalProperties: { type: integer, default: 5 }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// An out-of-range integer default for the field's width (`int32` here) is NOT representable: it
/// must fire W005 and stay rustdoc-only, never rendered into a literal that fails to compile.
#[test]
fn out_of_range_int_default_fires_w005_and_is_not_wired() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        big:
          type: integer
          format: int32
          default: 5000000000
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// A component that is a bare `$ref` with a sibling `default` drops the default when the reference
/// resolves; it must be acknowledged with W005 rather than lost silently, and still generate.
#[test]
fn component_root_ref_with_default_fires_w005_and_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Bar:
      type: string
    Alias:
      $ref: "#/components/schemas/Bar"
      default: aliased
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}
