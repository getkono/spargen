//! The rules one required dependency's declarations are held to: where it may be declared,
//! `workspace = true` resolution, version floor, features, `default-features`, `optional`, and
//! renames.

use std::collections::BTreeSet;

use camino::Utf8Path;
use cfg_expr::targets::{get_builtin_target_by_triple, ALL_BUILTINS};
use cfg_expr::Expression;
use semver::{Op, Version, VersionReq};

use crate::diag::Diagnostic;

use super::diagnostic;
use super::requirements::{Dependency, Placement, BLOCKING_GATE, WASM_ANCHOR};
use super::target::{predicate_value, target_table_name, Subject, TableKey, TargetContext};
use super::workspace::{ForeignSource, Lock};

pub(super) struct DependencyCheck<'a> {
    pub(super) placement: Placement,
    pub(super) dependency: Dependency,
    pub(super) features: &'a [&'a str],
    pub(super) require_no_defaults: bool,
    pub(super) require_optional: bool,
    /// Where a `workspace = true` dependency resolves from, so an unresolvable inheritance can say
    /// what actually happened rather than only that it failed.
    pub(super) origin: WorkspaceOrigin<'a>,
    /// The absolutized consumer manifest the audit read, which every diagnostic names.
    pub(super) audited: &'a Utf8Path,
    /// Whether Cargo reads the underscore `default_features` spelling in the audited manifest
    /// ([`reads_underscore_default_features`]), decided once per manifest.
    pub(super) underscore_default_features: bool,
    /// What a declaration's version is held to, decided once per manifest.
    pub(super) versions: Versions<'a>,
}

/// What a declaration's version is held to: its requirement string, or the version the
/// workspace's `Cargo.lock` resolves it to.
#[derive(Clone, Copy)]
pub(super) enum Versions<'a> {
    /// The package can be published, so its users resolve its requirements against their own
    /// lockfiles, and the requirement is what is checked.
    Publishable,
    /// The package sets `publish = false`. Where this lock locks the package, the version it
    /// resolves each crate to is checked; otherwise the requirement is, saying why.
    Unpublished(&'a Lock),
}

