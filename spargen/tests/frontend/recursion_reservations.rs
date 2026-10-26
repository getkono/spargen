//! Recursion, third part: the reservations a component holds while it is lowered — self-union
//! reservations, siblings over an open reservation, back-edge ranking, basic recursive generation,
//! and cycle-closing references, members and meets.

use super::*;

/// The one shape in this family that must **not** generate: a union that is a component's whole
/// body and whose only non-null member is a `$ref` back to that same component.
///
/// `Selfy = Selfy | null` describes no instance a decoder can ever terminate on — the generated
/// `Deserialize` opens by re-entering itself on the same value with no base case. The multi-member
/// path already refuses exactly this with `E007`; the single-member collapse jumped over that guard
/// and produced `E011` instead, so one shape drew two different codes according to how many members
/// were written beside it.
///
/// Both spellings are driven: the bare union, and the union carrying its own sibling keywords.
#[test]
fn a_union_whose_sole_member_is_its_own_reservation_is_rejected() {
    let bare = r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Selfy' } }
components:
  schemas:
    Selfy:
      oneOf:
        - { $ref: '#/components/schemas/Selfy' }
        - { type: "null" }
"##;
    let with_siblings = r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Selfy' } }
components:
  schemas:
    Selfy:
      type: object
      properties:
        x: { type: string }
      oneOf:
        - { $ref: '#/components/schemas/Selfy' }
        - { type: "null" }
"##;

    for (label, spec) in [("bare", bare), ("with siblings", with_siblings)] {
        let generated = generate(spec);
        let checked = check(spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}/{entry}: {report:#?}"
            );
            // A spec-facing code with a matrix cell, an `errors.md` row and this fixture — never
            // the internal invariant's `E011`, whose explain text claims malformed input.
            assert!(
                !has_code(report, Code::InvalidInput),
                "{label}/{entry}: {report:#?}"
            );
            // `E007`, and **only** `E007` — not a disjunction with `E013`. The two guards this
            // shape passes through are ordered deliberately, the cycle question before the
            // sibling-intersection question, and `oas31/lower/union.rs` records at that site that
            // a reordering turns this fixture red. A disjunction would not: it is satisfied by either verdict,
            // so the ordering it claims to protect would be free to flip in silence. The `with
            // siblings` spelling is the one that carries the difference, because it is the only one
            // the sibling guard can answer at all.
            let codes: Vec<Code> = report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .map(|diagnostic| diagnostic.code)
                .collect();
            assert_eq!(
                codes,
                vec![Code::NonDisjointUnion],
                "{label}/{entry}: {report:#?}"
            );
            // The remedy is the sentence the author acts on, and the one this rejecter carries has
            // to serve its cycle situations as well as its overlap ones — neither of which a
            // discriminator answers. Nothing else in the suite reads a remedy on this path, so
            // without this the whole text was free to be replaced by advice that contradicts the
            // fix.
            let remedy = report
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.code == Code::NonDisjointUnion)
                .and_then(|diagnostic| diagnostic.remedy.as_deref())
                .unwrap_or_default();
            assert!(
                remedy.contains("break the reference cycle"),
                "{label}/{entry}: the remedy must name what an author with a self-referential \
                 union actually has to change: {remedy:?}"
            );
        }
    }
}

/// The sub-file spellings of the sibling-bearing self-union above. The root spelling is answered by
/// the reservation arm of `member_is_this_union`; a sub-file member — bare `#/components/schemas/…`
/// resolved against `lib.yaml`, or the explicit `./lib.yaml#/…` — reaches no root reservation by
/// name, and only the resolved-identity arm keeps the document-half cycle guard from answering
/// `E013` for it. Both spellings must draw exactly `[E007]`, as the root spelling does.
#[test]
fn a_sibling_bearing_self_union_in_a_sub_file_is_rejected_as_e007_on_both_spellings() {
    const SELFY: &str = r##"
components:
  schemas:
    Selfy:
      type: object
      properties:
        x: { type: string }
      oneOf:
        - { $ref: 'PREFIX#/components/schemas/Selfy' }
        - { type: "null" }
"##;
    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let lib = SELFY.replace("PREFIX", prefix);
        let (generated, checked, _) = split("./lib.yaml#/components/schemas/Selfy", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: {report:#?}"
            );
            let codes: Vec<Code> = report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .map(|diagnostic| diagnostic.code)
                .collect();
            assert_eq!(
                codes,
                vec![Code::NonDisjointUnion],
                "{spelling}/{entry}: {report:#?}"
            );
        }
    }
}

