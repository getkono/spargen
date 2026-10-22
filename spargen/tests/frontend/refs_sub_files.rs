//! Reference resolution across sub-files: each file resolving its own components, recursion and
//! alias cycles inside a sub-file, sub-file defaults and non-object components, the depth cap, and
//! reuse generating one type per declaration.

use super::*;

/// A `$ref` inside a referenced sub-file spells that file's own components the ordinary way —
/// `#/components/schemas/<name>` — and it must resolve against the file it is written in. This is
/// the standard layout for a split description: the root references `./lib.yaml#/components/schemas/
/// Wrapper`, and `Wrapper`'s own properties reference its siblings by plain component name.
///
/// `Resolver::resolve` already implements exactly this, keying on the provenance's file and
/// shortcutting to the parsed component map only for the root document. `ensure_component` bypassed
/// the resolver for anything carrying the `#/components/schemas/` prefix and looked every such name
/// up in the ROOT document's map whatever file it sat in, so the sibling reference missed. Before
/// E004 fired that miss was a silent drop — the property simply vanished — which is the same
/// silent drop of an unresolved component reference that E004 exists to replace, just reached from
/// a sub-file.
///
/// `corpus-smoke` cannot see this: the one multi-file corpus case uses whole-file `$ref`s, and the
/// other relative-file fixture here uses a non-component fragment (`#/Pet`), which never enters
/// `ensure_component`. This fixture is the only evidence.
#[test]
fn a_sub_file_resolves_its_own_component_refs_rather_than_the_roots() {
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
            application/json:
              schema: { $ref: './lib.yaml#/components/schemas/Wrapper' }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Wrapper:
      type: object
      properties:
        inner: { $ref: '#/components/schemas/Inner' }
      required: [inner]
    Inner:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##,
    )
    .unwrap();

    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    // The sibling was genuinely followed: `Inner`'s own field reached the generated type, so the
    // property is typed rather than dropped.
    assert!(code.contains("pub inner"), "{code}");
    assert!(code.contains("pub id"), "{code}");

    // `check` must agree — it runs the same lowering.
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");
}

/// A sub-file sibling reference that names nothing. The root document's component map is consulted
/// first and misses, and the sub-file's own map misses too — so the reference is unresolvable and
/// must be reported, not dropped. This is the sub-file spelling of the very bug issue #107 exists
/// to remove: before the reference reached the resolver at all it was silently discarded, taking
/// the property with it.
#[test]
fn a_sub_file_ref_to_a_component_its_own_file_does_not_declare_is_rejected() {
    let (generated, checked, _) = split(
        "./lib.yaml#/components/schemas/Wrapper",
        r##"
components:
  schemas:
    Wrapper:
      type: object
      properties:
        inner: { $ref: '#/components/schemas/Missing' }
      required: [inner]
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e004: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .collect();
        assert!(!e004.is_empty(), "{entry}: {report:#?}");
        // The message must name the reference that could not be followed: a pointer says where the
        // `$ref` sits, not what it asked for, and `Missing` is the only thing wrong here.
        assert!(
            e004.iter()
                .any(|d| d.message.contains("#/components/schemas/Missing")),
            "{entry}: the rejection must name the reference: {report:#?}"
        );
    }
}

/// A self-recursive schema declared in a sub-file and referencing itself by plain component name.
///
/// `docs/support-matrix.md` lists recursive `$ref` cycles as supported and boxes the cycle-closing
/// reference; the root document has always done that. The sub-file spelling must reach the same
/// place. It did not: without an in-progress reservation keyed on the resolved target, every
/// re-entry re-resolved and re-lowered the schema afresh, so a 9-line document walked to
/// `MAX_SCHEMA_DEPTH` and rejected with `E014` — a cap whose own text says no real description
/// reaches it, on a `$ref` chain of length one. Neither remedy it offered applied: a recursive type
/// cannot be flattened, and the omit route ends in `E019` (pinned in `carve.rs`).
#[test]
fn a_self_recursive_sub_file_schema_is_boxed_rather_than_rejected() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Node",
        r##"
