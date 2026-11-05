//! Per-diagnostic frontend coverage: one minimal inline spec per rejection/warning code, asserting
//! the code fires and the pipeline outcome is what the taxonomy promises. Rejections travel through
//! `generate`. Check/generate parity is a property of the harness rather than a remark about one
//! case: every description `generate`, `generate_with_code` or a multi-file helper runs is also run
//! through `check`, which must reach the same accept/reject verdict and report the identical
//! diagnostics, code, pointer and message alike (`oracles::parity_violations`). `PARITY_FIXTURES`
//! adds a named set a companion test keeps spanning rejections, warnings and clean runs.
//!
//! Every run any fixture makes goes through `run_generate` or `run_check`, which hold each
//! diagnostic's declared `OutcomeClaim` to the run's own outcome (#413), and hold the run to
//! `oracles::run_violations`: no IR invariant or codegen failure reported as an `E011`, every
//! diagnostic at a real location (#454), and a `Generated` module that parses as Rust, whose
//! unions' variants a value can tell apart (#402) and that carries no unused second copy of a type
//! (#926). Those oracles live in `oracles/`, shared with `fuzz_frontend.rs` and
//! `lowering_props.rs`.
//!
//! This file holds that shared harness; the fixtures sit in one module per construct family or
//! sub-family beside it (`unions_dispatch`, `ref_sibling_unions`, `bodies_streaming`, ...), each
//! reaching the harness through `use super::*`.

use camino::Utf8PathBuf;
use spargen::{
    Build, CargoIntegration, Code, Diagnostic, Outcome, OutcomeClaim, Report, Severity, Spec,
};

#[path = "../oracles/mod.rs"]
mod oracles;

use oracles::readers::{
    alias_target, codes, declared_fields, declared_types, enum_variants, field_owner, field_type,
    has_code, messages_for,
};
use oracles::types_module;

mod all_of_arrays;
mod all_of_meet_conflicts;
mod all_of_merge;
mod all_of_nullability;
mod all_of_union_meet;
mod all_of_union_spellings;
mod audit_reach;
mod bodies_binary;
mod bodies_media_keys;
mod bodies_streaming;
mod defaults;
mod diagnostics_meta;
mod discriminator;
mod discriminator_pinned_tags;
mod document;
mod encoding;
mod encoding_headers;
mod encoding_style;
mod open_narrowing;
mod parameters;
mod pattern_properties;
mod placement;
mod recursion_aliases;
mod recursion_cycles;
mod recursion_reservations;
mod ref_sibling_categories;
mod ref_sibling_rejections;
mod ref_sibling_unions;
mod refs;
mod refs_bundle_identity;
mod refs_remote;
mod refs_sub_files;
mod requirements;
mod responses;
mod schema_keywords;
mod schemas;
mod security;
mod servers;
mod union_categories;
mod union_nullability;
mod unions_collapse;
mod unions_dispatch;
mod unions_null_branches;
mod unions_siblings;
mod untyped_branch_nullability;
mod xml;
mod xml_members;
mod xml_names;
mod xml_ref_elements;

/// A build for a fixture spec. These tests are not build scripts, so the Cargo integration is
/// explicitly off: no rebuild triggers to emit, no consumer manifest to audit, and — the reason it
/// matters here — no `W013` polluting the diagnostics a fixture is asserting on.
fn build(spec: Utf8PathBuf, out: Utf8PathBuf) -> Build {
    Spec::new(spec).build(out).cargo(CargoIntegration::Off)
}

/// `spargen::generate`, then [`hold_run`] on the report and on the module a `Generated` run wrote.
/// Every fixture here reaches `generate` through this function, so every run any fixture makes is
/// held to every oracle.
fn run_generate(build: &Build) -> Report {
    let report = spargen::generate(build);
    let module = (report.outcome() == Outcome::Generated)
        .then(|| std::fs::read_to_string(build.output()).unwrap_or_default());
    hold_run(&report, build.spec(), module.as_deref());
    report
}

/// `spargen::check`, then [`hold_run`] on the report, as [`run_generate`].
fn run_check(spec: &Spec) -> Report {
    let report = spargen::check(spec);
    hold_run(&report, spec, None);
    report
}

