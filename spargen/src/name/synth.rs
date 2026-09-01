use crate::ir::{Method, PathTemplate};

/// Synthesize an `operationId` from an operation's method and path template when the spec omits one.
/// Deterministic, e.g. `GET /users/{id}` → `get_users_by_id`.
pub(crate) fn synth_operation_id(method: &Method, path: &PathTemplate) -> String {
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
        Method::Custom(method) => method,
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

#[cfg(test)]
mod tests {
    use crate::ir::{Method, PathSegment, PathTemplate};

    use super::synth_operation_id;

    fn path(raw: &str, segments: Vec<PathSegment>) -> PathTemplate {
        PathTemplate {
            raw: raw.to_owned(),
            segments,
        }
    }

    #[test]
    fn the_documented_example_holds() {
        // `GET /users/{id}` → `get_users_by_id`, the example in this module's own doc comment.
        let template = path(
            "/users/{id}",
            vec![
                PathSegment::Literal("/users/".to_owned()),
                PathSegment::Param("id".to_owned()),
            ],
        );
        assert_eq!(
            synth_operation_id(&Method::Get, &template),
            "get_users_by_id"
        );
    }

    #[test]
    fn every_method_contributes_its_own_prefix() {
        let template = path("/things", vec![PathSegment::Literal("/things".to_owned())]);
        for (method, expected) in [
            (Method::Get, "get_things"),
            (Method::Put, "put_things"),
            (Method::Post, "post_things"),
            (Method::Delete, "delete_things"),
            (Method::Options, "options_things"),
            (Method::Head, "head_things"),
            (Method::Patch, "patch_things"),
            (Method::Trace, "trace_things"),
            (Method::Query, "query_things"),
            // OAS 3.2 `additionalOperations` carry a method token of their own.
            (Method::Custom("PURGE".to_owned()), "purge_things"),
        ] {
            assert_eq!(synth_operation_id(&method, &template), expected);
        }
    }

    #[test]
    fn two_operations_that_differ_only_in_a_parameter_name_get_different_ids() {
        // The synthesized id is the collision key when a spec omits `operationId`, so the
        // parameter name has to reach it — otherwise two operations silently share one method.
        let by_id = path(
            "/users/{id}",
            vec![
                PathSegment::Literal("/users/".to_owned()),
                PathSegment::Param("id".to_owned()),
            ],
        );
        let by_slug = path(
            "/users/{slug}",
            vec![
                PathSegment::Literal("/users/".to_owned()),
                PathSegment::Param("slug".to_owned()),
            ],
        );
        assert_ne!(
            synth_operation_id(&Method::Get, &by_id),
            synth_operation_id(&Method::Get, &by_slug)
        );
    }

    #[test]
    fn synthesis_is_deterministic_and_empty_segments_do_not_leak_separators() {
        let template = path("//a//b/", vec![PathSegment::Literal("//a//b/".to_owned())]);
        let first = synth_operation_id(&Method::Get, &template);
        assert_eq!(first, "get_a_b");
        assert_eq!(first, synth_operation_id(&Method::Get, &template));
    }

    #[test]
    fn a_root_path_still_yields_a_usable_id() {
        let template = path("/", vec![PathSegment::Literal("/".to_owned())]);
        assert_eq!(synth_operation_id(&Method::Get, &template), "get");
    }
}
