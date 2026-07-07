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
    /// Show extended documentation for a diagnostic code.
    Explain(ExplainArgs),
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
    /// Disable the `format: uuid` mapping (fall back to `String`).
    #[arg(long)]
    pub no_uuid: bool,
    /// Disable the `format: date-time`/`date` mappings (fall back to `String`).
    #[arg(long)]
    pub no_time: bool,
    /// Output format for diagnostics.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

/// Arguments for `spargen check`.
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Path to the root OpenAPI document.
    pub spec: Utf8PathBuf,
    /// Output format for the audit.
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

/// The rendering format for diagnostics and reports (PRD FR6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable, rustc-style.
    Human,
    /// Machine-readable JSON, for CI.
    Json,
}
