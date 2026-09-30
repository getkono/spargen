//! Structural validation of every file in the input bundle against the vendored OpenAPI document
//! schemas.
//!
//! The root document is validated as a whole OpenAPI document. A file reached by `$ref` usually is
//! not one — a Path Item file has no `openapi`, `info` or `paths` — so each `$ref` target is
//! validated against the schema location the *reference's position* implies instead, and that
//! location is read out of the vendored schema itself rather than out of a table here: the schema
//! is walked alongside the instance, and wherever it admits a Reference Object in place of some
//! object (its `*-or-reference` definitions) or a Path Item's own `$ref`, the target is queued for
//! validation against that same location. Validating against the `*-or-reference` location rather
//! than the object it wraps is what admits a target that is itself a Reference: the chain is
//! followed hop by hop, each hop validated where it lands.
//!
//! The result is that a construct the schema closes is rejected the same way whether it is written
//! inline or moved into another file, under the same code (`E011`).
//!
//! Schema Objects are the one kind of target this does not walk into: the vendored schema checks a
//! Schema Object only as `object | boolean` (its `schema` definition is a `$dynamicAnchor` over the
//! dialect), and a `$ref` inside one is JSON Schema's own reference, resolved and checked by
//! lowering.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::diag::{Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};
use crate::source::{canonical_pointer, InputBundle, Node, Number, SpannedValue};

const OAS31_SCHEMA: &str = include_str!("spec/oas-3.1-2025-09-15.json");
const OAS32_SCHEMA: &str = include_str!("spec/oas-3.2-2025-09-17.json");

/// How both vendored schemas spell a Reference Object in a `*-or-reference` definition's `then`.
const REFERENCE_OBJECT: &str = "#/$defs/reference";

/// Structural validator against the vendored official OAS 3.1 and 3.2 document schemas.
/// Targets fixed, checksummed in-repo artifacts under `spec/`, never a live URL.
pub(crate) struct MetaSchemaValidator {
    oas31: VersionSchema,
    oas32: VersionSchema,
}

/// One vendored document schema: as data, which the walk reads, and compiled.
struct VersionSchema {
    schema: Value,
    document: jsonschema::Validator,
}

impl MetaSchemaValidator {
    /// Load and parse the vendored meta-schemas from `spec/`.
    pub(crate) fn load_vendored() -> Self {
        Self {
            oas31: VersionSchema::load(OAS31_SCHEMA, "OpenAPI 3.1"),
            oas32: VersionSchema::load(OAS32_SCHEMA, "OpenAPI 3.2"),
        }
    }

