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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::JsonPointer;

    #[test]
    fn push_escapes_tilde_before_slash() {
        let root = JsonPointer::root();
        assert_eq!(root.as_str(), "");
        assert_eq!(root.push("paths").as_str(), "/paths");
        assert_eq!(
            root.push("paths").push("/pets/{id}").as_str(),
            "/paths/~1pets~1{id}"
        );
        assert_eq!(root.push("a~b").as_str(), "/a~0b");
        // `~` is escaped first, so a literal `~1` stays two characters and never decodes to `/`.
        assert_eq!(root.push("~1").as_str(), "/~01");
        assert_eq!(root.push("~/").as_str(), "/~0~1");
        // The empty token is a real member name, not a no-op.
        assert_eq!(root.push("").as_str(), "/");
        assert_eq!(root.push("tags").index(3).as_str(), "/tags/3");
    }

    #[test]
    fn parent_drops_exactly_the_last_token() {
        assert_eq!(JsonPointer::root().parent(), None);
        assert_eq!(
            JsonPointer::from("/paths").parent(),
            Some(JsonPointer::root())
        );
        assert_eq!(
            JsonPointer::from("/paths/~1pets/get").parent(),
            Some(JsonPointer::from("/paths/~1pets"))
        );
        // An escaped `/` inside a token is not a separator.
        assert_eq!(
            JsonPointer::from("/a~1b").parent(),
            Some(JsonPointer::root())
        );
        assert_eq!(JsonPointer::from("/").parent(), Some(JsonPointer::root()));
        assert_eq!(
            JsonPointer::from("/tags/3").parent(),
            Some(JsonPointer::from("/tags"))
        );
    }

    #[test]
    fn display_is_the_pointer_string() {
        let pointer = JsonPointer::root().push("components").push("a/b");
        assert_eq!(pointer.to_string(), "/components/a~1b");
        assert_eq!(pointer.to_string(), pointer.as_str());
        assert_eq!(JsonPointer::root().to_string(), "");
    }

    proptest! {
        /// `parent` undoes `push` for every token, whatever it holds.
        #[test]
        fn parent_inverts_push(
            tokens in proptest::collection::vec(r"[a-c~/]{0,4}", 0..4),
            last in r"[a-c~/]{0,4}",
        ) {
            let base = tokens
                .iter()
                .fold(JsonPointer::root(), |pointer, token| pointer.push(token));
            prop_assert_eq!(base.push(&last).parent(), Some(base));
        }
    }
}
