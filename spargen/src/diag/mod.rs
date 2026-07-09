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
        DiagnosticBuilder {
            code,
            severity: Severity::Error,
            provenance: at,
            message: None,
            remedy: None,
            interpretation: code.interpretation(),
        }
    }

    /// Begin building a warning diagnostic for `code` at `at`.
    pub fn warning(code: Code, at: Provenance) -> DiagnosticBuilder {
        DiagnosticBuilder {
            code,
            severity: Severity::Warning,
            provenance: at,
            message: None,
            remedy: None,
            interpretation: code.interpretation(),
        }
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
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attach a remedy suggestion (rendered as a `help:` line).
    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    /// Link the governing interpretation (PRD §3.3).
    pub fn interpretation(mut self, id: InterpId) -> Self {
        self.interpretation = Some(id);
        self
    }

    /// Finish building the diagnostic.
    pub fn build(self) -> Diagnostic {
        Diagnostic {
            code: self.code,
            severity: self.severity,
            pointer: self.provenance.pointer,
            span: self.provenance.span,
            message: self.message.unwrap_or_else(|| self.code.title().to_owned()),
            remedy: self.remedy,
            interpretation: self.interpretation,
        }
    }

    /// Build the diagnostic and record it into `diags` in one step.
    pub fn emit(self, diags: &mut Diagnostics) {
        diags.emit(self.build());
    }
}
