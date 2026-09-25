//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting. Codegen never
//! sees a spec document — it consumes only the IR and the [`crate::name::Names`] table.

mod emit;
mod format;

use crate::diag::Diagnostics;
use crate::ir::Api;
use crate::name::Names;
use quote::{format_ident, quote};

pub(crate) use format::format_tokens;

/// Options controlling code generation. The `uuid`/`time` flags mirror the emitted crate's
/// features: when off, the corresponding `format` mappings fall back to `String`.
#[derive(Debug, Clone)]
pub(crate) struct CodegenOptions {
    /// Map `format: uuid` to `uuid::Uuid` (else `String`).
    pub(crate) feature_uuid: bool,
    /// Map `format: date-time`/`date` to the `time` crate (else `String`).
    pub(crate) feature_time: bool,
    /// Max bytes of a response body retained on error variants; stamped into the
    /// generated client's default configuration.
    pub(crate) error_body_cap: usize,
}

impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            feature_uuid: true,
            feature_time: true,
            error_body_cap: 64 * 1024,
        }
    }
}

/// A single generated source file, already formatted rustfmt-clean via `prettyplease`.
#[derive(Debug, Clone)]
pub(crate) struct GeneratedFile {
    /// The formatted source.
    pub(crate) contents: String,
}

/// The complete generated code for one client (models, client, embedded support).
#[derive(Debug, Clone, Default)]
pub(crate) struct GeneratedCode {
    /// The generated files, in deterministic order.
    pub(crate) files: Vec<GeneratedFile>,
}

/// Generate the Rust source for a client from the IR and allocated names.
///
/// Output is deterministic: item ordering does not depend on input map ordering, so checked-in code
/// produces stable diffs. `diags` is retained for any future codegen-time diagnostic; codegen emits
/// none today (every spec construct is decided during lowering).
pub(crate) fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    diags: &mut Diagnostics,
) -> GeneratedCode {
    // Codegen emits no diagnostics of its own: multi-status responses are now lowered to typed
    // per-operation response enums rather than degraded (the retired W003).
    let _ = diags;
    let uses_streams = api.uses_streams();
    // The date mapping is off when the `time` knob is, in which case those primitives stay `String`
    // and the newtypes would be dead weight.
    let uses_time = options.feature_time && api.uses_time();
    let support = emit::emit_support(api.uses_xml(), uses_streams, uses_time);
    let models = emit::emit_models(api, names, options);
    let client = emit::emit_client(api, names, options);
    // The synchronous facade is always emitted, gated on the user-opt-in `blocking` feature; a
    // default build compiles it out entirely (no tokio reference, no `BlockingClient`).
    let blocking = emit::emit_blocking_client(api, names, options);
    // Attributes ride on items rather than the file (`#![…]`): inner attributes would make the
    // output unusable via `include!` from OUT_DIR, the build.rs consumption path.
    // The root surface is emitted from the lists `emit` owns, which `error_type_ident` also reads.
    let root_reexport = |names: &[&str]| {
        let idents = names.iter().map(|name| format_ident!("{}", name));
        quote! {
            #[allow(unused_imports)]
            pub use support::{ #(#idents),* };
        }
    };
    let root_exports = root_reexport(emit::ROOT_REEXPORTS);
    let stream_exports = uses_streams.then(|| root_reexport(emit::STREAM_ROOT_REEXPORTS));
    let datetime_exports = uses_time.then(|| root_reexport(emit::DATETIME_ROOT_REEXPORTS));
    let tokens = quote! {
        #root_exports
        #stream_exports
        #datetime_exports

        #support
        #models
        #client
        #blocking
    };
    let contents = format_tokens(tokens).unwrap_or_else(|error| {
        format!(
            "compile_error!({:?});\n",
            format!("spargen internal codegen error: {error}")
        )
    });
    GeneratedCode {
        files: vec![GeneratedFile { contents }],
    }
}
