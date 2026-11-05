//! Recursion: references that close a cycle back to the schema enclosing them. Beside a sibling that
//! bears a shape, as a union member, an `allOf` property or a meet, such a reference cannot be
//! intersected and is `E013` rather than discarded; met with an untyped sibling it is kept as
//! written; an array or tuple closing an alias cycle is a newtype; and the cycle predicate counts
//! only the edges lowering follows.

use super::*;

/// A recursive schema whose back-edge `$ref` carries **shape siblings**.
///
/// In JSON Schema 2020-12 `$ref` is an applicator, not a replacement, so the reference and its
/// siblings intersect — `lower_schema_inner` says exactly that four lines above the site. When the
/// reference is a cycle-closing back-edge, the `Ty` it returns points at a *reservation* whose body
/// has not been lowered yet. Intersecting against it read the reservation's placeholder kind, and an
/// intersection with an untyped value is the sibling alone, so **the `$ref` applicator was silently
/// discarded**: `Node`'s own `label` and `child` vanish from the child's type, and the matching
/// subtree of a conforming payload deserialises into nothing.
///
/// The fields are not merely absent from the Rust type — there is no diagnostic, which is the
/// standing invariant verbatim. `is_in_progress_root`'s own documentation states the rule this site
/// broke: the only safe thing to do with a reservation is refuse to read it.
///
/// All three spellings are pinned because all three reach it. The root-document form is **not** a
/// control here: it reproduces byte-identically on `2aa5ada`, so this is a pre-existing defect that
/// the resolved-reference memo widened the reach of rather than one the memo introduced.
#[test]
fn a_recursive_ref_with_shape_siblings_is_rejected_rather_than_silently_dropped() {
    const LIB: &str = r##"
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: { type: string }
        child:
          $ref: 'PREFIX#/components/schemas/Node'
          type: object
          properties:
            extra: { type: string }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Node",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: the `$ref` applicator cannot be intersected against a schema \
                 whose fields are not yet known, and dropping it silently is the degradation the \
                 taxonomy forbids: {report:#?}"
            );
            assert!(
                has_code(report, Code::AllOfIrreconcilable),
                "{spelling}/{entry}: {report:#?}"
            );
            // The code being right is not the same as the message being true. Both spellings name
            // one document, so both must name the same cause: one verdict and one wording for every
            // spelling of a reference. The generic empty-intersection message would be FALSE here:
            // the intersection is inhabited and representable. Asserting per spelling sees that.
            let messages = messages_for(report, Code::AllOfIrreconcilable);
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("closes a reference cycle")),
                "{spelling}/{entry}: the rejection must name the recursion as the cause, not the \
                 generic empty-intersection wording: {messages:?}"
            );
        }
        // The observable damage, asserted directly rather than through the verdict: whatever is
        // emitted, no type may carry the sibling's field while having silently lost the
        // reference's.
        assert!(
            !code.contains("pub extra:") || code.contains("pub label:"),
            "{spelling}: the sibling survived and the referenced component's fields did not: \
             {code}"
        );
    }

    // The same shape in the root document. It is pinned for the same reason and not as a control:
    // the fault is not specific to sub-files, and the root spelling must reject the same way.
    let root = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Node' }} }}
{}"##,
        LIB.replace("PREFIX", "")
    );
    let report = generate(&root);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A cycle-closing `$ref` carrying shape siblings, in the explicit `file#pointer` spelling, where
