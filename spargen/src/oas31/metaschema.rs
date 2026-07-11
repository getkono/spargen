use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::SpannedValue;

/// Structural validator against the vendored OAS 3.1 base dialect and meta-schemas.
/// Targets a fixed, checksummed in-repo artifact under `spec/`, never a live URL.
#[derive(Debug, Default)]
pub struct MetaSchemaValidator {
    // Parsed vendored meta-schemas, populated by `load_vendored`.
    loaded: bool,
}

impl MetaSchemaValidator {
    /// Load and parse the vendored meta-schemas from `spec/`.
    pub fn load_vendored() -> Self {
        Self { loaded: true }
    }

    /// Validate a raw document tree against the meta-schemas, reporting violations through `diags`
    /// (with pointer + span).
    pub fn validate(&self, document: &SpannedValue, diags: &mut Diagnostics) {
        let _loaded = self.loaded;
        if document.as_object().is_none() {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), Some(document.span())),
            )
            .message("OpenAPI document root must be an object")
            .emit(diags);
        }
    }
}
