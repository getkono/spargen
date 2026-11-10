//! # Subsystem: name
//! layer-deps: ir, diag
//!
//! Deterministic identifier allocation: Rust-conventional casing via ASCII segmentation (every
//! non-ASCII-alphanumeric character is a separator), keyword escaping, in-scope collision
//! resolution, and `operationId` synthesis.
//! Every allocation is deterministic and injective within its scope, and always yields a
//! valid Rust identifier — property-tested.

mod casing;
mod ident;
mod keyword;
mod scope;
mod synth;

use std::collections::HashMap;

use crate::ir::{AdditionalProps, Api, OperationId, TypeId, TypeKind};

pub(crate) use casing::hint_segment;
use casing::{to_pascal_case, to_snake_case};
pub(crate) use ident::Ident;
pub(crate) use keyword::{escape, IdentRole};
use scope::{RankedRequest, Scope};
pub(crate) use synth::synth_operation_id;

/// The identifiers allocated for a whole [`Api`]: one record per operation, and one name per type,
/// field, variant and server item. Codegen looks these names up rather than deriving them; the
/// status-selector labels are the exception, derived on demand through [`status_label`], which is
/// itself pure and deterministic.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// The names of each operation, read through [`Names::operation`].
    operations: HashMap<OperationId, OperationNames>,
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
    /// The variant holding any unlisted string, per open string enum, and the one holding any
    /// unlisted discriminator value, per open union. Allocated in the type's variant scope after
    /// every listed variant, so a listed variant keeps the name it has in the closed type, and one
    /// spelled `other` (`unknown` for a union) pushes this one to a disambiguated name instead. A
    /// type id is either an enum or a union, so the two never collide.
    pub(crate) open_variants: HashMap<TypeId, Ident>,
    /// Builder type name per declared server, by index.
    pub(crate) servers: Vec<Ident>,
    /// Enum type name per `(server index, variable name)`, for a variable with a closed `enum`.
    pub(crate) server_variable_enums: HashMap<(usize, String), Ident>,
    /// Enum variant name per `(server index, variable name, value)`.
    pub(crate) server_variable_variants: HashMap<(usize, String, String), Ident>,
    /// Setter/field name per `(server index, variable name)`.
    pub(crate) server_variable_fields: HashMap<(usize, String), Ident>,
}

impl Names {
    /// The names allocated for the operation `id`, which every operation of the allocated [`Api`]
    /// has.
    pub(crate) fn operation(&self, id: &OperationId) -> &OperationNames {
        self.operations
            .get(id)
            .expect("every operation of the allocated API has its names")
    }
}

/// The identifiers allocated for one operation.
///
/// Every name here but the generator-owned bindings and the header struct fields is allocated with
/// [`Scope::alloc_ranked`], so reordering the spec's operations renames none of them.
#[derive(Debug)]
pub(crate) struct OperationNames {
    /// The method on `Client` and `BlockingClient`.
    pub(crate) method: Ident,
    /// The URL-accessor method, `{method}_url`. Allocated in the method scope after every
    /// operation's own method, so an operation whose method already spells `{method}_url` keeps
    /// that name and the accessor takes a pointer-seeded suffix instead.
    pub(crate) url_accessor: Ident,
    /// The optional-parameters `…Params` struct.
    pub(crate) params_struct: Ident,
    /// The error type: `{Method}Error`, or `{Method}OperationError` where the first form would
    /// shadow a root re-export. Allocated as one ranked batch in the root scope, which the
    /// `…Params` structs and the success enums share, so two operations whose methods PascalCase
    /// alike (`get_v2` and `get_v_2`), or whose plain and widened forms meet (`request` and
    /// `requestOperation`), get distinct types.
    pub(crate) error: Ident,
    /// The `{Method}Response` success enum, present exactly when
    /// [`Responses::success`](crate::ir::Responses::success) is an enum, and allocated in the root
    /// scope beside the error types.
    success_enum: Option<Ident>,
    /// Generator-owned signature and request-building bindings. Required OpenAPI parameters are
    /// allocated in the same scope first, so these identifiers can never shadow caller-provided
    /// values.
    pub(crate) bindings: OperationBindings,
    /// Identifier per parameter, in `Operation::params` order: a method argument for a required
    /// parameter, a `…Params` field (and setter) for an optional one. Each kind shares one scope
    /// per operation, so two parameters whose names escape to the same spelling (a path and a
    /// query `id`, a query `page-size` and a header `page_size`) are disambiguated instead of
    /// emitting a duplicate argument or field.
    parameters: Vec<Ident>,
    /// The header struct per status label whose response documents a header.
    response_headers: HashMap<String, ResponseHeaderNames>,
}

