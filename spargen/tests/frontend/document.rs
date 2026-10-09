//! Document-level structure: versions and dialects, operations and path items, specification
//! extensions, and webhooks.

use super::*;

#[test]
fn e011_official_structure_schema_rejects_missing_info() {
    let spec = "openapi: 3.1.0\npaths: {}\n";
    let generated = generate(spec);
    let checked = check(spec);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e002_unsupported_dialect() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/not-the-base
paths: {}
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedDialect));
    // The diagnostic must point at the offending value on line 4 (column 20, where the
    // `jsonSchemaDialect` value begins), not at line 1 / the whole file. This pins the
    // span-preserving parser: pre-fix, every node carried the root (whole-file) span.
    let dialect = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::UnsupportedDialect)
        .expect("UnsupportedDialect diagnostic");
    let span = dialect.span.expect("dialect diagnostic has a span");
    assert_eq!(span.start.line, 4, "{dialect:#?}");
    assert_eq!(span.start.col, 20, "{dialect:#?}");
}

#[test]
fn e022_duplicate_object_key_is_rejected() {
    // A mapping that declares the same key twice used to be silently collapsed (JSON: last-wins;
    // YAML: `YamlLoader` errored) — now it is uniformly rejected with a stable code and a precise
    // span at the second (duplicate) occurrence, so a duplicated `type`/`properties` name cannot
    // silently reach lowering.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Foo:
      type: object
      type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DuplicateObjectKey), "{report:#?}");
    // The diagnostic points at the duplicate `type` on line 9, not line 1.
    let dup = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::DuplicateObjectKey)
        .expect("duplicate-key diagnostic");
    assert_eq!(dup.span.expect("span").start.line, 9, "{dup:#?}");

    // check/generate parity: parsing runs before lowering, so `check` rejects identically.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::DuplicateObjectKey), "{checked:#?}");
}

#[test]
fn oas32_document_with_compatible_constructs_generates() {
    // OpenAPI 3.2 is a compatible superset of 3.1: a 3.2 document using only 3.1-compatible
    // constructs lowers through the same frontend and generates — no `E001`, no warnings.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedOpenApiVersion),
        "{report:#?}"
    );
    // check/generate parity: the same acceptance is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedOpenApiVersion),
        "{checked:#?}"
    );
}

#[test]
fn oas30_document_still_rejected_e001() {
    // Widening to accept 3.2 must not accept 3.0: it uses different schema semantics and stays
    // rejected with `E001`.
    let report = generate(
        r##"
openapi: 3.0.0
info: { title: T, version: 1.0.0 }
paths: {}
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::UnsupportedOpenApiVersion),
        "{report:#?}"
    );
}

#[test]
fn oas32_retains_the_oas31_base_dialect_identifier() {
    let accepted = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://spec.openapis.org/oas/3.1/dialect/base
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(accepted);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnsupportedDialect), "{report:#?}");

    // The 3.2 prose names only the 3.1 URI, but 3.2's own published document schema gives this one
    // as `jsonSchemaDialect`'s default, so tooling that follows the schema writes it into otherwise
    // valid 3.2 documents. Both spellings are accepted on a 3.2 document.
    let published = accepted.replace(
        "https://spec.openapis.org/oas/3.1/dialect/base",
        "https://spec.openapis.org/oas/3.2/dialect/2025-09-17",
    );
    let report = generate(&published);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnsupportedDialect), "{report:#?}");

    // That leniency is scoped to 3.2: a 3.1 document claiming the 3.2 dialect is still wrong.
    let on_31 = published.replace("openapi: 3.2.0", "openapi: 3.1.0");
    let report = generate(&on_31);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedDialect), "{report:#?}");

    // A Schema Object's own `$schema` names a dialect outright, so both spellings are accepted
    // there. The version rule belongs to `jsonSchemaDialect`, the document-level default.
    let per_schema = accepted.replace(
        "        '200': { description: ok }",
        "        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $schema: https://spec.openapis.org/oas/3.2/dialect/2025-09-17\n                type: object",
    );
    let report = generate(&per_schema);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnsupportedDialect), "{report:#?}");

    // A URI that no version of the specification defines stays `E002` either way.
    let nonexistent = accepted.replace("oas/3.1/dialect", "oas/3.2/dialect");
    let report = generate(&nonexistent);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedDialect), "{report:#?}");
}

