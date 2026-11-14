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

/// Implement the two crate-private conversions between a two-state switch enum and the `bool` a
/// `spargen.toml` key and the pipeline's options carry: `on` is the variant `true` selects.
macro_rules! switch {
    ($name:ident, off = $off:ident, on = $on:ident) => {
        impl $name {
            /// The variant a `spargen.toml` boolean selects: `true` is the non-default one.
            pub(crate) fn from_on(on: bool) -> Self {
                if on {
                    Self::$on
                } else {
                    Self::$off
                }
            }

            /// Whether this is the variant a `spargen.toml` `true` selects.
            pub(crate) fn is_on(self) -> bool {
                self == Self::$on
            }
        }
    };
}

/// What `format: uuid` maps to, set with [`Spec::with_uuid`] and read with [`Spec::uuid`]; the
/// `uuid` key of `spargen.toml`, where `true` is [`Uuid`](Self::Uuid).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UuidMapping {
    /// `uuid::Uuid`, which the dependency audit then requires.
    #[default]
    Uuid,
    /// `String`: a deliberate, documented loss of typing that also drops `uuid` from the
    /// dependencies generated output requires.
    String,
}
switch!(UuidMapping, off = String, on = Uuid);

/// Whether a rejection fails generation or is carved away, set with [`Spec::with_carve`] and read
/// with [`Spec::carve`]; the `carve` key of `spargen.toml`, where `true` is [`Auto`](Self::Auto).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Carve {
    /// A rejection fails generation.
    #[default]
    Off,
    /// Auto-carve: iteratively omit the minimal enclosing omittable construct for each rejection
    /// and generate the rest, reporting every carved construct as `W009`.
    Auto,
}
switch!(Carve, off = Off, on = Auto);

/// How an `enum`/`const` narrowing a plain `string`, `integer` or `boolean` in a response body's
/// own schema lowers, set with [`Spec::with_open_narrowing`] and read with
/// [`Spec::open_narrowing`]; the `open_narrowing` key of `spargen.toml`, where `true` is
/// [`Open`](Self::Open).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Narrowing {
    /// A closed set: a value the description does not list is a decode failure.
    #[default]
    Exact,
    /// An open set: a string enum's one more variant holds any other string, and an integer or
    /// boolean set's checked newtype decodes any other value of its width.
    Open,
}
switch!(Narrowing, off = Exact, on = Open);

/// Whether a discriminated union takes a tag value it does not list, set with
/// [`Spec::with_open_unions`] and read with [`Spec::open_unions`]; the `open_unions` key of
/// `spargen.toml`, where `true` is [`Open`](Self::Open).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnionTags {
    /// An unlisted tag value is a decode failure.
    #[default]
    Closed,
    /// An unlisted tag value decodes into the union's `Unknown { tag, value }` variant.
    Open,
}
switch!(UnionTags, off = Closed, on = Open);

/// How the spec's prose reaches rustdoc, set with [`Spec::with_verbatim_docs`] and read with
/// [`Spec::verbatim_docs`]; the `verbatim_docs` key of `spargen.toml`, where `true` is
/// [`Verbatim`](Self::Verbatim).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CarriedDocs {
    /// Rewritten so rustdoc has nothing to report from wherever the module is `include!`d.
    #[default]
    Rewritten,
    /// Carried exactly as written.
    Verbatim,
}
switch!(CarriedDocs, off = Rewritten, on = Verbatim);

/// Whether the generated module carries a `testing` module, set with [`Spec::with_testing`] and
/// read with [`Spec::testing`]; the `testing` key of `spargen.toml`, where `true` is
/// [`Emitted`](Self::Emitted).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TestingModule {
    /// No `testing` module.
    #[default]
    Omitted,
    /// A `testing` module beside the client, holding the typed `testing::MockBackend`.
    Emitted,
}
switch!(TestingModule, off = Omitted, on = Emitted);

