use serde::Serialize;

/// Identifies a source file within an input bundle (owned by the `source` subsystem).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FileId(pub u32);

/// A position within a source file: 1-based `line` and `col`, plus the 0-based byte `offset`
/// (PRD FR1 span-preserving parse).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Loc {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in Unicode scalar values).
    pub col: u32,
    /// 0-based byte offset from the start of the file.
    pub offset: usize,
}

/// A half-open source span `[start, end)` within a single file, powering `file:line:column`
/// diagnostics (PRD FR6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    /// The file this span lies in.
    pub file: FileId,
    /// Inclusive start position.
    pub start: Loc,
    /// Exclusive end position.
    pub end: Loc,
}

impl Span {
    /// A zero-width span at a single position in `file`.
    pub fn point(file: FileId, at: Loc) -> Self {
        todo!()
    }
}
