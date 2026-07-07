//! # support-runtime
//!
//! The freestanding runtime support code shipped *inside* every spargen-generated client: the
//! dispatch routines, the FR5 error taxonomy, `ResponseValue<T>`, and auth plumbing. It is
//! real, standalone-compilable source (compiled and linted here in its own right, PRD §7.5)
//! that the `codegen` subsystem embeds verbatim via `include_str!` (PRD §2.3 rule 3).
//!
//! No spargen crate ever appears in a consumer's runtime graph: this crate is `publish = false`
//! and its only dependencies are the near-universal `reqwest` / `serde` / `serde_json` / `bytes`
//! set (PRD §2.1). The public surface is filled in the support subsystem commit.

#![forbid(unsafe_code)]
// TODO(impl): remove once runtime bodies are implemented — stub signatures leave params unused.
#![allow(unused_variables)]
