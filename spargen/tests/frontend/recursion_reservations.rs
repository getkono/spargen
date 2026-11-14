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
        let report = generate(spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
        // A spec-facing code with a matrix cell, an `errors.md` row and this fixture — never
        // the internal invariant's `E011`, whose explain text claims malformed input.
        assert!(
            !has_code(&report, Code::InvalidInput),
            "{label}: {report:#?}"
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
        assert_eq!(codes, vec![Code::NonDisjointUnion], "{label}: {report:#?}");
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
            "{label}: the remedy must name what an author with a self-referential union actually \
             has to change: {remedy:?}"
        );
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
        let (report, _, _) = split("./lib.yaml#/components/schemas/Selfy", &lib);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{spelling}: {report:#?}"
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
            "{spelling}: {report:#?}"
        );
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
    // The message read is the first `E013`, and the parity oracle compares diagnostics unordered,
    // so `check` stays.
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
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        codes(&report),
        vec!["E004", "E010"],
        "the failed component is reported where it fails, and the parameter whose shape depends \
         on it is refused rather than accepted: {report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::UnsupportedParameterStyle
                && d.pointer.as_str() == "/paths/~1q/get/parameters/0"
        }),
        "E010 must be reported against the parameter: {report:#?}"
    );
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
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "the merged self-reference must raise no error: {report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::ValidationKeywordIgnored
                && d.pointer.as_str() == "/components/schemas/S2/properties/a"
                && d.message.contains("same generated type")
        }),
        "the repeated member must still be reported as `W001`: {report:#?}"
    );
    let types = types_module(&code);
    assert!(
        types.contains("Option<Box<S2>>"),
        "`S2.a` must be the boxed component itself: {types}"
    );
    assert!(
        !types.contains("S2A"),
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
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "the merged nullable self-reference must raise no error: {report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::ValidationKeywordIgnored
                && d.pointer.as_str() == "/components/schemas/S2/properties/a"
                && d.message.contains("same generated type")
        }),
        "the repeated member must still be reported as `W001`: {report:#?}"
    );
    let types = types_module(&code);
    assert!(
        types.contains("pub a: Option<Box<S2>>"),
        "the required `S2.a` must be the nullable boxed component itself: {types}"
    );
    assert!(
        types.contains("pub type S2B = Vec<Option<S2>>;"),
        "`S2.b`'s items must be the nullable component itself: {types}"
    );
    assert!(
        types.contains("pub additional: BTreeMap<String, Option<S2>>,"),
        "`S2.c`'s values must be the nullable component itself: {types}"
    );
    for merged in ["S2A", "S2BItem", "S2CAdditional", "Variant"] {
        assert!(
            !types.contains(merged),
            "no merged union may be re-emitted as a type of its own (`{merged}`): {types}"
        );
    }
}

/// The same merge beside a third branch that the union's multi-category `type` array, which the
/// cycle-closing members meet in themselves, leaves no variant: at `a` the sibling excludes a
/// string enum (`W011`), whose own lowering inserted `S2Avariant2`; at `b` the meet narrows a
/// nullable string enum to `null`, inserting `S2Bvariant2Constrained`. Each position is the boxed
/// component, as with no third branch, and neither type is emitted. The types are inserted, so
/// the reservation branch's elision has something to elide; the output alone does not pin those
/// calls, since the pass that withholds every unreached type once lowering ends removes them too
/// (#1190).
#[test]
fn a_one_of_repeating_a_cycle_closing_ref_drops_the_branches_its_sibling_excludes() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    S2:
      type: object
      properties:
        a:
          type: [object, array]
          oneOf:
            - { $ref: '#/components/schemas/S2' }
            - { $ref: '#/components/schemas/S2' }
            - { type: string, enum: [x, y] }
        b:
          type: [object, array, 'null']
          oneOf:
            - { $ref: '#/components/schemas/S2' }
            - { $ref: '#/components/schemas/S2' }
            - { type: [string, 'null'], enum: [x, null] }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "the merged self-reference must raise no error: {report:#?}"
    );
    for property in ["a", "b"] {
        let pointer = format!("/components/schemas/S2/properties/{property}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == pointer
                    && d.message.contains("same generated type")
            }),
            "`{property}`'s repeated member must be reported as `W001`: {report:#?}"
        );
    }
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::DeclarationHasNoEffect
                && d.pointer.as_str() == "/components/schemas/S2/properties/a"
                && d.message.contains("union member 2 cannot satisfy")
        }),
        "the excluded string branch must be reported as `W011`: {report:#?}"
    );
    let types = types_module(&code);
    // `a` admits no `null`, so it is a plain optional `Option`; the nullable `b` is a `Nullable`.
    for (property, wrapper) in [("a", "Option"), ("b", "super::support::Nullable")] {
        assert!(
            types.contains(&format!("pub {property}: {wrapper}<Box<S2>>")),
            "`S2.{property}` must be the boxed component itself: {types}"
        );
    }
    for dropped in ["S2A", "S2B", "Variant"] {
        assert!(
            !types.contains(dropped),
            "no type may be emitted for a merged union or its dropped branch (`{dropped}`): {types}"
        );
    }
}

