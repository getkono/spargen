//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting. Codegen never
//! sees a spec document — it consumes only the IR and the [`Names`](crate::name::Names) table.

mod emit;
mod format;

use camino::Utf8PathBuf;

use crate::diag::Diagnostics;
use crate::ir::Api;
use crate::name::Names;
use quote::quote;

pub use format::format_tokens;

/// Options controlling code generation. The `uuid`/`time` flags mirror the emitted crate's
/// features: when off, the corresponding `format` mappings fall back to `String`.
#[derive(Debug, Clone)]
pub struct CodegenOptions {
    /// Map `format: uuid` to `uuid::Uuid` (else `String`).
    pub feature_uuid: bool,
    /// Map `format: date-time`/`date` to the `time` crate (else `String`).
    pub feature_time: bool,
    /// Max bytes of a response body retained on error variants; stamped into the
    /// generated client's default configuration.
    pub error_body_cap: usize,
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
pub struct GeneratedFile {
    /// The file's path, relative to the output root.
    pub path: Utf8PathBuf,
    /// The formatted source.
    pub contents: String,
}

/// The complete generated code for one client (models, client, embedded support).
#[derive(Debug, Clone, Default)]
pub struct GeneratedCode {
    /// The generated files, in deterministic order.
    pub files: Vec<GeneratedFile>,
}

/// Generate the Rust source for a client from the IR and allocated names.
///
/// Output is deterministic: item ordering does not depend on input map ordering, so checked-in code
/// produces stable diffs. `diags` is retained for any future codegen-time diagnostic; codegen emits
/// none today (every spec construct is decided during lowering).
pub fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    diags: &mut Diagnostics,
) -> GeneratedCode {
    // Codegen emits no diagnostics of its own: multi-status responses are now lowered to typed
    // per-operation response enums rather than degraded (the retired W003).
    let _ = diags;
    let support = emit::emit_support(api.uses_xml());
    let models = emit::emit_models(api, names, options);
    let client = emit::emit_client(api, names, options);
    // Attributes ride on items rather than the file (`#![…]`): inner attributes would make the
    // output unusable via `include!` from OUT_DIR, the build.rs consumption path.
    let tokens = quote! {
        #[allow(unused_imports)]
        pub use support::{
            AuthError, Credential, Error, ExecuteFuture, ExposeSecret, HttpBackend, LinkPaginator,
            ReqwestBackend, ResponseValue, SecretString, TokenFuture, TokenProvider,
            TransportError, next_link,
        };

        #support
        #models
        #client
    };
    let contents = format_tokens(tokens).unwrap_or_else(|error| {
        format!(
            "compile_error!({:?});\n",
            format!("spargen internal codegen error: {error}")
        )
    });
    GeneratedCode {
        files: vec![GeneratedFile {
            path: Utf8PathBuf::from("lib.rs"),
            contents,
        }],
    }
}
