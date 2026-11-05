//! The two span-preserving document parsers, `json` (a hand-rolled recursive-descent parser) and
//! `yaml` (an event-based builder over `yaml_rust2`'s JSON-compatible subset), and what they share:
//! the nesting cap and the duplicate-key (`E022`) report.

mod json;
mod yaml;

pub(crate) use json::parse_json;
pub(crate) use yaml::parse_yaml;

use crate::diag::JsonPointer;

/// One step of the path from the document root to a duplicate key: an object member or an array
/// element. Both parsers collect the path innermost first while the failure unwinds, so a document
/// that parses pays nothing for it.
enum Segment {
    Key(String),
    Index(usize),
}

/// The pointer that `path`, innermost segment first, addresses.
fn pointer_of(path: &[Segment]) -> JsonPointer {
    path.iter()
        .rev()
        .fold(JsonPointer::root(), |pointer, segment| match segment {
            Segment::Key(name) => pointer.push(name),
            Segment::Index(index) => pointer.index(*index),
        })
}

/// The message `E022` carries in both parsers.
fn duplicate_key_message(name: &str) -> String {
    format!("duplicate object key `{name}`")
}

/// The remedy `E022` carries in both parsers.
const DUPLICATE_KEY_REMEDY: &str =
    "remove or rename the duplicate key so the object is unambiguous";

/// The deepest a value may sit in a parsed document, the root at depth 0, in either parser. Both
/// recurse once per level of nesting, so the cap bounds their stack on pathological input
/// (`[[[…]]]`, or block-style `- - - …`), and a value past it is `E011`.
const MAX_NESTING_DEPTH: u32 = 256;

#[cfg(test)]
mod tests;