/// A `oneOf` beside a multi-category `type` array alone, whose first branch closes a cycle the
/// array admits and is kept unmet, before a branch the meet narrows to `null` and a typed one.
/// The null branch is dropped and hoisted to the property's `Option`, and the types its meet
/// inserted are elided by its own member position: the kept-unmet branch once recorded no meet
/// range, so the dropped branch read the typed branch's range and its own `null` was left behind
/// (#1226). Lowering asserts, in the debug build these tests run, that it records one range per
/// member; the leftover itself never reached the output, which the final reachability pass
/// withholds (#924).
#[test]
fn a_kept_unmet_cycle_branch_leaves_the_met_into_null_drop_aligned_with_its_members() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    S2:
      type: object
      properties:
        a:
          type: [object, string, 'null']
          oneOf:
            - { $ref: '#/components/schemas/S2' }
            - { items: { type: integer } }
            - { type: string, enum: [x, y] }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert!(report.diagnostics().is_empty(), "{entry}: {report:#?}");
    }
    let types = types_module(&code);
    assert!(
        types.contains("pub a: super::support::Nullable<S2A>"),
        "the dropped null branch must make `S2.a` nullable: {types}"
    );
    assert!(
        types.contains("S2(Box<S2>),") && types.contains("S2Avariant2(Box<S2Avariant2>),"),
        "the cycle-closing and string branches must stay the union's variants: {types}"
    );
    assert!(
        !types.contains("Variant1"),
        "no type may be emitted for the branch met into `null`: {types}"
    );
}

/// The same merge where the union is a component's whole body and its one member closes a cycle
/// through another component (`A = B | B`, `B.x -> A`) is a nullable alias for `B`, as the
/// one-member spelling `A = B` is: the reference to `A` taken while `B` is open is `B` itself,
/// boxed, and the repeated member is still reported as `W001` at `A`. It once failed the IR
/// invariant as `E011` (#1152), then was refused as `E007` (#1176). The repeated `$ref`s are
/// spelled two ways, a root pointer and the root file's own path, which resolve to one target.
#[test]
fn a_component_body_repeating_a_cycle_closing_ref_generates_as_an_alias() {
    for second in [
        "#/components/schemas/B",
        "./openapi.yaml#/components/schemas/B",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths: {{}}
components:
  schemas:
    B:
      type: object
      properties:
        x: {{ $ref: '#/components/schemas/A' }}
    A:
      oneOf: [{{ $ref: '#/components/schemas/B' }}, {{ $ref: '{second}' }}]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{second}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|d| d.severity != spargen::Severity::Error),
            "{second}: the repeated alias must raise no error: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/components/schemas/A"
                    && d.message
                        .contains("members 0, 1 lower to the same generated type")
            }),
            "{second}: the repeated member must still be reported as `W001`: {report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types.contains("pub x: Option<Box<B>>"),
            "{second}: `B.x` must be `B` itself, boxed where it closes the cycle: {types}"
        );
    }
}

