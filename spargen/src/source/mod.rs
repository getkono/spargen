//! # Subsystem: source
//! layer-deps: diag
//!
//! Input bundles (JSON/YAML, relative-file `$ref` loading) and a span-preserving, event-based
//! parse into a [`SpannedValue`] tree.
//!
//! Parsing preserves file/line/column per node so downstream diagnostics can point at exact
//! source locations. serde is deliberately *not* used for the document tree: it discards
//! spans, and precise diagnostics are non-negotiable — hence the in-house [`SpannedValue`].
//! The eventual YAML path uses an event-level parser over the JSON-compatible subset
//! OAS prescribes.

mod bundle;
mod lock;
mod parse;
mod remote;
mod sha256;
mod value;

#[cfg(feature = "remote-fetch")]
mod vendor;

pub use bundle::InputBundle;
pub use parse::{parse_json, parse_yaml};
pub use value::{Node, Number, SpannedKey, SpannedMap, SpannedValue};

// Remote-ref helpers shared with `oas31::resolve` for hermetic fragment resolution.
pub(crate) use remote::is_absolute_ref as is_remote_ref;
pub(crate) use remote::rewrite_refs_to_absolute as rewrite_refs_absolute;
pub(crate) use remote::split_fragment as remote_split_fragment;

#[cfg(feature = "remote-fetch")]
pub use vendor::{vendor, ReqwestFetcher, VendorReport, VendoredRef};
