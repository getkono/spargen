use super::{JsonPointer, Span};

/// Where a construct came from: its JSON Pointer and, when known, its source [`Span`].
///
/// Provenance is attached to every IR node so that a diagnostic raised after lowering — the IR
/// invariant check's `E011` — can still point back at the exact spec construct and source
/// location. It is also where [`super::Diagnostic::error`] and [`super::Diagnostic::warning`] place
/// the diagnostic they begin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// RFC 6901 pointer to the construct.
    pub(crate) pointer: JsonPointer,
    /// Source span, when available.
    pub(crate) span: Option<Span>,
}

impl Provenance {
    /// Construct provenance from a pointer and optional span.
    pub fn new(pointer: JsonPointer, span: Option<Span>) -> Self {
        Self { pointer, span }
    }
}
