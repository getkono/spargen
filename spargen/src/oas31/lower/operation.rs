//! Lowering one operation: its merged parameters, request body, responses, security requirements,
//! documentation and server override.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    Docs, Method, Operation, OperationId, ParamLoc, PathSegment, SchemeId, SecuritySchemeDef,
};
use crate::name::synth_operation_id;
use crate::oas31::{OperationObject, ParameterObject, PathItem, ResponseObject};

use super::security::lower_security_requirement;
use super::server::{lower_server_override, parse_path_template};
use super::{append_text, LowerCtx};

/// Lower the `operation` declared under `method` on the path item `item` of the template `path`:
/// its merged parameters, request body, responses, security requirements, documentation and
/// server override. Its id joins `operation_ids`, and a duplicate, a malformed parameter set, a
/// response it cannot decode where it stands, or a security requirement naming no lowered scheme
/// in `security_schemes` is reported; the operation is returned regardless, and the pass's
/// result carries the rejection.
pub(super) fn lower_operation(
    ctx: &mut LowerCtx<'_, '_>,
    path: &str,
    item: &PathItem,
    method: &Method,
    operation: &OperationObject,
    security_schemes: &IndexMap<SchemeId, SecuritySchemeDef>,
    operation_ids: &mut HashSet<String>,
) -> Operation {
    let path_template = parse_path_template(path);
    let id = operation
        .operation_id
        .clone()
        .unwrap_or_else(|| synth_operation_id(method, &path_template));
    if !operation_ids.insert(id.clone()) {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(format!(
                "operationId `{id}` is not unique within the API description"
            ))
            .emit(ctx.diags);
    }

    let mut params = Vec::new();
    let mut merged_parameters: IndexMap<(String, String), ParameterObject> = IndexMap::new();
    for parameter in &item.parameters {
        if let Some(parameter) = ctx.resolve_parameter(parameter) {
            let key = (parameter.location.clone(), parameter.name.clone());
            if merged_parameters.insert(key, parameter).is_some() {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message("path-item parameters contain a duplicate name/location pair")
                    .emit(ctx.diags);
            }
        }
    }
    let mut operation_parameter_keys = HashSet::new();
    for parameter in &operation.parameters {
        if let Some(parameter) = ctx.resolve_parameter(parameter) {
            let key = (parameter.location.clone(), parameter.name.clone());
            if !operation_parameter_keys.insert(key.clone()) {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message("operation parameters contain a duplicate name/location pair")
                    .emit(ctx.diags);
            }
            // Operation-level parameters override the matching path-item parameter.
            merged_parameters.insert(key, parameter);
        }
    }
    for parameter in merged_parameters.values() {
        if let Some(parameter) = ctx.lower_parameter(parameter) {
            params.push(parameter);
        }
    }
    let placeholders: HashSet<&str> = path_template
        .segments
        .iter()
        .filter_map(|segment| match segment {
            PathSegment::Param(name) => Some(name.as_str()),
            PathSegment::Literal(_) => None,
        })
        .collect();
    let path_parameters: HashSet<&str> = params
        .iter()
        .filter(|parameter| parameter.location == ParamLoc::Path)
        .map(|parameter| parameter.name.as_str())
        .collect();
    if placeholders != path_parameters {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(format!(
                "path template parameters {placeholders:?} do not match declared path \
                         parameters {path_parameters:?}"
            ))
            .emit(ctx.diags);
    }
    let querystrings = params
        .iter()
        .filter(|parameter| parameter.location == ParamLoc::QueryString)
        .count();
    let named_queries = params
        .iter()
        .any(|parameter| parameter.location == ParamLoc::Query);
    if querystrings > 1 || (querystrings == 1 && named_queries) {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(
                "an operation may declare at most one `in: querystring` parameter and may \
                         not combine it with `in: query` parameters",
            )
            .emit(ctx.diags);
    }

    let request_body = operation
        .request_body
        .as_ref()
        .and_then(|body| ctx.resolve_request_body(body))
        .and_then(|body| ctx.lower_request_body(&body));

    let responses = ctx.lower_responses(&operation.responses);
    // XML decode is scoped to the single-body success/error paths. An XML body beside a
    // second bodied status on the same side — a response enum that decodes two or more
    // bodies — is rejected cleanly (narrowed `E009`) rather than silently decoded as JSON.
    // A lone XML success body beside a bodyless status is still that one body.
    if responses.xml_in_multi_status() {
        // E009 case: xml-multi-body
        Diagnostic::error(Code::UnsupportedMediaType, operation.provenance.clone())
            .message(
                "an application/xml (or text/xml) response body is only supported as an \
                         operation's single bodied success or single bodied error response \
                         (bodyless statuses beside it are fine); it cannot share a response enum \
                         with a second bodied success or error status",
            )
            .remedy(
                "give the operation a single XML-bodied success/error response, use JSON \
                         for the other bodied responses, or omit this API segment with \
                         spargen::omit!",
            )
            .emit(ctx.diags);
    }
    // Streaming decode is scoped to the single bodied success (`EventStream<T>`). A stream
    // anywhere else — an error status, a `default` (which is always offered to the error
    // side), or a success enum beside a second bodied success — would be decoded as one
    // whole JSON body, so it is rejected (narrowed `E009`) rather than misread on the wire.
    if responses.stream_outside_single_success() {
        // E009 case: streaming-response-position
        Diagnostic::error(Code::UnsupportedMediaType, operation.provenance.clone())
                    .message(
                        "a streaming (text/event-stream, application/x-ndjson, or JSON Text \
                         Sequence) response body is only supported as an operation's single success body; on an error \
                         status, on a `default` response (which also documents error statuses), \
                         or beside a second bodied success status it would be decoded as one \
                         whole body",
                    )
                    .remedy(
                        "declare the stream under an explicit 2xx status as the operation's only \
                         bodied success, document error bodies with a whole-body media type, or \
                         omit this API segment with spargen::omit!",
                    )
                    .emit(ctx.diags);
    }

    let security: Vec<crate::ir::SecurityRequirement> = operation
        .security
        .as_ref()
        .unwrap_or(&ctx.document.security)
        .iter()
        .map(lower_security_requirement)
        .collect();
    // Codegen builds per-operation credential tables from the scheme map, so every
    // referenced scheme must have lowered; an undeclared or unsupported scheme would
    // otherwise silently generate an unauthenticated call.
    for requirement in &security {
        for scheme in &requirement.0 {
            if !security_schemes.contains_key(scheme) {
                Diagnostic::error(Code::UnknownSecurityScheme, operation.provenance.clone())
                    .message(format!(
                        "security requirement references undeclared or unsupported \
                             scheme `{}`",
                        scheme.0
                    ))
                    .remedy(
                        "declare the scheme under components.securitySchemes as http \
                             bearer/basic, apiKey, oauth2, openIdConnect, or mutualTLS",
                    )
                    .emit(ctx.diags);
            }
        }
    }

    let mut operation_description = operation.description.clone();
    // A Path Item's `summary`/`description` apply to every operation on the path. They are
    // additional context rather than a replacement, so the operation's own documentation
    // stays first and these follow it.
    for text in [item.summary.as_ref(), item.description.as_ref()]
        .into_iter()
        .flatten()
    {
        append_text(&mut operation_description, text.clone());
    }
    if !operation.tags.is_empty() {
        append_text(
            &mut operation_description,
            format!("Tags: {}.", operation.tags.join(", ")),
        );
    }
    for (status, response) in &operation.responses.by_status {
        if let Some(response) = ctx.resolve_response(response) {
            append_response_docs(&mut operation_description, status, &response);
        }
    }
    if let Some(response) = operation
        .responses
        .default
        .as_ref()
        .and_then(|response| ctx.resolve_response(response))
    {
        append_response_docs(&mut operation_description, "default", &response);
    }

    // Operation `servers` override the path item's, which override the document's. The
    // document's is the client's base URL, so "no override" is the common case.
    let server = if operation.servers.is_empty() {
        lower_server_override(&item.servers, ctx.diags)
    } else {
        lower_server_override(&operation.servers, ctx.diags)
    };

    Operation {
        id: OperationId(id),
        method: method.clone(),
        path: path_template,
        params,
        request_body,
        responses,
        security,
        deprecated: operation.deprecated,
        docs: Docs {
            title: None,
            summary: operation.summary.clone(),
            description: operation_description,
            deprecated: operation.deprecated,
        },
        server,
        provenance: operation.provenance.clone(),
    }
}

fn append_response_docs(target: &mut Option<String>, status: &str, response: &ResponseObject) {
    let mut docs = response.summary.clone();
    if let Some(description) = &response.description {
        append_text(&mut docs, description.clone());
    }
    if let Some(docs) = docs {
        append_text(target, format!("Response `{status}`: {docs}"));
    }
}
