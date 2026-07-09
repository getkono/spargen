use crate::diag::{Code, Diagnostic, Diagnostics, Disposition, JsonPointer};

use super::{Document, MediaTypeObject, RefOr, Resolver, Schema, SchemaOr, ValidationKeywords};

/// The per-keyword S/W/R disposition audit — the core of `spargen check`.
pub fn audit(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> DispositionReport {
    let _ = resolver;
    let mut report = DispositionReport::default();

    for (name, schema) in &document.components.schemas {
        if let RefOr::Item(schema) = schema {
            audit_schema(
                schema,
                JsonPointer::root()
                    .push("components")
                    .push("schemas")
                    .push(name),
                &mut report,
                diags,
            );
        }
    }

    for (path, item) in &document.paths.items {
        for (method, operation) in &item.operations {
            let op_pointer = JsonPointer::root()
                .push("paths")
                .push(path)
                .push(method_key(*method));
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
                            &mut report,
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
                            &mut report,
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
                        &mut report,
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
                            &mut report,
                            diags,
                        );
                    }
                }
            }
        }
    }

    report
}

/// The result of an [`audit`]: one entry per classified construct.
#[derive(Debug, Clone, Default)]
pub struct DispositionReport {
    /// The classified constructs, in document order.
    pub entries: Vec<DispositionEntry>,
}

/// One construct's disposition.
#[derive(Debug, Clone)]
pub struct DispositionEntry {
    /// Pointer to the construct.
    pub pointer: JsonPointer,
    /// Its S/W/R class.
    pub disposition: Disposition,
    /// The diagnostic code, for W and R constructs.
    pub code: Option<Code>,
}

fn audit_media(
    media: &MediaTypeObject,
    pointer: JsonPointer,
    report: &mut DispositionReport,
    diags: &mut Diagnostics,
) {
    if let Some(RefOr::Item(schema)) = &media.schema {
        audit_schema(schema, pointer.push("schema"), report, diags);
    }
}

fn audit_schema(
    schema: &Schema,
    pointer: JsonPointer,
    report: &mut DispositionReport,
    diags: &mut Diagnostics,
) {
    report.entries.push(DispositionEntry {
        pointer: pointer.clone(),
        disposition: Disposition::Supported,
        code: None,
    });

    if has_validation_keywords(&schema.validation) {
        report.entries.push(DispositionEntry {
            pointer: pointer.clone(),
            disposition: Disposition::Warned,
            code: Some(Code::ValidationKeywordIgnored),
        });
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message("validation-only schema keywords are not enforced at runtime")
            .remedy("keep producer-side validation for these constraints")
            .emit(diags);
    }

    for (name, child) in &schema.properties {
        audit_schema_or(child, pointer.push("properties").push(name), report, diags);
    }
    if let Some(child) = &schema.additional_properties {
        audit_schema_or(child, pointer.push("additionalProperties"), report, diags);
    }
    if let Some(child) = &schema.items {
        audit_schema_or(child, pointer.push("items"), report, diags);
    }
    for (index, child) in schema.prefix_items.iter().enumerate() {
        audit_schema_or(
            child,
            pointer.push("prefixItems").index(index),
            report,
            diags,
        );
    }
    for (index, child) in schema.all_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("allOf").index(index), report, diags);
    }
    for (index, child) in schema.one_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("oneOf").index(index), report, diags);
    }
    for (index, child) in schema.any_of.iter().enumerate() {
        audit_schema_or(child, pointer.push("anyOf").index(index), report, diags);
    }
    for (name, child) in &schema.defs {
        audit_schema_or(child, pointer.push("$defs").push(name), report, diags);
    }
}

fn audit_schema_or(
    schema: &SchemaOr,
    pointer: JsonPointer,
    report: &mut DispositionReport,
    diags: &mut Diagnostics,
) {
    match schema {
        SchemaOr::Bool(_) => report.entries.push(DispositionEntry {
            pointer,
            disposition: Disposition::Supported,
            code: None,
        }),
        SchemaOr::Schema(schema) => audit_schema(schema, pointer, report, diags),
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

fn method_key(method: crate::ir::Method) -> &'static str {
    match method {
        crate::ir::Method::Get => "get",
        crate::ir::Method::Put => "put",
        crate::ir::Method::Post => "post",
        crate::ir::Method::Delete => "delete",
        crate::ir::Method::Options => "options",
        crate::ir::Method::Head => "head",
        crate::ir::Method::Patch => "patch",
        crate::ir::Method::Trace => "trace",
    }
}
