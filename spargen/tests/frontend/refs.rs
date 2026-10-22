//! Reference resolution: the `E004` reference walkers, same-file subschema pointers, local relative
//! files, `$self`, `E006`/`E014` and the depth cap, percent-encoded pointers, and Reference Object
//! documentation; plus the two-file `Shape` helpers the `refs_bundle_identity` and `refs_remote`
//! families share. Sub-files, bundle identity and remote documents sit in the sibling `refs_*`
//! modules.

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

/// A relative-file `$ref` whose target cannot be read is an `E011` located at that `$ref`, with its
/// value's span in the file that holds it, not at the root document's root (#711): from the root
/// document and from a referenced file, through `check` and `generate` alike.
#[test]
fn an_unreadable_relative_file_ref_is_e011_at_the_ref() {
    let root = "openapi: 3.1.0\n\
                info: { title: T, version: 1.0.0 }\n\
                paths:\n\
                \x20 /pet:\n\
                \x20   get:\n\
                \x20     responses:\n\
                \x20       '200':\n\
                \x20         description: ok\n\
                \x20         content:\n\
                \x20           application/json:\n\
                \x20             schema: { $ref: 'TARGET#/Pet' }\n";
    let lib = "Pet:\n\
               \x20 type: object\n\
               \x20 properties:\n\
               \x20   tag: { $ref: './missing.yaml#/Tag' }\n";
    for (target, file, pointer, line) in [
        (
            "./missing.yaml",
            0,
            "/paths/~1pet/get/responses/200/content/application~1json/schema/$ref",
            11,
        ),
        ("lib.yaml", 1, "/Pet/properties/tag/$ref", 4),
    ] {
        for check_only in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
            std::fs::write(dir.join("openapi.yaml"), root.replace("TARGET", target)).unwrap();
            std::fs::write(dir.join("lib.yaml"), lib).unwrap();
            let input = Spec::new(dir.join("openapi.yaml"));
            let report = if check_only {
                run_check(&input)
            } else {
                run_generate(
                    &input
                        .build(dir.join("client.rs"))
                        .cargo(CargoIntegration::Off),
                )
            };
            let reported: Vec<Code> = report.diagnostics().iter().map(|d| d.code).collect();
            assert_eq!(reported, [Code::InvalidInput], "{report:#?}");
            let diag = &report.diagnostics()[0];
            assert!(
                diag.message.contains("failed to read") && diag.message.contains("missing.yaml"),
                "{diag:?}"
            );
            let span = diag.span.expect("located at a span");
            assert_eq!(
                (diag.pointer.as_str(), span.file.0, span.start.line),
                (pointer, file, line),
                "{target}, check only: {check_only}: {diag:?}"
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

/// A schema that opens a static resource scope, by `$id` or by `$anchor`, is `E004`'s
/// resource-scope case at that schema, with the remedy that says no reference is literally
/// unresolved.
#[test]
fn e004_a_schema_resource_scope_is_rejected_at_the_schema() {
    for keyword in ["$id: https://example.com/thing", "$anchor: thing"] {
        let report = generate(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    Thing:\n      {keyword}\n      type: object\n"
        ));
        assert_eq!(
            e004_pointers(&report, keyword),
            ["/components/schemas/Thing"],
            "{keyword}"
        );
        let remedy = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::UnresolvedRef)
            .and_then(|d| d.remedy.as_deref())
            .unwrap_or_default();
        assert!(
            remedy.contains("inline the subschema or reference it by JSON Pointer"),
            "{keyword}: {report:#?}"
        );
    }
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

/// A root document whose two operations reach a `Shape` declared in `a.yaml` and another declared
/// in `b.yaml`, listed `/a` first unless `b_first`; returns the generated client.
pub(super) fn generate_two_file_shapes(b_first: bool) -> String {
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
pub(super) const SHAPE_ALPHA_YAML: &str =
    "components:\n  schemas:\n    Shape:\n      type: object\n      \
                                required: [alpha]\n      properties: { alpha: { type: string } }\n";
/// A document declaring a `Shape` whose one field is `beta`.
pub(super) const SHAPE_BETA_YAML: &str =
    "components:\n  schemas:\n    Shape:\n      type: object\n      \
                               required: [beta]\n      properties: { beta: { type: integer } }\n";

/// A root document with one operation per `(path, $ref)` in `refs`, each answering `200` with the
/// schema the `$ref` names; the second entry is listed first when `swap`.
pub(super) fn two_shape_root(refs: [(&str, &str); 2], swap: bool) -> String {
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
