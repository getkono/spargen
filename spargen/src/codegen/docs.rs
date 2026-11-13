//! Spec-authored prose rendered as rustdoc: the `#[doc = …]` attribute of a documented item, the
//! rewrite that keeps carried prose clear of rustdoc's link and HTML lints, and the normalization
//! every emitted doc string goes through.

use std::borrow::Cow;
use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::quote;

use super::CodegenOptions;
use crate::ir::{closes_fence, fence_opener, leading_columns, list_indent};

/// Spec-carried prose as it is rendered under `options`: [`sanitize_carried`] applied, or, under
/// `Spec::verbatim_docs(true)`, the text as written.
pub(super) fn carried<'a>(text: &'a str, options: &CodegenOptions) -> Cow<'a, str> {
    if options.verbatim_docs {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(sanitize_carried(text))
    }
}

/// Turn lowered documentation into `#[doc = …]` attributes so IDE hover shows the API docs.
///
/// The summary and description become paragraphs, the description dropped when it repeats the
/// summary, and the title is used only when both are blank; blank fields count as absent, so a
/// `Docs` with nothing but whitespace yields no attribute at all. The text is spec-carried, so it
/// goes through [`carried`] before [`normalize_rustdoc`].
pub(super) fn doc_tokens(docs: &crate::ir::Docs, options: &CodegenOptions) -> TokenStream {
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
    let text = normalize_rustdoc(&carried(&paragraphs.join("\n\n"), options));
    quote! { #[doc = #text] }
}

/// Rewrite spec-carried prose so that, `include!`d anywhere, rustdoc's `broken_intra_doc_links`,
/// `private_intra_doc_links`, `bare_urls` and `invalid_html_tags` lints find nothing in it, while
/// it still reads as written:
///
/// - an intra-doc link — a link whose destination rustdoc reads as a Rust path (``[`crate::serve`]``,
///   `[Foo]`, `[text](crate::Foo)`, `[text][Foo]`) — becomes its text, so ``[`crate::serve`]`` is
///   the code span `` `crate::serve` ``. Carried text names items of whatever crate wrote it, which
///   no path from the generated module reaches, so no such link is kept. A link reference
///   definition whose destination is a path (`[Foo]: crate::Foo`) is dropped with it;
/// - a link to a URL (`[text](https://…)`, or a reference whose definition is one) and a bracketed
///   text rustdoc does not read as a path (`[see below]`, `[1.5]`) are kept, their text rewritten
///   like any other (`[Vec<T>](https://…)` becomes `[Vec\<T>](https://…)`), except that a URL in a
///   link's text stays bare; a shortcut or collapsed reference whose text that changes becomes a
///   full one (`[Vec\<T>][Vec<T>]`), still matching its definition;
/// - a `[` nested deeper than [`MAX_LINK_NESTING`] brackets is escaped as `\[`;
/// - a task-list marker (`- [x] done`) and an autolink (`<https://…>`) are kept as written;
/// - a bare `http://` or `https://` URL is wrapped in `<…>`, which makes it an autolink;
/// - an HTML tag with no partner (`Vec<T>`'s `<T>`, an unopened `</b>`, a `<` that never closes)
///   has its `<` escaped as `\<`, so it renders as text, and so does a self-closing tag of an
///   element that is not void (`<T/>`), which rustdoc rejects; a void element (`<br>`, `<br/>`)
///   and a closed pair (`<b>…</b>`) are HTML and stay;
/// - code spans and backslash escapes are left as written, and so are code blocks, except that
///   rustdoc would read an untagged one as a Rust doctest of the consuming crate: an untagged fence
///   (```` ``` ````, `~~~`) is tagged `text`, an indented code block becomes a fenced one tagged
///   `text`, and a fence still open at the end of the text is closed there, so the generated text
///   after it stays prose. A fence closes only on a fence of its character at least as long, as in
///   CommonMark.
///
/// Lines end where [`normalize_rustdoc`] ends them: at a `\n`, a `\r\n` or a lone `\r`.
pub(super) fn sanitize_carried(text: &str) -> String {
    let text = unify_line_ends(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut kinds = Vec::with_capacity(lines.len());
    // The open fence's character and length, and its closing line.
    let mut fence: Option<(u8, usize, String)> = None;
    let mut previous_blank = true;
    let mut previous_indented_code = false;
    // The content column of the list item the text is in, which an indented code block nested in
    // it is indented four past.
    let mut list_column: Option<usize> = None;
    for line in &lines {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let blank = trimmed.trim_end().is_empty();
        let base = list_column.unwrap_or(0);
        let indented_code = fence.is_none()
            && !blank
            && leading_columns(line) >= base + 4
            && (previous_blank || previous_indented_code);
        let kind = if let Some((marker, length, _)) = &fence {
            if closes_fence(line, *marker, *length) {
                fence = None;
            }
            LineKind::Code(None)
        } else if indented_code {
            LineKind::Indented { column: base }
        } else if let Some((marker, length, info)) = fence_opener(line) {
            let indent = &line[..line.len() - trimmed.len()];
            let run = &trimmed[..length];
            fence = Some((marker, length, format!("{indent}{run}")));
            LineKind::Code(info.is_empty().then(|| format!("{indent}{run}text")))
        } else if let Some((label, destination)) = link_definition(line) {
            LineKind::Definition {
                label,
                path: is_rust_path(destination),
            }
        } else {
            LineKind::Prose
        };
        // A list's content may be indented as far as code; a line back at the margin ends it.
        if matches!(kind, LineKind::Prose | LineKind::Definition { .. }) {
            if let Some(column) = list_indent(line) {
                list_column = Some(column);
            } else if !blank && line.len() - trimmed.len() < 2 {
                list_column = None;
            }
        }
        previous_indented_code = indented_code;
        previous_blank = blank;
        kinds.push(kind);
    }
    // A fence left open is closed at the end, before the empty line a trailing `\n` leaves.
    let mut lines: Vec<Cow<'_, str>> = lines.into_iter().map(Cow::Borrowed).collect();
    if let Some((_, _, closing)) = fence {
        let at = if lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
            lines.len() - 1
        } else {
            lines.len()
        };
        lines.insert(at, Cow::Owned(closing));
        kinds.insert(at, LineKind::Code(None));
    }

    // A definition to a URL keeps the links that use it; one to a path is dropped, and so is every
    // link that used it, down to its text. The first definition of a label wins, as in CommonMark.
    let mut definitions = BTreeMap::new();
    for kind in &kinds {
        if let LineKind::Definition { label, path } = kind {
            definitions.entry(label_key(label)).or_insert(*path);
        }
    }

    // Whether the next non-blank line from each index on is an indented code block's: a blank
    // line before one is inside the block, a blank line before anything else ends it.
    let mut indented_next = vec![false; lines.len() + 1];
    for index in (0..lines.len()).rev() {
        indented_next[index] = if lines[index].trim().is_empty() {
            indented_next[index + 1]
        } else {
            matches!(kinds[index], LineKind::Indented { .. })
        };
    }

    // Every line kept ends in `\n` among the pieces, and `render_pieces` drops the last one.
    let mut pieces = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let flush_prose = |prose: &mut Vec<&str>, pieces: &mut Vec<Piece>| {
        if !prose.is_empty() {
            Scanner::new(&definitions, pieces).scan(&prose.join("\n"));
            prose.clear();
        }
    };
    // The closing line of the fence an indented code block was rewritten into, while it is open.
    let mut indented_fence: Option<String> = None;
    for (index, (line, kind)) in lines.iter().zip(&kinds).enumerate() {
        if let Some(closing) = &indented_fence {
            if !indented_next[index] {
                pieces.push(Piece::Text(format!("{closing}\n")));
                indented_fence = None;
            } else if !matches!(kind, LineKind::Indented { .. }) {
                // A blank line inside the block.
                pieces.push(Piece::Text("\n".to_owned()));
                continue;
            }
        }
        match kind {
            LineKind::Prose => prose.push(line),
            LineKind::Definition { path: true, .. } => {}
            LineKind::Indented { column } => {
                if indented_fence.is_none() {
                    flush_prose(&mut prose, &mut pieces);
                    let run = "`".repeat(indented_fence_length(&lines[index..], &kinds[index..]));
                    let indent = " ".repeat(*column);
                    pieces.push(Piece::Text(format!("{indent}{run}text\n")));
                    indented_fence = Some(format!("{indent}{run}"));
                }
                let content = line.trim_start_matches([' ', '\t']);
                let columns = leading_columns(line) - 4;
                pieces.push(Piece::Text(format!("{}{content}\n", " ".repeat(columns))));
            }
            LineKind::Code(rewritten) => {
                flush_prose(&mut prose, &mut pieces);
                let line = rewritten.as_deref().unwrap_or(line);
                pieces.push(Piece::Text(format!("{line}\n")));
            }
            LineKind::Definition { path: false, .. } => {
                flush_prose(&mut prose, &mut pieces);
                pieces.push(Piece::Text(format!("{line}\n")));
            }
        }
    }
    if let Some(closing) = indented_fence {
        pieces.push(Piece::Text(format!("{closing}\n")));
    }
    flush_prose(&mut prose, &mut pieces);
    render_pieces(pieces)
}

/// The length of the backtick fence an indented code block starting at `lines[0]` is rewritten
/// into: three, or one past the longest backtick run a line of the block starts with, so no line of
/// it closes the fence.
fn indented_fence_length(lines: &[Cow<'_, str>], kinds: &[LineKind]) -> usize {
    let mut longest = 0;
    for (line, kind) in lines.iter().zip(kinds) {
        match kind {
            LineKind::Indented { .. } => {
                let run = line
                    .trim_start_matches([' ', '\t'])
                    .bytes()
                    .take_while(|byte| *byte == b'`')
                    .count();
                longest = longest.max(run);
            }
            _ if line.trim().is_empty() => {}
            _ => break,
        }
    }
    (longest + 1).max(3)
}

/// How [`sanitize_carried`] treats one line of carried text.
enum LineKind {
    /// Scanned for links, URLs and tags, together with the prose lines around it.
    Prose,
    /// A fenced code block's line: kept as written, or replaced by the line it holds (an untagged
    /// opening fence tagged `text`).
    Code(Option<String>),
    /// An indented code block's line, in a list item whose content starts at `column` (0 outside
    /// one): rewritten into a fenced block at that column.
    Indented { column: usize },
    /// A link reference definition, `[label]: destination`: kept when the destination is a URL,
    /// dropped when rustdoc would read it as a path.
    Definition { label: String, path: bool },
}

/// One run of [`sanitize_carried`]'s output: text already final, or an HTML tag whose fate waits on
/// whether the rest of the text closes it.
enum Piece {
    Text(String),
    Tag {
        raw: String,
        name: String,
        kind: TagKind,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Open,
    Close,
    /// A void element, `<br>` or `<br/>`: complete on its own.
    Complete,
}

/// The HTML elements that take no closing tag.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Join the pieces, escaping the `<` of every tag that is neither complete nor one half of a pair.
fn render_pieces(pieces: Vec<Piece>) -> String {
    let mut keep = vec![false; pieces.len()];
    let mut open: Vec<(usize, &str)> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let Piece::Tag { name, kind, .. } = piece else {
            continue;
        };
        match kind {
            TagKind::Complete => keep[index] = true,
            TagKind::Open => open.push((index, name)),
            TagKind::Close => {
                if let Some(position) = open.iter().rposition(|(_, opened)| opened == name) {
                    keep[open[position].0] = true;
                    keep[index] = true;
                    open.truncate(position);
                }
            }
        }
    }
    let mut output = String::new();
    for (piece, keep) in pieces.iter().zip(keep) {
        match piece {
            Piece::Text(text) => output.push_str(text),
            Piece::Tag { raw, .. } => {
                if !keep {
                    output.push('\\');
                }
                output.push_str(raw);
            }
        }
    }
    // The last line kept took a separator nothing follows.
    if output.ends_with('\n') {
        output.pop();
    }
    output
}

/// How deep [`Scanner::link`] reads brackets nested in a link's text. A `[` opened deeper is
/// escaped as `\[` and its contents scanned as plain text, so a description of nested brackets
/// costs bounded stack and at most this many scans of its length.
const MAX_LINK_NESTING: usize = 32;

/// The inline pass of [`sanitize_carried`] over a run of prose lines.
struct Scanner<'a> {
    /// The link reference definitions, by normalized label: `true` for one to a path.
    definitions: &'a BTreeMap<String, bool>,
    pieces: &'a mut Vec<Piece>,
    text: String,
    /// How many brackets enclose the text being scanned.
    depth: usize,
    /// Whether that text is a kept link's text, where a bare URL is left unwrapped: an autolink
    /// there would end the enclosing link.
    in_link_text: bool,
}

impl<'a> Scanner<'a> {
    fn new(definitions: &'a BTreeMap<String, bool>, pieces: &'a mut Vec<Piece>) -> Self {
        Self {
            definitions,
            pieces,
            text: String::new(),
            depth: 0,
            in_link_text: false,
        }
    }

    /// Scan the bracketed `label` one level deeper, as a kept link's text when `link_text` holds.
    fn nested(&mut self, label: &str, link_text: bool) {
        let outer = self.in_link_text;
        self.depth += 1;
        self.in_link_text = outer || link_text;
        self.inline(label);
        self.in_link_text = outer;
        self.depth -= 1;
    }

    /// Scan a kept reference link's `label` and finish it: as `[label]` plus `suffix` when scanning
    /// left the label as written, otherwise as a full reference, `[scanned][label]`, since the
    /// definition is matched on the label as written.
    fn reference_link(&mut self, label: &str, suffix: &str) {
        self.text.push('[');
        let pieces = self.pieces.len();
        let text = self.text.len();
        self.nested(label, true);
        if self.pieces.len() == pieces && self.text[text..] == *label {
            self.text.push(']');
            self.text.push_str(suffix);
        } else {
            self.text.push_str("][");
            self.text.push_str(label);
            self.text.push(']');
        }
    }

    /// Whether a reference to `label` is kept as a link: `Some(true)` when a definition to a URL
    /// serves it, `Some(false)` when it is an intra-doc link — a definition to a path, or none and
    /// a label rustdoc reads as a path — and `None` when it is no link at all.
    fn reference(&self, label: &str) -> Option<bool> {
        match self.definitions.get(&label_key(label)) {
            Some(path) => Some(!path),
            None => is_rust_path(label).then_some(false),
        }
    }

    /// Scan `source`, appending its pieces and then a line end.
    fn scan(mut self, source: &str) {
        self.inline(source);
        self.text.push('\n');
        self.flush();
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.pieces
                .push(Piece::Text(std::mem::take(&mut self.text)));
        }
    }

    fn inline(&mut self, source: &str) {
        let bytes = source.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            let rest = &source[at..];
            let consumed = match bytes[at] {
                b'\\' if bytes.get(at + 1).is_some_and(u8::is_ascii_punctuation) => {
                    self.text.push_str(&rest[..2]);
                    2
                }
                b'`' => {
                    let run = code_span(rest);
                    self.text.push_str(&rest[..run]);
                    run
                }
                b'<' => self.angle(rest),
                b'[' => self.link(rest),
                b'h' if !self.in_link_text
                    && (rest.starts_with("http://") || rest.starts_with("https://")) =>
                {
                    self.bare_url(rest)
                }
                _ => {
                    let width = rest.chars().next().map_or(1, char::len_utf8);
                    self.text.push_str(&rest[..width]);
                    width
                }
            };
            at += consumed;
        }
    }

    /// `rest` starts with `<`: an autolink or comment kept whole, a tag left for
    /// [`render_pieces`] to judge, or a lone `<` escaped. Returns the bytes consumed.
    fn angle(&mut self, rest: &str) -> usize {
        if let Some(end) = autolink(rest).or_else(|| html_comment(rest)) {
            self.text.push_str(&rest[..end]);
            return end;
        }
        if let Some((end, name, kind)) = html_tag(rest) {
            self.flush();
            self.pieces.push(Piece::Tag {
                raw: rest[..end].to_owned(),
                name,
                kind,
            });
            return end;
        }
        // A `<` that begins no tag but would read as an incomplete one.
        if rest.as_bytes().get(1).is_some_and(|byte| {
            byte.is_ascii_alphabetic() || *byte == b'/' || *byte == b'!' || *byte == b'?'
        }) {
            self.text.push('\\');
        }
        self.text.push('<');
        1
    }

    /// `rest` starts with `[`: an intra-doc link reduced to its text, anything else kept with its
    /// text scanned, and a `[` deeper than [`MAX_LINK_NESTING`] escaped. Returns the bytes
    /// consumed.
    fn link(&mut self, rest: &str) -> usize {
        if self.depth >= MAX_LINK_NESTING {
            self.text.push_str("\\[");
            return 1;
        }
        let Some(close) = closing_bracket(rest) else {
            self.text.push('[');
            return 1;
        };
        let label = &rest[1..close];
        let after = &rest[close + 1..];
        // A task-list marker (`- [x] done`) is no link.
        let line = self.text.rsplit('\n').next().unwrap_or_default();
        if matches!(label, " " | "x" | "X")
            && after.starts_with(' ')
            && list_indent(line) == Some(line.len())
        {
            self.text.push_str(&rest[..=close]);
            return close + 1;
        }
        if after.starts_with('(') {
            if let Some(end) = closing_paren(after) {
                let end = close + 1 + end;
                if is_rust_path(inline_destination(&rest[close + 2..end])) {
                    self.nested(label, false);
                } else {
                    self.text.push('[');
                    self.nested(label, true);
                    self.text.push_str(&rest[close..=end]);
                }
                return end + 1;
            }
        }
        if after.starts_with('[') {
            if let Some(reference) = after.find(']') {
                let end = close + 1 + reference;
                let reference_label = &rest[close + 2..end];
                let key = if reference_label.trim().is_empty() {
                    label
                } else {
                    reference_label
                };
                match self.reference(key) {
                    Some(true) if reference_label.trim().is_empty() => {
                        self.reference_link(label, "[]");
                    }
                    Some(true) => {
                        self.text.push('[');
                        self.nested(label, true);
                        self.text.push_str(&rest[close..=end]);
                    }
                    Some(false) => self.nested(label, false),
                    None => {
                        self.text.push('[');
                        self.nested(label, false);
                        self.text.push_str(&rest[close..=end]);
                    }
                }
                return end + 1;
            }
        }
        match self.reference(label) {
            Some(true) => self.reference_link(label, ""),
            Some(false) => self.nested(label, false),
            None => {
                self.text.push('[');
                self.nested(label, false);
                self.text.push(']');
            }
        }
        close + 1
    }

    /// `rest` starts with `http://` or `https://`: wrap the URL in `<…>`. Returns the bytes
    /// consumed.
    fn bare_url(&mut self, rest: &str) -> usize {
        let scheme = rest.find("//").expect("an http scheme") + 2;
        let mut end = scheme
            + rest[scheme..]
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || b"-@:%._+~#?&/=".contains(byte))
                .count();
        while end > scheme && matches!(rest.as_bytes()[end - 1], b'.' | b':' | b'?') {
            end -= 1;
        }
        if end == scheme {
            self.text.push_str(&rest[..scheme]);
            return scheme;
        }
        self.text.push('<');
        self.text.push_str(&rest[..end]);
        self.text.push('>');
        end
    }
}