/// What the workspace lookup found, for the one diagnostic that has to explain itself.
#[derive(Clone, Copy)]
pub(super) enum WorkspaceOrigin<'a> {
    /// A workspace manifest was found and parsed.
    Resolved(&'a Utf8Path),
    /// One was found but could not be read or parsed.
    ///
    /// Only a root the audit actually read reaches here — one `package.workspace` named, or one
    /// the walk parsed that then failed on the audit's own read — so its failure is already
    /// reported as a diagnostic of its own, and repeating the reason here would print it twice.
    Unreadable(&'a Utf8Path),
    /// None was found, having searched upwards from `searched_from`.
    NotFound {
        searched_from: &'a Utf8Path,
        /// The nearest ancestor manifest the walk skipped because it could not be read, with the
        /// failure. Rendered as a conditional hint, never as "its workspace manifest": nothing
        /// established it was one. It carries its reason because this message is the only place
        /// the failure appears — the candidate is never audited as a manifest.
        skipped: Option<(&'a Utf8Path, &'a str)>,
    },
}

pub(super) fn check_dependency(
    manifest: &toml::Value,
    workspace: Option<&toml::Value>,
    check: DependencyCheck<'_>,
    target: &TargetContext,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match check.placement {
        Placement::Dependencies => {
            let Some(declaration) = dotted_get(manifest, "dependencies", check.dependency.name)
            else {
                diagnostics.push(diagnostic(check.audited, missing_message(check.dependency)));
                return;
            };
            check_declaration(workspace, declaration, &check, "", diagnostics);
        }
        Placement::NativeTarget => {
            check_native_target(manifest, workspace, &check, target, diagnostics);
        }
    }
}

pub(super) fn missing_message(dependency: Dependency) -> String {
    format!(
        "generated client requires `{}`; add `{}` with version `{}`",
        dependency.name, dependency.name, dependency.floor
    )
}

/// The message for a required Cargo feature that `dependency`'s declarations leave off, ending
/// with `context`.
fn missing_feature_message(feature: &str, dependency: &str, context: &str) -> String {
    format!("generated client requires Cargo feature `{feature}` on `{dependency}`{context}")
}

/// Locate and check a dependency that belongs in a native-only target table — the blocking
/// client's `tokio`.
///
/// Every `[target.<key>.dependencies]` table declaring it is evaluated the way Cargo evaluates it,
/// never matched by spelling. A table counts only when it is native-only (it cannot apply on
/// [`WASM_ANCHOR`], where generated code never names the dependency) and it applies to the build:
///
/// - in a build script, to the target being built, and a build [`BLOCKING_GATE`] excludes needs no
///   such table at all;
/// - with no target visible (a proc-macro), the counting tables must together cover every builtin
///   target outside the wasm family, since any of them could be the one being built.
///
/// `[dependencies]` never counts: it applies on wasm too.
fn check_native_target(
    manifest: &toml::Value,
    workspace: Option<&toml::Value>,
    check: &DependencyCheck<'_>,
    target: &TargetContext,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let name = check.dependency.name;
    if let TargetContext::Build(build) = target {
        let gate =
            Expression::parse(BLOCKING_GATE).expect("the blocking gate is a valid cfg expression");
        if gate.eval(|predicate| predicate_value(predicate, Subject::Build(build))) != Some(true) {
            return;
        }
    }
    let wasm = get_builtin_target_by_triple(WASM_ANCHOR);
    let domain = match target {
        TargetContext::Build(_) => Vec::new(),
        TargetContext::Unknown => ALL_BUILTINS
            .iter()
            .filter(|info| !info.families.iter().any(|family| family.as_str() == "wasm"))
            .collect::<Vec<_>>(),
    };

    // Why each declaration that did not count was passed over, in key order.
    let mut rejected = Vec::new();
    if dotted_get(manifest, "dependencies", name).is_some() {
        rejected.push(format!(
            "`[dependencies]` applies on `{WASM_ANCHOR}`, where the blocking client is compiled \
             out, so `{name}` must be native-only"
        ));
    }
    let mut candidates = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(key, table)| Some((key.as_str(), table.get("dependencies")?.get(name)?)))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(right.0));

    // Each counting table, with whether it applies to each target it is checked for: the build
    // target alone, or every target in `domain`.
    let mut counting = Vec::new();
    for (key, declaration) in candidates {
        let table = target_table_name(key);
        let parsed = TableKey::parse(key);
        if let TableKey::Invalid(reason) = &parsed {
            rejected.push(format!("{table} does not parse: {reason}"));
            continue;
        }
        let Some(wasm) = wasm else {
            rejected.push(format!(
                "{table} cannot be shown to be native-only: no `{WASM_ANCHOR}` target is known"
            ));
            continue;
        };
        match parsed.evaluate(Subject::Builtin(wasm)) {
            Some(false) => {}
            Some(true) => {
                rejected.push(format!(
                    "{table} applies on `{WASM_ANCHOR}`, where the blocking client is compiled \
                     out, so `{name}` must be native-only"
                ));
                continue;
            }
            None => {
                rejected.push(format!(
                    "{table} cannot be evaluated: {}",
                    parsed.unevaluable(Subject::Builtin(wasm))
                ));
                continue;
            }
        }
        match target {
            TargetContext::Build(build) => match parsed.evaluate(Subject::Build(build)) {
                Some(true) => counting.push((table, declaration, vec![true])),
                Some(false) => {
                    rejected.push(format!("{table} does not apply to `{}`", build.triple()));
                }
                None => rejected.push(format!(
                    "{table} cannot be evaluated: {}",
                    parsed.unevaluable(Subject::Build(build))
                )),
            },
            TargetContext::Unknown => {
                let values = domain
                    .iter()
                    .map(|info| parsed.evaluate(Subject::Builtin(info)))
                    .collect::<Vec<_>>();
                if values.contains(&Some(true)) {
                    let applies = values.iter().map(|value| *value == Some(true)).collect();
                    counting.push((table, declaration, applies));
                } else if let Some(index) = values.iter().position(Option::is_none) {
                    rejected.push(format!(
                        "{table} cannot be evaluated: {}",
                        parsed.unevaluable(Subject::Builtin(domain[index]))
                    ));
                } else {
                    rejected.push(match &parsed {
                        // Cargo accepts any triple as a key, a custom target's included. Missing
                        // from the builtin list only means spargen cannot tell, not that the table
                        // applies to nothing native.
                        TableKey::Triple(triple)
                            if get_builtin_target_by_triple(triple).is_none() =>
                        {
                            format!(
                                "{table} names `{triple}`, which is not a target spargen knows, so \
                                 without the build target it cannot be shown to apply to the one \
                                 being built"
                            )
                        }
                        _ => format!("{table} applies to no known non-wasm target"),
                    });
                }
            }
        }
    }

    let checked_targets = match target {
        TargetContext::Build(_) => 1,
        TargetContext::Unknown => domain.len(),
    };
    let uncovered =
        (0..checked_targets).find(|&index| !counting.iter().any(|(_, _, applies)| applies[index]));
    if counting.is_empty() || uncovered.is_some() {
        let rule = match target {
            TargetContext::Build(build) => {
                format!("evaluated for the build target `{}`", build.triple())
            }
            TargetContext::Unknown => format!(
                "a proc-macro cannot see the build target, so the tables must cover every \
                 non-wasm target; {} — use the table above, or generate from `build.rs`, where \
                 the target being built is evaluated",
                uncovered.map_or_else(
                    || "no non-wasm target is known".to_owned(),
                    |index| format!("`{}` is not covered", domain[index].triple)
                )
            ),
        };
        let mut message = format!(
            "{} under `[target.'cfg({BLOCKING_GATE})'.dependencies]` ({rule})",
            missing_message(check.dependency)
        );
        for clause in rejected {
            message.push_str("; ");
            message.push_str(&clause);
        }
        diagnostics.push(diagnostic(check.audited, message));
        return;
    }

    // With one counting table a message names no counting table; with several, each says which table it
    // is about.
    let several = counting.len() > 1;
    let mut resolved = Vec::new();
    let mut all_resolved = true;
    for (table, declaration, applies) in &counting {
        let location = if several {
            format!(" (in {table})")
        } else {
            String::new()
        };
        match check_declaration(workspace, declaration, check, &location, diagnostics) {
            Some(features) => resolved.push((features, applies)),
            None => all_resolved = false,
        }
    }
    if !all_resolved {
        // An unresolvable inheritance is already reported, and has no feature list to check.
        return;
    }
    // Cargo unifies a dependency's features across every table that applies to the build, so a
    // required feature is judged on that union, target by target.
    for feature in check.features {
        let missing_on = (0..checked_targets).find(|&index| {
            !resolved
                .iter()
                .any(|(features, applies)| applies[index] && features.contains(*feature))
        });
        if let Some(index) = missing_on {
            let context = if several {
                let triple = match target {
                    TargetContext::Build(build) => build.triple(),
                    TargetContext::Unknown => domain[index].triple.as_str(),
                };
                format!(" (not enabled by the tables that apply to `{triple}`)")
            } else {
                String::new()
            };
            diagnostics.push(diagnostic(
                check.audited,
                missing_feature_message(feature, name, &context),
            ));
        }
    }
}

