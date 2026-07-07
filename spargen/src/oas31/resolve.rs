use crate::diag::{Aborted, Diagnostics, JsonPointer};
use crate::source::InputBundle;

use super::{Document, Schema};

/// Resolves `$ref`s within a [`Document`] and its input bundle (PRD §3.3 prec 6/7). Detects cycles
/// so the frontend can break them with `Box` in the IR (matrix: Schema shape → `$ref` cycles).
#[derive(Debug)]
pub struct Resolver<'doc> {
    document: &'doc Document,
    bundle: &'doc InputBundle,
}

/// A resolved reference target: the schema it points at and the pointer that addresses it.
#[derive(Debug)]
pub struct Resolved<'doc> {
    /// The target schema.
    pub schema: &'doc Schema,
    /// The pointer to the target within its file.
    pub pointer: JsonPointer,
}

impl<'doc> Resolver<'doc> {
    /// Build a resolver over a document and its bundle.
    pub fn new(document: &'doc Document, bundle: &'doc InputBundle) -> Self {
        todo!()
    }

    /// Resolve a `$ref` string that appears at `at`, reporting an unresolved/absolute ref through
    /// `diags` (PRD §3.2.10).
    pub fn resolve(
        &self,
        reference: &str,
        at: &JsonPointer,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        todo!()
    }

    /// Whether resolving `reference` participates in a reference cycle (→ `Box` in the IR).
    pub fn is_cyclic(&self, reference: &str) -> bool {
        todo!()
    }
}
