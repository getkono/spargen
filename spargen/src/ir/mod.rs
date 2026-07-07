//! # Subsystem: ir
//! layer-deps: diag
//!
//! The version-agnostic API model: operation set, type graph, auth requirements, media map;
//! provenance (pointer + span) on every node; well-formedness invariants. The IR is the
//! coupling firewall and primary extension seam — it never sees a spec document or Rust tokens
//! (PRD §2.3 rule 1, FR2/FR3).
