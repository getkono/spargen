use serde::Serialize;

/// An RFC 6901 JSON Pointer addressing a construct in the input document.
///
/// Used both for diagnostic addressing and for `$ref` fragment resolution. Reference tokens are
/// escaped (`~0` for `~`, `~1` for `/`) on construction, so the stored string is always a valid
/// pointer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JsonPointer(String);

impl JsonPointer {
    /// The root pointer (`""`), referring to the whole document.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Append an object-member reference token, escaping `~` and `/` per RFC 6901.
    pub fn push(&self, token: &str) -> Self {
        let escaped = token.replace('~', "~0").replace('/', "~1");
        if self.0.is_empty() {
            Self(format!("/{escaped}"))
        } else {
            Self(format!("{}/{escaped}", self.0))
        }
    }

    /// Append an array-index reference token.
    pub fn index(&self, index: usize) -> Self {
        self.push(&index.to_string())
    }

    /// The pointer to the containing construct, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            return None;
        }
        let parent = self
            .0
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or_default();
        Some(Self(parent.to_owned()))
    }

    /// The pointer as an RFC 6901 string, e.g. `"/components/schemas/Foo"`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for JsonPointer {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for JsonPointer {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for JsonPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