components:
  schemas:
    Node:
      type: object
      properties:
        next: { $ref: '#/components/schemas/Node' }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: a self-reference is a cycle to box, not a chain to reject: {report:#?}"
        );
        assert!(
            !has_code(report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }
    // Boxed, and boxed against the type itself — not against a second copy of it under another
    // name, which is what an unmemoized re-entry would have produced had it terminated.
    assert!(code.contains("Option<Box<Node>>"), "{code}");
    assert_eq!(
        declared_types(&code, "Node", |tail| tail.trim().is_empty()).len(),
        1,
        "one declared schema, one generated type: {code}"
    );

    // The identical shape written in the ROOT document is the control: it has always generated, so
    // what this fixture pins is the file the schema sits in, not the shape.
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
            application/json: { schema: { $ref: '#/components/schemas/Node' } }
components:
  schemas:
    Node:
      type: object
      properties:
        next: { $ref: '#/components/schemas/Node' }
"##;
    let (report, root_code) = generate_with_code(root);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(root_code.contains("Option<Box<Node>>"), "{root_code}");
}

/// Mutual recursion across two sub-file components, `A -> B -> A`, both referencing by plain
/// component name. One of the two edges is boxed, exactly as the root document's `Category`/`Item`
/// pair is. Separate from the self-reference above because it closes the cycle through a *second*
/// reservation rather than re-entering the one already on the stack.
#[test]
fn a_mutually_recursive_sub_file_pair_is_boxed_rather_than_rejected() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/A",
        r##"
components:
  schemas:
    A:
      type: object
      required: [name]
      properties:
        name: { type: string }
        b: { $ref: '#/components/schemas/B' }
    B:
      type: object
      required: [label]
      properties:
        label: { type: string }
        a: { $ref: '#/components/schemas/A' }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: {report:#?}"
        );
    }
    assert_eq!(
        declared_types(&code, "A", |tail| tail.is_empty()).len(),
        1,
        "{code}"
    );
    assert_eq!(
        declared_types(&code, "B", |tail| tail.is_empty()).len(),
        1,
        "{code}"
    );
    // Exactly one of the two edges carries the indirection; both would be redundant and neither
    // would compile.
    let boxed =
        usize::from(code.contains("Option<Box<A>>")) + usize::from(code.contains("Option<Box<B>>"));
    assert_eq!(boxed, 1, "exactly one edge in the cycle is boxed: {code}");
}

/// A cycle of sub-file component *aliases* — each component is a bare `$ref` to the next, so there
/// is no schema body to reserve a root against and the reserve/box machinery never engages. This is
/// the shape `remote_alias_stack` exists for on the remote path; the sub-file path needs its own
/// guard or the cycle only stops at the depth cap.
///
/// It is a genuine document error either way, so what this pins is *which* error: an alias cycle
/// named as one, not `E014`, whose message would blame chain length and offer a flattening remedy
/// for a document that has no chain to flatten.
#[test]
fn a_sub_file_component_alias_cycle_is_reported_as_a_cycle_not_as_excessive_depth() {
    let (generated, checked, _) = split(
        "./lib.yaml#/components/schemas/A",
        r##"
components:
  schemas:
    A: { $ref: '#/components/schemas/B' }
    B: { $ref: '#/components/schemas/A' }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::UnresolvedRef && d.message.contains("cycle")),
            "{entry}: the rejection must name the cycle: {report:#?}"
        );
        assert!(
            !has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: a two-component loop is a cycle, not a deep chain: {report:#?}"
        );
    }
}

