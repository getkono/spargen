//! What generated output requires of its consuming package: the tested dependency floors, the
//! capabilities one lowered API uses, and the requirement table that both the `E023` audit and
//! `spargen deps` read.

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::ir::{Api, MediaType, Prim, TypeKind};
use crate::Spec;

pub(super) const BYTES: Dependency = Dependency::stable("bytes", "1.12.1", 2);
pub(super) const FUTURES_CORE: Dependency = Dependency::unstable("futures-core", "0.3.32", 0, 4);
pub(super) const REQWEST: Dependency = Dependency::unstable("reqwest", "0.12.28", 0, 13);
pub(super) const SECRECY: Dependency = Dependency::unstable("secrecy", "0.10.3", 0, 11);
pub(super) const SERDE: Dependency = Dependency::stable("serde", "1.0.229", 2);
pub(super) const SERDE_JSON: Dependency = Dependency::stable("serde_json", "1.0.151", 2);
pub(super) const QUICK_XML: Dependency = Dependency::unstable("quick-xml", "0.41.0", 0, 42);
pub(super) const UUID: Dependency = Dependency::stable("uuid", "1.24.0", 2);
pub(super) const TIME: Dependency = Dependency::unstable("time", "0.3.55", 0, 4);
pub(super) const TOKIO: Dependency = Dependency::stable("tokio", "1.53.1", 2);

#[derive(Debug, Clone, Copy)]
pub(super) struct Dependency {
    pub(super) name: &'static str,
    pub(super) floor: &'static str,
    pub(super) ceiling_major: u64,
    pub(super) ceiling_minor: u64,
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

    pub(super) fn floor_version(self) -> Version {
        Version::parse(self.floor).expect("runtime dependency floors are valid semver")
    }

    pub(super) fn ceiling(self) -> Version {
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
            bytes_serde: api.uses_bytes_serde(),
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

/// One dependency the consuming package must declare, as spargen derived it from the lowered API.
///
/// This is the single source of truth behind both the audit (`E023`) and `spargen deps`: the two
/// read the same table, so what the audit demands and what `deps` prints cannot drift.
#[derive(Debug, Clone, Copy)]
pub(super) struct Requirement {
    /// The manifest table `spargen deps` prints it in — `dependencies`, or a target-specific
    /// table.
    pub(super) table: &'static str,
    /// How the audit locates it in the consumer manifest.
    pub(super) placement: Placement,
    pub(super) dependency: Dependency,
    pub(super) features: &'static [&'static str],
    /// `default-features = false` is part of the contract (reqwest's defaults pull in a TLS
    /// stack the generated client does not choose).
    pub(super) no_default_features: bool,
    /// Declared `optional = true` and wired to a Cargo feature of the consumer's own.
    pub(super) optional: bool,
    /// Only required when the consumer opts in — currently the `blocking` Cargo feature.
    pub(super) conditional: Option<&'static str>,
}

/// Where a requirement is declared, which decides how the audit finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placement {
    /// The plain `[dependencies]` table.
    Dependencies,
    /// Any native-only `[target.<key>.dependencies]` table Cargo applies to the build: the blocking
    /// client's `tokio`, which generated code names only under [`BLOCKING_GATE`].
    NativeTarget,
}

/// The `cfg` generated code puts around every use of the blocking client's `tokio` (together with
/// `feature = "blocking"`), and the table `spargen deps` prints for it.
pub(super) const BLOCKING_GATE: &str = "not(target_arch = \"wasm32\")";

/// The target the native-only rule is anchored to: the only wasm target generated code supports.
/// A `tokio` table that can apply here is not native-only.
pub(super) const WASM_ANCHOR: &str = "wasm32-unknown-unknown";

/// The dependency table for one lowered API. `deps` renders it; `audit` checks the consumer
/// manifest against it.
pub(super) fn requirement_table(requirements: &RuntimeRequirements) -> Vec<Requirement> {
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
/// Both read one private requirement table, so the block [`Requirements::manifest_block`] prints
/// passes the audit: as printed for a package that declares no opt-in feature, and with that
/// feature's commented lines uncommented (merged into any table or key the manifest already
/// declares) for one that does.
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
    /// under the feature that would require them, together with that feature's `[features]` entry
    /// enabling them (`blocking = ["dep:tokio"]`). Uncommenting is the whole opt-in, except that an
    /// entry whose `[features]` table, `blocking` key, or dependency table the manifest already
    /// declares merges into it rather than being added a second time, which TOML rejects.
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
        let mut features: Vec<&str> = Vec::new();
        for feature in self
            .dependencies
            .iter()
            .filter_map(|dependency| dependency.required_by_feature)
        {
            if !features.contains(&feature) {
                features.push(feature);
            }
        }
        for feature in features {
            let gated: Vec<&RequiredDependency> = self
                .dependencies
                .iter()
                .filter(|dependency| dependency.required_by_feature == Some(feature))
                .collect();
            // Uncommented as printed, a `[features]` table or `{feature}` key the manifest already
            // declares would be defined twice and the manifest would no longer parse, so the
            // header says each entry merges into what is already there.
            rendered.push_str(&format!(
                "\n# To opt in to the `{feature}` Cargo feature, uncomment the lines below, merging \
                 each entry into a table (or `{feature}` key) your manifest already declares:\n"
            ));
            // The feature must enable each optional dependency it gates; the audit rejects a
            // declared feature entry that does not.
            let enables = gated
                .iter()
                .filter(|dependency| dependency.optional)
                .map(|dependency| format!("\"dep:{}\"", dependency.name))
                .collect::<Vec<_>>()
                .join(", ");
            rendered.push_str("# [features]\n");
            rendered.push_str(&format!("# {feature} = [{enables}]\n"));
            let mut table: Option<&str> = None;
            for dependency in gated {
                if table != Some(dependency.table) {
                    rendered.push_str(&format!("# [{}]\n", dependency.table));
                    table = Some(dependency.table);
                }
                rendered.push_str(&format!("# {}\n", dependency.manifest_line()));
            }
        }
        rendered
    }
}

impl std::fmt::Display for Requirements {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.manifest_block().trim_end())
    }
}
