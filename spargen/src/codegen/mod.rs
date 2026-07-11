//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting (PRD §2.3, FR3, NFR2). Codegen never
//! sees a spec document — it consumes only the IR and the [`Names`](crate::name::Names) table.

mod emit;
mod format;

use camino::Utf8PathBuf;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ir::{Api, ErrorShape, SuccessShape};
use crate::name::Names;
use quote::quote;

pub use format::format_tokens;

/// Options controlling code generation. The `uuid`/`time` flags mirror the emitted crate's
/// features (PRD §6.2): when off, the corresponding `format` mappings fall back to `String`.
#[derive(Debug, Clone)]
pub struct CodegenOptions {
    /// Map `format: uuid` to `uuid::Uuid` (else `String`).
    pub feature_uuid: bool,
    /// Map `format: date-time`/`date` to the `time` crate (else `String`).
    pub feature_time: bool,
    /// Max bytes of a response body retained on error variants (PRD D7); stamped into the
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
/// produces stable diffs (PRD FR3). Any codegen-time diagnostic flows through `diags`.
pub fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    diags: &mut Diagnostics,
) -> GeneratedCode {
    for operation in &api.operations {
        let degraded = match operation.responses.success() {
            SuccessShape::Enum => Some("success"),
            _ => match operation.responses.error() {
                ErrorShape::Enum => Some("error"),
                _ => None,
            },
        };
        if let Some(kind) = degraded {
            Diagnostic::warning(Code::ResponseDegradedToValue, operation.provenance.clone())
                .message(format!(
                    "operation `{}` documents multiple {kind} bodies; the {kind} type is \
                     generated as serde_json::Value",
                    operation.id.0
                ))
                .remedy("restructure the responses, or omit the operation with spargen::omit!")
                .emit(diags);
        }
    }
    let support = emit::emit_support();
    let models = emit::emit_models(api, names, options);
    let client = emit::emit_client(api, names, options);
    // Attributes ride on items rather than the file (`#![…]`): inner attributes would make the
    // output unusable via `include!` from OUT_DIR, the build.rs consumption path.
    let tokens = quote! {
        #[allow(unused_imports)]
        pub use support::{
            AuthError, Credential, Error, ExposeSecret, ResponseValue, SecretString, TokenFuture,
            TokenProvider,
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
