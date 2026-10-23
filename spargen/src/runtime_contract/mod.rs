//! Cargo manifest auditing for the runtime required by generated output.
//!
//! This is facade plumbing rather than a public subsystem: the lowered API determines the
//! requirements, while Cargo remains responsible for resolving the consumer's declared graph.
//!
//! This file is the audit's driver: it reads the consumer manifest, locates the workspace root,
//! runs every requirement through the checks, and owns the `E023` diagnostic and the
//! `cargo:rerun-if-changed` lines. It also holds the Cargo-integration policy
//! ([`CargoEnvironment`]): whether this run emits directives and audits a manifest at all, and the
//! `W012`/`W013`/`E024` diagnostics that say why not. The rest is split by concern:
//!
//! - `requirements` — the tested dependency floors, what one lowered API requires, and the
//!   requirement table both the audit and `spargen deps` read;
//! - `target` — how a `[target.<key>.dependencies]` key is evaluated for the target being built;
//! - `workspace` — locating the workspace root a `workspace = true` declaration inherits from;
//! - `check` — the rules each dependency declaration is held to.

mod check;
mod requirements;
mod target;
mod workspace;

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::Diagnostic;
use crate::{Build, CargoIntegration, Code, Severity};

use check::{check_dependency, DependencyCheck, WorkspaceOrigin};
use requirements::requirement_table;
pub(crate) use requirements::RuntimeRequirements;
pub use requirements::{RequiredDependency, Requirements};
use target::TargetContext;
use workspace::{absolutized, workspace_root, WorkspaceRoot};

/// Whether this process is an actual Cargo build script. Only there do `cargo:` directives reach
/// Cargo and does the environment name the consuming package.
pub(crate) fn under_build_script() -> bool {
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
fn manifest_from_env() -> Option<Utf8PathBuf> {
    manifest_from(
        std::env::var("CARGO_MANIFEST_PATH"),
        std::env::var("CARGO_MANIFEST_DIR"),
    )
}

/// [`manifest_from_env`] over the results of looking up `CARGO_MANIFEST_PATH` (`path`) and
/// `CARGO_MANIFEST_DIR` (`directory`).
fn manifest_from(
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

pub(crate) struct Audit {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) manifests: Vec<Utf8PathBuf>,
}

pub(crate) fn cargo_directives(manifests: &[Utf8PathBuf]) {
    for directive in rerun_if_changed_lines(manifests) {
        println!("{directive}");
    }
}

/// One `cargo:rerun-if-changed` line per path, in the order given — the audited manifests here,
/// and the build's inputs and output in `cache`.
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

pub(crate) fn audit(manifest_path: &Utf8Path, requirements: &RuntimeRequirements) -> Audit {
    // A real build climbs as Cargo does, to the filesystem root.
    audit_in(
        manifest_path,
        requirements,
        &TargetContext::from_env(),
        None,
    )
}

/// [`audit`] for an explicit target context, so no test has to mutate the process environment,
/// and an explicit `ceiling` on the workspace-root walk (see [`workspace_root`]), so no test's
/// outcome depends on the manifests that happen to sit above its temporary directory.
fn audit_in(
    manifest_path: &Utf8Path,
    requirements: &RuntimeRequirements,
    target: &TargetContext,
    ceiling: Option<&Utf8Path>,
) -> Audit {
    let mut diagnostics = Vec::new();
    let mut manifests = vec![manifest_path.to_path_buf()];
    // The file every diagnostic below names, absolutized so the reader can open it. Naming it is
    // what tells a report about the wrong manifest apart from one about the right file (#71,
    // #339), so a discovery mismatch does not read as a specific, wrong complaint.
    let audited = absolutized(manifest_path);
    let audited = audited.as_path();
    let manifest = match read_toml(manifest_path, "consumer manifest") {
        Ok(value) => value,
        Err(message) => {
            diagnostics.push(diagnostic(audited, message));
            return Audit {
                diagnostics,
                manifests,
            };
        }
    };
    let root = workspace_root(manifest_path, &manifest, ceiling);
    // Only a *separate* workspace manifest is read and reported: a self-rooted one is this very
    // file, already parsed above and already in `manifests`.
    let separate = match &root {
        WorkspaceRoot::Separate(path) => {
            manifests.push(path.clone());
            match read_toml(path, "workspace manifest") {
                Ok(value) => Some(value),
                Err(message) => {
                    diagnostics.push(diagnostic(audited, message));
                    None
                }
            }
        }
        WorkspaceRoot::SelfRooted(_) | WorkspaceRoot::NotFound { .. } => None,
    };
    let workspace = match &root {
        WorkspaceRoot::SelfRooted(_) => Some(&manifest),
        WorkspaceRoot::Separate(_) | WorkspaceRoot::NotFound { .. } => separate.as_ref(),
    };
    // Three outcomes, not two: a root that resolved, a root that was found and could not be read
    // (already reported just above), and no root at all. Their remedies differ, so an unresolvable
    // inheritance has to be able to tell them apart.
    let origin = match &root {
        WorkspaceRoot::SelfRooted(path) => WorkspaceOrigin::Resolved(path),
        WorkspaceRoot::Separate(path) if separate.is_some() => WorkspaceOrigin::Resolved(path),
        // Read and failed, so `read_toml` has already reported why on its own line.
        WorkspaceRoot::Separate(path) => WorkspaceOrigin::Unreadable(path),
        // No root was found. A candidate the walk could not parse is not thereby a root — it may
        // be a sibling crate or a stray file far outside the project — so it never replaces "not
        // found"; it rides along as a hint, carrying its own reason because nothing else is going
        // to print one.
        WorkspaceRoot::NotFound {
            searched_from,
            unreadable,
        } => WorkspaceOrigin::NotFound {
            searched_from,
            skipped: unreadable
                .as_ref()
                .map(|(path, reason)| (path.as_path(), reason.as_str())),
        },
    };

    for required in requirement_table(requirements) {
        if let Some(feature) = required.conditional {
            if !declares_feature(&manifest, feature) {
                continue;
            }
        }
        let wiring = required
            .conditional
            .filter(|_| required.optional)
            .map(|feature| (feature, required.dependency.name));
        check_dependency(
            &manifest,
            workspace,
            DependencyCheck {
                placement: required.placement,
                dependency: required.dependency,
                features: required.features,
                require_no_defaults: required.no_default_features,
                require_optional: required.optional,
                origin,
                audited,
            },
            target,
            &mut diagnostics,
        );
        // An optional requirement is reachable only through the consumer feature that requires
        // it, so that feature has to enable it.
        if let Some((feature, dependency)) = wiring {
            if !feature_enables(&manifest, feature, dependency) {
                diagnostics.push(diagnostic(
                    audited,
                    format!(
                        "Cargo feature `{feature}` must enable the native optional dependency \
                         with `{feature} = [\"dep:{dependency}\"]`"
                    ),
                ));
            }
        }
    }

    manifests.sort();
    manifests.dedup();
    Audit {
        diagnostics,
        manifests,
    }
}

/// Whether the consumer manifest's Cargo feature `feature` lists an entry that enables the
/// optional dependency `dependency`.
fn feature_enables(manifest: &toml::Value, feature: &str, dependency: &str) -> bool {
    manifest
        .get("features")
        .and_then(|value| value.get(feature))
        .and_then(toml::Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .any(|item| enables(item, dependency))
        })
}

