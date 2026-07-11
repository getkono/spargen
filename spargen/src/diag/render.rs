use camino::Utf8Path;
use serde_json::json;

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
/// remedy) into `out`.
pub fn render_human(
    diagnostic: &Diagnostic,
    source: &dyn SourceSnippets,
    out: &mut dyn std::fmt::Write,
) -> std::fmt::Result {
    let severity = match diagnostic.severity {
        super::Severity::Error => "error",
        super::Severity::Warning => "warning",
    };
    writeln!(
        out,
        "{severity}[{}]: {}",
        diagnostic.code, diagnostic.message
    )?;
    if let Some(span) = diagnostic.span {
        if let Some(path) = source.path(span.file) {
            writeln!(out, "  --> {}:{}:{}", path, span.start.line, span.start.col)?;
            if let Some(line) = source.line_text(span.file, span.start.line) {
                writeln!(out, "   |")?;
                writeln!(out, "{:>3} | {}", span.start.line, line)?;
                let caret_col = span.start.col.max(1) as usize;
                let width = span.end.col.saturating_sub(span.start.col).max(1) as usize;
                writeln!(
                    out,
                    "   | {}{}",
                    " ".repeat(caret_col.saturating_sub(1)),
                    "^".repeat(width)
                )?;
            }
        }
    }
    writeln!(out, "  pointer: {}", diagnostic.pointer)?;
    if let Some(id) = diagnostic.interpretation {
        writeln!(out, "  note: {id}")?;
    }
    if let Some(remedy) = &diagnostic.remedy {
        writeln!(out, "  help: {remedy}")?;
    }
    Ok(())
}

/// Render a batch as a stable JSON structure for CI consumption (`--format json`). The
/// shape is product surface and schema-tested.
pub fn render_json(diagnostics: &Diagnostics) -> serde_json::Value {
    let items = diagnostics
        .items()
        .iter()
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code.as_str(),
                "severity": diagnostic.severity,
                "pointer": diagnostic.pointer,
                "span": diagnostic.span,
                "message": diagnostic.message,
                "remedy": diagnostic.remedy,
                "interpretation": diagnostic.interpretation.map(|id| id.to_string()),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "diagnostics": items,
        "has_errors": diagnostics.has_errors(),
        "cap_reached": diagnostics.cap_reached(),
    })
}
