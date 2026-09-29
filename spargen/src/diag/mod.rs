//! # Subsystem: diag
//! layer-deps:
//!
//! Diagnostic codes/severities, the JSON Pointer + span model, the `INT-###` interpretation
//! registry, and the S/W/R disposition table as data.
//! `diag` is the only vocabulary shared across pipeline stages, so it depends on nothing.
//!
//! Every diagnostic carries a severity, a stable [`Code`], the [`JsonPointer`] to the offending
//! construct, a [`Span`] (`file:line:column`), a one-line message, and an optional remedy —
//! Generation collects all diagnostics into a capped [`Diagnostics`] batch rather than stopping
//! at the first error; presentation belongs to the optional binary.

mod code;
mod collect;
mod interp;
mod pointer;
mod provenance;
mod severity;
mod span;

pub use code::{Code, UnknownCode};
pub(crate) use collect::{Aborted, Diagnostics};
pub use interp::InterpId;
pub use pointer::JsonPointer;
pub(crate) use provenance::Provenance;
pub use severity::Severity;
pub use span::{FileId, Loc, Span};

/// Whether `source` declares `fn {name}(` immediately preceded by `#[test]`, with only whitespace
/// between.
///
/// Test-only: the one predicate behind every check that a fixture cited for an explain clause is
/// a `#[test]` in the module named — `EXPLAIN_CLAUSES_OWNED_ELSEWHERE` in `code.rs` and the `E023`
/// byte-for-byte test in `runtime_contract.rs` — so a fix to it reaches both. It lives in `diag`
/// because that is the lowest layer both can reach.
#[cfg(test)]
pub(crate) fn is_test_fn(source: &str, name: &str) -> bool {
    source.match_indices(&format!("fn {name}(")).any(|(at, _)| {
        source[..at]
            .rsplit_once("#[test]")
            .is_some_and(|(_, between)| between.trim().is_empty())
    })
}

/// A single diagnostic emitted during parsing, validation, or codegen.
#[derive(Debug, Clone, serde::Serialize)]
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
    /// The governing interpretation, when this diagnostic's behavior depends on one.
    pub interpretation: Option<InterpId>,
}

impl std::fmt::Display for Diagnostic {
    /// A rustc-shaped one-entry rendering: severity, code, message, then the source location and
    /// remedy when known. Shared by the CLI's human renderer and by `Report`'s own `Display`, so
    /// the two can never drift.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(formatter, "{severity}[{}]: {}", self.code, self.message)?;
        if let Some(span) = self.span {
            write!(formatter, "\n  --> {}:{}", span.start.line, span.start.col)?;
        }
        if !self.pointer.as_str().is_empty() {
            write!(formatter, "\n  at {}", self.pointer.as_str())?;
        }
        if let Some(remedy) = &self.remedy {
            write!(formatter, "\n  help: {remedy}")?;
        }
        Ok(())
    }
}

impl Diagnostic {
    /// Begin building an error diagnostic for `code` at `at`.
    ///
    /// Crate-internal: a [`Diagnostic`] is something spargen hands *out*, so the facade re-exports
    /// the type for its public fields, not for construction. `Provenance` and `DiagnosticBuilder`
    /// are deliberately not part of that facade, which would leave these constructors unnameable
    /// — and so uncallable — from outside anyway.
    pub(crate) fn error(code: Code, at: Provenance) -> DiagnosticBuilder {
        DiagnosticBuilder {
            code,
            severity: Severity::Error,
            provenance: at,
            message: None,
            remedy: None,
            interpretation: code.interpretation(),
        }
    }

    /// Begin building a warning diagnostic for `code` at `at`. Crate-internal, as [`Self::error`].
    pub(crate) fn warning(code: Code, at: Provenance) -> DiagnosticBuilder {
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
pub(crate) struct DiagnosticBuilder {
    code: Code,
    severity: Severity,
    provenance: Provenance,
    message: Option<String>,
    remedy: Option<String>,
    interpretation: Option<InterpId>,
}

impl DiagnosticBuilder {
    /// Set the one-line explanation.
    pub(crate) fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attach a remedy suggestion (rendered as a `help:` line).
    pub(crate) fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    /// Finish building the diagnostic.
    pub(crate) fn build(self) -> Diagnostic {
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
    pub(crate) fn emit(self, diags: &mut Diagnostics) {
        diags.emit(self.build());
    }
}

#[cfg(test)]
mod tests {
    use super::is_test_fn;

    #[test]
    fn is_test_fn_accepts_a_function_its_test_attribute_immediately_precedes() {
        assert!(is_test_fn("#[test]\nfn pinned() {}\n", "pinned"));
        assert!(is_test_fn(
            "mod tests {\n    #[test]\n    fn pinned() {}\n}\n",
            "pinned"
        ));
    }

    #[test]
    fn is_test_fn_rejects_every_name_that_is_not_a_test() {
        // Every caller only ever asks about names it expects to be tests, so without these cases
        // the predicate could accept anything and no fixture citation would notice (#202).
        // A helper with no attribute at all.
        assert!(!is_test_fn("fn helper() {}\n", "helper"));
        // A helper that follows a test: the nearest `#[test]` above it belongs to another item.
        assert!(!is_test_fn(
            "#[test]\nfn pinned() {}\n\nfn helper() {}\n",
            "helper"
        ));
        // A name that is only a prefix of the test's.
        assert!(!is_test_fn("#[test]\nfn pinned_more() {}\n", "pinned"));
        // A name the source never declares.
        assert!(!is_test_fn("#[test]\nfn pinned() {}\n", "absent"));
    }
}
