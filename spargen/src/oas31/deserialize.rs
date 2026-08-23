use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::ir::Method;
use crate::source::{InputBundle, Node, Number, SpannedMap, SpannedValue};

use super::{
    Components, Discriminator, Document, EncodingObject, HeaderObject, Info, JsonType,
    MediaTypeObject, OperationObject, ParameterObject, PathItem, Paths, RefOr, Reference,
    RequestBodyObject, ResponseObject, ResponsesObject, Schema, SchemaOr, SecurityRequirement,
    SecuritySchemeObject, Server, ServerVariable, Tag, TypeSet, ValidationKeywords, XmlHints,
};

const OAS31_DIALECT: &str = "https://spec.openapis.org/oas/3.1/dialect/base";

/// Build the typed [`Document`] from a loaded [`InputBundle`], carrying spans through.
pub fn parse_document(bundle: &InputBundle, diags: &mut Diagnostics) -> Result<Document, Aborted> {
    let root = bundle.root();
    let root_pointer = JsonPointer::root();

    let Some(openapi_value) = required(root, "openapi", &root_pointer, diags) else {
        return Err(Aborted);
    };
    let openapi_text = string(openapi_value).unwrap_or_default();
    if !version_supported(openapi_text) {
        Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance(&root_pointer, openapi_value))
            .message(format!(
                "unsupported OpenAPI version `{openapi_text}`; spargen implements 3.1.x and 3.2.x"
            ))
            .remedy("use an OpenAPI 3.1.x or 3.2.x document; 3.0.x is rejected because it uses different schema semantics")
            .emit(diags);
        return Err(Aborted);
    }

    if let Some(dialect) = root.get("jsonSchemaDialect") {
        let dialect_text = string(dialect);
        if dialect_text != Some(OAS31_DIALECT) {
            Diagnostic::error(
                Code::UnsupportedDialect,
                provenance(&root_pointer.push("jsonSchemaDialect"), dialect),
            )
            .message("jsonSchemaDialect is not the OpenAPI Schema Object dialect")
            .remedy(format!(
                "set jsonSchemaDialect to `{OAS31_DIALECT}`, or omit it"
            ))
            .emit(diags);
        }
    }

    let info = root
        .get("info")
        .and_then(|value| parse_info(value, &root_pointer.push("info"), diags))
        .unwrap_or_else(|| Info {
            title: "API".to_owned(),
            version: "0.0.0".to_owned(),
            summary: None,
            description: None,
        });

    let servers = root
        .get("servers")
        .map(|value| parse_servers(value, &root_pointer.push("servers"), diags))
        .unwrap_or_default();

    let paths = root
        .get("paths")
        .map(|value| parse_paths(value, &root_pointer.push("paths"), diags))
        .unwrap_or_default();

    let components = root
        .get("components")
        .map(|value| parse_components(value, &root_pointer.push("components"), diags))
        .unwrap_or_default();

    let security = root
        .get("security")
        .map(|value| parse_security(value, &root_pointer.push("security")))
        .unwrap_or_default();
    let tags = root
        .get("tags")
        .map(|value| parse_tags(value, &root_pointer.push("tags"), diags))
        .unwrap_or_default();
    validate_tag_hierarchy(&tags, diags);

    if let Some(webhooks) = root.get("webhooks") {
        Diagnostic::warning(
            Code::ServerInitiatedFlowIgnored,
            provenance(&root_pointer.push("webhooks"), webhooks),
        )
        .message("webhooks describe server-initiated calls; no client code is generated for them")
        .emit(diags);
    }

    let document = Document {
        is_oas32: openapi_text.starts_with("3.2."),
        info,
        servers,
        paths,
        components,
        security,
        tags,
        provenance: provenance(&root_pointer, root),
    };
    diags.result(document)
}

