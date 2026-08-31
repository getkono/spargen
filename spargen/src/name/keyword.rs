use super::{to_pascal_case, to_snake_case, Ident};

/// The syntactic role an identifier plays, which governs both its casing and how keywords are
/// escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentRole {
    /// A type name (`PascalCase`).
    Type,
    /// A struct field (`snake_case`).
    Field,
    /// An enum variant (`PascalCase`).
    Variant,
    /// A method name (`snake_case`).
    Method,
    /// A function parameter (`snake_case`).
    Param,
}

/// Produce a legal Rust [`Ident`] for `raw` in the given `role`: cased per the role, with Rust
/// keywords escaped as raw identifiers (`r#type`) where legal and via a trailing underscore
/// otherwise, and leading digits / invalid starts repaired.
pub(crate) fn escape(raw: &str, role: IdentRole) -> Ident {
    let mut ident = match role {
        IdentRole::Type | IdentRole::Variant => to_pascal_case(raw),
        IdentRole::Field | IdentRole::Method | IdentRole::Param => to_snake_case(raw),
    };

    ident.retain(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    if ident.is_empty() {
        ident = match role {
            IdentRole::Type | IdentRole::Variant => "Generated".to_owned(),
            _ => "generated".to_owned(),
        };
    }

    if ident.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        ident = match role {
            IdentRole::Type | IdentRole::Variant => format!("N{ident}"),
            _ => format!("n_{ident}"),
        };
    }

    if is_keyword(&ident) {
        if can_raw_escape(&ident) && !matches!(role, IdentRole::Type | IdentRole::Variant) {
            ident = format!("r#{ident}");
        } else {
            ident.push('_');
        }
    }

    Ident::new(ident)
}

/// Every word that may not be spelled as a bare Rust identifier, in any edition spargen's output
/// has to compile in. Generated code is `include!`d into a consumer crate and inherits *that*
/// crate's edition, not spargen's, so this is the union across editions 2015-2024 rather than the
/// set for any single one.
///
/// Deliberately absent: the weak keywords `macro_rules`, `union`, `safe`, `raw`, and `dyn` before
/// 2018 are legal identifiers everywhere spargen emits one, and escaping them would rename fields
/// for no reason.
///
/// The tests read this same const, so it cannot disagree with `is_keyword` — but that also means
/// they cannot check its contents against an independent source, and the two failure modes need
/// different guards. A *missing* word is only caught by compiling generated output under the
/// edition that reserves it, which `spargen/tests/e2e.rs` does for 2024. A *deleted* word is caught
/// here: `the_table_holds_every_keyword_it_started_with` pins the count, and `KNOWN_KEYWORDS`
/// spot-checks a sample spelled out independently below.
const KEYWORDS: &[&str] = &[
    // Strict, edition 2015.
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", //
    // Strict, added in edition 2018.
    "async", "await", "dyn", //
    // Reserved for future use, edition 2015.
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual", "yield", //
    // Reserved for future use, added in edition 2018.
    "try", //
    // Reserved for future use, added in edition 2024 (`gen` blocks).
    "gen",
];

fn is_keyword(ident: &str) -> bool {
    KEYWORDS.contains(&ident)
}

