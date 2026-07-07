//! # Subsystem: oas31
//! layer-deps: source, ir, diag
//!
//! The OAS 3.1.1 typed document model, structural/meta-schema validation, `$ref` resolution,
//! per-keyword disposition audit, and lowering `SpannedValue` → IR. The only subsystem that
//! knows OpenAPI 3.1 syntax (PRD §2.3 rule 1).
