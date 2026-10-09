//! Reference resolution: sub-files, JSON Pointers, `$self`, depth caps, type naming across files,
//! and remote documents.

use super::*;

/// The Header and Media Type Object alias walkers report each `E004` case in its own words too:
/// an undeclared component as `unresolved … reference` naming the reference as written, a
/// pointer with nothing at it as `not found in the input bundle`, and only a reference the
/// bundle cannot place as `unsupported or unresolved`.
#[test]
fn e004_header_and_media_type_walkers_report_each_case_in_its_own_words() {
    let valid = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Rate: { $ref: '#/components/headers/Rate' }
          content:
            application/json: { $ref: '#/components/mediaTypes/ItemJson' }
components:
  headers:
    Rate: { schema: { type: integer } }
  mediaTypes:
    ItemJson: { schema: { type: string } }
"##;
    let report = generate(valid);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");

    let at = |header: &str, media: &str| {
        valid
            .replace(
                "{ $ref: '#/components/headers/Rate' }",
                &format!("{{ $ref: '{header}' }}"),
            )
            .replace(
                "{ $ref: '#/components/mediaTypes/ItemJson' }",
                &format!("{{ $ref: '{media}' }}"),
            )
    };
    for (case, spec, expected) in [
        (
            "undeclared component",
            at(
                "#/components/headers/Missing",
                "#/components/mediaTypes/Missing",
            ),
            [
                "unresolved header reference `#/components/headers/Missing`",
                "unresolved Media Type Object reference `#/components/mediaTypes/Missing`",
            ],
        ),
        (
            "absent target",
            at("#/nowhere/header", "#/nowhere/media"),
            [
                "header reference target `#/nowhere/header` was not found in the input bundle",
                "Media Type Object reference target `#/nowhere/media` was not found in the input \
                 bundle",
            ],
        ),
        (
            "unclassifiable reference",
            at("#header", "#media"),
            [
                "unsupported or unresolved header reference `#header`",
                "unsupported or unresolved Media Type Object reference `#media`",
            ],
        ),
    ] {
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{case}/{entry}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnresolvedRef);
            for message in expected {
                assert!(
                    messages.contains(&message),
                    "{case}/{entry}: expected {message:?} in {messages:#?}"
                );
            }
        }
    }
}

/// A `$ref` to a component schema that was never declared is an error, not a construct to drop
/// quietly. Every construct that reaches `LowerCtx::ensure_component` must report `E004`: before
/// this was pinned, an `application/octet-stream` request body whose schema `$ref`ed a missing
/// component reported `clean` and generated an `upload` method with no body argument at all — a
/// silent degradation with no diagnostic, which the taxonomy forbids. `check` and `generate` must
/// agree on every one of these, and each must point at its own `$ref` site.
#[test]
fn e004_fires_for_a_ref_to_a_component_schema_that_is_not_declared() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";

    // One spec per construct that reaches `ensure_component`. These are NOT one per call site: the
    // four operation-level cases (both request bodies, the response and the parameter) all arrive
    // through `lower_schema_ref`, and the two request bodies are the same path under two media
    // types — the octet-stream one is kept because it is the issue's own reproduction. The cases
    // that do reach distinct sites are the `oneOf` member, the `allOf` member, the component alias,
    // and the `$ref`-with-shape-siblings case, which is the only one that reaches
    // `lower_schema_inner`. The pointers below are what keep the four same-site cases from
    // collapsing into one another.
    let request_body_json = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    post:
      operationId: upload
      requestBody:
        content:
          application/json: { schema: { $ref: '#/components/schemas/Missing' } }
      responses: { '204': { description: ok } }
"##
    );
    // The issue's exact reproduction: this binary request body vanished entirely.
    let request_body_octets = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    post:
      operationId: upload
      requestBody:
        content:
          application/octet-stream: { schema: { $ref: '#/components/schemas/Missing' } }
      responses: { '204': { description: ok } }
"##
    );
    let response_body = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Missing' } }
"##
    );
    let parameter = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    get:
      operationId: getU
      parameters:
        - { name: q, in: query, schema: { $ref: '#/components/schemas/Missing' } }
      responses: { '204': { description: ok } }
"##
    );
    let union_variant = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Union' } }
components:
  schemas:
    Union:
      oneOf:
        - { $ref: '#/components/schemas/Present' }
        - { $ref: '#/components/schemas/Missing' }
    Present:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##
    );
    let all_of_member = format!(
        "{HEAD}{}",
        r##"paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Merged' } }
components:
  schemas:
    Merged:
      allOf:
        - { $ref: '#/components/schemas/Present' }
        - { $ref: '#/components/schemas/Missing' }
    Present:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##
    );
    // A `$ref` carrying shape siblings is an intersection, not an alias, so it is lowered by
    // `lower_schema_inner` rather than by the `RefOr::Ref` arm every other case above takes. It is
    // the only one of these that reaches that site.
    let ref_with_siblings = format!(
        "{HEAD}{}",
        r##"paths:
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
      $ref: '#/components/schemas/Missing'
      type: object
      properties: { extra: { type: string } }
"##
    );
    // A declared component that is itself a bare `$ref` to a missing one: reached from the
    // component-alias arm rather than from any operation.
    let component_alias = format!(
        "{HEAD}{}",
        r##"paths: {}
components:
  schemas:
    Alias: { $ref: '#/components/schemas/Missing' }
"##
    );

    // Each case pairs its spec with the RFC 6901 pointer the diagnostic must carry. The pointer is
    // the assertion that matters: the code alone would still pass if `ensure_component` emitted
    // against the document root, and a root pointer is what makes the rejection un-carvable (see
    // the cascade in `carve.rs`). Pointing at the `$ref` site is the contract, so it is pinned per
    // site rather than left to one coarse outcome elsewhere.
    let cases = [
        (
            "request body (application/json)",
            &request_body_json,
            "/paths/~1u/post/requestBody/content/application~1json/schema",
        ),
        (
            "request body (application/octet-stream)",
            &request_body_octets,
            "/paths/~1u/post/requestBody/content/application~1octet-stream/schema",
        ),
        (
            "response body",
            &response_body,
            "/paths/~1u/get/responses/200/content/application~1json/schema",
        ),
        (
            "parameter schema",
            &parameter,
            "/paths/~1u/get/parameters/0/schema",
        ),
        (
            "oneOf member",
            &union_variant,
            "/components/schemas/Union/oneOf/1",
        ),
        (
            "allOf member",
            &all_of_member,
            "/components/schemas/Merged/allOf/1",
        ),
        (
            "component alias",
            &component_alias,
            "/components/schemas/Alias",
        ),
        (
            "$ref with shape siblings",
            &ref_with_siblings,
            "/components/schemas/Ext",
        ),
    ];

    for (what, spec, pointer) in cases {
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: a `$ref` to an undeclared component must reject\n{report:#?}"
            );
            let e004: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::UnresolvedRef)
                .collect();
            assert!(
                !e004.is_empty(),
                "{what} via {entry}: the rejection must carry E004\n{report:#?}"
            );
            assert!(
                e004.iter().any(|d| d.pointer.as_str() == pointer),
                "{what} via {entry}: E004 must point at the `$ref` site `{pointer}`, not at \
                 {:?}\n{report:#?}",
                e004.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
            );
        }
    }
}

/// A same-file `#/components/schemas/…` fragment that addresses a *subschema* of a declared
/// component rather than a top-level component name. RFC 6901 gives it one meaning whichever file
/// it is written against, and the relative-file spelling (`./lib.yaml#/components/schemas/Envelope/
/// properties/payload`) already resolved through the resolver, so the same-file spelling resolves
/// too: the operation's body is the subschema's type, not a rejection and not a dropped body.
///
/// Before it was rejected, and before that it was silently dropped (`ResponseValue<()>`). The whole
/// test tree held no other `$ref` with a `/` inside the component name, so this fixture is what
/// pins the verdict either way.
#[test]
fn a_same_file_ref_into_a_component_subschema_resolves_like_the_cross_file_spelling() {
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
              schema: { $ref: '#/components/schemas/Envelope/properties/payload' }
components:
  schemas:
    Envelope:
      type: object
      properties:
        payload:
          type: object
          properties: { id: { type: string } }
          required: [id]
"##;
    let (generated, code) = generate_with_code(spec);
    for (entry, report) in [("generate", generated), ("check", check(spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }
    // The body is the subschema's own type, named for the pointer's last token — not `()`, which
    // is what the silent drop emitted, and not `Envelope`, which is the component it starts from.
    assert!(
        code.contains("ResponseValue<types::Payload>"),
        "the operation must return the subschema's type: {}",
        types_module(&code)
    );
    assert_eq!(
        declared_fields(&code, "Payload"),
        ["id"],
        "{}",
        types_module(&code)
    );

    // The relative-file spelling of the same pointer, with `Envelope` declared in `lib.yaml`, is the
    // behaviour the same-file spelling now matches: the same operation type, the same fields.
    let (cross_generated, cross_checked, cross) = split(
        "./lib.yaml#/components/schemas/Envelope/properties/payload",
        &spec[spec.find("components:").unwrap()..],
    );
    for (entry, report) in [("generate", cross_generated), ("check", cross_checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    assert!(
        cross.contains("ResponseValue<types::Payload>"),
        "{}",
        types_module(&cross)
    );
    assert_eq!(
        declared_fields(&cross, "Payload"),
        ["id"],
        "{}",
        types_module(&cross)
    );

    // Both spellings of ONE target in one document share one type. The resolver keys a lowered
    // target on its resolved `file#pointer`, and the root document's own relative-file spelling
    // (`./openapi.yaml#…`) resolves to the same pointer in the same file, so a second `Payload`
    // here would mean the same-file spelling bypassed that identity.
    let mixed = spec.replace(
        "components:\n",
        "  /v:\n    get:\n      operationId: getV\n      responses:\n        '200':\n          \
         description: ok\n          content:\n            application/json:\n              \
         schema: { $ref: './openapi.yaml#/components/schemas/Envelope/properties/payload' }\n\
         components:\n",
    );
    let (report, code) = generate_with_code(&mixed);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        declared_types(&code, "Payload", |tail| !tail.ends_with("id")),
        ["Payload"],
        "the two spellings of one subschema must share one type: {}",
        types_module(&code)
    );
    assert!(code.contains("fn get_v("), "{}", types_module(&code));

    // A subschema that refers back to itself by the same deep pointer, and to the component that
    // encloses it. The resolver's reservation is what makes this finite: the self-reference is a
    // back-edge onto the subschema's own reserved type, and is boxed rather than re-entered.
    let recursive = spec.replace(
        "properties: { id: { type: string } }",
        "properties:\n            id: { type: string }\n            \
         child: { $ref: '#/components/schemas/Envelope/properties/payload' }\n            \
         parent: { $ref: '#/components/schemas/Envelope' }",
    );
    let (generated, code) = generate_with_code(&recursive);
    for (entry, report) in [("generate", generated), ("check", check(&recursive))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }
    let payload_child = code
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with("pub struct Payload "))
        .take_while(|line| !line.starts_with('}'))
        .find(|line| line.starts_with("pub child:"))
        .map(str::to_owned);
    assert_eq!(
        payload_child.as_deref(),
        Some("pub child: Option<Box<Payload>>,"),
        "{}",
        types_module(&code)
    );
    assert_eq!(
        declared_fields(&code, "Payload"),
        ["id", "child", "parent"],
        "{}",
        types_module(&code)
    );

    // The same pointer as an `allOf` member contributes the subschema's fields, as a component
    // member contributes the component's.
    let all_of = spec.replace(
        "schema: { $ref: '#/components/schemas/Envelope/properties/payload' }",
        "schema:\n                allOf:\n                  \
         - { $ref: '#/components/schemas/Envelope/properties/payload' }\n                  \
         - { type: object, properties: { extra: { type: string } } }",
    );
    let (report, code) = generate_with_code(&all_of);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let merged = field_owner(&code, "pub extra:").expect("the sibling's field is emitted");
    assert_eq!(
        declared_fields(&code, &merged),
        ["id", "extra"],
        "the member's `id` and the sibling's `extra` must merge into one type: {}",
        types_module(&code)
    );

    // And as a union member it is a member like the relative-file spelling is: lowered through the
    // resolver to the same `Payload`, and — being a pointer into a component rather than a component
    // name — deriving no component name for the variant, as `./lib.yaml#…` derives none. Taking
    // `Envelope/properties/payload` as a name would leak the pointer into a variant identifier and
    // an implicit discriminator tag.
    let one_of = spec.replace(
        "schema: { $ref: '#/components/schemas/Envelope/properties/payload' }",
        "schema:\n                oneOf:\n                  \
         - { $ref: '#/components/schemas/Envelope/properties/payload' }\n                  \
         - { type: string }",
    );
    let (report, code) = generate_with_code(&one_of);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    assert!(code.contains("(Box<Payload>)"), "{}", types_module(&code));
    assert!(
        !code.contains("EnvelopePropertiesPayload"),
        "{}",
        types_module(&code)
    );
    let cross_one_of = one_of.replace(
        "'#/components/schemas/Envelope/properties/payload'",
        "'./openapi.yaml#/components/schemas/Envelope/properties/payload'",
    );
    let (report, cross_code) = generate_with_code(&cross_one_of);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        types_module(&code),
        types_module(&cross_code),
        "the two spellings of one union member must lower identically"
    );

    // A pointer that starts at a declared component but walks off its body is the resolver's
    // miss, reported against the `$ref` site with the whole reference — not the old "addresses a
    // subschema" wording, which would now claim a restriction that no longer exists.
    let off_body = spec.replace(
        "#/components/schemas/Envelope/properties/payload",
        "#/components/schemas/Envelope/properties/nope",
    );
    for (entry, report) in [
        ("generate", generate(&off_body)),
        ("check", check(&off_body)),
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e004: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .collect();
        assert!(
            e004.iter().any(|d| d.message.contains(
                "`#/components/schemas/Envelope/properties/nope` was not found in the input bundle"
            ) && d.pointer.as_str()
                == "/paths/~1u/get/responses/200/content/application~1json/schema"),
            "{entry}: {report:#?}"
        );
        assert!(
            !e004
                .iter()
                .any(|d| d.message.contains("addresses a subschema")),
            "{entry}: {report:#?}"
        );
    }

    // A deep pointer whose ROOT SEGMENT is not declared is a missing component, and keeps the
    // plain-name wording that names the whole reference. `Envelop` is a typo for `Envelope`; the
    // `/` in the fragment is not what is wrong with it.
    let typo = spec.replace(
        "#/components/schemas/Envelope/properties/payload",
        "#/components/schemas/Envelop/properties/payload",
    );
    for (entry, report) in [("generate", generate(&typo)), ("check", check(&typo))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e004: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .collect();
        assert!(
            e004.iter().any(|d| d.message.contains(
                "unresolved schema reference `#/components/schemas/Envelop/properties/payload`"
            )),
            "{entry}: an undeclared root segment is an unresolved reference, not a fragment-shape \
             problem: {report:#?}"
        );
    }

    // A trailing slash is a real pointer into `Envelope`: its final empty reference token
    // addresses a `""`-keyed member, which `Envelope` does not have. So it reaches the resolver,
    // and the resolver's miss is what reports it.
    let trailing = spec.replace(
        "#/components/schemas/Envelope/properties/payload",
        "#/components/schemas/Envelope/",
    );
    let report = generate(&trailing);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::UnresolvedRef
                && d.message.contains(
                    "`#/components/schemas/Envelope/` was not found in the input bundle"
                )),
        "{report:#?}"
    );

    // Control: the plain undeclared-name case keeps the "unresolved" wording, so the branch above
    // is a genuine split rather than a blanket rewording.
    let plain = spec.replace(
        "#/components/schemas/Envelope/properties/payload",
        "#/components/schemas/Missing",
    );
    let report = generate(&plain);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // The whole reference, so the message identifies WHICH component is missing — a pointer says
    // where the `$ref` is, not what it named.
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::UnresolvedRef
                && d.message
                    .contains("unresolved schema reference `#/components/schemas/Missing`")),
        "{report:#?}"
    );
}

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
/// `schema_is_nullable` reads only `type`, `enum` and `const`, so for `oneOf: [null, string]` it
/// answers `false`, while lowering the union answers `true`. `ensure_component` stopped writing
/// that provisional answer back over the body's; `ensure_resolved` still did, and cached it, so
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
/// back-edge against an in-progress reservation that kind is the placeholder `TypeKind::Any` the
/// reservation was created with — **the id is right, the kind is not, and only the kind is read** —
/// so the member was classified `Contribution::Scalar` and the whole property collapsed to
/// `serde_json::Value` with **no diagnostic at all**. `CLAUDE.md` names that exact outcome: generated
/// code "never silently degrades a typed schema to `serde_json::Value`", and every construct is
/// "supported, warned, or rejected — no fourth, silent behavior".
///
/// The root document has always refused to read an in-progress member and rejected with `E013`.
/// The remote path had the same pre-check but not the same coverage, and its id-keyed guard is this
/// branch's own addition — see `remote::a_direct_recursive_all_of_member_in_a_vendored_document_is_rejected`,
/// which is the fixture that guard did not have. All three spellings here must reach that same
/// rejection: the bare sub-file name, the explicit file reference, and the root-document control.
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