/// A union whose sibling keywords would have to be intersected against a target that is still being
/// lowered. Nothing true can be said about that intersection, so it must be refused — not guessed
/// at, and not quietly dropped.
///
/// Three positions, all of which `2aa5ada` accepted by guessing: the sole member of a nested
/// property's union, the sole member of an array `items` union, and one member of a multi-member
/// union. At `2aa5ada` `intersect_non_null`'s `TypeKind::Any` arm absorbed the placeholder and
/// returned the *sibling*, silently retyping the recursive branch to the inline object beside it.
/// Without the refusal, the multi-member case drops the variant with a `W011` whose message —
/// "cannot satisfy the enclosing schema's own constraints" — is false about the document: the
/// member can satisfy them perfectly well, it simply has not been lowered yet.
#[test]
fn a_union_sibling_over_an_open_reservation_is_rejected() {
    let sole_member_in_a_property = r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Node' } }
components:
  schemas:
    Node:
      type: object
      required: [name]
      properties:
        name: { type: string }
        next:
          type: object
          properties:
            inner: { type: string }
          anyOf:
            - { $ref: '#/components/schemas/Node' }
            - { type: "null" }
"##;
    let sole_member_in_array_items = r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Tree' } }
components:
  schemas:
    Tree:
      type: object
      required: [kids]
      properties:
        kids:
          type: array
          items:
            type: object
            properties:
              tag: { type: string }
            oneOf:
              - { $ref: '#/components/schemas/Tree' }
              - { type: "null" }
"##;
    let one_of_several_members = r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Node' } }
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: { type: string }
        child:
          type: object
          properties:
            tag: { type: string }
          oneOf:
            - { $ref: '#/components/schemas/Node' }
            - type: object
              properties:
                other: { type: string }
"##;

    for (label, spec) in [
        ("sole member in a property", sole_member_in_a_property),
        ("sole member in array items", sole_member_in_array_items),
        ("one of several members", one_of_several_members),
    ] {
        let (report, code) = generate_with_code(spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
        assert!(
            !has_code(&report, Code::InvalidInput),
            "{label}: {report:#?}"
        );
        assert!(
            has_code(&report, Code::AllOfIrreconcilable)
                || has_code(&report, Code::NonDisjointUnion),
            "{label}: {report:#?}"
        );
        // The false acknowledgement must be gone: the member is not a branch the sibling made
        // impossible, it is one nothing could yet be said about.
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{label}: {report:#?}"
        );
        assert!(
            code.is_empty(),
            "{label}: rejected runs emit nothing: {code}"
        );
    }
}

/// The narrowness of the nullable-alias rule, stated as a fixture rather than as a comment.
///
/// `B: {oneOf: [{$ref: C}, {type: "null"}]}` where `C` is an ordinary, fully lowered component is
/// the *same spelling* as the mutual-recursion case above, and it already generates: `B` is
/// re-emitted as a type of its own carrying `C`'s shape. Recognising every nullable alias — rather
/// than only one whose target is still being lowered — would delete `B` from the generated API,
/// which is a breaking change to output that has nothing wrong with it.
#[test]
fn a_nullable_ref_alias_to_a_finished_component_keeps_its_own_type() {
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
            application/json: { schema: { $ref: '#/components/schemas/A' } }
components:
  schemas:
    A:
      type: object
      required: [b]
      properties:
        b: { $ref: '#/components/schemas/B' }
    B:
      oneOf:
        - { $ref: '#/components/schemas/C' }
        - { type: "null" }
    C:
      type: object
      required: [v]
      properties:
        v: { type: string }
"##;
    let (generated, code) = generate_with_code(spec);
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    // `B` is a public type of the generated API and must stay one.
    assert!(code.contains("pub struct B "), "{code}");
    assert!(code.contains("pub struct C "), "{code}");
}

/// An `allOf` whose members disagree about `additionalProperties`, where one of the two value
/// schemas is a `$ref` back to the type being lowered.
///
/// `merge_additional` intersects the two value types and its caller reports the failure as
/// "`allOf` members declare conflicting `additionalProperties`". That is a true sentence about a
/// genuine conflict and a false one here: nothing conflicts, the target's body simply has not been
/// computed yet. The code is right — the same `E013` either way — and the message is what has to
/// tell the two apart, because "conflicting" sends the author looking for a disagreement that is
/// not in the document.
#[test]
fn an_all_of_additional_properties_back_edge_says_why_it_cannot_merge() {
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
            application/json: { schema: { $ref: '#/components/schemas/A' } }
components:
  schemas:
    A:
      allOf:
        - type: object
          additionalProperties: { $ref: '#/components/schemas/A' }
        - type: object
          additionalProperties: { type: string }
"##;
    for (entry, report) in [("generate", &generate(spec)), ("check", &check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let message = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::AllOfIrreconcilable)
            .map(|d| d.message.clone())
            .unwrap_or_default();
        assert!(
            !message.contains("conflicting"),
            "{entry}: the members do not conflict; the target is unlowered: {message}"
        );
        assert!(
            message.contains("additionalProperties"),
            "{entry}: {message}"
        );
    }
}

/// `type_specificity`'s reservation arm is **live**, and its value is emitted into the client.
///
/// The arm carried a comment justifying itself by saying a union holding a reservation is rejected
/// before ranking is reached, naming a function `reject_union_back_edge` that **has never existed
/// anywhere in the repository**. Neither half was true. `lower_union`'s guard tests the *direct*
/// member's id, while `type_specificity` recurses through `Array` and `Union` — so an array-wrapped
/// back edge walks straight past it on a document that generates **Clean, zero diagnostics**.
///
/// What the arm returns is not inert: it becomes the trial-match priority of an `anyOf` branch in
/// the generated `Deserialize`, which is a runtime dispatch decision in shipped code. Ranking a
/// reservation least specific keeps the concrete branch ahead of it; mutating the arm to `4_000`
/// raises the back-edge branch from 800 to 1200, past the string branch's 850, and **inverts which
/// variant wins** — a change that survived the entire workspace suite when nothing pinned it.
#[test]
fn an_array_wrapped_union_back_edge_ranks_least_specific() {
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
            application/json: { schema: { $ref: '#/components/schemas/Wrap' } }
components:
  schemas:
    Wrap:
      anyOf:
        - type: array
          items: { $ref: '#/components/schemas/Wrap' }
        - type: array
          items: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    // The document is accepted, which is what makes this arm reachable rather than defensive.
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");

    // Pull each variant's emitted trial priority out of the generated `Deserialize`.
    let priority = |variant: &str| -> u32 {
        let needle = format!("Wrap::{variant}(inner)");
        code.lines()
            .find(|line| line.contains(&needle) && line.contains("u32,"))
            .and_then(|line| {
                let start = line.find("Some((")? + "Some((".len();
                let end = line[start..].find("u32")? + start;
                line[start..end].parse().ok()
            })
            .unwrap_or_else(|| panic!("no emitted priority for {variant}: {code}"))
    };
    let back_edge = priority("WrapVariant0");
    let concrete = priority("WrapVariant1");

    // The reservation contributes nothing to its array's specificity, so the branch whose items are
    // a known type must outrank the branch whose items are not yet lowered. Raising the arm to
    // `4_000` makes `back_edge` 1200 against `concrete` 850 and reverses this.
    assert!(
        back_edge < concrete,
        "an array of a not-yet-lowered type must not outrank an array of a known one: \
         back-edge {back_edge}, concrete {concrete}"
    );
}

/// `json_category`'s reservation arm is **live** on a document that generates cleanly, and its
/// answer picks the emitted `Deserialize` strategy.
///
/// `lower_union` refuses a member that is the union's *own* reservation, but a member that is some
/// other open component's reservation is the ordinary recursive back edge and is kept. Here the
/// items union is lowered while `Tree` is still reserved, so the dispatch-strategy search asks
/// what JSON category `Tree` serialises as, and nothing is known yet. Answering `Object` (the
/// guess that is right for most recursive schemas) made the union provably disjoint, and the
/// emitted decoder routed the back edge on `value.is_object()`: a nested `Tree`, which is an
/// array, would then match no variant at runtime. That mutation survived every suite before this
/// fixture. Uncategorisable sends the union to trial matching, which decodes both branches.
#[test]
fn a_union_back_edge_to_an_open_component_is_not_categorised() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /t:
    get:
      operationId: getT
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Tree' } }
components:
  schemas:
    Tree:
      type: array
      items:
        oneOf:
          - $ref: '#/components/schemas/Tree'
          - type: string
"##;
    let (report, code) = generate_with_code(spec);
    // Accepted with no diagnostics at all, which is what makes the arm reachable, not defensive.
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(codes(&report), Vec::<&str>::new(), "{report:#?}");

    let types = types_module(&code);
    assert!(
        types.contains("pub enum TreeItem"),
        "the items union must be emitted as an enum: {types}"
    );
    // No category is claimed for the back edge: the spec has no object anywhere, so any
    // `is_object()` predicate in the output is a guessed category for the reservation.
    assert!(
        !types.contains("is_object()"),
        "a not-yet-lowered back edge must not be routed as an object: {types}"
    );
    // Trial matching emits a ranked candidate per variant; a disjoint decoder emits none.
    assert!(
        types.contains("TreeItem::Tree(inner)") && types.contains("u32, TreeItem::Tree(inner)"),
        "the union must be decoded by trial matching: {types}"
    );
}

