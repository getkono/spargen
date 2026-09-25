//! Per-diagnostic frontend coverage: one minimal inline spec per rejection/warning code, asserting
//! the code fires and the pipeline outcome is what the taxonomy promises. Rejections travel through
//! `generate`. Check/generate parity is a property over `PARITY_FIXTURES` rather than a remark
//! about one case: every fixture there must reach the same accept/reject verdict and report the
//! same codes through both entry points, and a companion test keeps that set spanning rejections,
//! warnings and clean runs so it cannot pass vacuously.

use camino::Utf8PathBuf;
use spargen::{Build, CargoIntegration, Code, Outcome, Report, Severity, Spec};

/// Run `generate` on an inline spec written into a throwaway tempdir, returning the report. The
/// tempdir (and any written output) is discarded once the report — which owns its data — is built.
/// A build for a fixture spec. These tests are not build scripts, so the Cargo integration is
/// explicitly off: no rebuild triggers to emit, no consumer manifest to audit, and — the reason it
/// matters here — no `W013` polluting the diagnostics a fixture is asserting on.
fn build(spec: Utf8PathBuf, out: Utf8PathBuf) -> Build {
    Spec::new(spec).build(out).cargo(CargoIntegration::Off)
}

fn generate(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    let out = temp.path().join("client.rs");
    spargen::generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out).unwrap(),
    ))
}

/// As [`generate`], but through the `check` entry point (no codegen/emit).
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    spargen::check(&Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap()))
}

fn generate_with_code(spec: &str) -> (Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
    ));
    let code = std::fs::read_to_string(out).unwrap_or_default();
    (report, code)
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics().iter().any(|d| d.code == code)
}

/// Every message a report carries for one code. A code being right is not the same as its message
/// being true — a diagnostic that fires on the correct construct while asserting something false
/// about it is still a defect — so the fixtures that pin wording assert on this, not on the code.
fn messages_for(report: &Report, code: Code) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// Everything from the generated `types` module to the end of the file, with the provenance header
/// and the embedded runtime before it stripped.
///
/// It is **not** bounded at the module's closing brace: the returned text also carries the
/// `Client` impl and the emitted scaffolding after `types`. A `.contains(…)` on it can therefore
/// match client code rather than a lowered type; a fixture that must pin a lowered type should
/// match a declaration (`pub struct X`, `pub type X =`) rather than a bare type name.
///
/// Two generations of the *same* spec already differ as whole files: the header carries the output
/// path and a per-run `input-sha256`/`content-sha256`. So a whole-file comparison between two
/// generated clients is unconditionally true and proves nothing. Comparing from `pub mod types {`
/// onward compares what the two documents actually lowered to.
fn types_module(code: &str) -> String {
    code.find("pub mod types {")
        .map(|start| code[start..].to_owned())
        .unwrap_or_default()
}

/// The name of the `pub struct` that declares the first field line starting with `field`.
///
/// A type count plus "both fields exist somewhere" is satisfied by either assignment of two names to
/// two schemas, so it cannot see a swap. This answers the question the count cannot: which generated
/// type a given field belongs to.
fn field_owner(code: &str, field: &str) -> Option<String> {
    let mut current: Option<String> = None;
    for line in code.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            current = rest
                .split([' ', '<', '{', '(', ';'])
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
        }
        if trimmed.starts_with(field) {
            return current;
        }
    }
    None
}

/// The declared type of the first field line starting with `field`, trimmed of its trailing comma.
///
/// [`field_owner`] says which type *declares* a field; this says which type the field *is*. Asking
/// only whether a type name appears somewhere in the module cannot tell two same-named schemas
/// apart, because both are emitted — so a fixture that must pin *which* of them a reference bound
/// has to read the field's own right-hand side.
fn field_type(code: &str, field: &str) -> Option<String> {
    code.lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_once(':'))
        .map(|(_, ty)| ty.trim().trim_end_matches(',').to_owned())
}

/// The field names `pub struct ty` declares, in source order.
///
/// The inverse of [`field_owner`], and the answer to "which schema is this type", which a name
/// alone cannot give when two declarations share a name and the emitter disambiguates one of them —
/// or when the alias path re-emits a target's *kind* under a third name.
fn declared_fields(code: &str, ty: &str) -> Vec<String> {
    let mut lines = code
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with(&format!("pub struct {ty} ")));
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|rest| rest.split_once(':'))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// The variant declarations `pub enum ty` carries, in source order, each trimmed of its trailing
/// comma.
///
/// The union counterpart of [`declared_fields`], and the only way to see a union *member* go
/// missing. Binding the enum's name says the union was represented; it does not say how many
/// branches survived, and a collapse that erases one member leaves a type with the right name and
/// the wrong contents.
fn enum_variants(code: &str, ty: &str) -> Vec<String> {
    let mut lines = code
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with(&format!("pub enum {ty} ")));
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter(|line| !line.is_empty() && !line.starts_with("#[") && !line.starts_with("///"))
        .map(|line| line.trim_end_matches(',').to_owned())
        .collect()
}

/// The names of `pub struct`s in generated source that begin with `prefix` and whose remainder
/// satisfies `suffix_ok`, in source order.
///
/// Counting `code.matches("pub struct Foo")` is the obvious thing and is wrong twice over: the
/// generated module embeds the runtime, whose own items can share a prefix (`pub struct L` also
/// matches `LinkPaginator`), and a count alone cannot say *which* types were emitted when it
/// disagrees. Returning the names makes a failure legible and makes an off-by-a-constant bound
/// impossible to mistake for a bound.
fn declared_types(code: &str, prefix: &str, suffix_ok: impl Fn(&str) -> bool) -> Vec<String> {
    code.lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub struct "))
        .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
        .filter(|name| !name.is_empty())
        .filter_map(|name| name.strip_prefix(prefix).map(|tail| (name, tail)))
        .filter(|(_, tail)| suffix_ok(tail))
        .map(|(name, _)| name.to_owned())
        .collect()
}

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
fn response_description_requirement_is_version_gated() {
    let body = r#"
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      responses:
        '200': { summary: ok }
"#;
    let oas31 = generate(&format!("openapi: 3.1.0\n{body}"));
    assert_eq!(oas31.outcome(), Outcome::Rejected, "{oas31:#?}");
    assert!(has_code(&oas31, Code::InvalidInput), "{oas31:#?}");

    let oas32 = generate(&format!("openapi: 3.2.0\n{body}"));
    assert_ne!(oas32.outcome(), Outcome::Rejected, "{oas32:#?}");
}

#[test]
fn boolean_false_schema_lowers_to_an_uninhabited_type() {
    let spec = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /forbidden:
    get:
      responses:
        '200':
          description: impossible
          content:
            application/json:
              schema: false
"#;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("enum ResponseBody"), "{code}");
}

#[test]
fn multi_type_array_generates_a_typed_union() {
    let spec = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /value:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: [string, integer] }
"#;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("enum ResponseBody"), "{code}");
    assert!(!code.contains("serde_json :: Value"), "{code}");
}

#[test]
fn schema_ref_siblings_are_intersected_not_dropped() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /extended:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Extended' }
components:
  schemas:
    Base:
      type: object
      properties: { id: { type: string } }
      required: [id]
    Extended:
      $ref: '#/components/schemas/Base'
      type: object
      properties: { extra: { type: integer } }
      required: [extra]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub id"), "{code}");
    assert!(code.contains("pub extra"), "{code}");
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

/// The negative control for the rejection above: this change turns a previously-silent success
/// into a rejection, so what it must NOT do is reject a `$ref` that resolves. A `$ref` carrying
/// shape siblings is the narrow case — it is the one construct that reaches `ensure_component`
/// through `lower_schema_inner`, and it is an intersection, so its target contributes fields rather
/// than replacing it. Both sides must survive into the generated type.
#[test]
fn a_ref_with_shape_siblings_that_resolves_is_not_rejected() {
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
            application/json: { schema: { $ref: '#/components/schemas/Ext' } }
components:
  schemas:
    Ext:
      $ref: '#/components/schemas/Base'
      type: object
      properties: { extra: { type: string } }
    Base:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    // The reference was genuinely followed, not merely tolerated: the sibling's own property and
    // the referenced component's property are both present.
    assert!(code.contains("pub extra"), "{code}");
    assert!(code.contains("pub id"), "{code}");

    // check/generate parity on the clean path too.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");
}

/// A same-file `#/components/schemas/…` fragment that addresses a *subschema* rather than a
/// top-level component name. spargen matches these by name only, so this is rejected — but the
/// component it starts from is declared, and the identical pointer written against a relative file
/// resolves through the resolver, so the message must not claim the target does not exist.
///
/// This pins a deliberate decision that nothing else constrains: the whole test tree contains no
/// other `$ref` with a `/` inside the component name, so routing these to the resolver instead
/// would flip a user-visible verdict with no test noticing.
#[test]
fn a_same_file_ref_into_a_component_subschema_is_rejected_as_not_a_component_name() {
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
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let subschema: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .collect();
        assert!(!subschema.is_empty(), "{entry}: {report:#?}");
        // `Envelope` IS declared, so the diagnostic must say the fragment is not a component name
        // rather than that the target could not be found.
        assert!(
            subschema
                .iter()
                .any(|d| d.message.contains("addresses a subschema")),
            "{entry}: the message must not claim the target is missing — `Envelope` is declared: \
             {report:#?}"
        );
        assert!(
            !subschema.iter().any(|d| d.message.contains("unresolved")),
            "{entry}: {report:#?}"
        );
        // This arm threads the `$ref` site's provenance exactly as the plain-name arm does, and for
        // the same reason: a root pointer is one `omittable_enclosing` maps to `None`, which turns
        // `--carve` on this document from clean into an un-carvable rejection.
        assert!(
            subschema.iter().any(|d| d.pointer.as_str()
                == "/paths/~1u/get/responses/200/content/application~1json/schema"),
            "{entry}: the subschema rejection must point at the `$ref` site, not at {:?}: \
             {report:#?}",
            subschema
                .iter()
                .map(|d| d.pointer.as_str())
                .collect::<Vec<_>>()
        );
        // The message makes two separate claims — which reference could not be followed, and which
        // component it was found to address a subschema of — and it interpolates `Envelope` for
        // both. `contains("Envelope")` is therefore satisfied by either half alone, so it pins
        // neither; both mutations survived it. Assert the two independently.
        assert!(
            subschema.iter().any(|d| d
                .message
                .contains("`#/components/schemas/Envelope/properties/payload`")),
            "{entry}: the message must name the whole reference that could not be followed, not \
             only the component it starts from: {report:#?}"
        );
        assert!(
            subschema
                .iter()
                .any(|d| d.message.contains("component `Envelope`")),
            "{entry}: the message must name the component it did find, not merely describe the \
             shape: {report:#?}"
        );
        // And carry the remedy, as the other rejections in this file do.
        assert!(
            subschema.iter().any(|d| d
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains("declare the subschema"))),
            "{entry}: the rejection must carry its remedy: {report:#?}"
        );
    }

    // A deep pointer whose ROOT SEGMENT is not declared is a different fault and must not borrow
    // this message. `Envelop` is a typo for `Envelope`; the document declares nothing by that name,
    // so "addresses a subschema" would assert by implication that it is there, and the remedy
    // "declare the subschema as its own entry" would send the reader to promote a subschema of a
    // component that does not exist. The `/` in the fragment is not what is wrong with it.
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
            e004.iter()
                .any(|d| d.message.contains("unresolved schema reference")),
            "{entry}: an undeclared root segment is an unresolved reference, not a fragment-shape \
             problem: {report:#?}"
        );
        assert!(
            !e004
                .iter()
                .any(|d| d.message.contains("addresses a subschema")),
            "{entry}: `Envelop` is not declared, so nothing was addressed inside it: {report:#?}"
        );
    }

    // A trailing slash is a real subschema fragment: `Foo` IS declared and the pointer's final
    // empty reference token addresses its `""`-keyed member, so this keeps the subschema wording.
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
            .any(|d| d.code == Code::UnresolvedRef && d.message.contains("addresses a subschema")),
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
/// E004 fired that miss was a silent drop — the property simply vanished — which is the same bug
/// this branch is about, just reached from a sub-file.
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
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    // The sibling was genuinely followed: `Inner`'s own field reached the generated type, so the
    // property is typed rather than dropped.
    assert!(code.contains("pub inner"), "{code}");
    assert!(code.contains("pub id"), "{code}");

    // `check` must agree — it runs the same lowering.
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");
}

/// Build a two-file description in a throwaway tempdir and run it through both entry points.
///
/// The root document is fixed — one operation whose `200` body `$ref`s `target` — and `lib` is
/// written beside it as `lib.yaml`. Every shape below differs only in that sub-file, so what a
/// fixture pins is the sub-file's own reference behaviour and nothing else. The tempdir is dropped
/// on return; the report owns its data and the emitted source is read out first.
fn split(target: &str, lib: &str) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
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
             application/json:\n              \
             schema: {{ $ref: '{target}' }}\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("lib.yaml"), lib).unwrap();
    let out = dir.join("client.rs");
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
    (generated, checked, code)
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
        let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
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

/// One sub-file schema carrying `xml.name`/`xml.attribute`, used as the **XML** body of one
/// operation and the **JSON** body of another.
///
/// A serde `rename` applies to every format, so `gate_xml_field_renames` suppresses XML hints on any
/// type that is not used exclusively as an XML body. That policy is right and pre-dates this branch.
/// What changed is what it sees: before the resolved-reference memo, the two operations lowered the
/// sub-file schema to two types — the XML one dedicated and keeping `#[serde(rename = "@Ident")]`,
/// the JSON one suppressed — and now they share one type, which is reachable from both and is
/// therefore suppressed for both. **The XML on the wire moved**, and the `W006` count did not change,
/// so an upgrading consumer had nothing to compare.
///
/// The verdict is not being reversed here: giving an XML use its own type would reintroduce two
/// types for one target, which is the defect this branch exists to remove. What is being fixed is
/// that the warning must say which of its two quite different situations it is in, so a consumer can
/// tell "your hint was inert" from "your XML body's field names just changed".
#[test]
fn a_schema_shared_between_an_xml_and_a_non_xml_body_says_so() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /xml:
    get:
      operationId: getXml
      responses:
        '200':
          description: ok
          content:
            application/xml: { schema: { $ref: './lib.yaml#/components/schemas/Item' } }
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './lib.yaml#/components/schemas/Item' } }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        r##"
components:
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id:
          type: string
          xml: { name: Ident, attribute: true }
"##,
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap();

    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // One target, one type — the repair this branch exists for, unchanged.
    assert_eq!(
        declared_types(&code, "Item", |tail| tail.is_empty()).len(),
        1,
        "{code}"
    );
    // And the warning names the situation it is actually in.
    let shared: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::XmlHintIgnored)
        .collect();
    assert!(!shared.is_empty(), "{report:#?}");
    assert!(
        shared.iter().any(|d| d
            .message
            .contains("shared between an XML body and a non-XML")),
        "the schema IS used as an XML body, so the warning must say the XML body's own field \
         names are affected rather than that the hint was never reachable: {report:#?}"
    );

    // The control, and the other half of the same `W006`: a schema carrying XML hints that is never
    // used as an XML body at all. Its hint is inert, nothing on any wire moved, and it must NOT
    // borrow the shared wording — otherwise one message covers both and pins neither.
    let inert = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /json:
    get:
      operationId: getJson
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Item' } }
components:
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id:
          type: string
          xml: { name: Ident, attribute: true }
"##;
    let report = generate(inert);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let inert_warnings: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::XmlHintIgnored)
        .collect();
    assert!(!inert_warnings.is_empty(), "{report:#?}");
    assert!(
        inert_warnings.iter().all(|d| !d
            .message
            .contains("shared between an XML body and a non-XML")),
        "this schema is never an XML body, so nothing was shared: {report:#?}"
    );
    assert!(
        inert_warnings
            .iter()
            .any(|d| d.message.contains("never used as an XML body")),
        "{report:#?}"
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
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /a:
    get:
      operationId: getA
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './a.yaml#/components/schemas/Shape' } }
  /b:
    get:
      operationId: getB
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: './b.yaml#/components/schemas/Shape' } }
"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("a.yaml"),
        "components:\n  schemas:\n    Shape:\n      type: object\n      required: [alpha]\n      \
         properties: { alpha: { type: string } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.yaml"),
        "components:\n  schemas:\n    Shape:\n      type: object\n      required: [beta]\n      \
         properties: { beta: { type: integer } }\n",
    )
    .unwrap();
    let out = dir.join("client.rs");
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();

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
    // API fact a consumer writes into their own code.
    //
    // This pins one ordering. It does **not** pin that the assignment survives reordering the
    // document: `Scope::alloc` hands the un-suffixed name to whichever schema is allocated first,
    // before it reads provenance at all, so swapping these two `paths` entries swaps which schema
    // is called `Shape`. That is disclosed rather than repaired — see this pull request's
    // `## Unresolved review notes`.
    assert_eq!(
        field_owner(&code, "pub alpha:").as_deref(),
        Some("Shape"),
        "the first-allocated schema owns the un-suffixed name: {code}"
    );
    assert_eq!(
        field_owner(&code, "pub beta:").as_deref(),
        Some("Shape93360b5f"),
        "and the second carries the pointer-seeded disambiguator: {code}"
    );
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
    // it reproduces identically on the merge base, so the fault is older than this branch.
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

/// A `oneOf` one of whose members is the union itself.
///
/// The member is a cycle-closing back-edge, so its `Ty` points at the union's own reservation. The
/// emitted `Deserialize` therefore opens with `serde_json::from_value::<Box<Loop>>(value.clone())`
/// — **the same impl, on the same value, with no base case** — so every decode recurses until the
/// stack is exhausted. It compiles, and the `e2e` gate compiles generated output rather than
/// decoding through every type, so nothing could have caught it.
///
/// A variant that *is* the whole union constrains nothing and cannot be decoded, so the right answer
/// is a rejection. Both spellings and the root document are pinned; as with the sibling case above,
/// the root form reproduces byte-identically on `2aa5ada`.
#[test]
fn a_union_variant_that_is_the_union_itself_is_rejected() {
    const LIB: &str = r##"
components:
  schemas:
    Loop:
      oneOf:
        - { $ref: 'PREFIX#/components/schemas/Loop' }
        - { type: string }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Loop",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a variant that is the whole union decodes by re-entering its \
                 own `Deserialize` on the same value: {report:#?}"
            );
            assert!(
                has_code(report, Code::NonDisjointUnion),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        // The runtime shape itself: nothing may emit a `Deserialize` arm that calls back into the
        // same type on the same value. That is what makes this a hang rather than a wrong type.
        assert!(
            !code.contains("from_value::<Box<Loop>>"),
            "{spelling}: the emitted decoder re-enters itself with no base case: {code}"
        );
    }

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
            application/json: {{ schema: {{ $ref: '#/components/schemas/Loop' }} }}
{}"##,
        LIB.replace("PREFIX", "")
    );
    let report = generate(&root);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");
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
        let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
        let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
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
/// map back through `ensure_component`, so `components` stays that target's single identity. Round 4
/// filed this branch as "executes but constrains nothing". That reading was wrong: disabling the
/// branch leaves every suite green and gives **`["RootOne", "RootOne55e60dbe"]`** — two public types
/// for one declared component, which is the precise defect this change exists to remove, in a shape
/// it wrote a dedicated branch for.
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
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
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

/// A **root-only** document — no sub-files, no remote refs — whose `allOf` member reaches the
/// component being lowered through a component **alias**.
///
/// This is the shape the breaking-change footer's scope statement missed. `gather_member`'s
/// pre-existing guard keys on the member's own *name*: `Alias` is not in `in_progress`, so it never
/// fired. `ensure_component("Alias")` then chains to `Node`, which **is** in progress, and hands
/// back a back-edge against `Node`'s reservation, whose placeholder `push_ref_member` read as a
/// scalar.
///
/// So this document is `clean` on `2aa5ada` and rejected here, verified by building the merge base
/// and running it. The rejection is right — base emitted `serde_json::Value` for a typed schema with
/// no diagnostic — but "regenerating from an unchanged description is otherwise unaffected" was not,
/// and this is a description that uses none of the multi-file machinery the change is about.
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
/// Every row below was measured against a build of the merge base. The D8 rows are the ones that
/// must still reject; everything else must still generate.
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
/// Restoring the merge base's behaviour is **not** the fix. At `2aa5ada` a reservation's kind was
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
        // And never the merge base's answer.
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
        // back-edge. The `Option` is then missing, and that is WRONG: `B`'s `"null"` member makes
        // `B` nullable, so `{"b": null}` is legal against this document and will not decode. It is
        // pinned as it is emitted rather than as it ought to be, because fixing it is issue #222 —
        // `ensure_component` overwrites a body's computed nullability with the value
        // `schema_is_nullable` produced at reserve time, and that function inspects `type`, `enum`
        // and `const` only, never a union's members. It reproduces on master with a document that
        // has no recursion in it at all, so it is not this change's to fix; when #222 lands this
        // expectation becomes `Option<Box<B>>`. Until then this assertion is the only thing
        // anywhere in the repository standing over what that order emits.
        // One message per order. The two orders are pinned for opposite reasons — one is the
        // correct type, one is the knowingly-wrong one — and a single format string over both
        // hands a reader of the first iteration a paragraph written about the second, 3,000
        // output lines above the `left`/`right` that would correct it.
        let (expected, why) = if first == "A" {
            (
                "Option<Box<A>>",
                "this order takes the nullable-alias path, so `b` binds `A` itself — optional \
                 because of the `\"null\"` member, boxed because the cycle must have a finite \
                 size. This is the correct type and is NOT issue #222: a red here is a \
                 regression in the alias path, not a pin that needs updating",
            )
        } else {
            (
                "Box<B>",
                "this order's expectation is the knowingly-wrong `Box<B>` that issue #222 \
                 exists to fix, pinned as emitted rather than as it ought to be — if this went \
                 red while fixing #222, the expectation becomes `Option<Box<B>>`",
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
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
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
        let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));

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
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));

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

/// The aliased spelling of [`a_nullable_alias_carries_its_targets_own_nullability`]'s document,
/// hoisted so [`PARITY_FIXTURES`] can drive it through `check` as well as `generate`.
///
/// That fixture reads emitted source, so it can only call `generate`; `PARITY_FIXTURES` is where a
/// spec is held to reporting the same thing through both entry points, and it is a hand-maintained
/// list, so an omission costs nothing and warns nobody. The fixture asserts this constant equals
/// what its own builder produces, so the two cannot drift apart.
const NULLABLE_ALIAS_CARRY_SPEC: &str = "openapi: 3.1.0
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
      type: [object, 'null']
      required: [b]
      properties:
        x: { type: string }
        b: { $ref: '#/components/schemas/B' }
    B:
      oneOf:
        - { $ref: '#/components/schemas/A' }
";

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
            // sibling-intersection question, and `lower.rs` records at that site that a reordering
            // turns this fixture red. A disjunction would not: it is satisfied by either verdict,
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

