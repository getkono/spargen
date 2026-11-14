use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// The `spargen` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "spargen",
    version,
    about = "A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x."
)]
pub(crate) struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// A `spargen` subcommand.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Audit a spec's feature support without generating code.
    Check(CheckArgs),
    /// Print the `[dependencies]` block generated output from this spec requires.
    Deps(DepsArgs),
    /// Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock` (the only networked step).
    ///
    /// It accepts every spec flag, as `check` does, but of them reads only `--config` and
    /// `--batch-cap`: every remote `$ref` the spec reaches is fetched whatever the rest say, and
    /// each other spec flag given is named in a warning on stderr.
    Lock(LockArgs),
    /// Show extended documentation for a diagnostic code.
    Explain(ExplainArgs),
    /// Report the semver impact of regenerating the client from a newer spec.
    Diff(DiffArgs),
}

/// The knobs that decide *what* is generated, shared by every subcommand that reads a spec.
///
/// One flatten-group rather than a per-subcommand copy: the flags are exactly the setters on
/// [`spargen::Spec`], so `check`, `deps`, `lock`, and `diff` cannot drift from each other or from
/// the `build.rs` and `spargen.toml` surfaces.
#[derive(Debug, Args)]
pub(crate) struct SpecArgs {
    /// Path to a `spargen.toml` config file. Defaults to `spargen.toml` beside the spec, if present.
    #[arg(long)]
    pub(crate) config: Option<Utf8PathBuf>,
    /// Auto-carve: omit the smallest omittable construct enclosing each rejected one (each
    /// reported via W009) so the rest audits clean. Un-carvable rejections are still reported.
    #[arg(long, conflicts_with = "no_carve")]
    pub(crate) carve: bool,
    /// Turn auto-carve off, overriding `carve = true` in `spargen.toml`.
    #[arg(long)]
    pub(crate) no_carve: bool,
    /// Map `format: uuid` to `uuid::Uuid`, overriding `uuid = false` in `spargen.toml`.
    #[arg(long, conflicts_with = "no_uuid")]
    pub(crate) uuid: bool,
    /// Map `format: uuid` to `String` instead of `uuid::Uuid`, overriding `uuid = true` in
    /// `spargen.toml`.
    #[arg(long)]
    pub(crate) no_uuid: bool,
    /// The crate `format: date-time`/`date` map to: `time` (the default), `jiff`, `chrono`, or
    /// `string` for no date-time crate, overriding `datetime` in `spargen.toml`.
    #[arg(long, value_name = "BACKEND")]
    pub(crate) datetime: Option<spargen::DateTimeBackend>,
    /// Max bytes of a response body retained on generated error variants (default 65536).
    #[arg(long, value_name = "BYTES")]
    pub(crate) error_body_cap: Option<usize>,
    /// Max diagnostics collected before batching stops (default 100).
    #[arg(long, value_name = "COUNT")]
    pub(crate) batch_cap: Option<usize>,
    /// Lower a response body's own `enum`/`const` narrowings of a plain `string`, `integer` or
    /// `boolean` as open sets, so a value the description does not list still decodes.
    #[arg(long, conflicts_with = "no_open_narrowing")]
    pub(crate) open_narrowing: bool,
    /// Keep response-body narrowings closed, overriding `open_narrowing = true` in
    /// `spargen.toml`.
    #[arg(long)]
    pub(crate) no_open_narrowing: bool,
    /// Give every discriminated union without a `defaultMapping` an `Unknown { tag, value }`
    /// variant, so a discriminator value the description does not list still decodes.
    #[arg(long, conflicts_with = "no_open_unions")]
    pub(crate) open_unions: bool,
    /// Keep discriminated unions closed, overriding `open_unions = true` in `spargen.toml`.
    #[arg(long)]
    pub(crate) no_open_unions: bool,
    /// Carry the spec's prose into rustdoc as written, overriding `verbatim_docs = false` in
    /// `spargen.toml`.
    #[arg(long, conflicts_with = "no_verbatim_docs")]
    pub(crate) verbatim_docs: bool,
    /// Rewrite the spec's prose for rustdoc (intra-doc links to text, bare URLs and stray tags
    /// escaped), overriding `verbatim_docs = true` in `spargen.toml`.
    #[arg(long)]
    pub(crate) no_verbatim_docs: bool,
    /// Emit the `testing` module and its typed `MockBackend`, overriding `testing = false` in
    /// `spargen.toml`.
    #[arg(long, conflicts_with = "no_testing")]
    pub(crate) testing: bool,
    /// Emit no `testing` module, overriding `testing = true` in `spargen.toml`.
    #[arg(long)]
    pub(crate) no_testing: bool,
    /// Omit a path item and every operation under it (repeatable), e.g. `--omit-path /pets/{id}`.
    #[arg(long = "omit-path", value_name = "PATH")]
    pub(crate) omit_path: Vec<String>,
    /// Omit one operation (repeatable), e.g. `--omit-operation "get /pets"`.
    #[arg(long = "omit-operation", value_name = "METHOD /path")]
    pub(crate) omit_operation: Vec<String>,
    /// Omit a named component (repeatable), e.g. `--omit-component "schema:LegacyPet"`.
    #[arg(long = "omit-component", value_name = "kind:name")]
    pub(crate) omit_component: Vec<String>,
    /// Omit an RFC 6901 pointer (repeatable), e.g. `--omit-pointer "[file#]/pointer"`.
    #[arg(long = "omit-pointer", value_name = "[file#]/pointer")]
    pub(crate) omit_pointer: Vec<String>,
    /// Keep one operation and omit every other (repeatable), e.g. `--keep-operation "get /pets"`.
    /// Components the kept operations do not reach are omitted too.
    #[arg(long = "keep-operation", value_name = "METHOD /path")]
    pub(crate) keep_operation: Vec<String>,
    /// Bind a header parameter once on the client instead of taking it at every call
    /// (repeatable), e.g. `--bound-header X-Api-Version`; added to `bound_headers` in
    /// `spargen.toml`.
    #[arg(long = "bound-header", value_name = "NAME")]
    pub(crate) bound_header: Vec<String>,
}