/// The rules every declaration that counts is held to: `workspace = true` resolution, version,
/// features, `default-features` (also spelled `default_features` where Cargo reads it),
/// `optional` both ways, and renames. `location` is appended to each message.
///
/// A `[dependencies]` declaration's features are checked here. A native-target one's are not:
/// Cargo unifies them across every table that applies, so [`check_native_target`] judges them on
/// that union, from the feature set this returns.
///
/// Returns the declaration's resolved feature set, or `None` when its inheritance could not be
/// resolved (already reported).
fn check_declaration<'v>(
    workspace: Option<&'v toml::Value>,
    declaration: &'v toml::Value,
    check: &DependencyCheck<'_>,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BTreeSet<&'v str>> {
    let dependency = check.dependency;
    let underscore = check.underscore_default_features;
    let inherited = declaration
        .as_table()
        .and_then(|table| table.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true);
    let workspace_declaration = inherited
        .then(|| {
            workspace.and_then(|root| dotted_get(root, "workspace.dependencies", dependency.name))
        })
        .flatten();
    if inherited && workspace_declaration.is_none() {
        // Name where the lookup actually went, so the message tells "the workspace has no such
        // entry" from "spargen never found the workspace": the remedies for those are opposite.
        let origin = match check.origin {
            WorkspaceOrigin::Resolved(path) => {
                format!("`{path}` declares no `{}` there", dependency.name)
            }
            WorkspaceOrigin::Unreadable(path) => {
                format!("its workspace manifest `{path}` could not be read")
            }
            WorkspaceOrigin::NotFound {
                searched_from,
                skipped: None,
            } => {
                format!("no workspace manifest was found above `{searched_from}`")
            }
            // Not found, first and unconditionally: the skipped file is only a possible cause,
            // so it is named conditionally and never as "its workspace manifest".
            WorkspaceOrigin::NotFound {
                searched_from,
                skipped: Some((path, reason)),
            } => format!(
                "no workspace manifest was found above `{searched_from}`; if the workspace root \
                 is `{path}`, it could not be read: {reason}"
            ),
        };
        diagnostics.push(diagnostic(
            check.audited,
            format!(
                "`{}` inherits from `[workspace.dependencies]`, but {origin}{location}",
                dependency.name
            ),
        ));
        return None;
    }

    let version = declaration_version(workspace_declaration.unwrap_or(declaration));
    // The workspace manifest an inherited requirement is declared in: only a resolved root
    // reaches here with an entry.
    let root = match (workspace_declaration, check.origin) {
        (Some(_), WorkspaceOrigin::Resolved(root)) => Some(root),
        _ => None,
    };
    match version {
        Some(requirement) => {
            if let Some(message) = version_message(requirement, root, check, location) {
                diagnostics.push(diagnostic(check.audited, message));
            }
        }
        None => diagnostics.push(diagnostic(check.audited, format!(
            "`{}` must declare a Cargo version requirement of `{}` or a higher compatible floor{location}",
            dependency.name, dependency.floor
        ))),
    }

    let mut features = declaration_features(workspace_declaration.unwrap_or(declaration));
    features.extend(declaration_features(declaration));
    if check.placement == Placement::Dependencies {
        for feature in check.features {
            if !features.contains(*feature) {
                diagnostics.push(diagnostic(
                    check.audited,
                    missing_feature_message(feature, dependency.name, location),
                ));
            }
        }
    }
    let defaults = if let Some(workspace_declaration) = workspace_declaration {
        declaration_default_features(workspace_declaration, underscore).unwrap_or(true)
            || declaration_default_features(declaration, underscore) == Some(true)
    } else {
        declaration_default_features(declaration, underscore).unwrap_or(true)
    };
    if check.require_no_defaults && defaults {
        diagnostics.push(diagnostic(check.audited, format!(
            "`{}` must set `default-features = false` for the supported freestanding runtime graph{location}",
            dependency.name
        )));
    }
    if check.require_optional && declaration_bool(declaration, "optional") != Some(true) {
        diagnostics.push(diagnostic(check.audited, format!(
            "`{}` must be optional because it is enabled only by the generated `blocking` feature{location}",
            dependency.name
        )));
    }
    // The mirror of the rule above: generated code names these
    // crates unconditionally, with no `cfg` to hide behind. Declaring one `optional = true` — even
    // wired into `default` — leaves a feature resolution (`--no-default-features`, or a dependent
    // that turns defaults off) in which the generated module references a crate that is not in the
    // graph, and the failure surfaces as a rustc error inside generated code rather than here.
    if !check.require_optional && declaration_bool(declaration, "optional") == Some(true) {
        diagnostics.push(diagnostic(
            check.audited,
            format!(
            "`{}` must not be optional: generated code references it unconditionally, so a build \
             with that feature disabled would not compile. Drop `optional = true`, or turn the \
             mapping off at generation time{location}",
            dependency.name
        ),
        ));
    }
    // A rename is a `package` that differs from the key. Cargo's `package` defaults to the key, so
    // `bytes = { package = "bytes", … }` is the fully-qualified spelling of an ordinary dependency
    // and renames nothing (#168). A non-string `package` is not a spelling Cargo accepts, so it is
    // not given the benefit of the doubt. An inheriting line's own `package` is not read: Cargo
    // warns that it is an unused key and resolves the root's declaration, so only the root's
    // `package` renames an inherited dependency (#317) — the same reason the version above comes
    // from the root alone.
    if workspace_declaration
        .unwrap_or(declaration)
        .get("package")
        .is_some_and(|package| package.as_str() != Some(dependency.name))
    {
        diagnostics.push(diagnostic(check.audited, format!(
            "`{}` cannot be renamed because generated code references that canonical crate name{location}",
            dependency.name
        )));
    }
    Some(features)
}

