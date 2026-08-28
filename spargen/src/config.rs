//! The generation inputs: [`Spec`] (*what* is generated) and [`Build`] (*where* it is written).
//!
//! Every entry point takes one of these two. [`check`](crate::check), [`diff`](crate::diff),
//! [`vendor`](crate::vendor) and [`requirements`](crate::requirements) analyse a spec and write
//! nothing, so they take a `Spec`; only [`generate`](crate::generate) writes a file, so only it
//! takes a `Build`. That split is why the analysis entry points no longer demand a fabricated
//! output path that is never used.
//!
//! Both types have private fields and chained setters, so a new knob is an additive change rather
//! than a breaking one.
//!
//! # `spargen.toml`
//!
//! The same knobs can be read from a config file, which is available to `build.rs`, the
//! `generate_api!` macro, and the CLI alike — the file is parsed here, in the library, so the
//! three cannot drift:
//!
//! ```toml
//! uuid = true             # map `format: uuid` to `uuid::Uuid` (default true)
//! time = true             # map `format: date-time`/`date` to the `time` crate (default true)
//! carve = false           # auto-carve unsupported constructs (same as `--carve`)
//! batch_cap = 100         # max diagnostics collected before batching stops
//! error_body_cap = 65536  # max bytes of a response body retained on error variants
//!
//! # Zero or more omit rules. The rule KIND is discriminated by field presence. A path/name (or
//! # pointer) value that contains a glob metacharacter (`*`, `**`, `?`) is matched as a glob and
//! # removes EVERY matching construct (bulk); a value with no metacharacter is an exact rule.
//! [[omit]]
//! path = "/pets/{id}"                    # → OmitRule::Path (exact)
//! [[omit]]
//! path = "/admin/**"                     # → OmitRule::Path (glob: every path under /admin)
//! [[omit]]
//! method = "get"                         # `method` + `path` → OmitRule::Operation
//! path = "/pets"
//! [[omit]]
//! component = "schema"                   # `component` + `name` → OmitRule::Component
//! name = "LegacyPet"                     #   component ∈ schemas/responses/parameters/…
//! [[omit]]
//! pointer = "/components/schemas/X"      # `pointer` → OmitRule::Pointer
//! file = "extra.yaml"                    #   `file` optional (file-local pointer)
//! ```

use camino::{Utf8Path, Utf8PathBuf};

use crate::compat::{Omit, OmitRule};

/// The default cap on bytes of a response body retained on generated error variants.
pub(crate) const DEFAULT_ERROR_BODY_CAP: usize = 64 * 1024;
/// The default cap on diagnostics collected before batching stops.
pub(crate) const DEFAULT_BATCH_CAP: usize = 100;

/// What to generate: the spec to read plus every knob that can change the generated API.
///
/// Two `Spec`s that compare equal produce byte-identical output for a given spargen version —
/// the build cache fingerprints exactly these fields.
///
/// ```no_run
/// let spec = spargen::Spec::new("api/openapi.yaml").uuid(false);
/// let report = spargen::check(&spec);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub(crate) path: Utf8PathBuf,
    pub(crate) uuid: bool,
    pub(crate) time: bool,
    pub(crate) omit: Omit,
    pub(crate) error_body_cap: usize,
    pub(crate) batch_cap: usize,
    pub(crate) carve: bool,
}

