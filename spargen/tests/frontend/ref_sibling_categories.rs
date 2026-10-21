//! A `$ref` sibling's category and nullability: an applicator sibling establishes its category and
//! keeps the target's `null`, compatible siblings generate, a component root with shapeless
//! siblings is an alias for its target, an unresolvable `$ref` reports `E004` alone, and only a
//! sibling that states `null` decides the union's nullability.

use super::*;

/// #140's two remaining halves of the category rule `E013`'s explain states for an untyped `$ref`
/// sibling. The object and array applicators say nothing about `null` — in 2020-12 they are
/// vacuously satisfied by it — so a nullable target stays nullable through the intersection; and a
/// sibling carrying both kinds with no `type` to pick one is rejected rather than lowered to either
/// category, which would silently discard the other kind's keywords.
#[test]
fn a_ref_sibling_applicator_establishes_its_category_and_keeps_the_targets_null() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n";
    let holder =
        "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
                  p: { $ref: '#/components/schemas/Sibling' }\n";

    for (keyword, target, sibling) in [
        (
            "required",
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "required: [a]",
        ),
        (
            "properties",
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "properties: { b: { type: integer } }",
        ),
        (
            "items",
            "{ type: [array, 'null'], items: { type: number } }",
            "items: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{keyword}: {report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types.contains("pub p: Option<Sibling>"),
            "an untyped `{keyword}` sibling dropped the nullable target's `null`, though the \
             keyword says nothing about it:\n{types}"
        );
    }

    // Keeping the target's `null` must not rescue a category contradiction. Against a nullable
    // target of ANOTHER category the inferred `[category, null]` shares only `null` with it, and
    // typing that as the exact JSON null (`pub type Sibling = ();`) would silently replace a
    // nullable string with a type that decodes nothing else. It is the empty intersection the
    // same sibling has against a non-null string, so it rejects the same way; an untyped
    // `properties` sibling rejected here before the category was inferred at all.
    for (keyword, target, sibling) in [
        ("required", "{ type: [string, 'null'] }", "required: [q]"),
        (
            "properties",
            "{ type: [string, 'null'] }",
            "properties: { b: { type: integer } }",
        ),
        (
            "items",
            "{ type: [string, 'null'] }",
            "items: { type: integer }",
        ),
        (
            "additionalProperties",
            "{ type: [array, 'null'], items: { type: string } }",
            "additionalProperties: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "an untyped `{keyword}` sibling against a nullable target of another category \
                 did not reject: {report:#?}"
            );
            assert_eq!(
                messages_for(&report, Code::AllOfIrreconcilable).len(),
                1,
                "{keyword}: {report:#?}"
            );
        }
    }

    // The exemption from the rule above: a target that is itself exactly `null`. There the
    // intersection's `null` is the target's whole type, not a remnant of a contradiction, and the
    // untyped applicators are vacuously satisfied by `null`, so the sibling keeps the target's type
    // rather than rejecting.
    let spec = format!(
        "{HEAD}    Target: {{ type: 'null' }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [q]\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "an untyped `required` sibling on a `$ref` to an exactly-`null` target rejected: \
             {report:#?}"
        );
        assert!(
            messages_for(&report, Code::AllOfIrreconcilable).is_empty(),
            "{report:#?}"
        );
    }
    let (_, code) = generate_with_code(&spec);
    let types = types_module(&code);
    assert!(
        types.contains("pub type Sibling = ();"),
        "an untyped `required` sibling changed an exactly-`null` target's type:\n{types}"
    );

    // A union target whose branches do not all share the inferred category. Intersecting the whole
    // union with the category would drop every branch of another category (the string one here),
    // so `Sibling` would reject the strings `Target` accepts. The applicators refine the branches of
    // their own category and keep the rest, as beside an inline union (#282): `Sibling` is still a
    // two-branch union, and it differs from `Target` in the branch the keyword reaches. Object and
    // array keywords together are no contradiction there: each set refines its own branches.
    for (keyword, target, sibling) in [
        (
            "required",
            "{ oneOf: [{ type: string }, { type: object, properties: { a: { type: string } } }] }",
            "required: [a]",
        ),
        (
            "items",
            "{ oneOf: [{ type: string }, { type: array, items: { type: number } }] }",
            "items: { type: integer }",
        ),
        (
            "required + items",
            "{ oneOf: [{ type: object, properties: { a: { type: string } } }, { type: array, \
             items: { type: number } }] }",
            "required: [a]\n      items: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "an untyped `{keyword}` sibling on a mixed union target rejected: {report:#?}"
            );
            for code in [Code::AllOfIrreconcilable, Code::DeclarationHasNoEffect] {
                assert!(!has_code(&report, code), "{keyword}: {report:#?}");
            }
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let target = enum_variants(&types, "Target");
        let refined = enum_variants(&types, "Sibling");
        assert_eq!(
            refined.len(),
            2,
            "an untyped `{keyword}` sibling dropped a branch of its union target:\n{types}"
        );
        assert_ne!(
            target, refined,
            "an untyped `{keyword}` sibling changed no branch of its union target:\n{types}"
        );
    }
    // Where no branch of the target union has the category, the sibling constrains nothing the
    // target accepts: `Sibling` is the target's union unchanged, and the keyword is `W011`, as it is
    // beside the inline union.
    let spec = format!(
        "{HEAD}    Target: {{ oneOf: [{{ type: string }}, {{ type: integer }}] }}\n    Sibling:\n      \
         $ref: '#/components/schemas/Target'\n      required: [a]\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
        let messages = messages_for(&report, Code::DeclarationHasNoEffect);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("no branch of its target union has that category"),
            "{messages:?}"
        );
    }
    let (_, code) = generate_with_code(&spec);
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "Target"),
        enum_variants(&types, "Sibling"),
        "{types}"
    );
    // Object and array keywords together beside a target branch that states no category have no
    // single category to establish for it.
    let spec = format!(
        "{HEAD}    Target: {{ oneOf: [{{}}, {{ type: string }}] }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [a]\n      items: {{ type: integer \
         }}\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("states no JSON category"),
            "{messages:?}"
        );
    }
    // A union whose every branch has the inferred category loses no branch, so it still composes.
    let spec = format!(
        "{HEAD}    A: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }}, \
         a: {{ type: string }} }} }}\n    B: {{ type: object, required: [kind], properties: {{ \
         kind: {{ type: string }}, a: {{ type: string }} }} }}\n    Target:\n      oneOf: [{{ \
         $ref: '#/components/schemas/A' }}, {{ $ref: '#/components/schemas/B' }}]\n      \
         discriminator: {{ propertyName: kind }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [a]\n{holder}"
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");

    let spec = format!(
        "{HEAD}    Target: {{ type: object, properties: {{ a: {{ type: string }} }} }}\n    \
         Sibling:\n      $ref: '#/components/schemas/Target'\n      required: [a]\n      \
         items: {{ type: integer }}\n"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("both object keywords") && messages[0].contains("array keywords"),
            "{messages:?}"
        );
    }
}

