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

#[cfg(feature = "cli")]
pub mod cli;

use std::str::FromStr;

use camino::Utf8PathBuf;

pub use compat::{ComponentKind, Omit, OmitMethod, OmitRule};
pub use diag::{Code, Diagnostic, JsonPointer, Severity, Span};
#[cfg(feature = "remote-fetch")]
pub use source::{VendorReport, VendoredRef};

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
    run_pipeline(config, PipelineMode::Generate)
}

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers.
pub fn check(config: &Config) -> Report {
    run_pipeline(config, PipelineMode::Check)
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

fn run_pipeline(config: &Config, mode: PipelineMode) -> Report {
    let mut diags = diag::Diagnostics::new(config.batch_cap);

    let mut bundle = match source::InputBundle::load(&config.spec, &mut diags) {
        Ok(bundle) => bundle,
        Err(_) => return report(diags, Outcome::Rejected),
    };

    if !config.omit.is_empty() && config.omit.apply(&mut bundle, &mut diags).is_err() {
        return report(diags, Outcome::Rejected);
    }

    let validator = oas31::MetaSchemaValidator::load_vendored();
    validator.validate(bundle.root(), &mut diags);
    if diags.has_errors() {
        return report(diags, Outcome::Rejected);
    }

    let document = match oas31::parse_document(&bundle, &mut diags) {
        Ok(document) => document,
        Err(_) => return report(diags, Outcome::Rejected),
    };

    let resolver = oas31::Resolver::new(&document, &bundle);
    oas31::audit(&document, &mut diags);
    if diags.has_errors() {
        return report(diags, Outcome::Rejected);
    }

    // `check` runs the full frontend — lowering, IR invariants, and name allocation — so it fires
    // exactly the diagnostics `generate` would, just without emitting code.
    let api = match oas31::lower(&document, &resolver, &mut diags) {
        Ok(api) => api,
        Err(_) => return report(diags, Outcome::Rejected),
    };
    ir::check_invariants(&api, &mut diags);
    if diags.has_errors() {
        return report(diags, Outcome::Rejected);
    }

    let names = name::allocate(&api, &mut diags);
    if diags.has_errors() {
        return report(diags, Outcome::Rejected);
    }

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
