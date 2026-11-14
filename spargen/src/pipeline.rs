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
    generate_rereading(build, cache_dir, |spec| {
        run_on_frontend_stack(|| cache::InputSnapshot::load(spec))
    })
}

/// [`generate_with_cache_dir`], with `reread` as the read of the inputs after each preview that
/// sees whether they changed while the preview read them, so a test can change them in between.
fn generate_rereading(
    build: &Build,
    cache_dir: Option<&Utf8Path>,
    mut reread: impl FnMut(&Spec) -> Result<cache::InputSnapshot, cache::LoadFailure>,
) -> Report {
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
    // A load that fails names what it read, the file that failed it included, so fixing that
    // file reruns the build.
    let rerun_inputs = |read: &[Utf8PathBuf]| {
        if emit_cargo_directives {
            runtime_contract::print_rerun(&cache::rerun_paths(spec, read, Some(&build.output)));
        }
    };

    let mut snapshot = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            rerun_inputs(&failure.paths);
            return with_leading(
                Report::new(*failure.diagnostics, Outcome::Rejected),
                cargo_diagnostics,
            );
        }
    };
    rerun_inputs(&snapshot.paths);

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
            return with_leading(
                pipeline_error_report("generation produced no module".to_owned()),
                cargo_diagnostics,
            );
        };
        let Some(requirements) = preview.requirements.as_ref() else {
            return with_leading(
                pipeline_error_report("generation produced no runtime requirements".to_owned()),
                cargo_diagnostics,
            );
        };
        // A fresh run rejects on any audit finding, before anything is written. The inner run's
        // truncation carries: merging its diagnostics into a wider list does not make the dropped
        // ones reappear.
        audit(&mut preview.report, requirements);
        if preview.report.outcome == Outcome::Rejected {
            return with_leading(preview.report, cargo_diagnostics);
        }

        let after = match reread(spec) {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                rerun_inputs(&failure.paths);
                return with_leading(
                    Report::new(*failure.diagnostics, Outcome::Rejected),
                    cargo_diagnostics,
                );
            }
        };
        if snapshot.digest != after.digest {
            snapshot = after;
            rerun_inputs(&snapshot.paths);
            continue;
        }

        let (contents, content_digest) = cache::finalized(&plan.contents, &snapshot.digest);
        if let Err(message) = output::write_output(&build.output, &contents) {
            return with_leading(pipeline_error_report(message), cargo_diagnostics);
        }
        // The module is written, so a record that cannot be written costs only the next build's
        // replay: a warning on a generating run, never a rejection of the module it wrote.
        if let Some(path) = &cache_path {
            if let Err(message) = cache::write_cache(
                path,
                &snapshot.digest,
                &content_digest,
                &preview.report,
                requirements,
            ) {
                preview.report.diagnostics.push(
                    Diagnostic::at_root(
                        Code::CacheWriteFailed,
                        format!(
                            "{message}; the module was written, and the next build regenerates it"
                        ),
                    )
                    .build(),
                );
            }
        }
        return with_leading(preview.report, cargo_diagnostics);
    }

    with_leading(
        pipeline_error_report(
            "generation inputs changed repeatedly while they were being read; retry the build"
                .to_owned(),
        ),
        cargo_diagnostics,
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
        bind_headers: &spec.bound_headers,
    };
    let api = oas31::lower(&document, &resolver, diags, options).map_err(|_| ())?;
    ir::check_invariants(&api, diags);
    if diags.has_errors() {
        return Err(());
    }

    // Lowering bound each header the spec names, so no argument, field or binding is named for
    // one; a binding no operation's header parameter matched is reported against `paths`, where
    // the operations it searched are declared.
    for name in api.unmatched_bindings(&spec.bound_headers) {
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
        // finitely many constructs); the user's `batch_cap` still governs the final report: a
        // converged carve reruns under it, and a rejection is replayed into it by `capped`.
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
            return Err(capped(report, spec.batch_cap));
        }
        omit.rules.extend(new_rules);
        last_rejection = Some(report);
    }

    // Exhausted the round cap while still rejecting: return the last honest rejection report.
    // With no round run at all, nothing was carved.
    match last_rejection {
        Some(report) => Err(capped(report, spec.batch_cap)),
        None => Ok(Spec {
            carve: crate::Carve::Off,
            ..spec.clone()
        }),
    }
}