#[test]
fn oas32_query_method_operation_generates() {
    // The OpenAPI 3.2 fixed `QUERY` path-item method is fully supported: it lowers to an operation
    // and generates a client method like any other verb — no warning, no rejection.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    query:
      operationId: searchItems
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn oas32_self_without_refs_generates_without_a_warning() {
    let spec = r##"
openapi: 3.2.0
$self: https://api.example.com/openapi.yaml
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    // check/generate parity.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::Oas32ConstructIgnored),
        "{checked:#?}"
    );
}

#[test]
fn oas32_additional_operations_generate_custom_methods() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        '200': { description: ok }
    additionalOperations:
      COPY:
        operationId: copyPets
        responses:
          '200': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    assert!(code.contains("copy_pets"), "{code}");
    assert!(code.contains("b\"COPY\""), "{code}");
}

#[test]
fn operation_ids_and_path_parameter_bindings_are_validated() {
    let duplicate_id = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /a:
    get: { operationId: same, responses: { '204': { description: ok } } }
  /b:
    get: { operationId: same, responses: { '204': { description: ok } } }
"##;
    let report = generate(duplicate_id);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");

    let missing_path_parameter = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets/{id}:
    get: { responses: { '204': { description: ok } } }
"##;
    let report = generate(missing_path_parameter);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn oas32_tag_hierarchy_is_validated_and_documented() {
    let valid = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
tags:
  - { name: api, summary: Public API, kind: nav }
  - { name: pets, parent: api, summary: Pet calls }
paths:
  /pets:
    get:
      tags: [pets]
      responses:
        '200': { summary: Listed, description: all pets }
"##;
    let (report, code) = generate_with_code(valid);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("Public API"), "{code}");
    assert!(code.contains("Response `200`: Listed"), "{code}");

    let cycle = valid.replace(
        "{ name: api, summary: Public API, kind: nav }",
        "{ name: api, parent: pets, summary: Public API, kind: nav }",
    );
    let report = generate(&cycle);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn w002_server_initiated_flow_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
webhooks:
  newThing:
    post:
      responses:
        "200": { description: OK }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ServerInitiatedFlowIgnored));
}

/// A webhook whose request and response bodies name a component the document never declares. A
/// webhook is acknowledged (`W002`) and never lowered for a client, so its schemas are never
/// resolved and the dangling reference is not `E004`: this is the "constructs never lowered for a
/// client, such as a webhook body, are unaffected" clause of `docs/support-matrix.md`'s References
/// row.
const W002_WEBHOOK_DANGLING_REF_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
webhooks:
  newThing:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Missing" }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/AlsoMissing" }
"##;

#[test]
fn w002_a_dangling_ref_in_a_webhook_body_is_not_e004() {
    let checked = check(W002_WEBHOOK_DANGLING_REF_SPEC);
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ServerInitiatedFlowIgnored),
        "{checked:#?}"
    );
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");

    let generated = generate(W002_WEBHOOK_DANGLING_REF_SPEC);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert!(
        has_code(&generated, Code::ServerInitiatedFlowIgnored),
        "{generated:#?}"
    );
    assert!(!has_code(&generated, Code::UnresolvedRef), "{generated:#?}");
}

#[test]
fn path_item_ref_resolves_into_operations() {
    // Regression: a Path Item `$ref` was ignored outright, so the path contributed no operations
    // and the client silently generated with fewer methods than the document describes.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    $ref: "#/components/pathItems/Pets"
components:
  pathItems:
    Pets:
      get:
        operationId: listPets
        responses:
          "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("list_pets"),
        "the referenced operation must be generated: {code}"
    );
}

#[test]
fn e016_path_item_ref_with_a_structural_sibling() {
    // The specification leaves `$ref` plus adjacent fields undefined, so either guess would ship a
    // client calling a different set of endpoints than the document describes.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    $ref: "#/components/pathItems/Pets"
    post:
      operationId: createPet
      responses:
        "204": { description: No Content }
components:
  pathItems:
    Pets:
      get:
        operationId: listPets
        responses:
          "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::SpecUndefinedBehavior),
            "{report:#?}"
        );
    }
}