impl Spec {
    /// A spec with defaults: `uuid` and `time` on, a 64 KiB error-body cap, a 100-diagnostic
    /// batch, no omissions, no carving.
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            path: path.into(),
            uuid: true,
            time: true,
            omit: Omit::default(),
            error_body_cap: DEFAULT_ERROR_BODY_CAP,
            batch_cap: DEFAULT_BATCH_CAP,
            carve: false,
        }
    }

    /// The path to the root OpenAPI document.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Map `format: uuid` to `uuid::Uuid` (default `true`). Turning this off falls back to
    /// `String` — a deliberate, documented loss of typing that also drops `uuid` from the
    /// dependencies generated output requires.
    pub fn uuid(mut self, enabled: bool) -> Self {
        self.uuid = enabled;
        self
    }

    /// Map `format: date-time`/`date` to the embedded RFC 3339 `DateTime`/`Date` newtypes over
    /// `time::OffsetDateTime`/`time::Date` (default `true`). Turning this off falls back to `String`
    /// and drops `time` from the required dependencies.
    pub fn time(mut self, enabled: bool) -> Self {
        self.time = enabled;
        self
    }

    /// Replace the compatibility omissions applied before OpenAPI validation and lowering.
    pub fn omit(mut self, omit: Omit) -> Self {
        self.omit = omit;
        self
    }

    /// Add one omission to the existing set.
    pub fn omit_rule(mut self, rule: OmitRule) -> Self {
        self.omit.rules.push(rule);
        self
    }

    /// Cap the bytes of a response body retained on generated error variants (default 64 KiB).
    pub fn error_body_cap(mut self, bytes: usize) -> Self {
        self.error_body_cap = bytes;
        self
    }

    /// Cap the diagnostics collected before batching stops (default 100).
    pub fn batch_cap(mut self, count: usize) -> Self {
        self.batch_cap = count;
        self
    }

    /// Auto-carve: instead of failing on rejections, iteratively omit the minimal enclosing
    /// omittable construct for each rejection and generate the rest. Every carved construct is
    /// reported via `W009`; residual, un-carvable rejections are reported honestly.
    pub fn carve(mut self, enabled: bool) -> Self {
        self.carve = enabled;
        self
    }

    /// Where to write the generated module. The result is the input to [`generate`](crate::generate).
    pub fn build(self, output: impl Into<Utf8PathBuf>) -> Build {
        Build::new(self, output)
    }

    /// Apply a `spargen.toml` over this spec. The file must exist; every key it sets overrides
    /// what is already on the spec, and its `[[omit]]` rules are appended.
    ///
    /// Call this *before* any setter that must win over the file — `Spec` setters apply in
    /// chained order, so last write wins.
    pub fn config_file(self, path: impl AsRef<Utf8Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path.as_std_path()).map_err(|error| {
            ConfigError::new(format!("cannot read config file `{path}`: {error}"))
        })?;
        self.apply_config(&text, path)
    }

    /// Apply `spargen.toml` from beside the spec, if one is there. A missing file is not an
    /// error — this is the discovery path every entry point uses by default.
    pub fn discover_config_file(self) -> Result<Self, ConfigError> {
        let discovered = self
            .path
            .parent()
            .unwrap_or_else(|| Utf8Path::new(""))
            .join("spargen.toml");
        if !discovered.as_std_path().is_file() {
            return Ok(self);
        }
        self.config_file(&discovered)
    }

    /// Apply already-read config-file text. Split out so the parser is testable without I/O.
    pub(crate) fn apply_config(mut self, text: &str, path: &Utf8Path) -> Result<Self, ConfigError> {
        let file = toml::from_str::<FileConfig>(text)
            .map_err(|error| ConfigError::new(format!("invalid config file `{path}`: {error}")))?;
        if file.features.is_some() {
            // `deny_unknown_fields` alone would say "unknown field `features`", which does not
            // tell a 0.2 user what to do. The keys moved to the top level in 0.3.
            return Err(ConfigError::new(format!(
                "invalid config file `{path}`: the `[features]` table was removed; \
                 move its keys (`carve`, `batch_cap`, …) to the top level of the file"
            )));
        }
        if let Some(value) = file.uuid {
            self.uuid = value;
        }
        if let Some(value) = file.time {
            self.time = value;
        }
        if let Some(value) = file.carve {
            self.carve = value;
        }
        if let Some(value) = file.batch_cap {
            self.batch_cap = value;
        }
        if let Some(value) = file.error_body_cap {
            self.error_body_cap = value;
        }
        for (index, entry) in file.omit.iter().enumerate() {
            let rule = entry.to_rule().map_err(|message| {
                ConfigError::new(format!(
                    "invalid config file `{path}`: omit rule #{}: {message}",
                    index + 1
                ))
            })?;
            self.omit.rules.push(rule);
        }
        Ok(self)
    }
}

