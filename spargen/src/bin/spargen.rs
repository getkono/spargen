//! Non-generating command-line tooling for vendoring and inspecting OpenAPI inputs.

#[path = "../cli/args.rs"]
mod args;
#[path = "../cli/config.rs"]
mod config;
#[path = "../cli/exit.rs"]
mod exit;
#[path = "../cli/run.rs"]
mod run;

use args::Cli;
use clap::Parser;

fn main() -> std::process::ExitCode {
    run::run(Cli::parse())
}
