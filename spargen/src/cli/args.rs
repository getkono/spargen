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
    /// `--batch-cap`: every remote `$ref` the spec reaches is fetched whatever the rest say.
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
    /// Map `format: date-time`/`date` to the `time` crate, overriding `time = false` in
    /// `spargen.toml`.
    #[arg(long, conflicts_with = "no_time")]
    pub(crate) time: bool,
    /// Map `format: date-time`/`date` to `String` instead of the `time` crate, overriding
    /// `time = true` in `spargen.toml`.
    #[arg(long)]
    pub(crate) no_time: bool,
    /// Max bytes of a response body retained on generated error variants (default 65536).
    #[arg(long, value_name = "BYTES")]
    pub(crate) error_body_cap: Option<usize>,
    /// Max diagnostics collected before batching stops (default 100).
    #[arg(long, value_name = "COUNT")]
    pub(crate) batch_cap: Option<usize>,
    /// Lower a response body's own string `enum`/`const` narrowings of a plain `string` as open
    /// enums, so a value the description does not list still decodes.
    #[arg(long, conflicts_with = "no_open_narrowing")]
    pub(crate) open_narrowing: bool,
    /// Keep response-body string narrowings closed, overriding `open_narrowing = true` in
    /// `spargen.toml`.
    #[arg(long)]
    pub(crate) no_open_narrowing: bool,
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
