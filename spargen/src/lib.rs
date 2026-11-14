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
//! This file holds the public entry points and the re-exports that make up the crate root. The
//! facade's other modules are plumbing rather than subsystems: `report` (the [`Report`] and
//! [`Outcome`] every entry point returns), `pipeline` (the runs those entry points drive),
//! `diff_api` (the [`diff`] result types), `output` (the atomic file write), `cache`, `config`,
//! and `runtime_contract`. They carry no `layer-deps:` header, the lint admits them by name in
//! its `FACADE_PLUMBING` list, and no subsystem may depend on them. `test_support`, compiled only
//! into the test build, holds helpers the subsystems' in-crate tests share; the lint admits it by
//! name in `TEST_SUPPORT`.
//!
//! ## How the public types grow
//!
//! A public type grows without a breaking change in one of two ways. [`Report`], [`Spec`],
//! [`Build`], [`DiffRejection`] and [`DiffSides`] keep their fields private and are read through
//! accessors, and `Spec` and `Build` are set through chained setters, so a new field or knob is
//! additive. A type with public fields or variants that may grow — [`Diagnostic`], [`Outcome`],
//! [`Code`], [`CargoIntegration`], [`ConfigError`], [`Loc`] and [`Span`] among them — is
//! `#[non_exhaustive]` instead, with a constructor where a caller builds one ([`Loc::new`],
//! [`Span::new`], [`ConfigError::flag`]). [`Severity`]
//! stays exhaustive: its two variants are the error/warning partition every [`Report`] splits on.

mod diag;

mod cache;
mod codegen;
mod compat;
mod config;
mod diff_api;
mod emit;
mod ir;
mod name;
mod oas31;
mod output;
mod pipeline;
mod report;
mod runtime_contract;
mod source;
mod support;
mod surface;
#[cfg(test)]
mod test_support;

use std::str::FromStr;

pub use compat::{ComponentKind, KeepRule, Omit, OmitMethod, OmitRule, UnknownOmitToken};
pub use config::{
    Build, CargoIntegration, CarriedDocs, Carve, ConfigError, Narrowing, Spec, TestingModule,
    UnionTags, UuidMapping,
};
pub use diag::{
    Code, Diagnostic, FileId, InterpId, JsonPointer, Loc, OutcomeClaim, Severity, Span, UnknownCode,
};
pub use diff_api::{DiffRejection, DiffSides, Side};
pub use ir::{DateTimeBackend, UnknownDateTimeBackend};
pub use report::{Outcome, Report};
pub use runtime_contract::{RequiredDependency, Requirements};
#[cfg(feature = "remote-fetch")]
pub use source::{VendorReport, VendoredRef};
pub use surface::{Change, ChangeKind, DiffReport, Impact};

/// Run the full pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`. The
/// primary `build.rs` entry point.
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines.
/// let build = spargen::Spec::new("api/openapi.yaml").build("src/api.rs");
/// spargen::generate(&build).expect_success();
/// ```
#[must_use = "the report carries the outcome and every diagnostic; a rejection is lost unless it is read, e.g. with `.expect_success()`"]
pub fn generate(build: &Build) -> Report {
    let cache_dir = cache::cache_dir();
    pipeline::generate_with_cache_dir(build, cache_dir.as_deref())
}

/// Run the support-audit only, without codegen (`spargen check`) — a CI contract gate between spec
/// producers and client consumers.
///
/// When called from a build script, this also runs the consumer-manifest dependency audit, so
/// `E023` is catchable before a single line of code is generated. Elsewhere there is no consuming
/// package to audit — `spargen deps` prints the dependency tables generated output
/// requires instead.
///
/// From a build script it also prints the `cargo:rerun-if-changed` directives [`generate`] prints
/// for its inputs: the spec, every file it references, `spargen.lock` and the applied config files
/// (or, when they fail to load, every file the attempt read, the one that failed it included),
/// and the manifests and lockfile the audit read.
#[must_use = "the report carries the outcome and every diagnostic; a rejection is lost unless it is read, e.g. with `.expect_success()`"]
pub fn check(spec: &Spec) -> Report {
    if runtime_contract::under_build_script() {
        let read = pipeline::run_on_frontend_stack(|| cache::read_paths(spec));
        runtime_contract::print_rerun(&cache::rerun_paths(spec, &read, None));
    }
    let result = pipeline::run_mode(spec, pipeline::PipelineMode::Requirements);
    let mut report = result.report;
    let manifest = runtime_contract::build_script_manifest();
    if let (Some(manifest), Some(requirements)) = (manifest, result.requirements) {
        let manifests = pipeline::apply_audit(&mut report, &manifest, &requirements);
        runtime_contract::print_rerun(&manifests);
    }
    report
}

/// Diff the **public API surface** of the client that would be generated from `old` versus `new`,
/// classifying the change as a semver bump (`major` breaking / `minor` additive / `patch` no-op).
///
/// Per the product contract, "the semver surface is the public API of generated output": this runs
/// the frontend (parse → lower → name-allocate) on both specs, models what a consumer of the
/// generated client sees (operations, their params/body/return types, and the public model types),
/// and reports every difference with its impact. A pure analysis step — it never writes output nor
/// touches the runtime. Deterministic: the same pair of specs yields a byte-identical report.
///
/// When both specs lower, the diff comes back beside [`DiffSides`], the report of each side's
/// lowering, which carries the warnings that lowering emitted, up to the spec's `batch_cap`.
pub fn diff(old: &Spec, new: &Spec) -> Result<(DiffReport, DiffSides), DiffRejection> {
    pipeline::run_on_frontend_stack(|| diff_api::diff_inner(old, new))
}