    /// Validate every file of `bundle` the root reaches through a `$ref` in an OpenAPI position,
    /// reporting each violation as `E011` with the pointer and span of the offending node in the
    /// file it is written in.
    ///
    /// The root document selects the schema version, and every referenced file is held to it: a
    /// referenced file carries no `openapi` field of its own to say otherwise.
    pub(crate) fn validate(&self, bundle: &InputBundle, diags: &mut Diagnostics) {
        let root = bundle.root();
        let version = match root.get("openapi").and_then(SpannedValue::as_str) {
            Some(version) if version.starts_with("3.1.") => &self.oas31,
            Some(version) if version.starts_with("3.2.") => &self.oas32,
            // `parse_document` owns E001 and its forward-compatible version explanation. Avoid
            // obscuring it with a second structure error from an arbitrarily selected schema.
            Some(_) => return,
            // Select 3.1 only to report the shared required `openapi` field as E011.
            None => &self.oas31,
        };

        let root_id = bundle.root_id();
        let mut instances: HashMap<FileId, Value> = HashMap::new();
        let mut reporter = Reporter::default();
        let mut walk = Walk::new(&version.schema);
        let mut covered: HashSet<(FileId, JsonPointer, JsonPointer)> = HashSet::new();
        let mut queue: VecDeque<(FileId, Pending)> = VecDeque::new();

        let root_instance = instances.entry(root_id).or_insert_with(|| to_json(root));
        for error in version.document.iter_errors(root_instance) {
            let at = JsonPointer::from(error.instance_path().as_str().to_owned());
            reporter.report(
                bundle,
                root_id,
                &JsonPointer::root(),
                &at,
                error.to_string(),
                None,
                diags,
            );
        }
        let mut found = Found::default();
        walk.navigate(
            &JsonPointer::root(),
            root_instance,
            &JsonPointer::root(),
            &mut found,
        );
        found.absorb(root_id, &mut covered, &mut queue);

        while let Some((from, pending)) = queue.pop_front() {
            // An unresolvable reference is lowering's to report, in its own words (`E004`).
            let Some((file, fragment)) = bundle.reference_target(&pending.reference, from) else {
                continue;
            };
            // The fragment is percent-encoded where lowering's lookup decodes it, so it is read
            // here through that same decoding: a target lowering reaches is a target validated,
            // and the pointer compares equal to the ones the walk builds (`covered`) and reports.
            let Some(pointer) = canonical_pointer(&fragment) else {
                continue;
            };
            // Already validated at this location: either queued before, or enclosed by a node an
            // earlier validation covered — the root document's own components, most often.
            if covered.contains(&(file, pointer.clone(), pending.location.clone())) {
                continue;
            }
            let file_instance = instances
                .entry(file)
                .or_insert_with(|| to_json(bundle.value_at(file)));
            let Some(instance) = file_instance.pointer(pointer.as_str()) else {
                continue;
            };
            let errors: Vec<(JsonPointer, String)> = walk
                .validator(&pending.location)
                .iter_errors(instance)
                .map(|error| {
                    (
                        JsonPointer::from(error.instance_path().as_str().to_owned()),
                        error.to_string(),
                    )
                })
                .collect();
            let context = format!(
                "reached through `$ref: {}` and validated as `{}`",
                pending.reference,
                walk.kind(&pending.location)
            );
            for (at, error) in errors {
                reporter.report(bundle, file, &pointer, &at, error, Some(&context), diags);
            }
            let mut found = Found::default();
            walk.navigate(&pending.location, instance, &pointer, &mut found);
            found.absorb(file, &mut covered, &mut queue);
        }
    }
}

impl VersionSchema {
    fn load(source: &str, label: &str) -> Self {
        let schema: Value = serde_json::from_str(source)
            .unwrap_or_else(|error| panic!("vendored {label} schema is invalid JSON: {error}"));
        let document = jsonschema::validator_for(&schema)
            .unwrap_or_else(|error| panic!("vendored {label} schema does not compile: {error}"));
        Self { schema, document }
    }
}

/// A `$ref` the walk found in an OpenAPI position, awaiting validation of its target.
struct Pending {
    reference: String,
    /// The schema location the target is validated against.
    location: JsonPointer,
}

/// What one walk over an instance found.
#[derive(Default)]
struct Found {
    /// `(instance pointer, schema location)` pairs at which a `$ref` target could be validated,
    /// visited under a validation that already covered them.
    covered: Vec<(JsonPointer, JsonPointer)>,
    references: Vec<Pending>,
}

impl Found {
    fn absorb(
        self,
        file: FileId,
        covered: &mut HashSet<(FileId, JsonPointer, JsonPointer)>,
        queue: &mut VecDeque<(FileId, Pending)>,
    ) {
        covered.extend(
            self.covered
                .into_iter()
                .map(|(pointer, location)| (file, pointer, location)),
        );
        queue.extend(self.references.into_iter().map(|pending| (file, pending)));
    }
}

/// Emits each violation once. A node can be validated twice at one location when two references
/// reach overlapping targets in one file; the reader is told once.
#[derive(Default)]
struct Reporter {
    emitted: HashSet<(FileId, JsonPointer, String)>,
}