/// How generated output interacts with Cargo: `cargo:rerun-if-changed` directives and the
/// consumer-manifest dependency audit (`E023`).
///
/// Both need a real build-script process (`OUT_DIR` and friends in the environment). Outside
/// one — a test, a CLI wrapper, an ad-hoc binary — spargen cannot emit directives Cargo would
/// read, nor find the manifest to audit. This enum makes that a decision rather than a silent
/// degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CargoIntegration {
    /// Integrate when running under a build script; otherwise degrade with `W013` (directives)
    /// and `W012` (audit). The right choice for a `build.rs`.
    #[default]
    Auto,
    /// Integrate, or fail with `E024`. Use this when a missing rebuild trigger would ship a
    /// stale client.
    Required,
    /// Never integrate, and say nothing about it. Use this for generation that is deliberately
    /// not part of a Cargo build — a test harness, a code-gen CLI, a one-shot script.
    Off,
}

/// Where to generate: a [`Spec`] plus the output path and Cargo integration policy. The input to
/// [`generate`](crate::generate).
///
/// ```no_run
/// // build.rs — spec to first typed API call in well under ten lines.
/// let build = spargen::Spec::new("api/openapi.yaml").build("src/api.rs");
/// spargen::generate(&build).expect_success();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub(crate) spec: Spec,
    pub(crate) output: Utf8PathBuf,
    pub(crate) cargo: CargoIntegration,
}

impl Build {
    /// A build with [`CargoIntegration::Auto`]. Prefer [`Spec::build`], which reads in one line.
    pub fn new(spec: Spec, output: impl Into<Utf8PathBuf>) -> Self {
        Self {
            spec,
            output: output.into(),
            cargo: CargoIntegration::Auto,
        }
    }

    /// Choose how this build interacts with Cargo.
    pub fn cargo(mut self, integration: CargoIntegration) -> Self {
        self.cargo = integration;
        self
    }

    /// The spec being generated from.
    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Where the generated module is written.
    pub fn output(&self) -> &Utf8Path {
        &self.output
    }
}

/// A clear, user-facing config-file error. Rendered by the CLI as a usage error — never a panic.
#[derive(Debug)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    /// A config error with a caller-supplied message. Public so a front end (the CLI, a build
    /// script wrapper) can report *its* own flag-syntax errors through the same type the library
    /// returns, rather than inventing a parallel one.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

// --- TOML DTOs ---------------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    uuid: Option<bool>,
    time: Option<bool>,
    carve: Option<bool>,
    batch_cap: Option<usize>,
    error_body_cap: Option<usize>,
    /// Retained solely to give the removed 0.2 `[features]` table a targeted error.
    features: Option<toml::Value>,
    #[serde(default)]
    omit: Vec<OmitToml>,
}

/// An `[[omit]]` entry. The rule kind is discriminated by which fields are present (TOML has no
/// native enums): `pointer` ⇒ Pointer, `component`+`name` ⇒ Component, `method`+`path` ⇒
/// Operation, `path` alone ⇒ Path.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OmitToml {
    path: Option<String>,
    method: Option<String>,
    component: Option<String>,
    name: Option<String>,
    pointer: Option<String>,
    file: Option<String>,
}