/// Private cross-crate bridge to the in-memory renderer, for `spargen-macro` and for tests that
/// render without writing.
#[doc(hidden)]
pub mod __private {
    use camino::{Utf8Path, Utf8PathBuf};

    use super::{cache, pipeline, Outcome, Report, Spec};

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
        let preview = pipeline::run_mode(spec, pipeline::PipelineMode::Preview);
        let mut report = preview.report;
        // Only a run that generated carries requirements to audit.
        let manifests = match (manifest, preview.requirements.as_ref()) {
            (Some(manifest), Some(requirements)) => {
                pipeline::apply_audit(&mut report, manifest, requirements)
            }
            _ => Vec::new(),
        };
        // The files a generation reads are the build cache's inputs: the root spec, every
        // relative-file `$ref` target and vendored remote copy, `spargen.lock`, and the applied
        // config files, whose edit changes what was generated and so re-expands, beside the
        // manifests the audit read. A rejected run, or an input set that does not load (a spec
        // malformed mid-edit), names the root spec alone, so the macro still waits for it to be
        // fixed.
        let mut source_files = vec![spec.path.clone()];
        if report.outcome == Outcome::Generated {
            source_files.extend(
                pipeline::run_on_frontend_stack(|| cache::input_paths(spec)).unwrap_or_default(),
            );
            source_files.extend(manifests);
            source_files.sort();
            source_files.dedup();
        }
        MacroPreview {
            report,
            contents: preview.plan.map(|plan| plan.contents),
            source_files,
        }
    }
}

/// The dependency tables generated output from `spec` requires — what backs `spargen deps`.
///
/// Generated output is freestanding, so the consuming package declares its own runtime
/// dependencies. Which ones depends on the API: `format: uuid` pulls in `uuid`, sequential
/// responses and binary `_stream` twins pull in `futures-core` and a `reqwest` feature. This
/// returns the
/// whole set before a build, rather than leaving `E023` to report what the consuming package
/// lacks.
///
/// The spec must lower — a rejection is returned as the [`Report`] that explains why. A spec that
/// lowers returns its requirements beside the run's own [`Report`], whose outcome is
/// [`Outcome::Clean`] and which carries the warnings the lowering emitted, up to the spec's
/// `batch_cap`.
pub fn requirements(spec: &Spec) -> Result<(Requirements, Report), Report> {
    let result = pipeline::run_mode(spec, pipeline::PipelineMode::Requirements);
    match result.requirements {
        Some(requirements) => Ok((Requirements::new(&requirements), result.report)),
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
///
/// Every diagnostic this step emits is an error, so a run that succeeds has nothing to report
/// beside its [`VendorReport`], and a failed one returns its errors, up to the spec's
/// `batch_cap`, in its [`Report`].
#[cfg(feature = "remote-fetch")]
pub fn vendor(spec: &Spec) -> Result<VendorReport, Report> {
    pipeline::run_on_frontend_stack(|| {
        let mut diags = diag::Diagnostics::new(spec.batch_cap);
        let fetcher = source::ReqwestFetcher;
        match source::vendor(&spec.path, &fetcher, &output::atomic_write_all, &mut diags) {
            Ok(vendored) if !diags.has_errors() => Ok(vendored),
            _ => Err(Report::new(diags, Outcome::Rejected)),
        }
    })
}

#[cfg(test)]
mod tests {
    /// [`vendor`](super::vendor) returns no report beside a [`VendorReport`](super::VendorReport),
    /// so a warning its run emitted would be dropped with the run. The `source` subsystem it runs
    /// is the only code that can emit one there (`source` depends on `diag` alone), so every
    /// diagnostic its production code builds must be an error: built with `Diagnostic::error`,
    /// never `Diagnostic::warning`, `Diagnostic::at_root` (which takes the code's own severity) or
    /// a literal `severity`.
    #[test]
    fn vendor_emits_no_warning_severity_diagnostics() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/source");
        let files = crate::test_support::rust_sources(&dir);
        let mut scanned = 0;
        let mut errors = 0;
        for file in &files {
            let name = file.file_name().and_then(|name| name.to_str()).unwrap();
            if name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(file).unwrap();
            // The test module, inline or declared, is the last item of a file that has one.
            let production = text.split("#[cfg(test)]").next().unwrap();
            scanned += 1;
            errors += production.matches("Diagnostic::error(").count();
            for builder in [
                "Diagnostic::warning(",
                "at_root(",
                "Severity::Warning",
                "severity:",
            ] {
                assert!(
                    !production.contains(builder),
                    "{} builds a diagnostic with `{builder}`, which can be a warning; `vendor` \
                     returns no report on success, so a warning would be dropped",
                    file.display()
                );
            }
        }
        assert!(
            scanned >= 5 && errors > 0,
            "the scan read {scanned} files under {} and found {errors} `Diagnostic::error` \
             calls; it has stopped reading the subsystem",
            dir.display()
        );
    }
}
