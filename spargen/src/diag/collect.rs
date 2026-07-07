use super::Diagnostic;

/// A capped batch of diagnostics collected during one pipeline run.
///
/// Generation collects all diagnostics rather than stopping at the first error (PRD FR6 batch
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
    /// Create a collector retaining at most `cap` diagnostics (PRD FR6).
    pub fn new(cap: usize) -> Self {
        todo!()
    }

    /// Record a diagnostic. Ignored once the cap is reached, but sets the cap-reached flag.
    pub fn emit(&mut self, diagnostic: Diagnostic) {
        todo!()
    }

    /// Whether any error-severity diagnostic has been recorded.
    pub fn has_errors(&self) -> bool {
        todo!()
    }

    /// Whether the retention cap has been hit and diagnostics dropped.
    pub fn cap_reached(&self) -> bool {
        todo!()
    }

    /// The collected diagnostics, in emission order.
    pub fn items(&self) -> &[Diagnostic] {
        todo!()
    }

    /// Collapse to `Ok(value)` when no errors were recorded, else `Err(`[`Aborted`]`)`.
    pub fn into_result<T>(&self, value: T) -> Result<T, Aborted> {
        todo!()
    }
}

impl Default for Diagnostics {
    /// A collector with an unbounded (`usize::MAX`) cap.
    fn default() -> Self {
        todo!()
    }
}
