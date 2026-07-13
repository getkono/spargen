//! # spargen
//!
//! A compile-time-correct Rust HTTP client generator for OpenAPI 3.1.x. Spec in, spar out:
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

pub mod diag;

mod codegen;
mod compat;
mod emit;
mod ir;
mod name;
mod oas31;
mod source;
mod support;
mod surface;

#[cfg(feature = "cli")]
pub mod cli;

use std::str::FromStr;

use camino::{Utf8Path, Utf8PathBuf};

pub use compat::{ComponentKind, Omit, OmitMethod, OmitRule};
pub use diag::{Code, Diagnostic, JsonPointer, Severity, Span};
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

/// Where generated code is written.
#[derive(Debug, Clone)]
pub enum OutputTarget {
    /// A module (file or directory) checked into an existing crate.
    Module(Utf8PathBuf),
    /// A standalone, publishable crate.
    Crate {
        /// The crate directory to create.
        dir: Utf8PathBuf,
        /// The crate name.
        name: String,
    },
}

/// Configuration for one generation run — the primary `build.rs` input. Construct with
/// [`Config::new`] and adjust fields as needed.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Where to write generated code.
    pub output: OutputTarget,
    /// Generated-output feature toggles.
    pub features: Features,
    /// Explicit compatibility omissions applied before OpenAPI validation/lowering.
    pub omit: Omit,
    /// Max bytes of a response body retained on error variants (default 64 KiB).
    pub error_body_cap: usize,
    /// Max diagnostics collected before batching stops.
    pub batch_cap: usize,
    /// Audit and check drift only; do not write output (`--check`).
    pub check_only: bool,
    /// Auto-carve: instead of failing on rejections, iteratively omit the minimal enclosing
    /// omittable construct for each rejection and generate the rest (`--carve`). Every carved
    /// construct is reported via `W009`; residual, un-carvable rejections are reported honestly.
    pub carve: bool,
}

impl Config {
    /// A config with sensible defaults: features on, 64 KiB error-body cap, a bounded diagnostic
    /// batch, writing enabled.
    pub fn new(spec: impl Into<Utf8PathBuf>, output: OutputTarget) -> Self {
        Self {
            spec: spec.into(),
            output,
            features: Features::default(),
            omit: Omit::default(),
            error_body_cap: 64 * 1024,
            batch_cap: 100,
            check_only: false,
            carve: false,
        }
    }
}

/// The outcome of a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Code was generated and written.
    Generated,
    /// `--check`: checked-in output matches the spec.
    Clean,
    /// `--check`: checked-in output drifted from the spec.
    Drifted,
    /// The spec used an R-class construct; generation failed loudly.
    Rejected,
}

/// The result of a pipeline run: the collected diagnostics plus the outcome.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every diagnostic emitted during the run (batch reporting).
    pub diagnostics: Vec<Diagnostic>,
    /// What happened.
    pub outcome: Outcome,
}

