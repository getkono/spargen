//! The runs the entry points drive: the frontend, the carve fixpoint, the in-memory render, the
//! consumer-manifest audit merged into a report, and `generate`'s cached write.

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::{self, Diagnostic};
use crate::runtime_contract::RuntimeRequirements;
use crate::{
    cache, codegen, compat, emit, ir, name, oas31, output, runtime_contract, source, Build, Code,
    Outcome, Report, Spec,
};

/// The body of [`generate`](crate::generate), with the cache directory as an argument so a test
/// can point it at a directory of its own.
pub(crate) fn generate_with_cache_dir(build: &Build, cache_dir: Option<&Utf8Path>) -> Report {
    let spec = &build.spec;
    let cargo = runtime_contract::cargo_environment(build);
    if cargo.fatal {
        return Report::rejected(cargo.diagnostics, false);
    }
    let cargo_diagnostics = cargo.diagnostics;
    let consumer_manifest = cargo.manifest;
    let emit_cargo_directives = cargo.directives;
    // Audit against the consumer manifest when there is one. The manifests the audit read are
    // named to Cargo once, however many runs below the input-change retry audits.
    let mut name_manifests = emit_cargo_directives;
    let mut audit = |report: &mut Report, requirements: &RuntimeRequirements| {
        if let Some(manifest) = consumer_manifest.as_deref() {
            let manifests = apply_audit(report, manifest, requirements);
            if std::mem::take(&mut name_manifests) {
                runtime_contract::print_rerun(&manifests);
            }
        }
    };
    let rerun_inputs = |snapshot: Option<&cache::InputSnapshot>| {
        if emit_cargo_directives {
            runtime_contract::print_rerun(&cache::rerun_paths(build, snapshot));
        }
    };

    let mut snapshot = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
        Ok(snapshot) => snapshot,
        Err(diagnostics) => {
            rerun_inputs(None);
            return with_leading(
                Report::new(diagnostics, Outcome::Rejected),
                cargo_diagnostics,
            );
        }
    };
    rerun_inputs(Some(&snapshot));

    let cache_path = cache_dir.map(|dir| cache::cache_path(dir, &build.output));
    if let Some(path) = &cache_path {
        let cached = cache::verified_output(&build.output, &snapshot.digest)
            .and_then(|content_digest| cache::read_cache(path, &snapshot.digest, &content_digest));
        if let Some(cached) = cached {
            // A verified cache hit rewrote nothing, so the outcome is `Cached`, not
            // `Generated` — `wrote_output` must stay true to what happened.
            let mut report = Report {
                diagnostics: cached.diagnostics,
                outcome: Outcome::Cached,
                truncated: cached.truncated,
                // Only a generating run is cached, and a generating run emitted no error.
                error_dropped: false,
            };
            audit(&mut report, &cached.requirements);
            return with_leading(report, cargo_diagnostics);
        }
    }

    for _ in 0..3 {
        let mut preview = run_mode(spec, PipelineMode::Preview);
        if preview.report.outcome != Outcome::Generated {
            return with_leading(preview.report, cargo_diagnostics);
        }
        let Some(plan) = preview.plan.as_ref() else {
            return pipeline_error_report("generation produced no module".to_owned());
        };
        let Some(requirements) = preview.requirements.as_ref() else {
            return pipeline_error_report("generation produced no runtime requirements".to_owned());
        };
        // A fresh run rejects on any audit finding, before anything is written. The inner run's
        // truncation carries: merging its diagnostics into a wider list does not make the dropped
        // ones reappear.
        audit(&mut preview.report, requirements);
        if preview.report.outcome == Outcome::Rejected {
            return with_leading(preview.report, cargo_diagnostics);
        }

        let after = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
            Ok(snapshot) => snapshot,
            Err(diagnostics) => {
                return with_leading(
                    Report::new(diagnostics, Outcome::Rejected),
                    cargo_diagnostics,
                );
            }
        };
        if snapshot.digest != after.digest {
            snapshot = after;
            rerun_inputs(Some(&snapshot));
            continue;
        }

        let (contents, content_digest) = cache::finalized(&plan.contents, &snapshot.digest);
        if let Err(message) = output::write_output(&build.output, &contents) {
            return pipeline_error_report(message);
        }
        if let Some(path) = &cache_path {
            if let Err(message) = cache::write_cache(
                path,
                &snapshot.digest,
                &content_digest,
                &preview.report,
                requirements,
            ) {
                return pipeline_error_report(message);
            }
        }
        return with_leading(preview.report, cargo_diagnostics);
    }

    pipeline_error_report(
        "generation inputs changed repeatedly while they were being read; retry the build"
            .to_owned(),
    )
}

