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
/// - lines inside a fenced code block (```` ``` ```` or `~~~`) take no continuation rewrite: past
///   the tab rule, which applies to the whole text, they are left as written.
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

#[cfg(test)]
mod tests {
    use super::normalize_rustdoc;

    /// `normalize_rustdoc` over each row's input yields exactly that row's output. Every rule the
    /// function documents has a row, and so does each edge of its list-marker and fence grammar,
    /// so a change to what it emits is a change to this table.
    #[test]
    fn normalization_follows_the_table() {
        let rows: &[(&str, &str, &str)] = &[
            // Tabs.
            ("a tab outside a fence", "a\tb", "a    b"),
            (
                "a tab inside a fence is rewritten too: the tab rule precedes the fence scan",
                "```\n\tcode\n```",
                "```\n    code\n```",
            ),
            (
                "a tab after a bullet widens its content column",
                "-\titem\n\tmore",
                "-    item\n     more",
            ),
            // Bullet lists.
            ("a lazy bullet continuation", "- a\nb", "- a\n  b"),
            ("a `*` bullet", "* a\nb", "* a\n  b"),
            ("a `+` bullet with two spaces", "+  a\nb", "+  a\n   b"),
            ("a bullet with no space is no list", "-a\nb", "-a\nb"),
            ("an indented bullet", "  - a\nb", "  - a\n    b"),
            // Ordered lists.
            ("an ordered `.` item", "1. item\nmore", "1. item\n   more"),
            (
                "an ordered `)` item",
                "10) item\nmore",
                "10) item\n    more",
            ),
            (
                "a partly indented continuation is topped up to the content column",
                "1. item\n  more",
                "1. item\n   more",
            ),
            (
                "a continuation already past the content column is kept",
                "1. item\n      more",
                "1. item\n      more",
            ),
            (
                "digits with no marker are no list",
                "2024 was\na year",
                "2024 was\na year",
            ),
            (
                "a marker with no space is no list",
                "1.5 is\na number",
                "1.5 is\na number",
            ),
            ("a marker alone is no list", "1.\nnext", "1.\nnext"),
            // Block quotes.
            ("a lazy quote continuation", "> q\nlazy", "> q\n> lazy"),
            // What ends a continuation.
            ("a blank line ends a list", "- a\n\nb", "- a\n\nb"),
            (
                "a whitespace-only line is emptied",
                "- a\n   \nb",
                "- a\n\nb",
            ),
            (
                "a new item replaces the old one's column",
                "- a\n10. b\nc",
                "- a\n10. b\n    c",
            ),
            // Fences.
            (
                "a fence ends a list, and its lines take no rewrite",
                "- a\n```\nb\n```\nc",
                "- a\n```\nb\n```\nc",
            ),
            (
                "a `~~~` fence",
                "- a\n~~~\n> b\nc\n~~~\nd",
                "- a\n~~~\n> b\nc\n~~~\nd",
            ),
            (
                "the other fence marker inside a fence neither opens nor closes it",
                "```\n~~~\n- a\nb\n```\n- c\nd",
                "```\n~~~\n- a\nb\n```\n- c\n  d",
            ),
            (
                "an indented fence is a fence",
                "- a\n   ```\nb\n   ```\nc",
                "- a\n   ```\nb\n   ```\nc",
            ),
            (
                "an unclosed fence runs to the end",
                "```\n- a\nb",
                "```\n- a\nb",
            ),
            // Line structure.
            ("a trailing newline is dropped", "a\n", "a"),
            ("CRLF line ends become LF", "- a\r\nb", "- a\n  b"),
            ("an empty text", "", ""),
        ];
        for (case, input, expected) in rows {
            assert_eq!(normalize_rustdoc(input), *expected, "{case}: {input:?}");
        }
    }
}