#[test]
fn path_item_ref_keeps_documentation_siblings() {
    // `summary`/`description` cannot change the wire, so they are allowed beside a `$ref` — and
    // "allowed" has to mean applied. A Path Item resolves to one generated construct per path, so
    // the reference site's documentation has a unique home and must reach the rustdoc. Each field
    // overrides independently: `summary` is declared here and wins, `description` is not and so
    // stays the referenced item's.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    $ref: "#/components/pathItems/Pets"
    summary: Everything about pets
components:
  pathItems:
    Pets:
      summary: Component-owned summary
      description: Component-owned description
      get:
        operationId: listPets
        responses:
          "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("Everything about pets"),
        "the reference site's `summary` never reached the generated docs:\n{code}"
    );
    assert!(
        !code.contains("Component-owned summary"),
        "the reference site's `summary` did not override the target's:\n{code}"
    );
    assert!(
        code.contains("Component-owned description"),
        "an undeclared `description` must stay the referenced item's:\n{code}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
}

#[test]
fn e004_a_chained_path_item_ref_is_rejected_at_the_path() {
    // One hop is what the specification requires and what spargen follows
    // (`path_item_ref_resolves_into_operations`); a Path Item `$ref` whose target is itself a
    // `$ref` is declined rather than followed without a cycle guard. Nothing reached this site
    // before, so the decision could change unseen.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: "https://e.com" }]
paths:
  /u:
    $ref: "#/components/pathItems/A"
components:
  pathItems:
    A: { $ref: "#/components/pathItems/B" }
    B:
      get:
        operationId: getU
        responses:
          "204": { description: No Content }
"##;
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        let pointers = e004_pointers(&report, entry);
        assert!(
            pointers.contains(&"/paths/~1u"),
            "{entry}: E004 must point at the referencing path, not {pointers:?}"
        );
        assert!(
            messages_for(&report, Code::UnresolvedRef).iter().any(|m| m
                .contains("resolves to another Path Item `$ref`; chained Path Item references")),
            "{entry}: {report:#?}"
        );
    }
}

#[test]
fn info_contact_license_and_external_docs_reach_the_client_docs() {
    // All three were parsed away with no diagnostic and no rustdoc.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info:
  title: T
  version: 1.0.0
  contact: { name: API Team, email: api@example.com, url: "https://example.com/support" }
  license: { name: MIT, identifier: MIT }
externalDocs:
  description: Full guide
  url: "https://example.com/docs"
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("API Team"), "{code}");
    assert!(code.contains("License: MIT (MIT)"), "{code}");
    assert!(code.contains("https://example.com/docs"), "{code}");
}

#[test]
fn path_item_summary_and_description_reach_operation_rustdoc() {
    // A Path Item's `summary`/`description` apply to every operation on the path. They were parsed
    // only when a `$ref` was present, and then discarded, so the matrix's claim that they become
    // rustdoc never held.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    summary: Everything about pets.
    description: Shared across both operations on this path.
    get:
      operationId: listPets
      description: List them.
      responses:
        "204": { description: No Content }
    delete:
      operationId: purgePets
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("Everything about pets."),
        "path-item summary must reach rustdoc: {code}"
    );
    // Both operations carry it. (The count is doubled by the blocking facade, which mirrors every
    // method's rustdoc, so this asserts the floor rather than an exact number.)
    assert!(
        code.matches("Shared across both operations on this path.")
            .count()
            >= 2,
        "the path item's description belongs on every operation of the path: {code}"
    );
    // The operation's own documentation is not displaced by it.
    assert!(code.contains("List them."), "{code}");
}

// --- additionalOperations method tokens -----------------------------------------------------
//
// The official document schema pins these keys to an RFC 9110 token and forbids restating a fixed
// field. It once validated the *root* document only, so a Path Item reached by `$ref` into another
// file never met it; both fixtures below route through a sub-file — the path that was once
// unguarded, and on which a non-token key reached codegen and was emitted as
// `Method::from_bytes(..).expect(..)`, panicking inside the consumer's client at request time.
// The token rule is now the schema's, applied to the sub-file (#234); the case-insensitive
// fixed-field rule is spargen's own, since the schema names the fixed fields in upper case only.

#[test]
fn e011_additional_operations_method_must_be_an_http_token() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    $ref: './sub.yaml#/item'
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("sub.yaml"),
        r##"
item:
  additionalOperations:
    "pu rge":
      operationId: purgeIt
      responses:
        '204': { description: ok }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out));
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e011_additional_operations_method_must_not_restate_a_fixed_field_method() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    $ref: './sub.yaml#/item'
"##,
    )
    .unwrap();
    // Compared case-insensitively: `Get` is strictly a distinct RFC 9110 token, but no server
    // implements it and generating a second method shadowing `get` is worse than refusing.
    std::fs::write(
        dir.join("sub.yaml"),
        r##"
item:
  additionalOperations:
    Get:
      operationId: getIt
      responses:
        '204': { description: ok }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out));
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// A root document whose Responses map carries `entries` verbatim, written inline: the twin of a
/// [`generate_and_check_refd_path_item`] fixture, so the two placements can be compared.
fn inline_spec_with_response_entries(entries: &str) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200': {{ description: ok }}\n{entries}"
    )
}

