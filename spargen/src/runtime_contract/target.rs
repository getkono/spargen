//! Evaluating a `[target.<key>.dependencies]` key the way Cargo does, for the target being built
//! or, with none visible, for every builtin target it could be.

use std::collections::BTreeMap;

use cfg_expr::expr::TargetMatcher;
use cfg_expr::targets::{Endian, TargetInfo};
use cfg_expr::{Expression, Predicate, TargetPredicate};

use super::under_build_script;

/// What the audit knows about the target it audits for.
///
/// Cargo applies a `[target.<key>.dependencies]` table by evaluating `<key>` for the target being
/// built, so whether a table supplies a dependency is a question about one target, not about how
/// the key is spelled.
pub(super) enum TargetContext {
    /// A build script: Cargo describes the target being built through `TARGET` and `CARGO_CFG_*`.
    Build(BuildTarget),
    /// No target is visible. A proc-macro runs inside rustc, and Cargo gives a crate being compiled
    /// neither `TARGET` nor any `CARGO_CFG_*`.
    Unknown,
}

impl TargetContext {
    /// The context of this process: a build target exactly where [`under_build_script`] holds,
    /// which already requires `CARGO_CFG_TARGET_ARCH`.
    pub(super) fn from_env() -> Self {
        if under_build_script() {
            Self::Build(BuildTarget::from_vars(std::env::vars_os().filter_map(
                |(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)),
            )))
        } else {
            Self::Unknown
        }
    }
}

/// The target Cargo is building, as a build script sees it.
pub(super) struct BuildTarget {
    /// `TARGET` plus every `CARGO_CFG_*` variable, by name.
    vars: BTreeMap<String, String>,
}

impl BuildTarget {
    pub(super) fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            vars: vars
                .into_iter()
                .filter(|(key, _)| key == "TARGET" || key.starts_with("CARGO_CFG_"))
                .collect(),
        }
    }

    pub(super) fn triple(&self) -> &str {
        self.vars.get("TARGET").map_or("", String::as_str)
    }

    /// `CARGO_CFG_<NAME>` for a cfg name as it is written in source.
    fn cfg(&self, name: &str) -> Option<&str> {
        let variable = format!("CARGO_CFG_{}", name.to_ascii_uppercase().replace('-', "_"));
        self.vars.get(&variable).map(String::as_str)
    }

    /// Whether the comma-joined `CARGO_CFG_<NAME>` list holds `value`. Cargo joins a cfg that has
    /// several values (`target_family`, `target_feature`, …) with commas.
    fn cfg_contains(&self, name: &str, value: &str) -> bool {
        self.cfg(name)
            .is_some_and(|values| values.split(',').any(|item| item == value))
    }
}

/// cfg-expr owns parsing, the logic, and the builtin target database; this is only the mapping
/// from its target predicates to the variables Cargo sets.
impl TargetMatcher for BuildTarget {
    fn matches(&self, predicate: &TargetPredicate) -> bool {
        match predicate {
            // An empty ABI or environment is left unset, and a cfg spells it as the empty string —
            // the same rule cfg-expr applies to its own builtin targets.
            TargetPredicate::Abi(abi) => self.cfg("target_abi").unwrap_or("") == abi.as_str(),
            TargetPredicate::Env(env) => self.cfg("target_env").unwrap_or("") == env.as_str(),
            TargetPredicate::Arch(arch) => self.cfg("target_arch") == Some(arch.as_str()),
            TargetPredicate::Os(os) => self.cfg("target_os") == Some(os.as_str()),
            TargetPredicate::Vendor(vendor) => self.cfg("target_vendor") == Some(vendor.as_str()),
            TargetPredicate::Family(family) => self.cfg_contains("target_family", family.as_str()),
            TargetPredicate::HasAtomic(atomic) => {
                self.cfg_contains("target_has_atomic", &atomic.to_string())
            }
            TargetPredicate::PointerWidth(width) => {
                self.cfg("target_pointer_width")
                    .and_then(|value| value.parse::<u8>().ok())
                    == Some(*width)
            }
            TargetPredicate::Endian(endian) => {
                self.cfg("target_endian")
                    == Some(match endian {
                        Endian::big => "big",
                        Endian::little => "little",
                    })
            }
            TargetPredicate::Panic(panic) => self.cfg("panic") == Some(panic.as_str()),
        }
    }
}

/// One target a table key is evaluated against.
#[derive(Clone, Copy)]
pub(super) enum Subject<'t> {
    /// The target being built.
    Build(&'t BuildTarget),
    /// A target from cfg-expr's builtin database.
    Builtin(&'static TargetInfo),
}

impl<'t> Subject<'t> {
    pub(super) fn triple(self) -> &'t str {
        match self {
            Self::Build(build) => build.triple(),
            Self::Builtin(info) => info.triple.as_str(),
        }
    }
}