/// The *file* half of the identity key. Two different files each declaring a schema by the same name
/// must stay two types: the key is `file#pointer`, and dropping the file component would collapse
/// them onto one — which is a wrongly *shared* type, the failure `ensure_resolved`'s own fallback
/// comment calls worse than a duplicated one.
///
/// Nothing pinned this but an incidental corpus snapshot, which would report the collapse as a type
/// count and not as a wrong type.
#[test]
fn two_files_declaring_the_same_component_name_stay_two_types() {
    let code = generate_two_file_shapes(false);

    // Two declarations in two files: two types, and each keeps its own field. A collapse would take
    // one of these fields with it.
    let shapes = declared_types(&code, "Shape", |_| true);
    assert_eq!(
        shapes.len(),
        2,
        "two files declare `Shape`; they are different schemas and must stay two types: {shapes:?}"
    );
    // Each kept its own field. A collapse onto one key would have taken one of these with it, which
    // is the wrongly-*shared* type `ensure_resolved`'s fallback comment calls worse than a
    // duplicated one.
    assert!(code.contains("pub alpha:"), "{code}");
    assert!(code.contains("pub beta:"), "{code}");

    // *Which* struct owns which field, not merely that both exist somewhere. Counting types and
    // checking for both fields passes under either assignment of the two names, so on its own it
    // says nothing about what `types::Shape` denotes — and what `types::Shape` denotes is a public
    // API fact a consumer writes into their own code. The contest is decided by each schema's own
    // `file#pointer`, so `a.yaml`'s declaration keeps the bare name; that it survives reordering is
    // `a_contested_type_name_survives_reordering_the_paths_that_reach_it`'s to prove.
    assert_eq!(
        field_owner(&code, "pub alpha:").as_deref(),
        Some("Shape"),
        "the lower `file#pointer` owns the un-suffixed name: {code}"
    );
    assert_eq!(
        field_owner(&code, "pub beta:").as_deref(),
        Some("Shape93360b5f"),
        "and the other carries the pointer-seeded disambiguator: {code}"
    );
}

/// Issue #169: which of two same-named schemas keeps the bare type name was decided by the order
/// lowering met them in, so swapping two `paths` entries — no schema changed — swapped what
/// `types::Shape` denotes, a rename of public items. The name is now awarded on each schema's own
/// `file#pointer`, so both orders must emit the same types module with the same owner per field.
#[test]
fn a_contested_type_name_survives_reordering_the_paths_that_reach_it() {
    let forward = types_module(&generate_two_file_shapes(false));
    let swapped = types_module(&generate_two_file_shapes(true));

    for field in ["pub alpha:", "pub beta:"] {
        assert_eq!(
            field_owner(&forward, field),
            field_owner(&swapped, field),
            "reordering `paths` moved `{field}` to another type"
        );
    }
    assert_eq!(
        field_owner(&swapped, "pub alpha:").as_deref(),
        Some("Shape")
    );
    assert_eq!(
        declared_fields(&forward, "Shape"),
        declared_fields(&swapped, "Shape")
    );
    assert_eq!(
        declared_fields(&forward, "Shape93360b5f"),
        declared_fields(&swapped, "Shape93360b5f")
    );
}

/// A definition with no identity of its own — a boolean-schema `false` property, which lowers to a
/// `Never` type whose provenance falls back to the root document's own (empty) pointer — must never
/// take a contested name from a declared schema. The empty pointer is the lowest a plain ordering
/// could produce, so without the `anonymous` term of the rank the synthesized `Holder x` would take
/// `Holderx` from the component declared under that name. Both component orders are generated, so
/// arrival order cannot be what decides it.
#[test]
fn a_synthesized_type_with_no_identity_never_takes_a_name_from_a_declared_schema() {
    const HOLDER: &str = "    Holder:\n      type: object\n      properties:\n        x: false\n";
    const DECLARED: &str = "    Holderx:\n      type: object\n      required: [declared]\n      \
                            properties: { declared: { type: string } }\n";
    for (first, second) in [(HOLDER, DECLARED), (DECLARED, HOLDER)] {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\npaths: {{}}\n\
             components:\n  schemas:\n{first}{second}"
        ));
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let types = types_module(&code);
        assert_eq!(
            field_owner(&types, "pub declared:").as_deref(),
            Some("Holderx"),
            "the declared schema keeps the bare name: {types}"
        );
        assert!(
            types.contains("pub enum Holderx84222325 {}"),
            "and the synthesized `Never` carries the root pointer's disambiguator: {types}"
        );
    }
}

/// A root document whose two operations reach a `Shape` declared in `a.yaml` and another declared
/// in `b.yaml`, listed `/a` first unless `b_first`; returns the generated client.
fn generate_two_file_shapes(b_first: bool) -> String {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let refs = [
        ("a", "./a.yaml#/components/schemas/Shape"),
        ("b", "./b.yaml#/components/schemas/Shape"),
    ];
    std::fs::write(dir.join("openapi.yaml"), two_shape_root(refs, b_first)).unwrap();
    std::fs::write(dir.join("a.yaml"), SHAPE_ALPHA_YAML).unwrap();
    std::fs::write(dir.join("b.yaml"), SHAPE_BETA_YAML).unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    std::fs::read_to_string(&out).unwrap()
}

/// A document declaring a `Shape` whose one field is `alpha`.
const SHAPE_ALPHA_YAML: &str = "components:\n  schemas:\n    Shape:\n      type: object\n      \
                                required: [alpha]\n      properties: { alpha: { type: string } }\n";
/// A document declaring a `Shape` whose one field is `beta`.
const SHAPE_BETA_YAML: &str = "components:\n  schemas:\n    Shape:\n      type: object\n      \
                               required: [beta]\n      properties: { beta: { type: integer } }\n";

/// A root document with one operation per `(path, $ref)` in `refs`, each answering `200` with the
/// schema the `$ref` names; the second entry is listed first when `swap`.
fn two_shape_root(refs: [(&str, &str); 2], swap: bool) -> String {
    let path_item = |(letter, reference): (&str, &str)| {
        format!(
            "  /{letter}:\n    get:\n      operationId: get{upper}\n      responses:\n        \
             '200':\n          description: ok\n          content:\n            \
             application/json: {{ schema: {{ $ref: '{reference}' }} }}\n",
            upper = letter.to_uppercase(),
        )
    };
    let [first, second] = refs;
    let (first, second) = if swap {
        (second, first)
    } else {
        (first, second)
    };
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\n\
         servers: [{{ url: 'https://e.com' }}]\npaths:\n{}{}",
        path_item(first),
        path_item(second),
    )
}

