//! Cargo manifest auditing for the runtime required by generated output.
//!
//! This is facade plumbing rather than a public subsystem: the lowered API determines the
//! requirements, while Cargo remains responsible for resolving the consumer's declared graph.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use cfg_expr::expr::TargetMatcher;
use cfg_expr::targets::{get_builtin_target_by_triple, Endian, TargetInfo, ALL_BUILTINS};
use cfg_expr::{Expression, Predicate, TargetPredicate};
use semver::{Op, Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::ir::{Api, MediaType, Prim, TypeGraph, TypeId, TypeKind};
use crate::{Code, JsonPointer, Spec};

const BYTES: Dependency = Dependency::stable("bytes", "1.12.1", 2);
const FUTURES_CORE: Dependency = Dependency::unstable("futures-core", "0.3.32", 0, 4);
const REQWEST: Dependency = Dependency::unstable("reqwest", "0.12.28", 0, 13);
const SECRECY: Dependency = Dependency::unstable("secrecy", "0.10.3", 0, 11);
const SERDE: Dependency = Dependency::stable("serde", "1.0.229", 2);
const SERDE_JSON: Dependency = Dependency::stable("serde_json", "1.0.151", 2);
const QUICK_XML: Dependency = Dependency::unstable("quick-xml", "0.41.0", 0, 42);
const UUID: Dependency = Dependency::stable("uuid", "1.24.0", 2);
const TIME: Dependency = Dependency::unstable("time", "0.3.55", 0, 4);
const TOKIO: Dependency = Dependency::stable("tokio", "1.53.1", 2);

#[derive(Debug, Clone, Copy)]
struct Dependency {
    name: &'static str,
    floor: &'static str,
    ceiling_major: u64,
    ceiling_minor: u64,
}

impl Dependency {
    const fn stable(name: &'static str, floor: &'static str, ceiling_major: u64) -> Self {
        Self {
            name,
            floor,
            ceiling_major,
            ceiling_minor: 0,
        }
    }

    const fn unstable(
        name: &'static str,
        floor: &'static str,
        ceiling_major: u64,
        ceiling_minor: u64,
    ) -> Self {
        Self {
            name,
            floor,
            ceiling_major,
            ceiling_minor,
        }
    }

    fn floor_version(self) -> Version {
        Version::parse(self.floor).expect("runtime dependency floors are valid semver")
    }

    fn ceiling(self) -> Version {
        Version::new(self.ceiling_major, self.ceiling_minor, 0)
    }
}

/// The dependency capabilities referenced by one generated module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuntimeRequirements {
    pub(crate) reqwest_json: bool,
    pub(crate) reqwest_multipart: bool,
    pub(crate) bytes_serde: bool,
    pub(crate) streams: bool,
    pub(crate) xml: bool,
    pub(crate) uuid: bool,
    pub(crate) time: bool,
}

impl RuntimeRequirements {
    pub(crate) fn for_api(api: &Api, spec: &Spec) -> Self {
        Self {
            reqwest_json: api.operations.iter().any(|operation| {
                operation
                    .request_body
                    .as_ref()
                    .is_some_and(|body| body.media == MediaType::Json)
            }),
            reqwest_multipart: api.operations.iter().any(|operation| {
                operation
                    .request_body
                    .as_ref()
                    .is_some_and(|body| body.media == MediaType::Multipart)
            }),
            bytes_serde: bytes_need_serde(api),
            streams: api.uses_streams(),
            xml: api.uses_xml(),
            uuid: spec.uuid
                && api.types.iter().any(|(_, definition)| {
                    matches!(definition.kind, TypeKind::Primitive(Prim::Uuid))
                }),
            time: spec.time && api.uses_time(),
        }
    }
}

fn bytes_need_serde(api: &Api) -> bool {
    let model_needs_serde =
        api.types
            .iter()
            .any(|(_, definition)| match &definition.kind {
                TypeKind::Struct(object) => {
                    object
                        .fields
                        .iter()
                        .any(|field| contains_bytes(&api.types, field.ty.id, &mut BTreeSet::new()))
                        || match &object.additional {
                            crate::ir::AdditionalProps::Typed(ty) => {
                                contains_bytes(&api.types, ty.id, &mut BTreeSet::new())
                            }
                            crate::ir::AdditionalProps::Allow
                            | crate::ir::AdditionalProps::Deny => false,
                        }
                }
                TypeKind::Union(union) => union
                    .variants
                    .iter()
                    .any(|variant| contains_bytes(&api.types, variant.ty.id, &mut BTreeSet::new())),
                _ => false,
            });
    model_needs_serde
        || api.operations.iter().any(|operation| {
            operation.params.iter().any(|parameter| {
                matches!(
                    &parameter.style,
                    crate::ir::ParamStyle::Content(MediaType::Json)
                ) && contains_bytes(&api.types, parameter.ty.id, &mut BTreeSet::new())
            }) || operation.request_body.as_ref().is_some_and(|body| {
                body.ty
                    .is_some_and(|ty| typed_body_needs_bytes_serde(api, body.media, ty.id))
            }) || operation
                .responses
                .by_status
                .iter()
                .map(|(_, response)| response)
                .chain(operation.responses.default.iter())
                .any(|response| {
                    response.body.is_some_and(|ty| {
                        response
                            .media
                            .is_some_and(|media| typed_body_needs_bytes_serde(api, media, ty.id))
                    })
                })
        })
}

fn typed_body_needs_bytes_serde(api: &Api, media: MediaType, id: TypeId) -> bool {
    serde_body_media(media)
        && (media.stream_framing().is_some()
            || !matches!(
                api.types.get(id).map(|definition| &definition.kind),
                Some(TypeKind::Bytes)
            ))
        && contains_bytes(&api.types, id, &mut BTreeSet::new())
}

fn serde_body_media(media: MediaType) -> bool {
    matches!(
        media,
        MediaType::Json
            | MediaType::FormUrlEncoded
            | MediaType::Xml
            | MediaType::EventStream
            | MediaType::Ndjson
            | MediaType::JsonSequence
    )
}

fn contains_bytes(types: &TypeGraph, id: TypeId, visiting: &mut BTreeSet<TypeId>) -> bool {
    if !visiting.insert(id) {
        return false;
    }
    let contains = match types.get(id).map(|definition| &definition.kind) {
        Some(TypeKind::Bytes) => true,
        Some(TypeKind::Struct(object)) => {
            object
                .fields
                .iter()
                .any(|field| contains_bytes(types, field.ty.id, visiting))
                || match &object.additional {
                    crate::ir::AdditionalProps::Typed(ty) => contains_bytes(types, ty.id, visiting),
                    crate::ir::AdditionalProps::Allow | crate::ir::AdditionalProps::Deny => false,
                }
        }
        Some(TypeKind::Array(item)) => contains_bytes(types, item.id, visiting),
        Some(TypeKind::Tuple(items)) => items
            .iter()
            .any(|item| contains_bytes(types, item.id, visiting)),
        Some(TypeKind::Union(union)) => union
            .variants
            .iter()
            .any(|variant| contains_bytes(types, variant.ty.id, visiting)),
        _ => false,
    };
    visiting.remove(&id);
    contains
}

/// Whether this process is an actual Cargo build script. Only there do `cargo:` directives reach
/// Cargo and does the environment name the consuming package.
pub(crate) fn under_build_script() -> bool {
    std::env::var_os("OUT_DIR").is_some() && std::env::var_os("CARGO_CFG_TARGET_ARCH").is_some()
}

/// What the audit knows about the target it audits for.
///
/// Cargo applies a `[target.<key>.dependencies]` table by evaluating `<key>` for the target being
/// built, so whether a table supplies a dependency is a question about one target, not about how
/// the key is spelled.
enum TargetContext {
    /// A build script: Cargo describes the target being built through `TARGET` and `CARGO_CFG_*`.
    Build(BuildTarget),
    /// No target is visible. A proc-macro runs inside rustc, and Cargo gives a crate being compiled
    /// neither `TARGET` nor any `CARGO_CFG_*`.
    Unknown,
}

impl TargetContext {
    /// The context of this process: a build target exactly where [`under_build_script`] holds,
    /// which already requires `CARGO_CFG_TARGET_ARCH`.
    fn from_env() -> Self {
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
struct BuildTarget {
    /// `TARGET` plus every `CARGO_CFG_*` variable, by name.
    vars: BTreeMap<String, String>,
}

impl BuildTarget {
    fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            vars: vars
                .into_iter()
                .filter(|(key, _)| key == "TARGET" || key.starts_with("CARGO_CFG_"))
                .collect(),
        }
    }

