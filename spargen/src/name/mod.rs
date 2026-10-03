//! # Subsystem: name
//! layer-deps: ir, diag
//!
//! Deterministic identifier allocation: Rust-conventional casing via Unicode-XID-aware
//! segmentation, keyword escaping, in-scope collision resolution, and `operationId` synthesis.
//! Every allocation is deterministic and injective within its scope, and always yields a
//! valid Rust identifier — property-tested.

mod casing;
mod ident;
mod keyword;
mod scope;
mod synth;

use std::collections::HashMap;

use crate::diag::Diagnostics;
use crate::ir::{AdditionalProps, Api, OperationId, ScalarValue, TypeId, TypeKind};

pub(crate) use casing::{to_pascal_case, to_snake_case};
pub(crate) use ident::Ident;
pub(crate) use keyword::{escape, IdentRole};
pub(crate) use scope::{RankedRequest, Scope};
pub(crate) use synth::synth_operation_id;

/// The identifiers allocated for a whole [`Api`]: one per operation, params struct, type, field,
/// and variant. Codegen looks names up here rather than deriving them, so naming stays in one
/// place and stays deterministic.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// Method name per operation.
    pub(crate) operations: HashMap<OperationId, Ident>,
    /// Optional-parameters `…Params` struct name per operation.
    pub(crate) params_structs: HashMap<OperationId, Ident>,
    /// Generator-owned signature and request-building bindings per operation. Required OpenAPI
    /// parameters are allocated in the same scope first, so these identifiers can never shadow
    /// caller-provided values.
    pub(crate) operation_bindings: HashMap<OperationId, OperationBindings>,
    /// Identifier per operation parameter, in `Operation::params` order: a method argument for a
    /// required parameter, a `…Params` field (and setter) for an optional one. Each kind shares
    /// one scope per operation, so two parameters whose names escape to the same spelling (a path
    /// and a query `id`, a query `page-size` and a header `page_size`) are disambiguated instead
    /// of emitting a duplicate argument or field.
    pub(crate) parameters: HashMap<OperationId, Vec<Ident>>,
    /// Type name per type.
    pub(crate) types: HashMap<TypeId, Ident>,
    /// Field name per `(type, wire property name)`.
    pub(crate) fields: HashMap<(TypeId, String), Ident>,
    /// The synthetic `#[serde(flatten)]` overflow-map field ident per struct that has a typed
    /// `additionalProperties`/`patternProperties` map. Allocated in the struct's field scope
    /// (reserved after the declared fields) so it can never collide with a declared property named
    /// `additional`.
    pub(crate) struct_overflow: HashMap<TypeId, Ident>,
    /// Variant name per `(type, wire variant value)`.
    pub(crate) variants: HashMap<(TypeId, String), Ident>,
    /// The variant holding any unlisted string, per open string enum. Allocated in the enum's
    /// variant scope after every listed value, so a listed value keeps the name it has in the
    /// closed enum, and one spelled `other` pushes this one to a disambiguated name instead.
    pub(crate) open_variants: HashMap<TypeId, Ident>,
    /// Builder type name per declared server, by index.
    pub(crate) servers: Vec<Ident>,
    /// Enum type name per `(server index, variable name)`, for a variable with a closed `enum`.
    pub(crate) server_variable_enums: HashMap<(usize, String), Ident>,
    /// Enum variant name per `(server index, variable name, value)`.
    pub(crate) server_variable_variants: HashMap<(usize, String, String), Ident>,
    /// Setter/field name per `(server index, variable name)`.
    pub(crate) server_variable_fields: HashMap<(usize, String), Ident>,
    /// Header-struct type name per `(operation, status label)`.
    pub(crate) response_header_structs: HashMap<(OperationId, String), Ident>,
    /// Field name per `(operation, status label, header name)`.
    pub(crate) response_header_fields: HashMap<(OperationId, String, String), Ident>,
}

/// The fixed inherent methods codegen emits on `Client` and `BlockingClient`, beside one method
/// per operation. Operation method names are allocated after these are reserved, so an
/// `operationId` that spells one of them (`withCredential`, `new`, …) is disambiguated instead of
/// emitting a duplicate definition. `with_default_server` is emitted only when the specification
/// declares a server and `inner` only on `BlockingClient`; both are reserved unconditionally, so
/// an operation's method name does not change when a server is added. `codegen`'s tests hold this
/// list to exactly the methods it emits.
pub(crate) const CLIENT_METHODS: &[&str] = &[
    "new",
    "with_default_server",
    "with_client",
    "with_backend",
    "core",
    "with_credential",
    "without_credential",
    "inner",
];

