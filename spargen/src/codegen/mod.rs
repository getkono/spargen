//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting (PRD §2.3, FR3, NFR2). Codegen never
//! sees a spec document — it consumes only the IR and the [`Names`](crate::name::Names) table.

mod emit;
mod format;

use camino::Utf8PathBuf;

use crate::diag::Diagnostics;
use crate::ir::Api;
use crate::name::Names;

pub use format::format_tokens;

/// Options controlling code generation. The `uuid`/`time` flags mirror the emitted crate's
/// features (PRD §6.2): when off, the corresponding `format` mappings fall back to `String`.
#[derive(Debug, Clone)]
pub struct CodegenOptions {
    /// Map `format: uuid` to `uuid::Uuid` (else `String`).
    pub feature_uuid: bool,
    /// Map `format: date-time`/`date` to the `time` crate (else `String`).
    pub feature_time: bool,
}

impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            feature_uuid: true,
            feature_time: true,
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
    todo!()
}