    fn triple(&self) -> &str {
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
enum Subject<'t> {
    /// The target being built.
    Build(&'t BuildTarget),
    /// A target from cfg-expr's builtin database.
    Builtin(&'static TargetInfo),
}

impl<'t> Subject<'t> {
    fn triple(self) -> &'t str {
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
fn predicate_value(predicate: &Predicate<'_>, subject: Subject<'_>) -> Option<bool> {
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
enum TableKey<'k> {
    /// `cfg(…)`, evaluated for a target. Boxed: a parsed expression is an order of magnitude
    /// larger than the other variants.
    Cfg(Box<Expression>),
    /// Any other key names exactly one target by its triple.
    Triple(&'k str),
    /// A `cfg(…)` key that does not parse, with why. Cargo rejects such a manifest itself.
    Invalid(String),
}

impl<'k> TableKey<'k> {
    fn parse(key: &'k str) -> Self {
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
    fn evaluate(&self, subject: Subject<'_>) -> Option<bool> {
        match self {
            Self::Cfg(expression) => {
                expression.eval(|predicate| predicate_value(predicate, subject))
            }
            Self::Triple(triple) => Some(subject.triple() == *triple),
            Self::Invalid(_) => None,
        }
    }

    /// Why [`TableKey::evaluate`] had no answer for `subject`.
    fn unevaluable(&self, subject: Subject<'_>) -> String {
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
fn target_table_name(key: &str) -> String {
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
    for manifest in manifests {
        if !manifest.as_str().contains(['\n', '\r']) {
            println!("cargo:rerun-if-changed={manifest}");
        }
    }
}

/// One dependency the consuming package must declare, as spargen derived it from the lowered API.
///
/// This is the single source of truth behind both the audit (`E023`) and `spargen deps`: the two
/// read the same table, so what the audit demands and what `deps` prints cannot drift.
#[derive(Debug, Clone, Copy)]
struct Requirement {
    /// The manifest table `spargen deps` prints it in — `dependencies`, or a target-specific
    /// table.
    table: &'static str,
    /// How the audit locates it in the consumer manifest.
    placement: Placement,
    dependency: Dependency,
    features: &'static [&'static str],
    /// `default-features = false` is part of the contract (reqwest's defaults pull in a TLS
    /// stack the generated client does not choose).
    no_default_features: bool,
    /// Declared `optional = true` and wired to a Cargo feature of the consumer's own.
    optional: bool,
    /// Only required when the consumer opts in — currently the `blocking` Cargo feature.
    conditional: Option<&'static str>,
}

/// Where a requirement is declared, which decides how the audit finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// The plain `[dependencies]` table.
    Dependencies,
    /// Any native-only `[target.<key>.dependencies]` table Cargo applies to the build: the blocking
    /// client's `tokio`, which generated code names only under [`BLOCKING_GATE`].
    NativeTarget,
}

/// The `cfg` generated code puts around every use of the blocking client's `tokio` (together with
/// `feature = "blocking"`), and the table `spargen deps` prints for it.
const BLOCKING_GATE: &str = "not(target_arch = \"wasm32\")";

/// The target the native-only rule is anchored to: the only wasm target generated code supports.
/// A `tokio` table that can apply here is not native-only.
const WASM_ANCHOR: &str = "wasm32-unknown-unknown";

/// The dependency table for one lowered API. `deps` renders it; `audit` checks the consumer
/// manifest against it.
fn requirement_table(requirements: &RuntimeRequirements) -> Vec<Requirement> {
    const NONE: &[&str] = &[];
    let mut table = Vec::new();
    let mut push = |dependency: Dependency, features: &'static [&str], no_defaults: bool| {
        table.push(Requirement {
            table: "dependencies",
            placement: Placement::Dependencies,
            dependency,
            features,
            no_default_features: no_defaults,
            optional: false,
            conditional: None,
        });
    };

    push(
        BYTES,
        if requirements.bytes_serde {
            &["serde"]
        } else {
            NONE
        },
        false,
    );
    push(
        REQWEST,
        match (
            requirements.reqwest_json,
            requirements.reqwest_multipart,
            requirements.streams,
        ) {
            (true, true, true) => &["json", "multipart", "stream"],
            (true, true, false) => &["json", "multipart"],
            (true, false, true) => &["json", "stream"],
            (true, false, false) => &["json"],
            (false, true, true) => &["multipart", "stream"],
            (false, true, false) => &["multipart"],
            (false, false, true) => &["stream"],
            (false, false, false) => NONE,
        },
        true,
    );
    if requirements.streams {
        push(FUTURES_CORE, NONE, false);
    }
    push(SECRECY, NONE, false);
    push(SERDE, &["derive"], false);
    push(SERDE_JSON, NONE, false);
    if requirements.xml {
        push(QUICK_XML, &["serialize"], false);
    }
    if requirements.uuid {
        push(UUID, &["serde"], false);
    }
    if requirements.time {
        // NOT `serde`: the embedded `DateTime`/`Date` newtypes write RFC 3339 themselves, because
        // `time`'s own serde representation is a nine-integer sequence without
        // `serde-human-readable` and a space-separated form with it — neither is what OpenAPI's
        // `format: date-time` means. `formatting`/`parsing` are what the RFC 3339 codec needs.
        push(TIME, &["formatting", "parsing"], false);
    }
    // The blocking client is opt-in: it is required only from a consumer that declares its own
    // `blocking` Cargo feature, and then only off wasm, where no thread-blocking runtime exists.
    table.push(Requirement {
        table: "target.'cfg(not(target_arch = \"wasm32\"))'.dependencies",
        placement: Placement::NativeTarget,
        dependency: TOKIO,
        features: &["rt"],
        no_default_features: false,
        optional: true,
        conditional: Some("blocking"),
    });
    table
}

/// One dependency the consuming package must declare for generated output to compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequiredDependency {
    /// The crate name.
    pub name: &'static str,
    /// The version requirement to declare — the tested lower bound. A higher semver-compatible
    /// caret requirement is equally acceptable to the audit.
    pub version: &'static str,
    /// Cargo features the generated code needs enabled.
    pub features: Vec<&'static str>,
    /// Whether `default-features = false` is part of the contract.
    pub no_default_features: bool,
    /// Whether the dependency must be declared `optional = true`.
    pub optional: bool,
    /// The manifest table it belongs in (`dependencies`, or a `target.'cfg(…)'` table).
    pub table: &'static str,
    /// The consumer Cargo feature that makes this dependency necessary, when it is opt-in.
    pub required_by_feature: Option<&'static str>,
}

impl RequiredDependency {
    /// The `name = { … }` line as it would appear in `Cargo.toml`.
    pub fn manifest_line(&self) -> String {
        let mut parts = vec![format!("version = \"{}\"", self.version)];
        if self.no_default_features {
            parts.push("default-features = false".to_owned());
        }
        if !self.features.is_empty() {
            let features = self
                .features
                .iter()
                .map(|feature| format!("\"{feature}\""))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("features = [{features}]"));
        }
        if self.optional {
            parts.push("optional = true".to_owned());
        }
        if parts.len() == 1 {
            return format!("{} = \"{}\"", self.name, self.version);
        }
        format!("{} = {{ {} }}", self.name, parts.join(", "))
    }
}

/// Every dependency generated output from one spec requires — what `spargen deps` prints and what
/// the `E023` audit checks a consumer manifest against.
///
/// Both read one private requirement table, so the block printed here is exactly the block that passes the
/// audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Requirements {
    /// The dependencies, in manifest order.
    pub dependencies: Vec<RequiredDependency>,
}

impl Requirements {
    pub(crate) fn new(requirements: &RuntimeRequirements) -> Self {
        Self {
            dependencies: requirement_table(requirements)
                .into_iter()
                .map(|required| RequiredDependency {
                    name: required.dependency.name,
                    version: required.dependency.floor,
                    features: required.features.to_vec(),
                    no_default_features: required.no_default_features,
                    optional: required.optional,
                    table: required.table,
                    required_by_feature: required.conditional,
                })
                .collect(),
        }
    }

    /// The `Cargo.toml` fragment to paste into the consuming package.
    ///
    /// Opt-in dependencies (currently the blocking client's `tokio`) are rendered commented out
    /// under the feature that would require them — uncommenting is the whole opt-in.
    pub fn manifest_block(&self) -> String {
        let mut rendered = String::new();
        let mut table: Option<&str> = None;
        for dependency in self
            .dependencies
            .iter()
            .filter(|dependency| dependency.required_by_feature.is_none())
        {
            if table != Some(dependency.table) {
                if table.is_some() {
                    rendered.push('\n');
                }
                rendered.push_str(&format!("[{}]\n", dependency.table));
                table = Some(dependency.table);
            }
            rendered.push_str(&dependency.manifest_line());
            rendered.push('\n');
        }
        for dependency in self
            .dependencies
            .iter()
            .filter(|dependency| dependency.required_by_feature.is_some())
        {
            let feature = dependency.required_by_feature.expect("filtered above");
            rendered.push_str(&format!(
                "\n# Only if your package declares a `{feature}` Cargo feature:\n"
            ));
            rendered.push_str(&format!("# [{}]\n", dependency.table));
            rendered.push_str(&format!("# {}\n", dependency.manifest_line()));
        }
        rendered
    }
}

impl std::fmt::Display for Requirements {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.manifest_block().trim_end())
    }
}

