//! # spargen
//!
//! A compile-time-correct Rust HTTP client generator for OpenAPI 3.1.x and 3.2.x. Spec in, spar out:
//! everything structural is decided at generation time; nothing is interpreted at runtime.
//!
//! This crate is the library half of the `spargen` tool. Its public surface is the `build.rs`
//! API — see the [facade](crate) items ([`Config`], [`generate`], [`check`], [`explain`]).
//!
//! ## Subsystem layering
//!
//! The crate is internally partitioned into subsystems with a declared dependency DAG. Each
//! subsystem module records its allowed dependencies in a machine-readable `//! layer-deps:`
//! header; the future `xtask lint-layers` job diffs those declarations against the actual
//! inter-module `use` edges and fails on any edge not in the table below.
//!
//! | Subsystem | May depend on |
//! |-----------|---------------|
//! | `diag`    | —             |
//! | `source`  | `diag`        |
//! | `ir`      | `diag`        |
//! | `oas31`   | `source`, `ir`, `diag` |
//! | `name`    | `ir`, `diag`  |
//! | `support` | — (compiles standalone against reqwest/serde) |
//! | `codegen` | `ir`, `name`, `support`, `diag` |
//! | `emit`    | `codegen`, `diag` |
//! | `cli`     | facade        |
//! | facade (`lib.rs`) | all of the above |
//!
//! Pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`, with `diag` as the
//! only vocabulary shared across stages.

mod diag;

mod cache;
mod codegen;
mod compat;
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
pub use diag::{Code, Diagnostic, FileId, InterpId, JsonPointer, Loc, Severity, Span, UnknownCode};
#[cfg(feature = "remote-fetch")]
pub use source::{VendorReport, VendoredRef};
pub use surface::{Change, ChangeKind, DiffReport, Impact};

/// Feature toggles for the generated output (both default **on**). Disabling one falls
/// back to `String` for the corresponding `format` mappings — a deliberate, documented loss of
/// typing for size-critical builds.
#[derive(Debug, Clone)]
pub struct Features {
    /// Map `format: uuid` to `uuid::Uuid`.
    pub uuid: bool,
    /// Map `format: date-time`/`date` to the `time` crate.
    pub time: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            uuid: true,
            time: true,
        }
    }
}

/// Configuration for one generation run — the primary `build.rs` input. Construct with
/// [`Config::new`] and adjust fields as needed.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Where to write generated code.
    pub output: Utf8PathBuf,
    /// Generated-output feature toggles.
    pub features: Features,
    /// Explicit compatibility omissions applied before OpenAPI validation/lowering.
    pub omit: Omit,
    /// Max bytes of a response body retained on error variants (default 64 KiB).
    pub error_body_cap: usize,
    /// Max diagnostics collected before batching stops.
    pub batch_cap: usize,
    /// Auto-carve: instead of failing on rejections, iteratively omit the minimal enclosing
    /// omittable construct for each rejection and generate the rest (`--carve`). Every carved
    /// construct is reported via `W009`; residual, un-carvable rejections are reported honestly.
    pub carve: bool,
}

impl Config {
    /// A config with sensible defaults: features on, 64 KiB error-body cap, a bounded diagnostic
    /// batch, writing enabled.
    pub fn new(spec: impl Into<Utf8PathBuf>, output: impl Into<Utf8PathBuf>) -> Self {
        Self {
            spec: spec.into(),
            output: output.into(),
            features: Features::default(),
            omit: Omit::default(),
            error_body_cap: 64 * 1024,
            batch_cap: 100,
            carve: false,
        }
    }
}

/// The outcome of a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    /// Every diagnostic emitted during the run (batch reporting).
    pub diagnostics: Vec<Diagnostic>,
    /// What happened.
    pub outcome: Outcome,
}

impl Report {
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

    /// Print every diagnostic as a `cargo:warning=` line, so they surface in a build's output.
    pub fn emit_cargo_warnings(&self) {
        for diagnostic in &self.diagnostics {
            println!("cargo:warning={diagnostic}");
        }
    }