/// A sub-file component whose body is a nullable union keeps its nullability at every use, as the
/// same component in the root document does.
///
/// The reserve-time guess this was found against, `schema_is_nullable`, read only `type`, `enum`
/// and `const`, so for `oneOf: [null, string]` it answered `false`, while lowering the union
/// answers `true` (the guess is now `null_admission`, which reads the union and agrees with the
/// body here; the fixture pins that the body's answer is the one kept). `ensure_component`
/// stopped writing that provisional answer back over the body's; `ensure_resolved` still did, and
/// cached it, so
/// both required fields below were emitted as a bare `M` and a `null` the description allows would
/// fail to decode. Two fields, because the first use returns the lowered `Ty` and the second the
/// cached one, and each carried the overwrite separately.
#[test]
fn a_sub_file_nullable_union_component_stays_nullable_at_every_use() {
    const COMPONENTS: &str = r##"
components:
  schemas:
    W:
      type: object
      required: [a, b]
      properties:
        a: { $ref: '#/components/schemas/M' }
        b: { $ref: '#/components/schemas/M' }
    M:
      oneOf:
        - { type: 'null' }
        - { type: string }
"##;
    let (generated, checked, code) = split("./lib.yaml#/components/schemas/W", COMPONENTS);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for field in ["pub a:", "pub b:"] {
        assert_eq!(
            field_type(&code, field).as_deref(),
            Some("Option<M>"),
            "{field}: {code}"
        );
    }

    // The root-document control: the same components, the same field types.
    let (report, root) = generate_with_code(&format!(
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
{COMPONENTS}"##
    ));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for field in ["pub a:", "pub b:"] {
        assert_eq!(
            field_type(&root, field).as_deref(),
            Some("Option<M>"),
            "{field}: {root}"
        );
    }
}

/// A sub-file component's `default` reaches its generated type's rustdoc, as a root component's
/// does. `ensure_resolved` lowers the sub-file root's body and then appends the note to the lifted
/// definition itself, so dropping that step loses the documented default with nothing else
/// changing.
#[test]
fn a_sub_file_component_default_is_documented_on_its_type() {
    // The doc line directly above the declaration, so a note that lands on another item, or a
    // declaration with no note, does not satisfy it.
    let doc_above = |code: &str, declaration: &str| {
        let lines: Vec<&str> = code.lines().map(str::trim).collect();
        lines
            .iter()
            .position(|line| *line == declaration)
            .and_then(|at| at.checked_sub(1))
            .map(|above| lines[above].to_owned())
    };
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Counted",
        r##"
components:
  schemas:
    Counted: { type: integer, default: 7 }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_eq!(
        doc_above(&code, "pub type Counted = i64;").as_deref(),
        Some("///Default: `7`."),
        "the sub-file default must document the type it declares: {code}"
    );

    // The root-document control: the same component, the same note.
    let (report, root) = generate_with_code(
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
            application/json: { schema: { $ref: '#/components/schemas/Counted' } }
components:
  schemas:
    Counted: { type: integer, default: 7 }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        doc_above(&root, "pub type Counted = i64;").as_deref(),
        Some("///Default: `7`."),
        "{root}"
    );
}

/// A sub-file component that is not an object. Deduplicating sub-file components lifts the lowered
/// root into a reserved id and asserts the root was the last definition its own body inserted — an
/// invariant a scalar (one insert, no children) and a union (a wrapper over boxed members) exercise
/// differently from the object every other fixture here uses.
#[test]
fn a_non_object_sub_file_component_lowers_to_its_own_shared_type() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Wrapper",
        r##"
components:
  schemas:
    Wrapper:
      type: object
      required: [name, either]
      properties:
        name: { $ref: '#/components/schemas/Name' }
        either: { $ref: '#/components/schemas/Either' }
    Name: { type: string }
    Either:
      oneOf:
        - type: string
        - type: integer
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }
    // Each reached the field as its own named type — not as an untyped value, and not dropped.
    assert!(code.contains("pub type Name = String;"), "{code}");
    assert!(code.contains("pub name: Name"), "{code}");
    assert!(code.contains("pub enum Either"), "{code}");
    assert!(code.contains("pub either: Either"), "{code}");
}

/// One sub-file component, referenced twice. This is the shape nothing could have caught: it is
/// `Generated` and `Clean` whichever way it behaves, so a fixture that pins only rejections is
/// blind to it.
///
/// Re-resolving per reference site produced one fresh type per *use* rather than per *declaration*
/// — `Inner` and `InnerD5129632` for a single declared schema — which is not one bug but three:
/// the two are not interchangeable in Rust, each is a separate item in the `spargen diff` semver
/// surface, and the duplication compounds multiplicatively down a reuse graph (a 17-schema,
/// 40-line description reached 2^16 lowerings and produced no output at all).
#[test]
fn a_sub_file_component_used_twice_generates_one_type() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Node",
        r##"
