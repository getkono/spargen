use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::ir::Method;
use crate::source::{InputBundle, Node, Number, SpannedMap, SpannedValue};

use super::{
    Components, Discriminator, Document, HeaderObject, Info, JsonType, MediaTypeObject,
    OperationObject, ParameterObject, PathItem, Paths, RefOr, Reference, RequestBodyObject,
    ResponseObject, ResponsesObject, Schema, SchemaOr, SecurityRequirement, SecuritySchemeObject,
    Server, ServerVariable, Tag, TypeSet, ValidationKeywords, Version,
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
    let Some(openapi) = parse_version(openapi_text) else {
        Diagnostic::error(Code::UnsupportedOpenApiVersion, provenance(&root_pointer, openapi_value))
            .message(format!(
                "unsupported OpenAPI version `{openapi_text}`; spargen currently implements 3.1.x"
            ))
            .remedy("use an OpenAPI 3.1.x document; 3.0.x is rejected because it uses different schema semantics")
            .emit(diags);
        return Err(Aborted);
    };

    if let Some(dialect) = root.get("jsonSchemaDialect") {
        if string(dialect) != Some(OAS31_DIALECT) {
            Diagnostic::error(
                Code::UnsupportedDialect,
                provenance(&root_pointer.push("jsonSchemaDialect"), dialect),
            )
            .message("jsonSchemaDialect is not the OAS 3.1 base dialect")
            .remedy(format!(
                "set jsonSchemaDialect to `{OAS31_DIALECT}` or omit it"
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
        .map(|value| parse_tags(value, &root_pointer.push("tags")))
        .unwrap_or_default();

    let document = Document {
        openapi,
        info,
        servers,
        paths,
        components,
        security,
        tags,
        provenance: provenance(&root_pointer, root),
    };
    diags.into_result(document)
}

fn parse_version(value: &str) -> Option<Version> {
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() || major != 3 || minor != 1 {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
    })
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
    let variables = value
        .get("variables")
        .and_then(SpannedValue::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    let default = value.get("default").and_then(string)?.to_owned();
                    Some((
                        key.name.clone(),
                        ServerVariable {
                            default,
                            enumeration: value
                                .get("enum")
                                .and_then(array)
                                .unwrap_or_default()
                                .iter()
                                .filter_map(string)
                                .map(str::to_owned)
                                .collect(),
                            description: value
                                .get("description")
                                .and_then(string)
                                .map(str::to_owned),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Server {
        url: value.get("url").and_then(string).unwrap_or("/").to_owned(),
        description: value.get("description").and_then(string).map(str::to_owned),
        variables,
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

fn parse_path_item(
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
    let parameters = value
        .get("parameters")
        .map(|value| parse_ref_array(value, &pointer.push("parameters"), diags, parse_parameter))
        .unwrap_or_default();
    Some(PathItem {
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
        tags: value
            .get("tags")
            .and_then(array)
            .unwrap_or_default()
            .iter()
            .filter_map(string)
            .map(str::to_owned)
            .collect(),
        deprecated: value
            .get("deprecated")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        provenance: provenance(pointer, value),
    })
}

fn parse_parameter(
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
        schema: value
            .get("schema")
            .and_then(|value| parse_ref_or(value, &pointer.push("schema"), diags, parse_schema)),
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
        provenance: provenance(pointer, value),
    })
}

fn parse_request_body(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<RequestBodyObject> {
    let _ = object(value, pointer, diags)?;
    Some(RequestBodyObject {
        description: value.get("description").and_then(string).map(str::to_owned),
        required: value
            .get("required")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
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

fn parse_response(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<ResponseObject> {
    let _ = object(value, pointer, diags)?;
    Some(ResponseObject {
        description: value
            .get("description")
            .and_then(string)
            .unwrap_or_default()
            .to_owned(),
        headers: value
            .get("headers")
            .and_then(SpannedValue::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| {
                        parse_ref_or(
                            value,
                            &pointer.push("headers").push(&key.name),
                            diags,
                            parse_header,
                        )
                        .map(|header| (key.name.clone(), header))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        content: value
            .get("content")
            .map(|value| parse_media_map(value, &pointer.push("content"), diags))
            .unwrap_or_default(),
        provenance: provenance(pointer, value),
    })
}

fn parse_header(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<HeaderObject> {
    let _ = object(value, pointer, diags)?;
    Some(HeaderObject {
        description: value.get("description").and_then(string).map(str::to_owned),
        required: value
            .get("required")
            .and_then(SpannedValue::as_bool)
            .unwrap_or(false),
        schema: value
            .get("schema")
            .and_then(|value| parse_ref_or(value, &pointer.push("schema"), diags, parse_schema)),
        provenance: provenance(pointer, value),
    })
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
                        MediaTypeObject {
                            schema: value.get("schema").and_then(|schema| {
                                parse_ref_or(
                                    schema,
                                    &pointer.push(&key.name).push("schema"),
                                    diags,
                                    parse_schema,
                                )
                            }),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
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
    components.schemas = parse_component_map(
        map.get("schemas"),
        &pointer.push("schemas"),
        diags,
        parse_schema,
    );
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
    components.headers = parse_component_map(
        map.get("headers"),
        &pointer.push("headers"),
        diags,
        parse_header,
    );
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
        openid_connect_url: value
            .get("openIdConnectUrl")
            .and_then(string)
            .map(str::to_owned),
        provenance: provenance(pointer, value),
    })
}

fn parse_schema(
    value: &SpannedValue,
    pointer: &JsonPointer,
    diags: &mut Diagnostics,
) -> Option<Schema> {
    let SchemaOr::Schema(schema) = parse_schema_or(value, pointer, diags)? else {
        Diagnostic::error(Code::InvalidInput, provenance(pointer, value))
            .message("boolean schema is not valid in this OpenAPI position")
            .emit(diags);
        return None;
    };
    Some(*schema)
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

    if map.get("patternProperties").is_some() {
        Diagnostic::error(
            Code::PatternPropertiesRejected,
            provenance(
                &pointer.push("patternProperties"),
                map.get("patternProperties").unwrap(),
            ),
        )
        .message("patternProperties is not represented in generated Rust types")
        .emit(diags);
    }
    if map.get("$dynamicRef").is_some() || map.get("$dynamicAnchor").is_some() {
        Diagnostic::error(Code::DynamicRefRejected, provenance(pointer, value))
            .message("$dynamicRef and $dynamicAnchor require dynamic schema scope evaluation")
            .emit(diags);
    }

    let schema = Schema {
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
        enum_values: map
            .get("enum")
            .and_then(array)
            .map(<[SpannedValue]>::to_vec),
        const_value: map.get("const").cloned(),
        format: map.get("format").and_then(string).map(str::to_owned),
        content_encoding: map
            .get("contentEncoding")
            .and_then(string)
            .map(str::to_owned),
        default: map.get("default").cloned(),
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
        extensions: map
            .iter()
            .filter(|(key, _)| key.name.starts_with("x-"))
            .map(|(key, value)| (key.name.clone(), value.clone()))
            .collect(),
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
    }
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

fn parse_tags(value: &SpannedValue, _pointer: &JsonPointer) -> Vec<Tag> {
    array(value)
        .unwrap_or_default()
        .iter()
        .filter_map(|value| {
            value.as_object().map(|_| Tag {
                name: value
                    .get("name")
                    .and_then(string)
                    .unwrap_or_default()
                    .to_owned(),
                description: value.get("description").and_then(string).map(str::to_owned),
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