impl OperationNames {
    /// The `{Method}Response` success enum of an operation whose
    /// [`Responses::success`](crate::ir::Responses::success) is an enum.
    pub(crate) fn success_enum(&self) -> &Ident {
        self.success_enum
            .as_ref()
            .expect("a success enum is allocated for every enum-shaped success")
    }

    /// Each parameter of `operation`, the operation these names were allocated for, beside its
    /// identifier, in `Operation::params` order.
    pub(crate) fn params<'a>(
        &'a self,
        operation: &'a crate::ir::Operation,
    ) -> impl Iterator<Item = (&'a crate::ir::Parameter, &'a Ident)> {
        debug_assert_eq!(operation.params.len(), self.parameters.len());
        operation.params.iter().zip(&self.parameters)
    }

    /// The header struct of the response under the status `label`, which documents a header.
    pub(crate) fn response_headers(&self, label: &str) -> &ResponseHeaderNames {
        self.response_headers
            .get(label)
            .expect("a header struct is allocated for every response that documents a header")
    }
}

/// The identifiers of one response-header struct: its type, allocated in the `types` module's
/// scope so it can never collide with a generated model, and one field per header.
#[derive(Debug)]
pub(crate) struct ResponseHeaderNames {
    /// The struct's type name.
    pub(crate) ty: Ident,
    /// Field name per header name.
    fields: HashMap<String, Ident>,
}

