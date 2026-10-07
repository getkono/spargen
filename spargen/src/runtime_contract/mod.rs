//! Cargo manifest auditing for the runtime required by generated output.
//!
//! This is facade plumbing rather than a public subsystem: the lowered API determines the
//! requirements, while Cargo remains responsible for resolving the consumer's declared graph.
//!
//! This file is the audit's driver: it reads the consumer manifest, locates the workspace root,
//! runs every requirement through the checks, and owns the `E023` diagnostic and the
//! `cargo:rerun-if-changed` lines. The rest is split by concern:
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

use crate::diag::{Diagnostic, OutcomeClaim};
use crate::{Code, JsonPointer};

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
pub(crate) fn manifest_from_env() -> Option<Utf8PathBuf> {
    std::env::var("CARGO_MANIFEST_PATH")
        .ok()
        .map(Utf8PathBuf::from)
        .or_else(|| {
            std::env::var("CARGO_MANIFEST_DIR")
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
    // The file every diagnostic below names, absolutized so the reader can open it. A report whose
    // text never said which manifest was read could not be told apart from one about the right
    // file (#71, #339): a discovery mismatch then reads as a specific, wrong complaint.
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
    }

    if declares_feature(&manifest, "blocking") {
        let wired = manifest
            .get("features")
            .and_then(|value| value.get("blocking"))
            .and_then(toml::Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .any(|item| item == "dep:tokio" || item == "tokio")
            });
        if !wired {
            diagnostics.push(diagnostic(
                audited,
                "Cargo feature `blocking` must enable the native optional dependency with `blocking = [\"dep:tokio\"]`"
                    .to_owned(),
            ));
        }
    }

    manifests.sort();
    manifests.dedup();
    Audit {
        diagnostics,
        manifests,
    }
}

/// Whether the consumer manifest declares a Cargo feature of its own by this name.
fn declares_feature(manifest: &toml::Value, feature: &str) -> bool {
    manifest
        .get("features")
        .and_then(|value| value.get(feature))
        .is_some()
}

/// `kind` names which manifest failed. Both call sites read a different file, and calling the
/// workspace root "the consumer manifest" contradicted the very diagnostic printed beside it.
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
/// pointer: the macro path is where a manifest-discovery mismatch hid (#71), and a remedy would
/// never have reached that report. A message that already names this file in its own clause (a
/// read failure of the consumer manifest, or a self-rooted workspace declaring no entry) still
/// carries the suffix: the clause names the file in its role there, and the suffix names it in the
/// same shape on every message.
fn diagnostic(audited: &Utf8Path, mut message: String) -> Diagnostic {
    // A TOML parse error ends in a newline, which would strand the suffix on a line of its own.
    message.truncate(message.trim_end().len());
    message.push_str(&format!("{AUDITED_MANIFEST}`{audited}`"));
    Diagnostic {
        code: Code::RuntimeDependencyContract,
        severity: Code::RuntimeDependencyContract.severity(),
        pointer: JsonPointer::root(),
        span: None,
        message,
        remedy: Some("declare the generated client's runtime dependencies in the consuming package's Cargo.toml using the documented supported ranges and features".to_owned()),
        interpretation: None,
        claim: OutcomeClaim::of(Code::RuntimeDependencyContract.severity()),
    }
}

#[cfg(test)]
mod tests;
