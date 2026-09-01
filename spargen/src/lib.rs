//! # spargen
//!
//! A compile-time-correct Rust HTTP client generator for OpenAPI 3.1.x and 3.2.x. Spec in, spar out:
//! everything structural is decided at generation time; nothing is interpreted at runtime.
//!
//! This crate is the library half of the `spargen` tool. Its public surface is the `build.rs`
//! API — see the [facade](crate) items ([`Spec`], [`Build`], [`generate`], [`check`], [`explain`]).
//!
//! ## Subsystem layering
//!
//! The crate is internally partitioned into subsystems with a declared dependency DAG. Each
//! subsystem module records its allowed dependencies in a machine-readable `//! layer-deps:`
//! header; `spargen/tests/layering.rs` diffs those declarations against the actual inter-module
//! `use` edges and fails on any edge not in the table below.
//!
//! | Subsystem | May depend on |
//! |-----------|---------------|
//! | `diag`    | —             |
//! | `source`  | `diag`        |
//! | `ir`      | `diag`        |
//! | `oas31`   | `source`, `ir`, `name`, `diag` |
//! | `name`    | `ir`, `diag`  |
//! | `support` | — (compiles standalone against reqwest/serde) |
//! | `codegen` | `ir`, `name`, `support`, `diag` |
//! | `emit`    | `codegen`, `diag` |
//! | `compat`  | `source`, `diag` |
//! | `surface` | `ir`, `name`  |
//! | `cli`     | facade        |
//! | facade (`lib.rs`) | all of the above |
//!
//! Pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`, with `diag` as the
//! only vocabulary shared across stages. `compat` preprocesses the bundle before `oas31` sees it;
//! `surface` reads the lowered API for `diff` and never feeds generation.
//!
//! `cache`, `config`, and `runtime_contract` are facade plumbing rather than subsystems: they say
//! so in their own module docs and carry no `layer-deps:` header, so the lint skips them.

mod diag;

mod cache;
mod codegen;
mod compat;
mod config;
mod emit;
mod ir;
mod name;
mod oas31;
mod runtime_contract;
mod source;
mod support;
mod surface;

use std::str::FromStr;

use camino::{Utf8Path, Utf8PathBuf};

pub use compat::{ComponentKind, Omit, OmitMethod, OmitRule, UnknownOmitToken};
pub use config::{Build, CargoIntegration, ConfigError, Spec};
pub use diag::{Code, Diagnostic, FileId, InterpId, JsonPointer, Loc, Severity, Span, UnknownCode};
pub use runtime_contract::{RequiredDependency, Requirements};
#[cfg(feature = "remote-fetch")]
pub use source::{VendorReport, VendoredRef};
pub use surface::{Change, ChangeKind, DiffReport, Impact};

/// The outcome of a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Outcome {
    /// The generated module was freshly rendered and written.
    Generated,
    /// The generated module was already up to date and was verified from the build cache.
    Cached,
    /// The support audit completed without a rejection.
    Clean,
    /// The spec used an R-class construct; generation failed loudly.
    Rejected,
}

impl Outcome {
    /// The stable lowercase string form, used by the `--format json` renderer.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Generated => "generated",
            Outcome::Cached => "cached",
            Outcome::Clean => "clean",
            Outcome::Rejected => "rejected",
        }
    }

    /// Whether the run got as far as it was asked to.
    pub fn is_success(self) -> bool {
        !matches!(self, Outcome::Rejected)
    }

    /// Whether this run actually rewrote the output file. A cache hit did not.
    pub fn wrote_output(self) -> bool {
        matches!(self, Outcome::Generated)
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The result of a pipeline run: the collected diagnostics plus the outcome.
///
/// Fields are private and read through accessors, matching [`Spec`], [`Build`], [`Diagnostic`],
/// and [`DiffRejection`], so the shape can grow without a breaking change.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    /// Every diagnostic emitted during the run (batch reporting).
    diagnostics: Vec<Diagnostic>,
    /// What happened.
    outcome: Outcome,
    /// Whether the run hit `batch_cap` and dropped diagnostics past it.
    truncated: bool,
}