components:
  schemas:
    Node:
      type: object
      required: [first, second]
      properties:
        first: { $ref: '#/components/schemas/Inner' }
        second: { $ref: '#/components/schemas/Inner' }
    Inner:
      type: object
      required: [id]
      properties: { id: { type: string } }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    // One declaration, one type. `pub struct Inner` is a prefix of every hash-suffixed duplicate
    // (`pub struct InnerD5129632`), so this count catches them too.
    assert_eq!(
        declared_types(&code, "Inner", |_| true).len(),
        1,
        "one declared schema must generate one type: {:?}",
        declared_types(&code, "Inner", |_| true)
    );
    // And both uses reached that one type, rather than one of them reaching a copy.
    assert!(code.contains("pub first: Inner"), "{code}");
    assert!(code.contains("pub second: Inner"), "{code}");
}

/// The control for the three fixtures above: sharing one type per sub-file component must not
/// disarm the depth cap. A genuinely long chain — each sub-file component `$ref`ing the next, no
/// reuse and no cycle, so nothing is ever a repeat visit — still exceeds `MAX_SCHEMA_DEPTH` and
/// still rejects with `E014`. Without this, removing the cap entirely would leave the suite green.
#[test]
fn a_long_sub_file_ref_chain_still_exceeds_the_depth_cap() {
    let depth = 200;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..depth {
        lib.push_str(&format!(
            "    L{level}:\n      type: object\n      required: [next]\n      properties:\n        next: {{ $ref: '#/components/schemas/L{}' }}\n",
            level + 1
        ));
    }
    lib.push_str(&format!(
        "    L{depth}:\n      type: object\n      properties: {{ id: {{ type: string }} }}\n"
    ));
    let (generated, checked, _) = split("./lib.yaml#/components/schemas/L0", &lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: {report:#?}"
        );
    }
}

/// An `allOf` member that is a direct `$ref` back to the sub-file schema currently being lowered.
///
/// `push_ref_member` decides a member's contribution by reading `graph.get(ty.id).kind`. For a
/// back-edge against an in-progress reservation that kind is the placeholder `TypeKind::Reserved`
/// the reservation was created with — **the id is right, the kind is not** — and the placeholder
/// was once read as a scalar, so the member was classified `Contribution::Scalar` and the whole
/// property collapsed to `serde_json::Value` with **no diagnostic at all**. `CLAUDE.md` names that
/// exact outcome: generated code "never silently degrades a typed schema to `serde_json::Value`",
/// and every construct is "supported, warned, or rejected — no fourth, silent behavior".
/// `push_ref_member` now rejects a `TypeKind::Reserved` member with `E013`.
///
/// The root document refuses to read an in-progress member and rejects it with `E013`. On the
/// remote path the id-keyed guard is pinned by
/// `refs_remote::a_direct_recursive_remote_all_of_member_reached_through_an_alias_is_rejected`. All
/// three spellings here must reach that same rejection: the bare sub-file name, the explicit file
/// reference, and the root-document control.
#[test]
fn a_direct_recursive_all_of_member_in_a_sub_file_is_rejected_as_the_root_document_is() {
    // One shape, two spellings of the same target. `PREFIX` is empty for the sub-file's own
    // component name and `./lib.yaml` for the explicit file reference; both address `Tree`.
    const TREE: &str = r##"
components:
  schemas:
    Tree:
      type: object
      properties:
        label: { type: string }
        child:
          description: the child node
          allOf:
            - { $ref: 'PREFIX#/components/schemas/Tree' }
"##;
    let body = |prefix: &str| TREE.replace("PREFIX", prefix);

    for (spelling, lib) in [("bare", body("")), ("explicit", body("./lib.yaml"))] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Tree", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a direct recursive `allOf` member must be rejected, not \
                 silently retyped: {report:#?}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable
                        && d.message.contains("direct recursive")),
                "{spelling}/{entry}: it is a direct recursive member, and must be named as one: \
                 {report:#?}"
            );
        }
        // The silent degradation itself, asserted directly: nothing may type this property as an
        // untyped value, whatever the verdict.
        assert!(
            !code.contains("pub type Treechild = serde_json::Value;"),
            "{spelling}: the recursive member was silently retyped: {code}"
        );
    }

    // The root-document control: the same shape, always rejected, and the message the sub-file
    // spellings must now match.
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Tree' }} }}
{}"##,
        body("")
    );
    let report = generate(&root);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::AllOfIrreconcilable && d.message.contains("direct recursive")),
        "{report:#?}"
    );
}