/// A union that collapses to its sole non-null member, whose intersection with the enclosing
/// schema's own sibling keywords is irreconcilable (`type: object` against `type: string`), leaves
/// the union with no variant. Before the rejection existed, the collapse `?`-propagated the failed
/// intersection with no diagnostic, so the run came back clean with the response body silently
/// dropped — the fourth behaviour. It must be refused with `E007` at the schema that carries the
/// union, through both entry points.
#[test]
fn a_sole_union_member_irreconcilable_with_its_siblings_is_rejected() {
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
                properties:
                  a: { type: string }
                oneOf:
                  - { type: string }
                  - { type: "null" }
"##;
    let pointer = "/paths/~1u/get/responses/200/content/application~1json/schema";
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e007: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::NonDisjointUnion)
            .collect();
        assert!(
            e007.iter().any(
                |d| d.pointer.as_str() == pointer && d.message.contains("sole non-null member")
            ),
            "{entry}: E007 for the empty sole-member intersection must point at `{pointer}`, \
             not at {:?}\n{report:#?}",
            e007.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
        );
    }
}

/// A union whose sibling keywords would have to be intersected against a target that is still being
/// lowered. Nothing true can be said about that intersection, so it must be refused — not guessed
/// at, and not quietly dropped.
///
/// Three positions, all of which the merge base accepted by guessing: the sole member of a nested
/// property's union, the sole member of an array `items` union, and one member of a multi-member
/// union. At `2aa5ada` `intersect_non_null`'s `TypeKind::Any` arm absorbed the placeholder and
/// returned the *sibling*, silently retyping the recursive branch to the inline object beside it.
/// At this head the multi-member case instead drops the variant with a `W011` whose message —
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
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
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
        let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));

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
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(out).unwrap();
    assert!(code.contains("pub id"), "{code}");
    assert!(!code.contains("serde_json :: Value"), "{code}");
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
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let code = std::fs::read_to_string(out).unwrap();
    assert!(code.contains("pub id"), "{code}");
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
            spargen::check(&spec)
        } else {
            spargen::generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
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
            spargen::check(&spec)
        } else {
            spargen::generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
        };
        (report, temp, out)
    }

    /// The remote counterpart of the direct-recursive `allOf` member, reached through an **alias**,
    /// which is the shape that needs the id-keyed guard rather than the spelling-keyed one.
    ///
    /// `gather_member`'s remote arm has two checks. The pre-existing one keys on the reference
    /// *string* — `remote_in_progress.contains_key(reference)` — and this branch added a second
    /// keyed on the returned `Ty`'s id. Only the second can see this case: `node.yaml` composes
    /// `allOf: [alias.yaml]`, `alias.yaml` is a bare `$ref` back to `node.yaml`, so the member's own
    /// spelling is never the in-progress key, and `ensure_remote` chains through the alias and hands
    /// back a back-edge against `node.yaml`'s reservation.
    ///
    /// **Removing the id-keyed check leaves every other test in the workspace green.** Without it
    /// this document generates, with zero diagnostics, and emits
    /// `pub type …child = serde_json::Value;` — the same silent degradation the component path was
    /// repaired for in this branch, on a path nothing exercised. The guard was added here; the
    /// fixture was not.
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
fn oas32_querystring_param_generates_a_typed_argument() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: q
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
      responses:
        '200': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
    assert!(code.contains("pub q: Option<types::Q>"), "{code}");
    assert!(code.contains("serialize_form"), "{code}");
    assert!(code.contains("build_url_with_query_string"), "{code}");
}

#[test]
fn oas32_querystring_and_named_query_are_rejected_together() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      parameters:
        - name: whole
          in: querystring
          content:
            application/json:
              schema: { type: object }
        - name: page
          in: query
          schema: { type: integer }
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
}

#[test]
fn oas32_cookie_style_generates_cookie_header_serialization() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /prefs:
    get:
      parameters:
        - name: prefs
          in: cookie
          style: cookie
          required: true
          schema:
            type: object
            properties: { theme: { type: string }, compact: { type: boolean } }
      responses:
        '200': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedParameterStyle),
        "{report:#?}"
    );
    assert!(code.contains("join(\"; \")"), "{code}");
}

#[test]
fn oas32_component_media_type_references_generate_typed_bodies() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /pet:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/PetJson'
components:
  mediaTypes:
    PetJson:
      schema:
        type: object
        properties: { id: { type: string } }
        required: [id]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub id"), "{code}");
    assert!(!code.contains("serde_json :: Value"), "{code}");
}

#[test]
fn oas32_stream_item_schema_types_the_stream_not_dropped() {
    // OpenAPI 3.2 gives a sequential/streaming media its per-item type in `itemSchema` (not
    // `schema`). A `text/event-stream` response typed only via `itemSchema` must lower to a typed
    // streaming body — the operation still generates, the item type is NOT dropped to a bodyless
    // `()`, and no `itemSchema` warning fires (on streaming media it IS used).
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        '200':
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/Event"
components:
  schemas:
    Event:
      type: object
      required: [seq]
      properties:
        seq: { type: integer }
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
fn oas32_sse_json_content_schema_types_the_payload() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamAdminEvents
      responses:
        '200':
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/SseEnvelope"
components:
  schemas:
    SseEnvelope:
      type: object
      required: [data]
      properties:
        data:
          type: string
          contentMediaType: application/json
          contentSchema:
            $ref: "#/components/schemas/AdminEvent"
        id: { type: string }
        retry: { type: integer }
    AdminEvent:
      type: object
      required: [kind, libraryId]
      properties:
        kind: { type: string, const: scanStarted }
        libraryId: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::ValidationKeywordIgnored),
        "consumed SSE content annotations must not warn: {report:#?}"
    );
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("EventStream < types :: AdminEvent >")
            || flat.contains("EventStream<types::AdminEvent>"),
        "contentSchema must become the stream payload type: {flat}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::ValidationKeywordIgnored),
        "check/generate must agree that the SSE content annotations are consumed: {checked:#?}"
    );
}

#[test]
fn content_schema_outside_sse_remains_an_explicit_warning() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /value:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  encoded:
                    type: string
                    contentMediaType: application/json
                    contentSchema: { type: object, properties: { id: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "check/generate must report the same content annotation warning: {checked:#?}"
    );
}

#[test]
fn oas32_json_sequence_item_schema_generates_rfc7464_streaming() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json-seq:
              itemSchema:
                type: object
                properties: { id: { type: integer } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("Framing::JsonSequence"), "{code}");
}

#[test]
fn oas32_sequential_schema_is_not_misread_as_an_item_schema() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          description: ok
          content:
            application/jsonl:
              schema:
                type: array
                items: { type: string }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn explicit_media_encoding_generates() {
    // The Encoding Object's RFC 6570 mode: an explicit `style`/`explode` selects query-style
    // serialization for that property and makes `contentType` inert.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties: { tags: { type: array, items: { type: string } } }
            encoding:
              tags: { style: form, explode: true }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }
}

#[test]
fn multipart_encoding_content_type_generates() {
    // The Encoding Object's media-type mode: each part is sent as its declared `contentType`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                sdp: { type: string }
                session: { type: object, properties: { id: { type: string } } }
            encoding:
              sdp: { contentType: application/sdp }
              session: { contentType: application/json }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_on_a_json_body_has_no_effect() {
    // `encoding` applies only to form and multipart content; elsewhere the specification says it
    // SHALL be ignored, so it is acknowledged rather than rejected.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
            encoding:
              a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_entry_without_a_property_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties: { a: { type: string } }
            encoding:
              missing: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_nested_encoding_object() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties: { part: { type: object, properties: { a: { type: string } } } }
            encoding:
              part:
                contentType: multipart/mixed
                encoding:
                  a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_wildcard_encoding_content_type() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties: { image: { type: string, contentEncoding: base64 } }
            encoding:
              image: { contentType: "image/*" }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_form_urlencoded_body_requires_an_object_schema() {
    // A non-object form body used to compile and then fail at runtime inside the form encoder.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema: { type: string }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn oas32_discriminator_default_mapping_generates_a_fallback_branch() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Cat' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        defaultMapping: Dog
    Cat: { type: object, properties: { kind: { const: cat } } }
    Dog: { type: object, properties: { kind: { type: string } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
}

#[test]
fn e007_discriminator_default_mapping_outside_the_union() {
    // A fallback naming a schema that is not a member describes a branch the generated enum does
    // not have, so it cannot be quietly downgraded to another dispatch strategy.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - { $ref: '#/components/schemas/Cat' }
        - { $ref: '#/components/schemas/Dog' }
      discriminator:
        propertyName: kind
        defaultMapping: Fish
    Cat: { type: object, properties: { kind: { const: cat } } }
    Dog: { type: object, properties: { kind: { type: string } } }
    Fish: { type: object, properties: { kind: { const: fish } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
}

#[test]
fn oas32_xml_attribute_node_type_maps_to_the_existing_typed_xml_path() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: attribute } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    assert!(code.contains("@id"), "{code}");
}

/// OpenAPI 3.2 says `attribute`/`wrapped` MUST NOT be present when `nodeType` is. The two
/// spellings can disagree and the specification names no winner, so the document is rejected
/// rather than resolved by a guess.
#[test]
fn e011_xml_deprecated_flag_beside_node_type_is_rejected() {
    for (deprecated, node_type) in [("attribute: true", "element"), ("wrapped: true", "element")] {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id:
                    type: array
                    items: {{ type: string }}
                    xml: {{ nodeType: {node_type}, {deprecated} }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{deprecated}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::InvalidInput),
                "{deprecated}: {report:#?}"
            );
        }
    }
}

/// The specification enumerates exactly five `nodeType` values, and the document schema does not
/// validate Schema Objects, so nothing else catches an unknown one. It must take a disposition
/// rather than being ignored — `E009` on a type actually serialized as XML.
#[test]
fn e009_unknown_xml_node_type_is_dispositioned_not_ignored() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: fragment } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

/// The same unknown hint on a type that is never serialized as XML genuinely has no effect, so it
/// warns rather than rejecting — the existing `W006` path, now reached by unknown values too.
#[test]
fn w006_unknown_xml_node_type_on_a_never_xml_type_warns() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  id: { type: string, xml: { nodeType: fragment } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn conditional_schema_keywords_warn_and_their_children_are_audited() {
    let warned = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Conditional:
      type: object
      if: { required: [kind] }
      then: { properties: { value: { type: string } } }
"##;
    let report = generate(warned);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );

    let dynamic = warned.replace(
        "then: { properties: { value: { type: string } } }",
        "then: { $dynamicRef: '#node' }",
    );
    let report = generate(&dynamic);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DynamicRefRejected), "{report:#?}");
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
fn operation_parameters_override_matching_path_item_parameters() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    parameters:
      - { name: limit, in: query, schema: { type: integer } }
    get:
      parameters:
        - { name: limit, in: query, schema: { type: string } }
      responses: { '204': { description: ok } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(code.matches("pub limit:").count(), 1, "{code}");
}

#[test]
fn response_component_aliases_resolve_and_cycles_reject() {
    let base = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200': { $ref: '#/components/responses/A' }
components:
  responses:
    A: { $ref: '#/components/responses/B' }
    B:
      summary: shared result
      description: ok
      content:
        application/json: { schema: { type: string } }
"##;
    let (report, code) = generate_with_code(base);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("shared result"), "{code}");

    let cycle = base.replace(
        "B:\n      summary: shared result\n      description: ok\n      content:\n        application/json: { schema: { type: string } }",
        "B: { $ref: '#/components/responses/A' }",
    );
    let report = generate(&cycle);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
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
fn oas32_item_schema_on_non_streaming_media_warns_w010() {
    // `itemSchema` is only meaningful for sequential/streaming media. On a plain JSON response it is
    // acknowledged with `W010` (not silently dropped) and generation still succeeds via `schema`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: string }
              itemSchema: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
}

#[test]
fn validation_keywords_in_reusable_stream_item_schema_warn_w001() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        '200':
          content:
            application/x-ndjson:
              $ref: '#/components/mediaTypes/EventStream'
components:
  mediaTypes:
    EventStream:
      itemSchema: { type: string, minLength: 1 }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
}

#[test]
fn pattern_properties_lowers_to_typed_map_with_w001() {
    // A representable `patternProperties` now GENERATES a typed overflow map instead of being
    // rejected. Two inline `{type: string}` value schemas under different patterns collapse to one
    // `BTreeMap<String, String>` (bounded structural equivalence over leaf primitives). The key
    // regexes are validation-only and acknowledged as `W001`, never silently dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^x-": { type: string }
        "^y-": { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    // check/generate parity: the same disposition is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "{checked:#?}"
    );
}

#[test]
fn pattern_properties_cyclic_array_values_terminate() {
    // Mutually-recursive array value schemas (`A = [B]`, `B = [A]`) form a cycle in the structural
    // homogeneity comparison. The visited-pair guard must terminate (return an outcome, never abort
    // with a stack overflow) and, since both patterns lower to the same array type, GENERATE one
    // typed overflow map. The check/generate parity assertion catches a regression that reintroduces
    // the crash.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: array, items: { $ref: "#/components/schemas/B" } }
    B: { type: array, items: { $ref: "#/components/schemas/A" } }
    Thing:
      type: object
      patternProperties:
        "^a-": { $ref: "#/components/schemas/A" }
        "^b-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e005_pattern_properties_heterogeneous_rejected() {
    // Two pattern value schemas that lower to different types cannot share one typed map → E005.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: string }
    B: { type: integer }
    Thing:
      type: object
      patternProperties:
        "^s-": { $ref: "#/components/schemas/A" }
        "^i-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
    // check/generate parity: the rejection fires in `check` too.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::PatternPropertiesRejected));
}

#[test]
fn e005_pattern_properties_with_deny_rejected() {
    // `patternProperties` + `additionalProperties: false` cannot be faithfully represented → E005.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      additionalProperties: false
      patternProperties:
        "^x-": { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
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

#[test]
fn overlapping_numeric_one_of_generates_with_typed_trial_matching() {
    // `integer | number` overlaps on integral payloads. The generated typed trial union enforces
    // exact-one matching at runtime (`1` is ambiguous; `1.5` selects number).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: integer
        - type: number
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn overlapping_object_one_of_generates_with_typed_trial_matching() {
    // Object variants that overlap structurally use typed trial matching and exact-one semantics.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: object
          required: [kind]
          properties: { kind: { type: string }, a: { type: string } }
        - type: object
          required: [kind]
          properties: { kind: { type: string }, b: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn string_integer_union_generates() {
    // `string | integer` occupy distinct JSON type categories (string vs number) → provably disjoint
    // → GENERATES (this replaced the old, incorrect E007 fixture, which asserted rejection here).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn union_sibling_constraints_intersect_every_branch() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    StringOnly:
      type: string
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn discriminated_union_with_mapping_generates() {
    // A `discriminator` with an explicit mapping over object `$ref` variants → an internally-tagged
    // enum. Generates without E007. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Dog:
      type: object
      required: [bark]
      properties: { bark: { type: boolean } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn discriminated_union_with_unique_non_object_category_generates() {
    // A non-object variant dispatches by JSON category while object variants dispatch by tag.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [name]
      properties: { name: { type: string } }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - type: string
      discriminator:
        propertyName: petType
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn disjoint_string_array_union_generates() {
    // `string | string[]` occupy distinct JSON type categories (string vs array) → provably disjoint
    // (ollama's dominant shape). Generates without E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn required_key_disjoint_objects_generate() {
    // Two CLOSED object variants (`additionalProperties: false`) each with a unique required key
    // (`a` / `b`) → provably disjoint by key presence → GENERATES with a content-inspecting custom
    // Deserialize. Closed is required for this fast path; open variants use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      additionalProperties: false
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      additionalProperties: false
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn open_object_union_generates_with_typed_trial_matching() {
    // Open objects cannot use the required-key fast path, so they use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e007_combined_one_of_and_any_of_applicators_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
      anyOf:
        - type: string
        - type: boolean
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));

    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion));
}

#[test]
fn nullable_variant_hoists_to_option() {
    // A variant that is itself nullable (`{type: [string, "null"]}`) has its nullability HOISTED to
    // the union: the union becomes `Option<Enum>` and the string/array variants stay disjoint. A
    // `null` payload resolves at the outer `Option`, so the custom Deserialize only sees non-null.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn nullable_union_collapses_to_option() {
    // A 2-member union where one member is `{type: "null"}` strips the null and collapses to
    // `Option<String>` — no enum, no E007. Generates.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: "null"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e008_non_scalar_enum() {
    // Mixed scalar kinds with no null are genuinely unrepresentable: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      enum: ["a", 1]
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn e008_stays_for_object_member_enum() {
    // Object (or array) enum members have no scalar-variant representation: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Structured:
      enum: [{ a: 1 }]
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn null_mixed_scalar_enum_generates() {
    // A `null` member is stripped; the remaining homogeneous string scalars lower as a nullable
    // enum (`Option<Enum>`). No E008, and generation succeeds. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Severity:
      type: [string, "null"]
      enum: [low, medium, high, null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::NonScalarEnum), "{checked:#?}");
}

#[test]
fn all_null_enum_generates_as_exact_null() {
    // A value set of only `null` has no scalar remainder: it lowers to exact JSON null (`()`) rather
    // than an unconstrained value or E008.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nothing:
      enum: [null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e009_unsupported_media_type() {
    // A genuinely unsupported media (`application/pdf`) still rejects with E009 — the narrowing only
    // added JSON-adjacent/XML/streaming media, not arbitrary binary content types.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/pdf:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn textual_vendor_and_structured_json_media_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /html:
    get:
      responses:
        "200":
          description: OK
          content:
            text/html:
              schema: { type: string }
  /octocat:
    get:
      responses:
        "200":
          description: OK
          content:
            application/octocat-stream:
              schema: { type: string }
  /problem:
    get:
      responses:
        "200":
          description: OK
          content:
            application/problem+json:
              schema:
                type: object
                properties: { detail: { type: string } }
"##;
    let generated = generate(spec);
    assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(!has_code(&generated, Code::UnsupportedMediaType));
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnsupportedMediaType));
}

#[test]
fn e009_raw_media_requires_a_compatible_schema() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: not representable as raw text
          content:
            text/html:
              schema: { type: object, properties: { value: { type: string } } }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn e009_request_only_media_is_rejected_in_responses() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: unsupported response codec
          content:
            application/x-www-form-urlencoded:
              schema: { type: object }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn xml_request_body_generates() {
    // an `application/xml` request body lowers to a typed struct and generates (no E009);
    // it is serialized through the runtime's quick-xml codec. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [name]
              properties:
                name: { type: string }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn xml_response_body_generates() {
    // a `text/xml` response body lowers to a typed struct and generates (no E009); it is
    // decoded through the runtime's quick-xml codec rather than serde_json.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            text/xml:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn json_alternative_wins_over_xml_on_same_body() {
    // When a body offers both JSON and XML, media selection deterministically prefers JSON, so the
    // API does not use XML at all — generation succeeds with no E009.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema: { type: object }
          application/json:
            schema: { type: object, required: [id], properties: { id: { type: string } } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
}

#[test]
fn e009_wire_changing_xml_hint_on_an_xml_body() {
    // `wrapped`/`namespace` change the XML wire. Ignoring them on a type that IS serialized as XML
    // would put structurally different bytes on the wire while reporting success, so they reject.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id]
              properties:
                id:
                  type: string
                  xml: { attribute: true, name: "Id" }
                tags:
                  type: array
                  items: { type: string }
                  xml: { wrapped: true, namespace: "urn:example" }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w006_unsupported_xml_hint_on_a_non_xml_type_warns_but_generates() {
    // The same hint on a type never serialized as XML genuinely has no effect, so it is
    // acknowledged and the document is not refused for it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [id]
              properties:
                id:
                  type: string
                  xml: { attribute: true, name: "Id" }
                tags:
                  type: array
                  items: { type: string }
                  xml: { wrapped: true, namespace: "urn:example" }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn json_only_schema_with_xml_hints_suppresses_rename_and_warns_w006() {
    // regression guard: a schema carrying `xml.name`/`xml.attribute` but reachable only
    // from a JSON body must NOT have the format-agnostic serde rename applied (it would corrupt
    // JSON). The suppression is acknowledged with W006 (never silent), and generation still succeeds.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    let checked = check(spec);
    assert!(has_code(&checked, Code::XmlHintIgnored), "{checked:#?}");
}

#[test]
fn xml_dedicated_schema_applies_hints_without_w006() {
    // A schema used *exclusively* as an XML body is XML-dedicated, so `xml.name`/`xml.attribute` are
    // honored — no suppression, and with no unsupported (namespace/prefix/wrapped) hint present, no
    // W006 fires at all.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              required: [id, sku]
              properties:
                id: { type: integer, xml: { attribute: true } }
                sku: { type: string, xml: { name: "ProductSku" } }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn schema_shared_by_json_and_xml_ops_suppresses_rename_and_warns_w006() {
    // A component referenced by BOTH a JSON operation and an XML operation is non-dedicated (it is
    // non-XML-reachable), so the rename is suppressed to keep JSON correct, with W006.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /json:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
  /xml:
    post:
      requestBody:
        content:
          application/xml:
            schema: { $ref: "#/components/schemas/Shared" }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Shared:
      type: object
      required: [id]
      properties:
        id: { type: integer, xml: { attribute: true } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
}

#[test]
fn xml_body_in_multi_status_enum_is_rejected() {
    // XML decode is scoped to single-body success/error. An XML body that would land in a
    // multi-status success enum (two bodied success statuses) is rejected cleanly with narrowed E009
    // rather than silently decoded as JSON.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema: { type: object, required: [a], properties: { a: { type: string } } }
        "201":
          description: Created
          content:
            application/json:
              schema: { type: object, required: [b], properties: { b: { type: string } } }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    // check/generate parity: the same rejection is reached without emitting.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn sse_response_body_generates() {
    // a `text/event-stream` (SSE) success response is now a typed stream, not `E009`. It
    // generates without the code firing, and check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn ndjson_response_body_generates() {
    // an `application/x-ndjson` success response is a typed stream, not `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /lines:
    get:
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson:
              schema: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn json_alternative_wins_over_stream_media_on_same_response() {
    // When a response offers BOTH a whole-body (JSON) and a streaming alternative, media selection
    // deterministically picks JSON — the operation is a normal `ResponseValue<T>`, not a stream —
    // and generation succeeds with no `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /both:
    get:
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema: { type: object }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
}

#[test]
fn e009_streaming_request_body_rejected() {
    // Streaming media is response-only: a `text/event-stream` REQUEST body has no representation and
    // stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /push:
    post:
      requestBody:
        content:
          text/event-stream:
            schema: { type: object }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn multipart_form_data_request_body_generates() {
    // A `multipart/form-data` request body whose schema is an object (a file part + a text part) is
    // now supported: it generates without E009 firing. check/generate stay in parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::UnsupportedMediaType),
        "{checked:#?}"
    );
}

#[test]
fn e009_multipart_non_object_body_rejected() {
    // A `multipart/form-data` body whose schema is NOT an object has no properties to enumerate as
    // form parts, so it stays rejected with the (narrowed) E009.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType));
}

#[test]
fn binary_format_in_param_and_text_body_positions_generate() {
    // Regression guard for `format: binary` → `bytes::Bytes` in positions rendered as strings: a
    // binary PATH param, a binary QUERY param, and a `text/plain` body of `format: binary` must all
    // generate cleanly (the e2e suite compile-verifies they do not silently miscompile). `Bytes` is
    // not `Display`; params are remapped to `String` and a Bytes body is sent raw, never `.to_string`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob/{token}:
    get:
      parameters:
        - name: token
          in: path
          required: true
          schema: { type: string, format: binary }
        - name: cursor
          in: query
          schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
  /raw:
    post:
      requestBody:
        required: true
        content:
          text/plain:
            schema: { type: string, format: binary }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn deep_object_query_style_generates() {
    // `style: deepObject` over an object of scalars is fully specified: `filter[key]=value`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: deepObject
          explode: true
          schema:
            type: object
            additionalProperties: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

#[test]
fn matrix_and_label_path_styles_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /map/{position}/{ext}:
    get:
      parameters:
        - name: position
          in: path
          required: true
          style: matrix
          schema:
            type: array
            items: { type: integer }
        - name: ext
          in: path
          required: true
          style: label
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

#[test]
fn delimited_query_styles_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: spaced
          in: query
          style: spaceDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: piped
          in: query
          style: pipeDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    }
}

#[test]
fn e010_delimited_style_with_explode_true() {
    // The specification's own serialization table marks this combination n/a, so there is no
    // correct wire form to emit.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: spaced
          in: query
          style: spaceDelimited
          explode: true
          schema:
            type: array
            items: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedParameterStyle));
    }
}

#[test]
fn e011_parameter_style_illegal_for_location() {
    // The official document schema enumerates the legal styles per location, so an illegal
    // pairing is caught structurally before lowering ever sees it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: label
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn allow_reserved_query_parameter_generates() {
    // `allowReserved: true` selects RFC 6570 reserved expansion — a different encoding set, not an
    // unrepresentable construct.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: expression
          in: query
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_allow_reserved_has_no_effect_where_nothing_is_encoded() {
    // OpenAPI 3.1 scopes `allowReserved` to `in: query`, so its metaschema rejects it elsewhere
    // structurally. 3.2 broadens it to "wherever the location percent-encodes" — which makes it
    // declarable, but still inert, on a header and on `style: cookie`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: X-Expression
          in: header
          allowReserved: true
          schema: { type: string }
        - name: session
          in: cookie
          style: cookie
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e011_allow_reserved_on_a_3_1_header_is_structurally_invalid() {
    // Pins the version difference above: 3.1 permits `allowReserved` only on a query parameter.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: X-Expression
          in: header
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e010_nested_parameter_value() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: matrix
          in: query
          schema:
            type: array
            items:
              type: array
              items: { type: integer }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedParameterStyle));

    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::UnsupportedParameterStyle));
}

#[test]
fn e012_unknown_security_scheme() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - undeclared: []
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnknownSecurityScheme));
}

/// A single-member `allOf` now MERGES into one typed struct instead of being rejected (E013 is
/// repurposed to mean "irreconcilable composition"). Generation succeeds with no E013.
#[test]
fn all_of_single_member_merges_into_struct() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Composed:
      allOf:
        - type: object
          properties:
            a: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf: [{$ref: Base}, {properties: {extra}}]` flattens the referenced component's fields plus
/// the inline member's fields into one struct.
#[test]
fn all_of_ref_plus_inline_members_merge() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
        - type: object
          properties:
            extra: { type: integer }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// A nested `allOf` (an `allOf` member that itself has an `allOf`) flattens recursively into one
/// struct.
#[test]
fn all_of_nested_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nested:
      allOf:
        - allOf:
            - type: object
              properties:
                a: { type: string }
        - type: object
          properties:
            b: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// `allOf` beside the enclosing schema's own sibling `properties`: both sets of fields merge.
#[test]
fn all_of_beside_sibling_properties_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Sibling:
      type: object
      properties:
        own: { type: string }
      allOf:
        - type: object
          properties:
            base: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// Repeated properties in an `allOf` are intersections. Compatible refinements retain the narrower
/// typed shape recursively: integer within number, enum within string, non-null within nullable,
/// exact null, and nested array/object item constraints.
#[test]
fn all_of_recursively_intersects_compatible_property_types() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Refined:
      allOf:
        - type: object
          properties:
            run_id: { type: number }
            status: { type: string }
            marker: { type: [string, "null"] }
            items:
              type: array
              items: { type: [object, "null"] }
        - type: object
          properties:
            run_id: { type: integer }
            status: { type: string, enum: [queued, complete] }
            marker: { type: "null" }
            items:
              type: array
              items:
                type: object
                required: [name]
                properties:
                  name: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::AllOfIrreconcilable),
        "{checked:#?}"
    );
}

/// A property declared with different lowered types in two `allOf` members is irreconcilable → E013.
const ALL_OF_CONFLICT_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Conflict:
      allOf:
        - type: object
          properties:
            x: { type: string }
        - type: object
          properties:
            x: { type: integer }
"##;

#[test]
fn e013_all_of_conflicting_property_types_rejected() {
    let report = generate(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Mixing an object member with a scalar member has no single representable type → E013.
#[test]
fn e013_all_of_object_scalar_mix_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      allOf:
        - type: object
          properties:
            a: { type: string }
        - type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// `check` must run the same lowering as `generate`, so an irreconcilable `allOf` rejects
/// identically through both entry points (check/generate parity).
#[test]
fn e013_check_generate_parity() {
    let report = check(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// In JSON Schema 2020-12 `$ref` is an applicator, so a `$ref`'s shape-bearing siblings are
/// intersected with the referenced schema rather than discarded. When that intersection is empty no
/// value can satisfy the schema, which is a document error the author must hear about: before this
/// was pinned, `spargen check` reported `clean` and the construct simply vanished — a request body
/// whose method then took no body argument at all. Every construct that reaches the `$ref` arm of
/// `LowerCtx::lower_schema_inner` must report `E013`, through both `generate` and `check`.
#[test]
fn e013_fires_when_a_ref_sibling_contradicts_its_target() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    // `Name` is a string; every site below intersects it with `type: integer`, which is empty.
    const TAIL: &str = "components:\n  schemas:\n    Name: { type: string }\n";

    // The issue's exact reproduction: the body vanished and `upload` lost its body argument.
    let request_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    let response_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', type: integer }
"##
    );
    let parameter = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      parameters:
        - name: filter
          in: query
          schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    // A component property, which reaches the same arm through `object_body`/`ensure_component`.
    let component_property = format!(
        "{HEAD}paths: {{}}\n{}",
        r##"components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##
    );

    // The pointer is not decoration: `compat::carve_rules` maps it to the smallest omittable
    // construct, so a diagnostic carrying the document root instead of the offending node yields no
    // carve rule and turns a carvable rejection into an un-carvable residual. Each site therefore
    // pins the exact pointer it must produce, and `carve.rs` proves the consequence end to end.
    for (site, spec, pointer) in [
        (
            "request body",
            &request_body,
            "/paths/~1u/post/requestBody/content/application~1json/schema",
        ),
        (
            "response body",
            &response_body,
            "/paths/~1u/get/responses/200/content/application~1json/schema",
        ),
        (
            "parameter",
            &parameter,
            "/paths/~1u/get/parameters/0/schema",
        ),
        (
            "component property",
            &component_property,
            "/components/schemas/Holder/properties/field",
        ),
    ] {
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{site}` was not rejected by {entry}: {report:#?}"
            );
            let pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::AllOfIrreconcilable)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                pointers,
                vec![pointer],
                "`{site}` through {entry} must report E013 once, at the offending schema: \
                 {report:#?}"
            );
        }
    }
}

/// `intersect_types` returns `None` for TWO conditions: the intersection is empty, so no value
/// satisfies both sides, and the intersection is inhabited but has no single Rust type (the catch-all
/// in `intersect_non_null` — `Bytes` against a primitive, an array against a tuple). The emitted
/// message must not claim the first when it may be the second: `{$ref: Data, format: binary}` over a
/// string `Data` is satisfied by any string, and the `E013` explain and the `allOf` scalar site both
/// already say "empty or unrepresentable". This pins the site to the same honest wording. It asserts
/// what the message may NOT say as well as what it must, because the defect this replaced was a
/// message that named the wrong one of the two.
#[test]
fn the_ref_sibling_rejection_does_not_claim_more_than_it_knows() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("empty or unrepresentable intersection"),
        "the site must use the same wording as the `allOf` scalar site and the E013 explain: {:?}",
        messages[0]
    );
    // `None` is not proof of emptiness, so the message may not assert unsatisfiability.
    assert!(
        !messages[0].contains("no value can satisfy"),
        "the message asserts unsatisfiability, which `intersect_types` returning `None` does not \
         establish: {:?}",
        messages[0]
    );
    // The remedy is the author's only route out of a rejection, so it is pinned too: it must name
    // the construct and offer the omit escape the taxonomy promises.
    let remedy = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::AllOfIrreconcilable)
        .and_then(|d| d.remedy.clone())
        .unwrap_or_default();
    assert!(remedy.contains("`$ref` target"), "{remedy:?}");
    assert!(remedy.contains("spargen::omit!"), "{remedy:?}");
}

