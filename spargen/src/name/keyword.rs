use super::{to_pascal_case, to_snake_case, Ident};

/// The syntactic role an identifier plays, which governs both its casing and how keywords are
/// escaped (PRD D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentRole {
    /// A type name (`PascalCase`).
    Type,
    /// A struct field (`snake_case`).
    Field,
    /// An enum variant (`PascalCase`).
    Variant,
    /// A method name (`snake_case`).
    Method,
    /// A module name (`snake_case`).
    Module,
    /// A function parameter (`snake_case`).
    Param,
}

/// Produce a legal Rust [`Ident`] for `raw` in the given `role`: cased per the role, with Rust
/// keywords escaped as raw identifiers (`r#type`) where legal and via a trailing underscore
/// otherwise, and leading digits / invalid starts repaired (PRD D9).
pub fn escape(raw: &str, role: IdentRole) -> Ident {
    let mut ident = match role {
        IdentRole::Type | IdentRole::Variant => to_pascal_case(raw),
        IdentRole::Field | IdentRole::Method | IdentRole::Module | IdentRole::Param => {
            to_snake_case(raw)
        }
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

fn is_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}

fn can_raw_escape(ident: &str) -> bool {
    !matches!(
        ident,
        "self" | "Self" | "super" | "crate" | "true" | "false"
    )
}

#[cfg(test)]
mod tests {
    use super::{escape, IdentRole};

    #[test]
    fn escapes_field_keywords_with_raw_identifier() {
        assert_eq!(escape("type", IdentRole::Field).as_str(), "r#type");
    }

    #[test]
    fn repairs_digits_and_special_keywords() {
        assert_eq!(escape("123-name", IdentRole::Field).as_str(), "n_123_name");
        assert_eq!(escape("self", IdentRole::Field).as_str(), "self_");
    }
}