/// The precondition `ensure_resolved`'s root-component routing rests on: a pointer deeper than a
/// component, such as `/components/schemas/Tree/properties/x`, is routed by `contains_key` alone,
/// which is sound only because no root component key can contain `/`. Structural validation rejects
/// such a key under both versions before lowering runs, so the nested reference below never reaches
/// the component literally named `Tree/properties/x`.
#[test]
fn a_root_component_key_containing_a_slash_is_rejected_before_lowering() {
    for version in ["3.1.0", "3.2.0"] {
        let spec = format!(
            r##"
openapi: {version}
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Tree/properties/x' }} }}
components:
  schemas:
    Tree:
      type: object
      properties:
        x: {{ type: integer }}
    Tree/properties/x: {{ type: string }}
"##
        );
        for (entry, report) in [("generate", &generate(&spec)), ("check", &check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{version}/{entry}: {report:#?}"
            );
            assert!(
                messages_for(report, Code::InvalidInput)
                    .iter()
                    .any(|message| message.contains("\"Tree/properties/x\" does not match")),
                "{version}/{entry}: the key itself must be what is rejected: {report:#?}"
            );
        }
    }
}

/// The fourth spelling of the shape above, and the one the root-document branch of
/// `reservation_at` exists for: a **root** component whose `allOf` member names itself through an
/// explicit file reference to the root document. `ensure_resolved` routes that reference back to
/// `ensure_component`, so the in-progress schema is recorded in the root component map rather than
/// the resolved-reference one, and only the root-document branch finds it.
///
/// Without that branch the member is not recognised as in progress, is lowered again, and recurses
/// until the depth cap: the document is rejected with `E014`, a chain-length blame for a cycle of
/// length one — the misdiagnosis
/// `a_sub_file_component_alias_cycle_is_reported_as_a_cycle_not_as_excessive_depth` forbids.
#[test]
fn a_direct_recursive_all_of_member_named_by_explicit_root_file_reference_is_rejected() {
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
            application/json: { schema: { $ref: '#/components/schemas/Tree' } }
components:
  schemas:
    Tree:
      type: object
      properties:
        label: { type: string }
        child:
          description: the child node
          allOf:
            - { $ref: './openapi.yaml#/components/schemas/Tree' }
"##;
    for (entry, report) in [("generate", &generate(root)), ("check", &check(root))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::AllOfIrreconcilable
                    && d.message.contains("direct recursive")),
            "{entry}: it is a direct recursive member, and must be named as one: {report:#?}"
        );
        assert!(
            !has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: a member that names its own component is a cycle, not a deep chain: \
             {report:#?}"
        );
    }
}