/// A document reached through `..`, outside the root document's directory, is ranked by its full
/// loaded path, which begins with the filesystem root and so sorts before any root-relative spelling.
/// `z.yaml` sits outside and `a.yaml` inside: a key spelled by file name alone would hand the bare
/// `Shape` to `a.yaml`, and one spelled by discovery order would move it when `paths` is reordered.
/// Both orders must give it to the outside file.
#[test]
fn a_contested_type_name_ranks_a_file_outside_the_root_directory_by_its_full_loaded_path() {
    for swap in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let root_dir = dir.join("spec");
        std::fs::create_dir_all(&root_dir).unwrap();
        let refs = [
            ("a", "./a.yaml#/components/schemas/Shape"),
            ("z", "../z.yaml#/components/schemas/Shape"),
        ];
        std::fs::write(root_dir.join("openapi.yaml"), two_shape_root(refs, swap)).unwrap();
        std::fs::write(root_dir.join("a.yaml"), SHAPE_BETA_YAML).unwrap();
        std::fs::write(dir.join("z.yaml"), SHAPE_ALPHA_YAML).unwrap();
        let out = dir.join("client.rs");
        let report = run_generate(&build(root_dir.join("openapi.yaml"), out.clone()));
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let code = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            field_owner(&code, "pub alpha:").as_deref(),
            Some("Shape"),
            "swap={swap}: the outside file's full path ranks first: {code}"
        );
        assert_eq!(
            field_owner(&code, "pub beta:").as_deref(),
            Some("Shape93360b5f"),
            "swap={swap}: {code}"
        );
    }
}

/// The nullability half of the memo entry. A shared sub-file component whose own schema admits
/// `null` must reach every use site as `Option<T>`; the memo stores nullability beside the id
/// precisely so a cache hit and a first use agree on it.
///
/// Forcing it false emits `pub maybe: Maybe` where `Option<Maybe>` is correct — a field that rejects
/// a payload the schema declares as valid — and nothing else in the suite is red.
#[test]
fn a_nullable_sub_file_component_reaches_every_use_as_an_option() {
    let (generated, checked, code) = split(
        "./lib.yaml#/components/schemas/Holder",
        r##"
components:
  schemas:
    Holder:
      type: object
      required: [first, second]
      properties:
        first: { $ref: '#/components/schemas/Maybe' }
        second: { $ref: '#/components/schemas/Maybe' }
    Maybe:
      type: [string, 'null']
"##,
    );
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    // `required` on both, so the `Option` can only come from the component's own nullability — and
    // it must come through on the second use (a memo hit) exactly as on the first.
    assert!(
        code.contains("pub first: Option<Maybe>"),
        "the first use must carry the component's nullability: {code}"
    );
    assert!(
        code.contains("pub second: Option<Maybe>"),
        "and so must the memo hit: {code}"
    );
}

/// Decision 23's invariant, applied to the sub-file spelling.
///
/// Every spelling now asks the DOCUMENT whether a `$ref` closes a cycle through a schema enclosing
/// it, so the verdict is order-independent by construction. With siblings on both edges it was
/// order-independent even when the sub-file spelling still asked `is_in_progress_root`: the
/// reservation is made *before* the body is lowered, so whichever of a mutually recursive pair is
/// lowered first, the re-entrant edge meets an open reservation. This fixture writes the same two
/// schemas in both orders and requires one verdict; the one-edge fixture below is the case that
/// lowering-state question got wrong.
#[test]
fn a_sub_file_cycle_verdict_does_not_depend_on_the_order_the_schemas_are_declared() {
    const A: &str = r##"    A:
      type: [object, 'null']
      properties:
        b:
          $ref: './lib.yaml#/components/schemas/B'
          type: [object, 'null']
          properties:
            extra: { type: string }
"##;
    // Siblings on BOTH back-edges, so the conjunction meets an open reservation whichever schema is
    // entered first. Siblings on one edge only are the harder case — whether the conjunction meets
    // one then depends on the entry point — and are pinned separately below.
    const B: &str = r##"    B:
      type: [object, 'null']
      properties:
        a:
          $ref: './lib.yaml#/components/schemas/A'
          type: [object, 'null']
          properties:
            extra: { type: string }
"##;

    let mut verdicts = Vec::new();
    for (order, lib) in [
        ("A first", format!("components:\n  schemas:\n{A}{B}")),
        ("B first", format!("components:\n  schemas:\n{B}{A}")),
    ] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/A", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{order}/{entry}: {report:#?}"
            );
            assert!(
                has_code(report, Code::AllOfIrreconcilable),
                "{order}/{entry}: {report:#?}"
            );
        }
        assert!(
            !code.contains("= ();"),
            "{order}: the failed intersection collapsed to the exact JSON null type: {code}"
        );
        // The outcome alone is held up by `intersect_types`' fail-closed arm, which rejects with
        // the generic empty-intersection wording whatever the back-edge predicate answered. Naming
        // the cause is what requires the predicate itself to be right, so it is asserted here too.
        let messages = messages_for(&generated, Code::AllOfIrreconcilable);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("closes a reference cycle")),
            "{order}: the rejection must name the recursion as the cause: {messages:?}"
        );
        verdicts.push((
            order,
            messages_for(&generated, Code::AllOfIrreconcilable)
                .iter()
                .map(|m| (*m).to_owned())
                .collect::<Vec<_>>(),
        ));
    }
    // Not merely the same outcome — the same diagnosis. Reordering a YAML map is a no-op in
    // OpenAPI, so it may not change what the tool says about the document either.
    assert_eq!(
        verdicts[0].1, verdicts[1].1,
        "reordering two sub-file schemas changed the diagnosis: {verdicts:?}"
    );
}

/// A sub-file component that shares its NAME with a root component in a cycle is not in that cycle.
///
/// The root declares `Item` and `Other`, each referring to the other. `lib.yaml` declares its own
/// `Item`, whose property carries siblings beside a `$ref` to the root's `Other`. Nothing reaches
/// `lib.yaml`'s `Item` from `Other`, so there is no cycle through it and the intersection is an
/// ordinary one. The back-edge test used to take the enclosing name from the sub-file pointer and
/// look it up in the ROOT document's map, where `Other` does reach an `Item` — the root's — and so
/// it rejected with `E013`. Renaming the sub-file component is the control: the document means the
/// same thing under either name and must get the same verdict.
#[test]
fn a_sub_file_component_named_like_a_root_cycle_member_is_not_a_back_edge() {
    for name in ["Item", "Leaf"] {
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
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: './lib.yaml#/components/schemas/{name}' }} }}
  /r:
    get:
      operationId: getR
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Item' }} }}
components:
  schemas:
    Item:
      type: object
      properties:
        other: {{ $ref: '#/components/schemas/Other' }}
    Other:
      type: object
      properties:
        item: {{ $ref: '#/components/schemas/Item' }}
"##
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("lib.yaml"),
            format!(
                r##"
components:
  schemas:
    {name}:
      type: object
      properties:
        o:
          $ref: '#/components/schemas/Other'
          type: object
          properties:
            extra: {{ type: string }}
"##
            ),
        )
        .unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
        for (run, report) in [("generate", &generated), ("check", &checked)] {
            assert!(
                !has_code(report, Code::AllOfIrreconcilable),
                "{name}/{run}: no cycle passes through the sub-file component: {report:#?}"
            );
            assert!(report.outcome().is_success(), "{name}/{run}: {report:#?}");
        }
        assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
        let code = std::fs::read_to_string(&out).unwrap();
        assert!(
            code.contains("pub extra:"),
            "{name}: the sibling's property must survive the intersection: {code}"
        );
    }
}

/// A sub-file schema that reaches a **root document** component twice, by explicit file reference.
///
/// `ensure_resolved` routes a resolved target that lands inside the root document's own component
/// map back through `ensure_component`, so `components` stays that target's single identity. That
/// routing can look like code that executes but constrains nothing, and without this test it is:
/// disabling it leaves every other suite green and gives **`["RootOne", "RootOne55e60dbe"]`** —
/// two public types for one declared component, which is the precise defect the resolved-reference
/// memo exists to prevent.
///
/// The reason a second memo is not harmless is that it is a second *identity*: `resolved_components`
/// would key the same schema by `file#pointer` while `components` keys it by name, and neither would
/// see the other's entry.
#[test]
fn a_root_component_reached_by_file_reference_keeps_the_root_map_as_its_identity() {
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
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Holder' } }
components:
  schemas:
    RootOne:
      type: object
      required: [id]
      properties: { id: { type: string } }
"##,
    )
    .unwrap();
    // Both properties address the root document's `RootOne` from inside the sub-file, spelled as a
    // file reference — the only spelling that reaches the routing branch.
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Holder:
      type: object
      required: [first, second]
      properties:
        first: { $ref: './openapi.yaml#/components/schemas/RootOne' }
        second: { $ref: './openapi.yaml#/components/schemas/RootOne' }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();

    let roots = declared_types(&code, "RootOne", |_| true);
    assert_eq!(
        roots.len(),
        1,
        "one declared component, one generated type — a resolved reference that lands on a root \
         component must not take a second identity beside the root map: {roots:?}"
    );
    // Both uses reached it, so the count is not met by losing one of them.
    assert!(code.contains("pub first: RootOne"), "{code}");
    assert!(code.contains("pub second: RootOne"), "{code}");
}

/// The fan-out bound for an `allOf` member addressed by **file reference** (issue #163).
///
/// Such a member is not given a type of its own: `allOf` is an applicator, so the target's fields
/// are flattened into the enclosing object. That arm used to re-expand the target at every use, so
/// a branching reuse graph whose edges are `allOf: [$ref]` generated 2^(DEPTH+1) - 1 types — 2047
/// here, 32,767 at depth 14 — from DEPTH + 1 declared schemas, with no diagnostic. The target's
/// contribution is now expanded once per resolved `file#pointer` and replayed at every later use.
///
/// The exact count is one type for the response's own target plus one per `allOf` property site
/// per *declaration* — `L0`..`L(DEPTH-1)` each declare two — which is `2 * DEPTH + 1`. The shape
/// of every type is pinned too, so the bound cannot be met by dropping what a member contributes:
/// flattening still copies the target's fields into each enclosing struct.
#[test]
fn a_file_referenced_all_of_member_reused_through_a_deep_graph_is_expanded_once() {
    const DEPTH: usize = 10;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..DEPTH {
        lib.push_str(&format!(
            "    L{level}:\n      type: object\n      properties:\n        a: {{ allOf: [{{ $ref: \
             './lib.yaml#/components/schemas/L{next}' }}] }}\n        b: {{ allOf: [{{ $ref: \
             './lib.yaml#/components/schemas/L{next}' }}] }}\n",
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
    let types = types_module(&code);
    // Every lowered type here is named from an `L<digit>` hint; the client scaffolding that
    // `types_module` also carries is not.
    let declared = declared_types(&types, "L", |tail| {
        tail.starts_with(|character: char| character.is_ascii_digit())
    });
    assert_eq!(
        declared.len(),
        2 * DEPTH + 1,
        "{} declared schemas generated {} types: {declared:?}",
        DEPTH + 1,
        declared.len()
    );
    // Every generated struct is either an interior level (`a` and `b`, flattened from the member) or
    // the leaf level (`id`), so each wrapper carries exactly what its target contributes.
    let leaves = declared
        .iter()
        .filter(|ty| declared_fields(&types, ty) == ["id"])
        .count();
    let interior = declared
        .iter()
        .filter(|ty| declared_fields(&types, ty) == ["a", "b"])
        .count();
    assert_eq!(
        (interior, leaves),
        (2 * DEPTH - 1, 2),
        "every type must carry its target's flattened fields: {declared:?}"
    );
}

/// A replayed contribution is the member's, not the enclosing composition's.
///
/// Expanding a file-referenced `allOf` member once and replaying it is only sound if nothing the
/// enclosing `allOf` does to the merged fields — promoting a field to required, intersecting a
/// repeated property — leaks back into what the member contributes to the next use. `Strict`
/// requires `id` beside the member and `Loose` does not, so the two uses of one member must differ
/// exactly there, in whichever order they are lowered.
#[test]
fn a_file_referenced_all_of_member_contributes_the_same_fields_to_every_use() {
    let lib = r##"
components:
  schemas:
    Holder:
      type: object
      required: [strict, loose]
      properties:
        strict: { $ref: './lib.yaml#/components/schemas/Strict' }
        loose: { $ref: './lib.yaml#/components/schemas/Loose' }
    Strict:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Base'
        - required: [id]
    Loose:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Base'
        - properties: { note: { type: string } }
    Base:
      type: object
      properties: { id: { type: string } }
"##;
    let strict_first =
        "        strict: { $ref: './lib.yaml#/components/schemas/Strict' }\n        \
                        loose: { $ref: './lib.yaml#/components/schemas/Loose' }\n";
    let loose_first = "        loose: { $ref: './lib.yaml#/components/schemas/Loose' }\n        \
                       strict: { $ref: './lib.yaml#/components/schemas/Strict' }\n";
    assert!(lib.contains(strict_first));
    for order in [lib.to_owned(), lib.replace(strict_first, loose_first)] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &order);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        let types = types_module(&code);
        assert_eq!(declared_fields(&types, "Strict"), ["id"], "{types}");
        assert_eq!(declared_fields(&types, "Loose"), ["id", "note"], "{types}");
        let strict_id = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with("pub struct Strict "))
            .find(|line| line.starts_with("pub id:"))
            .map(str::to_owned);
        let loose_id = types
            .lines()
            .map(str::trim_start)
            .skip_while(|line| !line.starts_with("pub struct Loose "))
            .find(|line| line.starts_with("pub id:"))
            .map(str::to_owned);
        let strict_id = strict_id.expect("`Strict` declares `id`");
        let loose_id = loose_id.expect("`Loose` declares `id`");
        let inner = strict_id
            .strip_prefix("pub id: ")
            .and_then(|ty| ty.strip_suffix(','))
            .expect("a field line");
        assert!(
            !inner.starts_with("Option<"),
            "`Strict` requires `id`: {types}"
        );
        assert_eq!(
            loose_id,
            format!("pub id: Option<{inner}>,"),
            "`Strict`'s requirement must not leak into `Loose` through the shared member, and \
             both uses must name the one type the member's property lowered to: {types}"
        );
    }
}

