//! # spargen
//!
//! A compile-time-correct Rust HTTP client generator for OpenAPI 3.1.x and 3.2.x. Spec in, spar out:
//! everything structural is decided at generation time; nothing is interpreted at runtime.
//!
//! This crate is the library half of the `spargen` tool. Its public surface is the `build.rs`
//! API: the [crate-root](crate) items, starting from [`Build`] and [`generate`].
//!
//! ## Subsystem layering
//!
//! The crate is internally partitioned into subsystems with a declared dependency DAG. Each
//! subsystem module records its allowed dependencies in a machine-readable `//! layer-deps:`
//! header; `spargen/tests/layering/` diffs those declarations against the inter-module edges the
//! production (non-test) code actually takes, fails on any edge not in the table below, and
//! checks that the declared graph is acyclic.
//!
//! | Subsystem | May depend on |
//! |-----------|---------------|
//! | `diag`    | —             |
//! | `source`  | `diag`        |
//! | `ir`      | `diag`        |
//! | `oas31`   | `source`, `ir`, `name`, `diag` |
//! | `name`    | `ir`, `diag`  |
//! | `support` | — (an `include_str!` handle to `support-runtime`'s sources, which compile standalone) |
//! | `codegen` | `ir`, `name`, `support` |
//! | `emit`    | `codegen`     |
//! | `compat`  | `source`, `diag` |
//! | `surface` | `ir`, `name`  |
//! | `cli`     | facade        |
//! | facade (`lib.rs`) | all of the above |
//!
//! Pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`, with `diag` as the
//! lowest layer, which the stages that report diagnostics reach and which depends on nothing, and
//! `ir` as the lowered model `oas31` produces and `name`, `codegen` and `surface` read. `compat`
//! preprocesses the bundle before `oas31` sees it; `surface` reads the lowered API for `diff` and
//! never feeds generation.
//!
//! `cache`, `config`, and `runtime_contract` are facade plumbing rather than subsystems: they say
//! so in their own module docs and carry no `layer-deps:` header, so the lint skips them.
//! `test_support`, compiled only into the test build, holds helpers the subsystems' in-crate tests
//! share; the lint admits it by name as neither.

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
#[cfg(test)]
mod test_support;

use std::str::FromStr;

use camino::{Utf8Path, Utf8PathBuf};

pub use compat::{ComponentKind, Omit, OmitMethod, OmitRule, UnknownOmitToken};
pub use config::{Build, CargoIntegration, ConfigError, Spec};
pub use diag::{
    Code, Diagnostic, DiagnosticBuilder, FileId, InterpId, JsonPointer, Loc, OutcomeClaim,
    Provenance, Severity, Span, UnknownCode,
};
pub use runtime_contract::{RequiredDependency, Requirements};
#[cfg(feature = "remote-fetch")]
pub use source::{VendorReport, VendoredRef};
pub use surface::{Change, ChangeKind, DiffReport, Impact};

/// The outcome of a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Outcome {
    /// The generated module was rendered by this run rather than replayed from the build cache.
    /// The output file is rewritten only when its bytes change.
    Generated,
    /// The generated module was already up to date and was verified from the build cache.
    Cached,
    /// The support audit completed without a rejection.
    Clean,
    /// An error-severity diagnostic was emitted, so the run did not get as far as it was asked
    /// to.
    Rejected,
}

impl Outcome {
    /// The stable lowercase string form. [`Display`](std::fmt::Display) writes it, and the
    /// serialized form (`#[serde(rename_all = "lowercase")]`, which `--format json` emits) spells
    /// each variant the same way.
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

    /// Whether this run rendered the output rather than replaying it from the build cache. A
    /// rendered output is written only when its bytes differ from the file already on disk.
    pub fn wrote_output(self) -> bool {
        matches!(self, Outcome::Generated)
    }