/// A specification extension is skipped whatever its value is, **inline and behind a `$ref`**.
///
/// `specification-extensions` is `^x-: true` in both vendored schemas — *any* value — so an `x-`
/// entry under `responses` is not a Response and its contents are not spargen's business. Before
/// the skip, a scalar-, array- or null-valued extension was parsed as a Response and rejected with
/// `E011: expected an object`, **inline as well as behind a `$ref`**, so this is a relaxation for
/// documents that pass today and not only a consequence of the new grammar check.
///
/// This is the fixture that pins the skip's *scope*. Narrowing it to object-valued extensions —
/// the shape that already passed before this change — reverts all four relaxations and is
/// otherwise invisible to the workspace; that mutation reds exactly here.
#[test]
fn a_specification_extension_is_skipped_whatever_its_value_shape() {
    for value in [
        "hello",
        "[1, 2]",
        "null",
        "42",
        "true",
        "{ note: an object, which already passed }",
    ] {
        let refd = format!(
            "get:\n  operationId: getPet\n  responses:\n    '200': {{ description: ok }}\n    x-note: {value}\n"
        );
        let (generated, checked) = generate_and_check_refd_path_item(&refd);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "`x-note: {value}` behind a $ref: {generated:#?}"
        );
        assert!(
            generated.diagnostics().is_empty(),
            "`x-note: {value}` behind a $ref: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Clean,
            "`x-note: {value}` behind a $ref: {checked:#?}"
        );

        let inline = inline_spec_with_response_entries(&format!("        x-note: {value}\n"));
        let generated = generate(&inline);
        let checked = check(&inline);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "`x-note: {value}` inline: {generated:#?}"
        );
        assert!(
            generated.diagnostics().is_empty(),
            "`x-note: {value}` inline: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Clean,
            "`x-note: {value}` inline: {checked:#?}"
        );
    }
}

