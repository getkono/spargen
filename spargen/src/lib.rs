//! # spargen
//!
//! A compile-time-correct Rust HTTP client generator for OpenAPI 3.1.x. Spec in, spar out:
//! everything structural is decided at generation time; nothing is interpreted at runtime.
//!
//! This crate is the library half of the `spargen` tool. Its public surface is the `build.rs`
//! API — see the [facade](crate) items ([`Config`], [`generate`], [`check`], [`explain`]).
//!
//! ## Subsystem layering (PRD §2.3)
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

// TODO(impl): remove these once subsystem bodies are implemented and the pipeline is wired.
// Stub signatures leave params unused; stub structs carry private fields nothing reads yet; and
// subsystem re-exports have no in-crate consumers until later stages depend on them.
#![allow(unused_variables, dead_code, unused_imports)]

pub mod diag;

mod codegen;
mod emit;
mod ir;
mod name;
mod oas31;
mod source;
mod support;

#[cfg(feature = "cli")]
pub mod cli;

use camino::Utf8PathBuf;

pub use diag::{Code, Diagnostic, JsonPointer, Severity, Span};

/// Feature toggles for the generated output (both default **on**; PRD §6.2). Disabling one falls
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

/// Configuration for one generation run — the primary `build.rs` input (PRD §2.1). Construct with
/// [`Config::new`] and adjust fields as needed.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Where to write generated code.
    pub output: OutputTarget,
    /// Generated-output feature toggles.
    pub features: Features,
    /// Max bytes of a response body retained on error variants (default 64 KiB; PRD D7).
    pub error_body_cap: usize,
    /// Max diagnostics collected before batching stops (PRD FR6).
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
    /// The spec used an R-class construct; generation failed loudly (PRD FR2).
    Rejected,
}

/// The result of a pipeline run: the collected diagnostics plus the outcome.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every diagnostic emitted during the run (PRD FR6 batch reporting).
    pub diagnostics: Vec<Diagnostic>,
    /// What happened.
    pub outcome: Outcome,
}

/// Run the full pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit` (PRD §2.3). The
/// primary `build.rs` entry point.
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines (PRD DoD #6).
/// let config = spargen::Config::new(
///     "api/openapi.yaml",
///     spargen::OutputTarget::Module("src/api.rs".into()),
/// );
/// let report = spargen::generate(&config);
/// println!("cargo:warning=spargen outcome: {:?}", report.outcome);
/// ```
pub fn generate(config: &Config) -> Report {
    todo!()
}

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers (PRD FR6).
pub fn check(config: &Config) -> Report {
    todo!()
}

/// Extended documentation for a stable diagnostic code, backing `spargen explain E###` and the
/// published errors index (PRD FR6).
pub fn explain(code: &str) -> Option<&'static str> {
    todo!()
}
