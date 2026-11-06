//! Cargo manifest auditing for the runtime required by generated output. The lowered API
//! determines the requirements, while Cargo remains responsible for resolving the consumer's
//! declared graph.
//!
//! This file is the audit's driver: it reads the consumer manifest, locates the workspace root,
//! runs every requirement through the checks, and owns the `E023` diagnostic. The rest is split
//! by concern:
//!
//! - `integration` — the Cargo-integration policy ([`CargoEnvironment`](integration::CargoEnvironment)), the build-script
//!   manifest, and the `cargo:rerun-if-changed` lines;
//! - `requirements` — the tested dependency floors, what one lowered API requires, and the
//!   requirement table both the audit and `spargen deps` read;
//! - `target` — how a `[target.<key>.dependencies]` key is evaluated for the target being built;
//! - `workspace` — locating the workspace root a `workspace = true` declaration inherits from, and
//!   reading the `Cargo.lock` beside it;
//! - `check` — the rules each dependency declaration is held to.

mod check;
mod integration;
mod requirements;
mod target;
mod workspace;

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::Diagnostic;
use crate::Code;

use check::{
    check_dependency, reads_underscore_default_features, DependencyCheck, Versions, WorkspaceOrigin,
};
use integration::cargo_home;
#[cfg(test)]
pub(crate) use integration::rerun_if_changed_lines;
pub(crate) use integration::{build_script_manifest, cargo_environment, print_rerun};
use requirements::requirement_table;
pub(crate) use requirements::RuntimeRequirements;
pub use requirements::{RequiredDependency, Requirements};
use target::TargetContext;
use workspace::{
    absolutized, cargo_source_cache, lockfile_path, read_lock, unpublished, workspace_root, Lock,
    WorkspaceRoot,
};

pub(crate) struct Audit {
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Every file the audit read, for rebuild tracking: the consumer manifest, its workspace
    /// manifest, and the `Cargo.lock` a `publish = false` package resolves with.
    pub(crate) manifests: Vec<Utf8PathBuf>,
}

pub(crate) fn audit(manifest_path: &Utf8Path, requirements: &RuntimeRequirements) -> Audit {
    // A real build climbs as Cargo does, to the filesystem root.
    audit_in(
        manifest_path,
        requirements,
        &TargetContext::from_env(),
        cargo_home().as_deref(),
        None,
    )
}

/// [`audit`] for an explicit target context and Cargo home directory, so no test has to mutate
/// the process environment, and an explicit `ceiling` on the workspace-root walk (see
/// [`workspace_root`]), so no test's outcome depends on the manifests that happen to sit above its
/// temporary directory.
fn audit_in(
    manifest_path: &Utf8Path,
    requirements: &RuntimeRequirements,
    target: &TargetContext,
    cargo_home: Option<&Utf8Path>,
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

    let underscore_default_features = reads_underscore_default_features(&manifest, workspace);
    // A `publish = false` package builds only with the versions its workspace's lockfile
    // resolves, so that lockfile is read, and named to Cargo beside the manifests, whose edits it
    // follows. A publishable one's users resolve its requirements themselves, so it is not, and
    // neither is one in Cargo's source cache: that is a git or registry dependency of another
    // workspace, built with the other workspace's lockfile.
    let lock = unpublished(&manifest, workspace).then(|| {
        if let Some(cache) = cargo_source_cache(manifest_path, cargo_home) {
            return Lock::Foreign(cache);
        }
        let package = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str);
        read_lock(lockfile_path(&root), package)
    });
    if let Some(path) = lock.as_ref().and_then(Lock::existing) {
        manifests.push(path.to_path_buf());
    }
    let versions = lock
        .as_ref()
        .map_or(Versions::Publishable, Versions::Unpublished);
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
                underscore_default_features,
                versions,
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
/// It is the only constructor, so no message can leave out which file it is about, on the build
/// script and macro paths alike (a manifest-discovery mismatch shows on the macro path, #71). A
/// message that already names this file in its own clause (a read failure of the consumer
/// manifest, or a self-rooted workspace declaring no entry) still carries the suffix: the clause
/// names the file in its role there, and the suffix names it in the same shape on every message.
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
