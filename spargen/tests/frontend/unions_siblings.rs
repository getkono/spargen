//! A union beside its own sibling keywords: an untyped sibling refines only the branches of its
//! category, a sole member with no typed intersection is rejected under its own cause, `E007`'s
//! explain covers its causes, only a `null` member rescues an empty intersection, a component and
//! an inline schema agree about `null`, and a scoped refiner that excludes every branch leaves the
//! null type.

use super::*;

/// An untyped object or array applicator beside `oneOf`/`anyOf` lowered to `TypeKind::Any`, which
/// intersects as identity, so `required`, `additionalProperties`, `items` and `prefixItems` alone
/// vanished from every branch with no diagnostic (#282). In JSON Schema 2020-12 such a keyword is
/// vacuously satisfied by an instance of another category, so it refines exactly the branches of
/// its own category and leaves the rest as they are; where no branch has its category it reaches
/// nothing the union accepts, which is the contradiction the `$ref` spelling already rejects.
#[test]
fn an_untyped_union_sibling_refines_only_the_branches_of_its_category() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n";
    const AB: &str = "    A: { type: object, properties: { a: { type: string } } }\n    B: { \
                      type: object, properties: { b: { type: string } } }\n";
    // Every variant of `pub enum {name}`, each as the type it wraps.
    let variants_of = |types: &str, name: &str| -> Vec<String> {
        enum_variants(types, name)
            .iter()
            .filter_map(|variant| {
                let (_, inner) = variant.split_once('(')?;
                let inner = inner.trim_end_matches(')');
                let inner = inner
                    .strip_prefix("Box<")
                    .and_then(|boxed| boxed.strip_suffix('>'))
                    .unwrap_or(inner);
                Some(inner.to_owned())
            })
            .collect()
    };
    let variant_types = |types: &str| variants_of(types, "U");
    // The declaration of field `a` inside `pub struct {name}`, or empty where it has none.
    let field_a = |types: &str, name: &str| -> String {
        types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with(&format!("pub struct {name} ")))
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .find(|line| line.starts_with("pub a: "))
            .unwrap_or_default()
            .to_owned()
    };
    let generates = |spec: &str| -> String {
        let (report, code) = generate_with_code(spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{spec}\n{report:#?}");
        for diagnostic in [Code::DeclarationHasNoEffect, Code::AllOfIrreconcilable] {
            assert!(!has_code(&report, diagnostic), "{spec}\n{report:#?}");
        }
        types_module(&code)
    };

    // The issue's reproduction: both branches are objects, so both now require `a`. `A` declares
    // it and keeps its type; `B` does not, so `a` joins it as a required unconstrained field.
    let spec = format!(
        "{HEAD}{AB}    U:\n      oneOf: [{{ $ref: '#/components/schemas/A' }}, {{ $ref: \
         '#/components/schemas/B' }}]\n      required: [a]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        assert!(
            declared_fields(&types, variant).contains(&"a".to_owned()),
            "`{variant}` does not carry `a`:\n{types}"
        );
        let a = field_a(&types, variant);
        assert!(
            !a.is_empty() && !a.contains("Option<"),
            "`required: [a]` beside the union left `a` optional in `{variant}`: {a}\n{types}"
        );
    }

    // A mixed-category union: each refiner reaches its own category's branch and leaves the
    // string branch alone, so no branch is dropped and nothing warns.
    for (keyword, other, sibling, requires_a) in [
        (
            "required",
            "{ type: object, properties: { a: { type: string } } }",
            "required: [a]",
            true,
        ),
        (
            "additionalProperties",
            "{ type: object, properties: { a: { type: string } } }",
            "additionalProperties: false",
            false,
        ),
        (
            "items",
            "{ type: array, items: { type: number } }",
            "items: { type: integer }",
            false,
        ),
        (
            "prefixItems",
            "{ type: array }",
            "prefixItems: [{ type: integer }]",
            false,
        ),
    ] {
        let spec =
            format!("{HEAD}    U:\n      oneOf: [{{ type: string }}, {other}]\n      {sibling}\n");
        let types = generates(&spec);
        let variants = variant_types(&types);
        assert_eq!(
            variants.len(),
            2,
            "`{keyword}` beside a mixed union dropped a branch:\n{types}"
        );
        assert!(
            types.contains(&format!("pub type {} = String;", variants[0])),
            "`{keyword}` beside a mixed union touched the string branch:\n{types}"
        );
        if requires_a {
            let line = field_a(&types, &variants[1]);
            assert!(
                !line.is_empty() && !line.contains("Option<"),
                "`{keyword}` did not reach the object branch: {line}\n{types}"
            );
        }
    }
    // The refiners change the branches they reach: compared with the same union bare.
    for (sibling, branch) in [
        (
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        ("prefixItems: [{ type: integer }]", "{ type: array }"),
    ] {
        let bare = generates(&format!(
            "{HEAD}    U:\n      oneOf: [{{ type: string }}, {branch}]\n"
        ));
        let refined = generates(&format!(
            "{HEAD}    U:\n      oneOf: [{{ type: string }}, {branch}]\n      {sibling}\n"
        ));
        assert_ne!(
            bare, refined,
            "`{sibling}` changed nothing beside the union"
        );
    }

    // Object and array refiners with no `type` beside a union are not a contradiction there: each
    // set refines its own category's branches.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ type: string }}, {{ type: object, properties: {{ a: {{ \
         type: string }} }} }}, {{ type: array, items: {{ type: number }} }}]\n      required: \
         [a]\n      items: {{ type: integer }}\n"
    );
    let types = generates(&spec);
    assert_eq!(variant_types(&types).len(), 3, "{types}");

    // A nested union branch is refined branch by branch too, so its string branch stays.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ oneOf: [{{ type: string }}, {{ type: object, properties: \
         {{ a: {{ type: string }} }} }}] }}, {{ type: integer }}]\n      required: [a]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    let nested = variants_of(&types, &variants[0]);
    assert_eq!(nested.len(), 2, "the nested union lost a branch:\n{types}");
    assert!(
        nested
            .iter()
            .any(|inner| types.contains(&format!("pub type {inner} = String;"))),
        "the nested union lost its string branch:\n{types}"
    );

    // The sole non-null member is refined the same way, and keeps the union's `null`.
    let spec = format!(
        "{HEAD}    U:\n      oneOf: [{{ type: object, properties: {{ a: {{ type: string }} }} }}, \
         {{ type: 'null' }}]\n      required: [a]\n    Holder:\n      type: object\n      \
         required: [u]\n      properties:\n        u: {{ $ref: '#/components/schemas/U' }}\n"
    );
    let types = generates(&spec);
    assert!(types.contains("pub u: Option<U>"), "{types}");
    let a = field_a(&types, "U");
    assert!(
        !a.is_empty() && !a.contains("Option<"),
        "`required: [a]` did not reach the sole member: {a}\n{types}"
    );

    // A branch that states no category takes the one the keywords establish, as an untyped `$ref`
    // target does. This is the shape of GitHub's `secret-scanning-custom-pattern-to-update`:
    // `properties` beside an `anyOf` of `required`-only branches is an object in every branch, so
    // every branch requires `v`. (What each branch's own `required` contributes is decided where
    // the branch is lowered, not here.)
    let spec = format!(
        "{HEAD}    U:\n      required: [v]\n      properties:\n        v: {{ type: integer \
         }}\n        x: {{ type: string }}\n        y: {{ type: string }}\n      anyOf: [{{ \
         required: [x] }}, {{ required: [y] }}]\n"
    );
    let types = generates(&spec);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        let line = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with(&format!("pub struct {variant} ")))
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .find(|line| line.starts_with("pub v: "))
            .unwrap_or_default()
            .to_owned();
        assert!(
            !line.is_empty() && !line.contains("Option<"),
            "branch `{variant}` is not the object that requires `v`: {line}\n{types}"
        );
    }

    // A refiner that reaches no branch of its category constrains nothing the union accepts: the
    // union generates exactly as it is, and the keyword is acknowledged with `W011` rather than
    // dropped in silence, through both entry points.
    for (case, members, sibling) in [
        (
            "no object branch",
            "[{ type: string }, { type: integer }]",
            "required: [a]",
        ),
        (
            "no array branch",
            "[{ type: string }, { type: object }]",
            "items: { type: integer }",
        ),
        (
            "sole member of another category",
            "[{ type: string }, { type: 'null' }]",
            "required: [a]",
        ),
    ] {
        let spec = format!("{HEAD}    U:\n      oneOf: {members}\n      {sibling}\n");
        for report in [check(&spec), generate(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{case}: {report:#?}"
            );
            let messages = messages_for(&report, Code::DeclarationHasNoEffect);
            assert_eq!(messages.len(), 1, "{case}: {report:#?}");
            assert!(
                messages[0].contains("no branch of its union has that category"),
                "{case}: {messages:?}"
            );
        }
        // The union and the types its branches wrap are the bare union's. (The lowered keyword
        // itself is still emitted as an unused type of its own, as every lowered sibling is.)
        let shape = |spec: &str| {
            let types = types_module(&generate_with_code(spec).1);
            let mut shape = enum_variants(&types, "U");
            if shape.is_empty() {
                shape.extend(
                    types
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| line.starts_with("pub type U "))
                        .map(str::to_owned),
                );
            }
            for inner in variants_of(&types, "U") {
                shape.extend(
                    types
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| line.starts_with(&format!("pub type {inner} ")))
                        .map(str::to_owned),
                );
            }
            shape
        };
        let bare = format!("{HEAD}    U:\n      oneOf: {members}\n");
        let refined = shape(&spec);
        assert!(!refined.is_empty(), "{case}: no shape for `U`");
        assert_eq!(
            refined,
            shape(&bare),
            "{case}: a keyword that reaches no branch changed the union"
        );
    }

    // Where the sibling carries both halves and neither reaches a branch of its category, each
    // half is acknowledged with a `W011` of its own: reporting only the object half dropped the
    // array keywords in silence. All three sites that meet a scoped sibling are covered: a
    // multi-variant union, a sole non-null member, and a `$ref` to a union.
    for (case, schema, needle) in [
        (
            "multi-variant union",
            "    U:\n      oneOf: [{ type: string }, { type: integer }]\n      required: [a]\n      \
             items: { type: integer }\n",
            "no branch of its union has that category",
        ),
        (
            "sole non-null member",
            "    U:\n      oneOf: [{ type: string }, { type: 'null' }]\n      required: [a]\n      \
             items: { type: integer }\n",
            "no branch of its union has that category",
        ),
        (
            "`$ref` to a union",
            "    Target: { oneOf: [{ type: string }, { type: integer }] }\n    U:\n      $ref: \
             '#/components/schemas/Target'\n      required: [a]\n      items: { type: integer }\n",
            "no branch of its target union has that category",
        ),
    ] {
        let spec = format!("{HEAD}{schema}");
        for report in [check(&spec), generate(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{case}: {report:#?}"
            );
            let messages = messages_for(&report, Code::DeclarationHasNoEffect);
            assert_eq!(
                messages.len(),
                2,
                "{case}: one W011 per unreached half: {report:#?}"
            );
            assert!(
                messages.iter().all(|message| message.contains(needle)),
                "{case}: {messages:?}"
            );
            for keywords in ["`required`", "`items`"] {
                assert_eq!(
                    messages
                        .iter()
                        .filter(|message| message.contains(keywords))
                        .count(),
                    1,
                    "{case}: exactly one W011 names {keywords}: {messages:?}"
                );
            }
        }
    }

    // A branch that states no category can be given none when object and array keywords come
    // together, or when a multi-type `type` array beside them admits another category too: that is
    // rejected through both entry points rather than generated without them.
    for (case, schema) in [
        (
            "both kinds",
            "      oneOf: [{}, { type: string }]\n      required: [a]\n      items: { type: \
             integer }",
        ),
        (
            "a type array admitting another category",
            "      type: [object, string]\n      oneOf: [{}, { type: string }]\n      required: \
             [a]",
        ),
    ] {
        let spec = format!("{HEAD}    U:\n{schema}\n");
        for report in [check(&spec), generate(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{case}: {report:#?}");
            let messages = messages_for(&report, Code::AllOfIrreconcilable);
            assert_eq!(messages.len(), 1, "{case}: {report:#?}");
            assert!(
                messages[0].contains("union member 0 states no JSON category"),
                "{case}: {messages:?}"
            );
        }
    }

    // A multi-type `type` array deleted for lowering still excludes the branches of the categories
    // it omits, and its object keywords still reach the object branch.
    let spec = format!(
        "{HEAD}    U:\n      type: [object, integer]\n      oneOf: [{{ type: string }}, {{ type: \
         integer }}, {{ type: object, properties: {{ a: {{ type: string }} }} }}]\n      required: \
         [a]\n"
    );
    for report in [check(&spec), generate(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::DeclarationHasNoEffect);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(messages[0].contains("union member 0"), "{messages:?}");
    }
    let types = types_module(&generate_with_code(&spec).1);
    let variants = variant_types(&types);
    assert_eq!(variants.len(), 2, "{types}");
    let a = field_a(&types, &variants[1]);
    assert!(
        !a.is_empty() && !a.contains("Option<"),
        "`required: [a]` beside a type array did not reach the object branch: {a}\n{types}"
    );

    // A typed sibling still speaks for every branch: `type: object` excludes the string one.
    let spec = format!(
        "{HEAD}    U:\n      type: object\n      oneOf: [{{ type: string }}, {{ type: object, \
         properties: {{ a: {{ type: string }} }} }}]\n      required: [a]\n"
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

/// A union whose only non-null member has no typed intersection with the enclosing schema's own
/// sibling constraints has no representable variant left. The multi-variant path already rejects
/// that with `E007` once every variant is excluded, so the one-member collapse reports the same
/// terminal code — otherwise the code would depend on how many variants the author happened to
/// write. It used to be dropped silently.
///
/// Site B widened `E007`'s emission set, so its message has to describe the construct that actually
/// reached it. A right code under a wrong message is still a wrong diagnostic: reusing the
/// multi-variant path's wording verbatim would say "every variant impossible" of a union with one
/// variant, and would inherit the same unsatisfiability claim `intersect_types` cannot support.
/// This pins the sole-member wording, the empty-or-unrepresentable hedge, and that the message does
/// not name some other cause entirely.
#[test]
fn the_single_member_union_rejection_names_its_own_cause() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Collapsed:
      type: integer
      oneOf:
        - { type: string }
        - { type: 'null' }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::NonDisjointUnion);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("sole non-null member"),
        "{:?}",
        messages[0]
    );
    assert!(
        messages[0].contains("empty or unrepresentable"),
        "{:?}",
        messages[0]
    );
    // The causes `E007`'s explain already named must not be borrowed for this one.
    assert!(!messages[0].contains("discriminator"), "{:?}", messages[0]);
    assert!(!messages[0].contains("`anyOf`"), "{:?}", messages[0]);

    // The pointer, which both new `E013` sites pin and this one did not. It is not decoration:
    // `compat::carve_rules` maps it to the smallest omittable construct, so a diagnostic carrying
    // the document root instead of the offending node yields no rule, and a carvable rejection
    // becomes an un-carvable residual that ends the whole run `Rejected`. `carve.rs` proves that
    // consequence for this site end to end.
    let pointers: Vec<&str> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::NonDisjointUnion)
        .map(|d| d.pointer.as_str())
        .collect();
    assert_eq!(
        pointers,
        vec!["/components/schemas/Collapsed"],
        "the sole-member collapse must report at the offending component, not the document root: \
         {report:#?}"
    );

    // And the abstinence Site B spends five lines of comment justifying: the multi-variant path
    // emits a per-variant `W011` before its `E007` and this one deliberately does not, because
    // `W011` means "declared construct has no effect" and describes something dropped from output
    // that still exists — when the SOLE member is excluded no enum is generated at all, so there is
    // no surviving construct for the warning to describe. Filtering to `NonDisjointUnion` above
    // cannot see a second code, so adding exactly that `W011` survived the whole suite. One cause,
    // one diagnostic.
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "the sole-member collapse emitted `W011` beside its `E007`: two diagnostics for one cause, \
         and the `W011` would assert a generated enum that does not exist: {report:#?}"
    );
    assert_eq!(
        report.diagnostics().len(),
        1,
        "the sole-member collapse must report exactly its own cause: {report:#?}"
    );
}

