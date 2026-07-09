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
    const TAXONOMY: &[ErrorClassInfo] = &[
        ErrorClassInfo {
            number: 1,
            name: "RequestConstruction",
            summary: "invalid base URL or request serialization failure",
        },
        ErrorClassInfo {
            number: 2,
            name: "Transport",
            summary: "DNS, connection, TLS, or socket failure",
        },
        ErrorClassInfo {
            number: 3,
            name: "Timeout",
            summary: "connect or total request timeout",
        },
        ErrorClassInfo {
            number: 4,
            name: "Protocol",
            summary: "malformed HTTP or decompression failure",
        },
        ErrorClassInfo {
            number: 5,
            name: "Redirect",
            summary: "redirect policy exhaustion",
        },
        ErrorClassInfo {
            number: 6,
            name: "Api",
            summary: "documented non-success response",
        },
        ErrorClassInfo {
            number: 7,
            name: "UnexpectedStatus",
            summary: "undocumented non-success response with retained body",
        },
        ErrorClassInfo {
            number: 8,
            name: "Decode",
            summary: "response body deserialization failure",
        },
        ErrorClassInfo {
            number: 9,
            name: "InterruptedBody",
            summary: "connection dropped while reading a body",
        },
        ErrorClassInfo {
            number: 10,
            name: "Cancellation",
            summary: "dropping a request future is safe under standard HTTP semantics",
        },
    ];
    TAXONOMY
}