/// A union whose only non-null member has no typed intersection with the enclosing schema's own
/// sibling constraints has no representable variant left. The multi-variant path already rejects
/// that with `E007` once every variant is excluded, so the one-member collapse reports the same
/// terminal code — otherwise the code would depend on how many variants the author happened to
/// write. It used to be dropped silently.
#[test]
fn e007_fires_when_a_single_real_member_union_contradicts_its_sibling() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Collapsed:
      type: integer
      oneOf:
        - { type: string }
        - { type: 'null' }
"##;
    let generated = generate(spec);
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(
        has_code(&generated, Code::NonDisjointUnion),
        "{generated:#?}"
    );
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion), "{checked:#?}");
}

/// The shape-bearing sibling keywords `E013`'s explain names, READ OUT of the published text
/// rather than copied into the fixture beside it. Two independent lists cannot pin each other: a
/// keyword deleted from the prose simply disappears, and a hard-coded copy goes on passing.
fn shape_bearing_keywords_the_explain_names() -> Vec<String> {
    const LEAD: &str = "A sibling bears a shape of its own through ";
    let explain = Code::AllOfIrreconcilable.explain();
    let start = explain.find(LEAD).unwrap_or_else(|| {
        panic!("E013's explain no longer enumerates its shape-bearing sibling keywords: {explain}")
    }) + LEAD.len();
    let sentence = &explain[start..];
    let sentence = &sentence[..sentence
        .find(". ")
        .unwrap_or_else(|| panic!("E013's shape-bearing sentence never ends: {sentence}"))];
    // The keywords are the backticked spans of that one sentence.
    sentence
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// `E013`'s explain names the sibling keywords intersected with a `$ref`'s target rather than
/// discarded. That is a published promise, and nothing but this fixture ties it to the code:
/// `schema_has_shape_constraint` is the private gate deciding whether the intersection happens, and
/// a sibling that clears the gate but lowers to `TypeKind::Any` intersects as identity and is
/// discarded anyway.
///
/// The list under test is DERIVED from `explain()`, not repeated here, so deleting a keyword from
/// the prose fails this fixture rather than quietly shrinking what it checks. And each row is a
/// DIFFERENTIAL — the same document with and without the keyword — so the keyword is what flips the
/// outcome. An earlier revision paired several keywords with a `type` against a target of another
/// category, where the `type` alone already rejected and the keyword beside it was never
/// load-bearing; three rows proved nothing at all.
///
/// The `$ref` sits at a response body rather than a component root. A component root whose value is
/// a `$ref` with only non-shape-bearing siblings trips a release-level `assert_eq!` inside
/// `ensure_component` (filed as #148, pre-existing on master), which would turn every "without"
/// control here into an opaque panic instead of this fixture's own message.
///
/// Each target is chosen so the intersection is *genuinely empty*, never merely unrepresentable:
/// the `contentEncoding`/`format: binary` rows sit against an integer, not a string, so they do not
/// pin the `(Bytes, Primitive(Str))` case that has a representation spargen has not learned yet.
#[test]
fn every_sibling_keyword_the_explain_names_is_actually_intersected() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const BODY: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Target'
SIBLING
"##;

    // (keyword, the target it must contradict, the sibling spelling of that keyword)
    let cases: &[(&str, &str, &str)] = &[
        ("type", "{ type: string }", "type: integer"),
        // `properties` alone, with no `type` beside it to account for the rejection. The target
        // REQUIRES `a`, so the conflicting property genuinely empties the composition — without
        // that, an optional conflicting property is representable and correctly generates.
        (
            "properties",
            "{ type: object, required: [a], properties: { a: { type: string } } }",
            "properties: { a: { type: integer } }",
        ),
        (
            "patternProperties",
            "{ type: string }",
            "patternProperties: { '^a': { type: string } }",
        ),
        ("enum", "{ type: integer }", "enum: ['a']"),
        ("const", "{ type: integer }", "const: 'a'"),
        (
            "contentEncoding",
            "{ type: integer }",
            "contentEncoding: base64",
        ),
        ("format: binary", "{ type: integer }", "format: binary"),
        ("allOf", "{ type: string }", "allOf: [{ type: integer }]"),
    ];

    // The fixture's table and the published text must name the same keywords, in the same order.
    let named = shape_bearing_keywords_the_explain_names();
    let covered: Vec<String> = cases
        .iter()
        .map(|(keyword, ..)| (*keyword).to_owned())
        .collect();
    assert_eq!(
        named, covered,
        "`E013`'s explain and this fixture disagree about which sibling keywords bear a shape; \
         whichever moved, the other must move with it"
    );

    let mut unconstrained = Vec::new();
    let mut spurious = Vec::new();
    for (keyword, target, sibling) in cases {
        let spec = |sibling: &str| {
            format!(
                "{HEAD}{}components:\n  schemas:\n    Target: {target}\n",
                BODY.replace("SIBLING", sibling)
            )
        };
        let with = generate(&spec(&format!("                {sibling}")));
        if with.outcome() != Outcome::Rejected || !has_code(&with, Code::AllOfIrreconcilable) {
            unconstrained.push(*keyword);
        }
        // The control: the identical document without the keyword must generate, so the rejection
        // above is attributable to the keyword and to nothing else in the row.
        let without = generate(&spec(""));
        if without.outcome() == Outcome::Rejected {
            spurious.push(*keyword);
        }
    }
    assert!(
        unconstrained.is_empty(),
        "the explain names these sibling keywords as intersected, but a `$ref` carrying one against \
         an irreconcilable target still generates: {unconstrained:?}"
    );
    assert!(
        spurious.is_empty(),
        "these rows reject even without their keyword, so they pin the rest of the row rather than \
         the keyword the explain names: {spurious:?}"
    );

    // The other half of the published rule: the four refining keywords do NOT constrain alone,
    // because each lowers to `TypeKind::Any` without a `type`/`properties` to give it a shape. If
    // one of these ever starts rejecting, the explain's second clause has become wrong in the
    // opposite direction and must move with it.
    for (keyword, sibling) in [
        ("required", "required: [a]"),
        ("additionalProperties", "additionalProperties: false"),
        ("items", "items: { type: integer }"),
        ("prefixItems", "prefixItems: [{ type: integer }]"),
    ] {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Target: {{ type: string }}\n    Sibling:\n      \
             $ref: '#/components/schemas/Target'\n      {sibling}\n"
        );
        let report = generate(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "a bare `{keyword}` sibling now constrains, so the explain's \"refine a shape rather \
             than establish one\" clause is no longer true: {report:#?}"
        );
    }

    // And the third clause, which says WHICH establishing keyword each refiner needs. Neither tier
    // above can check it: tier one pairs every refiner with a `type` against a target of a
    // different category, where the `type` alone already rejects, so the keyword beside it is never
    // load-bearing. This tier is a differential instead — the same document with and without the
    // refining keyword, against a target the paired `type` AGREES with, so the only thing that can
    // move the lowering is the keyword itself.
    //
    // (keyword, the establishing keywords beside it, the target it agrees with, must it participate)
    let refiners: &[(&str, &str, &str, &str)] = &[
        (
            "additionalProperties",
            "type: object",
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items",
            "type: array",
            "items: { type: integer }",
            "{ type: array, items: { type: string } }",
        ),
        (
            "prefixItems",
            "type: array",
            "prefixItems: [{ type: integer }]",
            "{ type: array, items: { type: string } }",
        ),
        // `required` beside `properties` participates: `object_body` consumes it per declared
        // property, and the property is declared.
        (
            "required",
            "properties: { a: { type: string } }",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
    ];
    // What is compared is `Sibling`'s OWN emitted definition, not the whole `types` module.
    //
    // The module also holds the intermediate aliases — `SiblingConstraint`,
    // `SiblingReferenceIntersection` — which are emitted whenever the sibling clears
    // `schema_has_shape_constraint`, whether the refining keyword participates in the intersection
    // or not. So a whole-module comparison answers "did adding this keyword change ANY emitted
    // byte", which is liveness; it does not answer "did it change the type", which is the claim in
    // the failure message. Measured under the sibling-discarded mutation: `Sibling` itself is
    // byte-identical with and without the refiner, and the whole-module comparison passed anyway.
    let lowering = |establishing: &str, refiner: &str, target: &str| {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Target: {target}\n    Sibling:\n      \
             $ref: '#/components/schemas/Target'\n      {establishing}\n{refiner}"
        );
        let (report, code) = generate_with_code(&spec);
        // `Sibling`'s own item, ATTRIBUTES INCLUDED — `#[serde(deny_unknown_fields)]` sits above
        // the declaration and is exactly what `additionalProperties` contributes. A rejection
        // yields no item at all, which is itself a difference worth seeing.
        let types = types_module(&code);
        let lines: Vec<&str> = types.lines().collect();
        let declares = |line: &str| {
            let t = line.trim_start();
            ["pub struct ", "pub type ", "pub enum "]
                .iter()
                .filter_map(|decl| t.strip_prefix(decl))
                .any(|rest| {
                    rest.split([' ', '<', '{', '(', ';', '='])
                        .next()
                        .is_some_and(|name| name == "Sibling")
                })
        };
        let definition = lines
            .iter()
            .position(|line| declares(line))
            .map(|decl| {
                // Walk back over the item's attributes and rustdoc.
                let mut start = decl;
                while start > 0 {
                    let prev = lines[start - 1].trim_start();
                    if prev.starts_with("#[") || prev.starts_with("///") {
                        start -= 1;
                    } else {
                        break;
                    }
                }
                // Forward to the end of the item.
                let mut end = decl;
                if lines[decl].trim_end().ends_with(';') {
                    end = decl + 1;
                } else {
                    while end < lines.len() {
                        end += 1;
                        if lines[end - 1].trim_end() == "}" {
                            break;
                        }
                    }
                }
                lines[start..end].join("\n")
            })
            .unwrap_or_default();
        (report.outcome(), definition)
    };
    for (keyword, establishing, refiner, target) in refiners {
        assert_ne!(
            lowering(establishing, &format!("      {refiner}\n"), target),
            lowering(establishing, "", target),
            "`{keyword}` beside `{establishing}` changed nothing about the lowering, so the \
             explain's claim that it then takes part is not true"
        );
    }

    // The clause that round 1 got wrong in the other direction: `required` does NOT take part
    // beside a bare `type: object`. `object_body` materialises fields only from `properties` and
    // consumes `required` as a per-field flag, so a sibling that declares no property has no field
    // to mark and the requirement is dropped — the generated type accepts and can emit `{}`, which
    // the description forbids. That is #140's territory to repair; the published text must not
    // claim it is already handled.
    assert_eq!(
        lowering(
            "type: object",
            "      required: [a]\n",
            "{ type: object, properties: { a: { type: string } } }"
        ),
        lowering(
            "type: object",
            "",
            "{ type: object, properties: { a: { type: string } } }"
        ),
        "`required` beside a bare `type: object` now changes the lowering, so the explain's \
         \"only beside the sibling's own `properties`\" clause has become wrong and must move"
    );
}

/// Site B widened `E007`'s emission set, so its message has to describe the construct that actually
/// reached it. A right code under a wrong message is still a wrong diagnostic: reusing the
/// multi-variant path's wording verbatim would say "every variant impossible" of a union with one
/// variant, and would inherit the same unsatisfiability claim `intersect_types` cannot support.
/// This pins the sole-member wording, the empty-or-unrepresentable hedge, and that the message does
/// not name some other cause entirely.
#[test]
fn the_single_member_union_rejection_names_its_own_cause() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Collapsed:
      type: integer
      oneOf:
        - { type: string }
        - { type: 'null' }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::NonDisjointUnion);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("sole non-null member"),
        "{:?}",
        messages[0]
    );
    assert!(
        messages[0].contains("empty or unrepresentable"),
        "{:?}",
        messages[0]
    );
    // The causes `E007`'s explain already named must not be borrowed for this one.
    assert!(!messages[0].contains("discriminator"), "{:?}", messages[0]);
    assert!(!messages[0].contains("`anyOf`"), "{:?}", messages[0]);

    // The pointer, which both new `E013` sites pin and this one did not. It is not decoration:
    // `compat::carve_rules` maps it to the smallest omittable construct, so a diagnostic carrying
    // the document root instead of the offending node yields no rule, and a carvable rejection
    // becomes an un-carvable residual that ends the whole run `Rejected`. `carve.rs` proves that
    // consequence for this site end to end.
    let pointers: Vec<&str> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::NonDisjointUnion)
        .map(|d| d.pointer.as_str())
        .collect();
    assert_eq!(
        pointers,
        vec!["/components/schemas/Collapsed"],
        "the sole-member collapse must report at the offending component, not the document root: \
         {report:#?}"
    );

    // check/generate parity for the same site, which the `E013` sites also assert.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::NonDisjointUnion), "{checked:#?}");

    // And the abstinence Site B spends five lines of comment justifying: the multi-variant path
    // emits a per-variant `W011` before its `E007` and this one deliberately does not, because
    // `W011` means "declared construct has no effect" and describes something dropped from output
    // that still exists — when the SOLE member is excluded no enum is generated at all, so there is
    // no surviving construct for the warning to describe. Filtering to `NonDisjointUnion` above
    // cannot see a second code, so adding exactly that `W011` survived the whole suite. One cause,
    // one diagnostic.
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "the sole-member collapse emitted `W011` beside its `E007`: two diagnostics for one cause, \
         and the `W011` would assert a generated enum that does not exist: {report:#?}"
    );
    assert_eq!(
        report.diagnostics().len(),
        1,
        "the sole-member collapse must report exactly its own cause: {report:#?}"
    );
}