/// The type-namespace names codegen's `types` module already uses when a model is emitted into
/// it: the items it brings in with `use` (`serde`'s derives, `BTreeMap`, and the runtime `Date` and
/// `DateTime`) and the prelude types it writes bare. Model type names are allocated after these are
/// reserved, so a schema spelling one of them (a path parameter `date` whose inline schema is
/// hinted `Date`, a component named `String`) is disambiguated instead of redefining an imported
/// name (`E0255`) or shadowing the prelude type every other model refers to.
///
/// `Date` and `DateTime` are imported only when the API uses a date type with the `time` mapping
/// on; they are reserved unconditionally, so a model's name does not change when an unrelated
/// part of the spec starts using dates. `codegen`'s tests hold this list to every such name the
/// emitted module uses.
pub(crate) const TYPES_MODULE_NAMES: &[&str] = &[
    "BTreeMap",
    "Box",
    "Date",
    "DateTime",
    "Deserialize",
    "Option",
    "Result",
    "Serialize",
    "String",
    "Vec",
];

/// Generator-owned bindings emitted inside one operation method.
#[derive(Debug)]
pub(crate) struct OperationBindings {
    /// The optional-parameters struct argument, when one is emitted.
    pub(crate) params: Option<Ident>,
    /// The request-body argument, when one is emitted.
    pub(crate) body: Option<Ident>,
    /// Mutable path assembled before URL construction.
    pub(crate) path: Ident,
    /// Mutable query-pair collection.
    pub(crate) query: Ident,
    /// Serialized whole-query value for an `in: querystring` parameter.
    pub(crate) raw_query: Ident,
    /// Fully constructed request URL.
    pub(crate) url: Ident,
    /// Mutable request builder, then the built request.
    pub(crate) request: Ident,
    /// Clone of a streaming request retained for opt-in SSE reconnects.
    pub(crate) reconnect_request: Ident,
    /// Mutable cookie-fragment collection.
    pub(crate) cookies: Ident,
}