impl Report {
    /// Every diagnostic emitted during the run, in emission order.
    ///
    /// This is the whole list only when [`Self::truncated`] is `false`.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// What happened.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Whether the run reached `batch_cap` and dropped the diagnostics past it.
    ///
    /// A truncated report is a partial view: the constructs behind the dropped diagnostics were
    /// still diagnosed, so fixing every diagnostic listed here may not be enough to make the run
    /// clean. Raise [`Spec::batch_cap`] to see the rest.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Every error-severity diagnostic.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> + '_ {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Every warning-severity diagnostic.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> + '_ {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
    }

    /// Whether any error-severity diagnostic was emitted.
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Whether the run succeeded with no errors — the single check a build script wants.
    pub fn succeeded(&self) -> bool {
        self.outcome.is_success() && !self.has_errors()
    }

    /// Turn the report into a `Result`, so a build script can use `?`.
    pub fn into_result(self) -> Result<Self, Self> {
        if self.succeeded() {
            Ok(self)
        } else {
            Err(self)
        }
    }

    /// Print every diagnostic as a Cargo build-script directive, so it surfaces in the build's
    /// output at its own severity: an error-severity diagnostic goes out as `cargo::error=`, a
    /// warning as `cargo::warning=`. Announcing a rejection as a warning would make the one
    /// message that stops generation look like the ones that do not.
    pub fn emit_cargo_diagnostics(&self) {
        for line in self.cargo_directive_lines() {
            println!("{line}");
        }
    }

    /// The directive lines [`Self::emit_cargo_diagnostics`] prints, split out so the severity
    /// mapping is testable without capturing the process's stdout.
    fn cargo_directive_lines(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .map(|diagnostic| match diagnostic.severity {
                Severity::Error => format!("cargo::error={diagnostic}"),
                Severity::Warning => format!("cargo::warning={diagnostic}"),
            })
            .collect()
    }

    /// Panic with the rendered report unless the run succeeded. The build-script one-liner —
    /// deliberately not `#[must_use]`, because discarding the returned report is the normal use.
    pub fn expect_success(self) -> Self {
        assert!(self.succeeded(), "{self}");
        self
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "spargen: {}", self.outcome)?;
        for diagnostic in &self.diagnostics {
            writeln!(formatter, "{diagnostic}")?;
        }
        if self.truncated {
            writeln!(
                formatter,
                "spargen: diagnostic list truncated at batch_cap ({} shown); raise batch_cap to see the rest",
                self.diagnostics.len()
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

/// Run the full pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`. The
/// primary `build.rs` entry point.
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines.
/// let build = spargen::Spec::new("api/openapi.yaml").build("src/api.rs");
/// spargen::generate(&build).expect_success();
/// ```
pub fn generate(build: &Build) -> Report {
    let cache_dir = cache::cache_dir();
    generate_with_cache_dir(build, cache_dir.as_deref())
}

fn generate_with_cache_dir(build: &Build, cache_dir: Option<&Utf8Path>) -> Report {
    let spec = &build.spec;
    let cargo = cargo_environment(build);
    let mut cargo_diagnostics = cargo.diagnostics;
    if cargo.fatal {
        return Report {
            diagnostics: cargo_diagnostics,
            outcome: Outcome::Rejected,
            truncated: false,
        };
    }
    let consumer_manifest = cargo.manifest;
    let emit_cargo_directives = cargo.directives;
    let mut snapshot = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
        Ok(snapshot) => snapshot,
        Err(diagnostics) => {
            if emit_cargo_directives {
                cache::cargo_directives(build, None);
            }
            let truncated = diagnostics.cap_reached();
            cargo_diagnostics.extend(diagnostics.items().to_vec());
            return Report {
                diagnostics: cargo_diagnostics,
                outcome: Outcome::Rejected,
                truncated,
            };
        }
    };
    if emit_cargo_directives {
        cache::cargo_directives(build, Some(&snapshot));
    }

    let cache_path = cache_dir.map(|dir| cache::cache_path(dir, &build.output));
    if cache_dir.is_some() {
        if let Some(content_digest) = cache::verified_output(&build.output, &snapshot.digest) {
            let cached = cache_path
                .as_deref()
                .and_then(|path| cache::read_cache(path, &snapshot.digest, &content_digest));
            if let Some(mut cached) = cached {
                if let Some(manifest) = consumer_manifest.as_deref() {
                    let audit = runtime_contract::audit(manifest, &cached.requirements);
                    if emit_cargo_directives {
                        runtime_contract::cargo_directives(&audit.manifests);
                    }
                    cached.diagnostics.extend(audit.diagnostics);
                }
                cargo_diagnostics.extend(cached.diagnostics);
                if cargo_diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error)
                {
                    return Report {
                        diagnostics: cargo_diagnostics,
                        outcome: Outcome::Rejected,
                        truncated: false,
                    };
                }
                // A verified cache hit rewrote nothing, so the outcome is `Cached`, not
                // `Generated` — `wrote_output` must stay true to what happened.
                return Report {
                    diagnostics: cargo_diagnostics,
                    outcome: Outcome::Cached,
                    truncated: false,
                };
            }
        }
    }

    for _ in 0..3 {
        let preview = preview_inner(spec);
        if preview.report.outcome != Outcome::Generated {
            let mut report = preview.report;
            prepend(&mut report.diagnostics, &cargo_diagnostics);
            return report;
        }
        let Some(rendered) = preview.files.first().map(String::as_str) else {
            return pipeline_error_report("generation produced no module".to_owned());
        };
        let Some(requirements) = preview.requirements.as_ref() else {
            return pipeline_error_report("generation produced no runtime requirements".to_owned());
        };
        if let Some(manifest) = consumer_manifest.as_deref() {
            let audit = runtime_contract::audit(manifest, requirements);
            if emit_cargo_directives {
                runtime_contract::cargo_directives(&audit.manifests);
            }
            if !audit.diagnostics.is_empty() {
                let mut diagnostics = cargo_diagnostics;
                // The inner run's truncation carries: merging its diagnostics into a wider list
                // does not make the dropped ones reappear.
                let truncated = preview.report.truncated;
                diagnostics.extend(preview.report.diagnostics);
                diagnostics.extend(audit.diagnostics);
                return Report {
                    diagnostics,
                    outcome: Outcome::Rejected,
                    truncated,
                };
            }
        }

        let after = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
            Ok(snapshot) => snapshot,
            Err(diagnostics) => {
                let truncated = diagnostics.cap_reached();
                cargo_diagnostics.extend(diagnostics.items().to_vec());
                return Report {
                    diagnostics: cargo_diagnostics,
                    outcome: Outcome::Rejected,
                    truncated,
                };
            }
        };
        if snapshot.digest != after.digest {
            snapshot = after;
            if emit_cargo_directives {
                cache::cargo_directives(build, Some(&snapshot));
            }
            continue;
        }

        let (contents, content_digest) = cache::finalized(rendered, &snapshot.digest);
        if let Err(message) = cache::write_output(&build.output, &contents) {
            return pipeline_error_report(message);
        }
        if let Some(path) = &cache_path {
            if let Err(message) = cache::write_cache(
                path,
                &snapshot.digest,
                &content_digest,
                &preview.report.diagnostics,
                requirements,
            ) {
                return pipeline_error_report(message);
            }
        }
        let mut report = preview.report;
        prepend(&mut report.diagnostics, &cargo_diagnostics);
        return report;
    }

    pipeline_error_report(
        "generation inputs changed repeatedly while they were being read; retry the build"
            .to_owned(),
    )
}