/// `parameter_shape_supported_inner`'s reservation arm is reachable, but only after an upstream
/// failure, and answering no is what keeps an unknown shape from being accepted as a parameter.
///
/// Parameters are lowered after every component, so no reservation is open by then. One is left
/// behind for good when a component's lowering fails: `A` reserves its id, `B` is lowered inside
/// it and closes the cycle back to that reservation, `B` completes and is cached, and then `A`
/// fails on its unresolved `x`. `B`'s items still name `A`'s never-filled reservation, and the
/// query parameter referencing `B` walks into it. The document is rejected by `E004` whatever the
/// arm says; the arm decides whether the parameter is also refused (`E010`) or waved through on
/// the strength of nothing. The waving-through mutation survived every suite before this fixture.
#[test]
fn a_parameter_reaching_a_failed_components_reservation_is_refused() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /q:
    get:
      operationId: getQ
      parameters:
        - { name: q, in: query, schema: { $ref: '#/components/schemas/B' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    A:
      type: object
      properties:
        bs: { $ref: '#/components/schemas/B' }
        x: { $ref: '#/components/schemas/Missing' }
    B:
      type: array
      items: { $ref: '#/components/schemas/A' }
"##;
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert_eq!(
            codes(&report),
            vec!["E004", "E010"],
            "{entry}: the failed component is reported where it fails, and the parameter whose \
             shape depends on it is refused rather than accepted: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedParameterStyle
                    && d.pointer.as_str() == "/paths/~1q/get/parameters/0"
            }),
            "{entry}: E010 must be reported against the parameter: {report:#?}"
        );
    }
}

/// A self-referential component (`Node.next -> Node`) once recursed forever, then was rejected as
/// E014. It must now generate: the cycle-closing `$ref` is boxed so the recursive type is finite.
#[test]
fn self_recursive_ref_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Node:
      type: object
      properties:
        next:
          $ref: "#/components/schemas/Node"
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "recursive schema must not raise an error: {report:#?}"
    );
}

/// Mutually-recursive components (`A -> B -> A`) must also generate, directly and through an
/// array. Directly, exactly one back-edge in the cycle is boxed; through an array, the array's
/// `Vec` already puts the cycle behind a pointer, so neither edge is.
#[test]
fn mutually_recursive_refs_generate() {
    let spec = |back_edge: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths: {{}}
components:
  schemas:
    A:
      type: object
      properties:
        b:
          $ref: "#/components/schemas/B"
    B:
      type: object
      properties:
{back_edge}"##
        )
    };
    // Each struct declares one field, so the field types below are every edge of the cycle: in
    // the direct case `A.b` is unboxed and `B.a` boxed, and through the array neither is.
    let cases = [
        (
            "direct",
            spec("        a:\n          $ref: \"#/components/schemas/A\"\n"),
            ("pub a:", "Option<Box<A>>"),
            None,
        ),
        (
            "through an array",
            spec(
                "        children:\n          type: array\n          items:\n            \
                 $ref: \"#/components/schemas/A\"\n",
            ),
            ("pub children:", "Option<Bchildren>"),
            Some(("Bchildren", "Vec<A>")),
        ),
    ];
    for (case, spec, (back_edge, back_edge_type), alias) in cases {
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{case}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|d| d.severity != spargen::Severity::Error),
            "{case}: mutually-recursive schemas must not raise an error: {report:#?}"
        );
        // Read from the `types` module on, past the embedded runtime's own declarations.
        let types = types_module(&code);
        assert_eq!(declared_fields(&types, "A"), ["b"], "{case}: {types}");
        assert_eq!(
            field_type(&types, "pub b:").as_deref(),
            Some("Option<B>"),
            "{case}: {types}"
        );
        assert_eq!(declared_fields(&types, "B").len(), 1, "{case}: {types}");
        assert_eq!(
            field_type(&types, back_edge).as_deref(),
            Some(back_edge_type),
            "{case}: {types}"
        );
        if let Some((name, target)) = alias {
            assert_eq!(
                alias_target(&types, name).as_deref(),
                Some(target),
                "{case}: {types}"
            );
        }
    }
}

