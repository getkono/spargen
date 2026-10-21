//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting. Codegen never
//! sees a spec document — it consumes only the IR and the [`crate::name::Names`] table.
//!
//! One module per emitted construct, each producing a deterministically-ordered token fragment
//! that [`generate`] assembles and formats: [`models`] (the `types` module) and [`union`] (its
//! union enums), [`client`] (the `Client`, its servers, and the per-operation items beside it),
//! [`operation`] (one async method), [`params`], [`body`], and [`dispatch`] (the method's
//! parameter, request-body, and status-dispatch code), [`responses`] (response enums, error types,
//! and header structs), [`blocking`] (the synchronous facade), [`runtime`] (the embedded `support`
//! module and the root re-exports), and the shared [`docs`] and [`ty`] renderers.

mod blocking;
mod body;
mod client;
mod dispatch;
mod docs;
mod format;
mod models;
mod operation;
mod params;
mod responses;
mod runtime;
mod ty;
mod union;

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::ir::Api;
use crate::name::Names;
use quote::{format_ident, quote};

use format::format_tokens;

/// Options controlling code generation. The `uuid`/`time` flags carry the `Spec::uuid` /
/// `Spec::time` knobs into the one generated module: when off, the corresponding `format`
/// mappings fall back to `String`.
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
/// produces stable diffs. Every spec construct is decided during lowering (multi-status responses,
/// once degraded under the retired `W003`, lower to typed per-operation response enums). A
/// formatter failure is an internal codegen bug, reported as `E011` and returned as an error
/// instead of generated source.
pub(crate) fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    diags: &mut Diagnostics,
) -> Result<GeneratedCode, ()> {
    let uses_streams = api.uses_streams();
    // The date mapping is off when the `time` knob is, in which case those primitives stay `String`
    // and the newtypes would be dead weight.
    let uses_time = options.feature_time && api.uses_time();
    let support = runtime::emit_support(api.uses_xml(), uses_streams, uses_time);
    let models = models::emit_models(api, names, options, uses_time);
    let client = client::emit_client(api, names, options);
    // The synchronous facade is always emitted, gated on the user-opt-in `blocking` feature; a
    // default build compiles it out entirely (no tokio reference, no `BlockingClient`).
    let blocking = blocking::emit_blocking_client(api, names);
    // Attributes ride on items rather than the file (`#![…]`): inner attributes would make the
    // output unusable via `include!` from OUT_DIR, the build.rs consumption path.
    // The root surface is emitted from the lists `name` owns, whose root scope reserves them before
    // allocating the operation types declared beside them.
    let root_reexport = |names: &[&str]| {
        let idents = names.iter().map(|name| format_ident!("{}", name));
        quote! {
            #[allow(unused_imports)]
            pub use support::{ #(#idents),* };
        }
    };
    let root_exports = root_reexport(crate::name::ROOT_REEXPORTS);
    let stream_exports = uses_streams.then(|| root_reexport(crate::name::STREAM_ROOT_REEXPORTS));
    let datetime_exports = uses_time.then(|| root_reexport(crate::name::DATETIME_ROOT_REEXPORTS));
    let tokens = quote! {
        #root_exports
        #stream_exports
        #datetime_exports

        #support
        #models
        #client
        #blocking
    };
    format_generated(tokens, diags)
}

/// Format a completed generated module. A token stream that does not parse is an internal
/// invariant failure: reject the pipeline so no output or cache entry can be written.
fn format_generated(
    tokens: proc_macro2::TokenStream,
    diags: &mut Diagnostics,
) -> Result<GeneratedCode, ()> {
    let contents = match format_tokens(tokens) {
        Ok(contents) => contents,
        Err(error) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(format!(
                "internal code generation error: generated Rust could not be formatted: {error}"
            ))
            .remedy("Please report this spargen bug with a minimal reproducer; no output was generated.")
            .emit(diags);
            return Err(());
        }
    };
    Ok(GeneratedCode {
        files: vec![GeneratedFile { contents }],
    })
}

#[cfg(test)]
mod tests {
    use super::format_generated;
    use crate::diag::{Code, Diagnostics};
    use quote::quote;

    #[test]
    fn unparseable_generated_tokens_reject_with_e011() {
        let mut diags = Diagnostics::new(4);
        let result = format_generated(quote! { fn broken() -> }, &mut diags);

        assert!(
            result.is_err(),
            "invalid tokens must not become generated code"
        );
        assert_eq!(diags.items().len(), 1, "{:#?}", diags.items());
        assert_eq!(diags.items()[0].code, Code::InvalidInput);
        assert!(
            diags.items()[0]
                .message
                .contains("internal code generation error"),
            "{:#?}",
            diags.items()[0]
        );
        assert!(
            diags.items()[0]
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains("no output was generated")),
            "{:#?}",
            diags.items()[0]
        );
    }
}
