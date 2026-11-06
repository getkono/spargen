//! How a run integrates with Cargo: whether this process is a build script, the consuming
//! package's manifest it names, the [`CargoIntegration`] policy resolved against both, and the
//! `cargo:rerun-if-changed` lines.

use camino::Utf8PathBuf;

use crate::diag::Diagnostic;
use crate::{Build, CargoIntegration, Code, Severity};

// Mutation testing: replacing `under_build_script` with `false`, `build_script_manifest` with
// `None`, `manifest_from_env`, or `print_rerun` survives the unit tests and is declared in
// `.cargo/mutants.toml` rather than killed. Their only input is the process-wide environment, or
// their only output standard output, which a unit test could set or read only under every test
// running beside it. The decisions they feed are the pure functions below (`manifest_from`,
// `resolve_cargo_environment`, `rerun_if_changed_lines`), which the unit tests pin, and
// `spargen/tests/e2e/manifest_audit.rs` runs the readers from a real build script.

/// Whether this process is an actual Cargo build script. Only there do `cargo:` directives reach
/// Cargo and does the environment name the consuming package.
pub(super) fn under_build_script() -> bool {
    std::env::var_os("OUT_DIR").is_some() && std::env::var_os("CARGO_CFG_TARGET_ARCH").is_some()
}

/// The consuming package's manifest when this process is a build script, and `None` otherwise:
/// [`manifest_from_env`] read only where [`under_build_script`] holds.
pub(crate) fn build_script_manifest() -> Option<Utf8PathBuf> {
    under_build_script().then(manifest_from_env).flatten()
}

/// What the Cargo environment affords this run, resolved from [`CargoIntegration`] once so the
/// generation path never has to re-decide it.
pub(crate) struct CargoEnvironment {
    /// Emit `cargo:rerun-if-changed` directives.
    pub(crate) directives: bool,
    /// The consumer manifest to audit against (`E023`), when one was found.
    pub(crate) manifest: Option<Utf8PathBuf>,
    /// `W012`/`W013` for a degraded run, or `E024` for a required one.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Whether `diagnostics` contains a hard failure.
    pub(crate) fatal: bool,
}

/// Resolve the Cargo integration policy against the actual process environment.
pub(crate) fn cargo_environment(build: &Build) -> CargoEnvironment {
    resolve_cargo_environment(build.cargo, under_build_script(), build_script_manifest())
}

/// The policy decision itself, as a pure function of what the environment affords — so every
/// branch is reachable in a test without mutating process-global state.
///
/// Each diagnostic is about the build environment rather than the document, so it points at the
/// document root.
pub(super) fn resolve_cargo_environment(
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
                (None, CargoIntegration::Required) => vec![Diagnostic::at_root(
                    Code::CargoIntegrationRequired,
                    "cargo integration is required, but no consumer manifest was found; \
                     set CARGO_MANIFEST_DIR or use `CargoIntegration::Auto`",
                )
                .build()],
                (None, _) => vec![Diagnostic::at_root(
                    Code::RuntimeAuditSkipped,
                    "no consumer manifest was found, so the runtime-dependency audit was \
                     skipped; run `spargen deps <spec>` for the dependencies generated output \
                     requires",
                )
                .build()],
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
            diagnostics: vec![Diagnostic::at_root(
                Code::CargoIntegrationRequired,
                "cargo integration is required, but this is not a build-script process; \
                 call `generate` from a `build.rs`, or relax to `CargoIntegration::Auto`",
            )
            .build()],
            fatal: true,
        },
        CargoIntegration::Auto => CargoEnvironment {
            directives: false,
            manifest: None,
            diagnostics: vec![Diagnostic::at_root(
                Code::CargoIntegrationDegraded,
                "not a build-script process: no rebuild triggers were emitted and the \
                 runtime-dependency audit was skipped; call `generate` from a `build.rs`, or \
                 silence this with `CargoIntegration::Off`",
            )
            .build()],
            fatal: false,
        },
    }
}

/// The consuming package's manifest, as a build script's environment names it:
/// `CARGO_MANIFEST_PATH` when set, otherwise `Cargo.toml` inside `CARGO_MANIFEST_DIR`, otherwise
/// `None`.
///
/// Its edge cases differ from `generate_api!`'s locator in `spargen-macro`; aligning the two would
/// change what a build script audits, so they are stated here rather than unified:
///
/// - A variable whose value is not UTF-8 counts as unset, so a non-UTF-8 `CARGO_MANIFEST_PATH`
///   falls back to `CARGO_MANIFEST_DIR` silently. The macro's locator reports it instead.
/// - An empty value is taken as given: an empty `CARGO_MANIFEST_PATH` names the empty path, which
///   the audit then fails to read, and an empty `CARGO_MANIFEST_DIR` names `Cargo.toml` relative
///   to the working directory. The macro's locator treats an empty value as unset.
///
/// Both are decided by [`manifest_from`], which takes the two lookups as arguments so they can be
/// exercised without mutating the process environment.
// Mutation testing: an environment reader, declared in `.cargo/mutants.toml` with the others
// named at the top of this file.
fn manifest_from_env() -> Option<Utf8PathBuf> {
    manifest_from(
        std::env::var("CARGO_MANIFEST_PATH"),
        std::env::var("CARGO_MANIFEST_DIR"),
    )
}

/// [`manifest_from_env`] over the results of looking up `CARGO_MANIFEST_PATH` (`path`) and
/// `CARGO_MANIFEST_DIR` (`directory`).
pub(super) fn manifest_from(
    path: Result<String, std::env::VarError>,
    directory: Result<String, std::env::VarError>,
) -> Option<Utf8PathBuf> {
    path.ok().map(Utf8PathBuf::from).or_else(|| {
        directory
            .ok()
            .map(Utf8PathBuf::from)
            .map(|directory| directory.join("Cargo.toml"))
    })
}

/// Print [`rerun_if_changed_lines`] for `paths`: the one writer of `cargo:rerun-if-changed`
/// directives, for the build's inputs and output and for the manifests the audit read alike.
// Mutation testing: a standard-output writer, declared in `.cargo/mutants.toml` with the
// environment readers named at the top of this file.
pub(crate) fn print_rerun(paths: &[Utf8PathBuf]) {
    for directive in rerun_if_changed_lines(paths) {
        println!("{directive}");
    }
}

/// One `cargo:rerun-if-changed` line per path, in the order given.
///
/// Cargo reads build-script output line by line and has no escape for a line break, so a path
/// carrying one cannot be named: written out, everything after the break would reach Cargo as a
/// directive of its own. Such a path is left out rather than split.
pub(crate) fn rerun_if_changed_lines(paths: &[Utf8PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !path.as_str().contains(['\n', '\r']))
        .map(|path| format!("cargo:rerun-if-changed={path}"))
        .collect()
}
