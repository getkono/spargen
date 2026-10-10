//! Recursion: cycles, back edges, nullable aliases, and the reservations a component holds while it
//! is lowered.

use super::*;

/// A nullable alias on a cycle — `B: oneOf: [<A>, null]` with `A.next: {$ref: B}` — is recognised
/// as a back-edge to `A` only when its real member is a bare `$ref`. A member carrying a
/// `oneOf`/`anyOf` beside its `$ref` is an intersection, not another name for `A`: taking it as the
/// alias boxed a plain `A` and dropped the member's union with no diagnostic. Here the union
/// (`anyOf: [{type: integer}]`) even contradicts the object `A`. As an intersection, the member's
/// `$ref` closes the cycle back to `A`, so it is `E013` at the member — in either declaration
/// order, through `generate` and `check` alike, for `oneOf` and `anyOf` beside the `$ref`.
#[test]
fn a_nullable_alias_member_with_a_union_beside_its_ref_is_not_a_cycle_alias() {
    for keyword in ["oneOf", "anyOf"] {
        let a = "    A:\n      type: object\n      properties:\n        next: { $ref: \
                 '#/components/schemas/B' }\n";
        let b = format!(
            "    B:\n      oneOf:\n        - $ref: '#/components/schemas/A'\n          {keyword}: \
             [{{ type: integer }}]\n        - type: 'null'\n"
        );
        for (order, schemas) in [
            ("A first", format!("{a}{b}")),
            ("B first", format!("{b}{a}")),
        ] {
            let spec = format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/A' }} }}
components:
  schemas:
{schemas}"##
            );
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{keyword}, {order}, via {entry}: the member's union must not be dropped by \
                     taking it as an alias: {report:#?}\n{spec}"
                );
                assert!(
                    report.diagnostics().iter().any(|d| {
                        d.code == Code::AllOfIrreconcilable
                            && d.pointer.as_str() == "/components/schemas/B/oneOf/0"
                    }),
                    "{keyword}, {order}, via {entry}: E013 must point at the member: {report:#?}"
                );
            }
        }
    }
}

#[test]
fn schema_component_alias_chains_resolve_and_cycles_reject() {
    let valid = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/A' } }
components:
  schemas:
    A: { $ref: '#/components/schemas/B' }
    B: { $ref: '#/components/schemas/Item' }
    Item:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##;
    let (report, code) = generate_with_code(valid);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub id"), "{code}");

    let cycle = valid.replace(
        "B: { $ref: '#/components/schemas/Item' }",
        "B: { $ref: '#/components/schemas/A' }",
    );
    let report = generate(&cycle);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
}

/// `E004`'s explain text promises that each message is more specific than its list of cases, and
/// that only a reference spargen could not classify reads `unsupported or unresolved`. The
/// Parameter, Request Body and Response alias walkers broke both: a cycle among their components
/// reported ``unresolved parameter reference `cycle` `` — naming a reference nobody wrote, and calling
/// a declared-but-circular target unresolved — and a non-component reference the bundle could not
/// follow read as a plain `unresolved`, indistinguishable from an undeclared component.
#[test]
fn e004_alias_walkers_name_a_cycle_and_an_unclassifiable_reference_as_what_they_are() {
    let valid = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    post:
      parameters:
        - $ref: '#/components/parameters/P'
      requestBody: { $ref: '#/components/requestBodies/B' }
      responses:
        '200': { $ref: '#/components/responses/R' }
components:
  parameters:
    P: { $ref: '#/components/parameters/Q' }
    Q: { name: q, in: query, schema: { type: string } }
  requestBodies:
    B: { $ref: '#/components/requestBodies/C' }
    C: { content: { application/json: { schema: { type: string } } } }
  responses:
    R: { $ref: '#/components/responses/S' }
    S: { description: ok }
"##;
    let report = generate(valid);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");

    let cycles = valid
        .replace(
            "Q: { name: q, in: query, schema: { type: string } }",
            "Q: { $ref: '#/components/parameters/P' }",
        )
        .replace(
            "C: { content: { application/json: { schema: { type: string } } } }",
            "C: { $ref: '#/components/requestBodies/B' }",
        )
        .replace(
            "S: { description: ok }",
            "S: { $ref: '#/components/responses/R' }",
        );
    for (entry, report) in [("generate", generate(&cycles)), ("check", check(&cycles))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(&report, Code::UnresolvedRef);
        for kind in ["parameter", "request body", "response"] {
            assert!(
                messages
                    .iter()
                    .any(|m| *m == format!("{kind} reference cycle cannot be resolved")),
                "{entry}: a {kind} alias cycle must say it is a cycle: {messages:#?}"
            );
        }
        assert!(
            messages.iter().all(|m| !m.contains("`cycle`")),
            "{entry}: no message may name a reference `cycle`: {messages:#?}"
        );
    }

    // A reference outside `#/components/<kind>/` whose JSON Pointer names nothing: the resolver
    // knows the target is absent, so the message says so in the absent-target wording rather
    // than the one the explain text reserves for a reference it cannot place.
    let at = |parameter: &str, body: &str, response: &str| {
        valid
            .replace(
                "- $ref: '#/components/parameters/P'",
                &format!("- $ref: '{parameter}'"),
            )
            .replace(
                "requestBody: { $ref: '#/components/requestBodies/B' }",
                &format!("requestBody: {{ $ref: '{body}' }}"),
            )
            .replace(
                "'200': { $ref: '#/components/responses/R' }",
                &format!("'200': {{ $ref: '{response}' }}"),
            )
    };
    let absent = at(
        "#/nowhere/parameter",
        "#/nowhere/body",
        "#/nowhere/response",
    );
    for (entry, report) in [("generate", generate(&absent)), ("check", check(&absent))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(&report, Code::UnresolvedRef);
        for (kind, target) in [
            ("parameter", "#/nowhere/parameter"),
            ("request body", "#/nowhere/body"),
            ("response", "#/nowhere/response"),
        ] {
            assert!(
                messages.iter().any(|m| *m
                    == format!(
                        "{kind} reference target `{target}` was not found in the input bundle"
                    )),
                "{entry}: {kind}: {messages:#?}"
            );
        }
        assert!(
            messages
                .iter()
                .all(|m| !m.contains("unsupported or unresolved")),
            "{entry}: an absent target is not an unclassifiable reference: {messages:#?}"
        );
    }

    // A named-anchor fragment the bundle cannot place at a JSON Pointer: here, and only here,
    // the resolver cannot tell an absent target from a form it declines to walk.
    let unclassifiable = at("#parameter", "#body", "#response");
    let report = generate(&unclassifiable);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::UnresolvedRef);
    for (kind, target) in [
        ("parameter", "#parameter"),
        ("request body", "#body"),
        ("response", "#response"),
    ] {
        assert!(
            messages
                .iter()
                .any(|m| *m == format!("unsupported or unresolved {kind} reference `{target}`")),
            "{kind}: {messages:#?}"
        );
    }

    // A target that exists but is not an object is the parser's to reject, at the target
    // (`E011`); the alias walker adds no `E004` on top of it.
    let unparsable = at("#/info/title", "#/info/title", "#/info/title");
    let report = generate(&unparsable);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    assert!(
        messages_for(&report, Code::UnresolvedRef).is_empty(),
        "{report:#?}"
    );

    // Control: an undeclared component keeps the plain `unresolved` wording.
    let undeclared = valid.replace(
        "- $ref: '#/components/parameters/P'",
        "- $ref: '#/components/parameters/Missing'",
    );
    let report = generate(&undeclared);
    assert!(
        messages_for(&report, Code::UnresolvedRef)
            .contains(&"unresolved parameter reference `#/components/parameters/Missing`"),
        "{report:#?}"
    );
}