/// The label and destination of a link reference definition line, `[label]: destination`.
fn link_definition(line: &str) -> Option<(String, &str)> {
    let leading = line.len() - line.trim_start_matches(' ').len();
    if leading > 3 {
        return None;
    }
    let rest = &line[leading..];
    if !rest.starts_with('[') {
        return None;
    }
    let close = closing_bracket(rest)?;
    let destination = rest[close + 1..].strip_prefix(':')?.trim();
    let label = &rest[1..close];
    if label.trim().is_empty() || destination.is_empty() {
        return None;
    }
    Some((label.to_owned(), inline_destination(destination)))
}

/// A link label as the definitions are matched: case-folded, whitespace collapsed.
fn label_key(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The destination of an inline link's `(…)` contents, less any title.
fn inline_destination(inside: &str) -> &str {
    let inside = inside.trim();
    if let Some(angled) = inside.strip_prefix('<') {
        return angled.split('>').next().unwrap_or_default();
    }
    inside.split_whitespace().next().unwrap_or_default()
}

/// Whether rustdoc reads `destination` as an intra-doc link: a Rust path, in backticks or not,
/// with an optional disambiguator (`struct@`), call or macro suffix, generics and `#fragment`.
/// Anything holding a `/`, or a character no path or its decorations can, is an ordinary link.
fn is_rust_path(destination: &str) -> bool {
    let destination = destination.trim().trim_matches('`').trim();
    if destination.contains('/') {
        return false;
    }
    let path = destination.split('#').next().unwrap_or_default();
    if path.is_empty()
        || !path
            .chars()
            .all(|ch| ch.is_alphanumeric() || ":_<>, !*&;@()".contains(ch))
    {
        return false;
    }
    let mut depth = 0usize;
    let mut stripped = String::new();
    for ch in path.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => stripped.push(ch),
            _ => {}
        }
    }
    let stripped = stripped.trim();
    !stripped.is_empty() && !stripped.contains(char::is_whitespace)
}

