use super::Diagnostic;

/// A capped batch of diagnostics collected during one pipeline run.
///
/// Generation collects all diagnostics rather than stopping at the first error (batch
/// reporting); the cap bounds memory under pathological inputs. Once the cap is reached, further
/// diagnostics are dropped but [`cap_reached`](Diagnostics::cap_reached) is set so the renderer
/// can note the truncation.
#[derive(Debug)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    cap: usize,
    error_count: usize,
    cap_reached: bool,
}

/// A fatal-outcome marker returned when a pipeline stage recorded an error and cannot continue.
/// The diagnostics themselves live in the [`Diagnostics`] batch; this only signals control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aborted;

impl Diagnostics {
    /// Create a collector retaining at most `cap` diagnostics.
    pub fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            cap,
            error_count: 0,
            cap_reached: false,
        }
    }

    /// Record a diagnostic. Ignored once the cap is reached, but sets the cap-reached flag.
    pub fn emit(&mut self, diagnostic: Diagnostic) {
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

    /// Whether any error-severity diagnostic has been recorded.
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// Whether the retention cap has been hit and diagnostics dropped.
    pub fn cap_reached(&self) -> bool {
        self.cap_reached
    }

    /// The collected diagnostics, in emission order.
    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Collapse to `Ok(value)` when no errors were recorded, else `Err(`[`Aborted`]`)`.
    pub fn into_result<T>(&self, value: T) -> Result<T, Aborted> {
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

    #[test]
    fn duplicate_at_capacity_does_not_report_truncation() {
        let mut diagnostics = Diagnostics::new(1);

        diagnostics.emit(diagnostic("invalid operation"));
        diagnostics.emit(diagnostic("invalid operation"));

        assert!(!diagnostics.cap_reached());

        diagnostics.emit(diagnostic("another failure"));

        assert!(diagnostics.cap_reached());
    }
}
