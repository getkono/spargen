use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics};
use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, Docs, EnumVariant, Field, HeaderSpec, HttpScheme, Info,
    MediaType, OAuthMeta, OidcMeta, Operation, OperationId, ParamLoc, ParamStyle, Parameter,
    PathSegment, PathTemplate, Prim, PropertyName, RequestBody, Response, Responses, ScalarDefault,
    ScalarEnum, ScalarRepr, ScalarValue, SchemeId, SecurityScheme, Server, ServerVariable,
    StatusSpec, Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
};
use crate::name::synth_operation_id;
use crate::source::{Node, Number, SpannedValue};

use super::{
    Document, JsonType, ParameterObject, RefOr, RequestBodyObject, Resolver, ResponseObject,
    Schema, SchemaOr, SecurityRequirement,
};

/// Lower the typed 3.1.1 [`Document`] into the version-agnostic [`Api`] IR.
pub fn lower(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Result<Api, Aborted> {
    let mut ctx = LowerCtx {
        document,
        resolver,
        diags,
        graph: TypeGraph::default(),
        components: HashMap::new(),
    };

    for (name, schema) in &document.components.schemas {
        if matches!(schema, RefOr::Item(_)) {
            let _ = ctx.ensure_component(name);
        }
    }

    let mut operations = Vec::new();
    for (path, item) in &document.paths.items {
        for (method, operation) in &item.operations {
            let path_template = parse_path_template(path);
            let id = operation
                .operation_id
                .clone()
                .unwrap_or_else(|| synth_operation_id(*method, &path_template));

            let mut params = Vec::new();
            for parameter in item.parameters.iter().chain(operation.parameters.iter()) {
                if let Some(parameter) = ctx.resolve_parameter(parameter) {
                    if let Some(parameter) = ctx.lower_parameter(&parameter) {
                        params.push(parameter);
                    }
                }
            }

            let request_body = operation
                .request_body
                .as_ref()
                .and_then(|body| ctx.resolve_request_body(body))
                .and_then(|body| ctx.lower_request_body(&body));

            let responses = ctx.lower_responses(&operation.responses);

            operations.push(Operation {
                id: OperationId(id),
                method: *method,
                path: path_template,
                params,
                request_body,
                responses,
                security: operation
                    .security
                    .as_ref()
                    .unwrap_or(&document.security)
                    .iter()
                    .map(lower_security_requirement)
                    .collect(),
                deprecated: operation.deprecated,
                docs: Docs {
                    title: None,
                    summary: operation.summary.clone(),
                    description: operation.description.clone(),
                    deprecated: operation.deprecated,
                },
                provenance: operation.provenance.clone(),
            });
        }
    }

    let api = Api {
        info: Info {
            title: document.info.title.clone(),
            version: document.info.version.clone(),
            description: document
                .info
                .description
                .clone()
                .or(document.info.summary.clone()),
        },
        servers: document
            .servers
            .iter()
            .map(|server| Server {
                url: server.url.clone(),
                description: server.description.clone(),
                variables: server
                    .variables
                    .iter()
                    .map(|(name, variable)| {
                        (
                            name.clone(),
                            ServerVariable {
                                default: variable.default.clone(),
                                enumeration: variable.enumeration.clone(),
                                description: variable.description.clone(),
                            },
                        )
                    })
                    .collect(),
            })
            .collect(),
        operations,
        types: ctx.graph,
        security_schemes: lower_security_schemes(document),
        provenance: document.provenance.clone(),
    };
    ctx.diags.into_result(api)
}

struct LowerCtx<'a, 'doc> {
    document: &'doc Document,
    resolver: &'a Resolver<'doc>,
    diags: &'a mut Diagnostics,
    graph: TypeGraph,
    components: HashMap<String, TypeId>,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    fn ensure_component(&mut self, name: &str) -> Option<Ty> {
        if let Some(id) = self.components.get(name).copied() {
            return Some(Ty {
                id,
                nullable: false,
                boxed: false,
            });
        }
        let RefOr::Item(schema) = self.document.components.schemas.get(name)? else {
            return None;
        };
        let ty = self.lower_schema(schema, name)?;
        self.components.insert(name.to_owned(), ty.id);
        Some(ty)
    }

    fn lower_schema_or(&mut self, schema: &SchemaOr, hint: &str) -> Option<Ty> {
        match schema {
            SchemaOr::Bool(true) => {
                Some(self.insert_type(hint, TypeKind::Any, Docs::default(), None))
            }
            SchemaOr::Bool(false) => {
                Diagnostic::error(Code::InvalidInput, self.document.provenance.clone())
                    .message(
                        "boolean false schemas are not representable in generated client types",
                    )
                    .emit(self.diags);
                None
            }
            SchemaOr::Schema(schema) => self.lower_schema(schema, hint),
        }
    }

    fn lower_schema(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if let Some(reference) = &schema.reference {
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                return self.ensure_component(name);
            }
            let resolved = self
                .resolver
                .resolve(reference, &schema.provenance.pointer, self.diags)
                .ok()?;
            return self.lower_schema(resolved.schema, hint);
        }

        if (!schema.one_of.is_empty() || !schema.any_of.is_empty())
            && schema.discriminator.is_none()
        {
            Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
                .message("oneOf/anyOf without a discriminator is not statically proven disjoint")
                .remedy("add a discriminator or omit this API segment with spargen::omit!")
                .emit(self.diags);
            return None;
        }

        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
                .message("discriminated oneOf/anyOf lowering is not implemented in this slice")
                .remedy(
                    "omit this API segment with spargen::omit! until union lowering is extended",
                )
                .emit(self.diags);
            return None;
        }

        if let Some(enumeration) = &schema.enum_values {
            return self.lower_enum(enumeration, schema, hint);
        }
        if let Some(value) = &schema.const_value {
            return self.lower_enum(std::slice::from_ref(value), schema, hint);
        }

        if schema.content_encoding.as_deref() == Some("base64") {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Bytes));
        }

        let nullable = schema.types.types.contains(&JsonType::Null);
        let primary = schema
            .types
            .types
            .iter()
            .find(|ty| **ty != JsonType::Null)
            .copied();

        let mut ty = match primary {
            Some(JsonType::Boolean) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::Bool))
            }
            Some(JsonType::Integer) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("int32") => Prim::I32,
                    _ => Prim::I64,
                }),
            ),
            Some(JsonType::Number) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::F64))
            }
            Some(JsonType::String) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("uuid") => Prim::Uuid,
                    Some("date-time") => Prim::DateTime,
                    Some("date") => Prim::Date,
                    _ => Prim::String,
                }),
            ),
            Some(JsonType::Array) => {
                if !schema.prefix_items.is_empty() {
                    let mut items = Vec::new();
                    for (index, child) in schema.prefix_items.iter().enumerate() {
                        items.push(self.lower_schema_or(child, &format!("{hint}Item{index}"))?);
                    }
                    self.insert_schema_type(schema, hint, TypeKind::Tuple(items))
                } else {
                    let item = match &schema.items {
                        Some(items) => self.lower_schema_or(items, &format!("{hint}Item"))?,
                        None => self.insert_type(
                            &format!("{hint}Item"),
                            TypeKind::Any,
                            Docs::default(),
                            None,
                        ),
                    };
                    self.insert_schema_type(schema, hint, TypeKind::Array(Box::new(item)))
                }
            }
            Some(JsonType::Object) | None if !schema.properties.is_empty() => {
                self.lower_object(schema, hint)?
            }
            Some(JsonType::Object) => self.lower_object(schema, hint)?,
            Some(JsonType::Null) | None => self.insert_schema_type(schema, hint, TypeKind::Any),
        };
        ty.nullable = nullable;
        Some(ty)
    }

    fn lower_object(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let required = schema.required.iter().cloned().collect::<HashSet<_>>();
        let mut fields = Vec::new();
        for (name, child) in &schema.properties {
            let ty = self.lower_schema_or(child, &format!("{hint}{name}"))?;
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: required.contains(name),
                default: schema.default.as_ref().and_then(lower_default),
                deprecated: schema.deprecated,
                read_only: schema.read_only,
                write_only: schema.write_only,
                docs: Docs::default(),
            });
        }
        let additional = match &schema.additional_properties {
            Some(schema) => match schema.as_ref() {
                SchemaOr::Bool(false) => AdditionalProps::Deny,
                SchemaOr::Bool(true) => AdditionalProps::Allow,
                schema => AdditionalProps::Typed(Box::new(
                    self.lower_schema_or(schema, &format!("{hint}Additional"))?,
                )),
            },
            None => AdditionalProps::Allow,
        };
        Some(self.insert_schema_type(
            schema,
            hint,
            TypeKind::Struct(Struct { fields, additional }),
        ))
    }

    fn lower_enum(&mut self, values: &[SpannedValue], schema: &Schema, hint: &str) -> Option<Ty> {
        let mut variants = Vec::new();
        let mut repr = None;
        for value in values {
            let scalar = match scalar_value(value) {
                Some(value) => value,
                None => {
                    Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                        .message("enum/const values must be homogeneous scalars")
                        .emit(self.diags);
                    return None;
                }
            };
            let scalar_repr = match scalar {
                ScalarValue::Bool(_) => ScalarRepr::Bool,
                ScalarValue::Int(_) => ScalarRepr::Int,
                ScalarValue::String(_) => ScalarRepr::String,
            };
            if repr
                .replace(scalar_repr)
                .is_some_and(|previous| previous != scalar_repr)
            {
                Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                    .message("enum/const values must all share the same scalar kind")
                    .emit(self.diags);
                return None;
            }
            variants.push(EnumVariant {
                value: scalar,
                docs: Docs::default(),
            });
        }
        Some(self.insert_schema_type(
            schema,
            hint,
            TypeKind::Enum(ScalarEnum {
                repr: repr.unwrap_or(ScalarRepr::String),
                variants,
            }),
        ))
    }

    fn lower_parameter(&mut self, parameter: &ParameterObject) -> Option<Parameter> {
        let location = match parameter.location.as_str() {
            "path" => ParamLoc::Path,
            "query" => ParamLoc::Query,
            "header" => ParamLoc::Header,
            "cookie" => ParamLoc::Cookie,
            _ => {
                Diagnostic::error(Code::InvalidInput, parameter.provenance.clone())
                    .message(format!(
                        "unsupported parameter location `{}`",
                        parameter.location
                    ))
                    .emit(self.diags);
                return None;
            }
        };
        let style_name = parameter.style.as_deref().unwrap_or(match location {
            ParamLoc::Path | ParamLoc::Header => "simple",
            ParamLoc::Query | ParamLoc::Cookie => "form",
        });
        let style = match style_name {
            "simple" => ParamStyle::Simple,
            "form" => ParamStyle::Form,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!("parameter style `{style_name}` is not supported"))
                .emit(self.diags);
                return None;
            }
        };
        let ty = if let Some(schema) = &parameter.schema {
            let schema = self.resolve_schema_ref(schema)?;
            self.lower_schema(&schema, &parameter.name)?
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
            let schema = object
                .schema
                .as_ref()
                .and_then(|schema| self.resolve_schema_ref(schema))?;
            let ty = self.lower_schema(&schema, &parameter.name)?;
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required || location == ParamLoc::Path,
                style: ParamStyle::Content(media),
                explode: parameter.explode.unwrap_or(false),
                deprecated: parameter.deprecated,
            });
        } else {
            self.insert_type(
                &parameter.name,
                TypeKind::Any,
                Docs::default(),
                Some(parameter.provenance.clone()),
            )
        };
        let explode = parameter
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form));
        Some(Parameter {
            name: parameter.name.clone(),
            location,
            ty,
            required: parameter.required || location == ParamLoc::Path,
            style,
            explode,
            deprecated: parameter.deprecated,
        })
    }

    fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        let (media_name, object) = choose_media(&body.content, &body.provenance, self.diags)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
        let ty = object.schema.as_ref().and_then(|schema| {
            let schema = self.resolve_schema_ref(schema)?;
            self.lower_schema(&schema, "RequestBody")
        });
        Some(RequestBody {
            media,
            ty,
            required: body.required,
        })
    }

    fn lower_responses(&mut self, responses: &super::ResponsesObject) -> Responses {
        let mut by_status = Vec::new();
        for (status, response) in &responses.by_status {
            if let Some(status) = parse_status(status) {
                if let Some(response) = self
                    .resolve_response(response)
                    .and_then(|r| self.lower_response(&r))
                {
                    by_status.push((status, response));
                }
            }
        }
        let default = responses
            .default
            .as_ref()
            .and_then(|response| self.resolve_response(response))
            .and_then(|response| self.lower_response(&response));
        Responses { by_status, default }
    }

    fn lower_response(&mut self, response: &ResponseObject) -> Option<Response> {
        let body = choose_media(&response.content, &response.provenance, self.diags).and_then(
            |(media_name, object)| {
                let media = lower_media_type(media_name, &response.provenance, self.diags)?;
                let ty = object.schema.as_ref().and_then(|schema| {
                    let schema = self.resolve_schema_ref(schema)?;
                    self.lower_schema(&schema, "ResponseBody")
                });
                Some((media, ty))
            },
        );
        Some(Response {
            media: body.as_ref().map(|(media, _)| *media),
            body: body.and_then(|(_, ty)| ty),
            headers: response
                .headers
                .iter()
                .filter_map(|(name, header)| {
                    let header = match header {
                        RefOr::Item(header) => header,
                        RefOr::Ref(_) => return None,
                    };
                    let schema = header
                        .schema
                        .as_ref()
                        .and_then(|schema| self.resolve_schema_ref(schema))?;
                    Some(HeaderSpec {
                        name: name.clone(),
                        ty: self.lower_schema(&schema, name)?,
                        required: header.required,
                        docs: Docs {
                            description: header.description.clone(),
                            ..Docs::default()
                        },
                    })
                })
                .collect(),
        })
    }

    fn resolve_parameter(&self, parameter: &RefOr<ParameterObject>) -> Option<ParameterObject> {
        match parameter {
            RefOr::Item(parameter) => Some(parameter.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/parameters/")
                .and_then(|name| self.document.components.parameters.get(name))
                .and_then(|parameter| match parameter {
                    RefOr::Item(parameter) => Some(parameter.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn resolve_request_body(&self, body: &RefOr<RequestBodyObject>) -> Option<RequestBodyObject> {
        match body {
            RefOr::Item(body) => Some(body.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/requestBodies/")
                .and_then(|name| self.document.components.request_bodies.get(name))
                .and_then(|body| match body {
                    RefOr::Item(body) => Some(body.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn resolve_response(&self, response: &RefOr<ResponseObject>) -> Option<ResponseObject> {
        match response {
            RefOr::Item(response) => Some(response.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/responses/")
                .and_then(|name| self.document.components.responses.get(name))
                .and_then(|response| match response {
                    RefOr::Item(response) => Some(response.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn resolve_schema_ref(&self, schema: &RefOr<Schema>) -> Option<Schema> {
        match schema {
            RefOr::Item(schema) => Some(schema.clone()),
            RefOr::Ref(reference) => reference
                .reference
                .strip_prefix("#/components/schemas/")
                .and_then(|name| self.document.components.schemas.get(name))
                .and_then(|schema| match schema {
                    RefOr::Item(schema) => Some(schema.clone()),
                    RefOr::Ref(_) => None,
                }),
        }
    }

    fn insert_schema_type(&mut self, schema: &Schema, hint: &str, kind: TypeKind) -> Ty {
        self.insert_type(
            hint,
            kind,
            Docs {
                title: schema.title.clone(),
                description: schema.description.clone(),
                deprecated: schema.deprecated,
                ..Docs::default()
            },
            Some(schema.provenance.clone()),
        )
    }

    fn insert_type(
        &mut self,
        hint: &str,
        kind: TypeKind,
        docs: Docs,
        provenance: Option<crate::diag::Provenance>,
    ) -> Ty {
        let id = self.graph.insert(TypeDef {
            name_hint: hint.to_owned(),
            kind,
            docs,
            provenance: provenance.unwrap_or_else(|| self.document.provenance.clone()),
        });
        Ty {
            id,
            nullable: false,
            boxed: false,
        }
    }
}

fn lower_security_requirement(requirement: &SecurityRequirement) -> crate::ir::SecurityRequirement {
    crate::ir::SecurityRequirement(
        requirement
            .0
            .iter()
            .map(|(name, scopes)| (SchemeId(name.clone()), scopes.clone()))
            .collect(),
    )
}

fn lower_security_schemes(document: &Document) -> IndexMap<SchemeId, SecurityScheme> {
    let mut schemes = IndexMap::new();
    for (name, scheme) in &document.components.security_schemes {
        let RefOr::Item(scheme) = scheme else {
            continue;
        };
        let lowered = match scheme.scheme_type.as_str() {
            "http" => match scheme.scheme.as_deref() {
                Some("bearer") => SecurityScheme::Http(HttpScheme::Bearer),
                Some("basic") => SecurityScheme::Http(HttpScheme::Basic),
                _ => continue,
            },
            "apiKey" => {
                let location = match scheme.location.as_deref() {
                    Some("header") => ApiKeyLoc::Header,
                    Some("query") => ApiKeyLoc::Query,
                    Some("cookie") => ApiKeyLoc::Cookie,
                    _ => continue,
                };
                SecurityScheme::ApiKey {
                    location,
                    name: scheme.name.clone().unwrap_or_else(|| name.clone()),
                }
            }
            "oauth2" => SecurityScheme::OAuth2(OAuthMeta { flows: Vec::new() }),
            "openIdConnect" => SecurityScheme::OpenIdConnect(OidcMeta {
                openid_connect_url: scheme.openid_connect_url.clone().unwrap_or_default(),
            }),
            _ => continue,
        };
        schemes.insert(SchemeId(name.clone()), lowered);
    }
    schemes
}

fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    Some(match media.split(';').next().unwrap_or(media).trim() {
        "application/json" => MediaType::Json,
        "application/x-www-form-urlencoded" => MediaType::FormUrlEncoded,
        "application/octet-stream" => MediaType::OctetStream,
        "text/plain" => MediaType::TextPlain,
        other => {
            Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
                .message(format!("media type `{other}` is not supported"))
                .emit(diags);
            return None;
        }
    })
}

fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<(&'a str, &'a T)> {
    if content.is_empty() {
        return None;
    }
    for preferred in [
        "application/json",
        "application/x-www-form-urlencoded",
        "application/octet-stream",
        "text/plain",
    ] {
        if let Some(value) = content.get(preferred) {
            return Some((preferred, value));
        }
    }
    let (media, _) = content.first()?;
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
    None
}

fn parse_status(status: &str) -> Option<StatusSpec> {
    if let Some(prefix) = status.strip_suffix("XX") {
        return Some(StatusSpec::Range(prefix.parse().ok()?));
    }
    Some(StatusSpec::Exact(status.parse().ok()?))
}

fn parse_path_template(path: &str) -> PathTemplate {
    let mut segments = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let (literal, after_literal) = rest.split_at(open);
        if !literal.is_empty() {
            segments.push(PathSegment::Literal(literal.to_owned()));
        }
        if let Some(close) = after_literal.find('}') {
            let name = &after_literal[1..close];
            segments.push(PathSegment::Param(name.to_owned()));
            rest = &after_literal[close + 1..];
        } else {
            rest = after_literal;
            break;
        }
    }
    if !rest.is_empty() {
        segments.push(PathSegment::Literal(rest.to_owned()));
    }
    PathTemplate {
        raw: path.to_owned(),
        segments,
    }
}

fn scalar_value(value: &SpannedValue) -> Option<ScalarValue> {
    match &value.node {
        Node::Bool(value) => Some(ScalarValue::Bool(*value)),
        Node::Number(Number::Int(value)) => Some(ScalarValue::Int(*value)),
        Node::Number(Number::UInt(value)) => i64::try_from(*value).ok().map(ScalarValue::Int),
        Node::String(value) => Some(ScalarValue::String(value.clone())),
        _ => None,
    }
}

fn lower_default(value: &SpannedValue) -> Option<ScalarDefault> {
    match &value.node {
        Node::Bool(value) => Some(ScalarDefault::Bool(*value)),
        Node::Number(Number::Int(value)) => Some(ScalarDefault::Int(*value)),
        Node::Number(Number::UInt(value)) => i64::try_from(*value).ok().map(ScalarDefault::Int),
        Node::Number(Number::Float(value)) => Some(ScalarDefault::Float(*value)),
        Node::String(value) => Some(ScalarDefault::String(value.clone())),
        _ => None,
    }
}
