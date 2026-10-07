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
            check_declaration(workspace, declaration, &check, true, "", diagnostics);
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

    // With one counting table every message reads exactly as it always has; with several, each
    // says which table it is about.
    let several = counting.len() > 1;
    let mut resolved = Vec::new();
    let mut all_resolved = true;
    for (table, declaration, applies) in &counting {
        let location = if several {
            format!(" (in {table})")
        } else {
            String::new()
        };
        match check_declaration(workspace, declaration, check, false, &location, diagnostics) {
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
                    TargetContext::Build(build) => build.triple().to_owned(),
                    TargetContext::Unknown => domain[index].triple.to_string(),
                };
                format!(" (not enabled by the tables that apply to `{triple}`)")
            } else {
                String::new()
            };
            diagnostics.push(diagnostic(
                check.audited,
                format!("generated client requires Cargo feature `{feature}` on `{name}`{context}"),
            ));
        }
    }
}

/// The rules every declaration that counts is held to: `workspace = true` resolution, version,
/// features (when `check_features`), `default-features`, `optional` both ways, and renames.
/// `location` is appended to each message.
///
/// Returns the declaration's resolved feature set, or `None` when its inheritance could not be
/// resolved (already reported).
fn check_declaration<'v>(
    workspace: Option<&'v toml::Value>,
    declaration: &'v toml::Value,
    check: &DependencyCheck<'_>,
    check_features: bool,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BTreeSet<&'v str>> {
    let dependency = check.dependency;
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
        // Name where the lookup actually went. The original report of this diagnostic could not
        // tell "the workspace has no such entry" from "spargen never found the workspace", and the
        // remedies for those are opposite.
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
    match version {
        Some(requirement) if supported_requirement(requirement, dependency) => {}
        Some(requirement) => diagnostics.push(diagnostic(check.audited, format!(
            "`{}` version requirement `{requirement}` is outside the supported range >={}, <{}; use `{}` or a higher compatible caret requirement{location}",
            dependency.name,
            dependency.floor,
            dependency.ceiling(),
            dependency.floor
        ))),
        None => diagnostics.push(diagnostic(check.audited, format!(
            "`{}` must declare a Cargo version requirement of `{}` or a higher compatible floor{location}",
            dependency.name, dependency.floor
        ))),
    }

    let mut features = declaration_features(workspace_declaration.unwrap_or(declaration));
    features.extend(declaration_features(declaration));
    if check_features {
        for feature in check.features {
            if !features.contains(*feature) {
                diagnostics.push(diagnostic(
                    check.audited,
                    format!(
                        "generated client requires Cargo feature `{feature}` on `{}`{location}",
                        dependency.name
                    ),
                ));
            }
        }
    }
    let defaults = if let Some(workspace_declaration) = workspace_declaration {
        declaration_bool(workspace_declaration, "default-features").unwrap_or(true)
            || declaration_bool(declaration, "default-features") == Some(true)
    } else {
        declaration_bool(declaration, "default-features").unwrap_or(true)
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
    // The mirror of the rule above, and the one that was missing: generated code names these
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