/// Put the Cargo-integration diagnostics ahead of the pipeline's own, so a build log reads in the
/// order things happened.
fn prepend(diagnostics: &mut Vec<Diagnostic>, leading: &[Diagnostic]) {
    if leading.is_empty() {
        return;
    }
    let mut merged = leading.to_vec();
    merged.append(diagnostics);
    *diagnostics = merged;
}

/// What the Cargo environment affords this run, resolved from [`CargoIntegration`] once so the
/// generation path below never has to re-decide it.
struct CargoEnvironment {
    /// Emit `cargo:rerun-if-changed` directives.
    directives: bool,
    /// The consumer manifest to audit against (`E023`), when one was found.
    manifest: Option<Utf8PathBuf>,
    /// `W012`/`W013` for a degraded run, or `E024` for a required one.
    diagnostics: Vec<Diagnostic>,
    /// Whether `diagnostics` contains a hard failure.
    fatal: bool,
}

/// Resolve the Cargo integration policy against the actual process environment.
fn cargo_environment(build: &Build) -> CargoEnvironment {
    let under_build_script = runtime_contract::under_build_script();
    let manifest = under_build_script
        .then(runtime_contract::manifest_from_env)
        .flatten();
    resolve_cargo_environment(build.cargo, under_build_script, manifest)
}

/// The policy decision itself, as a pure function of what the environment affords — so every
/// branch is reachable in a test without mutating process-global state.
fn resolve_cargo_environment(
    integration: CargoIntegration,
    under_build_script: bool,
    manifest: Option<Utf8PathBuf>,
) -> CargoEnvironment {
    match integration {
        CargoIntegration::Off => CargoEnvironment {
            directives: false,
            manifest: None,
            diagnostics: Vec::new(),
            fatal: false,
        },
        CargoIntegration::Auto | CargoIntegration::Required if under_build_script => {
            let diagnostics = match (&manifest, integration) {
                // A build script with no discoverable manifest cannot be audited, and an
                // un-audited build is exactly how `E023` reaches the user as a compile error in
                // generated code instead of a spargen diagnostic.
                (None, CargoIntegration::Required) => vec![cargo_diagnostic(
                    Severity::Error,
                    Code::CargoIntegrationRequired,
                    "cargo integration is required, but no consumer manifest was found;                      set CARGO_MANIFEST_DIR or use `CargoIntegration::Auto`",
                )],
                (None, _) => vec![cargo_diagnostic(
                    Severity::Warning,
                    Code::RuntimeAuditSkipped,
                    "no consumer manifest was found, so the runtime-dependency audit was                      skipped; run `spargen deps <spec>` for the dependencies generated output                      requires",
                )],
                (Some(_), _) => Vec::new(),
            };
            let fatal = diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error);
            CargoEnvironment {
                directives: true,
                manifest,
                diagnostics,
                fatal,
            }
        }
        CargoIntegration::Required => CargoEnvironment {
            directives: false,
            manifest: None,
            diagnostics: vec![cargo_diagnostic(
                Severity::Error,
                Code::CargoIntegrationRequired,
                "cargo integration is required, but this is not a build-script process;                  call `generate` from a `build.rs`, or relax to `CargoIntegration::Auto`",
            )],
            fatal: true,
        },
        CargoIntegration::Auto => CargoEnvironment {
            directives: false,
            manifest: None,
            diagnostics: vec![cargo_diagnostic(
                Severity::Warning,
                Code::CargoIntegrationDegraded,
                "not a build-script process: no rebuild triggers were emitted and the                  runtime-dependency audit was skipped; call `generate` from a `build.rs`, or                  silence this with `CargoIntegration::Off`",
            )],
            fatal: false,
        },
    }
}

