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
        (
            "a fence on a list item's line keeps its code as written",
            [
                None,
                None,
                Some("- ```yaml\n  a:\n      b: 1\n      # c\n  ```"),
            ],
            Some("- ```yaml\n  a:\n      b: 1\n      # c\n  ```"),
        ),
        (
            "an over-indented list continuation is paragraph text, not a fence",
            [None, None, Some("- a\n        - ```\n  b")],
            Some("- a\n  \\- ```\n  b"),
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
            "an untagged fence on a list item's line is tagged, and its lines kept",
            "- ```\n  fn main() {}\n  Vec<T> [Store]\n  ```\nb [Store]",
            "- ```text\n  fn main() {}\n  Vec<T> [Store]\n  ```\nb Store",
        ),
        (
            "an over-indented list continuation opens no fence on a list item's line",
            "- a\n        - ```\n  b",
            "- a\n        - ```\n  b",
        ),
        (
            "an over-indented list continuation leaves the item's content column",
            "- a\n        - b\n\n      [Store]",
            "- a\n        - b\n\n  ```text\n  [Store]\n  ```",
        ),
        (
            "a block quote ends the list item's paragraph, so a fence opens on the item after it",
            "- a\n> q\n        - ```",
            "- a\n> q\n        - ```text\n          ```",
        ),
        (
            "an unclosed fence on a list item's line is closed at its content column",
            "1. ~~~json\n   {}",
            "1. ~~~json\n   {}\n   ~~~",
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
                "a clamped paragraph line that would open a bullet item is escaped",
                "- a\n        - b\n      * c\n      + d",
                "- a\n  \\- b\n  \\* c\n  \\+ d",
            ),
            (
                "a clamped paragraph line that would open an ordered item is escaped",
                "- a\n        1. b\n        2) c",
                "- a\n  1\\. b\n  2\\) c",
            ),
            (
                "a clamped paragraph line that would open a quote is escaped",
                "- a\n        > b",
                "- a\n  \\> b",
            ),
            (
                "a nested item indented less than four past the column stays an item",
                "- a\n     - b\n  c",
                "- a\n     - b\n       c",
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
                "a fence opened on a list item's line takes no rewrite",
                "- ```yaml\n  a:\n      b: 1\n      # c\n  ```\nd",
                "- ```yaml\n  a:\n      b: 1\n      # c\n  ```\nd",
            ),
            (
                "a fence on an ordered item's line closes only on one at least as long",
                "1. ````\n   ```\n   - x\n   ````\n- c\nd",
                "1. ````\n   ```\n   - x\n   ````\n- c\n  d",
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