/// A file-referenced `allOf` member whose target is itself an `allOf`, or a bare `$ref` alias,
/// contributes what that target constrains (issue #306).
///
/// The member's target used to be read only for object keywords and scalar keywords. A body that
/// is `allOf: [...]` or a bare `$ref` carries neither, so it was taken for a pure annotation and
/// contributed nothing: `Holder` generated as an empty struct, both required properties gone, and
/// no diagnostic. The root-component spelling of the same shape always lowered the target first,
/// so the two spellings are held to the same fields here.
#[test]
fn a_file_referenced_all_of_member_whose_target_is_an_all_of_or_an_alias_contributes_its_fields() {
    const LIB: &str = r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: 'PREFIX#/components/schemas/Inner'
        - $ref: 'PREFIX#/components/schemas/Alias'
        - $ref: 'PREFIX#/components/schemas/Narrowed'
    Inner:
      allOf:
        - { type: object, required: [x], properties: { x: { type: string } } }
    Alias: { $ref: 'PREFIX#/components/schemas/Other' }
    Other: { type: object, required: [y], properties: { y: { type: integer } } }
    Narrowed:
      $ref: 'PREFIX#/components/schemas/Loose'
      required: [z]
    Loose: { type: object, properties: { z: { type: boolean } } }
"##;
    for (spelling, prefix) in [("explicit", "./lib.yaml"), ("bare", "")] {
        let lib = LIB.replace("PREFIX", prefix);
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().is_empty(),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        let types = types_module(&code);
        assert_eq!(
            declared_fields(&types, "Holder"),
            ["x", "y", "z"],
            "{spelling}: every member's target must contribute its properties: {types}"
        );
        // Each is required — by the `allOf` target, by the alias's target, and by the alias's own
        // `required` sibling — so none may be optional. The bare spelling lowers `Narrowed` to a
        // type of its own first, and that path still drops a `$ref`'s bare `required` sibling
        // (#140), so `z`'s requirement is asserted only where the target is flattened.
        let required: &[&str] = if prefix.is_empty() {
            &["x", "y"]
        } else {
            &["x", "y", "z"]
        };
        let holder: String = types
            .lines()
            .skip_while(|line| !line.trim_start().starts_with("pub struct Holder "))
            .take_while(|line| !line.trim_start().starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        for field in required {
            let ty = field_type(&holder, &format!("pub {field}:")).unwrap_or_default();
            assert!(
                !ty.is_empty() && !ty.starts_with("Option<"),
                "{spelling}: `{field}` is required: {types}"
            );
        }
    }
}

/// Expanding a file-referenced `allOf` member's target through its own `$ref` and `allOf` gives the
/// expansion a way back to where it started, and nothing reserves a type along the way. Every such
/// loop is rejected as the cycle it is, in the code the same loop reports elsewhere: a chain of bare
/// aliases is `E004`'s alias cycle, as `ensure_resolved` reports it; a loop through an `allOf` body
/// is `E013`'s recursive member, as the root document reports it. None may walk to the depth cap
/// (`E014`), and none may generate.
#[test]
fn a_file_referenced_all_of_member_that_reaches_itself_is_rejected_as_a_cycle() {
    let cases = [
        (
            "alias cycle",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: './lib.yaml#/components/schemas/B' }
    B: { $ref: './lib.yaml#/components/schemas/A' }
"##,
            Code::UnresolvedRef,
        ),
        (
            "allOf of itself",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Inner'
    Inner:
      allOf:
        - $ref: './lib.yaml#/components/schemas/Inner'
"##,
            Code::AllOfIrreconcilable,
        ),
        (
            "alias into an allOf of the alias",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: './lib.yaml#/components/schemas/B' }
    B:
      allOf:
        - { type: object, properties: { x: { type: string } } }
        - $ref: './lib.yaml#/components/schemas/A'
"##,
            Code::AllOfIrreconcilable,
        ),
        // The same loop with the alias spelled as the sub-file's own bare component name. That
        // link reaches `B` through the component arm, which lowers `B` to a reserved type rather
        // than flattening it, so the loop still passes through `B`'s body and is `E013`, not an
        // alias cycle.
        (
            "bare alias into an allOf of the alias",
            r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: './lib.yaml#/components/schemas/A'
    A: { $ref: '#/components/schemas/B' }
    B:
      allOf:
        - { type: object, properties: { x: { type: string } } }
        - $ref: './lib.yaml#/components/schemas/A'
"##,
            Code::AllOfIrreconcilable,
        ),
    ];
    for (shape, lib, expected) in cases {
        let (generated, checked, _) = split("./lib.yaml#/components/schemas/Holder", lib);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{shape}/{entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| d.code == expected
                    && (d.message.contains("cycle") || d.message.contains("recursive"))),
                "{shape}/{entry}: the rejection must be {expected:?} naming the cycle: {report:#?}"
            );
            assert!(
                !has_code(report, Code::SchemaNestingTooDeep),
                "{shape}/{entry}: a loop is a cycle, not a deep chain: {report:#?}"
            );
            if expected != Code::UnresolvedRef {
                assert!(
                    !has_code(report, Code::UnresolvedRef),
                    "{shape}/{entry}: a loop through a body is not an alias cycle: {report:#?}"
                );
            }
        }
    }
}

/// A file-referenced `allOf` member whose expansion lowers a property that refers back to that
/// member is an ordinary recursive type, not a member recursive through its own composition.
///
/// `Holder`'s member `Node` flattens `NodeBase`, whose `children` items refer to `Node`. Lowering
/// those items gives `Node` a type of its own, with its own reservation, and that lowering flattens
/// `NodeBase` again. It is a new type, so re-entering `NodeBase` there is not a loop of the outer
/// expansion. The root-document spelling of the same shape always generated, and every spelling is
/// held to it here.
#[test]
fn a_file_referenced_all_of_member_with_a_property_back_edge_generates() {
    const LIB: &str = r##"
components:
  schemas:
    Holder:
      allOf:
        - $ref: 'PREFIX#/components/schemas/Node'
    Node:
      allOf:
        - $ref: 'PREFIX#/components/schemas/NodeBase'
    NodeBase:
      type: object
      properties:
        children:
          type: array
          items: { $ref: 'PREFIX#/components/schemas/Node' }
"##;
    let mut runs = Vec::new();
    for (spelling, prefix) in [("explicit", "./lib.yaml"), ("bare", "")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Holder",
            &LIB.replace("PREFIX", prefix),
        );
        runs.push((spelling, generated, checked, code));
    }
    let root = format!(
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
         application/json:\n              \
         schema: {{ $ref: '#/components/schemas/Holder' }}\n{}",
        LIB.replace("PREFIX", "").trim_start_matches('\n')
    );
    let (generated, code) = generate_with_code(&root);
    runs.push(("root", generated, check(&root), code));
    for (spelling, generated, checked, code) in &runs {
        for (entry, report) in [("generate", generated), ("check", checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a property back-edge is recursion a type boxes: {report:#?}"
            );
            assert!(
                !has_code(report, Code::AllOfIrreconcilable),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        assert_eq!(
            declared_fields(&types_module(code), "Holder"),
            ["children"],
            "{spelling}: `Holder` carries `NodeBase`'s property: {code}"
        );
    }
}

/// A long, acyclic chain of file-referenced `allOf` members through alias and `allOf` targets is
/// bounded by the depth cap, as a long `$ref` chain is, rather than by the stack.
#[test]
fn a_long_file_referenced_all_of_member_chain_still_exceeds_the_depth_cap() {
    let depth = 200;
    let mut lib = String::from("components:\n  schemas:\n");
    for level in 0..depth {
        let next = format!("./lib.yaml#/components/schemas/L{}", level + 1);
        if level % 2 == 0 {
            lib.push_str(&format!("    L{level}: {{ $ref: '{next}' }}\n"));
        } else {
            lib.push_str(&format!(
                "    L{level}: {{ allOf: [{{ $ref: '{next}' }}] }}\n"
            ));
        }
    }
    lib.push_str(&format!(
        "    L{depth}:\n      type: object\n      properties: {{ id: {{ type: string }} }}\n"
    ));
    lib.push_str("    Holder: { allOf: [{ $ref: './lib.yaml#/components/schemas/L0' }] }\n");
    let (generated, checked, _) = split("./lib.yaml#/components/schemas/Holder", &lib);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(report, Code::SchemaNestingTooDeep),
            "{entry}: {report:#?}"
        );
    }
}

/// One schema that is both a file-referenced `allOf` member and a direct `$ref` is lowered twice —
/// once to its own type by `ensure_resolved`, once flattened as a member — and the two copies must
/// not compete for one name.
///
/// The member's body is named for the target, so a hint equal to the one `ensure_resolved` gives
/// the direct type would put two types on one name, and the naming scope hands the bare name to
/// whichever lowering reaches it first. Reordering two properties would then move `Basemeta` (and,
/// for a scalar target, `Code` itself) from the direct type to the member's copy. Both orders must
/// generate the same set of names, with the direct type and its nested types on the bare ones.
#[test]
fn a_schema_used_as_file_referenced_all_of_member_and_direct_ref_names_both_copies_stably() {
    let lib = r##"
components:
  schemas:
    Holder:
      type: object
      properties:
        direct: { $ref: './lib.yaml#/components/schemas/Base' }
        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }
        code: { $ref: './lib.yaml#/components/schemas/Code' }
        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }
    Base:
      type: object
      properties:
        meta: { type: object, properties: { tag: { type: string } } }
    Code:
      type: string
      enum: [a, b]
"##;
    let direct_first = "        direct: { $ref: './lib.yaml#/components/schemas/Base' }\n        \
                        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }\n        \
                        code: { $ref: './lib.yaml#/components/schemas/Code' }\n        \
                        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }\n";
    let member_first = "        wrappedCode: { allOf: [{ $ref: './lib.yaml#/components/schemas/Code' }] }\n        \
                        wrapped: { allOf: [{ $ref: './lib.yaml#/components/schemas/Base' }] }\n        \
                        code: { $ref: './lib.yaml#/components/schemas/Code' }\n        \
                        direct: { $ref: './lib.yaml#/components/schemas/Base' }\n";
    assert!(lib.contains(direct_first));
    let mut names = Vec::new();
    for order in [lib.to_owned(), lib.replace(direct_first, member_first)] {
        let (generated, checked, code) = split("./lib.yaml#/components/schemas/Holder", &order);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        let types = types_module(&code);
        let field_type = |owner: &str, field: &str| {
            types
                .lines()
                .map(str::trim_start)
                .skip_while(|line| !line.starts_with(&format!("pub struct {owner} ")))
                .take_while(|line| !line.starts_with('}'))
                .find_map(|line| line.strip_prefix(&format!("pub {field}: ")))
                .map(|ty| ty.trim_end_matches(',').to_owned())
        };
        assert_eq!(
            field_type("Holder", "direct").as_deref(),
            Some("Option<Base>"),
            "{types}"
        );
        assert_eq!(
            field_type("Base", "meta").as_deref(),
            Some("Option<Basemeta>"),
            "{types}"
        );
        assert_eq!(
            field_type("Holder", "code").as_deref(),
            Some("Option<Code>"),
            "{types}"
        );
        let wrapped_meta = field_type("Holderwrapped", "meta").expect("the member's `meta`");
        assert_ne!(wrapped_meta, "Option<Basemeta>", "{types}");
        let mut declared: Vec<String> = types
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                line.strip_prefix("pub struct ")
                    .or_else(|| line.strip_prefix("pub enum "))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .map(str::to_owned)
            .collect();
        declared.sort();
        names.push((declared, wrapped_meta, field_type("Holder", "wrappedCode")));
    }
    assert_eq!(names[0], names[1], "lowering order renamed a type");
}