pub(crate) fn audit(manifest_path: &Utf8Path, requirements: &RuntimeRequirements) -> Audit {
    audit_in(manifest_path, requirements, &TargetContext::from_env())
}

/// [`audit`] for an explicit target context, so no test has to mutate the process environment.
fn audit_in(
    manifest_path: &Utf8Path,
    requirements: &RuntimeRequirements,
    target: &TargetContext,
) -> Audit {
    let mut diagnostics = Vec::new();
    let mut manifests = vec![manifest_path.to_path_buf()];
    let manifest = match read_toml(manifest_path, "consumer manifest") {
        Ok(value) => value,
        Err(message) => {
            diagnostics.push(diagnostic(message));
            return Audit {
                diagnostics,
                manifests,
            };
        }
    };
    let root = workspace_root(manifest_path, &manifest);
    // Only a *separate* workspace manifest is read and reported: a self-rooted one is this very
    // file, already parsed above and already in `manifests`.
    let separate = root
        .path
        .as_deref()
        .filter(|_| !root.is_self)
        .and_then(|path| {
            manifests.push(path.to_path_buf());
            match read_toml(path, "workspace manifest") {
                Ok(value) => Some(value),
                Err(message) => {
                    diagnostics.push(diagnostic(message));
                    None
                }
            }
        });
    let workspace = if root.is_self {
        Some(&manifest)
    } else {
        separate.as_ref()
    };
    // Three outcomes, not two: a root that resolved, a root that was found and could not be read
    // (already reported just above), and no root at all. Their remedies differ, so an unresolvable
    // inheritance has to be able to tell them apart.
    let origin = match (&root.path, workspace.is_some()) {
        (Some(path), true) => WorkspaceOrigin::Resolved(path),
        // Read and failed, so `read_toml` has already reported why on its own line.
        (Some(path), false) => WorkspaceOrigin::Unreadable { path, reason: None },
        // No root was read. A candidate the walk could not parse is still the better answer than
        // "nothing found" — it names a file to open — but it stays a message and nothing more, so
        // it has to carry its own reason: nothing else is going to print one.
        (None, _) => match &root.unreadable {
            Some((path, reason)) => WorkspaceOrigin::Unreadable {
                path,
                reason: Some(reason),
            },
            None => WorkspaceOrigin::NotFound(&root.searched_from),
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

/// Which manifest a `workspace = true` dependency resolves its declaration against.
struct WorkspaceRoot {
    /// The manifest carrying `[workspace.dependencies]`, when one was found.
    path: Option<Utf8PathBuf>,
    /// Whether that manifest is the consumer manifest itself — `[package]` and `[workspace]` in
    /// one file. It is already parsed, so it must not be read a second time.
    is_self: bool,
    /// The absolutized consumer manifest the search ran from. A diagnostic that names the caller's
    /// raw spelling would say "resolved nothing from `./Cargo.toml`", which tells the reader
    /// nothing at all.
    searched_from: Utf8PathBuf,
    /// The nearest ancestor manifest that exists and does not parse, with the failure that stopped
    /// it, when the search ended without a root.
    ///
    /// Strictly a better *message* than "no workspace manifest was found": it names a file the
    /// reader can open, and says what is wrong with it. It is never treated as a manifest and
    /// never joins `manifests`, because nothing here knows it was the workspace root — the walk
    /// gave up on it precisely because it could not tell. Treating it as a root would turn an
    /// ordinary crate that happens to sit under an unparseable `Cargo.toml` into a hard `E023`,
    /// which is a far worse answer than a vague one.
    unreadable: Option<(Utf8PathBuf, String)>,
}

/// Locate the workspace manifest a `workspace = true` dependency inherits from.
///
/// Three layouts reach here, and Cargo accepts all three:
///
/// - the manifest is itself the workspace root — `[package]` and `[workspace]` in one file, the
///   single-crate repository — and inheritance resolves against the file already in hand;
/// - `package.workspace` names the workspace root *directory*, which is Cargo's own spelling of
///   that field;
/// - otherwise the nearest *lexical* ancestor manifest that parses and declares `[workspace]` wins.
///
/// The ancestor walk runs over an absolutized path. `manifest_path` can legitimately be relative —
/// the `generate_api!` shim falls back to a bare `./Cargo.toml` — and a relative path has no
/// ancestors to walk, which would report every inherited dependency as unresolvable. Absolutizing
/// is lexical and keeps any `..`, since folding those away changes which file a path names when a
/// component is a symlink; the walk therefore climbs the path as written, not as the filesystem
/// would resolve it.
fn workspace_root(manifest_path: &Utf8Path, manifest: &toml::Value) -> WorkspaceRoot {
    let absolute = std::path::absolute(manifest_path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| manifest_path.to_path_buf());
    if manifest.get("workspace").is_some() {
        return WorkspaceRoot {
            // Absolutized for the same reason `searched_from` is: this path is what an
            // unresolvable inheritance names, and `./Cargo.toml` tells the reader nothing. It is
            // not added to `manifests` — a self-rooted manifest is the consumer manifest, already
            // recorded — so naming it fully cannot duplicate a `rerun-if-changed` directive.
            path: Some(absolute.clone()),
            is_self: true,
            searched_from: absolute,
            unreadable: None,
        };
    }
    let separate = |path| WorkspaceRoot {
        path,
        is_self: false,
        searched_from: absolute.clone(),
        unreadable: None,
    };
    if let Some(relative) = manifest
        .get("package")
        .and_then(|value| value.get("workspace"))
        .and_then(toml::Value::as_str)
    {
        return separate(
            absolute
                .parent()
                .map(|parent| parent.join(relative).join("Cargo.toml")),
        );
    }
    let mut directory = absolute.parent().and_then(Utf8Path::parent);
    // The nearest candidate that exists and does not parse. Remembered, never acted on: a valid
    // root further up still wins, exactly as before, and nothing here can tell whether this file
    // was the root at all — the walk skipped it precisely because it could not read it.
    let mut unreadable = None;
    while let Some(candidate_dir) = directory {
        let candidate = candidate_dir.join("Cargo.toml");
        if candidate.is_file() {
            // One read, keeping the failure rather than discarding it: it is the only account of
            // what is wrong with this file that anything will ever print.
            match std::fs::read_to_string(&candidate)
                .map_err(|error| error.to_string())
                .and_then(|contents| {
                    toml::from_str::<toml::Value>(&contents).map_err(|error| error.to_string())
                }) {
                Ok(value) if value.get("workspace").is_some() => {
                    return separate(Some(candidate));
                }
                // A manifest that parses but declares no `[workspace]` is an ordinary member or an
                // unrelated crate: keep climbing.
                Ok(_) => {}
                Err(reason) => unreadable = unreadable.or(Some((candidate, reason))),
            }
        }
        directory = candidate_dir.parent();
    }
    // Nothing on the path declared `[workspace]`, so there is no root to audit. The unreadable
    // candidate rides along as `unreadable` rather than as `path`: it sharpens the message an
    // unresolvable inheritance prints, and nothing else. Handing it back as a root would have it
    // audited and recorded as a dependency of the build, turning an ordinary crate that merely
    // sits beneath a broken `Cargo.toml` into a hard `E023`.
    WorkspaceRoot {
        path: None,
        is_self: false,
        searched_from: absolute,
        unreadable,
    }
}

struct DependencyCheck<'a> {
    placement: Placement,
    dependency: Dependency,
    features: &'a [&'a str],
    require_no_defaults: bool,
    require_optional: bool,
    /// Where a `workspace = true` dependency resolves from, so an unresolvable inheritance can say
    /// what actually happened rather than only that it failed.
    origin: WorkspaceOrigin<'a>,
}

/// What the workspace lookup found, for the one diagnostic that has to explain itself.
#[derive(Clone, Copy)]
enum WorkspaceOrigin<'a> {
    /// A workspace manifest was found and parsed.
    Resolved(&'a Utf8Path),
    /// One was found but could not be read or parsed.
    ///
    /// `reason` carries the read failure for the walk's fallback, where nothing else reports it:
    /// that candidate is never audited as a manifest, so no separate parse diagnostic accompanies
    /// it and this message is the only place the failure appears. It is `None` for a root named by
    /// `package.workspace`, which *is* audited, and whose failure is therefore already reported as
    /// its own diagnostic — repeating it here would print it twice.
    Unreadable {
        path: &'a Utf8Path,
        reason: Option<&'a str>,
    },
    /// None was found, having searched upwards from this manifest path.
    NotFound(&'a Utf8Path),
}

fn check_dependency(
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
                diagnostics.push(diagnostic(missing_message(check.dependency)));
                return;
            };
            check_declaration(workspace, declaration, &check, true, "", diagnostics);
        }
        Placement::NativeTarget => {
            check_native_target(manifest, workspace, &check, target, diagnostics);
        }
    }
}

