//! # Subsystem: source
//! layer-deps: diag
//!
//! Input bundles (JSON/YAML, relative-file `$ref` loading) and a span-preserving, event-based
//! parse into a [`SpannedValue`] tree.
//!
//! Parsing preserves file/line/column per node so downstream diagnostics can point at exact
//! source locations. serde is deliberately *not* used for the document tree: it discards
//! spans, and precise diagnostics are non-negotiable — hence the in-house [`SpannedValue`].
//! YAML is parsed through `yaml_rust2`'s event-level parser into the same tree, over the
//! JSON-compatible subset OAS prescribes: aliases expand to their anchored values, and non-string
//! keys and multi-document streams are diagnosed rather than accepted.
//!
//! Remote `$ref`s are covered by four more modules: `lock` (the `spargen.lock` pin file and the
//! vendor-directory layout), `remote` (URL math, `$ref` classification and rewriting), `vendor`
//! (the `remote-fetch`-gated walk behind `spargen lock` that fetches and pins remote documents),
//! and `sha256` (the in-tree digest that pins vendored bytes and fingerprints the build cache).

mod bundle;
mod lock;
mod parse;
mod remote;
mod sha256;
mod value;

// The one `remote-fetch` gate over `vendor`: it covers every item in the module, its tests
// included.
#[cfg(feature = "remote-fetch")]
mod vendor;

// The lock walk's and the build's `$self`, format and `http(s)` readings, held to one table. Gated
// as `vendor` is, since it calls into the lock walk.
#[cfg(all(test, feature = "remote-fetch"))]
mod selfid_diff_tests;

pub(crate) use bundle::InputBundle;
pub(crate) use parse::{parse_json, parse_yaml};
pub(crate) use value::{canonical_pointer, Node, Number, SpannedKey, SpannedMap, SpannedValue};

// Remote-ref helpers for `oas31`: `is_remote_ref` routes remote `$ref`s in `oas31::lower`,
// `rewrite_refs_absolute` serves `oas31::resolve`'s hermetic fragment resolution, and
// `security_requirement_key_reference` reads security-requirement keys in `oas31::lower::security`.
pub(crate) use remote::is_absolute_ref as is_remote_ref;
pub(crate) use remote::rewrite_refs_to_absolute as rewrite_refs_absolute;
pub(crate) use remote::security_requirement_key_reference;
pub(crate) use sha256::sha256_hex;

#[cfg(feature = "remote-fetch")]
pub(crate) use vendor::{vendor, ReqwestFetcher};
#[cfg(feature = "remote-fetch")]
pub use vendor::{VendorReport, VendoredRef};