/// Allocate every identifier the API needs, in one deterministic pass. Naming conflicts
/// that cannot be resolved are reported through `diags`.
pub(crate) fn allocate(api: &Api, diags: &mut Diagnostics) -> Names {
    let _ = diags;
    let mut names = Names::default();

    // Servers live in their own module, so they get their own scopes and can never collide with a
    // generated model or operation name.
    let mut server_scope = Scope::default();
    let mut server_enum_scope = Scope::default();
    for (index, server) in api.servers.iter().enumerate() {
        let hint = server
            .name
            .clone()
            .unwrap_or_else(|| format!("server{index}"));
        let pointer = crate::diag::JsonPointer::root();
        names
            .servers
            .push(server_scope.alloc(&hint, IdentRole::Type, &pointer));
        let mut field_scope = Scope::default();
        for (variable_name, variable) in &server.variables {
            names.server_variable_fields.insert(
                (index, variable_name.clone()),
                field_scope.alloc(variable_name, IdentRole::Field, &pointer),
            );
            if variable.enum_values.is_empty() {
                continue;
            }
            names.server_variable_enums.insert(
                (index, variable_name.clone()),
                server_enum_scope.alloc(
                    &format!("{hint} {variable_name}"),
                    IdentRole::Type,
                    &pointer,
                ),
            );
            let mut variant_scope = Scope::default();
            for value in &variable.enum_values {
                names.server_variable_variants.insert(
                    (index, variable_name.clone(), value.clone()),
                    variant_scope.alloc(value, IdentRole::Variant, &pointer),
                );
            }
        }
    }

    // Type names are public API, so which of two same-named definitions keeps the bare name must not
    // depend on the order lowering met them in — that follows `paths` order and `$ref` discovery,
    // and reordering a mapping changes no schema. The contest is decided on each definition's own
    // `(document, pointer)` identity instead. A definition carrying none (the root document's own
    // pointer, which synthesized types fall back to) ranks after every definition that has one, so
    // it can never take a name from a declared schema. The names the `types` module itself uses are
    // taken before any definition asks for one.
    let mut type_scope = Scope::default();
    for name in TYPES_MODULE_NAMES {
        type_scope.reserve(name, IdentRole::Type);
    }
    let definitions: Vec<_> = api.types.iter().collect();
    let requests: Vec<_> = definitions
        .iter()
        .map(|(_, def)| {
            let pointer = &def.provenance.pointer;
            let anonymous = def.document.is_empty() && pointer.as_str().is_empty();
            RankedRequest {
                hint: &def.name_hint,
                provenance: pointer,
                rank: (anonymous, def.document.as_str(), pointer.as_str()),
            }
        })
        .collect();
    let allocated = type_scope.alloc_ranked(&requests, IdentRole::Type);
    names
        .types
        .extend(definitions.iter().map(|(id, _)| *id).zip(allocated));

    // Response-header structs live in the same scope as the other per-operation types, so a
    // documented header can never collide with a generated model.
    for operation in &api.operations {
        let responses = operation
            .responses
            .by_status
            .iter()
            .map(|(spec, response)| (status_label(*spec), response))
            .chain(
                operation
                    .responses
                    .default
                    .as_ref()
                    .map(|response| (status_label(crate::ir::StatusSpec::Default), response)),
            );
        for (label, response) in responses {
            if response.headers.is_empty() {
                continue;
            }
            let hint = format!("{} {label} headers", operation.id.0);
            let pointer = crate::diag::JsonPointer::root();
            names.response_header_structs.insert(
                (operation.id.clone(), label.clone()),
                type_scope.alloc(&hint, IdentRole::Type, &pointer),
            );
            let mut field_scope = Scope::default();
            for header in &response.headers {
                names.response_header_fields.insert(
                    (operation.id.clone(), label.clone(), header.name.clone()),
                    field_scope.alloc(&header.name, IdentRole::Field, &pointer),
                );
            }
        }
    }

    // Operation methods share `impl Client` and `impl BlockingClient` with the fixed methods, so
    // those spellings are taken before any operation asks for one.
    let mut operation_scope = Scope::default();
    for method in CLIENT_METHODS {
        operation_scope.reserve(method, IdentRole::Method);
    }
    let mut params_scope = Scope::default();
    for operation in &api.operations {
        names.operations.insert(
            operation.id.clone(),
            operation_scope.alloc(
                &operation.id.0,
                IdentRole::Method,
                &operation.provenance.pointer,
            ),
        );
        names.params_structs.insert(
            operation.id.clone(),
            params_scope.alloc(
                &format!("{} params", operation.id.0),
                IdentRole::Type,
                &operation.provenance.pointer,
            ),
        );

        // Required parameters are fixed by the generated public surface. Allocate them first, then
        // every generator-owned binding in the same lexical scope so the implementation yields on
        // collision without renaming ordinary arguments. Optional parameters are fields of the
        // `…Params` struct, a scope of their own.
        let pointer = &operation.provenance.pointer;
        let mut binding_scope = Scope::default();
        let mut field_scope = Scope::default();
        names.parameters.insert(
            operation.id.clone(),
            allocate_parameters(operation, &mut binding_scope, &mut field_scope),
        );
        let params = operation
            .params
            .iter()
            .any(|parameter| !parameter.required)
            .then(|| binding_scope.alloc("params", IdentRole::Param, pointer));
        let body = operation
            .request_body
            .as_ref()
            .and_then(|request_body| request_body.ty)
            .map(|_| binding_scope.alloc("body", IdentRole::Param, pointer));
        names.operation_bindings.insert(
            operation.id.clone(),
            OperationBindings {
                params,
                body,
                path: binding_scope.alloc("path", IdentRole::Param, pointer),
                query: binding_scope.alloc("query", IdentRole::Param, pointer),
                raw_query: binding_scope.alloc("raw_query", IdentRole::Param, pointer),
                url: binding_scope.alloc("url", IdentRole::Param, pointer),
                request: binding_scope.alloc("request", IdentRole::Param, pointer),
                reconnect_request: binding_scope.alloc(
                    "reconnect_request",
                    IdentRole::Param,
                    pointer,
                ),
                cookies: binding_scope.alloc("cookies", IdentRole::Param, pointer),
            },
        );
    }

    for (id, def) in api.types.iter() {
        match &def.kind {
            TypeKind::Struct(object) => {
                let mut scope = Scope::default();
                for field in &object.fields {
                    names.fields.insert(
                        (id, field.name.wire.clone()),
                        scope.alloc(&field.name.wire, IdentRole::Field, &def.provenance.pointer),
                    );
                }
                // The flatten overflow field shares the struct's field scope, so it is disambiguated
                // against any declared property (e.g. one named `additional`) instead of emitting a
                // second literal `additional` field that would fail to compile.
                if matches!(object.additional, AdditionalProps::Typed(_)) {
                    names.struct_overflow.insert(
                        id,
                        scope.alloc("additional", IdentRole::Field, &def.provenance.pointer),
                    );
                }
            }
            TypeKind::Enum(enumeration) => {
                let mut scope = Scope::default();
                for variant in &enumeration.variants {
                    let value = match variant {
                        ScalarValue::Bool(value) => value.to_string(),
                        ScalarValue::Int(value) => value.to_string(),
                        ScalarValue::String(value) => value.clone(),
                    };
                    names.variants.insert(
                        (id, value.clone()),
                        scope.alloc(&value, IdentRole::Variant, &def.provenance.pointer),
                    );
                }
                if enumeration.is_open() {
                    names.open_variants.insert(
                        id,
                        scope.alloc("Other", IdentRole::Variant, &def.provenance.pointer),
                    );
                }
            }
            TypeKind::Union(union) => {
                // Union variants share the scalar-enum `variants` table, keyed by `(TypeId, hint)`.
                // A type id is either an enum or a union, so the two never collide; hints are made
                // unique per union at lowering time, keeping this allocation injective in scope.
                let mut scope = Scope::default();
                for variant in &union.variants {
                    names.variants.insert(
                        (id, variant.name_hint.clone()),
                        scope.alloc(
                            &variant.name_hint,
                            IdentRole::Variant,
                            &def.provenance.pointer,
                        ),
                    );
                }
            }
            // Names are allocated only for an `Api` that passed `check_invariants`, which rejects
            // a surviving reservation; allocating nothing for one would leave codegen to find a
            // missing name far from the cause.
            TypeKind::Reserved => unreachable!(
                "a reservation reached name allocation; `check_invariants` should have rejected it"
            ),
            _ => {}
        }
    }

    names
}