impl Reporter {
    #[allow(clippy::too_many_arguments)]
    fn report(
        &mut self,
        bundle: &InputBundle,
        file: FileId,
        base: &JsonPointer,
        at: &JsonPointer,
        error: String,
        context: Option<&str>,
        diags: &mut Diagnostics,
    ) {
        let pointer = JsonPointer::from(format!("{}{}", base.as_str(), at.as_str()));
        if !self.emitted.insert((file, pointer.clone(), error.clone())) {
            return;
        }
        let document = bundle.value_at(file);
        let span = document
            .pointer(&pointer)
            .or_else(|| document.pointer(base))
            .map(SpannedValue::span)
            .or(Some(document.span()));
        let message = match context {
            None => format!("OpenAPI structure violation: {error}"),
            Some(context) => format!("OpenAPI structure violation: {error} ({context})"),
        };
        Diagnostic::error(Code::InvalidInput, Provenance::new(pointer, span))
            .message(message)
            .emit(diags);
    }
}

/// A walk of one vendored schema alongside an instance, compiling the schema locations it needs to
/// evaluate on first use.
struct Walk<'schema> {
    schema: &'schema Value,
    compiled: HashMap<String, jsonschema::Validator>,
}

impl<'schema> Walk<'schema> {
    fn new(schema: &'schema Value) -> Self {
        Self {
            schema,
            compiled: HashMap::new(),
        }
    }

    /// A validator for the subschema at `location` of the vendored schema.
    ///
    /// The subschema is compiled as a document of its own that carries the vendored `$defs`, so its
    /// `#/$defs/...` references — the only kind either vendored schema uses — resolve exactly as
    /// they do in place, `$dynamicRef: "#meta"` included.
    fn validator(&mut self, location: &JsonPointer) -> &jsonschema::Validator {
        let schema = self.schema;
        self.compiled
            .entry(location.as_str().to_owned())
            .or_insert_with(|| {
                let node = schema
                    .pointer(location.as_str())
                    .unwrap_or_else(|| panic!("vendored schema has no subschema at `{location}`"));
                let document = match node {
                    Value::Object(node) => {
                        let mut document = node.clone();
                        for key in ["$schema", "$id", "$defs"] {
                            if let Some(value) = schema.get(key) {
                                document.insert(key.to_owned(), value.clone());
                            }
                        }
                        Value::Object(document)
                    }
                    other => other.clone(),
                };
                jsonschema::validator_for(&document).unwrap_or_else(|error| {
                    panic!("vendored subschema at `{location}` does not compile: {error}")
                })
            })
    }

    fn is_valid(&mut self, location: &JsonPointer, instance: &Value) -> bool {
        self.validator(location).is_valid(instance)
    }

    /// Whether `key` matches a `patternProperties` pattern, by the same engine the validators use.
    fn matches(&mut self, pattern: &str, key: &str) -> bool {
        let entry = format!("pattern:{pattern}");
        self.compiled
            .entry(entry)
            .or_insert_with(|| {
                jsonschema::validator_for(&serde_json::json!({ "pattern": pattern }))
                    .unwrap_or_else(|error| {
                        panic!("vendored pattern {pattern:?} does not compile: {error}")
                    })
            })
            .is_valid(&Value::String(key.to_owned()))
    }

    /// The definition a target validated at `location` is read against, for diagnostics.
    fn kind(&self, location: &JsonPointer) -> String {
        let node = self.schema.pointer(location.as_str());
        node.and_then(|node| node.get("else"))
            .and_then(|branch| branch.get("$ref"))
            .and_then(Value::as_str)
            .and_then(|reference| reference.rsplit('/').next())
            .or_else(|| {
                location
                    .as_str()
                    .strip_prefix("/$defs/")
                    .and_then(|rest| rest.split('/').next())
            })
            .unwrap_or("document")
            .to_owned()
    }