/// Fail unless every diagnostic in `report`, a run over `spec`, makes a claim its run's outcome
/// admits ([`claim_violations`], #413), and the shared oracles find no defect in the run or in
/// `module`, the module it wrote ([`oracles::run_violations`]). The root document is read back from
/// `spec`'s path, so a span can be compared with the file it lies in.
fn hold_run(report: &Report, spec: &Spec, module: Option<&str>) {
    let mut violations = claim_violations(report.outcome(), report.diagnostics());
    let root = std::fs::read(spec.path()).unwrap_or_default();
    violations.extend(oracles::run_violations(report, &root, module));
    assert!(
        violations.is_empty(),
        "a `{}` run broke an oracle every run is held to: {violations:#?}\n{report:#?}",
        report.outcome()
    );
}

/// Each diagnostic in `diagnostics` that says something false about a run whose outcome is
/// `outcome`, with the reason.
///
/// A message is composed where it is emitted, before the outcome is known, and `check` and
/// `generate` emit the same diagnostics. A message that asserted an outcome was false on every run
/// that ended differently, such as `W014`'s old "is generated" on a `check` run (#174). Two things
/// are checked. The declared [`OutcomeClaim`] must be one `outcome` admits. And a message that
/// states an outcome in so many words ([`stated_claim`]) must declare that claim, so the first
/// check reads what the message says.
fn claim_violations(outcome: Outcome, diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let claim = diagnostic.claim;
            let stated = stated_claim(&diagnostic.message);
            if !outcome.admits(claim) {
                Some(format!(
                    "{claim:?} is false of a `{outcome}` run: {diagnostic:?}"
                ))
            } else if stated.is_some_and(|stated| stated != claim) {
                Some(format!(
                    "the message states {stated:?} but the claim is {claim:?}: {diagnostic:?}"
                ))
            } else {
                None
            }
        })
        .collect()
}

/// The outcome a message states in so many words, if it states one: an unnegated "is generated"
/// or "is rejected" (or "are", "be", "been").
///
/// This reads prose, so it is a backstop and not the check. [`claim_violations`] is the check,
/// and it trusts the declared [`OutcomeClaim`]. This catches the case where the two disagree: a
/// message that asserts an outcome while its diagnostic declares a different claim. That is the
/// shape of `W014`'s old "`{media}` is generated", whose claim was never declared (#174). A
/// rephrasing it does not recognise ("gets emitted") gets past it. Backticked spans are dropped
/// first, so a quoted name cannot supply the predicate or the negation.
fn stated_claim(message: &str) -> Option<OutcomeClaim> {
    let prose: String = message
        .split('`')
        .step_by(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    prose.split([';', ':', ',', '.']).find_map(|clause| {
        let words: Vec<&str> = clause.split_whitespace().collect();
        words.windows(2).enumerate().find_map(|(at, pair)| {
            let claim = match pair {
                ["is" | "are" | "be" | "been", "generated"] => OutcomeClaim::Generated,
                ["is" | "are" | "be" | "been", "rejected"] => OutcomeClaim::Rejected,
                _ => return None,
            };
            let negated = words[..at]
                .iter()
                .any(|word| matches!(*word, "no" | "not" | "never" | "nothing" | "none"));
            (!negated).then_some(claim)
        })
    })
}

/// `generate` on an inline spec, holding the run to parity with `check` over the same document
/// ([`assert_check_agrees`]) on top of the oracles [`run_generate`] holds every run to.
fn generate(spec: &str) -> Report {
    generate_with_code(spec).0
}

/// As [`generate`], but through the `check` entry point (no codegen/emit).
fn check(spec: &str) -> Report {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, spec).unwrap();
    run_check(&Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap()))
}

/// As [`generate`], also returning the emitted module (empty when nothing was written).
fn generate_with_code(spec: &str) -> (Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, spec).unwrap();
    generate_spec_with_code(Spec::new(spec_path))
}

/// As [`generate_with_code`], over the description already written at `spec`'s path and with the
/// options `spec` carries.
fn generate_spec_with_code(spec: Spec) -> (Report, String) {
    let (generated, _, code) = generate_and_check_spec(spec);
    (generated, code)
}

