use serde::Serialize;

/// Identifies a source file within an input bundle (owned by the `source` subsystem).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FileId(pub(crate) u32);

impl FileId {
    /// The file's index in its bundle, in load order: the root document, which a bundle loads
    /// first, is `0`.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// A position within a source file: 1-based `line` and `col`, plus the 0-based byte `offset`
/// (span-preserving parse).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub struct Loc {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in Unicode scalar values).
    pub col: u32,
    /// 0-based byte offset from the start of the file, or from just after the leading UTF-8
    /// byte-order mark of a file that begins with one.
    pub offset: usize,
}

impl Loc {
    /// The position at 1-based `line` and `col`, `offset` bytes from the start of the file.
    pub fn new(line: u32, col: u32, offset: usize) -> Self {
        Self { line, col, offset }
    }
}

/// A half-open source span `[start, end)`: line and column positions within the file its
/// [`FileId`] names. A diagnostic renders its start as `line:column`, prefixed by the file's path
/// when the run loaded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub struct Span {
    /// The file this span lies in.
    pub file: FileId,
    /// Inclusive start position.
    pub start: Loc,
    /// Exclusive end position.
    pub end: Loc,
}

impl Span {
    /// The span from `start` (inclusive) to `end` (exclusive) in `file`.
    pub fn new(file: FileId, start: Loc, end: Loc) -> Self {
        Self { file, start, end }
    }

    /// A zero-width span at a single position in `file`.
    pub(crate) fn point(file: FileId, at: Loc) -> Self {
        Self {
            file,
            start: at,
            end: at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileId, Loc, Span};

    #[test]
    fn a_file_id_reads_back_the_index_it_wraps() {
        for index in [0, 1, 2, 7, u32::MAX] {
            assert_eq!(FileId(index).get(), index);
        }
    }

    /// Each constructor puts every argument in the field of its name, in the order taken.
    #[test]
    fn the_constructors_fill_the_fields_they_name() {
        let start = Loc::new(3, 5, 17);
        assert_eq!((start.line, start.col, start.offset), (3, 5, 17));
        let end = Loc::new(4, 1, 30);
        let span = Span::new(FileId(2), start, end);
        assert_eq!((span.file, span.start, span.end), (FileId(2), start, end));
    }
}
