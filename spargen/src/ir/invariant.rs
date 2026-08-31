use crate::diag::{Code, Diagnostic, Diagnostics};

use super::{AdditionalProps, Api, Ty, TypeKind};

/// Check the IR's well-formedness invariants, reporting any violation through `diags`.
///
/// Run unconditionally after every lowering, on every entry point. The invariant is referential
/// integrity of the type graph: every [`super::Ty`] reachable from the API — operation
/// parameters, request bodies, response bodies, response headers, and, transitively, struct
/// fields, typed additional properties, array items, tuple elements, and union variants — names a
/// `TypeId` that resolves in the [`TypeGraph`](super::TypeGraph). A failure here is a frontend
/// bug, not a spec problem, so it is reported as [`Code::InvalidInput`] against the construct that
/// carries the dangling reference.
///
/// Semantic checks that are *not* here, because the frontend enforces them where it has the
/// document in hand and the diagnostics to point at it: discriminator property existence
/// (`E007`, in `oas31::lower`), path-parameter/path-template agreement (`E011`, likewise), and
/// unique operation IDs (`E011`).
pub fn check_invariants(api: &Api, diags: &mut Diagnostics) {
    for operation in &api.operations {
        for parameter in &operation.params {
            check_ty(
                api,
                parameter.ty,
                diags,
                &parameter.name,
                operation.provenance.clone(),
            );
        }
        if let Some(body) = &operation.request_body {
            if let Some(ty) = body.ty {
                check_ty(api, ty, diags, "request body", operation.provenance.clone());
            }
        }
        for (_, response) in &operation.responses.by_status {
            check_response(api, response, diags, "response", &operation.provenance);
        }
        if let Some(response) = &operation.responses.default {
            check_response(
                api,
                response,
                diags,
                "default response",
                &operation.provenance,
            );
        }
    }

    for (_, def) in api.types.iter() {
        match &def.kind {
            TypeKind::Struct(object) => {
                for field in &object.fields {
                    check_ty(
                        api,
                        field.ty,
                        diags,
                        &field.name.wire,
                        def.provenance.clone(),
                    );
                }
                if let AdditionalProps::Typed(ty) = &object.additional {
                    check_ty(
                        api,
                        **ty,
                        diags,
                        "additionalProperties",
                        def.provenance.clone(),
                    );
                }
            }
            TypeKind::Array(ty) => {
                check_ty(api, **ty, diags, &def.name_hint, def.provenance.clone());
            }
            TypeKind::Tuple(items) => {
                for ty in items {
                    check_ty(api, *ty, diags, &def.name_hint, def.provenance.clone());
                }
            }
            TypeKind::Union(union) => {
                for variant in &union.variants {
                    check_ty(
                        api,
                        variant.ty,
                        diags,
                        &def.name_hint,
                        def.provenance.clone(),
                    );
                }
            }
            TypeKind::Primitive(_)
            | TypeKind::Enum(_)
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any => {}
        }
    }
}

/// Walk one response: its body and every documented header. Headers are decoded by generated code
/// just like a body is, so a dangling header type is exactly as fatal — it was simply never walked.
fn check_response(
    api: &Api,
    response: &super::Response,
    diags: &mut Diagnostics,
    label: &str,
    provenance: &crate::diag::Provenance,
) {
    if let Some(ty) = response.body {
        check_ty(api, ty, diags, &format!("{label} body"), provenance.clone());
    }
    for header in &response.headers {
        check_ty(
            api,
            header.ty,
            diags,
            &format!("{label} header `{}`", header.name),
            provenance.clone(),
        );
    }
}

fn check_ty(
    api: &Api,
    ty: Ty,
    diags: &mut Diagnostics,
    label: &str,
    provenance: crate::diag::Provenance,
) {
    if api.types.get(ty.id).is_none() {
        Diagnostic::error(Code::InvalidInput, provenance)
            .message(format!(
                "IR invariant failed: `{label}` references missing type {}",
                ty.id.0
            ))
            .emit(diags);
    }
}

#[cfg(test)]
mod tests {
    use super::check_invariants;
    use crate::diag::{Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        Api, HeaderShape, Info, MediaType, Method, Operation, OperationId, PathSegment,
        PathTemplate, Prim, Response, ResponseHeader, Responses, StatusSpec, Ty, TypeDef,
        TypeGraph, TypeId, TypeKind,
    };
    use indexmap::IndexMap;

    fn ty(id: u32) -> Ty {
        Ty {
            id: TypeId(id),
            nullable: false,
            boxed: false,
        }
    }

    /// An API with one operation whose `200` carries a resolvable body and one documented header
    /// whose type is supplied by the caller, so a test can dangle exactly that reference.
    fn api_with_header_ty(header: Ty) -> Api {
        let mut types = TypeGraph::default();
        let body = types.insert(TypeDef {
            name_hint: "Body".to_owned(),
            kind: TypeKind::Primitive(Prim::String),
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
        });
        Api {
            info: Info {
                title: "T".to_owned(),
                version: "1.0.0".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: vec![Operation {
                id: OperationId("listItems".to_owned()),
                method: Method::Get,
                path: PathTemplate {
                    raw: "/items".to_owned(),
                    segments: vec![PathSegment::Literal("items".to_owned())],
                },
                params: Vec::new(),
                request_body: None,
                responses: Responses {
                    by_status: vec![(
                        StatusSpec::Exact(200),
                        Response {
                            body: Some(Ty {
                                id: body,
                                nullable: false,
                                boxed: false,
                            }),
                            media: Some(MediaType::Json),
                            stream: None,
                            headers: vec![ResponseHeader {
                                name: "X-Total-Count".to_owned(),
                                ty: header,
                                required: true,
                                explode: false,
                                shape: HeaderShape::Scalar,
                                deprecated: false,
                                docs: Default::default(),
                            }],
                        },
                    )],
                    default: None,
                },
                security: Vec::new(),
                deprecated: false,
                docs: Default::default(),
                server: None,
                provenance: Provenance::new(JsonPointer::root(), None),
            }],
            types,
            security_schemes: IndexMap::new(),
        }
    }

    #[test]
    fn a_resolvable_response_header_type_is_accepted() {
        let api = api_with_header_ty(ty(0));
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors());
    }

    #[test]
    fn a_dangling_response_header_type_is_caught() {
        // Response headers are decoded by generated code exactly as bodies are, so a header whose
        // `TypeId` does not resolve is just as fatal — it was simply never walked.
        let api = api_with_header_ty(ty(999));
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(diags.has_errors(), "{diags:#?}");
    }
}