/// The same merge where one member reaches the target through a component alias of it
/// (`A = B | C`, `C = B`, `B.x -> A`) generates in every order `components.schemas` declares the
/// three in, as the one-member alias spelling `A = C | null` does (#1189). Lowered `A` first, `B`
/// finishes inside `A` and the union is `B`'s shape re-emitted as `A`; lowered `B` first, `A` is
/// reached while `B` is open and the alias `C` is followed to `B`, so `A` is the nullable alias
/// for `B` rather than the `E007` "cannot be given a generated type" refusal it once drew in that
/// order alone (#1152). `C` written as a root alias of a sub-file alias of `B` is followed the
/// same way.
#[test]
fn a_component_body_reaching_a_cycle_closing_ref_through_an_alias_generates_in_every_order() {
    let b = "    B:\n      type: object\n      properties:\n        x: { $ref: '#/components/schemas/A' }\n";
    let bodies = [
        (
            "oneOf",
            "    A:\n      oneOf: [{ $ref: '#/components/schemas/B' }, { $ref: '#/components/schemas/C' }]\n",
        ),
        (
            "nullable",
            "    A:\n      oneOf: [{ $ref: '#/components/schemas/C' }, { type: 'null' }]\n",
        ),
    ];
    let aliases = [
        ("root", "    C:\n      $ref: '#/components/schemas/B'\n"),
        ("sub-file", "    C:\n      $ref: './lib.yaml#/C'\n"),
    ];
    let lib = "C:\n  $ref: './openapi.yaml#/components/schemas/B'\n";
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for (body, a) in bodies {
        for (alias, c) in aliases {
            for order in orders {
                let entries = [a, b, c];
                let schemas: String = order.iter().map(|&i| entries[i]).collect();
                let spec = format!(
                    "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n\
                     components:\n  schemas:\n{schemas}"
                );
                let (report, _, code) =
                    generate_and_check_files(&[("openapi.yaml", &spec), ("lib.yaml", lib)]);
                let case = format!("{body} {alias} {order:?}");
                assert!(
                    report.outcome().is_success()
                        && report
                            .diagnostics()
                            .iter()
                            .all(|d| d.severity != spargen::Severity::Error),
                    "{case}: every order must generate without an error: {report:#?}"
                );
                assert_eq!(
                    body == "oneOf",
                    report.diagnostics().iter().any(|d| {
                        d.code == Code::ValidationKeywordIgnored
                            && d.pointer.as_str() == "/components/schemas/A"
                            && d.message
                                .contains("members 0, 1 lower to the same generated type")
                    }),
                    "{case}: the repeated member is `W001` at `A` exactly where `A` repeats it: \
                     {report:#?}"
                );
                let types = types_module(&code);
                assert!(
                    types.contains("pub struct B"),
                    "{case}: `B` must be generated: {types}"
                );
            }
        }
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

/// A discriminated union one of whose members reaches back to it (#1400): `A` holds a `U`, so
/// whichever of the two is declared first, one of them is still an open reservation while the
/// other is lowered. With `A` first, `U` was lowered while `A` was a reservation, which has no
/// JSON category, so the discriminator was dropped in silence and `U` was trial-matched: `{"kind":
/// "B"}` matched both `A` and `B` and failed to decode, though its tag names `B`. Tag dispatch is
/// decided once every reservation is filled, so the declaration order decides nothing: both
/// orders dispatch by tag and generate the same `U`.
#[test]
fn a_recursive_discriminated_member_keeps_tag_dispatch_in_either_declaration_order() {
    let a = "    A: { type: object, required: [kind], properties: { kind: { type: string }, \
             next: { $ref: '#/components/schemas/U' } } }\n";
    let b = "    B: { type: object, required: [kind], properties: { kind: { type: string } } }\n";
    let u = "    U: { oneOf: [{ $ref: '#/components/schemas/A' }, { $ref: \
             '#/components/schemas/B' }], discriminator: { propertyName: kind } }\n";
    let spec = |schemas: [&str; 3]| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  \
             /u:\n    \
             get:\n      \
             operationId: getU\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json: {{ schema: {{ $ref: '#/components/schemas/U' }} }}\n\
             components:\n  \
             schemas:\n{}",
            schemas.concat()
        )
    };
    let mut bodies = Vec::new();
    for (order, schemas) in [("U first", [u, a, b]), ("A first", [a, b, u])] {
        let (report, code) = generate_with_code(&spec(schemas));
        assert_ne!(report.outcome(), Outcome::Rejected, "{order}: {report:#?}");
        assert_eq!(codes(&report), Vec::<&str>::new(), "{order}: {report:#?}");
        let arms = super::discriminator::discriminated_arms(&code, "U");
        assert_eq!(
            arms.decode,
            vec![
                ("A".to_owned(), vec!["A".to_owned()]),
                ("B".to_owned(), vec!["B".to_owned()]),
            ],
            "{order}: {code}"
        );
        assert_eq!(
            arms.encode,
            vec![
                ("A".to_owned(), Some("A".to_owned())),
                ("B".to_owned(), Some("B".to_owned())),
            ],
            "{order}: {code}"
        );
        // `U`'s own items, its enum and both impls; where the cycle is boxed, and the order the
        // items are emitted in, follow the declaration order whatever the dispatch.
        let start = code
            .find("pub enum U {")
            .unwrap_or_else(|| panic!("{order}: no `U`: {code}"));
        let end = code[start..]
            .find("impl serde::Serialize for U {")
            .and_then(|at| {
                code[start + at..]
                    .find("\n    }\n")
                    .map(|end| start + at + end)
            })
            .unwrap_or_else(|| panic!("{order}: no `Serialize` for `U`: {code}"));
        bodies.push(code[start..end].to_owned());
    }
    assert_eq!(bodies[0], bodies[1], "the declaration order changed `U`");
}

