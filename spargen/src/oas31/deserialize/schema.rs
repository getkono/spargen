//! The Schema Object parser: a schema position that may be a `$ref`, the Schema Object with every
//! subschema it holds, and the Discriminator and XML Objects it carries. The keywords whose value
//! is held to a JSON shape are read by the sibling `keywords` module.

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::{SpannedMap, SpannedValue};

use super::super::schema::DiscriminatorTarget;
use super::super::{
    is_xml_name, Discriminator, RefOr, Schema, SchemaOr, TypeSet, ValidationKeywords, XmlHints,
};
use super::keywords::{parse_required, parse_type_set, parse_validation, SUBSCHEMA_KEYWORDS};
use super::{
    expect_array, expect_bool, expect_string, is_oas_dialect, object, provenance, reference_object,
    required,
};

/// Parse a schema position that may be a `$ref` or an inline [`Schema`]. Unlike the generic
/// [`parse_ref_or`](super::parse_ref_or), a `default` declared *alongside* a `$ref` here is a schema `default` that the
/// reference-resolution drops on the floor; acknowledge it as `W005` so it is never silently lost.
/// Property-position `$ref`+`default` is handled elsewhere (via `SchemaOr`) and does not reach here.
///
/// The sibling rule: a Schema Object's `$ref` is a JSON Schema 2020-12 keyword that applies beside
/// its siblings, not a Reference Object that replaces them. So a `$ref` beside any keyword but
/// `default` is parsed as an inline [`Schema`] carrying the `$ref` (`summary` and `description`
/// count as such keywords), and only a `$ref` alone or beside `default` becomes a
/// [`RefOr::Ref`], whose `summary` and `description` are therefore always `None`.
pub(super) fn parse_schema_ref_or(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<RefOr<Schema>> {
    let Some(reference) = reference_object(value, pointer) else {
        return parse_schema(value, pointer, diags).map(RefOr::Item);
    };
    let has_shape_sibling = value.as_object().is_some_and(|map| {
        map.iter()
            .any(|(key, _)| key.name != "$ref" && key.name != "default")
    });
    if has_shape_sibling {
        return parse_schema(value, pointer, diags).map(RefOr::Item);
    }
    if let Some(default) = value.get("default") {
        Diagnostic::warning(
            Code::SchemaDefaultNotApplied,
            provenance(&pointer.push("default"), default),
        )
        .message(
            "a schema `default` declared alongside `$ref` is dropped when the reference \
             resolves and is not applied",
        )
        .remedy("move the default onto the referenced schema, or set the value explicitly")
        .emit(diags);
    }
    Some(RefOr::Ref(reference))
}

pub(in crate::oas31) fn parse_schema(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Schema> {
    match parse_schema_or(value, pointer, diags)? {
        SchemaOr::Schema(schema) => Some(*schema),
        SchemaOr::Bool(boolean) => Some(boolean_schema(boolean, provenance(pointer, value))),
    }
}

fn boolean_schema(value: bool, provenance: Provenance) -> Schema {
    Schema {
        boolean: Some(value),
        types: TypeSet::default(),
        reference: None,
        properties: IndexMap::new(),
        required: Vec::new(),
        additional_properties: None,
        pattern_properties: IndexMap::new(),
        items: None,
        prefix_items: Vec::new(),
        all_of: Vec::new(),
        one_of: Vec::new(),
        any_of: Vec::new(),
        discriminator: None,
        defs: IndexMap::new(),
        validation_children: Vec::new(),
        enum_values: None,
        const_value: None,
        default: None,
        format: None,
        content_encoding: None,
        content_media_type: None,
        content_schema: None,
        xml: None,
        validation: ValidationKeywords::default(),
        deprecated: false,
        read_only: false,
        write_only: false,
        title: None,
        description: None,
        provenance,
    }
}

fn parse_schema_or(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<SchemaOr> {
    if let Some(value) = value.as_bool() {
        return Some(SchemaOr::Bool(value));
    }
    let map = object(value, pointer, diags)?;

    if map.get("$dynamicRef").is_some() || map.get("$dynamicAnchor").is_some() {
        Diagnostic::error(Code::DynamicRefRejected, provenance(pointer, value))
            .message("$dynamicRef and $dynamicAnchor require dynamic schema scope evaluation")
            .emit(diags);
    }
    if map.get("$id").is_some() || map.get("$anchor").is_some() {
        // E004 case: resource-scope
        Diagnostic::error(Code::UnresolvedRef, provenance(pointer, value))
            .message("static `$id`/`$anchor` schema resource scopes are not yet supported")
            // The code reads "unresolved $ref" because scope-aware resolution is what is missing;
            // say so, since nothing here is literally an unresolved reference.
            .remedy(
                "spargen resolves references against the document and its files, not against \
                 `$id`/`$anchor` schema resource scopes; inline the subschema or reference it by \
                 JSON Pointer",
            )
            .emit(diags);
    }
    let dialect_pointer = pointer.push("$schema");
    if let Some((value, dialect)) = map.get("$schema").and_then(|value| {
        expect_string(value, &dialect_pointer, diags).map(|dialect| (value, dialect))
    }) {
        if !is_oas_dialect(dialect) {
            Diagnostic::error(
                Code::UnsupportedDialect,
                provenance(&dialect_pointer, value),
            )
            .message(format!(
                "schema resource uses unsupported dialect `{dialect}`"
            ))
            .emit(diags);
        }
    }

    let schema = Schema {
        boolean: None,
        types: parse_type_set(map.get("type"), &pointer.push("type"), diags),
        reference: string_keyword(map, "$ref", pointer, diags),
        properties: parse_schema_map(map, "properties", pointer, diags),
        required: parse_required(map.get("required"), &pointer.push("required"), diags),
        additional_properties: map.get("additionalProperties").and_then(|value| {
            parse_schema_or(value, &pointer.push("additionalProperties"), diags).map(Box::new)
        }),
        pattern_properties: parse_schema_map(map, "patternProperties", pointer, diags),
        items: map
            .get("items")
            .and_then(|value| parse_schema_or(value, &pointer.push("items"), diags).map(Box::new)),
        prefix_items: parse_schema_array(
            map.get("prefixItems"),
            &pointer.push("prefixItems"),
            diags,
        ),
        all_of: parse_schema_array(map.get("allOf"), &pointer.push("allOf"), diags),
        one_of: parse_schema_array(map.get("oneOf"), &pointer.push("oneOf"), diags),
        any_of: parse_schema_array(map.get("anyOf"), &pointer.push("anyOf"), diags),
        discriminator: map
            .get("discriminator")
            .and_then(|value| parse_discriminator(value, &pointer.push("discriminator"), diags)),
        defs: parse_schema_map(map, "$defs", pointer, diags),
        validation_children: parse_validation_children(map, pointer, diags),
        enum_values: map
            .get("enum")
            .and_then(|value| expect_array(value, &pointer.push("enum"), diags))
            .map(<[SpannedValue]>::to_vec),
        const_value: map.get("const").cloned(),
        default: map.get("default").cloned(),
        format: string_keyword(map, "format", pointer, diags),
        content_encoding: string_keyword(map, "contentEncoding", pointer, diags),
        content_media_type: string_keyword(map, "contentMediaType", pointer, diags),
        content_schema: map.get("contentSchema").and_then(|value| {
            parse_schema_or(value, &pointer.push("contentSchema"), diags).map(Box::new)
        }),
        xml: map
            .get("xml")
            .and_then(|value| parse_xml(value, &pointer.push("xml"), diags)),
        validation: parse_validation(map, pointer, diags),
        deprecated: bool_keyword(map, "deprecated", pointer, diags),
        read_only: bool_keyword(map, "readOnly", pointer, diags),
        write_only: bool_keyword(map, "writeOnly", pointer, diags),
        title: string_keyword(map, "title", pointer, diags),
        description: string_keyword(map, "description", pointer, diags),
        provenance: provenance(pointer, value),
    };
    Some(SchemaOr::Schema(Box::new(schema)))
}

/// A composition or tuple keyword (`allOf`, `oneOf`, `anyOf`, `prefixItems`): an array of
/// schemas, or `E011` at `pointer` when it is not an array.
fn parse_schema_array(
    value: Option<&SpannedValue>,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<SchemaOr> {
    value
        .and_then(|value| expect_array(value, pointer, diags))
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, value)| parse_schema_or(value, &pointer.index(index), diags))
        .collect()
}

/// Parse a Discriminator Object. Every field it carries is load-bearing — the tag field, and the
/// schemas the tags name — so a field of the wrong shape is a malformed document (`E011`) at that
/// field rather than something to read leniently.
fn parse_discriminator(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Discriminator> {
    let _ = object(value, pointer, diags)?;
    let mut malformed = false;
    let property_name = required(value, "propertyName", pointer, diags).and_then(|name| {
        let name = expect_string(name, &pointer.push("propertyName"), diags);
        malformed |= name.is_none();
        name
    });
    let default_mapping = value.get("defaultMapping").and_then(|target| {
        let pointer = pointer.push("defaultMapping");
        let parsed = discriminator_target(target, &pointer, diags);
        malformed |= parsed.is_none();
        parsed
    });
    let mut mapping = IndexMap::new();
    if let Some(map) = value.get("mapping") {
        let map_pointer = pointer.push("mapping");
        match object(map, &map_pointer, diags) {
            Some(map) => {
                for (key, target) in map.iter() {
                    match discriminator_target(target, &map_pointer.push(&key.name), diags) {
                        Some(target) => {
                            mapping.insert(key.name.clone(), target);
                        }
                        None => malformed = true,
                    }
                }
            }
            None => malformed = true,
        }
    }
    let property_name = property_name?;
    if malformed {
        return None;
    }
    Some(Discriminator {
        provenance: provenance(pointer, value),
        property_name: property_name.to_owned(),
        default_mapping,
        mapping,
    })
}

/// A `mapping` value or `defaultMapping`: a string naming a schema, kept with its own provenance.
fn discriminator_target(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<DiscriminatorTarget> {
    let target = expect_string(value, pointer, diags)?;
    Some(DiscriminatorTarget {
        value: target.to_owned(),
        provenance: provenance(pointer, value),
    })
}

/// Parse the OpenAPI `xml` object. Malformed shapes (a non-object `xml`) yield `None`; individual
/// missing keys default. Lowering later reports the unsupported namespace/prefix/wrapped hints —
/// `E009` on a type serialized as XML, `W006` elsewhere — so they are captured here rather than
/// dropped at parse time.
///
/// OpenAPI 3.2 deprecates `attribute`/`wrapped` in favour of `nodeType` and states that if
/// `nodeType` is present, neither field MUST be present. The two spellings can disagree
/// (`nodeType: element` beside `attribute: true`), and the specification defines no winner, so a
/// document that carries both is rejected rather than resolved by a guess.
fn parse_xml(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<XmlHints> {
    let _ = value.as_object()?;
    if value.get("nodeType").is_some() {
        for deprecated in ["attribute", "wrapped"] {
            if let Some(sibling) = value.get(deprecated) {
                Diagnostic::error(
                    Code::InvalidInput,
                    provenance(&pointer.push(deprecated), sibling),
                )
                .message(format!(
                    "xml.{deprecated} must not be present beside xml.nodeType"
                ))
                .remedy(format!(
                    "drop `{deprecated}` and express it through `nodeType` alone"
                ))
                .emit(diags);
            }
        }
    }
    // An `xml.name` becomes an element or attribute name as written, and quick-xml refuses to
    // write one that is not an XML name, so a body carrying it would fail at run time. The name is
    // checked here, where every `xml` object is read, so the rule does not depend on which
    // position lowering reads it in or whether an XML body reaches its type.
    let name = value.get("name").and_then(SpannedValue::as_str);
    if let Some(name) = name.filter(|name| !is_xml_name(name)) {
        let at = pointer.push("name");
        let sibling = value.get("name").unwrap_or(value);
        Diagnostic::error(Code::InvalidInput, provenance(&at, sibling))
            .message(if name.is_empty() {
                "xml.name is empty, which is not an XML name".to_owned()
            } else {
                format!("xml.name `{name}` is not an XML name")
            })
            .remedy(
                "use an XML 1.1 name: a letter, `_` or `:` first, then letters, digits, `-`, `.`, \
                 `_` or `:`",
            )
            .emit(diags);
    }
    Some(XmlHints {
        name: name.map(str::to_owned),
        attribute: value.get("nodeType").and_then(SpannedValue::as_str) == Some("attribute")
            || value
                .get("attribute")
                .and_then(SpannedValue::as_bool)
                .unwrap_or(false),
        node_type: value
            .get("nodeType")
            .and_then(SpannedValue::as_str)
            .map(str::to_owned),
        namespace: value
            .get("namespace")
            .and_then(SpannedValue::as_str)
            .map(str::to_owned),
        prefix: value
            .get("prefix")
            .and_then(SpannedValue::as_str)
            .map(str::to_owned),
        wrapped: value
            .get("wrapped")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
    })
}

fn parse_validation_children(
    map: &SpannedMap,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<(String, SchemaOr)> {
    let mut children = Vec::new();
    for keyword in SUBSCHEMA_KEYWORDS {
        if let Some(value) = map.get(keyword) {
            if let Some(schema) = parse_schema_or(value, &pointer.push(keyword), diags) {
                children.push((keyword.to_owned(), schema));
            }
        }
    }
    for (name, schema) in parse_schema_map(map, "dependentSchemas", pointer, diags) {
        children.push((format!("dependentSchemas/{name}"), schema));
    }
    children
}

/// A keyword whose value maps names to schemas (`properties`, `patternProperties`, `$defs`,
/// `dependentSchemas`): each entry parsed at its own pointer, or `E011` at the keyword when the
/// value is not an object, which used to read as no entries at all.
fn parse_schema_map(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> IndexMap<String, SchemaOr> {
    let Some(value) = map.get(keyword) else {
        return IndexMap::new();
    };
    let pointer = pointer.push(keyword);
    let Some(entries) = object(value, &pointer, diags) else {
        return IndexMap::new();
    };
    entries
        .iter()
        .filter_map(|(key, value)| {
            parse_schema_or(value, &pointer.push(&key.name), diags)
                .map(|schema| (key.name.clone(), schema))
        })
        .collect()
}

/// A string-valued keyword (`$ref`, `format`, `title`, ...): absent, or the string, or `E011` at
/// the keyword, which used to read as absent.
fn string_keyword(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<String> {
    map.get(keyword)
        .and_then(|value| expect_string(value, &pointer.push(keyword), diags))
        .map(str::to_owned)
}

/// A boolean keyword (`deprecated`, `readOnly`, `writeOnly`): `false` when absent, or `E011` at
/// the keyword when it is not a boolean, which used to read as `false`.
fn bool_keyword(
    map: &SpannedMap,
    keyword: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> bool {
    map.get(keyword)
        .and_then(|value| expect_bool(value, &pointer.push(keyword), diags))
        .unwrap_or(false)
}