/// `E013`'s explain is what `spargen explain E013` prints, and this change REWROTE it: the code was
/// repurposed from "irreconcilable allOf composition" to a composition-generic one, and the text
/// moved with it. Nothing held the new text to the code. Measured before this fixture existed:
/// reverting the explain to its `allOf`-only wording, and replacing it with text flatly
/// contradicting the code ("a `$ref` replaces the containing schema: its sibling keywords are
/// discarded, never intersected"), BOTH survived the entire suite.
///
/// The assertions are claims, not typography — a string-equality snapshot would pin the wording and
/// go stale on every edit without ever catching a false clause. Each one below is a promise some
/// other fixture in this file enforces against the code, so the two cannot drift apart silently.
#[test]
fn the_composition_explain_covers_every_cause_that_reports_it() {
    let explain = Code::AllOfIrreconcilable.explain();

    // Both constructs that report it, and that `$ref` siblings are INTERSECTED rather than
    // discarded — the sentence the whole change exists to make true.
    assert!(explain.contains("`allOf`"), "{explain}");
    assert!(
        explain.contains("`$ref` is an applicator"),
        "the explain must say why a `$ref`'s siblings are not discarded: {explain}"
    );
    assert!(
        explain.contains("instead of being discarded"),
        "the explain must say the siblings are intersected rather than discarded: {explain}"
    );

    // The two-tier sibling-keyword rule, pinned against the code by
    // `every_sibling_keyword_the_explain_names_is_actually_intersected`.
    assert!(
        explain.contains("A sibling bears a shape of its own through"),
        "{explain}"
    );
    assert!(
        explain.contains("refine a shape rather than establish one"),
        "{explain}"
    );

    // The array-arm doctrine. `intersect_structs` now mirrors it for an optional conflicting
    // property, so withdrawing this sentence would leave that behaviour unexplained.
    assert!(
        explain.contains("uninhabited item type"),
        "the explain must keep the doctrine that an empty item intersection stays representable: \
         {explain}"
    );

    // The hedge the round-1 repair put on every message that reports this code: `intersect_types`
    // returns `None` for an empty intersection AND for an inhabited one with no single Rust type,
    // and the text must not claim the first when it may be the second.
    assert!(explain.contains("empty or unrepresentable"), "{explain}");

    // The rejection causes, including the recursive one all three spellings now share — stated as a
    // property of the document rather than of lowering order, which is what makes the verdict
    // reproducible when `components.schemas` is reordered.
    assert!(explain.contains("closes a reference cycle"), "{explain}");
    assert!(
        !explain.contains("not yet known") && !explain.contains("still being lowered"),
        "the explain still describes the recursive cause as a lowering-order fact, which the \
         guard no longer is: {explain}"
    );

    // Presence assertions cannot see a contradiction ADDED after them. Appending a paragraph
    // saying sibling keywords are "discarded and never intersected, so none of the above applies"
    // left every assertion above true and eleven suites green. The remedy is the last thing the
    // explain says, so anything appended moves it — which is a structural rule, not typography.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E013`'s explain must end with its remedy; text after it can contradict everything \
         above and no presence assertion would notice: {explain}"
    );
    // And the in-place forms of the same contradiction.
    for denial in [
        "never intersected",
        "none of the above",
        "always exactly its target",
        "siblings are discarded",
    ] {
        assert!(
            !explain.contains(denial),
            "`E013`'s explain contradicts the code it documents (`{denial}`): {explain}"
        );
    }
}

/// `E007`'s published explain is what `spargen explain E007` prints, and Site B added a cause it did
/// not describe. The `oneOf`-plus-`anyOf` and `defaultMapping` causes must survive, and the new one
/// must be there beside them — the same standard this change applied to `E013`'s text.
#[test]
fn the_union_explain_covers_every_cause_that_reports_it() {
    let explain = Code::NonDisjointUnion.explain();
    assert!(explain.contains("`oneOf` and `anyOf`"), "{explain}");
    assert!(explain.contains("defaultMapping"), "{explain}");
    assert!(explain.contains("no branch at all"), "{explain}");
    assert!(explain.contains("single non-null member"), "{explain}");

    // The `W011` clause had no reader while its three neighbours did, and it is the clause Site B's
    // abstinence rests on: a branch the adjacent constraints exclude is dropped with `W011` *while
    // the rest of the enum stands*, which is exactly what does not happen when the excluded branch
    // is the only one. `the_single_member_union_rejection_names_its_own_cause` asserts the
    // behaviour; without this the published reason for it could be deleted silently.
    assert!(
        explain.contains("`W011`"),
        "the explain must say what happens to a branch the adjacent constraints exclude: {explain}"
    );
    assert!(
        explain.contains("while the rest of the enum stands"),
        "the explain must keep the clause that distinguishes an excluded branch from an excluded \
         sole member, which is why one warns and the other does not: {explain}"
    );

    // The same anti-append guard as `E013`'s: presence assertions cannot see a contradiction added
    // after them, and the remedy is the last thing the explain says.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E007`'s explain must end with its remedy: {explain}"
    );
}

/// The guard on the two rejections above: only an EMPTY intersection is an error. A sibling that
/// merely narrows its target still lowers to the narrower type and generates, so the new rejection
/// cannot creep into the ordinary applicator case.
#[test]
fn a_compatible_ref_sibling_still_generates() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Count', type: number }
components:
  schemas:
    Count: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    assert!(!code.contains("serde_json :: Value"), "{code}");

    // The claim in the doc comment above — "lowers to the NARROWER type" — and the whole point of
    // treating `$ref` as an applicator. Outcome assertions cannot see it: they pin when the tool
    // refuses, and this is what it emits when it proceeds.
    //
    // `integer` ∧ `number` is `integer`, so the body must be `i64`. Two substitutions were green
    // before this: taking the SIBLING alone, which is the exact defect this pull request exists to
    // fix, and taking the TARGET alone, which is the behaviour before it. The first widens the body
    // to `f64` — a generated type that accepts values the description forbids — and neither changes
    // an outcome, a code, or introduces `serde_json::Value`.
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`integer` narrowed by `number` must stay `i64`; `f64` would accept values the document \
         forbids: {types}"
    );
    // And the sibling is not simply discarded either: the intersection is taken, so the derived
    // type exists rather than the response naming the target component directly.
    assert!(
        types.contains("pub type Count = i64;"),
        "the target must still be emitted under its own name: {types}"
    );

    // The mirror, and the row that catches the OTHER substitution. Above, `integer` is both the
    // intersection and the target, so taking the target alone happens to give the right answer and
    // is invisible. Here the SIBLING is the narrower side: `number` narrowed by `integer` is
    // `integer`, so target-alone would emit `f64` and widen the body.
    let mirrored = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Measure', type: integer }
components:
  schemas:
    Measure: { type: number }
"##;
    let (report, code) = generate_with_code(mirrored);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`number` narrowed by `integer` must be `i64`; `f64` is the target alone, which is the \
         behaviour before `$ref` was treated as an applicator: {types}"
    );
}

/// Over-rejection is the whole risk of reporting where the code used to drop, and the fixture above
/// pins one shape only — primitive narrowing. These are the other shapes that reach the `$ref` arm
/// and must keep generating. Each is a direction the rejection could creep in, and each lands on a
/// different mechanism: the early exit before any intersection, an intersection that succeeds
/// unchanged, and `intersect_types`' both-sides-nullable rescue, which returns the exact JSON null
/// type rather than `None` and so never reaches the new rejection at all.
#[test]
fn the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', SIBLING }
"##;

    // (what it exercises, the sibling keywords, the target, what must appear in the output)
    let cases: &[(&str, &str, &str, &str)] = &[
        // Validation-only siblings bear no shape, so the `$ref` arm exits before intersecting. They
        // are reported as ignored (`W001`), never as irreconcilable.
        (
            "validation-only siblings",
            "maxLength: 5, pattern: '^a'",
            "{ type: string }",
            "String",
        ),
        // The sibling agrees with its target: the intersection succeeds and is the target's type.
        (
            "an agreeing type",
            "type: string",
            "{ type: string }",
            "String",
        ),
        // Both sides accept null and nothing else is shared. `intersect_types` returns the exact
        // JSON null type — `null` genuinely is the only satisfying value — so this must NOT reject.
        // The decision record lists "collapse to Null when nullable" as rejected; the code does it,
        // and this fixture is why the record now says the code is right.
        (
            "a nullable-only intersection",
            "type: [integer, 'null']",
            "{ type: [string, 'null'] }",
            "()",
        ),
    ];

    for (what, sibling, target, expected) in cases {
        let spec = format!(
            "{HEAD}{}components:\n  schemas:\n    Name: {target}\n",
            PATH.replace("SIBLING", sibling)
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` was rejected, so the new rejection has crept: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` reported E013: {report:#?}"
        );
        // The RESPONSE BODY's own type, resolved through the operation signature. The earlier
        // revision asserted `code.contains("String")` and `code.contains("()")`, and both of those
        // are true of every generated client — `pub fn url(&self) -> String` and `Server0::new()`
        // are always emitted — so the column could not distinguish any behaviour at all.
        let types = types_module(&code);
        let body = code
            .split("ResponseValue<types::")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
            .trim()
            .to_owned();
        assert!(
            types.contains(&format!("pub type {body} = {expected};")),
            "`{what}` must lower its response body to `{expected}`, but `{body}` is not: {types}"
        );
    }

    // The validation-only case is also the one that must still be *acknowledged*: bearing no shape
    // is not the same as being silently dropped.
    let validation_only = format!(
        "{HEAD}{}components:\n  schemas:\n    Name: {{ type: string }}\n",
        PATH.replace("SIBLING", "maxLength: 5, pattern: '^a'")
    );
    assert!(
        has_code(&generate(&validation_only), Code::ValidationKeywordIgnored),
        "a validation-only sibling must still be acknowledged"
    );
}

/// When a `$ref` is BOTH unresolvable and carries a contradictory sibling, exactly one code must
/// win and it must be `E004`: `ensure_component` returns `None` before the intersection is reached,
/// so the missing component is reported and the sibling never gets a second, confusing diagnostic
/// about a target that does not exist. The two sites are twenty lines apart in the same block, which
/// is why this is pinned rather than assumed.
///
/// The absence of `E013` is only a SYMPTOM of that ordering, and a weak one: letting the miss fall
/// through to a `TypeKind::Any` placeholder and reach the intersection looks identical from
/// outside, because an `Any` sibling identity-intersects and emits nothing. So the ordering itself
/// is measured, with a sibling whose own LOWERING would report — `patternProperties` whose value
/// schemas disagree is `E005`. If the sibling is never lowered, `E005` cannot fire; the control
/// proves it fires the moment the same sibling is lowered against a target that does exist.
#[test]
fn an_unresolvable_ref_reports_only_e004_even_when_its_sibling_contradicts() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Nope', type: integer }
      responses: { '204': { description: ok } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "a missing component must not also be reported as an irreconcilable intersection — \
             there is no target to intersect with: {report:#?}"
        );
    }

    // The mechanism. `SIBLING` is shape-bearing, so it clears `schema_has_shape_constraint` and
    // would be lowered if the `$ref` arm ever got that far.
    const TRACED: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/TARGET'
              type: object
              patternProperties: { '^a': { type: string }, '^b': { type: integer } }
      responses: { '204': { description: ok } }
components:
  schemas:
    Present: { type: object }
"##;
    for report in [
        generate(&TRACED.replace("TARGET", "Nope")),
        check(&TRACED.replace("TARGET", "Nope")),
    ] {
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::PatternPropertiesRejected),
            "the sibling of an unresolvable `$ref` was lowered, so `ensure_component` no longer \
             returns `None` before the intersection is reached and the ordering this fixture \
             names is gone: {report:#?}"
        );
    }
    // The control: the identical sibling against a target that resolves IS lowered, and reports.
    let present = generate(&TRACED.replace("TARGET", "Present"));
    assert!(
        has_code(&present, Code::PatternPropertiesRejected),
        "the marker sibling no longer reports when it is lowered, so its absence above proves \
         nothing: {present:#?}"
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

#[test]
fn w001_validation_keyword_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
components:
  schemas:
    Age:
      type: integer
      minimum: 0
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ValidationKeywordIgnored));
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

const W005_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        count:
          type: integer
          default: "not-a-number"
        meta:
          type: object
          default: { a: 1 }
"##;

