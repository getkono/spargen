//! Cargo manifest auditing for the runtime required by generated output.
//!
//! This is facade plumbing rather than a public subsystem: the lowered API determines the
//! requirements, while Cargo remains responsible for resolving the consumer's declared graph.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
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
    pub reqwest_json: bool,
    pub reqwest_multipart: bool,
    pub bytes_serde: bool,
    pub streams: bool,
    pub xml: bool,
    pub uuid: bool,
    pub time: bool,
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
    pub diagnostics: Vec<Diagnostic>,
    pub manifests: Vec<Utf8PathBuf>,
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
    /// The manifest table it belongs in — `dependencies`, or a target-specific table.
    table: &'static str,
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

/// The dependency table for one lowered API. `deps` renders it; `audit` checks the consumer
/// manifest against it.
fn requirement_table(requirements: &RuntimeRequirements) -> Vec<Requirement> {
    const NONE: &[&str] = &[];
    let mut table = Vec::new();
    let mut push = |dependency: Dependency, features: &'static [&str], no_defaults: bool| {
        table.push(Requirement {
            table: "dependencies",
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
/// Both read [`requirement_table`], so the block printed here is exactly the block that passes the
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
    let mut diagnostics = Vec::new();
    let mut manifests = vec![manifest_path.to_path_buf()];
    let manifest = match read_toml(manifest_path) {
        Ok(value) => value,
        Err(message) => {
            diagnostics.push(diagnostic(message));
            return Audit {
                diagnostics,
                manifests,
            };
        }
    };
    let workspace_path = workspace_manifest_path(manifest_path, &manifest);
    let workspace = workspace_path.as_deref().and_then(|path| {
        manifests.push(path.to_path_buf());
        match read_toml(path) {
            Ok(value) => Some(value),
            Err(message) => {
                diagnostics.push(diagnostic(message));
                None
            }
        }
    });

    for required in requirement_table(requirements) {
        if let Some(feature) = required.conditional {
            if !declares_feature(&manifest, feature) {
                continue;
            }
        }
        check_dependency(
            &manifest,
            workspace.as_ref(),
            DependencyCheck {
                table: required.table,
                dependency: required.dependency,
                features: required.features,
                require_no_defaults: required.no_default_features,
                require_optional: required.optional,
            },
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

fn read_toml(path: &Utf8Path) -> Result<toml::Value, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read consumer manifest `{path}`: {error}"))?;
    toml::from_str(&contents)
        .map_err(|error| format!("failed to parse consumer manifest `{path}`: {error}"))
}

fn workspace_manifest_path(
    manifest_path: &Utf8Path,
    manifest: &toml::Value,
) -> Option<Utf8PathBuf> {
    if manifest.get("workspace").is_some() {
        return None;
    }
    if let Some(relative) = manifest
        .get("package")
        .and_then(|value| value.get("workspace"))
        .and_then(toml::Value::as_str)
    {
        return manifest_path
            .parent()
            .map(|parent| parent.join(relative).join("Cargo.toml"));
    }
    let mut directory = manifest_path.parent()?.parent();
    while let Some(candidate_dir) = directory {
        let candidate = candidate_dir.join("Cargo.toml");
        if candidate.is_file()
            && std::fs::read_to_string(&candidate)
                .ok()
                .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
                .is_some_and(|value| value.get("workspace").is_some())
        {
            return Some(candidate);
        }
        directory = candidate_dir.parent();
    }
    None
}

struct DependencyCheck<'a> {
    table: &'a str,
    dependency: Dependency,
    features: &'a [&'a str],
    require_no_defaults: bool,
    require_optional: bool,
}

fn check_dependency(
    manifest: &toml::Value,
    workspace: Option<&toml::Value>,
    check: DependencyCheck<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let dependency = check.dependency;
    let Some(declaration) = dotted_get(manifest, check.table, dependency.name) else {
        diagnostics.push(diagnostic(format!(
            "generated client requires `{}`; add `{}` with version `{}`",
            dependency.name, dependency.name, dependency.floor
        )));
        return;
    };
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
        diagnostics.push(diagnostic(format!(
            "`{}` inherits from `[workspace.dependencies]`, but no workspace declaration could be resolved",
            dependency.name
        )));
        return;
    }

    let version = declaration_version(workspace_declaration.unwrap_or(declaration));
    match version {
        Some(requirement) if supported_requirement(requirement, dependency) => {}
        Some(requirement) => diagnostics.push(diagnostic(format!(
            "`{}` version requirement `{requirement}` is outside the supported range >={}, <{}; use `{}` or a higher compatible caret requirement",
            dependency.name,
            dependency.floor,
            dependency.ceiling(),
            dependency.floor
        ))),
        None => diagnostics.push(diagnostic(format!(
            "`{}` must declare a Cargo version requirement of `{}` or a higher compatible floor",
            dependency.name, dependency.floor
        ))),
    }

    let mut features = declaration_features(workspace_declaration.unwrap_or(declaration));
    features.extend(declaration_features(declaration));
    for feature in check.features {
        if !features.contains(*feature) {
            diagnostics.push(diagnostic(format!(
                "generated client requires Cargo feature `{feature}` on `{}`",
                dependency.name
            )));
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
            "`{}` must set `default-features = false` for the supported freestanding runtime graph",
            dependency.name
        )));
    }
    if check.require_optional && declaration_bool(declaration, "optional") != Some(true) {
        diagnostics.push(diagnostic(format!(
            "`{}` must be optional because it is enabled only by the generated `blocking` feature",
            dependency.name
        )));
    }
    if [Some(declaration), workspace_declaration]
        .into_iter()
        .flatten()
        .any(|value| value.get("package").is_some())
    {
        diagnostics.push(diagnostic(format!(
            "`{}` cannot be renamed because generated code references that canonical crate name",
            dependency.name
        )));
    }
}

fn dotted_get<'a>(value: &'a toml::Value, dotted: &str, key: &str) -> Option<&'a toml::Value> {
    if dotted == "target.'cfg(not(target_arch = \"wasm32\"))'.dependencies" {
        return value
            .get("target")?
            .get("cfg(not(target_arch = \"wasm32\"))")?
            .get("dependencies")?
            .get(key);
    }
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

    fn audit_manifest(contents: &str, requirements: RuntimeRequirements) -> Vec<Diagnostic> {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
        std::fs::write(&manifest, contents).unwrap();
        audit(&manifest, &requirements).diagnostics
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
