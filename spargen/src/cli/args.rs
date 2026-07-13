use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// The `spargen` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "spargen",
    version,
    about = "A compile-time-correct Rust client generator for OpenAPI 3.1.x."
)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// A `spargen` subcommand.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Generate a client from a spec (or, with `--check`, fail on drift).
    Generate(GenerateArgs),
    /// Audit a spec's feature support without generating code.
    Check(CheckArgs),
    /// Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock` (the only networked step).
    Lock(LockArgs),
    /// Show extended documentation for a diagnostic code.
    Explain(ExplainArgs),
    /// Report the semver impact of regenerating the client from a newer spec.
    Diff(DiffArgs),
}

/// Arguments for `spargen generate`.
#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Output module path, or crate directory with `--as-crate`.
    #[arg(short, long)]
    pub out: Utf8PathBuf,
    /// Fail if the checked-in output has drifted from the spec, instead of writing.
    #[arg(long)]
    pub check: bool,
    /// Emit a standalone publishable crate rather than a module.
    #[arg(long)]
    pub as_crate: bool,
    /// Watch the spec (and its referenced files, config, and lock) and regenerate on every change.
    /// Runs until interrupted (Ctrl-C).
    #[arg(long)]
    pub watch: bool,
    /// Auto-carve: instead of failing on rejections, omit the minimal set of unsupported constructs
    /// (each reported via W009) and generate the rest. Un-carvable rejections are still reported.
    #[arg(long)]
    pub carve: bool,
    /// Disable the `format: uuid` mapping (fall back to `String`).
    #[arg(long)]
    pub no_uuid: bool,
    /// Disable the `format: date-time`/`date` mappings (fall back to `String`).
    #[arg(long)]
    pub no_time: bool,
    /// Path to a `spargen.toml` config file. Defaults to `spargen.toml` beside the spec, if present.
    #[arg(long)]
    pub config: Option<Utf8PathBuf>,
    /// Omit a path item and every operation under it (repeatable), e.g. `--omit-path /pets/{id}`.
    #[arg(long = "omit-path", value_name = "PATH")]
    pub omit_path: Vec<String>,
    /// Omit one operation (repeatable), e.g. `--omit-operation "get /pets"`.
    #[arg(long = "omit-operation", value_name = "METHOD /path")]
    pub omit_operation: Vec<String>,
    /// Omit a named component (repeatable), e.g. `--omit-component "schema:LegacyPet"`.
    #[arg(long = "omit-component", value_name = "kind:name")]
    pub omit_component: Vec<String>,
    /// Omit an RFC 6901 pointer (repeatable), e.g. `--omit-pointer "[file#]/pointer"`.
    #[arg(long = "omit-pointer", value_name = "[file#]/pointer")]
    pub omit_pointer: Vec<String>,
    /// Output format for diagnostics.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// Arguments for `spargen check`.
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Path to a `spargen.toml` config file. Defaults to `spargen.toml` beside the spec, if present.
    #[arg(long)]
    pub config: Option<Utf8PathBuf>,
    /// Auto-carve: omit the minimal set of unsupported constructs (each reported via W009) so the
    /// rest audits clean. Un-carvable rejections are still reported.
    #[arg(long)]
    pub carve: bool,
    /// Omit a path item and every operation under it (repeatable), e.g. `--omit-path /pets/{id}`.
    #[arg(long = "omit-path", value_name = "PATH")]
    pub omit_path: Vec<String>,
    /// Omit one operation (repeatable), e.g. `--omit-operation "get /pets"`.
    #[arg(long = "omit-operation", value_name = "METHOD /path")]
    pub omit_operation: Vec<String>,
    /// Omit a named component (repeatable), e.g. `--omit-component "schema:LegacyPet"`.
    #[arg(long = "omit-component", value_name = "kind:name")]
    pub omit_component: Vec<String>,
    /// Omit an RFC 6901 pointer (repeatable), e.g. `--omit-pointer "[file#]/pointer"`.
    #[arg(long = "omit-pointer", value_name = "[file#]/pointer")]
    pub omit_pointer: Vec<String>,
    /// Output format for the audit.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// Arguments for `spargen lock`.
#[derive(Debug, Args)]
pub struct LockArgs {
    /// Path to the root OpenAPI document. Remote `$ref`s reachable from it are fetched, vendored
    /// under `.spargen/vendor/`, and pinned in `spargen.lock` beside the spec.
    pub spec: Utf8PathBuf,
    /// Output format for the report.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// Arguments for `spargen diff`.
#[derive(Debug, Args)]
pub struct DiffArgs {
    /// Path to the OLD (baseline) OpenAPI document.
    pub old: Utf8PathBuf,
    /// Path to the NEW (candidate) OpenAPI document.
    pub new: Utf8PathBuf,
    /// Exit non-zero (status 1) when the diff is a breaking (`major`) change — a CI gate. Without
    /// this flag `diff` always exits 0 (a spec that fails to lower still exits 1 either way).
    #[arg(long)]
    pub exit_code: bool,
    /// Output format for the report.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// Arguments for `spargen explain`.
#[derive(Debug, Args)]
pub struct ExplainArgs {
    /// The diagnostic code, e.g. `E042`.
    pub code: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// The rendering format for diagnostics and reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable, rustc-style.
    Human,
    /// Machine-readable JSON, for CI.
    Json,
}
