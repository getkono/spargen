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
    const FILES: &[SupportFile] = &[
        SupportFile {
            name: "auth.rs",
            contents: include_str!("../../../support-runtime/src/auth.rs"),
        },
        SupportFile {
            name: "client.rs",
            contents: include_str!("../../../support-runtime/src/client.rs"),
        },
        SupportFile {
            name: "dispatch.rs",
            contents: include_str!("../../../support-runtime/src/dispatch.rs"),
        },
        SupportFile {
            name: "error.rs",
            contents: include_str!("../../../support-runtime/src/error.rs"),
        },
        SupportFile {
            name: "response.rs",
            contents: include_str!("../../../support-runtime/src/response.rs"),
        },
    ];
    FILES
}
