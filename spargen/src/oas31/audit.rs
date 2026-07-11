use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer};

use super::{Document, MediaTypeObject, RefOr, Schema, SchemaOr, ValidationKeywords};

/// The per-keyword W-class audit: walks every reachable schema and emits the once-per-site
/// warnings (validation-only keywords). R-class rejections fire during parsing and lowering.
pub fn audit(document: &Document, diags: &mut Diagnostics) {
    for (name, schema) in &document.components.schemas {
        if let RefOr::Item(schema) = schema {
            audit_schema(
                schema,
                JsonPointer::root()
                    .push("components")
                    .push("schemas")
                    .push(name),
                diags,
            );
        }
    }

    for (path, item) in &document.paths.items {
        for (method, operation) in &item.operations {
            let op_pointer = JsonPointer::root()
                .push("paths")
                .push(path)
                .push(method.as_str());
            for (index, parameter) in item
                .parameters
                .iter()
                .chain(operation.parameters.iter())
                .enumerate()
            {
                if let RefOr::Item(parameter) = parameter {
                    if let Some(RefOr::Item(schema)) = &parameter.schema {
                        audit_schema(
                            schema,
                            op_pointer.push("parameters").index(index).push("schema"),
                            diags,
                        );
                    }
                    for (media, object) in &parameter.content {
                        audit_media(
                            object,
                            op_pointer
                                .push("parameters")
                                .index(index)
                                .push("content")
                                .push(media),
                            diags,
                        );
                    }
                }
            }
            if let Some(RefOr::Item(body)) = &operation.request_body {
                for (media, object) in &body.content {
                    audit_media(
                        object,
                        op_pointer.push("requestBody").push("content").push(media),
                        diags,
                    );
                }
            }
            for (status, response) in &operation.responses.by_status {
                if let RefOr::Item(response) = response {
                    for (media, object) in &response.content {
                        audit_media(
                            object,
                            op_pointer
                                .push("responses")
                                .push(status)
                                .push("content")
                                .push(media),
                            diags,
                        );
                    }
                }
            }
        }
    }
}

fn audit_media(media: &MediaTypeObject, pointer: JsonPointer, diags: &mut Diagnostics) {
    if let Some(RefOr::Item(schema)) = &media.schema {
        audit_schema(schema, pointer.push("schema"), diags);
    }
}

fn audit_schema(schema: &Schema, pointer: JsonPointer, diags: &mut Diagnostics) {
    if has_validation_keywords(&schema.validation) {
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message("validation-only schema keywords are not enforced at runtime")
            .remedy("keep producer-side validation for these constraints")
            .emit(diags);
    }

    for (name, child) in &schema.properties {
        audit_schema_or(child, pointer.push("properties").push(name), diags);
    }
    if let Some(child) = &schema.additional_properties {
        audit_schema_or(child, pointer.push("additionalProperties"), diags);
    }
    if let Some(child) = &schema.items {
        audit_schema_or(child, pointer.push("items"), diags);
    }
    for (index, child) in schema.prefix_items.iter().enumerate() {
        audit_schema_or(child, pointer.push("prefixItems").index(index), diags);
    }
    for (index, child) in schema.all_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("allOf").index(index), diags);
    }
    for (index, child) in schema.one_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("oneOf").index(index), diags);
    }
    for (index, child) in schema.any_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("anyOf").index(index), diags);
    }
    for (name, child) in &schema.defs {
        audit_schema_or(child, pointer.push("$defs").push(name), diags);
    }
}

fn audit_schema_or(schema: &SchemaOr, pointer: JsonPointer, diags: &mut Diagnostics) {
    if let SchemaOr::Schema(schema) = schema {
        audit_schema(schema, pointer, diags);
    }
}

fn has_validation_keywords(validation: &ValidationKeywords) -> bool {
    validation.pattern.is_some()
        || validation.minimum.is_some()
        || validation.maximum.is_some()
        || validation.exclusive_minimum.is_some()
        || validation.exclusive_maximum.is_some()
        || validation.multiple_of.is_some()
        || validation.min_length.is_some()
        || validation.max_length.is_some()
        || validation.min_items.is_some()
        || validation.max_items.is_some()
        || validation.unique_items
        || validation.min_properties.is_some()
        || validation.max_properties.is_some()
}
