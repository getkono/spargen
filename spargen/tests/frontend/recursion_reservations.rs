//! Recursion: the reservations a component holds while it is lowered — self-union reservations,
//! siblings over an open reservation, back-edge ranking, and basic recursive generation.

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
// E007 case: resolves-to-itself
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

/// A `oneOf` that lists the same cycle-closing `$ref` twice merges its two members under `W001`
/// like any repeated member, and the one variant left is the component's own open reservation.
/// That variant's kind could not be read: re-emitting it cloned a `TypeKind::Reserved` into a
/// second reservation nothing filled, and `check_invariants` refused the document with `E011`
/// (#1152). The position is the component itself, boxed, as the direct `$ref` spelling is.
#[test]
fn a_one_of_repeating_a_cycle_closing_ref_merges_to_the_boxed_component() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    S2:
      type: object
      properties:
        a: { oneOf: [{ $ref: '#/components/schemas/S2' }, { $ref: '#/components/schemas/S2' }] }
"##;
    let (report, code) = generate_with_code(spec);
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|d| d.severity != spargen::Severity::Error),
            "{entry}: the merged self-reference must raise no error: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/components/schemas/S2/properties/a"
                    && d.message.contains("same generated type")
            }),
            "{entry}: the repeated member must still be reported as `W001`: {report:#?}"
        );
    }
    let types = types_module(&code);
    assert!(
        types.contains("Option<Box<S2>>"),
        "`S2.a` must be the boxed component itself: {types}"
    );
    assert!(
        !types.contains("S2a"),
        "the merged union must not be re-emitted as a type of its own: {types}"
    );
}

/// The same merge with a `null` member beside the repeated cycle-closing `$ref`, at a required
/// property, an array's items and a map's values: each position is the component itself taking
/// the union's `null` (`Option<Box<S2>>` although the property is required, `Option<S2>` inside
/// the `Vec` and the map), and no type is emitted for any of the merged unions (#1152).
#[test]
fn a_nullable_one_of_repeating_a_cycle_closing_ref_merges_to_the_optional_boxed_component() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    S2:
      type: object
      required: [a]
      properties:
        a:
          oneOf:
            - { $ref: '#/components/schemas/S2' }
            - { $ref: '#/components/schemas/S2' }
            - { type: 'null' }
        b:
          type: array
          items:
            oneOf:
              - { $ref: '#/components/schemas/S2' }
              - { $ref: '#/components/schemas/S2' }
              - { type: 'null' }
        c:
          type: object
          additionalProperties:
            oneOf:
              - { $ref: '#/components/schemas/S2' }
              - { $ref: '#/components/schemas/S2' }
              - { type: 'null' }
"##;
    let (report, code) = generate_with_code(spec);
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|d| d.severity != spargen::Severity::Error),
            "{entry}: the merged nullable self-reference must raise no error: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/components/schemas/S2/properties/a"
                    && d.message.contains("same generated type")
            }),
            "{entry}: the repeated member must still be reported as `W001`: {report:#?}"
        );
    }
    let types = types_module(&code);
    assert!(
        types.contains("pub a: Option<Box<S2>>"),
        "the required `S2.a` must be the nullable boxed component itself: {types}"
    );
    assert!(
        types.contains("pub type S2b = Vec<Option<S2>>;"),
        "`S2.b`'s items must be the nullable component itself: {types}"
    );
    assert!(
        types.contains("pub additional: BTreeMap<String, Option<S2>>,"),
        "`S2.c`'s values must be the nullable component itself: {types}"
    );
    for merged in ["S2a", "S2bItem", "S2cAdditional", "Variant"] {
        assert!(
            !types.contains(merged),
            "no merged union may be re-emitted as a type of its own (`{merged}`): {types}"
        );
    }
}

/// The same merge where the union is a component's whole body and its one member closes a cycle
/// through another component (`A = B | B`, `B.x -> A`) has no def to hand back as `A`'s root, so
/// it is refused as `E007` with its own wording rather than failing the IR invariant as `E011`
/// (#1152).
#[test]
fn a_component_body_repeating_a_cycle_closing_ref_is_refused_not_an_invariant_failure() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    B:
      type: object
      properties:
        x: { $ref: '#/components/schemas/A' }
    A:
      oneOf: [{ $ref: '#/components/schemas/B' }, { $ref: '#/components/schemas/B' }]
"##;
    for (entry, report) in [("generate", &generate(spec)), ("check", &check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let errors: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.severity == spargen::Severity::Error)
            .collect();
        assert!(
            errors.iter().all(|d| d.code == Code::NonDisjointUnion
                && d.pointer.as_str() == "/components/schemas/A"
                && d.message.contains("members all lower to one `$ref`")),
            "{entry}: the only error must be `E007` at `A`: {report:#?}"
        );
        assert!(!errors.is_empty(), "{entry}: {report:#?}");
    }
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