/// A diagnostic about the build environment rather than the document: it has no meaningful
/// location in the spec, so it points at the document root.
fn cargo_diagnostic(severity: Severity, code: Code, message: &str) -> Diagnostic {
    let provenance = diag::Provenance::new(JsonPointer::root(), None);
    let builder = match severity {
        Severity::Error => Diagnostic::error(code, provenance),
        Severity::Warning => Diagnostic::warning(code, provenance),
    };
    builder.message(message.to_owned()).build()
}

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers.
///
/// When called from a build script, this also runs the consumer-manifest dependency audit, so
/// `E023` is catchable before a single line of code is generated. Elsewhere there is no consuming
/// package to audit — `spargen deps` prints the required `[dependencies]` block instead.
pub fn check(spec: &Spec) -> Report {
    let result = run_on_frontend_stack(|| {
        if spec.carve {
            run_carve(spec, PipelineMode::Requirements)
        } else {
            run_pipeline(spec, PipelineMode::Requirements)
        }
    });
    let mut report = result.report;
    let manifest = runtime_contract::under_build_script()
        .then(runtime_contract::manifest_from_env)
        .flatten();
    if let (Some(manifest), Some(requirements)) = (manifest, result.requirements) {
        let audit = runtime_contract::audit(&manifest, &requirements);
        if !audit.diagnostics.is_empty() {
            report.diagnostics.extend(audit.diagnostics);
            report.outcome = Outcome::Rejected;
        }
    }
    report
}

#[derive(Debug, Clone)]
struct PipelinePreview {
    report: Report,
    files: Vec<String>,
    requirements: Option<runtime_contract::RuntimeRequirements>,
}

fn preview_inner(spec: &Spec) -> PipelinePreview {
    let result = run_on_frontend_stack(|| {
        if spec.carve {
            run_carve(spec, PipelineMode::Preview)
        } else {
            run_pipeline(spec, PipelineMode::Preview)
        }
    });
    let files = result
        .plan
        .map(|plan| plan.files.into_iter().map(|file| file.contents).collect())
        .unwrap_or_default();
    PipelinePreview {
        report: result.report,
        files,
        requirements: result.requirements,
    }
}

/// The stack size for the dedicated frontend worker thread. Parsing, deserialization, meta-schema
/// validation, and lowering are all recursive over the (possibly deeply nested) document. Lowering
/// caps its own recursion at a fixed depth (`oas31::MAX_SCHEMA_DEPTH`) and the parser bounds nesting
/// too, so the peak stack is bounded — but only *this* thread guarantees it has room for that bound,
/// no matter how small the caller's own stack is. A build.rs, a CLI, a proptest worker, or a
/// libFuzzer target all inherit the same guarantee: no spec — however deep or adversarial — can
/// overflow the stack, because the recursive work always runs here.
const FRONTEND_STACK: usize = 64 * 1024 * 1024;

/// Run `f` (the whole recursive frontend/pipeline) on a dedicated thread with a large, fixed stack,
/// decoupling spargen's recursion budget from the caller's stack size. This is the mechanism behind
/// the no-overflow invariant: combined with the lowering depth cap, it makes stack exhaustion on any
/// input impossible for every entry point. A panic inside the worker is propagated to the caller
/// unchanged (so genuine bugs still surface). Thread creation is not input-driven; the only way it
/// fails is OS resource exhaustion, which is outside the "no input crashes the generator" contract.
fn run_on_frontend_stack<F, R>(f: F) -> R
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

/// The result of a [`diff`] run: the semver-impact report when both specs lowered, plus each
/// spec's rejection [`Report`] when it failed to lower.
///
/// `report` is `Some` iff **both** specs lowered successfully. When a spec rejects (used an R-class
/// construct, failed validation, …), its diagnostics are surfaced in `old_rejection` / `new_rejection`
/// and no diff is produced — the surfaces are simply not comparable. `diff` never panics on a bad spec.
#[derive(Debug, Clone)]
pub struct DiffRejection {
    old: Option<Report>,
    new: Option<Report>,
}

/// Which side of a diff a rejection came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The old (baseline) spec.
    Old,
    /// The new spec.
    New,
}

impl Side {
    /// The stable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Old => "old",
            Side::New => "new",
        }
    }
}

impl DiffRejection {
    /// The old spec's rejection report, if it failed to lower.
    pub fn old_spec(&self) -> Option<&Report> {
        self.old.as_ref()
    }

    /// The new spec's rejection report, if it failed to lower.
    pub fn new_spec(&self) -> Option<&Report> {
        self.new.as_ref()
    }

    /// Every side that failed to lower, with its report.
    pub fn rejections(&self) -> impl Iterator<Item = (Side, &Report)> + '_ {
        self.old
            .iter()
            .map(|report| (Side::Old, report))
            .chain(self.new.iter().map(|report| (Side::New, report)))
    }
}