impl ResponseHeaderNames {
    /// The field of the header `name`, which the response documents.
    pub(crate) fn field(&self, name: &str) -> &Ident {
        self.fields
            .get(name)
            .expect("a field is allocated for every documented header")
    }
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
    "with_config",
    "with_default_server",
    "with_client",
    "with_backend",
    "core",
    "with_credential",
    "without_credential",
    "with_timeout",
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

/// The type-namespace names codegen's `servers` module writes bare beside the server builders and
/// server-variable enums it declares: `String` for a free-form variable's field, setter argument and
/// `url()`, `Into` for that setter's `impl Into<String>`, and `Default` for the derive's
/// `<Enum as Default>` and the hand-written `impl Default`. Server and enum names are allocated
/// after these are reserved, so a server named `string` or `default` is disambiguated instead of
/// declaring a struct that shadows the prelude item the module refers to. `Into` is written only
/// for a free-form variable; like the others it is reserved unconditionally, so a server's name
/// does not change when such a variable is added.
pub(crate) const SERVERS_MODULE_NAMES: &[&str] = &["Default", "Into", "String"];

/// The names every generated module re-exports from the embedded runtime into its root, in the
/// order `codegen` emits them.
///
/// The embedded `support` module is private, so this list and its two conditional siblings are the
/// whole nameable runtime surface of a generated client, and so part of its semver surface. It has
/// to cover every type that appears in a signature the output emits: `HeaderError` is the return
/// type of each `…Headers::from_headers`, `RetryWait` is what a caller's `RetryPolicy` must return,
/// `ClientCore` is what `Client::core` hands back, `HttpBody` is the body of every request and
/// response an `HttpBackend` handles, `MaybeSend` and `MaybeSync` bound the body `HttpBody::wrap`
/// adopts, and the taxonomy's payload types are matched on.
/// Anything short of that leaves a generated signature a caller can call but cannot write down.
/// `ApiErrorBody` is the bound `Error::api_body` needs, implemented by the uniform-body error enum,
/// the single-body newtype, and the uninhabited shape (an enum whose bodies are different generated
/// types gets none). `ApiErrorProblem` is the bound `Error::problem` needs, implemented by every
/// error shape, and `ProblemDetails` is what that reader returns. The list follows signatures, not
/// bodies: a runtime item the output names only inside a function body, such as the `HeaderShape`
/// each `…Headers::from_headers` passes to the header readers, is spelled `support::…` there and
/// stays on the private side.
///
/// `codegen` emits the root `pub use` from these lists and the root scope [`allocate`] draws the
/// operation types from reserves them, so the two read one source.
/// `spargen/tests/reexport_lists.rs` holds each name to the embedded `support` module's own
/// re-exports, and pins the emitted root surface as a golden file.
pub(crate) const ROOT_REEXPORTS: &[&str] = &[
    "ApiErrorBody",
    "ApiErrorProblem",
    "AuthError",
    "ClientConfig",
    "ClientCore",
    "Credential",
    "Error",
    "ExecuteFuture",
    "ExposeSecret",
    "HeaderError",
    "HttpBackend",
    "HttpBody",
    "LinkPaginator",
    "MaybeSend",
    "MaybeSync",
    "Middleware",
    "MiddlewareBackend",
    "Next",
    "ProblemDetails",
    "ProtocolError",
    "RedirectError",
    "RejectionCheck",
    "RequestCause",
    "RequestError",
    "ReqwestBackend",
    "ResponseValue",
    "RetryBackend",
    "RetryOutcome",
    "RetryPolicy",
    "RetryWait",
    "SecretString",
    "TimeoutKind",
    "TokenFuture",
    "TokenProvider",
    "TokenRefresher",
    "TransportError",
    "exponential_backoff",
    "next_link",
    "token_provider",
];

/// The root re-exports emitted only when the API has a sequential (streaming) response.
pub(crate) const STREAM_ROOT_REEXPORTS: &[&str] = &[
    "EventStream",
    "ReconnectPolicy",
    "ReconnectReason",
    "ReconnectWait",
    "StreamError",
];

/// The root re-exports emitted only when a date-typed primitive survives lowering with the `time`
/// mapping on. `DateParseError` is the `FromStr` error of the other two, so `"…".parse::<Date>()`
/// yields a `Result` whose error type a caller can write down.
pub(crate) const DATETIME_ROOT_REEXPORTS: &[&str] = &["Date", "DateParseError", "DateTime"];

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
    /// What a secured operation needs to replay itself once after a refreshable credential is
    /// rejected.
    pub(crate) replay: Ident,
}