/// What to generate: the spec to read plus every knob that can change the generated API.
///
/// Two `Spec`s that compare equal produce byte-identical output for a given spargen version *and
/// identical spec-file contents*. Equality covers the knobs below, including the spec's path and
/// the paths of the config files applied to it; it cannot see the bytes at those paths, which the
/// build cache fingerprints separately.
///
/// Each knob is read by a getter named for it ([`uuid`](Self::uuid),
/// [`batch_cap`](Self::batch_cap), …) and set by a `with_` setter of the same name
/// ([`with_uuid`](Self::with_uuid), [`with_batch_cap`](Self::with_batch_cap), …), which replaces
/// the value; [`omit`](Self::omit), [`omit_rule`](Self::omit_rule),
/// [`keep_operation`](Self::keep_operation) and [`bind_header`](Self::bind_header) append to a
/// list instead, which its own getter reads back. Every setter returns the spec by value, which
/// `#[must_use]` holds a caller to using.
///
/// ```no_run
/// use spargen::UuidMapping;
///
/// let spec = spargen::Spec::new("api/openapi.yaml").with_uuid(UuidMapping::String);
/// let report = spargen::check(&spec);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a `Spec` does nothing until it is passed to an entry point such as `check` or built into a `Build`"]
pub struct Spec {
    pub(crate) path: Utf8PathBuf,
    pub(crate) uuid: UuidMapping,
    pub(crate) datetime: DateTimeBackend,
    pub(crate) omit: Omit,
    pub(crate) error_body_cap: usize,
    pub(crate) batch_cap: usize,
    pub(crate) carve: Carve,
    pub(crate) open_narrowing: Narrowing,
    pub(crate) open_unions: UnionTags,
    pub(crate) verbatim_docs: CarriedDocs,
    pub(crate) testing: TestingModule,
    /// The header names [`Spec::bind_header`] bound, in the order bound, no two equal in any
    /// letter case.
    pub(crate) bind_headers: Vec<String>,
    /// Every config file applied through [`Spec::config_file`] (and so through
    /// [`Spec::discover_config_file`]), in the order applied. Generation reads them, so `build.rs`
    /// names each as a `cargo:rerun-if-changed` path and `generate_api!` tracks each.
    pub(crate) config_files: Vec<Utf8PathBuf>,
}

