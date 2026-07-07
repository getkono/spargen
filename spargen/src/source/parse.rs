use crate::diag::{Aborted, Diagnostics, FileId};

use super::SpannedValue;

/// Parse a JSON document into a span-preserving [`SpannedValue`] tree (PRD FR1, D4).
///
/// Malformed input is reported through `diags` (with spans) rather than by panic; a fatal parse
/// error returns [`Aborted`]. The parser is event-level so it can attach a span to every node.
pub fn parse_json(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    todo!()
}

/// Parse a YAML 1.2 document into a span-preserving [`SpannedValue`] tree.
///
/// YAML is restricted to the JSON-compatible subset OAS 3.1 prescribes (PRD §3.3 prec 5);
/// constructs outside that subset are diagnosed. Errors are reported through `diags`.
pub fn parse_yaml(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    todo!()
}