/// A resolved target is named for its final pointer token, and that token is RFC 6901-unescaped
/// first — the result is a public type name in the generated API. Two targets pin both halves:
///
/// - `a~1b` is the key `a/b`, so it is named `AB`; left escaped it would be `A1b`.
/// - `a~01b` is the key `a~1b` — a literal tilde — so it is named `A1b`. Decoding `~0` before `~1`
///   turns it into `a/b` instead, and it collides with the first target's name.
///
/// So the unescape and its order (`~1` first, then `~0`) each change a public name here.
#[test]
fn a_resolved_target_is_named_for_its_unescaped_final_pointer_token() {
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
            application/json:
              schema:
                type: object
                required: [slash, tilde]
                properties:
                  slash: { $ref: '#/x-weird/a~1b' }
                  tilde: { $ref: '#/x-weird/a~01b' }
x-weird:
  a/b:
    type: object
    properties: { x: { type: string } }
  a~1b:
    type: object
    properties: { y: { type: string } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub struct AB "),
        "`a~1b` names the key `a/b`: {code}"
    );
    assert!(
        code.contains("pub slash: AB"),
        "the `a/b` target is the slash field's type: {code}"
    );
    assert!(
        code.contains("pub struct A1b "),
        "`a~01b` names the key `a~1b`: {code}"
    );
    assert!(
        code.contains("pub tilde: A1b"),
        "the `a~1b` target is the tilde field's type: {code}"
    );
}

/// The one-member form of the same fault, which has no sibling to disguise it: a sub-file component
/// whose entire body is `allOf: [$ref to itself]`. The placeholder read made the component itself
/// `serde_json::Value`, so the operation's whole response body was untyped — `clean`.
#[test]
fn a_sub_file_component_that_is_an_all_of_of_itself_is_rejected() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Loop",
        r##"
components:
  schemas:
    Loop:
      allOf:
        - { $ref: '#/components/schemas/Loop' }
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(report, Code::AllOfIrreconcilable),
            "{entry}: {report:#?}"
        );
    }
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

/// An untyped (`{}`) sub-file component used as an `application/octet-stream` body by one operation
/// and an `application/json` body by another.
///
/// `opaque_octets` retypes an untyped body to `bytes::Bytes`. When the type it is about to retype is
/// the last definition inserted it rewrites it *in place*, which is right for a use-site type and
/// catastrophic for a shared one — every other reference to that component silently becomes `Bytes`
/// too. `is_component_root` exists to stop exactly that, and it consulted the root-component and
/// remote memos but not the resolved-reference memo the preceding round added, so a sub-file
/// component was not recognised as a named root.
///
/// The result was a JSON operation returning `bytes::Bytes` for a schema that is `serde_json::Value`
/// — the wrong Rust type on a typed API, with no diagnostic. The root-document control below is the
/// same shape and has always been correct, so what this pins is the memo the check reads, not the
/// policy.
#[test]
fn an_untyped_sub_file_component_is_not_retyped_in_place_by_an_octet_use() {
    let split_layout = |prefix: &str| {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.join("openapi.yaml"),
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /raw:
    get:
      operationId: getRaw
      responses:
        '200':
          description: ok
          content:
            application/octet-stream: {{ schema: {{ $ref: '{prefix}#/components/schemas/Opaque' }} }}
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '{prefix}#/components/schemas/Opaque' }} }}
"##
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("lib.yaml"),
            "components:\n  schemas:\n    Opaque: {}\n",
        )
        .unwrap();
        let out = dir.join("client.rs");
        let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        (report, code)
    };

    let (report, code) = split_layout("./lib.yaml");
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // The shared component keeps the type its own schema declares. The octet use site gets its own
    // `Bytes` type; it does not get to rewrite everyone else's.
    assert!(
        !code.contains("pub type Opaque = bytes::Bytes;"),
        "an octet use retyped the shared component for every other reference: {code}"
    );
    assert!(
        code.contains("pub type Opaque = serde_json::Value;"),
        "{code}"
    );

    // The root-document control: identical shape, always correct, because `is_component_root`
    // already consulted the map a root component lives in.
    let root = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /raw:
    get:
      operationId: getRaw
      responses:
        '200':
          description: ok
          content:
            application/octet-stream: { schema: { $ref: '#/components/schemas/Opaque' } }
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Opaque' } }
components:
  schemas:
    Opaque: {}
"##;
    let (report, code) = generate_with_code(root);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type Opaque = serde_json::Value;"),
        "{code}"
    );
}

