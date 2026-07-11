use proc_macro2::TokenStream;

/// Format a token stream into deterministic, rustfmt-clean Rust source via `prettyplease`.
/// Returns a [`syn::Error`] if the tokens are not a parseable Rust file — a codegen bug, not
/// a spec problem.
pub fn format_tokens(tokens: TokenStream) -> Result<String, syn::Error> {
    Ok(prettyplease::unparse(&syn::parse2(tokens)?))
}