/// Allocate every identifier the API needs, in one deterministic pass. Allocation is total: each
/// scope resolves a collision by suffixing, so every lowered API gets a complete set of names.
pub(crate) fn allocate(api: &Api) -> Names {
    let mut names = Names::default();

    // Servers live in their own module, so they get their own scope and can never collide with a
    // generated model or operation name. The builders and the variable enums are declared side by
    // side in that module and share Rust's type namespace, so they are drawn from one scope: two
    // scopes could hand a builder and an enum the same spelling. A spelling is suffixed only where
    // a builder and an enum would collide. The prelude
    // names the module itself writes are taken first, unconditionally, so a server's name does not
    // change when it gains or loses a free-form variable.
    let mut server_scope = Scope::default();
    for name in SERVERS_MODULE_NAMES {
        server_scope.reserve(name, IdentRole::Type);
    }
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
                server_scope.alloc(
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
    // it can never take a name from a declared schema. Next, a component schema
    // (`/components/schemas/{name}`, in any document) or a whole sub-file schema ranks before an
    // inline one, so a component keeps its bare name when an inline type's hint spells it too
    // (`PetId` against `Pet`'s `id` property). The names the `types` module itself uses are
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
                rank: (
                    anonymous,
                    !anonymous && !is_declared_schema(pointer),
                    def.document.as_str(),
                    pointer.as_str(),
                ),
            }
        })
        .collect();
    let allocated = type_scope.alloc_ranked(&requests, IdentRole::Type);
    names
        .types
        .extend(definitions.iter().map(|(id, _)| *id).zip(allocated));

    // Response-header structs are allocated in the model type scope above (the `types` module's),
    // so a documented header's struct can never collide with a generated model. The `…Params`
    // structs, error types and success enums take the module root's scope below.
    let response_headers = allocate_response_headers(api, &mut type_scope);

    // Operation methods share `impl Client` and `impl BlockingClient` with the fixed methods, so
    // those spellings are taken before any operation asks for one.
    let mut operation_scope = Scope::default();
    for method in CLIENT_METHODS {
        operation_scope.reserve(method, IdentRole::Method);
    }
    // The `…Params` structs, the error types, and the success enums are all declared in the
    // generated module's root beside the runtime re-exports, so they are drawn from one scope that
    // holds those re-exports first. The names a type can take there are reserved; the function
    // re-exports live in the value namespace and are not.
    let mut root_scope = Scope::default();
    for name in [
        ROOT_REEXPORTS,
        STREAM_ROOT_REEXPORTS,
        DATETIME_ROOT_REEXPORTS,
    ]
    .concat()
    .into_iter()
    .filter(|name| name.starts_with(|ch: char| ch.is_ascii_uppercase()))
    {
        root_scope.reserve(name, IdentRole::Type);
    }
    // Methods, `…Params` structs and URL accessors are public names, so each family is allocated
    // as one ranked batch: of two operations whose ids escape to one spelling, the lexically
    // smaller `operationId` keeps it whatever order the spec lists them in, and the other takes a
    // suffix seeded from its own pointer.
    let method_hints: Vec<String> = api
        .operations
        .iter()
        .map(|operation| operation.id.0.clone())
        .collect();
    let methods =
        operation_scope.alloc_ranked(&operation_requests(api, &method_hints), IdentRole::Method);
    let params_hints: Vec<String> = api
        .operations
        .iter()
        .map(|operation| format!("{} params", operation.id.0))
        .collect();
    let params_structs =
        root_scope.alloc_ranked(&operation_requests(api, &params_hints), IdentRole::Type);
    let mut signatures = Vec::with_capacity(api.operations.len());
    for ((operation, method), params_struct) in
        api.operations.iter().zip(methods).zip(params_structs)
    {
        // Required parameters are fixed by the generated public surface. Allocate them first, then
        // every generator-owned binding in the same lexical scope so the implementation yields on
        // collision without renaming ordinary arguments. Optional parameters are fields of the
        // `…Params` struct, a scope of their own.
        let pointer = &operation.provenance.pointer;
        let mut binding_scope = Scope::default();
        let mut field_scope = Scope::default();
        let parameters = allocate_parameters(operation, &mut binding_scope, &mut field_scope);
        let params = operation
            .has_optional_params()
            .then(|| binding_scope.alloc("params", IdentRole::Param, pointer));
        let body = operation
            .typed_body()
            .map(|_| binding_scope.alloc("body", IdentRole::Param, pointer));
        let bindings = OperationBindings {
            params,
            body,
            path: binding_scope.alloc("path", IdentRole::Param, pointer),
            query: binding_scope.alloc("query", IdentRole::Param, pointer),
            raw_query: binding_scope.alloc("raw_query", IdentRole::Param, pointer),
            url: binding_scope.alloc("url", IdentRole::Param, pointer),
            request: binding_scope.alloc("request", IdentRole::Param, pointer),
            reconnect_request: binding_scope.alloc("reconnect_request", IdentRole::Param, pointer),
            cookies: binding_scope.alloc("cookies", IdentRole::Param, pointer),
            replay: binding_scope.alloc("replay", IdentRole::Param, pointer),
        };
        signatures.push((method, params_struct, parameters, bindings));
    }
    // After every method, so the accessor yields (see `OperationNames::url_accessor`).
    let accessor_hints: Vec<String> = signatures
        .iter()
        .map(|(method, ..)| format!("{}_url", method.unraw()))
        .collect();
    let url_accessors =
        operation_scope.alloc_ranked(&operation_requests(api, &accessor_hints), IdentRole::Method);
    let methods: Vec<&Ident> = signatures.iter().map(|(method, ..)| method).collect();
    let (errors, success_enums) = allocate_operation_types(api, &methods, &mut root_scope);
    let records = api
        .operations
        .iter()
        .zip(signatures)
        .zip(url_accessors)
        .zip(errors)
        .zip(success_enums)
        .zip(response_headers);
    for (((((operation, signature), url_accessor), error), success_enum), response_headers) in
        records
    {
        let (method, params_struct, parameters, bindings) = signature;
        names.operations.insert(
            operation.id.clone(),
            OperationNames {
                method,
                url_accessor,
                params_struct,
                error,
                success_enum,
                bindings,
                parameters,
                response_headers,
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
                    let value = variant.wire();
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
                if union.is_open() {
                    names.open_variants.insert(
                        id,
                        scope.alloc("Unknown", IdentRole::Variant, &def.provenance.pointer),
                    );
                }
            }
            // Unreachable on a checked `Api`; see `TypeKind::Reserved`. Allocating nothing would
            // leave codegen to find a missing name far from the cause.
            TypeKind::Reserved => unreachable!(
                "a reservation reached name allocation; `check_invariants` should have rejected it"
            ),
            _ => {}
        }
    }

    names
}