/// A sub-file's own `#/components/schemas/<name>` when the **root** document declares that name
/// too. The root wins, and that is now said out loud.
///
/// Before this change the unshadowed branch did not resolve at all — it was issue #107's silent
/// drop — so only one of the two branches existed and no precedence had to be chosen. Making the
/// sub-file reference resolve makes both live, and therefore chooses. Measured: with the root
/// declaring `Shared`, `lib.yaml`'s own `$ref: '#/components/schemas/Shared'` yields the **root's**
/// `Shared`; delete that one unrelated root component and the same unchanged `$ref` yields the
/// sub-file's. So adding a component to the root retargets a reference written in another file.
///
/// Root-wins is kept — changing it is a generated-API break — but "silently" is the fourth
/// behaviour the standing invariants forbid, and prose in a comment does not remove silence. `W011`
/// (a declaration that has no effect) is exactly what the sub-file's own `Shared` is here, and it
/// already has a code, a matrix cell and an `errors.md` row, so no ordinal moves.
#[test]
fn a_sub_file_component_shadowed_by_the_root_is_acknowledged() {
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
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Wrapper' } }
components:
  schemas:
    Shared:
      type: object
      required: [from_root]
      properties: { from_root: { type: string } }
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
      required: [inner]
      properties:
        inner: { $ref: '#/components/schemas/Shared' }
    Shared:
      type: object
      required: [from_sub_file]
      properties: { from_sub_file: { type: string } }
"##,
    )
    .unwrap();

    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let shadow = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::DeclarationHasNoEffect)
            .map(|d| d.message.clone())
            .find(|message| message.contains("Shared"));
        let shadow = shadow.unwrap_or_else(|| {
            panic!("{entry}: the shadowed declaration is not acknowledged: {report:#?}")
        });
        // Naming one namespace is not enough: the reader has to be told which declaration was read
        // and which one was not.
        assert!(
            shadow.contains("lib.yaml"),
            "{entry}: the message must name the file whose declaration lost: {shadow}"
        );
    }
    // Root-wins is the behaviour being documented, not changed.
    assert!(code.contains("from_root"), "{code}");
    assert!(!code.contains("from_sub_file"), "{code}");
}

/// The matched pair for the warning above: a reference that **names its own document** shadowed
/// nothing, and must not be told that it did.
///
/// Only the bare-fragment spelling can be shadowed. `#/components/schemas/Shared` addresses the
/// file it is written in, so writing it inside `lib.yaml` asks for `lib.yaml`'s `Shared` and is
/// given the root's — the whole of `W011`, and what `W011`'s own explain text scopes it to.
/// `./openapi.yaml#/components/schemas/Shared` asks for exactly one declaration and gets that one.
///
/// It warned on both. `ensure_resolved` routes any target that lands in the root's component map
/// back to `ensure_component` carrying the *referring site's* provenance, and the warning was
/// raised there on the strength of three facts that never included how the reference was written.
/// So a deliberately disambiguated reference drew a message quoting a `$ref` string absent from
/// that site, and a remedy — "address the file-local one explicitly with a relative-file
/// reference" — naming the form already in use. Worse, the same string through the nullable-alias
/// path correctly did *not* warn, so one reference got two answers depending only on whether the
/// root's component happened to be open at the time.
///
/// Two further cases vary the **sub-file's body** rather than the spelling, because varying the
/// spelling cannot reach every answer the warning gives.
///
/// The third drops `Shared` from `lib.yaml` altogether: a sub-file referring to a root component
/// it does not redeclare, which is the commonest shape a multi-file description has. Nothing is
/// shadowed, so nothing may be reported. That negative is decided in `Resolver::declares_locally`,
/// by the one line that asks whether the node the local address names actually exists —
/// `Bundle::reference_target` computes an address and never checks, so for an in-document fragment
/// it answers `Some` unconditionally. Without the third case that line is unheld: deleting it
/// leaves the whole workspace suite green while every such description gains a warning naming a
/// declaration it does not contain. The spelling cases cannot cover it, because their `shadows:
/// false` half is turned away by the bare-fragment gate in `warn_if_root_shadows_the_referring_file`
/// and never reaches `declares_locally` at all.
///
/// The fourth drives `W011`'s **own remedy**: "address the file-local one explicitly with a
/// relative-file reference". The explicit spelling driven above is `./openapi.yaml#…`, the *root's*
/// — the one the remedy does not recommend. `./lib.yaml#/components/schemas/Shared` is the one it
/// does, and it must both silence the warning and change the answer, binding the sub-file's
/// `Shared` rather than the root's. Otherwise a precedence change could leave spargen printing a
/// remedy that no longer works, with the suite green.
///
/// All four are driven against one pair of files so the fixture cannot pass by the warning having
/// gone dead: the bare-over-a-local-declaration case must still fire, in the same run.
#[test]
fn an_explicit_relative_file_reference_to_the_roots_component_is_not_a_shadowing() {
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
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Wrapper' } }
components:
  schemas:
    Shared:
      type: object
      required: [from_root]
      properties: { from_root: { type: string } }
"##;
    let lib = |spelling: &str, declares_shared: bool| {
        let shared = if declares_shared {
            "\n    Shared:\n      type: object\n      required: [from_sub_file]\n      \
             properties: { from_sub_file: { type: string } }"
        } else {
            ""
        };
        format!(
            r##"
components:
  schemas:
    Wrapper:
      type: object
      required: [inner]
      properties:
        inner: {{ $ref: '{spelling}' }}{shared}
"##
        )
    };

    for (case, spelling, declares_shared, shadows, from) in [
        (
            "the bare fragment, redeclared locally",
            "#/components/schemas/Shared",
            true,
            true,
            "from_root",
        ),
        (
            "the root addressed explicitly",
            "./openapi.yaml#/components/schemas/Shared",
            true,
            false,
            "from_root",
        ),
        (
            "the bare fragment, nothing local to shadow",
            "#/components/schemas/Shared",
            false,
            false,
            "from_root",
        ),
        (
            "the remedy's own spelling",
            "./lib.yaml#/components/schemas/Shared",
            true,
            false,
            "from_sub_file",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), root).unwrap();
        std::fs::write(dir.join("lib.yaml"), lib(spelling, declares_shared)).unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = run_check(&Spec::new(dir.join("openapi.yaml")));

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{case}/{entry}: {report:#?}"
            );
            let raised = report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect && d.message.contains("Shared"));
            assert_eq!(
                raised,
                shadows,
                "{case} ({spelling}, lib.yaml {} `Shared`)/{entry}: W011 must fire when the bare \
                 fragment is written over a declaration the referring file makes itself, and in \
                 no other case — every other case has one declaration in play, so nothing of the \
                 author's is without effect and the remedy would ask for a spelling that is \
                 already written or would change which declaration is read: {report:#?}",
                if declares_shared {
                    "declares"
                } else {
                    "does not declare"
                }
            );
        }

        // Which declaration was actually read. The first three cases all reach the root's, by
        // precedence or because it is the only one; the fourth is the remedy W011 prints, and the
        // point of driving it is that it does something different — it binds the sub-file's own
        // `Shared`. Read on the bound type's fields rather than its name, because the emitter
        // suffixes the sub-file's colliding copy and a substring test would pass on the wrong
        // answer.
        let absent = if from == "from_root" {
            "from_sub_file"
        } else {
            "from_root"
        };
        let inner = field_type(&code, "pub inner")
            .unwrap_or_else(|| panic!("{case}: no `inner` field at all: {code}"));
        let bound = inner
            .rsplit_once('<')
            .map_or(inner.as_str(), |(_, tail)| tail)
            .trim_end_matches('>');
        let fields = declared_fields(&code, bound);
        assert!(
            fields.iter().any(|field| field == from) && !fields.iter().any(|field| field == absent),
            "{case}: `inner` bound `{inner}`, whose fields are {fields:?}, expected `{from}`: \
             {code}"
        );
    }
}

#[test]
fn local_relative_schema_refs_resolve_from_their_own_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: 'schemas.yaml#/Pet' }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("schemas.yaml"),
        r##"
Id: { type: string }
Pet:
  type: object
  properties:
    id: { $ref: '#/Id' }
  required: [id]
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(out).unwrap();
    assert!(code.contains("pub id"), "{code}");
    assert_no_untyped_value(&code);
}

#[test]
fn oas32_self_is_a_canonical_reference_identity() {
    let spec = r##"
openapi: 3.2.0
$self: https://api.example.test/openapi.yaml
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: 'https://api.example.test/openapi.yaml#/components/schemas/Pet'
components:
  schemas:
    Pet:
      type: object
      properties: { id: { type: string } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    assert!(
        !has_code(&report, Code::AbsoluteRefUnsupported),
        "{report:#?}"
    );
}

#[test]
fn oas32_relative_self_establishes_the_local_reference_base() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::create_dir(dir.join("canonical")).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.2.0
$self: canonical/api.yaml
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          content:
            application/json:
              schema: { $ref: 'api.yaml#/components/schemas/Pet' }
components:
  schemas:
    Pet: { $ref: 'schemas.yaml#/Pet' }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("canonical/schemas.yaml"),
        r##"
Pet:
  type: object
  properties: { id: { type: string } }
  required: [id]
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(out).unwrap();
    assert!(code.contains("pub id"), "{code}");
}

/// A reference naming a local `$self` identity through a path spelled differently from the one
/// `$self` gives (`../canonical/api.yaml` resolved against `canonical/api.yaml` is
/// `canonical/../canonical/api.yaml`) is that document, not a file to load from disk (#220). No
/// file exists at `canonical/api.yaml`, so a spelling-sensitive comparison fails the load.
#[test]
fn a_reference_naming_a_relative_self_by_another_spelling_is_that_document() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::create_dir(dir.join("canonical")).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.2.0
$self: canonical/api.yaml
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '../canonical/api.yaml#/components/schemas/Pet' }
components:
  schemas:
    Pet:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##,
    )
    .unwrap();
    let spec = dir.join("openapi.yaml");
    let out = dir.join("client.rs");
    let generated = run_generate(&build(spec.clone(), out.clone()));
    let checked = run_check(&Spec::new(spec));
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::DeclarationHasNoEffect),
            "{entry}: {report:#?}"
        );
    }
    let code = std::fs::read_to_string(out).unwrap();
    assert_eq!(code.matches("pub struct Pet").count(), 1, "{code}");
}

/// A remote-`$ref` spec fixture referencing a single vendored schema, plus a helper to lay it out
/// in a tempdir with a hand-written lock + vendored file (no network) and run `generate`/`check`.
mod remote {
    use super::*;

    // The exact bytes of the vendored remote document and their real SHA-256 (see the module test
    // asserting spargen's own `sha256` matches this). A mismatch here is a pin-drift fixture.
    const GIZMO_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n";
    const GIZMO_SHA256: &str = "6d9d14b78ee36c68c62cfbde1e06186a7ded59991eb2f5b6aa8b4503209d8974";
    const GIZMO_URL: &str = "https://api.example.com/schemas/gizmo.yaml";
    const GIZMO_VENDOR_PATH: &str = "api.example.com/schemas/gizmo.yaml";