/// `report`, a rejection collected without a cap, as a batch capped at `cap` would have collected
/// it: its diagnostics replayed in emission order, so past the cap a warning is dropped and an
/// error evicts the most recent retained warning, and the report says it was truncated.
fn capped(report: Report, cap: usize) -> Report {
    let mut diags = diag::Diagnostics::new(cap);
    for diagnostic in report.diagnostics {
        diags.emit(diagnostic);
    }
    Report::new(diags, report.outcome)
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

    /// A spec in a fresh directory whose one schema is named `schema`, built to `api.rs` beside it
    /// outside any build script.
    fn rereading_fixture(schema: &str) -> (tempfile::TempDir, Build) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec_naming(schema)).unwrap();
        let build = Spec::new(dir.join("openapi.yaml"))
            .build(dir.join("api.rs"))
            .with_cargo(crate::CargoIntegration::Off);
        (temp, build)
    }

    /// A description whose one component schema is named `schema`.
    fn spec_naming(schema: &str) -> String {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n\
             components:\n  schemas:\n    {schema}:\n      type: object\n      \
             properties: {{ name: {{ type: string }} }}\n"
        )
    }

    /// Inputs changed between a preview and the reread after it send the run round again, over the
    /// changed inputs: the module written is the one the last preview rendered.
    #[test]
    fn inputs_changed_during_a_preview_are_previewed_again() {
        let (_temp, build) = rereading_fixture("Before");
        let mut rereads = 0;
        let report = generate_rereading(&build, None, |spec| {
            rereads += 1;
            if rereads == 1 {
                std::fs::write(&spec.path, spec_naming("After")).unwrap();
            }
            cache::InputSnapshot::load(spec)
        });
        assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
        assert_eq!(rereads, 2);
        let module = std::fs::read_to_string(&build.output).unwrap();
        assert!(module.contains("pub struct After"), "{module}");
        assert!(!module.contains("Before"), "{module}");
    }

    /// Inputs that change after every one of the three previews end the run rejected, with nothing
    /// written.
    #[test]
    fn inputs_that_keep_changing_reject_the_run_after_three_previews() {
        let (_temp, build) = rereading_fixture("Schema0");
        let mut rereads = 0;
        let report = generate_rereading(&build, None, |spec| {
            rereads += 1;
            std::fs::write(&spec.path, spec_naming(&format!("Schema{rereads}"))).unwrap();
            cache::InputSnapshot::load(spec)
        });
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(rereads, 3);
        let errors: Vec<_> = report
            .errors()
            .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str()))
            .collect();
        assert_eq!(
            errors,
            [(
                "E011",
                "generation inputs changed repeatedly while they were being read; retry the build"
            )],
            "{report:#?}"
        );
        assert!(!build.output.exists());
    }

    /// A spec in a fresh directory whose one schema is named `Pet`, built to `output` relative to
    /// that directory under the default `CargoIntegration::Auto`, which outside a build script (as
    /// a test process is) puts `W013` ahead of the run's own diagnostics.
    fn auto_integration_fixture(output: &str) -> (tempfile::TempDir, Build) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("openapi.yaml"), spec_naming("Pet")).unwrap();
        let build = Spec::new(dir.join("openapi.yaml")).build(dir.join(output));
        (temp, build)
    }

    /// The codes of `report`'s diagnostics, in order.
    fn codes(report: &Report) -> Vec<&'static str> {
        report
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    /// An output that cannot be written rejects the run with the write's `E011`, and keeps the
    /// build-environment warning the run opened with ahead of it.
    #[test]
    fn an_output_write_failure_keeps_the_build_environment_warnings() {
        let (temp, build) = auto_integration_fixture("occupied/api.rs");
        // A regular file where the output's directory should be, so the write cannot land.
        std::fs::write(temp.path().join("occupied"), "").unwrap();

        let report = generate_with_cache_dir(&build, None);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(codes(&report), ["W013", "E011"], "{report:#?}");
        assert_eq!(
            report.diagnostics()[0].code,
            Code::CargoIntegrationDegraded,
            "{report:#?}"
        );
    }

    /// A cache record that cannot be written once the module is leaves the run generating: the
    /// module stands, and the report carries a `W016` after the build-environment warning, and no
    /// error.
    #[test]
    fn a_cache_write_failure_after_the_output_is_written_is_a_warning() {
        let (temp, build) = auto_integration_fixture("api.rs");
        // A regular file where the cache directory should be, so the record cannot land.
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();
        std::fs::write(&cache_dir, "").unwrap();

        let report = generate_with_cache_dir(&build, Some(&cache_dir));
        assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
        assert_eq!(codes(&report), ["W013", "W016"], "{report:#?}");
        let warning = &report.diagnostics()[1];
        assert_eq!(warning.code, Code::CacheWriteFailed, "{report:#?}");
        assert!(
            warning.message.contains("failed to write cache")
                && warning.message.ends_with("the next build regenerates it"),
            "{report:#?}"
        );
        let module = std::fs::read_to_string(&build.output).unwrap();
        assert!(module.contains("pub struct Pet"), "{module}");
    }

    /// Inputs that keep changing reject the run with the build-environment warning it opened with
    /// still ahead of the rejection.
    #[test]
    fn inputs_that_keep_changing_keep_the_build_environment_warnings() {
        let (_temp, build) = auto_integration_fixture("api.rs");
        let mut rereads = 0;
        let report = generate_rereading(&build, None, |spec| {
            rereads += 1;
            std::fs::write(&spec.path, spec_naming(&format!("Schema{rereads}"))).unwrap();
            cache::InputSnapshot::load(spec)
        });
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(codes(&report), ["W013", "E011"], "{report:#?}");
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

    /// Both rejections the carve driver returns are bounded by the spec's `batch_cap`, though its
    /// probe collects without one: the residual error evicts the `W009`s past the cap, and the
    /// report says it was truncated.
    #[test]
    fn a_carve_rejection_is_bounded_by_the_batch_cap() {
        let temp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
        let carve_capped = |contents: &str| {
            std::fs::write(&path, contents).unwrap();
            check(
                &Spec::new(path.clone())
                    .with_carve(crate::Carve::Auto)
                    .with_batch_cap(1),
            )
        };

        // A round that finds no new rule: the `$dynamicRef` operation carved, the dialect left.
        let stalled = carve_capped(
            "openapi: 3.1.0
info: { title: T, version: 1.0.0 }
jsonSchemaDialect: https://example.com/custom-dialect
paths:
  /dynamic:
    get:
      operationId: getDynamic
      responses:
        \"200\":
          description: OK
          content:
            application/json:
              schema: { $dynamicRef: \"#meta\" }
",
        );
        assert_eq!(stalled.outcome(), Outcome::Rejected, "{stalled:#?}");
        assert!(stalled.truncated(), "{stalled:#?}");
        assert_eq!(codes(&stalled), ["E002"], "{stalled:#?}");

        // The round cap: `cap - 1` links carved, the next one's `E004` left.
        let exhausted = carve_capped(&dangling_chain(compat::MAX_CARVE_ROUNDS));
        assert_eq!(exhausted.outcome(), Outcome::Rejected, "{exhausted:#?}");
        assert!(exhausted.truncated(), "{exhausted:#?}");
        assert_eq!(codes(&exhausted), ["E004"], "{exhausted:#?}");
    }
}