/// A `$ref` with siblings met with a deferred discriminated union (#1400): `C` refines `U`, whose
/// member `A` holds a `U`, so the meet copies `U`'s variants while `U`'s dispatch still waits for
/// `A`'s reservation to fill. The copy takes the decision `U` takes, so whichever of `A` and `U`
/// is declared first, `C` dispatches by tag too: `{"kind": "B", "extra": "x"}` matched both
/// variants of an all-untagged copy and failed to decode, though its tag names `B`.
#[test]
fn a_meet_copy_of_a_deferred_discriminated_union_keeps_tag_dispatch_in_either_order() {
    let a = "    A: { type: object, required: [kind], properties: { kind: { type: string }, \
             next: { $ref: '#/components/schemas/U' } } }\n";
    let b = "    B: { type: object, required: [kind], properties: { kind: { type: string } } }\n";
    let u = "    U: { oneOf: [{ $ref: '#/components/schemas/A' }, { $ref: \
             '#/components/schemas/B' }], discriminator: { propertyName: kind } }\n";
    let c = "    C: { $ref: '#/components/schemas/U', required: [extra], properties: { extra: { \
             type: string } } }\n";
    let spec = |schemas: [&str; 4]| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  \
             /u:\n    \
             get:\n      \
             operationId: getU\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json: {{ schema: {{ $ref: '#/components/schemas/U' }} }}\n  \
             /c:\n    \
             get:\n      \
             operationId: getC\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json: {{ schema: {{ $ref: '#/components/schemas/C' }} }}\n\
             components:\n  \
             schemas:\n{}",
            schemas.concat()
        )
    };
    let mut arms = Vec::new();
    for (order, schemas) in [("U first", [u, a, b, c]), ("A first", [a, b, u, c])] {
        let (report, code) = generate_with_code(&spec(schemas));
        assert_ne!(report.outcome(), Outcome::Rejected, "{order}: {report:#?}");
        let found = super::discriminator::discriminated_arms(&code, "C");
        let tags: Vec<Vec<String>> = found.decode.iter().map(|(_, tags)| tags.clone()).collect();
        assert_eq!(
            tags,
            vec![vec!["A".to_owned()], vec!["B".to_owned()]],
            "{order}: {code}"
        );
        arms.push((found.decode, found.encode));
    }
    assert_eq!(
        arms[0], arms[1],
        "the declaration order changed `C`'s dispatch"
    );
}

/// #1421 over a deferred discriminated union (#1400), in a 3.2 document: `A` holds a `U`, so with
/// `A` declared first `U`'s dispatch is decided once every reservation is filled, and that is
/// where an optional tag property is checked. `A` leaving `kind` optional, with no
/// `defaultMapping`, is `E007` at the Discriminator Object in either declaration order. `A`
/// requiring it generates in either order, tag-dispatched: the inline member pinning `kind` to
/// `b` and requiring it has that field dropped only after the check, so it is never read as
/// leaving the tag optional.
#[test]
fn a_recursive_member_leaving_the_tag_optional_is_e007_in_either_declaration_order() {
    let a = |required: &str| {
        format!(
            "    A: {{ type: object, {required}properties: {{ kind: {{ type: string }}, \
             next: {{ $ref: '#/components/schemas/U' }} }} }}\n"
        )
    };
    let u =
        "    U: { oneOf: [{ $ref: '#/components/schemas/A' }, { type: object, required: [kind], \
             properties: { kind: { type: string, const: b } } }], discriminator: { propertyName: \
             kind } }\n";
    let spec = |schemas: [&str; 2]| {
        format!(
            "openapi: 3.2.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n\
             components:\n  \
             schemas:\n{}",
            schemas.concat()
        )
    };
    let optional = a("");
    let required = a("required: [kind], ");
    for (order, schemas) in [("U first", [u, &optional]), ("A first", [&optional, u])] {
        let report = generate(&spec(schemas));
        assert_eq!(report.outcome(), Outcome::Rejected, "{order}: {report:#?}");
        let rejection = report
            .diagnostics()
            .iter()
            .find(|d| {
                d.code == Code::NonDisjointUnion
                    && d.pointer.as_str() == "/components/schemas/U/discriminator"
            })
            .unwrap_or_else(|| panic!("{order}: {report:#?}"));
        assert!(
            rejection
                .message
                .contains("the discriminator property `kind` is optional in union member 0 (`A`)"),
            "{order}: {rejection:#?}"
        );
        assert!(
            !rejection.message.contains("union member 1"),
            "{order}: {rejection:#?}"
        );
    }
    for (order, schemas) in [("U first", [u, &required]), ("A first", [&required, u])] {
        let (report, code) = generate_with_code(&spec(schemas));
        assert_ne!(report.outcome(), Outcome::Rejected, "{order}: {report:#?}");
        assert!(
            !has_code(&report, Code::NonDisjointUnion),
            "{order}: {report:#?}"
        );
        let tags: Vec<Vec<String>> = super::discriminator::discriminated_arms(&code, "U")
            .decode
            .into_iter()
            .map(|(_, tags)| tags)
            .collect();
        assert_eq!(
            tags,
            vec![vec!["A".to_owned()], vec!["b".to_owned()]],
            "{order}: {code}"
        );
    }
}