#[test]
fn w005_schema_default_not_applied_still_generates() {
    let report = generate(W005_SPEC);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// `check` runs the same lowering as `generate`, so the W005 disposition fires identically.
#[test]
fn w005_check_generate_parity() {
    let report = check(W005_SPEC);
    assert_eq!(report.outcome(), Outcome::Clean, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// A representable scalar default on an optional field is applied via serde and must not raise
/// W005 (or any error): generation succeeds and the field is documented with its default.
#[test]
fn representable_scalar_default_applies_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        color:
          type: string
          default: "red"
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A parameter `default` is documented in rustdoc (never serde-wired) — generation is clean and
/// must NOT raise W005 (parameters always have a documentation home).
#[test]
fn parameter_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      parameters:
        - name: per_page
          in: query
          schema: { type: integer, default: 30 }
        - name: sort
          in: query
          required: true
          schema: { type: string, default: name }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` on a component schema itself (here an enum) is documented on the generated named
/// type — generation is clean, with no W005 and no double-handling.
#[test]
fn component_root_default_documented_without_w005() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != spargen::Severity::Error),
        "{report:#?}"
    );
}

/// A `default` in a structural position with no field home — array `items` and an
/// `additionalProperties` value — is non-silent: it fires W005 and still generates.
#[test]
fn structural_defaults_fire_w005_and_still_generate() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Tags:
      type: array
      items: { type: string, default: hi }
    Counts:
      type: object
      additionalProperties: { type: integer, default: 5 }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// An out-of-range integer default for the field's width (`int32` here) is NOT representable: it
/// must fire W005 and stay rustdoc-only, never rendered into a literal that fails to compile.
#[test]
fn out_of_range_int_default_fires_w005_and_is_not_wired() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      properties:
        big:
          type: integer
          format: int32
          default: 5000000000
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

/// A component that is a bare `$ref` with a sibling `default` drops the default when the reference
/// resolves; it must be acknowledged with W005 rather than lost silently, and still generate.
#[test]
fn component_root_ref_with_default_fires_w005_and_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Bar:
      type: string
    Alias:
      $ref: "#/components/schemas/Bar"
      default: aliased
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        has_code(&report, Code::SchemaDefaultNotApplied),
        "{report:#?}"
    );
}

#[test]
fn multi_status_success_bodies_generate_a_typed_enum_not_a_degraded_value() {
    // Two success statuses with DIFFERENT bodies used to degrade to `serde_json::Value` (W003).
    // W003 is retired: the success type is now a typed per-operation response enum, generated with
    // no diagnostic at all.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyA" }
        "201":
          description: Created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyB" }
components:
  schemas:
    BodyA:
      type: object
      properties:
        a: { type: string }
    BodyB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    // No diagnostics at all — the retired W003 must not fire under any code.
    assert!(report.diagnostics().is_empty(), "{report:#?}");
}

#[test]
fn multi_status_error_bodies_generate_a_typed_enum_not_a_degraded_value() {
    // Two error statuses with DIFFERENT bodies likewise generate a typed error enum, no W003.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string }
        "404":
          description: Not Found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrA" }
        "409":
          description: Conflict
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrB" }
components:
  schemas:
    ErrA:
      type: object
      properties:
        a: { type: string }
    ErrB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
}

#[test]
fn multi_status_enum_precedence_emits_exact_arm_before_range_and_a_bodyless_unit_variant() {
    // Both classes list a RANGE before an overlapping EXACT in document order (and mix in a bodyless
    // 204). The emitter must reorder to exact-before-range so a real 200/409 dispatches to its exact
    // variant, and the bodyless 204 must appear as a payload-free unit variant — never a silent drop.
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      operationId: getP
      responses:
        "2XX":
          description: RangeOk
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeOk" } } }
        "200":
          description: ExactOk
          content: { application/json: { schema: { $ref: "#/components/schemas/ExactOk" } } }
        "204":
          description: No Content
        "4XX":
          description: RangeErr
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeErr" } } }
        "409":
          description: Conflict
          content: { application/json: { schema: { $ref: "#/components/schemas/Conflict" } } }
components:
  schemas:
    RangeOk: { type: object, properties: { r: { type: string } } }
    ExactOk: { type: object, properties: { e: { type: string } } }
    RangeErr: { type: object, properties: { x: { type: string } } }
    Conflict: { type: object, properties: { c: { type: string } } }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    // Success dispatch: the exact 200 arm is emitted (and thus checked) before the 2XX range arm.
    let exact_200 = code.find("Exact(200u16)").expect("exact 200 selector");
    let range_2xx = code.find("Range(2u8)").expect("2XX range selector");
    assert!(
        exact_200 < range_2xx,
        "exact 200 must precede the 2XX range in the emitted decode chain"
    );
    // Error classification: the exact 409 arm precedes the 4XX range arm.
    let exact_409 = code.find("Exact(409u16)").expect("exact 409 selector");
    let range_4xx = code.find("Range(4u8)").expect("4XX range selector");
    assert!(
        exact_409 < range_4xx,
        "exact 409 must precede the 4XX range in the emitted classification chain"
    );
    // The bodyless 204 is a payload-free unit variant, not dropped and not a `serde_json::Value`.
    assert!(
        code.contains("Status204,"),
        "bodyless 204 must emit a unit variant"
    );
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
fn property_annotations_come_from_the_property_not_the_object() {
    // Regression: `deprecated`/`readOnly`/`writeOnly` were read from the enclosing object, so an
    // object-level `deprecated: true` marked every field and a property-level one was ignored.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
components:
  schemas:
    Item:
      type: object
      deprecated: true
      properties:
        current: { type: string }
        legacy: { type: string, deprecated: true }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("legacy"), "legacy field emitted: {code}");
    assert!(code.contains("current"), "current field emitted: {code}");
    // Exactly one item is deprecated — the property that says so — not every field of the
    // deprecated object, and not zero.
    assert_eq!(
        code.matches("#[deprecated]").count(),
        1,
        "exactly one item is deprecated, not every field of a deprecated object: {code}"
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
fn w011_reserved_header_parameters_are_ignored() {
    // `Accept`, `Content-Type`, and `Authorization` belong to the protocol layer.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: Accept
          in: header
          schema: { type: string }
        - name: authorization
          in: header
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .count(),
            2,
            "one per reserved header, matched case-insensitively: {report:#?}"
        );
    }
}

#[test]
fn e009_content_parameter_with_an_unrenderable_media_type() {
    // An XML `content` parameter used to fall through to `simple` serialization and be sent in the
    // wrong format entirely.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          content:
            application/xml:
              schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn optional_request_bodies_take_an_option_argument() {
    // `requestBody.required` was dropped entirely, so an optional body was indistinguishable from
    // a required one and the caller had to invent a value.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /required:
    post:
      operationId: postRequired
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
      responses:
        "204": { description: No Content }
  /optional:
    post:
      operationId: postOptional
      requestBody:
        content:
          application/json:
            schema: { type: object, properties: { a: { type: string } } }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("body: Option<&"),
        "an optional body is passed as an Option: {code}"
    );
    assert!(
        code.contains("body: &"),
        "a required body is passed by reference: {code}"
    );
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
fn e015_items_beside_prefix_items() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: array
                prefixItems:
                  - { type: string }
                items: { type: integer }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::TupleRestNotRepresentable),
            "{report:#?}"
        );
    }
}

#[test]
fn items_false_beside_prefix_items_is_a_closed_tuple() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: array
                prefixItems:
                  - { type: string }
                  - { type: integer }
                items: false
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::TupleRestNotRepresentable),
            "{report:#?}"
        );
    }
}

#[test]
fn e012_http_security_scheme_that_cannot_be_attached() {
    // A `digest` scheme used to vanish silently, surfacing only as a confusing E012 at the
    // requirement site naming a scheme the document plainly declares.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    digestAuth:
      type: http
      scheme: digest
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::UnknownSecurityScheme),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_mutual_tls_is_satisfied_by_the_transport() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
security:
  - mtls: []
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    mtls:
      type: mutualTLS
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_allow_empty_value_has_no_effect() {
    // Deprecated in 3.2 and inert for a typed client: an omitted optional parameter is not sent.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: flag
          in: query
          allowEmptyValue: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn server_variables_generate_a_typed_builder() {
    // Regression: `servers[].variables` was dropped entirely, so a templated URL reached rustdoc
    // with its `{braces}` intact and no way to fill them.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
servers:
  - name: regional
    url: "https://{region}.example.com/{basePath}"
    variables:
      region:
        default: us
        enum: [us, eu]
      basePath:
        default: v2
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub mod servers"), "{code}");
    assert!(code.contains("pub fn default_url()"), "{code}");
    // The `enum` variable becomes a closed type, so an illegal region cannot be constructed.
    assert!(code.contains("pub enum RegionalRegion"), "{code}");
    assert!(code.contains("with_default_server"), "{code}");
}

#[test]
fn e011_server_variable_default_outside_its_enum() {
    // The default is actually sent, so a default outside its own `enum` would make the
    // no-argument path put an illegal value on the wire.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{region}.example.com"
    variables:
      region:
        default: apac
        enum: [us, eu]
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e011_server_url_references_an_undeclared_variable() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{region}.example.com"
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn documented_response_headers_get_typed_accessors() {
    // Regression: `response.headers` was dropped entirely, with no diagnostic.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          headers:
            X-RateLimit-Remaining:
              required: true
              schema: { type: integer }
            X-Next:
              $ref: "#/components/headers/Next"
            Content-Type:
              schema: { type: string }
          content:
            application/json:
              schema: { type: array, items: { type: string } }
components:
  headers:
    Next:
      description: The cursor for the next page.
      schema: { type: string }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("ListPetsStatus200Headers"), "{code}");
    // A required header is a plain field; an optional one is an Option. Inline header schemas get
    // a synthesized named type, exactly as inline schemas elsewhere do.
    assert!(
        code.contains("pub x_rate_limit_remaining: types::HeaderXRateLimitRemaining"),
        "{code}"
    );
    assert!(code.contains("pub x_next: Option<"), "{code}");
    assert!(code.contains("from_response"), "{code}");
    // A documented `Content-Type` is ignored per the specification, and said so.
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
    assert!(
        !code.contains("content_type:"),
        "a documented Content-Type header must not become a field: {code}"
    );
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

#[test]
fn oas32_security_requirement_uri_resolves() {
    // OpenAPI 3.2 lets a requirement name a Security Scheme Object by URI. A component name always
    // wins, per the specification, so only a name matching no component is resolved as a reference.
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("bearer.yaml"),
        "type: http\nscheme: bearer\n",
    )
    .unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
security:
  - "./bearer.yaml": []
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out).unwrap(),
    ));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnknownSecurityScheme),
        "{report:#?}"
    );
}

#[test]
fn path_item_and_operation_servers_override_the_base_url() {
    // Regression: `servers` was read only at the document root, so a Path Item or Operation Object
    // that redirects its calls to another host was skipped along with every other non-method key —
    // silently generating a client that called the document's server instead.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "204": { description: No Content }
  /upload:
    servers:
      - url: https://files.example.net/store
    post:
      operationId: uploadItem
      responses:
        "204": { description: No Content }
  /reports:
    get:
      operationId: getReport
      servers:
        - url: https://{region}.reports.example.org/v2
          variables:
            region:
              default: eu
              enum: [eu, us]
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // No override: the client's base URL stands.
    assert!(
        code.contains("build_url_on(&self.core, None, &path, &query)"),
        "an operation without an override must pass no server: {code}"
    );
    // A path-item override applies to every operation on that path.
    assert!(
        code.contains(r#"Some("https://files.example.net/store")"#),
        "a path-item `servers` override must reach the URL builder: {code}"
    );
    // An operation override wins, and its variables are substituted with their declared defaults.
    assert!(
        code.contains(r#"Some("https://eu.reports.example.org/v2")"#),
        "an operation `servers` override must be rendered with variable defaults: {code}"
    );
}

#[test]
fn w011_server_override_past_the_first_has_no_effect() {
    // The specification defines no rule for a client to choose among several per-operation servers,
    // so the first is used and the rest are acknowledged rather than dropped in silence.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /x:
    get:
      servers:
        - url: https://first.example.net
        - url: https://second.example.net
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e011_server_override_variables_are_validated_like_the_document_s() {
    // The override goes through the same `lower_server`, so a template naming an undeclared
    // variable is rejected wherever it appears rather than only at the document root.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /x:
    get:
      servers:
        - url: https://{stage}.example.net
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e009_multipart_deep_object_encoding_rejected() {
    // `deepObject` builds `name[key]=value` query fragments. A multipart part carries its name in
    // `Content-Disposition` and its value alone, so the style has no representation there — it used
    // to generate parts holding only the values, with the keys dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                filter:
                  type: object
                  properties:
                    a: { type: string }
            encoding:
              filter:
                style: deepObject
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_multipart_rfc6570_object_property_rejected() {
    // The specification applies the Encoding Object to the entire value for a non-array property,
    // but defines no part representation for an object. Reusing the query builders silently emitted
    // one part per member carrying only the value.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                meta:
                  type: object
                  properties:
                    a: { type: string }
            encoding:
              meta:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_encoding_delimited_style_with_explode_rejected() {
    // The specification's serialization table marks `spaceDelimited`/`pipeDelimited` with
    // `explode: true` as *n/a*. The identical parameter-side construct is already `E010`; an
    // Encoding Object used to accept it and then ignore the `explode`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: pipeDelimited
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn multipart_rfc6570_array_encoding_generates() {
    // The shapes that *are* defined stay supported: an array property under a delimited or form
    // style, exploded or not.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                names:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: spaceDelimited
                explode: false
              names:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    }
}

#[test]
fn set_cookie_response_header_is_a_list_of_lines() {
    // RFC 9110 s5.3 exempts `Set-Cookie` from the rule that lets a repeated header be folded into a
    // comma-separated line, and OpenAPI 3.2 gives it a section saying each value stays on its own
    // line. The declared schema describes one cookie, so the accessor is a list of them — the
    // generic path would have joined the occurrences and split them back apart on every comma,
    // fragmenting any cookie with an `Expires=Wed, 09 Jun ...` attribute.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "204":
          description: No Content
          headers:
            Set-Cookie:
              required: true
              schema: { type: string }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("support::HeaderShape::SetCookie"),
        "a documented Set-Cookie header must use the non-joining shape: {code}"
    );
    assert!(
        code.contains("Vec<String>"),
        "the accessor must be a list of per-line values: {code}"
    );
}

#[test]
fn w014_alternative_media_type_is_not_generated() {
    // A generated method sends and decodes exactly one media type, so a body offering both JSON and
    // XML narrows to JSON. That is a real reduction of the documented surface and used to happen
    // with no diagnostic at all — the one silent disposition left in the media path.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: string }
          application/xml:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn a_single_media_type_draws_no_alternative_warning() {
    // The warning must fire only when something is actually dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn an_empty_schema_on_a_binary_media_type_is_a_byte_body() {
    // OpenAPI 3.1 aligned Schema Objects with JSON Schema 2020-12 and removed `format: binary`, so
    // `schema: {}` — or no schema at all — is how a 3.1 document says "any octets" on a media type
    // that already carries the meaning. It used to be rejected with `E009` for not being
    // `type: string`, which is the 3.0 spelling 3.1 retired.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    post:
      operationId: putBlob
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {}
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected, "{report:#?}");
    // The point of the fix: a byte body must not degrade to `serde_json::Value`, and retyping the
    // untyped schema must not leave a second alias behind.
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        !code.contains("= serde_json::Value;"),
        "an octet body must not lower to an untyped value: {code}"
    );
}

#[test]
fn retyping_a_binary_body_never_rewrites_a_shared_component() {
    // Retyping the untyped body replaces the definition in place when it is anonymous. A childless
    // component (`Opaque: {}`) is lifted into its reserved id and is then the graph's *last*
    // definition too, so "last inserted" alone cannot tell the two apart — and rewriting a named
    // component would silently retype it for every other reference in the document.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200":
          description: OK
          content:
            application/octet-stream:
              schema: { $ref: "#/components/schemas/Opaque" }
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Opaque" }
components:
  schemas:
    Opaque: {}
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type Opaque = serde_json::Value;"),
        "the component must stay exactly as declared: {code}"
    );
    assert!(!code.contains("pub type Opaque = bytes::Bytes;"), "{code}");
}

#[test]
fn a_media_range_is_generated_as_the_family_it_names() {
    // Media *ranges* are permitted `content` keys. They classified as nothing, so a response body
    // whose only entry was `video/*` was rejected outright rather than read as its family.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: OK
          content:
            video/*: { schema: {} }
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn an_image_or_audio_range_as_the_only_response_key_is_a_byte_body() {
    // The repro from #82 verbatim: `image/*` (and `audio/*`) as the *sole* `content` key, with
    // `schema: {}`. It was rejected outright, and only generated when an unrelated
    // `application/octet-stream` sibling happened to be listed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork/{id}:
    get:
      operationId: getArtwork
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: An image
          content:
            image/*: { schema: {} }
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: A clip
          content:
            audio/*: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
    assert_eq!(code.matches("= bytes::Bytes;").count(), 2, "{code}");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

#[test]
fn a_concrete_image_audio_or_video_type_is_a_byte_body() {
    // #82's second finding: `image/*` was accepted while `image/jpeg` — the same family, named
    // exactly — still fell to `E009`. A concrete member of a family that RFC 6838 reserves for
    // non-textual data is opaque octets under the same gate as `application/octet-stream`, in
    // both 3.1 spellings (`schema: {}`, no schema) and the 3.0 one (`format: binary`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: {} }
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: OK
          content:
            video/mp4: { schema: { type: string, format: binary } }
  /track:
    get:
      operationId: getTrack
      responses:
        "200":
          description: OK
          content:
            audio/mpeg: {}
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
    assert_eq!(code.matches("= bytes::Bytes;").count(), 3, "{code}");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

#[test]
fn a_concrete_binary_request_body_is_sent_as_its_own_content_type() {
    // Unlike a range, `image/png` is a dispatchable `Content-Type`, so it is accepted as a request
    // body and the header names it verbatim rather than `application/octet-stream`.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
}

#[test]
fn octet_stream_outranks_a_concrete_binary_type_listed_before_it() {
    // A concrete family member ranks just *below* `application/octet-stream`, so a document that
    // generated before the family rule existed keeps its selection: here `image/png` came first
    // and was merely an unsupported alternative, and it still is one. Octet-stream is selected,
    // the body is `bytes::Bytes`, and `image/png` — whose `format: byte` schema constrains
    // something, so it is not proved to decode identically — is reported as ignored (`W014`),
    // never rejected by the octet gate (`E009`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork:
    get:
      operationId: getArtwork
      responses:
        "200":
          description: OK
          content:
            image/png: { schema: { type: string, format: byte } }
            application/octet-stream: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message
                    .contains("`application/octet-stream` is generated")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

    // The same tie on the request side decides the wire `Content-Type`: octet-stream wins there
    // too, so the header a client already sent does not switch to the family type.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
          application/octet-stream: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"application/octet-stream\""), "{code}");
    assert!(!code.contains("\"image/png\""), "{code}");
}

#[test]
fn a_concrete_binary_type_ranks_last_except_against_a_request_range() {
    // A concrete family member sits at the very end of the ladder, below every other key spargen
    // can classify — octet-stream, text, the sequential kinds, and on a response every range. Each
    // document here but (iv) generated before the family rule existed with `image/png` as an
    // unsupported alternative; its selection, body type, outcome and wire `Content-Type` must not
    // move now that `image/png` classifies. The exception is a request offering a range, which it
    // cannot send as `Content-Type`: that was rejected before, and now sends the concrete key.

    // (i) Text keeps a response: `String`, and `image/png` is the alternative not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/csv: { schema: { type: string } }
            image/png: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`text/csv` is generated")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

    // (ii) Text keeps a request, and with it the header a client already sent.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    put:
      operationId: putNote
      requestBody:
        required: true
        content:
          text/plain: { schema: { type: string } }
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("\"text/plain\""), "{code}");
    assert!(!code.contains("\"image/png\""), "{code}");

    // (iii) A sequential kind keeps a response: the stream is still the selection.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              itemSchema: { type: object, required: [seq], properties: { seq: { type: integer } } }
            image/png: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );

    // (iv) A range is the one key a request cannot send — `Content-Type` must be concrete — so
    // on a request it is considered only once no concrete key classifies. Beside `image/png` the
    // concrete key is sent and the range is reported as the alternative not generated, rather
    // than the whole operation being rejected because an unsendable alternative was listed.
    for range in ["video/*", "*/*", "text/*"] {
        let range_schema = if range == "text/*" {
            "{ type: string }"
        } else {
            "{}"
        };
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          "{range}": {{ schema: {range_schema} }}
          image/png: {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{range}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{range}: {report:#?}"
        );
        assert!(
            code.contains("pub type RequestBody = bytes::Bytes;"),
            "{range}: {code}"
        );
        assert!(code.contains("\"image/png\""), "{range}: {code}");
        assert!(!code.contains(&format!("\"{range}\"")), "{range}: {code}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is generated")
                    && d.message.contains(&format!("`{range}`"))
            }),
            "{range}: {report:#?}"
        );
        assert_ne!(check(&spec).outcome(), Outcome::Rejected, "{range}");
    }

    // ... but only a concrete key that classifies: `application/pdf` names no codec, so the range
    // is still the selection and the request is still rejected for naming no sendable
    // `Content-Type`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          application/pdf: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains("`video/*` is a media range")
            }),
            "{report:#?}"
        );
    }

    // ... and the preferred concrete key still meets the octet gate: an object under `image/png`
    // is rejected on its own account, not waved through because a range stood beside it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          image/png: { schema: { type: object, properties: { a: { type: string } } } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message
                        .contains("`image/png` requires a string-like or binary schema")
            }),
            "{report:#?}"
        );
    }

    // (v) The `text/*` range keeps a response too: it generated as text beside an unclassified
    // `image/png` on master, and a concrete family member ranks below it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            image/png: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`text/*` is generated")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

    // (vi) ... whatever schema `image/png` carries: a losing alternative never reaches the octet
    // gate, so an object under `image/png` beside `text/*` generates as text instead of `E009`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /report:
    get:
      operationId: getReport
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            image/png: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("pub type ResponseBody = String;"), "{code}");
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

    // (vii) `*/*` keeps a response the same way: it generated as bytes beside an unclassified
    // `image/png` on master, and it still does whatever schema `image/png` carries — an object
    // there is reported as the alternative not generated (`W014`), never rejected by the octet
    // gate (`E009`).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            image/png: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        report.diagnostics().iter().any(|d| {
            d.code == Code::AlternativeMediaIgnored
                && d.message.contains("`*/*` is generated")
                && d.message.contains("`image/png`")
        }),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);

    // (viii) The family's own range outranks its concrete member: `image/*` beside `image/png`
    // with a constraining schema generates from the range, as on master.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/*: { schema: {} }
            image/png: { schema: { type: string, format: byte } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
}

#[test]
fn a_request_prefers_a_concrete_binary_key_listed_before_a_range() {
    // Source order is only the last tie-break: a concrete `image/png` listed *before* `video/*`
    // is sent for the same reason it is when listed after — a range is no `Content-Type` — and
    // the range is still reported as the alternative not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
    assert!(!code.contains("\"video/*\""), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is generated")
                    && d.message.contains("`video/*`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_concrete_binary_key_is_the_sendable_sibling_that_withholds_a_suffix_range() {
    // A structured-suffix range such as `application/*+json` is withheld from a request's choice
    // while a sibling can be sent. A concrete `image/png` is such a sibling: it classifies, is no
    // range, and is not streaming. So the request sends `image/png`, and the withheld range is
    // reported as the alternative not generated rather than selected and then refused.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          application/*+json: { schema: { type: object } }
          image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub type RequestBody = bytes::Bytes;"),
        "{code}"
    );
    assert!(code.contains("\"image/png\""), "{code}");
    assert!(!code.contains("\"application/*+json\""), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::AlternativeMediaIgnored
                    && d.message.contains("`image/png` is generated")
                    && d.message.contains("`application/*+json`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_request_offering_only_ranges_is_rejected_on_the_first() {
    // With no concrete key at all every candidate is a range, so the ladder and then source order
    // decide as before: `video/*` ties `*/*` at the same rank and, listed first, is selected; the
    // other range is reported as not generated (`W014`) and the selection is then rejected as a
    // request `Content-Type` (`E009`) — both diagnostics, naming each key once.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    put:
      operationId: putClip
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
          "*/*": { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let rejections: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnsupportedMediaType)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            rejections,
            [
                "media type `video/*` is a media range, which describes a family rather than the \
                 concrete `Content-Type` a request must send"
            ],
            "{report:#?}"
        );
        let ignored: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AlternativeMediaIgnored)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            ignored,
            ["`video/*` is generated; the alternative media type(s) `*/*` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_is_silent_across_concrete_and_ranged_byte_bodies() {
    // `image/jpeg`, `image/png`, `image/*` and `application/octet-stream` over empty schemas are one
    // representation four times over; picking one narrows nothing.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork:
    get:
      operationId: getArtwork
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: {} }
            image/png: { schema: {} }
            image/*: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_fires_for_a_concrete_binary_request_alternative() {
    // A request sends exactly one `Content-Type`, so a server documented as also accepting
    // `image/png` is narrowed at the wire whatever the decoded type — the octet exemption is a
    // response rule, and a concrete family alternative must not slip into it on a request.
    for (selection, alternative) in [
        ("application/octet-stream", "image/png"),
        ("image/jpeg", "image/png"),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /avatar:
    put:
      operationId: putAvatar
      requestBody:
        required: true
        content:
          {selection}: {{ schema: {{}} }}
          {alternative}: {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::AlternativeMediaIgnored
                        && d.message.contains(&format!("`{selection}` is generated"))
                        && d.message.contains(&format!("`{alternative}`"))
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn a_concrete_binary_type_is_matched_case_insensitively() {
    // `IMAGE/*` already reads as its family; `IMAGE/JPEG` must agree with it.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            IMAGE/JPEG: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn e009_a_concrete_binary_type_still_needs_a_binary_schema() {
    // The family says "octets"; the schema still has to agree. An object under `image/jpeg` is
    // rejected by the octet gate exactly as it is under `application/octet-stream`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    get:
      operationId: getPhoto
      responses:
        "200":
          description: OK
          content:
            image/jpeg: { schema: { type: object, properties: { a: { type: string } } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }

    // The request gate is reached through the same octet classification, and must name the
    // schema it refuses rather than call the family unsupported.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /photo:
    put:
      operationId: putPhoto
      requestBody:
        required: true
        content:
          image/jpeg: { schema: { type: object, properties: { a: { type: string } } } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message
                        .contains("requires a string-like or binary schema")
            }),
            "{report:#?}"
        );
    }
}

/// The raw-body schemas that admit `null` (#104), one per spelling: a union with `null` over
/// `format: binary`, over `contentEncoding: base64`, and over a `$ref` to a binary component, the
/// `anyOf` form, and the type-array form `type: [string, 'null']`, which used to lose its `null`
/// before any gate saw it.
const NULLABLE_BYTE_SCHEMAS: &[&str] = &[
    "{ oneOf: [ { type: string, format: binary }, { type: 'null' } ] }",
    "{ anyOf: [ { type: string, format: binary }, { type: 'null' } ] }",
    "{ oneOf: [ { type: string, contentEncoding: base64 }, { type: 'null' } ] }",
    "{ oneOf: [ { $ref: '#/components/schemas/Blob' }, { type: 'null' } ] }",
    "{ type: [string, 'null'], format: binary }",
    "{ type: [string, 'null'], contentEncoding: base64 }",
    "{ $ref: '#/components/schemas/NullableBlob' }",
];

const NULLABLE_BYTE_COMPONENTS: &str = r#"
components:
  schemas:
    Blob: { type: string, format: binary }
    NullableBlob: { type: [string, 'null'], format: binary }
"#;

fn assert_rejected_as_nullable_raw_body(spec: &str) {
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{spec}\n{report:#?}");
        assert!(
            messages_for(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message.contains("has no wire representation of `null`")),
            "{spec}\n{report:#?}"
        );
    }
}

#[test]
fn e009_a_nullable_raw_request_body_is_rejected() {
    // A raw request body is sent verbatim as octets, and octets carry no `null`: an
    // `Option<bytes::Bytes>` argument would ask the caller for a value the wire cannot send (the
    // absent body is `required: false`, which is a different construct). Every spelling used to
    // pass the frontend clean and generate `.body(body.clone())` over `Option<Bytes>`, which does
    // not compile (`E0277`) — under octet-stream, and under text and JSON too, because a `Bytes`
    // body is sent raw whatever its media.
    for version in ["3.1.0", "3.2.0"] {
        for media in ["application/octet-stream", "text/plain", "application/json"] {
            for required in [true, false] {
                for schema in NULLABLE_BYTE_SCHEMAS {
                    let spec = format!(
                        r#"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: {required}
        content:
          {media}: {{ schema: {schema} }}
      responses: {{ "204": {{ description: ok }} }}
{NULLABLE_BYTE_COMPONENTS}"#
                    );
                    assert_rejected_as_nullable_raw_body(&spec);
                }
            }
        }
    }

    // The raw text codec has the same hole: `text/plain` over a nullable string used to emit
    // `.body(body.to_string())` over `&Option<String>`, which does not compile (`E0599`).
    for schema in [
        "{ oneOf: [ { type: string }, { type: 'null' } ] }",
        "{ type: [string, 'null'] }",
    ] {
        let spec = format!(
            r#"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          text/plain: {{ schema: {schema} }}
      responses: {{ "204": {{ description: ok }} }}
"#
        );
        assert_rejected_as_nullable_raw_body(&spec);
    }
}

#[test]
fn e009_a_nullable_byte_response_body_is_rejected() {
    // A byte response is read as the raw octets of the body, so `null` is never what arrives; the
    // byte decoder has no `Option` to build, and the union spelling used to generate a
    // `ResponseValue<Bytes>` where `ResponseValue<Option<Bytes>>` was declared (`E0308`), while the
    // type-array spelling silently decoded plain `Bytes`. Success and error bodies, single and
    // multi-status, go through the same gate.
    let responses = [
        r#"{ "200": { description: ok, content: { MEDIA: { schema: SCHEMA } } } }"#,
        r#"{ "204": { description: ok }, "400": { description: bad, content: { MEDIA: { schema: SCHEMA } } } }"#,
        r#"{ "200": { description: ok, content: { MEDIA: { schema: SCHEMA } } }, "201": { description: ok, content: { application/json: { schema: { type: string } } } } }"#,
    ];
    for media in ["application/octet-stream", "text/plain", "application/json"] {
        for schema in NULLABLE_BYTE_SCHEMAS {
            for shape in responses {
                let responses = shape.replace("MEDIA", media).replace("SCHEMA", schema);
                let spec = format!(
                    r#"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /d:
    get:
      operationId: download
      responses: {responses}
{NULLABLE_BYTE_COMPONENTS}"#
                );
                assert_rejected_as_nullable_raw_body(&spec);
            }
        }
    }
}

#[test]
fn a_nullable_byte_string_keeps_its_null_outside_a_raw_body() {
    // `type: [string, 'null']` with `format: binary` or `contentEncoding: base64` is `bytes::Bytes`
    // that also admits `null`, exactly as the union spelling is. Where the value is a JSON member,
    // or a multipart part, `null` has a representation, so the field is `Option<bytes::Bytes>` —
    // it used to be plain `Bytes`, which rejects the very `null` the schema allows.
    let (report, code) = generate_with_code(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [typed, united]
              properties:
                typed: { type: [string, 'null'], contentEncoding: base64 }
                united: { oneOf: [ { type: string, contentEncoding: base64 }, { type: 'null' } ] }
      responses: { "204": { description: ok } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        code.contains("pub type RequestBodytyped = bytes::Bytes;"),
        "{code}"
    );
    assert!(
        code.contains("pub typed: Option<RequestBodytyped>,"),
        "{code}"
    );
    assert!(code.contains("pub united: Option<"), "{code}");

    // A nullable raw *text* response stays supported: the text codec decodes through serde, so
    // `Option<String>` is built (always `Some`) and compiles. Only the byte codec, which has no
    // `Option` to build, is rejected.
    let report = generate(
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /d:
    get:
      operationId: download
      responses:
        "200": { description: ok, content: { text/plain: { schema: { type: [string, 'null'] } } } }
"#,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
}

#[test]
fn e009_a_concrete_type_outside_the_byte_families_stays_unsupported() {
    // `application/*` mixes binary (`pdf`) with textual (`sdp`) subtypes and `font/*` is not
    // claimed, so none of them may be read as bytes on the strength of a prefix. This is the
    // response-side twin of `e009_unsupported_media_type`, and what keeps the openai corpus
    // expectation honest.
    // A family key with no subtype, or with a wildcard that does not make it a range, is not a
    // member of the family either.
    for media in [
        "application/pdf",
        "application/sdp",
        "font/woff2",
        "image/",
        "image/pn*",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{media}": {{ schema: {{}} }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "{media}: {report:#?}"
            );
        }
    }

    // On a request such a key would be sent verbatim as `Content-Type`, so it must be rejected
    // there too rather than read as bytes.
    for media in ["image/", "image/pn*"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    put:
      operationId: putX
      requestBody:
        required: true
        content:
          "{media}": {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "{media}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_member_of_a_byte_family_stays_unsupported() {
    // `image/svg+xml` sits in the `image` family, but its RFC 6838 `+xml` suffix says the payload
    // is a text syntax, so bytes would be the silently-wrong reading the family rule exists to
    // avoid. It stays `E009`, as it was before the family rule existed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /logo:
    get:
      operationId: getLogo
      responses:
        "200":
          description: OK
          content:
            image/svg+xml: { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_a_form_urlencoded_string_property_declaring_a_binary_family_content_type() {
    // The family rule reaches Encoding Objects through the shared classifier: a form-urlencoded
    // property whose `contentType` names `image/png` is binary, which a form body cannot carry —
    // the disposition `application/octet-stream` already has there — and the message names what
    // was declared, since the schema itself is a plain string a reader cannot call binary.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                pic: { type: string }
            encoding:
              pic: { contentType: image/png }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains("`pic`")
                    && d.message.contains("`contentType: image/png`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_form_urlencoded_binary_property_without_a_declared_content_type() {
    // The other arm of the same rejection: no Encoding Object at all, so the property is binary
    // on the strength of its own schema (`contentEncoding: base64`, which lowers to bytes) and its
    // `contentType` merely defaulted to `application/octet-stream`. The message must not name a
    // `contentType` the document never wrote.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                pic: { type: string, contentEncoding: base64 }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains("`pic`")
                    && d.message.contains(
                        "is binary, which has no `application/x-www-form-urlencoded` \
                         representation",
                    )
                    && !d.message.contains("declares `contentType:")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_multipart_string_property_declaring_a_binary_family_content_type_is_a_text_part() {
    // The same declaration on multipart is unchanged by the family rule: a part is rendered by its
    // property's own lowered type, so a string stays a text part, and the declared `contentType`
    // rides on it as the part's header, exactly as `application/sdp` does.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /profile:
    post:
      operationId: postProfile
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                pic: { type: string }
            encoding:
              pic: { contentType: image/png }
      responses:
        '204': { description: ok }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(code.contains("reqwest::multipart::Part::text("), "{code}");
    assert!(code.contains(".mime_str(\"image/png\")"), "{code}");
}

#[test]
fn w011_a_response_header_with_binary_family_content_is_acknowledged() {
    // A response header's `content` keyed `image/png` now classifies (as opaque octets) instead of
    // failing to classify, and lands on the existing header rule either way: octets are not a
    // header value spargen decodes, so the accessor is dropped with `W011` and the operation
    // generates — the disposition `*/*` already has there.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          headers:
            X-Thumbnail:
              content:
                image/png: { schema: {} }
          content:
            application/json: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::DeclarationHasNoEffect && d.message.contains("`X-Thumbnail`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_binary_family_parameter_content_is_rejected_by_its_position() {
    // Parameter `content` shares the body classifier, so `image/png` there classifies as octets
    // and meets each position's own rule instead of the generic "not supported": a querystring
    // takes only JSON or form content (`E010`, as `video/*` already gets there), and any other
    // `content` parameter needs a single-token codec (`E009`).
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: q
          in: querystring
          content:
            image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedParameterStyle
                    && d.message
                        .contains("querystring media type `image/png` is not supported")
            }),
            "{report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }

    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: thumb
          in: query
          content:
            image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains(
                        "`content` parameter media type `image/png` has no single-token \
                         serialization",
                    )
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_concrete_media_type_outranks_a_range_that_precedes_it() {
    // Ranges rank below every codec, so a concrete sibling wins wherever it sits in the document
    // (the one type ranked below the ranges is a concrete `image`/`audio`/`video` member, and only
    // on a response; pinned elsewhere). Two ranges at the same rank still tie by source order, as
    // equal-ranked concrete media already do.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            application/json: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // JSON won, so the alternative that was dropped is the range — and that one really is a
    // narrowing, because a JSON body and an opaque one decode differently.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn w014_is_silent_when_every_alternative_decodes_identically() {
    // The ranged-media response shape from #72, as emitted for a byte range: three keys, one
    // representation. Every opaque-octets body is `bytes::Bytes` whatever its schema says, so
    // generating one of them gives up nothing and the warning was pure noise on a common shape.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /media/{id}:
    get:
      operationId: getMedia
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: OK
          content:
            video/*: { schema: {} }
            audio/*: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_still_fires_for_an_alternative_spargen_cannot_decode() {
    // The narrowing above must not swallow a real one: an unregistered media type sitting beside a
    // byte body is a documented surface that is genuinely not generated.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            application/sdp: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn e009_a_media_range_cannot_be_a_request_content_type() {
    // A range describes what a server may return, not what a client sends. A generated request puts
    // its media key on the wire verbatim, and `Content-Type: video/*` is not a dispatchable header
    // — while choosing a concrete member of the family would be spargen inventing what the document
    // declined to say.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w014_still_fires_when_an_octet_alternative_constrains_a_shape() {
    // The suppression may only cover alternatives that really do decode identically. An
    // octet-classified alternative carrying an object schema would be *rejected* by the octet gate,
    // not turned into bytes, so it is a genuine narrowing and must still be reported.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {}
            video/*: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_a_referenced_media_object() {
    // A 3.2 Media Type Object may itself be a Reference Object, which parses with no `schema` of
    // its own. Reading that absence as "constrains nothing" would call the alternative opaque on
    // the strength of a field the `$ref` spelling never sets, and drop the warning for a structured
    // body — the byte-identical inline spelling reports it, so the two must agree.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { $ref: "#/components/mediaTypes/Manifest" }
components:
  mediaTypes:
    Manifest:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_still_fires_when_an_alternative_schema_is_a_ref() {
    // The other place a reference hides. Proving what a `$ref` points at would mean resolving it
    // during selection; answering "unknown" as "not opaque" only keeps a warning that was already
    // being reported.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: { $ref: "#/components/schemas/Manifest" } }
components:
  schemas:
    Manifest: { type: object, properties: { a: { type: string } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_text_alternatives_that_only_look_identical() {
    // The identical-decode rule stays confined to octet-stream. Two textual bodies that constrain
    // nothing look interchangeable and are not: this pair is `serde_json::Value` twice, while the
    // same pair written without `schema:` at all is `()` twice, and a mixed pair is one of each.
    // Only the octet gate collapses every body it admits onto one type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: {} }
            text/csv: { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_fires_when_two_empty_spellings_lower_differently() {
    // `constrains nothing` has two spellings that disagree outside the octet gate: no `schema` key
    // lowers to `()`, `schema: {}` lowers to `Any`. Suppressing between them would make the *order*
    // of two content keys decide the response type, in silence.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: {}
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    // The selection is the one with no schema at all, so the body really is unit-typed. The
    // `schema: {}` sibling would have been `serde_json::Value`, which is the whole point: the two
    // spellings are not interchangeable, so neither may silence the other.
    assert!(
        code.contains("support::ResponseValue<()>"),
        "the no-schema selection should lower to `()`: {code}"
    );
}

#[test]
fn w014_fires_for_an_octet_request_alternative_that_decodes_alike() {
    // A request narrows at the wire even when both entries are `bytes::Bytes`: the selected media
    // key becomes the `Content-Type` verbatim, so this client only ever sends octet-stream to a
    // server documented as also accepting `video/*`. The range-as-request rejection does not cover
    // this — that fires on the media actually selected, and a suppressed alternative never is.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_fires_when_an_octet_alternative_describes_itself_outside_its_schema() {
    // A Media Type Object says things outside `schema`. `itemSchema` in particular carries a type,
    // so an entry declaring one is not interchangeable with an empty body even though its `schema`
    // is empty — reading only `schema` is how that slipped through before.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { itemSchema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }

    // `encoding` is the other half of the same claim, and the explain text names it. It is inert on
    // an octet media, but an entry that spells it out is still saying more than an empty one.
    let with_encoding = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { encoding: { part: { contentType: text/plain } } }
"##;
    for report in [generate(with_encoding), check(with_encoding)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_fires_for_sequential_alternatives_with_different_item_types() {
    // A sequential media's item type lives in `itemSchema`, outside the body schema entirely. Two
    // entries can both constrain nothing in `schema` and still stream different types.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson: { itemSchema: { type: string } }
            application/jsonl: { itemSchema: { type: integer } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_fires_for_request_body_alternatives_that_decode_alike() {
    // A request narrows at the wire, not at the type. The chosen media key becomes `Content-Type`
    // verbatim, so a server documented as accepting both is only ever sent one — whatever the Rust
    // types do.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/json: { schema: {} }
          application/vnd.acme.v2+json: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_still_fires_when_the_text_selection_is_typed() {
    // The *selection* has to reach the shared representation too. A textual body carrying a string
    // enum lowers to a typed value rather than to `String`, so an opaque sibling beside it really
    // does document something the client will never accept.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string, enum: [a, b] } }
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn an_octet_selection_with_a_binary_schema_still_silences_an_opaque_range() {
    // Octet-stream is the one codec whose gate admits only bodies that collapse to `bytes::Bytes`,
    // so the selection does not have to be empty for the alternatives to decode identically. The
    // 3.0 spelling of a binary body beside the 3.1 one narrows nothing.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { type: string, format: binary } }
            video/*: { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_still_fires_when_an_alternative_carries_a_validation_keyword() {
    // A validation keyword never changes the storage type, so both bodies are `bytes::Bytes` — but
    // the document still said something about one of them, and an alternative that says something
    // is not interchangeable with one that says nothing.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: { maxLength: 10 } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_the_always_false_schema() {
    // `false` is the one boolean schema that constrains — to nothing at all. It is not the empty
    // schema and must not be read as one.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: false }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_every_kind_of_constraining_alternative() {
    // `constrains_nothing` is a long conjunction, and dropping any one term would silently widen
    // the suppression — the exhaustive destructure it is built on catches a field that is *added*
    // to `Schema`, never one that stops being consulted. Each schema below trips exactly one term,
    // so deleting that term turns this test red instead of turning a real narrowing silent.
    for constraining in [
        "{ type: object, properties: { a: { type: string } } }",
        "{ required: [a] }",
        "{ additionalProperties: false }",
        "{ patternProperties: { \"^x-\": { type: string } } }",
        "{ items: { type: string } }",
        "{ prefixItems: [{ type: string }] }",
        "{ allOf: [{ type: string }] }",
        "{ oneOf: [{ type: string }] }",
        "{ anyOf: [{ type: string }] }",
        "{ enum: [a, b] }",
        "{ const: a }",
        "{ format: uuid }",
        "{ contentEncoding: base64 }",
        "{ contentMediaType: application/json }",
        "{ maxLength: 10 }",
        "false",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {{ schema: {{}} }}
            video/*: {{ schema: {constraining} }}
"##
        );
        let report = generate(&spec);
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "an alternative constrained by `{constraining}` was suppressed as decoding \
             identically: {report:#?}"
        );
    }
}

#[test]
fn a_binary_body_whose_schema_failed_to_lower_is_not_retyped_to_bytes() {
    // Retyping to `Bytes` applies to a body that *said nothing*. A body that declared a schema
    // which then failed to lower has already reported its own failure, and quietly handing it back
    // as bytes would turn that error into a silently different type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { $ref: "#/components/schemas/Loop" } }
components:
  schemas:
    Loop: { $ref: "#/components/schemas/Loop" }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
    }
}

#[test]
fn a_media_range_is_matched_case_insensitively() {
    // Media types are case-insensitive (RFC 9110 § 8.3.1). Reading `TEXT/*` as a binary family
    // would be silently wrong rather than loudly unsupported.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            TEXT/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "an upper-case text range is still text: {code}"
    );
}

#[test]
fn e009_a_range_naming_no_family_is_unsupported() {
    // `/*` has no type before the slash, so it names nothing. It stays unsupported rather than
    // being read as opaque octets on the strength of its suffix alone.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "/*": { schema: {} }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn e009_a_media_range_with_an_extra_slash_is_unsupported() {
    // `a/b/*` ends in `/*`, but what precedes the suffix is `a/b`, which is not a type name. It was
    // read as the family `a/b` and generated as opaque octets. A key that is not a media range names
    // no family, so it is unsupported like any other key that is not a media type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "a/b/*": { schema: {} }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

/// Assert that each `(key, request, schema)` case, as the sole `content` key of a request body
/// (`request`) or a response, is rejected with `E009` through both `generate` and `check`.
fn assert_each_media_key_is_unsupported(cases: &[(&str, bool, &str)]) {
    fn document(key: &str, request: bool, schema: &str) -> String {
        if request {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "{key}": {{ schema: {schema} }}
      responses:
        "204": {{ description: No Content }}
"##
            )
        } else {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
            )
        }
    }
    for &(key, request, schema) in cases {
        let spec = document(key, request, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{key}` through {entry}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::UnsupportedMediaType),
                "`{key}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_media_key_that_is_not_a_restricted_name_is_unsupported() {
    // A media type is exactly one `/` between two RFC 6838 § 4.2 restricted names. Each key below
    // breaks that, yet most reached a codec through an arm that matched only part of the key: the
    // `text/` prefix, the `application/…+json` suffix, or a range's `/*`. `image/jpeg/extra` pins
    // the concrete binary family, which must not be read as `image` octets.
    // One byte past the 127-byte limit on a restricted name.
    let too_long = format!("text/{}", "a".repeat(128));
    assert_each_media_key_is_unsupported(&[
        ("image/jpeg/extra", true, "{}"),
        ("text/plain/extra", false, "{ type: string }"),
        ("application/vnd.a/b+json", true, "{ type: object }"),
        ("text/", false, "{ type: string }"),
        ("text/pl ain", false, "{ type: string }"),
        ("**/*", false, "{}"),
        (too_long.as_str(), false, "{ type: string }"),
    ]);
}