/// Whether a type's `pointer` is a declared schema's own: a component (`/components/schemas/{name}`)
/// or a whole document (the empty pointer, which a sub-file schema carries), rather than a
/// position inside one.
fn is_declared_schema(pointer: &crate::diag::JsonPointer) -> bool {
    let tokens = pointer.tokens();
    tokens.is_empty()
        || matches!(tokens.as_slice(), [components, schemas, _] if components == "components" && schemas == "schemas")
}

/// One ranked request per operation of `api`, asking for its entry of `hints` and ranked by its
/// `operationId`, which lowering keeps unique.
fn operation_requests<'a>(api: &'a Api, hints: &'a [String]) -> Vec<RankedRequest<'a, &'a str>> {
    api.operations
        .iter()
        .zip(hints)
        .map(|(operation, hint)| RankedRequest {
            hint,
            provenance: &operation.provenance.pointer,
            rank: operation.id.0.as_str(),
        })
        .collect()
}

/// Allocate each operation's error type and, where its success is an enum, its success enum, in the
/// root `scope`, from `methods`, every operation's allocated method in operation order. Both are
/// returned in operation order.
///
/// Each name is derived from the operation's method, which is unique, but two methods can
/// PascalCase alike (`get_v2` and `get_v_2` both give `GetV2`), and the widened error form of one
/// operation can be the plain form of another (`request` widens to `RequestOperationError`, the
/// plain form of `requestOperation`). Each family is allocated as one ranked batch, so which of two
/// contending operations keeps the bare spelling depends on the operations and not on the order
/// the spec lists them in: a plain error form wins over a widened one, then the lexically smaller
/// method. The loser takes a suffix seeded from its operation's pointer.
///
/// The widening reads the *union* of the root re-exports, the conditional ones included, so an
/// operation's type name never changes because an unrelated part of the spec started or stopped
/// using streams or dates.
fn allocate_operation_types(
    api: &Api,
    methods: &[&Ident],
    scope: &mut Scope,
) -> (Vec<Ident>, Vec<Option<Ident>>) {
    let methods: Vec<(&crate::ir::Operation, &str)> = api
        .operations
        .iter()
        .zip(methods)
        .map(|(operation, method)| (operation, method.unraw()))
        .collect();

    let reexported = |name: &str| {
        [
            ROOT_REEXPORTS,
            STREAM_ROOT_REEXPORTS,
            DATETIME_ROOT_REEXPORTS,
        ]
        .iter()
        .any(|names| names.contains(&name))
    };
    let error_hints: Vec<(String, bool)> = methods
        .iter()
        .map(|(_, method)| {
            let plain = format!("{method} error");
            if reexported(escape(&plain, IdentRole::Type).as_str()) {
                (format!("{method} operation error"), true)
            } else {
                (plain, false)
            }
        })
        .collect();
    let requests: Vec<_> = methods
        .iter()
        .zip(&error_hints)
        .map(|((operation, method), (hint, widened))| RankedRequest {
            hint,
            provenance: &operation.provenance.pointer,
            rank: (*widened, *method),
        })
        .collect();
    let errors = scope.alloc_ranked(&requests, IdentRole::Type);

    let enums: Vec<(usize, &crate::ir::Operation, &str)> = methods
        .iter()
        .enumerate()
        .filter(|(_, (operation, _))| {
            matches!(
                operation.responses.success(),
                crate::ir::SuccessShape::Enum(_)
            )
        })
        .map(|(index, (operation, method))| (index, *operation, *method))
        .collect();
    let hints: Vec<String> = enums
        .iter()
        .map(|(_, _, method)| format!("{method} response"))
        .collect();
    let requests: Vec<_> = enums
        .iter()
        .zip(&hints)
        .map(|((_, operation, method), hint)| RankedRequest {
            hint,
            provenance: &operation.provenance.pointer,
            rank: *method,
        })
        .collect();
    let mut success_enums = vec![None; methods.len()];
    for ((index, _, _), ident) in enums
        .iter()
        .zip(scope.alloc_ranked(&requests, IdentRole::Type))
    {
        success_enums[*index] = Some(ident);
    }
    (errors, success_enums)
}

