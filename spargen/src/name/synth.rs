use crate::diag::JsonPointer;
use crate::ir::{Method, PathTemplate};

use super::scope::{RankedRequest, Scope};

/// One operation as [`assign_operation_ids`] reads it.
#[derive(Debug)]
pub(crate) struct OperationIdRequest<'a> {
    /// The `operationId` the spec declares, if it declares one.
    pub(crate) declared: Option<&'a str>,
    /// The method the operation is declared under.
    pub(crate) method: &'a Method,
    /// The path template the operation is declared on.
    pub(crate) path: &'a PathTemplate,
    /// The operation's pointer, which seeds a contested synthesized id's suffix.
    pub(crate) provenance: &'a JsonPointer,
}

/// The `operationId` of every operation in `requests`, in request order.
///
/// A declared id is returned as written; two operations declaring one id is the spec's error, which
/// the caller reports. An operation that declares none gets [`synth_operation_id`]'s spelling,
/// except where that spelling is contested, since synthesis folds distinct templates onto one
/// spelling (`/users` and `/users/`, `/a-b` and `/a_b`, `/Users` and `/users`, `/users/by_id` and
/// `/users/{id}`): a declared id always keeps its spelling, the synthesized contender ranked
/// lowest by its pointer and then its path template keeps a spelling only synthesized ids
/// contest, and every other contender takes a suffix seeded from its own pointer. Which operation
/// is suffixed depends on the operations, never on the order `requests` lists them in.
pub(crate) fn assign_operation_ids(requests: &[OperationIdRequest<'_>]) -> Vec<String> {
    let mut scope = Scope::default();
    for declared in requests.iter().filter_map(|request| request.declared) {
        scope.reserve_exact(declared);
    }
    let synthesized: Vec<(usize, String)> = requests
        .iter()
        .enumerate()
        .filter(|(_, request)| request.declared.is_none())
        .map(|(index, request)| (index, synth_operation_id(request.method, request.path)))
        .collect();
    let ranked: Vec<RankedRequest<'_, (&str, &str)>> = synthesized
        .iter()
        .map(|(index, hint)| RankedRequest {
            hint,
            provenance: requests[*index].provenance,
            rank: (
                requests[*index].provenance.as_str(),
                requests[*index].path.raw.as_str(),
            ),
        })
        .collect();
    let allocated = scope.alloc_ranked_verbatim(&ranked);

    let mut ids: Vec<Option<String>> = requests
        .iter()
        .map(|request| request.declared.map(str::to_owned))
        .collect();
    for ((index, _), id) in synthesized.iter().zip(allocated) {
        ids[*index] = Some(id);
    }
    ids.into_iter()
        .map(|id| id.expect("every operation either declares an id or is allocated one"))
        .collect()
}

/// Synthesize an `operationId` from an operation's method and path template when the spec omits one.
/// Deterministic, e.g. `GET /users/{id}` → `get_users_by_id`. Distinct templates can synthesize one
/// id; [`assign_operation_ids`] tells them apart.
pub(crate) fn synth_operation_id(method: &Method, path: &PathTemplate) -> String {
    let mut parts = vec![method.as_str().to_owned()];
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

/// The name hint an inline request-body schema lowers under: its operation's id followed by
/// `Body` (`storeEscrow` → `StoreEscrowBody`), so the type a caller constructs names the call it
/// belongs to, as `{Method}Error` and `{Method}Response` do. An id with no ASCII alphanumeric
/// segments to nothing and keeps the role's own `RequestBody`. A collision this leaves (another
/// type spelled the same) is resolved by the type scope's pointer-seeded suffix.
pub(crate) fn request_body_hint(operation_id: &str) -> String {
    if operation_id.chars().any(|ch| ch.is_ascii_alphanumeric()) {
        format!("{}Body", crate::name::to_pascal_case(operation_id))
    } else {
        "RequestBody".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use proptest::prelude::*;

    use crate::diag::JsonPointer;
    use crate::ir::{Method, PathSegment, PathTemplate};

    use super::{assign_operation_ids, request_body_hint, synth_operation_id, OperationIdRequest};

    fn path(raw: &str, segments: Vec<PathSegment>) -> PathTemplate {
        PathTemplate {
            raw: raw.to_owned(),
            segments,
        }
    }

    /// One `GET` operation per template in `paths`, declaring the id paired with it, if any, in
    /// the order listed; the ids [`assign_operation_ids`] gives them, by template.
    fn assigned(paths: &[(&str, Option<&str>)]) -> HashMap<String, String> {
        let templates: Vec<PathTemplate> = paths
            .iter()
            .map(|(raw, _)| path(raw, vec![PathSegment::Literal((*raw).to_owned())]))
            .collect();
        let pointers: Vec<JsonPointer> = paths
            .iter()
            .map(|(raw, _)| JsonPointer::root().push("paths").push(raw).push("get"))
            .collect();
        let requests: Vec<OperationIdRequest<'_>> = paths
            .iter()
            .zip(&templates)
            .zip(&pointers)
            .map(
                |(((_, declared), template), provenance)| OperationIdRequest {
                    declared: *declared,
                    method: &Method::Get,
                    path: template,
                    provenance,
                },
            )
            .collect();
        paths
            .iter()
            .map(|(raw, _)| (*raw).to_owned())
            .zip(assign_operation_ids(&requests))
            .collect()
    }

    #[test]
    fn a_contested_synthesized_id_keeps_its_spelling_for_the_lowest_pointer() {
        // `/users` and `/users/` both synthesize `get_users`; `~1users/get` sorts before
        // `~1users~1/get`, so `/users` keeps it, and `/users/` is suffixed from its own pointer.
        let ids = assigned(&[("/users/", None), ("/users", None), ("/accounts", None)]);
        assert_eq!(ids["/users"], "get_users");
        assert_eq!(ids["/accounts"], "get_accounts");
        let suffix = super::super::scope::Scope::default()
            .alloc_ranked_verbatim(&[
                super::RankedRequest {
                    hint: "get_users",
                    provenance: &JsonPointer::from("/x"),
                    rank: 0,
                },
                super::RankedRequest {
                    hint: "get_users",
                    provenance: &JsonPointer::root()
                        .push("paths")
                        .push("/users/")
                        .push("get"),
                    rank: 1,
                },
            ])
            .remove(1);
        assert_eq!(ids["/users/"], suffix);
        assert_ne!(suffix, "get_users");
    }

    #[test]
    fn a_declared_id_keeps_its_spelling_over_a_synthesized_one() {
        for declared_path in ["/accounts", "/zzz"] {
            let ids = assigned(&[("/users", None), (declared_path, Some("get_users"))]);
            assert_eq!(ids[declared_path], "get_users");
            assert!(ids["/users"].starts_with("get_users_"), "{ids:?}");
        }
        // A synthesized id taking a declared spelling is the one rule; two declared ids are left
        // as written for the caller to report.
        let ids = assigned(&[("/a", Some("same")), ("/b", Some("same"))]);
        assert_eq!((ids["/a"].as_str(), ids["/b"].as_str()), ("same", "same"));
    }

    /// A template biased to synthesize alike: casing, separators and a trailing slash all fold.
    fn template() -> impl Strategy<Value = String> {
        "/[uU][-_]?[sS]?/?"
    }

    proptest! {
        /// Every operation that declares no id gets one no other operation has, a declared id is
        /// returned as written, a synthesized one is its synthesis or that suffixed, and permuting
        /// the operations permutes the ids and changes nothing else.
        #[test]
        fn assigned_ids_are_injective_and_independent_of_order(
            operations in proptest::collection::hash_map(
                template(),
                proptest::option::of(proptest::sample::select(vec!["get_u", "get_us", "list"])),
                1..12,
            ),
            rotation in 0usize..12,
        ) {
            let forward: Vec<(&str, Option<&str>)> = operations
                .iter()
                .map(|(raw, declared)| (raw.as_str(), *declared))
                .collect();
            let mut backward = forward.clone();
            backward.rotate_left(rotation % forward.len());
            backward.reverse();

            let ids = assigned(&forward);
            prop_assert_eq!(&assigned(&backward), &ids);

            let declared: HashSet<&str> = forward.iter().filter_map(|(_, id)| *id).collect();
            let mut synthesized = HashSet::new();
            for (raw, id) in &forward {
                match id {
                    Some(id) => prop_assert_eq!(ids[*raw].as_str(), *id),
                    None => {
                        let assigned = ids[*raw].as_str();
                        let template = path(raw, vec![PathSegment::Literal((*raw).to_owned())]);
                        prop_assert!(assigned.starts_with(&synth_operation_id(&Method::Get, &template)));
                        prop_assert!(!declared.contains(assigned), "{assigned} is declared");
                        prop_assert!(synthesized.insert(assigned), "{assigned} is assigned twice");
                    }
                }
            }
        }
    }

    #[test]
    fn the_documented_example_holds() {
        // `GET /users/{id}` → `get_users_by_id`, the example in `synth_operation_id`'s doc.
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

    /// Each literal's non-empty `/`-separated parts in order, each parameter as `by_{name}`, and
    /// the whole snake-cased; an empty part leaks no separator. An exact expected id per row is
    /// what determinism means for a synthesized id, which is spelled into a public method name.
    #[test]
    fn synthesis_follows_the_examples_table() {
        let literal = |text: &str| PathSegment::Literal(text.to_owned());
        let param = |name: &str| PathSegment::Param(name.to_owned());
        for (raw, segments, expected) in [
            ("//a//b/", vec![literal("//a//b/")], "get_a_b"),
            (
                "/users/{id}/posts",
                vec![literal("/users/"), param("id"), literal("/posts")],
                "get_users_by_id_posts",
            ),
            (
                "/items/{itemId}",
                vec![literal("/items/"), param("itemId")],
                "get_items_by_item_id",
            ),
            (
                "/{a}{b}",
                vec![literal("/"), param("a"), param("b")],
                "get_by_a_by_b",
            ),
            (
                "/v1/user-profiles",
                vec![literal("/v1/user-profiles")],
                "get_v1_user_profiles",
            ),
            (
                "/userProfiles",
                vec![literal("/userProfiles")],
                "get_user_profiles",
            ),
        ] {
            let template = path(raw, segments);
            assert_eq!(
                synth_operation_id(&Method::Get, &template),
                expected,
                "{raw}"
            );
        }
    }

    /// The hint is the operation id Pascal-cased with `Body` appended, whatever casing the id is
    /// written in; an id with nothing to segment falls back to the role name.
    #[test]
    fn a_request_body_hint_names_its_operation() {
        for (operation_id, expected) in [
            ("storeEscrow", "StoreEscrowBody"),
            ("append_drop_chunk", "AppendDropChunkBody"),
            ("upload-file", "UploadFileBody"),
            ("Pets.create", "PetsCreateBody"),
            ("post_v1_items", "PostV1ItemsBody"),
            ("2fa", "2faBody"),
            ("-/-", "RequestBody"),
            ("日本", "RequestBody"),
        ] {
            assert_eq!(
                request_body_hint(operation_id),
                expected,
                "{operation_id:?}"
            );
        }
    }

    #[test]
    fn a_root_path_still_yields_a_usable_id() {
        let template = path("/", vec![PathSegment::Literal("/".to_owned())]);
        assert_eq!(synth_operation_id(&Method::Get, &template), "get");
    }
}
