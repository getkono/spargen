//! The generation inputs: [`Spec`] (*what* is generated) and [`Build`] (*where* it is written).
//!
//! Every entry point takes one of these two. [`check`](crate::check), [`diff`](crate::diff) and
//! [`requirements`](crate::requirements) analyse a spec and write nothing, so they take a `Spec`.
//! [`vendor`](crate::vendor) takes a `Spec` too: it writes only beside the spec
//! (`.spargen/vendor/` and `spargen.lock`). [`generate`](crate::generate) writes the generated
//! module, so it takes a `Build`, and the other entry points demand no output path they would
//! never use.
//!
//! # `spargen.toml`
//!
//! The same knobs can be read from a config file, which is available to `build.rs`, the
//! `generate_api!` macro, and the CLI alike — the file is parsed here, in the library, so the
//! three cannot drift. Its keys other than `[[omit]]` and `[[keep]]`, with their defaults, are
//! listed once, in the CLI reference's
//! [`spargen.toml` reference](https://github.com/getkono/spargen/blob/master/docs/book/src/cli.md#spargentoml-reference);
//! one `[[omit]]` rule of each kind, how a rule's kind is told apart, and a `[[keep]]` entry are
//! shown once, in the
//! [compatibility guide](https://github.com/getkono/spargen/blob/master/docs/compatibility.md#spargentoml-and-the-analysis-cli).

use std::borrow::Cow;

use camino::{Utf8Path, Utf8PathBuf};

use crate::codegen::DEFAULT_ERROR_BODY_CAP;
use crate::compat::{KeepRule, Omit, OmitMethod, OmitRule, UnknownOmitToken};
use crate::ir::DateTimeBackend;

/// The default cap on diagnostics collected before batching stops.
const DEFAULT_BATCH_CAP: usize = 100;

/// What to generate: the spec to read plus every knob that can change the generated API.
///
/// Two `Spec`s that compare equal produce byte-identical output for a given spargen version *and
/// identical spec-file contents*. Equality covers the knobs below, including the spec's path and
/// the paths of the config files applied to it; it cannot see the bytes at those paths, which the
/// build cache fingerprints separately.
///
/// ```no_run
/// let spec = spargen::Spec::new("api/openapi.yaml").uuid(false);
/// let report = spargen::check(&spec);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub(crate) path: Utf8PathBuf,
    pub(crate) uuid: bool,
    pub(crate) datetime: DateTimeBackend,
    pub(crate) omit: Omit,
    pub(crate) error_body_cap: usize,
    pub(crate) batch_cap: usize,
    pub(crate) carve: bool,
    pub(crate) open_narrowing: bool,
    pub(crate) open_unions: bool,
    pub(crate) verbatim_docs: bool,
    /// The header names [`Spec::bind_header`] bound, in the order bound, no two equal in any
    /// letter case.
    pub(crate) bind_headers: Vec<String>,
    /// Every config file applied through [`Spec::config_file`] (and so through
    /// [`Spec::discover_config_file`]), in the order applied. Generation reads them, so `build.rs`
    /// names each as a `cargo:rerun-if-changed` path and `generate_api!` tracks each.
    pub(crate) config_files: Vec<Utf8PathBuf>,
}