/// The message a mixed recursive `allOf` carries. Reading the reservation's placeholder made the
/// recursive member look scalar, so a composition of **two object members** was reported as one that
/// "mixes object and scalar members" — naming a member class the document does not contain and
/// sending the reader to remove something that is not there. The root document reports the true
/// fault, and the sub-file spelling must say the same thing.
#[test]
fn a_recursive_all_of_member_beside_an_object_is_not_reported_as_a_scalar_mix() {
    let lib = r##"
components:
  schemas:
    Tree:
      type: object
      properties:
        label: { type: string }
        child:
          allOf:
            - { $ref: '#/components/schemas/Tree' }
            - type: object
              properties: { extra: { type: string } }
"##;
    let (generated, checked, _) = split("./lib.yaml#/components/schemas/Tree", lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let all_of: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AllOfIrreconcilable)
            .collect();
        assert!(
            all_of
                .iter()
                .any(|d| d.message.contains("direct recursive")),
            "{entry}: {report:#?}"
        );
        assert!(
            !all_of
                .iter()
                .any(|d| d.message.contains("mixes object and scalar")),
            "{entry}: both members are objects; naming a scalar member sends the reader to remove \
             something the document does not contain: {report:#?}"
        );
    }
}

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
            // one document, so both must name the same cause — decision 23's rule applied to the
            // wording rather than only to the verdict. The explicit spelling reported the generic
            // empty-intersection message, which is FALSE here: the intersection is inhabited and
            // representable, and the reader who goes looking for a contradiction will not find one.
            // Asserting only the code cannot see that; asserting per spelling can.
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

/// A reservation intersected with **itself** is not unanswerable: `X ∩ X = X`, which is how every
/// ordinary recursive schema composes when two `allOf` members repeat one construct.
///
/// `intersect_types` refuses to read a `TypeKind::Reserved` operand, and that refusal ran before
/// `intersect_non_null`'s identity short-circuit, so two members naming the same recursive target
/// failed to intersect. Most callers turn that `None` into a false `E013` ("conflicting types" for
/// two operands that are the same type); the array-item and optional-property callers turn it into
/// an uninhabited type, which is worse — a `kids` array whose item type is an empty enum decodes
/// only `[]`, the one array the `minItems: 1` member forbids, with no diagnostic at all.
#[test]
fn a_recursive_target_repeated_across_all_of_members_intersects_as_itself() {
    let spec = |members: &str| {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  \
             /t:\n    \
             get:\n      \
             operationId: getT\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Tree' }}\n\
             components:\n  \
             schemas:\n    \
             Tree:\n      \
             allOf:\n{members}"
        )
    };
    let array_items = spec(
        "        - { type: object, properties: { kids: { type: array, items: { $ref: '#/components/schemas/Tree' } } } }\n\
         \x20       - { type: object, properties: { kids: { type: array, minItems: 1, items: { $ref: '#/components/schemas/Tree' } } } }\n",
    );
    let property = spec(
        "        - { type: object, properties: { kid: { $ref: '#/components/schemas/Tree' } } }\n\
         \x20       - { type: object, properties: { kid: { $ref: '#/components/schemas/Tree' } } }\n",
    );
    let additional = spec(
        "        - { type: object, additionalProperties: { $ref: '#/components/schemas/Tree' } }\n\
         \x20       - { type: object, additionalProperties: { $ref: '#/components/schemas/Tree' } }\n",
    );
    let prefix_items = spec(
        "        - { type: array, prefixItems: [{ $ref: '#/components/schemas/Tree' }], items: false }\n\
         \x20       - { type: array, prefixItems: [{ $ref: '#/components/schemas/Tree' }], items: false }\n",
    );

    for (label, document) in [
        ("array items", &array_items),
        ("property", &property),
        ("additionalProperties", &additional),
        ("prefixItems", &prefix_items),
    ] {
        for (entry, report) in [("generate", generate(document)), ("check", check(document))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{label}/{entry}: two members naming the same recursive target were reported as \
                 conflicting: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{label}/{entry}: {report:#?}"
            );
        }
    }

    // The silent half, asserted on what was emitted: the item type is `Tree`, not an empty enum.
    let (_, code) = generate_with_code(&array_items);
    let types = types_module(&code);
    assert!(
        types.contains("= Vec<Tree>;"),
        "the repeated recursive item type must stay `Tree`: {types}"
    );
    assert!(
        !types.lines().any(|line| {
            let line = line.trim();
            line.starts_with("pub enum ") && line.ends_with("{}")
        }),
        "an uninhabited item type was emitted for an item both members type as `Tree`: {types}"
    );
    let (_, code) = generate_with_code(&property);
    assert_eq!(
        field_type(&types_module(&code), "pub kid:").as_deref(),
        Some("Option<Box<Tree>>"),
        "{code}"
    );
}

/// A two-schema cycle with shape siblings on ONE edge only, entered from each end.
///
/// `A.b` carries siblings beside its `$ref` to `B`; `B.a` is a plain `$ref` back to `A`. Entered at
/// `B`, lowering reaches `A.b` while `B` is still open, so the conjunction meets a placeholder.
/// Entered at `A`, `B` is lowered in full before the conjunction is reached. A back-edge test that
/// asks which reservations are open therefore answered differently for the two entry points of one
/// unchanged `lib.yaml` — `E013` from `B`, a clean generation from `A` — while the root-component
/// spelling of the same schemas, asked of the document, rejected from both. The verdict is a
/// property of the document, so all three spellings, from both ends, must give the same one.
///
/// The edge is written twice: as a `$ref` beside shape siblings, and as the sole `oneOf` member of a
/// schema carrying them. The union spelling had the same fault in the explicit `./lib.yaml#/…`
/// form, whose document-half guard was asked only of `#/components/schemas/…`.
#[test]
fn a_one_edge_cycle_rejects_whichever_end_lowering_enters() {
    const LIB: &str = r##"
components:
  schemas:
    A:
      type: object
      properties:
        b:
EDGE
    B:
      type: object
      properties:
        a: { $ref: 'PREFIX#/components/schemas/A' }
"##;
    const SIBLING: &str = "          $ref: 'PREFIX#/components/schemas/B'
          type: object
          properties:
            extra: { type: string }";
    const UNION: &str = "          type: object
          properties:
            extra: { type: string }
          oneOf:
            - $ref: 'PREFIX#/components/schemas/B'";

    for (shape, edge) in [("sibling", SIBLING), ("union", UNION)] {
        let lib = LIB.replace("EDGE", edge);
        let mut diagnoses = Vec::new();
        for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
            for entry in ["A", "B"] {
                let (generated, checked, _) = split(
                    &format!("./lib.yaml#/components/schemas/{entry}"),
                    &lib.replace("PREFIX", prefix),
                );
                for (run, report) in [("generate", &generated), ("check", &checked)] {
                    assert_eq!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{shape}/{spelling}/{entry}/{run}: {report:#?}"
                    );
                    let messages = messages_for(report, Code::AllOfIrreconcilable);
                    assert!(
                        messages
                            .iter()
                            .any(|m| m.contains("closes a reference cycle")),
                        "{shape}/{spelling}/{entry}/{run}: the rejection must name the \
                         recursion: {messages:?}"
                    );
                }
                diagnoses.push((
                    format!("{spelling}/{entry}"),
                    messages_for(&generated, Code::AllOfIrreconcilable)
                        .iter()
                        .map(|m| (*m).to_owned())
                        .collect::<Vec<_>>(),
                ));
            }
        }
        assert!(
            diagnoses.iter().all(|(_, d)| *d == diagnoses[0].1),
            "{shape}: the entry point or the spelling changed the diagnosis: {diagnoses:?}"
        );
        one_edge_root_control(shape, &lib);
    }
}