/// `report` with the Cargo-integration diagnostics put ahead of its own, so a build log reads in
/// the order things happened.
fn with_leading(mut report: Report, leading: Vec<Diagnostic>) -> Report {
    report.diagnostics.splice(0..0, leading);
    report
}

/// Run the consumer-manifest dependency audit (`E023`) of `manifest` against `requirements`,
/// append its diagnostics to `report`'s, after the run's own, and reject the report when the
/// audit reported anything: every caller rejects on any audit finding. Returns every manifest the
/// audit read, for rebuild tracking.
pub(crate) fn apply_audit(
    report: &mut Report,
    manifest: &Utf8Path,
    requirements: &RuntimeRequirements,
) -> Vec<Utf8PathBuf> {
    let audit = runtime_contract::audit(manifest, requirements);
    if !audit.diagnostics.is_empty() {
        report.outcome = Outcome::Rejected;
    }
    report.diagnostics.extend(audit.diagnostics);
    audit.manifests
}

/// The stack size for the dedicated frontend worker thread, room for the bounded peak recursion
/// [`run_on_frontend_stack`] relies on.
const FRONTEND_STACK: usize = 64 * 1024 * 1024;

/// Run `f` (the whole recursive frontend/pipeline) on a dedicated thread with a large, fixed stack,
/// decoupling spargen's recursion budget from the caller's stack size. Parsing, deserialization,
/// meta-schema validation, and lowering all recurse over the document. Lowering caps its own
/// recursion at a fixed depth (`oas31::lower::MAX_SCHEMA_DEPTH`), and the JSON and YAML parsers
/// both refuse a document nested past a fixed depth before recursing past it, an alias's copy
/// counted at the depth it reaches, so the peak stack is bounded; this thread guarantees room for
/// that bound however small the caller's own stack is. Together they make stack exhaustion on any
/// input impossible for the work that runs here, whether the caller is a `build.rs`, the CLI, a
/// proptest worker, or a libFuzzer target. A panic inside the worker is
/// propagated to the caller unchanged (so genuine bugs still surface). Thread creation is not
/// input-driven; the only way it fails is OS resource exhaustion, which is outside the "no input
/// crashes the generator" contract.
pub(crate) fn run_on_frontend_stack<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("spargen-frontend".to_owned())
            .stack_size(FRONTEND_STACK)
            .spawn_scoped(scope, f)
            .expect("spawn spargen frontend worker thread")
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PipelineMode {
    /// Lower far enough to derive the runtime dependency set, no codegen (`spargen::check` and
    /// `spargen::requirements`).
    Requirements,
    /// Render in memory for the build cache or private proc-macro bridge.
    Preview,
}

/// The result of a pipeline run: the user-facing [`Report`], the runtime requirements of an API
/// that lowered, and, when the run rendered code in [`PipelineMode::Preview`], the in-memory
/// [`emit::EmitPlan`]. A [`PipelineMode::Requirements`] run and a rejection carry no plan.
pub(crate) struct PipelineResult {
    pub(crate) report: Report,
    pub(crate) plan: Option<emit::EmitPlan>,
    pub(crate) requirements: Option<RuntimeRequirements>,
}

impl PipelineResult {
    /// A report-only result (no retained plan) — the shape of every non-preview terminal.
    fn bare(report: Report) -> Self {
        Self {
            report,
            plan: None,
            requirements: None,
        }
    }
}

/// Run the whole frontend — `source` → `oas31` (validate/parse/audit/lower) → IR invariants →
/// `name` allocation — and return the lowered [`Api`](ir::Api) plus its allocated
/// [`Names`](name::Names). This is the exact work `generate` and `check` share before codegen, and
/// the sole input `diff` needs; on any rejection it returns `Err(())` with `diags` already carrying
/// the error diagnostics.
pub(crate) fn lower_frontend(
    spec: &Spec,
    diags: &mut diag::Diagnostics,
) -> Result<(ir::Api, name::Names), ()> {
    let bundle = load_bundle(spec, diags)?;
    lower_bundle(spec, &bundle, diags)
}

