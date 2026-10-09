//! Server Objects and URL and path templates.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ir::{PathSegment, PathTemplate, Server, UrlSegment};

use super::append_text;

/// Resolve the base-URL override an Operation or Path Item Object declares, rendered with every
/// server variable at its declared default.
///
/// The specification defines no way for a client to *choose* among several `servers` entries in
/// this position, so the first is used and the rest are acknowledged as having no effect (`W011`).
/// Variables are substituted with their declared defaults — what the specification says is sent
/// when nothing selects another value. Unlike the document's `servers`, a per-operation override
/// gets no typed builder: there is no constructor to hand a selection to, since the choice is made
/// per call rather than per client.
pub(super) fn lower_server_override(
    servers: &[crate::oas31::Server],
    diags: &mut Diagnostics,
) -> Option<String> {
    let (first, rest) = servers.split_first()?;
    for extra in rest {
        // W011 case: extra-servers
        Diagnostic::warning(Code::DeclarationHasNoEffect, extra.provenance.clone())
            .message(format!(
                "`servers` entry `{}` past the first has no effect here: the specification \
                 defines no rule for selecting among per-operation servers, so the first is used",
                extra.url
            ))
            .emit(diags);
    }
    lower_server(first, diags).map(|server| render_server_url(&server))
}

/// Render a lowered server URL template with each variable at its declared default.
///
/// `lower_server` has already rejected a template naming an undeclared variable, so a missing
/// entry here can only occur on a document that is already failing.
fn render_server_url(server: &Server) -> String {
    let mut url = String::with_capacity(server.url.len());
    for segment in &server.segments {
        match segment {
            UrlSegment::Literal(text) => url.push_str(text),
            UrlSegment::Variable(name) => {
                if let Some(variable) = server.variables.get(name) {
                    url.push_str(&variable.default);
                }
            }
        }
    }
    url
}

/// Lower one Server Object, parsing its URL template and validating its variables.
///
/// A Server Variable `default` is unlike a Schema Object `default`: the specification says it is
/// actually sent when the caller supplies no alternative, so it changes the wire and must be
/// modeled rather than documented.
pub(super) fn lower_server(
    server: &crate::oas31::Server,
    diags: &mut Diagnostics,
) -> Option<Server> {
    let segments = parse_url_template(&server.url);
    let mut seen: HashSet<&str> = HashSet::new();
    for segment in &segments {
        let UrlSegment::Variable(name) = segment else {
            continue;
        };
        if !seen.insert(name.as_str()) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` appears more than once in `{}`",
                    server.url
                ))
                .emit(diags);
            return None;
        }
        if !server.variables.contains_key(name) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server URL `{}` references undeclared variable `{name}`",
                    server.url
                ))
                .remedy("declare it under the server's `variables`")
                .emit(diags);
            return None;
        }
    }
    for (name, variable) in &server.variables {
        // A default outside its own `enum` would make the no-argument path send an illegal value.
        if !variable.enum_values.is_empty() && !variable.enum_values.contains(&variable.default) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` has default `{}`, which is not one of its declared \
                     `enum` values",
                    variable.default
                ))
                .emit(diags);
            return None;
        }
        if !seen.contains(name.as_str()) {
            // W011 case: unused-server-variable
            Diagnostic::warning(Code::DeclarationHasNoEffect, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` is declared but does not appear in `{}`",
                    server.url
                ))
                .emit(diags);
        }
    }
    let mut docs = server.name.as_ref().map(|name| format!("Server `{name}`."));
    if let Some(description) = &server.description {
        append_text(&mut docs, description.clone());
    }
    Some(Server {
        name: server.name.clone(),
        url: server.url.clone(),
        segments,
        variables: server
            .variables
            .iter()
            .map(|(name, variable)| {
                (
                    name.clone(),
                    crate::ir::ServerVariable {
                        default: variable.default.clone(),
                        enum_values: variable.enum_values.clone(),
                        description: variable.description.clone(),
                    },
                )
            })
            .collect(),
        description: docs,
    })
}

/// Split a server URL template into literals and `{variable}` references.
///
/// An unmatched `{` is kept as literal text: the document schema constrains the template shape, so
/// there is nothing useful to diagnose here that it has not already refused.
fn parse_url_template(url: &str) -> Vec<UrlSegment> {
    let mut segments = Vec::new();
    let mut rest = url;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|at| open + at) else {
            break;
        };
        if open > 0 {
            segments.push(UrlSegment::Literal(rest[..open].to_owned()));
        }
        segments.push(UrlSegment::Variable(rest[open + 1..close].to_owned()));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(UrlSegment::Literal(rest.to_owned()));
    }
    segments
}

pub(super) fn parse_path_template(path: &str) -> PathTemplate {
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