/// Run the full pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`. The
/// primary `build.rs` entry point.
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines.
/// let config = spargen::Config::new(
///     "api/openapi.yaml",
///     spargen::OutputTarget::Module("src/api.rs".into()),
/// );
/// let report = spargen::generate(&config);
/// println!("cargo:warning=spargen outcome: {:?}", report.outcome);
/// ```
pub fn generate(config: &Config) -> Report {
    run_on_frontend_stack(|| {
        if config.carve {
            run_carve(config, PipelineMode::Generate)
        } else {
            run_pipeline(config, PipelineMode::Generate)
        }
    })
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
pub struct DiffOutcome {
    /// The semver-impact diff, present iff both specs lowered successfully.
    pub report: Option<DiffReport>,
    /// The old spec's rejection report, if it failed to lower.
    pub old_rejection: Option<Report>,
    /// The new spec's rejection report, if it failed to lower.
    pub new_rejection: Option<Report>,
}

/// Diff the **public API surface** of the client that would be generated from `old` versus `new`,
/// classifying the change as a semver bump (`major` breaking / `minor` additive / `patch` no-op).
///
/// Per the product contract, "the semver surface is the public API of generated output": this runs
/// the frontend (parse → lower → name-allocate) on both specs, models what a consumer of the
/// generated client sees (operations, their params/body/return types, and the public model types),
/// and reports every difference with its impact. A pure analysis step — it never writes output nor
/// touches the runtime. Deterministic: the same pair of specs yields a byte-identical report.
pub fn diff(old: &Config, new: &Config) -> DiffOutcome {
    run_on_frontend_stack(|| diff_inner(old, new))
}

fn diff_inner(old: &Config, new: &Config) -> DiffOutcome {
    let mut old_diags = diag::Diagnostics::new(old.batch_cap);
    let mut new_diags = diag::Diagnostics::new(new.batch_cap);
    let old_lowered = lower_frontend(old, &mut old_diags);
    let new_lowered = lower_frontend(new, &mut new_diags);

    match (old_lowered, new_lowered) {
        (Ok((old_api, old_names)), Ok((new_api, new_names))) => {
            let old_surface = surface::build(&old_api, &old_names);
            let new_surface = surface::build(&new_api, &new_names);
            DiffOutcome {
                report: Some(surface::diff(&old_surface, &new_surface)),
                old_rejection: None,
                new_rejection: None,
            }
        }
        (old_lowered, new_lowered) => DiffOutcome {
            report: None,
            old_rejection: old_lowered
                .is_err()
                .then(|| report(old_diags, Outcome::Rejected)),
            new_rejection: new_lowered
                .is_err()
                .then(|| report(new_diags, Outcome::Rejected)),
        },
    }
}

/// The filesystem paths [`generate`]/[`check`] read for `config`: the root spec, every
/// relative-file `$ref` target reachable from it, and each vendored remote copy. This is the raw
/// on-disk footprint of a spec — the CLI `--watch` loop builds its watch set on top of it (adding
/// the config and lock files).
///
/// Best-effort and side-effect-free: it loads the bundle only (no lowering, no output, no
/// network). If the spec cannot even be loaded (e.g. it is momentarily malformed mid-edit), the
/// returned list is just the spec path, so a watcher can still wait for it to be fixed.
/// Deterministic for a given on-disk state.
pub fn source_files(config: &Config) -> Vec<Utf8PathBuf> {
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

/// Extended documentation for a stable diagnostic code, backing `spargen explain E###` and the
/// published errors index.
pub fn explain(code: &str) -> Option<&'static str> {
    Code::from_str(code).ok().map(Code::explain)
}

/// The outcome of a [`vendor`] run: the report (present on success) and any diagnostics.
#[cfg(feature = "remote-fetch")]
#[derive(Debug, Clone)]
pub struct VendorOutcome {
    /// The vendored-refs report, or `None` if vendoring failed.
    pub report: Option<VendorReport>,
    /// Diagnostics emitted while vendoring (fetch failures, unfetchable schemes, …).
    pub diagnostics: Vec<Diagnostic>,
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
    Generate,
    Check,
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
    oas31::audit(&document, diags);
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

fn run_pipeline(config: &Config, mode: PipelineMode) -> Report {
    let mut diags = diag::Diagnostics::new(config.batch_cap);

    let (api, names) = match lower_frontend(config, &mut diags) {
        Ok(pair) => pair,
        Err(()) => return report(diags, Outcome::Rejected),
    };

    if matches!(mode, PipelineMode::Check) {
        return report(diags, Outcome::Clean);
    }

    let code = codegen::generate(
        &api,
        &names,
        &codegen::CodegenOptions {
            feature_uuid: config.features.uuid,
            feature_time: config.features.time,
            error_body_cap: config.error_body_cap,
        },
        &mut diags,
    );

    let emit_options = emit::EmitOptions {
        layout: match &config.output {
            OutputTarget::Module(path) => emit::OutputLayout::Module { path: path.clone() },
            OutputTarget::Crate { dir, name } => emit::OutputLayout::Crate {
                dir: dir.clone(),
                package: emit::PackageMeta {
                    name: name.clone(),
                    version: "0.0.0".to_owned(),
                },
            },
        },
        features: emit::FeatureSet {
            uuid: config.features.uuid,
            time: config.features.time,
            // Derived from the API so the synthesized manifest carries exactly the extra
            // reqwest/bytes features the emitted code needs (deterministic, minimal).
            multipart: api.operations.iter().any(|operation| {
                operation
                    .request_body
                    .as_ref()
                    .is_some_and(|body| body.media == ir::MediaType::Multipart)
            }),
            bytes_serde: api.types.iter().any(|(_, def)| {
                matches!(&def.kind, ir::TypeKind::Struct(object)
                if object.fields.iter().any(|field| matches!(
                    api.types.get(field.ty.id).map(|def| &def.kind),
                    Some(ir::TypeKind::Bytes)
                )))
            }),
            // Pull in quick-xml exactly when the API uses an XML body (same predicate that gates the
            // embedded `support::xml` module), so the manifest and embedded code stay in lockstep.
            xml: api.uses_xml(),
        },
        spec: emit::SpecMeta {
            source: if config.omit.is_empty() {
                config.spec.to_string()
            } else {
                format!("{} omit={}", config.spec, config.omit.fingerprint())
            },
            spargen_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    };
    let plan = match emit::plan(&code, &emit_options) {
        Ok(plan) => plan,
        Err(error) => {
            emit_pipeline_error(&mut diags, error.to_string());
            return report(diags, Outcome::Rejected);
        }
    };

    if config.check_only {
        match emit::check_drift(&plan, camino::Utf8Path::new("")) {
            Ok(emit::DriftReport::Clean) => report(diags, Outcome::Clean),
            Ok(emit::DriftReport::Drifted(paths)) => {
                emit_drift(&mut diags, &paths, "drifted from the spec");
                report(diags, Outcome::Drifted)
            }
            Ok(emit::DriftReport::Missing(paths)) => {
                emit_drift(&mut diags, &paths, "missing on disk");
                report(diags, Outcome::Drifted)
            }
            Err(error) => {
                emit_pipeline_error(&mut diags, error.to_string());
                report(diags, Outcome::Rejected)
            }
        }
    } else {
        match emit::write(&plan) {
            Ok(()) => report(diags, Outcome::Generated),
            Err(error) => {
                emit_pipeline_error(&mut diags, error.to_string());
                report(diags, Outcome::Rejected)
            }
        }
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
fn run_carve(config: &Config, mode: PipelineMode) -> Report {
    let mut omit = config.omit.clone();
    let mut last_rejection: Option<Report> = None;

    for _ in 0..compat::MAX_CARVE_ROUNDS {
        let probe = Config {
            omit: omit.clone(),
            carve: false,
            check_only: false,
            // The carve mapper must see *every* error diagnostic to carve correctly, so the probe
            // runs with an unbounded batch (a spec has finitely many constructs). The user's
            // `batch_cap` still governs the final, user-facing report below.
            batch_cap: usize::MAX,
            ..config.clone()
        };
        let report = run_pipeline(&probe, PipelineMode::Check);
        if report.outcome != Outcome::Rejected {
            // Converged: generate (or check) for real with the carved omit set.
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
            return report;
        }
        omit.rules.extend(new_rules);
        last_rejection = Some(report);
    }

    // Exhausted the round cap while still rejecting: return the last honest rejection report.
    last_rejection.unwrap_or_else(|| run_pipeline(config, PipelineMode::Check))
}

fn emit_drift(diags: &mut diag::Diagnostics, paths: &[camino::Utf8PathBuf], what: &str) {
    for path in paths {
        diag::Diagnostic::warning(
            Code::OutputDrifted,
            diag::Provenance::new(JsonPointer::root(), None),
        )
        .message(format!("checked-in output `{path}` is {what}"))
        .remedy("re-run spargen generate and commit the result")
        .emit(diags);
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

fn report(diags: diag::Diagnostics, outcome: Outcome) -> Report {
    Report {
        diagnostics: diags.items().to_vec(),
        outcome,
    }
}
