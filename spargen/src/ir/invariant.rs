use crate::diag::{Code, Diagnostic, Diagnostics};

use super::{AdditionalProps, Api, BytesEncoding, MediaType, Ty, TypeKind, UnionStrategy};

/// The prefix of every message [`check_invariants`] emits. The fuzz harness
/// (`tests/fuzz_frontend.rs`) declares the same literal to recognise a report that carries one,
/// and a test in this module holds the two equal.
pub(crate) const INVARIANT_FAILURE: &str = "IR invariant failed";

/// Check the IR's well-formedness invariants, reporting any violation through `diags`.
///
/// Run unconditionally after every lowering, on every entry point. It checks six invariants:
///
/// 1. Referential integrity of the type graph: every [`super::Ty`] reachable from the API —
///    operation parameters, request bodies, response bodies, response headers, and, transitively,
///    struct fields, typed additional properties, array items, tuple elements, and union variants
///    — names a `TypeId` that resolves in the [`TypeGraph`](super::TypeGraph) to a definition it
///    does not [elide](super::TypeGraph::elide).
/// 2. The kind of an octet-stream request body's type: when a request body with
///    [`MediaType::OctetStream`] media has a type whose definition resolves, that definition's
///    kind is [`TypeKind::Bytes`] and the reference is not nullable, because the emitter sends
///    such a body only through its raw-bytes path, which sets `Content-Type` and passes the value
///    to `.body(..)`, which accepts `bytes::Bytes` but not `Option<bytes::Bytes>`.
/// 3. No definition that is not elided is still a [`TypeKind::Reserved`].
/// 4. Every variant of a union that is not elided carries a [`super::VariantDispatch`] of its
///    union's [`super::UnionStrategy`] arm ([`super::UnionStrategy::admits`]).
/// 5. No whole non-streaming body — a request body, or a response body that is not a stream — is
///    of a [`BytesEncoding::Base64`] definition: the emitter sends and decodes a whole `bytes` body
///    raw, as `bytes::Bytes`, which the base64 newtype is not.
/// 6. Every request body has a type: the emitter gives an operation a body argument, and sends a
///    body, only for a typed one, so an untyped body would be dropped from the request.
///
/// A failure here is a frontend bug, not a spec problem, so it is reported as
/// [`Code::InvalidInput`] against the construct that carries the violation.
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
            // An untyped body takes no argument, so its method would send nothing for a body the
            // operation declares: lowering types every body it keeps, even a schema-less one.
            if body.ty.is_none() {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message(format!(
                        "{INVARIANT_FAILURE}: request body `{}` has no type, so its operation \
                         would send no body",
                        body.content_type
                    ))
                    .emit(diags);
            }
            if let Some(ty) = body.ty {
                check_ty(api, ty, diags, "request body", operation.provenance.clone());
                check_raw_body(api, ty, diags, "request body", &operation.provenance);
                // A missing definition is already reported by `check_ty`; only a definition that
                // exists with the wrong kind, or is nullable, is an octet-stream violation.
                if body.media == MediaType::OctetStream
                    && api
                        .types
                        .get(ty.id)
                        .is_some_and(|def| ty.nullable || !matches!(def.kind, TypeKind::Bytes(_)))
                {
                    Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                        .message(format!(
                            "{INVARIANT_FAILURE}: request body `{}` is an octet-stream body whose \
                             type is not a non-nullable `bytes::Bytes`",
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

    // An elided definition is no type of the output, so what it refers to is never read.
    for (_, def) in api.types.emitted() {
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
                // The emitter routes an unlisted tag to the `defaultMapping` variant or to the
                // open union's `Unknown`, so a union claiming both has two answers for it.
                if let UnionStrategy::Discriminated {
                    default_variant: Some(_),
                    open: true,
                    ..
                } = union.strategy
                {
                    Diagnostic::error(Code::InvalidInput, def.provenance.clone())
                        .message(format!(
                            "{INVARIANT_FAILURE}: union `{}` is open and has a `defaultMapping` \
                             variant, two destinations for a tag it does not list",
                            def.name_hint
                        ))
                        .emit(diags);
                }
                for variant in &union.variants {
                    check_ty(
                        api,
                        variant.ty,
                        diags,
                        &def.name_hint,
                        def.provenance.clone(),
                    );
                    // The emitter reads each variant's dispatch under the union's strategy, so a
                    // variant of another arm has nothing it could emit.
                    if !union.strategy.admits(&variant.dispatch) {
                        Diagnostic::error(Code::InvalidInput, def.provenance.clone())
                            .message(format!(
                                "{INVARIANT_FAILURE}: variant `{}` of union `{}` carries a \
                                 dispatch of another strategy than its union's",
                                variant.name_hint, def.name_hint
                            ))
                            .emit(diags);
                    }
                }
            }
            // A reservation is not a type: it is an id handed out before its body was lowered, and
            // `fill` replaces it as soon as the body finishes. One surviving into a *successful*
            // lowering means some path reserved an id and never filled it, and everything
            // downstream — emit, surface, the runtime contract — would be reading a shape that was
            // never computed. This is the proof that no consumer of a finished graph sees one.
            TypeKind::Reserved => {
                Diagnostic::error(Code::InvalidInput, def.provenance.clone())
                    .message(format!(
                        "{INVARIANT_FAILURE}: type `{}` is still a reservation, so its body was \
                         never lowered",
                        def.name_hint
                    ))
                    .emit(diags);
            }
            TypeKind::Primitive(_)
            | TypeKind::Enum(_)
            | TypeKind::Bytes(_)
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any => {}
        }
    }
}

/// Walk one response: its body and every documented header. Headers are decoded by generated code
/// just like a body is, so a dangling header type is exactly as fatal.
fn check_response(
    api: &Api,
    response: &super::Response,
    diags: &mut Diagnostics,
    label: &str,
    provenance: &crate::diag::Provenance,
) {
    if let Some(ty) = response.body {
        check_ty(api, ty, diags, &format!("{label} body"), provenance.clone());
        if response.stream.is_none() {
            check_raw_body(api, ty, diags, &format!("{label} body"), provenance);
        }
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

/// Invariant 5: a whole non-streaming body is never of a base64 bytes definition.
fn check_raw_body(
    api: &Api,
    ty: Ty,
    diags: &mut Diagnostics,
    label: &str,
    provenance: &crate::diag::Provenance,
) {
    if api
        .types
        .get(ty.id)
        .is_some_and(|def| matches!(def.kind, TypeKind::Bytes(BytesEncoding::Base64)))
    {
        Diagnostic::error(Code::InvalidInput, provenance.clone())
            .message(format!(
                "{INVARIANT_FAILURE}: `{label}` is a whole body of a base64 bytes type, which the \
                 emitter would send or decode raw"
            ))
            .emit(diags);
    }
}

fn check_ty(
    api: &Api,
    ty: Ty,
    diags: &mut Diagnostics,
    label: &str,
    provenance: crate::diag::Provenance,
) {
    // An elided definition emits no item, so a reference to one names a type the output lacks,
    // exactly as a dangling id does.
    if api.types.get(ty.id).is_none() || api.types.is_elided(ty.id) {
        Diagnostic::error(Code::InvalidInput, provenance)
            .message(format!(
                "{INVARIANT_FAILURE}: `{label}` references missing type {}",
                ty.id.0
            ))
            .emit(diags);
    }
}

#[cfg(test)]
mod tests {
    use super::{check_invariants, INVARIANT_FAILURE};
    use crate::diag::{Code, Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        AdditionalProps, Api, BodyEncoding, DisjointFeature, Field, HeaderShape, Info,
        JsonCategory, MediaType, Method, Operation, OperationId, ParamLoc, ParamStyle, Parameter,
        PathSegment, PathTemplate, Prim, PropertyName, RequestBody, Response, ResponseHeader,
        Responses, StatusSpec, Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind, Union, UnionMode,
        UnionStrategy, UnionVariant, VariantDispatch, XmlField,
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
            document: String::new(),
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
                            accept: None,
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
                bound_headers: Vec::new(),
            }],
            types,
            security_schemes: IndexMap::new(),
        }
    }

    /// The message of the one diagnostic `check_invariants` reports for `api`, after asserting it
    /// is the only one and carries [`Code::InvalidInput`].
    fn only_failure(api: &Api) -> String {
        let mut diags = Diagnostics::new(100);
        check_invariants(api, &mut diags);
        let [diagnostic] = diags.items() else {
            panic!("expected exactly one diagnostic: {diags:#?}");
        };
        assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
        diagnostic.message.clone()
    }

    fn def(name_hint: &str, kind: TypeKind) -> TypeDef {
        TypeDef {
            name_hint: name_hint.to_owned(),
            kind,
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        }
    }

    /// Every position `check_invariants` documents as walked, each holding the one dangling
    /// reference in an otherwise resolvable API, reported once under that position's label.
    #[test]
    fn every_reachable_type_reference_is_checked_for_dangling() {
        let dangling = ty(999);
        let response = |body, header| Response {
            body: Some(body),
            media: Some(MediaType::Json),
            stream: None,
            accept: None,
            headers: vec![ResponseHeader {
                name: "X-Rate".to_owned(),
                ty: header,
                required: false,
                explode: false,
                shape: HeaderShape::Scalar,
                deprecated: false,
                docs: Default::default(),
            }],
        };
        let field = |name: &str, ty| Field {
            name: PropertyName {
                wire: name.to_owned(),
            },
            ty,
            required: true,
            deprecated: false,
            read_only: false,
            write_only: false,
            default: None,
            xml: XmlField::default(),
            undeclared: false,
        };
        type Dangle<'a> = Box<dyn Fn(&mut Api) + 'a>;
        let rows: Vec<(&str, Dangle<'_>)> = vec![
            (
                "petId",
                Box::new(|api| {
                    api.operations[0].params.push(Parameter {
                        name: "petId".to_owned(),
                        location: ParamLoc::Query,
                        ty: dangling,
                        required: true,
                        style: ParamStyle::Form,
                        allow_reserved: false,
                        explode: true,
                        deprecated: false,
                        default_display: None,
                    });
                }),
            ),
            (
                "request body",
                Box::new(|api| {
                    api.operations[0].request_body = Some(RequestBody {
                        media: MediaType::Json,
                        content_type: "application/json".to_owned(),
                        ty: Some(dangling),
                        required: true,
                        encoding: BodyEncoding::default(),
                    });
                }),
            ),
            (
                "response body",
                Box::new(|api| {
                    api.operations[0].responses.by_status[0].1.body = Some(dangling);
                }),
            ),
            (
                "response header `X-Total-Count`",
                Box::new(|api| {
                    api.operations[0].responses.by_status[0].1.headers[0].ty = dangling;
                }),
            ),
            (
                "default response body",
                Box::new(|api| {
                    api.operations[0].responses.default = Some(response(dangling, ty(0)));
                }),
            ),
            (
                "default response header `X-Rate`",
                Box::new(|api| {
                    api.operations[0].responses.default = Some(response(ty(0), dangling));
                }),
            ),
            (
                "name",
                Box::new(|api| {
                    let fields = vec![field("id", ty(0)), field("name", dangling)];
                    api.types.insert(def(
                        "Pet",
                        TypeKind::Struct(Struct {
                            fields,
                            additional: AdditionalProps::Allow,
                        }),
                    ));
                }),
            ),
            (
                "additionalProperties",
                Box::new(|api| {
                    api.types.insert(def(
                        "Labels",
                        TypeKind::Struct(Struct {
                            fields: vec![field("id", ty(0))],
                            additional: AdditionalProps::Typed(Box::new(dangling)),
                        }),
                    ));
                }),
            ),
            (
                "Tags",
                Box::new(|api| {
                    api.types
                        .insert(def("Tags", TypeKind::Array(Box::new(dangling))));
                }),
            ),
            (
                "Point",
                Box::new(|api| {
                    api.types
                        .insert(def("Point", TypeKind::Tuple(vec![ty(0), dangling])));
                }),
            ),
            (
                "Shape",
                Box::new(|api| {
                    let variant = |name: &str, ty| UnionVariant {
                        name_hint: name.to_owned(),
                        ty,
                        dispatch: VariantDispatch::Trial {
                            priority: 0,
                            category: None,
                        },
                    };
                    api.types.insert(def(
                        "Shape",
                        TypeKind::Union(Union {
                            variants: vec![variant("Circle", ty(0)), variant("Square", dangling)],
                            strategy: UnionStrategy::Trial {
                                mode: UnionMode::OneOf,
                            },
                        }),
                    ));
                }),
            ),
        ];
        for (label, dangle) in rows {
            let mut api = api_with_header_ty(ty(0));
            dangle(&mut api);
            assert_eq!(
                only_failure(&api),
                format!("{INVARIANT_FAILURE}: `{label}` references missing type 999"),
                "the reference under `{label}`"
            );
        }
    }

    #[test]
    fn a_resolvable_response_header_type_is_accepted() {
        let api = api_with_header_ty(ty(0));
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors());
    }

    /// The reservation check is the load-bearing half of `TypeKind::Reserved`: it is what lets every
    /// other site say "unreachable" or "unknown" instead of guessing, and what stops a reserved id
    /// reaching codegen, where the emitter's arm is an `unreachable!`.
    ///
    /// No document reaches it, because lowering fills every id it reserves, so only a hand-built
    /// graph exercises it. Were that ordering ever wrong, the failure would be a panic inside a
    /// consumer's `build.rs` rather than a diagnostic.
    #[test]
    fn a_surviving_reservation_is_caught() {
        let mut api = api_with_header_ty(ty(0));
        api.types.insert(TypeDef {
            name_hint: String::new(),
            kind: TypeKind::Reserved,
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        assert_eq!(
            only_failure(&api),
            format!(
                "{INVARIANT_FAILURE}: type `` is still a reservation, so its body was never \
                 lowered"
            ),
            "a reservation that survived lowering must be caught here, because the next stage \
             emits it"
        );
    }

    /// Each strategy admits exactly its own dispatch arm, and a union variant of another arm is
    /// caught with its own message: the emitter reads a variant's dispatch under its union's
    /// strategy, so a mismatch reaching it would be a panic in a consumer's `build.rs`. Lowering
    /// assigns every variant its dispatch together with the strategy, so no document reaches it.
    #[test]
    fn a_union_variant_of_another_strategy_is_caught() {
        let strategies = [
            UnionStrategy::Discriminated {
                tag_field: "kind".to_owned(),
                default_variant: None,
                mode: UnionMode::OneOf,
                open: false,
            },
            UnionStrategy::Disjoint,
            UnionStrategy::Trial {
                mode: UnionMode::AnyOf,
            },
        ];
        let dispatches = [
            VariantDispatch::Discriminated {
                tags: vec!["circle".to_owned()],
                category: None,
                untagged: None,
            },
            VariantDispatch::Disjoint(DisjointFeature::JsonType(JsonCategory::String)),
            VariantDispatch::Trial {
                priority: 0,
                category: None,
            },
        ];
        for (at, strategy) in strategies.iter().enumerate() {
            for (other, dispatch) in dispatches.iter().enumerate() {
                assert_eq!(
                    strategy.admits(dispatch),
                    at == other,
                    "{strategy:?} admitting {dispatch:?}"
                );
            }
            // A dispatch lowering left undecided is admitted by no strategy, so it is caught.
            let deferred = VariantDispatch::Deferred {
                union: 0,
                member: 0,
            };
            assert!(
                !strategy.admits(&deferred),
                "{strategy:?} admitting {deferred:?}"
            );
        }
        let mut api = api_with_header_ty(ty(0));
        api.types.insert(def(
            "Shape",
            TypeKind::Union(Union {
                variants: vec![
                    UnionVariant {
                        name_hint: "Circle".to_owned(),
                        ty: ty(0),
                        dispatch: VariantDispatch::Trial {
                            priority: 0,
                            category: None,
                        },
                    },
                    UnionVariant {
                        name_hint: "Square".to_owned(),
                        ty: ty(0),
                        dispatch: VariantDispatch::Disjoint(DisjointFeature::RequiredKey(
                            "side".to_owned(),
                        )),
                    },
                ],
                strategy: UnionStrategy::Trial {
                    mode: UnionMode::OneOf,
                },
            }),
        ));
        assert_eq!(
            only_failure(&api),
            format!(
                "{INVARIANT_FAILURE}: variant `Square` of union `Shape` carries a dispatch of \
                 another strategy than its union's"
            )
        );
    }

    /// An open union routes a tag it does not list to `Unknown`, and a `defaultMapping` union to
    /// its fallback variant; lowering opens only a union without a fallback, so one claiming both
    /// is caught rather than emitted with one of the two silently winning.
    #[test]
    fn an_open_union_with_a_default_variant_is_caught() {
        let open_with_default = |default_variant| {
            let mut api = api_with_header_ty(ty(0));
            api.types.insert(def(
                "Event",
                TypeKind::Union(Union {
                    variants: vec![UnionVariant {
                        name_hint: "Tick".to_owned(),
                        ty: ty(0),
                        dispatch: VariantDispatch::Discriminated {
                            tags: vec!["tick".to_owned()],
                            category: None,
                            untagged: None,
                        },
                    }],
                    strategy: UnionStrategy::Discriminated {
                        tag_field: "kind".to_owned(),
                        default_variant,
                        mode: UnionMode::OneOf,
                        open: true,
                    },
                }),
            ));
            api
        };
        let mut diags = Diagnostics::new(100);
        check_invariants(&open_with_default(None), &mut diags);
        assert!(
            diags.items().is_empty(),
            "an open union with no fallback is valid: {diags:#?}"
        );
        assert_eq!(
            only_failure(&open_with_default(Some(0))),
            format!(
                "{INVARIANT_FAILURE}: union `Event` is open and has a `defaultMapping` variant, \
                 two destinations for a tag it does not list"
            )
        );
    }

    #[test]
    fn a_dangling_response_header_type_is_caught() {
        // Response headers are decoded by generated code exactly as bodies are, so a header whose
        // `TypeId` does not resolve is just as fatal.
        let api = api_with_header_ty(ty(999));
        assert_eq!(
            only_failure(&api),
            format!(
                "{INVARIANT_FAILURE}: `response header `X-Total-Count`` references missing type \
                 999"
            )
        );
    }

    /// An elided definition emits no item, so a reference to one is as dangling as an id that
    /// resolves to nothing: lowering may elide only what nothing refers to (#428).
    #[test]
    fn a_reference_to_an_elided_type_is_caught() {
        let mut api = api_with_header_ty(ty(0));
        api.types.elide(TypeId(0));
        // The elided id also types the `200` body, so both references are reported.
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        let messages: Vec<&str> = diags
            .items()
            .iter()
            .inspect(|diagnostic| assert_eq!(diagnostic.code, Code::InvalidInput))
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                format!("{INVARIANT_FAILURE}: `response body` references missing type 0"),
                format!(
                    "{INVARIANT_FAILURE}: `response header `X-Total-Count`` references missing \
                     type 0"
                ),
            ],
            "{diags:#?}"
        );
    }

    /// What an elided definition itself refers to is never read, so it is not checked: here a
    /// surviving reservation, which an emitted definition would be reported for.
    #[test]
    fn an_elided_type_is_not_checked() {
        let mut api = api_with_header_ty(ty(0));
        let reserved = api.types.insert(TypeDef {
            name_hint: String::new(),
            kind: TypeKind::Reserved,
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        api.types.elide(reserved);
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        assert!(!diags.has_errors(), "{diags:#?}");
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
                document: String::new(),
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
            Some(TypeKind::Bytes(super::BytesEncoding::Raw)),
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
    fn a_nullable_octet_stream_request_body_is_caught() {
        // A nullable `Bytes` body is emitted as `Option<bytes::Bytes>`, which `.body(..)` does
        // not accept (#104); the frontend refuses the schema, so one here is a frontend bug.
        let mut api = api_with_request_body(
            MediaType::OctetStream,
            "application/octet-stream",
            Some(TypeKind::Bytes(super::BytesEncoding::Raw)),
        );
        api.operations[0]
            .request_body
            .as_mut()
            .and_then(|body| body.ty.as_mut())
            .expect("typed request body installed")
            .nullable = true;
        let mut diags = Diagnostics::new(100);
        check_invariants(&api, &mut diags);
        let [diagnostic] = diags.items() else {
            panic!("expected exactly one diagnostic: {diags:#?}");
        };
        assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
        assert!(
            diagnostic.message.contains("non-nullable `bytes::Bytes`"),
            "{diags:#?}"
        );
    }

    /// Lowering takes a whole base64 body raw (#1230), since the emitter sends and decodes every
    /// whole `bytes` body raw, which the base64 newtype is not; one here is a frontend bug, under
    /// any media.
    #[test]
    fn a_whole_base64_request_body_is_caught() {
        for (media, content_type) in [
            (MediaType::Json, "application/json"),
            (MediaType::OctetStream, "application/octet-stream"),
        ] {
            let api = api_with_request_body(
                media,
                content_type,
                Some(TypeKind::Bytes(super::BytesEncoding::Base64)),
            );
            let mut diags = Diagnostics::new(100);
            check_invariants(&api, &mut diags);
            let [diagnostic] = diags.items() else {
                panic!("{content_type}: expected exactly one diagnostic: {diags:#?}");
            };
            assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
            assert!(
                diagnostic.message.contains("base64 bytes type"),
                "{diags:#?}"
            );
        }
    }

    /// Lowering types every request body it keeps, even a schema-less one (#1339); an untyped
    /// one would take no argument and send nothing, so one here is a frontend bug, under any
    /// media.
    #[test]
    fn an_untyped_request_body_is_caught() {
        for (media, content_type) in [
            (MediaType::Json, "application/json"),
            (MediaType::Text, "text/plain"),
            (MediaType::OctetStream, "application/octet-stream"),
        ] {
            let api = api_with_request_body(media, content_type, None);
            let mut diags = Diagnostics::new(100);
            check_invariants(&api, &mut diags);
            let [diagnostic] = diags.items() else {
                panic!("{content_type}: expected exactly one diagnostic: {diags:#?}");
            };
            assert_eq!(diagnostic.code, Code::InvalidInput, "{diags:#?}");
            assert!(
                diagnostic
                    .message
                    .contains(&format!("request body `{content_type}` has no type")),
                "{diags:#?}"
            );
        }
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

    /// Every invariant failure starts with [`INVARIANT_FAILURE`], and the shared test oracles match
    /// that very literal, and every harness that runs the frontend holds its runs to them, so each
    /// no-invariant-failure oracle sees each one: renaming the prefix here without the oracles, or
    /// a harness that stops calling them, fails this test rather than silently blinding the oracle.
    #[test]
    fn every_harness_matches_the_invariant_failure_prefix() {
        let mut reserved = api_with_header_ty(ty(0));
        reserved.types.insert(TypeDef {
            name_hint: String::new(),
            kind: TypeKind::Reserved,
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        let failing = [
            reserved,
            api_with_header_ty(ty(999)),
            api_with_request_body(
                MediaType::OctetStream,
                "application/octet-stream",
                Some(TypeKind::Primitive(Prim::String)),
            ),
        ];
        for api in &failing {
            let mut diags = Diagnostics::new(100);
            check_invariants(api, &mut diags);
            assert!(!diags.items().is_empty(), "{diags:#?}");
            for diagnostic in diags.items() {
                assert!(
                    diagnostic.message.starts_with(INVARIANT_FAILURE),
                    "{diagnostic:#?}"
                );
            }
        }
        let read = |path: &str| {
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .unwrap_or_else(|error| panic!("{path} is unreadable: {error}"))
        };
        let declared = format!("pub const INVARIANT_FAILURE: &str = \"{INVARIANT_FAILURE}\";");
        assert!(
            read("tests/oracles/mod.rs").contains(&declared),
            "tests/oracles/mod.rs must declare `{declared}`"
        );
        for harness in [
            "tests/frontend/main.rs",
            "tests/fuzz_frontend.rs",
            "tests/lowering_props.rs",
        ] {
            assert!(
                read(harness).contains("oracles::run_violations("),
                "{harness} must hold its runs to `oracles::run_violations`"
            );
        }
    }
}
