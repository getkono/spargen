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
mod parse;
mod value;

pub use bundle::InputBundle;
pub use parse::{parse_json, parse_yaml};
pub use value::{Node, Number, SpannedKey, SpannedMap, SpannedValue};