    fn spec() -> String {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /gizmo:\n\
             \x20   get:\n\
             \x20     operationId: getGizmo\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{GIZMO_URL}\"\n"
        )
    }

    fn lock(sha256: &str) -> String {
        format!(
            "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{sha256}\"\npath = \"{GIZMO_VENDOR_PATH}\"\n"
        )
    }

    /// Write the spec and (optionally) a lock + vendored file into a fresh tempdir, then run the
    /// pipeline. Returns the report and the generated module text (when generation ran).
    fn run(
        with_lock: Option<String>,
        with_vendor: Option<&str>,
        check_only: bool,
    ) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec()).unwrap();
        if let Some(lock) = with_lock {
            std::fs::write(dir.join("spargen.lock"), lock).unwrap();
        }
        if let Some(vendor) = with_vendor {
            let path = dir.join(".spargen/vendor").join(GIZMO_VENDOR_PATH);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vendor).unwrap();
        }
        let out = dir.join("client.rs");
        let spec = Spec::new(dir.join("openapi.yaml"));
        let report = if check_only {
            run_check(&spec)
        } else {
            run_generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
        };
        (report, temp, out)
    }

    #[test]
    fn unpinned_remote_ref_is_e003_with_remedy() {
        // No lock present ⇒ the remote ref is unpinned. This must be rejected with the *narrowed*
        // E003 and an actionable remedy pointing at `spargen lock`.
        let (report, _temp, _out) = run(None, None, false);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::AbsoluteRefUnsupported)
            .expect("E003 fires");
        let remedy = diag.remedy.as_deref().unwrap_or_default();
        assert!(
            remedy.contains("spargen lock"),
            "actionable remedy: {remedy}"
        );
        assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    /// The one diagnostic `report` carries, which must be `code`, with its pointer and the line its
    /// span starts on, after the location oracle has found nothing to report about it.
    fn located_at(report: &Report, root: &[u8], code: Code) -> (String, u32) {
        let reported: Vec<Code> = report.diagnostics().iter().map(|d| d.code).collect();
        assert_eq!(reported, [code], "{report:#?}");
        let violations: Vec<String> = oracles::location_violations(report.diagnostics(), root)
            .into_iter()
            .map(|violation| violation.reason)
            .collect();
        assert!(violations.is_empty(), "{violations:#?}");
        let diag = &report.diagnostics()[0];
        let span = diag.span.expect("located at a span");
        assert_eq!(span.file.0, 0, "{diag:?}");
        (diag.pointer.as_str().to_owned(), span.start.line)
    }

    /// `E003` and `E021` are raised at the `$ref` that names the remote document, with that
    /// value's span, not at the referring document's root (#534), through
    /// `check` and `generate` alike: unpinned, pinned with the vendored file missing, and pinned
    /// with drifted vendored bytes.
    #[test]
    fn e003_and_e021_point_at_the_remote_ref() {
        let wrong_sha = "0".repeat(64);
        for (code, lock, vendored) in [
            (Code::AbsoluteRefUnsupported, None, None),
            (Code::VendoredRefDrift, Some(lock(GIZMO_SHA256)), None),
            (
                Code::VendoredRefDrift,
                Some(lock(&wrong_sha)),
                Some(GIZMO_YAML),
            ),
        ] {
            for check_only in [true, false] {
                let (report, temp, _) = run(lock.clone(), vendored, check_only);
                let root = std::fs::read(temp.path().join("openapi.yaml")).unwrap();
                assert_eq!(
                    located_at(&report, &root, code),
                    (
                        "/paths/~1gizmo/get/responses/200/content/application~1json/schema/$ref"
                            .to_owned(),
                        13
                    ),
                    "{code:?}, check only: {check_only}"
                );
            }
        }
    }

    /// A remote reference that only a reference into a specification extension reaches is located
    /// at its place inside the extension, and one a Security Requirement key spells is located at
    /// that key (#534). A `$ref` at the root document's top level is located at its member, so
    /// even that one is not reported at the root.
    #[test]
    fn e003_points_at_a_remote_ref_inside_an_extension_target_and_at_a_requirement_key() {
        let extension = format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /gizmo:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: '#/x-defs/Gizmo' }}\n\
             x-defs:\n\
             \x20 Gizmo:\n\
             \x20   $ref: \"{GIZMO_URL}\"\n"
        );
        let requirement = "openapi: 3.1.0\n\
             info: { title: T, version: 1.0.0 }\n\
             security:\n\
             \x20 - https://api.example.com/schemes/key.yaml: []\n\
             paths: {}\n";
        let top_level = format!(
            "$ref: \"{GIZMO_URL}\"\n\
             openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n"
        );
        for (spec, pointer, line) in [
            (extension.as_str(), "/x-defs/Gizmo/$ref", 14),
            (top_level.as_str(), "/$ref", 1),
            (
                requirement,
                "/security/0/https:~1~1api.example.com~1schemes~1key.yaml",
                4,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path =
                camino::Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
            std::fs::write(&path, spec).unwrap();
            let report = run_check(&Spec::new(path));
            assert_eq!(
                located_at(&report, spec.as_bytes(), Code::AbsoluteRefUnsupported),
                (pointer.to_owned(), line),
                "{spec}"
            );
        }
    }

    /// A URL cited from several sites is reported once per site, each at its own `$ref` (#534):
    /// the sites' locations differ, so `Diagnostics::emit` no longer merges them into one report
    /// per referring file, as it did while every report sat at the root. Unpinned (`E003`) and
    /// drifted (`E021`) alike, through `check` and `generate`.
    #[test]
    fn a_remote_url_cited_from_two_sites_is_reported_at_each() {
        let spec = format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /a:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"{GIZMO_URL}\" }}\n\
             \x20 /b:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"{GIZMO_URL}\" }}\n"
        );
        let drifted = lock(&"0".repeat(64));
        for (code, lock, vendor) in [
            (Code::AbsoluteRefUnsupported, None, &[][..]),
            (
                Code::VendoredRefDrift,
                Some(drifted.as_str()),
                &[(GIZMO_VENDOR_PATH, GIZMO_YAML)][..],
            ),
        ] {
            for check_only in [true, false] {
                let (report, _temp, _out) = run_layout(&spec, lock, vendor, check_only);
                let located: Vec<(Code, &str, Option<u32>)> = report
                    .diagnostics()
                    .iter()
                    .map(|d| (d.code, d.pointer.as_str(), d.span.map(|s| s.start.line)))
                    .collect();
                assert_eq!(
                    located,
                    [
                        (
                            code,
                            "/paths/~1a/get/responses/200/content/application~1json/schema/$ref",
                            Some(11)
                        ),
                        (
                            code,
                            "/paths/~1b/get/responses/200/content/application~1json/schema/$ref",
                            Some(19)
                        ),
                    ],
                    "check only: {check_only}: {report:#?}"
                );
            }
        }
    }

    #[test]
    fn pinned_remote_ref_resolves_hermetically_to_typed_schema() {
        // Lock pins the correct sha256 and the vendored bytes match ⇒ the remote ref resolves with
        // no network and lowers to a typed struct (never `serde_json::Value`).
        let (report, _temp, out) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), false);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AbsoluteRefUnsupported),
            "{report:#?}"
        );
        assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
        let generated = std::fs::read_to_string(&out).expect("module written");
        assert!(
            generated.contains("id"),
            "the vendored schema's field is emitted:\n{generated}"
        );

        // check/generate parity: `check` resolves the same remote ref, also without network.
        let (checked, _temp2, _out2) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), true);
        assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
        assert!(
            !has_code(&checked, Code::AbsoluteRefUnsupported),
            "{checked:#?}"
        );
    }

    #[test]
    fn drifted_vendored_content_is_e021() {
        // The vendored bytes are fine, but the lock pins a different sha256 ⇒ the lock is the source
        // of truth, so the drift is refused (E021) rather than silently used.
        let wrong_sha = "0".repeat(64);
        let (report, _temp, _out) = run(Some(lock(&wrong_sha)), Some(GIZMO_YAML), false);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    #[test]
    fn missing_vendored_file_is_e021() {
        // Lock pins the ref but the vendored copy is absent ⇒ drift (nothing to hash against).
        let (report, _temp, _out) = run(Some(lock(GIZMO_SHA256)), None, false);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }

    /// Lay out `spec` + `lock` + arbitrary vendored files `(vendor-relative path, bytes)` in a
    /// fresh tempdir, then run the pipeline (no network). Returns the report and generated module
    /// path.
    fn run_layout(
        spec: &str,
        lock: Option<&str>,
        vendor: &[(&str, &str)],
        check_only: bool,
    ) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec).unwrap();
        if let Some(lock) = lock {
            std::fs::write(dir.join("spargen.lock"), lock).unwrap();
        }
        for (rel, content) in vendor {
            let path = dir.join(".spargen/vendor").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        let out = dir.join("client.rs");
        let spec = Spec::new(dir.join("openapi.yaml"));
        let report = if check_only {
            run_check(&spec)
        } else {
            run_generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
        };
        (report, temp, out)
    }

    /// The remote counterpart of the direct-recursive `allOf` member, reached through an **alias**,
    /// which is the shape that needs the id-keyed guard rather than the spelling-keyed one.
    ///
    /// `gather_member`'s remote arm has two checks. One keys on the reference *string* —
    /// `remote_in_progress.contains_key(reference)` — and the other on the returned `Ty`'s id.
    /// Only the second can see this case: `node.yaml` composes
    /// `allOf: [alias.yaml]`, `alias.yaml` is a bare `$ref` back to `node.yaml`, so the member's own
    /// spelling is never the in-progress key, and `ensure_remote` chains through the alias and hands
    /// back a back-edge against `node.yaml`'s reservation.
    ///
    /// **Removing the id-keyed check leaves every other test in the workspace green.** Without it
    /// this document generates, with zero diagnostics, and emits
    /// `pub type …child = serde_json::Value;` — the same silent degradation the component path
    /// guards against, on a path no other test exercises.
    #[test]
    fn a_direct_recursive_remote_all_of_member_reached_through_an_alias_is_rejected() {
        const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
        const ALIAS_URL: &str = "https://api.example.com/schemas/alias.yaml";
        const NODE_YAML: &str = "type: object\nrequired: [label]\nproperties:\n  label: { type: string }\n  child:\n    allOf:\n      - { $ref: \"alias.yaml\" }\n";
        const ALIAS_YAML: &str = "$ref: \"node.yaml\"\n";
        const NODE_SHA: &str = "09216246cfa803064f874532df513b6458892853137616dd83c386ee3c4a49bd";
        const ALIAS_SHA: &str = "394e78d465e607843bb3b04078679cd2015aea58c67b79b9da1129591d8831a0";

        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n\n[[remote]]\nurl = \"{ALIAS_URL}\"\nsha256 = \
             \"{ALIAS_SHA}\"\npath = \"api.example.com/schemas/alias.yaml\"\n"
        );
        let vendor = [
            ("api.example.com/schemas/node.yaml", NODE_YAML),
            ("api.example.com/schemas/alias.yaml", ALIAS_YAML),
        ];

        let (generated, _temp, out) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let (checked, _temp2, _out2) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            // The pins are live, so the document really reaches lowering rather than being
            // rejected for drift or for being unpinned.
            assert!(
                !has_code(report, Code::VendoredRefDrift),
                "{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::AbsoluteRefUnsupported),
                "{entry}: {report:#?}"
            );
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{entry}: the member resolves to the schema being lowered, whose fields are not \
                 yet known: {report:#?}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable
                        && d.message.contains("direct recursive")),
                "{entry}: {report:#?}"
            );
        }
        // The degradation itself, so the guard's removal fails on the emitted output and not only
        // on the verdict.
        assert!(!code.contains("= serde_json::Value;"), "{code}");
    }

    /// Two vendored remote schemas in mutual recursion through a nullable alias — the remote
    /// spelling of the commonest recursive idiom there is, and the one frame of the three whose
    /// open reservations no guard could see.
    ///
    /// `node.yaml` has a `parent` of `maybe.yaml`; `maybe.yaml` is nothing but "a `node.yaml`, or
    /// null". Both are hash-pinned, so the document is legal and hermetic. The union collapse's
    /// escape hatch asked `reservation_at`, which consults the root component map and
    /// `resolved_in_progress` and **not** `remote_in_progress`, so the collapse handed
    /// `ensure_remote` a foreign id and inserted no def — and `pop_last()` then popped a def that
    /// was not this frame's root into an `assert_eq!` that is live in release builds too. The
    /// failure was a **process abort**, not a diagnostic: inside a consumer's `build.rs` it is a
    /// panicking build script with no code, no pointer and no `--carve` escape, which is the
    /// prohibited fourth behaviour in its worst form. A panic is not an `Outcome`, so nothing in
    /// this suite could have observed it; this fixture is what observes it.
    #[test]
    fn a_nullable_alias_between_two_vendored_remote_schemas_generates() {
        const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
        const MAYBE_URL: &str = "https://api.example.com/schemas/maybe.yaml";
        const NODE_YAML: &str =
            "type: object\nproperties:\n  name: { type: string }\n  parent: { $ref: 'maybe.yaml' }\n";
        const MAYBE_YAML: &str = "oneOf:\n  - $ref: 'node.yaml'\n  - type: 'null'\n";
        const NODE_SHA: &str = "b0b0741c3519d771b634a8cee182500b35efbfb9e20e84dd3b4dd62497174569";
        const MAYBE_SHA: &str = "33b37680c6082b1e81bb769383543219f1d1477bca2e9838f7da314631eac815";

        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n\n[[remote]]\nurl = \"{MAYBE_URL}\"\nsha256 = \
             \"{MAYBE_SHA}\"\npath = \"api.example.com/schemas/maybe.yaml\"\n"
        );
        let vendor = [
            ("api.example.com/schemas/node.yaml", NODE_YAML),
            ("api.example.com/schemas/maybe.yaml", MAYBE_YAML),
        ];

        let (generated, _temp, out) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let (checked, _temp2, _out2) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            // The pins are live, so the document really reaches lowering.
            assert!(
                !has_code(report, Code::VendoredRefDrift),
                "{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::AbsoluteRefUnsupported),
                "{entry}: {report:#?}"
            );
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{entry}: a hermetically pinned, legal description: {report:#?}"
            );
            // The reservation must not survive into the finished graph either.
            assert!(
                !has_code(report, Code::InvalidInput),
                "{entry}: {report:#?}"
            );
        }
        // The alias is the target, optional and boxed — what the direct `{$ref: T}` spelling of the
        // same construct already produces and what the support matrix promises for it. Named, and
        // read off `parent`'s own declaration: the embedded runtime supplies an `Option<Box<…>>` of
        // its own to every generated module, so a bare `contains("Option<Box<")` would hold even if
        // this reference had been emitted unboxed.
        assert_eq!(
            field_type(&code, "pub parent").as_deref(),
            Some("Option<Box<HttpsApiExampleComSchemasNodeYaml>>"),
            "{code}"
        );
        assert!(!code.contains("serde_json::Value>"), "{code}");
    }

    /// The remote spelling of
    /// `a_sub_file_nullable_union_component_stays_nullable_at_every_use`: a vendored schema whose
    /// body is `oneOf: [null, string]`, used by two required fields. `ensure_remote` wrote
    /// `schema_is_nullable`'s provisional `false` back over the union's own `true` and cached it,
    /// so both fields were a bare type a `null` would fail to decode into.
    #[test]
    fn a_vendored_nullable_union_stays_nullable_at_every_use() {
        const MAYBE_URL: &str = "https://api.example.com/schemas/maybe.yaml";
        const MAYBE_YAML: &str = "oneOf:\n  - type: 'null'\n  - type: string\n";
        const MAYBE_SHA: &str = "734c50f67b492acbbf4be1e9e09901db3f69ba14154ce2256cdd9bfe2e039fa7";

        let spec = format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /it:\n\
             \x20   get:\n\
             \x20     operationId: getIt\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               type: object\n\
             \x20               required: [a, b]\n\
             \x20               properties:\n\
             \x20                 a: {{ $ref: \"{MAYBE_URL}\" }}\n\
             \x20                 b: {{ $ref: \"{MAYBE_URL}\" }}\n"
        );
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{MAYBE_URL}\"\nsha256 = \"{MAYBE_SHA}\"\npath = \
             \"api.example.com/schemas/maybe.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/maybe.yaml", MAYBE_YAML)];

        let (generated, _temp, out) = run_layout(&spec, Some(&lock), &vendor, false);
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let (checked, _temp2, _out2) = run_layout(&spec, Some(&lock), &vendor, true);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            // The pin is live, so the document really reaches lowering.
            assert!(
                !has_code(report, Code::VendoredRefDrift),
                "{entry}: {report:#?}"
            );
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        for field in ["pub a:", "pub b:"] {
            assert_eq!(
                field_type(&code, field).as_deref(),
                Some("Option<HttpsApiExampleComSchemasMaybeYaml>"),
                "{field}: {code}"
            );
        }
    }

    /// A vendored remote schema that refers to **itself** directly, with no alias in between: the
    /// plainest recursion there is, and the one that reaches `ensure_remote`'s own in-progress arm.
    ///
    /// The root-component and resolved-file frames each have a fixture that pins their back-edge is
    /// boxed; the remote frame did not. Flipping that arm to `boxed: false` left the entire
    /// workspace green while emitting `pub parent: Option<…NodeYaml>` inside the struct it names —
    /// an infinitely sized type. The two remote fixtures beside this one both reach their back-edge
    /// through the *alias* path, which boxes at a different site, so neither could see it.
    #[test]
    fn a_self_recursive_vendored_remote_schema_is_boxed() {
        const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
        const NODE_YAML: &str =
            "type: object\nproperties:\n  name: { type: string }\n  parent: { $ref: 'node.yaml' }\n";
        const NODE_SHA: &str = "119cdb35650ee7b837b855d28d5e3eef2f0b622aba52b5721817ae313aa19300";

        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/node.yaml", NODE_YAML)];

        let (generated, _temp, out) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let (checked, _temp2, _out2) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert!(
                !has_code(report, Code::VendoredRefDrift),
                "{entry}: {report:#?}"
            );
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
            assert!(
                !has_code(report, Code::InvalidInput),
                "{entry}: {report:#?}"
            );
        }
        // Boxed, or the generated struct has no finite size and does not compile.
        assert_eq!(
            field_type(&code, "pub parent").as_deref(),
            Some("Option<Box<HttpsApiExampleComSchemasNodeYaml>>"),
            "{code}"
        );
    }

    /// The remote spelling of `Selfy = Selfy | null`: a vendored document whose **whole body** is a
    /// union over a `$ref` back to itself plus `null`.
    ///
    /// This is the one document that reaches `reservation_at`'s `remote_in_progress`
    /// canonicalisation, and the only way to reach it. That loop exists because a remote frame is
    /// keyed by the URL it was reached through while a provenance canonicalises to a
    /// `file#pointer`, so a string comparison between the two can never match; the frame is only
    /// *omitted* — rather than merely spelled differently — when the schema being asked about is a
    /// remote document's root, which is exactly this shape and nothing else. Every other remote
    /// recursion sits at a property or an `allOf` member, whose pointer is not the frame's.
    ///
    /// Without the canonicalisation this document does not reject. The collapse hands
    /// `ensure_remote` a foreign id and reserves nothing, and `TypeDefs::fill` is then called on an
    /// id that was never reserved — `fill of an unreserved id`, a **process abort** from inside a
    /// consumer's `build.rs` with no code, no pointer and no `--carve` escape. A panic is not an
    /// `Outcome`, so nothing in this suite could have observed it. That assertion is a
    /// `debug_assert!`, so a build with debug assertions off does not abort: it fills the
    /// unreserved id and carries on, which is worse and not better.
    ///
    /// The loop had zero iterations across the whole workspace before this fixture, while carrying
    /// a paragraph crediting it with a fix the two-file remote fixture passes without. It is this
    /// document that the paragraph is true of.
    #[test]
    fn a_vendored_remote_schema_that_is_a_union_over_itself_is_rejected() {
        const SELFY_URL: &str = "https://api.example.com/schemas/selfy.yaml";
        const SELFY_YAML: &str = "oneOf:\n  - $ref: 'selfy.yaml'\n  - type: 'null'\n";
        const SELFY_SHA: &str = "a85d6698c240a7a1bc9f53f18466c179a2f8fcc01492d822aa913b7a7f4e009a";

        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{SELFY_URL}\"\nsha256 = \"{SELFY_SHA}\"\npath = \
             \"api.example.com/schemas/selfy.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/selfy.yaml", SELFY_YAML)];

        let (generated, _temp, _out) =
            run_layout(&responds_with(SELFY_URL), Some(&lock), &vendor, false);
        let (checked, _temp2, _out2) =
            run_layout(&responds_with(SELFY_URL), Some(&lock), &vendor, true);

        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            // The pin is live, so the document really reaches lowering rather than stopping at the
            // lock.
            assert!(
                !has_code(report, Code::VendoredRefDrift),
                "{entry}: {report:#?}"
            );
            assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
            // A spec-facing code, the same one the root-document spelling of this shape draws —
            // never the internal invariant's `E011`, and never a process abort.
            assert!(
                has_code(report, Code::NonDisjointUnion),
                "{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::InvalidInput),
                "{entry}: {report:#?}"
            );
        }
    }

    fn responds_with(url: &str) -> String {
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /it:\n\
             \x20   get:\n\
             \x20     operationId: getIt\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{url}\"\n"
        )
    }

    /// The remote arm of issue #185: an `allOf` member that is a pinned remote `$ref` with
    /// shape-bearing siblings contributes the target AND the siblings, exactly as the local arms
    /// do (`an_all_of_member_ref_intersects_its_shape_bearing_siblings`). It used to contribute the
    /// vendored `Gizmo` alone, deleting `dropped` and generating `id` as the target's `String`
    /// over a member that required it to be an integer.
    #[test]
    fn a_remote_all_of_member_ref_intersects_its_shape_bearing_siblings() {
        let wrapping = |siblings: &str| {
            format!(
                "openapi: 3.1.0\n\
                 info: {{ title: T, version: 1.0.0 }}\n\
                 paths:\n\
                 \x20 /w:\n\
                 \x20   get:\n\
                 \x20     operationId: getW\n\
                 \x20     responses:\n\
                 \x20       '200':\n\
                 \x20         description: ok\n\
                 \x20         content:\n\
                 \x20           application/json:\n\
                 \x20             schema: {{ $ref: '#/components/schemas/Wrap' }}\n\
                 components:\n\
                 \x20 schemas:\n\
                 \x20   Wrap:\n\
                 \x20     allOf:\n\
                 \x20       - {{ $ref: '{GIZMO_URL}', {siblings} }}\n"
            )
        };
        let lock = lock(GIZMO_SHA256);
        let vendor = [(GIZMO_VENDOR_PATH, GIZMO_YAML)];

        let kept = wrapping("properties: { dropped: { type: integer } }, required: [dropped]");
        let (generated, _temp, out) = run_layout(&kept, Some(&lock), &vendor, false);
        let (checked, _temp2, _out2) = run_layout(&kept, Some(&lock), &vendor, true);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        }
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        assert_eq!(
            declared_fields(&code, "Wrap"),
            vec!["id", "dropped"],
            "{code}"
        );
        let dropped = field_type(&code, "pub dropped").unwrap_or_default();
        assert!(!dropped.starts_with("Option<"), "{code}");

        let contradicted = wrapping("properties: { id: { type: integer } }, required: [id]");
        for check_only in [false, true] {
            let (report, _temp, _out) = run_layout(&contradicted, Some(&lock), &vendor, check_only);
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "check_only={check_only}: {report:#?}"
            );
            let pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::AllOfIrreconcilable)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                pointers,
                vec!["/components/schemas/Wrap"],
                "check_only={check_only}: {report:#?}"
            );
        }
    }

    #[test]
    fn self_recursive_remote_schema_generates_boxed_not_stack_overflow() {
        // A vendored remote schema that refers to ITSELF (a linked-list `next`) must terminate at
        // lowering with a boxed back-edge — ordinary OpenAPI — instead of recursing forever.
        const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
        const NODE_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n  next:\n    $ref: \"https://api.example.com/schemas/node.yaml\"\n";
        const NODE_SHA: &str = "926f0bc154b93b63208fb4895964ca3e7f67ae3bd7b5f6882156edcefb08fffb";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \"api.example.com/schemas/node.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/node.yaml", NODE_YAML)];

        let (report, _temp, out) =
            run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let generated = std::fs::read_to_string(&out).unwrap();
        assert!(
            generated.contains("Box"),
            "recursion is closed with a boxed field:\n{generated}"
        );

        // check/generate parity: a regression that reintroduces the crash must fail here too, and
        // both must return an outcome rather than aborting.
        let (checked, _t2, _o2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);
        assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    }

    /// The cycle guard's two message arms, pinned SEPARATELY.
    ///
    /// Both said "direct recursive", and that was the only thing asserted, so swapping the local
    /// and remote strings was green — which left the LOCAL arm's wording unpinned too: the local
    /// case could have told the reader its reference was remote and nothing would have noticed.
    /// Each arm now asserts the noun it must use and the noun it must not.
    #[test]
    fn the_cycle_guard_names_the_right_kind_of_reference_in_each_arm() {
        const NODE_URL: &str = "https://api.example.com/schemas/recursive.yaml";
        // A remote document whose own schema references itself with shape-bearing siblings.
        const NODE_YAML: &str = "type: object\nproperties:\n  next:\n    $ref: \"https://api.example.com/schemas/recursive.yaml\"\n    type: object\n    properties:\n      x:\n        type: string\n";
        const NODE_SHA: &str = "1c720abdd7ea1b336ad3d49b6a0c5a80430c55d28746fd7540d37e4ae0f091d3";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \"api.example.com/schemas/recursive.yaml\"\n"
        );
        let vendor = [("api.example.com/schemas/recursive.yaml", NODE_YAML)];

        let (report, _t, _o) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let remote_messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(remote_messages.len(), 1, "{report:#?}");
        assert!(
            remote_messages[0].contains("remote `$ref`"),
            "the remote arm must say the reference is remote: {:?}",
            remote_messages[0]
        );
        assert!(
            remote_messages[0].contains("closes a reference cycle"),
            "{:?}",
            remote_messages[0]
        );

        // The local arm, against the same guard. Asserting what it must NOT say is what closes the
        // swap: with the two strings exchanged, this message would call a local component's
        // reference remote.
        let local = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          $ref: '#/components/schemas/Node'\n          type: object\n          properties: { x: { type: string } }\n";
        let report = generate(local);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let local_messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(local_messages.len(), 1, "{report:#?}");
        assert!(
            !local_messages[0].contains("remote"),
            "a local component reference must not be described as remote: {:?}",
            local_messages[0]
        );
        assert!(
            local_messages[0].contains("back to the schema that encloses it"),
            "the local arm must name the enclosing schema: {:?}",
            local_messages[0]
        );

        // And the two arms must not be the same string: a single shared message would satisfy every
        // assertion above only by accident of wording, and this states the requirement directly.
        assert_ne!(
            local_messages[0], remote_messages[0],
            "the two arms report the same message, so neither is pinned to its own case"
        );
    }

    #[test]
    fn mutually_recursive_remote_docs_generate_boxed() {
        // a.yaml ↔ b.yaml reference each other across two vendored documents; the cross-doc cycle
        // must terminate (boxed) rather than overflow.
        const A_URL: &str = "https://api.example.com/schemas/a.yaml";
        const A_YAML: &str = "type: object\nproperties:\n  b:\n    $ref: \"https://api.example.com/schemas/b.yaml\"\n";
        const A_SHA: &str = "bb995ec038973f6ca10fd6674a76a516dc29962fcdf061e1ad49717b2f6e2544";
        const B_URL: &str = "https://api.example.com/schemas/b.yaml";
        const B_YAML: &str = "type: object\nproperties:\n  a:\n    $ref: \"https://api.example.com/schemas/a.yaml\"\n";
        const B_SHA: &str = "62d1762eb79467f3a7204c626a7a268647910a218ac4b344a878a6421c300674";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{A_URL}\"\nsha256 = \"{A_SHA}\"\npath = \"api.example.com/schemas/a.yaml\"\n\n[[remote]]\nurl = \"{B_URL}\"\nsha256 = \"{B_SHA}\"\npath = \"api.example.com/schemas/b.yaml\"\n"
        );
        let vendor = [
            ("api.example.com/schemas/a.yaml", A_YAML),
            ("api.example.com/schemas/b.yaml", B_YAML),
        ];
        let (report, _temp, out) = run_layout(&responds_with(A_URL), Some(&lock), &vendor, false);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let generated = std::fs::read_to_string(&out).unwrap();
        assert!(
            generated.contains("Box"),
            "cross-doc cycle is boxed:\n{generated}"
        );
    }

    #[test]
    fn traversal_vendor_path_in_lock_is_rejected_without_reading() {
        // A hand-edited lock whose `path` escapes the vendor dir must be rejected at lock-parse
        // time — before any file is opened — rather than reading an arbitrary file.
        for bad_path in ["../../etc/passwd", "/etc/passwd"] {
            let lock = format!(
                "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{GIZMO_SHA256}\"\npath = \"{bad_path}\"\n"
            );
            let (report, _temp, _out) = run(Some(lock), Some(GIZMO_YAML), false);
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{bad_path}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::InvalidInput),
                "{bad_path} rejected at parse: {report:#?}"
            );
            // It never reached resolution, so no drift/unpinned diagnostic fires.
            assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
        }
    }

    /// A vendored remote document is ranked by its retrieval URL, not by where its copy is
    /// vendored. Every type a remote document yields is hinted by its whole reference URL, so two
    /// remote documents contest a name only when their URLs spell the same identifier:
    /// `a-example.com` and `a.example.com` both become `HttpsAExampleComShapeYaml`. Both are whole
    /// documents at the empty pointer, so only the document key tells them apart, and `-` sorts
    /// before `.`. The lock vendors the copies in the opposite order (`z/` and `a/`), so ranking by
    /// the vendored path would hand the bare name to `a.example.com`'s schema, and ranking by
    /// discovery order would move it when `paths` is reordered. Both orders must give it to
    /// `a-example.com`'s.
    #[test]
    fn a_contested_type_name_ranks_a_vendored_remote_document_by_its_url() {
        const DASH_URL: &str = "https://a-example.com/shape.yaml";
        const DOT_URL: &str = "https://a.example.com/shape.yaml";
        const ALPHA_YAML: &str =
            "type: object\nrequired: [alpha]\nproperties: { alpha: { type: string } }\n";
        const BETA_YAML: &str =
            "type: object\nrequired: [beta]\nproperties: { beta: { type: integer } }\n";
        const ALPHA_SHA: &str = "ddc1311e15bb569ce61ed7e650a34cdcbc0a1c123025149aede8df402b86e9b8";
        const BETA_SHA: &str = "ae8c71c6ff85f26973d98c92f545011b417f902b20990dcbf631997fc1de047a";
        const NAME: &str = "HttpsAExampleComShapeYaml";
        let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{DASH_URL}\"\nsha256 = \"{ALPHA_SHA}\"\npath = \
             \"z/shape.yaml\"\n\n[[remote]]\nurl = \"{DOT_URL}\"\nsha256 = \"{BETA_SHA}\"\npath = \
             \"a/shape.yaml\"\n"
        );
        let vendor = [("z/shape.yaml", ALPHA_YAML), ("a/shape.yaml", BETA_YAML)];
        for swap in [false, true] {
            let spec = two_shape_root([("a", DASH_URL), ("b", DOT_URL)], swap);
            let (report, _temp, out) = run_layout(&spec, Some(&lock), &vendor, false);
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            let code = std::fs::read_to_string(&out).unwrap();
            assert_eq!(
                field_owner(&code, "pub alpha:").as_deref(),
                Some(NAME),
                "swap={swap}: the lower retrieval URL keeps the bare name: {code}"
            );
            // The loser's suffix is the hash of its pointer, which is empty for a whole document.
            assert_eq!(
                field_owner(&code, "pub beta:").as_deref(),
                Some("HttpsAexampleComShapeYaml84222325"),
                "swap={swap}: the other document carries the disambiguator: {code}"
            );
        }
    }
}

