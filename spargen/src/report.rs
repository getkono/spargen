//! What every entry point hands back: the [`Report`] of a run and its [`Outcome`].

use crate::diag::{Diagnostic, Diagnostics, OutcomeClaim, Severity};

/// The outcome of a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Outcome {
    /// The generated module was rendered by this run rather than replayed from the build cache.
    /// The output file is rewritten only when its bytes change.
    Generated,
    /// The generated module was already up to date and was verified from the build cache.
    Cached,
    /// An analysis completed without a rejection: [`check`](crate::check)'s support audit, or the
    /// lowering whose report [`requirements`](crate::requirements) or [`diff`](crate::diff)
    /// returns.
    Clean,
    /// An error-severity diagnostic was emitted, so the run did not get as far as it was asked
    /// to.
    Rejected,
}

impl Outcome {
    /// The stable lowercase string form. [`Display`](std::fmt::Display) writes it, and the
    /// serialized form (`#[serde(rename_all = "lowercase")]`, which `--format json` emits) spells
    /// each variant the same way.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Generated => "generated",
            Outcome::Cached => "cached",
            Outcome::Clean => "clean",
            Outcome::Rejected => "rejected",
        }
    }

    /// Whether the run got as far as it was asked to.
    pub fn is_success(self) -> bool {
        !matches!(self, Outcome::Rejected)
    }

    /// Whether this run rendered the output rather than replaying it from the build cache. A
    /// rendered output is written only when its bytes differ from the file already on disk.
    pub fn wrote_output(self) -> bool {
        matches!(self, Outcome::Generated)
    }

    /// Whether a diagnostic that makes `claim` about its run can be true on a run with this
    /// outcome.
    ///
    /// A diagnostic's [`Diagnostic::claim`] records what its message asserts. Every diagnostic a
    /// run reports must make a claim its outcome admits, through `check` and through `generate`
    /// alike. Any other combination is a message that says something false about the run.
    pub fn admits(self, claim: OutcomeClaim) -> bool {
        match claim {
            OutcomeClaim::Independent => true,
            OutcomeClaim::Rejected => self == Outcome::Rejected,
            OutcomeClaim::Generated => matches!(self, Outcome::Generated | Outcome::Cached),
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The result of a pipeline run: the collected diagnostics plus the outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    /// Every diagnostic emitted during the run (batch reporting).
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// What happened.
    pub(crate) outcome: Outcome,
    /// Whether the run hit `batch_cap` and dropped diagnostics past it.
    pub(crate) truncated: bool,
}

impl Report {
    /// The report of a run that collected `diagnostics` and ended in `outcome`, truncated when
    /// the batch reached its cap.
    pub(crate) fn new(diagnostics: Diagnostics, outcome: Outcome) -> Self {
        Self {
            diagnostics: diagnostics.items().to_vec(),
            outcome,
            truncated: diagnostics.cap_reached(),
        }
    }

    /// A rejection carrying `diagnostics`, `truncated` when the batch that produced them reached
    /// its cap.
    pub(crate) fn rejected(diagnostics: Vec<Diagnostic>, truncated: bool) -> Self {
        Self {
            diagnostics,
            outcome: Outcome::Rejected,
            truncated,
        }
    }

    /// Every diagnostic emitted during the run, in emission order.
    ///
    /// This is the whole list only when [`Self::truncated`] is `false`.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// What happened.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Whether the run reached `batch_cap` and dropped the diagnostics past it.
    ///
    /// A truncated report is a partial view: the constructs behind the dropped diagnostics were
    /// still diagnosed, so fixing every diagnostic listed here may not be enough to make the run
    /// clean. Raise [`Spec::batch_cap`](crate::Spec::batch_cap) to see the rest.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Every error-severity diagnostic.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> + '_ {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Every warning-severity diagnostic.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> + '_ {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
    }

    /// Whether any error-severity diagnostic was emitted.
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Whether the run succeeded with no errors — the single check a build script wants.
    pub fn succeeded(&self) -> bool {
        self.outcome.is_success() && !self.has_errors()
    }

    /// Turn the report into a `Result`, so a build script can use `?`.
    pub fn into_result(self) -> Result<Self, Self> {
        if self.succeeded() {
            Ok(self)
        } else {
            Err(self)
        }
    }

    /// Print every diagnostic as a Cargo build-script directive, so it surfaces in the build's
    /// output at its own severity: an error-severity diagnostic goes out as `cargo::error=`, a
    /// warning as `cargo::warning=`. Announcing a rejection as a warning would make the one
    /// message that stops generation look like the ones that do not.
    ///
    /// A directive is one line, and Cargo shows a build script's other stdout only under `-vv`,
    /// so every line of a diagnostic's rendering (its location, pointer, and `help:` remedy
    /// included) goes out as its own directive at that diagnostic's severity. A truncated report
    /// ends with the truncation notice as a `cargo::warning=`.
    pub fn emit_cargo_diagnostics(&self) {
        for line in self.cargo_directive_lines() {
            println!("{line}");
        }
    }

    /// The directive lines [`Self::emit_cargo_diagnostics`] prints, split out so the severity
    /// mapping is testable without capturing the process's stdout.
    fn cargo_directive_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for diagnostic in &self.diagnostics {
            let directive = match diagnostic.severity {
                Severity::Error => "cargo::error=",
                Severity::Warning => "cargo::warning=",
            };
            let rendered = diagnostic.to_string();
            lines.extend(rendered.lines().map(|line| format!("{directive}{line}")));
        }
        if self.truncated {
            lines.push(format!("cargo::warning={}", self.truncation_notice()));
        }
        lines
    }

    /// The line saying the report stops at `batch_cap`, shared by [`Report`]'s `Display` and
    /// [`Self::emit_cargo_diagnostics`] so the two cannot drift.
    fn truncation_notice(&self) -> String {
        format!(
            "spargen: diagnostic list truncated at batch_cap ({} shown); raise batch_cap to see the rest",
            self.diagnostics.len()
        )
    }

    /// Panic with the rendered report unless the run succeeded. The build-script one-liner —
    /// deliberately not `#[must_use]`, because discarding the returned report is the normal use.
    pub fn expect_success(self) -> Self {
        assert!(self.succeeded(), "{self}");
        self
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "spargen: {}", self.outcome)?;
        for diagnostic in &self.diagnostics {
            writeln!(formatter, "{diagnostic}")?;
        }
        if self.truncated {
            writeln!(formatter, "{}", self.truncation_notice())?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{self, JsonPointer};
    use crate::Code;

    /// Build a report with no diagnostics and the given outcome.
    fn report(outcome: Outcome) -> Report {
        Report {
            diagnostics: Vec::new(),
            outcome,
            truncated: false,
        }
    }

    /// `wrote_output` holds only for a run that rendered the output, which rewrites the file only
    /// when its bytes change. `Cached` is the case that matters: it is a success that rendered
    /// nothing, and a build script keying off it would otherwise re-run work on every warm build.
    #[test]
    fn only_a_generated_outcome_reports_that_output_was_written() {
        assert!(Outcome::Generated.wrote_output());

        for quiet in [Outcome::Cached, Outcome::Clean, Outcome::Rejected] {
            assert!(
                !quiet.wrote_output(),
                "{quiet} must not claim it wrote output"
            );
        }

        // `wrote_output` is narrower than `is_success`: `Cached` and `Clean` are both successes
        // that wrote nothing, so one is never a substitute for the other.
        assert!(Outcome::Cached.is_success() && !Outcome::Cached.wrote_output());
        assert!(Outcome::Clean.is_success() && !Outcome::Clean.wrote_output());
    }

    /// Every outcome, in declaration order.
    const OUTCOMES: [Outcome; 4] = [
        Outcome::Generated,
        Outcome::Cached,
        Outcome::Clean,
        Outcome::Rejected,
    ];

    /// `admits` is the oracle every diagnostic's claim is checked against, so its whole truth
    /// table is pinned: an independent claim holds on every run, a rejection claim only on a
    /// rejected one, and a generation claim on both outcomes that leave a generated module.
    #[test]
    fn every_outcome_admits_exactly_the_claims_true_of_it() {
        for outcome in OUTCOMES {
            assert!(outcome.admits(OutcomeClaim::Independent), "{outcome}");
            assert_eq!(
                outcome.admits(OutcomeClaim::Rejected),
                outcome == Outcome::Rejected,
                "{outcome}"
            );
            assert_eq!(
                outcome.admits(OutcomeClaim::Generated),
                matches!(outcome, Outcome::Generated | Outcome::Cached),
                "{outcome}"
            );
        }
    }

    /// `as_str`, `Display`, and the serialized form (what `--format json` emits) spell each
    /// outcome alike, pinned outcome by outcome.
    #[test]
    fn each_outcome_renders_its_stable_lowercase_name() {
        for (outcome, name) in
            OUTCOMES
                .into_iter()
                .zip(["generated", "cached", "clean", "rejected"])
        {
            assert_eq!(outcome.as_str(), name);
            assert_eq!(outcome.to_string(), name);
            assert_eq!(serde_json::to_value(outcome).unwrap(), name);
        }
    }

    /// `errors` and `warnings` split `diagnostics` by severity with nothing lost or shared, each
    /// in emission order, and `has_errors` follows the error side alone.
    #[test]
    fn errors_and_warnings_partition_the_diagnostics_by_severity() {
        let provenance = diag::Provenance::new(JsonPointer::root(), None);
        let warning = |message: &str| {
            Diagnostic::warning(Code::ServerInitiatedFlowIgnored, provenance.clone())
                .message(message)
                .build()
        };
        let error = |message: &str| {
            Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance.clone())
                .message(message)
                .build()
        };
        let mixed = Report {
            diagnostics: vec![
                warning("first warning"),
                error("first error"),
                warning("second warning"),
                error("second error"),
            ],
            outcome: Outcome::Rejected,
            truncated: false,
        };

        let messages = |diagnostics: Vec<&Diagnostic>| {
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            messages(mixed.errors().collect()),
            ["first error", "second error"]
        );
        assert_eq!(
            messages(mixed.warnings().collect()),
            ["first warning", "second warning"]
        );
        assert_eq!(
            mixed.errors().count() + mixed.warnings().count(),
            mixed.diagnostics().len()
        );
        assert!(mixed.has_errors());

        // Warnings alone are not errors, whatever the outcome.
        let warned = Report {
            diagnostics: vec![warning("only a warning")],
            outcome: Outcome::Generated,
            truncated: false,
        };
        assert!(!warned.has_errors());
        assert_eq!(warned.errors().count(), 0);
        assert_eq!(warned.warnings().count(), 1);
        assert!(!report(Outcome::Rejected).has_errors());
    }

    /// `expect_success` hands a successful report back unchanged.
    #[test]
    fn expect_success_returns_a_successful_report() {
        let returned = report(Outcome::Cached).expect_success();
        assert_eq!(returned.outcome(), Outcome::Cached);
    }

    /// `expect_success` panics with the rendered report, whose first line names the outcome.
    #[test]
    #[should_panic(expected = "spargen: rejected")]
    fn expect_success_panics_on_a_rejected_report() {
        report(Outcome::Rejected).expect_success();
    }

    /// `into_result` is the `?` form of `succeeded`, so it has to split on exactly what `succeeded`
    /// splits on — including an error-severity diagnostic under a non-`Rejected` outcome, which is
    /// the case a bare `outcome` check misses.
    #[test]
    fn into_result_agrees_with_succeeded_and_preserves_the_report() {
        let generated = report(Outcome::Generated);
        let diagnostics = generated.diagnostics().len();
        let recovered = generated.into_result().expect("a clean run is Ok");
        assert_eq!(recovered.outcome(), Outcome::Generated);
        assert_eq!(recovered.diagnostics().len(), diagnostics);

        // A rejection is Err, and the report survives so the caller can render it.
        let rejected = report(Outcome::Rejected)
            .into_result()
            .expect_err("a rejected run is Err");
        assert_eq!(rejected.outcome(), Outcome::Rejected);

        // Errors reported without a rejection still fail: `succeeded` checks both halves.
        let provenance = diag::Provenance::new(JsonPointer::root(), None);
        let with_error = Report {
            diagnostics: vec![
                Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance)
                    .message("an error under a non-rejecting outcome")
                    .build(),
            ],
            outcome: Outcome::Generated,
            truncated: false,
        };
        assert!(!with_error.succeeded());
        assert!(with_error.into_result().is_err());
    }

    /// A rejection announced as a warning looks like the diagnostics that do *not* stop
    /// generation. Each diagnostic goes out at its own severity, and so does every line of it:
    /// Cargo shows a build script's non-directive stdout only under `-vv`, so an unprefixed
    /// location or `help:` line would be invisible in an ordinary build. A truncated report also
    /// says so, as a warning.
    #[test]
    fn cargo_directives_carry_each_diagnostics_own_severity() {
        let span = diag::Span::point(
            diag::FileId(0),
            diag::Loc {
                line: 3,
                col: 5,
                offset: 20,
            },
        );
        let provenance =
            diag::Provenance::new(JsonPointer::root().push("paths").push("/pets"), Some(span));
        let report = Report {
            diagnostics: vec![
                Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance.clone())
                    .message("a rejection")
                    .remedy("fix the rejection")
                    .build(),
                Diagnostic::warning(Code::ServerInitiatedFlowIgnored, provenance)
                    .message("a warning")
                    .remedy("fix the warning")
                    .build(),
            ],
            outcome: Outcome::Rejected,
            truncated: true,
        };

        let lines = report.cargo_directive_lines();
        assert_eq!(
            lines,
            [
                "cargo::error=error[E001]: a rejection",
                "cargo::error=  --> 3:5",
                "cargo::error=  at /paths/~1pets",
                "cargo::error=  help: fix the rejection",
                "cargo::warning=warning[W002]: a warning",
                "cargo::warning=  --> 3:5",
                "cargo::warning=  at /paths/~1pets",
                "cargo::warning=  help: fix the warning",
                "cargo::warning=spargen: diagnostic list truncated at batch_cap (2 shown); raise batch_cap to see the rest",
            ],
            "{lines:#?}"
        );
        for line in &lines {
            assert!(!line.contains('\n'), "a directive is one line: {line:?}");
        }
    }
}
