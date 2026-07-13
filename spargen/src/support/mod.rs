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
            name: "middleware.rs",
            contents: include_str!("runtime/middleware.rs"),
        },
        SupportFile {
            name: "paginate.rs",
            contents: include_str!("runtime/paginate.rs"),
        },
        SupportFile {
            name: "response.rs",
            contents: include_str!("runtime/response.rs"),
        },
        SupportFile {
            name: "retry.rs",
            contents: include_str!("runtime/retry.rs"),
        },
        SupportFile {
            name: "stream.rs",
            contents: include_str!("runtime/stream.rs"),
        },
        SupportFile {
            name: "transport.rs",
            contents: include_str!("runtime/transport.rs"),
        },
    ];
    FILES
}

/// The XML codec runtime source, embedded only when an output uses an `application/xml` / `text/xml`
/// body (see [`crate::ir::Api::uses_xml`]). Kept out of [`runtime_files`] so a non-XML output never
/// carries the `quick-xml`-dependent module; its bytes still ship inside the published crate through
/// the `runtime/xml.rs` symlink so any XML-using consumer gets self-contained output.
pub fn xml_runtime_file() -> SupportFile {
    SupportFile {
        name: "xml.rs",
        contents: include_str!("runtime/xml.rs"),
    }
}

/// The blocking-facade runtime source (`BlockingRuntime`), embedded into every crate output but
/// gated behind the `blocking` feature at the module level, so the tokio-dependent code is compiled
/// only when a consumer opts in. Kept out of [`runtime_files`] (which is embedded unconditionally
/// with no cfg) because it must carry the `#[cfg(feature = "blocking")]` gate; its bytes ship inside
/// the published crate through the `runtime/blocking.rs` symlink.
pub fn blocking_runtime_file() -> SupportFile {
    SupportFile {
        name: "blocking.rs",
        contents: include_str!("runtime/blocking.rs"),
    }
}
