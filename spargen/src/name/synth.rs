use crate::ir::{Method, PathTemplate};

/// Synthesize an `operationId` from an operation's method and path template when the spec omits one.
/// Deterministic, e.g. `GET /users/{id}` → `get_users_by_id`.
pub fn synth_operation_id(method: Method, path: &PathTemplate) -> String {
    let method = match method {
        Method::Get => "get",
        Method::Put => "put",
        Method::Post => "post",
        Method::Delete => "delete",
        Method::Options => "options",
        Method::Head => "head",
        Method::Patch => "patch",
        Method::Trace => "trace",
        Method::Query => "query",
    };

    let mut parts = vec![method.to_owned()];
    for segment in &path.segments {
        match segment {
            crate::ir::PathSegment::Literal(value) => {
                for part in value.split('/').filter(|part| !part.is_empty()) {
                    parts.push(part.to_owned());
                }
            }
            crate::ir::PathSegment::Param(name) => {
                parts.push("by".to_owned());
                parts.push(name.to_owned());
            }
        }
    }

    crate::name::to_snake_case(&parts.join("_"))
}