/// Allocate one identifier per parameter of `operation`, in `Operation::params` order: required
/// parameters as method arguments in `arguments`, optional ones as `…Params` fields in `fields`.
///
/// Each kind is allocated as one ranked batch, so which of two parameters escaping to the same
/// spelling keeps it depends on the parameters themselves and not on the order the spec lists
/// them in: the earlier location (path, query, querystring, header, cookie) wins, then the
/// lexically smaller wire name. The loser takes a suffix seeded from the operation's pointer, its
/// location and its wire name, so it is as stable as the parameter is.
fn allocate_parameters(
    operation: &crate::ir::Operation,
    arguments: &mut Scope,
    fields: &mut Scope,
) -> Vec<Ident> {
    use crate::ir::ParamLoc;
    let location = |location: ParamLoc| match location {
        ParamLoc::Path => (0u8, "path"),
        ParamLoc::Query => (1, "query"),
        ParamLoc::QueryString => (2, "querystring"),
        ParamLoc::Header => (3, "header"),
        ParamLoc::Cookie => (4, "cookie"),
    };
    let seeds: Vec<crate::diag::JsonPointer> = operation
        .params
        .iter()
        .map(|parameter| {
            operation
                .provenance
                .pointer
                .push("parameters")
                .push(location(parameter.location).1)
                .push(&parameter.name)
        })
        .collect();
    let mut allocated: Vec<Option<Ident>> = vec![None; operation.params.len()];
    for (required, scope, role) in [
        (true, arguments, IdentRole::Param),
        (false, fields, IdentRole::Field),
    ] {
        let indices: Vec<usize> = (0..operation.params.len())
            .filter(|&index| operation.params[index].required == required)
            .collect();
        let requests: Vec<_> = indices
            .iter()
            .map(|&index| {
                let parameter = &operation.params[index];
                RankedRequest {
                    hint: parameter.name.as_str(),
                    provenance: &seeds[index],
                    rank: (
                        location(parameter.location).0,
                        parameter.name.as_str(),
                        index,
                    ),
                }
            })
            .collect();
        for (index, ident) in indices.iter().zip(scope.alloc_ranked(&requests, role)) {
            allocated[*index] = Some(ident);
        }
    }
    allocated
        .into_iter()
        .map(|ident| ident.expect("every parameter is either required or optional"))
        .collect()
}

