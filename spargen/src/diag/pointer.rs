use serde::Serialize;

/// An RFC 6901 JSON Pointer addressing a construct in the input document.
///
/// Used both for diagnostic addressing and for `$ref` fragment resolution. [`push`](Self::push)
/// and [`index`](Self::index) escape each reference token (`~0` for `~`, `~1` for `/`); the `From`
/// conversions store an already-encoded pointer string verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JsonPointer(String);

impl JsonPointer {
    /// The root pointer (`""`), referring to the whole document.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Append an object-member reference token, escaping `~` and `/` per RFC 6901.
    pub fn push(&self, token: &str) -> Self {
        let escaped = escape_token(token);
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

    /// The pointer's reference tokens, each unescaped: the inverse of [`push`](Self::push). The
    /// root pointer has none; a string without the leading `/` is split as if it carried one.
    pub(crate) fn tokens(&self) -> Vec<String> {
        if self.0.is_empty() {
            return Vec::new();
        }
        self.0
            .strip_prefix('/')
            .unwrap_or(&self.0)
            .split('/')
            .map(unescape_token)
            .collect()
    }
}

/// Escape one RFC 6901 reference token: `~` to `~0` first, then `/` to `~1`, so a literal `~1`
/// stays two characters and never decodes to `/`.
pub(crate) fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Unescape one RFC 6901 reference token, the inverse of [`escape_token`]: `~1` to `/` first, then
/// `~0` to `~`, or a literal `~01` would decode as `/`.
fn unescape_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
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

    use super::{escape_token, JsonPointer};

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

    #[test]
    fn tokens_unescape_slash_before_tilde() {
        assert_eq!(JsonPointer::root().tokens(), Vec::<String>::new());
        assert_eq!(JsonPointer::from("/").tokens(), vec![String::new()]);
        assert_eq!(
            JsonPointer::from("/paths/~1pets~1{id}/get").tokens(),
            vec!["paths", "/pets/{id}", "get"]
        );
        // `~01` is an escaped `~` followed by `1`, never an escaped `/`.
        assert_eq!(JsonPointer::from("/~01").tokens(), vec!["~1"]);
        assert_eq!(JsonPointer::from("/~0~1").tokens(), vec!["~/"]);
        // A string without the leading `/` splits as if it had one.
        assert_eq!(JsonPointer::from("a/b").tokens(), vec!["a", "b"]);
    }

    #[test]
    fn escape_token_is_what_push_appends() {
        assert_eq!(escape_token("~/"), "~0~1");
        assert_eq!(escape_token("~1"), "~01");
        assert_eq!(escape_token("plain"), "plain");
    }

    proptest! {
        /// `tokens` undoes `push` for every token sequence, whatever the tokens hold.
        #[test]
        fn tokens_invert_push(tokens in proptest::collection::vec(r"[a-c~/01]{0,4}", 0..5)) {
            let pointer = tokens
                .iter()
                .fold(JsonPointer::root(), |pointer, token| pointer.push(token));
            prop_assert_eq!(pointer.tokens(), tokens);
        }

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
