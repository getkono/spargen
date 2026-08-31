//! Casing conversions for identifier allocation. Segmentation keeps ASCII alphanumerics and treats
//! every other character as a separator, so the output is always a subset of the XID characters a
//! Rust identifier admits — a non-ASCII hint segments to nothing and takes the caller's fallback
//! rather than producing an identifier that only some editions accept. `heck` is deliberately not
//! used: its boundary rules differ, and identifier allocation needs a segmentation this crate pins.

/// Convert `raw` to `PascalCase` (for types and variants).
pub(crate) fn to_pascal_case(raw: &str) -> String {
    let words = words(raw);
    if words.is_empty() {
        return "Generated".to_owned();
    }
    words
        .into_iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Convert `raw` to `snake_case` (for fields, methods, modules).
pub(crate) fn to_snake_case(raw: &str) -> String {
    let words = words(raw);
    if words.is_empty() {
        "generated".to_owned()
    } else {
        words.join("_")
    }
}

fn words(raw: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lowercase = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            let is_upper = ch.is_ascii_uppercase();
            if is_upper && previous_lowercase && !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            current.push(ch.to_ascii_lowercase());
            previous_lowercase = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lowercase = false;
        }
    }

    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{to_pascal_case, to_snake_case, words};

    #[test]
    fn segmentation_splits_on_separators_and_camel_boundaries() {
        // The table the two public conversions are built on. A camel boundary is a lowercase (or
        // digit) followed by an uppercase; every non-alphanumeric is a separator.
        for (raw, expected) in [
            ("petId", vec!["pet", "id"]),
            ("PetId", vec!["pet", "id"]),
            ("pet_id", vec!["pet", "id"]),
            ("pet-id", vec!["pet", "id"]),
            ("pet id", vec!["pet", "id"]),
            ("pet/id", vec!["pet", "id"]),
            // A run of capitals is one word: the boundary needs a preceding lowercase.
            ("HTTPResponse", vec!["httpresponse"]),
            ("parseHTTPResponse", vec!["parse", "httpresponse"]),
            ("v2Api", vec!["v2", "api"]),
            ("__leading", vec!["leading"]),
            ("trailing__", vec!["trailing"]),
            ("", vec![]),
            ("---", vec![]),
        ] {
            assert_eq!(words(raw), expected, "words({raw:?})");
        }
    }

    #[test]
    fn conversions_fall_back_when_nothing_survives_segmentation() {
        // Not cosmetic: the fallback is what keeps `escape` from having to invent a name, and the
        // two spellings differ by casing family.
        assert_eq!(to_pascal_case(""), "Generated");
        assert_eq!(to_snake_case(""), "generated");
        assert_eq!(to_pascal_case("-/-"), "Generated");
        assert_eq!(to_snake_case("-/-"), "generated");
    }

    #[test]
    fn conversions_case_each_word() {
        assert_eq!(to_pascal_case("pet_id"), "PetId");
        assert_eq!(to_snake_case("petId"), "pet_id");
        assert_eq!(to_pascal_case("HTTPResponse"), "Httpresponse");
        assert_eq!(to_snake_case("v2Api"), "v2_api");
    }

    proptest! {
        /// Both conversions feed identifier allocation, so byte-identical output depends on them
        /// being pure functions.
        #[test]
        fn conversions_are_deterministic(raw in ".{0,48}") {
            prop_assert_eq!(to_pascal_case(&raw), to_pascal_case(&raw));
            prop_assert_eq!(to_snake_case(&raw), to_snake_case(&raw));
        }

        /// Segmentation keeps only ASCII alphanumerics, so a conversion can never introduce a
        /// character that `escape` would then have to strip.
        #[test]
        fn conversions_emit_only_ascii_alphanumerics_and_underscores(raw in ".{0,48}") {
            for produced in [to_pascal_case(&raw), to_snake_case(&raw)] {
                prop_assert!(
                    produced.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
                    "{produced:?}"
                );
                prop_assert!(!produced.is_empty());
            }
        }
    }
}