/// The stable label for one documented status, shared by naming and codegen so a header struct and
/// its response variant always agree.
pub(crate) fn status_label(spec: crate::ir::StatusSpec) -> String {
    match spec {
        crate::ir::StatusSpec::Exact(code) => format!("Status{code}"),
        crate::ir::StatusSpec::Range(prefix) => format!("Status{prefix}xx"),
        crate::ir::StatusSpec::Default => "Default".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{allocate, Ident, Names};
    use crate::diag::{Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        Api, BodyEncoding, Info, MediaType, Method, Operation, OperationId, ParamLoc, ParamStyle,
        Parameter, PathSegment, PathTemplate, Prim, RequestBody, Responses, Ty, TypeDef, TypeGraph,
        TypeKind,
    };
    use indexmap::IndexMap;

    fn parameter(name: &str, location: ParamLoc, required: bool) -> Parameter {
        Parameter {
            name: name.to_owned(),
            location,
            ty: Ty {
                id: crate::ir::TypeId(0),
                nullable: false,
                boxed: false,
            },
            required,
            style: match location {
                ParamLoc::Path | ParamLoc::Header => ParamStyle::Simple,
                ParamLoc::Query | ParamLoc::QueryString | ParamLoc::Cookie => ParamStyle::Form,
            },
            allow_reserved: false,
            explode: false,
            deprecated: false,
            default_display: None,
        }
    }

    /// An API with one `GET /items/{id}` operation carrying `params` and, when `body` is set, a
    /// typed JSON request body, so the `body` binding is allocated too.
    fn api(params: Vec<Parameter>, body: bool) -> Api {
        let mut types = TypeGraph::default();
        let string = types.insert(TypeDef {
            name_hint: "Text".to_owned(),
            kind: TypeKind::Primitive(Prim::String),
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        let ty = Ty {
            id: string,
            nullable: false,
            boxed: false,
        };
        let params = params
            .into_iter()
            .map(|parameter| Parameter { ty, ..parameter })
            .collect();
        Api {
            info: Info {
                title: "T".to_owned(),
                version: "1.0.0".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: vec![Operation {
                id: OperationId("getItem".to_owned()),
                method: Method::Get,
                path: PathTemplate {
                    raw: "/items/{id}".to_owned(),
                    segments: vec![
                        PathSegment::Literal("items".to_owned()),
                        PathSegment::Param("id".to_owned()),
                    ],
                },
                params,
                request_body: body.then(|| RequestBody {
                    media: MediaType::Json,
                    content_type: "application/json".to_owned(),
                    ty: Some(ty),
                    required: true,
                    encoding: BodyEncoding::default(),
                }),
                responses: Responses {
                    by_status: Vec::new(),
                    default: None,
                },
                security: Vec::new(),
                deprecated: false,
                docs: Default::default(),
                server: None,
                provenance: Provenance::new(
                    JsonPointer::root()
                        .push("paths")
                        .push("/items/{id}")
                        .push("get"),
                    None,
                ),
            }],
            types,
            security_schemes: IndexMap::new(),
        }
    }

    fn names(params: Vec<Parameter>, body: bool) -> Names {
        allocate(&api(params, body), &mut Diagnostics::new(100))
    }

    /// The allocated identifier of every parameter, keyed by what identifies a parameter in
    /// OpenAPI (`(in, name)`), so allocations of differently ordered lists compare directly.
    fn by_parameter(params: &[Parameter], names: &Names) -> Vec<((u8, String), Ident)> {
        let idents = &names.parameters[&OperationId("getItem".to_owned())];
        assert_eq!(idents.len(), params.len(), "one identifier per parameter");
        let mut keyed: Vec<_> = params
            .iter()
            .zip(idents)
            .map(|(parameter, ident)| {
                let location = match parameter.location {
                    ParamLoc::Path => 0u8,
                    ParamLoc::Query => 1,
                    ParamLoc::QueryString => 2,
                    ParamLoc::Header => 3,
                    ParamLoc::Cookie => 4,
                };
                ((location, parameter.name.clone()), ident.clone())
            })
            .collect();
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        keyed
    }

    /// Parameters whose names escape to the same spelling in each scope: `id` twice and `ID`
    /// among the method arguments, `page-size`, `page_size` and `Page Size` among the fields.
    fn colliding() -> Vec<Parameter> {
        vec![
            parameter("page_size", ParamLoc::Header, false),
            parameter("id", ParamLoc::Query, true),
            parameter("Page Size", ParamLoc::Cookie, false),
            parameter("ID", ParamLoc::Header, true),
            parameter("id", ParamLoc::Path, true),
            parameter("page-size", ParamLoc::Query, false),
        ]
    }

    fn assert_distinct_per_scope(params: &[Parameter], names: &Names) {
        let idents = &names.parameters[&OperationId("getItem".to_owned())];
        for required in [true, false] {
            let scope: Vec<&str> = params
                .iter()
                .zip(idents)
                .filter(|(parameter, _)| parameter.required == required)
                .map(|(_, ident)| ident.as_str())
                .collect();
            let mut unique = scope.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                scope.len(),
                "required={required} identifiers collide: {scope:?}"
            );
        }
    }

    #[test]
    fn colliding_parameter_identifiers_are_distinct_within_each_scope() {
        let params = colliding();
        let names = names(params.clone(), false);
        assert_distinct_per_scope(&params, &names);

        // The earliest location keeps the bare spelling; the others take a suffix.
        let keyed = by_parameter(&params, &names);
        let ident = |location: u8, name: &str| {
            keyed
                .iter()
                .find(|(key, _)| *key == (location, name.to_owned()))
                .map(|(_, ident)| ident.as_str().to_owned())
                .expect("parameter is allocated")
        };
        assert_eq!(ident(0, "id"), "id");
        assert_ne!(ident(1, "id"), "id");
        assert_ne!(ident(3, "ID"), "id");
        assert_eq!(ident(1, "page-size"), "page_size");
        assert_ne!(ident(3, "page_size"), "page_size");
        assert_ne!(ident(4, "Page Size"), "page_size");
    }

    #[test]
    fn parameter_identifiers_do_not_depend_on_listing_order() {
        let params = colliding();
        let expected = by_parameter(&params, &names(params.clone(), false));
        for rotation in 0..params.len() {
            for reversed in [false, true] {
                let mut reordered = params.clone();
                reordered.rotate_left(rotation);
                if reversed {
                    reordered.reverse();
                }
                let names = names(reordered.clone(), false);
                assert_distinct_per_scope(&reordered, &names);
                assert_eq!(
                    by_parameter(&reordered, &names),
                    expected,
                    "rotation {rotation}, reversed {reversed}"
                );
            }
        }
    }

    #[test]
    fn generator_bindings_yield_to_every_parameter() {
        let bindings = [
            "params",
            "body",
            "path",
            "query",
            "raw_query",
            "url",
            "request",
            "reconnect_request",
            "cookies",
        ];
        let mut params: Vec<Parameter> = bindings
            .iter()
            .map(|name| parameter(name, ParamLoc::Query, true))
            .collect();
        // An optional parameter, so the `params` binding is allocated at all.
        params.push(parameter("limit", ParamLoc::Query, false));
        let names = names(params.clone(), true);
        let operation = OperationId("getItem".to_owned());
        let idents = &names.parameters[&operation];

        // Every required parameter keeps its bare spelling.
        for (parameter, ident) in params.iter().zip(idents) {
            assert_eq!(ident.as_str(), parameter.name);
        }

        let allocated = &names.operation_bindings[&operation];
        let generated = [
            allocated.params.as_ref().expect("an optional parameter"),
            allocated.body.as_ref().expect("a typed request body"),
            &allocated.path,
            &allocated.query,
            &allocated.raw_query,
            &allocated.url,
            &allocated.request,
            &allocated.reconnect_request,
            &allocated.cookies,
        ];
        for (binding, ident) in bindings.iter().zip(generated) {
            assert_ne!(
                ident.as_str(),
                *binding,
                "`{binding}` kept its bare spelling"
            );
            assert!(
                params
                    .iter()
                    .zip(idents)
                    .filter(|(parameter, _)| parameter.required)
                    .all(|(_, parameter)| parameter != ident),
                "binding `{ident}` shadows a parameter"
            );
        }
        let mut unique: Vec<&str> = generated.iter().map(|ident| ident.as_str()).collect();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            generated.len(),
            "bindings collide: {generated:?}"
        );
    }
}