#[test]
fn e006_dynamic_ref_rejected() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      $dynamicRef: "#meta"
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DynamicRefRejected));
}

/// Build an OpenAPI document whose components form a chain `S0 -> S1 -> ... -> S{depth}`, where each
/// `S{i}` composes the next via `allOf: [{ $ref: S{i+1} }]` and `S{depth}` is a plain string. Every
/// component is parsed shallowly, so this defeats the parser's own nesting cap and forces lowering
/// to recurse the full chain — the shape that used to overflow the stack (issue #32).
fn deep_component_chain(depth: usize) -> String {
    let mut schemas = String::new();
    for i in 0..depth {
        schemas.push_str(&format!(
            "\"S{i}\":{{\"allOf\":[{{\"$ref\":\"#/components/schemas/S{}\"}}]}},",
            i + 1
        ));
    }
    schemas.push_str(&format!("\"S{depth}\":{{\"type\":\"string\"}}"));
    format!(
        "{{\"openapi\":\"3.1.0\",\"info\":{{\"title\":\"T\",\"version\":\"1.0.0\"}},\
         \"paths\":{{}},\"components\":{{\"schemas\":{{{schemas}}}}}}}"
    )
}

#[test]
fn e014_deep_ref_chain_is_rejected_not_overflowed() {
    // Regression for issue #32: a `$ref` chain far deeper than the lowering depth cap must be
    // rejected with a diagnostic (E014) rather than recursing until the stack overflows and the
    // process aborts. `deep_component_chain` builds a chain whose lowering depth exceeds
    // `MAX_SCHEMA_DEPTH` (128); the pre-fix generator crashed on it.
    let spec = deep_component_chain(400);
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaNestingTooDeep),
        "expected E014 SchemaNestingTooDeep; got {report:#?}"
    );

    // check/generate parity: the depth guard lives in lowering, which `check` runs identically.
    let checked = check(&spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::SchemaNestingTooDeep),
        "{checked:#?}"
    );
}

