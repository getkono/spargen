use std::process::ExitCode;

/// The stable exit-code contract for the CLI — suitable as a CI gate between spec producers and
/// consumers. The numeric values are product surface and contract-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ExitStatus {
    /// Success — no errors (warnings may still have been emitted).
    Ok = 0,
    /// One or more error-severity diagnostics were reported, or `spargen diff --exit-code` found a
    /// breaking change.
    Diagnostics = 1,
    /// Invalid usage that parsed as a command: a `spargen.toml` or `--config` file that cannot be
    /// read or applied, a malformed omit flag, or an unknown code given to `spargen explain`.
    /// An argument clap itself rejects exits with clap's own status, 2.
    Usage = 3,
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        ExitCode::from(status as u8)
    }
}
