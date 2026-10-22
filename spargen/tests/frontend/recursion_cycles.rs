//! Recursion, first part: alias chains and cycles, the `E004` alias walkers, recursive `allOf`
//! members, cycle-closing references carrying siblings, and one-edge cycles.

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
        let report = generate(document);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{label}: two members naming the same recursive target were reported as conflicting: \
             {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{label}: {report:#?}"
        );
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
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{applicator}: the member is `Node` and the union is `Nodechild` — two different \
             types, so the decoder terminates: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::NonDisjointUnion),
            "{applicator}: {report:#?}"
        );
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
