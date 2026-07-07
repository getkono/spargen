//! # Subsystem: support
//! layer-deps:
//!
//! The generator-side handle to the freestanding runtime shipped inside generated output. The
//! runtime itself is real, standalone-compilable source in the `support-runtime` workspace
//! member; this module embeds it verbatim (`include_str!`) and exposes the FR5 error-taxonomy
//! metadata as data (PRD §2.3 rule 3, §2.1).