fn missing_message(dependency: Dependency) -> String {
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
                    rejected.push(format!("{table} applies to no non-wasm target"));
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
        diagnostics.push(diagnostic(message));
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
            diagnostics.push(diagnostic(format!(
                "generated client requires Cargo feature `{feature}` on `{name}`{context}"
            )));
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
            WorkspaceOrigin::Unreadable { path, reason: None } => {
                format!("its workspace manifest `{path}` could not be read")
            }
            WorkspaceOrigin::Unreadable {
                path,
                reason: Some(reason),
            } => {
                format!("its workspace manifest `{path}` could not be read: {reason}")
            }
            WorkspaceOrigin::NotFound(searched_from) => {
                format!("no workspace manifest was found above `{searched_from}`")
            }
        };
        diagnostics.push(diagnostic(format!(
            "`{}` inherits from `[workspace.dependencies]`, but {origin}{location}",
            dependency.name
        )));
        return None;
    }

    let version = declaration_version(workspace_declaration.unwrap_or(declaration));
    match version {
        Some(requirement) if supported_requirement(requirement, dependency) => {}
        Some(requirement) => diagnostics.push(diagnostic(format!(
            "`{}` version requirement `{requirement}` is outside the supported range >={}, <{}; use `{}` or a higher compatible caret requirement{location}",
            dependency.name,
            dependency.floor,
            dependency.ceiling(),
            dependency.floor
        ))),
        None => diagnostics.push(diagnostic(format!(
            "`{}` must declare a Cargo version requirement of `{}` or a higher compatible floor{location}",
            dependency.name, dependency.floor
        ))),
    }

    let mut features = declaration_features(workspace_declaration.unwrap_or(declaration));
    features.extend(declaration_features(declaration));
    if check_features {
        for feature in check.features {
            if !features.contains(*feature) {
                diagnostics.push(diagnostic(format!(
                    "generated client requires Cargo feature `{feature}` on `{}`{location}",
                    dependency.name
                )));
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
        diagnostics.push(diagnostic(format!(
            "`{}` must set `default-features = false` for the supported freestanding runtime graph{location}",
            dependency.name
        )));
    }
    if check.require_optional && declaration_bool(declaration, "optional") != Some(true) {
        diagnostics.push(diagnostic(format!(
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
        diagnostics.push(diagnostic(format!(
            "`{}` must not be optional: generated code references it unconditionally, so a build \
             with that feature disabled would not compile. Drop `optional = true`, or turn the \
             mapping off at generation time{location}",
            dependency.name
        )));
    }
    if [Some(declaration), workspace_declaration]
        .into_iter()
        .flatten()
        .any(|value| value.get("package").is_some())
    {
        diagnostics.push(diagnostic(format!(
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

fn supported_requirement(raw: &str, dependency: Dependency) -> bool {
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

fn diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        code: Code::RuntimeDependencyContract,
        severity: Code::RuntimeDependencyContract.severity(),
        pointer: JsonPointer::root(),
        span: None,
        message,
        remedy: Some("declare the generated client's runtime dependencies in the consuming package's Cargo.toml using the documented supported ranges and features".to_owned()),
        interpretation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE_MANIFEST: &str = r#"
[package]
name = "consumer"
version = "0.0.0"

[dependencies]
bytes = "1.12.1"
reqwest = { version = "0.12.28", default-features = false }
secrecy = "0.10.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
"#;

    /// `format: date-time`/`date` lower to hand-written RFC 3339 newtypes, so `time` is required
    /// with `formatting`/`parsing` and deliberately **without** `serde`: `time`'s own serde
    /// representation is not RFC 3339 with or without `serde-human-readable`, and that mismatch
    /// once shipped wrong bytes on the wire. Keeping the feature off is what makes the sequence
    /// fallback unreachable, so the contract asserts it rather than only commenting on it.
    #[test]
    fn the_time_requirement_never_asks_for_serde() {
        let requirements = RuntimeRequirements {
            time: true,
            ..RuntimeRequirements::default()
        };
        let time = requirement_table(&requirements)
            .into_iter()
            .find(|requirement| requirement.dependency.name == "time")
            .expect("a time-using API requires the time crate");

        assert!(
            !time.features.contains(&"serde"),
            "time must not be required with serde: {:?}",
            time.features
        );
        assert!(time.features.contains(&"formatting"), "{:?}", time.features);
        assert!(time.features.contains(&"parsing"), "{:?}", time.features);
    }

    fn audit_manifest(contents: &str, requirements: RuntimeRequirements) -> Vec<Diagnostic> {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        std::fs::write(&manifest, contents).unwrap();
        audit_in(&manifest, &requirements, &TargetContext::Unknown).diagnostics
    }

    #[test]
    fn exact_floors_and_higher_compatible_caret_requirements_are_supported() {
        for dependency in [
            BYTES,
            FUTURES_CORE,
            REQWEST,
            SECRECY,
            SERDE,
            SERDE_JSON,
            QUICK_XML,
            UUID,
            TIME,
            TOKIO,
        ] {
            assert!(supported_requirement(dependency.floor, dependency));
            let floor = dependency.floor_version();
            let higher = Version::new(floor.major, floor.minor, floor.patch + 1).to_string();
            assert!(supported_requirement(&higher, dependency));
            assert!(!supported_requirement(
                &format!(">={}, <{}", dependency.floor, dependency.ceiling()),
                dependency
            ));
            assert!(!supported_requirement(
                &format!("^{}.0.0", dependency.ceiling_major),
                dependency
            ));
        }
    }

    #[test]
    fn a_requirement_that_admits_a_version_below_the_floor_is_rejected() {
        let manifest = CORE_MANIFEST.replace("1.12.1", "1.12.0");
        let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, Code::RuntimeDependencyContract);
        assert!(diagnostics[0].message.contains("bytes"));
        assert!(diagnostics[0].message.contains(">=1.12.1, <2.0.0"));
    }

    #[test]
    fn conditional_dependencies_and_features_are_required_only_when_used() {
        assert!(audit_manifest(CORE_MANIFEST, RuntimeRequirements::default()).is_empty());

        let requirements = RuntimeRequirements {
            reqwest_json: true,
            reqwest_multipart: true,
            bytes_serde: true,
            streams: true,
            xml: true,
            uuid: true,
            time: true,
        };
        let diagnostics = audit_manifest(CORE_MANIFEST, requirements);
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            messages.contains("feature `json` on `reqwest`"),
            "{messages}"
        );
        assert!(
            messages.contains("feature `multipart` on `reqwest`"),
            "{messages}"
        );
        assert!(
            messages.contains("feature `serde` on `bytes`"),
            "{messages}"
        );
        assert!(
            messages.contains("feature `stream` on `reqwest`"),
            "{messages}"
        );
        assert!(messages.contains("requires `futures-core`"), "{messages}");
        assert!(messages.contains("requires `quick-xml`"), "{messages}");
        assert!(messages.contains("requires `uuid`"), "{messages}");
        assert!(messages.contains("requires `time`"), "{messages}");
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract));
    }

    #[test]
    fn reqwest_defaults_and_blocking_wiring_are_part_of_the_contract() {
        let manifest = CORE_MANIFEST.replace(
            "reqwest = { version = \"0.12.28\", default-features = false }",
            "reqwest = \"0.12.28\"\n\n[features]\nblocking = []",
        );
        let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(messages.contains("default-features = false"), "{messages}");
        assert!(messages.contains("requires `tokio`"), "{messages}");
        assert!(
            messages.contains("blocking = [\"dep:tokio\"]"),
            "{messages}"
        );
    }

    const TOKIO_DECLARATION: &str =
        "tokio = { version = \"1.53.1\", features = [\"rt\"], optional = true }";

    /// The core manifest opted into `blocking`, followed by `tables` verbatim.
    fn blocking_manifest(tables: &str) -> String {
        format!("{CORE_MANIFEST}\n[features]\nblocking = [\"dep:tokio\"]\n\n{tables}")
    }

    /// `[target.'<key>'.dependencies]` declaring `tokio` as `declaration`.
    fn tokio_table(key: &str, declaration: &str) -> String {
        format!("[target.'{key}'.dependencies]\n{declaration}\n\n")
    }

    fn audit_manifest_for(contents: &str, target: &TargetContext) -> Vec<Diagnostic> {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        std::fs::write(&manifest, contents).unwrap();
        audit_in(&manifest, &RuntimeRequirements::default(), target).diagnostics
    }

    fn messages(diagnostics: &[Diagnostic]) -> String {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A build-script environment as Cargo sets it: `TARGET` plus `CARGO_CFG_*`, each `(cfg,
    /// value)` pair written without the prefix.
    fn build_target(triple: &str, cfgs: &[(&str, &str)]) -> TargetContext {
        TargetContext::Build(BuildTarget::from_vars(
            std::iter::once(("TARGET".to_owned(), triple.to_owned()))
                .chain(
                    cfgs.iter()
                        .map(|(cfg, value)| (format!("CARGO_CFG_{cfg}"), (*value).to_owned())),
                )
                // Unrelated variables are ignored, as a real build script's environment has many.
                .chain(std::iter::once(("PATH".to_owned(), "/bin".to_owned()))),
        ))
    }

    const LINUX_CFGS: &[(&str, &str)] = &[
        ("TARGET_ARCH", "x86_64"),
        ("TARGET_OS", "linux"),
        ("TARGET_FAMILY", "unix"),
        ("UNIX", ""),
        ("TARGET_ENV", "gnu"),
        ("TARGET_VENDOR", "unknown"),
        ("TARGET_POINTER_WIDTH", "64"),
        ("TARGET_ENDIAN", "little"),
        ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
        ("PANIC", "unwind"),
    ];

    fn linux() -> TargetContext {
        build_target("x86_64-unknown-linux-gnu", LINUX_CFGS)
    }

    fn windows() -> TargetContext {
        build_target(
            "x86_64-pc-windows-msvc",
            &[
                ("TARGET_ARCH", "x86_64"),
                ("TARGET_OS", "windows"),
                ("TARGET_FAMILY", "windows"),
                ("WINDOWS", ""),
                ("TARGET_ENV", "msvc"),
                ("TARGET_VENDOR", "pc"),
                ("TARGET_POINTER_WIDTH", "64"),
                ("TARGET_ENDIAN", "little"),
                ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
                ("PANIC", "unwind"),
            ],
        )
    }

    fn wasm32() -> TargetContext {
        build_target(
            "wasm32-unknown-unknown",
            &[
                ("TARGET_ARCH", "wasm32"),
                ("TARGET_OS", "unknown"),
                ("TARGET_FAMILY", "wasm"),
                ("TARGET_VENDOR", "unknown"),
                ("TARGET_POINTER_WIDTH", "32"),
                ("TARGET_ENDIAN", "little"),
                ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
                ("PANIC", "abort"),
            ],
        )
    }

    #[test]
    fn equivalent_native_cfg_spellings_satisfy_the_blocking_requirement() {
        // The three spellings #88 reported: each is a table Cargo applies on exactly the native
        // targets, and each used to be looked up by its literal key and reported missing.
        for target in [TargetContext::Unknown, linux(), windows()] {
            let failures = [
                r#"cfg(not(target_arch="wasm32"))"#,
                r#"cfg(not(target_family = "wasm"))"#,
                r#"cfg(all(not(target_arch = "wasm32")))"#,
            ]
            .into_iter()
            .filter_map(|key| {
                let manifest = blocking_manifest(&tokio_table(key, TOKIO_DECLARATION));
                let diagnostics = audit_manifest_for(&manifest, &target);
                (!diagnostics.is_empty()).then(|| format!("{key}: {diagnostics:#?}"))
            })
            .collect::<Vec<_>>();
            assert!(failures.is_empty(), "{}", failures.join("\n"));
        }
    }

    #[test]
    fn without_a_build_target_tables_must_jointly_cover_every_native_target() {
        let covering = blocking_manifest(&format!(
            "{}{}",
            tokio_table("cfg(unix)", TOKIO_DECLARATION),
            tokio_table(
                r#"cfg(all(not(unix), not(target_family = "wasm")))"#,
                TOKIO_DECLARATION
            )
        ));
        let diagnostics = audit_manifest_for(&covering, &TargetContext::Unknown);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");

        // Cargo builds this on every unix and windows host, but a proc-macro cannot tell which
        // target it is expanding for, and OS-less targets are covered by neither table.
        let gapped = blocking_manifest(&format!(
            "{}{}",
            tokio_table("cfg(unix)", TOKIO_DECLARATION),
            tokio_table("cfg(windows)", TOKIO_DECLARATION)
        ));
        let diagnostics = audit_manifest_for(&gapped, &TargetContext::Unknown);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("requires `tokio`"), "{message}");
        assert!(
            message.contains("proc-macro cannot see the build target"),
            "{message}"
        );
        assert!(message.contains("is not covered"), "{message}");
        assert!(
            message.contains("[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]")
                && message.contains("generate from `build.rs`"),
            "both remedies must be named: {message}"
        );
    }

    #[test]
    fn a_tokio_table_that_applies_on_wasm_is_not_native_only() {
        let in_dependencies = format!(
            "{CORE_MANIFEST}{TOKIO_DECLARATION}\n\n[features]\nblocking = [\"dep:tokio\"]\n"
        );
        let wasm_applicable = blocking_manifest(&tokio_table(
            r#"cfg(any(unix, target_arch = "wasm32"))"#,
            TOKIO_DECLARATION,
        ));
        for target in [TargetContext::Unknown, linux()] {
            for (manifest, table) in [
                (&in_dependencies, "`[dependencies]`"),
                (
                    &wasm_applicable,
                    "`[target.'cfg(any(unix, target_arch = \"wasm32\"))'.dependencies]`",
                ),
            ] {
                let diagnostics = audit_manifest_for(manifest, &target);
                let messages = messages(&diagnostics);
                assert!(messages.contains("requires `tokio`"), "{messages}");
                assert!(
                    messages.contains(&format!(
                        "{table} applies on `wasm32-unknown-unknown`, where the blocking client \
                         is compiled out, so `tokio` must be native-only"
                    )),
                    "{messages}"
                );
            }
        }
    }

    #[test]
    fn unevaluable_or_unparseable_target_keys_explain_themselves() {
        for (tables, expected) in [
            (
                tokio_table(r#"cfg(not(feature = "x"))"#, TOKIO_DECLARATION),
                "`[target.'cfg(not(feature = \"x\"))'.dependencies]` cannot be evaluated: \
                 `feature = \"x\"` does not select target tables",
            ),
            (
                tokio_table("cfg(not(target_arch = ))", TOKIO_DECLARATION),
                "`[target.'cfg(not(target_arch = ))'.dependencies]` does not parse:",
            ),
            (
                format!("[target.x86_64-unknown-linux-gnu.dependencies]\n{TOKIO_DECLARATION}\n"),
                "is not covered",
            ),
        ] {
            let diagnostics =
                audit_manifest_for(&blocking_manifest(&tables), &TargetContext::Unknown);
            let messages = messages(&diagnostics);
            assert!(messages.contains("requires `tokio`"), "{messages}");
            assert!(messages.contains(expected), "{expected}\n{messages}");
        }
    }

    #[test]
    fn a_build_target_selects_the_tables_cargo_would_apply() {
        let manifest = blocking_manifest(&format!(
            "{}{}",
            tokio_table("cfg(unix)", TOKIO_DECLARATION),
            tokio_table("cfg(windows)", TOKIO_DECLARATION)
        ));
        for target in [linux(), windows()] {
            let diagnostics = audit_manifest_for(&manifest, &target);
            assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        }
    }

    #[test]
    fn a_table_that_misses_the_build_target_names_it() {
        let manifest = blocking_manifest(&tokio_table(
            r#"cfg(target_os = "none")"#,
            TOKIO_DECLARATION,
        ));
        let diagnostics = audit_manifest_for(&manifest, &linux());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("requires `tokio`"), "{message}");
        assert!(
            message.contains("evaluated for the build target `x86_64-unknown-linux-gnu`"),
            "{message}"
        );
        assert!(
            message.contains(
                "`[target.'cfg(target_os = \"none\")'.dependencies]` does not apply to \
                 `x86_64-unknown-linux-gnu`"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_wasm32_build_needs_no_tokio_but_keeps_the_wiring_check() {
        let manifest = format!("{CORE_MANIFEST}\n[features]\nblocking = []\n");
        let diagnostics = audit_manifest_for(&manifest, &wasm32());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(
            diagnostics[0]
                .message
                .contains("blocking = [\"dep:tokio\"]"),
            "{diagnostics:#?}"
        );

        // The same manifest on a native build still needs the table.
        let messages = messages(&audit_manifest_for(&manifest, &linux()));
        assert!(messages.contains("requires `tokio`"), "{messages}");
    }

    #[test]
    fn build_flags_and_target_names_are_read_from_cargo() {
        let flagged = blocking_manifest(&tokio_table(
            "cfg(all(unix, not(my_flag)))",
            TOKIO_DECLARATION,
        ));
        assert!(audit_manifest_for(&flagged, &linux()).is_empty());
        let with_flag = build_target(
            "x86_64-unknown-linux-gnu",
            &[LINUX_CFGS, &[("MY_FLAG", "")]].concat(),
        );
        let messages_with_flag = messages(&audit_manifest_for(&flagged, &with_flag));
        assert!(
            messages_with_flag.contains("does not apply to `x86_64-unknown-linux-gnu`"),
            "{messages_with_flag}"
        );

        let named = blocking_manifest(&format!(
            "[target.x86_64-unknown-linux-gnu.dependencies]\n{TOKIO_DECLARATION}\n"
        ));
        assert!(audit_manifest_for(&named, &linux()).is_empty());
        let musl = build_target("x86_64-unknown-linux-musl", LINUX_CFGS);
        let messages_on_musl = messages(&audit_manifest_for(&named, &musl));
        assert!(
            messages_on_musl.contains(
                "`[target.x86_64-unknown-linux-gnu.dependencies]` does not apply to \
                 `x86_64-unknown-linux-musl`"
            ),
            "{messages_on_musl}"
        );
    }

    #[test]
    fn contract_rules_still_apply_under_an_alternative_spelling() {
        let key = r#"cfg(not(target_family = "wasm"))"#;
        let not_optional = blocking_manifest(&tokio_table(
            key,
            "tokio = { version = \"1.53.1\", features = [\"rt\"] }",
        ));
        let without_rt = blocking_manifest(&tokio_table(
            key,
            "tokio = { version = \"1.53.1\", optional = true }",
        ));
        for target in [TargetContext::Unknown, linux()] {
            let messages_not_optional = messages(&audit_manifest_for(&not_optional, &target));
            assert!(
                messages_not_optional.contains("`tokio` must be optional"),
                "{messages_not_optional}"
            );
            let messages_without_rt = messages(&audit_manifest_for(&without_rt, &target));
            assert!(
                messages_without_rt
                    .contains("generated client requires Cargo feature `rt` on `tokio`"),
                "{messages_without_rt}"
            );
        }

        // Cargo unifies features across the tables that apply, so `rt` has to reach every target
        // through one of them. Here the table covering non-unix targets leaves it out.
        let split = blocking_manifest(&format!(
            "{}{}",
            tokio_table("cfg(unix)", TOKIO_DECLARATION),
            tokio_table(
                r#"cfg(all(not(unix), not(target_family = "wasm")))"#,
                "tokio = { version = \"1.53.1\", optional = true }"
            )
        ));
        let messages_split = messages(&audit_manifest_for(&split, &TargetContext::Unknown));
        assert!(
            messages_split.contains(
                "requires Cargo feature `rt` on `tokio` (not enabled by the tables that apply to"
            ),
            "{messages_split}"
        );
        // On a build only the tables that apply are unified: `cfg(windows)` carries `rt`, but it
        // does not apply on linux.
        let build_split = blocking_manifest(&format!(
            "{}{}",
            tokio_table(
                "cfg(unix)",
                "tokio = { version = \"1.53.1\", optional = true }"
            ),
            tokio_table("cfg(windows)", TOKIO_DECLARATION)
        ));
        let messages_build = messages(&audit_manifest_for(&build_split, &linux()));
        assert!(
            messages_build.contains("generated client requires Cargo feature `rt` on `tokio`"),
            "{messages_build}"
        );
        assert!(audit_manifest_for(&build_split, &windows())
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("feature `rt`")));
    }

    #[test]
    fn a_below_floor_tokio_in_one_of_two_counting_tables_names_that_table() {
        // Both tables count on either path: `cfg(unix)` applies to the unix builtins and to linux,
        // and `cfg(not(target_family = "wasm"))` covers everything else. Each counting declaration
        // meets the version floor on its own, and with more than one the message says which.
        let manifest = blocking_manifest(&format!(
            "{}{}",
            tokio_table(
                "cfg(unix)",
                "tokio = { version = \"1.53.0\", features = [\"rt\"], optional = true }"
            ),
            tokio_table(r#"cfg(not(target_family = "wasm"))"#, TOKIO_DECLARATION)
        ));
        for target in [TargetContext::Unknown, linux()] {
            let diagnostics = audit_manifest_for(&manifest, &target);
            assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
            assert_eq!(
                diagnostics[0].message,
                "`tokio` version requirement `1.53.0` is outside the supported range >=1.53.1, \
                 <2.0.0; use `1.53.1` or a higher compatible caret requirement (in \
                 `[target.'cfg(unix)'.dependencies]`)"
            );
        }
    }

    #[test]
    fn a_renamed_tokio_in_a_target_table_follows_the_untargeted_rename_rule() {
        // Untargeted, the rule has two halves: a `package` key on the canonical name is rejected,
        // and the crate declared under another name is not found at all.
        let untargeted_package = CORE_MANIFEST.replace(
            "secrecy = \"0.10.3\"",
            "secrecy = { package = \"secrecy\", version = \"0.10.3\" }",
        );
        assert_eq!(
            messages(&audit_manifest_for(
                &untargeted_package,
                &TargetContext::Unknown
            )),
            "`secrecy` cannot be renamed because generated code references that canonical crate \
             name"
        );
        let untargeted_alias = CORE_MANIFEST.replace(
            "secrecy = \"0.10.3\"",
            "secret = { package = \"secrecy\", version = \"0.10.3\" }",
        );
        assert_eq!(
            messages(&audit_manifest_for(
                &untargeted_alias,
                &TargetContext::Unknown
            )),
            "generated client requires `secrecy`; add `secrecy` with version `0.10.3`"
        );

        // A target table applies the same two halves to `tokio`, on both paths.
        let key = r#"cfg(not(target_family = "wasm"))"#;
        let targeted_package = blocking_manifest(&tokio_table(
            key,
            "tokio = { package = \"tokio\", version = \"1.53.1\", features = [\"rt\"], optional = \
             true }",
        ));
        let targeted_alias = blocking_manifest(&tokio_table(
            key,
            "tokio_rt = { package = \"tokio\", version = \"1.53.1\", features = [\"rt\"], \
             optional = true }",
        ));
        for target in [TargetContext::Unknown, linux()] {
            assert_eq!(
                messages(&audit_manifest_for(&targeted_package, &target)),
                "`tokio` cannot be renamed because generated code references that canonical crate \
                 name"
            );
            let alias_messages = messages(&audit_manifest_for(&targeted_alias, &target));
            assert!(
                alias_messages
                    .starts_with("generated client requires `tokio`; add `tokio` with version")
                    && !alias_messages.contains('\n'),
                "{alias_messages}"
            );
        }
    }

    #[test]
    fn tokio_optional_in_one_counting_table_and_not_the_other_is_reported_on_that_table() {
        // Assumption A2: every counting declaration is held to `optional = true`, so a pair that
        // disagrees is reported on the declaration that is not optional rather than accepted.
        let manifest = blocking_manifest(&format!(
            "{}{}",
            tokio_table("cfg(unix)", TOKIO_DECLARATION),
            tokio_table(
                r#"cfg(not(target_family = "wasm"))"#,
                "tokio = { version = \"1.53.1\", features = [\"rt\"] }"
            )
        ));
        for target in [TargetContext::Unknown, linux()] {
            let diagnostics = audit_manifest_for(&manifest, &target);
            assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
            assert_eq!(
                diagnostics[0].message,
                "`tokio` must be optional because it is enabled only by the generated `blocking` \
                 feature (in `[target.'cfg(not(target_family = \"wasm\"))'.dependencies]`)"
            );
        }
    }

    #[test]
    fn workspace_inherited_tokio_under_an_alternative_spelling_resolves() {
        let directory = tempfile::tempdir().unwrap();
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}\
                 tokio = {{ version = \"1.53.1\", features = [\"rt\"] }}\n",
                core_workspace_dependencies()
            ),
        )
        .unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(
            &member,
            format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[features]\n\
                 blocking = [\"dep:tokio\"]\n\n{CORE_INHERITED}\n\
                 [target.'cfg(not(target_arch=\"wasm32\"))'.dependencies]\n\
                 tokio = {{ workspace = true, optional = true }}\n"
            ),
        )
        .unwrap();
        for target in [TargetContext::Unknown, linux()] {
            let result = audit_in(&member, &RuntimeRequirements::default(), &target);
            assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        }
    }

    #[test]
    fn every_target_predicate_reads_the_variable_cargo_sets() {
        // The build-script mapping must agree with cfg-expr's own database for the same triple, for
        // every kind of target predicate, both ways round.
        let TargetContext::Build(build) = linux() else {
            unreachable!("linux() is a build target")
        };
        let builtin = get_builtin_target_by_triple("x86_64-unknown-linux-gnu").unwrap();
        for predicate in [
            r#"target_arch = "x86_64""#,
            r#"target_arch = "aarch64""#,
            r#"target_os = "linux""#,
            r#"target_os = "none""#,
            r#"target_family = "unix""#,
            r#"target_family = "windows""#,
            "unix",
            "windows",
            r#"target_env = "gnu""#,
            r#"target_env = "musl""#,
            r#"target_env = """#,
            r#"target_abi = """#,
            r#"target_abi = "eabihf""#,
            r#"target_vendor = "unknown""#,
            r#"target_vendor = "apple""#,
            r#"target_pointer_width = "64""#,
            r#"target_pointer_width = "32""#,
            r#"target_endian = "little""#,
            r#"target_endian = "big""#,
            r#"target_has_atomic = "ptr""#,
            r#"target_has_atomic = "64""#,
            r#"panic = "unwind""#,
            r#"panic = "abort""#,
        ] {
            let spelled = format!("cfg({predicate})");
            let key = TableKey::parse(&spelled);
            assert_eq!(
                key.evaluate(Subject::Build(&build)),
                key.evaluate(Subject::Builtin(builtin)),
                "{predicate}"
            );
        }
    }

    #[test]
    fn the_printed_tokio_table_is_the_generated_blocking_gate() {
        // `spargen deps` prints the table as a string, the audit evaluates `BLOCKING_GATE`, and
        // `e2e.rs` pins the gate generated code emits. This keeps the first two the same cfg.
        let requirements = Requirements::new(&RuntimeRequirements::default());
        let tokio = requirements
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "tokio")
            .expect("the blocking client's tokio is always in the table");
        assert_eq!(
            tokio.table,
            format!("target.'cfg({BLOCKING_GATE})'.dependencies")
        );
        assert!(Expression::parse(BLOCKING_GATE).is_ok());
        assert!(get_builtin_target_by_triple(WASM_ANCHOR).is_some());
    }

    #[test]
    fn workspace_inheritance_uses_the_workspace_version_and_features() {
        let directory = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(
            &root,
            format!(
                "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
                CORE_MANIFEST.split_once("[dependencies]\n").unwrap().1
            ),
        )
        .unwrap();
        std::fs::write(
            &member,
            r#"[package]
name = "consumer"
version = "0.0.0"

[dependencies]
bytes.workspace = true
reqwest.workspace = true
secrecy.workspace = true
serde.workspace = true
serde_json.workspace = true
"#,
        )
        .unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        assert_eq!(result.manifests, vec![root, member]);
    }

    /// The five core dependencies as a `[workspace.dependencies]` body, reusing `CORE_MANIFEST` so
    /// the floors in these fixtures cannot drift from the ones every other test audits against.
    fn core_workspace_dependencies() -> &'static str {
        CORE_MANIFEST.split_once("[dependencies]\n").unwrap().1
    }

    const CORE_INHERITED: &str = "\
[dependencies]
bytes.workspace = true
reqwest.workspace = true
secrecy.workspace = true
serde.workspace = true
serde_json.workspace = true
";

    #[test]
    fn a_root_package_inherits_its_own_workspace_dependencies() {
        // `[package]` and `[workspace]` in one file is the single-crate repository, and `workspace
        // = true` there resolves against the table directly below it. Resolution used to give up
        // the moment the consumer manifest declared `[workspace]` at all, so this layout reported
        // every inherited dependency as unresolvable — about a table spargen had already parsed.
        let directory = tempfile::tempdir().unwrap();
        let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        std::fs::write(
            &manifest,
            format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[workspace]\n\n\
                 [workspace.dependencies]\n{}\n{CORE_INHERITED}",
                core_workspace_dependencies()
            ),
        )
        .unwrap();

        let result = audit(&manifest, &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        // Self-rooted: one manifest, not the same file reported twice.
        assert_eq!(result.manifests, vec![manifest]);
    }

    #[test]
    fn package_workspace_names_the_workspace_root_directory() {
        // Cargo's `package.workspace` is a path to the root *directory*, not to its manifest. The
        // root here is deliberately not an ancestor of the member, so only that field can resolve
        // it and the assertion cannot pass through the ancestor walk by accident.
        let directory = tempfile::tempdir().unwrap();
        let root_dir = directory.path().join("root");
        let member_dir = directory.path().join("outside");
        std::fs::create_dir(&root_dir).unwrap();
        std::fs::create_dir(&member_dir).unwrap();
        let root = Utf8PathBuf::from_path_buf(root_dir.join("Cargo.toml")).unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(
            &root,
            format!(
                "[workspace]\nmembers = []\n\n[workspace.dependencies]\n{}",
                core_workspace_dependencies()
            ),
        )
        .unwrap();
        std::fs::write(
            &member,
            format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
                 {CORE_INHERITED}"
            ),
        )
        .unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        // The root is reported as the field spells it — `…/outside/../root/Cargo.toml`. `..` is
        // deliberately not folded away: doing that lexically changes which file a path names when
        // a component is a symlink, and Cargo accepts the unfolded form for `rerun-if-changed`
        // just the same.
        assert_eq!(result.manifests.len(), 2, "{:#?}", result.manifests);
        assert!(
            result
                .manifests
                .iter()
                .any(|path| path.canonicalize().ok() == root.canonicalize().ok()),
            "{:#?}",
            result.manifests
        );
    }

    /// The working directory is process-global, so the one test that has to change it restores it
    /// on the way out — including on a panic — and holds a lock so a second such test cannot race
    /// it.
    static WORKING_DIRECTORY: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct RestoreWorkingDirectory(std::path::PathBuf);

    impl Drop for RestoreWorkingDirectory {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn a_relative_manifest_path_still_resolves_the_workspace_root() {
        // `generate_api!` falls back to a bare `./Cargo.toml` when Cargo names no manifest in the
        // environment. A one-component path has no ancestors to walk, so the workspace root was
        // never found and every inherited dependency reported as unresolvable.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Cargo.toml");
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        std::fs::write(
            &root,
            format!(
                "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
                core_workspace_dependencies()
            ),
        )
        .unwrap();
        std::fs::write(
            member_dir.join("Cargo.toml"),
            format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
        )
        .unwrap();

        let _lock = WORKING_DIRECTORY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
        std::env::set_current_dir(&member_dir).unwrap();

        let result = audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        // The member as given plus the workspace root the walk reached. The root's textual form
        // depends on how the platform resolves the temporary directory, so only the count is
        // asserted.
        assert_eq!(result.manifests.len(), 2, "{:#?}", result.manifests);
    }

    #[test]
    fn a_self_rooted_manifest_names_an_absolute_path_when_an_entry_is_missing() {
        // The layout the relative-path fix exists for: a single-crate repository whose `[package]`
        // and `[workspace]` share one file, reached through the `generate_api!` `./Cargo.toml`
        // fallback. The diagnostic names the manifest it resolved against, and naming it
        // `./Cargo.toml` would tell the reader nothing about which file to open.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[workspace]\n\n\
                 [workspace.dependencies]\n{}\n\n{CORE_INHERITED}",
                core_workspace_dependencies()
                    .lines()
                    .filter(|line| !line.starts_with("bytes"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();

        let _lock = WORKING_DIRECTORY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
        std::env::set_current_dir(directory.path()).unwrap();

        let result = audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
        assert_eq!(result.diagnostics.len(), 1, "{:#?}", result.diagnostics);
        let message = &result.diagnostics[0].message;
        assert!(
            message.contains("`bytes` inherits") && message.contains("declares no `bytes` there"),
            "{message}"
        );
        // The point of the fix: a path the reader can open, not the caller's bare spelling.
        assert!(
            !message.contains("`Cargo.toml` declares")
                && !message.contains("`./Cargo.toml` declares"),
            "the diagnostic names the raw spelling rather than a path the reader can open: \
             {message}"
        );
    }

    #[test]
    fn a_corrupt_root_reached_by_the_ancestor_walk_is_not_reported_as_missing() {
        // The commonest layout reaches its root through the walk rather than through
        // `package.workspace`, and the walk used to skip any candidate it could not parse and keep
        // climbing — collapsing the three-way distinction back to "nothing found" for exactly the
        // case where a file the reader can open is the problem.
        let directory = tempfile::tempdir().unwrap();
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(&root, "[workspace\nthis is not toml\n").unwrap();
        std::fs::write(
            &member,
            format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
        )
        .unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("`bytes` inherits")
                    && diagnostic.message.contains("could not be read")
                    && diagnostic.message.contains(root.as_str())
                    // The candidate is never audited as a manifest, so nothing else prints why
                    // it failed. The message has to carry the reason itself.
                    && diagnostic.message.contains("TOML parse error")
            }),
            "{:#?}",
            result.diagnostics
        );
        assert!(
            !result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("no workspace manifest was found above")
            }),
            "{:#?}",
            result.diagnostics
        );
    }

    #[test]
    fn an_unparseable_ancestor_manifest_is_not_an_error_on_its_own() {
        // The walk climbs to the filesystem root on every standalone crate, because a `cargo new`
        // manifest declares no `[workspace]`. Anything unreadable it passes on the way — a broken
        // `Cargo.toml`, one it lacks permission to read — must stay invisible to a consumer that
        // inherits nothing: it is not this crate's workspace root, and nothing here can tell
        // whether it is anyone's. Treating it as one turned an ordinary build into a hard `E023`.
        let directory = tempfile::tempdir().unwrap();
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            "[workspace\nthis is not toml\n",
        )
        .unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        // Declares every runtime dependency directly — nothing inherits, so nothing needs a root.
        std::fs::write(&member, CORE_MANIFEST).unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        // And it is not recorded as a build input either: a `rerun-if-changed` on an unrelated
        // file would rebuild the consumer whenever it changed.
        assert_eq!(result.manifests, vec![member], "{:#?}", result.manifests);
    }

    #[test]
    fn a_broken_manifest_below_the_real_root_does_not_stop_the_walk() {
        // Distinguishing "corrupt" from "missing" must not change *which* root resolves. An
        // unrelated manifest that happens not to parse can sit between a member and its real
        // workspace root — Cargo reaches the root regardless, and a spargen that stopped short
        // would report `E023` for a layout that builds perfectly well.
        let directory = tempfile::tempdir().unwrap();
        let broken_dir = directory.path().join("group");
        let member_dir = broken_dir.join("client");
        std::fs::create_dir(&broken_dir).unwrap();
        std::fs::create_dir(&member_dir).unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                "[workspace]\nmembers = [\"group/client\"]\n\n[workspace.dependencies]\n{}",
                core_workspace_dependencies()
            ),
        )
        .unwrap();
        std::fs::write(broken_dir.join("Cargo.toml"), "[package\nnot toml at all\n").unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(
            &member,
            format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
        )
        .unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    }

    #[test]
    fn an_unresolvable_inheritance_says_where_the_lookup_went() {
        // One message used to cover two opposite situations: the workspace has no such entry (fix
        // the root), and no workspace was found at all (fix the layout, or spell the version out).
        // The report behind #71 landed in exactly that ambiguity.
        let directory = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        let member_dir = directory.path().join("client");
        std::fs::create_dir(&member_dir).unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        std::fs::write(
            &member,
            format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
        )
        .unwrap();

        // No workspace manifest anywhere above the member.
        let orphaned = audit(&member, &RuntimeRequirements::default());
        for dependency in ["bytes", "reqwest", "secrecy", "serde", "serde_json"] {
            assert!(
                orphaned.diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .message
                        .contains(&format!("`{dependency}` inherits"))
                        && diagnostic
                            .message
                            .contains("no workspace manifest was found above")
                        && diagnostic.message.contains(member.as_str())
                }),
                "{:#?}",
                orphaned.diagnostics
            );
        }

        // A workspace that resolves, but declares four of the five.
        std::fs::write(
            &root,
            format!(
                "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
                core_workspace_dependencies()
                    .lines()
                    .filter(|line| !line.starts_with("bytes"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let partial = audit(&member, &RuntimeRequirements::default());
        assert_eq!(partial.diagnostics.len(), 1, "{:#?}", partial.diagnostics);
        let message = &partial.diagnostics[0].message;
        assert!(
            message.contains("`bytes` inherits")
                && message.contains(root.as_str())
                && message.contains("declares no `bytes` there"),
            "{message}"
        );
    }

    #[test]
    fn a_workspace_root_that_cannot_be_read_is_not_reported_as_missing() {
        // Found-but-broken is a third state. Reporting it as "no workspace manifest was found"
        // contradicts the read failure reported beside it and points at the opposite remedy.
        let directory = tempfile::tempdir().unwrap();
        let root_dir = directory.path().join("root");
        let member_dir = directory.path().join("outside");
        std::fs::create_dir(&root_dir).unwrap();
        std::fs::create_dir(&member_dir).unwrap();
        let root = Utf8PathBuf::from_path_buf(root_dir.join("Cargo.toml")).unwrap();
        let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
        // `package.workspace` names the root explicitly, so the walk does not get to pre-parse and
        // silently skip it: the audit reads exactly this file, and it does not parse.
        std::fs::write(&root, "[workspace\nthis is not toml\n").unwrap();
        std::fs::write(
            &member,
            format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
                 {CORE_INHERITED}"
            ),
        )
        .unwrap();

        let result = audit(&member, &RuntimeRequirements::default());
        // The read failure names the file for what it is. Calling the workspace root "the consumer
        // manifest" contradicted the inheritance diagnostic asserted just below, which calls the
        // same path a workspace manifest.
        assert!(
            result.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("failed to parse workspace manifest")),
            "{:#?}",
            result.diagnostics
        );
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("`bytes` inherits")
                    && diagnostic.message.contains("could not be read")
            }),
            "{:#?}",
            result.diagnostics
        );
        assert!(
            !result.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("no workspace manifest was found")),
            "found-but-broken must not be reported as missing: {:#?}",
            result.diagnostics
        );
    }

    /// The anti-drift property: the block `spargen deps` prints must be exactly a block the audit
    /// accepts. If the two ever diverge — a feature demanded but not printed, or printed with the
    /// wrong floor — this fails.
    #[test]
    fn the_printed_dependency_block_passes_the_audit_it_describes() {
        // Every capability on at once, so the table is exercised in full.
        let requirements = RuntimeRequirements {
            reqwest_json: true,
            reqwest_multipart: true,
            bytes_serde: true,
            streams: true,
            xml: true,
            uuid: true,
            time: true,
        };
        let block = Requirements::new(&requirements).manifest_block();
        // `deps` renders the blocking dependency commented out, under the feature that requires
        // it; a consumer that opts in uncomments both, which is what this reconstructs.
        let opted_in = block
            .replace("# [target", "[target")
            .replace("# tokio", "tokio");
        let manifest = format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n             [features]\nblocking = [\"dep:tokio\"]\n\n{opted_in}"
        );

        let diagnostics = audit_manifest(&manifest, requirements.clone());
        assert!(
            diagnostics.is_empty(),
            "the block `spargen deps` prints must satisfy the audit:\n{manifest}\n{diagnostics:#?}"
        );

        // And with the feature absent, the commented-out block is genuinely not required.
        let without_blocking =
            format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{block}");
        let diagnostics = audit_manifest(&without_blocking, requirements);
        assert!(
            diagnostics.is_empty(),
            "{without_blocking}\n{diagnostics:#?}"
        );
    }
}
