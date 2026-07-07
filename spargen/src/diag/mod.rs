//! # Subsystem: diag
//! layer-deps:
//!
//! Diagnostic codes/severities, the JSON Pointer + span model, the `INT-###` interpretation
//! registry, human/JSON renderers, and the S/W/R disposition table as data (PRD §2.3, FR6).
//! `diag` is the only vocabulary shared across pipeline stages, so it depends on nothing.
//!
//! Every diagnostic carries a severity, a stable [`Code`], the [`JsonPointer`] to the offending
//! construct, a [`Span`] (`file:line:column`), a one-line message, and an optional remedy —
//! rendered rustc-style for humans ([`render_human`]) or as stable JSON for CI ([`render_json`]).
//! Generation collects all diagnostics into a capped [`Diagnostics`] batch rather than stopping
//! at the first error.

mod code;
mod collect;
mod interp;
mod pointer;
mod provenance;
mod render;
mod severity;
mod span;

pub use code::Code;
pub use collect::{Aborted, Diagnostics};
pub use interp::{all_interpretations, interpretation, InterpId, Interpretation};
pub use pointer::JsonPointer;
pub use provenance::Provenance;
pub use render::{render_human, render_json, SourceSnippets};
pub use severity::{Disposition, Severity};
pub use span::{FileId, Loc, Span};

/// A single diagnostic emitted during parsing, validation, or codegen (PRD FR6).
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// The stable, documented code (`E###`/`W###`).
    pub code: Code,
    /// Error or warning.
    pub severity: Severity,
    /// RFC 6901 pointer to the offending construct.
    pub pointer: JsonPointer,
    /// Source location, when the construct's span is known.
    pub span: Option<Span>,
    /// One-line human explanation.
    pub message: String,
    /// An optional suggested fix.
    pub remedy: Option<String>,
    /// The governing interpretation, when this diagnostic's behavior depends on one (PRD §3.3).
    pub interpretation: Option<InterpId>,
}

impl Diagnostic {
    /// Begin building an error diagnostic for `code` at `at`.
    pub fn error(code: Code, at: Provenance) -> DiagnosticBuilder {
        todo!()
    }

    /// Begin building a warning diagnostic for `code` at `at`.
    pub fn warning(code: Code, at: Provenance) -> DiagnosticBuilder {
        todo!()
    }
}

/// Fluent builder for a [`Diagnostic`]; attaches the message, remedy, and interpretation before
/// the diagnostic is recorded into a [`Diagnostics`] batch.
#[derive(Debug)]
pub struct DiagnosticBuilder {
    code: Code,
    severity: Severity,
    provenance: Provenance,
    message: Option<String>,
    remedy: Option<String>,
    interpretation: Option<InterpId>,
}

impl DiagnosticBuilder {
    /// Set the one-line explanation.
    pub fn message(self, message: impl Into<String>) -> Self {
        todo!()
    }

    /// Attach a remedy suggestion (rendered as a `help:` line).
    pub fn remedy(self, remedy: impl Into<String>) -> Self {
        todo!()
    }

    /// Link the governing interpretation (PRD §3.3).
    pub fn interpretation(self, id: InterpId) -> Self {
        todo!()
    }

    /// Finish building the diagnostic.
    pub fn build(self) -> Diagnostic {
        todo!()
    }

    /// Build the diagnostic and record it into `diags` in one step.
    pub fn emit(self, diags: &mut Diagnostics) {
        todo!()
    }
}
