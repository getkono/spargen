use crate::diag::{Code, Diagnostic, Diagnostics};

use super::{AdditionalProps, Api, MediaType, Ty, TypeKind};

/// Check the IR's well-formedness invariants, reporting any violation through `diags`.
///
/// Run unconditionally after every lowering, on every entry point. The first invariant is
/// referential integrity of the type graph: every [`super::Ty`] reachable from the API —
/// operation parameters, request bodies, response bodies, response headers, and, transitively,
/// struct fields, typed additional properties, array items, tuple elements, and union variants —
/// names a `TypeId` that resolves in the [`TypeGraph`](super::TypeGraph). The second is the kind
/// of an octet-stream request body's type: when a request body with [`MediaType::OctetStream`]
/// media has a type whose definition resolves, that definition's kind is [`TypeKind::Bytes`],
/// because the emitter sends such a body only through its raw-bytes path, which sets
/// `Content-Type`. Only the definition's kind is checked, not the reference's `nullable` flag: a
/// nullable byte body passes here yet generates `.body(..)` over an `Option<bytes::Bytes>` that
/// does not compile, a known gap tracked as #104. A failure here is a frontend bug, not a spec
/// problem, so it is reported as [`Code::InvalidInput`] against the construct that carries the
/// violation.
///
/// Semantic checks that are *not* here, because the frontend enforces them where it has the
/// document in hand and the diagnostics to point at it: discriminator property existence
/// (`E007`, in `oas31::lower`), path-parameter/path-template agreement (`E011`, likewise), and
/// unique operation IDs (`E011`).
pub(crate) fn check_invariants(api: &Api, diags: &mut Diagnostics) {
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
                // A missing definition is already reported by `check_ty`; only a definition that
                // exists with the wrong kind is an octet-stream violation.
                if body.media == MediaType::OctetStream
                    && api
                        .types
                        .get(ty.id)
                        .is_some_and(|def| !matches!(def.kind, TypeKind::Bytes))
                {
                    Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                        .message(format!(
                            "IR invariant failed: request body `{}` is an octet-stream body whose \
                             type is not `bytes::Bytes`",
                            body.content_type
                        ))
                        .emit(diags);
                }
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
    use crate::diag::{Code, Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        Api, BodyEncoding, HeaderShape, Info, MediaType, Method, Operation, OperationId,
        PathSegment, PathTemplate, Prim, RequestBody, Response, ResponseHeader, Responses,
        StatusSpec, Ty, TypeDef, TypeGraph, TypeId, TypeKind,
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

    /// The resolvable header API with one request body of `media` / `content_type`. When `kind` is
    /// supplied, the body is typed by a freshly inserted definition of that kind; otherwise it is
    /// untyped (`ty: None`).
    fn api_with_request_body(media: MediaType, content_type: &str, kind: Option<TypeKind>) -> Api {
        let mut api = api_with_header_ty(ty(0));
        let body_ty = kind.map(|kind| {
            let id = api.types.insert(TypeDef {
                name_hint: "RequestBody".to_owned(),
                kind,
                docs: Default::default(),
                provenance: Provenance::new(JsonPointer::root(), None),
            });
            Ty {
                id,
                nullable: false,
                boxed: false,
            }
        });
        api.operations[0].request_body = Some(RequestBody {
            media,
            content_type: content_type.to_owned(),
            ty: body_ty,
            required: true,
            encoding: BodyEncoding::default(),
        });
        api
    }

    #[test]
    fn an_octet_stream_request_body_over_bytes_is_accepted() {
        let api = api_with_request_body(
            MediaType::OctetStream,
            "application/octet-stream",
            Some(TypeKind::Bytes),
        );
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors(), "{diags:#?}");
    }

    #[test]
    fn an_octet_stream_request_body_over_a_non_byte_type_is_caught() {
        // The emitter sends an octet-stream body only through its raw-bytes path; a non-`Bytes`
        // type there would generate code that does not compile, so the frontend must never let
        // one through.
        let api = api_with_request_body(
            MediaType::OctetStream,
            "image/png",
            Some(TypeKind::Primitive(Prim::String)),
        );
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(diags.has_errors(), "{diags:#?}");
        let [diagnostic] = diags.items() else {
            panic!("expected exactly one diagnostic: {diags:#?}");
        };
        assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
        assert!(
            diagnostic.message.contains("`image/png`")
                && diagnostic.message.contains("octet-stream"),
            "{diags:#?}"
        );
    }

    #[test]
    fn an_untyped_octet_stream_request_body_is_left_to_the_frontend() {
        let api = api_with_request_body(MediaType::OctetStream, "application/octet-stream", None);
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors(), "{diags:#?}");
    }

    #[test]
    fn a_dangling_octet_stream_request_body_type_is_reported_once_as_a_missing_type() {
        // A missing definition is the reference check's to report; the octet-stream check must
        // not add a second diagnostic for the same dangling `TypeId`.
        let mut api =
            api_with_request_body(MediaType::OctetStream, "application/octet-stream", None);
        api.operations[0]
            .request_body
            .as_mut()
            .expect("request body installed")
            .ty = Some(ty(999));
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        let [diagnostic] = diags.items() else {
            panic!("expected exactly one diagnostic: {diags:#?}");
        };
        assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
        assert!(
            diagnostic.message.contains("references missing type 999")
                && !diagnostic.message.contains("octet-stream"),
            "{diags:#?}"
        );
    }

    #[test]
    fn a_non_octet_request_body_over_a_string_is_not_an_octet_violation() {
        let api = api_with_request_body(
            MediaType::Text,
            "text/plain",
            Some(TypeKind::Primitive(Prim::String)),
        );
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors(), "{diags:#?}");
    }
}
