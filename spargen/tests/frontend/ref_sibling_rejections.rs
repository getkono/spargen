//! The rejections a `$ref` sibling's contradiction draws: a `not` sibling is reported as
//! validation-only, shape siblings that resolve generate, a component `$ref` union member whose
//! siblings contradict its target is `E013`, and every sibling keyword `E013`'s explain names is
//! intersected.

use super::ref_sibling_unions::ref_union_sibling_documents;
use super::*;

/// The control: `not` beside a `$ref` was never silent — it is validation-only and says so with
/// `W001` — and admitting the union keywords to the shape gate must leave it exactly that, in every
/// position, with the position still typed as the target.
#[test]
fn a_ref_with_a_not_sibling_still_reports_it_as_validation_only() {
    let site = "{ $ref: '#/components/schemas/Name', not: { enum: [forbidden] } }";
    for (position, spec, _) in ref_union_sibling_documents(site) {
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{position} via {entry}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::ValidationKeywordIgnored),
                "{position} via {entry}: `not` must still be reported: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{position} via {entry}: {report:#?}"
            );
        }
    }
}

/// The negative control for the rejection above: this change turns a previously-silent success
/// into a rejection, so what it must NOT do is reject a `$ref` that resolves. A `$ref` carrying
/// shape siblings is the narrow case — it is the one construct that reaches `ensure_component`
/// through `lower_schema_inner`, and it is an intersection, so its target contributes fields rather
/// than replacing it. Both sides must survive into the generated type.
#[test]
fn a_ref_with_shape_siblings_that_resolves_is_not_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Ext' } }
components:
  schemas:
    Ext:
      $ref: '#/components/schemas/Base'
      type: object
      properties: { extra: { type: string } }
    Base:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    // The reference was genuinely followed, not merely tolerated: the sibling's own property and
    // the referenced component's property are both present.
    assert!(code.contains("pub extra"), "{code}");
    assert!(code.contains("pub id"), "{code}");

    // check/generate parity on the clean path too.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");
}

/// A `oneOf`/`anyOf` member written `{$ref: '#/components/schemas/Name', type: integer}` is the
/// conjunction `Name ∩ integer` — in 2020-12 `$ref` is an applicator — exactly as the same schema is
/// at a body or property position. With `Name` a string, nothing satisfies it.
///
/// The multi-member union used to lower a root-component member through its `$ref` string alone,
/// so the member's own `type` was discarded and the run came back clean with the branch typed as
/// the plain target (#279). The sole-member collapse and every other `$ref` spelling already
/// intersected it; the verdict is now the one they give, `E013` at the member.
#[test]
fn a_component_ref_union_member_whose_siblings_contradict_its_target_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                oneOf:
                  - { $ref: '#/components/schemas/Name', type: integer }
                  - { type: boolean }
components:
  schemas:
    Name: { type: string }
"##;
    let pointer = "/paths/~1p/get/responses/200/content/application~1json/schema/oneOf/0";
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: the member's `type: integer` must not be dropped in silence: {report:#?}"
        );
        let e013: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AllOfIrreconcilable)
            .collect();
        assert!(
            e013.iter().any(|d| d.pointer.as_str() == pointer),
            "{entry}: E013 must point at the member `{pointer}`, not at {:?}\n{report:#?}",
            e013.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
        );
    }
}

/// The satisfiable counterpart: `{$ref: Pet, properties: {owner: …}, required: [owner]}` narrows
/// `Pet` to the pets that carry an `owner`. The branch must be that narrowed type — `Pet`'s fields
/// plus a required `owner` — while still taking its variant name and its implicit discriminator tag
/// from the component it references, since the member is still written as that component.
#[test]
fn a_component_ref_union_member_with_siblings_is_narrowed_and_keeps_its_component_name() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      type: object
      properties:
        kind: { type: string }
        name: { type: string }
    Dog:
      type: object
      required: [kind, bark]
      properties:
        kind: { type: string }
        bark: { type: boolean }
    Animal:
      oneOf:
        - $ref: '#/components/schemas/Pet'
          properties: { owner: { type: string } }
          required: [owner]
        - $ref: '#/components/schemas/Dog'
      discriminator:
        propertyName: kind