impl std::fmt::Display for DiffRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (side, report) in self.rejections() {
            writeln!(formatter, "{} spec rejected:", side.as_str())?;
            write!(formatter, "{report}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DiffRejection {}

impl serde::Serialize for DiffRejection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (side, report) in self.rejections() {
            map.serialize_entry(side.as_str(), report)?;
        }
        map.end()
    }
}

/// Diff the **public API surface** of the client that would be generated from `old` versus `new`,
/// classifying the change as a semver bump (`major` breaking / `minor` additive / `patch` no-op).
///
/// Per the product contract, "the semver surface is the public API of generated output": this runs
/// the frontend (parse → lower → name-allocate) on both specs, models what a consumer of the
/// generated client sees (operations, their params/body/return types, and the public model types),
/// and reports every difference with its impact. A pure analysis step — it never writes output nor
/// touches the runtime. Deterministic: the same pair of specs yields a byte-identical report.
pub fn diff(old: &Spec, new: &Spec) -> Result<DiffReport, DiffRejection> {
    run_on_frontend_stack(|| diff_inner(old, new))
}

fn diff_inner(old: &Spec, new: &Spec) -> Result<DiffReport, DiffRejection> {
    let mut old_diags = diag::Diagnostics::new(old.batch_cap);
    let mut new_diags = diag::Diagnostics::new(new.batch_cap);
    let old_lowered = lower_frontend(old, &mut old_diags);
    let new_lowered = lower_frontend(new, &mut new_diags);

    match (old_lowered, new_lowered) {
        (Ok((old_api, old_names)), Ok((new_api, new_names))) => {
            let old_surface = surface::build(&old_api, &old_names);
            let new_surface = surface::build(&new_api, &new_names);
            Ok(surface::diff(&old_surface, &new_surface))
        }
        // A spec that failed to lower has no surface, so the two are simply not comparable.
        (old_lowered, new_lowered) => Err(DiffRejection {
            old: old_lowered
                .is_err()
                .then(|| report(old_diags, Outcome::Rejected)),
            new: new_lowered
                .is_err()
                .then(|| report(new_diags, Outcome::Rejected)),
        }),
    }
}

/// The filesystem paths generation reads for `spec`: the root spec, every relative-file `$ref`
/// target reachable from it, and each vendored remote copy.
///
/// Best-effort and side-effect-free: it loads the bundle only (no lowering, no output, no
/// network). If the spec cannot even be loaded (e.g. it is momentarily malformed mid-edit), the
/// returned list is just the spec path, so a watcher can still wait for it to be fixed.
/// Deterministic for a given on-disk state.
fn source_files(spec: &Spec) -> Vec<Utf8PathBuf> {
    let mut diags = diag::Diagnostics::new(spec.batch_cap);
    match source::InputBundle::load(&spec.path, &mut diags) {
        Ok(bundle) => {
            let mut paths: Vec<Utf8PathBuf> =
                bundle.source_paths().map(Utf8Path::to_path_buf).collect();
            if !paths.iter().any(|path| path == &spec.path) {
                paths.push(spec.path.clone());
            }
            paths
        }
        Err(_) => vec![spec.path.clone()],
    }
}

/// Private cross-crate bridge used exclusively by `spargen-macro`.
#[doc(hidden)]
pub mod __private {
    use camino::{Utf8Path, Utf8PathBuf};

    use super::{Outcome, Report, Spec};

    #[doc(hidden)]
    pub struct MacroPreview {
        pub report: Report,
        pub contents: Option<String>,
        pub source_files: Vec<Utf8PathBuf>,
    }

    #[doc(hidden)]
    pub fn preview(spec: &Spec) -> MacroPreview {
        preview_impl(spec, None)
    }

    #[doc(hidden)]
    pub fn preview_for_macro(spec: &Spec, manifest: &str) -> MacroPreview {
        preview_impl(spec, Some(Utf8Path::new(manifest)))
    }

    fn preview_impl(spec: &Spec, manifest: Option<&Utf8Path>) -> MacroPreview {
        let preview = super::preview_inner(spec);
        let contents = preview.files.first().cloned();
        let mut source_files = super::source_files(spec);
        let mut report = preview.report;
        if report.outcome == Outcome::Generated {
            if let (Some(manifest), Some(requirements)) = (manifest, preview.requirements.as_ref())
            {
                let audit = super::runtime_contract::audit(manifest, requirements);
                source_files.extend(audit.manifests);
                if !audit.diagnostics.is_empty() {
                    report.diagnostics.extend(audit.diagnostics);
                    report.outcome = Outcome::Rejected;
                }
            }
        }
        let spec_dir = spec
            .path
            .parent()
            .unwrap_or_else(|| camino::Utf8Path::new(""));
        let lock = spec_dir.join("spargen.lock");
        if lock.is_file() {
            source_files.push(lock);
        }
        source_files.sort();
        source_files.dedup();
        if report.outcome != Outcome::Generated {
            source_files.retain(|path| path == &spec.path);
        }
        MacroPreview {
            report,
            contents,
            source_files,
        }
    }
}