#[test]
fn e009_a_wildcard_inside_a_name_is_unsupported() {
    // `*` is a whole-name wildcard, never part of a name: `*/json` is no range (a range fixes the
    // type and wildcards the subtype), and `image/pn*` is no type at all. Neither ever reached an
    // arm, so `text/pl*in` and `application/vn*+json` are here to discriminate: without the rule,
    // the `text/` prefix arm and the `+json` suffix arm would accept them.
    assert_each_media_key_is_unsupported(&[
        ("*/json", false, "{ type: object }"),
        ("image/pn*", false, "{}"),
        ("text/pl*in", false, "{ type: string }"),
        ("application/vn*+json", false, "{ type: object }"),
    ]);
}

#[test]
fn e009_a_name_starting_with_a_symbol_is_unsupported() {
    // `.`, `-` and the other symbols RFC 6838 § 4.2 permits may follow the first byte of a
    // restricted name but may not be it, in the type position or the subtype position.
    assert_each_media_key_is_unsupported(&[
        (".type/x", false, "{ type: string }"),
        ("-x/y", false, "{ type: string }"),
        ("text/.plain", false, "{ type: string }"),
        ("text/-plain", false, "{ type: string }"),
    ]);
}

#[test]
fn e009_a_malformed_parameter_content_key_is_unsupported() {
    // Both parameter call sites classify their `content` key: a `content` parameter and a 3.2
    // `in: querystring` parameter. Neither may render a value through a key that is not a type.
    let content_parameter = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: filter
          in: query
          content:
            "text/plain/extra": { schema: { type: string } }
      responses:
        "204": { description: No Content }
"##;
    let querystring_parameter = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: q
          in: querystring
          content:
            "text/plain/extra": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for spec in [content_parameter, querystring_parameter] {
        for report in [generate(spec), check(spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::UnsupportedMediaType
                        && diagnostic.message == "media type `text/plain/extra` is not supported"
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_malformed_response_header_content_key_is_unsupported() {
    // A response header's `content` key is classified like any other, so a key that is not a type
    // is reported rather than decoded as text on the strength of its `text/` prefix.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          headers:
            X-Detail:
              content:
                "text/plain/extra": { schema: { type: string } }
          content:
            application/json: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message == "media type `text/plain/extra` is not supported"
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_malformed_multipart_part_content_type_is_not_diagnosed() {
    // Pinned as it stands, not endorsed. A multipart part's `contentType` is a header value, not a
    // `content` key: the part is built from the property's own type, and the declared string is
    // attached verbatim through `mime_str`. Generation reports nothing for a malformed one. It
    // surfaces only when a request is built, as a request-construction error, because reqwest's
    // media type parser rejects the extra `/`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                note: { type: string }
            encoding:
              note: { contentType: "text/plain/extra" }
      responses:
        "204": { description: No Content }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
    }
    assert!(code.contains("mime_str(\"text/plain/extra\")"), "{code}");
    assert!(
        code.contains("reqwest::multipart::Part::text(value.to_string())"),
        "the part is built from the string property, not from the declared type: {code}"
    );
}

#[test]
fn w014_a_malformed_key_beside_a_well_formed_sibling_is_ignored() {
    // A malformed key does not reject the whole map when a well-formed sibling exists: only that
    // key is dropped, and it is named under W014 like any other alternative that is not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "text/plain/extra": { schema: { type: string } }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::AlternativeMediaIgnored
                    && diagnostic.message
                        == "`application/json` is generated; the alternative media type(s) \
                            `text/plain/extra` are not"
            }),
            "{report:#?}"
        );
    }
    assert!(code.contains("pub type ResponseBodyid = i64;"), "{code}");
}

#[test]
fn well_formed_media_keys_still_generate() {
    // The restricted-name check must not reject what real descriptions write: dotted vendor `+json`
    // types, a key with parameters (stripped before the check), both kinds of range, every
    // non-alphanumeric byte RFC 6838 permits, and a subtype at exactly the 127-byte limit.
    let at_limit = format!("text/{}", "a".repeat(127));
    let keys = [
        ("application/vnd.github+json", "{ type: object }"),
        ("application/vnd.github.v3.star+json", "{ type: object }"),
        ("text/plain; charset=utf-8", "{ type: string }"),
        ("*/*", "{}"),
        ("application/*", "{}"),
        ("text/x-a!b#c$d&e^f_g.h+i", "{ type: string }"),
        (at_limit.as_str(), "{ type: string }"),
    ];
    // One operation per key, each with a single content entry, so no key competes with another.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for (index, (key, schema)) in keys.into_iter().enumerate() {
        spec += &format!(
            r##"  /op{index}:
    get:
      operationId: op{index}
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
        );
    }
    let (report, code) = generate_with_code(&spec);
    let checked = check(&spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
    }
    // Seven operations share the `ResponseBody` name, so each alias carries a disambiguating suffix.
    let opaque = code
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("pub type ResponseBody") && line.ends_with(" = bytes::Bytes;")
        })
        .count();
    assert_eq!(
        opaque, 2,
        "exactly the `*/*` and `application/*` ranges are opaque octets: {code}"
    );
}

#[test]
fn a_structured_suffix_range_response_generates_json() {
    // `application/*+json` is a media range over every structured JSON subtype (RFC 9110's
    // media-range grammar, with RFC 6838 § 4.2.8 structured syntax suffixes). The support matrix
    // promises it as JSON, and a response offering it alone is decoded as JSON rather than
    // rejected for its `*`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "application/*+json":
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
    }
    assert!(
        code.contains("pub struct ResponseBody {")
            && code.contains("pub type ResponseBodyid = i64;"),
        "a typed JSON body, not bytes or text: {code}"
    );
}

#[test]
fn e009_a_structured_suffix_range_cannot_be_a_request_content_type() {
    // A request puts its media key on the wire verbatim, and `Content-Type: application/*+json`
    // names a family rather than a type, exactly like `video/*`. It is rejected as a range.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message.contains("is a media range")),
            "{report:#?}"
        );
    }
}

/// A request body whose `content` lists `application/*+json` first and then `sibling`.
fn suffix_range_request_document(sibling: &str, sibling_schema: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": {{ schema: {{ type: object }} }}
          "{sibling}": {{ schema: {sibling_schema} }}
      responses:
        "204": {{ description: No Content }}
"##
    )
}

#[test]
fn w014_a_structured_suffix_range_yields_to_a_sendable_request_sibling() {
    // `application/*+json` ranks with the concrete JSON types, so listed first it would win the
    // tie by source order and then be refused as a range, rejecting a body that offered something
    // sendable. While a concrete sibling can be sent, the range is not a candidate: the sibling is
    // generated and the range is reported as the alternative that is not, whatever the sibling's
    // rank (`text/plain` ranks below every JSON type).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("application/merge-patch+json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            let expected = format!(
                "`{sibling}` is generated; the alternative media type(s) `application/*+json` are not"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_with_no_sendable_request_sibling_is_unsupported() {
    // With nothing beside it that a request could send, the suffix range is still what the body
    // offers, and it is still refused as a range. Neither another range nor a streaming media
    // counts as sendable.
    for (sibling, schema) in [("video/*", "{}"), ("text/event-stream", "{ type: string }")] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::UnsupportedMediaType
                        && diagnostic
                            .message
                            .starts_with("media type `application/*+json` is a media range")
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

/// A request body whose `content` lists each `(key, schema)` entry in order.
fn request_body_document(entries: &[(&str, &str)]) -> String {
    let content: String = entries
        .iter()
        .map(|(key, schema)| format!("          \"{key}\": {{ schema: {schema} }}\n"))
        .collect();
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
{content}      responses:
        "204": {{ description: No Content }}
"##
    )
}

/// The message of every `code` diagnostic in `report`, in report order.
fn messages_with_code(report: &Report, code: Code) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
}

