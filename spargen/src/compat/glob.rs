//! The glob matcher omit rules are read with: which patterns are globs, how a backslash escapes a
//! metacharacter, and what `*`, `**`, and `?` match. See the module documentation of
//! [`compat`](super) for how a glob rule differs from an exact one.

/// Whether `pattern` contains an *unescaped* glob metacharacter (`*` or `?`) and should be matched
/// as a glob rather than compared exactly.
///
/// A metacharacter is escapable with a backslash, because a URI path may legitimately contain `*`
/// (RFC 3986 lists it as a sub-delimiter) and without an escape such a path is unaddressable: an
/// exact rule for it cannot be written, and auto-carve would silently widen into a bulk rule.
pub(super) fn has_glob_meta(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '*' | '?' => return true,
            _ => {}
        }
    }
    false
}

/// Resolve the escapes in a pattern with no unescaped metacharacter, so an exact rule compares
/// against the literal text the author meant. A backslash escapes whatever follows it — `\*` →
/// `*`, `\?` → `?`, `\\` → `\`, and `\b` → `b` — exactly as [`compile_glob`] reads it, so an exact
/// rule and a glob rule never disagree on what a pattern names. A trailing lone backslash escapes
/// nothing and is kept as itself.
pub(super) fn unescape_glob(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            out.push(chars.next().unwrap_or('\\'));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Escape every glob metacharacter in literal document text, so a rule built from it targets that
/// one construct instead of being reinterpreted as a bulk pattern.
pub(super) fn escape_glob_meta(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// One compiled glob token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    /// A literal character.
    Lit(char),
    /// `?` — exactly one character other than `/`.
    Any,
    /// `*` — zero or more characters, none of which is `/` (a single path/name segment).
    Star,
    /// `**` — zero or more characters, including `/` (any depth).
    DoubleStar,
}

fn compile_glob(pattern: &str) -> Vec<GlobToken> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            // A backslash escapes the next character, so a literal `*`/`?` in a path stays literal.
            '\\' if i + 1 < chars.len() => {
                tokens.push(GlobToken::Lit(chars[i + 1]));
                i += 2;
            }
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                tokens.push(GlobToken::DoubleStar);
                i += 2;
            }
            '*' => {
                tokens.push(GlobToken::Star);
                i += 1;
            }
            '?' => {
                tokens.push(GlobToken::Any);
                i += 1;
            }
            other => {
                tokens.push(GlobToken::Lit(other));
                i += 1;
            }
        }
    }
    tokens
}

/// Match `text` against a glob `pattern`. `/`-aware: `*` and `?` never cross a `/` segment
/// separator, while `**` matches across any depth. Polynomial (memoized) so pathological patterns
/// cannot blow up. Used for bulk omit rules; the module documentation of [`compat`](super) says
/// which rules are read as globs.
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
    let tokens = compile_glob(pattern);
    let text: Vec<char> = text.chars().collect();
    let mut memo = vec![vec![None; text.len() + 1]; tokens.len() + 1];
    glob_match_at(&tokens, &text, 0, 0, &mut memo)
}

fn glob_match_at(
    tokens: &[GlobToken],
    text: &[char],
    ti: usize,
    pi: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pi][ti] {
        return cached;
    }
    let result = match tokens.get(pi) {
        None => ti == text.len(),
        Some(GlobToken::Lit(expected)) => {
            text.get(ti) == Some(expected) && glob_match_at(tokens, text, ti + 1, pi + 1, memo)
        }
        Some(GlobToken::Any) => {
            matches!(text.get(ti), Some(&ch) if ch != '/')
                && glob_match_at(tokens, text, ti + 1, pi + 1, memo)
        }
        Some(GlobToken::Star) => {
            glob_match_at(tokens, text, ti, pi + 1, memo)
                || matches!(text.get(ti), Some(&ch) if ch != '/')
                    && glob_match_at(tokens, text, ti + 1, pi, memo)
        }
        Some(GlobToken::DoubleStar) => {
            glob_match_at(tokens, text, ti, pi + 1, memo)
                || (ti < text.len() && glob_match_at(tokens, text, ti + 1, pi, memo))
        }
    };
    memo[pi][ti] = Some(result);
    result
}

#[cfg(test)]
mod tests {
    use super::{escape_glob_meta, glob_match, has_glob_meta, unescape_glob};
    use proptest::prelude::*;