    /// Panic with the rendered report unless the run succeeded. The build-script one-liner.
    #[must_use]
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
        Ok(())
    }
}

impl std::error::Error for Report {}

/// Run the full pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`. The
/// primary `build.rs` entry point.
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines.
/// let config = spargen::Config::new(
///     "api/openapi.yaml",
///     "src/api.rs",
/// );
/// let report = spargen::generate(&config);
/// println!("cargo:warning=spargen outcome: {:?}", report.outcome);
/// ```
pub fn generate(config: &Config) -> Report {
    let cache_dir = cache::cache_dir();
    generate_with_cache_dir(config, cache_dir.as_deref(), cache_dir.is_some())
}

fn generate_with_cache_dir(
    config: &Config,
    cache_dir: Option<&Utf8Path>,
    emit_cargo_directives: bool,
) -> Report {
    let consumer_manifest = runtime_contract::build_script_manifest();
    let mut snapshot = match run_on_frontend_stack(|| cache::InputSnapshot::load(config)) {
        Ok(snapshot) => snapshot,
        Err(diagnostics) => {
            if emit_cargo_directives {
                cache::cargo_directives(config, None);
            }
            return Report {
                diagnostics,
                outcome: Outcome::Rejected,
            };
        }
    };
    if emit_cargo_directives {
        cache::cargo_directives(config, Some(&snapshot));
    }

    let cache_path = cache_dir.map(|dir| cache::cache_path(dir, &config.output));
    if cache_dir.is_some() {
        if let Some(content_digest) = cache::verified_output(&config.output, &snapshot.digest) {
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
                    if cached
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.severity == Severity::Error)
                    {
                        return Report {
                            diagnostics: cached.diagnostics,
                            outcome: Outcome::Rejected,
                        };
                    }
                }
                // A verified cache hit did not rewrite anything, and saying `Generated` made
                // `wrote_output` a lie.
                return Report {
                    diagnostics: cached.diagnostics,
                    outcome: Outcome::Cached,
                };
            }
        }
    }

    for _ in 0..3 {
        let preview = preview_inner(config);
        if preview.report.outcome != Outcome::Generated {
            return preview.report;
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
                let mut diagnostics = preview.report.diagnostics;
                diagnostics.extend(audit.diagnostics);
                return Report {
                    diagnostics,
                    outcome: Outcome::Rejected,
                };
            }
        }

        let after = match run_on_frontend_stack(|| cache::InputSnapshot::load(config)) {
            Ok(snapshot) => snapshot,
            Err(diagnostics) => {
                return Report {
                    diagnostics,
                    outcome: Outcome::Rejected,
                };
            }
        };
        if snapshot.digest != after.digest {
            snapshot = after;
            if emit_cargo_directives {
                cache::cargo_directives(config, Some(&snapshot));
            }
            continue;
        }

        let (contents, content_digest) = cache::finalized(rendered, &snapshot.digest);
        if let Err(message) = cache::write_output(&config.output, &contents) {
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
        return preview.report;
    }

    pipeline_error_report(
        "generation inputs changed repeatedly while they were being read; retry the build"
            .to_owned(),
    )
}

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers.
pub fn check(config: &Config) -> Report {
    run_on_frontend_stack(|| {
        if config.carve {
            run_carve(config, PipelineMode::Check)
        } else {
            run_pipeline(config, PipelineMode::Check)
        }
    })
    .report
}

#[derive(Debug, Clone)]
struct PipelinePreview {
    report: Report,
    files: Vec<String>,
    requirements: Option<runtime_contract::RuntimeRequirements>,
}

