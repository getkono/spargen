use proc_macro2::TokenStream;
use quote::ToTokens;

/// A validated Rust identifier. Every `Ident` is a legal Rust identifier — raw-escaped (`r#type`)
/// where that is legal, trailing-underscore otherwise — so codegen can splice it directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident(String);

impl Ident {
    /// Construct an identifier from already-validated text.
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The identifier text (including any `r#` prefix).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl ToTokens for Ident {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = if let Some(raw) = self.0.strip_prefix("r#") {
            proc_macro2::Ident::new_raw(raw, proc_macro2::Span::call_site())
        } else {
            proc_macro2::Ident::new(&self.0, proc_macro2::Span::call_site())
        };
        ident.to_tokens(tokens);
    }
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;

    use super::Ident;
    use crate::name::{escape, IdentRole};

    #[test]
    fn an_identifier_renders_as_itself_in_text_and_in_tokens() {
        let ident = escape("petId", IdentRole::Field);
        assert_eq!(ident.as_str(), "pet_id");
        assert_eq!(ident.to_string(), "pet_id");
        assert_eq!(ident.to_token_stream().to_string(), "pet_id");
    }

    /// The `r#` prefix is part of the identifier's text but must reach the token stream as a raw
    /// identifier rather than as the two tokens `r` and `#`. This is the reason `to_tokens` splits
    /// on the prefix at all.
    #[test]
    fn a_raw_identifier_tokenizes_as_one_raw_ident() {
        let ident = escape("type", IdentRole::Field);
        assert_eq!(ident.as_str(), "r#type");

        let stream = ident.to_token_stream();
        let mut tokens = stream.into_iter();
        let Some(proc_macro2::TokenTree::Ident(single)) = tokens.next() else {
            panic!("a raw identifier must be one Ident token");
        };
        assert!(tokens.next().is_none(), "expected exactly one token");
        assert_eq!(single.to_string(), "r#type");
    }

    #[test]
    fn identifiers_compare_and_hash_by_their_text() {
        // `Scope` keys its occupancy set on the rendered text, so equality must be textual.
        assert_eq!(Ident::new("pet_id"), Ident::new("pet_id"));
        assert_ne!(Ident::new("pet_id"), Ident::new("petId"));

        let mut set = std::collections::HashSet::new();
        assert!(set.insert(Ident::new("pet_id")));
        assert!(!set.insert(Ident::new("pet_id")));
    }
}