    /// Literals match exactly, `*` within one segment, `**` across any depth, `?` one non-`/`
    /// character, and a pattern with none of them is no glob.
    #[test]
    fn each_glob_metacharacter_matches_what_it_documents() {
        // Exact literals.
        assert!(glob_match("/pets", "/pets"));
        assert!(!glob_match("/pets", "/pet"));
        // `*` matches within one segment but never crosses `/`.
        assert!(glob_match("/pets/*", "/pets/dog"));
        assert!(!glob_match("/pets/*", "/pets/dog/paw"));
        assert!(glob_match("Legacy*", "LegacyPet"));
        assert!(glob_match("*Pet", "LegacyPet"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*", "a/b"));
        // `**` matches across any depth.
        assert!(glob_match("/admin/**", "/admin/users"));
        assert!(glob_match("/admin/**", "/admin/users/{id}"));
        assert!(!glob_match("/admin/**", "/public/users"));
        // `?` matches exactly one non-`/` character.
        assert!(glob_match("/pet?", "/pets"));
        assert!(!glob_match("/pet?", "/pet"));
        assert!(!glob_match("/pet?", "/pe/s"));
        // No metacharacter ⇒ not treated as a glob.
        assert!(!has_glob_meta("/pets/{id}"));
        assert!(has_glob_meta("/pets/*"));
        assert!(has_glob_meta("/pet?"));
    }

    /// The escape half of the matcher, pinned against mutation testing: a scoped `cargo mutants`
    /// run over this file left every arithmetic and guard mutant in `compile_glob` and the
    /// backslash arm of `has_glob_meta` alive, because the integration tests exercise escaping
    /// only through paths where an escaped and an unescaped rule happen to remove the same
    /// construct. These assert the two functions directly.
    #[test]
    fn escaped_metacharacters_are_literal_and_cannot_run_off_the_pattern() {
        // An escaped metacharacter is not a metacharacter, so the rule stays exact.
        assert!(!has_glob_meta(r"\*"));
        assert!(!has_glob_meta(r"\?"));
        assert!(!has_glob_meta(r"/files/\*"));
        // Escaping is per-character: a second, unescaped one still makes it a glob.
        assert!(has_glob_meta(r"/files/\*/*"));
        // An escaped backslash consumes itself, so the `*` after it is live again.
        assert!(has_glob_meta(r"\\*"));
        // A trailing lone backslash escapes nothing and must not read past the end.
        assert!(!has_glob_meta(r"/files/\"));

        // The escaped character matches itself, and nothing else.
        assert!(glob_match(r"/files/\*", "/files/*"));
        assert!(!glob_match(r"/files/\*", "/files/other"));
        assert!(glob_match(r"/files/\?", "/files/?"));
        assert!(!glob_match(r"/files/\?", "/files/x"));
        // An escaped backslash is one literal backslash.
        assert!(glob_match(r"/files/\\", r"/files/\"));
        // `**` still spans depth when it is not escaped, and its first `*` escapes cleanly.
        assert!(glob_match(r"/a/**", "/a/b/c"));
        assert!(glob_match(r"/a/\**", "/a/*b"));
        // A `*` that is not the final character must stay a single-segment star: reading the
        // lookahead off by one turns every `*` into `**` and silently widens the rule.
        assert!(glob_match("/pets/*/paw", "/pets/dog/paw"));
        assert!(!glob_match("/pets/*/paw", "/pets/dog/left/paw"));
        // `**` must consume both of its characters, so what follows it still has to match.
        assert!(glob_match("/a/**/z", "/a/b/c/z"));
        assert!(!glob_match("/a/**/z", "/a/b/c/y"));
        assert!(glob_match("/a/**x", "/a/b/cx"));
        assert!(!glob_match("/a/**/z", "/a/bz"));
        // A pattern that is nothing but a trailing backslash compiles and terminates.
        assert!(glob_match(r"\", r"\"));
        // A pattern opening with an escape must not index before its start.
        assert!(glob_match(r"\*x", "*x"));
    }

    proptest! {
        /// Escaping literal text yields a rule that matches exactly that text.
        #[test]
        fn an_escaped_text_matches_itself(text in r"[a-c/*?\\]{0,12}") {
            let pattern = escape_glob_meta(&text);
            prop_assert!(!has_glob_meta(&pattern), "{pattern:?}");
            prop_assert!(glob_match(&pattern, &text), "{pattern:?} vs {text:?}");
            prop_assert_eq!(unescape_glob(&pattern), text);
        }

        /// A pattern with no unescaped metacharacter matches as a glob exactly the text its
        /// exact-rule reading names, so the two forms can never disagree.
        #[test]
        fn a_meta_free_pattern_matches_exactly_its_unescaped_text(
            // Ordinary characters and escapes of anything, then an optional trailing backslash:
            // every pattern with no unescaped metacharacter, built rather than filtered.
            pattern in r"([a-c/]|\\[a-c/*?\\]){0,8}\\?",
            text in r"[a-c/*?\\]{0,12}",
        ) {
            prop_assert!(!has_glob_meta(&pattern), "{pattern:?}");
            let literal = unescape_glob(&pattern);
            prop_assert!(glob_match(&pattern, &literal), "{pattern:?} vs {literal:?}");
            prop_assert_eq!(glob_match(&pattern, &text), literal == text);
        }
    }
}
