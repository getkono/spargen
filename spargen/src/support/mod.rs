//! # Subsystem: support
//! layer-deps:
//!
//! The generator-side handle to the freestanding runtime shipped inside generated output. The
//! runtime itself is real, standalone-compilable source in the `support-runtime` workspace member
//! (compiled and tested in its own right, PRD §7.5); this module embeds it verbatim
//! (`include_str!`) into a private `support` module of the generated code (PRD §2.3 rule 3), and
//! exposes the FR5 error-taxonomy metadata as data for docs cross-referencing.

/// One runtime source file to embed into generated output.
#[derive(Debug, Clone, Copy)]
pub struct SupportFile {
    /// The emitted file/module name (e.g. `error.rs`).
    pub name: &'static str,
    /// The verbatim source contents.
    pub contents: &'static str,
}

/// The runtime source files, embedded from the `support-runtime` crate via `include_str!`. Emitted
/// as a private `support` module carrying `#![forbid(unsafe_code)]` (PRD FR3, §2.3 rule 3).
pub fn runtime_files() -> &'static [SupportFile] {
    todo!()
}

/// Metadata for one class of the FR5 error taxonomy, kept as data so the published docs and the
/// emitted `Error` type derive from one source (PRD §2.3 rule 2, FR5).
#[derive(Debug, Clone, Copy)]
pub struct ErrorClassInfo {
    /// The taxonomy class number (1–10).
    pub number: u8,
    /// The class name (e.g. `Transport`).
    pub name: &'static str,
    /// A one-line summary.
    pub summary: &'static str,
}

/// The FR5 error taxonomy as data (all ten classes, including the cancellation drop-safety
/// guarantee that has no `Error` variant).
pub fn taxonomy() -> &'static [ErrorClassInfo] {
    todo!()
}