/// Allocate the header struct of each response of every operation of `api` that documents a
/// header, in `scope`, the `types` module's, and each struct's fields in a scope of its own. The
/// records are returned in operation order, keyed by status label.
///
/// The structs are allocated as one ranked batch, so which of two contending structs keeps the bare
/// spelling depends on the operations and not on the order the spec lists them in: the lexically
/// smaller `operationId` wins, then the smaller status label.
fn allocate_response_headers(
    api: &Api,
    scope: &mut Scope,
) -> Vec<HashMap<String, ResponseHeaderNames>> {
    let pointer = crate::diag::JsonPointer::root();
    let documented: Vec<(usize, &str, String, &crate::ir::Response)> = api
        .operations
        .iter()
        .enumerate()
        .flat_map(|(index, operation)| {
            operation
                .responses
                .entries()
                .filter(|(_, response)| !response.headers.is_empty())
                .map(move |(spec, response)| {
                    (index, operation.id.0.as_str(), status_label(spec), response)
                })
        })
        .collect();
    let hints: Vec<String> = documented
        .iter()
        .map(|(_, id, label, _)| format!("{id} {label} headers"))
        .collect();
    let requests: Vec<_> = documented
        .iter()
        .zip(&hints)
        .map(|((_, id, label, _), hint)| RankedRequest {
            hint,
            provenance: &pointer,
            rank: (*id, label.as_str()),
        })
        .collect();
    let types = scope.alloc_ranked(&requests, IdentRole::Type);
    let mut records: Vec<HashMap<String, ResponseHeaderNames>> =
        api.operations.iter().map(|_| HashMap::new()).collect();
    for ((index, _, label, response), ty) in documented.into_iter().zip(types) {
        let mut field_scope = Scope::default();
        let fields = response
            .headers
            .iter()
            .map(|header| {
                let field = field_scope.alloc(&header.name, IdentRole::Field, &pointer);
                (header.name.clone(), field)
            })
            .collect();
        records[index].insert(label, ResponseHeaderNames { ty, fields });
    }
    records
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
    let rank = |location: ParamLoc| match location {
        ParamLoc::Path => 0u8,
        ParamLoc::Query => 1,
        ParamLoc::QueryString => 2,
        ParamLoc::Header => 3,
        ParamLoc::Cookie => 4,
    };
    let seeds: Vec<crate::diag::JsonPointer> = operation
        .params
        .iter()
        .map(|parameter| {
            operation
                .provenance
                .pointer
                .push("parameters")
                .push(parameter.location.as_openapi_in())
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
                    rank: (rank(parameter.location), parameter.name.as_str(), index),
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
mod tests;