/// The emitted module below its provenance header, whose digests and spec path differ between
/// two documents that generate the same client.
fn module_body(code: &str) -> String {
    code.lines()
        .skip_while(|line| line.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `check` must stand in for `generate`: the same accept/reject decision and the identical
/// diagnostics ([`oracles::parity_violations`]). The outcomes themselves differ by design (`Clean`
/// against `Generated`), since only one of the two writes a module.
fn assert_check_agrees(generated: &Report, checked: &Report) {
    let violations = oracles::parity_violations(generated, checked);
    assert!(
        violations.is_empty(),
        "check does not stand in for generate: {violations:#?}\n{checked:#?}\n{generated:#?}"
    );
}

/// Fail if anything from the generated `types` module onward uses `serde_json::Value` as a type:
/// the silent degradation of a typed schema the standing invariants forbid. The embedded runtime
/// is stripped first, since it legitimately uses the type. The emitted file is
/// prettyplease-formatted, so the spelling searched for is the formatted one, never the token
/// stream's `serde_json :: Value`.
///
/// Two uses are not a degradation and are skipped: a path through the type
/// (`serde_json::Value::deserialize`, `serde_json::Value::Object`), which is how a union's own
/// `Deserialize` buffers its input, and a `let` binding inside an emitted impl body, which is how a
/// discriminated union's `Serialize` re-inserts its tag. What remains is a field, alias, variant
/// payload or signature type, where `serde_json::Value` would be the schema's lowered type.
fn assert_no_untyped_value(code: &str) {
    let types = types_module(code);
    assert!(!types.is_empty(), "no `types` module was emitted: {code}");
    let degraded: Vec<&str> = types
        .lines()
        .filter(|line| !line.trim_start().starts_with("let "))
        .filter(|line| {
            line.match_indices("serde_json::Value")
                .any(|(at, needle)| !line[at + needle.len()..].starts_with("::"))
        })
        .collect();
    assert!(
        degraded.is_empty(),
        "a typed schema degraded to `serde_json::Value`: {degraded:#?}\n{types}"
    );
}

/// A document whose only component `U` is `body`, reached from one response body.
fn single_component_document(body: &str) -> String {
    format!(
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
            application/json: {{ schema: {{ $ref: '#/components/schemas/U' }} }}
components:
  schemas:
    Name: {{ type: string }}
    Base:
      type: object
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    U:
{body}"##
    )
}

/// Build a two-file description in a throwaway tempdir and run it through both entry points.
///
/// The root document is fixed — one operation whose `200` body `$ref`s `target` — and `lib` is
/// written beside it as `lib.yaml`. Every shape below differs only in that sub-file, so what a
/// fixture pins is the sub-file's own reference behaviour and nothing else. The tempdir is dropped
/// on return; the report owns its data and the emitted source is read out first. The run is held
/// to parity and parseability by [`generate_and_check_dir`].
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
    generate_and_check_dir(&dir, "openapi.yaml")
}

/// The aliased spelling of [`a_nullable_alias_carries_its_targets_own_nullability`]'s document,
/// hoisted so [`PARITY_FIXTURES`] keeps a clean run on the nullable-alias lowering path among its
/// labelled ones and moves its schemas into a referenced file. The fixture asserts this constant
/// equals what its own builder produces, so the two cannot drift apart.
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

/// A document whose `components.schemas` are `schemas`, beside a declared `Cat`.
fn with_schemas(version: &str, schemas: &str) -> String {
    format!(
        "openapi: {version}\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         paths: {{}}\n\
         components:\n  \
         schemas:\n{schemas}    \
         Cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }} }} }}\n"
    )
}

/// A property declared with different lowered types in two `allOf` members, and required by one of
/// them, is irreconcilable → E013. The requirement is what empties the object: without it `{}` is
/// valid and the property is typed uninhabited instead (see
/// `an_all_of_conflict_on_an_optional_property_agrees_with_every_other_spelling`).
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
          required: [x]
          properties:
            x: { type: integer }
"##;

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

/// Every `E004` diagnostic's pointer, asserting the report rejected and carries at least one.
fn e004_pointers<'a>(report: &'a Report, what: &str) -> Vec<&'a str> {
    assert_eq!(report.outcome(), Outcome::Rejected, "{what}\n{report:#?}");
    let pointers: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::UnresolvedRef)
        .map(|d| d.pointer.as_str())
        .collect();
    assert!(!pointers.is_empty(), "{what}: expected E004\n{report:#?}");
    pointers
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