    /// Whether a diagnostic that makes `claim` about its run can be true on a run with this
    /// outcome.
    ///
    /// A diagnostic's [`Diagnostic::claim`] records what its message asserts. Every diagnostic a
    /// run reports must make a claim its outcome admits, through `check` and through `generate`
    /// alike. Any other combination is a message that says something false about the run.
    pub fn admits(self, claim: OutcomeClaim) -> bool {
        match claim {
            OutcomeClaim::Independent => true,
            OutcomeClaim::Rejected => self == Outcome::Rejected,
            OutcomeClaim::Generated => matches!(self, Outcome::Generated | Outcome::Cached),
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The result of a pipeline run: the collected diagnostics plus the outcome.
///
/// Fields are private and read through accessors, so the shape can grow without a breaking
/// change. [`Spec`], [`Build`], and [`DiffRejection`] keep their fields private for the same
/// reason: [`Spec`] is set through its builder methods, with [`Spec::path`] reading back its
/// root document, and [`Build`] and [`DiffRejection`] are read through accessors.
/// [`Diagnostic`] grows the other way: its fields are public and the struct is
/// `#[non_exhaustive]`.
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
    /// A rejection carrying `diagnostics`, `truncated` when the batch that produced them reached
    /// its cap.
    fn rejected(diagnostics: Vec<Diagnostic>, truncated: bool) -> Self {
        Self {
            diagnostics,
            outcome: Outcome::Rejected,
            truncated,
        }
    }

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
    ///
    /// A directive is one line, and Cargo shows a build script's other stdout only under `-vv`,
    /// so every line of a diagnostic's rendering (its location, pointer, and `help:` remedy
    /// included) goes out as its own directive at that diagnostic's severity. A truncated report
    /// ends with the truncation notice as a `cargo::warning=`.
    pub fn emit_cargo_diagnostics(&self) {
        for line in self.cargo_directive_lines() {
            println!("{line}");
        }
    }

    /// The directive lines [`Self::emit_cargo_diagnostics`] prints, split out so the severity
    /// mapping is testable without capturing the process's stdout.
    fn cargo_directive_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for diagnostic in &self.diagnostics {
            let directive = match diagnostic.severity {
                Severity::Error => "cargo::error=",
                Severity::Warning => "cargo::warning=",
            };
            let rendered = diagnostic.to_string();
            lines.extend(rendered.lines().map(|line| format!("{directive}{line}")));
        }
        if self.truncated {
            lines.push(format!("cargo::warning={}", self.truncation_notice()));
        }
        lines
    }

    /// The line saying the report stops at `batch_cap`, shared by [`Report`]'s `Display` and
    /// [`Self::emit_cargo_diagnostics`] so the two cannot drift.
    fn truncation_notice(&self) -> String {
        format!(
            "spargen: diagnostic list truncated at batch_cap ({} shown); raise batch_cap to see the rest",
            self.diagnostics.len()
        )
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
            writeln!(formatter, "{}", self.truncation_notice())?;
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
    let cargo = runtime_contract::cargo_environment(build);
    let mut cargo_diagnostics = cargo.diagnostics;
    if cargo.fatal {
        return Report::rejected(cargo_diagnostics, false);
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
            return Report::rejected(cargo_diagnostics, truncated);
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
            if let Some(cached) = cached {
                cargo_diagnostics.extend(cached.diagnostics);
                // A verified cache hit rewrote nothing, so the outcome is `Cached`, not
                // `Generated` — `wrote_output` must stay true to what happened.
                let mut report = Report {
                    diagnostics: cargo_diagnostics,
                    outcome: Outcome::Cached,
                    truncated: cached.truncated,
                };
                if let Some(manifest) = consumer_manifest.as_deref() {
                    let audited = apply_audit(&mut report, manifest, &cached.requirements);
                    if emit_cargo_directives {
                        runtime_contract::cargo_directives(&audited.manifests);
                    }
                }
                // A replay rejects on any error-severity diagnostic, the audit's or the replayed
                // run's.
                if report.has_errors() {
                    report.outcome = Outcome::Rejected;
                }
                return report;
            }
        }
    }

    for _ in 0..3 {
        let mut preview = preview_inner(spec);
        if preview.report.outcome != Outcome::Generated {
            let mut report = preview.report;
            prepend(&mut report.diagnostics, &cargo_diagnostics);
            return report;
        }
        let Some(rendered) = preview.contents.as_deref() else {
            return pipeline_error_report("generation produced no module".to_owned());
        };
        let Some(requirements) = preview.requirements.as_ref() else {
            return pipeline_error_report("generation produced no runtime requirements".to_owned());
        };
        if let Some(manifest) = consumer_manifest.as_deref() {
            let audited = apply_audit(&mut preview.report, manifest, requirements);
            if emit_cargo_directives {
                runtime_contract::cargo_directives(&audited.manifests);
            }
            // A fresh run rejects on any audit finding, before anything is written. The inner
            // run's truncation carries: merging its diagnostics into a wider list does not make
            // the dropped ones reappear.
            if audited.found {
                let mut report = preview.report;
                report.outcome = Outcome::Rejected;
                prepend(&mut report.diagnostics, &cargo_diagnostics);
                return report;
            }
        }

        let after = match run_on_frontend_stack(|| cache::InputSnapshot::load(spec)) {
            Ok(snapshot) => snapshot,
            Err(diagnostics) => {
                let truncated = diagnostics.cap_reached();
                cargo_diagnostics.extend(diagnostics.items().to_vec());
                return Report::rejected(cargo_diagnostics, truncated);
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
                preview.report.truncated,
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

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers.
///
/// When called from a build script, this also runs the consumer-manifest dependency audit, so
/// `E023` is catchable before a single line of code is generated. Elsewhere there is no consuming
/// package to audit — `spargen deps` prints the dependency tables generated output
/// requires instead.
pub fn check(spec: &Spec) -> Report {
    let result = run_mode(spec, PipelineMode::Requirements);
    let mut report = result.report;
    let manifest = runtime_contract::build_script_manifest();
    if let (Some(manifest), Some(requirements)) = (manifest, result.requirements) {
        // `check` rejects on any audit finding.
        if apply_audit(&mut report, &manifest, &requirements).found {
            report.outcome = Outcome::Rejected;
        }
    }
    report
}

/// What [`apply_audit`] merged into a report.
struct Audited {
    /// Every manifest the audit read, for rebuild tracking.
    manifests: Vec<Utf8PathBuf>,
    /// Whether the audit reported anything.
    found: bool,
}

/// Run the consumer-manifest dependency audit (`E023`) of `manifest` against `requirements`, and
/// append its diagnostics to `report`'s, after the run's own. The outcome is left alone: each
/// caller states its own reject rule beside the call.
fn apply_audit(
    report: &mut Report,
    manifest: &Utf8Path,
    requirements: &runtime_contract::RuntimeRequirements,
) -> Audited {
    let audit = runtime_contract::audit(manifest, requirements);
    let found = !audit.diagnostics.is_empty();
    report.diagnostics.extend(audit.diagnostics);
    Audited {
        manifests: audit.manifests,
        found,
    }
}

#[derive(Debug, Clone)]
struct PipelinePreview {
    report: Report,
    contents: Option<String>,
    requirements: Option<runtime_contract::RuntimeRequirements>,
}

fn preview_inner(spec: &Spec) -> PipelinePreview {
    let result = run_mode(spec, PipelineMode::Preview);
    PipelinePreview {
        report: result.report,
        contents: result.plan.map(|plan| plan.contents),
        requirements: result.requirements,
    }
}

/// The stack size for the dedicated frontend worker thread. Parsing, deserialization, meta-schema
/// validation, and lowering are all recursive over the (possibly deeply nested) document. Lowering
/// caps its own recursion at a fixed depth (`oas31::MAX_SCHEMA_DEPTH`), and the JSON and YAML
/// parsers both refuse a document nested past a fixed depth before recursing past it, an alias's
/// copy counted at the depth it reaches, so the peak stack is bounded — but only *this* thread
/// guarantees it has room for that bound, no matter how small the caller's own stack is. A
/// build.rs, a CLI, a proptest worker, or a libFuzzer target all inherit the same guarantee: no
/// spec — however deep or adversarial — can overflow the stack, because the recursive work always
/// runs here.
const FRONTEND_STACK: usize = 64 * 1024 * 1024;

/// Run `f` (the whole recursive frontend/pipeline) on a dedicated thread with a large, fixed stack,
/// decoupling spargen's recursion budget from the caller's stack size. This is the mechanism behind
/// the no-overflow invariant: combined with the parsers' and lowering's depth caps, it makes stack
/// exhaustion on any input impossible for every entry point. A panic inside the worker is
/// propagated to the caller unchanged (so genuine bugs still surface). Thread creation is not
/// input-driven; the only way it fails is OS resource exhaustion, which is outside the "no input
/// crashes the generator" contract.
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

/// The `Err` of a [`diff`] run: at least one spec failed to lower, so the two surfaces are not
/// comparable and no [`DiffReport`] is produced.
///
/// Each side that failed to lower carries its rejection [`Report`], read through
/// [`old_spec`](Self::old_spec), [`new_spec`](Self::new_spec), or [`rejections`](Self::rejections)
/// for every failed side in turn. `diff` never panics on a bad spec.
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
    match (lower_side(old), lower_side(new)) {
        (Ok((old_api, old_names)), Ok((new_api, new_names))) => {
            let old_surface = surface::build(&old_api, &old_names);
            let new_surface = surface::build(&new_api, &new_names);
            Ok(surface::diff(&old_surface, &new_surface))
        }
        // A spec that failed to lower has no surface, so the two are simply not comparable.
        (old_lowered, new_lowered) => Err(DiffRejection {
            old: old_lowered.err(),
            new: new_lowered.err(),
        }),
    }
}

/// Lower one side of a [`diff`]: through the carve driver when `spec.carve` asks for it, as
/// [`run_mode`] does for every other entry point, and the plain frontend otherwise. A side that
/// does not lower returns its rejection [`Report`].
fn lower_side(spec: &Spec) -> Result<(ir::Api, name::Names), Report> {
    let carved;
    let spec = if spec.carve {
        carved = carve_spec(spec)?;
        &carved
    } else {
        spec
    };
    let mut diags = diag::Diagnostics::new(spec.batch_cap);
    lower_frontend(spec, &mut diags).map_err(|()| report(diags, Outcome::Rejected))
}

/// Private cross-crate bridge for `spargen-macro`; this crate's integration tests also call
/// `preview` and `preview_for_macro`.
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
        let contents = preview.contents;
        // The files a generation reads are the build cache's inputs: the root spec, every
        // relative-file `$ref` target and vendored remote copy, `spargen.lock`, and the applied
        // config files, whose edit changes what was generated and so re-expands. An input set that
        // does not load (a spec malformed mid-edit) leaves the root spec alone, so the macro still
        // waits for it to be fixed.
        let mut source_files = match super::cache::InputSnapshot::load(spec) {
            Ok(snapshot) => snapshot.paths,
            Err(_) => vec![spec.path.clone()],
        };
        source_files.push(spec.path.clone());
        let mut report = preview.report;
        if report.outcome == Outcome::Generated {
            if let (Some(manifest), Some(requirements)) = (manifest, preview.requirements.as_ref())
            {
                let audited = super::apply_audit(&mut report, manifest, requirements);
                source_files.extend(audited.manifests);
                // The macro rejects on any audit finding.
                if audited.found {
                    report.outcome = Outcome::Rejected;
                }
            }
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

/// The dependency tables generated output from `spec` requires — what backs `spargen deps`.
///
/// Generated output is freestanding, so the consuming package declares its own runtime
/// dependencies. Which ones depends on the API: multipart bodies pull in a `reqwest` feature,
/// `format: uuid` pulls in `uuid`, sequential responses pull in `futures-core`. This returns the
/// whole set before a build, rather than leaving `E023` to report what the consuming package
/// lacks.
///
/// The spec must lower — a rejection is returned as the [`Report`] that explains why.
pub fn requirements(spec: &Spec) -> Result<Requirements, Report> {
    let result = run_mode(spec, PipelineMode::Requirements);
    match result.requirements {
        Some(requirements) => Ok(Requirements::new(&requirements)),
        None => Err(result.report),
    }
}

/// Extended documentation for a stable diagnostic code, backing `spargen explain E###`. The
/// published errors index, `docs/errors.md`, lists each code's severity and title.
pub fn explain(code: &str) -> Result<&'static str, UnknownCode> {
    Code::from_str(code).map(Code::explain)
}

/// Fetch, vendor, and hash-pin every remote (`http`/`https`) `$ref` reachable from `spec.path`,
/// writing copies under `.spargen/vendor/` and (re)writing `spargen.lock` next to the spec.
///
/// This step fetches each remote document over HTTP(S) through `reqwest`; `generate` and `check`
/// resolve remote refs purely from the vendored, pinned copies it produces, so builds stay
/// hermetic. Gated behind the `remote-fetch` feature (implied by `cli`).
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
    /// Lower far enough to derive the runtime dependency set, no codegen (`spargen::check` and
    /// `spargen::requirements`).
    Requirements,
    /// Render in memory for the build cache or private proc-macro bridge.
    Preview,
}

/// The result of a pipeline run: the user-facing [`Report`], the runtime requirements of an API
/// that lowered, and, when the run rendered code in [`PipelineMode::Preview`], the in-memory
/// [`emit::EmitPlan`]. A [`PipelineMode::Requirements`] run and a rejection carry no plan.
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
    // exactly the diagnostics `generate` would, just without emitting code.
    let options = oas31::LowerOptions {
        open_narrowing: spec.open_narrowing,
    };
    let api = oas31::lower(&document, &resolver, diags, options).map_err(|_| ())?;
    ir::check_invariants(&api, diags);
    if diags.has_errors() {
        return Err(());
    }

    // Allocation is total: every API that lowered gets its names.
    let names = name::allocate(&api);
    Ok((api, names))
}

/// Codegen + emit-planning for an already-lowered API: the shared tail of `generate` and
/// the internal preview. Returns the fully rendered module plan or `Err(())` with the codegen
/// error already pushed onto `diags` as `E011`, so nothing is written or cached.
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
    )
    .map_err(|error| emit_pipeline_error(diags, error.to_string()))?;

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

