use serde::Serialize;

/// An RFC 6901 JSON Pointer addressing a construct in the input document (PRD FR6, §3.3 prec 6).
///
/// Used both for diagnostic addressing and for `$ref` fragment resolution. Reference tokens are
/// escaped (`~0` for `~`, `~1` for `/`) on construction, so the stored string is always a valid
/// pointer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JsonPointer(String);

impl JsonPointer {
    /// The root pointer (`""`), referring to the whole document.
    pub fn root() -> Self {
        todo!()
    }

    /// Append an object-member reference token, escaping `~` and `/` per RFC 6901.
    pub fn push(&self, token: &str) -> Self {
        todo!()
    }

    /// Append an array-index reference token.
    pub fn index(&self, index: usize) -> Self {
        todo!()
    }

    /// The pointer to the containing construct, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        todo!()
    }

    /// The pointer as an RFC 6901 string, e.g. `"/components/schemas/Foo"`.
    pub fn as_str(&self) -> &str {
        todo!()
    }
}

impl std::fmt::Display for JsonPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