/// The guard on the two rejections above: only an EMPTY intersection is an error. A sibling that
/// merely narrows its target still lowers to the narrower type and generates, so the new rejection
/// cannot creep into the ordinary applicator case.
#[test]
fn a_compatible_ref_sibling_still_generates() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Count', type: number }
components:
  schemas:
    Count: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    assert_no_untyped_value(&code);

    // The claim in the doc comment above — "lowers to the NARROWER type" — and the whole point of
    // treating `$ref` as an applicator. Outcome assertions cannot see it: they pin when the tool
    // refuses, and this is what it emits when it proceeds.
    //
    // `integer` ∧ `number` is `integer`, so the body must be `i64`. Two substitutions were green
    // before this: taking the SIBLING alone, which is the exact defect this pull request exists to
    // fix, and taking the TARGET alone, which is the behaviour before it. The first widens the body
    // to `f64` — a generated type that accepts values the description forbids — and neither changes
    // an outcome, a code, or introduces `serde_json::Value`.
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`integer` narrowed by `number` must stay `i64`; `f64` would accept values the document \
         forbids: {types}"
    );
    // And the sibling is not simply discarded either: the intersection is taken, so the derived
    // type exists rather than the response naming the target component directly.
    assert!(
        types.contains("pub type Count = i64;"),
        "the target must still be emitted under its own name: {types}"
    );

    // The mirror, and the row that catches the OTHER substitution. Above, `integer` is both the
    // intersection and the target, so taking the target alone happens to give the right answer and
    // is invisible. Here the SIBLING is the narrower side: `number` narrowed by `integer` is
    // `integer`, so target-alone would emit `f64` and widen the body.
    let mirrored = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Measure', type: integer }