/// Whether one entry of a Cargo feature's list enables the optional dependency `dependency`:
/// `"dep:<dependency>"`, the implicit feature `"<dependency>"`, or a dependency feature
/// `"<dependency>/<feature>"`, which enables the dependency along with its feature. The weak form
/// `"<dependency>?/<feature>"` enables a feature only of a dependency something else already
/// enables, so it does not count.
fn enables(item: &str, dependency: &str) -> bool {
    item.strip_prefix("dep:") == Some(dependency)
        || item == dependency
        || item
            .strip_prefix(dependency)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|feature| !feature.is_empty())
}

/// Whether the consumer manifest declares a Cargo feature of its own by this name.
fn declares_feature(manifest: &toml::Value, feature: &str) -> bool {
    manifest
        .get("features")
        .and_then(|value| value.get(feature))
        .is_some()
}

/// `kind` names which manifest failed, so the message names the file actually read: the consumer
/// manifest at one call site, the workspace manifest at the other.
fn read_toml(path: &Utf8Path, kind: &str) -> Result<toml::Value, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read {kind} `{path}`: {error}"))?;
    toml::from_str(&contents).map_err(|error| format!("failed to parse {kind} `{path}`: {error}"))
}

/// The suffix every `E023` message ends with, naming the manifest the audit read.
const AUDITED_MANIFEST: &str = "; audited manifest: ";

/// An `E023` diagnostic whose message ends by naming `audited`, the absolutized consumer manifest
/// the audit read.
///
/// It is the only constructor, so no message can leave out which file it is about. It rides on the
/// message rather than the `help:` remedy because `generate_api!` reports only code, message, and
/// pointer: a manifest-discovery mismatch shows on the macro path (#71), and a remedy does not
/// reach that report. A message that already names this file in its own clause (a
/// read failure of the consumer manifest, or a self-rooted workspace declaring no entry) still
/// carries the suffix: the clause names the file in its role there, and the suffix names it in the
/// same shape on every message.
fn diagnostic(audited: &Utf8Path, mut message: String) -> Diagnostic {
    // A TOML parse error ends in a newline, which would strand the suffix on a line of its own.
    message.truncate(message.trim_end().len());
    message.push_str(&format!("{AUDITED_MANIFEST}`{audited}`"));
    Diagnostic::at_root(Code::RuntimeDependencyContract, message)
        .remedy("declare the generated client's runtime dependencies in the consuming package's Cargo.toml using the documented supported ranges and features")
        .build()
}

#[cfg(test)]
mod tests;
