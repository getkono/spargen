//! # Subsystem: support
//! layer-deps:
//!
//! The generator-side handle to the freestanding runtime shipped inside generated output. The
//! runtime itself is real, standalone-compilable source in the `support-runtime` workspace member
//! (compiled and tested in its own right); this module carries each file's source
//! (`include_str!`) for codegen to embed into a private `support` module of the generated code:
//! [`runtime_files`] for the files every client embeds as they are, and one accessor for each file
//! embedded on its own terms: streams, XML, the RFC 3339 date types and the base64 codec only for
//! an API that uses them, and the blocking client behind its `blocking` feature on a non-wasm32 target.
//!
//! The embed is not verbatim: `codegen::runtime::emit_support` keeps each file only up to its
//! `#[cfg(test)]` marker, rewrites every `crate::` to `super::` so the paths resolve from the
//! file's new submodule, and re-parses the result as tokens, which drops ordinary comments.

/// One runtime source file to embed into generated output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SupportFile {
    /// The emitted file/module name (e.g. `error.rs`).
    pub(crate) name: &'static str,
    /// The verbatim source contents.
    pub(crate) contents: &'static str,
}

/// The runtime source files, embedded from the `support-runtime` crate via `include_str!`. Emitted
/// as a private `support` module carrying an outer `#[forbid(unsafe_code)]`.
///
/// The `include_str!` paths resolve through `src/support/runtime/`, whose entries are symlinks to
/// the canonical `support-runtime/src/*.rs` sources. That indirection keeps a single source of
/// truth while ensuring `cargo publish` follows the links and ships the bytes *inside* the
/// `spargen` crate — so the published crate is self-contained.
pub(crate) fn runtime_files() -> &'static [SupportFile] {
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
            name: "header.rs",
            contents: include_str!("runtime/header.rs"),
        },
        SupportFile {
            name: "middleware.rs",
            contents: include_str!("runtime/middleware.rs"),
        },
        SupportFile {
            name: "parameter.rs",
            contents: include_str!("runtime/parameter.rs"),
        },
        SupportFile {
            name: "paginate.rs",
            contents: include_str!("runtime/paginate.rs"),
        },
        SupportFile {
            name: "request.rs",
            contents: include_str!("runtime/request.rs"),
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
            name: "transport.rs",
            contents: include_str!("runtime/transport.rs"),
        },
        SupportFile {
            name: "wasm.rs",
            contents: include_str!("runtime/wasm.rs"),
        },
    ];
    FILES
}

/// The streaming runtime, embedded only when an operation has a sequential response. With
/// [`chunked_runtime_file`], it carries the only references to `futures-core` and reqwest's
/// `stream` capability.
pub(crate) fn stream_runtime_file() -> SupportFile {
    SupportFile {
        name: "stream.rs",
        contents: include_str!("runtime/stream.rs"),
    }
}

/// The `wasm32` chunked read of a `fetch` response, embedded only when an operation reads a body
/// as it arrives: a sequential response, or a binary `_stream` twin (see
/// [`crate::ir::RuntimeFeatures::chunked_reads`]). It reads through `futures-core` and reqwest's
/// `stream` capability.
pub(crate) fn chunked_runtime_file() -> SupportFile {
    SupportFile {
        name: "chunked.rs",
        contents: include_str!("runtime/chunked.rs"),
    }
}

/// The XML codec runtime source, embedded only when an output uses an `application/xml` / `text/xml`
/// body (see [`crate::ir::Api::uses_xml`]). Kept out of [`runtime_files`] so a non-XML output never
/// carries the `quick-xml`-dependent module; its bytes still ship inside the published crate through
/// the `runtime/xml.rs` symlink so any XML-using consumer gets self-contained output.
pub(crate) fn xml_runtime_file() -> SupportFile {
    SupportFile {
        name: "xml.rs",
        contents: include_str!("runtime/xml.rs"),
    }
}

/// The RFC 3339 date-time runtime source over the `time` crate, embedded only when an output maps
/// `format: date-time` or `format: date` to a typed value under the `time` backend (see
/// [`crate::ir::Api::uses_time`] and the `datetime` config knob). Kept out of [`runtime_files`] so an
/// output with no dates never carries a `time` reference; its bytes still ship inside the published
/// crate through the `runtime/datetime.rs` symlink.
pub(crate) fn datetime_runtime_file() -> SupportFile {
    SupportFile {
        name: "datetime.rs",
        contents: include_str!("runtime/datetime.rs"),
    }
}

/// The same newtypes over the `jiff` crate, embedded in place of [`datetime_runtime_file`] (under
/// its module name, `datetime`) under the `jiff` backend, beside [`rfc3339_runtime_file`].
pub(crate) fn datetime_jiff_runtime_file() -> SupportFile {
    SupportFile {
        name: "datetime.rs",
        contents: include_str!("runtime/datetime_jiff.rs"),
    }
}

/// The same newtypes over the `chrono` crate, embedded in place of [`datetime_runtime_file`] (under
/// its module name, `datetime`) under the `chrono` backend, beside [`rfc3339_runtime_file`].
pub(crate) fn datetime_chrono_runtime_file() -> SupportFile {
    SupportFile {
        name: "datetime.rs",
        contents: include_str!("runtime/datetime_chrono.rs"),
    }
}

/// The RFC 3339 codec the `jiff` and `chrono` newtypes share, embedded beside either of them and
/// under no other backend. It adds no dependency.
pub(crate) fn rfc3339_runtime_file() -> SupportFile {
    SupportFile {
        name: "rfc3339.rs",
        contents: include_str!("runtime/rfc3339.rs"),
    }
}

/// The `contentEncoding: base64` codec runtime source, embedded only when an output emits a base64
/// newtype (see [`crate::ir::Api::uses_base64`]). Kept out of [`runtime_files`] so an output with no
/// such schema carries no unused codec; its bytes still ship inside the published crate through the
/// `runtime/base64.rs` symlink.
pub(crate) fn base64_runtime_file() -> SupportFile {
    SupportFile {
        name: "base64.rs",
        contents: include_str!("runtime/base64.rs"),
    }
}

/// The blocking-facade runtime source (`BlockingRuntime`), embedded into every generated module but
/// gated at the module level on the `blocking` feature on a non-wasm32 target, so the
/// tokio-dependent code is compiled only when a consumer opts in on a native target. Kept out of
/// [`runtime_files`] (which is embedded with no cfg) because it must carry the
/// `#[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]` gate; its bytes ship inside the
/// published crate through the `runtime/blocking.rs` symlink.
pub(crate) fn blocking_runtime_file() -> SupportFile {
    SupportFile {
        name: "blocking.rs",
        contents: include_str!("runtime/blocking.rs"),
    }
}