/// The root-component spelling of [`a_one_edge_cycle_rejects_whichever_end_lowering_enters`]'s two
/// schemas, entered from each end: the control the sub-file spellings are held to.
fn one_edge_root_control(shape: &str, lib: &str) {
    for entry in ["A", "B"] {
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/{entry}' }} }}
{}"##,
            lib.replace("PREFIX", "")
        );
        let report = generate(&root);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{shape}/root/{entry}: {report:#?}"
        );
        assert!(
            messages_for(&report, Code::AllOfIrreconcilable)
                .iter()
                .any(|m| m.contains("closes a reference cycle")),
            "{shape}/root/{entry}: {report:#?}"
        );
    }
}

/// A **root-only** document — no sub-files, no remote refs — whose `allOf` member reaches the
/// component being lowered through a component **alias**.
///
/// This is the shape the breaking-change footer's scope statement missed. `gather_member`'s
/// pre-existing guard keys on the member's own *name*: `Alias` is not in `in_progress`, so it never
/// fired. `ensure_component("Alias")` then chains to `Node`, which **is** in progress, and hands
/// back a back-edge against `Node`'s reservation, whose placeholder `push_ref_member` read as a
/// scalar.
///
/// So this document was `clean` at `2aa5ada` (verified by building that commit and running it) and
/// is rejected now. The rejection is right — `2aa5ada` emitted `serde_json::Value` for a typed
/// schema with no diagnostic — but "regenerating from an unchanged description is otherwise
/// unaffected" was not, and this is a description that uses none of the multi-file machinery.
#[test]
fn a_recursive_all_of_member_reached_through_a_root_alias_is_rejected() {
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
            application/json: { schema: { $ref: '#/components/schemas/Node' } }
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: { type: string }
        child:
          allOf:
            - { $ref: '#/components/schemas/Alias' }
    Alias:
      $ref: '#/components/schemas/Node'
"##;
    let (generated, code) = generate_with_code(spec);
    let checked = check(spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::AllOfIrreconcilable
                    && d.message.contains("direct recursive")),
            "{entry}: {report:#?}"
        );
        // Not an alias *cycle*: `Alias` is entered once. Reporting one would blame the alias for a
        // loop it does not form and send the reader to break a chain of length one.
        assert!(
            !report
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("alias cycle")
                    || d.message.contains("forms a reference cycle")),
            "{entry}: {report:#?}"
        );
    }
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

/// The archetypal recursive schema: a tree whose `child` is a `oneOf` of itself and something else.
///
/// This is the most common recursive construct in real descriptions, `docs/support-matrix.md` lists
/// recursive `$ref` cycles as supported, and it generates on `2aa5ada`. Round 6's union guard
/// rejected it, because it asked `is_in_progress_root(ty.id)` — "is the member **any** open
/// reservation" — when the question it needed was "is the member **this union's own** reservation".
/// Those coincide only when the union *is* the component's whole body, which is D8's shape and not
/// this one: here the union is `Nodechild` and the member is `Node`, a different type, so the
/// generated decoder calls into another impl and terminates on any finite document.
///
/// The rejection's message was also false about the document — it said the member was the union
/// itself when the two are different types — and the fault had two properties worth stating: it
/// depended on the order `components.schemas` keys were written in, because root components are
/// pre-lowered in key order, and a description that generated in one file stopped generating when
/// split, because sub-file components are never pre-lowered.
///
/// Every row below was measured against a build of `2aa5ada`. The D8 rows are the ones that must
/// still reject; everything else must still generate.
#[test]
fn a_union_member_that_is_a_different_recursive_type_still_generates() {
    // The union sits in a property, so it is not the component's own reservation.
    let archetype = |applicator: &str| {
        format!(
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
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: {{ type: string }}
        child:
          {applicator}:
            - {{ $ref: '#/components/schemas/Node' }}
            - {{ type: string }}
"##
        )
    };

    for applicator in ["oneOf", "anyOf"] {
        let spec = archetype(applicator);
        let (generated, code) = generate_with_code(&spec);
        let checked = check(&spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{applicator}/{entry}: the member is `Node` and the union is `Nodechild` — two \
                 different types, so the decoder terminates: {report:#?}"
            );
            assert!(
                !has_code(report, Code::NonDisjointUnion),
                "{applicator}/{entry}: {report:#?}"
            );
        }
        // The recursion is closed by boxing, as the matrix promises, rather than refused.
        assert!(code.contains("Box<Node>"), "{applicator}: {code}");
    }

    // Nested one level deeper — the union inside an array's items — and mutual recursion in both
    // key orders, because the guard's fault was sensitive to pre-lowering order.
    let nested = r##"
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
        kids:
          type: array
          items:
            oneOf:
              - { $ref: '#/components/schemas/Node' }
              - { type: string }
"##;
    assert_ne!(generate(nested).outcome(), Outcome::Rejected, "{nested}");

    let mutual = |first: &str, second: &str| {
        format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/{first}' }} }}
components:
  schemas:
    {first}:
      type: object
      required: [one]
      properties:
        one: {{ type: string }}
        via: {{ oneOf: [{{ $ref: '#/components/schemas/{second}' }}, {{ type: string }}] }}
    {second}:
      type: object
      required: [two]
      properties:
        two: {{ type: string }}
        via: {{ oneOf: [{{ $ref: '#/components/schemas/{first}' }}, {{ type: string }}] }}
"##
        )
    };
    // Both key orders: the guard's fault made acceptance depend on which component was pre-lowered
    // first, so one order passed and the other did not.
    for (first, second) in [("A", "B"), ("B", "A")] {
        let spec = mutual(first, second);
        assert_ne!(
            generate(&spec).outcome(),
            Outcome::Rejected,
            "{first} before {second}: {spec}"
        );
    }

    // And the same archetype split across files, which never pre-lowers its components at all.
    let (generated, checked, _) = split(
        "./lib.yaml#/components/schemas/Node",
        r##"
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: { type: string }
        child:
          oneOf:
            - { $ref: '#/components/schemas/Node' }
            - { type: string }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "split/{entry}: {report:#?}"
        );
    }
}

/// The canonical OpenAPI 3.1 spelling of a nullable recursive reference: `oneOf`/`anyOf` over the
/// enclosing component and `{type: "null"}`.
///
/// 3.1 removed `nullable: true`, so this *is* how a description says "optionally another `Node`".
/// `lower_union`'s single-real-member collapse re-emits the member's kind as this position's own
/// def by cloning `graph.get(inner.id).kind`. For a cycle-closing `$ref` that kind is
/// `TypeKind::Reserved`, so the clone inserted a **second** reservation that nothing would ever
/// `fill`; `Api::check_invariants` then rejected the whole document with `E011`, whose shipped
/// explain text asserts the input is malformed JSON — which this document is not.
///
/// The guard written for exactly this class sits inside the multi-member loop, which the
/// `real_members.len() == 1` early return jumps straight over.
///
/// Restoring `2aa5ada`'s behaviour is **not** the fix. At `2aa5ada` a reservation's kind was
/// `TypeKind::Any`, so the clone emitted `Option<serde_json::Value>` — the silent degradation of a
/// typed schema the standing invariants forbid outright. The answer `docs/support-matrix.md` and
/// the direct `{$ref: Node}` spelling both already promise is `Option<Box<Node>>`, so that is what
/// is asserted here: "did not reject" would pass on either wrong answer.
#[test]
fn a_nullable_recursive_ref_collapses_to_an_optional_box() {
    let archetype = |applicator: &str| {
        format!(
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
components:
  schemas:
    Node:
      type: object
      required: [label]
      properties:
        label: {{ type: string }}
        parent:
          {applicator}:
            - {{ $ref: '#/components/schemas/Node' }}
            - {{ type: "null" }}
"##
        )
    };

    for applicator in ["oneOf", "anyOf"] {
        let spec = archetype(applicator);
        let (generated, code) = generate_with_code(&spec);
        let checked = check(&spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{applicator}/{entry}: a nullable recursive `$ref` is the matrix's own example of a \
                 supported construct: {report:#?}"
            );
            // The internal-invariant code must not be how a user learns about this document.
            assert!(
                !has_code(report, Code::InvalidInput),
                "{applicator}/{entry}: {report:#?}"
            );
        }
        // Boxed, because the cycle needs a finite size; `Option`, because the `null` member is what
        // the union collapsed away. Both halves are the matrix's promise.
        assert!(code.contains("Option<Box<Node>>"), "{applicator}: {code}");
        // And never `2aa5ada`'s answer.
        assert!(!code.contains("serde_json::Value>"), "{applicator}: {code}");
    }
}