fn preview_inner(config: &Config) -> PipelinePreview {
    let result = run_on_frontend_stack(|| {
        if config.carve {
            run_carve(config, PipelineMode::Preview)
        } else {
            run_pipeline(config, PipelineMode::Preview)
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
pub fn diff(old: &Config, new: &Config) -> Result<DiffReport, DiffRejection> {
    run_on_frontend_stack(|| diff_inner(old, new))
}

fn diff_inner(old: &Config, new: &Config) -> Result<DiffReport, DiffRejection> {
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

/// The filesystem paths generation reads for `config`: the root spec, every relative-file `$ref`
/// target reachable from it, and each vendored remote copy.
///
/// Best-effort and side-effect-free: it loads the bundle only (no lowering, no output, no
/// network). If the spec cannot even be loaded (e.g. it is momentarily malformed mid-edit), the
/// returned list is just the spec path, so a watcher can still wait for it to be fixed.
/// Deterministic for a given on-disk state.
fn source_files(config: &Config) -> Vec<Utf8PathBuf> {
    let mut diags = diag::Diagnostics::new(config.batch_cap);
    match source::InputBundle::load(&config.spec, &mut diags) {
        Ok(bundle) => {
            let mut paths: Vec<Utf8PathBuf> =
                bundle.source_paths().map(Utf8Path::to_path_buf).collect();
            if !paths.iter().any(|path| path == &config.spec) {
                paths.push(config.spec.clone());
            }
            paths
        }
        Err(_) => vec![config.spec.clone()],
    }
}

/// Private cross-crate bridge used exclusively by `spargen-macro`.
#[doc(hidden)]
pub mod __private {
    use camino::{Utf8Path, Utf8PathBuf};

    use super::{Config, Outcome, Report};

    #[doc(hidden)]
    pub struct MacroPreview {
        pub report: Report,
        pub contents: Option<String>,
        pub source_files: Vec<Utf8PathBuf>,
    }

    #[doc(hidden)]
    pub fn preview(config: &Config) -> MacroPreview {
        preview_impl(config, None)
    }

    #[doc(hidden)]
    pub fn preview_for_macro(config: &Config, manifest: &str) -> MacroPreview {
        preview_impl(config, Some(Utf8Path::new(manifest)))
    }

    fn preview_impl(config: &Config, manifest: Option<&Utf8Path>) -> MacroPreview {
        let preview = super::preview_inner(config);
        let contents = preview.files.first().cloned();
        let mut source_files = super::source_files(config);
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
        let spec_dir = config
            .spec
            .parent()
            .unwrap_or_else(|| camino::Utf8Path::new(""));
        let lock = spec_dir.join("spargen.lock");
        if lock.is_file() {
            source_files.push(lock);
        }
        source_files.sort();
        source_files.dedup();
        if report.outcome != Outcome::Generated {
            source_files.retain(|path| path == &config.spec);
        }
        MacroPreview {
            report,
            contents,
            source_files,
        }
    }
}

/// Extended documentation for a stable diagnostic code, backing `spargen explain E###` and the
/// published errors index.
pub fn explain(code: &str) -> Result<&'static str, UnknownCode> {
    Code::from_str(code).map(Code::explain)
}

/// The outcome of a [`vendor`] run: the report (present on success) and any diagnostics.
#[cfg(feature = "remote-fetch")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct VendorOutcome {
    /// The vendored-refs report, or `None` if vendoring failed.
    pub report: Option<VendorReport>,
    /// Diagnostics emitted while vendoring (fetch failures, unfetchable schemes, …).
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(feature = "remote-fetch")]
impl VendorOutcome {
    /// Whether vendoring completed and wrote a lock file.
    pub fn succeeded(&self) -> bool {
        self.report.is_some()
            && !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[cfg(feature = "remote-fetch")]
impl std::fmt::Display for VendorOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for diagnostic in &self.diagnostics {
            writeln!(formatter, "{diagnostic}")?;
        }
        let Some(report) = &self.report else {
            return Ok(());
        };
        if report.refs.is_empty() {
            return write!(
                formatter,
                "no remote $refs found; wrote {}",
                report.lock_path
            );
        }
        writeln!(
            formatter,
            "vendored {} remote document(s) under {}:",
            report.refs.len(),
            report.vendor_dir
        )?;
        for vendored in &report.refs {
            writeln!(formatter, "  {} -> {}", vendored.url, vendored.path)?;
        }
        write!(formatter, "wrote {}", report.lock_path)
    }
}

/// Fetch, vendor, and hash-pin every remote (`http`/`https`) `$ref` reachable from `config.spec`,
/// writing copies under `.spargen/vendor/` and (re)writing `spargen.lock` next to the spec.
///
/// This is the **only** spargen entry point that performs network I/O — `generate` and `check`
/// resolve remote refs purely from the vendored, pinned copies this step produces, so builds stay
/// hermetic. Backed by `reqwest` and gated behind the `remote-fetch` feature (implied by `cli`).
#[cfg(feature = "remote-fetch")]
pub fn vendor(config: &Config) -> VendorOutcome {
    let mut diags = diag::Diagnostics::new(config.batch_cap);
    let fetcher = source::ReqwestFetcher;
    let report = source::vendor(&config.spec, &fetcher, &mut diags).ok();
    VendorOutcome {
        report,
        diagnostics: diags.items().to_vec(),
    }
}

#[derive(Debug, Clone, Copy)]
enum PipelineMode {
    /// Frontend audit only, no codegen (`spargen::check`).
    Check,
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
    config: &Config,
    diags: &mut diag::Diagnostics,
) -> Result<(ir::Api, name::Names), ()> {
    let mut bundle = source::InputBundle::load(&config.spec, diags).map_err(|_| ())?;

    if !config.omit.is_empty() && config.omit.apply(&mut bundle, diags).is_err() {
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
    config: &Config,
    api: &ir::Api,
    names: &name::Names,
    diags: &mut diag::Diagnostics,
) -> Result<emit::EmitPlan, ()> {
    let code = codegen::generate(
        api,
        names,
        &codegen::CodegenOptions {
            feature_uuid: config.features.uuid,
            feature_time: config.features.time,
            error_body_cap: config.error_body_cap,
        },
        diags,
    );

    let emit_options = emit::EmitOptions {
        spec: emit::SpecMeta {
            source: if config.omit.is_empty() {
                config.spec.to_string()
            } else {
                format!("{} omit={}", config.spec, config.omit.fingerprint())
            },
            spargen_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    };

    emit::plan(&code, &emit_options).map_err(|error| {
        emit_pipeline_error(diags, error.to_string());
    })
}

fn run_pipeline(config: &Config, mode: PipelineMode) -> PipelineResult {
    let mut diags = diag::Diagnostics::new(config.batch_cap);

    let (api, names) = match lower_frontend(config, &mut diags) {
        Ok(pair) => pair,
        Err(()) => return PipelineResult::bare(report(diags, Outcome::Rejected)),
    };
    let requirements = runtime_contract::RuntimeRequirements::for_api(&api, &config.features);

    if matches!(mode, PipelineMode::Check) {
        return PipelineResult::bare(report(diags, Outcome::Clean));
    }

    let plan = match build_emit_plan(config, &api, &names, &mut diags) {
        Ok(plan) => plan,
        Err(()) => return PipelineResult::bare(report(diags, Outcome::Rejected)),
    };

    match mode {
        // Handled above; `plan` was never built for it.
        PipelineMode::Check => unreachable!("check returns before codegen"),
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
fn run_carve(config: &Config, mode: PipelineMode) -> PipelineResult {
    let mut omit = config.omit.clone();
    let mut last_rejection: Option<Report> = None;

    for _ in 0..compat::MAX_CARVE_ROUNDS {
        let probe = Config {
            omit: omit.clone(),
            carve: false,
            // The carve mapper must see *every* error diagnostic to carve correctly, so the probe
            // runs with an unbounded batch (a spec has finitely many constructs). The user's
            // `batch_cap` still governs the final, user-facing report below.
            batch_cap: usize::MAX,
            ..config.clone()
        };
        // The probe is always a `Check` run, so it never retains a plan.
        let report = run_pipeline(&probe, PipelineMode::Check).report;
        if report.outcome != Outcome::Rejected {
            // Converged: generate/preview/check for real with the carved omit set.
            let resolved = Config {
                omit,
                carve: false,
                ..config.clone()
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
        None => run_pipeline(config, PipelineMode::Check),
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
    }
}