/// The exact `[dependencies]` block generated output from `spec` requires — what backs
/// `spargen deps`.
///
/// Generated output is freestanding, so the consuming package declares its own runtime
/// dependencies. Which ones depends on the API: multipart bodies pull in a `reqwest` feature,
/// `format: uuid` pulls in `uuid`, sequential responses pull in `futures-core`. Until now that
/// set could only be discovered reactively, one `E023` at a time; this returns all of it at once.
///
/// The spec must lower — a rejection is returned as the [`Report`] that explains why.
pub fn requirements(spec: &Spec) -> Result<Requirements, Report> {
    let result = run_on_frontend_stack(|| {
        if spec.carve {
            run_carve(spec, PipelineMode::Requirements)
        } else {
            run_pipeline(spec, PipelineMode::Requirements)
        }
    });
    match result.requirements {
        Some(requirements) => Ok(Requirements::new(&requirements)),
        None => Err(result.report),
    }
}

/// Extended documentation for a stable diagnostic code, backing `spargen explain E###` and the
/// published errors index.
pub fn explain(code: &str) -> Result<&'static str, UnknownCode> {
    Code::from_str(code).map(Code::explain)
}

/// Fetch, vendor, and hash-pin every remote (`http`/`https`) `$ref` reachable from `spec.path`,
/// writing copies under `.spargen/vendor/` and (re)writing `spargen.lock` next to the spec.
///
/// This is the **only** spargen entry point that performs network I/O — `generate` and `check`
/// resolve remote refs purely from the vendored, pinned copies this step produces, so builds stay
/// hermetic. Backed by `reqwest` and gated behind the `remote-fetch` feature (implied by `cli`).
#[cfg(feature = "remote-fetch")]
pub fn vendor(spec: &Spec) -> Result<VendorReport, Report> {
    let mut diags = diag::Diagnostics::new(spec.batch_cap);
    let fetcher = source::ReqwestFetcher;
    match source::vendor(&spec.path, &fetcher, &mut diags) {
        Ok(vendored) if !diags.has_errors() => Ok(vendored),
        _ => Err(report(diags, Outcome::Rejected)),
    }
}

#[derive(Debug, Clone, Copy)]
enum PipelineMode {
    /// Frontend audit only, no codegen (`spargen::check`).
    Check,
    /// Lower far enough to derive the runtime dependency set, no codegen (`spargen::requirements`).
    Requirements,
    /// Render in memory for the build cache or private proc-macro bridge.
    Preview,
}

/// The result of a pipeline run: the user-facing [`Report`] plus, when the run rendered code in
/// [`PipelineMode::Preview`], the in-memory [`emit::EmitPlan`]. `plan` is `None` for `check`,
/// On-disk writes and rejections retain no plan; only a preview keeps it.
struct PipelineResult {
    report: Report,
    plan: Option<emit::EmitPlan>,
    requirements: Option<runtime_contract::RuntimeRequirements>,
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
fn lower_frontend(
    spec: &Spec,
    diags: &mut diag::Diagnostics,
) -> Result<(ir::Api, name::Names), ()> {
    let mut bundle = source::InputBundle::load(&spec.path, diags).map_err(|_| ())?;

    if !spec.omit.is_empty() && spec.omit.apply(&mut bundle, diags).is_err() {
        return Err(());
    }

    let validator = oas31::MetaSchemaValidator::load_vendored();
    validator.validate(bundle.root(), diags);
    if diags.has_errors() {
        return Err(());
    }

    let document = oas31::parse_document(&bundle, diags).map_err(|_| ())?;

    let resolver = oas31::Resolver::new(&document, &bundle);
    oas31::audit(&document, &resolver, diags);
    if diags.has_errors() {
        return Err(());
    }

    // `check` runs the full frontend — lowering, IR invariants, and name allocation — so it fires
    // exactly the diagnostics `generate` would, just without emitting code.
    let api = oas31::lower(&document, &resolver, diags).map_err(|_| ())?;
    ir::check_invariants(&api, diags);
    if diags.has_errors() {
        return Err(());
    }

    let names = name::allocate(&api, diags);
    if diags.has_errors() {
        return Err(());
    }

    Ok((api, names))
}

/// Codegen + emit-planning for an already-lowered API: the shared tail of `generate` and
/// the internal preview. Returns the fully rendered module plan or `Err(())` with the layout error
/// already pushed onto `diags`.
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
            feature_uuid: spec.uuid,
            feature_time: spec.time,
            error_body_cap: spec.error_body_cap,
        },
        diags,
    );

    let emit_options = emit::EmitOptions {
        spec: emit::SpecMeta {
            source: if spec.omit.is_empty() {
                spec.path.to_string()
            } else {
                format!("{} omit={}", spec.path, spec.omit.fingerprint())
            },
            spargen_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    };

    emit::plan(&code, &emit_options).map_err(|error| {
        emit_pipeline_error(diags, error.to_string());
    })
}