fn can_raw_escape(ident: &str) -> bool {
    !matches!(
        ident,
        "self" | "Self" | "super" | "crate" | "true" | "false"
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{escape, is_keyword, IdentRole, KEYWORDS};

    /// Every role `escape` accepts. Written out rather than derived so a new role has to be
    /// classified here before the property tests silently stop covering it.
    const ROLES: &[IdentRole] = &[
        IdentRole::Type,
        IdentRole::Variant,
        IdentRole::Field,
        IdentRole::Method,
        IdentRole::Param,
    ];

    /// Does `text` lex as exactly one Rust identifier token? This is the real question — `Ident`'s
    /// `ToTokens` hands the text to `proc_macro2::Ident::new`, which *panics* on anything else, so
    /// an identifier that is merely "probably fine" is a panic in a consumer's build script.
    fn lexes_as_one_identifier(text: &str) -> bool {
        let Ok(stream) = text.parse::<proc_macro2::TokenStream>() else {
            return false;
        };
        let mut tokens = stream.into_iter();
        let Some(proc_macro2::TokenTree::Ident(ident)) = tokens.next() else {
            return false;
        };
        tokens.next().is_none() && ident == text
    }

    #[test]
    fn escapes_field_keywords_with_raw_identifier() {
        assert_eq!(escape("type", IdentRole::Field).as_str(), "r#type");
    }

    #[test]
    fn repairs_digits_and_special_keywords() {
        assert_eq!(escape("123-name", IdentRole::Field).as_str(), "n_123_name");
        assert_eq!(escape("self", IdentRole::Field).as_str(), "self_");
    }

    /// `Ident::new` takes "already-validated text" on trust, and `escape` is what does the
    /// validating. The six keywords that cannot be raw-escaped (`self`, `Self`, `super`, `crate`,
    /// `true`, `false`) must take the trailing-underscore path instead; the rest may take either.
    #[test]
    fn every_keyword_escapes_to_a_legal_identifier_in_every_role() {
        for keyword in KEYWORDS {
            for role in ROLES {
                let ident = escape(keyword, *role);
                let text = ident.as_str();
                assert!(
                    lexes_as_one_identifier(text),
                    "escape({keyword:?}, {role:?}) produced {text:?}, which is not an identifier"
                );
                // Stronger than "did not return its own input": escaping one keyword into a
                // *different* keyword would also be a bug.
                assert!(
                    !is_keyword(text),
                    "escape({keyword:?}, {role:?}) returned the keyword {text:?}"
                );
            }
        }
    }

    /// `gen` is an ordinary identifier in edition 2021 and reserved in 2024. Generated output
    /// adopts the consumer's edition, so it has to be escaped even though every test oracle in this
    /// crate — `proc_macro2`, compiled at edition 2021 — considers the bare word perfectly valid.
    /// The end-to-end proof is `a_gen_named_spec_compiles_under_edition_2024` in `e2e.rs`.
    #[test]
    fn gen_is_escaped_although_edition_2021_would_accept_it() {
        for role in [IdentRole::Field, IdentRole::Method, IdentRole::Param] {
            assert_eq!(escape("gen", role).as_str(), "r#gen", "{role:?}");
        }
        // A type or variant cases to `Gen`, which is not a keyword at all.
        for role in [IdentRole::Type, IdentRole::Variant] {
            assert_eq!(escape("gen", role).as_str(), "Gen", "{role:?}");
        }
    }

    /// A sample of the table, written out here rather than read from `KEYWORDS`, so these words
    /// cannot be dropped from it unnoticed. Deliberately small: a full second copy is what this
    /// module used to carry, and it bought deletion-detection at the price of an omission blind
    /// spot in both lists at once. The count assertion below covers deletion in general; this
    /// covers the words whose loss would hurt most.
    const KNOWN_KEYWORDS: &[&str] = &[
        "as", "fn", "type", "match", "self", "Self", "async", "dyn", "try", "yield", "become",
        "gen",
    ];

    #[test]
    fn a_known_keyword_is_never_left_bare() {
        for word in KNOWN_KEYWORDS {
            assert!(
                is_keyword(word),
                "`{word}` is a Rust keyword and left the table"
            );
            for role in ROLES {
                let escaped = escape(word, *role);
                assert_ne!(
                    escaped.as_str(),
                    *word,
                    "escape({word:?}, {role:?}) left the keyword bare"
                );
            }
        }
    }

    /// `is_keyword` and the test that iterates it now read one list, so a word deleted from that
    /// list is simply never tested. The count is the guard: adding or removing an entry is a
    /// deliberate act that has to update this number, and a reviewer sees it in the diff.
    #[test]
    fn the_table_holds_every_keyword_it_started_with() {
        assert_eq!(
            KEYWORDS.len(),
            52,
            "the keyword table changed size; update this count only alongside a deliberate edit"
        );
    }

    /// A `matches!` chain warns on a repeated arm; a flat slice does not, so the duplicate check
    /// that the old table got from the compiler has to be written down.
    #[test]
    fn the_keyword_table_has_no_duplicates() {
        let unique: std::collections::BTreeSet<&str> = KEYWORDS.iter().copied().collect();
        assert_eq!(
            unique.len(),
            KEYWORDS.len(),
            "duplicate keyword in the table"
        );
    }

    #[test]
    fn a_type_never_takes_the_raw_escape_path() {
        // `r#Type` is legal Rust, but a raw type name reads as a mistake in generated output, so
        // types and variants always take the trailing underscore.
        for keyword in KEYWORDS {
            for role in [IdentRole::Type, IdentRole::Variant] {
                let ident = escape(keyword, role);
                assert!(
                    !ident.as_str().starts_with("r#"),
                    "escape({keyword:?}, {role:?}) produced {ident}"
                );
            }
        }
    }

    #[test]
    fn an_empty_or_wholly_unusable_hint_falls_back_per_role() {
        for hint in ["", "-", "   ", "!!!", "…"] {
            assert_eq!(
                escape(hint, IdentRole::Type).as_str(),
                "Generated",
                "{hint:?}"
            );
            assert_eq!(
                escape(hint, IdentRole::Field).as_str(),
                "generated",
                "{hint:?}"
            );
        }
    }

    proptest! {
        /// The invariant `Ident`'s constructor documents but does not check: whatever a spec puts
        /// in a name position, `escape` returns something Rust will accept.
        #[test]
        fn escape_always_yields_a_legal_identifier(raw in ".{0,48}") {
            for role in ROLES {
                let ident = escape(&raw, *role);
                prop_assert!(
                    lexes_as_one_identifier(ident.as_str()),
                    "escape({raw:?}, {role:?}) produced {:?}",
                    ident.as_str()
                );
            }
        }

        /// Byte-identical output requires identifier allocation to be a pure function of its
        /// inputs. Nothing asserted this, and it is the first thing a hash-ordered rewrite breaks.
        #[test]
        fn escape_is_deterministic(raw in ".{0,48}") {
            for role in ROLES {
                prop_assert_eq!(
                    escape(&raw, *role).as_str().to_owned(),
                    escape(&raw, *role).as_str().to_owned()
                );
            }
        }

        /// Casing is a role property: two roles that share a casing must agree, and the two
        /// families must not collide on spelling by accident.
        #[test]
        fn roles_sharing_a_casing_produce_the_same_identifier(raw in "[A-Za-z][A-Za-z0-9_ -]{0,24}") {
            prop_assert_eq!(
                escape(&raw, IdentRole::Type).as_str().to_owned(),
                escape(&raw, IdentRole::Variant).as_str().to_owned()
            );
            prop_assert_eq!(
                escape(&raw, IdentRole::Field).as_str().to_owned(),
                escape(&raw, IdentRole::Method).as_str().to_owned()
            );
            prop_assert_eq!(
                escape(&raw, IdentRole::Field).as_str().to_owned(),
                escape(&raw, IdentRole::Param).as_str().to_owned()
            );
        }
    }
}
