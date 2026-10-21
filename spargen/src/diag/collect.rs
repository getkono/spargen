use super::{Diagnostic, FileId};

/// A capped batch of diagnostics collected during one pipeline run.
///
/// Generation collects all diagnostics rather than stopping at the first error (batch
/// reporting); the cap bounds memory under pathological inputs. Once the cap is reached, further
/// diagnostics are dropped and `cap_reached` records that it happened.
///
/// `cap_reached` is what `Report::truncated` reports, so a run that hits `batch_cap` says so
/// instead of presenting a partial list as a complete one.
///
/// `Clone` so a stage that may discard a whole attempt (`oas31::lower`'s passes) can collect into a
/// copy and keep only the attempt it accepts.
#[derive(Debug, Clone)]
pub(crate) struct Diagnostics {
    items: Vec<Diagnostic>,
    cap: usize,
    error_count: usize,
    cap_reached: bool,
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
            cap,
            error_count: 0,
            cap_reached: false,
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

    /// Record a diagnostic. Ignored once the cap is reached, but sets the cap-reached flag.
    ///
    /// A diagnostic with a span and no file is given the path [`Self::register_file`] recorded
    /// for the span's file, when one was.
    pub(crate) fn emit(&mut self, mut diagnostic: Diagnostic) {
        if diagnostic.file.is_none() {
            diagnostic.file = diagnostic
                .span
                .and_then(|span| self.files.get(span.file.0 as usize).cloned().flatten());
        }
        if self
            .items
            .iter()
            .any(|item| same_identity(item, &diagnostic))
        {
            return;
        }

        if matches!(diagnostic.severity, super::Severity::Error) {
            self.error_count += 1;
        }
        if self.items.len() < self.cap {
            self.items.push(diagnostic);
        } else {
            self.cap_reached = true;
        }
    }

    /// Whether the cap was reached and diagnostics past it were dropped.
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

    /// Collapse to `Ok(value)` when no errors were recorded, else `Err(`[`Aborted`]`)`.
    pub(crate) fn result<T>(&self, value: T) -> Result<T, Aborted> {
        if self.has_errors() {
            Err(Aborted)
        } else {
            Ok(value)
        }
    }
}

fn same_identity(left: &Diagnostic, right: &Diagnostic) -> bool {
    left.code == right.code
        && left.pointer == right.pointer
        && left.span == right.span
        && left.message == right.message
        && left.remedy == right.remedy
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
        assert!(!diagnostics.cap_reached);
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
    /// the run never registered, names none and renders as before (#727).
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

    #[test]
    fn duplicate_at_capacity_does_not_report_truncation() {
        let mut diagnostics = Diagnostics::new(1);

        diagnostics.emit(diagnostic("invalid operation"));
        diagnostics.emit(diagnostic("invalid operation"));

        assert!(!diagnostics.cap_reached);

        diagnostics.emit(diagnostic("another failure"));

        assert!(diagnostics.cap_reached);
    }
}