    Ok(emit::plan(&code, &emit_options))
}

/// Run `mode` over `spec` on the dedicated frontend stack, through the auto-carve driver when
/// `spec.carve` asks for it and the plain pipeline otherwise. Every public entry point that runs
/// the pipeline dispatches here; [`diff`], which lowers two specs and runs no mode, makes the same
/// decision per side in [`lower_side`], through the same [`carve_spec`] fixpoint.
fn run_mode(spec: &Spec, mode: PipelineMode) -> PipelineResult {
    run_on_frontend_stack(|| {
        if spec.carve {
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
        Err(()) => return PipelineResult::bare(report(diags, Outcome::Rejected)),
    };
    let requirements = runtime_contract::RuntimeRequirements::for_api(&api, spec);

    match mode {
        // Requirements stops before codegen.
        PipelineMode::Requirements => PipelineResult {
            report: report(diags, Outcome::Clean),
            plan: None,
            requirements: Some(requirements),
        },
        // Preview keeps the rendered plan and writes nothing.
        PipelineMode::Preview => match build_emit_plan(spec, &api, &names, &mut diags) {
            Ok(plan) => PipelineResult {
                plan: Some(plan),
                report: report(diags, Outcome::Generated),
                requirements: Some(requirements),
            },
            Err(()) => PipelineResult::bare(report(diags, Outcome::Rejected)),
        },
    }
}

/// Auto-carve driver: iterate the frontend to a fixpoint, omitting the smallest enclosing
/// omittable construct for each rejection, then run `mode` for real with the carved omit set.
///
/// Each round runs the frontend alone (no codegen, and no output is written while carving) with the
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
    match carve_spec(spec) {
        Ok(resolved) => run_pipeline(&resolved, mode),
        Err(report) => PipelineResult::bare(report),
    }
}

/// The carve fixpoint [`run_carve`] describes, without the final run: `spec` with `carve` off and
/// the carved omit set in place of its own once the frontend lowers under it, or the rejection
/// [`Report`] of the round that could make no progress (or of the last round, at the cap).
fn carve_spec(spec: &Spec) -> Result<Spec, Report> {
    let mut omit = spec.omit.clone();
    let mut last_rejection: Option<Report> = None;

    for _ in 0..compat::MAX_CARVE_ROUNDS {
        let probe = Spec {
            omit: omit.clone(),
            carve: false,
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
                carve: false,
                ..spec.clone()
            });
        }
        let report = report(diags, Outcome::Rejected);

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
            carve: false,
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

    /// Every outcome, in declaration order.
    const OUTCOMES: [Outcome; 4] = [
        Outcome::Generated,
        Outcome::Cached,
        Outcome::Clean,
        Outcome::Rejected,
    ];

    /// `admits` is the oracle every diagnostic's claim is checked against, so its whole truth
    /// table is pinned: an independent claim holds on every run, a rejection claim only on a
    /// rejected one, and a generation claim on both outcomes that leave a generated module.
    #[test]
    fn every_outcome_admits_exactly_the_claims_true_of_it() {
        for outcome in OUTCOMES {
            assert!(outcome.admits(OutcomeClaim::Independent), "{outcome}");
            assert_eq!(
                outcome.admits(OutcomeClaim::Rejected),
                outcome == Outcome::Rejected,
                "{outcome}"
            );
            assert_eq!(
                outcome.admits(OutcomeClaim::Generated),
                matches!(outcome, Outcome::Generated | Outcome::Cached),
                "{outcome}"
            );
        }
    }

    /// `as_str`, `Display`, and the serialized form (what `--format json` emits) spell each
    /// outcome alike, pinned outcome by outcome.
    #[test]
    fn each_outcome_renders_its_stable_lowercase_name() {
        for (outcome, name) in
            OUTCOMES
                .into_iter()
                .zip(["generated", "cached", "clean", "rejected"])
        {
            assert_eq!(outcome.as_str(), name);
            assert_eq!(outcome.to_string(), name);
            assert_eq!(serde_json::to_value(outcome).unwrap(), name);
        }
    }

    /// `errors` and `warnings` split `diagnostics` by severity with nothing lost or shared, each
    /// in emission order, and `has_errors` follows the error side alone.
    #[test]
    fn errors_and_warnings_partition_the_diagnostics_by_severity() {
        let provenance = diag::Provenance::new(JsonPointer::root(), None);
        let warning = |message: &str| {
            Diagnostic::warning(Code::ServerInitiatedFlowIgnored, provenance.clone())
                .message(message)
                .build()
        };
        let error = |message: &str| {
            Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance.clone())
                .message(message)
                .build()
        };
        let mixed = Report {
            diagnostics: vec![
                warning("first warning"),
                error("first error"),
                warning("second warning"),
                error("second error"),
            ],
            outcome: Outcome::Rejected,
            truncated: false,
        };

        let messages = |diagnostics: Vec<&Diagnostic>| {
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            messages(mixed.errors().collect()),
            ["first error", "second error"]
        );
        assert_eq!(
            messages(mixed.warnings().collect()),
            ["first warning", "second warning"]
        );
        assert_eq!(
            mixed.errors().count() + mixed.warnings().count(),
            mixed.diagnostics().len()
        );
        assert!(mixed.has_errors());

        // Warnings alone are not errors, whatever the outcome.
        let warned = Report {
            diagnostics: vec![warning("only a warning")],
            outcome: Outcome::Generated,
            truncated: false,
        };
        assert!(!warned.has_errors());
        assert_eq!(warned.errors().count(), 0);
        assert_eq!(warned.warnings().count(), 1);
        assert!(!report(Outcome::Rejected).has_errors());
    }

    /// `expect_success` hands a successful report back unchanged.
    #[test]
    fn expect_success_returns_a_successful_report() {
        let returned = report(Outcome::Cached).expect_success();
        assert_eq!(returned.outcome(), Outcome::Cached);
    }

    /// `expect_success` panics with the rendered report, whose first line names the outcome.
    #[test]
    #[should_panic(expected = "spargen: rejected")]
    fn expect_success_panics_on_a_rejected_report() {
        report(Outcome::Rejected).expect_success();
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
    /// generation. Each diagnostic goes out at its own severity, and so does every line of it:
    /// Cargo shows a build script's non-directive stdout only under `-vv`, so an unprefixed
    /// location or `help:` line would be invisible in an ordinary build. A truncated report also
    /// says so, as a warning.
    #[test]
    fn cargo_directives_carry_each_diagnostics_own_severity() {
        let span = diag::Span::point(
            diag::FileId(0),
            diag::Loc {
                line: 3,
                col: 5,
                offset: 20,
            },
        );
        let provenance =
            diag::Provenance::new(JsonPointer::root().push("paths").push("/pets"), Some(span));
        let report = Report {
            diagnostics: vec![
                Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance.clone())
                    .message("a rejection")
                    .remedy("fix the rejection")
                    .build(),
                Diagnostic::warning(Code::ServerInitiatedFlowIgnored, provenance)
                    .message("a warning")
                    .remedy("fix the warning")
                    .build(),
            ],
            outcome: Outcome::Rejected,
            truncated: true,
        };

        let lines = report.cargo_directive_lines();
        assert_eq!(
            lines,
            [
                "cargo::error=error[E001]: a rejection",
                "cargo::error=  --> 3:5",
                "cargo::error=  at /paths/~1pets",
                "cargo::error=  help: fix the rejection",
                "cargo::warning=warning[W002]: a warning",
                "cargo::warning=  --> 3:5",
                "cargo::warning=  at /paths/~1pets",
                "cargo::warning=  help: fix the warning",
                "cargo::warning=spargen: diagnostic list truncated at batch_cap (2 shown); raise batch_cap to see the rest",
            ],
            "{lines:#?}"
        );
        for line in &lines {
            assert!(!line.contains('\n'), "a directive is one line: {line:?}");
        }
    }

    /// `check` with auto-carve on, over `contents` written as the root document in `dir`.
    fn carve_check(dir: &tempfile::TempDir, contents: &str) -> Report {
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, contents).unwrap();
        check(&Spec::new(Utf8PathBuf::from_path_buf(path).unwrap()).carve(true))
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

    /// The round cap is exact: a chain carve clears in [`compat::MAX_CARVE_ROUNDS`] rounds lowers,
    /// and one link longer ends `Rejected` with the last round's report — the link it carved last
    /// as `W009`s, and the one its final rule dangled as the residual `E004`.
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