/// The input bundle `spec` names, with its omit profile applied: the first half of
/// [`lower_frontend`], split out so the carve driver can map a rejection back onto the very bundle
/// that produced it.
fn load_bundle(spec: &Spec, diags: &mut diag::Diagnostics) -> Result<source::InputBundle, ()> {
    let mut bundle = source::InputBundle::load(&spec.path, diags).map_err(|_| ())?;

    if !spec.omit.is_empty() && spec.omit.apply(&mut bundle, diags).is_err() {
        return Err(());
    }
    Ok(bundle)
}

/// The rest of [`lower_frontend`]: validate, parse, audit, lower, and allocate names over `bundle`,
/// with the lowering options `spec` sets.
fn lower_bundle(
    spec: &Spec,
    bundle: &source::InputBundle,
    diags: &mut diag::Diagnostics,
) -> Result<(ir::Api, name::Names), ()> {
    let validator = oas31::MetaSchemaValidator::load_vendored();
    validator.validate(bundle, diags);
    if diags.has_errors() {
        return Err(());
    }

    let document = oas31::parse_document(bundle, diags).map_err(|_| ())?;

    let resolver = oas31::Resolver::new(&document, bundle);
    oas31::audit(&document, &resolver, diags);
    if diags.has_errors() {
        return Err(());
    }

    // `check` runs the full frontend — lowering, IR invariants, and name allocation — so it fires
    // exactly the frontend diagnostics `generate` would, just without emitting code.
    let options = oas31::LowerOptions {
        open_narrowing: spec.open_narrowing.is_on(),
        open_unions: spec.open_unions.is_on(),
        bind_headers: &spec.bind_headers,
    };
    let api = oas31::lower(&document, &resolver, diags, options).map_err(|_| ())?;
    ir::check_invariants(&api, diags);
    if diags.has_errors() {
        return Err(());
    }

    // Lowering bound each header the spec names, so no argument, field or binding is named for
    // one; a binding no operation's header parameter matched is reported against `paths`, where
    // the operations it searched are declared.
    for name in api.unmatched_bindings(&spec.bind_headers) {
        Diagnostic::error(
            Code::BoundHeaderNotDeclared,
            diag::Provenance::new(diag::JsonPointer::root().push("paths"), None),
        )
        .message(format!(
            "the header binding `{name}` matches no header parameter any operation declares"
        ))
        .emit(diags);
    }
    if diags.has_errors() {
        return Err(());
    }

    // Allocation is total: every API that lowered gets its names.
    let names = name::allocate(&api);
    Ok((api, names))
}

