use camino::Utf8Path;

use super::{Diagnostic, Diagnostics, FileId};

/// Access to source text so the human renderer can show rustc-style code snippets.
///
/// Implemented by the `source` subsystem. Keeping it a trait here inverts the dependency: `diag`
/// stays at the bottom of the layering DAG and never depends on `source`.
pub trait SourceSnippets {
    /// The text of the 1-based `line` in `file`, if available.
    fn line_text(&self, file: FileId, line: u32) -> Option<&str>;

    /// The path of `file`, for the `file:line:column` location line.
    fn path(&self, file: FileId) -> Option<&Utf8Path>;
}

/// Render one diagnostic rustc-style (code, message, location, source snippet with caret, and
/// remedy) into `out` (PRD FR6).
pub fn render_human(
    diagnostic: &Diagnostic,
    source: &dyn SourceSnippets,
    out: &mut dyn std::fmt::Write,
) -> std::fmt::Result {
    todo!()
}

/// Render a batch as a stable JSON structure for CI consumption (`--format json`, PRD FR6). The
/// shape is product surface and schema-tested.
pub fn render_json(diagnostics: &Diagnostics) -> serde_json::Value {
    todo!()
}