/// Why a publishable package's requirement is checked rather than its lockfile, closing every
/// out-of-range requirement message for one.
pub(super) const PUBLISHABLE: &str = "the package can be published, and its users resolve the \
     requirement rather than this workspace's lockfile; with `publish = false` the version \
     `Cargo.lock` resolves is checked instead";

/// The diagnostic for a declaration whose version falls outside the supported range, or `None`
/// when it is inside it. `root` is the workspace manifest an inherited `requirement` is declared
/// in, which the message names as the entry to change.
///
/// For a `publish = false` package whose lockfile locks a version of the crate `requirement`
/// matches, that locked version is what is held to the range, and `requirement` may be any
/// requirement Cargo accepts. Otherwise `requirement` itself is, by [`supported_requirement`].
/// Either way the message says which of the two it checked and why.
fn version_message(
    requirement: &str,
    root: Option<&Utf8Path>,
    check: &DependencyCheck<'_>,
    location: &str,
) -> Option<String> {
    let dependency = check.dependency;
    let name = dependency.name;
    let floor = dependency.floor;
    let range = format!(">={floor}, <{}", dependency.ceiling());
    let entry = root.map_or_else(String::new, |root| {
        format!(" in the `{name}` entry of `[workspace.dependencies]` in `{root}`")
    });
    let why = match check.versions {
        Versions::Publishable => PUBLISHABLE.to_owned(),
        Versions::Unpublished(Lock::Foreign(ForeignSource::Cache(cache))) => {
            format!(
                "the package is built from Cargo's source cache `{cache}`, as a dependency of \
                 another workspace whose `Cargo.lock` resolves it"
            )
        }
        Versions::Unpublished(Lock::Foreign(ForeignSource::Directory(package))) => {
            format!(
                "the package is built from the directory source `{package}` its \
                 `.cargo-checksum.json` marks, as a dependency of another workspace whose \
                 `Cargo.lock` resolves it"
            )
        }
        Versions::Unpublished(Lock::Absent(path)) => {
            format!("no `Cargo.lock` exists at `{path}`")
        }
        Versions::Unpublished(Lock::Unusable { path, reason }) => {
            format!("`{path}` could not be used: {reason}")
        }
        Versions::Unpublished(Lock::Locked { path, dependencies }) => {
            let matching = VersionReq::parse(requirement)
                .map(|parsed| {
                    dependencies
                        .iter()
                        .filter(|(locked, version)| locked == name && parsed.matches(version))
                        .map(|(_, version)| version)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !matching.is_empty() {
                let supported = dependency.floor_version()..dependency.ceiling();
                return matching
                    .into_iter()
                    .find(|version| !supported.contains(*version))
                    .map(|version| {
                        format!(
                            "`{name}` is locked at `{version}` in `{path}`, outside the supported \
                             range {range}; replace the requirement `{requirement}`{entry} with \
                             `{floor}` or a higher compatible caret requirement{location}; the \
                             locked version was checked, not the requirement, because the package \
                             sets `publish = false` and `{path}` resolves its dependencies"
                        )
                    });
            }
            format!("`{path}` locks no `{name}` matching `{requirement}` for this package")
        }
    };
    (!supported_requirement(requirement, dependency)).then(|| {
        format!(
            "`{name}` version requirement `{requirement}` is outside the supported range {range}; \
             use `{floor}` or a higher compatible caret requirement{entry}{location}; the \
             requirement was checked, not a lockfile, because {why}"
        )
    })
}

fn dotted_get<'a>(value: &'a toml::Value, dotted: &str, key: &str) -> Option<&'a toml::Value> {
    let mut current = value;
    for segment in dotted.split('.') {
        current = current.get(segment)?;
    }
    current.get(key)
}

