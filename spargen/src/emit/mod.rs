//! # Subsystem: emit
//! layer-deps: codegen, diag
//!
//! Module assembly and provenance stamping. Emit turns
//! [`GeneratedCode`](crate::codegen::GeneratedCode) into one deterministic module plan.

mod header;

use crate::codegen::{GeneratedCode, GeneratedFile};

pub use header::provenance_header;

/// Identity of the source spec, stamped into the provenance header.
#[derive(Debug, Clone)]
pub struct SpecMeta {
    /// A description of the source spec (path or URL as vendored).
    pub source: String,
    /// The spargen version that produced the output.
    pub spargen_version: String,
}

/// Options for one emission.
#[derive(Debug, Clone)]
pub struct EmitOptions {
    /// Spec provenance to stamp.
    pub spec: SpecMeta,
}

/// A fully-rendered emission plan with its final on-disk contents and provenance header.
#[derive(Debug, Clone, Default)]
pub struct EmitPlan {
    /// The files to write, in deterministic order.
    pub files: Vec<GeneratedFile>,
}

/// An emission failure.
#[derive(Debug)]
pub enum EmitError {
    /// A filesystem error.
    Io(std::io::Error),
    /// The requested layout is inconsistent with the generated code.
    Layout(String),
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmitError::Io(e) => write!(f, "emit I/O error: {e}"),
            EmitError::Layout(msg) => write!(f, "emit layout error: {msg}"),
        }
    }
}

impl std::error::Error for EmitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EmitError::Io(e) => Some(e),
            EmitError::Layout(_) => None,
        }
    }
}

impl From<std::io::Error> for EmitError {
    fn from(e: std::io::Error) -> Self {
        EmitError::Io(e)
    }
}

/// Build the on-disk emission plan from generated code and options: stamp the provenance header,
/// synthesize `Cargo.toml` for crate layout, and resolve module paths.
pub fn plan(code: &GeneratedCode, options: &EmitOptions) -> Result<EmitPlan, EmitError> {
    let header = provenance_header(&options.spec);
    let mut files = Vec::new();
    let Some(file) = code.files.first() else {
        return Err(EmitError::Layout("codegen produced no files".to_owned()));
    };
    files.push(GeneratedFile {
        contents: format!("{header}{}", file.contents),
    });
    Ok(EmitPlan { files })
}
