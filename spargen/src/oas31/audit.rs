use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};

use super::{Document, MediaTypeObject, RefOr, Resolver, Schema, SchemaOr, ValidationKeywords};

type AnnotationKey = (Option<FileId>, JsonPointer);

/// The per-keyword W-class audit: walks every reachable schema and emits the once-per-site
/// warnings (validation-only keywords). R-class rejections fire during parsing and lowering.
pub fn audit(document: &Document, resolver: &Resolver<'_>, diags: &mut Diagnostics) {
    let consumed_content = consumed_sse_content(document, resolver, diags);
    for (name, schema) in &document.components.schemas {
        if let RefOr::Item(schema) = schema {
            audit_schema(
                schema,
                JsonPointer::root()
                    .push("components")
                    .push("schemas")
                    .push(name),
                &consumed_content,
                diags,
            );
        }
    }

    let components_pointer = JsonPointer::root().push("components");
    for (name, parameter) in &document.components.parameters {
        if let RefOr::Item(parameter) = parameter {
            audit_parameter(
                parameter,
                components_pointer.push("parameters").push(name),
                &consumed_content,
                diags,
            );
        }
    }
    for (name, body) in &document.components.request_bodies {
        if let RefOr::Item(body) = body {
            audit_content(
                &body.content,
                components_pointer
                    .push("requestBodies")
                    .push(name)
                    .push("content"),
                &consumed_content,
                diags,
            );
        }
    }
    for (name, response) in &document.components.responses {
        if let RefOr::Item(response) = response {
            audit_content(
                &response.content,
                components_pointer
                    .push("responses")
                    .push(name)
                    .push("content"),
                &consumed_content,
                diags,
            );
        }
    }
    for (name, media) in &document.components.media_types {
        audit_media(
            media,
            components_pointer.push("mediaTypes").push(name),
            &consumed_content,
            diags,
        );
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
                    audit_parameter(
                        parameter,
                        op_pointer.push("parameters").index(index),
                        &consumed_content,
                        diags,
                    );
                }
            }
            if let Some(RefOr::Item(body)) = &operation.request_body {
                audit_content(
                    &body.content,
                    op_pointer.push("requestBody").push("content"),
                    &consumed_content,
                    diags,
                );
            }
            for (status, response) in &operation.responses.by_status {
                if let RefOr::Item(response) = response {
                    audit_content(
                        &response.content,
                        op_pointer.push("responses").push(status).push("content"),
                        &consumed_content,
                        diags,
                    );
                }
            }
            if let Some(RefOr::Item(response)) = &operation.responses.default {
                audit_content(
                    &response.content,
                    op_pointer.push("responses").push("default").push("content"),
                    &consumed_content,
                    diags,
                );
            }
        }
    }
}

fn audit_parameter(
    parameter: &super::ParameterObject,
    pointer: JsonPointer,
    consumed_content: &HashSet<AnnotationKey>,
    diags: &mut Diagnostics,
) {
    if let Some(RefOr::Item(schema)) = &parameter.schema {
        audit_schema(schema, pointer.push("schema"), consumed_content, diags);
    }
    audit_content(
        &parameter.content,
        pointer.push("content"),
        consumed_content,
        diags,
    );
}

fn audit_content(
    content: &indexmap::IndexMap<String, MediaTypeObject>,
    pointer: JsonPointer,
    consumed_content: &HashSet<AnnotationKey>,
    diags: &mut Diagnostics,
) {
    for (media, object) in content {
        audit_media(object, pointer.push(media), consumed_content, diags);
    }
}

fn audit_media(
    media: &MediaTypeObject,
    pointer: JsonPointer,
    consumed_content: &HashSet<AnnotationKey>,
    diags: &mut Diagnostics,
) {
    if let Some(RefOr::Item(schema)) = &media.schema {
        audit_schema(schema, pointer.push("schema"), consumed_content, diags);
    }
    if let Some(RefOr::Item(schema)) = &media.item_schema {
        audit_schema(schema, pointer.push("itemSchema"), consumed_content, diags);
    }
}