fn declaration_version(value: &toml::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("version").and_then(toml::Value::as_str))
}

fn declaration_features(value: &toml::Value) -> BTreeSet<&str> {
    value
        .get("features")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect()
}

fn declaration_bool(value: &toml::Value, key: &str) -> Option<bool> {
    value.get(key).and_then(toml::Value::as_bool)
}

/// A declaration's `default-features`, as Cargo resolves it. Below edition 2024 Cargo also
/// accepts the deprecated `default_features` (`underscore`), and with both keys present the
/// hyphenated one wins; from edition 2024 it refuses the manifest instead, so the underscore key
/// is not read there and the declaration keeps its defaults.
fn declaration_default_features(value: &toml::Value, underscore: bool) -> Option<bool> {
    declaration_bool(value, "default-features").or_else(|| {
        underscore
            .then(|| declaration_bool(value, "default_features"))
            .flatten()
    })
}

/// Whether Cargo reads the underscore `default_features` spelling in the audited `manifest`:
/// only when its package's edition is below 2024. The edition is the package's own, or its
/// `[workspace.package]` one under `edition.workspace = true`, and 2015 when none is stated —
/// also for a `[workspace.dependencies]` entry, which Cargo judges by the inheriting member's
/// edition. An edition that does not resolve to a known year is treated as 2024 or later.
pub(super) fn reads_underscore_default_features(
    manifest: &toml::Value,
    workspace: Option<&toml::Value>,
) -> bool {
    let Some(package) = manifest.get("package") else {
        return true;
    };
    let edition = match package.get("edition") {
        None => return true,
        Some(edition) if edition.get("workspace").and_then(toml::Value::as_bool) == Some(true) => {
            workspace.and_then(|root| dotted_get(root, "workspace.package", "edition"))
        }
        Some(edition) => Some(edition),
    };
    edition
        .and_then(toml::Value::as_str)
        .and_then(|year| year.parse::<u16>().ok())
        .is_some_and(|year| year < 2024)
}