/// One predicate's value for `subject`, or `None` when it has no value there.
///
/// `feature`, `test`, `debug_assertions` and `proc_macro` never have one: Cargo does not select
/// target tables by them. Build flags and target features are known only for a build target, not
/// for a builtin one, where `RUSTFLAGS` could set anything.
pub(super) fn predicate_value(predicate: &Predicate<'_>, subject: Subject<'_>) -> Option<bool> {
    match (predicate, subject) {
        (Predicate::Target(target), Subject::Build(build)) => Some(build.matches(target)),
        (Predicate::Target(target), Subject::Builtin(info)) => Some(target.matches(info)),
        (Predicate::TargetFeature(feature), Subject::Build(build)) => {
            Some(build.cfg_contains("target_feature", feature))
        }
        (Predicate::Flag(flag), Subject::Build(build)) => Some(build.cfg(flag).is_some()),
        (Predicate::KeyValue { key, val }, Subject::Build(build)) => {
            Some(build.cfg_contains(key, val))
        }
        (
            Predicate::TargetFeature(_) | Predicate::Flag(_) | Predicate::KeyValue { .. },
            Subject::Builtin(_),
        )
        | (
            Predicate::Feature(_)
            | Predicate::Test
            | Predicate::DebugAssertions
            | Predicate::ProcMacro,
            _,
        ) => None,
    }
}

/// Why `predicate` has no value for `subject`.
fn unevaluable_reason(predicate: &Predicate<'_>, subject: Subject<'_>) -> String {
    let build_only = |spelled: String| {
        format!(
            "`{spelled}` depends on the build's flags, which are not known for `{}`",
            subject.triple()
        )
    };
    match predicate {
        Predicate::Feature(feature) => {
            format!("`feature = \"{feature}\"` does not select target tables")
        }
        Predicate::Test => "`test` does not select target tables".to_owned(),
        Predicate::DebugAssertions => "`debug_assertions` does not select target tables".to_owned(),
        Predicate::ProcMacro => "`proc_macro` does not select target tables".to_owned(),
        Predicate::TargetFeature(feature) => build_only(format!("target_feature = \"{feature}\"")),
        Predicate::Flag(flag) => build_only((*flag).to_owned()),
        Predicate::KeyValue { key, val } => build_only(format!("{key} = \"{val}\"")),
        Predicate::Target(target) => format!("`{target:?}` has no value here"),
    }
}

/// A `[target.<key>]` table key, read the way Cargo reads it.
pub(super) enum TableKey<'k> {
    /// `cfg(…)`, evaluated for a target. Boxed: a parsed expression is an order of magnitude
    /// larger than the other variants.
    Cfg(Box<Expression>),
    /// Any other key names exactly one target by its triple.
    Triple(&'k str),
    /// A `cfg(…)` key that does not parse, with why. Cargo rejects such a manifest itself.
    Invalid(String),
}

impl<'k> TableKey<'k> {
    pub(super) fn parse(key: &'k str) -> Self {
        if key.starts_with("cfg(") {
            match Expression::parse(key) {
                Ok(expression) => Self::Cfg(Box::new(expression)),
                Err(error) => Self::Invalid(error.reason.to_string()),
            }
        } else {
            Self::Triple(key)
        }
    }

    /// Whether Cargo applies this table on `subject`: `None` when that cannot be known.
    pub(super) fn evaluate(&self, subject: Subject<'_>) -> Option<bool> {
        match self {
            Self::Cfg(expression) => {
                expression.eval(|predicate| predicate_value(predicate, subject))
            }
            Self::Triple(triple) => Some(subject.triple() == *triple),
            Self::Invalid(_) => None,
        }
    }

    /// Why [`TableKey::evaluate`] had no answer for `subject`.
    pub(super) fn unevaluable(&self, subject: Subject<'_>) -> String {
        match self {
            Self::Cfg(expression) => expression
                .predicates()
                .find_map(|predicate| {
                    predicate_value(&predicate, subject)
                        .is_none()
                        .then(|| unevaluable_reason(&predicate, subject))
                })
                .unwrap_or_else(|| format!("it has no value for `{}`", subject.triple())),
            Self::Invalid(reason) => format!("it does not parse: {reason}"),
            Self::Triple(_) => format!("it has no value for `{}`", subject.triple()),
        }
    }
}

/// `[target.<key>.dependencies]`, spelled as a manifest would write it.
pub(super) fn target_table_name(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        format!("`[target.{key}.dependencies]`")
    } else if !key.contains(['\'', '\n', '\r']) {
        format!("`[target.'{key}'.dependencies]`")
    } else {
        format!("`[target.{key:?}.dependencies]`")
    }
}