fn run_pipeline(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    let mut diags = diag::Diagnostics::new(spec.batch_cap);

    let (api, names) = match lower_frontend(spec, &mut diags) {
        Ok(pair) => pair,
        Err(()) => return PipelineResult::bare(report(diags, Outcome::Rejected)),
    };
    let requirements = runtime_contract::RuntimeRequirements::for_api(&api, spec);

    match mode {
        PipelineMode::Check => return PipelineResult::bare(report(diags, Outcome::Clean)),
        PipelineMode::Requirements => {
            return PipelineResult {
                report: report(diags, Outcome::Clean),
                plan: None,
                requirements: Some(requirements),
            };
        }
        PipelineMode::Preview => {}
    }

    let plan = match build_emit_plan(spec, &api, &names, &mut diags) {
        Ok(plan) => plan,
        Err(()) => return PipelineResult::bare(report(diags, Outcome::Rejected)),
    };

    match mode {
        // Handled above; `plan` was never built for them.
        PipelineMode::Check | PipelineMode::Requirements => {
            unreachable!("check and requirements return before codegen")
        }
        // Preview keeps the rendered plan and writes nothing.
        PipelineMode::Preview => PipelineResult {
            report: report(diags, Outcome::Generated),
            plan: Some(plan),
            requirements: Some(requirements),
        },
    }
}

/// Auto-carve driver: iterate the frontend to a fixpoint, omitting the smallest enclosing
/// omittable construct for each rejection, then run `mode` for real with the carved omit set.
///
/// Each round runs the frontend (in `Check` mode — no output is written while carving) with the
/// current omit set. If it is not rejected, the carve converged and we run `mode` once with that
/// omit set (which re-applies every omit rule, emitting a `W009` for each carved construct — carving
/// is never silent). If it is rejected, we map the error pointers to omittable constructs
/// ([`compat::carve_rules`]) and add any *new* rules; when a round adds no new rule (an un-carvable
/// residual — a root/unmodelled rejection, or a rule that did not clear its error), we return that
/// round's report as-is: it already carries the `W009`s for what *was* carved plus the residual
/// error diagnostics, with `Outcome::Rejected`. Omitting a construct can dangle a `$ref` and surface
/// a fresh `E004`/`E020`; that new error is itself carved on the next round (its enclosing operation
/// is omitted) or, if un-carvable, reported honestly — the document is never emitted broken. The
/// round cap ([`compat::MAX_CARVE_ROUNDS`]) guarantees termination.
fn run_carve(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    let mut omit = spec.omit.clone();
    let mut last_rejection: Option<Report> = None;

    for _ in 0..compat::MAX_CARVE_ROUNDS {
        let probe = Spec {
            omit: omit.clone(),
            carve: false,
            // The carve mapper must see *every* error diagnostic to carve correctly, so the probe
            // runs with an unbounded batch (a spec has finitely many constructs). The user's
            // `batch_cap` still governs the final, user-facing report below.
            batch_cap: usize::MAX,
            ..spec.clone()
        };
        // The probe is always a `Check` run, so it never retains a plan.
        let report = run_pipeline(&probe, PipelineMode::Check).report;
        if report.outcome != Outcome::Rejected {
            // Converged: generate/preview/check for real with the carved omit set.
            let resolved = Spec {
                omit,
                carve: false,
                ..spec.clone()
            };
            return run_pipeline(&resolved, mode);
        }

        let new_rules: Vec<compat::OmitRule> = compat::carve_rules(&report.diagnostics)
            .into_iter()
            .filter(|rule| !omit.rules.contains(rule))
            .collect();
        if new_rules.is_empty() {
            // No progress possible — report the residual rejections (and any carved W009s) honestly.
            return PipelineResult::bare(report);
        }
        omit.rules.extend(new_rules);
        last_rejection = Some(report);
    }

    // Exhausted the round cap while still rejecting: return the last honest rejection report.
    match last_rejection {
        Some(report) => PipelineResult::bare(report),
        None => run_pipeline(spec, PipelineMode::Check),
    }
}

fn emit_pipeline_error(diags: &mut diag::Diagnostics, message: String) {
    diag::Diagnostic::error(
        Code::InvalidInput,
        diag::Provenance::new(JsonPointer::root(), None),
    )
    .message(message)
    .emit(diags);
}

fn pipeline_error_report(message: String) -> Report {
    let mut diagnostics = diag::Diagnostics::new(1);
    emit_pipeline_error(&mut diagnostics, message);
    report(diagnostics, Outcome::Rejected)
}