#[test]
fn w014_a_suffix_range_listed_after_a_sendable_request_sibling_is_withheld() {
    // Order does not decide it. A sendable sibling listed before the range is generated and the
    // range is named as not generated, even for a sibling whose rank the range's rank 0 would
    // otherwise beat (`text/plain`).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = request_body_document(&[
            (sibling, schema),
            ("application/*+json", "{ type: object }"),
        ]);
        let expected = format!(
            "`{sibling}` is generated; the alternative media type(s) `application/*+json` are not"
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert_eq!(
                messages_with_code(&report, Code::AlternativeMediaIgnored),
                [expected.as_str()],
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_every_suffix_range_beside_a_sendable_request_sibling_is_withheld() {
    // Every structured-suffix range that classifies is withheld, not only the first, and all of
    // them are named in one report.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/*+json-seq", "{ type: object }"),
        ("application/json", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is generated; the alternative media type(s) \
                 `application/*+json`, `application/*+json-seq` are not"
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_suffix_range_beside_only_an_unclassified_request_sibling_is_a_media_range() {
    // A sibling that does not classify is not sendable, so nothing is withheld: the range is still
    // the only thing the body offers, and it is refused as a range.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/pdf", "{}"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            messages_with_code(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message
                    .starts_with("media type `application/*+json` is a media range")),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_sendable_request_sibling_that_fails_its_own_gate_is_reported_for_itself() {
    // Sendable is decided by classification alone. A `text/plain` sibling carrying an object schema
    // is still chosen over the range, and then refused by the raw-text gate for its own reason
    // rather than the range's.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("text/plain", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let unsupported = messages_with_code(&report, Code::UnsupportedMediaType);
        assert!(
            unsupported
                .iter()
                .all(|message| !message.contains("is a media range")),
            "{report:#?}"
        );
        assert!(
            unsupported
                .iter()
                .any(|message| message.contains("text/plain")),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_a_withheld_suffix_range_beside_two_request_entries_is_reported_separately() {
    // Pinned as it stands. `choose_media` names the alternatives it passed over, and the withheld
    // range gets its own W014 right after, so this body carries two: both true, always in this
    // order.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/json", "{ type: object }"),
        ("application/xml", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is generated; the alternative media type(s) \
                 `application/xml` are not",
                "`application/json` is generated; the alternative media type(s) \
                 `application/*+json` are not",
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn a_sequential_media_outranks_a_text_range() {
    // `text/*` used to classify as `Text` by accident of the `text/` prefix arm, at the same rank
    // as a concrete textual type and *above* sequential media — so this response was a whole-body
    // `String`. As a range it now ranks below both, and the concrete streaming type wins. That is a
    // change to the generated public API, so it is pinned rather than left to be rediscovered.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    // Two media types that decode differently: the range really is not generated.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn a_textual_content_response_header_gets_a_typed_accessor() {
    // `Content-Range` on a ranged response is routinely documented with `content:` rather than
    // `schema:`. A textual content entry describes the field value itself, so it decodes exactly
    // like the `schema:` spelling — refusing it cost the accessor and reported `W011` on a shape
    // that is neither rare nor wrong.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    get:
      operationId: getClip
      responses:
        "206":
          description: Partial Content
          headers:
            Content-Range:
              content:
                text/plain: { schema: { type: string } }
          content:
            application/octet-stream: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
    assert!(code.contains("content_range"), "{code}");
    assert_ne!(check(spec).outcome(), Outcome::Rejected, "{report:#?}");
}

#[test]
fn w011_response_header_content_media_spargen_cannot_decode_is_reported() {
    // The other side of the change, and a raise site that had no fixture: a `content` media type
    // that is neither JSON nor textual still drops the accessor, and still says so. `*/*` is here
    // because it used to be rejected outright (`E009`, unclassifiable) and now classifies as a
    // binary range — the operation generates and the header is acknowledged instead.
    for media in ["application/xml", r#""*/*""#] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          headers:
            X-Detail:
              content:
                {media}: {{ schema: {{ type: string }} }}
          content:
            application/json: {{ schema: {{ type: string }} }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                has_code(&report, Code::DeclarationHasNoEffect),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn w011_a_textual_content_response_header_that_is_not_a_single_value_is_reported() {
    // Textual content carries the field value verbatim, so a list says nothing about how the value
    // is framed — and `simple` is not that framing.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          headers:
            X-Detail:
              content:
                text/plain: { schema: { type: array, items: { type: string } } }
          content:
            application/json: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

#[test]
fn security_scheme_documentation_reaches_credential_registration() {
    // The support matrix promises `bearerFormat`, flows, `openIdConnectUrl` and deprecation become
    // rustdoc on credential registration. `SecuritySchemeObject` used to carry four fields and none
    // of these, so every one of them was dropped without a trace.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - oauth: [read]
      responses:
        "204": { description: No Content }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
      bearerFormat: JWT
      description: A short-lived service token.
      deprecated: true
    oidc:
      type: openIdConnect
      openIdConnectUrl: https://id.example.com/.well-known/openid-configuration
    oauth:
      type: oauth2
      oauth2MetadataUrl: https://id.example.com/.well-known/oauth-authorization-server
      flows:
        authorizationCode:
          authorizationUrl: https://id.example.com/authorize
          tokenUrl: https://id.example.com/token
          scopes:
            read: Read your data
        deviceAuthorization:
          deviceAuthorizationUrl: https://id.example.com/device
          tokenUrl: https://id.example.com/token
          scopes:
            read: Read your data
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for expected in [
        "Bearer format: `JWT`",
        "A short-lived service token.",
        "**Deprecated.**",
        "OpenID Connect discovery: <https://id.example.com/.well-known/openid-configuration>",
        "OAuth 2 metadata: <https://id.example.com/.well-known/oauth-authorization-server>",
        "Flow `authorizationCode`",
        // OpenAPI 3.2's device flow, which `docs/openapi-3.2.md` also claims is documented.
        "Flow `deviceAuthorization`",
        "device authorization: <https://id.example.com/device>",
        "scope `read` — Read your data",
    ] {
        assert!(code.contains(expected), "missing {expected:?} in: {code}");
    }
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
    let report = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let code = std::fs::read_to_string(&out).unwrap();
    // The header became a typed accessor and the media type contributed the body type, so both
    // references were genuinely followed rather than merely tolerated.
    assert!(code.contains("x_request_id"), "{code}");
    assert!(code.contains("pub id"), "{code}");
}

/// `encoding.headers` accepts a Header Object or a `$ref` to one. The reference was never
/// resolved, so a target that pins a `const` was reported as pinning no value (`W011`) and the
/// part shipped without the header. Resolving it makes the two spellings equivalent.
#[test]
fn encoding_header_ref_is_resolved_and_pins_its_const() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/PartKind' }
      responses:
        '204': { description: ok }
components:
  headers:
    PartKind:
      description: which part this is
      schema: { type: string, const: profile }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "a resolved header that pins a const has an effect: {report:#?}"
    );
    assert!(code.contains("X-Part-Kind"), "{code}");
    assert!(code.contains("profile"), "{code}");
}

/// An `encoding.headers` reference that resolves to nothing is `E004` — and only `E004`, so one
/// defect does not also collect a "pins no value" warning naming a second, wrong reason.
#[test]
fn e004_unresolvable_encoding_header_ref_does_not_also_warn() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/Missing' }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

/// A run that reaches `batch_cap` drops every diagnostic past it. Presenting that shortened list
/// as if it were the whole one is the silent behavior the disposition invariant forbids, so the
/// report says it is partial and `Display` carries the marker.
#[test]
fn a_report_that_hit_the_batch_cap_says_it_is_truncated() {
    // Each of these paths carries its own unsupported media type, so the run has far more
    // diagnostics to emit than the cap allows.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for index in 0..12 {
        spec.push_str(&format!(
            "  /item{index}:\n    get:\n      responses:\n        '200':\n          description: ok\n          content:\n            application/vnd.unsupported: {{ schema: {{ type: string }} }}\n"
        ));
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("openapi.yaml");
    std::fs::write(&path, &spec).unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(path).unwrap();

    let uncapped = spargen::check(&Spec::new(spec_path.clone()).batch_cap(100));
    assert!(!uncapped.truncated(), "{uncapped:#?}");
    let all = uncapped.diagnostics().len();
    assert!(all > 3, "need more than the cap to prove truncation: {all}");

    let capped = spargen::check(&Spec::new(spec_path).batch_cap(3));
    assert!(capped.truncated(), "{capped:#?}");
    assert_eq!(capped.diagnostics().len(), 3, "{capped:#?}");
    assert!(
        capped.to_string().contains("truncated at batch_cap"),
        "{capped}"
    );
}

/// The support matrix says homogeneous scalar enums are supported; floats are the boundary. They
/// have no representable Rust enum discriminant, so they are `E008` rather than a silent demotion,
/// and the matrix now states the boundary rather than implying floats are included.
#[test]
fn e008_float_enum_members_are_the_documented_boundary() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Ratio:
      type: number
      enum: [1.5, 2.5]
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::NonScalarEnum), "{report:#?}");
    }

    // Strings, integers, and booleans stay supported.
    for (kind, values) in [
        ("string", "[a, b]"),
        ("integer", "[1, 2]"),
        ("boolean", "[true, false]"),
    ] {
        let ok = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  schemas:\n    Kind:\n      type: {kind}\n      enum: {values}\n"
        );
        let report = generate(&ok);
        assert_ne!(report.outcome(), Outcome::Rejected, "{kind}: {report:#?}");
    }
}

// --- Media Type Object dispositions outside the request body -------------------------------
//
// `resolve_media_object` is the seam every Media Type Object passes through. Before these
// fixtures, only the request-body path dispositioned `encoding`; the same declaration on a
// response, a parameter, a header, or a `components.mediaTypes` entry was dropped in silence.

#[test]
fn w011_encoding_on_a_response_media_type_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { type: object, properties: { a: { type: string } } }
              encoding:
                a: { contentType: text/plain }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_on_a_parameter_content_media_type_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          content:
            application/json:
              schema: { type: object, properties: { a: { type: string } } }
              encoding:
                a: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_encoding_reached_through_a_component_media_type_reference_has_no_effect() {
    // The declaration is one `$ref` hop away from its use site. Resolving it is what makes the
    // disposition reachable at all.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/Payload'
components:
  mediaTypes:
    Payload:
      schema: { type: object, properties: { a: { type: string } } }
      encoding:
        a: { contentType: text/plain }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_media_type_reference_summary_documents_the_use_site() {
    // A Reference Object's own `summary`/`description` documents this use site, which one shared
    // generated item cannot express — the same disposition parameters and responses already get.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              $ref: '#/components/mediaTypes/Payload'
              summary: the payload as returned by this operation
components:
  mediaTypes:
    Payload:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_prefix_encoding_on_form_urlencoded_has_no_effect() {
    // The specification scopes `prefixEncoding`/`itemEncoding` to `multipart`, so on a form body
    // they are inert rather than an error — this was previously over-rejected as E009.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema: { type: object, properties: { a: { type: string } } }
            prefixEncoding:
              - { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_prefix_encoding_on_multipart_is_rejected() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: object, properties: { a: { type: string } } }
            prefixEncoding:
              - { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn e009_item_encoding_on_multipart_is_rejected() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: { type: object, properties: { a: { type: string } } }
            itemEncoding: { contentType: text/plain }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn response_header_content_media_type_reference_resolves() {
    // Reading `schema` off an unresolved Reference Object found `None` and dropped the typed
    // accessor with nothing said. Resolving first is what makes the header typed.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json:
                  $ref: '#/components/mediaTypes/Meta'
          content:
            application/json:
              schema: { type: string }
components:
  mediaTypes:
    Meta:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("x_meta"),
        "the typed header accessor should be generated: {code}"
    );
}

#[test]
fn w011_response_header_content_without_a_schema_is_reported() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json: {}
          content:
            application/json:
              schema: { type: string }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn w010_item_schema_on_a_response_header_is_ignored() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Meta:
              content:
                application/json:
                  schema: { type: object, properties: { a: { type: string } } }
                  itemSchema: { type: string }
          content:
            application/json:
              schema: { type: string }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::Oas32ConstructIgnored),
            "{report:#?}"
        );
    }
}

// --- OpenAPI 3.2 XML nodeType defaulting ----------------------------------------------------
//
// 3.2 replaced `attribute`/`wrapped` with `nodeType` and gave it a defaulting table: `$ref` and
// `type: array` schemas default to `none`, everything else to `element`. Before these fixtures
// `nodeType` was a plain string match, so `nodeType: element` on an array — which is exactly
// `wrapped: true` — was accepted and emitted unwrapped XML, while `wrapped: true` was rejected.

#[test]
fn e009_element_node_type_on_an_array_is_rejected_like_wrapped() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: element }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w006_element_node_type_on_an_array_never_serialized_as_xml_warns() {
    // Same declaration on a type that never reaches the wire as XML genuinely has no effect.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: element }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn none_node_type_on_an_array_is_the_default_and_generates() {
    // `none` is the 3.2 default for an array, so restating it is a no-op and takes no
    // disposition. This was previously rejected outright.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                  xml: { nodeType: none }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(!has_code(&report, Code::XmlHintIgnored), "{report:#?}");
    }
}

#[test]
fn e009_none_node_type_on_a_scalar_property_is_rejected() {
    // On a scalar the default is `element`, so `none` deletes a node from the wire.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/xml:
            schema:
              type: object
              properties:
                name:
                  type: string
                  xml: { nodeType: none }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

// --- additionalOperations method tokens -----------------------------------------------------
//
// The official document schema pins these keys to an RFC 9110 token and forbids restating a fixed
// field, but it validates the *root* document only. A Path Item reached by `$ref` into another
// file never meets it, so both fixtures below route through a sub-file — the path that was
// previously unguarded, and on which a non-token key reached codegen and was emitted as
// `Method::from_bytes(..).expect(..)`, panicking inside the consumer's client at request time.

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
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out));
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
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
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out));
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

// --- check/generate parity ----------------------------------------------------------------------
//
// The module header calls parity a contract, and `spargen check` is sold as telling you what
// `generate` would do. It was asserted by seven fixtures in the whole file, each written by hand;
// the other 175 go through `generate` alone. A `check` that quietly stopped running one frontend
// stage would keep passing.

/// The sorted diagnostic codes a report carries, duplicates kept: a stage that fires the same
/// warning twice differs from one that fires it once.
fn codes(report: &Report) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    codes.sort_unstable();
    codes
}

/// `check` must reach the same verdict as `generate`: it accepts exactly what `generate` accepts,
/// and reports exactly the same diagnostics. The two outcomes are deliberately *not* compared for
/// equality — a successful `check` is `Clean` and a successful `generate` is `Generated`, because
/// only one of them wrote a module. What must agree is the accept/reject decision.
///
/// `check` skips codegen and emit, so any diagnostic the two disagree on is one that only a
/// generating run reports — which is exactly what would stop `check` standing in for it.
fn assert_parity(name: &str, spec: &str) {
    let generated = generate(spec);
    let checked = check(spec);

    assert_eq!(
        checked.outcome() == Outcome::Rejected,
        generated.outcome() == Outcome::Rejected,
        "`{name}`: check says {:?} but generate says {:?}",
        checked.outcome(),
        generated.outcome()
    );
    assert_eq!(
        codes(&checked),
        codes(&generated),
        "`{name}`: check and generate report different diagnostics"
    );

    // A fixture whose name begins with a code must actually report it. Without this the parity
    // suite is satisfied by both entry points being equally wrong: the `E004 unresolvable ref`
    // fixture reported `clean` for as long as the bug it was named for existed, and passed, because
    // parity compares the two reports to each other and the span test only *counts* verdicts.
    if let Some(labelled) = parity_label(name) {
        assert!(
            codes(&checked).contains(&labelled),
            "`{name}`: the fixture is named for {labelled} but reports {:?}",
            codes(&checked)
        );
    }
}

/// The `E###`/`W###` code a [`PARITY_FIXTURES`] name is labelled with, if it is labelled at all.
/// Shared by [`assert_parity`], which holds a labelled fixture to its label, and by
/// [`every_parity_fixture_that_reports_is_labelled`], which stops the label convention from
/// quietly becoming optional.
fn parity_label(name: &str) -> Option<&str> {
    name.split_whitespace().next().filter(|token| {
        token.len() == 4
            && matches!(token.as_bytes()[0], b'E' | b'W')
            && token[1..].bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// One spec per diagnostic family the frontend can reach, plus a clean one. Rejections and warnings
/// both matter: a rejection proves `check` runs the stage that refuses, a warning proves it runs
/// the stage that merely notices.
const PARITY_FIXTURES: &[(&str, &str)] = &[
    (
        "clean",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths:\n  /a:\n    get:\n      operationId: getA\n      responses: { '204': { description: ok } }\n",
    ),
    (
        "E001 unsupported version",
        "openapi: 3.0.3\ninfo: { title: T, version: 1.0.0 }\npaths: {}\n",
    ),
    ("E011 structurally invalid", "openapi: 3.1.0\npaths: {}\n"),
    (
        "E004 unresolvable ref",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: { $ref: '#/components/schemas/Missing' }\n",
    ),
    (
        "E012 unknown security scheme",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n  /a:\n    get:\n      operationId: getA\n      security: [{ nope: [] }]\n      responses: { '204': { description: ok } }\n",
    ),
    ("E013 irreconcilable allOf", ALL_OF_CONFLICT_SPEC),
    // A nullable alias that generates cleanly. The suite's clean cases are all trivial documents;
    // this one drives the lowering path this branch reworked, where `check` and `generate` take the
    // same code and could silently stop agreeing.
    ("nullable alias carries its target", NULLABLE_ALIAS_CARRY_SPEC),
    ("W005 schema default", W005_SPEC),
    (
        "W001 validation-only keyword",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    S: { type: string, minLength: 3 }\n",
    ),
    (
        "W002 server-initiated flow",
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\nwebhooks:\n  ping:\n    post:\n      operationId: ping\n      responses: { '204': { description: ok } }\n",
    ),
];

#[test]
fn check_and_generate_agree_on_every_parity_fixture() {
    for (name, spec) in PARITY_FIXTURES {
        assert_parity(name, spec);
    }
}

#[test]
fn the_parity_fixtures_span_both_verdicts() {
    // A parity suite made only of clean specs would pass trivially. This keeps it honest: it has to
    // contain rejections, warnings, and specs that succeed.
    let mut rejected = 0;
    let mut warned = 0;
    let mut succeeded = 0;
    for (_, spec) in PARITY_FIXTURES {
        let report = check(spec);
        if report.outcome() == Outcome::Rejected {
            rejected += 1;
        } else {
            succeeded += 1;
        }
        if codes(&report).iter().any(|code| code.starts_with('W')) {
            warned += 1;
        }
    }
    assert!(rejected >= 4, "only {rejected} fixtures reject");
    assert!(warned >= 3, "only {warned} fixtures warn");
    assert!(succeeded >= 2, "only {succeeded} fixtures succeed");
}

/// Write a root document whose only Path Item is a `$ref` to a sibling file holding `path_item`,
/// then run both entry points over it. The indirection is the point: `lower_frontend` validates
/// `bundle.root()` against the metaschema and nothing else, so a Path Item reached by `$ref` never
/// meets it. Returns `(generate, check)` so a fixture can assert the two agree, the way
/// `PARITY_FIXTURES` does for inline specs — which those cannot, being single-file by construction.
fn generate_and_check_refd_path_item(path_item: &str) -> (Report, Report) {
    let (generated, checked, _) = generate_and_check_refd_path_item_with_code(path_item);
    (generated, checked)
}

/// As [`generate_and_check_refd_path_item`], but also returning the emitted module's source (empty
/// if generation wrote nothing), so a fixture can assert that an accepted response key actually
/// reaches the client rather than only that it raised no diagnostic.
fn generate_and_check_refd_path_item_with_code(path_item: &str) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths:\n  /pet:\n    $ref: 'pet.yaml'\n",
    )
    .unwrap();
    std::fs::write(dir.join("pet.yaml"), path_item).unwrap();
    let out = dir.join("client.rs");
    let generated = spargen::generate(&build(dir.join("openapi.yaml"), out.clone()));
    let checked = spargen::check(&Spec::new(dir.join("openapi.yaml")));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    (generated, checked, code)
}

/// A root document whose Responses map carries `entries` verbatim, written inline so the
/// metaschema — which `lower_frontend` runs over `bundle.root()` and nothing else — does see it.
fn inline_spec_with_response_entries(entries: &str) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200': {{ description: ok }}\n{entries}"
    )
}

/// A Responses key the specification does not define, reached through a Path Item `$ref`.
///
/// `references/3.2.0.md` closes the grammar — *"Only the following range definitions are allowed:
/// `1XX`, `2XX`, `3XX`, `4XX`, and `5XX`"* — and the metaschema spells it `^[1-5](?:[0-9]{2}|XX)$`.
/// Before the key was checked at parse time, `0XX` lowered to `StatusSpec::Range(0)`, which is the
/// sentinel `default` itself lowers to: the operation's error enum got **two** `Default` variants
/// and `rustc` refused the emitted module with `E0428` — at outcome `Generated`, with zero
/// diagnostics. That is the fourth, silent behavior the contract forbids, on a construct the
/// specification explicitly closes.
#[test]
fn e011_out_of_grammar_response_key_behind_a_ref_is_rejected() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    '0XX': { description: collides with the default sentinel }\n    default: { description: fallback }\n",
    );
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(has_code(&generated, Code::InvalidInput), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::InvalidInput), "{checked:#?}");
}

/// The other faces of the same defect, each reached through a `$ref` so the metaschema never sees
/// it. `02XX` lowered to the same `StatusSpec` as `2XX`, and `0200`/`+200` to the same one as
/// `200` — a duplication `E022` cannot catch, because these are distinct map keys. `6XX`-`9XX`
/// lowered to a match arm no status can reach. `XX`, `2xx` and `banana` were dropped with no
/// diagnostic at all, handing the caller a client with no arm for a response they wrote down.
///
/// The last three keys are rejected by the grammar *only*: an empty key and a `200` with leading
/// or trailing whitespace parse as neither a range nor an exact status, so before this check they
/// too vanished silently, and nothing anywhere in the tree named them.
#[test]
fn e011_every_out_of_grammar_response_key_is_rejected_behind_a_ref() {
    for key in [
        "0XX", "02XX", "0200", "+200", "6XX", "9XX", "XX", "2xx", "banana", "200.0", "1000", "20X",
        "", " 200", "200 ",
    ] {
        let (generated, checked) = generate_and_check_refd_path_item(&format!(
            "get:\n  operationId: getPet\n  responses:\n    '200': {{ description: ok }}\n    '2XX': {{ description: range }}\n    '{key}': {{ description: out of grammar }}\n"
        ));
        assert_eq!(
            generated.outcome(),
            Outcome::Rejected,
            "`{key}` was accepted: {generated:#?}"
        );
        assert!(
            has_code(&generated, Code::InvalidInput),
            "`{key}`: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Rejected,
            "`{key}`: check disagreed with generate: {checked:#?}"
        );
        assert!(
            has_code(&checked, Code::InvalidInput),
            "`{key}`: {checked:#?}"
        );
    }
}

/// The parity property the parse-time check exists to establish: the *same* out-of-grammar key
/// written inline — where the metaschema does see it — reaches the same verdict under the same
/// code. Should the vendored metaschema's pattern and the hand-written grammar ever diverge, this
/// fixture and the one above stop agreeing, which is the only signal that divergence would give.
#[test]
fn an_out_of_grammar_response_key_rejects_identically_inline_and_behind_a_ref() {
    let inline = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200': { description: ok }\n        '0XX': { description: out of grammar }\n        default: { description: fallback }\n";
    let (refd_generated, refd_checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    '0XX': { description: out of grammar }\n    default: { description: fallback }\n",
    );
    for report in [
        &generate(inline),
        &check(inline),
        &refd_generated,
        &refd_checked,
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// The keys the metaschema *does* admit keep working behind a `$ref`: both response-key shapes it
/// allows, the `default` sentinel, and a specification extension, which `specification-extensions`
/// admits under `^x-` and which is therefore skipped before the grammar is applied. Without this
/// the grammar check could over-reject with every other suite still green.
///
/// The verdict is asserted exactly rather than as "not rejected", and the diagnostics are asserted
/// empty: a change that started *warning* on in-grammar keys would otherwise pass here. And each
/// accepted key is asserted to reach the emitted module, so "no diagnostic" cannot be satisfied by
/// dropping the entry — which is the very failure mode this change exists to close.
#[test]
fn in_grammar_response_keys_and_extensions_still_generate_behind_a_ref() {
    // Every response carries a body, so the emitted client has to build a status→variant table and
    // the accepted keys become observable selectors in it rather than only doc comments.
    let body = "content: { application/json: { schema: { type: string } } }";
    let (generated, checked, code) = generate_and_check_refd_path_item_with_code(&format!(
        "get:\n  operationId: getPet\n  responses:\n    '200': {{ description: ok, {body} }}\n    '2XX': {{ description: range, {body} }}\n    '404': {{ description: gone, {body} }}\n    '5XX': {{ description: server, {body} }}\n    default: {{ description: fallback, {body} }}\n    x-internal-note: {{ description: a specification extension, not a response }}\n"
    ));
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert!(generated.diagnostics().is_empty(), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
    assert!(checked.diagnostics().is_empty(), "{checked:#?}");
    for selector in [
        "StatusSpec::Exact(200u16)",
        "StatusSpec::Range(2u8)",
        "StatusSpec::Exact(404u16)",
        "StatusSpec::Range(5u8)",
    ] {
        assert!(
            code.contains(selector),
            "accepted key's selector `{selector}` never reached the emitted client:\n{code}"
        );
    }
    // The skipped extension contributes nothing to the emitted client — not even the
    // `Response `x-…`: <description>` doc line an object-valued extension produced before the
    // skip, when it was parsed as a Response Object. It is not a response, so it is not
    // documented as one.
    for fragment in [
        "x-internal-note",
        "a specification extension, not a response",
    ] {
        assert!(
            !code.contains(fragment),
            "skipped extension leaked `{fragment}` into the emitted client:\n{code}"
        );
    }
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
/// specification extension: `unevaluatedProperties: false` rejects it inline, and behind a `$ref`,
/// where the metaschema never looks, the grammar check is what rejects it. Both under `E011`, so
/// the verdict does not depend on placement.
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

/// The asymmetry the skip leaves behind, pinned so it is visible rather than merely true: a
/// dangling **file** ref inside an extension still rejects, because the bundle loader resolves and
/// reads `$ref` targets *before* anything parses a Responses key, so the skip never gets a say.
/// A dangling **pointer** ref in the same position does not (the fixture above). Two kinds of
/// dangling reference inside arbitrary user data now reach different verdicts.
///
/// This is bundle-loader behavior, outside this change's reach — filed as #239 rather than fixed.
/// Should the loader stop eagerly reading refs it has no reason to interpret, this fixture is the
/// one that says so.
#[test]
fn a_dangling_file_ref_inside_an_extension_still_rejects_unlike_a_pointer_ref() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    x-note: { $ref: 'nowhere.yaml' }\n",
    );
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    }
}

/// The silent-drop face on its own. `XX` parses as neither a range nor an exact status, so
/// `parse_status` returned `None` and the entry simply vanished: outcome `Generated`, no
/// diagnostic, and a client missing an arm for a response the author had documented. It reports.
#[test]
fn an_unparseable_response_key_behind_a_ref_reports_rather_than_vanishing() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    'XX': { description: silently dropped }\n",
    );
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(has_code(&generated, Code::InvalidInput), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::InvalidInput), "{checked:#?}");
}