/// A `$ref` that closes a cycle resolves to a *reserved* id whose def is still the
/// `TypeKind::Any` placeholder `TypeGraph::reserve` put there — the component's real shape is not
/// known until its own body finishes. `intersect_non_null`'s `(Any, _)` arm returns the sibling
/// unchanged, so intersecting against that placeholder is a no-op and **the target is silently
/// discarded**: `Node.next: {$ref: Node, type: object, properties: {x}}` generated a standalone
/// `Nodenext` with the recursion gone, and `{$ref: Node, type: string}` generated `String` for a
/// schema nothing satisfies. Both were `Generated` and `check`-clean, which is the silent
/// degradation the taxonomy forbids.
///
/// The `allOf` spelling of the identical conjunction has always rejected this, so the two
/// spellings now agree. A cycle-closing `$ref` with NO shape-bearing sibling still boxes and
/// generates — that is the ordinary recursive schema, and the third case pins it.
#[test]
fn a_cycle_closing_ref_whose_siblings_bear_a_shape_is_rejected_not_discarded() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths: {}\n";

    // (what it exercises, the `next` subschema, the pointer E013 must carry)
    let rejected: &[(&str, &str, &str)] = &[
        (
            "an object sibling on a self-recursive back-edge",
            "$ref: '#/components/schemas/Node'\n          type: object\n          properties: { x: { type: string } }",
            "/components/schemas/Node/properties/next",
        ),
        (
            "a scalar sibling on a self-recursive back-edge",
            "$ref: '#/components/schemas/Node'\n          type: string",
            "/components/schemas/Node/properties/next",
        ),
    ];
    for (what, next, pointer) in rejected {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          {next}\n"
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` was not rejected by {entry}, so the recursive target is still being \
                 silently discarded: {report:#?}"
            );
            let pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::AllOfIrreconcilable)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                pointers,
                vec![*pointer],
                "`{what}` through {entry} must report E013 once, at the offending `$ref`: \
                 {report:#?}"
            );
        }
        // The message must name the cause the reader can act on — a recursive reference — not the
        // generic empty-intersection wording, which would send them looking for a contradiction
        // that is not there.
        let report = generate(&spec);
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("closes a reference cycle"),
            "`{what}` must say the reference is recursive: {:?}",
            messages[0]
        );
    }

    // Mutual recursion reaches the same placeholder one component further out.
    let mutual = format!(
        "{HEAD}components:\n  schemas:\n    A:\n      type: object\n      properties:\n        b: {{ $ref: '#/components/schemas/B' }}\n    B:\n      type: object\n      properties:\n        a:\n          $ref: '#/components/schemas/A'\n          type: object\n          properties: {{ x: {{ type: string }} }}\n"
    );
    let report = generate(&mutual);
    assert_eq!(
        report.outcome(),
        Outcome::Rejected,
        "a mutually recursive back-edge was not rejected: {report:#?}"
    );
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // The control, and the reason the guard is gated on the sibling bearing a shape at all: an
    // ordinary recursive schema still boxes its back-edge and generates, keeping the recursion.
    let plain = format!(
        "{HEAD}components:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          $ref: '#/components/schemas/Node'\n          description: an ordinary recursive reference\n"
    );
    let (report, code) = generate_with_code(&plain);
    assert_ne!(
        report.outcome(),
        Outcome::Rejected,
        "the new rejection crept into an ordinary recursive schema: {report:#?}"
    );
    assert!(
        code.contains("Option<Box<Node>>"),
        "the recursion must survive as a boxed back-edge: {code}"
    );
}

/// A repeated `allOf` property whose one side is a `$ref` back to the schema being lowered cannot
/// be intersected: the target's body is not known yet, so the merge cannot tell an empty
/// intersection from a meeting one, and typing the property uninhabited — what an optional
/// conflicting property now gets — would be a guess that deletes every value it might hold. It
/// stays `E013`, and the message names the cycle rather than a conflict nobody wrote.
#[test]
fn an_all_of_property_typed_by_an_unlowered_cycle_is_refused_not_made_uninhabited() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /n:
    get:
      operationId: getN
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Node' }
components:
  schemas:
    Node:
      allOf:
        - { type: object, properties: { a: { $ref: '#/components/schemas/Node' } } }
        - { type: object, properties: { a: { type: string } } }
"##;
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("closes a reference cycle")),
            "{entry} must name the cycle, not a conflict: {messages:?}"
        );
    }
}

/// Accept-versus-reject must not key on `components.schemas` map order.
///
/// The round-2 guard tested `in_progress` membership, which is a property of *when* lowering
/// happens: `oas31::lower` pre-lowers components in map iteration order, so for mutual recursion it
/// fired on whichever entry was declared first. Two documents identical but for the order of two
/// map entries — a no-op in OpenAPI, and a routine difference between description generators — got
/// opposite verdicts: one `Rejected`, one `Generated`.
///
/// The guard now keys on the schema: a `$ref` whose target reaches back to the component enclosing
/// it closes a reference cycle, and that is true of the document however its entries are ordered.
/// The verdict is the same one the `allOf` spelling has always given; what changed is that it no
/// longer depends on serialisation.
#[test]
fn the_recursive_ref_guard_does_not_key_on_component_declaration_order() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths: {}\n";
    const A: &str = r##"    A:
      type: object
      properties:
        b: { $ref: '#/components/schemas/B' }
"##;
    const B: &str = r##"    B:
      type: object
      properties:
        a:
          $ref: '#/components/schemas/A'
          type: object
          properties: { x: { type: string } }
"##;

    let a_first = format!("{HEAD}components:\n  schemas:\n{A}{B}");
    let b_first = format!("{HEAD}components:\n  schemas:\n{B}{A}");

    let mut verdicts = Vec::new();
    for (order, spec) in [("A first", &a_first), ("B first", &b_first)] {
        let report = generate(spec);
        verdicts.push((
            order.to_owned(),
            report.outcome(),
            has_code(&report, Code::AllOfIrreconcilable),
        ));
    }
    let first = (verdicts[0].1, verdicts[0].2);
    for (label, outcome, coded) in &verdicts {
        assert_eq!(
            (*outcome, *coded),
            first,
            "`{label}` disagrees with `{}`: re-ordering two `components.schemas` entries is a \
             no-op in OpenAPI, so it cannot change accept-versus-reject. All verdicts: {verdicts:#?}",
            verdicts[0].0
        );
    }
    // And the verdict both orderings must reach is the `allOf` spelling's, so the three spellings
    // of one conjunction still agree.
    assert_eq!(first.0, Outcome::Rejected, "{verdicts:#?}");
    assert!(first.1, "{verdicts:#?}");

    // The message is now a statement about the schema, not about lowering order: "whose fields are
    // not yet known" was only true in the ordering that happened to reject.
    let report = generate(&a_first);
    let messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("closes a reference cycle"),
        "the message must name a property of the document, not of the lowering order: {:?}",
        messages[0]
    );
    assert!(
        !messages[0].contains("not yet known"),
        "`not yet known` is a lowering-order claim, false in the other ordering: {:?}",
        messages[0]
    );

    // The controls, in both orderings: a `$ref` with shape-bearing siblings whose target does NOT
    // reach back to it still intersects and generates.
    const PLAIN: &str = r##"    Leaf: { type: object, properties: { y: { type: integer } } }
    Holder:
      type: object
      properties:
        l:
          $ref: '#/components/schemas/Leaf'
          type: object
          properties: { x: { type: string } }
"##;
    let acyclic = format!("{HEAD}components:\n  schemas:\n{PLAIN}");
    let report = generate(&acyclic);
    assert_ne!(
        report.outcome(),
        Outcome::Rejected,
        "an acyclic `$ref` with shape-bearing siblings must still intersect: {report:#?}"
    );
}

/// The third spelling of the same conjunction. The `$ref`-sibling arm and the `allOf` arm both
/// guard a cycle-closing reference; the `oneOf`/`anyOf` sibling path — which is this pull
/// request's own subject — did not, so it went on reading the `TypeKind::Any` placeholder.
/// `lower_union_variant` receives the target's RESERVED id, `intersect_non_null`'s `(Any, _)` arm
/// returns the sibling unchanged, and the recursive target is silently discarded:
/// `Node.next: {type: object, properties: {x}, oneOf: [{$ref: Node}]}` generated cleanly with
/// `Node`'s own `next` field gone from the emitted struct.
///
/// An independent Draft 2020-12 validator accepts arbitrarily deep `next` chains under that schema,
/// so the emitted type described a strictly smaller language than the document. Three spellings of
/// one conjunction were `Rejected` / `Rejected` / silently wrong.
#[test]
fn a_cycle_closing_union_member_is_rejected_not_discarded() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths: {}\n";

    // (what it exercises, the `next` subschema)
    let rejected: &[(&str, &str)] = &[
        (
            "a sole-member `oneOf` back-edge under a shape-bearing sibling",
            "type: object\n          properties: { x: { type: string } }\n          oneOf: [{ $ref: '#/components/schemas/Node' }]",
        ),
        (
            "a sole-member `anyOf` back-edge under a shape-bearing sibling",
            "type: object\n          properties: { x: { type: string } }\n          anyOf: [{ $ref: '#/components/schemas/Node' }]",
        ),
        (
            "a multi-variant union whose back-edge variant meets a shape-bearing sibling",
            "type: object\n          properties: { x: { type: string } }\n          oneOf: [{ $ref: '#/components/schemas/Node' }, { type: object, properties: { y: { type: integer } } }]",
        ),
        (
            "a back-edge beside a null member, under a shape-bearing sibling",
            "type: object\n          properties: { x: { type: string } }\n          oneOf: [{ $ref: '#/components/schemas/Node' }, { type: 'null' }]",
        ),
    ];
    for (what, next) in rejected {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          {next}\n"
        );
        let report = generate(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` was not rejected, so the recursive target is still being silently discarded \
             on the union path: {report:#?}"
        );
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` did not report E013: {report:#?}"
        );
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("closes a reference cycle")),
            "`{what}` must name the cycle rather than the generic empty-intersection wording: \
             {messages:?}"
        );
    }

    // The controls: the guard is reached only when there IS a sibling, so it must not fire on a
    // recursive union that has none.
    //
    // What those documents then do is not this change's to decide, and the answer moved under the
    // merge. Before it, the sole-member collapse read the reserved id's placeholder for its own
    // kind and emitted `pub type Nodenext = serde_json::Value;` — the recursion lost and the type
    // degraded, which the standing invariant forbids; filed as #160 and measured byte-identical at
    // `a45d95c`. The parent's reservation work replaced that placeholder with `TypeKind::Reserved`
    // and added an IR invariant, so the same documents now REJECT with `E011` naming the unlowered
    // reservation. That closes #160's silent degradation, and it is the parent's decision, not this
    // one's — so the assertion here is only ever about this guard: `E013` must not fire.
    let permitted: &[(&str, &str)] = &[
        (
            "a recursive union with no shape-bearing sibling",
            "oneOf: [{ $ref: '#/components/schemas/Node' }, { type: 'null' }]",
        ),
        (
            "a recursive union whose only sibling is validation-only",
            "description: plain\n          oneOf: [{ $ref: '#/components/schemas/Node' }, { type: 'null' }]",
        ),
        (
            "a sole-member recursive union with no sibling",
            "oneOf: [{ $ref: '#/components/schemas/Node' }]",
        ),
    ];
    for (what, next) in permitted {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          {next}\n"
        );
        let report = generate(&spec);
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "the union guard crept into `{what}`, which has no sibling to intersect with: \
             {report:#?}"
        );
        // And the outcome is the parent's reservation invariant, not a silent degradation: the
        // `serde_json::Value` #160 records is gone in both directions.
        let (_, code) = generate_with_code(&spec);
        assert_no_untyped_value(&code);
    }

    // A plain recursive `$ref` with no sibling at all still boxes and generates — the shape that
    // makes recursion usable, and the one neither this guard nor the reservation invariant touches.
    let plain = format!(
        "{HEAD}components:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          $ref: '#/components/schemas/Node'\n"
    );
    let (report, code) = generate_with_code(&plain);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("Option<Box<Node>>"), "{code}");

    // And a union member that is an ACYCLIC `$ref` must still intersect with the sibling.
    let acyclic = format!(
        "{HEAD}components:\n  schemas:\n    Leaf: {{ type: object, properties: {{ y: {{ type: integer }} }} }}\n    Holder:\n      type: object\n      properties:\n        l:\n          type: object\n          properties: {{ x: {{ type: string }} }}\n          oneOf: [{{ $ref: '#/components/schemas/Leaf' }}]\n"
    );
    let report = generate(&acyclic);
    assert_ne!(
        report.outcome(),
        Outcome::Rejected,
        "an acyclic union member with a shape-bearing sibling must still intersect: {report:#?}"
    );
}

/// The cycle predicate must count only the edges lowering actually follows.
///
/// `collect_schema_refs` chained `schema.defs` and `schema.validation_children`, so `$defs`, `not`,
/// `if`/`then`/`else`, `contains`, `propertyNames`, `unevaluated*` and `dependentSchemas` were all
/// treated as cycle edges. **Lowering never descends into any of them** — `.defs` and
/// `validation_children` appeared exactly once each in the whole of the lowering pass (then
/// `lower.rs`, now `oas31/lower/`), inside that walk — so a `$ref` reachable only that way can
/// never put a component mid-flight and can never yield a placeholder. The guard rejected anyway, asserting a dependence that does not exist.
///
/// The consequence is sharp: adding **unreferenced `$defs`** to a document, which contributes zero
/// emitted bytes and does not change the instance set by a single value, turned `Generated` into a
/// hard `E013`. Both oracles agree the two documents below admit exactly the same instances.
#[test]
fn the_cycle_predicate_counts_only_edges_lowering_follows() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths: {}\n";
    // `Holder.l` intersects `Leaf` with shape-bearing siblings. `Leaf` reaches back to `Holder`
    // ONLY through the keyword under test, so the cycle is invisible to lowering.
    const HOLDER: &str = r##"    Holder:
      type: object
      properties:
        l:
          $ref: '#/components/schemas/Leaf'
          type: object
          properties: { x: { type: string } }
"##;

    let baseline = format!(
        "{HEAD}components:\n  schemas:\n    Leaf:\n      type: object\n      properties: {{ y: {{ type: integer }} }}\n{HOLDER}"
    );
    let (base_report, base_code) = generate_with_code(&baseline);
    assert_ne!(base_report.outcome(), Outcome::Rejected, "{base_report:#?}");
    let base_types = types_module(&base_code);

    // Each of these adds a back-edge through a keyword lowering does not traverse. None changes the
    // instance set, and none can produce a placeholder.
    let inert: &[(&str, &str)] = &[
        (
            "an unreferenced `$defs` entry",
            "      $defs:\n        Back: { $ref: '#/components/schemas/Holder' }\n",
        ),
        (
            "an `if` with no `then`/`else`",
            "      if: { $ref: '#/components/schemas/Holder' }\n",
        ),
        (
            "a `not`",
            "      not: { $ref: '#/components/schemas/Holder' }\n",
        ),
        (
            "a `propertyNames`",
            "      propertyNames: { $ref: '#/components/schemas/Holder' }\n",
        ),
    ];
    for (what, extra) in inert {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Leaf:\n      type: object\n      properties: {{ y: {{ type: integer }} }}\n{extra}{HOLDER}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` is not an edge lowering follows, so it cannot make the `$ref` a \
             cycle-closing one and must not flip into a rejection: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` reported E013: {report:#?}"
        );
        // Stronger than "still generates": the emitted types are the ones the baseline emits, so
        // the keyword is confirmed inert rather than merely tolerated.
        let types = types_module(&code);
        assert_eq!(
            types, base_types,
            "`{what}` changed the emitted types, so it is not inert after all"
        );
    }

    // The control, and the reason the predicate exists: a back-edge through a `properties` value —
    // an edge lowering DOES follow — still closes the cycle and still rejects.
    let real_cycle = format!(
        "{HEAD}components:\n  schemas:\n    Leaf:\n      type: object\n      properties:\n        back: {{ $ref: '#/components/schemas/Holder' }}\n{HOLDER}"
    );
    let report = generate(&real_cycle);
    assert_eq!(
        report.outcome(),
        Outcome::Rejected,
        "a cycle through `properties` is an edge lowering follows and must still reject: \
         {report:#?}"
    );
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// The rows of `untyped_branch_nullability::a_cycle_closing_untyped_ref_branch_beside_a_null_branch_matches_the_same_union_outside_it`
/// for a component whose body is an untyped `allOf` rather than object keywords of its own, which
/// lowers to the struct the members merge into (#627): `Node.next` sits in a member, `Holder.pick`
/// carries the same union, and the two agree. A `oneOf` beside a `null` branch admits no `null`
/// whether the members are inline, sit beside object keywords, or include a `$ref` to an untyped
/// component; a member stating `type: object` decides `null`, and an `anyOf` admits it, so those
/// stay `Option` in both places.
#[test]
fn a_cycle_closing_ref_branch_to_an_untyped_all_of_component_matches_the_same_union_outside_it() {
    let one_of = "{ oneOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }";
    let any_of = "{ anyOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }";
    let mut mismatches = Vec::new();
    for (body, site, nullable) in [
        (
            "allOf: [ { properties: { a: { type: string } } }, { properties: { next: SITE }, \
             required: [next] } ]",
            one_of,
            false,
        ),
        (
            "properties: { a: { type: string } }, allOf: [ { properties: { next: SITE }, \
             required: [next] } ]",
            one_of,
            false,
        ),
        (
            "allOf: [ { $ref: '#/components/schemas/Base' }, { properties: { next: SITE }, \
             required: [next] } ]",
            one_of,
            false,
        ),
        (
            "allOf: [ { $ref: '#/components/schemas/Arr' }, { properties: { next: SITE }, \
             required: [next] } ]",
            one_of,
            false,
        ),
        (
            "allOf: [ { items: { type: string } }, { properties: { next: SITE }, required: \
             [next] } ]",
            one_of,
            false,
        ),
        (
            "allOf: [ { type: object, properties: { a: { type: string } } }, { properties: { \
             next: SITE }, required: [next] } ]",
            one_of,
            true,
        ),
        (
            "allOf: [ { properties: { a: { type: string } } }, { properties: { next: SITE }, \
             required: [next] } ]",
            any_of,
            true,
        ),
    ] {
        let body = body.replace("SITE", site);
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Base: {{ properties: {{ b: {{ type: string }} }} }}\n    Arr: {{ items: {{ \
                 type: string }} }}\n    Node: {{ {body} }}\n    \
                 Holder:\n      type: object\n      properties:\n        pick: {site}\n      \
                 required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        let types = types_module(&code);
        for field in ["pub next", "pub pick"] {
            let ty = field_type(&types, field)
                .unwrap_or_else(|| panic!("{body}: no `{field}` field: {types}"));
            if ty.starts_with("Option<") != nullable {
                let validity = if nullable { "valid" } else { "invalid" };
                mismatches.push(format!(
                    "{body}: `{field}` is `{ty}`, but `null` is {validity} here"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// The array counterpart of
/// `a_cycle_closing_ref_branch_to_an_untyped_all_of_component_matches_the_same_union_outside_it`
/// (#636): an untyped `allOf` whose only member is untyped `items` merges into a `Vec`, which
/// leaves `null` undecided, so a `oneOf` beside a `null` branch admits no `null` both in the
/// cycle-closing `items` and at `Holder.pick`, as the same `items` written without the `allOf`
/// already lowered. The cycle site read the merge as a shape it could not read and kept the
/// reservation's `Option`.
#[test]
fn a_cycle_closing_ref_branch_to_an_untyped_array_all_of_matches_the_same_union_outside_it() {
    let site = "{ oneOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }";
    for body in [
        format!("allOf: [ {{ items: {site} }} ]"),
        format!("items: {site}"),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Node: {{ {body} }}\n    Holder:\n      type: object\n      properties:\n        \
                 pick: {site}\n      required: [pick]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{body}: {report:#?}");
        let types = types_module(&code);
        assert!(
            types.contains("pub struct Node(pub Vec<Node>);"),
            "{body}: `null` is invalid in `Node`'s items, so they are not `Option`: {types}"
        );
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{body}: no `pick` field: {types}"));
        assert!(
            !pick.starts_with("Option<"),
            "{body}: `pick` is `{pick}`, but `null` is invalid here: {types}"
        );
    }
}

/// Issue #648: an array or tuple component whose items reach that component again through other
/// arrays and tuples alone was emitted as a type alias naming itself (`pub type Node = Vec<Node>;`),
/// which rustc rejects (`E0391`, a cycle expanding the alias). Each member of such a cycle is a
/// transparent newtype instead, whatever `Option`/`Box` wraps the reference, while an array that
/// only points into the cycle, or one whose cycle passes through a struct, stays an alias.
#[test]
fn an_array_or_tuple_closing_an_alias_cycle_is_a_newtype() {
    let spec = with_schemas(
        "3.1.0",
        "    Node: { type: array, items: { $ref: '#/components/schemas/Node' } }\n    \
         Maybe: { type: array, items: { oneOf: [{ $ref: '#/components/schemas/Maybe' }, { type: \
         'null' }] } }\n    \
         Tup: { type: array, prefixItems: [{ $ref: '#/components/schemas/Tup' }], items: false }\n    \
         Ping: { type: array, items: { $ref: '#/components/schemas/Pong' } }\n    \
         Pong: { type: array, prefixItems: [{ $ref: '#/components/schemas/Ping' }], items: false }\n    \
         Into: { type: array, items: { $ref: '#/components/schemas/Node' } }\n    \
         Leaf: { type: object, properties: { kids: { $ref: '#/components/schemas/Kids' } } }\n    \
         Kids: { type: array, items: { $ref: '#/components/schemas/Leaf' } }\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = types_module(&code);
    for newtype in [
        "pub struct Node(pub Vec<Node>);",
        "pub struct Maybe(pub Vec<Option<Maybe>>);",
        "pub struct Tup(pub (Box<Tup>,));",
        "pub struct Ping(pub Vec<Pong>);",
        "pub struct Pong(pub (Box<Ping>,));",
    ] {
        assert!(types.contains(newtype), "no `{newtype}`: {types}");
    }
    assert_eq!(
        types.matches("#[serde(transparent)]").count(),
        5,
        "every newtype, and nothing else, is transparent on the wire: {types}"
    );
    for alias in ["pub type Into = Vec<Node>;", "pub type Kids = Vec<Leaf>;"] {
        assert!(types.contains(alias), "no `{alias}`: {types}");
    }
}

/// An untyped `allOf` whose `items` member closes a cycle through a union, beside a member that
/// constrains the items with no type of their own (`{items: {maxItems: 3}}`, inline or as a `$ref`
/// to an `items`-only component), generates as the same `allOf` without that member does (#641).
/// The second member's items lower to `Value`, and the merge meets the cycle-closing `Node` with
/// it: `Node ∩ Value` is `Node` whatever `Node`'s body turns out to be, so the meet answers with
/// the reservation, as it answers with a finished target outside the cycle. It used to refuse
/// every reservation not met with itself, which was `E013` here while the spelling over a `$ref`
/// outside the cycle generated. Both member orders, through `generate` and `check`.
#[test]
fn a_cycle_closing_item_met_with_untyped_items_generates_as_without_them() {
    let site = "{ oneOf: [ { $ref: '#/components/schemas/Node' }, { type: 'null' } ] }";
    let cycle = format!("{{ items: {site} }}");
    for other in [
        "{ $ref: '#/components/schemas/Arr' }",
        "{ items: { maxItems: 3 } }",
    ] {
        for members in [format!("{cycle}, {other}"), format!("{other}, {cycle}")] {
            let spec = with_schemas(
                "3.1.0",
                &format!(
                    "    Arr: {{ items: {{ maxItems: 3 }} }}\n    Node: {{ allOf: [ {members} ] \
                     }}\n    Holder:\n      type: object\n      properties:\n        pick: \
                     {site}\n      required: [pick]\n"
                ),
            );
            let (report, code) = generate_with_code(&spec);
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{members}: {report:#?}"
            );
            assert!(
                messages_for(&report, Code::AllOfIrreconcilable).is_empty(),
                "{members}: {report:#?}"
            );
            let types = types_module(&code);
            assert!(
                types.contains("pub struct Node(pub Vec<Node>);"),
                "{members}: the merge is the cycle-closing `items` alone, a newtype as the array \
                 closing its own cycle (#648): {types}"
            );
            let pick = field_type(&types, "pub pick")
                .unwrap_or_else(|| panic!("{members}: no `pick` field: {types}"));
            assert!(
                !pick.starts_with("Option<"),
                "{members}: `pick` is `{pick}`, as without the untyped member: {types}"
            );
        }
    }
}

/// The other positions the same meet reaches (#641): a repeated `allOf` property typed by a
/// cycle-closing `$ref` on one side and by an untyped schema on the other, and a tuple position
/// that closes the cycle met with untyped `items`. `X ∩ Value` is `X` there too, so each generates
/// with the cycle-closing reference — boxed where it is a struct field — rather than `E013`.
#[test]
fn a_cycle_closing_ref_met_with_an_untyped_schema_keeps_the_ref_in_a_field_and_a_tuple() {
    let spec = with_schemas(
        "3.1.0",
        "    Node:\n      allOf:\n        - { type: object, properties: { next: { $ref: \
         '#/components/schemas/Node' } } }\n        - { type: object, properties: { next: { \
         maxLength: 3 } } }\n    Tup:\n      allOf:\n        - { type: array, prefixItems: [ { \
         $ref: '#/components/schemas/Tup' } ] }\n        - { type: array, items: { maxItems: 3 } \
         }\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let next = field_type(&types, "pub next").unwrap_or_else(|| panic!("no `next`: {types}"));
    assert!(
        next.contains("Box<Node>"),
        "`next` keeps the boxed cycle-closing `Node`: `{next}` in {types}"
    );
    // The meet is the `prefixItems` member alone. `Tup` closes its own cycle through that tuple, so
    // it is a transparent newtype over it (#648); the member's own lowering, `TupMember0`, which
    // only pointed into the cycle, is no type of the output (#924).
    let line_after = |prefix: &str| {
        types
            .lines()
            .map(str::trim_start)
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::to_owned)
    };
    let tuple = line_after("pub struct Tup(pub ")
        .and_then(|rest| rest.strip_suffix(");").map(|body| format!("{body};")))
        .unwrap_or_else(|| panic!("no `Tup` newtype: {types}"));
    assert!(
        tuple.contains("Box<Tup>"),
        "`Tup` keeps the cycle-closing position: `{tuple}`"
    );
    assert_eq!(
        tuple, "(Box<Tup>,);",
        "`Tup` is its `prefixItems` member, as without the untyped `items`: {types}"
    );
    assert_eq!(line_after("pub type TupMember0 = "), None, "{types}");
}

/// The `additionalProperties` position of the same meet (#641): two `allOf` members both constrain
/// the value schema, one with a `$ref` that closes the cycle and the other untyped
/// (`{maxLength: 3}`). `merge_additional` meets the two value types through `intersect_types`, so
/// `A ∩ Value` is `A` here as well, and the map's values keep the cycle-closing reference rather
/// than `E013`. A typed other side (`type: string`) is still refused, as
/// `an_all_of_additional_properties_back_edge_says_why_it_cannot_merge` pins.
#[test]
fn a_cycle_closing_additional_properties_met_with_an_untyped_value_keeps_the_ref() {
    let spec = with_schemas(
        "3.1.0",
        "    A:\n      allOf:\n        - { type: object, additionalProperties: { $ref: \
         '#/components/schemas/A' } }\n        - { type: object, additionalProperties: { \
         maxLength: 3 } }\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        messages_for(&report, Code::AllOfIrreconcilable).is_empty(),
        "{report:#?}"
    );
    let types = types_module(&code);
    assert!(
        types.contains("BTreeMap<String, A>"),
        "the map's values keep the cycle-closing `A`: {types}"
    );
}
