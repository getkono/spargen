//! # Subsystem: support
//! layer-deps:
//!
//! The generator-side handle to the freestanding runtime shipped inside generated output. The
//! runtime itself is real, standalone-compilable source in the `support-runtime` workspace member
//! (compiled and tested in its own right); this module embeds it verbatim
//! (`include_str!`) into a private `support` module of the generated code, and
//! exposes the error-taxonomy metadata as data for docs cross-referencing.

/// One runtime source file to embed into generated output.
#[derive(Debug, Clone, Copy)]
pub struct SupportFile {
    /// The emitted file/module name (e.g. `error.rs`).
    pub name: &'static str,
    /// The verbatim source contents.
    pub contents: &'static str,
}

/// The runtime source files, embedded from the `support-runtime` crate via `include_str!`. Emitted
/// as a private `support` module carrying `#![forbid(unsafe_code)]`.
///
/// The `include_str!` paths resolve through `src/support/runtime/`, whose entries are symlinks to
/// the canonical `support-runtime/src/*.rs` sources. That indirection keeps a single source of
/// truth while ensuring `cargo publish` follows the links and ships the bytes *inside* the
/// `spargen` crate — so the published crate is self-contained.
pub fn runtime_files() -> &'static [SupportFile] {
    const FILES: &[SupportFile] = &[
        SupportFile {
            name: "auth.rs",
            contents: include_str!("runtime/auth.rs"),
        },
        SupportFile {
            name: "client.rs",
            contents: include_str!("runtime/client.rs"),
        },
        SupportFile {
            name: "dispatch.rs",
            contents: include_str!("runtime/dispatch.rs"),
        },
        SupportFile {
            name: "error.rs",
            contents: include_str!("runtime/error.rs"),
        },
        SupportFile {
            name: "response.rs",
            contents: include_str!("runtime/response.rs"),
        },
        SupportFile {
            name: "stream.rs",
            contents: include_str!("runtime/stream.rs"),
        },
    ];
    FILES
}