/// `E007`'s published explain is what `spargen explain E007` prints, and Site B added a cause it did
/// not describe. The `oneOf`-plus-`anyOf` and `defaultMapping` causes must survive, and the new one
/// must be there beside them — the same standard this change applied to `E013`'s text.
#[test]
fn the_union_explain_covers_every_cause_that_reports_it() {
    let explain = Code::NonDisjointUnion.explain();
    assert!(explain.contains("`oneOf` and `anyOf`"), "{explain}");
    assert!(explain.contains("defaultMapping"), "{explain}");
    assert!(explain.contains("no branch at all"), "{explain}");
    assert!(explain.contains("single non-null member"), "{explain}");

    // The `W011` clause had no reader while its three neighbours did, and it is the clause Site B's
    // abstinence rests on: a branch the adjacent constraints exclude is dropped with `W011` *while
    // the rest of the enum stands*, which is exactly what does not happen when the excluded branch
    // is the only one. `the_single_member_union_rejection_names_its_own_cause` asserts the
    // behaviour; without this the published reason for it could be deleted silently.
    assert!(
        explain.contains("`W011`"),
        "the explain must say what happens to a branch the adjacent constraints exclude: {explain}"
    );
    assert!(
        explain.contains("while the rest of the enum stands"),
        "the explain must keep the clause that distinguishes an excluded branch from an excluded \
         sole member, which is why one warns and the other does not: {explain}"
    );

    // The same anti-append guard as `E013`'s: presence assertions cannot see a contradiction added
    // after them, and the remedy is the last thing the explain says.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E007`'s explain must end with its remedy: {explain}"
    );
}

