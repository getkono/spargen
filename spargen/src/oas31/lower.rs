use crate::diag::{Aborted, Diagnostics};
use crate::ir::Api;

use super::{Document, Resolver};

/// Lower the typed 3.1.1 [`Document`] into the version-agnostic [`Api`] IR (PRD §2.3 rule 1).
///
/// This is where the design decisions execute and emit diagnostics: numeric mappings (D5),
/// homogeneous scalar enums (D6), deserialization defaults (D8), `format` type mappings (§6.2),
/// disjoint-`oneOf` vs discriminator vs reject (D1), and `$ref` cycle boxing. Codegen never sees a
/// spec document; this function is the only bridge from OAS to IR.
pub fn lower(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Result<Api, Aborted> {
    todo!()
}
