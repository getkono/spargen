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
/// cannot blow up. Used for bulk omit rules and (via [`glob_match`]) documented in the module docs.
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
