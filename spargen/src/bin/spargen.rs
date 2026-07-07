//! The `spargen` binary — gated behind the default-on `cli` feature. A thin wrapper that parses
//! arguments and delegates to [`spargen::cli::run`].

use clap::Parser;
use spargen::cli::{run, Cli};

fn main() -> std::process::ExitCode {
    run(Cli::parse())
}
