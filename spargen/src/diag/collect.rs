use std::collections::HashSet;

use super::{Code, Diagnostic, FileId, JsonPointer, Span};

/// A capped batch of diagnostics collected during one pipeline run.
///
/// Generation collects all diagnostics rather than stopping at the first error (batch
/// reporting); the cap bounds memory under pathological inputs. Once the cap is reached, further
/// warnings are dropped and `cap_reached` records that it happened; a further error evicts the
/// most recent retained warning instead, so a capped batch keeps its errors up to the cap.
///
/// `cap_reached` is what `Report::truncated` reports, so a run that hits `batch_cap` says so
/// instead of presenting a partial list as a complete one.
///
/// `Clone` so a stage that may discard a whole attempt (`oas31::lower`'s passes) can collect into a
/// copy and keep only the attempt it accepts.
#[derive(Debug, Clone)]
pub(crate) struct Diagnostics {
    items: Vec<Diagnostic>,
    /// The [`Identity`] of each diagnostic in `items`, so [`Self::emit`] finds a duplicate in
    /// expected constant time rather than by comparing it with every diagnostic already held.
    /// Only membership is read, never iteration order, so output stays deterministic.
    identities: HashSet<Identity>,
    cap: usize,
    error_count: usize,
    cap_reached: bool,
    /// Whether an error was dropped at the cap rather than retained: only when every retained
    /// diagnostic is already an error, or the cap is zero.
    error_dropped: bool,
    /// The path of each file the run loaded, indexed by its [`FileId`]: what names the file a
    /// recorded diagnostic's span lies in.
    files: Vec<Option<String>>,
}

/// A fatal-outcome marker returned when a pipeline stage recorded an error and cannot continue.
/// The diagnostics themselves live in the [`Diagnostics`] batch; this only signals control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Aborted;

impl Diagnostics {
    /// Create a collector retaining at most `cap` diagnostics.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            identities: HashSet::new(),
            cap,
            error_count: 0,
            cap_reached: false,
            error_dropped: false,
            files: Vec::new(),
        }
    }

    /// Record that `file` was loaded from `path`, so every diagnostic recorded from here on whose
    /// span lies in `file` names that path. The source layer calls this as it assigns the id,
    /// before parsing, so a parse diagnostic names its file too.
    pub(crate) fn register_file(&mut self, file: FileId, path: impl Into<String>) {
        let index = file.0 as usize;
        if self.files.len() <= index {
            self.files.resize(index + 1, None);
        }
        self.files[index] = Some(path.into());
    }

    /// Record a diagnostic.
    ///
    /// A diagnostic identical to one already recorded (same code, pointer, span, message, and
    /// remedy) is ignored. Past the cap, a warning is dropped and sets the cap-reached flag. An
    /// error past the cap evicts the most recent retained warning and takes its place; where
    /// every retained diagnostic is already an error (or the cap is zero), it is dropped, and
    /// [`Self::into_parts`] reports that an error went unretained. Either way it counts toward
    /// [`Self::has_errors`].
    ///
    /// A diagnostic with a span and no file is given the path [`Self::register_file`] recorded
    /// for the span's file, when one was.
    pub(crate) fn emit(&mut self, mut diagnostic: Diagnostic) {
        if diagnostic.file.is_none() {
            diagnostic.file = diagnostic
                .span
                .and_then(|span| self.files.get(span.file.0 as usize).cloned().flatten());
        }
        let identity = Identity::of(&diagnostic);
        if self.identities.contains(&identity) {
            return;
        }

        let is_error = matches!(diagnostic.severity, super::Severity::Error);
        if is_error {
            self.error_count += 1;
        }
        if self.items.len() < self.cap {
            self.identities.insert(identity);
            self.items.push(diagnostic);
            return;
        }
        self.cap_reached = true;
        if !is_error {
            return;
        }
        // An error makes room by evicting the most recent retained warning, so a run the error
        // rejects still lists an error however many warnings came before it.
        let evicted = self
            .items
            .iter()
            .rposition(|item| matches!(item.severity, super::Severity::Warning));
        match evicted {
            Some(index) => {
                let warning = self.items.remove(index);
                self.identities.remove(&Identity::of(&warning));
                self.identities.insert(identity);
                self.items.push(diagnostic);
            }
            None => self.error_dropped = true,
        }
    }

    /// Whether the cap was reached and diagnostics past it were dropped. A run reads it through
    /// [`Self::into_parts`]; this is the tests' view of a batch still being collected.
    #[cfg(test)]
    pub(crate) fn cap_reached(&self) -> bool {
        self.cap_reached
    }

    /// Whether any error-severity diagnostic has been recorded.
    pub(crate) fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// The collected diagnostics, in emission order.
    pub(crate) fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    /// The collected diagnostics, in emission order; whether the cap was reached and
    /// diagnostics past it were dropped; and whether an error was among the dropped ones.
    pub(crate) fn into_parts(self) -> (Vec<Diagnostic>, bool, bool) {
        (self.items, self.cap_reached, self.error_dropped)
    }

    /// Collapse to `Ok(value)` when no errors were recorded, else `Err(`[`Aborted`]`)`.
    pub(crate) fn result<T>(&self, value: T) -> Result<T, Aborted> {
        if self.has_errors() {
            Err(Aborted)
        } else {
            Ok(value)
        }
    }
}

