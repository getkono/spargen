use std::process::ExitCode;

/// The stable exit-code contract for the CLI — suitable as a CI gate between spec producers and
/// consumers (PRD FR6, §7.5). The numeric values are product surface and contract-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitStatus {
    /// Success — no errors (warnings may still have been emitted).
    Ok = 0,
    /// One or more error-severity diagnostics were reported.
    Diagnostics = 1,
    /// `generate --check` found the checked-in output had drifted.
    Drift = 2,
    /// Invalid command-line usage.
    Usage = 3,
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        ExitCode::from(status as u8)
    }
}
