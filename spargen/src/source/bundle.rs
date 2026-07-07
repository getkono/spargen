use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;

use crate::diag::{Aborted, Diagnostics, FileId, JsonPointer, SourceSnippets};

use super::SpannedValue;

/// A single loaded source file: its id, path, and full text. The text is shared (`Arc<str>`) so
/// rustc-style snippet rendering can borrow lines cheaply.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Bundle-unique file id.
    pub id: FileId,
    /// Path as loaded (relative to the bundle root).
    pub path: Utf8PathBuf,
    /// Full file contents.
    pub text: Arc<str>,
}

impl SourceFile {
    /// The text of the 1-based `line`, if present.
    pub fn line_text(&self, line: u32) -> Option<&str> {
        todo!()
    }
}

/// An input bundle: the root document plus every local file reachable through relative-file
/// `$ref`s (PRD FR1). Absolute-URL `$ref`s are rejected (§3.2.10); network fetches never happen —
/// builds are hermetic (§3.2.10).
#[derive(Debug, Default)]
pub struct InputBundle {
    root: Option<FileId>,
    files: IndexMap<FileId, SourceFile>,
    values: IndexMap<FileId, SpannedValue>,
}

impl InputBundle {
    /// Load the root document at `root`, pulling in referenced local files on demand. The parse
    /// format is chosen by extension (`.json` vs `.yaml`/`.yml`). Diagnostics flow through `diags`.
    pub fn load(root: &Utf8Path, diags: &mut Diagnostics) -> Result<InputBundle, Aborted> {
        todo!()
    }

    /// The root document's value tree.
    pub fn root(&self) -> &SpannedValue {
        todo!()
    }

    /// The value tree of a loaded `file`.
    pub fn value_at(&self, file: FileId) -> &SpannedValue {
        todo!()
    }

    /// The loaded record for `file`, if present.
    pub fn file(&self, file: FileId) -> Option<&SourceFile> {
        todo!()
    }

    /// Resolve a `$ref` originating in `base`, loading the target file if it is not yet in the
    /// bundle. Absolute-URL refs are rejected with a diagnostic (PRD §3.2.10). On success returns
    /// the target file id and the JSON Pointer within it.
    pub fn resolve_ref(
        &mut self,
        base: FileId,
        reference: &str,
        diags: &mut Diagnostics,
    ) -> Result<(FileId, JsonPointer), Aborted> {
        todo!()
    }
}

impl SourceSnippets for InputBundle {
    fn line_text(&self, file: FileId, line: u32) -> Option<&str> {
        todo!()
    }

    fn path(&self, file: FileId) -> Option<&Utf8Path> {
        todo!()
    }
}