impl Spec {
    /// A spec with defaults: `uuid` on, dates mapped through the `time` crate
    /// ([`DateTimeBackend::Time`]), a 64 KiB error-body cap, a 100-diagnostic
    /// batch, no omissions, no carving, no open narrowing, no open unions, carried docs
    /// rewritten for rustdoc rather than verbatim, and no bound headers.
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            path: path.into(),
            uuid: true,
            datetime: DateTimeBackend::Time,
            omit: Omit::default(),
            error_body_cap: DEFAULT_ERROR_BODY_CAP,
            batch_cap: DEFAULT_BATCH_CAP,
            carve: false,
            open_narrowing: false,
            open_unions: false,
            verbatim_docs: false,
            bind_headers: Vec::new(),
            config_files: Vec::new(),
        }
    }

    /// The path to the root OpenAPI document.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Whether `format: uuid` maps to `uuid::Uuid`, as [`uuid`](Self::uuid) set it.
    pub fn uuid_enabled(&self) -> bool {
        self.uuid
    }

    /// The crate `format: date-time`/`date` map to, as [`datetime`](Self::datetime) set it.
    pub fn datetime_backend(&self) -> DateTimeBackend {
        self.datetime
    }

    /// The compatibility omissions and keep rules, in chained order: [`omit`](Self::omit) replaces
    /// the set, and [`omit_rule`](Self::omit_rule), [`keep_operation`](Self::keep_operation) and
    /// each applied config file append to it.
    pub fn omissions(&self) -> &Omit {
        &self.omit
    }

    /// The cap on retained error-body bytes, as [`error_body_cap`](Self::error_body_cap) set it.
    pub fn error_body_cap_bytes(&self) -> usize {
        self.error_body_cap
    }

    /// The cap on diagnostics collected before batching stops, as
    /// [`batch_cap`](Self::batch_cap) set it.
    pub fn batch_cap_count(&self) -> usize {
        self.batch_cap
    }

    /// Whether auto-carve is on, as [`carve`](Self::carve) set it.
    pub fn carve_enabled(&self) -> bool {
        self.carve
    }

    /// Whether a narrowing string `enum`/`const` lowers as an open set, as
    /// [`open_narrowing`](Self::open_narrowing) set it.
    pub fn open_narrowing_enabled(&self) -> bool {
        self.open_narrowing
    }

    /// Whether a discriminated union decodes a tag value it does not list into its `Unknown`
    /// variant, as [`open_unions`](Self::open_unions) set it.
    pub fn open_unions_enabled(&self) -> bool {
        self.open_unions
    }

    /// Whether the spec's prose is carried into rustdoc as written, as
    /// [`verbatim_docs`](Self::verbatim_docs) set it.
    pub fn verbatim_docs_enabled(&self) -> bool {
        self.verbatim_docs
    }

    /// The header names [`bind_header`](Self::bind_header) bound, in the order bound.
    pub fn bound_headers(&self) -> &[String] {
        &self.bind_headers
    }

    /// Every config file applied through [`config_file`](Self::config_file) or
    /// [`discover_config_file`](Self::discover_config_file), in the order applied.
    pub fn config_files(&self) -> &[Utf8PathBuf] {
        &self.config_files
    }

    /// Map `format: uuid` to `uuid::Uuid` (default `true`). Turning this off falls back to
    /// `String` — a deliberate, documented loss of typing that also drops `uuid` from the
    /// dependencies generated output requires.
    pub fn uuid(mut self, enabled: bool) -> Self {
        self.uuid = enabled;
        self
    }

    /// Choose the crate `format: date-time`/`date` map to (default [`DateTimeBackend::Time`]).
    ///
    /// A typed backend maps them to the embedded RFC 3339 `DateTime`/`Date` newtypes over that
    /// crate's types, with spargen's own RFC 3339 codec whichever crate it is, and the dependency
    /// audit requires that crate. [`DateTimeBackend::String`] falls back to `String` and requires
    /// no date-time crate.
    ///
    /// ```no_run
    /// use spargen::DateTimeBackend;
    ///
    /// let spec = spargen::Spec::new("api/openapi.yaml").datetime(DateTimeBackend::Jiff);
    /// ```
    pub fn datetime(mut self, backend: DateTimeBackend) -> Self {
        self.datetime = backend;
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

    /// Keep one operation: once any keep rule is set, every operation no keep rule names is
    /// omitted, and so is every component of the root document the kept operations do not reach.
    /// Omit rules still apply on top. A keep rule naming an operation the document does not declare
    /// fails generation with `E019`. Keep rules are part of the [`Omit`] profile, so
    /// [`omit`](Self::omit) replaces them too; call this after it.
    ///
    /// ```no_run
    /// use spargen::OmitMethod;
    ///
    /// let spec = spargen::Spec::new("api/openapi.yaml")
    ///     .keep_operation(OmitMethod::Get, "/v1/sync")
    ///     .keep_operation(OmitMethod::Get, "/v1/blob/{address}");
    /// ```
    pub fn keep_operation(
        mut self,
        method: OmitMethod,
        path: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.omit.keep.push(KeepRule::operation(method, path));
        self
    }

    /// Cap the bytes of a response body retained on generated error variants (default 64 KiB).
    ///
    /// For bodies read as errors on native targets, the cap bounds reading as well as retention:
    /// one that exceeds it is abandoned partway, which forgoes reuse of that connection. Setting
    /// it far below the error bodies an API actually returns therefore trades connection reuse for
    /// a smaller retained prefix. On `wasm32` the `fetch` backend has no incremental read, so the
    /// cap bounds only what is retained — see the Targets row of `docs/support-matrix.md`.
    ///
    /// A *success* body that fails to decode is capped on retention only, on every target:
    /// deserialization needs the whole body, so it is always read whole. See
    /// `ClientConfig::max_error_body` in the emitted runtime for the full disposition, including
    /// the one path that is not capped at all.
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

    /// Lower a string `enum`/`const` that narrows a plain `string` as an **open** set (default
    /// `false`), inside a response body's own schema.
    ///
    /// The construct is a property that one `allOf` member (or the `$ref` it sits beside) declares
    /// as a plain `string` and another narrows with `enum`/`const` — the RFC 9457 shape
    /// `allOf: [$ref: Problem, {properties: {type: {const: …}}}]` that gives each documented error
    /// status its own problem `type`. By default the narrowing is exact: the field is a closed
    /// enum, and a value the description does not list is a decode failure (`Error::Decode`).
    /// With this on, the field is an open enum instead: each listed value is still its own
    /// variant, and one more variant holds any other string, which is the domain the wider
    /// declaration admits. A server that adds a problem type then still decodes into `Error::Api`.
    ///
    /// It applies to the schema written in the response itself, and to what that schema contains
    /// other than a `oneOf`/`anyOf`; a narrowing written inside a `$ref` target stays closed. A
    /// target is shared by every use and can be a union variant somewhere else, and a union tells
    /// its variants apart by what each one refuses, so opening a set there could make two variants
    /// accept the same value. A narrowing whose value set is itself a `$ref` target opens a copy
    /// and leaves the target closed. Request bodies and parameters are not affected.
    pub fn open_narrowing(mut self, enabled: bool) -> Self {
        self.open_narrowing = enabled;
        self
    }

    /// Give every discriminated union one more variant, `Unknown { tag, value }`, holding a
    /// payload whose discriminator value the union does not list (default `false`).
    ///
    /// By default a discriminated union is closed: a tag value that selects none of its variants
    /// is a decode failure (`Error::Decode`), the same as a malformed payload. An API that
    /// documents "new kinds may be added; a client ignores kinds it does not know" needs the two
    /// told apart. With this on, a payload whose tag is a string the union lists for no variant,
    /// and that no untagged variant accepts, decodes as `Unknown`: `tag` is that string and
    /// `value` the whole payload as it arrived. Serializing `Unknown` writes `value` back, with
    /// `tag` inserted under the tag property when `value` does not already carry it. A missing or
    /// non-string tag is still a decode failure.
    ///
    /// It applies to every union with a `discriminator`, in requests and responses alike, except
    /// one with an OpenAPI 3.2 `defaultMapping`, whose fallback variant already takes every tag
    /// value it does not list. Within the union the tag selects exactly one variant, and `Unknown`
    /// only where it selects none. An open union that is itself a variant of an enclosing union
    /// with no discriminator counts as a match there only when it decodes a payload as one of its
    /// listed variants: a payload it takes only as `Unknown` goes to another variant that matches
    /// it, as with this option off, and to the open union only when none does. An open union
    /// reached deeper inside such a variant, such as through a property, is not held back that
    /// way: it makes that variant accept more, so a `oneOf` payload another variant also matches
    /// can fail to decode where it decoded with this option off.
    pub fn open_unions(mut self, enabled: bool) -> Self {
        self.open_unions = enabled;
        self
    }

    /// Carry the spec's prose — `summary`, `description` and `title` — into rustdoc exactly as
    /// written (default `false`).
    ///
    /// By default that prose is rewritten so rustdoc has nothing to report from wherever the
    /// module is `include!`d, without changing what it says. A server framework that publishes its
    /// handlers' rustdoc as `description` carries intra-doc links naming items of the server's
    /// crate (``[`crate::serve`]``, `[Store]`), which no path from the generated module reaches:
    /// each such link becomes its text, so ``[`crate::serve`]`` renders as `` `crate::serve` ``. A
    /// bare `http(s)://` URL is wrapped in `<…>`, and an HTML tag nothing closes (`Vec<T>`'s
    /// `<T>`) has its `<` escaped. Links to URLs, code spans and code blocks are kept as written.
    ///
    /// With this on, the prose is left as the spec wrote it, and the `include!` site answers for
    /// `rustdoc::broken_intra_doc_links`, `rustdoc::bare_urls` and `rustdoc::invalid_html_tags`.
    pub fn verbatim_docs(mut self, enabled: bool) -> Self {
        self.verbatim_docs = enabled;
        self
    }

    /// Bind the header parameter `name` once on the client, rather than at every call.
    ///
    /// Every operation that declares a header parameter of that name, matched in any letter case,
    /// takes no argument for it: the generated client holds one value for it, set with
    /// `Client::with_bound_header(name, value)`, and each such operation sends that value as
    /// written. An operation that requires the header fails before anything is sent, with
    /// `RequestError::MissingBoundParameter`, while no value is bound; one that declares it
    /// optional leaves it off. Binding a name already bound, in any letter case, changes nothing.
    /// A name no operation declares as a header parameter fails generation with `E026`.
    ///
    /// ```no_run
    /// let spec = spargen::Spec::new("api/openapi.yaml")
    ///     .bind_header("X-Api-Version")
    ///     .bind_header("X-Tenant");
    /// ```
    pub fn bind_header(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !self
            .bind_headers
            .iter()
            .any(|bound| bound.eq_ignore_ascii_case(&name))
        {
            self.bind_headers.push(name);
        }
        self
    }

    /// Where to write the generated module. The result is the input to [`generate`](crate::generate).
    pub fn build(self, output: impl Into<Utf8PathBuf>) -> Build {
        Build::new(self, output)
    }

    /// Apply a `spargen.toml` over this spec. The file must exist; every key it sets overrides
    /// what is already on the spec, and its `[[omit]]` and `[[keep]]` rules are appended.
    ///
    /// Call this *before* any setter that must win over the file — `Spec` setters apply in
    /// chained order, so last write wins.
    ///
    /// The spec records `path`, so an edit to the file reruns the build:
    /// [`generate`](crate::generate) names it as a `cargo:rerun-if-changed` path and fingerprints
    /// its bytes, and `generate_api!` tracks it.
    pub fn config_file(self, path: impl AsRef<Utf8Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path.as_std_path()).map_err(|error| {
            ConfigError::new(format!("cannot read config file `{path}`: {error}"))
        })?;
        let mut spec = self.apply_config(&text, path)?;
        spec.config_files.push(path.to_path_buf());
        Ok(spec)
    }

    /// Apply `spargen.toml` from beside the spec, if one is there. A missing file is not an
    /// error. `generate_api!` always calls this, and the CLI does when no `--config` is given; a
    /// `build.rs` that wants the file calls it explicitly.
    ///
    /// A file that is found is recorded as [`config_file`](Self::config_file) records it. A
    /// missing one records nothing: creating `spargen.toml` later does not rerun a build on its
    /// own, because Cargo reruns a build script on every build for a `rerun-if-changed` path
    /// that does not exist, and `include_bytes!` of one does not compile. Touch the spec, or
    /// `cargo clean -p` the crate, after adding the file.
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
    fn apply_config(mut self, text: &str, path: &Utf8Path) -> Result<Self, ConfigError> {
        let file = toml::from_str::<FileConfig>(text)
            .map_err(|error| ConfigError::new(format!("invalid config file `{path}`: {error}")))?;
        if file.features.is_some() {
            // `deny_unknown_fields` alone would say "unknown field `features`", which does not
            // tell the author of a file that still carries the table where its keys belong.
            return Err(ConfigError::new(format!(
                "invalid config file `{path}`: the `[features]` table was removed; \
                 move its keys (`carve`, `batch_cap`, …) to the top level of the file"
            )));
        }
        if file.time.is_some() {
            // The removed boolean's two values each have one spelling under its replacement, so
            // the error states them rather than `deny_unknown_fields`' bare "unknown field".
            return Err(ConfigError::new(format!(
                "invalid config file `{path}`: the `time` key was replaced by `datetime`; write \
                 `datetime = \"time\"` for `time = true` or `datetime = \"string\"` for \
                 `time = false` (or `datetime = \"jiff\"` or `\"chrono\"`)"
            )));
        }
        if let Some(value) = file.uuid {
            self.uuid = value;
        }
        if let Some(value) = file.datetime {
            self.datetime = value;
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
        if let Some(value) = file.open_narrowing {
            self.open_narrowing = value;
        }
        if let Some(value) = file.open_unions {
            self.open_unions = value;
        }
        if let Some(value) = file.verbatim_docs {
            self.verbatim_docs = value;
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
        for (index, entry) in file.keep.iter().enumerate() {
            let method = entry.method.parse().map_err(|error: UnknownOmitToken| {
                ConfigError::new(format!(
                    "invalid config file `{path}`: keep rule #{}: {error}",
                    index + 1
                ))
            })?;
            self.omit
                .keep
                .push(KeepRule::operation(method, entry.path.clone()));
        }
        for name in file.bind_headers {
            self = self.bind_header(name);
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
#[non_exhaustive]
pub enum CargoIntegration {
    /// Integrate when running under a build script, degrading with `W012` (audit skipped) when
    /// that build script locates no consumer manifest; outside a build script, degrade with
    /// `W013` (no directives, audit skipped). The right choice for a `build.rs`.
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
/// [`generate`](crate::generate), whose doc shows the `build.rs` call.
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

    /// How this build interacts with Cargo, as [`cargo`](Self::cargo) set it.
    pub fn cargo_integration(&self) -> CargoIntegration {
        self.cargo
    }
}

/// A clear, user-facing config-file error. Rendered by the CLI as a usage error — never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// The message [`Display`](std::fmt::Display) writes.
    pub fn message(&self) -> &str {
        &self.message
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
    datetime: Option<DateTimeBackend>,
    /// Retained solely to give the removed `time` switch a targeted error naming `datetime`.
    time: Option<toml::Value>,
    carve: Option<bool>,
    batch_cap: Option<usize>,
    error_body_cap: Option<usize>,
    open_narrowing: Option<bool>,
    open_unions: Option<bool>,
    verbatim_docs: Option<bool>,
    /// Retained solely to give the removed 0.2 `[features]` table a targeted error.
    features: Option<toml::Value>,
    #[serde(default)]
    omit: Vec<OmitToml>,
    #[serde(default)]
    keep: Vec<KeepToml>,
    #[serde(default)]
    bind_headers: Vec<String>,
}

/// A `[[keep]]` entry: one operation, by `method` and `path`, both required.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KeepToml {
    method: String,
    path: String,
}

/// An `[[omit]]` entry. The rule kind is discriminated by which fields are present (TOML has no
/// native enums): `pointer` ⇒ Pointer, `component`+`name` ⇒ Component, `method`+`path` ⇒
/// Operation, `path` alone ⇒ Path. A key the selected kind does not use is an error.
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
        self.reject_foreign_keys()?;
        if let Some(pointer) = &self.pointer {
            return Ok(OmitRule::Pointer {
                file: self.file.clone().map(Into::into),
                pointer: pointer.clone().into(),
            });
        }
        if let Some(component) = &self.component {
            let name = self.name.as_ref().ok_or("`component` requires a `name`")?;
            return Ok(OmitRule::Component {
                kind: component
                    .parse()
                    .map_err(|error: UnknownOmitToken| error.to_string())?,
                name: name.clone().into(),
            });
        }
        if let Some(path) = &self.path {
            return match &self.method {
                Some(method) => Ok(OmitRule::Operation {
                    method: method
                        .parse()
                        .map_err(|error: UnknownOmitToken| error.to_string())?,
                    path: path.clone().into(),
                }),
                None => Ok(OmitRule::Path {
                    path: path.clone().into(),
                }),
            };
        }
        Err("must specify one of `path`, `component` (+`name`), or `pointer`".to_owned())
    }

    /// Reject a key the entry's kind does not use. The kind is picked by the same presence order
    /// [`to_rule`](Self::to_rule) dispatches on, so a well-spelled key in the wrong kind (`file`
    /// beside `path`) fails here, as a misspelled one fails `deny_unknown_fields`, instead of
    /// being dropped.
    fn reject_foreign_keys(&self) -> Result<(), String> {
        let (kind, takes): (&str, &[&str]) = if self.pointer.is_some() {
            ("pointer", &["pointer", "file"])
        } else if self.component.is_some() {
            ("component", &["component", "name"])
        } else if self.path.is_some() {
            if self.method.is_some() {
                ("operation", &["method", "path"])
            } else {
                ("path", &["path"])
            }
        } else {
            return Ok(());
        };
        let present = [
            ("path", self.path.is_some()),
            ("method", self.method.is_some()),
            ("component", self.component.is_some()),
            ("name", self.name.is_some()),
            ("pointer", self.pointer.is_some()),
            ("file", self.file.is_some()),
        ];
        match present
            .iter()
            .find(|(key, set)| *set && !takes.contains(key))
        {
            Some((key, _)) => Err(format!(
                "`{key}` does not apply to a {kind} rule, which takes only {}",
                takes
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{ComponentKind, OmitMethod};

    fn apply(text: &str) -> Result<Spec, ConfigError> {
        Spec::new("openapi.yaml").apply_config(text, Utf8Path::new("spargen.toml"))
    }

    /// Two config errors are equal exactly when their messages are, and a clone keeps the message
    /// its `Display` writes.
    #[test]
    fn config_errors_compare_and_clone_by_message() {
        let error = ConfigError::new("bad flag");
        assert_eq!(error.clone(), error);
        assert_eq!(error.clone().to_string(), "bad flag");
        assert_eq!(error.message(), "bad flag");
        assert_eq!(error, ConfigError::new("bad flag"));
        assert_ne!(error, ConfigError::new("another flag"));
    }

    #[test]
    fn defaults_are_pinned() {
        // These are a published contract, so a changed default fails a test rather than only a doc.
        let spec = Spec::new("openapi.yaml");
        assert!(spec.uuid);
        assert_eq!(spec.datetime, DateTimeBackend::Time);
        assert!(!spec.carve);
        assert_eq!(spec.batch_cap, 100);
        assert_eq!(spec.error_body_cap, 64 * 1024);
        assert!(!spec.open_narrowing);
        assert!(!spec.open_unions);
        assert!(!spec.verbatim_docs);
        assert!(spec.bind_headers.is_empty());
        assert!(spec.omit.is_empty());
        assert_eq!(Build::new(spec, "out.rs").cargo, CargoIntegration::Auto);
    }

    /// `bind_headers` in the file is appended to what the spec already binds, in order, and a
    /// name already bound in another letter case is not bound twice, from the file or a setter.
    #[test]
    fn config_file_bound_headers_are_appended_and_deduplicated_in_any_case() {
        let spec = Spec::new("openapi.yaml")
            .bind_header("X-Api-Version")
            .apply_config(
                "bind_headers = [\"X-Tenant\", \"x-api-version\"]\n",
                Utf8Path::new("spargen.toml"),
            )
            .unwrap()
            .bind_header("X-TENANT");
        assert_eq!(spec.bound_headers(), ["X-Api-Version", "X-Tenant"]);

        let error = apply("bind_headers = \"X-Tenant\"\n").unwrap_err();
        assert!(error.message().contains("bind_headers"), "{error}");
    }

    #[test]
    fn every_knob_round_trips_through_the_config_file() {
        let spec = apply(
            r#"
            uuid = false
            datetime = "jiff"
            carve = true
            batch_cap = 7
            error_body_cap = 11
            open_narrowing = true
            open_unions = true
            verbatim_docs = true
            "#,
        )
        .unwrap();
        assert!(!spec.uuid);
        assert_eq!(spec.datetime, DateTimeBackend::Jiff);
        assert!(spec.carve);
        assert_eq!(spec.batch_cap, 7);
        assert_eq!(spec.error_body_cap, 11);
        assert!(spec.open_narrowing);
        assert!(spec.open_unions);
        assert!(spec.verbatim_docs);
    }

    /// The removed `time` switch is refused, whatever its value, with an error naming the
    /// `datetime` spelling of each value; an unknown backend names the four accepted ones.
    #[test]
    fn the_removed_time_key_names_its_datetime_replacement() {
        for text in ["time = true\n", "time = false\n"] {
            let error = apply(text).unwrap_err();
            assert!(
                error
                    .message()
                    .contains("the `time` key was replaced by `datetime`")
                    && error
                        .message()
                        .contains("`datetime = \"string\"` for `time = false`"),
                "{error}"
            );
        }
        let error = apply("datetime = \"Jiff\"\n").unwrap_err();
        assert!(
            error.message().contains("unknown variant `Jiff`"),
            "{error}"
        );
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

    /// Each `[[keep]]` entry appends one kept operation after the setters' own, and leaves the omit
    /// rules alone.
    #[test]
    fn toml_keep_entries_append_kept_operations() {
        let spec = Spec::new("openapi.yaml")
            .keep_operation(OmitMethod::Get, "/first")
            .apply_config(
                "[[keep]]\nmethod = \"POST\"\npath = \"/second\"\n\n[[keep]]\nmethod = \"get\"\npath = \"/v1/*\"\n",
                Utf8Path::new("c.toml"),
            )
            .unwrap();
        assert_eq!(
            spec.omit.keep,
            vec![
                KeepRule::operation(OmitMethod::Get, "/first"),
                KeepRule::operation(OmitMethod::Post, "/second"),
                KeepRule::operation(OmitMethod::Get, "/v1/*"),
            ]
        );
        assert!(spec.omit.rules.is_empty());
        assert!(!spec.omit.is_empty());
    }

    /// A `[[keep]]` entry with a method the parser does not know names the entry and every
    /// accepted method; one missing a key, or carrying one it does not take, is rejected.
    #[test]
    fn a_malformed_keep_entry_is_rejected() {
        assert_eq!(
            apply("[[keep]]\nmethod = \"fetch\"\npath = \"/pets\"\n")
                .unwrap_err()
                .to_string(),
            "invalid config file `spargen.toml`: keep rule #1: unknown HTTP method: `fetch`; \
             expected one of get/put/post/delete/options/head/patch/trace/query"
        );
        for (entry, key) in [
            ("[[keep]]\nmethod = \"get\"\n", "missing field `path`"),
            ("[[keep]]\npath = \"/pets\"\n", "missing field `method`"),
            (
                "[[keep]]\nmethod = \"get\"\npath = \"/pets\"\nname = \"X\"\n",
                "unknown field `name`",
            ),
        ] {
            let error = apply(entry).unwrap_err().to_string();
            assert!(
                error.starts_with("invalid config file `spargen.toml`: ") && error.contains(key),
                "{entry}: {error}"
            );
        }
    }

    #[test]
    fn the_removed_features_table_gets_a_targeted_error() {
        let error = apply("[features]\ncarve = true\n").unwrap_err().to_string();
        assert_eq!(
            error,
            "invalid config file `spargen.toml`: the `[features]` table was removed; move its \
             keys (`carve`, `batch_cap`, …) to the top level of the file",
            "expected migration guidance"
        );
    }

    /// A malformed `[[omit]]` entry is rejected with an error naming the rule and what it lacks:
    /// the `name` a component needs, the key that would select a kind, or the kind a stray key
    /// does not belong to.
    #[test]
    fn each_malformed_omit_entry_names_what_it_lacks() {
        for (entry, expected) in [
            (
                "[[omit]]\ncomponent = \"schema\"\n",
                "omit rule #1: `component` requires a `name`",
            ),
            (
                "[[omit]]\n",
                "omit rule #1: must specify one of `path`, `component` (+`name`), or `pointer`",
            ),
            (
                "[[omit]]\npointer = \"/x\"\npath = \"/y\"\n",
                "omit rule #1: `path` does not apply to a pointer rule, which takes only \
                 `pointer` and `file`",
            ),
        ] {
            let error = apply(entry).unwrap_err().to_string();
            assert_eq!(
                error,
                format!("invalid config file `spargen.toml`: {expected}"),
                "{entry}"
            );
        }
    }

    #[test]
    fn a_key_the_omit_kind_does_not_use_names_the_key_and_the_kind() {
        let error = apply("[[omit]]\npath = \"/pets\"\nfile = \"extra.yaml\"\n")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "invalid config file `spargen.toml`: omit rule #1: `file` does not apply to a path \
             rule, which takes only `path`"
        );
        let error = apply("[[omit]]\npointer = \"/x\"\nname = \"X\"\n")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "invalid config file `spargen.toml`: omit rule #1: `name` does not apply to a \
             pointer rule, which takes only `pointer` and `file`"
        );
    }

    #[test]
    fn an_unknown_omit_token_names_every_value_the_parser_accepts() {
        let component = apply("[[omit]]\ncomponent = \"path_itemz\"\nname = \"Foo\"\n")
            .unwrap_err()
            .to_string();
        assert_eq!(
            component,
            "invalid config file `spargen.toml`: omit rule #1: unknown component kind: \
             `path_itemz`; expected one of schemas/responses/parameters/request_bodies/headers/\
             security_schemes/path_items/media_types"
        );
        let method = apply("[[omit]]\nmethod = \"qeury\"\npath = \"/pets\"\n")
            .unwrap_err()
            .to_string();
        assert_eq!(
            method,
            "invalid config file `spargen.toml`: omit rule #1: unknown HTTP method: `qeury`; \
             expected one of get/put/post/delete/options/head/patch/trace/query"
        );
    }

    /// An unknown key is rejected with an error that names the file and the key. The rest of the
    /// message is the TOML parser's, so only those two are pinned.
    #[test]
    fn an_unknown_key_is_named_in_the_error() {
        let error = apply("frobnicate = true\n").unwrap_err().to_string();
        assert!(
            error.starts_with("invalid config file `spargen.toml`: ")
                && error.contains("unknown field `frobnicate`"),
            "{error}"
        );
    }
}
