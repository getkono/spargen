use crate::diag::{Aborted, Diagnostics};
use crate::source::InputBundle;

use super::Document;

/// Build the typed [`Document`] from a loaded [`InputBundle`], carrying spans through.
///
/// Enforces the FR1 input gates: the `openapi` field must match `3.1.*` (a 3.0.x input is rejected
/// with a dedicated diagnostic that explains the dialect difference and does **not** offer
/// conversion, §3.2.1); a present `jsonSchemaDialect` must be the default OAS 3.1 dialect (§3.3).
/// All problems are reported through `diags`; a fatal one returns [`Aborted`].
pub fn parse_document(bundle: &InputBundle, diags: &mut Diagnostics) -> Result<Document, Aborted> {
    todo!()
}
