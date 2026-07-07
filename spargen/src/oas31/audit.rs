use crate::diag::{Code, Diagnostics, Disposition, JsonPointer};

use super::{Document, Resolver};

/// The per-keyword S/W/R disposition audit — the core of `spargen check` and the product's central
/// debuggability artifact (PRD FR2, FR6). Every OpenAPI/JSON-Schema construct in the document gets
/// exactly one disposition; W and R constructs also emit a diagnostic through `diags`.
pub fn audit(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> DispositionReport {
    todo!()
}

/// The result of an [`audit`]: one entry per classified construct.
#[derive(Debug, Clone, Default)]
pub struct DispositionReport {
    /// The classified constructs, in document order.
    pub entries: Vec<DispositionEntry>,
}

/// One construct's disposition.
#[derive(Debug, Clone)]
pub struct DispositionEntry {
    /// Pointer to the construct.
    pub pointer: JsonPointer,
    /// Its S/W/R class.
    pub disposition: Disposition,
    /// The diagnostic code, for W and R constructs.
    pub code: Option<Code>,
}