/// A union whose own null acceptance came from a stripped `{type: "null"}` member or a `"null"` in
/// the enclosing `type` array is still satisfied by `null`, and must not be rejected as having no
/// variant. `lower_union` folds that acceptance into a local *before* it intersects, so the member
/// it hands `intersect_types` carries `nullable: false` and `type_accepts_null` — which reads
/// `Ty::nullable` — cannot see it. Both collapse paths had the blind spot: the sole-real-member
/// site, and the pre-existing every-variant-excluded site below it.
///
/// The `$ref` spelling of the identical instance set already emits `()`
/// (`the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate` pins it), so two
/// spellings of one schema disagreed. An independent Draft 2020-12 validator says `null` is the one
/// value each of the rescued documents admits, and that each control admits nothing at all.
#[test]
fn a_union_that_null_still_satisfies_is_not_rejected_as_having_no_variant() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // Sole real member: `null` satisfies the enclosing `type` array AND the null-only member,
        // so `null` is the only value satisfying the whole schema.
        (
            "a sole-member `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        (
            "a sole-member `anyOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                anyOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // Every real variant excluded: the same blind spot on the multi-variant path.
        (
            "an every-variant-excluded `oneOf` whose enclosing type array admits null",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        // The controls that must keep rejecting: the union admits null but the sibling refuses it,
        // so nothing satisfies both and the rejection is right.
        (
            "a sole-member union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an every-variant-excluded union whose sibling refuses null",
            "type: integer\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            false,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            if *null_satisfies {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` is satisfied by `null`, so {entry} must not reject it: {report:#?}"
                );
                assert!(
                    !has_code(&report, Code::NonDisjointUnion),
                    "`{what}` reported E007 through {entry}: {report:#?}"
                );
            } else {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` admits no value at all, so {entry} must still reject it: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::NonDisjointUnion),
                    "`{what}` did not report E007 through {entry}: {report:#?}"
                );
            }
        }
        if *null_satisfies {
            // The exact JSON null type, not a silently dropped body and not `Option<()>`: `null` is
            // the only satisfying value, so the response type has exactly one inhabitant. Followed
            // through the operation's own signature rather than by guessing an alias name — the
            // sole-member path lowers its member under the same hint, so the union's own def gets a
            // disambiguating suffix.
            let (_, code) = generate_with_code(&spec);
            let body = code
                .split("ResponseValue<types::")
                .nth(1)
                .and_then(|rest| rest.split('>').next())
                .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
                .trim()
                .to_owned();
            assert!(
                code.contains(&format!("pub type {body} = ();")),
                "`{what}` must lower its response body to the exact JSON null type, but \
                 `{body}` is not `()`: {code}"
            );
        }
    }

    // The other half of restoring the union's null acceptance ONTO the member: nullability now
    // flows THROUGH the intersection instead of around it, so the sibling gets a say in it. It
    // previously did not, and the union's acceptance was OR-ed back on afterwards regardless —
    // `{type: [string], oneOf: [{type: string}, {type: 'null'}]}` emitted `Option<String>` for a
    // schema the enclosing `type` array forbids `null` in. An independent Draft 2020-12 validator
    // rejects `null` for the first two rows and accepts it for the third.
    //
    // (what it exercises, the schema body, whether the generated response is optional)
    let nullability: &[(&str, &str, bool)] = &[
        (
            "a null member under a sibling `type` array that excludes null",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a nullable sole member under a sibling that excludes null",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        (
            "a null member under a sibling `type` array that admits null",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];
    for (what, body, optional) in nullability {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body: {code}",
            if *optional { "have" } else { "not have" }
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

/// A property whose two sides cannot be intersected does not make the composition empty unless
/// some instance is obliged to carry it. When the property is optional on BOTH sides, `{}` and
/// `{"zz": 1}` still satisfy the whole schema — an independent Draft 2020-12 validator confirms
/// both — so rejecting the document deletes a body that has valid instances.
///
/// This is `E013`'s own published doctrine, which the array arm of `intersect_non_null` already
/// implements: "an empty array-item intersection becomes an uninhabited item type so the valid
/// empty array remains representable". `intersect_structs` propagated the failure instead. It now
/// mirrors the array arm: the field takes an uninhabited type, so the instances that remain are
/// exactly the ones that omit it.
///
/// Requiring the property on either side is the real empty composition, and both controls must
/// keep rejecting — the validator says nothing satisfies them.
#[test]
fn a_property_conflict_on_an_optional_property_does_not_empty_the_object() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // Both sites that reach `intersect_structs` with a `$ref`-sibling composition: the `$ref` arm
    // of `lower_schema_inner`, and the sole-real-member union collapse.
    let inhabited: &[(&str, &str, &str)] = &[
        (
            "a `$ref` sibling conflicting on a property neither side requires",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "a sole-member union conflicting on a property neither side requires",
            "type: object\n                properties: { a: { type: integer } }\n                oneOf: [{ type: object, properties: { a: { type: string } } }]",
            "components:\n  schemas:\n    Unused: { type: string }\n",
        ),
    ];
    for (what, body, components) in inhabited {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` still admits `{{}}`, so {entry} must not reject it: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable)
                    && !has_code(&report, Code::NonDisjointUnion),
                "`{what}` reported an irreconcilable composition through {entry}: {report:#?}"
            );
        }
        // Not silently widened to something that accepts `{"a": 1}`: the field itself is
        // uninhabited, so only instances omitting the property can be built or decoded.
        let (_, code) = generate_with_code(&spec);
        assert!(
            code.contains("no JSON value can inhabit schema"),
            "`{what}` must give the conflicting property an uninhabited type: {code}"
        );
        assert!(!code.contains("serde_json :: Value"), "{code}");
    }

    // The controls. Requiring the property on either side obliges every instance to carry a value
    // no type admits, so the composition really is empty and the rejection is right.
    let empty: &[(&str, &str, &str)] = &[
        (
            "the sibling requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                required: [a]\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "the target requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, required: [a], properties: { a: { type: string } } }\n",
        ),
    ];
    for (what, body, components) in empty {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` admits no value at all, so {entry} must still reject it: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "`{what}` did not report E013 through {entry}: {report:#?}"
            );
        }
    }
}

/// A null-only union MEMBER and a `"null"` in the enclosing `type` array are not the same fact, and
/// only one of them can rescue an empty intersection.
///
/// A member supplies a branch that `null` validates against. A `"null"` in the enclosing `type`
/// array only *permits* null — `oneOf` still demands exactly one matching member and `anyOf` at
/// least one, so with no null member there is nothing for `null` to match and the schema admits
/// nothing at all. Folding both into one flag made those two cases indistinguishable: an
/// independent Draft 2020-12 validator says the first row below is satisfied by `null` and the
/// second by **nothing**, and they produced byte-identical output — the tool could no longer tell
/// "accepts only null" from "accepts nothing", and typed a body `()` that no real payload decodes
/// into, with `check` reporting clean.
#[test]
fn only_a_null_member_can_rescue_an_empty_union_intersection() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // A null MEMBER: `null` matches it, and the enclosing `type` array permits null, so `null`
        // satisfies the whole schema.
        (
            "a sole-member `oneOf` with a null member beside a nullable type array",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // The SAME enclosing type array with NO null member. Nothing satisfies this: `null` matches
        // no member, and anything matching the `string` member fails the `type` array.
        (
            "a sole-member `oneOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }]",
            false,
        ),
        (
            "an `anyOf` whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                anyOf: [{ type: string }]",
            false,
        ),
        // The same distinction on the every-variant-excluded path.
        (
            "an every-variant-excluded union with a null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }, { type: 'null' }]",
            true,
        ),
        (
            "an every-variant-excluded union whose nullable type array supplies no null member",
            "type: [integer, 'null']\n                oneOf: [{ type: string }, { type: boolean }]",
            false,
        ),
    ];

    let mut emitted: Vec<(&str, String)> = Vec::new();
    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            if *null_satisfies {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` is satisfied by `null`, so {entry} must not reject it: {report:#?}"
                );
            } else {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "`{what}` admits no value at all, so {entry} must reject it rather than type \
                     the body `()`: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::NonDisjointUnion),
                    "`{what}` did not report E007 through {entry}: {report:#?}"
                );
            }
        }
        let (_, code) = generate_with_code(&spec);
        emitted.push((what, code));
    }

    // What each generating row lowers TO, asserted positively.
    //
    // This replaces a differential that could not fail on the defect it named. It compared row 0
    // (two members, lowers to `()`) against a one-member probe (lowers to `i64`) and required them
    // to differ — but two documents of different member counts and different result types differ in
    // every reachable state, independent of the null fact, so the `assert_ne!` was true whatever
    // the production code did. Measured directly: under the re-merged-flag mutation the fixture
    // fails at the per-case outcome assertion above, and with those assertions neutralised the
    // fixture PASSED. It was held up entirely by its neighbours.
    //
    // Among the rows above, varying only the null fact flips a row between generating and
    // rejecting, so no pair of THEM is a differential. One that generates on both sides does
    // exist, and is asserted after the control below. Here the pin is positive: a row only `null`
    // satisfies must lower to the exact JSON null
    // type — not to `serde_json::Value`, not to `Option<T>` of anything — and that is a statement a
    // mutation can falsify without changing any outcome, which is what the removed assertion could
    // not manage.
    for (what, code) in &emitted {
        let module = types_module(code);
        if module.is_empty() {
            // A rejected row emits nothing; its verdict is asserted per case above.
            continue;
        }
        assert!(
            module.contains("= ();"),
            "`{what}` is satisfied by `null` and nothing else, so it must lower to the exact JSON \
             null type: {module}"
        );
    }
    assert!(
        !types_module(&emitted[0].1).is_empty(),
        "the null-only row generated nothing: {:?}",
        emitted[0].0
    );

    // The control the whole repair must not disturb: a non-empty intersection under the same
    // nullable type array is unaffected either way.
    let control = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }]"
        )
    );
    let (report, code) = generate_with_code(&control);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("= i64;"),
        "the non-empty intersection must still lower to its narrowed type: {code}"
    );

    // The differential that varies ONLY the null fact and generates on both sides: the control,
    // and the control with a `{type: 'null'}` member added. The member is what makes `null` a
    // valid body, so it — and only it — must make the response optional. The null fact lives on
    // the operation signature, not in the types module: `types_module` is nullability-blind here
    // (both documents emit the same `= i64;` alias), which is why this reads the signature.
    let with_null_member = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, 'null']\n                oneOf: [{ type: integer }, { type: 'null' }]"
        )
    );
    let (partner_report, partner) = generate_with_code(&with_null_member);
    assert_ne!(
        partner_report.outcome(),
        Outcome::Rejected,
        "{partner_report:#?}"
    );
    assert!(
        !code.contains("ResponseValue<Option<"),
        "without a null member `null` matches no branch, so the response is not optional: {code}"
    );
    assert!(partner.contains("= i64;"), "{partner}");
    assert!(
        partner.contains("ResponseValue<Option<"),
        "the null member is a branch `null` satisfies, so the response must be optional: {partner}"
    );
}

/// A union sibling gets a say in the union's nullability only where it makes a statement about
/// null, and a sibling that carries no `type` makes none.
///
/// `properties` and `patternProperties` are OBJECT APPLICATORS in 2020-12: they constrain an
/// object and are vacuously satisfied by every non-object, `null` included. They nonetheless lower
/// to a non-nullable `Struct`, so reading `Ty::nullable` off one and letting it decide removed an
/// acceptance the sibling never denied. An independent Draft 2020-12 validator says `null` is valid
/// for all three rows below; they went non-optional, so a `200` of literal `null` that used to
/// decode began failing at runtime with nothing reported at generate time.
///
/// The controls matter as much as the rows: a sibling that DOES carry a `type` is entitled to
/// remove the acceptance, and must keep doing so.
#[test]
fn a_union_sibling_without_a_type_does_not_decide_nullability() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether the response must be optional)
    let cases: &[(&str, &str, bool)] = &[
        // No `type` on the sibling: it says nothing about null, so the union's own acceptance wins.
        (
            "a `properties`-only sibling beside a null member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling beside a null member",
            "patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `properties`-only sibling beside a nullable sole member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: [object, 'null'] }]",
            true,
        ),
        (
            "a `required`-only sibling beside a null member",
            "required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "an `additionalProperties`-only sibling beside a null member",
            "additionalProperties: false\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling carries a `type` that excludes null, so it IS entitled to remove the
        // acceptance — this is the case the round-2 change correctly fixed and must keep fixing.
        (
            "a sibling whose `type` excludes null, beside a null member",
            "type: object\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` array excludes null, beside a null member",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` excludes null, beside a nullable sole member",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        // And a `type` that ADMITS null must not remove it either.
        (
            "a sibling whose `type` array admits null, beside a null member",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, optional) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *optional { "have" } else { "not have" },
            if *optional { "valid" } else { "invalid" }
        );
    }
}

/// Accept-versus-reject must not key on `components.schemas` map order.
///
/// The round-2 guard tested `in_progress` membership, which is a property of *when* lowering
/// happens: `lower.rs` pre-lowers components in map iteration order, so for mutual recursion it
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
        assert!(
            !code.contains("serde_json :: Value"),
            "`{what}` degraded to an untyped value: {code}"
        );
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

/// The question is "does the SIBLING make a statement about `null`?", and the gate asked the
/// ENCLOSING schema's `type`. The two differ in two ways, and each produces a wrong answer in the
/// opposite direction.
///
/// `lower_union_sibling` deletes the enclosing `type` array whenever it holds more than one non-null
/// type, because no single lowered type represents it. That deletion also throws away the array's
/// `"null"`, so the lowered sibling reads as null-rejecting even where the array admitted null —
/// **an under-accept that is a regression against master**, whose compiled client fails on a
/// spec-legal `null` with `invalid type: null, expected struct …`. In the other direction, a sibling
/// that speaks about null through `enum` or `const` rather than `type` was treated as silent, so the
/// union's acceptance survived a sibling that denied it.
///
/// Both oracles — Python `jsonschema` 4.26 and the Rust crate 0.49.3 — agree on every row.
#[test]
fn the_nullability_gate_asks_the_sibling_not_the_enclosing_schema() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // The sibling carries a WIDE type array that admits null. Lowering deletes the array, so
        // before this the sibling read as null-rejecting and the response went non-optional.
        (
            "a `properties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `required`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The narrow spelling of the first row: one non-null type, so the array survives lowering.
        // It was already right, and the wide spelling must now agree with it.
        (
            "the same sibling under a narrow nullable type array",
            "type: [object, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling speaks about null through `enum`/`const`, not `type`. It denies null, and the
        // gate must let it.
        (
            "an `enum` sibling that excludes null",
            "enum: ['a', 'b']\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a `const` sibling",
            "const: 'a'\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an `allOf` sibling that excludes null",
            "allOf: [{ type: object }]\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        // An `enum` that lists null admits it.
        (
            "an `enum` sibling that includes null",
            "enum: ['a', null]\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *null_satisfies,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *null_satisfies { "have" } else { "not have" },
            if *null_satisfies { "valid" } else { "invalid" }
        );
    }

    // The third symptom of the same root, and the sharpest: a document whose ONLY valid instance is
    // `null` was REJECTED, with a message asserting an empty intersection when the intersection is
    // `{null}`. The one-non-null-type spelling of the same instance set already generated `()`.
    let only_null = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, boolean, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: string }, { type: 'null' }]"
        )
    );
    for (entry, report) in [
        ("generate", generate(&only_null)),
        ("check", check(&only_null)),
    ] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "a schema whose only valid instance is `null` must not be rejected as having no \
             variant by {entry}: {report:#?}"
        );
        assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
    let (_, code) = generate_with_code(&only_null);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        code.contains(&format!("pub type {body} = ();")),
        "the intersection is `{{null}}`, so the exact JSON null type is the answer, not a \
         dropped body and not `Option<()>`: {code}"
    );
}

/// The cycle predicate must count only the edges lowering actually follows.
///
/// `collect_schema_refs` chained `schema.defs` and `schema.validation_children`, so `$defs`, `not`,
/// `if`/`then`/`else`, `contains`, `propertyNames`, `unevaluated*` and `dependentSchemas` were all
/// treated as cycle edges. **Lowering never descends into any of them** — `.defs` and
/// `validation_children` appear exactly once each in the whole of `lower.rs`, inside that walk — so
/// a `$ref` reachable only that way can never put a component mid-flight and can never yield a
/// placeholder. The guard rejected anyway, asserting a dependence that does not exist.
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

/// A schema must lower to the same nullability whether it is written inline or named as a
/// component. `ensure_component` computed `schema_is_nullable(schema)` *before* the body was
/// lowered and wrote it back over whatever the body computed — and `schema_is_nullable` is three
/// disjuncts over `types`, `enum_values` and `const_value` that never look at `oneOf`, `anyOf`,
/// `$ref` or `allOf`. So **every decision `lower_union` makes about null was discarded at a
/// `components.schemas` boundary**, which is the dominant spelling in real descriptions.
///
/// The comment there claimed nullability was "a pure function of the component's own schema — the
/// same inputs `lower_schema`/`lower_enum` use". That is true for a plain type/enum/const body and
/// false for every composed one.
///
/// Both oracles agree on each row, and each row is asserted in BOTH spellings, so neither can drift
/// from the other again.
#[test]
fn a_component_and_an_inline_schema_agree_about_null() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";

    fn inline_spec(body: &str) -> String {
        format!(
            "{HEAD}paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                {body}\ncomponents:\n  schemas:\n    Ignore: {{ type: string }}\n",
            body = body.replace('\n', "\n                ")
        )
    }
    fn named_spec(body: &str) -> String {
        format!(
            "{HEAD}paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{ $ref: '#/components/schemas/Body' }}\ncomponents:\n  schemas:\n    Body:\n      {body}\n",
            body = body.replace('\n', "\n      ")
        )
    }

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // `schema_is_nullable` sees the `"null"` in the type array and says nullable. The union says
        // otherwise, and the union is right: with no null MEMBER there is no branch for `null` to
        // match, so `oneOf` fails.
        (
            "a union whose type array admits null but whose members supply no null branch",
            "type: [string, 'null']\noneOf: [{ type: string }]",
            false,
        ),
        // The mirror: the type array is silent about null, the members are not.
        (
            "a union whose null branch comes from a member, under no type array",
            "properties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a union under a wide nullable type array",
            "type: [object, array, 'null']\nproperties: { a: { type: string } }\noneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling denies null through `enum`, which `schema_is_nullable` also reads — but it
        // reads the ENUM, not the intersection, so it must still agree.
        (
            "a union whose `enum` sibling excludes null",
            "enum: ['a', 'b']\noneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a union whose `enum` sibling includes null",
            "enum: ['a', null]\noneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
        // A plain body, where the reserve-time answer and the body's agree. This is the case the
        // old comment described, and it must not move.
        (
            "a plain nullable type array with no composition",
            "type: [string, 'null']",
            true,
        ),
        (
            "a plain non-nullable type",
            "type: string",
            false,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let mut seen = Vec::new();
        for (spelling, spec) in [
            ("inline", inline_spec(body)),
            ("named component", named_spec(body)),
        ] {
            let (report, code) = generate_with_code(&spec);
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` ({spelling}): {report:#?}"
            );
            let optional = code.contains("ResponseValue<Option<types::");
            assert_eq!(
                optional,
                *null_satisfies,
                "`{what}` as {spelling} must {} an optional response body — `null` is {} under \
                 this schema: {code}",
                if *null_satisfies { "have" } else { "not have" },
                if *null_satisfies { "valid" } else { "invalid" }
            );
            seen.push((spelling, optional));
        }
        assert_eq!(
            seen[0].1, seen[1].1,
            "`{what}` lowers to different nullability inline and as a component: {seen:?}"
        );
    }

    // A component whose union admits ONLY `null` must be the exact null type in both spellings too
    // — the component boundary previously wrapped it back into an `Option`.
    let only_null = "type: [integer, 'null']\noneOf: [{ type: string }, { type: 'null' }]";
    for (spelling, spec) in [
        ("inline", inline_spec(only_null)),
        ("named component", named_spec(only_null)),
    ] {
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !code.contains("ResponseValue<Option<types::"),
            "`{spelling}`: the intersection is `{{null}}`, so the type already has exactly one \
             inhabitant and must not be wrapped in `Option`: {code}"
        );
    }
}

/// The label check in [`assert_parity`] is opt-in by naming convention, so on its own it can be
/// disarmed rather than satisfied: renaming `"E004 unresolvable ref"` to `"unresolvable ref"`, or
/// widening [`parity_label`] until it matches nothing, makes the whole suite pass again — the same
/// silent escape the label check was added to close.
///
/// This requires the convention instead of hoping for it: a fixture that reports any diagnostic at
/// all must be named for one. Only the deliberately clean fixture reports nothing, and it is the
/// only one allowed to go unlabelled.
#[test]
fn every_parity_fixture_that_reports_is_labelled() {
    let mut labelled = 0;
    for (name, spec) in PARITY_FIXTURES {
        let report = check(spec);
        match parity_label(name) {
            Some(_) => labelled += 1,
            None => assert!(
                report.diagnostics().is_empty(),
                "`{name}` reports {:?} but is not named for a code",
                codes(&report)
            ),
        }
    }
    // Belt and braces: if `parity_label` itself stopped matching, every fixture would fall into the
    // arm above and this floor is what notices.
    assert!(
        labelled >= 7,
        "only {labelled} parity fixtures are labelled; the convention has been disarmed"
    );
}
