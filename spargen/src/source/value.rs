use crate::diag::{JsonPointer, Span};

/// A JSON/YAML number. Preserved as one of three concrete kinds; arbitrary precision is not
/// supported. Out-of-range wire values surface later as Decode errors, never silent wraps.
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
/// span-preserving document tree.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedValue {
    /// The value.
    pub node: Node,
    /// Where it came from.
    pub span: Span,
}

impl SpannedValue {
    /// Construct a spanned value.
    pub fn new(node: Node, span: Span) -> Self {
        Self { node, span }
    }

    /// The source span of this value.
    pub fn span(&self) -> Span {
        self.span
    }

    /// This value as an object, if it is one.
    pub fn as_object(&self) -> Option<&SpannedMap> {
        match &self.node {
            Node::Object(object) => Some(object),
            _ => None,
        }
    }

    /// This value as an array slice, if it is one.
    pub fn as_array(&self) -> Option<&[SpannedValue]> {
        match &self.node {
            Node::Array(array) => Some(array),
            _ => None,
        }
    }

    /// This value as a string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match &self.node {
            Node::String(value) => Some(value),
            _ => None,
        }
    }

    /// This value as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match &self.node {
            Node::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Object-member lookup by key (first occurrence).
    pub fn get(&self, key: &str) -> Option<&SpannedValue> {
        self.as_object()?.get(key)
    }

    /// Remove the value at `pointer`, returning it when it existed.
    pub(crate) fn remove_pointer(&mut self, pointer: &JsonPointer) -> Option<SpannedValue> {
        if pointer.as_str().is_empty() {
            return None;
        }
        let (parent, leaf) = pointer.as_str().rsplit_once('/')?;
        let parent = JsonPointer::from(parent.to_owned());
        let leaf = unescape_pointer_token(leaf)?;
        let container = self.pointer_mut(&parent)?;
        match &mut container.node {
            Node::Object(object) => object.remove(&leaf),
            Node::Array(array) => {
                let index = leaf.parse::<usize>().ok()?;
                (index < array.len()).then(|| array.remove(index))
            }
            _ => None,
        }
    }

    fn pointer_mut(&mut self, pointer: &JsonPointer) -> Option<&mut SpannedValue> {
        if pointer.as_str().is_empty() {
            return Some(self);
        }
        let mut current = self;
        for token in pointer.as_str().strip_prefix('/')?.split('/') {
            let token = unescape_pointer_token(token)?;
            current = match &mut current.node {
                Node::Object(object) => object.get_mut(&token)?,
                Node::Array(array) => array.get_mut(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
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
    /// Push a member, preserving insertion order and duplicates.
    pub fn push(&mut self, key: SpannedKey, value: SpannedValue) {
        self.entries.push((key, value));
    }

    /// The value for `key` (first occurrence), if present.
    pub fn get(&self, key: &str) -> Option<&SpannedValue> {
        self.entries
            .iter()
            .find_map(|(candidate, value)| (candidate.name == key).then_some(value))
    }

    /// The mutable value for `key` (first occurrence), if present.
    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut SpannedValue> {
        self.entries
            .iter_mut()
            .find_map(|(candidate, value)| (candidate.name == key).then_some(value))
    }

    /// Remove the first entry with `key`.
    pub(crate) fn remove(&mut self, key: &str) -> Option<SpannedValue> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate.name == key)?;
        Some(self.entries.remove(index).1)
    }

    /// Iterate members in source order.
    pub fn iter(&self) -> impl Iterator<Item = (&SpannedKey, &SpannedValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

fn unescape_pointer_token(token: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' {
            match chars.next()? {
                '0' => out.push('~'),
                '1' => out.push('/'),
                _ => return None,
            }
        } else {
            out.push(ch);
        }
    }
    Some(out)
}