"##;
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "check: {checked:#?}");
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let variants = enum_variants(&types, "Animal");
    let pet = variants
        .iter()
        .find_map(|variant| variant.strip_prefix("Pet(")?.strip_suffix(')'))
        .map(|payload| {
            payload
                .strip_prefix("Box<")
                .and_then(|inner| inner.strip_suffix('>'))
                .unwrap_or(payload)
        })
        .unwrap_or_else(|| panic!("the member lost its component name, got {variants:?}: {types}"));
    assert_ne!(
        pet, "Pet",
        "the branch is the plain target, so its siblings were discarded: {types}"
    );
    let mut fields = declared_fields(&types, pet);
    fields.sort();
    assert_eq!(
        fields,
        ["kind", "name", "owner"],
        "the narrowed branch must carry Pet's properties and the member's own: {types}"
    );
    let owner = types
        .split(&format!("pub struct {pet} "))
        .nth(1)
        .and_then(|body| field_type(body, "pub owner"));
    assert!(
        owner
            .as_deref()
            .is_some_and(|ty| !ty.starts_with("Option<")),
        "the member's `required: [owner]` must make `owner` required, got {owner:?}: {types}"
    );
    assert!(
        types.contains("\"Pet\""),
        "the implicit discriminator tag must stay the component name: {types}"
    );
}

/// In JSON Schema 2020-12 `$ref` is an applicator, so a `$ref`'s shape-bearing siblings are
/// intersected with the referenced schema rather than discarded. When that intersection is empty no
/// value can satisfy the schema, which is a document error the author must hear about: before this
/// was pinned, `spargen check` reported `clean` and the construct simply vanished — a request body
/// whose method then took no body argument at all. Every construct that reaches the `$ref` arm of
/// `LowerCtx::lower_schema_inner` must report `E013`, through both `generate` and `check`.
#[test]
fn e013_fires_when_a_ref_sibling_contradicts_its_target() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    // `Name` is a string; every site below intersects it with `type: integer`, which is empty.
    const TAIL: &str = "components:\n  schemas:\n    Name: { type: string }\n";

    // The issue's exact reproduction: the body vanished and `upload` lost its body argument.
    let request_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    let response_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', type: integer }