/// **both sides accept `null`**.
///
/// This is the conjunction `a_recursive_ref_with_shape_siblings_is_rejected_rather_than_silently_dropped`
/// already pins, with one thing added: the target and the sibling are both nullable. That addition
/// turns a wrong rejection into wrong *code*. `back_edge` consulted `remote_in_progress` for every
/// spelling that is not `#/components/schemas/…`, and a sub-file target's reservation lives in
/// `resolved_in_progress`, so the explicit spelling was never recognised as a back-edge. The
/// `TypeKind::Reserved` placeholder then reached `intersect_types`, `intersect_non_null` has no
/// `Reserved` arm and returned `None`, and the null-collapse rescue swallowed that `None` into
/// `TypeKind::Null` because both operands accept null — emitting `pub type Treekid = ();` with no
/// diagnostic at all.
///
/// `()` is not a degraded type, it is the *wrong* type: the description accepts
/// `{"kid": {"x": "a"}}` and the generated client rejects it at runtime with
/// `invalid type: map, expected unit`. Only `null` decodes. Output is byte-stable across two
/// generations, so `determinism.rs` cannot see it, and `check` reports clean.
///
/// Both spellings are pinned together because the verdict is a property of the DOCUMENT — decision
/// 23's rule — and these two documents are the same document.
#[test]
fn a_nullable_cycle_closing_ref_with_nullable_siblings_is_rejected_in_every_spelling() {
    const LIB: &str = r##"
components:
  schemas:
    Tree:
      type: [object, 'null']
      properties:
        kid:
          $ref: 'PREFIX#/components/schemas/Tree'
          type: [object, 'null']
          properties:
            extra: { type: string }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Tree",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a reservation cannot be intersected against, and collapsing \
                 the failed intersection to the JSON null type emits a client that decodes only \
                 `null` for a schema that accepts objects: {report:#?}"
            );
            assert!(
                has_code(report, Code::AllOfIrreconcilable),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        // The observable damage, asserted directly rather than through the verdict: `()` accepts
        // exactly one JSON value, and this schema accepts objects.
        assert!(
            !code.contains("= ();"),
            "{spelling}: the failed intersection collapsed to the exact JSON null type, so the \
             generated client decodes only `null`: {code}"
        );
    }
}

/// The same conjunction again, written as a **union member** — the third spelling.
///
/// `member_closes_a_cycle` stripped only `#/components/schemas/` and answered `false` for anything
/// else, so a sub-file or remote member reference was never recognised as a back-edge. The single
/// real member then collapsed through `intersect_types` against the union's own sibling, hit the
/// same `Reserved`/`None`/null-rescue path, and emitted `pub type Treekid = ();`.
///
/// Three spellings of one conjunction must give one verdict. They gave two silent generations and
/// one rejection.
#[test]
fn a_nullable_cycle_closing_union_member_is_rejected_in_every_spelling() {
    const LIB: &str = r##"
components:
  schemas:
    Tree:
      type: [object, 'null']
      properties:
        kid:
          type: [object, 'null']
          properties:
            extra: { type: string }
          oneOf:
            - { $ref: 'PREFIX#/components/schemas/Tree' }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Tree",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a union member that closes a reference cycle cannot be \
                 intersected against the union's own siblings: {report:#?}"
            );
            // The cause, not only the verdict: with the two union guards neutralised, the explicit
            // spelling still rejects — as `E007` with a "sole non-null member" message, the wrong
            // cause — so an outcome assertion alone cannot tell the guards are present. Both
            // spellings must name the cycle, under the code the `$ref`-sibling spelling uses.
            assert!(
                has_code(report, Code::AllOfIrreconcilable),
                "{spelling}/{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::NonDisjointUnion),
                "{spelling}/{entry}: the cycle was reported as an empty union: {report:#?}"
            );
            assert!(
                messages_for(report, Code::AllOfIrreconcilable)
                    .iter()
                    .any(|message| message.contains("closes a reference cycle")),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        assert!(
            !code.contains("= ();"),
            "{spelling}: the failed member intersection collapsed to the exact JSON null type: \
             {code}"
        );
    }
}

/// A cycle-closing `$ref` with shape siblings whose target is a local schema that is **not a
/// component** — `./lib.yaml#/bag/Tree`, in a sub-file with no `components` key at all.
///
/// The guard's local message used to say the reference closed a cycle back to "the component that
/// encloses it", naming a construct this document does not contain.
// E013 case: cycle
#[test]
fn a_cycle_closing_ref_to_a_non_component_schema_names_no_component() {
    const LIB: &str = r##"
bag:
  Tree:
    type: object
    properties:
      kid:
        $ref: './lib.yaml#/bag/Tree'
        type: object
        properties:
          extra: { type: string }
"##;
    let (generated, checked, _) = split("./lib.yaml#/bag/Tree", LIB);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(report, Code::AllOfIrreconcilable);
        assert!(
            messages.iter().any(|message| {
                message.contains("closes a reference cycle back to the schema that encloses it")
                    && !message.contains("component")
                    && !message.contains("remote")
            }),
            "{entry}: {report:#?}"
        );
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
    // recursive union that has none. Each of these documents generates, and the recursive member
    // keeps its type rather than degrading to `serde_json::Value`, the silent degradation #160
    // records for the sole-member collapse.
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
        let (report, code) = generate_with_code(&spec);
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "the union guard crept into `{what}`, which has no sibling to intersect with: \
             {report:#?}"
        );
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
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
