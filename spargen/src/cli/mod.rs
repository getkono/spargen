//! # Subsystem: cli (feature `cli`)
//! layer-deps: facade
//!
//! The command surface (`generate` / `check` / `explain`), the exit-code contract, and
//! `--format json` (PRD §2.3, FR6). Depends only on the crate facade; the binary
//! (`src/bin/spargen.rs`) is a thin wrapper over [`run`].

mod args;
mod exit;
mod run;

pub use args::{CheckArgs, Cli, Command, ExplainArgs, Format, GenerateArgs};
pub use exit::ExitStatus;
pub use run::run;