/// The fan-out bound, discriminated deeper than one nesting level.
///
/// A two-level reuse graph distinguishes 2 types from 1, which any memo that fires *somewhere*
/// satisfies — a memo that inserted only at depth 1 and skipped deeper insertions passed the
/// duplicate-type fixture above while restoring 4097 types from a 14-schema description. Depth is
/// what the bound is about, so depth is what this measures: 12 declared schemas must generate 12
/// `L*` types and not 4095.
///
/// The control below is the same graph written in the root document, which has always been linear,
/// so the assertion is anchored to a number the repository already produces rather than to one this
/// fixture invents.
#[test]
fn a_deep_sub_file_reuse_graph_generates_one_type_per_declaration() {
    const DEPTH: usize = 11;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..DEPTH {
        lib.push_str(&format!(
            "    L{level}:\n      type: object\n      properties:\n        a: {{ $ref: \
             '#/components/schemas/L{next}' }}\n        b: {{ $ref: \
             '#/components/schemas/L{next}' }}\n",
            next = level + 1
        ));
    }
    lib.push_str(&format!(
        "    L{DEPTH}:\n      type: object\n      properties: {{ id: {{ type: string }} }}\n"
    ));

    let (generated, checked, code) = split("./lib.yaml#/components/schemas/L0", &lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    // One declaration, one type — at every level, not merely at the first. Without the bound this
    // is 2^(DEPTH+1) - 1. Counted on `L<digits>` exactly: a bare `pub struct L` prefix also matches
    // the embedded runtime's own `LinkPaginator`, and a bound that is off by a constant is not one.
    // Every `L`-prefixed type whose name continues with a digit: `L1` and any disambiguated
    // duplicate of it (`L1a1b2c3`) alike, so a suffixed copy is counted rather than filtered out.
    // Excluding non-digit tails drops the embedded runtime's `LinkPaginator` and nothing else.
    let declared = declared_types(&code, "L", |tail| {
        tail.starts_with(|character: char| character.is_ascii_digit())
    });
    assert_eq!(
        declared.len(),
        DEPTH + 1,
        "{} declared schemas generated {} types: {declared:?}",
        DEPTH + 1,
        declared.len()
    );
    // And every level is present by name, so the count cannot be met by collapsing distinct schemas.
    for level in 0..=DEPTH {
        assert!(
            code.contains(&format!("pub struct L{level} ")),
            "L{level} is missing: {code}"
        );
    }
}

/// The explicit file spelling of a shared sub-file component — the row the key's design exists for,
/// and the one with no fixture of its own.
///
/// The identity is the resolved target's `file#pointer`, not the `$ref` spelling, precisely so that
/// `./lib.yaml#/components/schemas/Inner` and the sub-file's own `#/components/schemas/Inner` are
/// one target. Keying on `(FileId, name)` would close only the bare spelling and leave this one
/// duplicating per reference site, which is the state the preceding round measured at 43 MB.
#[test]
fn the_explicit_file_spelling_of_one_component_also_generates_one_type() {
    let lib = r##"
components:
  schemas:
    Node:
      type: object
      required: [first, second]
      properties:
        first: { $ref: './lib.yaml#/components/schemas/Inner' }
        second: { $ref: './lib.yaml#/components/schemas/Inner' }
    Inner:
      type: object
      required: [id]
      properties: { id: { type: string } }
"##;
    let (generated, checked, code) = split("./lib.yaml#/components/schemas/Node", lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert_eq!(declared_types(&code, "Inner", |_| true).len(), 1, "{code}");
    assert!(code.contains("pub first: Inner"), "{code}");
    assert!(code.contains("pub second: Inner"), "{code}");

    // Mixing the two spellings of one target in one document must still give one type: that is the
    // whole claim the resolved-pointer key makes, and neither spelling alone can test it.
    let mixed = lib.replace(
        "second: { $ref: './lib.yaml#/components/schemas/Inner' }",
        "second: { $ref: '#/components/schemas/Inner' }",
    );
    let (_, _, code) = split("./lib.yaml#/components/schemas/Node", &mixed);
    assert_eq!(
        declared_types(&code, "Inner", |_| true).len(),
        1,
        "the two spellings of one target must share one type: {:?}",
        declared_types(&code, "Inner", |_| true)
    );
}
