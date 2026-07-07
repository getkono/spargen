use proc_macro2::TokenStream;
use quote::ToTokens;

/// A validated Rust identifier. Every `Ident` is a legal Rust identifier — raw-escaped (`r#type`)
/// where that is legal, trailing-underscore otherwise (PRD D9) — so codegen can splice it directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident(String);

impl Ident {
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
        todo!()
    }
}
