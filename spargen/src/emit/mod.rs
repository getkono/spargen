//! # Subsystem: emit
//! layer-deps: codegen, diag
//!
//! Output layout (module vs standalone crate), `Cargo.toml` synthesis, the provenance header, and
//! `--check` drift diffing (PRD §2.3, §2.1). Emit turns [`GeneratedCode`](crate::codegen::GeneratedCode)
//! into a concrete on-disk [`EmitPlan`] that can be written or compared against checked-in output.

mod check;
mod header;
mod manifest;

use camino::Utf8PathBuf;

use crate::codegen::{GeneratedCode, GeneratedFile};

pub use check::{check_drift, DriftReport, FileDiff};
pub use header::provenance_header;
pub use manifest::synth_cargo_toml;

/// Where and how generated code is written (PRD §2.2).
#[derive(Debug, Clone)]
pub enum OutputLayout {
    /// A module (file or directory) checked into an existing crate.
    Module {
        /// The module path to write.
        path: Utf8PathBuf,
    },
    /// A standalone, publishable crate.
    Crate {
        /// The crate directory to create.
        dir: Utf8PathBuf,
        /// Package identity for the synthesized `Cargo.toml`.
        package: PackageMeta,
    },
}

/// Identity of a synthesized standalone crate.
#[derive(Debug, Clone)]
pub struct PackageMeta {
    /// Crate name.
    pub name: String,
    /// Crate version.
    pub version: String,
}

/// The generated crate's feature set (default `uuid`+`time` on; PRD §6.2).
#[derive(Debug, Clone)]
pub struct FeatureSet {
    /// Enable the `uuid` mapping feature.
    pub uuid: bool,
    /// Enable the `time` mapping feature.
    pub time: bool,
}

impl Default for FeatureSet {
    fn default() -> Self {
        Self {
            uuid: true,
            time: true,
        }
    }
}

/// Identity of the source spec, stamped into the provenance header (PRD §2.1).
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
    /// The output layout.
    pub layout: OutputLayout,
    /// The generated crate's features.
    pub features: FeatureSet,
    /// Spec provenance to stamp.
    pub spec: SpecMeta,
}

/// A fully-rendered emission plan: every output file with its final on-disk contents (provenance
/// header stamped, `Cargo.toml` synthesized), ready to [`write`] or [`check_drift`].
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
    match &options.layout {
        OutputLayout::Module { path } => {
            let Some(file) = code.files.first() else {
                return Err(EmitError::Layout("codegen produced no files".to_owned()));
            };
            files.push(GeneratedFile {
                path: path.clone(),
                contents: format!("{header}{}", file.contents),
            });
        }
        OutputLayout::Crate { dir, package } => {
            files.push(GeneratedFile {
                path: dir.join("Cargo.toml"),
                contents: synth_cargo_toml(package, &options.features),
            });
            for file in &code.files {
                files.push(GeneratedFile {
                    path: dir.join("src").join(&file.path),
                    contents: format!("{header}{}", file.contents),
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(EmitPlan { files })
}

/// Write a plan to disk.
pub fn write(plan: &EmitPlan) -> Result<(), EmitError> {
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file.path, &file.contents)?;
    }
    Ok(())
}