/// Whether `raw`, a Cargo version requirement, is one the audit accepts for `dependency`.
///
/// It must parse as exactly one comparator with no pre-release, using either
///
/// - the caret operator, spelled bare (`x.y.z`) or with `^` (`^x.y.z`), or
/// - the exact operator, `=x.y.z` (or a partial `=x.y`),
///
/// and the lowest version it names, with omitted components read as `0`, must be at or above
/// the tested floor and below the next breaking line ([`Dependency::ceiling`]). So `=floor` and
/// `=` of any later release on that line are accepted, like the floor and a higher caret floor.
/// Every other operator (`~`, `>=`, `<`, `*`, wildcards), a range of several comparators, and a
/// requirement that does not parse are refused.
pub(super) fn supported_requirement(raw: &str, dependency: Dependency) -> bool {
    let Ok(requirement) = VersionReq::parse(raw) else {
        return false;
    };
    if requirement.comparators.len() != 1 {
        return false;
    }
    let comparator = &requirement.comparators[0];
    if !matches!(comparator.op, Op::Caret | Op::Exact) || !comparator.pre.is_empty() {
        return false;
    }
    let lower = Version::new(
        comparator.major,
        comparator.minor.unwrap_or(0),
        comparator.patch.unwrap_or(0),
    );
    lower >= dependency.floor_version() && lower < dependency.ceiling()
}