/// Codegen + emit-planning for an already-lowered API, the step [`PipelineMode::Preview`] adds to
/// the frontend. Returns the fully rendered module plan or `Err(())` with the codegen error already
/// pushed onto `diags` as `E011`, so nothing is written or cached.
fn build_emit_plan(
    spec: &Spec,
    api: &ir::Api,
    names: &name::Names,
    diags: &mut diag::Diagnostics,
) -> Result<emit::EmitPlan, ()> {
    let code = codegen::generate(
        api,
        names,
        &codegen::CodegenOptions {
            feature_uuid: spec.uuid.is_on(),
            datetime: spec.datetime,
            error_body_cap: spec.error_body_cap,
            verbatim_docs: spec.verbatim_docs.is_on(),
            testing: spec.testing.is_on(),
        },
    )
    .map_err(|error| emit_pipeline_error(diags, error.to_string()))?;

    // The spec's file name, not its path: the header is committed output, so it must read the same
    // in every checkout and on every OS (#1347).
    let name = crate::cache::spec_name(&spec.path);
    let emit_options = emit::EmitOptions {
        spec: emit::SpecMeta {
            source: if spec.omit.is_empty() {
                name.to_owned()
            } else {
                format!("{name} omit={}", spec.omit.fingerprint())
            },
            spargen_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    };

    Ok(emit::plan(&code, &emit_options))
}

/// Run `mode` over `spec` on the dedicated frontend stack, through the auto-carve driver when
/// `spec.carve` asks for it and the plain pipeline otherwise. Every public entry point that runs
/// the pipeline dispatches here; [`diff`](crate::diff), which lowers two specs and runs no mode,
/// makes the same decision per side in `diff_api`'s `lower_side`, through the same
/// [`carve_spec`] fixpoint.
pub(crate) fn run_mode(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    run_on_frontend_stack(|| {
        if spec.carve.is_on() {
            run_carve(spec, mode)
        } else {
            run_pipeline(spec, mode)
        }
    })
}

fn run_pipeline(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    let mut diags = diag::Diagnostics::new(spec.batch_cap);

    let (api, names) = match lower_frontend(spec, &mut diags) {
        Ok(pair) => pair,
        Err(()) => return PipelineResult::bare(Report::new(diags, Outcome::Rejected)),
    };
    let requirements = RuntimeRequirements::for_api(&api, spec);

    match mode {
        // Requirements stops before codegen.
        PipelineMode::Requirements => PipelineResult {
            report: Report::new(diags, Outcome::Clean),
            plan: None,
            requirements: Some(requirements),
        },
        // Preview keeps the rendered plan and writes nothing.
        PipelineMode::Preview => match build_emit_plan(spec, &api, &names, &mut diags) {
            Ok(plan) => PipelineResult {
                plan: Some(plan),
                report: Report::new(diags, Outcome::Generated),
                requirements: Some(requirements),
            },
            Err(()) => PipelineResult::bare(Report::new(diags, Outcome::Rejected)),
        },
    }
}

/// Auto-carve driver: resolve the carved omit set with [`carve_spec`], then run `mode` once with
/// it, which re-applies every omit rule and so emits a `W009` for each carved construct.
fn run_carve(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    match carve_spec(spec) {
        Ok(resolved) => run_pipeline(&resolved, mode),
        Err(report) => PipelineResult::bare(report),
    }
}

/// The carve fixpoint: `spec` with `carve` off and the carved omit set in place of its own once the
/// frontend lowers under it, or the rejection [`Report`] of the round that could make no progress
/// (or of the last round, at the cap).
///
/// Each round runs the frontend alone (no codegen, no output written) with the current omit set.
/// If it is not rejected, the carve converged. If it is rejected, the error pointers are mapped to
/// the smallest enclosing omittable constructs ([`compat::carve_rules`]) and any *new* rules are
/// added; when a round adds none (an un-carvable residual — a root/unmodelled rejection, or a rule
/// that did not clear its error), that round's report is returned as-is: it carries the `W009`s
/// for what *was* carved plus the residual errors, with `Outcome::Rejected`. Omitting a construct
/// can dangle a `$ref` and surface a fresh `E004`/`E020`; that error is carved on the next round
/// (its enclosing operation is omitted) or, if un-carvable, reported — the document is never
/// emitted broken. The round cap ([`compat::MAX_CARVE_ROUNDS`]) guarantees termination.
pub(crate) fn carve_spec(spec: &Spec) -> Result<Spec, Report> {
    let mut omit = spec.omit.clone();
    let mut last_rejection: Option<Report> = None;

    for _ in 0..compat::MAX_CARVE_ROUNDS {
        let probe = Spec {
            omit: omit.clone(),
            carve: crate::Carve::Off,
            ..spec.clone()
        };
        // The probe is the frontend alone — what a `check` run lowers — kept by hand rather than
        // through `run_pipeline` because the carve mapper needs the bundle it ran over: a
        // diagnostic's pointer addresses the file its span lies in, and only the bundle can name
        // that file in a rule or find the `$ref`s that reach it. The mapper must also see *every*
        // error diagnostic to carve correctly, so the probe's batch is unbounded (a spec has
        // finitely many constructs); the user's `batch_cap` still governs the final report below.
        let mut diags = diag::Diagnostics::new(usize::MAX);
        let bundle = load_bundle(&probe, &mut diags).ok();
        let lowered = bundle
            .as_ref()
            .is_some_and(|bundle| lower_bundle(&probe, bundle, &mut diags).is_ok());
        if lowered {
            // Converged: the caller runs for real with the carved omit set.
            return Ok(Spec {
                omit,
                carve: crate::Carve::Off,
                ..spec.clone()
            });
        }
        let report = Report::new(diags, Outcome::Rejected);

        // No bundle means the input did not load or the omit profile itself failed (`E019`/`E020`),
        // and neither encloses a construct to carve.
        let new_rules: Vec<compat::OmitRule> = bundle
            .map(|bundle| compat::carve_rules(&report.diagnostics, &bundle))
            .unwrap_or_default()
            .into_iter()
            .filter(|rule| !omit.rules.contains(rule))
            .collect();
        if new_rules.is_empty() {
            // No progress possible — report the residual rejections (and any carved W009s) honestly.
            return Err(report);
        }
        omit.rules.extend(new_rules);
        last_rejection = Some(report);
    }

    // Exhausted the round cap while still rejecting: return the last honest rejection report.
    // With no round run at all, nothing was carved.
    match last_rejection {
        Some(report) => Err(report),
        None => Ok(Spec {
            carve: crate::Carve::Off,
            ..spec.clone()
        }),
    }
}

fn emit_pipeline_error(diags: &mut diag::Diagnostics, message: String) {
    Diagnostic::at_root(Code::InvalidInput, message).emit(diags);
}

fn pipeline_error_report(message: String) -> Report {
    let mut diagnostics = diag::Diagnostics::new(1);
    emit_pipeline_error(&mut diagnostics, message);
    Report::new(diagnostics, Outcome::Rejected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check;

    /// `check` with auto-carve on, over `contents` written as the root document in `dir`.
    fn carve_check(dir: &tempfile::TempDir, contents: &str) -> Report {
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, contents).unwrap();
        check(&Spec::new(Utf8PathBuf::from_path_buf(path).unwrap()).with_carve(crate::Carve::Auto))
    }

    /// The `(code, pointer)` of every error-severity diagnostic in `report`.
    fn errors(report: &Report) -> Vec<(&'static str, &str)> {
        report
            .errors()
            .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.pointer.as_str()))
            .collect()
    }

    /// How many `W009`s — constructs carve removed — `report` carries.
    fn carved(report: &Report) -> usize {
        report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == Code::OmittedConstruct)
            .count()
    }

    /// One healthy operation, then the `$ref` chain `Link0 → Link1 → … → Missing` of `links`
    /// components. Only the last link dangles; omitting it dangles the one before, and so on, so
    /// carve removes exactly one link per round and needs `links` rounds before a probe lowers.
    fn dangling_chain(links: usize) -> String {
        let mut spec = String::from(
            "openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses:
        \"200\":
          description: OK
          content:
            application/json:
              schema: { type: string }
components:
  schemas:
",
        );
        for link in 0..links {
            let next = if link + 1 == links {
                "Missing".to_owned()
            } else {
                format!("Link{}", link + 1)
            };
            spec.push_str(&format!(
                "    Link{link}: {{ $ref: \"#/components/schemas/{next}\" }}\n"
            ));
        }
        spec
    }

    /// A round that finds no new rule ends the carve with that round's report: the residual
    /// rejection carve cannot map to a construct, beside the `W009` for what it did carve.
    #[test]
    fn a_carve_that_stops_making_progress_reports_its_residual_rejection() {
        let temp = tempfile::tempdir().unwrap();
        let report = carve_check(
            &temp,
            r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/custom-dialect
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses: { "204": { description: OK } }
  /dynamic:
    get:
      operationId: getDynamic
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $dynamicRef: "#meta" }
"##,
        );
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        // The root-level dialect has nothing above it to carve, so it is all that is left.
        assert_eq!(
            errors(&report),
            [("E002", "/jsonSchemaDialect")],
            "{report:#?}"
        );
        // The first round did carve the `$dynamicRef` operation, and says so.
        assert_eq!(carved(&report), 1, "{report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::OmittedConstruct
                    && diagnostic.message.contains("get /dynamic")),
            "{report:#?}"
        );
    }

    /// The round cap is exact: a chain whose carve clears within [`compat::MAX_CARVE_ROUNDS`]
    /// rounds lowers, and one link longer ends `Rejected` with the last round's report — the link
    /// it carved last as `W009`s, and the one its final rule dangled as the residual `E004`.
    #[test]
    fn a_carve_that_exhausts_the_round_cap_reports_its_last_rejection() {
        let cap = compat::MAX_CARVE_ROUNDS;
        let temp = tempfile::tempdir().unwrap();

        let within = carve_check(&temp, &dangling_chain(cap - 1));
        assert!(within.succeeded(), "{within:#?}");
        assert_eq!(carved(&within), cap - 1, "{within:#?}");

        let beyond = carve_check(&temp, &dangling_chain(cap));
        assert_eq!(beyond.outcome(), Outcome::Rejected, "{beyond:#?}");
        // The last round ran with `cap - 1` links carved and rejected the next one; the rule it
        // derived for that link was never tried, because the cap ended the loop first. (The code is
        // spelled as its string: `diag`'s emission-site scan reads every `Code::` variant named
        // under `src/` as a site that must carry an explain-text case marker.)
        assert_eq!(
            errors(&beyond),
            [("E004", "/components/schemas/Link0")],
            "{beyond:#?}"
        );
        assert_eq!(carved(&beyond), cap - 1, "{beyond:#?}");
    }
}