/// The same idiom under **mutual** recursion, which is the commoner spelling in real descriptions:
/// `A.b` references `B`, and `B` is nothing but "an `A`, or null".
///
/// Here the union *is* `B`'s whole body, so the collapse has no def of its own to insert as the
/// component root — cloning the reservation's kind inserts a second reservation (`E011`), and
/// returning the reservation itself breaks the last-insert invariant `ensure_component` asserts.
/// `B` is a nullable **alias**, and naming it as one is what lets the document generate.
///
/// Both key orders are driven, because root components are pre-lowered in map order and a
/// re-ordered YAML map is a no-op in OpenAPI: only one of the two orders reaches the collapse at
/// all, and the other already generated, so a fixture on one order proves nothing about the idiom.
#[test]
fn a_nullable_alias_under_mutual_recursion_generates() {
    let mutual = |first: &str, second: &str| {
        let bodies = |name: &str| {
            if name == "A" {
                "      type: object\n      required: [b]\n      properties:\n        b: { $ref: '#/components/schemas/B' }\n"
            } else {
                "      oneOf:\n        - { $ref: '#/components/schemas/A' }\n        - { type: \"null\" }\n"
            }
        };
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
             application/json: {{ schema: {{ $ref: '#/components/schemas/A' }} }}\n\
             components:\n  \
             schemas:\n    \
             {first}:\n{}    {second}:\n{}",
            bodies(first),
            bodies(second)
        )
    };

    for (first, second) in [("A", "B"), ("B", "A")] {
        let spec = mutual(first, second);
        let (generated, code) = generate_with_code(&spec);
        let checked = check(&spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{first} before {second}/{entry}: {report:#?}\n{spec}"
            );
            assert!(
                !has_code(report, Code::InvalidInput),
                "{first} before {second}/{entry}: {report:#?}"
            );
        }
        // The recursion is closed by boxing whichever way round the map is written — read off
        // `b`'s own declaration, not searched for in the file. `code.contains("Box<")` is true of
        // *any* successful generation (the embedded runtime alone supplies ten occurrences:
        // `Pin<Box<..>>` in `transport.rs`/`auth.rs`/`retry.rs`/`wasm.rs` and
        // `source: Option<Box<dyn Error + Send + Sync>>` in `error.rs`), so the assertion that
        // stood here pinned nothing: under the mutation that unboxes the alias back-edge it stayed
        // green while five of its neighbours went red.
        //
        // The two orders emit different types because they take different paths, and this is the
        // only fixture that drives both. Declaring the target first lowers `A` first, so `B`'s body
        // meets an open `A`, is recognised as a nullable alias, and `b` binds `A` itself — optional
        // because of the `"null"` member, boxed because the cycle must have a finite size.
        //
        // Declaring the alias first lowers `B` first, so `A` is not open when `B`'s body is read,
        // the alias recogniser does not fire, and `b` binds `B` as an ordinary in-progress
        // back-edge. That back-edge is taken before `B`'s body has an answer about null, so it
        // reads the reserve-time guess — `schema_is_nullable`, which never looks at a union's
        // members — and `B`'s `"null"` member made `B` nullable only after `A.b` had been typed
        // `Box<B>`, a field that could not decode the legal `{"b": null}` (issue #222). The
        // back-edge now carries `B`'s lowered nullability, so it is `Option<Box<B>>`: optional
        // because `B` is, boxed because the cycle must have a finite size.
        // One message per order: the two orders take different paths, and a single format string
        // over both hands a reader of the first iteration a paragraph written about the second,
        // 3,000 output lines above the `left`/`right` that would correct it.
        let (expected, why) = if first == "A" {
            (
                "Option<Box<A>>",
                "this order takes the nullable-alias path, so `b` binds `A` itself — optional \
                 because of the `\"null\"` member, boxed because the cycle must have a finite \
                 size: a red here is a regression in the alias path",
            )
        } else {
            (
                "Option<Box<B>>",
                "this order binds `b` as an ordinary back-edge into `B` while `B` is still \
                 open, and `B`'s `\"null\"` member makes it nullable: `Box<B>` here is issue \
                 #222 back, a back-edge carrying the reserve-time nullability guess instead of \
                 the body's lowered answer",
            )
        };
        assert_eq!(
            field_type(&code, "pub b").as_deref(),
            Some(expected),
            "{first} before {second}: {why}; the comment above this assertion says why: {code}"
        );
        assert!(
            !code.contains("serde_json::Value>"),
            "{first} before {second}: {code}"
        );
    }
}

/// A back-edge into a component whose nullability only its lowered body knows carries that
/// nullability, not the reserve-time guess — in the root document, a sub-file, and a vendored remote.
///
/// `N` is "a node with a required `next` that is another `N`, or null". Its `"null"` lives in a
/// `oneOf` member, which `schema_is_nullable` never reads, so while `N`'s body is open the only
/// answer there is says non-nullable, and `next` — the one reference taken while it is open — was
/// typed `Box<N>`: `{"next": null}`, which ends every such list, did not decode (issue #222). `W.n`
/// is the control, taken after `N` finished: it was already `Option<N>`, so one schema meant two
/// things depending on when it was referenced. Both must be optional now.
#[test]
fn a_back_edge_into_a_union_nullable_component_is_optional() {
    const COMPONENTS: &str = r##"
components:
  schemas:
    W:
      type: object
      required: [n]
      properties:
        n: { $ref: '#/components/schemas/N' }
    N:
      oneOf:
        - type: object
          required: [next]
          properties:
            next: { $ref: '#/components/schemas/N' }
        - { type: 'null' }
"##;
    let document = |components: &str| {
        format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/W' }} }}
{components}"##
        )
    };
    let root = document(COMPONENTS);
    let (generated, root_code) = generate_with_code(&root);
    let checked = check(&root);
    let (split_generated, split_checked, split_code) =
        split("./lib.yaml#/components/schemas/W", COMPONENTS);
    for (entry, report) in [
        ("root/generate", &generated),
        ("root/check", &checked),
        ("split/generate", &split_generated),
        ("split/check", &split_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for (spelling, code) in [("root", &root_code), ("split", &split_code)] {
        assert_eq!(
            field_type(code, "pub n:").as_deref(),
            Some("Option<N>"),
            "{spelling}: the control, taken after `N` finished: {code}"
        );
        assert_eq!(
            field_type(code, "pub next:").as_deref(),
            Some("Option<Box<N>>"),
            "{spelling}: the back-edge taken while `N` was open must carry `N`'s lowered \
             nullability, as the control does: {code}"
        );
    }

    // The vendored-remote spelling: `N` is a whole remote document whose `next` refers back to
    // that document by URL, so the back-edge is `ensure_remote`'s own. Its type is named for the
    // URL, so the field types are compared with each other rather than with a literal name.
    use sha2::{Digest, Sha256};
    const URL: &str = "https://api.example.com/schemas/node.yaml";
    const VENDORED: &str = "api.example.com/schemas/node.yaml";
    let node = format!(
        "oneOf:\n  - type: object\n    required: [next]\n    properties:\n      next: {{ $ref: '{URL}' }}\n  - {{ type: 'null' }}\n"
    );
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        document(&format!(
            "components:\n  schemas:\n    W:\n      type: object\n      required: [n]\n      properties:\n        n: {{ $ref: '{URL}' }}\n"
        )),
    )
    .unwrap();
    std::fs::write(
        dir.join("spargen.lock"),
        format!(
            "version = 1\n\n[[remote]]\nurl = \"{URL}\"\nsha256 = \"{:x}\"\npath = \"{VENDORED}\"\n",
            Sha256::digest(node.as_bytes())
        ),
    )
    .unwrap();
    let vendored = dir.join(".spargen/vendor").join(VENDORED);
    std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
    std::fs::write(&vendored, &node).unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    assert_ne!(report.outcome(), Outcome::Rejected, "remote: {report:#?}");
    let control = field_type(&code, "pub n:").expect("remote: `W.n` is emitted");
    let target = control
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or_else(|| panic!("remote: the control is optional: {control}\n{code}"));
    assert_eq!(
        field_type(&code, "pub next:"),
        Some(format!("Option<Box<{target}>>")),
        "remote: the back-edge taken while the remote target was open must carry its lowered \
         nullability, as the control does: {code}"
    );
}