/// A union whose own null acceptance came from a stripped `{type: "null"}` member or a `"null"` in
/// the enclosing `type` array is still satisfied by `null`, and must not be rejected as having no
/// variant. `lower_union` folds that acceptance into a local *before* it intersects, so the member
/// it hands `intersect_types` carries `nullable: false` and `type_accepts_null` — which reads
/// `Ty::nullable` — cannot see it. Both collapse paths had the blind spot: the sole-real-member
/// site, and the pre-existing every-variant-excluded site below it.
///
/// The `$ref` spelling of the identical instance set already emits `()`
/// (`the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate` pins it), so two
/// spellings of one schema disagreed. An independent Draft 2020-12 validator says `null` is the one
/// value each of the rescued documents admits, and that each control admits nothing at all.
#[test]
fn a_union_that_null_still_satisfies_is_not_rejected_as_having_no_variant() {
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
        // Sole real member: `null` satisfies the enclosing `type` array AND the null-only member,
        // so `null` is the only value satisfying the whole schema.
        (
            "a sole-member `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        (
            "a sole-member `anyOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                anyOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // Every real variant excluded: the same blind spot on the multi-variant path.
        (
            "an every-variant-excluded `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        // The controls that must keep rejecting: the union admits null but the sibling refuses it,
        // so nothing satisfies both and the rejection is right.
        (
            "a sole-member union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an every-variant-excluded union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            false,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        if *null_satisfies {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` is satisfied by `null`, so it must not be rejected: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::NonDisjointUnion),
                "`{what}` reported E007: {report:#?}"
            );
        } else {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` admits no value at all, so it must still be rejected: {report:#?}"
            );
            assert!(
                has_code(&report, Code::NonDisjointUnion),
                "`{what}` did not report E007: {report:#?}"
            );
        }
        if *null_satisfies {
            // The exact JSON null type, not a silently dropped body and not `Option<()>`: `null` is
            // the only satisfying value, so the response type has exactly one inhabitant. Followed
            // through the operation's own signature rather than by guessing an alias name — the
            // sole-member path lowers its member under the same hint, so the union's own def gets a
            // disambiguating suffix.
            let body = code
                .split("ResponseValue<types::")
                .nth(1)
                .and_then(|rest| rest.split('>').next())
                .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
                .trim()
                .to_owned();
            assert!(
                code.contains(&format!("pub type {body} = ();")),
                "`{what}` must lower its response body to the exact JSON null type, but \
                 `{body}` is not `()`: {code}"
            );
        }
    }

    // The other half of restoring the union's null acceptance ONTO the member: nullability now
    // flows THROUGH the intersection instead of around it, so the sibling gets a say in it. It
    // previously did not, and the union's acceptance was OR-ed back on afterwards regardless —
    // `{type: [string], oneOf: [{type: string}, {type: 'null'}]}` emitted `Option<String>` for a
    // schema the enclosing `type` array forbids `null` in. An independent Draft 2020-12 validator
    // rejects `null` for the first two rows and accepts it for the third.
    //
    // (what it exercises, the schema body, whether the generated response is optional)
    let nullability: &[(&str, &str, bool)] = &[
        (
            "a null member under a sibling `type` array that excludes null",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a nullable sole member under a sibling that excludes null",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        (
            "a null member under a sibling `type` array that admits null",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];
    for (what, body, optional) in nullability {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body: {code}",
            if *optional { "have" } else { "not have" }
        );
    }
}

/// A null-only union MEMBER and a `"null"` in the enclosing `type` array are not the same fact, and
/// only one of them can rescue an empty intersection.
///
/// A member supplies a branch that `null` validates against. A `"null"` in the enclosing `type`
/// array only *permits* null — `oneOf` still demands exactly one matching member and `anyOf` at
/// least one, so with no null member there is nothing for `null` to match and the schema admits
/// nothing at all. Folding both into one flag made those two cases indistinguishable: an
/// independent Draft 2020-12 validator says the first row below is satisfied by `null` and the
/// second by **nothing**, and they produced byte-identical output — the tool could no longer tell
/// "accepts only null" from "accepts nothing", and typed a body `()` that no real payload decodes
/// into, with `check` reporting clean.
#[test]
fn only_a_null_member_can_rescue_an_empty_union_intersection() {
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
        // A null MEMBER: `null` matches it, and the enclosing `type` array permits null, so `null`
        // satisfies the whole schema.
        (
            "a sole-member `oneOf` with a null member beside a nullable type array",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // The SAME enclosing type array with NO null member. Nothing satisfies this: `null` matches
        // no member, and anything matching the `string` member fails the `type` array.
        (
            "a sole-member `oneOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }]",
            false,
        ),
        (
            "an `anyOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                anyOf: [{ type: string }]",
            false,
        ),
        // The same distinction on the every-variant-excluded path.
        (
            "an every-variant-excluded union with a null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        (
            "an every-variant-excluded union whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }]",
            false,
        ),
    ];

    let mut emitted: Vec<(&str, String)> = Vec::new();
    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        if *null_satisfies {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` is satisfied by `null`, so it must not be rejected: {report:#?}"
            );
        } else {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` admits no value at all, so it must be rejected rather than the body \
                 typed `()`: {report:#?}"
            );
            assert!(
                has_code(&report, Code::NonDisjointUnion),
                "`{what}` did not report E007: {report:#?}"
            );
        }
        emitted.push((what, code));
    }

    // What each generating row lowers TO, asserted positively.
    //
    // This replaces a differential that could not fail on the defect it named. It compared row 0
    // (two members, lowers to `()`) against a one-member probe (lowers to `i64`) and required them
    // to differ — but two documents of different member counts and different result types differ in
    // every reachable state, independent of the null fact, so the `assert_ne!` was true whatever
    // the production code did. Measured directly: under the re-merged-flag mutation the fixture
    // fails at the per-case outcome assertion above, and with those assertions neutralised the
    // fixture PASSED. It was held up entirely by its neighbours.
    //
    // Among the rows above, varying only the null fact flips a row between generating and
    // rejecting, so no pair of THEM is a differential. One that generates on both sides does
    // exist, and is asserted after the control below. Here the pin is positive: a row only `null`
    // satisfies must lower to the exact JSON null
    // type — not to `serde_json::Value`, not to `Option<T>` of anything — and that is a statement a
    // mutation can falsify without changing any outcome, which is what the removed assertion could
    // not manage.
    for (what, code) in &emitted {
        let module = types_module(code);
        if module.is_empty() {
            // A rejected row emits nothing; its verdict is asserted per case above.
            continue;
        }
        assert!(
            module.contains("= ();"),
            "`{what}` is satisfied by `null` and nothing else, so it must lower to the exact JSON \
             null type: {module}"
        );
    }
    assert!(
        !types_module(&emitted[0].1).is_empty(),
        "the null-only row generated nothing: {:?}",
        emitted[0].0
    );

    // The control the whole repair must not disturb: a non-empty intersection under the same
    // nullable type array is unaffected either way.
    let control = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }]"
        )
    );
    let (report, code) = generate_with_code(&control);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("= i64;"),
        "the non-empty intersection must still lower to its narrowed type: {code}"
    );

    // The differential that varies ONLY the null fact and generates on both sides: the control,
    // and the control with a `{type: 'null'}` member added. The member is what makes `null` a
    // valid body, so it — and only it — must make the response optional. The null fact lives on
    // the operation signature, not in the types module: `types_module` is nullability-blind here
    // (both documents emit the same `= i64;` alias), which is why this reads the signature.
    let with_null_member = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }, { type: 'null' }]"
        )
    );
    let (partner_report, partner) = generate_with_code(&with_null_member);
    assert_ne!(
        partner_report.outcome(),
        Outcome::Rejected,
        "{partner_report:#?}"
    );
    assert!(
        !code.contains("ResponseValue<Option<"),
        "without a null member `null` matches no branch, so the response is not optional: {code}"
    );
    assert!(partner.contains("= i64;"), "{partner}");
    assert!(
        partner.contains("ResponseValue<Option<"),
        "the null member is a branch `null` satisfies, so the response must be optional: {partner}"
    );
}