/// A document whose `/page` selects `text/plain` over `text/html` and passes every gate of its
/// own, so it emits `W014`, while `/doc` is rejected (`E009`).
const W014_REJECTED_ELSEWHERE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string } }
            text/html: { schema: { type: string } }
  /doc:
    get:
      operationId: getDoc
      responses:
        "200":
          description: OK
          content:
            application/pdf: { schema: {} }
"##;

// --- check/generate parity ----------------------------------------------------------------------
//
// The module header calls parity a contract, and `spargen check` is sold as telling you what
// `generate` would do. Every fixture that goes through `generate`, `generate_with_code` or a
// multi-file helper already gets it from `assert_check_agrees`. `PARITY_FIXTURES` keeps ten named
// specs on top of that: a list a companion test holds to span rejections, warnings and clean runs,
// and whose labelled names each hold the fixture to the code it is named for.

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
    // this one drives the nullable-alias lowering path, where `check` and `generate` take the
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

/// Write a root document whose only Path Item is a `$ref` to a sibling file holding `path_item`,
/// then run both entry points over it. The indirection is the point: the metaschema once validated
/// the root document and nothing else, so a Path Item reached by `$ref` never met it, and these
/// fixtures pin that it now does (#234). Returns `(generate, check)`, which
/// [`generate_and_check_dir`] has already held to the same verdict and codes.
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
    generate_and_check_dir(&dir, "openapi.yaml")
}

/// Write `files` (the first is the root, `openapi.yaml`) into a tempdir and run both entry points,
/// as [`generate_and_check_dir`].
fn generate_and_check_files(files: &[(&str, &str)]) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    generate_and_check_dir(&dir, files[0].0)
}

/// Run both entry points over the multi-file description rooted at `dir/root`, as
/// [`generate_and_check_spec`].
fn generate_and_check_dir(dir: &camino::Utf8Path, root: &str) -> (Report, Report, String) {
    generate_and_check_spec(Spec::new(dir.join(root)))
}

/// Run both entry points over `spec`, writing the module to `client.rs` beside its root document,
/// and hold the pair to [`assert_check_agrees`]. Returns `(generate, check, module)`, the module
/// empty when nothing was written.
///
/// Every helper in this file that runs `generate` over a description ends here, so a fixture that
/// writes several files, or sets an option, gets the same parity check as one written inline.
fn generate_and_check_spec(spec: Spec) -> (Report, Report, String) {
    let out = spec.path().with_file_name("client.rs");
    let checked = run_check(&spec);
    let generated = run_generate(&spec.build(out.clone()).cargo(CargoIntegration::Off));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    assert_check_agrees(&generated, &checked);
    (generated, checked, code)
}

/// A root document whose two operations reach a `Shape` declared in `a.yaml` and another declared
/// in `b.yaml`, listed `/a` first unless `b_first`; returns the generated client.
fn generate_two_file_shapes(b_first: bool) -> String {
    let refs = [
        ("a", "./a.yaml#/components/schemas/Shape"),
        ("b", "./b.yaml#/components/schemas/Shape"),
    ];
    let (report, _, code) = generate_and_check_files(&[
        ("openapi.yaml", &two_shape_root(refs, b_first)),
        ("a.yaml", SHAPE_ALPHA_YAML),
        ("b.yaml", SHAPE_BETA_YAML),
    ]);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    code
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

/// Write `files` into a fresh directory and run both entry points over its `openapi.json`, as
/// [`generate_and_check_dir`].
fn run_placement(files: &[(&str, serde_json::Value)]) -> (Report, Report) {
    let (generated, checked, _) = run_placement_with_client(files);
    (generated, checked)
}

/// [`run_placement`], also returning the generated client (empty when nothing was written).
fn run_placement_with_client(files: &[(&str, serde_json::Value)]) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    for (name, value) in files {
        std::fs::write(dir.join(name), serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    generate_and_check_dir(&dir, "openapi.json")
}
