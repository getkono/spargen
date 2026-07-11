use super::{JsonPointer, Span};

/// Where a construct came from: its JSON Pointer and, when known, its source [`Span`].
///
/// Provenance is attached to every IR node so that any diagnostic — even one raised late, in
/// codegen — can still point back at the exact spec construct and source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// RFC 6901 pointer to the construct.
    pub pointer: JsonPointer,
    /// Source span, when available.
    pub span: Option<Span>,
}

impl Provenance {
    /// Construct provenance from a pointer and optional span.
    pub fn new(pointer: JsonPointer, span: Option<Span>) -> Self {
        Self { pointer, span }
    }
}
