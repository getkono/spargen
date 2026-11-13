//! What generated output requires of its consuming package: the tested dependency floors, the
//! capabilities one lowered API uses, and the requirement table that both the `E023` audit and
//! `spargen deps` read.

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::ir::{Api, DateTimeBackend, Prim, TypeKind};
use crate::Spec;

pub(super) const BYTES: Dependency = Dependency::new("bytes", "1.12.1", 2, 0);
pub(super) const FUTURES_CORE: Dependency = Dependency::new("futures-core", "0.3.32", 0, 4);
pub(super) const HTTP: Dependency = Dependency::new("http", "1.4.2", 2, 0);
pub(super) const HTTP_BODY: Dependency = Dependency::new("http-body", "1.0.1", 2, 0);
pub(super) const REQWEST: Dependency = Dependency::new("reqwest", "0.13.5", 0, 14);
pub(super) const SECRECY: Dependency = Dependency::new("secrecy", "0.10.3", 0, 11);
pub(super) const SERDE: Dependency = Dependency::new("serde", "1.0.229", 2, 0);
pub(super) const SERDE_JSON: Dependency = Dependency::new("serde_json", "1.0.151", 2, 0);
pub(super) const QUICK_XML: Dependency = Dependency::new("quick-xml", "0.42.0", 0, 43);
pub(super) const UUID: Dependency = Dependency::new("uuid", "1.24.0", 2, 0);
pub(super) const TIME: Dependency = Dependency::new("time", "0.3.55", 0, 4);
pub(super) const JIFF: Dependency = Dependency::new("jiff", "0.2.42", 0, 3);
pub(super) const CHRONO: Dependency = Dependency::new("chrono", "0.4.45", 0, 5);
pub(super) const TOKIO: Dependency = Dependency::new("tokio", "1.53.3", 2, 0);
pub(super) const URL: Dependency = Dependency::new("url", "2.5.8", 3, 0);

#[derive(Debug, Clone, Copy)]
pub(super) struct Dependency {
    pub(super) name: &'static str,
    pub(super) floor: &'static str,
    pub(super) ceiling_major: u64,
    pub(super) ceiling_minor: u64,
}

impl Dependency {
    /// A dependency tested from `floor` up to, not including, `ceiling_major.ceiling_minor.0`:
    /// the next major for a crate at 1.0 or later (`ceiling_minor` 0), the next minor of a `0.x`
    /// line.
    const fn new(
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
    pub(crate) bytes_serde: bool,
    /// The module reads a body as it arrives, through a sequential response or a binary `_stream`
    /// twin ([`crate::ir::RuntimeFeatures::chunked_reads`]): `futures-core` and reqwest's `stream`
    /// feature, which the `wasm32` chunked read of a `fetch` response needs.
    pub(crate) streams: bool,
    pub(crate) xml: bool,
    pub(crate) uuid: bool,
    /// The crate the embedded date newtypes wrap, when the module embeds them.
    pub(crate) datetime: Option<DateTimeBackend>,
}

impl RuntimeRequirements {
    pub(crate) fn for_api(api: &Api, spec: &Spec) -> Self {
        let features = api.runtime_features(spec.datetime);
        Self {
            bytes_serde: api.uses_bytes_serde(),
            streams: features.chunked_reads,
            xml: features.xml,
            uuid: spec.uuid.is_on()
                && api.types.emitted().any(|(_, definition)| {
                    matches!(definition.kind, TypeKind::Primitive(Prim::Uuid))
                }),
            datetime: features.datetime,
        }
    }
}

/// One dependency the consuming package must declare, as spargen derived it from the lowered API:
/// a row of the table both the audit (`E023`) and `spargen deps` read.
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
    /// Only required when the consumer opts in through the named Cargo feature (`blocking`).
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
/// `feature = "blocking"`).
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
    // The transport seam's request, response, and body trait: every generated client carries
    // `HttpBackend`, so both are unconditional.
    push(HTTP, NONE, false);
    push(HTTP_BODY, NONE, false);
    // The default backend. A `multipart/form-data` body is encoded by the runtime itself, so only
    // a body read as it arrives needs a feature of it (`stream`, through which a `wasm32` `fetch`
    // response is read chunk by chunk, for an `EventStream` or a binary `_stream` twin's
    // `ByteStream`).
    push(
        REQWEST,
        if requirements.streams {
            &["stream"]
        } else {
            NONE
        },
        true,
    );
    if requirements.streams {
        push(FUTURES_CORE, NONE, false);
    }
    push(SECRECY, NONE, false);
    push(SERDE, &["derive"], false);
    push(SERDE_JSON, NONE, false);
    // The URL every request is built on, and the type `{op}_url`, `ClientCore::base_url` and
    // pagination name: unconditional, like the seam's `http`.
    push(URL, NONE, false);
    if requirements.xml {
        push(QUICK_XML, &["serialize"], false);
    }
    if requirements.uuid {
        push(UUID, &["serde"], false);
    }
    match requirements.datetime {
        // NOT `serde`: the embedded `DateTime`/`Date` newtypes write RFC 3339 themselves, because
        // `time`'s own serde representation is a nine-integer sequence without
        // `serde-human-readable` and a space-separated form with it — neither is what OpenAPI's
        // `format: date-time` means. `formatting`/`parsing` are what the RFC 3339 codec needs.
        Some(DateTimeBackend::Time) => push(TIME, &["formatting", "parsing"], false),
        // The other backends need no feature: the embedded `rfc3339` codec reads and writes the
        // text, and only each crate's calendar types and conversions are named.
        Some(DateTimeBackend::Jiff) => push(JIFF, NONE, false),
        Some(DateTimeBackend::Chrono) => push(CHRONO, NONE, false),
        Some(DateTimeBackend::String) | None => {}
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
#[non_exhaustive]
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
/// the `E023` audit checks a consumer manifest against. Both read one requirement table, so the
/// block [`Requirements::manifest_block`] prints passes the audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
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
    /// Opt-in dependencies (the blocking client's `tokio`) are rendered commented out
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
