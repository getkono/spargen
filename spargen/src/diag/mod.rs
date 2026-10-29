//! # Subsystem: diag
//! layer-deps:
//!
//! Diagnostic codes and severities, each code's title and `spargen explain` text, the JSON
//! Pointer + span model, and the [`InterpId`] (`INT-###`) a code may name for the interpretation
//! its behavior depends on.
//! `diag` is the lowest layer, which every pipeline stage may reach, so it depends on nothing.
//!
//! Every diagnostic carries a severity, a stable [`Code`], the [`JsonPointer`] to the offending
//! construct, a [`Span`] (line and column within the file its [`FileId`] names) when the
//! construct's location is known, a one-line
//! message, an optional remedy, and the [`OutcomeClaim`] its message makes.
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
pub(crate) use pointer::escape_token;
pub use pointer::JsonPointer;
pub(crate) use provenance::Provenance;
pub use severity::Severity;
pub use span::{FileId, Loc, Span};

/// Whether `source` declares `fn {name}(` immediately preceded by `#[test]`, with only whitespace
/// between.
///
/// Test-only: the one predicate behind every check that a fixture cited for an explain clause is
/// a `#[test]` in the module named — `EXPLAIN_CLAUSES_OWNED_ELSEWHERE` in `code.rs` and the `E023`
/// byte-for-byte test in `runtime_contract/tests/explain.rs` — so a fix to it reaches both. It
/// lives in `diag` because that is the lowest layer both can reach.
#[cfg(test)]
pub(crate) fn is_test_fn(source: &str, name: &str) -> bool {
    source.match_indices(&format!("fn {name}(")).any(|(at, _)| {
        source[..at]
            .rsplit_once("#[test]")
            .is_some_and(|(_, between)| between.trim().is_empty())
    })
}

/// A single diagnostic emitted during a pipeline run.
///
/// Every field is public, so a caller reads and updates each one directly. The struct is
/// `#[non_exhaustive]`, so a field can be added without a breaking change: a caller receives a
/// diagnostic from a run's report rather than building one.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct Diagnostic {
    /// The stable, documented code (`E###`/`W###`).
    pub code: Code,
    /// Error or warning.
    pub severity: Severity,
    /// RFC 6901 pointer to the offending construct.
    pub pointer: JsonPointer,
    /// Source location, when the construct's span is known.
    pub span: Option<Span>,
    /// The path of the file [`Self::span`] lies in, as the input bundle loaded it: the spec path
    /// as given for the root document, the path a relative `$ref` resolved to for a referenced
    /// one, and the vendored copy for a remote one. Set exactly when `span` is and its file was
    /// loaded by the run that emitted the diagnostic.
    pub file: Option<String>,
    /// One-line human explanation.
    pub message: String,
    /// An optional suggested fix.
    pub remedy: Option<String>,
    /// The governing interpretation, when this diagnostic's behavior depends on one.
    pub interpretation: Option<InterpId>,
    /// What [`Self::message`] asserts about the outcome of the run that emitted it.
    ///
    /// The message is prose composed at the emission site. This field records the same claim as
    /// data, so it can be checked against the run's `Outcome`, which the emission site does not
    /// know. `Outcome::admits` is that check.
    pub claim: OutcomeClaim,
}

/// What a diagnostic's message asserts about the outcome of the run that emitted it.
///
/// A diagnostic is emitted before the run's outcome is decided, and the same diagnostic is
/// emitted by `check` and by `generate`. A message can only be true on every run if it claims
/// nothing more than what its emission site decides, or if it declares the outcome it claims
/// here. `Outcome::admits` then checks that claim against the run that emitted it (#413).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum OutcomeClaim {
    /// The message asserts nothing that depends on the run's outcome, so it holds on every run.
    ///
    /// Negative claims belong here. "no client code is generated for them" is true on a run that
    /// generates nothing too. So is a selection: "`{media}` is selected" names a decision made
    /// where the message is emitted, and a rejection elsewhere does not change it. Every warning
    /// is built with this claim.
    Independent,
    /// The message asserts that the run is rejected. It holds only when the outcome is
    /// `Rejected`. Every error is built with this claim, because an error is the rejection.
    Rejected,
    /// The message asserts that the run generates client code, for example "`{media}` is
    /// generated". It holds only when the outcome is `Generated` or `Cached`. It never holds on a
    /// `check` run, which generates nothing, or on a run rejected anywhere in the document.
    Generated,
}

impl OutcomeClaim {
    /// The claim every diagnostic of `severity` is built with: an error is the rejection it
    /// reports, and a warning claims nothing about the outcome.
    pub(crate) fn of(severity: Severity) -> Self {
        match severity {
            Severity::Error => OutcomeClaim::Rejected,
            Severity::Warning => OutcomeClaim::Independent,
        }
    }
}

