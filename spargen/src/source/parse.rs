use crate::diag::{
    Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Loc, Provenance, Span,
};

use super::{Node, Number, SpannedKey, SpannedMap, SpannedValue};

/// Parse a JSON document into a span-preserving [`SpannedValue`] tree (PRD FR1, D4).
///
/// Malformed input is reported through `diags` (with spans) rather than by panic; a fatal parse
/// error returns [`Aborted`]. The parser is event-level so it can attach a span to every node.
pub fn parse_json(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => Ok(from_json_value(root_span(file, text), value)),
        Err(error) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(
                    JsonPointer::root(),
                    Some(Span::point(
                        file,
                        loc_for_line_col(text, error.line() as u32, error.column() as u32),
                    )),
                ),
            )
            .message(format!("malformed JSON: {error}"))
            .remedy("fix the JSON syntax before running spargen")
            .emit(diags);
            Err(Aborted)
        }
    }
}

/// Parse a YAML 1.2 document into a span-preserving [`SpannedValue`] tree.
///
/// YAML is restricted to the JSON-compatible subset OAS 3.1 prescribes (PRD §3.3 prec 5);
/// constructs outside that subset are diagnosed. Errors are reported through `diags`.
pub fn parse_yaml(
    file: FileId,
    text: &str,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    match yaml_rust2::YamlLoader::load_from_str(text) {
        Ok(mut docs) => {
            if docs.len() != 1 {
                Diagnostic::error(
                    Code::InvalidInput,
                    Provenance::new(JsonPointer::root(), Some(root_span(file, text))),
                )
                .message("YAML input must contain exactly one document")
                .emit(diags);
                return Err(Aborted);
            }
            yaml_to_value(root_span(file, text), docs.remove(0), diags)
        }
        Err(error) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), Some(root_span(file, text))),
            )
            .message(format!("malformed YAML: {error}"))
            .remedy("fix the YAML syntax before running spargen")
            .emit(diags);
            Err(Aborted)
        }
    }
}

fn from_json_value(span: Span, value: serde_json::Value) -> SpannedValue {
    let node = match value {
        serde_json::Value::Null => Node::Null,
        serde_json::Value::Bool(value) => Node::Bool(value),
        serde_json::Value::Number(value) => {
            let number = if let Some(value) = value.as_i64() {
                Number::Int(value)
            } else if let Some(value) = value.as_u64() {
                Number::UInt(value)
            } else {
                Number::Float(value.as_f64().unwrap_or_default())
            };
            Node::Number(number)
        }
        serde_json::Value::String(value) => Node::String(value),
        serde_json::Value::Array(values) => Node::Array(
            values
                .into_iter()
                .map(|value| from_json_value(span, value))
                .collect(),
        ),
        serde_json::Value::Object(values) => {
            let mut map = SpannedMap::default();
            for (name, value) in values {
                map.push(SpannedKey { name, span }, from_json_value(span, value));
            }
            Node::Object(map)
        }
    };
    SpannedValue::new(node, span)
}

fn yaml_to_value(
    span: Span,
    value: yaml_rust2::Yaml,
    diags: &mut Diagnostics,
) -> Result<SpannedValue, Aborted> {
    let node = match value {
        yaml_rust2::Yaml::Null | yaml_rust2::Yaml::BadValue => Node::Null,
        yaml_rust2::Yaml::Boolean(value) => Node::Bool(value),
        yaml_rust2::Yaml::Integer(value) => Node::Number(Number::Int(value)),
        yaml_rust2::Yaml::Real(value) => Node::Number(Number::Float(value.parse().unwrap_or(0.0))),
        yaml_rust2::Yaml::String(value) => Node::String(value),
        yaml_rust2::Yaml::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(yaml_to_value(span, value, diags)?);
            }
            Node::Array(out)
        }
        yaml_rust2::Yaml::Hash(values) => {
            let mut map = SpannedMap::default();
            for (key, value) in values {
                let yaml_rust2::Yaml::String(name) = key else {
                    Diagnostic::error(
                        Code::InvalidInput,
                        Provenance::new(JsonPointer::root(), Some(span)),
                    )
                    .message("YAML object keys must be strings")
                    .emit(diags);
                    return Err(Aborted);
                };
                map.push(
                    SpannedKey { name, span },
                    yaml_to_value(span, value, diags)?,
                );
            }
            Node::Object(map)
        }
        yaml_rust2::Yaml::Alias(_) => {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), Some(span)),
            )
            .message("YAML aliases are outside spargen's JSON-compatible YAML subset")
            .emit(diags);
            return Err(Aborted);
        }
    };
    Ok(SpannedValue::new(node, span))
}

fn root_span(file: FileId, text: &str) -> Span {
    Span {
        file,
        start: Loc {
            line: 1,
            col: 1,
            offset: 0,
        },
        end: loc_for_offset(text, text.len()),
    }
}

fn loc_for_line_col(text: &str, line: u32, col: u32) -> Loc {
    let mut current_line = 1;
    let mut current_col = 1;
    for (offset, ch) in text.char_indices() {
        if current_line == line && current_col == col {
            return Loc { line, col, offset };
        }
        if ch == '\n' {
            current_line += 1;
            current_col = 1;
        } else {
            current_col += 1;
        }
    }
    Loc {
        line,
        col,
        offset: text.len(),
    }
}

fn loc_for_offset(text: &str, target: usize) -> Loc {
    let mut line = 1;
    let mut col = 1;
    let mut offset = 0;
    for (current, ch) in text.char_indices() {
        if current >= target {
            break;
        }
        offset = current + ch.len_utf8();
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    Loc { line, col, offset }
}