fn audit_schema(
    schema: &Schema,
    pointer: JsonPointer,
    consumed_content: &HashSet<AnnotationKey>,
    diags: &mut Diagnostics,
) {
    if has_validation_keywords(&schema.validation) {
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message("validation-only schema keywords are not enforced at runtime")
            .remedy("keep producer-side validation for these constraints")
            .emit(diags);
    }

    if (schema.content_media_type.is_some() || schema.content_schema.is_some())
        && !consumed_content.contains(&annotation_key(&schema.provenance))
    {
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(
                "`contentMediaType`/`contentSchema` are decoded only on an OpenAPI 3.2 SSE \
                 envelope's string `data` property",
            )
            .remedy(
                "place the annotations on the `data` property of a text/event-stream itemSchema, \
                 or decode the string content in application code",
            )
            .emit(diags);
    }

    // A `patternProperties` key regex is a validation-only constraint: the generated typed overflow
    // map captures every non-declared property regardless of the pattern, so the key regex is not
    // enforced. Acknowledge it as `W001` (never silent) — the value schemas still lower.
    if !schema.pattern_properties.is_empty() {
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(
                "`patternProperties` key patterns are not enforced: the generated typed map \
                 captures all non-declared properties, not only pattern-matching keys",
            )
            .remedy("keep producer-side validation for the key pattern")
            .emit(diags);
    }

    for (name, child) in &schema.properties {
        audit_schema_or(
            child,
            pointer.push("properties").push(name),
            consumed_content,
            diags,
        );
    }
    if let Some(child) = &schema.additional_properties {
        audit_schema_or(
            child,
            pointer.push("additionalProperties"),
            consumed_content,
            diags,
        );
    }
    for (pattern, child) in &schema.pattern_properties {
        audit_schema_or(
            child,
            pointer.push("patternProperties").push(pattern),
            consumed_content,
            diags,
        );
    }
    if let Some(child) = &schema.items {
        audit_schema_or(child, pointer.push("items"), consumed_content, diags);
    }
    for (index, child) in schema.prefix_items.iter().enumerate() {
        audit_schema_or(
            child,
            pointer.push("prefixItems").index(index),
            consumed_content,
            diags,
        );
    }
    for (index, child) in schema.all_of.iter().enumerate() {
        audit_schema_or(
            child,
            pointer.push("allOf").index(index),
            consumed_content,
            diags,
        );
    }
    for (index, child) in schema.one_of.iter().enumerate() {
        audit_schema_or(
            child,
            pointer.push("oneOf").index(index),
            consumed_content,
            diags,
        );
    }
    for (index, child) in schema.any_of.iter().enumerate() {
        audit_schema_or(
            child,
            pointer.push("anyOf").index(index),
            consumed_content,
            diags,
        );
    }
    for (name, child) in &schema.defs {
        audit_schema_or(
            child,
            pointer.push("$defs").push(name),
            consumed_content,
            diags,
        );
    }
    for (keyword, child) in &schema.validation_children {
        audit_schema_or(child, pointer.push(keyword), consumed_content, diags);
    }
    if let Some(child) = schema.content_schema.as_deref() {
        audit_schema_or(
            child,
            pointer.push("contentSchema"),
            consumed_content,
            diags,
        );
    }
}

fn audit_schema_or(
    schema: &SchemaOr,
    pointer: JsonPointer,
    consumed_content: &HashSet<AnnotationKey>,
    diags: &mut Diagnostics,
) {
    if let SchemaOr::Schema(schema) = schema {
        audit_schema(schema, pointer, consumed_content, diags);
    }
}

fn consumed_sse_content(
    document: &Document,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
) -> HashSet<AnnotationKey> {
    let mut consumed = HashSet::new();
    let mut inspect = |content: &indexmap::IndexMap<String, MediaTypeObject>| {
        for (media_name, media) in content {
            if media_name
                .split(';')
                .next()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                let media = resolve_media(document, media);
                if let Some(item) = media.and_then(|media| media.item_schema.as_ref()) {
                    if let Some(json) = super::sse::json_data_schema(item, resolver, diags) {
                        consumed.insert(annotation_key(&json.annotation_site));
                    }
                }
            }
        }
    };
    for response in document.components.responses.values() {
        if let RefOr::Item(response) = response {
            inspect(&response.content);
        }
    }
    for item in document.paths.items.values() {
        for operation in item.operations.values() {
            for response in operation.responses.by_status.values() {
                if let Some(response) = resolve_response(document, response) {
                    inspect(&response.content);
                }
            }
            if let Some(response) = operation
                .responses
                .default
                .as_ref()
                .and_then(|response| resolve_response(document, response))
            {
                inspect(&response.content);
            }
        }
    }
    consumed
}

fn resolve_media<'a>(
    document: &'a Document,
    media: &'a MediaTypeObject,
) -> Option<&'a MediaTypeObject> {
    let mut current = media;
    let mut seen = HashSet::new();
    while let Some(reference) = current.reference.as_ref() {
        let name = reference
            .reference
            .strip_prefix("#/components/mediaTypes/")?;
        if !seen.insert(name) {
            return None;
        }
        current = document.components.media_types.get(name)?;
    }
    Some(current)
}

fn resolve_response<'a>(
    document: &'a Document,
    response: &'a RefOr<super::ResponseObject>,
) -> Option<&'a super::ResponseObject> {
    let mut current = response;
    let mut seen = HashSet::new();
    loop {
        match current {
            RefOr::Item(response) => return Some(response),
            RefOr::Ref(reference) => {
                let name = reference
                    .reference
                    .strip_prefix("#/components/responses/")?;
                if !seen.insert(name) {
                    return None;
                }
                current = document.components.responses.get(name)?;
            }
        }
    }
}

fn annotation_key(provenance: &Provenance) -> AnnotationKey {
    (
        provenance.span.map(|span| span.file),
        provenance.pointer.clone(),
    )
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
        || validation.other
}
