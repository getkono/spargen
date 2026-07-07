use crate::diag::{JsonPointer, Span};

/// A JSON/YAML number. Preserved as one of three concrete kinds; arbitrary precision is not
/// supported (PRD D5). Out-of-range wire values surface later as Decode errors, never silent wraps.
#[derive(Debug, Clone, PartialEq)]
pub enum Number {
    /// A signed integer that fits `i64`.
    Int(i64),
    /// An unsigned integer that exceeds `i64` but fits `u64`.
    UInt(u64),
    /// A floating-point number.
    Float(f64),
}

/// A parsed value node, without its span.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(Number),
    /// A string.
    String(String),
    /// An array, in source order.
    Array(Vec<SpannedValue>),
    /// An object, preserving source order and any duplicate keys.
    Object(SpannedMap),
}

/// A value node paired with the source [`Span`] it was parsed from — the unit of the
/// span-preserving document tree (PRD FR1, D4).
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedValue {
    /// The value.
    pub node: Node,
    /// Where it came from.
    pub span: Span,
}

impl SpannedValue {
    /// The source span of this value.
    pub fn span(&self) -> Span {
        self.span
    }

    /// This value as an object, if it is one.
    pub fn as_object(&self) -> Option<&SpannedMap> {
        todo!()
    }

    /// This value as an array slice, if it is one.
    pub fn as_array(&self) -> Option<&[SpannedValue]> {
        todo!()
    }

    /// This value as a string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        todo!()
    }

    /// This value as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        todo!()
    }

    /// Object-member lookup by key (first occurrence).
    pub fn get(&self, key: &str) -> Option<&SpannedValue> {
        todo!()
    }

    /// Resolve an RFC 6901 JSON Pointer relative to this value (PRD §3.3 prec 6).
    pub fn pointer(&self, pointer: &JsonPointer) -> Option<&SpannedValue> {
        todo!()
    }
}

/// An object key together with its own span, so duplicate-key and unknown-key diagnostics can
/// point at the key itself.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedKey {
    /// The key text.
    pub name: String,
    /// Where the key appears.
    pub span: Span,
}

/// An ordered map of object members. Preserves source order (for deterministic downstream
/// behavior) and retains duplicate keys (so they can be diagnosed rather than silently merged).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpannedMap {
    entries: Vec<(SpannedKey, SpannedValue)>,
}

impl SpannedMap {
    /// The value for `key` (first occurrence), if present.
    pub fn get(&self, key: &str) -> Option<&SpannedValue> {
        todo!()
    }

    /// Iterate members in source order.
    pub fn iter(&self) -> impl Iterator<Item = (&SpannedKey, &SpannedValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    /// The number of members (counting duplicates).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the object has no members.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Pairs of duplicate keys as `(first, later)`, for duplicate-key diagnostics.
    pub fn duplicate_keys(&self) -> Vec<(&SpannedKey, &SpannedKey)> {
        todo!()
    }
}
