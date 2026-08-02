use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::{Node, Number, SpannedValue};

const OAS31_SCHEMA: &str = include_str!("spec/oas-3.1-2025-09-15.json");
const OAS32_SCHEMA: &str = include_str!("spec/oas-3.2-2025-09-17.json");

/// Structural validator against the vendored official OAS 3.1 and 3.2 document schemas.
/// Targets fixed, checksummed in-repo artifacts under `spec/`, never a live URL.
pub struct MetaSchemaValidator {
    oas31: jsonschema::Validator,
    oas32: jsonschema::Validator,
}

impl MetaSchemaValidator {
    /// Load and parse the vendored meta-schemas from `spec/`.
    pub fn load_vendored() -> Self {
        Self {
            oas31: compile(OAS31_SCHEMA, "OpenAPI 3.1"),
            oas32: compile(OAS32_SCHEMA, "OpenAPI 3.2"),
        }
    }

    /// Validate a raw document tree against the meta-schemas, reporting violations through `diags`
    /// (with pointer + span).
    pub fn validate(&self, document: &SpannedValue, diags: &mut Diagnostics) {
        let version = document.get("openapi").and_then(SpannedValue::as_str);
        let validator = match version {
            Some(version) if version.starts_with("3.1.") => &self.oas31,
            Some(version) if version.starts_with("3.2.") => &self.oas32,
            // `parse_document` owns E001 and its forward-compatible version explanation. Avoid
            // obscuring it with a second structure error from an arbitrarily selected schema.
            Some(_) => return,
            // Select 3.1 only to report the shared required `openapi` field as E011.
            None => &self.oas31,
        };
        let instance = to_json(document);
        for error in validator.iter_errors(&instance) {
            let pointer = JsonPointer::from(error.instance_path().as_str().to_owned());
            let span = document
                .pointer(&pointer)
                .map(SpannedValue::span)
                .or(Some(document.span()));
            Diagnostic::error(Code::InvalidInput, Provenance::new(pointer, span))
                .message(format!("OpenAPI structure violation: {error}"))
                .emit(diags);
        }
    }
}

fn compile(source: &str, label: &str) -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(source)
        .unwrap_or_else(|error| panic!("vendored {label} schema is invalid JSON: {error}"));
    jsonschema::validator_for(&schema)
        .unwrap_or_else(|error| panic!("vendored {label} schema does not compile: {error}"))
}

fn to_json(value: &SpannedValue) -> serde_json::Value {
    match &value.node {
        Node::Null => serde_json::Value::Null,
        Node::Bool(value) => serde_json::Value::Bool(*value),
        Node::Number(Number::Int(value)) => (*value).into(),
        Node::Number(Number::UInt(value)) => (*value).into(),
        Node::Number(Number::Float(value)) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Node::String(value) => serde_json::Value::String(value.clone()),
        Node::Array(values) => serde_json::Value::Array(values.iter().map(to_json).collect()),
        Node::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.name.clone(), to_json(value)))
                .collect(),
        ),
    }
}
