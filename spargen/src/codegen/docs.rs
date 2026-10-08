//! Spec-authored prose rendered as rustdoc: the `#[doc = …]` attribute of a documented item, and
//! the normalization every emitted doc string goes through.

use proc_macro2::TokenStream;
use quote::quote;

/// Turn lowered documentation into `#[doc = …]` attributes so IDE hover shows the API docs.
pub(super) fn doc_tokens(docs: &crate::ir::Docs) -> TokenStream {
    let mut paragraphs: Vec<&str> = Vec::new();
    let summary = docs
        .summary
        .as_deref()
        .filter(|text| !text.trim().is_empty());
    if let Some(summary) = summary {
        paragraphs.push(summary);
    }
    if let Some(description) = docs
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        if summary != Some(description) {
            paragraphs.push(description);
        }
    }
    if paragraphs.is_empty() {
        if let Some(title) = docs.title.as_deref().filter(|text| !text.trim().is_empty()) {
            paragraphs.push(title);
        }
    }
    if paragraphs.is_empty() {
        return quote! {};
    }
    let text = normalize_rustdoc(&paragraphs.join("\n\n"));
    quote! { #[doc = #text] }
}

/// Normalize spec-authored prose for Rust's documentation lint surface, without altering what it
/// says:
///
/// - every tab becomes four spaces: tabs are visually ambiguous in rustdoc and trip Clippy's
///   `tabs_in_doc_comments`, and four spaces preserve table and code alignment;
/// - a lazy continuation line — a non-blank line that follows a list item or block quote without
///   itself starting one — is made explicit, indented to the list item's content column or
///   prefixed with `> `, which is what Clippy's `doc_lazy_continuation` asks for;
/// - lines inside a fenced code block (```` ``` ```` or `~~~`) are left exactly as written.
pub(super) fn normalize_rustdoc(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum Continuation {
        None,
        List(usize),
        Quote,
    }

    fn list_indent(line: &str) -> Option<usize> {
        let leading = line.len() - line.trim_start_matches(' ').len();
        let text = &line[leading..];
        if matches!(text.as_bytes().first(), Some(b'-' | b'*' | b'+')) {
            let spacing = text.as_bytes()[1..]
                .iter()
                .take_while(|byte| **byte == b' ')
                .count();
            return (spacing > 0).then_some(leading + 1 + spacing);
        }
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let marker = *text.as_bytes().get(digits)?;
        if digits == 0 || !matches!(marker, b'.' | b')') {
            return None;
        }
        let spacing = text.as_bytes()[digits + 1..]
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        (spacing > 0).then_some(leading + digits + 1 + spacing)
    }

    let text = text.replace('\t', "    ");
    let mut continuation = Continuation::None;
    let mut fence: Option<char> = None;
    let mut output = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().next().expect("non-empty fence");
            fence = match fence {
                Some(active) if active == marker => None,
                None => Some(marker),
                active => active,
            };
            continuation = Continuation::None;
            output.push(line.to_owned());
            continue;
        }
        if fence.is_some() {
            output.push(line.to_owned());
            continue;
        }
        if trimmed.is_empty() {
            continuation = Continuation::None;
            output.push(String::new());
        } else if trimmed.starts_with('>') {
            continuation = Continuation::Quote;
            output.push(line.to_owned());
        } else if let Some(indent) = list_indent(line) {
            continuation = Continuation::List(indent);
            output.push(line.to_owned());
        } else {
            match continuation {
                Continuation::None => output.push(line.to_owned()),
                Continuation::Quote => output.push(format!("> {line}")),
                Continuation::List(indent) => {
                    let leading = line.len() - trimmed.len();
                    output.push(format!(
                        "{}{line}",
                        " ".repeat(indent.saturating_sub(leading))
                    ));
                }
            }
        }
    }
    output.join("\n")
}
