//! # Subsystem: codegen
//! layer-deps: ir, name, support, diag
//!
//! IR + allocated names → Rust tokens: models, client, embedded `support`; deterministic item
//! ordering; `prettyplease` formatting (PRD §2.3, FR3, NFR2).
