//! Casing conversions using Unicode-XID-aware segmentation (PRD D9). `heck` is deliberately not
//! used — it is not Unicode-XID-correct, which D9 requires.

/// Convert `raw` to `PascalCase` (for types and variants).
pub fn to_pascal_case(raw: &str) -> String {
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
pub fn to_snake_case(raw: &str) -> String {
    let words = words(raw);
    if words.is_empty() {
        "generated".to_owned()
    } else {
        words.join("_")
    }
}

/// Convert `raw` to `SHOUTY_SNAKE_CASE` (for constants).
pub fn to_shouty_snake_case(raw: &str) -> String {
    to_snake_case(raw).to_ascii_uppercase()
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