impl std::fmt::Display for Diagnostic {
    /// A rustc-shaped one-entry rendering: severity, code, message, then the source location and
    /// remedy when known. Shared by the CLI's human renderer and by `Report`'s own `Display`, so
    /// the two can never drift.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}[{}]: {}",
            self.severity.as_str(),
            self.code,
            self.message
        )?;
        if let Some(span) = self.span {
            write!(formatter, "\n  --> ")?;
            if let Some(file) = &self.file {
                write!(formatter, "{file}:")?;
            }
            write!(formatter, "{}:{}", span.start.line, span.start.col)?;
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
    /// The diagnostic claims [`OutcomeClaim::Rejected`], names `code`'s interpretation, carries
    /// no file, and takes `code`'s title as its message unless [`DiagnosticBuilder::message`]
    /// sets one.
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

    /// Begin building a warning diagnostic for `code` at `at`. It claims
    /// [`OutcomeClaim::Independent`]; everything else is as [`Self::error`] builds it.
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

    /// Begin building a diagnostic for `code`, at `code`'s own severity, about something with no
    /// location in the document — the build environment, an output or cache file, the consumer
    /// manifest — so it points at the document root and carries no span.
    pub(crate) fn at_root(code: Code, message: impl Into<String>) -> DiagnosticBuilder {
        let at = Provenance::new(JsonPointer::root(), None);
        let builder = match code.severity() {
            Severity::Error => Self::error(code, at),
            Severity::Warning => Self::warning(code, at),
        };
        builder.message(message)
    }
}

/// Fluent builder for a [`Diagnostic`], begun by [`Diagnostic::error`] or [`Diagnostic::warning`]:
/// attaches the message and remedy, then [`Self::build`] finishes the diagnostic.
#[derive(Debug)]
#[must_use = "a builder does nothing until it is built"]
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
            file: None,
            message: self.message.unwrap_or_else(|| self.code.title().to_owned()),
            remedy: self.remedy,
            interpretation: self.interpretation,
            claim: OutcomeClaim::of(self.severity),
        }
    }

    /// Build the diagnostic and record it into `diags` in one step.
    pub(crate) fn emit(self, diags: &mut Diagnostics) {
        diags.emit(self.build());
    }
}

#[cfg(test)]
mod tests {
    use super::{is_test_fn, Code, Diagnostic, JsonPointer, OutcomeClaim, Provenance, Severity};

    #[test]
    fn an_error_claims_the_rejection_and_a_warning_claims_nothing() {
        // `tests/frontend/` holds every claim to its run's outcome. If every diagnostic were built
        // `Independent`, that check would pass on every run and hold nothing, including that an
        // error only ever reaches a rejected run.
        let at = || Provenance::new(JsonPointer::root(), None);
        assert_eq!(
            Diagnostic::error(Code::UnsupportedMediaType, at())
                .build()
                .claim,
            OutcomeClaim::Rejected
        );
        assert_eq!(
            Diagnostic::warning(Code::AlternativeMediaIgnored, at())
                .build()
                .claim,
            OutcomeClaim::Independent
        );
    }

    #[test]
    fn a_root_diagnostic_takes_its_codes_severity_and_points_at_the_root() {
        // The four root-pointer sites (the facade's pipeline errors, the Cargo-integration
        // policy, the build cache, and the `E023` audit) built these by hand before; each must
        // read exactly as it did, which is what the explicit builders produce.
        for (code, built) in [
            (
                Code::InvalidInput,
                Diagnostic::error(
                    Code::InvalidInput,
                    Provenance::new(JsonPointer::root(), None),
                ),
            ),
            (
                Code::CargoIntegrationDegraded,
                Diagnostic::warning(
                    Code::CargoIntegrationDegraded,
                    Provenance::new(JsonPointer::root(), None),
                ),
            ),
        ] {
            let at_root = Diagnostic::at_root(code, "the message").build();
            assert_eq!(
                serde_json::to_value(&at_root).unwrap(),
                serde_json::to_value(built.message("the message").build()).unwrap()
            );
            assert_eq!(at_root.severity, code.severity());
            assert_eq!(at_root.pointer, JsonPointer::root());
            assert!(at_root.span.is_none() && at_root.file.is_none());
            assert_eq!(at_root.claim, OutcomeClaim::of(code.severity()));
        }
    }

    #[test]
    fn a_serialized_diagnostic_carries_its_claim_in_kebab_case() {
        // `--format json` renders a `Report`'s diagnostics through this `Serialize`, so the key
        // and these spellings are what a JSON consumer reads.
        let at = || Provenance::new(JsonPointer::root(), None);
        let claim_of =
            |diagnostic: Diagnostic| serde_json::to_value(diagnostic).unwrap()["claim"].clone();
        assert_eq!(
            claim_of(Diagnostic::error(Code::UnsupportedMediaType, at()).build()),
            "rejected"
        );
        assert_eq!(
            claim_of(Diagnostic::warning(Code::AlternativeMediaIgnored, at()).build()),
            "independent"
        );
        let mut generated = Diagnostic::warning(Code::AlternativeMediaIgnored, at()).build();
        generated.claim = OutcomeClaim::Generated;
        assert_eq!(claim_of(generated), "generated");
    }

    /// `as_str`, `Display`, and the serialized form (what `--format json` emits) spell each
    /// severity alike, and a diagnostic's rendering opens with the same word.
    #[test]
    fn each_severity_renders_its_stable_lowercase_name() {
        let at = || Provenance::new(JsonPointer::root(), None);
        for (severity, name, diagnostic) in [
            (
                Severity::Error,
                "error",
                Diagnostic::error(Code::UnsupportedMediaType, at()).build(),
            ),
            (
                Severity::Warning,
                "warning",
                Diagnostic::warning(Code::AlternativeMediaIgnored, at()).build(),
            ),
        ] {
            assert_eq!(severity.as_str(), name);
            assert_eq!(severity.to_string(), name);
            assert_eq!(serde_json::to_value(severity).unwrap(), name);
            assert_eq!(diagnostic.severity, severity);
            assert!(
                diagnostic
                    .to_string()
                    .starts_with(&format!("{name}[{}]", diagnostic.code)),
                "{diagnostic}"
            );
        }
    }

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