fn report(diags: diag::Diagnostics, outcome: Outcome) -> Report {
    Report {
        diagnostics: diags.items().to_vec(),
        outcome,
        truncated: diags.cap_reached(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(environment: &CargoEnvironment) -> Vec<&'static str> {
        environment
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    /// Build a report with no diagnostics and the given outcome.
    fn report(outcome: Outcome) -> Report {
        Report {
            diagnostics: Vec::new(),
            outcome,
            truncated: false,
        }
    }

    /// `wrote_output` distinguishes a run that rewrote the file from one that did not. `Cached` is
    /// the case that matters: it is a success, but nothing was written, and a build script keying
    /// off it would otherwise re-run work on every warm build.
    #[test]
    fn only_a_generated_outcome_reports_that_output_was_written() {
        assert!(Outcome::Generated.wrote_output());

        for quiet in [Outcome::Cached, Outcome::Clean, Outcome::Rejected] {
            assert!(
                !quiet.wrote_output(),
                "{quiet} must not claim it wrote output"
            );
        }

        // `wrote_output` is narrower than `is_success`: `Cached` and `Clean` are both successes
        // that wrote nothing, so one is never a substitute for the other.
        assert!(Outcome::Cached.is_success() && !Outcome::Cached.wrote_output());
        assert!(Outcome::Clean.is_success() && !Outcome::Clean.wrote_output());
    }

    /// `into_result` is the `?` form of `succeeded`, so it has to split on exactly what `succeeded`
    /// splits on — including an error-severity diagnostic under a non-`Rejected` outcome, which is
    /// the case a bare `outcome` check misses.
    #[test]
    fn into_result_agrees_with_succeeded_and_preserves_the_report() {
        let generated = report(Outcome::Generated);
        let diagnostics = generated.diagnostics().len();
        let recovered = generated.into_result().expect("a clean run is Ok");
        assert_eq!(recovered.outcome(), Outcome::Generated);
        assert_eq!(recovered.diagnostics().len(), diagnostics);

        // A rejection is Err, and the report survives so the caller can render it.
        let rejected = report(Outcome::Rejected)
            .into_result()
            .expect_err("a rejected run is Err");
        assert_eq!(rejected.outcome(), Outcome::Rejected);

        // Errors reported without a rejection still fail: `succeeded` checks both halves.
        let provenance = diag::Provenance::new(JsonPointer::root(), None);
        let with_error = Report {
            diagnostics: vec![
                Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance)
                    .message("an error under a non-rejecting outcome")
                    .build(),
            ],
            outcome: Outcome::Generated,
            truncated: false,
        };
        assert!(!with_error.succeeded());
        assert!(with_error.into_result().is_err());
    }

    /// A rejection announced as a warning looks like the diagnostics that do *not* stop
    /// generation. Each diagnostic goes out at its own severity.
    #[test]
    fn cargo_directives_carry_each_diagnostics_own_severity() {
        let provenance = diag::Provenance::new(JsonPointer::root(), None);
        let report = Report {
            diagnostics: vec![
                Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance.clone())
                    .message("a rejection")
                    .build(),
                Diagnostic::warning(Code::DeclarationHasNoEffect, provenance)
                    .message("a warning")
                    .build(),
            ],
            outcome: Outcome::Rejected,
            truncated: false,
        };

        let lines = report.cargo_directive_lines();
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert!(lines[0].starts_with("cargo::error="), "{lines:#?}");
        assert!(lines[1].starts_with("cargo::warning="), "{lines:#?}");
    }

    /// Every Cargo-integration branch, without touching process-global environment state.
    #[test]
    fn cargo_integration_policy_covers_each_environment() {
        let manifest = Some(Utf8PathBuf::from("Cargo.toml"));

        // A real build script: directives are emitted and the manifest is audited, silently.
        let ideal = resolve_cargo_environment(CargoIntegration::Auto, true, manifest.clone());
        assert!(ideal.directives);
        assert_eq!(ideal.manifest, manifest);
        assert!(codes(&ideal).is_empty());
        assert!(!ideal.fatal);

        // A build script with no discoverable manifest: nothing to audit, said out loud.
        let unauditable = resolve_cargo_environment(CargoIntegration::Auto, true, None);
        assert!(unauditable.directives);
        assert_eq!(codes(&unauditable), ["W012"]);
        assert!(!unauditable.fatal);

        // Not a build script: no directives, no audit — the degraded default.
        let degraded = resolve_cargo_environment(CargoIntegration::Auto, false, manifest.clone());
        assert!(!degraded.directives);
        assert!(degraded.manifest.is_none());
        assert_eq!(codes(&degraded), ["W013"]);
        assert!(!degraded.fatal);

        // The same, but the caller declared it must not happen.
        let required =
            resolve_cargo_environment(CargoIntegration::Required, false, manifest.clone());
        assert_eq!(codes(&required), ["E024"]);
        assert!(required.fatal);

        // Required *and* under a build script, but unauditable: also fatal, by request.
        let unauditable_required =
            resolve_cargo_environment(CargoIntegration::Required, true, None);
        assert_eq!(codes(&unauditable_required), ["E024"]);
        assert!(unauditable_required.fatal);

        // Off says nothing in any environment — that is the whole point of it.
        for under_build_script in [true, false] {
            let off = resolve_cargo_environment(
                CargoIntegration::Off,
                under_build_script,
                manifest.clone(),
            );
            assert!(!off.directives);
            assert!(off.manifest.is_none());
            assert!(codes(&off).is_empty());
            assert!(!off.fatal);
        }
    }
}
