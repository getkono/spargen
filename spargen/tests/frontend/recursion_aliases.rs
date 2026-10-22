//! Recursion, second part: nullable recursive references, nullable aliases under mutual
//! recursion, back-edge nullability, and which unions are recognised as aliases.

use super::*;

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
        // reads the reserve-time guess. At issue #222 the guess was `schema_is_nullable`, which
        // never looked at a union's members, and `B`'s `"null"` member made `B` nullable only after
        // `A.b` had been typed `Box<B>`, a field that could not decode the legal `{"b": null}`. The
        // guess is now `null_admission` over `B`'s body, which reads the `"null"` member, so the
        // back-edge opens with `B`'s answer and is `Option<Box<B>>`: optional because `B` is, boxed
        // because the cycle must have a finite size.
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
/// `oneOf` member, which the reserve-time guess at issue #222, `schema_is_nullable`, never read, so
/// while `N`'s body was open the only answer there was said non-nullable, and `next` — the one
/// reference taken while it is open — was typed `Box<N>`: `{"next": null}`, which ends every such
/// list, did not decode. `W.n` is the control, taken after `N` finished: it was already
/// `Option<N>`, so one schema meant two things depending on when it was referenced. The guess is
/// now `null_admission` over the body, which reads the `oneOf` member, so both are optional from
/// the first pass; [`a_back_edge_whose_guess_the_body_contradicts_takes_the_bodys_answer`] is the
/// fixture whose guess the body still contradicts.
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
/// the reserve-time guess at the time, `schema_is_nullable`, never read. `B` is a one-member
/// `oneOf` over `A`, so when `A.b` lowers `B` while `A` is still open, the alias recogniser
/// answers `b` with `A`'s open reservation instead of taking an ordinary back-edge into `B`. That
/// reservation's nullability is still the guess, so when the guess was wrong and no read was
/// recorded the pass was never revised and `b` was `Box<A>`, which cannot decode the legal
/// `{"b": null}`. The guess is now `null_admission` over `A`'s body, which reads the `oneOf`
/// member, so it agrees with the body here. `W.a` is the control, taken after `A` finished.
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

/// The mirror of [`a_back_edge_into_a_union_nullable_component_is_optional`]: the type array says
/// nullable and the body says otherwise, and the back-edge follows the body.
///
/// `N`'s type array admits `"null"`, which was all the reserve-time guess then,
/// `schema_is_nullable`, read, but its `oneOf` has no null branch, so `null` matches no member and
/// fails the union: `N` is not nullable, and the finished reference `W.n` is a plain `N`. The
/// back-edge `next` read the guess and was `Option<Box<N>>`, which accepts a `null` the schema
/// rejects. The guess is now `null_admission` over the body, which reads the `oneOf` too, so it
/// denies `null` from the first pass.
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

/// A back-edge whose reserve-time guess the body contradicts takes the body's answer through a
/// second pass, not the guess.
///
/// The guess is whether `Pick`'s body admits `null` (`null_admission`): its `anyOf` has a
/// `const: null` branch, so it does, and `next` — read while `Pick` is still open — opens as
/// `Option<Box<Pick>>`. The finished body carries that `null` in its `()` arm alone, never also as
/// the union's `Option`, so `Pick` is not nullable. `settle_reservation` records the
/// contradiction and the next pass types `next` from the body: `Box<Pick>`, whose `()` arm decodes
/// `{"next": null}`. Without that pass `next` keeps the guess and carries the `null` twice, beside
/// the `()` arm, as the finished reference `W.pick` does not.
#[test]
fn a_back_edge_whose_guess_the_body_contradicts_takes_the_bodys_answer() {
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
      required: [pick]
      properties:
        pick: { $ref: '#/components/schemas/Pick' }
    Pick:
      anyOf:
        - const: null
        - type: object
          required: [next]
          properties:
            next: { $ref: '#/components/schemas/Pick' }
"##;
    let (generated, code) = generate_with_code(spec);
    let checked = check(spec);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_eq!(
        field_type(&code, "pub pick:").as_deref(),
        Some("Pick"),
        "the control: {code}"
    );
    assert_eq!(
        field_type(&code, "pub next:").as_deref(),
        Some("Box<Pick>"),
        "the back-edge must follow the body, not the reserve-time guess: {code}"
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