components:
  schemas:
    Measure: { type: number }
"##;
    let (report, code) = generate_with_code(mirrored);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`number` narrowed by `integer` must be `i64`; `f64` is the target alone, which is the \
         behaviour before `$ref` was treated as an applicator: {types}"
    );
}

/// Over-rejection is the whole risk of reporting where the code used to drop, and the fixture above
/// pins one shape only — primitive narrowing. These are the other shapes that reach the `$ref` arm
/// and must keep generating. Each is a direction the rejection could creep in, and each lands on a
/// different mechanism: the early exit before any intersection, an intersection that succeeds
/// unchanged, and `intersect_types`' both-sides-nullable rescue, which returns the exact JSON null
/// type rather than `None` and so never reaches the new rejection at all.
#[test]
fn the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', SIBLING }
"##;

    // (what it exercises, the sibling keywords, the target, what must appear in the output)
    let cases: &[(&str, &str, &str, &str)] = &[
        // Validation-only siblings bear no shape, so the `$ref` arm exits before intersecting. They
        // are reported as ignored (`W001`), never as irreconcilable.
        (
            "validation-only siblings",
            "maxLength: 5, pattern: '^a'",
            "{ type: string }",
            "String",
        ),
        // The sibling agrees with its target: the intersection succeeds and is the target's type.
        (
            "an agreeing type",
            "type: string",
            "{ type: string }",
            "String",
        ),
        // Both sides accept null and nothing else is shared. `intersect_types` returns the exact
        // JSON null type — `null` genuinely is the only satisfying value — so this must NOT reject.
        // The decision record lists "collapse to Null when nullable" as rejected; the code does it,
        // and this fixture is why the record now says the code is right.
        (
            "a nullable-only intersection",
            "type: [integer, 'null']",
            "{ type: [string, 'null'] }",
            "()",
        ),
    ];

    for (what, sibling, target, expected) in cases {
        let spec = format!(
            "{HEAD}{}components:\n  schemas:\n    Name: {target}\n",
            PATH.replace("SIBLING", sibling)
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` was rejected, so the new rejection has crept: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` reported E013: {report:#?}"
        );
        // The RESPONSE BODY's own type, resolved through the operation signature. The earlier
        // revision asserted `code.contains("String")` and `code.contains("()")`, and both of those
        // are true of every generated client — `pub fn url(&self) -> String` and `Server0::new()`
        // are always emitted — so the column could not distinguish any behaviour at all.
        let types = types_module(&code);
        let body = code
            .split("ResponseValue<types::")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
            .trim()
            .to_owned();
        assert!(
            types.contains(&format!("pub type {body} = {expected};")),
            "`{what}` must lower its response body to `{expected}`, but `{body}` is not: {types}"
        );
    }

    // The validation-only case is also the one that must still be *acknowledged*: bearing no shape
    // is not the same as being silently dropped.
    let validation_only = format!(
        "{HEAD}{}components:\n  schemas:\n    Name: {{ type: string }}\n",
        PATH.replace("SIBLING", "maxLength: 5, pattern: '^a'")
    );
    assert!(
        has_code(&generate(&validation_only), Code::ValidationKeywordIgnored),
        "a validation-only sibling must still be acknowledged"
    );
}