/// A nullable-alias back-edge carries its target's lowered nullability, not the reserve-time
/// guess — for each spelling `open_reservation_for_ref` keys a reservation by.
///
/// `A` is "an object with a required `b`, or null"; its `"null"` lives in a `oneOf` member, which
/// `schema_is_nullable` never reads. `B` is a one-member `oneOf` over `A`, so when `A.b` lowers
/// `B` while `A` is still open, the alias recogniser answers `b` with `A`'s open reservation
/// instead of taking an ordinary back-edge into `B`. That reservation's nullability is still the
/// guess, so without a recorded read the pass is never revised and `b` is `Box<A>`, which cannot
/// decode the legal `{"b": null}`. `W.a` is the control, taken after `A` finished.
///
/// The spellings reach the four sites that record the read: a root component named by its local
/// pointer, a root component named through the root file's own path (routed back to the
/// component map), a sub-file's own component (the resolved-pointer frame), and a vendored remote
/// document (the remote frame).
#[test]
fn a_nullable_alias_back_edge_carries_its_targets_lowered_nullability() {
    let components = |alias_target: &str| {
        format!(
            r##"
components:
  schemas:
    W:
      type: object
      required: [a]
      properties:
        a: {{ $ref: '#/components/schemas/A' }}
    A:
      oneOf:
        - type: object
          required: [b]
          properties:
            b: {{ $ref: '#/components/schemas/B' }}
        - {{ type: 'null' }}
    B:
      oneOf:
        - {{ $ref: '{alias_target}' }}
"##
        )
    };
    let document = |components: &str| {
        format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/W' }} }}
{components}"##
        )
    };
    let assert_optional = |spelling: &str, code: &str| {
        let control = field_type(code, "pub a:")
            .unwrap_or_else(|| panic!("{spelling}: `W.a` is emitted: {code}"));
        let target = control
            .strip_prefix("Option<")
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or_else(|| panic!("{spelling}: the control is optional: {control}\n{code}"));
        assert_eq!(
            field_type(code, "pub b:"),
            Some(format!("Option<Box<{target}>>")),
            "{spelling}: the alias back-edge taken while `A` was open must carry `A`'s lowered \
             nullability, as the control does: {code}"
        );
    };

    let root = document(&components("#/components/schemas/A"));
    let (generated, code) = generate_with_code(&root);
    let checked = check(&root);
    for (entry, report) in [("root/generate", &generated), ("root/check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_optional("root", &code);

    // The root component `A` named through the root file's own path: not the local
    // `#/components/schemas/` spelling, so it goes through the resolver and is routed back to the
    // component map.
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        document(&components("./openapi.yaml#/components/schemas/A")),
    )
    .unwrap();
    let out = dir.join("client.rs");
    let routed = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let routed_checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for (entry, report) in [
        ("routed/generate", &routed),
        ("routed/check", &routed_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_optional("routed", &std::fs::read_to_string(&out).unwrap_or_default());

    let (split_generated, split_checked, split_code) = split(
        "./lib.yaml#/components/schemas/W",
        &components("#/components/schemas/A"),
    );
    for (entry, report) in [
        ("split/generate", &split_generated),
        ("split/check", &split_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_optional("split", &split_code);

    // The vendored-remote spelling: `A` and `B` in a remote document, whose local references are
    // rewritten absolute, so `B`'s member is the remote reservation's own key. The control `W.a`
    // is the root's, referring to the remote `A` by URL; the remote document's own `W` is unused.
    use sha2::{Digest, Sha256};
    const URL: &str = "https://api.example.com/schemas/lib.yaml";
    const VENDORED: &str = "api.example.com/schemas/lib.yaml";
    let lib = components("#/components/schemas/A");
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        document(&format!(
            "components:\n  schemas:\n    W:\n      type: object\n      required: [a]\n      properties:\n        a: {{ $ref: '{URL}#/components/schemas/A' }}\n"
        )),
    )
    .unwrap();
    std::fs::write(
        dir.join("spargen.lock"),
        format!(
            "version = 1\n\n[[remote]]\nurl = \"{URL}\"\nsha256 = \"{:x}\"\npath = \"{VENDORED}\"\n",
            Sha256::digest(lib.as_bytes())
        ),
    )
    .unwrap();
    let vendored = dir.join(".spargen/vendor").join(VENDORED);
    std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
    std::fs::write(&vendored, &lib).unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "remote: {report:#?}");
    assert_optional("remote", &std::fs::read_to_string(&out).unwrap_or_default());
}

/// The mirror of [`a_back_edge_into_a_union_nullable_component_is_optional`]: the reserve-time
/// guess says nullable and the body says otherwise, and the back-edge follows the body.
///
/// `N`'s type array admits `"null"`, which is all `schema_is_nullable` reads, but its `oneOf` has
/// no null branch, so `null` matches no member and fails the union: `N` is not nullable, and the
/// finished reference `W.n` is a plain `N`. The back-edge `next` read the guess and was
/// `Option<Box<N>>`, which accepts a `null` the schema rejects.
#[test]
fn a_back_edge_into_a_component_whose_body_denies_null_is_not_optional() {
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
            application/json: { schema: { $ref: '#/components/schemas/W' } }
components:
  schemas:
    W:
      type: object
      required: [n]
      properties:
        n: { $ref: '#/components/schemas/N' }
    N:
      type: [object, 'null']
      oneOf:
        - type: object
          required: [next]
          properties:
            next: { $ref: '#/components/schemas/N' }
"##;
    let (generated, code) = generate_with_code(spec);
    let checked = check(spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_eq!(
        field_type(&code, "pub n:").as_deref(),
        Some("N"),
        "the control: {code}"
    );
    assert_eq!(
        field_type(&code, "pub next:").as_deref(),
        Some("Box<N>"),
        "the back-edge must follow the body, not the type array's guess: {code}"
    );
}

/// The `A`/`B` mutual-recursion skeleton of `a_nullable_alias_under_mutual_recursion_generates`,
/// with `B`'s body substituted — the shape the four narrowness fixtures below vary.
///
/// `A` is always declared first, because that is the order that reaches `nullable_alias_back_edge`
/// at all: lowering `A` first leaves it open when `B`'s body is read. Declared the other way round
/// the recogniser never runs, so that order cannot observe a guard inside it and driving it would
/// weaken these fixtures rather than widen them.
fn alias_shaped_mutual_recursion(b_body: &str) -> String {
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
         application/json: {{ schema: {{ $ref: '#/components/schemas/A' }} }}\n\
         components:\n  \
         schemas:\n    \
         A:\n      \
         type: object\n      \
         required: [b]\n      \
         properties:\n        \
         b: {{ $ref: '#/components/schemas/B' }}\n    \
         B:\n{b_body}"
    )
}

/// A component carrying a `discriminator` or a `default` beside its union is **not** a nullable
/// alias, however alias-shaped the union itself looks.
///
/// `nullable_alias_back_edge` answers with the target's own id and inserts no def, so everything
/// the component said apart from the union has nowhere left to go.
/// `schema_has_shape_constraint`, which holds the rest of that narrowness, checks neither `default`
/// nor `discriminator`, so the early return at the head of the function is the only thing refusing
/// these two — and nothing held that early return. Deleting it left
/// `cargo test --workspace --all-features` entirely green while this document, with
/// `discriminator: {propertyName: kind}` on `B`, went from `E007` to a clean `Generated` emitting
/// `Option<Box<A>>`, the discriminator gone and nothing said: the fourth, silent behaviour the
/// standing invariants forbid.
///
/// What is pinned is the verdict as it stands, not an endorsement of it. Both are refused and the
/// refusal is reported; a change that chooses to represent either one has to move this fixture
/// deliberately.
#[test]
fn a_union_carrying_a_discriminator_or_a_default_is_not_an_alias() {
    let cases = [
        (
            "discriminator",
            "      oneOf:\n        - { $ref: '#/components/schemas/A' }\n        \
             - { type: \"null\" }\n      discriminator: { propertyName: kind }\n",
        ),
        (
            "default",
            "      oneOf:\n        - { $ref: '#/components/schemas/A' }\n        \
             - { type: \"null\" }\n      default: null\n",
        ),
    ];
    for (label, body) in cases {
        let spec = alias_shaped_mutual_recursion(body);
        let generated = generate(&spec);
        let checked = check(&spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}/{entry}: a `{label}` beside the union has nowhere to go, so the document \
                 must be refused rather than have it dropped: {report:#?}\n{spec}"
            );
            assert!(
                has_code(report, Code::NonDisjointUnion),
                "{label}/{entry}: {report:#?}"
            );
        }
    }
}

/// A union with a **second** non-null member is a union, not an alias — even when one of its
/// members is the cycle-closing `$ref` that would otherwise make it one.
///
/// The sharpest of the four. The recogniser answers with the *target's* type, so every member
/// beside the one `$ref` is erased. Deleting the `if real.next().is_some()` arity check survives
/// the whole workspace, and on this document it turns `pub b: B` — a two-variant enum — into
/// `pub b: Box<A>`: the `string` branch disappears from the generated API with a clean report and
/// no diagnostic at all. A dropped union member is worse than the `serde_json::Value` degradation
/// the invariants name, because nothing in the output records that the branch ever existed.
#[test]
fn a_union_with_a_second_real_member_beside_the_back_edge_stays_a_union() {
    let spec = alias_shaped_mutual_recursion(
        "      oneOf:\n        - { $ref: '#/components/schemas/A' }\n        - { type: string }\n",
    );
    let (generated, code) = generate_with_code(&spec);
    let checked = check(&spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    // `b` binds `B` itself rather than the target of `B`'s `$ref` member: the union is represented,
    // so it has a type of its own.
    assert_eq!(
        field_type(&code, "pub b").as_deref(),
        Some("B"),
        "the second union member was collapsed away: {code}"
    );
    // And that type is an enum holding both branches. Asserted separately, because binding `B`
    // alone would still be satisfied by a `B` that had quietly become a newtype over `A`.
    let variants = enum_variants(&code, "B");
    assert_eq!(
        variants.len(),
        2,
        "`B` must keep one variant per union member, got {variants:?}: {code}"
    );
    assert!(
        variants.iter().any(|variant| variant.starts_with("A(")),
        "the `$ref` member lost its variant, got {variants:?}: {code}"
    );
}

/// A member spelled `{$ref: A, properties: {...}}` is a `$ref` with shape-bearing siblings — an
/// intersection — and an intersection is not an alias.
///
/// Reading it as one answers with `A` and discards the siblings, which is the silent-sibling
/// discard the `allOf` path refuses under `E013`. Deleting the member-side
/// `schema_has_shape_constraint` guard survives the whole workspace and flips this document from
/// `Rejected`/`E013` to a clean `Generated` with `pub b: Option<Box<A>>` and the declared `x` gone.
#[test]
fn an_alias_member_with_shape_bearing_siblings_is_not_an_alias() {
    let spec = alias_shaped_mutual_recursion(
        "      oneOf:\n        \
         - { $ref: '#/components/schemas/A', properties: { x: { type: string } } }\n        \
         - { type: \"null\" }\n",
    );
    let generated = generate(&spec);
    let checked = check(&spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: the member's sibling `properties` must not be dropped in silence: \
             {report:#?}\n{spec}"
        );
        assert!(
            has_code(report, Code::AllOfIrreconcilable),
            "{entry}: {report:#?}"
        );
    }
}

/// A component that declares a shape of its own beside the union is not another name for its
/// target: an alias carries no shape, and this one does.
///
/// This is the guard that makes that sentence true, and nothing held it. Deleting it survives the
/// whole workspace, and this document — whose `B` declares `type: object` and an `extra` property
/// beside the union — goes from `Rejected`/`E013` to a clean `Generated` with
/// `pub b: Option<Box<A>>`, `extra` absent from the generated API and nothing said about it.
#[test]
fn an_alias_shaped_component_that_declares_its_own_shape_is_not_an_alias() {
    let spec = alias_shaped_mutual_recursion(
        "      type: object\n      properties: { extra: { type: string } }\n      oneOf:\n        \
         - { $ref: '#/components/schemas/A' }\n        - { type: \"null\" }\n",
    );
    let generated = generate(&spec);
    let checked = check(&spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: the component's own `extra` must not be dropped in silence: \
             {report:#?}\n{spec}"
        );
        assert!(
            has_code(report, Code::AllOfIrreconcilable),
            "{entry}: {report:#?}"
        );
    }
}