impl OmitToml {
    fn to_rule(&self) -> Result<OmitRule, String> {
        if let Some(pointer) = &self.pointer {
            if self.path.is_some() || self.method.is_some() || self.component.is_some() {
                return Err("`pointer` cannot be combined with path/method/component".to_owned());
            }
            return Ok(OmitRule::Pointer {
                file: self.file.clone().map(Into::into),
                pointer: pointer.clone().into(),
            });
        }
        if let Some(component) = &self.component {
            let name = self.name.as_ref().ok_or("`component` requires a `name`")?;
            return Ok(OmitRule::Component {
                kind: component.parse().map_err(|error| {
                    format!(
                        "{error}; expected one of \
                         schemas/responses/parameters/request_bodies/headers/security_schemes"
                    )
                })?,
                name: name.clone().into(),
            });
        }
        if let Some(path) = &self.path {
            return match &self.method {
                Some(method) => Ok(OmitRule::Operation {
                    method: method.parse().map_err(|error| {
                        format!(
                            "{error}; expected one of \
                             get/put/post/delete/options/head/patch/trace"
                        )
                    })?,
                    path: path.clone().into(),
                }),
                None => Ok(OmitRule::Path {
                    path: path.clone().into(),
                }),
            };
        }
        Err("must specify one of `path`, `component` (+`name`), or `pointer`".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{ComponentKind, OmitMethod};

    fn apply(text: &str) -> Result<Spec, ConfigError> {
        Spec::new("openapi.yaml").apply_config(text, Utf8Path::new("spargen.toml"))
    }

    #[test]
    fn defaults_are_pinned() {
        // These are a published contract, so a changed default fails a test rather than only a doc.
        let spec = Spec::new("openapi.yaml");
        assert!(spec.uuid);
        assert!(spec.time);
        assert!(!spec.carve);
        assert_eq!(spec.batch_cap, 100);
        assert_eq!(spec.error_body_cap, 64 * 1024);
        assert!(spec.omit.is_empty());
        assert_eq!(Build::new(spec, "out.rs").cargo, CargoIntegration::Auto);
    }

    #[test]
    fn every_knob_round_trips_through_the_config_file() {
        let spec = apply(
            r#"
            uuid = false
            time = false
            carve = true
            batch_cap = 7
            error_body_cap = 11
            "#,
        )
        .unwrap();
        assert!(!spec.uuid);
        assert!(!spec.time);
        assert!(spec.carve);
        assert_eq!(spec.batch_cap, 7);
        assert_eq!(spec.error_body_cap, 11);
    }

    #[test]
    fn setters_after_the_config_file_win() {
        let spec = apply("carve = true\nbatch_cap = 7\n").unwrap().carve(false);
        assert!(!spec.carve);
        assert_eq!(spec.batch_cap, 7, "untouched keys survive");
    }

    #[test]
    fn toml_omit_entries_map_by_field_presence() {
        let spec = apply(
            r#"
            [[omit]]
            path = "/pets/{id}"

            [[omit]]
            method = "post"
            path = "/pets"

            [[omit]]
            component = "schema"
            name = "LegacyPet"

            [[omit]]
            pointer = "/components/schemas/X"
            file = "extra.yaml"
            "#,
        )
        .unwrap();
        assert_eq!(spec.omit.rules[0], OmitRule::path("/pets/{id}"));
        assert_eq!(
            spec.omit.rules[1],
            OmitRule::operation(OmitMethod::Post, "/pets")
        );
        assert_eq!(
            spec.omit.rules[2],
            OmitRule::component(ComponentKind::Schemas, "LegacyPet")
        );
        assert_eq!(
            spec.omit.rules[3],
            OmitRule::pointer(Some("extra.yaml".into()), "/components/schemas/X")
        );
    }

    #[test]
    fn config_file_omits_are_appended_not_replaced() {
        let spec = Spec::new("openapi.yaml")
            .omit_rule(OmitRule::path("/first"))
            .apply_config("[[omit]]\npath = \"/second\"\n", Utf8Path::new("c.toml"))
            .unwrap();
        assert_eq!(
            spec.omit.rules,
            vec![OmitRule::path("/first"), OmitRule::path("/second")]
        );
    }

    #[test]
    fn the_removed_features_table_gets_a_targeted_error() {
        let error = apply("[features]\ncarve = true\n").unwrap_err().to_string();
        assert!(
            error.contains("move its keys"),
            "expected migration guidance, got: {error}"
        );
    }

    #[test]
    fn ambiguous_or_empty_omit_entry_errors() {
        assert!(apply("[[omit]]\ncomponent = \"schema\"\n").is_err());
        assert!(apply("[[omit]]\n").is_err());
        assert!(apply("[[omit]]\npointer = \"/x\"\npath = \"/y\"\n").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(apply("frobnicate = true\n").is_err());
    }
}