/// The length of the code span `rest` starts with, or of its opening backtick run when nothing
/// closes it, which is then literal text.
fn code_span(rest: &str) -> usize {
    let run = rest.bytes().take_while(|byte| *byte == b'`').count();
    let mut at = run;
    while let Some(offset) = rest[at..].find('`') {
        let start = at + offset;
        let length = rest[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        if length == run {
            return start + length;
        }
        at = start + length;
    }
    run
}

/// The index of the `]` closing the `[` that `rest` starts with, skipping escapes, code spans and
/// nested brackets.
fn closing_bracket(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 1,
            b'`' => {
                at += code_span(&rest[at..]);
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The index of the `)` closing the `(` that `rest` starts with, skipping escapes and nested
/// parentheses; a line end first means it is not an inline link.
fn closing_paren(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 1,
            b'\n' => return None,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The length of the URI or email autolink `rest` starts with.
fn autolink(rest: &str) -> Option<usize> {
    let end = rest.find('>')?;
    let inside = &rest[1..end];
    if inside.is_empty() || inside.contains(|ch: char| ch.is_whitespace() || ch == '<') {
        return None;
    }
    let uri = inside.split_once(':').is_some_and(|(scheme, _)| {
        (2..=32).contains(&scheme.len())
            && scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "+.-".contains(ch))
    });
    let email = inside
        .split_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    (uri || email).then_some(end + 1)
}

/// The length of the HTML comment `rest` starts with.
fn html_comment(rest: &str) -> Option<usize> {
    rest.strip_prefix("<!--")?
        .find("-->")
        .map(|end| "<!--".len() + end + "-->".len())
}

/// The length, lowercased name and kind of the HTML tag `rest` starts with: `<name …>`,
/// `</name>`, or `<name …/>` of a void element, on one line. A non-void element's `<name …/>` is
/// no tag: rustdoc rejects it as an invalid self-closing tag, so its `<` is escaped like any other.
fn html_tag(rest: &str) -> Option<(usize, String, TagKind)> {
    let bytes = rest.as_bytes();
    let close = bytes.get(1) == Some(&b'/');
    let start = if close { 2 } else { 1 };
    if !bytes.get(start)?.is_ascii_alphabetic() {
        return None;
    }
    let name_end = start
        + rest[start..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            .count();
    let end = 1 + rest[1..].find(['>', '<', '\n'])?;
    if bytes[end] != b'>' || end < name_end {
        return None;
    }
    let between = &rest[name_end..end];
    if !between.is_empty() && !between.starts_with(|ch: char| ch.is_whitespace() || ch == '/') {
        return None;
    }
    let name = rest[start..name_end].to_ascii_lowercase();
    let void = VOID_ELEMENTS.contains(&name.as_str());
    let kind = if close {
        TagKind::Close
    } else if between.ends_with('/') {
        if !void {
            return None;
        }
        TagKind::Complete
    } else if void {
        TagKind::Complete
    } else {
        TagKind::Open
    };
    Some((end + 1, name, kind))
}

/// Normalize spec-authored prose for Rust's documentation lint surface, without altering what it
/// says:
///
/// - a line ends at a `\n`, a `\r\n` or a lone `\r`, and every line end becomes `\n`: rustc rejects
///   a bare CR in a doc comment;
/// - every bidirectional formatting control (U+202A–U+202E, U+2066–U+2069) becomes its escape as
///   visible text (`\u{202E}`): rustc's deny-by-default `text_direction_codepoint_in_comment`
///   rejects them in a doc comment;
/// - every tab becomes four spaces: tabs are visually ambiguous in rustdoc and trip Clippy's
///   `tabs_in_doc_comments`, and four spaces preserve table and code alignment;
/// - a continuation line — a non-blank line that follows a list item or block quote without itself
///   starting one — is made explicit, indented to exactly the list item's content column or
///   prefixed with `> `, which is what Clippy's `doc_lazy_continuation` and
///   `doc_overindented_list_items` ask for. A list continuation indented four or more columns past
///   that column is paragraph text, so when its text would open a heading, a thematic break or a
///   setext underline at the content column its first character is escaped (`\#`, `\---`);
/// - lines inside a fenced code block (```` ``` ```` or `~~~`) take no continuation rewrite: past
///   the tab rule, which applies to the whole text, they are left as written. A fence closes only on
///   a fence of its character at least as long, with nothing after it, as in CommonMark.
pub(super) fn normalize_rustdoc(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum Continuation {
        None,
        List(usize),
        Quote,
    }

    let text = escape_bidi_controls(&unify_line_ends(text)).replace('\t', "    ");
    let mut continuation = Continuation::None;
    let mut fence: Option<(u8, usize)> = None;
    let mut output = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        if let Some((marker, length)) = fence {
            if closes_fence(line, marker, length) {
                fence = None;
            }
            output.push(line.to_owned());
            continue;
        }
        if let Some((marker, length, _)) = fence_opener(line) {
            fence = Some((marker, length));
            continuation = Continuation::None;
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
                    let escape = if leading >= indent + 4 && opens_block(trimmed) {
                        "\\"
                    } else {
                        ""
                    };
                    output.push(format!("{}{escape}{trimmed}", " ".repeat(indent)));
                }
            }
        }
    }
    output.join("\n")
}

/// `text` with every `\r\n` and every lone `\r` replaced by `\n`.
fn unify_line_ends(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// `text` with every bidirectional embedding, override and isolate control replaced by its Rust
/// escape, `\u{202E}`, as visible text.
fn escape_bidi_controls(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
            escaped.push_str(&format!("\\u{{{:04X}}}", u32::from(ch)));
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Whether `text`, at the start of a line in a paragraph, ends the paragraph as an ATX heading
/// (`# h`), or is a thematic break or setext underline (`---`, `***`, `___`, `===`).
fn opens_block(text: &str) -> bool {
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes)
        && text[hashes..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        return true;
    }
    let Some(marker) = text.chars().next().filter(|ch| "-*_=".contains(*ch)) else {
        return false;
    };
    text.trim_end().chars().all(|ch| ch == marker || ch == ' ')
}

#[cfg(test)]
mod tests {
    use super::{doc_tokens, normalize_rustdoc, sanitize_carried, MAX_LINK_NESTING};
    use crate::codegen::CodegenOptions;

    /// `doc_tokens` over each row's `(title, summary, description)` yields exactly that row's doc
    /// text, or no attribute for `None`: one row per rule its doc states, and one showing the text
    /// goes through `normalize_rustdoc`.
    #[test]
    fn doc_tokens_follows_the_table() {
        type Row<'a> = (&'a str, [Option<&'a str>; 3], Option<&'a str>);
        let rows: &[Row<'_>] = &[
            ("nothing", [None, None, None], None),
            ("summary alone", [None, Some("S"), None], Some("S")),
            ("description alone", [None, None, Some("D")], Some("D")),
            (
                "summary and description are paragraphs",
                [Some("T"), Some("S"), Some("D")],
                Some("S\n\nD"),
            ),
            (
                "a description repeating the summary is dropped",
                [None, Some("S"), Some("S")],
                Some("S"),
            ),
            (
                "the title only when both are blank",
                [Some("T"), Some(" "), Some("\n")],
                Some("T"),
            ),
            (
                "a blank summary is absent",
                [None, Some("\t"), Some("D")],
                Some("D"),
            ),
            (
                "a blank description is absent",
                [None, Some("S"), Some(" ")],
                Some("S"),
            ),
            ("whitespace alone", [Some(" "), Some(" "), Some(" ")], None),
            (
                "the text is normalized",
                [None, Some("a\tb"), None],
                Some("a    b"),
            ),
        ];
        for (case, [title, summary, description], expected) in rows {
            let docs = crate::ir::Docs {
                title: title.map(str::to_owned),
                summary: summary.map(str::to_owned),
                description: description.map(str::to_owned),
                deprecated: false,
            };
            let expected = match expected {
                Some(text) => quote::quote! { #[doc = #text] },
                None => quote::quote! {},
            };
            assert_eq!(
                doc_tokens(&docs, &CodegenOptions::default()).to_string(),
                expected.to_string(),
                "{case}"
            );
        }
    }

    /// The carried text is sanitized by default and left as written under `verbatim_docs`, and
    /// `normalize_rustdoc` applies either way.
    #[test]
    fn doc_tokens_sanitizes_unless_verbatim() {
        let docs = crate::ir::Docs {
            title: None,
            summary: Some("See [`crate::serve`]\tand https://example.com.".to_owned()),
            description: None,
            deprecated: false,
        };
        let rendered = |verbatim_docs| {
            let options = CodegenOptions {
                verbatim_docs,
                ..CodegenOptions::default()
            };
            doc_tokens(&docs, &options).to_string()
        };
        let expected = |text: &str| quote::quote! { #[doc = #text] }.to_string();
        assert_eq!(
            rendered(false),
            expected("See `crate::serve`    and <https://example.com>.")
        );
        assert_eq!(
            rendered(true),
            expected("See [`crate::serve`]    and https://example.com.")
        );
    }

    /// `sanitize_carried` over each row's input yields exactly that row's output: one row per rule
    /// its doc states, each edge of the link, URL and tag grammars it reads, and the constructs it
    /// keeps as written.
    #[test]
    fn sanitization_follows_the_table() {
        let rows: &[(&str, &str, &str)] = &[
            // Intra-doc links become their text.
            (
                "a code-span path link",
                "Calls [`crate::serve`].",
                "Calls `crate::serve`.",
            ),
            ("a bare path link", "Uses [Store].", "Uses Store."),
            (
                "a multi-segment path",
                "See [Store::get].",
                "See Store::get.",
            ),
            (
                "a disambiguated path",
                "See [struct@Store].",
                "See struct@Store.",
            ),
            ("a function path", "See [serve()].", "See serve()."),
            ("a macro path", "See [vec!].", "See vec!."),
            (
                "a path with a fragment",
                "See [Store#method].",
                "See Store#method.",
            ),
            ("digits read as a path", "Note [1].", "Note 1."),
            (
                "an inline link to a path",
                "See [the store](crate::Store).",
                "See the store.",
            ),
            (
                "a full reference to an undefined path",
                "See [the store][Store].",
                "See the store.",
            ),
            (
                "a collapsed reference to an undefined path",
                "See [`Store`][].",
                "See `Store`.",
            ),
            (
                "a reference definition to a path is dropped, and its uses become text",
                "See [the store].\n\n[the store]: crate::Store",
                "See the store.\n",
            ),
            (
                "generics in a path link are escaped once it is text",
                "Holds [Vec<u8>].",
                "Holds Vec\\<u8>.",
            ),
            // Kept links.
            (
                "an inline link to a URL",
                "See [docs](https://example.com/a).",
                "See [docs](https://example.com/a).",
            ),
            (
                "an inline link to a relative page",
                "See [docs](guide.html).",
                "See [docs](guide.html).",
            ),
            (
                "a fragment-only link",
                "See [below](#errors).",
                "See [below](#errors).",
            ),
            (
                "a reference to a URL definition",
                "See [docs] and [the docs][docs].\n\n[docs]: https://example.com",
                "See [docs] and [the docs][docs].\n\n[docs]: https://example.com",
            ),
            (
                "a reference definition is matched case-insensitively",
                "See [Docs].\n\n[docs]: https://example.com",
                "See [Docs].\n\n[docs]: https://example.com",
            ),
            (
                "bracketed text with a space is no link",
                "Pick [one of these].",
                "Pick [one of these].",
            ),
            ("bracketed text with a dot is no link", "v[1.5]", "v[1.5]"),
            (
                "a path inside bracketed text is still rewritten",
                "Pick [one of [Store] or more].",
                "Pick [one of Store or more].",
            ),
            (
                "a full reference to bracketed text keeps its brackets",
                "See [a b][c d].",
                "See [a b][c d].",
            ),
            ("an unclosed bracket", "a [b", "a [b"),
            (
                "an inline link split over a line end is a shortcut then text",
                "[Store](crate::\nStore)",
                "Store(crate::\nStore)",
            ),
            // Bare URLs.
            (
                "a bare URL",
                "Visit https://example.com/a?b=c#d now.",
                "Visit <https://example.com/a?b=c#d> now.",
            ),
            (
                "trailing punctuation stays outside",
                "At http://example.com/a. Then https://x.io?",
                "At <http://example.com/a>. Then <https://x.io>?",
            ),
            (
                "a URL ends where its characters do",
                "(https://example.com/a)",
                "(<https://example.com/a>)",
            ),
            (
                "a scheme alone",
                "https:// is a scheme",
                "https:// is a scheme",
            ),
            (
                "an autolink is kept",
                "<https://example.com>",
                "<https://example.com>",
            ),
            (
                "an email autolink is kept",
                "<ops@example.com>",
                "<ops@example.com>",
            ),
            (
                "a URL inside a link is kept",
                "[https://example.com](https://example.com)",
                "[https://example.com](https://example.com)",
            ),
            (
                "a URL inside a reference link's text is kept",
                "[see https://x.io][docs]\n\n[docs]: https://example.com",
                "[see https://x.io][docs]\n\n[docs]: https://example.com",
            ),
            (
                "a URL link's text is scanned",
                "See [Vec<T>](https://example.com) and [[Store] docs](https://example.com).",
                "See [Vec\\<T>](https://example.com) and [Store docs](https://example.com).",
            ),
            (
                "a full reference link's text is scanned",
                "See [Vec<T>][docs].\n\n[docs]: https://example.com",
                "See [Vec\\<T>][docs].\n\n[docs]: https://example.com",
            ),
            (
                "a shortcut reference whose text changes becomes a full reference",
                "See [Vec<T>] and [Vec<T>][].\n\n[vec<t>]: https://example.com",
                "See [Vec\\<T>][Vec<T>] and [Vec\\<T>][Vec<T>].\n\n[vec<t>]: https://example.com",
            ),
            // Tags.
            ("an unclosed tag", "Returns Vec<T>.", "Returns Vec\\<T>."),
            ("an unopened closing tag", "a </b> c", "a \\</b> c"),
            (
                "a closed pair is HTML",
                "<b>bold</b> and <em>x</em>",
                "<b>bold</b> and <em>x</em>",
            ),
            (
                "a pair closes case-insensitively",
                "<B>bold</b>",
                "<B>bold</b>",
            ),
            ("a void element", "a<br>b", "a<br>b"),
            (
                "a self-closing void element",
                "a<br/>b<IMG src=x />",
                "a<br/>b<IMG src=x />",
            ),
            (
                "a self-closing tag of a non-void element is escaped",
                "a <T/> b <x a=1 /> c",
                "a \\<T/> b \\<x a=1 /> c",
            ),
            (
                "a closing tag pairs with no self-closing one",
                "<b/>x</b>",
                "\\<b/>x\\</b>",
            ),
            // Line ends.
            (
                "a lone CR ends a line",
                "x\r```\nfn main() {}\n```\r[Store]",
                "x\n```text\nfn main() {}\n```\nStore",
            ),
            (
                "a tag with attributes, closed",
                "<a href=\"https://example.com\">x</a>",
                "<a href=\"https://example.com\">x</a>",
            ),
            (
                "an inner tag left open is escaped, its outer pair kept",
                "<b><i>x</b>",
                "<b>\\<i>x</b>",
            ),
            (
                "a pair across lines is HTML",
                "<details>\nbody\n</details>",
                "<details>\nbody\n</details>",
            ),
            ("an incomplete tag", "a <b c", "a \\<b c"),
            ("a comparison", "a < b and a<3", "a < b and a<3"),
            ("a comment", "<!-- x -->", "<!-- x -->"),
            // Kept as written.
            (
                "a code span",
                "`[Store]` and `Vec<T>` and `https://x.io`",
                "`[Store]` and `Vec<T>` and `https://x.io`",
            ),
            (
                "a double-backtick code span",
                "``a ` [Store]``",
                "``a ` [Store]``",
            ),
            ("an unclosed backtick", "`[Store]", "`Store"),
            // Code blocks: kept, and kept from rustdoc's doctests.
            (
                "a fenced code block, tagged `text`",
                "x\n```\n[Store] Vec<T>\n```\ny [Store]",
                "x\n```text\n[Store] Vec<T>\n```\ny Store",
            ),
            (
                "an untagged fence followed by spaces is untagged",
                "```  \n{\"id\":1}\n```",
                "```text\n{\"id\":1}\n```",
            ),
            (
                "an untagged `~~~` fence, indented, is tagged",
                "  ~~~\nfn main() {}\n  ~~~",
                "  ~~~text\nfn main() {}\n  ~~~",
            ),
            (
                "a tagged fence is kept",
                "```json\n[Store]\n```\n```rust\nVec<T>\n```",
                "```json\n[Store]\n```\n```rust\nVec<T>\n```",
            ),
            (
                "a backtick run with a backtick after it is a code span, not a fence",
                "```a``` [Store]",
                "```a``` Store",
            ),
            (
                "a fence closes only on one at least as long",
                "````\n```\nVec<T>\n```\n````\n[Store]",
                "````text\n```\nVec<T>\n```\n````\nStore",
            ),
            (
                "a longer fence closes a shorter one",
                "```\nx\n`````\n[Store]",
                "```text\nx\n`````\nStore",
            ),
            (
                "a fence with an info string closes nothing",
                "```\n```json\nVec<T>\n```\n[Store]",
                "```text\n```json\nVec<T>\n```\nStore",
            ),
            (
                "the other fence character closes nothing",
                "~~~\n```\nVec<T>\n~~~\n[Store]",
                "~~~text\n```\nVec<T>\n~~~\nStore",
            ),
            (
                "an unclosed fence is closed at the end",
                "```\nVec<T>",
                "```text\nVec<T>\n```",
            ),
            (
                "an unclosed fence is closed before a trailing newline",
                "x\n  ````json\nVec<T>\n",
                "x\n  ````json\nVec<T>\n  ````\n",
            ),
            (
                "an unclosed fence at a list item's content indentation is closed there",
                "- a\n\n    ```json\n    {}",
                "- a\n\n    ```json\n    {}\n    ```",
            ),
            (
                "an indented code block becomes a fenced one",
                "x\n\n    [Store] Vec<T>\n\ny",
                "x\n\n```text\n[Store] Vec<T>\n```\n\ny",
            ),
            (
                "an indented code block keeps its inner blank lines and indentation",
                "    a\n\n      b\n\n\nc",
                "```text\na\n\n  b\n```\n\n\nc",
            ),
            (
                "a tab indents a code block",
                "\tcurl https://example.com/b",
                "```text\ncurl https://example.com/b\n```",
            ),
            (
                "an indented code block holding a fence takes a longer one",
                "    ```\n    x\n    ```",
                "````text\n```\nx\n```\n````",
            ),
            (
                "an indented code block ends at the next unindented line",
                "    a\nb [Store]",
                "```text\na\n```\nb Store",
            ),
            (
                "indented list content is prose",
                "- a\n\n    [Store]",
                "- a\n\n    Store",
            ),
            (
                "an indented code block under a list item is fenced at its column",
                "- a\n\n      curl https://example.com/c\n- b",
                "- a\n\n  ```text\n  curl https://example.com/c\n  ```\n- b",
            ),
            (
                "an indented line straight after a paragraph is prose",
                "x\n    [Store]",
                "x\n    Store",
            ),
            (
                "an indented line straight after a fence is prose",
                "```\nx\n```\n    [Store]",
                "```text\nx\n```\n    Store",
            ),
            (
                "task-list markers are no links",
                "- [x] done\n- [ ] open\n1. [X] [Store]",
                "- [x] done\n- [ ] open\n1. [X] Store",
            ),
            ("a lone `[x]` mid-line is a path", "a [x] b", "a x b"),
            (
                "backslash escapes",
                "\\[Store\\] \\<T>",
                "\\[Store\\] \\<T>",
            ),
            ("non-ASCII text", "Grüße [Store] — ok", "Grüße Store — ok"),
            // Line structure.
            ("a trailing newline is kept", "a\n", "a\n"),
            ("an empty text", "", ""),
        ];
        for (case, input, expected) in rows {
            assert_eq!(sanitize_carried(input), *expected, "{case}: {input:?}");
            assert_eq!(
                sanitize_carried(expected),
                *expected,
                "{case}: sanitizing is idempotent"
            );
        }
    }

    /// Brackets nested to [`MAX_LINK_NESTING`] are read as links, the innermost path link
    /// reduced to its text; one level more escapes the innermost `[`.
    #[test]
    fn link_nesting_is_read_to_the_bound_and_escaped_past_it() {
        let nested = |depth: usize| format!("{}x{}", "[".repeat(depth), "]".repeat(depth));
        let at = MAX_LINK_NESTING;
        assert_eq!(
            sanitize_carried(&nested(at)),
            format!("{}x{}", "[".repeat(at - 1), "]".repeat(at - 1)),
            "at the bound"
        );
        assert_eq!(
            sanitize_carried(&nested(at + 1)),
            format!("{}\\[x]{}", "[".repeat(at), "]".repeat(at)),
            "one past the bound"
        );
    }

    /// A description of brackets nested far past the bound, as long as the one that overflowed the
    /// stack, is sanitized on a test thread's stack, with every `[` past the bound escaped.
    #[test]
    fn deeply_nested_brackets_do_not_exhaust_the_stack() {
        let depth = 60_000;
        let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let sanitized = sanitize_carried(&text);
        assert_eq!(
            sanitized.matches("\\[").count(),
            depth - MAX_LINK_NESTING,
            "every `[` past the bound is escaped"
        );
    }

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
                "a continuation past the content column is clamped to it",
                "1. item\n      more",
                "1. item\n   more",
            ),
            (
                "an over-indented bullet continuation is clamped",
                "- a\n        over indented\n   and more",
                "- a\n  over indented\n  and more",
            ),
            (
                "a nested item's over-indented continuation is clamped to its column",
                "- a\n  - b\n          c",
                "- a\n  - b\n    c",
            ),
            (
                "a clamped paragraph line that would open a heading is escaped",
                "- a\n      # not a heading",
                "- a\n  \\# not a heading",
            ),
            (
                "a clamped paragraph line that would underline or break is escaped",
                "- a\n      ---\n- b\n      ===\n- c\n      ___",
                "- a\n  \\---\n- b\n  \\===\n- c\n  \\___",
            ),
            (
                "a clamped line indented less than four past the column keeps its meaning",
                "- a\n   # h",
                "- a\n  # h",
            ),
            (
                "a clamped line that opens no block is not escaped",
                "- a\n      #hashtag and -x",
                "- a\n  #hashtag and -x",
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
            (
                "a marker with no digits is no list",
                "> q\n. a\n) b",
                "> q\n> . a\n> ) b",
            ),
            (
                "digits with no marker are no list, however many spaces follow",
                "> q\n2024  was",
                "> q\n> 2024  was",
            ),
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
                "a fence closes only on one at least as long",
                "````\n```\n- a\nb\n````\n- c\nd",
                "````\n```\n- a\nb\n````\n- c\n  d",
            ),
            (
                "a fence with an info string closes nothing",
                "```\n```rust\n- a\nb\n```\n- c\nd",
                "```\n```rust\n- a\nb\n```\n- c\n  d",
            ),
            (
                "a backtick run with a backtick after it is no fence",
                "- a\n```x`\nb",
                "- a\n  ```x`\n  b",
            ),
            (
                "an unclosed fence runs to the end",
                "```\n- a\nb",
                "```\n- a\nb",
            ),
            // Line structure.
            ("a trailing newline is dropped", "a\n", "a"),
            ("CRLF line ends become LF", "- a\r\nb", "- a\n  b"),
            (
                "a lone CR ends a line",
                "First.\rSecond.\nThird.\r\n- a\rb",
                "First.\nSecond.\nThird.\n- a\n  b",
            ),
            // Bidirectional controls.
            (
                "every embedding, override and isolate control is escaped",
                "a\u{202A}b\u{202B}c\u{202C}d\u{202D}e\u{202E}f\u{2066}g\u{2067}h\u{2068}i\u{2069}j",
                "a\\u{202A}b\\u{202B}c\\u{202C}d\\u{202D}e\\u{202E}f\\u{2066}g\\u{2067}h\\u{2068}i\\u{2069}j",
            ),
            (
                "a control inside a fence is escaped too",
                "```\n\u{202E}x\n```",
                "```\n\\u{202E}x\n```",
            ),
            (
                "a direction mark is no control, and is kept",
                "a\u{200E}b\u{200F}c",
                "a\u{200E}b\u{200F}c",
            ),
            ("an empty text", "", ""),
        ];
        for (case, input, expected) in rows {
            assert_eq!(normalize_rustdoc(input), *expected, "{case}: {input:?}");
        }
    }
}