/// The same alias spelled `anyOf` rather than `oneOf` is still one.
///
/// `nullable_alias_back_edge` picks its members from whichever of the two applicators the
/// component carries, and only the `oneOf` arm of that selection was driven by anything: every
/// alias fixture in this file, including the four narrowness fixtures above, spells the union
/// `oneOf`. Replacing the `anyOf` arm with `return None` left
/// `cargo test --workspace --all-features` entirely green, while this document went from a clean
/// `Generated` emitting `Option<Box<A>>` to `Rejected`/`E007` — one of the two spellings the
/// specification gives the same meaning here stops reaching the recogniser, and nothing said so.
///
/// The four above pin what the recogniser refuses once it runs. This one pins *whether it runs*.
#[test]
fn a_nullable_alias_spelled_any_of_is_recognised_as_one() {
    let spec = alias_shaped_mutual_recursion(
        "      anyOf:\n        - { $ref: '#/components/schemas/A' }\n        \
         - { type: \"null\" }\n",
    );
    let (generated, code) = generate_with_code(&spec);
    let checked = check(&spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: an `anyOf`-spelled nullable alias is the same shape as the `oneOf` one and \
             must generate: {report:#?}\n{spec}"
        );
    }
    // The pair the mutation destroys: the alias binds its *target*, optional for the `"null"`
    // member and boxed for the cycle. Refusing the spelling gives `Rejected`/`E007` instead, so
    // both halves of this are what stand over the arm.
    assert_eq!(
        field_type(&code, "pub b").as_deref(),
        Some("Option<Box<A>>"),
        "the `anyOf` spelling did not reach the alias recogniser: {code}"
    );
}