    /// Walk the subschema at `location` over `instance` (found at `at`), recording where a `$ref`
    /// written in the instance takes the place of an object the schema describes.
    ///
    /// Only the applicators that carry an instance to a subschema are followed; assertions are the
    /// validator's business. `if`, `oneOf` and `anyOf` are followed into the branches the instance
    /// satisfies, `not` and `contains` never (neither describes a member the instance has), and a
    /// `$dynamicRef` never — it names the Schema Object dialect, which this does not walk into.
    fn navigate(
        &mut self,
        location: &JsonPointer,
        instance: &Value,
        at: &JsonPointer,
        found: &mut Found,
    ) {
        let schema = self.schema;
        let Some(node) = schema.pointer(location.as_str()).and_then(Value::as_object) else {
            return;
        };

        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            if let Some(target) = reference.strip_prefix('#') {
                self.navigate(&JsonPointer::from(target), instance, at, found);
            }
        }

        // A `*-or-reference` definition: `if` the instance is an object with a `$ref`, `then` it is
        // a Reference Object, `else` the object it stands in for. A Reference Object is not walked
        // into; its target is validated here, against this same location.
        let reference_slot = node.contains_key("if")
            && node
                .get("then")
                .and_then(|then| then.get("$ref"))
                .and_then(Value::as_str)
                == Some(REFERENCE_OBJECT);
        if reference_slot {
            found.covered.push((at.clone(), location.clone()));
        }
        if node.contains_key("if") {
            let branch = if self.is_valid(&location.push("if"), instance) {
                "then"
            } else {
                "else"
            };
            if reference_slot && branch == "then" {
                if let Some(reference) = instance.get("$ref").and_then(Value::as_str) {
                    found.references.push(Pending {
                        reference: reference.to_owned(),
                        location: location.clone(),
                    });
                }
            } else if node.contains_key(branch) {
                self.navigate(&location.push(branch), instance, at, found);
            }
        }

        if let Some(all) = node.get("allOf").and_then(Value::as_array) {
            for index in 0..all.len() {
                self.navigate(&location.push("allOf").index(index), instance, at, found);
            }
        }
        for keyword in ["anyOf", "oneOf"] {
            if let Some(branches) = node.get(keyword).and_then(Value::as_array) {
                for index in 0..branches.len() {
                    let branch = location.push(keyword).index(index);
                    if self.is_valid(&branch, instance) {
                        self.navigate(&branch, instance, at, found);
                    }
                }
            }
        }

        if let Some(object) = instance.as_object() {
            if let Some(dependent) = node.get("dependentSchemas").and_then(Value::as_object) {
                for key in dependent.keys() {
                    if object.contains_key(key) {
                        let branch = location.push("dependentSchemas").push(key);
                        self.navigate(&branch, instance, at, found);
                    }
                }
            }
            let properties = node.get("properties").and_then(Value::as_object);
            let patterns = node.get("patternProperties").and_then(Value::as_object);
            for (key, value) in object {
                let member = at.push(key);
                let mut described = false;
                if properties.is_some_and(|properties| properties.contains_key(key)) {
                    described = true;
                    let branch = location.push("properties").push(key);
                    self.navigate(&branch, value, &member, found);
                }
                for pattern in patterns.into_iter().flat_map(|patterns| patterns.keys()) {
                    if self.matches(pattern, key) {
                        described = true;
                        let branch = location.push("patternProperties").push(pattern);
                        self.navigate(&branch, value, &member, found);
                    }
                }
                if !described && node.contains_key("additionalProperties") {
                    let branch = location.push("additionalProperties");
                    self.navigate(&branch, value, &member, found);
                }
            }
            // An object the schema itself gives a `$ref` member — the Path Item. Its target is
            // validated as the same object, so the definition is read in place.
            if !reference_slot
                && properties.is_some_and(|properties| properties.contains_key("$ref"))
            {
                found.covered.push((at.clone(), location.clone()));
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    found.references.push(Pending {
                        reference: reference.to_owned(),
                        location: location.clone(),
                    });
                }
            }
        }

        if let Some(items) = instance.as_array() {
            if node.contains_key("items") {
                let branch = location.push("items");
                for (index, item) in items.iter().enumerate() {
                    self.navigate(&branch, item, &at.index(index), found);
                }
            }
        }
    }
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