/// What makes two diagnostics the same one for [`Diagnostics::emit`]: the code, pointer, span,
/// message, and remedy, each compared by its derived `Eq` and hashed by its derived `Hash`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Identity {
    code: Code,
    pointer: JsonPointer,
    span: Option<Span>,
    message: String,
    remedy: Option<String>,
}

impl Identity {
    fn of(diagnostic: &Diagnostic) -> Self {
        Self {
            code: diagnostic.code,
            pointer: diagnostic.pointer.clone(),
            span: diagnostic.span,
            message: diagnostic.message.clone(),
            remedy: diagnostic.remedy.clone(),
        }
    }
}

impl Default for Diagnostics {
    /// A collector with an unbounded (`usize::MAX`) cap.
    fn default() -> Self {
        Self::new(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{Code, InterpId, JsonPointer, Provenance};

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic::error(
            Code::InvalidInput,
            Provenance::new(JsonPointer::from("/paths/~1repos"), None),
        )
        .message(message)
        .remedy("repair the input")
        .build()
    }

    #[test]
    fn identical_diagnostics_are_retained_once() {
        let mut diagnostics = Diagnostics::default();

        diagnostics.emit(diagnostic("invalid operation"));
        diagnostics.emit(diagnostic("invalid operation"));

        assert_eq!(diagnostics.items().len(), 1);
        assert!(diagnostics.has_errors());
        assert!(!diagnostics.cap_reached());
    }

    #[test]
    fn interpretation_does_not_distinguish_diagnostics() {
        let mut diagnostics = Diagnostics::default();
        let first = diagnostic("invalid operation");
        let mut duplicate = first.clone();
        duplicate.interpretation = Some(InterpId(999));

        diagnostics.emit(first);
        diagnostics.emit(duplicate);

        assert_eq!(diagnostics.items().len(), 1);
    }

    #[test]
    fn every_identity_field_distinguishes_diagnostics() {
        let base = diagnostic("invalid operation");
        let mut variants = Vec::new();

        let mut code = base.clone();
        code.code = Code::UnsupportedDialect;
        variants.push(code);

        let mut pointer = base.clone();
        pointer.pointer = JsonPointer::from("/paths/~1issues");
        variants.push(pointer);

        let mut span = base.clone();
        span.span = Some(crate::diag::Span::point(
            crate::diag::FileId(0),
            crate::diag::Loc {
                line: 1,
                col: 1,
                offset: 0,
            },
        ));
        variants.push(span);

        let mut message = base.clone();
        message.message = "another failure".to_owned();
        variants.push(message);

        let mut remedy = base.clone();
        remedy.remedy = Some("use another repair".to_owned());
        variants.push(remedy);

        let mut diagnostics = Diagnostics::default();
        diagnostics.emit(base);
        for variant in variants {
            diagnostics.emit(variant);
        }

        assert_eq!(diagnostics.items().len(), 6);
    }

    /// A diagnostic names the file its span lies in once that file is registered, and the
    /// rendering puts the path before the line and column; one with no span, or a span in a file
    /// the run never registered, names none and renders only the line and column (#727).
    #[test]
    fn a_spanned_diagnostic_names_the_registered_file_its_span_lies_in() {
        let at = |file: u32, line: u32| {
            Provenance::new(
                JsonPointer::from("/Pet"),
                Some(crate::diag::Span::point(
                    crate::diag::FileId(file),
                    crate::diag::Loc {
                        line,
                        col: 5,
                        offset: 0,
                    },
                )),
            )
        };
        let mut diagnostics = Diagnostics::default();
        diagnostics.register_file(crate::diag::FileId(0), "api/openapi.yaml");
        diagnostics.register_file(crate::diag::FileId(1), "api/schemas.yaml");

        Diagnostic::error(Code::InvalidInput, at(1, 12))
            .message("in the referenced file")
            .emit(&mut diagnostics);
        Diagnostic::error(Code::InvalidInput, at(0, 3))
            .message("in the root")
            .emit(&mut diagnostics);
        Diagnostic::error(Code::InvalidInput, at(7, 2))
            .message("in a file never registered")
            .emit(&mut diagnostics);
        diagnostics.emit(diagnostic("with no span"));

        let files: Vec<Option<&str>> = diagnostics
            .items()
            .iter()
            .map(|item| item.file.as_deref())
            .collect();
        assert_eq!(
            files,
            [
                Some("api/schemas.yaml"),
                Some("api/openapi.yaml"),
                None,
                None
            ]
        );
        let locations: Vec<Option<String>> = diagnostics
            .items()
            .iter()
            .map(|item| {
                item.to_string()
                    .lines()
                    .find_map(|line| line.trim_start().strip_prefix("--> ").map(str::to_owned))
            })
            .collect();
        assert_eq!(
            locations,
            [
                Some("api/schemas.yaml:12:5".to_owned()),
                Some("api/openapi.yaml:3:5".to_owned()),
                Some("2:5".to_owned()),
                None
            ]
        );
    }

    /// The identity index holds only what the batch retains: a diagnostic dropped past the cap is
    /// not indexed, so the index grows no larger than the cap.
    #[test]
    fn a_diagnostic_dropped_past_the_cap_is_not_indexed() {
        let mut diagnostics = Diagnostics::new(1);
        diagnostics.emit(diagnostic("kept"));
        diagnostics.emit(diagnostic("dropped"));
        diagnostics.emit(diagnostic("dropped"));

        assert_eq!(diagnostics.items().len(), 1);
        assert_eq!(diagnostics.identities.len(), 1);
        assert!(diagnostics.cap_reached());
    }

    /// An error past the cap evicts the most recent retained warning, unindexing it, so the
    /// batch keeps its errors; with no warning left to evict, the error is dropped and reported
    /// as such, and a warning past the cap is dropped either way (#1345).
    #[test]
    fn an_error_past_the_cap_evicts_the_most_recent_warning() {
        let warning = |message: &str| {
            Diagnostic::warning(
                Code::ServerInitiatedFlowIgnored,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(message)
            .build()
        };
        let mut diagnostics = Diagnostics::new(3);
        diagnostics.emit(warning("first warning"));
        diagnostics.emit(warning("second warning"));
        diagnostics.emit(warning("third warning"));
        diagnostics.emit(diagnostic("first error"));
        diagnostics.emit(warning("late warning"));
        diagnostics.emit(diagnostic("second error"));

        let messages = |diagnostics: &Diagnostics| {
            diagnostics
                .items()
                .iter()
                .map(|item| item.message.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            messages(&diagnostics),
            ["first warning", "first error", "second error"]
        );
        assert_eq!(diagnostics.identities.len(), 3);
        assert!(diagnostics.cap_reached());

        // An evicted warning is no longer indexed, so it is not mistaken for a duplicate; past
        // the cap it is dropped like any other warning.
        diagnostics.emit(warning("third warning"));
        assert_eq!(diagnostics.items().len(), 3);

        diagnostics.emit(diagnostic("fourth error"));
        assert_eq!(
            messages(&diagnostics),
            ["first error", "second error", "fourth error"]
        );
        assert!(!diagnostics.clone().into_parts().2);

        diagnostics.emit(diagnostic("fifth error"));
        let (items, truncated, error_dropped) = diagnostics.into_parts();
        assert_eq!(items.len(), 3);
        assert!(truncated);
        assert!(error_dropped);
    }

    /// A zero cap retains nothing, and an error it drops is reported as dropped.
    #[test]
    fn a_zero_cap_reports_a_dropped_error() {
        let mut diagnostics = Diagnostics::new(0);
        diagnostics.emit(diagnostic("dropped"));
        assert!(diagnostics.has_errors());
        let (items, truncated, error_dropped) = diagnostics.into_parts();
        assert!(items.is_empty());
        assert!(truncated);
        assert!(error_dropped);
    }

    /// A duplicate of any earlier diagnostic in a large batch is found, not only of the latest,
    /// and the index stays the same size as the retained items.
    #[test]
    fn a_duplicate_anywhere_in_a_large_batch_is_retained_once() {
        let mut diagnostics = Diagnostics::default();
        for index in 0..500 {
            diagnostics.emit(diagnostic(&format!("failure {index}")));
        }
        for index in (0..500).rev() {
            diagnostics.emit(diagnostic(&format!("failure {index}")));
        }

        assert_eq!(diagnostics.items().len(), 500);
        assert_eq!(diagnostics.identities.len(), 500);
        assert_eq!(diagnostics.items()[0].message, "failure 0");
        assert_eq!(diagnostics.items()[499].message, "failure 499");
    }

    /// Read through the `cap_reached` accessor while the batch is collected, and at the end
    /// through `into_parts`, which is what `Report::truncated` reports, so the assertions observe
    /// the methods and not only the field they return.
    #[test]
    fn duplicate_at_capacity_does_not_report_truncation() {
        let mut diagnostics = Diagnostics::new(1);
        assert!(!diagnostics.cap_reached());

        diagnostics.emit(diagnostic("invalid operation"));
        diagnostics.emit(diagnostic("invalid operation"));

        assert!(!diagnostics.cap_reached());
        assert!(!diagnostics.clone().into_parts().1);

        diagnostics.emit(diagnostic("another failure"));

        assert!(diagnostics.cap_reached());
        assert_eq!(diagnostics.items().len(), 1);
        let (items, truncated, _) = diagnostics.into_parts();
        assert!(truncated);
        assert_eq!(
            items
                .iter()
                .map(|item| item.message.as_str())
                .collect::<Vec<_>>(),
            ["invalid operation"]
        );
    }
}