/// The same alias, spelled the other three ways one target can be written.
///
/// A `$ref` is not identified by its spelling. `#/components/schemas/Node`, the root's own
/// `./openapi.yaml#/components/schemas/Node`, a split description's
/// `./lib.yaml#/components/schemas/Node` and a whole-file `./node.yaml` can all name one schema —
/// `ensure_resolved`'s own contract says the last two "resolve to the same file and pointer and
/// share one type". The alias recognition keyed on the literal `#/components/schemas/` prefix and
/// on the root component map alone, so only the first spelling was an alias and the other three
/// fell through to a rejection whose sentence — "names no shape of its own and cannot be given a
/// generated type" — the same binary disproves by generating `Option<Box<Node>>` for spelling one.
///
/// The split-description spelling is the case issue #107 exists to make resolve, so it is the one
/// that must not reject.
#[test]
fn a_nullable_alias_is_recognised_however_its_target_is_spelled() {
    // (1) The root document referring to its own components by relative file path.
    let self_file = r##"
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
      required: [parent]
      properties:
        name: { type: string }
        parent: { $ref: '#/components/schemas/MaybeNode' }
    MaybeNode:
      oneOf:
        - { $ref: './openapi.yaml#/components/schemas/Node' }
        - { type: "null" }
"##;
    let (generated, code) = generate_with_code(self_file);
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert_ne!(check(self_file).outcome(), Outcome::Rejected);
    // Named, and read off `parent`'s own declaration rather than searched for in the file. The
    // embedded runtime emits `source: Option<Box<dyn std::error::Error + Send + Sync>>` into every
    // generated module, so `code.contains("Option<Box<")` is unconditionally true of any successful
    // generation and pins nothing beyond the `assert_ne!` above it. What must hold here is that the
    // alias resolved to `Node` — optional, and boxed so the recursion has a finite size.
    //
    // `parent` is **required** precisely so the `Option` can only have come from the alias: every
    // other fixture leaves the field absent from `required`, which makes it optional for a reason
    // that has nothing to do with the union's `"null"` member, so none of them could see that half
    // of the alias's nullability being dropped. `Box<Node>` here would make `{"parent": null}`
    // undecodable against a document that declares it legal.
    assert_eq!(
        field_type(&code, "pub parent").as_deref(),
        Some("Option<Box<Node>>"),
        "{code}"
    );

    // (2) The split description: every schema in the sub-file, the alias member spelled as that
    //     file's own sibling reference. This is #107's namespace case.
    let lib = r##"
components:
  schemas:
    Node:
      type: object
      properties:
        name: { type: string }
        parent: { $ref: '#/components/schemas/MaybeNode' }
    MaybeNode:
      oneOf:
        - { $ref: '#/components/schemas/Node' }
        - { type: "null" }
"##;
    let (generated, checked, code) = split("./lib.yaml#/components/schemas/Node", lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::InvalidInput),
            "{entry}: {report:#?}"
        );
    }
    assert_eq!(
        field_type(&code, "pub parent").as_deref(),
        Some("Option<Box<Node>>"),
        "{code}"
    );

    // (3) Whole-file references, which carry no pointer at all — the spelling whose rejection had
    //     no `at` to point the reader at.
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        "openapi: 3.1.0\n\
         info: { title: T, version: 1.0.0 }\n\
         servers: [{ url: 'https://e.com' }]\n\
         paths:\n  \
         /u:\n    \
         get:\n      \
         operationId: getU\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json: { schema: { $ref: './node.yaml' } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("node.yaml"),
        "type: object\nproperties:\n  name: { type: string }\n  parent: { $ref: './maybe.yaml' }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("maybe.yaml"),
        "oneOf:\n  - { $ref: './node.yaml' }\n  - { type: \"null\" }\n",
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::InvalidInput),
            "{entry}: {report:#?}"
        );
    }
    // The whole-file spelling has no component name to take, so the target is named for the
    // position that reached it — but it is still one boxed optional reference to one type.
    assert_eq!(
        field_type(&code, "pub parent").as_deref(),
        Some("Option<Box<ResponseBody>>"),
        "{code}"
    );
}

/// A nullable alias whose member is a name the **root** declares reads the root's component, and
/// says so — exactly as the direct `$ref` spelling of that same member already does.
///
/// `ensure_component`'s precedence is root first, file second: the root's component map is
/// consulted before anything else, and only a name the root does **not** declare is handed to
/// `ensure_resolved` against the referring file. `open_reservation_for_ref` answered the
/// `#/components/schemas/` arm only on an *open reservation* and otherwise fell through to
/// `reference_identity`, whose `from` is the **referring file** — so a name the root declares but
/// is not currently lowering missed the arm, fell through, and bound the sub-file's declaration
/// that the root shadows. The alias was recognised for a target `lower_schema` would never have
/// chosen, and `W011` — raised inside `ensure_component`, which the alias never reached — did not
/// fire, so the document was neither supported as documented nor warned nor rejected.
///
/// The two files here are the matched pair: `next` spelled `{$ref: MaybeShared}` against `next`
/// spelled `{$ref: Shared}` directly, one reference string, one binary, and before this fixture two
/// different answers. `docs/support-matrix.md`'s References row states the precedence this pins.
///
/// The existing spelling fixture cannot catch it: its sub-file names collide with nothing in the
/// root, so the fall-through and the root map agree there by accident.
#[test]
fn a_nullable_alias_member_the_root_shadows_binds_the_roots_component_and_warns() {
    // The sub-file's `Shared` is reached by the *explicit file* spelling, so it is genuinely the
    // open reservation when its own `next` property asks for `#/components/schemas/Shared` — which
    // is the only way the shadowed declaration is a live candidate at all.
    let root = r##"
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
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Shared' } }
components:
  schemas:
    Shared:
      type: object
      properties:
        root_only: { type: string }
      required: [root_only]
"##;
    let lib = |member: &str| {
        format!(
            r##"
components:
  schemas:
    Shared:
      type: object
      properties:
        lib_only: {{ type: string }}
        next: {{ $ref: '#/components/schemas/{member}' }}
    MaybeShared:
      oneOf:
        - {{ $ref: '#/components/schemas/Shared' }}
        - {{ type: "null" }}
"##
        )
    };

    for spelling in ["MaybeShared", "Shared"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), root).unwrap();
        std::fs::write(dir.join("lib.yaml"), lib(spelling)).unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: {report:#?}"
            );
            // Half one: the shadowing is *said*. Both entry points raise it, or `check` is no
            // longer reporting what `generate` reports.
            assert!(
                has_code(report, Code::DeclarationHasNoEffect),
                "{spelling}/{entry}: the root shadows `Shared`, so W011 must fire: {report:#?}"
            );
        }

        // Half two: the shadowing is *done*. Read on the bound type's own fields rather than on its
        // name: the emitter disambiguates the collision by *suffixing* the sub-file's copy, so
        // `Option<Box<Shared2f46d127>>` contains `Shared` and a substring test would pass on the
        // wrong answer — and the alias spelling reaches its target through `MaybeShared`, a third
        // name again. `root_only` is declared only by the root's `Shared` and `lib_only` only by
        // the sub-file's, so the fields say which declaration was read whatever it got called.
        let next = field_type(&code, "pub next")
            .unwrap_or_else(|| panic!("{spelling}: no `next` field at all: {code}"));
        let bound = next
            .rsplit_once('<')
            .map_or(next.as_str(), |(_, tail)| tail)
            .trim_end_matches('>');
        let fields = declared_fields(&code, bound);
        assert!(
            fields.iter().any(|field| field == "root_only"),
            "{spelling}: `next` bound `{next}`, whose fields are {fields:?} — not the root's \
             `Shared`: {code}"
        );
        assert!(
            !fields.iter().any(|field| field == "lib_only"),
            "{spelling}: `next` bound the sub-file's shadowed `Shared` as `{next}`: {code}"
        );
    }
}