fn version_supported(value: &str) -> bool {
    let mut parts = value.split('.');
    let (Some(major), Some(minor), Some(patch)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    // 3.1.x and 3.2.x share the same JSON Schema 2020-12 semantics and lower through one frontend;
    // 3.0.x, sub-3.1, and any 3.3+/malformed version stay genuinely unsupported (`E001`).
    parts.next().is_none()
        && major == "3"
        && (minor == "1" || minor == "2")
        && patch.parse::<u16>().is_ok()
}

fn parse_info(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Info> {
    let _ = object(value, pointer, diags)?;
    Some(Info {
        title: value
            .get("title")
            .and_then(string)
            .unwrap_or("API")
            .to_owned(),
        version: value
            .get("version")
            .and_then(string)
            .unwrap_or("0.0.0")
            .to_owned(),
        summary: value.get("summary").and_then(string).map(str::to_owned),
        description: value.get("description").and_then(string).map(str::to_owned),
    })
}

fn parse_servers(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<Server> {
    array(value)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, value)| parse_server(value, &pointer.index(index), diags))
        .collect()
}

fn parse_server(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Server> {
    let _ = object(value, pointer, diags)?;
    Some(Server {
        name: value.get("name").and_then(string).map(str::to_owned),
        url: value.get("url").and_then(string).unwrap_or("/").to_owned(),
        description: value.get("description").and_then(string).map(str::to_owned),
        variables: value
            .get("variables")
            .and_then(SpannedValue::as_object)
            .map(|variables| {
                variables
                    .iter()
                    .filter_map(|(key, value)| {
                        parse_server_variable(value).map(|variable| (key.name.clone(), variable))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        provenance: provenance(pointer, value),
    })
}

/// Parse one Server Variable Object. `default` is required by the document schema, so a variable
/// without one cannot reach here in a validated document.
fn parse_server_variable(value: &SpannedValue) -> Option<ServerVariable> {
    let _ = value.as_object()?;
    Some(ServerVariable {
        default: value.get("default").and_then(string)?.to_owned(),
        enum_values: value
            .get("enum")
            .and_then(SpannedValue::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(string)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        description: value.get("description").and_then(string).map(str::to_owned),
    })
}

fn parse_paths(value: &SpannedValue, pointer: &JsonPointer, diags: &mut Diagnostics) -> Paths {
    let mut paths = Paths::default();
    if let Some(map) = object(value, pointer, diags) {
        for (key, value) in map.iter() {
            if let Some(item) = parse_path_item(value, &pointer.push(&key.name), diags) {
                paths.items.insert(key.name.clone(), item);
            }
        }
    }
    paths
}

pub(super) fn parse_path_item(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<PathItem> {
    let map = object(value, pointer, diags)?;
    let mut operations = IndexMap::new();
    for (key, value) in map.iter() {
        if let Some(method) = parse_method(&key.name) {
            if let Some(operation) = parse_operation(value, &pointer.push(&key.name), diags) {
                operations.insert(method, operation);
            }
        }
    }
    if let Some(additional) = value.get("additionalOperations") {
        if let Some(additional) = object(additional, &pointer.push("additionalOperations"), diags) {
            for (method, value) in additional.iter() {
                if let Some(operation) = parse_operation(
                    value,
                    &pointer.push("additionalOperations").push(&method.name),
                    diags,
                ) {
                    operations.insert(Method::Custom(method.name.clone()), operation);
                }
            }
        }
    }
    let parameters = value
        .get("parameters")
        .map(|value| parse_ref_array(value, &pointer.push("parameters"), diags, parse_parameter))
        .unwrap_or_default();
    // `$ref` on a Path Item is not a Reference Object: the specification explicitly leaves the
    // behavior of adjacent fields undefined. `summary`/`description` are documentation and cannot
    // change the wire, so they are allowed to override; anything structural is refused.
    let reference = value
        .get("$ref")
        .and_then(string)
        .map(|reference| Reference {
            reference: reference.to_owned(),
            provenance: provenance(pointer, value),
        });
    let reference_siblings = if reference.is_some() {
        map.iter()
            .map(|(key, _)| key.name.clone())
            .filter(|key| !matches!(key.as_str(), "$ref" | "summary" | "description"))
            .collect()
    } else {
        Vec::new()
    };
    Some(PathItem {
        reference,
        reference_siblings,
        operations,
        parameters,
    })
}

fn parse_operation(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<OperationObject> {
    let _ = object(value, pointer, diags)?;
    if let Some(callbacks) = value.get("callbacks") {
        Diagnostic::warning(
            Code::ServerInitiatedFlowIgnored,
            provenance(&pointer.push("callbacks"), callbacks),
        )
        .message("callbacks describe server-initiated calls; no client code is generated for them")
        .emit(diags);
    }
    let parameters = value
        .get("parameters")
        .map(|value| parse_ref_array(value, &pointer.push("parameters"), diags, parse_parameter))
        .unwrap_or_default();
    let request_body = value.get("requestBody").and_then(|value| {
        parse_ref_or(
            value,
            &pointer.push("requestBody"),
            diags,
            parse_request_body,
        )
    });
    let responses = value
        .get("responses")
        .map(|value| parse_responses(value, &pointer.push("responses"), diags))
        .unwrap_or_default();
    Some(OperationObject {
        operation_id: value.get("operationId").and_then(string).map(str::to_owned),
        summary: value.get("summary").and_then(string).map(str::to_owned),
        description: value.get("description").and_then(string).map(str::to_owned),
        parameters,
        request_body,
        responses,
        security: value
            .get("security")
            .map(|value| parse_security(value, &pointer.push("security"))),
        deprecated: value
            .get("deprecated")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        tags: value
            .get("tags")
            .and_then(array)
            .unwrap_or_default()
            .iter()
            .filter_map(string)
            .map(str::to_owned)
            .collect(),
        provenance: provenance(pointer, value),
    })
}

pub(super) fn parse_parameter(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<ParameterObject> {
    let _ = object(value, pointer, diags)?;
    Some(ParameterObject {
        name: value
            .get("name")
            .and_then(string)
            .unwrap_or_default()
            .to_owned(),
        location: value
            .get("in")
            .and_then(string)
            .unwrap_or_default()
            .to_owned(),
        required: value
            .get("required")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        deprecated: value
            .get("deprecated")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        style: value.get("style").and_then(string).map(str::to_owned),
        explode: value.get("explode").and_then(SpannedValue::as_bool),
        allow_reserved: value
            .get("allowReserved")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        allow_empty_value: value
            .get("allowEmptyValue")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        schema: value
            .get("schema")
            .and_then(|value| parse_schema_ref_or(value, &pointer.push("schema"), diags)),
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
        provenance: provenance(pointer, value),
    })
}

pub(super) fn parse_request_body(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<RequestBodyObject> {
    let _ = object(value, pointer, diags)?;
    Some(RequestBodyObject {
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
        required: value
            .get("required")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        provenance: provenance(pointer, value),
    })
}

fn parse_responses(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> ResponsesObject {
    let mut responses = ResponsesObject::default();
    if let Some(map) = object(value, pointer, diags) {
        for (key, value) in map.iter() {
            let parsed = parse_ref_or(value, &pointer.push(&key.name), diags, parse_response);
            if key.name == "default" {
                responses.default = parsed;
            } else if let Some(parsed) = parsed {
                responses.by_status.insert(key.name.clone(), parsed);
            }
        }
    }
    responses
}

pub(super) fn parse_response(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<ResponseObject> {
    let _ = object(value, pointer, diags)?;
    if let Some(links) = value.get("links") {
        Diagnostic::warning(
            Code::ServerInitiatedFlowIgnored,
            provenance(&pointer.push("links"), links),
        )
        .message("response links describe hypermedia flows; no client code is generated for them")
        .emit(diags);
    }
    Some(ResponseObject {
        summary: value.get("summary").and_then(string).map(str::to_owned),
        description: value.get("description").and_then(string).map(str::to_owned),
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
        provenance: provenance(pointer, value),
    })
}

fn parse_tags(value: &SpannedValue, pointer: &JsonPointer, diags: &mut Diagnostics) -> Vec<Tag> {
    array(value)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let pointer = pointer.index(index);
            let _ = object(value, &pointer, diags)?;
            Some(Tag {
                name: value
                    .get("name")
                    .and_then(string)
                    .unwrap_or_default()
                    .to_owned(),
                summary: value.get("summary").and_then(string).map(str::to_owned),
                description: value.get("description").and_then(string).map(str::to_owned),
                parent: value.get("parent").and_then(string).map(str::to_owned),
                kind: value.get("kind").and_then(string).map(str::to_owned),
                provenance: provenance(&pointer, value),
            })
        })
        .collect()
}

fn validate_tag_hierarchy(tags: &[Tag], diags: &mut Diagnostics) {
    let by_name: IndexMap<&str, &Tag> = tags.iter().map(|tag| (tag.name.as_str(), tag)).collect();
    for tag in tags {
        let mut seen = std::collections::HashSet::new();
        let mut current = tag;
        while let Some(parent) = current.parent.as_deref() {
            if !seen.insert(current.name.as_str()) {
                Diagnostic::error(Code::InvalidInput, tag.provenance.clone())
                    .message(format!(
                        "tag hierarchy containing `{}` has a cycle",
                        tag.name
                    ))
                    .emit(diags);
                break;
            }
            let Some(next) = by_name.get(parent) else {
                Diagnostic::error(Code::InvalidInput, tag.provenance.clone())
                    .message(format!(
                        "tag `{}` references missing parent `{parent}`",
                        tag.name
                    ))
                    .emit(diags);
                break;
            };
            current = next;
        }
    }
}

fn parse_media_map(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> IndexMap<String, MediaTypeObject> {
    object(value, pointer, diags)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.name.clone(),
                        parse_media_type(value, &pointer.push(&key.name), diags),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_media_type(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> MediaTypeObject {
    MediaTypeObject {
        reference: value
            .get("$ref")
            .and_then(string)
            .map(|reference| Reference {
                reference: reference.to_owned(),
                provenance: provenance(pointer, value),
            }),
        schema: value
            .get("schema")
            .and_then(|schema| parse_schema_ref_or(schema, &pointer.push("schema"), diags)),
        item_schema: value
            .get("itemSchema")
            .and_then(|schema| parse_schema_ref_or(schema, &pointer.push("itemSchema"), diags)),
        encoding: value
            .get("encoding")
            .and_then(SpannedValue::as_object)
            .map(|map| {
                let encoding_pointer = pointer.push("encoding");
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.name.clone(),
                            parse_encoding(value, &encoding_pointer.push(&key.name), diags),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        prefix_encoding: value
            .get("prefixEncoding")
            .and_then(SpannedValue::as_array)
            .map(|items| {
                let prefix_pointer = pointer.push("prefixEncoding");
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let at = prefix_pointer.index(index);
                        let encoding = parse_encoding(item, &at, diags);
                        let where_ = provenance(&at, item);
                        (encoding, where_)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        item_encoding: value.get("itemEncoding").map(|item| {
            let at = pointer.push("itemEncoding");
            let encoding = parse_encoding(item, &at, diags);
            let where_ = provenance(&at, item);
            (encoding, where_)
        }),
        provenance: provenance(pointer, value),
    }
}

/// Parse one Encoding Object. The RFC 6570 fields stay `Option` because the specification's mode
/// switch keys on their presence rather than their value.
fn parse_encoding(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> EncodingObject {
    let Some(map) = object(value, pointer, diags) else {
        return empty_encoding(provenance(pointer, value));
    };
    EncodingObject {
        content_type: map.get("contentType").and_then(string).map(str::to_owned),
        headers: map
            .get("headers")
            .and_then(SpannedValue::as_object)
            .map(|headers| {
                let headers_pointer = pointer.push("headers");
                headers
                    .iter()
                    .filter_map(|(key, value)| {
                        let at = headers_pointer.push(&key.name);
                        parse_ref_or(value, &at, diags, parse_header_object)
                            .map(|header| (key.name.clone(), header))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        style: map.get("style").and_then(string).map(str::to_owned),
        explode: map.get("explode").and_then(SpannedValue::as_bool),
        allow_reserved: map.get("allowReserved").and_then(SpannedValue::as_bool),
        nested: ["encoding", "prefixEncoding", "itemEncoding"]
            .into_iter()
            .filter_map(|field| {
                map.get(field)
                    .map(|value| (field.to_owned(), provenance(&pointer.push(field), value)))
            })
            .collect(),
        provenance: provenance(pointer, value),
    }
}

/// An Encoding Object that declares nothing, used when the node is not an object (already
/// reported) so parsing can continue and collect the rest of the document's diagnostics.
fn empty_encoding(provenance: Provenance) -> EncodingObject {
    EncodingObject {
        content_type: None,
        headers: IndexMap::new(),
        style: None,
        explode: None,
        allow_reserved: None,
        nested: Vec::new(),
        provenance,
    }
}

/// Parse one Header Object — the Parameter Object shape without `name`/`in`.
fn parse_header_object(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<HeaderObject> {
    let map = object(value, pointer, diags)?;
    Some(HeaderObject {
        schema: map
            .get("schema")
            .and_then(|schema| parse_schema_ref_or(schema, &pointer.push("schema"), diags)),
    })
}

fn parse_components(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Components {
    let mut components = Components::default();
    let Some(map) = object(value, pointer, diags) else {
        return components;
    };
    // Schema components use `parse_schema_ref_or` (not the generic `parse_component_map`) so a
    // component-root `$ref`+`default` is acknowledged with `W005` rather than silently dropped.
    components.schemas = map
        .get("schemas")
        .and_then(SpannedValue::as_object)
        .map(|schemas| {
            let schemas_pointer = pointer.push("schemas");
            schemas
                .iter()
                .filter_map(|(key, value)| {
                    parse_schema_ref_or(value, &schemas_pointer.push(&key.name), diags)
                        .map(|item| (key.name.clone(), item))
                })
                .collect()
        })
        .unwrap_or_default();
    components.responses = parse_component_map(
        map.get("responses"),
        &pointer.push("responses"),
        diags,
        parse_response,
    );
    components.parameters = parse_component_map(
        map.get("parameters"),
        &pointer.push("parameters"),
        diags,
        parse_parameter,
    );
    components.request_bodies = parse_component_map(
        map.get("requestBodies"),
        &pointer.push("requestBodies"),
        diags,
        parse_request_body,
    );
    components.media_types = map
        .get("mediaTypes")
        .and_then(SpannedValue::as_object)
        .map(|media_types| {
            media_types
                .iter()
                .map(|(key, value)| {
                    (
                        key.name.clone(),
                        parse_media_type(value, &pointer.push("mediaTypes").push(&key.name), diags),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    components.path_items = map
        .get("pathItems")
        .and_then(SpannedValue::as_object)
        .map(|items| {
            let items_pointer = pointer.push("pathItems");
            items
                .iter()
                .filter_map(|(key, value)| {
                    parse_path_item(value, &items_pointer.push(&key.name), diags)
                        .map(|item| (key.name.clone(), item))
                })
                .collect()
        })
        .unwrap_or_default();
    components.security_schemes = parse_component_map(
        map.get("securitySchemes"),
        &pointer.push("securitySchemes"),
        diags,
        parse_security_scheme,
    );
    components
}

fn parse_component_map<T>(
    value: Option<&SpannedValue>,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
    parse: fn(&SpannedValue, &JsonPointer, &mut Diagnostics) -> Option<T>,
) -> IndexMap<String, RefOr<T>> {
    value
        .and_then(SpannedValue::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    parse_ref_or(value, &pointer.push(&key.name), diags, parse)
                        .map(|item| (key.name.clone(), item))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_security_scheme(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<SecuritySchemeObject> {
    let _ = object(value, pointer, diags)?;
    Some(SecuritySchemeObject {
        scheme_type: value
            .get("type")
            .and_then(string)
            .unwrap_or_default()
            .to_owned(),
        scheme: value.get("scheme").and_then(string).map(str::to_owned),
        location: value.get("in").and_then(string).map(str::to_owned),
        name: value.get("name").and_then(string).map(str::to_owned),
        provenance: provenance(pointer, value),
    })
}

/// Parse a schema position that may be a `$ref` or an inline [`Schema`]. Unlike the generic
/// [`parse_ref_or`], a `default` declared *alongside* a `$ref` here is a schema `default` that the
/// reference-resolution drops on the floor; acknowledge it as `W005` so it is never silently lost.
/// Property-position `$ref`+`default` is handled elsewhere (via `SchemaOr`) and does not reach here.
fn parse_schema_ref_or(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<RefOr<Schema>> {
    if let Some(reference) = value.get("$ref").and_then(string) {
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
        Some(RefOr::Ref(Reference {
            reference: reference.to_owned(),
            provenance: provenance(pointer, value),
        }))
    } else {
        parse_schema(value, pointer, diags).map(RefOr::Item)
    }
}

pub(super) fn parse_schema(
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
        Diagnostic::error(Code::UnresolvedRef, provenance(pointer, value))
            .message("static `$id`/`$anchor` schema resource scopes are not yet supported")
            .emit(diags);
    }
    if let Some(dialect) = map.get("$schema").and_then(string) {
        if dialect != OAS31_DIALECT {
            Diagnostic::error(
                Code::UnsupportedDialect,
                provenance(
                    &pointer.push("$schema"),
                    map.get("$schema").expect("present"),
                ),
            )
            .message(format!(
                "schema resource uses unsupported dialect `{dialect}`"
            ))
            .emit(diags);
        }
    }

    let schema = Schema {
        boolean: None,
        types: parse_type_set(map.get("type")),
        reference: map.get("$ref").and_then(string).map(str::to_owned),
        properties: map
            .get("properties")
            .and_then(SpannedValue::as_object)
            .map(|properties| {
                properties
                    .iter()
                    .filter_map(|(key, value)| {
                        parse_schema_or(value, &pointer.push("properties").push(&key.name), diags)
                            .map(|schema| (key.name.clone(), schema))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        required: map
            .get("required")
            .and_then(array)
            .unwrap_or_default()
            .iter()
            .filter_map(string)
            .map(str::to_owned)
            .collect(),
        additional_properties: map.get("additionalProperties").and_then(|value| {
            parse_schema_or(value, &pointer.push("additionalProperties"), diags).map(Box::new)
        }),
        pattern_properties: map
            .get("patternProperties")
            .and_then(SpannedValue::as_object)
            .map(|patterns| {
                patterns
                    .iter()
                    .filter_map(|(key, value)| {
                        parse_schema_or(
                            value,
                            &pointer.push("patternProperties").push(&key.name),
                            diags,
                        )
                        .map(|schema| (key.name.clone(), schema))
                    })
                    .collect()
            })
            .unwrap_or_default(),
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
        defs: map
            .get("$defs")
            .and_then(SpannedValue::as_object)
            .map(|defs| {
                defs.iter()
                    .filter_map(|(key, value)| {
                        parse_schema_or(value, &pointer.push("$defs").push(&key.name), diags)
                            .map(|schema| (key.name.clone(), schema))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        validation_children: parse_validation_children(map, pointer, diags),
        enum_values: map
            .get("enum")
            .and_then(array)
            .map(<[SpannedValue]>::to_vec),
        const_value: map.get("const").cloned(),
        default: map.get("default").cloned(),
        format: map.get("format").and_then(string).map(str::to_owned),
        content_encoding: map
            .get("contentEncoding")
            .and_then(string)
            .map(str::to_owned),
        content_media_type: map
            .get("contentMediaType")
            .and_then(string)
            .map(str::to_owned),
        content_schema: map.get("contentSchema").and_then(|value| {
            parse_schema_or(value, &pointer.push("contentSchema"), diags).map(Box::new)
        }),
        xml: map.get("xml").and_then(parse_xml),
        validation: parse_validation(map),
        deprecated: map
            .get("deprecated")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        read_only: map
            .get("readOnly")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        write_only: map
            .get("writeOnly")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        title: map.get("title").and_then(string).map(str::to_owned),
        description: map.get("description").and_then(string).map(str::to_owned),
        provenance: provenance(pointer, value),
    };
    Some(SchemaOr::Schema(Box::new(schema)))
}

fn parse_schema_array(
    value: Option<&SpannedValue>,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<SchemaOr> {
    value
        .and_then(array)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, value)| parse_schema_or(value, &pointer.index(index), diags))
        .collect()
}

fn parse_discriminator(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Discriminator> {
    let _ = object(value, pointer, diags)?;
    if let Some(default_mapping) = value.get("defaultMapping") {
        Diagnostic::error(
            Code::NonDisjointUnion,
            provenance(&pointer.push("defaultMapping"), default_mapping),
        )
        .message(
            "discriminator.defaultMapping requires a generated fallback branch for absent or \
             unknown discriminator values, which is not yet representable",
        )
        .emit(diags);
    }
    Some(Discriminator {
        property_name: value
            .get("propertyName")
            .and_then(string)
            .unwrap_or_default()
            .to_owned(),
        mapping: value
            .get("mapping")
            .and_then(SpannedValue::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| {
                        string(value).map(|value| (key.name.clone(), value.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse the OpenAPI `xml` object. Malformed shapes (a non-object `xml`) yield `None`; individual
/// missing keys default. Lowering later warns (`W006`) on the unsupported namespace/prefix/wrapped
/// hints, so they are captured here rather than dropped at parse time.
fn parse_xml(value: &SpannedValue) -> Option<XmlHints> {
    let _ = value.as_object()?;
    Some(XmlHints {
        name: value.get("name").and_then(string).map(str::to_owned),
        attribute: value.get("nodeType").and_then(string) == Some("attribute")
            || value
                .get("attribute")
                .and_then(SpannedValue::as_bool)
                .unwrap_or(false),
        node_type: value.get("nodeType").and_then(string).map(str::to_owned),
        namespace: value.get("namespace").and_then(string).map(str::to_owned),
        prefix: value.get("prefix").and_then(string).map(str::to_owned),
        wrapped: value
            .get("wrapped")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
    })
}

fn parse_type_set(value: Option<&SpannedValue>) -> TypeSet {
    let mut types = Vec::new();
    match value.map(|value| &value.node) {
        Some(Node::String(value)) => {
            if let Some(ty) = parse_json_type(value) {
                types.push(ty);
            }
        }
        Some(Node::Array(values)) => {
            for value in values {
                if let Some(ty) = value.as_str().and_then(parse_json_type) {
                    types.push(ty);
                }
            }
        }
        _ => {}
    }
    TypeSet { types }
}

fn parse_json_type(value: &str) -> Option<JsonType> {
    Some(match value {
        "null" => JsonType::Null,
        "boolean" => JsonType::Boolean,
        "object" => JsonType::Object,
        "array" => JsonType::Array,
        "number" => JsonType::Number,
        "integer" => JsonType::Integer,
        "string" => JsonType::String,
        _ => return None,
    })
}

fn parse_validation(map: &SpannedMap) -> ValidationKeywords {
    ValidationKeywords {
        pattern: map.get("pattern").and_then(string).map(str::to_owned),
        minimum: map.get("minimum").and_then(number_f64),
        maximum: map.get("maximum").and_then(number_f64),
        exclusive_minimum: map.get("exclusiveMinimum").and_then(number_f64),
        exclusive_maximum: map.get("exclusiveMaximum").and_then(number_f64),
        multiple_of: map.get("multipleOf").and_then(number_f64),
        min_length: map.get("minLength").and_then(number_u64),
        max_length: map.get("maxLength").and_then(number_u64),
        min_items: map.get("minItems").and_then(number_u64),
        max_items: map.get("maxItems").and_then(number_u64),
        unique_items: map
            .get("uniqueItems")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        min_properties: map.get("minProperties").and_then(number_u64),
        max_properties: map.get("maxProperties").and_then(number_u64),
        other: [
            "not",
            "if",
            "then",
            "else",
            "contains",
            "minContains",
            "maxContains",
            "dependentSchemas",
            "dependentRequired",
            "propertyNames",
            "unevaluatedProperties",
            "unevaluatedItems",
        ]
        .iter()
        .any(|keyword| map.get(keyword).is_some()),
    }
}

fn parse_validation_children(
    map: &SpannedMap,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Vec<(String, SchemaOr)> {
    let mut children = Vec::new();
    for keyword in [
        "not",
        "if",
        "then",
        "else",
        "contains",
        "propertyNames",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if let Some(value) = map.get(keyword) {
            if let Some(schema) = parse_schema_or(value, &pointer.push(keyword), diags) {
                children.push((keyword.to_owned(), schema));
            }
        }
    }
    if let Some(dependent) = map
        .get("dependentSchemas")
        .and_then(SpannedValue::as_object)
    {
        for (key, value) in dependent.iter() {
            if let Some(schema) = parse_schema_or(
                value,
                &pointer.push("dependentSchemas").push(&key.name),
                diags,
            ) {
                children.push((format!("dependentSchemas/{}", key.name), schema));
            }
        }
    }
    children
}

fn parse_ref_array<T>(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
    parse: fn(&SpannedValue, &JsonPointer, &mut Diagnostics) -> Option<T>,
) -> Vec<RefOr<T>> {
    array(value)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, value)| parse_ref_or(value, &pointer.index(index), diags, parse))
        .collect()
}

fn parse_ref_or<T>(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
    parse: fn(&SpannedValue, &JsonPointer, &mut Diagnostics) -> Option<T>,
) -> Option<RefOr<T>> {
    if let Some(reference) = value.get("$ref").and_then(string) {
        Some(RefOr::Ref(Reference {
            reference: reference.to_owned(),
            provenance: provenance(pointer, value),
        }))
    } else {
        parse(value, pointer, diags).map(RefOr::Item)
    }
}

fn parse_security(value: &SpannedValue, _pointer: &JsonPointer) -> Vec<SecurityRequirement> {
    array(value)
        .unwrap_or_default()
        .iter()
        .filter_map(|value| {
            value.as_object().map(|map| {
                SecurityRequirement(
                    map.iter()
                        .map(|(key, value)| {
                            (
                                key.name.clone(),
                                array(value)
                                    .unwrap_or_default()
                                    .iter()
                                    .filter_map(string)
                                    .map(str::to_owned)
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
        })
        .collect()
}

fn parse_method(value: &str) -> Option<Method> {
    Some(match value {
        "get" => Method::Get,
        "put" => Method::Put,
        "post" => Method::Post,
        "delete" => Method::Delete,
        "options" => Method::Options,
        "head" => Method::Head,
        "patch" => Method::Patch,
        "trace" => Method::Trace,
        // The `QUERY` method is a fixed path-item field added by OpenAPI 3.2.
        "query" => Method::Query,
        _ => return None,
    })
}

fn required<'a>(
    value: &'a SpannedValue,
    key: &str,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<&'a SpannedValue> {
    let found = value.get(key);
    if found.is_none() {
        Diagnostic::error(Code::InvalidInput, provenance(pointer, value))
            .message(format!("missing required OpenAPI field `{key}`"))
            .emit(diags);
    }
    found
}

fn object<'a>(
    value: &'a SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<&'a SpannedMap> {
    let object = value.as_object();
    if object.is_none() {
        Diagnostic::error(Code::InvalidInput, provenance(pointer, value))
            .message("expected an object")
            .emit(diags);
    }
    object
}

fn array(value: &SpannedValue) -> Option<&[SpannedValue]> {
    value.as_array()
}

fn string(value: &SpannedValue) -> Option<&str> {
    value.as_str()
}

fn number_f64(value: &SpannedValue) -> Option<f64> {
    match &value.node {
        Node::Number(Number::Float(value)) => Some(*value),
        Node::Number(Number::Int(value)) => Some(*value as f64),
        Node::Number(Number::UInt(value)) => Some(*value as f64),
        _ => None,
    }
}

fn number_u64(value: &SpannedValue) -> Option<u64> {
    match &value.node {
        Node::Number(Number::UInt(value)) => Some(*value),
        Node::Number(Number::Int(value)) => (*value >= 0).then_some(*value as u64),
        _ => None,
    }
}

fn provenance(pointer: &JsonPointer, value: &SpannedValue) -> Provenance {
    Provenance::new(pointer.clone(), Some(value.span()))
}
