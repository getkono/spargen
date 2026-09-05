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
use quote::quote;

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
    let stream_exports = uses_streams.then(|| {
        quote! {
            #[allow(unused_imports)]
            pub use support::{
                EventStream, ReconnectPolicy, ReconnectReason, ReconnectWait, StreamError,
            };
        }
    });
    let datetime_exports = uses_time.then(|| {
        quote! {
            #[allow(unused_imports)]
            pub use support::{Date, DateTime};
        }
    });
    let tokens = quote! {
        // The embedded `support` module is private, so this list is the whole nameable runtime
        // surface. It therefore has to cover every type that appears in a signature this output
        // emits: `HeaderError` is the return type of each `…Headers::from_headers`, `RetryWait` is
        // what a caller's `RetryPolicy` must return, `ClientCore` is what `Client::core` hands
        // back, and the taxonomy's payload types are matched on. Anything short of that leaves a
        // generated signature a caller can call but cannot write down. `ApiErrorBody` is the
        // bound `Error::api_body` needs, implemented by the uniform-body error enum, the
        // single-body newtype, and the uninhabited shape (an enum mixing bodies gets none).
        #[allow(unused_imports)]
        pub use support::{
            ApiErrorBody, AuthError, ClientConfig, ClientCore, Credential, Error, ExecuteFuture,
            ExposeSecret, HeaderError, HeaderShape, HttpBackend, LinkPaginator, Middleware,
            MiddlewareBackend, Next, ProtocolError, RedirectError, ReqwestBackend, RequestError,
            ResponseValue, RetryBackend, RetryOutcome, RetryPolicy, RetryWait, SecretString,
            TimeoutKind, TokenFuture, TokenProvider, TransportError, exponential_backoff, next_link,
        };

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
