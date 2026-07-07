use std::process::ExitCode;

use super::Cli;

/// Execute a parsed CLI invocation and return the process exit code (PRD FR6, §7.5).
///
/// Delegates to the crate facade — [`generate`](crate::generate), [`check`](crate::check),
/// [`explain`](crate::explain) — renders diagnostics in the requested [`Format`](super::Format),
/// and maps the outcome onto the [`ExitStatus`](super::ExitStatus) contract. Per the DAG, the CLI
/// depends only on the facade.
pub fn run(cli: Cli) -> ExitCode {
    todo!()
}