/// A schema must lower to the same nullability whether it is written inline or named as a
/// component. `ensure_component` computed `schema_is_nullable(schema)` *before* the body was
/// lowered and wrote it back over whatever the body computed — and `schema_is_nullable` was three
/// disjuncts over `types`, `enum_values` and `const_value` that never looked at `oneOf`, `anyOf`,
/// `$ref` or `allOf`. So **every decision `lower_union` makes about null was discarded at a
/// `components.schemas` boundary**, which is the dominant spelling in real descriptions. The
/// reserve-time guess is now `null_admission`, which reads every composition, and the body's
/// answer is the one kept whatever the guess says; the cases below pin the kept answer.
///
/// The comment there claimed nullability was "a pure function of the component's own schema — the
/// same inputs `lower_schema`/`lower_enum` use". That is true for a plain type/enum/const body and
/// false for every composed one.
///
/// Both oracles agree on each row, and each row is asserted in EVERY spelling, so none can drift
/// from the others again. There are four, because three functions reserve a type before lowering
/// its body and each once wrote the provisional answer back over the body's: `ensure_component`
/// (a root component), `ensure_resolved` (a sub-file component) and `ensure_remote` (a vendored
/// remote schema). A per-site fixture pins one row in one direction; this table pins every row
/// through every site, so reverting any one of the three to the provisional answer fails a row.
/// Each spelling is used by two operations, because the first use returns the freshly lowered `Ty`
/// and the second the cached one, and each path carried the overwrite separately.
#[test]
fn a_component_and_an_inline_schema_agree_about_null() {
    use sha2::{Digest, Sha256};

    const REMOTE_URL: &str = "https://api.example.com/schemas/body.yaml";
    const REMOTE_PATH: &str = "api.example.com/schemas/body.yaml";

    /// The root document: two operations whose `200` bodies are each `schema`, over `components`.
    fn root(schema: &str, components: &str) -> String {
        let schema = schema.replace('\n', "\n                ");
        let operation = |path: &str, id: &str| {
            format!(
                "  {path}:\n    get:\n      operationId: {id}\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                {schema}\n"
            )
        };
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n{}{}components:\n  schemas:\n{components}",
            operation("/u", "fetch"),
            operation("/v", "again"),
        )
    }
    fn component(body: &str) -> String {
        format!("    Body:\n      {}\n", body.replace('\n', "\n      "))
    }
    /// Write `files` (paths relative to the root document's directory) and generate.
    fn run(files: &[(&str, String)]) -> (Report, String) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        let out = dir.join("client.rs");
        let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        (report, code)
    }
    /// `body` generated in each spelling: `(spelling, report, emitted source)`.
    fn every_spelling(body: &str) -> Vec<(&'static str, Report, String)> {
        let ignore = "    Ignore: { type: string }\n";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{REMOTE_URL}\"\nsha256 = \"{:x}\"\npath = \"{REMOTE_PATH}\"\n",
            Sha256::digest(body.as_bytes())
        );
        let vendored = format!(".spargen/vendor/{REMOTE_PATH}");
        let spellings = [
            ("inline", vec![("openapi.yaml", root(body, ignore))]),
            (
                "root component",
                vec![(
                    "openapi.yaml",
                    root("$ref: '#/components/schemas/Body'", &component(body)),
                )],
            ),
            (
                "sub-file component",
                vec![
                    (
                        "openapi.yaml",
                        root("$ref: './lib.yaml#/components/schemas/Body'", ignore),
                    ),
                    (
                        "lib.yaml",
                        format!("components:\n  schemas:\n{}", component(body)),
                    ),
                ],
            ),
            (
                "vendored remote",
                vec![
                    (
                        "openapi.yaml",
                        root(&format!("$ref: '{REMOTE_URL}'"), ignore),
                    ),
                    ("spargen.lock", lock),
                    (&vendored, body.to_owned()),
                ],
            ),
        ];
        spellings
            .into_iter()
            .map(|(spelling, files)| {
                let (report, code) = run(&files);
                (spelling, report, code)
            })
            .collect()
    }

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // `schema_is_nullable` saw the `"null"` in the type array and said nullable. The union says
        // otherwise, and the union is right: with no null MEMBER there is no branch for `null` to
        // match, so `oneOf` fails.
        (
            "a union whose type array admits null but whose members supply no null branch",
            "type: [string, 'null']\noneOf: [{ type: string }]",
            false,
        ),
        // The mirror: the type array is silent about null, the members are not.
        (
            "a union whose null branch comes from a member, under no type array",
            "properties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a union under a wide nullable type array",
            "type: [object, array, 'null']\nproperties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling denies null through `enum`, which `schema_is_nullable` also read — but it
        // read the ENUM, not the intersection, so it must still agree.
        (
            "a union whose `enum` sibling excludes null",
            "enum: ['a', 'b']\noneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a union whose `enum` sibling includes null",
            "enum: ['a', null]\noneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // A plain body, where the reserve-time answer and the body's agree. This is the case the
        // old comment described, and it must not move.
        (
            "a plain nullable type array with no composition",
            "type: [string, 'null']",
            true,
        ),
        (
            "a plain non-nullable type",
            "type: string",
            false,
        ),
    ];

    // Every emitted operation return (both operations, on every client surface emitted), so a wrong
    // answer on either the lowered or the cached path shows.
    let returns = |code: &str| code.matches("support::ResponseValue<").count();
    let optional_returns = |code: &str| {
        code.matches("support::ResponseValue<Option<types::")
            .count()
    };

    for (what, body, null_satisfies) in cases {
        let mut seen = Vec::new();
        for (spelling, report, code) in every_spelling(body) {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` ({spelling}): {report:#?}"
            );
            assert!(
                returns(&code) >= 2,
                "`{what}` ({spelling}): both operations must be emitted: {code}"
            );
            let optional = optional_returns(&code);
            assert_eq!(
                optional,
                if *null_satisfies { returns(&code) } else { 0 },
                "`{what}` as {spelling} must {} an optional response body on both operations — \
                 `null` is {} under this schema: {code}",
                if *null_satisfies { "have" } else { "not have" },
                if *null_satisfies { "valid" } else { "invalid" }
            );
            seen.push((spelling, optional));
        }
        assert_eq!(seen.len(), 4, "`{what}`: every spelling must run: {seen:?}");
        assert!(
            seen.iter().all(|(_, optional)| *optional == seen[0].1),
            "`{what}` lowers to different nullability across spellings: {seen:?}"
        );
    }

    // A component whose union admits ONLY `null` must be the exact null type in every spelling too
    // — the component boundary previously wrapped it back into an `Option`.
    let only_null = "type: [integer, 'null']\noneOf: [{ type: string }, { type: 'null' }]";
    for (spelling, report, code) in every_spelling(only_null) {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{spelling}: {report:#?}"
        );
        assert_eq!(
            optional_returns(&code),
            0,
            "`{spelling}`: the intersection is `{{null}}`, so the type already has exactly one \
             inhabitant and must not be wrapped in `Option`: {code}"
        );
    }
}

/// #450: a nullable union of two or more non-null branches, refined by untyped object keywords
/// that every non-null branch contradicts, still admits `null`, which those keywords are
/// vacuously satisfied by. The scoped meet an untyped `allOf` member beside the union and an
/// untyped `$ref` sibling of the union both take rejected it with `E013`, where the inline sibling
/// spelling of the same keywords types it as the exact JSON null type. Every spelling now agrees.
/// A refiner that itself denies `null` (`type: object`) leaves no value at all, and still rejects.
#[test]
fn a_nullable_union_whose_every_branch_a_scoped_refiner_excludes_is_null() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n    Cat:\n      \
                        type: object\n      required: [kind]\n      properties: { kind: { type: \
                        string } }\n    Dog:\n      type: object\n      required: [kind, bark]\n      \
                        properties: { kind: { type: string }, bark: { type: boolean } }\n";
    const UNION: &str = "[{ $ref: '#/components/schemas/Cat' }, { $ref: \
                         '#/components/schemas/Dog' }, { type: 'null' }]";
    let head = HEAD;
    let holder =
        "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
                  p: { $ref: '#/components/schemas/Pet' }\n";
    let refined = "properties: { kind: { type: integer } }";

    let spellings = [
        (
            "an untyped `allOf` member",
            format!("    Pet:\n      allOf: [{{ {refined} }}]\n      oneOf: {UNION}\n"),
        ),
        (
            "untyped `$ref` siblings",
            format!(
                "    U:\n      oneOf: {UNION}\n    Pet:\n      $ref: '#/components/schemas/U'\n      \
                 {refined}\n"
            ),
        ),
        (
            "inline siblings (the control)",
            format!("    Pet:\n      {refined}\n      oneOf: {UNION}\n"),
        ),
        // The scoped meet also reaches a nullable union nested as a branch of the target union
        // (`meet_refiner`'s union arm). Its every non-null branch is excluded, so it is `null`,
        // and the object branch beside it is excluded too: `null` is the only value left.
        (
            "an untyped `allOf` member beside a nested nullable union",
            format!(
                "    Pet:\n      allOf: [{{ {refined} }}]\n      anyOf:\n        - oneOf: \
                 {UNION}\n        - {{ type: object, required: [kind], properties: {{ kind: {{ \
                 type: string }} }} }}\n"
            ),
        ),
    ];
    for (spelling, pet) in &spellings {
        let spec = format!("{head}{pet}{holder}");
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{spelling}: a nullable union refined to only `null` rejected: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{spelling}: {report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types.contains("pub type Pet = ();"),
            "{spelling}: a nullable union refined to only `null` is not the null type:\n{types}"
        );
        // The use site agrees too: the exact null type is the whole value set, so the holder's
        // field is `Pet` itself, never an `Option` over it.
        assert!(
            types.contains("pub p: Pet,"),
            "{spelling}: the holder's field is not the bare null type:\n{types}"
        );
    }

    // The rescue is `null`'s, so it needs a refiner that admits `null`. A typed `allOf` member
    // denies it, and then no value satisfies the schema.
    let spec = format!(
        "{head}    Pet:\n      allOf: [{{ type: object, {refined} }}]\n      oneOf: {UNION}\n{holder}"
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}
