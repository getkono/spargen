use crate::diag::Diagnostics;
use crate::source::SpannedValue;

/// Structural validator against the vendored OAS 3.1 base dialect and meta-schemas (PRD §3.3
/// prec 2, §7.5). Targets a fixed, checksummed in-repo artifact under `spec/`, never a live URL.
#[derive(Debug, Default)]
pub struct MetaSchemaValidator {
    // Parsed vendored meta-schemas, populated by `load_vendored`.
    loaded: bool,
}

impl MetaSchemaValidator {
    /// Load and parse the vendored meta-schemas from `spec/`.
    pub fn load_vendored() -> Self {
        todo!()
    }

    /// Validate a raw document tree against the meta-schemas, reporting violations through `diags`
    /// (with pointer + span).
    pub fn validate(&self, document: &SpannedValue, diags: &mut Diagnostics) {
        todo!()
    }
}