"##
    );
    let parameter = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      parameters:
        - name: filter
          in: query
          schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    // A component property, which reaches the same arm through `object_body`/`ensure_component`.
    let component_property = format!(
        "{HEAD}paths: {{}}\n{}",
        r##"components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##
    );

    // The pointer is not decoration: `compat::carve_rules` maps it to the smallest omittable
    // construct, so a diagnostic carrying the document root instead of the offending node yields no
    // carve rule and turns a carvable rejection into an un-carvable residual. Each site therefore
    // pins the exact pointer it must produce, and `carve.rs` proves the consequence end to end.
    for (site, spec, pointer) in [
        (
            "request body",
            &request_body,
            "/paths/~1u/post/requestBody/content/application~1json/schema",
        ),
        (
            "response body",
            &response_body,
            "/paths/~1u/get/responses/200/content/application~1json/schema",
        ),
        (
            "parameter",
            &parameter,
            "/paths/~1u/get/parameters/0/schema",
        ),
        (
            "component property",
            &component_property,
            "/components/schemas/Holder/properties/field",
        ),
    ] {
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{site}` was not rejected by {entry}: {report:#?}"
            );
            let pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::AllOfIrreconcilable)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                pointers,
                vec![pointer],
                "`{site}` through {entry} must report E013 once, at the offending schema: \
                 {report:#?}"
            );
        }
    }
}

/// `intersect_types` fails for TWO conditions: the intersection is empty (`NoMeet::Empty`), so no
/// value satisfies both sides, and the intersection is inhabited but has no single Rust type
/// (`NoMeet::Unrepresentable` — `Bytes` against a `uuid` or date string, say). The emitted message must
/// not claim the first when it may be the second: `{$ref: Id, contentEncoding: base64}` over a
/// `uuid` string `Id` is satisfied by a base64 UUID, and the `E013` explain and the `allOf` scalar
/// site both already say "empty or unrepresentable". This pins the site to the same honest wording. It asserts
/// what the message may NOT say as well as what it must, because the defect this replaced was a
/// message that named the wrong one of the two.
#[test]
fn the_ref_sibling_rejection_does_not_claim_more_than_it_knows() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("empty or unrepresentable intersection"),
        "the site must use the same wording as the `allOf` scalar site and the E013 explain: {:?}",
        messages[0]
    );
    // `None` is not proof of emptiness, so the message may not assert unsatisfiability.
    assert!(
        !messages[0].contains("no value can satisfy"),
        "the message asserts unsatisfiability, which `intersect_types` returning `None` does not \
         establish: {:?}",
        messages[0]
    );
    // The remedy is the author's only route out of a rejection, so it is pinned too: it must name
    // the construct and offer the omit escape the taxonomy promises.
    let remedy = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::AllOfIrreconcilable)
        .and_then(|d| d.remedy.clone())
        .unwrap_or_default();
    assert!(remedy.contains("`$ref` target"), "{remedy:?}");
    assert!(remedy.contains("spargen::omit!"), "{remedy:?}");
}

/// The shape-bearing sibling keywords `E013`'s explain names, READ OUT of the published text
/// rather than copied into the fixture beside it. Two independent lists cannot pin each other: a
/// keyword deleted from the prose simply disappears, and a hard-coded copy goes on passing.
fn shape_bearing_keywords_the_explain_names() -> Vec<String> {
    const LEAD: &str = "A sibling bears a shape of its own through ";
    let explain = Code::AllOfIrreconcilable.explain();
    let start = explain.find(LEAD).unwrap_or_else(|| {
        panic!("E013's explain no longer enumerates its shape-bearing sibling keywords: {explain}")
    }) + LEAD.len();
    let sentence = &explain[start..];
    let sentence = &sentence[..sentence
        .find(". ")
        .unwrap_or_else(|| panic!("E013's shape-bearing sentence never ends: {sentence}"))];
    // The keywords are the backticked spans of that one sentence.
    sentence
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// `E013`'s explain names the sibling keywords intersected with a `$ref`'s target rather than
/// discarded. That is a published promise, held to the code at two levels. Which keywords the
/// private gate `schema_has_shape_constraint` reads is pinned in-crate: its `SHAPE_KEYWORDS` table
/// and this explain name one set, in both directions (`oas31::lower`'s tests). What this fixture
/// adds is behaviour, which the table cannot show: a sibling that clears the gate but lowers to
/// `TypeKind::Any` intersects as identity and is discarded anyway.
///
/// The list under test is DERIVED from `explain()`, not repeated here, so deleting a keyword from
/// the prose fails this fixture rather than quietly shrinking what it checks. And each row is a
/// DIFFERENTIAL — the same document with and without the keyword — so the keyword is what flips the
/// outcome. An earlier revision paired several keywords with a `type` against a target of another
/// category, where the `type` alone already rejected and the keyword beside it was never
/// load-bearing; three rows proved nothing at all.
///
/// The `$ref` sits at a response body rather than a component root, so each row exercises the
/// sibling gate alone. A component root whose value is a `$ref` with only non-shape-bearing
/// siblings once tripped a release-level `assert_eq!` inside `ensure_component` (#148), which
/// turned every "without" control into an opaque panic instead of this fixture's own message.
///
/// Each target is chosen so the intersection is *genuinely empty*, never merely unrepresentable:
/// the `contentEncoding`/`format: binary` rows sit against an integer, not a string, because a
/// plain string target intersects to `bytes::Bytes` and generates (see
/// `a_binary_string_lowers_to_bytes_however_the_binary_constraint_is_spelled`).
#[test]
fn every_sibling_keyword_the_explain_names_is_actually_intersected() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const BODY: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Target'
SIBLING
"##;

    // (keyword, the target it must contradict, the sibling spelling of that keyword)
    let cases: &[(&str, &str, &str)] = &[
        ("type", "{ type: string }", "type: integer"),
        // `properties` alone, with no `type` beside it to account for the rejection. The target
        // REQUIRES `a`, so the conflicting property genuinely empties the composition — without
        // that, an optional conflicting property is representable and correctly generates.
        (
            "properties",
            "{ type: object, required: [a], properties: { a: { type: string } } }",
            "properties: { a: { type: integer } }",
        ),
        (
            "patternProperties",
            "{ type: string }",
            "patternProperties: { '^a': { type: string } }",
        ),
        // The four refining keywords, each ALONE: no `type`, no `properties`. They used to clear
        // `schema_has_shape_constraint` and then lower to `TypeKind::Any`, which intersects as
        // identity, so each was discarded with no diagnostic (#140). Each now establishes the
        // category it applies to, so against a string it empties the intersection.
        ("required", "{ type: string }", "required: [a]"),
        (
            "additionalProperties",
            "{ type: string }",
            "additionalProperties: false",
        ),
        ("items", "{ type: string }", "items: { type: integer }"),
        (
            "prefixItems",
            "{ type: string }",
            "prefixItems: [{ type: integer }]",
        ),
        ("enum", "{ type: integer }", "enum: ['a']"),
        ("const", "{ type: integer }", "const: 'a'"),
        (
            "contentEncoding",
            "{ type: integer }",
            "contentEncoding: base64",
        ),
        ("format: binary", "{ type: integer }", "format: binary"),
        ("allOf", "{ type: string }", "allOf: [{ type: integer }]"),
        ("oneOf", "{ type: string }", "oneOf: [{ type: integer }]"),
        ("anyOf", "{ type: string }", "anyOf: [{ type: integer }]"),
    ];

    // The fixture's table and the published text must name the same keywords, in the same order.
    let named = shape_bearing_keywords_the_explain_names();
    let covered: Vec<String> = cases
        .iter()
        .map(|(keyword, ..)| (*keyword).to_owned())
        .collect();
    assert_eq!(
        named, covered,
        "`E013`'s explain and this fixture disagree about which sibling keywords bear a shape; \
         whichever moved, the other must move with it"
    );

    let mut unconstrained = Vec::new();
    let mut spurious = Vec::new();
    for (keyword, target, sibling) in cases {
        let spec = |sibling: &str| {
            format!(
                "{HEAD}{}components:\n  schemas:\n    Target: {target}\n",
                BODY.replace("SIBLING", sibling)
            )
        };
        let with = generate(&spec(&format!("                {sibling}")));
        if with.outcome() != Outcome::Rejected || !has_code(&with, Code::AllOfIrreconcilable) {
            unconstrained.push(*keyword);
        }
        // The control: the identical document without the keyword must generate, so the rejection
        // above is attributable to the keyword and to nothing else in the row.
        let without = generate(&spec(""));
        if without.outcome() == Outcome::Rejected {
            spurious.push(*keyword);
        }
    }
    assert!(
        unconstrained.is_empty(),
        "the explain names these sibling keywords as intersected, but a `$ref` carrying one against \
         an irreconcilable target still generates: {unconstrained:?}"
    );
    assert!(
        spurious.is_empty(),
        "these rows reject even without their keyword, so they pin the rest of the row rather than \
         the keyword the explain names: {spurious:?}"
    );

    // Tier one proves each refiner constrains against a target of ANOTHER category, where an empty
    // intersection is the only possible verdict. It cannot see a refiner that establishes its
    // category and then refines nothing — a `required` that became an empty open struct would
    // still reject against a string. This tier is a differential instead — the same document with
    // and without the refining keyword, against a target of the category it applies to, so the
    // only thing that can move the lowering is the keyword itself. The rows with nothing beside the
    // refiner are #140's leak; the rows with an establishing keyword pin that the category a
    // `type`/`properties` gives it is the same one it now implies alone.
    //
    // (keyword, the establishing keywords beside it, the refiner, the target it agrees with)
    let refiners: &[(&str, &str, &str, &str)] = &[
        (
            "required",
            "",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        // A name the target does not declare either: the requirement is the key's presence, and
        // it must survive as a required field rather than vanish for want of a property to mark.
        ("required", "", "required: [a]", "{ type: object }"),
        (
            "additionalProperties",
            "",
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items",
            "",
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        (
            "prefixItems",
            "",
            "prefixItems: [{ type: integer }]",
            "{ type: array, items: { type: number } }",
        ),
        (
            "additionalProperties",
            "type: object",
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items",
            "type: array",
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        (
            "prefixItems",
            "type: array",
            "prefixItems: [{ type: integer }]",
            "{ type: array, items: { type: number } }",
        ),
        // `required` beside `properties` participates: `object_body` consumes it per declared
        // property, and the property is declared.
        (
            "required",
            "properties: { a: { type: string } }",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        // And beside a bare `type: object`, which declares no property for it to mark. It used to
        // be dropped there — `object_body` consumed `required` only as a per-property flag — so
        // the generated type accepted and could emit `{}`, which the description forbids.
        (
            "required",
            "type: object",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
    ];
    // What is compared is `Sibling`'s OWN emitted definition, not the whole `types` module.
    //
    // The module also holds the intermediate aliases — `SiblingConstraint`,
    // `SiblingReferenceIntersection` — which are emitted whenever the sibling clears
    // `schema_has_shape_constraint`, whether the refining keyword participates in the intersection
    // or not. So a whole-module comparison answers "did adding this keyword change ANY emitted
    // byte", which is liveness; it does not answer "did it change the type", which is the claim in
    // the failure message. Measured under the sibling-discarded mutation: `Sibling` itself is
    // byte-identical with and without the refiner, and the whole-module comparison passed anyway.
    let lowering = |establishing: &str, refiner: &str, target: &str| {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Target: {target}\n    Sibling:\n      \
             $ref: '#/components/schemas/Target'\n      {establishing}\n{refiner}"
        );
        let (report, code) = generate_with_code(&spec);
        // `Sibling`'s own item, ATTRIBUTES INCLUDED — `#[serde(deny_unknown_fields)]` sits above
        // the declaration and is exactly what `additionalProperties` contributes. A rejection
        // yields no item at all, which is itself a difference worth seeing.
        let types = types_module(&code);
        let lines: Vec<&str> = types.lines().collect();
        let declares = |line: &str| {
            let t = line.trim_start();
            ["pub struct ", "pub type ", "pub enum "]
                .iter()
                .filter_map(|decl| t.strip_prefix(decl))
                .any(|rest| {
                    rest.split([' ', '<', '{', '(', ';', '='])
                        .next()
                        .is_some_and(|name| name == "Sibling")
                })
        };
        let definition = lines
            .iter()
            .position(|line| declares(line))
            .map(|decl| {
                // Walk back over the item's attributes and rustdoc.
                let mut start = decl;
                while start > 0 {
                    let prev = lines[start - 1].trim_start();
                    if prev.starts_with("#[") || prev.starts_with("///") {
                        start -= 1;
                    } else {
                        break;
                    }
                }
                // Forward to the end of the item.
                let mut end = decl;
                if lines[decl].trim_end().ends_with(';') {
                    end = decl + 1;
                } else {
                    while end < lines.len() {
                        end += 1;
                        if lines[end - 1].trim_end() == "}" {
                            break;
                        }
                    }
                }
                lines[start..end].join("\n")
            })
            .unwrap_or_default();
        (report.outcome(), definition)
    };
    for (keyword, establishing, refiner, target) in refiners {
        let with = lowering(establishing, &format!("      {refiner}\n"), target);
        let without = lowering(establishing, "", target);
        // Every target here AGREES with the refiner's category, so a rejection is not the
        // refiner taking part — it is the refiner being mistaken for a contradiction. Without
        // this, a rejection would satisfy the differential below by yielding no item at all.
        assert_ne!(
            with.0,
            Outcome::Rejected,
            "`{keyword}` beside `{establishing}` rejected against `{target}`, a target of the \
             category it applies to"
        );
        assert_ne!(
            with, without,
            "`{keyword}` beside `{establishing}` changed nothing about the lowering, so the \
             explain's claim that it takes part is not true"
        );
    }
}