impl SpecArgs {
    /// The flags given that `spargen lock` does not read, each named once in declaration order.
    ///
    /// `lock` reads `--config` (whose file can set `batch_cap`) and `--batch-cap`; every other
    /// flag here shapes what is generated, and `vendor` fetches every remote `$ref` the spec
    /// reaches whatever they say. The destructuring is exhaustive, so a flag added to this struct
    /// does not compile until it is classified here.
    pub(crate) fn flags_lock_ignores(&self) -> Vec<&'static str> {
        let Self {
            config: _,
            batch_cap: _,
            carve,
            no_carve,
            uuid,
            no_uuid,
            datetime,
            error_body_cap,
            open_narrowing,
            no_open_narrowing,
            open_unions,
            no_open_unions,
            verbatim_docs,
            no_verbatim_docs,
            testing,
            no_testing,
            omit_path,
            omit_operation,
            omit_component,
            omit_pointer,
            keep_operation,
            bound_header,
        } = self;
        [
            (*carve, "--carve"),
            (*no_carve, "--no-carve"),
            (*uuid, "--uuid"),
            (*no_uuid, "--no-uuid"),
            (datetime.is_some(), "--datetime"),
            (error_body_cap.is_some(), "--error-body-cap"),
            (*open_narrowing, "--open-narrowing"),
            (*no_open_narrowing, "--no-open-narrowing"),
            (*open_unions, "--open-unions"),
            (*no_open_unions, "--no-open-unions"),
            (*verbatim_docs, "--verbatim-docs"),
            (*no_verbatim_docs, "--no-verbatim-docs"),
            (*testing, "--testing"),
            (*no_testing, "--no-testing"),
            (!omit_path.is_empty(), "--omit-path"),
            (!omit_operation.is_empty(), "--omit-operation"),
            (!omit_component.is_empty(), "--omit-component"),
            (!omit_pointer.is_empty(), "--omit-pointer"),
            (!keep_operation.is_empty(), "--keep-operation"),
            (!bound_header.is_empty(), "--bound-header"),
        ]
        .into_iter()
        .filter_map(|(given, flag)| given.then_some(flag))
        .collect()
    }
}

/// Arguments for `spargen check`.
#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// Path to the root OpenAPI document.
    pub(crate) spec: Utf8PathBuf,
    #[command(flatten)]
    pub(crate) options: SpecArgs,
    /// Output format for the audit.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,
}

/// Arguments for `spargen deps`.
#[derive(Debug, Args)]
pub(crate) struct DepsArgs {
    /// Path to the root OpenAPI document.
    pub(crate) spec: Utf8PathBuf,
    #[command(flatten)]
    pub(crate) options: SpecArgs,
    /// Output format for the dependency block.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,
}

/// Arguments for `spargen lock`.
#[derive(Debug, Args)]
pub(crate) struct LockArgs {
    /// Path to the root OpenAPI document. Remote `$ref`s reachable from it are fetched, vendored
    /// under `.spargen/vendor/`, and pinned in `spargen.lock` beside the spec.
    pub(crate) spec: Utf8PathBuf,
    #[command(flatten)]
    pub(crate) options: SpecArgs,
    /// Output format for the report.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,
}

/// Arguments for `spargen diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    /// Path to the OLD (baseline) OpenAPI document.
    pub(crate) old: Utf8PathBuf,
    /// Path to the NEW (candidate) OpenAPI document.
    pub(crate) new: Utf8PathBuf,
    /// Exit non-zero (status 1) when the diff is a breaking (`major`) change — a CI gate. Without
    /// this flag a completed comparison exits 0 whatever its bump; a spec that fails to lower
    /// exits 1, and a configuration error 3, either way.
    #[arg(long)]
    pub(crate) exit_code: bool,
    /// Applied to BOTH specs, so the two surfaces stay comparable.
    #[command(flatten)]
    pub(crate) options: SpecArgs,
    /// Output format for the report.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,
}

/// Arguments for `spargen explain`.
#[derive(Debug, Args)]
pub(crate) struct ExplainArgs {
    /// The diagnostic code, e.g. `E013`.
    pub(crate) code: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,
}

/// The rendering format for diagnostics and reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// Human-readable, rustc-style.
    Human,
    /// Machine-readable JSON, for CI.
    Json,
}