/// The skip is `^x-`, exactly as the metaschema spells it — case-sensitively. `X-note` is not a
/// specification extension: `unevaluatedProperties: false` rejects it inline and behind a `$ref`
/// alike, under `E011`, so the verdict does not depend on placement.
#[test]
fn an_uppercase_extension_key_is_not_a_specification_extension() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    X-note: { description: not an extension }\n",
    );
    let inline =
        inline_spec_with_response_entries("        X-note: { description: not an extension }\n");
    for report in [&generated, &checked, &generate(&inline), &check(&inline)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// Skipping an extension means not resolving a `$ref` inside one either. A dangling *pointer* ref
/// in arbitrary user data no longer raises `E004`, which is correct: `^x-: true` admits any value,
/// so the object is not a Reference Object and never was one — the old diagnostic was spurious.
#[test]
fn a_dangling_pointer_ref_inside_an_extension_is_not_resolved() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    x-note: { $ref: '#/does/not/exist' }\n",
    );
    let inline =
        inline_spec_with_response_entries("        x-note: { $ref: '#/does/not/exist' }\n");
    for report in [&generated, &checked, &generate(&inline), &check(&inline)] {
        assert!(report.outcome().is_success(), "{report:#?}");
        assert!(!has_code(report, Code::UnresolvedRef), "{report:#?}");
        assert!(!has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// A dangling **file** ref in the same position reaches the same verdict as the pointer ref above
/// (#239). It once rejected with `E011` "failed to read", because the bundle loader read every
/// `$ref` target before anything parsed a Responses key, so the skip never got a say; the loader
/// now stops at specification extensions itself.
#[test]
fn a_dangling_file_ref_inside_an_extension_is_not_read_like_a_pointer_ref() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    x-note: { $ref: 'nowhere.yaml' }\n",
    );
    let inline = inline_spec_with_response_entries("        x-note: { $ref: 'nowhere.yaml' }\n");
    for report in [&generated, &checked, &generate(&inline), &check(&inline)] {
        assert!(report.outcome().is_success(), "{report:#?}");
        assert!(!has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// The loader's stop is not confined to `responses`: an extension on any object whose keys are
/// fixed fields is author data wherever it sits, so no file it names is read. That includes the
/// Paths Object, whose other keys all start with `/` (#370).
#[test]
fn a_dangling_file_ref_is_not_read_in_any_extension_position() {
    let dangling = "{ $ref: 'nowhere.yaml' }";
    let head =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    let schema = "content:\n            application/json:\n              schema:\n                type: string\n";
    // (the position, the document)
    let cases = [
        ("the document root", format!("{head}x-note: {dangling}\npaths: {{}}\n")),
        ("the Info Object", format!("openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0, x-note: {dangling} }}\nservers: [{{ url: 'https://e.com' }}]\npaths: {{}}\n")),
        ("the Paths Object", format!("{head}paths:\n  x-note: {dangling}\n")),
        ("a Path Item", format!("{head}paths:\n  /pet:\n    x-note: {dangling}\n    get:\n      operationId: getPet\n      responses:\n        '200': {{ description: ok }}\n")),
        ("an Operation", format!("{head}paths:\n  /pet:\n    get:\n      operationId: getPet\n      x-note: {dangling}\n      responses:\n        '200': {{ description: ok }}\n")),
        ("a Response", format!("{head}paths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200':\n          description: ok\n          x-note: {dangling}\n")),
        ("a Schema", format!("{head}paths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200':\n          description: ok\n          {schema}                x-note: {dangling}\n")),
        ("the Components Object", format!("{head}paths: {{}}\ncomponents:\n  x-note: {dangling}\n")),
    ];
    for (position, spec) in &cases {
        for report in [&generate(spec), &check(spec)] {
            assert!(report.outcome().is_success(), "{position}: {report:#?}");
            assert!(
                !has_code(report, Code::InvalidInput),
                "{position}: {report:#?}"
            );
        }
    }
}

/// A root document with one real operation, `getPet`, and `entries` appended under `paths`.
fn spec_with_paths_entries(entries: &str) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200': {{ description: ok }}\n{entries}"
    )
}

/// The Paths Object is `patternProperties: { "^/": path-item }` plus `specification-extensions`
/// (`^x-: true`) in both vendored schemas, so an `x-` key there is author data, never a path item
/// (#370). `parse_paths` once handed it to `parse_path_item`, whatever its value: a scalar was
/// rejected with `E011: expected an object`, a dangling pointer `$ref` with `E004`, and an
/// operation-shaped value, inline or through a `$ref` that resolves, became an operation of the
/// generated client. Each of those documents is valid, and each now generates the one real
/// operation and nothing else, with no diagnostic.
#[test]
fn a_paths_object_extension_is_not_a_path_item_whatever_its_value_shape() {
    const GHOST: &str = "get: { operationId: ghost, responses: { '200': { description: ok } } }";
    for (value, rest) in [
        ("hello", ""),
        ("[1, 2]", ""),
        ("null", ""),
        ("42", ""),
        ("{ $ref: '#/nope' }", ""),
        (format!("{{ {GHOST} }}").as_str(), ""),
        (
            "{ $ref: '#/components/pathItems/Ghost' }",
            format!("components:\n  pathItems:\n    Ghost: {{ {GHOST} }}\n").as_str(),
        ),
    ] {
        let spec = spec_with_paths_entries(&format!("  x-note: {value}\n{rest}"));
        let (generated, code) = generate_with_code(&spec);
        let checked = check(&spec);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "`x-note: {value}`: {generated:#?}"
        );
        assert!(
            generated.diagnostics().is_empty(),
            "`x-note: {value}`: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Clean,
            "`x-note: {value}`: {checked:#?}"
        );
        assert!(code.contains("fn get_pet"), "`x-note: {value}`");
        assert!(!code.contains("ghost"), "`x-note: {value}`: {code}");
    }
}

/// The loader stops at a Paths Object extension as the parser does (#370), so a file ref there
/// is never read: a missing target is not the loader's `E011` "failed to read", and a present one
/// contributes no operation.
#[test]
fn a_file_ref_under_a_paths_object_extension_is_not_read() {
    const ITEM: &str = "get:\n  operationId: ghost\n  responses:\n    '200': { description: ok }\n";
    let spec = spec_with_paths_entries("  x-note: { $ref: 'item.yaml' }\n");
    for files in [
        &[("openapi.yaml", spec.as_str()), ("item.yaml", ITEM)][..],
        &[("openapi.yaml", spec.as_str())][..],
    ] {
        let (generated, checked, code) = generate_and_check_files(files);
        assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
        assert!(generated.diagnostics().is_empty(), "{generated:#?}");
        assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
        assert!(code.contains("fn get_pet"));
        assert!(!code.contains("ghost"), "{code}");
    }
}