/// A component whose root is a `$ref` carrying only siblings that bear no shape — an annotation such
/// as `description`/`title`, or a validation keyword such as `maxLength` — is an alias for its
/// target, exactly as the bare `{$ref: T}` component is. It used to panic in release builds
/// (`component root was not the last inserted def`) when the target was declared first, and when
/// the target was declared second it passed the assertion by lifting the TARGET's def out from
/// under the target's own component entry, and the IR invariant check then rejected a valid
/// document (`response body references missing type`).
///
/// Every existing no-shape fixture put the `$ref` in a response body, which never reserves a
/// component root, so none of them could reach this. Both declaration orders are driven because
/// they fail in different ways, and the output is compared with the bare-`$ref` spelling of the
/// same alias: the siblings must change nothing the generated types say.
#[test]
fn a_component_root_ref_with_only_shapeless_siblings_is_an_alias_for_its_target() {
    const HEAD: &str = r##"openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /alias:
    get:
      operationId: getAlias
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Alias' }
  /target:
    get:
      operationId: getTarget
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Target' }
components:
  schemas:
"##;
    let targets = [
        "{ type: string }",
        "{ type: object, required: [id], properties: { id: { type: integer } } }",
    ];
    // `default` rides beside `description` because alone it is the bare spelling: the parser folds a
    // lone `$ref`+`default` into the alias and reports it as `W005` there.
    let siblings = [
        "description: hello",
        "title: Hello",
        "maxLength: 5",
        "description: hello, default: x",
    ];

    for target in targets {
        for sibling in siblings {
            for target_first in [true, false] {
                let target_line = format!("    Target: {target}\n");
                let spec = |alias: &str| {
                    let alias_line = format!("    Alias: {alias}\n");
                    if target_first {
                        format!("{HEAD}{target_line}{alias_line}")
                    } else {
                        format!("{HEAD}{alias_line}{target_line}")
                    }
                };
                let with_sibling = spec(&format!(
                    "{{ $ref: '#/components/schemas/Target', {sibling} }}"
                ));
                let bare = spec("{ $ref: '#/components/schemas/Target' }");
                let what = format!(
                    "`{sibling}` beside a `$ref` to `{target}`, target first: {target_first}"
                );

                let checked = check(&with_sibling);
                assert_ne!(
                    checked.outcome(),
                    Outcome::Rejected,
                    "{what} was rejected by check: {checked:#?}"
                );
                let (report, code) = generate_with_code(&with_sibling);
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what} was rejected by generate: {report:#?}"
                );
                let (bare_report, bare_code) = generate_with_code(&bare);
                assert_ne!(bare_report.outcome(), Outcome::Rejected, "{bare_report:#?}");
                assert_eq!(
                    types_module(&code),
                    types_module(&bare_code),
                    "{what} must lower exactly as the bare `$ref` alias does"
                );
                if sibling.starts_with("maxLength") {
                    assert!(
                        has_code(&report, Code::ValidationKeywordIgnored),
                        "{what}: a validation-only sibling must still be acknowledged: {report:#?}"
                    );
                }
                // The alias has no type of its own to document a default on, so it is dropped —
                // and must say so, as the bare `$ref`+`default` spelling does.
                assert_eq!(
                    has_code(&report, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: `W005` must fire exactly when a default is dropped: {report:#?}"
                );
                assert_eq!(
                    has_code(&checked, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: check must agree with generate on `W005`: {checked:#?}"
                );
            }
        }
    }

    // A self-referential alias names no type at all. The bare spelling reports the cycle as `E004`;
    // a sibling beside it must not change that into a generated placeholder or a panic.
    for alias in [
        "{ $ref: '#/components/schemas/Loop' }",
        "{ $ref: '#/components/schemas/Loop', description: hello }",
    ] {
        let spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Loop' }}\ncomponents:\n  schemas:\n    Loop: {alias}\n"
        );
        for report in [check(&spec), generate(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{alias}`: {report:#?}"
            );
            assert!(
                has_code(&report, Code::UnresolvedRef),
                "`{alias}` must report the alias cycle as E004: {report:#?}"
            );
        }
    }

    // The guard hands the alias to `chain_component_alias`, which routes a relative-file target
    // through `ensure_resolved` rather than `ensure_component`: a file target must lower exactly as
    // its bare spelling does too, in both declaration orders of the root document's two uses.
    let lib = "components:\n  schemas:\n    \
               Target: { type: object, required: [id], properties: { id: { type: integer } } }\n";
    let file_target = "./lib.yaml#/components/schemas/Target";
    for sibling in ["description: hello", "description: hello, default: x"] {
        for target_first in [true, false] {
            let run = |alias: &str| {
                let temp = tempfile::tempdir().unwrap();
                let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
                let alias_path = "  /alias:\n    get:\n      operationId: getAlias\n      \
                    responses:\n        '200':\n          description: ok\n          content:\n            \
                    application/json:\n              schema: { $ref: '#/components/schemas/Alias' }\n";
                let target_path = format!(
                    "  /target:\n    get:\n      operationId: getTarget\n      \
                     responses:\n        '200':\n          description: ok\n          content:\n            \
                     application/json:\n              schema: {{ $ref: '{file_target}' }}\n"
                );
                let paths = if target_first {
                    format!("{target_path}{alias_path}")
                } else {
                    format!("{alias_path}{target_path}")
                };
                std::fs::write(
                    dir.join("openapi.yaml"),
                    format!(
                        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\n\
                         servers: [{{ url: 'https://e.com' }}]\npaths:\n{paths}\
                         components:\n  schemas:\n    Alias: {alias}\n"
                    ),
                )
                .unwrap();
                std::fs::write(dir.join("lib.yaml"), lib).unwrap();
                let out = dir.join("client.rs");
                let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
                let code = std::fs::read_to_string(&out).unwrap_or_default();
                let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
                (generated, checked, code)
            };
            let what = format!(
                "`{sibling}` beside a `$ref` to a relative-file target, target first: {target_first}"
            );
            let (generated, checked, code) =
                run(&format!("{{ $ref: '{file_target}', {sibling} }}"));
            let (bare_generated, _, bare_code) = run(&format!("{{ $ref: '{file_target}' }}"));
            for (entry, report) in [
                ("generate", &generated),
                ("check", &checked),
                ("bare generate", &bare_generated),
            ] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what}: {entry}: {report:#?}"
                );
            }
            assert_eq!(
                types_module(&code),
                types_module(&bare_code),
                "{what} must lower exactly as the bare `$ref` alias does"
            );
            for (entry, report) in [("generate", &generated), ("check", &checked)] {
                assert_eq!(
                    has_code(report, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: {entry}: `W005` must fire exactly when a default is dropped: {report:#?}"
                );
            }
        }
    }
}

/// When a `$ref` is BOTH unresolvable and carries a contradictory sibling, exactly one code must
/// win and it must be `E004`: `ensure_component` returns `None` before the intersection is reached,
/// so the missing component is reported and the sibling never gets a second, confusing diagnostic
/// about a target that does not exist. The two sites are twenty lines apart in the same block, which
/// is why this is pinned rather than assumed.
///
/// The absence of `E013` is only a SYMPTOM of that ordering, and a weak one: letting the miss fall
/// through to a `TypeKind::Any` placeholder and reach the intersection looks identical from
/// outside, because an `Any` sibling identity-intersects and emits nothing. So the ordering itself
/// is measured, with a sibling whose own LOWERING would report — `patternProperties` whose value
/// schemas disagree is `E005`. If the sibling is never lowered, `E005` cannot fire; the control
/// proves it fires the moment the same sibling is lowered against a target that does exist.
#[test]
fn an_unresolvable_ref_reports_only_e004_even_when_its_sibling_contradicts() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Nope', type: integer }
      responses: { '204': { description: ok } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "a missing component must not also be reported as an irreconcilable intersection — \
             there is no target to intersect with: {report:#?}"
        );
    }

    // The mechanism. `SIBLING` is shape-bearing, so it clears `schema_has_shape_constraint` and
    // would be lowered if the `$ref` arm ever got that far.
    const TRACED: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/TARGET'
              type: object
              patternProperties: { '^a': { type: string }, '^b': { type: integer } }
      responses: { '204': { description: ok } }
components:
  schemas:
    Present: { type: object }
"##;
    for report in [
        generate(&TRACED.replace("TARGET", "Nope")),
        check(&TRACED.replace("TARGET", "Nope")),
    ] {
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::PatternPropertiesRejected),
            "the sibling of an unresolvable `$ref` was lowered, so `ensure_component` no longer \
             returns `None` before the intersection is reached and the ordering this fixture \
             names is gone: {report:#?}"
        );
    }
    // The control: the identical sibling against a target that resolves IS lowered, and reports.
    let present = generate(&TRACED.replace("TARGET", "Present"));
    assert!(
        has_code(&present, Code::PatternPropertiesRejected),
        "the marker sibling no longer reports when it is lowered, so its absence above proves \
         nothing: {present:#?}"
    );
}

/// A union sibling gets a say in the union's nullability only where it makes a statement about
/// null, and a sibling that carries no `type` makes none.
///
/// `properties` and `patternProperties` are OBJECT APPLICATORS in 2020-12: they constrain an
/// object and are vacuously satisfied by every non-object, `null` included. They nonetheless lower
/// to a non-nullable `Struct`, so reading `Ty::nullable` off one and letting it decide removed an
/// acceptance the sibling never denied. An independent Draft 2020-12 validator says `null` is valid
/// for all three rows below; they went non-optional, so a `200` of literal `null` that used to
/// decode began failing at runtime with nothing reported at generate time.
///
/// The controls matter as much as the rows: a sibling that DOES carry a `type` is entitled to
/// remove the acceptance, and must keep doing so.
#[test]
fn a_union_sibling_without_a_type_does_not_decide_nullability() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether the response must be optional)
    let cases: &[(&str, &str, bool)] = &[
        // No `type` on the sibling: it says nothing about null, so the union's own acceptance wins.
        (
            "a `properties`-only sibling beside a null member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling beside a null member",
            "patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `properties`-only sibling beside a nullable sole member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: [object, 'null'] }]",
            true,
        ),
        (
            "a `required`-only sibling beside a null member",
            "required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "an `additionalProperties`-only sibling beside a null member",
            "additionalProperties: false\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling carries a `type` that excludes null, so it IS entitled to remove the
        // acceptance — this is the case the round-2 change correctly fixed and must keep fixing.
        (
            "a sibling whose `type` excludes null, beside a null member",
            "type: object\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` array excludes null, beside a null member",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` excludes null, beside a nullable sole member",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        // And a `type` that ADMITS null must not remove it either.
        (
            "a sibling whose `type` array admits null, beside a null member",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, optional) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *optional { "have" } else { "not have" },
            if *optional { "valid" } else { "invalid" }
        );
    }
}

/// The question is "does the SIBLING make a statement about `null`?", and the gate asked the
/// ENCLOSING schema's `type`. The two differ in two ways, and each produces a wrong answer in the
/// opposite direction.
///
/// `lower_union_sibling` deleted the enclosing `type` array whenever it held more than one non-null
/// type, because no single lowered type represents it. That deletion also threw away the array's
/// `"null"`, so the lowered sibling read as null-rejecting even where the array admitted null —
/// **an under-accept**, whose compiled client fails on a
/// spec-legal `null` with `invalid type: null, expected struct …`. In the other direction, a sibling
/// that speaks about null through `enum` or `const` rather than `type` was treated as silent, so the
/// union's acceptance survived a sibling that denied it.
///
/// Both oracles — Python `jsonschema` 4.26 and the Rust crate 0.49.3 — agree on every row.
#[test]
fn the_nullability_gate_asks_the_sibling_not_the_enclosing_schema() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // The sibling carries a WIDE type array that admits null. Lowering deletes the array, so
        // before this the sibling read as null-rejecting and the response went non-optional.
        (
            "a `properties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `required`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The narrow spelling of the first row: one non-null type, so the array survives lowering.
        // It was already right, and the wide spelling must now agree with it.
        (
            "the same sibling under a narrow nullable type array",
            "type: [object, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling speaks about null through `enum`/`const`, not `type`. It denies null, and the
        // gate must let it.
        (
            "an `enum` sibling that excludes null",
            "enum: ['a', 'b']\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a `const` sibling",
            "const: 'a'\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an `allOf` sibling that excludes null",
            "allOf: [{ type: object }]\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        // An `enum` that lists null admits it.
        (
            "an `enum` sibling that includes null",
            "enum: ['a', null]\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *null_satisfies,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *null_satisfies { "have" } else { "not have" },
            if *null_satisfies { "valid" } else { "invalid" }
        );
    }

    // The third symptom of the same root, and the sharpest: a document whose ONLY valid instance is
    // `null` was REJECTED, with a message asserting an empty intersection when the intersection is
    // `{null}`. The one-non-null-type spelling of the same instance set already generated `()`.
    let only_null = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, boolean, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: string }, { type: 'null' }]"
        )
    );
    for (entry, report) in [
        ("generate", generate(&only_null)),
        ("check", check(&only_null)),
    ] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "a schema whose only valid instance is `null` must not be rejected as having no \
             variant by {entry}: {report:#?}"
        );
        assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
    let (_, code) = generate_with_code(&only_null);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        code.contains(&format!("pub type {body} = ();")),
        "the intersection is `{{null}}`, so the exact JSON null type is the answer, not a \
         dropped body and not `Option<()>`: {code}"
    );
}
