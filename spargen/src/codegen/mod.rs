//! # Subsystem: codegen
//! layer-deps: ir, name, support
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
//! module and the root re-exports), the shared [`docs`] and [`ty`] renderers, and [`idents`] (the
//! allocated-name lookups and status variant names the operation, blocking-client, parameter,
//! response and dispatch emitters read).

mod blocking;
mod body;
mod client;
mod dispatch;
mod docs;
mod format;
mod idents;
mod models;
mod operation;
mod params;
mod responses;
mod runtime;
mod ty;
mod union;

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

/// The default cap on bytes of a response body retained on generated error variants, which
/// [`crate::Spec`] starts from.
pub(crate) const DEFAULT_ERROR_BODY_CAP: usize = 64 * 1024;

/// The options a `Spec` with every knob at its default selects; only tests build them this way.
#[cfg(test)]
impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            feature_uuid: true,
            feature_time: true,
            error_body_cap: DEFAULT_ERROR_BODY_CAP,
        }
    }
}

/// The complete generated code for one client (models, client, embedded support): one module.
#[derive(Debug, Clone, Default)]
pub(crate) struct GeneratedCode {
    /// The module's source, already formatted rustfmt-clean via `prettyplease`.
    pub(crate) contents: String,
}

/// The prefix of the message a [`CodegenError`] displays. The frontend and fuzz harnesses
/// (`tests/frontend/main.rs`, `tests/fuzz_frontend.rs`) declare the same literal to recognise a
/// report that carries one, and a test in this module holds the declarations equal.
pub(crate) const CODEGEN_FAILURE: &str = "codegen produced unparseable Rust";

/// The assembled token stream did not parse as a Rust file: a codegen bug, not a spec problem.
/// The pipeline reports it as `E011` and writes and caches nothing.
#[derive(Debug)]
pub(crate) struct CodegenError(syn::Error);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{CODEGEN_FAILURE} (a spargen bug): {}", self.0)
    }
}

/// Generate the Rust source for a client from the IR and allocated names.
///
/// Output is deterministic: item ordering does not depend on input map ordering, so checked-in code
/// produces stable diffs. Codegen emits no diagnostics: every spec construct is decided during
/// lowering, so it takes no `Diagnostics` (multi-status responses, once degraded under the retired
/// `W003`, lower to typed per-operation response enums). Its one failure is a token stream that
/// does not parse as a Rust file, returned as a [`CodegenError`] for the caller to report.
pub(crate) fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> Result<GeneratedCode, CodegenError> {
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
    render(tokens)
}

/// Format the assembled module into the one generated file, or the [`CodegenError`] its tokens
/// raise when they are not a parseable Rust file.
fn render(tokens: proc_macro2::TokenStream) -> Result<GeneratedCode, CodegenError> {
    let contents = format_tokens(tokens).map_err(CodegenError)?;
    Ok(GeneratedCode { contents })
}

#[cfg(test)]
mod tests {
    use super::{render, CODEGEN_FAILURE};
    use quote::quote;

    /// No known spec reaches it, so the failure is pinned here: a token stream that is not a Rust
    /// file comes back as an error carrying [`CODEGEN_FAILURE`], not as a module.
    #[test]
    fn an_unparseable_token_stream_is_an_error_not_a_module() {
        let error = render(quote! { fn }).expect_err("`fn` alone is not a Rust file");
        assert!(error.to_string().starts_with(CODEGEN_FAILURE), "{error}");
        assert!(render(quote! { fn f() {} }).is_ok());
    }

    /// The frontend and fuzz harnesses match this very literal, so their no-codegen-failure
    /// oracles see each one: renaming the prefix here without them fails this test rather than
    /// silently blinding them.
    #[test]
    fn the_harnesses_match_the_codegen_failure_prefix() {
        let declared = format!("const CODEGEN_FAILURE: &str = \"{CODEGEN_FAILURE}\";");
        for harness in ["tests/frontend/main.rs", "tests/fuzz_frontend.rs"] {
            let source = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(harness),
            )
            .expect("the harness is readable");
            assert!(
                source.contains(&declared),
                "{harness} must declare `{declared}`"
            );
        }
    }
}