/// The skip is `^x-`, exactly as the metaschema spells it — case-sensitively. `X-note` under
/// `paths` is neither an extension nor a path (`unevaluatedProperties: false`), so it is rejected
/// under `E011` rather than skipped or read as a path item.
#[test]
fn an_uppercase_paths_object_key_is_not_a_specification_extension() {
    let spec = spec_with_paths_entries(
        "  X-note: { get: { operationId: ghost, responses: { '200': { description: ok } } } }\n",
    );
    for report in [&generate(&spec), &check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// A root document whose `200` body is `$ref: '{target}'`, with `rest` appended at the top level.
fn root_with_body_ref(target: &str, rest: &str) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{ $ref: '{target}' }}\n{rest}"
    )
}

/// The stop has one exception, and it is what keeps it sound: a reference *into* an extension
/// (`#/x-defs/Pet`) makes that value part of the description, interpreted as whatever its site
/// expects, so the files its own references name are read — in the root document and in a
/// sub-file alike. A pet file that exists generates the client; one that does not rejects with the
/// loader's `E011`, exactly as it would outside an extension.
#[test]
fn a_file_ref_inside_an_extension_a_reference_addresses_is_still_read() {
    const PET: &str =
        "type: object\nrequired: [petName]\nproperties:\n  petName: { type: string }\n";
    let in_root = root_with_body_ref("#/x-defs/Pet", "x-defs:\n  Pet: { $ref: 'pet.yaml' }\n");
    let in_lib = root_with_body_ref("lib.yaml#/x-defs/Pet", "");
    let lib = "x-defs:\n  Pet: { $ref: 'pet.yaml' }\n";
    for (placement, files) in [
        ("in the root", vec![("openapi.yaml", in_root.as_str())]),
        (
            "in a sub-file",
            vec![("openapi.yaml", in_lib.as_str()), ("lib.yaml", lib)],
        ),
    ] {
        let mut present = files.clone();
        present.push(("pet.yaml", PET));
        let (generated, checked, code) = generate_and_check_files(&present);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "{placement}: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Clean,
            "{placement}: {checked:#?}"
        );
        assert!(
            code.contains("pet_name"),
            "{placement}: the pet file's schema reaches the client"
        );

        let (generated, checked, _) = generate_and_check_files(&files);
        for report in [&generated, &checked] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{placement}: {report:#?}"
            );
            assert!(
                messages_for(report, Code::InvalidInput)
                    .iter()
                    .any(|message| message.contains("failed to read")
                        && message.contains("pet.yaml")),
                "{placement}: {report:#?}"
            );
        }
    }
}

/// `x-` is an extension only where keys are fixed fields. Where they are names the author chose —
/// a response header, a schema property, a component — `x-rate-limit` is an entry like any other,
/// and the file its `$ref` names is read: the dangling spelling of each rejects with `E011`.
#[test]
fn a_file_ref_under_an_x_named_entry_is_still_read() {
    let header = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200':\n          description: ok\n          headers:\n            x-rate-limit: { $ref: 'part.yaml' }\n";
    let property = root_with_body_ref(
        "#/components/schemas/Pet",
        "components:\n  schemas:\n    Pet:\n      type: object\n      properties:\n        x-owner: { $ref: 'part.yaml' }\n",
    );
    let component = root_with_body_ref(
        "#/components/schemas/x-pet",
        "components:\n  schemas:\n    x-pet: { $ref: 'part.yaml' }\n",
    );
    for (entry, spec, part) in [
        ("a response header", header, "schema: { type: integer }\n"),
        ("a schema property", property.as_str(), "type: string\n"),
        ("a component", component.as_str(), "type: string\n"),
    ] {
        let (generated, checked, _) =
            generate_and_check_files(&[("openapi.yaml", spec), ("part.yaml", part)]);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "{entry}: {generated:#?}"
        );
        assert!(checked.outcome().is_success(), "{entry}: {checked:#?}");

        let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", spec)]);
        for report in [&generated, &checked] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
            assert!(
                messages_for(report, Code::InvalidInput)
                    .iter()
                    .any(|message| message.contains("failed to read")
                        && message.contains("part.yaml")),
                "{entry}: {report:#?}"
            );
        }
    }
}
