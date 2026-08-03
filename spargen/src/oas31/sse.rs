use std::collections::HashSet;

use crate::diag::{Diagnostics, Provenance};

use super::{JsonType, RefOr, Resolver, Schema, SchemaOr};

/// The typed JSON carried in the `data` field of an OpenAPI 3.2 SSE envelope.
#[derive(Debug, Clone)]
pub(super) struct JsonDataSchema {
    pub schema: SchemaOr,
    /// The `data` property whose content annotations are consumed by the SSE codec.
    pub annotation_site: Provenance,
}

/// Find one unambiguous `data: string` carrying JSON described by `contentSchema` in an SSE
/// `itemSchema`. References and `allOf` members are followed through the normal resolver so the
/// common "generic envelope + specialized data property" shape is recognized.
pub(super) fn json_data_schema(
    item: &RefOr<Schema>,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
) -> Option<JsonDataSchema> {
    let mut candidates = Vec::new();
    let mut visiting = HashSet::new();
    match item {
        RefOr::Item(schema) => {
            collect_schema(schema, resolver, diags, &mut visiting, &mut candidates)
        }
        RefOr::Ref(reference) => {
            if let Ok(resolved) =
                resolver.resolve(&reference.reference, &reference.provenance, diags)
            {
                collect_schema(
                    resolved.schema.as_ref(),
                    resolver,
                    diags,
                    &mut visiting,
                    &mut candidates,
                );
            }
        }
    }
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn collect_schema(
    schema: &Schema,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    visiting: &mut HashSet<String>,
    out: &mut Vec<JsonDataSchema>,
) {
    if let Some(reference) = &schema.reference {
        collect_reference(
            reference,
            &schema.provenance,
            resolver,
            diags,
            visiting,
            out,
        );
    }
    if let Some(data) = schema.properties.get("data") {
        collect_data_schema(data, resolver, diags, visiting, out);
    }
    for member in &schema.all_of {
        collect_schema_or(member, resolver, diags, visiting, out);
    }
}

fn collect_data_schema(
    data: &SchemaOr,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    visiting: &mut HashSet<String>,
    out: &mut Vec<JsonDataSchema>,
) {
    let SchemaOr::Schema(data) = data else {
        return;
    };
    collect_data_schema_node(data, resolver, diags, visiting, out);
}

fn collect_data_schema_node(
    data: &Schema,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    visiting: &mut HashSet<String>,
    out: &mut Vec<JsonDataSchema>,
) {
    if let Some(reference) = &data.reference {
        if visiting.insert(reference.to_owned()) {
            if let Ok(resolved) = resolver.resolve(reference, &data.provenance, diags) {
                collect_data_schema_node(resolved.schema.as_ref(), resolver, diags, visiting, out);
            }
            visiting.remove(reference);
        }
    }
    for member in &data.all_of {
        collect_data_schema(member, resolver, diags, visiting, out);
    }
    let is_string = data.types.types.contains(&JsonType::String)
        && data
            .types
            .types
            .iter()
            .all(|kind| matches!(kind, JsonType::String | JsonType::Null));
    let is_json = data
        .content_media_type
        .as_deref()
        .and_then(|media| media.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if is_string && is_json {
        if let Some(schema) = data.content_schema.as_deref() {
            out.push(JsonDataSchema {
                schema: schema.clone(),
                annotation_site: data.provenance.clone(),
            });
        }
    }
}

fn collect_schema_or(
    schema: &SchemaOr,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    visiting: &mut HashSet<String>,
    out: &mut Vec<JsonDataSchema>,
) {
    if let SchemaOr::Schema(schema) = schema {
        collect_schema(schema, resolver, diags, visiting, out);
    }
}

fn collect_reference(
    reference: &str,
    provenance: &Provenance,
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    visiting: &mut HashSet<String>,
    out: &mut Vec<JsonDataSchema>,
) {
    if !visiting.insert(reference.to_owned()) {
        return;
    }
    if let Ok(resolved) = resolver.resolve(reference, provenance, diags) {
        collect_schema(resolved.schema.as_ref(), resolver, diags, visiting, out);
    }
    visiting.remove(reference);
}