/// The other half of the same promise: when the shadowed name's root component *is* open, the
/// alias binds it — correctly — and must still say that it shadowed something.
///
/// This is the one case in which the alias path answers the reference itself rather than handing it
/// back to `ensure_component`, and `ensure_component` is where `W011` is raised. So the type was
/// right and the acknowledgement was missing: a sub-file's `Shared` silently had no effect on a
/// reference that reads it by that name, which is exactly what the References row of
/// `docs/support-matrix.md` promises will be reported.
///
/// The root's `Shared` is open here because it reaches the sub-file and the sub-file comes back to
/// it — mutual recursion across the file boundary, which is the only way a root component is
/// mid-lowering while a sub-file schema is being read.
#[test]
fn a_nullable_alias_that_binds_an_open_root_component_still_reports_the_shadowing() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
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
            application/json: { schema: { $ref: '#/components/schemas/Shared' } }
components:
  schemas:
    Shared:
      type: object
      properties:
        root_only: { type: string }
        holder: { $ref: './lib.yaml#/components/schemas/Holder' }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Holder:
      type: object
      properties:
        next: { $ref: '#/components/schemas/Maybe' }
    Shared:
      type: object
      properties:
        lib_only: { type: string }
    Maybe:
      oneOf:
        - { $ref: '#/components/schemas/Shared' }
        - { type: "null" }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(report, Code::DeclarationHasNoEffect),
            "{entry}: the sub-file's `Shared` is shadowed and read past in silence: {report:#?}"
        );
    }
    // And the binding itself is the root's, boxed because it closes the cycle back to it.
    assert_eq!(
        field_type(&code, "pub next").as_deref(),
        Some("Option<Box<Shared>>"),
        "{code}"
    );
    assert_eq!(
        declared_fields(&code, "Shared"),
        vec!["root_only".to_owned(), "holder".to_owned()],
        "{code}"
    );
}

/// A nullable alias whose target is itself nullable is exactly as optional as the direct `$ref`.
///
/// `B: {oneOf: [{$ref: A}]}` with `A: {type: [object, "null"]}` carries no `{"type": "null"}`
/// member of its own, and the alias read its nullability from the members alone — discarding the
/// reserve-time flag that `ensure_component` records precisely so "every `$ref` consumer … agrees
/// on it". So a field spelled `{$ref: B}` emitted `Box<A>` where the same field spelled `{$ref: A}`
/// emitted `Option<Box<A>>`: one schema, two optionalities, chosen by which name was written.
#[test]
fn a_nullable_alias_carries_its_targets_own_nullability() {
    let spec = |field: &str| {
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
             application/json: {{ schema: {{ $ref: '#/components/schemas/A' }} }}\n\
             components:\n  \
             schemas:\n    \
             A:\n      \
             type: [object, 'null']\n      \
             required: [b]\n      \
             properties:\n        \
             x: {{ type: string }}\n        \
             b: {{ $ref: '#/components/schemas/{field}' }}\n    \
             B:\n      \
             oneOf:\n        \
             - {{ $ref: '#/components/schemas/A' }}\n"
        )
    };
    // The direct spelling is the control: `A` is nullable, so the field is optional.
    let (direct, direct_code) = generate_with_code(&spec("A"));
    assert_ne!(direct.outcome(), Outcome::Rejected, "{direct:#?}");
    assert!(
        direct_code.contains("pub b: Option<Box<A>>"),
        "{direct_code}"
    );

    // The alias spelling must agree with it. Held to the constant `PARITY_FIXTURES` drives, so the
    // spec this fixture asserts on and the spec `check` is run against stay the same document.
    assert_eq!(spec("B"), NULLABLE_ALIAS_CARRY_SPEC);
    let (aliased, aliased_code) = generate_with_code(&spec("B"));
    assert_ne!(aliased.outcome(), Outcome::Rejected, "{aliased:#?}");
    assert!(
        aliased_code.contains("pub b: Option<Box<A>>"),
        "the aliased field lost its target's nullability: {aliased_code}"
    );
}

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
        let (generated, code) = generate_with_code(spec);
        let checked = check(spec);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}/{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::InvalidInput),
                "{label}/{entry}: {report:#?}"
            );
            assert!(
                has_code(report, Code::AllOfIrreconcilable)
                    || has_code(report, Code::NonDisjointUnion),
                "{label}/{entry}: {report:#?}"
            );
            // The false acknowledgement must be gone: the member is not a branch the sibling made
            // impossible, it is one nothing could yet be said about.
            assert!(
                !has_code(report, Code::DeclarationHasNoEffect),
                "{label}/{entry}: {report:#?}"
            );
        }
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
    assert_ne!(
        check(spec).outcome(),
        Outcome::Rejected,
        "{:#?}",
        check(spec)
    );
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

/// Mutually-recursive components (`A -> B -> A`, including recursion through an array) must also
/// generate: exactly one back-edge in the cycle is boxed.
#[test]
fn mutually_recursive_refs_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
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
        children:
          type: array
          items:
            $ref: "#/components/schemas/A"
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "mutually-recursive schemas must not raise an error: {report:#?}"
    );
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
    for (entry, report) in [("generate", generate(&mutual)), ("check", check(&mutual))] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "a mutually recursive back-edge was not rejected by {entry}: {report:#?}"
        );
        assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    }

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
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            verdicts.push((
                format!("{order}/{entry}"),
                report.outcome(),
                has_code(&report, Code::AllOfIrreconcilable),
            ));
        }
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
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` was not rejected by {entry}, so the recursive target is still being \
                 silently discarded on the union path: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "`{what}` did not report E013 through {entry}: {report:#?}"
            );
        }
        let report = generate(&spec);
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
    let base_types = base_code
        .find("pub mod types {")
        .map(|i| base_code[i..].to_owned())
        .unwrap_or_default();

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
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` is not an edge lowering follows, so it cannot make the `$ref` a \
                 cycle-closing one and must not flip {entry} into a rejection: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "`{what}` reported E013 through {entry}: {report:#?}"
            );
        }
        // Stronger than "still generates": the emitted types are the ones the baseline emits, so
        // the keyword is confirmed inert rather than merely tolerated.
        let (_, code) = generate_with_code(&spec);
        let types = code
            .find("pub mod types {")
            .map(|i| code[i..].to_owned())
            .unwrap_or_default();
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

/// The rows of `schemas::a_cycle_closing_untyped_ref_branch_beside_a_null_branch_matches_the_same_union_outside_it`
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
            types.contains("pub type Node = Vec<Node>;"),
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
            let checked = check(&spec);
            assert_ne!(
                checked.outcome(),
                Outcome::Rejected,
                "check, {members}: {checked:#?}"
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
                types.contains("pub type Node = Vec<Node>;"),
                "{members}: the merge is the cycle-closing `items` alone: {types}"
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
    let checked = check(&spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "check: {checked:#?}");
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let next = field_type(&types, "pub next").unwrap_or_else(|| panic!("no `next`: {types}"));
    assert!(
        next.contains("Box<Node>"),
        "`next` keeps the boxed cycle-closing `Node`: `{next}` in {types}"
    );
    // The meet is the `prefixItems` member alone, which lowers on its own as `TupMember0`.
    let alias = |name: &str| {
        types
            .lines()
            .map(str::trim_start)
            .find_map(|line| line.strip_prefix(&format!("pub type {name} = ")))
            .map(str::to_owned)
    };
    let tuple = alias("Tup").unwrap_or_else(|| panic!("no `Tup` alias: {types}"));
    assert!(
        tuple.contains("Box<Tup>"),
        "`Tup` keeps the cycle-closing position: `{tuple}`"
    );
    assert_eq!(
        Some(tuple),
        alias("TupMember0"),
        "`Tup` is its `prefixItems` member, as without the untyped `items`: {types}"
    );
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
    let checked = check(&spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "check: {checked:#?}");
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
