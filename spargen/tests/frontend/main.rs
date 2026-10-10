//! Per-diagnostic frontend coverage: one minimal inline spec per rejection/warning code, asserting
//! the code fires and the pipeline outcome is what the taxonomy promises. Rejections travel through
//! `generate`. Check/generate parity is a property of the harness rather than a remark about one
//! case: every inline spec `generate` or `generate_with_code` runs is also run through `check`,
//! which must reach the same accept/reject verdict and report the same sorted codes, and every
//! `Generated` module must parse as Rust with no `compile_error!` fallback. `PARITY_FIXTURES` adds
//! a named set a companion test keeps spanning rejections, warnings and clean runs.
//!
//! Every run any fixture makes goes through `run_generate` or `run_check`, which hold each
//! diagnostic's declared `OutcomeClaim` to the run's own outcome (#413) and to a real location
//! (#454), and hold every union a `Generated` run emits to variants a value can tell apart (#402).
//! The last two oracles live in `oracles/`, shared with `fuzz_frontend.rs` and
//! `lowering_props.rs`.
//!
//! This file holds that shared harness; the fixtures sit in one module per construct family
//! beside it (`unions`, `ref_siblings`, `bodies`, ...), each reaching the harness through
//! `use super::*`.

use camino::Utf8PathBuf;
use spargen::{
    Build, CargoIntegration, Code, Diagnostic, Outcome, OutcomeClaim, Report, Severity, Spec,
};

#[path = "../oracles/mod.rs"]
mod oracles;

mod all_of;
mod bodies;
mod defaults;
mod diagnostics_meta;
mod discriminator;
mod document;
mod encoding;
mod parameters;
mod placement;
mod recursion;
mod ref_siblings;
mod refs;
mod responses;
mod schemas;
mod security;
mod servers;
mod union_nullability;
mod unions;
mod xml;

/// Run `generate` on an inline spec written into a throwaway tempdir, returning the report. The
/// tempdir (and any written output) is discarded once the report — which owns its data — is built.
/// A build for a fixture spec. These tests are not build scripts, so the Cargo integration is
/// explicitly off: no rebuild triggers to emit, no consumer manifest to audit, and — the reason it
/// matters here — no `W013` polluting the diagnostics a fixture is asserting on.
fn build(spec: Utf8PathBuf, out: Utf8PathBuf) -> Build {
    Spec::new(spec).build(out).cargo(CargoIntegration::Off)
}

/// `spargen::generate`, then [`assert_claims_hold`] and [`assert_located`] on the report, and
/// [`assert_distinguishable_variants`] on the module a `Generated` run wrote. Every fixture here
/// reaches `generate` through this function, so every diagnostic any fixture provokes is held to
/// its run, and every union any fixture emits is held to variants a value can tell apart.
fn run_generate(build: &Build) -> Report {
    let report = spargen::generate(build);
    assert_claims_hold(&report);
    assert_located(&report, build.spec());
    if report.outcome() == Outcome::Generated {
        let code = std::fs::read_to_string(build.output()).unwrap_or_default();
        assert_distinguishable_variants(&report, &code);
    }
    report
}

/// `spargen::check`, then [`assert_claims_hold`] and [`assert_located`] on the report, as
/// [`run_generate`].
fn run_check(spec: &Spec) -> Report {
    let report = spargen::check(spec);
    assert_claims_hold(&report);
    assert_located(&report, spec);
    report
}

/// Fail unless every diagnostic in `report` is held to a real location (#454), except where an open
/// issue tracks the gap: see [`oracles::location_violations`]. The root document is read back from
/// `spec`'s path, so a span can be compared with the file it lies in.
fn assert_located(report: &Report, spec: &Spec) {
    let root = std::fs::read(spec.path()).unwrap_or_default();
    let violations = oracles::unknown(oracles::location_violations(report.diagnostics(), &root));
    assert!(
        violations.is_empty(),
        "a `{}` run reported diagnostics with no real location: {violations:#?}",
        report.outcome()
    );
}

/// Fail if a `Generated` run emitted a union whose variants cannot be told apart and no warning
/// says why, except where an open issue tracks the gap: see
/// [`oracles::indistinguishable_variants`].
fn assert_distinguishable_variants(report: &Report, code: &str) {
    let violations = oracles::unexplained_variants(report, code);
    assert!(
        violations.is_empty(),
        "a union's variants cannot be told apart and no warning says why: {violations:#?}\n\
         {report:#?}"
    );
}