#[test]
fn moderate_ref_chain_below_the_cap_still_lowers() {
    // The guard must not reject legitimately-nested specs: a chain well under `MAX_SCHEMA_DEPTH`
    // lowers cleanly. This pins the cap as a safety backstop, not a routine rejection.
    let spec = deep_component_chain(32);
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaNestingTooDeep),
        "a 32-deep chain must lower without E014: {report:#?}"
    );
}

#[test]
fn percent_encoded_pointer_fragments_resolve() {
    // A `$ref` pointer travels in a URI fragment, so `{`/`}` must be percent-encoded there. The
    // token is percent-decoded before `~1`/`~0` are unescaped, so this addresses `/pets/{petId}`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets/{petId}:
    get:
      parameters:
        - name: petId
          in: path
          required: true
          schema: { type: string }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/paths/~1pets~1%7BpetId%7D/get/responses/200/content/application~1json/schema"
"##;
    let report = generate(spec);
    // The schema at that pointer *is* this `$ref`, so it names only itself: a cycle, and rejected
    // for being one. What must never happen is the report the missing percent-decoding used to
    // produce — that the target could not be found — so this asserts the wording, not merely the
    // code. (`E004` covers both, since an alias cycle resolves to nothing; asserting its absence
    // would now fail for the right reason rather than pass for the wrong one.)
    let e004: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::UnresolvedRef)
        .collect();
    assert!(
        e004.iter().all(|d| !d.message.contains("was not found")
            && !d.message.contains("unsupported or unresolved")),
        "the percent-encoded pointer must resolve: {report:#?}"
    );
    assert!(
        e004.iter().any(|d| d.message.contains("alias cycle")),
        "a schema that references itself is a cycle, and is named as one: {report:#?}"
    );
}

#[test]
fn w011_reference_object_documentation_override() {
    // A Reference Object's summary/description document the reference site, but spargen emits one
    // shared item per component, so the override has nowhere to land.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - $ref: "#/components/parameters/Limit"
          description: How many to return on this endpoint.
      responses:
        "204": { description: No Content }
components:
  parameters:
    Limit:
      name: limit
      in: query
      schema: { type: integer }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

/// A Header Object and a Media Type Object reached by a relative-file `$ref` resolve through the
/// input bundle, exactly as a Parameter or Response Object reference already did. Restricting these
/// two to `#/components/...` aliases made an otherwise valid multi-file description an `E004`.
#[test]
fn relative_file_header_and_media_type_refs_resolve_like_parameters() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Request-Id: { $ref: 'shared.yaml#/RequestId' }
          content:
            application/json: { $ref: 'shared.yaml#/PetBody' }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("shared.yaml"),
        r##"
RequestId:
  description: correlation id
  schema: { type: string }
PetBody:
  schema:
    type: object
    properties:
      id: { type: string }
    required: [id]
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    // The header became a typed accessor and the media type contributed the body type, so both
    // references were genuinely followed rather than merely tolerated.
    assert!(code.contains("x_request_id"), "{code}");
    assert!(code.contains("pub id"), "{code}");
}
