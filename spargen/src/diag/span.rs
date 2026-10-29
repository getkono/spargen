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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Loc {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in Unicode scalar values).
    pub col: u32,
    /// 0-based byte offset from the start of the file.
    pub offset: usize,
}

/// A half-open source span `[start, end)`: line and column positions within the file its
/// [`FileId`] names. A diagnostic renders its start as `line:column`, prefixed by the file's path
/// when the run loaded it.
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
    use super::FileId;

    #[test]
    fn get_returns_the_index_the_id_was_built_from() {
        for index in [0, 1, 2, 7, u32::MAX] {
            assert_eq!(FileId(index).get(), index);
        }
        assert_ne!(FileId(2).get(), FileId(3).get());
    }
}