/// Fail unless every diagnostic in `report` makes a claim its run's outcome admits (#413).
fn assert_claims_hold(report: &Report) {
    let violations = claim_violations(report.outcome(), report.diagnostics());
    assert!(
        violations.is_empty(),
        "a `{}` run reported diagnostics whose claims are false of it: {violations:#?}",
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

/// `generate` on an inline spec, holding the run to the two oracles every fixture gets for free:
/// `check` over the same document reaches the same accept/reject verdict and reports the same
/// sorted codes ([`assert_check_agrees`]), and a `Generated` run wrote parseable Rust that is not
/// the `compile_error!` stub codegen falls back to ([`assert_parseable`]).
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
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let report = run_generate(&build(spec_path.clone(), out.clone()));
    let code = std::fs::read_to_string(out).unwrap_or_default();
    assert_check_agrees(&report, &run_check(&Spec::new(spec_path)));
    if report.outcome() == Outcome::Generated {
        assert_parseable(&code);
    }
    (report, code)
}

/// `check` must stand in for `generate`: the same accept/reject decision and the same sorted
/// diagnostic codes. The outcomes themselves differ by design (`Clean` against `Generated`), since
/// only one of the two writes a module.
fn assert_check_agrees(generated: &Report, checked: &Report) {
    assert_eq!(
        checked.outcome() == Outcome::Rejected,
        generated.outcome() == Outcome::Rejected,
        "check says {:?} but generate says {:?}: {checked:#?} {generated:#?}",
        checked.outcome(),
        generated.outcome()
    );
    assert_eq!(
        codes(checked),
        codes(generated),
        "check and generate report different diagnostics"
    );
}

/// A `Generated` run's module parses as a Rust file and is not the `compile_error!` stub codegen
/// emits when its own tokens fail to format, which keeps the outcome `Generated`.
fn assert_parseable(code: &str) {
    if let Err(error) = syn::parse_file(code) {
        panic!("a `Generated` run wrote unparseable Rust ({error}):\n{code}");
    }
    assert!(
        !code.contains("compile_error!"),
        "a `Generated` run wrote the codegen fallback stub:\n{code}"
    );
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

/// A diagnostic for the location oracle's own fixtures, at `pointer` with `span` in file `file`.
fn located(
    code: Code,
    pointer: &str,
    file: u32,
    span: (u32, usize, usize),
    message: &str,
) -> Diagnostic {
    let (line, start, end) = span;
    Diagnostic {
        code,
        severity: Severity::Warning,
        pointer: spargen::JsonPointer::from(pointer),
        span: Some(spargen::Span {
            file: spargen::FileId(file),
            start: spargen::Loc {
                line,
                col: 1,
                offset: start,
            },
            end: spargen::Loc {
                line,
                col: 1,
                offset: end,
            },
        }),
        message: message.to_owned(),
        remedy: None,
        interpretation: None,
        claim: OutcomeClaim::Independent,
    }
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

/// The right-hand side of `pub type ty = …;`, the type an alias names, or `None` when no such alias
/// is declared.
fn alias_target(code: &str, ty: &str) -> Option<String> {
    let head = format!("pub type {ty} = ");
    code.lines()
        .map(str::trim_start)
        .find_map(|line| line.strip_prefix(&head))
        .map(|rest| rest.trim_end().trim_end_matches(';').to_owned())
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
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    (generated, checked, code)
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

/// The message of every `code` diagnostic in `report`, in report order.
fn messages_with_code(report: &Report, code: Code) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
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
// `generate` would do. Every inline fixture that goes through `generate` or `generate_with_code`
// already gets the verdict-and-codes half of it from `assert_check_agrees`. `PARITY_FIXTURES` keeps
// ten named specs on top of that: a list a companion test holds to span rejections, warnings and
// clean runs, and whose labelled names each hold the fixture to the code it is named for.

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
/// fixtures pin that it now does (#234). Returns `(generate, check)` so a fixture can assert the two agree, the way
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
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    (generated, checked, code)
}

/// Write `files` (the first is the root, `openapi.yaml`) into a tempdir and run both entry points.
fn generate_and_check_files(files: &[(&str, &str)]) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let root = dir.join(files[0].0);
    let out = dir.join("client.rs");
    let generated = run_generate(&build(root.clone(), out.clone()));
    let checked = run_check(&Spec::new(root));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    (generated, checked, code)
}

/// Write `files` into a fresh directory and run both entry points over its `openapi.json`.
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
    let generated = run_generate(&build(dir.join("openapi.json"), dir.join("client.rs")));
    let client = std::fs::read_to_string(dir.join("client.rs")).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.json")));
    (generated, checked, client)
}