impl Spec {
    /// A spec with defaults: `format: uuid` mapped to `uuid::Uuid` ([`UuidMapping::Uuid`]), dates
    /// mapped through the `time` crate ([`DateTimeBackend::Time`]), a 64 KiB error-body cap, a
    /// 100-diagnostic batch, no omissions, no carving ([`Carve::Off`]), exact narrowing
    /// ([`Narrowing::Exact`]), closed unions ([`UnionTags::Closed`]), carried docs rewritten for
    /// rustdoc ([`CarriedDocs::Rewritten`]), no `testing` module ([`TestingModule::Omitted`]), and
    /// no bound headers.
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            path: path.into(),
            uuid: UuidMapping::Uuid,
            datetime: DateTimeBackend::Time,
            omit: Omit::default(),
            error_body_cap: DEFAULT_ERROR_BODY_CAP,
            batch_cap: DEFAULT_BATCH_CAP,
            carve: Carve::Off,
            open_narrowing: Narrowing::Exact,
            open_unions: UnionTags::Closed,
            verbatim_docs: CarriedDocs::Rewritten,
            testing: TestingModule::Omitted,
            bind_headers: Vec::new(),
            config_files: Vec::new(),
        }
    }

    /// The path to the root OpenAPI document.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// What `format: uuid` maps to, as [`with_uuid`](Self::with_uuid) set it.
    pub fn uuid(&self) -> UuidMapping {
        self.uuid
    }

    /// The crate `format: date-time`/`date` map to, as [`with_datetime`](Self::with_datetime) set
    /// it.
    pub fn datetime(&self) -> DateTimeBackend {
        self.datetime
    }

    /// The compatibility omissions and keep rules, in chained order: [`omit`](Self::omit),
    /// [`omit_rule`](Self::omit_rule), [`keep_operation`](Self::keep_operation) and each applied
    /// config file append to them, and [`clear_omissions`](Self::clear_omissions) empties them.
    pub fn omissions(&self) -> &Omit {
        &self.omit
    }

    /// The cap on retained error-body bytes, as [`with_error_body_cap`](Self::with_error_body_cap)
    /// set it.
    pub fn error_body_cap(&self) -> usize {
        self.error_body_cap
    }

    /// The cap on diagnostics collected before batching stops, as
    /// [`with_batch_cap`](Self::with_batch_cap) set it.
    pub fn batch_cap(&self) -> usize {
        self.batch_cap
    }

    /// Whether a rejection is carved away, as [`with_carve`](Self::with_carve) set it.
    pub fn carve(&self) -> Carve {
        self.carve
    }

    /// How a narrowing string `enum`/`const` lowers, as
    /// [`with_open_narrowing`](Self::with_open_narrowing) set it.
    pub fn open_narrowing(&self) -> Narrowing {
        self.open_narrowing
    }

    /// Whether a discriminated union takes a tag value it does not list, as
    /// [`with_open_unions`](Self::with_open_unions) set it.
    pub fn open_unions(&self) -> UnionTags {
        self.open_unions
    }

    /// How the spec's prose reaches rustdoc, as [`with_verbatim_docs`](Self::with_verbatim_docs)
    /// set it.
    pub fn verbatim_docs(&self) -> CarriedDocs {
        self.verbatim_docs
    }

    /// Whether the generated module carries its `testing` module, as
    /// [`with_testing`](Self::with_testing) set it.
    pub fn testing(&self) -> TestingModule {
        self.testing
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

    /// Choose what `format: uuid` maps to (default [`UuidMapping::Uuid`], `uuid::Uuid`).
    /// [`UuidMapping::String`] falls back to `String` — a deliberate, documented loss of typing
    /// that also drops `uuid` from the dependencies generated output requires.
    pub fn with_uuid(mut self, mapping: UuidMapping) -> Self {
        self.uuid = mapping;
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
    /// let spec = spargen::Spec::new("api/openapi.yaml").with_datetime(DateTimeBackend::Jiff);
    /// ```
    pub fn with_datetime(mut self, backend: DateTimeBackend) -> Self {
        self.datetime = backend;
        self
    }

    /// Add every omit and keep rule of `omit` to the compatibility omissions applied before
    /// OpenAPI validation and lowering, after the rules already set, as
    /// [`omit_rule`](Self::omit_rule), [`keep_operation`](Self::keep_operation) and each applied
    /// config file add theirs. [`clear_omissions`](Self::clear_omissions) empties the set.
    ///
    /// ```no_run
    /// let spec = spargen::Spec::new("api/openapi.yaml")
    ///     .discover_config_file()?
    ///     .omit(spargen::omit! { paths { "/internal/**"; } });
    /// # Ok::<(), spargen::ConfigError>(())
    /// ```
    pub fn omit(mut self, omit: Omit) -> Self {
        self.omit.rules.extend(omit.rules);
        self.omit.keep.extend(omit.keep);
        self
    }

    /// Remove every omit and keep rule set so far, by a setter or an applied config file.
    pub fn clear_omissions(mut self) -> Self {
        self.omit = Omit::default();
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
    /// [`clear_omissions`](Self::clear_omissions) removes them too.
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
    pub fn with_error_body_cap(mut self, bytes: usize) -> Self {
        self.error_body_cap = bytes;
        self
    }

    /// Cap the diagnostics collected before batching stops (default 100).
    ///
    /// Past the cap an error displaces the most recent retained warning, so a rejected run
    /// lists an error whenever the cap is at least one.
    pub fn with_batch_cap(mut self, count: usize) -> Self {
        self.batch_cap = count;
        self
    }

    /// Choose whether a rejection fails generation (default [`Carve::Off`]). With
    /// [`Carve::Auto`], instead of failing on rejections, iteratively omit the minimal enclosing
    /// omittable construct for each rejection and generate the rest. Every carved construct is
    /// reported via `W009`; residual, un-carvable rejections are reported honestly.
    pub fn with_carve(mut self, carve: Carve) -> Self {
        self.carve = carve;
        self
    }

    /// With [`Narrowing::Open`], lower a string `enum`/`const` that narrows a plain `string` as an
    /// **open** set (default [`Narrowing::Exact`]), inside a response body's own schema.
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
    pub fn with_open_narrowing(mut self, narrowing: Narrowing) -> Self {
        self.open_narrowing = narrowing;
        self
    }

    /// With [`UnionTags::Open`], give every discriminated union one more variant,
    /// `Unknown { tag, value }`, holding a payload whose discriminator value the union does not
    /// list (default [`UnionTags::Closed`]).
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
    pub fn with_open_unions(mut self, tags: UnionTags) -> Self {
        self.open_unions = tags;
        self
    }

    /// With [`CarriedDocs::Verbatim`], carry the spec's prose — `summary`, `description` and
    /// `title` — into rustdoc exactly as written (default [`CarriedDocs::Rewritten`]).
    ///
    /// By default that prose is rewritten so rustdoc has nothing to report from wherever the
    /// module is `include!`d, without changing what it says. A server framework that publishes its
    /// handlers' rustdoc as `description` carries intra-doc links naming items of the server's
    /// crate (``[`crate::serve`]``, `[Store]`), which no path from the generated module reaches:
    /// each such link becomes its text, so ``[`crate::serve`]`` renders as `` `crate::serve` ``. A
    /// bare `http(s)://` URL is wrapped in `<…>`, and an HTML tag nothing closes (`Vec<T>`'s
    /// `<T>`) has its `<` escaped. Links to URLs, code spans and code blocks are kept as written,
    /// except that a code block rustdoc would run as a doctest of the consuming crate (an untagged
    /// fence, or an indented block) is fenced and tagged `text`, and a fence the prose leaves open
    /// is closed at its end.
    ///
    /// With this on, the prose is left as the spec wrote it (a fence it leaves open still closes
    /// before the text spargen adds after it, such as an operation's response list), and the
    /// `include!` site answers for `rustdoc::broken_intra_doc_links`, `rustdoc::bare_urls` and
    /// `rustdoc::invalid_html_tags`, and its `cargo test --doc` for the prose's untagged code
    /// blocks.
    pub fn with_verbatim_docs(mut self, docs: CarriedDocs) -> Self {
        self.verbatim_docs = docs;
        self
    }

    /// With [`TestingModule::Emitted`], emit a `testing` module beside the client (default
    /// [`TestingModule::Omitted`]): a typed test double, `testing::MockBackend`, to build a client
    /// on with `Client::with_backend`.
    ///
    /// The mock routes each request by the `ops::Operation` its operation method attaches, never
    /// by path, to the handler registered with that operation's `on_{method}`; the handler reads
    /// the recorded `testing::Call` and answers with the operation's own `testing::{method}::Reply`,
    /// whose constructors encode a typed body with the codec the client decodes that status with.
    /// Every request is recorded, read back with `calls(ops::…)`. A request no handler is
    /// registered for fails with a `testing::Unrouted` error naming its operation, method and
    /// path, rather than with any response.
    ///
    /// The module is compiled wherever the generated module is, so a build that never names it
    /// still compiles it.
    pub fn with_testing(mut self, module: TestingModule) -> Self {
        self.testing = module;
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
    /// its bytes, and `generate_api!` tracks it. Applying a path already applied, spelled the same,
    /// sets the file's keys again but appends neither its rules nor a second record of the path.
    pub fn config_file(self, path: impl AsRef<Utf8Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path.as_std_path()).map_err(|source| ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        let applied = self.config_files.iter().any(|applied| applied == path);
        let mut spec = self.apply_config(&text, path, !applied)?;
        if !applied {
            spec.config_files.push(path.to_path_buf());
        }
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

    /// Apply already-read config-file text, appending its `[[omit]]` and `[[keep]]` rules only
    /// when `append_rules` says the file was not applied before. Split out so the parser is
    /// testable without I/O.
    fn apply_config(
        mut self,
        text: &str,
        path: &Utf8Path,
        append_rules: bool,
    ) -> Result<Self, ConfigError> {
        let parse = |message: String| ConfigError::Parse {
            path: path.to_path_buf(),
            message,
        };
        let file = toml::from_str::<FileConfig>(text).map_err(|error| parse(error.to_string()))?;
        if file.features.is_some() {
            // `deny_unknown_fields` alone would say "unknown field `features`", which does not
            // tell the author of a file that still carries the table where its keys belong.
            return Err(parse(
                "the `[features]` table was removed; move its keys (`carve`, `batch_cap`, …) to \
                 the top level of the file"
                    .to_owned(),
            ));
        }
        if file.time.is_some() {
            // The removed boolean's two values each have one spelling under its replacement, so
            // the error states them rather than `deny_unknown_fields`' bare "unknown field".
            return Err(parse(
                "the `time` key was replaced by `datetime`; write `datetime = \"time\"` for \
                 `time = true` or `datetime = \"string\"` for `time = false` (or \
                 `datetime = \"jiff\"` or `\"chrono\"`)"
                    .to_owned(),
            ));
        }
        if let Some(value) = file.uuid {
            self.uuid = UuidMapping::from_on(value);
        }
        if let Some(value) = file.datetime {
            self.datetime = value;
        }
        if let Some(value) = file.carve {
            self.carve = Carve::from_on(value);
        }
        if let Some(value) = file.batch_cap {
            self.batch_cap = value;
        }
        if let Some(value) = file.error_body_cap {
            self.error_body_cap = value;
        }
        if let Some(value) = file.open_narrowing {
            self.open_narrowing = Narrowing::from_on(value);
        }
        if let Some(value) = file.open_unions {
            self.open_unions = UnionTags::from_on(value);
        }
        if let Some(value) = file.verbatim_docs {
            self.verbatim_docs = CarriedDocs::from_on(value);
        }
        if let Some(value) = file.testing {
            self.testing = TestingModule::from_on(value);
        }
        for (index, entry) in file.omit.iter().enumerate() {
            let rule = entry
                .to_rule()
                .map_err(|message| ConfigError::InvalidOmitRule {
                    path: path.to_path_buf(),
                    index,
                    message,
                })?;
            if append_rules {
                self.omit.rules.push(rule);
            }
        }
        for (index, entry) in file.keep.iter().enumerate() {
            let method = entry.method.parse().map_err(|error: UnknownOmitToken| {
                ConfigError::InvalidKeepRule {
                    path: path.to_path_buf(),
                    index,
                    message: error.to_string(),
                }
            })?;
            if append_rules {
                self.omit
                    .keep
                    .push(KeepRule::operation(method, entry.path.clone()));
            }
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
///
/// Its knobs follow [`Spec`]'s scheme: a getter named for the knob ([`cargo`](Self::cargo)) and a
/// `with_` setter ([`with_cargo`](Self::with_cargo)).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a `Build` does nothing until it is passed to `generate`"]
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

    /// Choose how this build interacts with Cargo (default [`CargoIntegration::Auto`]).
    pub fn with_cargo(mut self, integration: CargoIntegration) -> Self {
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

    /// How this build interacts with Cargo, as [`with_cargo`](Self::with_cargo) set it.
    pub fn cargo(&self) -> CargoIntegration {
        self.cargo
    }
}

/// A clear, user-facing configuration error, by kind. Rendered by the CLI as a usage error — never
/// a panic.
///
/// [`Display`](std::fmt::Display) writes one line naming the file and what is wrong with it; the
/// I/O error behind [`Read`](Self::Read) is its [`source`](std::error::Error::source), which
/// `Display` leaves out, so a reporter that walks the chain writes it once.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// The config file could not be read.
    #[non_exhaustive]
    Read {
        /// The config file, as given.
        path: Utf8PathBuf,
        /// Why reading it failed; also [`source`](std::error::Error::source).
        source: std::io::Error,
    },
    /// The file's text is not a `spargen.toml`: malformed TOML, an unknown or mistyped key, or a
    /// removed key, whose message names its replacement.
    #[non_exhaustive]
    Parse {
        /// The config file, as given.
        path: Utf8PathBuf,
        /// What is wrong with the text.
        message: String,
    },
    /// An `[[omit]]` entry that is no rule: a missing or foreign key, or an unknown token.
    #[non_exhaustive]
    InvalidOmitRule {
        /// The config file, as given.
        path: Utf8PathBuf,
        /// The entry's 0-based position among the file's `[[omit]]` entries; `Display` writes it
        /// 1-based, as `omit rule #1` for the first.
        index: usize,
        /// What is wrong with the entry.
        message: String,
    },
    /// A `[[keep]]` entry with a method the parser does not know.
    #[non_exhaustive]
    InvalidKeepRule {
        /// The config file, as given.
        path: Utf8PathBuf,
        /// The entry's 0-based position among the file's `[[keep]]` entries; `Display` writes it
        /// 1-based, as `keep rule #1` for the first.
        index: usize,
        /// What is wrong with the entry.
        message: String,
    },
    /// A front end's own flag-syntax error, made with [`flag`](Self::flag).
    #[non_exhaustive]
    Flag {
        /// The message `Display` writes.
        message: String,
    },
}

impl ConfigError {
    /// A [`Flag`](Self::Flag) error with a caller-supplied message. Public so a front end (the
    /// CLI, a build script wrapper) can report *its* own flag-syntax errors through the same type
    /// the library returns, rather than inventing a parallel one.
    pub fn flag(message: impl Into<String>) -> Self {
        Self::Flag {
            message: message.into(),
        }
    }

    /// The config file the error is about, or `None` for a [`Flag`](Self::Flag) error.
    pub fn path(&self) -> Option<&Utf8Path> {
        match self {
            Self::Read { path, .. }
            | Self::Parse { path, .. }
            | Self::InvalidOmitRule { path, .. }
            | Self::InvalidKeepRule { path, .. } => Some(path),
            Self::Flag { .. } => None,
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, .. } => write!(formatter, "cannot read config file `{path}`"),
            Self::Parse { path, message } => {
                write!(formatter, "invalid config file `{path}`: {message}")
            }
            Self::InvalidOmitRule {
                path,
                index,
                message,
            } => write!(
                formatter,
                "invalid config file `{path}`: omit rule #{}: {message}",
                index + 1
            ),
            Self::InvalidKeepRule {
                path,
                index,
                message,
            } => write!(
                formatter,
                "invalid config file `{path}`: keep rule #{}: {message}",
                index + 1
            ),
            Self::Flag { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}

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
    testing: Option<bool>,
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
        Spec::new("openapi.yaml").apply_config(text, Utf8Path::new("spargen.toml"), true)
    }

    /// Each kind of config error is told apart by its variant, names its file through `path`, and
    /// only `Read` has a `source`, which its `Display` leaves out.
    #[test]
    fn each_config_error_kind_is_typed_and_only_a_read_failure_has_a_source() {
        use std::error::Error as _;

        let file = Utf8Path::new("spargen.toml");
        let flag = ConfigError::flag("bad flag");
        assert!(matches!(&flag, ConfigError::Flag { message } if message == "bad flag"));
        assert_eq!(flag.to_string(), "bad flag");
        assert_eq!(flag.path(), None);
        assert!(flag.source().is_none());

        let missing = Spec::new("openapi.yaml")
            .config_file("/nonexistent/spargen-1328/spargen.toml")
            .unwrap_err();
        let ConfigError::Read { path, source } = &missing else {
            panic!("a missing file is a read error: {missing:?}");
        };
        assert_eq!(path, "/nonexistent/spargen-1328/spargen.toml");
        assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(
            missing.to_string(),
            "cannot read config file `/nonexistent/spargen-1328/spargen.toml`"
        );
        assert_eq!(
            missing.source().map(ToString::to_string),
            Some(source.to_string())
        );

        let parse = apply("carve = 1\n").unwrap_err();
        assert!(matches!(parse, ConfigError::Parse { .. }), "{parse:?}");
        let omit = apply("[[omit]]\n[[omit]]\npath = \"/x\"\nfile = \"y\"\n").unwrap_err();
        assert!(
            matches!(omit, ConfigError::InvalidOmitRule { index: 0, .. }),
            "{omit:?}"
        );
        let keep = apply(
            "[[keep]]\nmethod = \"get\"\npath = \"/a\"\n[[keep]]\nmethod = \"x\"\npath = \"/b\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(keep, ConfigError::InvalidKeepRule { index: 1, .. }),
            "{keep:?}"
        );
        for error in [&parse, &omit, &keep] {
            assert_eq!(error.path(), Some(file));
            assert!(error.source().is_none(), "{error:?}");
        }
        assert!(keep.to_string().contains("keep rule #2: "), "{keep}");
    }

    #[test]
    fn defaults_are_pinned() {
        // These are a published contract, so a changed default fails a test rather than only a doc.
        let spec = Spec::new("openapi.yaml");
        assert_eq!(spec.uuid, UuidMapping::Uuid);
        assert_eq!(spec.datetime, DateTimeBackend::Time);
        assert_eq!(spec.carve, Carve::Off);
        assert_eq!(spec.batch_cap, 100);
        assert_eq!(spec.error_body_cap, 64 * 1024);
        assert_eq!(spec.open_narrowing, Narrowing::Exact);
        assert_eq!(spec.open_unions, UnionTags::Closed);
        assert_eq!(spec.verbatim_docs, CarriedDocs::Rewritten);
        assert_eq!(spec.testing, TestingModule::Omitted);
        assert!(spec.bind_headers.is_empty());
        assert!(spec.omit.is_empty());
        assert_eq!(Build::new(spec, "out.rs").cargo, CargoIntegration::Auto);
    }

    /// A config file reached by a second spelling of its path (here `discover_config_file` from a
    /// spec spelled `<dir>/./openapi.yaml`, and an explicit `..` detour) is the file already
    /// applied: its rules are not appended again and the path is recorded once, as first spelled.
    #[test]
    fn a_config_file_reached_by_another_spelling_is_applied_once() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let file = dir.join("spargen.toml");
        std::fs::write(
            &file,
            "[[omit]]\npath = \"/x\"\n[[keep]]\nmethod = \"get\"\npath = \"/k\"\n",
        )
        .unwrap();

        let spec = Spec::new(dir.join(".").join("openapi.yaml"))
            .config_file(&file)
            .unwrap()
            .discover_config_file()
            .unwrap()
            .config_file(dir.join("sub").join("..").join("spargen.toml"))
            .unwrap();
        assert_eq!(spec.omit.rules, [OmitRule::path("/x")]);
        assert_eq!(spec.omit.keep, [KeepRule::operation(OmitMethod::Get, "/k")]);
        assert_eq!(spec.config_files(), [file]);
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
                true,
            )
            .unwrap()
            .bind_header("X-TENANT");
        assert_eq!(spec.bound_headers(), ["X-Api-Version", "X-Tenant"]);

        let error = apply("bind_headers = \"X-Tenant\"\n").unwrap_err();
        assert!(error.to_string().contains("bind_headers"), "{error}");
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
            testing = true
            "#,
        )
        .unwrap();
        assert_eq!(spec.uuid, UuidMapping::String);
        assert_eq!(spec.datetime, DateTimeBackend::Jiff);
        assert_eq!(spec.carve, Carve::Auto);
        assert_eq!(spec.batch_cap, 7);
        assert_eq!(spec.error_body_cap, 11);
        assert_eq!(spec.open_narrowing, Narrowing::Open);
        assert_eq!(spec.open_unions, UnionTags::Open);
        assert_eq!(spec.verbatim_docs, CarriedDocs::Verbatim);
        assert_eq!(spec.testing, TestingModule::Emitted);
    }

    /// The removed `time` switch is refused, whatever its value, with an error naming the
    /// `datetime` spelling of each value; an unknown backend names the four accepted ones.
    #[test]
    fn the_removed_time_key_names_its_datetime_replacement() {
        for text in ["time = true\n", "time = false\n"] {
            let error = apply(text).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("the `time` key was replaced by `datetime`")
                    && error
                        .to_string()
                        .contains("`datetime = \"string\"` for `time = false`"),
                "{error}"
            );
        }
        let error = apply("datetime = \"Jiff\"\n").unwrap_err();
        assert!(
            error.to_string().contains("unknown variant `Jiff`"),
            "{error}"
        );
    }

    #[test]
    fn setters_after_the_config_file_win() {
        let spec = apply("carve = true\nbatch_cap = 7\n")
            .unwrap()
            .with_carve(Carve::Off);
        assert_eq!(spec.carve, Carve::Off);
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
            .apply_config(
                "[[omit]]\npath = \"/second\"\n",
                Utf8Path::new("c.toml"),
                true,
            )
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
                true,
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
