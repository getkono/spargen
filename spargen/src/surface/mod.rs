//! # Subsystem: surface
//! layer-deps: ir, name
//!
//! The **consumer-visible API surface** of generated output, and semver-impact diffing between two
//! surfaces. This is a pure analysis subsystem: it reads a lowered [`crate::ir::Api`] plus its
//! allocated [`crate::name::Names`] and models exactly what a *consumer* of the generated
//! client sees — the client methods (operations) with their typed response-header structs, the
//! public model types with their typed `additionalProperties` maps, and the server builders with
//! their variable enums — then classifies every difference between an old and a new surface as a
//! semver bump. It never emits code and has no effect on generation or the runtime; it backs
//! `spargen diff`.
//!
//! Per the product contract, "the semver surface is the public API of generated output".
//!
//! ## Canonicalisation
//!
//! Two specs assign different (opaque) [`TypeId`](crate::ir::TypeId)s to structurally identical
//! types, so types cannot be compared by id. Every type reference is instead rendered to a *stable
//! structural string* by [`canon_ty`]:
//!
//! * primitives → their canonical scalar label (`String`, `i32`, `i64`, `f64`, `bool`, `Uuid`,
//!   `DateTime`, `Date`) — independent of the `uuid`/`time` feature flags, which only change the
//!   concrete Rust type, not the surface identity;
//! * a **nominal** type — a `struct`, a string `enum`, a union (the models a consumer names,
//!   constructs, and matches), an array or tuple that closes an alias cycle (emitted as a
//!   newtype, #648), or an uninhabited schema (emitted as an empty `pub enum`) → its generated
//!   type name;
//! * an array → `Vec<inner>`, a tuple → `(a, b, …)` (one position → `(a,)`, as Rust spells it),
//!   bytes → `Bytes`, an untyped node → `Value`;
//! * an integer/boolean scalar `enum`/`const` (which generates a `pub type X = i64`/`bool` alias,
//!   carrying no consumer-facing structure beyond its scalar) → that scalar;
//! * a nullable reference wraps the above in `Option<…>`.
//!
//! `boxed` is *not* rendered: boxing is an internal cycle-break artifact, not a surface distinction.
//! Because nominal types render by name (never expanded in place), rendering always terminates, and
//! each nominal type is compared once, as its own surface entry.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    AdditionalProps, Api, ApiErrorBodyImpl, ErrorShape, Prim, ScalarRepr, ScalarValue, StatusSpec,
    SuccessShape, Ty, TypeKind,
};
use crate::name::Names;

/// The full consumer-visible surface of one generated client: its operations (client methods), its
/// public model types, and its server builders, each keyed by a stable identity so two surfaces
/// can be diffed.
pub(crate) struct Surface {
    /// Operations keyed by `"METHOD /path"` — the stable endpoint identity (independent of a
    /// possibly-synthesised `operationId`).
    operations: BTreeMap<String, OpSurface>,
    /// Public model types keyed by their generated type name.
    types: BTreeMap<String, TypeSurface>,
    /// The generated names of the components emitted as a `pub type X = …;` alias. An alias has
    /// no entry in `types` (its target is compared at every use site), but its name is public, so
    /// a type that turns into one or out of one is a kind change rather than an addition or a
    /// removal.
    aliases: BTreeSet<String>,
    /// The server builders of the generated `servers` module, keyed by generated builder name.
    servers: BTreeMap<String, ServerSurface>,
}

/// One server builder's surface: a setter per server variable, keyed by the generated setter name.
/// Its fields are private; a consumer reaches it through `new`, `url`, `Default`, which every
/// builder has, and these setters.
struct ServerSurface {
    variables: BTreeMap<String, ServerVariableSurface>,
}

/// What a server variable's setter takes.
#[derive(PartialEq, Eq)]
enum ServerVariableSurface {
    /// A free-form variable: `impl Into<String>`.
    Free,
    /// A variable with an `enum`: the generated variable enum, by name, and its variant names.
    Enum {
        name: String,
        variants: BTreeSet<String>,
    },
}

impl ServerVariableSurface {
    /// The setter's argument type, as a consumer writes it.
    fn argument(&self) -> String {
        match self {
            ServerVariableSurface::Free => "impl Into<String>".to_owned(),
            ServerVariableSurface::Enum { name, .. } => format!("servers::{name}"),
        }
    }
}

/// One client method's surface: the generated method name, its parameters, request body, and the
/// success/error return types — everything the method's signature exposes.
struct OpSurface {
    /// The generated Rust method identifier.
    method_name: String,
    /// Parameters keyed by `(wire name, location)`: OpenAPI identifies a parameter by the pair, so a
    /// path `id` and a query `id` are two distinct entries rather than one overwriting the other.
    params: BTreeMap<ParamKey, ParamSurface>,
    /// The request body, or `None` for a bodyless operation or a body without a type.
    request_body: Option<BodySurface>,
    /// Canonical rendering of the success return type.
    success: String,
    /// Canonical rendering of the error type.
    error: String,
    /// The canonical `<E as ApiErrorBody>::Body` of the error type, or `None` when it does not
    /// implement `ApiErrorBody`.
    error_body: Option<String>,
    /// The typed response-header structs, keyed by the status they document as the specification
    /// spells it (`200`, `4XX`, `default`). A status documenting no header has no entry.
    response_headers: BTreeMap<String, HeadersSurface>,
}

/// One status's typed response-header struct: its generated name and its public fields, one per
/// documented header, keyed by generated field name. A required header is a `T` field read with
/// `require_header`, an optional one an `Option<T>` field.
struct HeadersSurface {
    /// The generated struct name.
    name: String,
    /// The header fields, keyed by generated field name.
    fields: BTreeMap<String, FieldSurface>,
}

/// A request body's surface: a required body is a `&T` argument in the generated signature, an
/// optional one `Option<&T>`.
struct BodySurface {
    /// The canonical body type.
    ty: String,
    /// Whether the body is `required`.
    required: bool,
}

/// A parameter's identity: its wire name and its location as the OpenAPI `in` value spells it.
type ParamKey = (String, &'static str);

/// A single parameter's surface: its canonical type and whether it is required (required params are
/// positional method arguments; optional ones ride in the `…Params` struct).
struct ParamSurface {
    ty: String,
    required: bool,
}

/// A public model type's surface, by generation kind. Only nominal types with real distinct
/// structure are modelled: structs, string `enum`s, unions, cycle-member newtypes, and uninhabited
/// empty enums.
/// Integer/boolean enums and other alias-only components carry no structure beyond a scalar the
/// consumer already sees at every use site, so they are compared structurally there rather than as
/// standalone entries.
enum TypeSurface {
    /// A `struct`: fields keyed by generated field name, and its typed `additionalProperties` map,
    /// when it has one.
    Struct(BTreeMap<String, FieldSurface>, Option<OverflowSurface>),
    /// A string `enum`: the set of variant wire values, and for an open enum (`open_narrowing`)
    /// the generated name of the variant holding any other string.
    Enum(BTreeSet<String>, Option<String>),
    /// A union enum: variants keyed by generated variant name → canonical payload type.
    Union(BTreeMap<String, String>),
    /// An array or tuple that closes an alias cycle, emitted as `pub struct X(pub T);` (#648): the
    /// canonical rendering of the wrapped `T`, which is the type of the public `.0` field.
    Newtype(String),
    /// An uninhabited schema (`false`, or one whose constraints admit nothing), emitted as an empty
    /// `pub enum X {}`: it has no structure, but it is a public type a use site names.
    Never,
}

impl TypeSurface {
    /// A short label for the generation kind, used to report a kind change (`struct` → `enum`, …).
    fn kind_label(&self) -> &'static str {
        match self {
            TypeSurface::Struct(..) => "struct",
            TypeSurface::Enum(..) => "enum",
            TypeSurface::Union(_) => "union",
            TypeSurface::Newtype(_) => "newtype",
            TypeSurface::Never => "empty enum",
        }
    }
}

/// A struct field's surface: its canonical type and whether it is required. Required-ness matters
/// because the consumer both reads the field (a required field is `T`, optional is `Option<T>`) and
/// constructs the value (a newly-required field breaks every existing constructor).
struct FieldSurface {
    ty: String,
    required: bool,
}

/// A struct's typed `additionalProperties` map, emitted as a public `#[serde(flatten)]` field of
/// type `BTreeMap<String, T>`.
struct OverflowSurface {
    /// The generated field name.
    field: String,
    /// The canonical value type `T`.
    ty: String,
}

/// The semver impact of one change (and of a whole diff, taken as the max over its changes). The
/// ordering `Patch < Minor < Major` is load-bearing: the overall bump is the maximum impact.
///
/// Serializes as the lowercase bump label (`major` / `minor` / `patch`) rather than the Rust
/// variant name, so the JSON form agrees with [`Impact::as_str`] and with the human rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Impact {
    /// No consumer-visible surface change (docs-only / internal).
    Patch,
    /// Purely additive: existing code keeps compiling.
    Minor,
    /// Breaking: existing consumer code may fail to compile or change behaviour.
    Major,
}

impl Impact {
    /// The lowercase semver bump label (`major` / `minor` / `patch`).
    pub fn as_str(self) -> &'static str {
        match self {
            Impact::Major => "major",
            Impact::Minor => "minor",
            Impact::Patch => "patch",
        }
    }
}

/// The kind of a single surface change. Each kind maps to a fixed [`Impact`] and a stable
/// machine-readable code; the mapping is the documented classification policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeKind {
    /// A new operation (client method) appeared. **Minor** — additive.
    OperationAdded,
    /// An operation was removed. **Major** — a method the consumer calls disappears.
    OperationRemoved,
    /// The generated method name for an endpoint changed. **Major** — the callable renames.
    MethodRenamed,
    /// A request body was added to an operation. **Major** — a new `&T` or `Option<&T>` argument.
    RequestBodyAdded,
    /// An operation's request body was removed. **Major** — an argument disappears.
    RequestBodyRemoved,
    /// An operation's request-body type changed. **Major**.
    RequestBodyTypeChanged,
    /// An operation's request body flipped between required and optional. **Major** — `&T` ↔
    /// `Option<&T>`.
    RequestBodyRequirednessChanged,
    /// An operation's success return type changed. **Major**.
    SuccessTypeChanged,
    /// An operation's error type changed. **Major**.
    ErrorTypeChanged,
    /// An operation's error type now implements the runtime's `ApiErrorBody` (a uniform-body enum
    /// also gains `body()`). **Minor** — additive.
    ApiErrorBodyAdded,
    /// An operation's error type no longer implements `ApiErrorBody`. **Major** — `body()`,
    /// `Error::api_body()`, and `E: ApiErrorBody` bounds stop compiling.
    ApiErrorBodyRemoved,
    /// A new required parameter was added. **Major** — a new positional argument.
    RequiredParamAdded,
    /// A new optional parameter was added. **Minor** — a new `…Params` field defaulting to unset.
    OptionalParamAdded,
    /// A parameter was removed. **Major**.
    ParamRemoved,
    /// A parameter's type changed. **Major**.
    ParamTypeChanged,
    /// A parameter's required-ness flipped. **Major** — the method signature changes either way.
    ParamRequirednessChanged,
    /// A new public type appeared. **Minor** — additive.
    TypeAdded,
    /// A public type was removed. **Major**.
    TypeRemoved,
    /// A public type's generation kind changed (`struct` ↔ `enum` ↔ `union` ↔ `newtype`), or a
    /// `pub type` alias of the same name became one of those, or the reverse. **Major**.
    TypeKindChanged,
    /// A new optional field was added to a struct. **Minor**.
    FieldAdded,
    /// A new required field was added to a struct. **Major** — every existing constructor breaks.
    RequiredFieldAdded,
    /// A struct field was removed. **Major**.
    FieldRemoved,
    /// A struct field's type changed, or the type a cycle-member newtype wraps in its public `.0`
    /// field. **Major**.
    FieldTypeChanged,
    /// A struct field's required-ness flipped. **Major** — `T` ↔ `Option<T>`.
    FieldRequirednessChanged,
    /// A new variant was added to an enum/union. **Minor** — the documented additive rule (a value a
    /// consumer may now receive), consistent for string enums and unions alike.
    VariantAdded,
    /// A variant was removed from an enum/union. **Major**.
    VariantRemoved,
    /// A union variant's payload type changed. **Major**.
    VariantTypeChanged,
    /// A documented status gained its typed response-header struct. **Minor** — a new type.
    ResponseHeadersAdded,
    /// A status's typed response-header struct was removed. **Major** — code naming it breaks.
    ResponseHeadersRemoved,
    /// A status's typed response-header struct was renamed. **Major**.
    ResponseHeadersRenamed,
    /// A new optional header was added to a response-header struct. **Minor** — an `Option<T>`
    /// field, the struct-field rule.
    ResponseHeaderAdded,
    /// A new required header was added to a response-header struct. **Major** — reading the
    /// headers now fails on a response without it, and every literal of the struct breaks.
    RequiredResponseHeaderAdded,
    /// A header was removed from a response-header struct. **Major**.
    ResponseHeaderRemoved,
    /// A response header's type changed. **Major** — its field's type changes.
    ResponseHeaderTypeChanged,
    /// A response header's required-ness flipped. **Major** — `T` ↔ `Option<T>`.
    ResponseHeaderRequirednessChanged,
    /// A new server builder appeared in the `servers` module. **Minor** — a new type.
    ServerAdded,
    /// A server builder was removed. **Major** — code naming it breaks.
    ServerRemoved,
    /// A server builder gained a variable setter. **Minor** — the new variable defaults.
    ServerVariableAdded,
    /// A server builder lost a variable setter. **Major** — calls to it break.
    ServerVariableRemoved,
    /// A server variable's setter now takes another type: a free-form variable gained or lost its
    /// `enum`, or its variable enum was renamed. **Major**.
    ServerVariableTypeChanged,
    /// A struct gained a typed `additionalProperties` map. **Minor** — a new public map field, the
    /// optional-field rule.
    AdditionalPropertiesAdded,
    /// A struct lost its typed `additionalProperties` map. **Major** — reads of the field break.
    AdditionalPropertiesRemoved,
    /// The value type of a struct's typed `additionalProperties` map changed. **Major**.
    AdditionalPropertiesTypeChanged,
}

impl ChangeKind {
    /// This change kind's fixed semver impact — the classification policy.
    pub fn impact(self) -> Impact {
        match self {
            ChangeKind::OperationAdded
            | ChangeKind::ApiErrorBodyAdded
            | ChangeKind::OptionalParamAdded
            | ChangeKind::TypeAdded
            | ChangeKind::FieldAdded
            | ChangeKind::VariantAdded
            | ChangeKind::ResponseHeadersAdded
            | ChangeKind::ResponseHeaderAdded
            | ChangeKind::ServerAdded
            | ChangeKind::ServerVariableAdded
            | ChangeKind::AdditionalPropertiesAdded => Impact::Minor,
            ChangeKind::OperationRemoved
            | ChangeKind::MethodRenamed
            | ChangeKind::RequestBodyAdded
            | ChangeKind::RequestBodyRemoved
            | ChangeKind::RequestBodyTypeChanged
            | ChangeKind::RequestBodyRequirednessChanged
            | ChangeKind::SuccessTypeChanged
            | ChangeKind::ErrorTypeChanged
            | ChangeKind::ApiErrorBodyRemoved
            | ChangeKind::RequiredParamAdded
            | ChangeKind::ParamRemoved
            | ChangeKind::ParamTypeChanged
            | ChangeKind::ParamRequirednessChanged
            | ChangeKind::TypeRemoved
            | ChangeKind::TypeKindChanged
            | ChangeKind::RequiredFieldAdded
            | ChangeKind::FieldRemoved
            | ChangeKind::FieldTypeChanged
            | ChangeKind::FieldRequirednessChanged
            | ChangeKind::VariantRemoved
            | ChangeKind::VariantTypeChanged
            | ChangeKind::ResponseHeadersRemoved
            | ChangeKind::ResponseHeadersRenamed
            | ChangeKind::RequiredResponseHeaderAdded
            | ChangeKind::ResponseHeaderRemoved
            | ChangeKind::ResponseHeaderTypeChanged
            | ChangeKind::ResponseHeaderRequirednessChanged
            | ChangeKind::ServerRemoved
            | ChangeKind::ServerVariableRemoved
            | ChangeKind::ServerVariableTypeChanged
            | ChangeKind::AdditionalPropertiesRemoved
            | ChangeKind::AdditionalPropertiesTypeChanged => Impact::Major,
        }
    }

    /// A stable, machine-readable kebab-case code for JSON output and tests.
    pub fn code(self) -> &'static str {
        match self {
            ChangeKind::OperationAdded => "operation-added",
            ChangeKind::OperationRemoved => "operation-removed",
            ChangeKind::MethodRenamed => "method-renamed",
            ChangeKind::RequestBodyAdded => "request-body-added",
            ChangeKind::RequestBodyRemoved => "request-body-removed",
            ChangeKind::RequestBodyTypeChanged => "request-body-type-changed",
            ChangeKind::RequestBodyRequirednessChanged => "request-body-requiredness-changed",
            ChangeKind::SuccessTypeChanged => "success-type-changed",
            ChangeKind::ErrorTypeChanged => "error-type-changed",
            ChangeKind::ApiErrorBodyAdded => "api-error-body-added",
            ChangeKind::ApiErrorBodyRemoved => "api-error-body-removed",
            ChangeKind::RequiredParamAdded => "required-param-added",
            ChangeKind::OptionalParamAdded => "optional-param-added",
            ChangeKind::ParamRemoved => "param-removed",
            ChangeKind::ParamTypeChanged => "param-type-changed",
            ChangeKind::ParamRequirednessChanged => "param-requiredness-changed",
            ChangeKind::TypeAdded => "type-added",
            ChangeKind::TypeRemoved => "type-removed",
            ChangeKind::TypeKindChanged => "type-kind-changed",
            ChangeKind::FieldAdded => "field-added",
            ChangeKind::RequiredFieldAdded => "required-field-added",
            ChangeKind::FieldRemoved => "field-removed",
            ChangeKind::FieldTypeChanged => "field-type-changed",
            ChangeKind::FieldRequirednessChanged => "field-requiredness-changed",
            ChangeKind::VariantAdded => "variant-added",
            ChangeKind::VariantRemoved => "variant-removed",
            ChangeKind::VariantTypeChanged => "variant-type-changed",
            ChangeKind::ResponseHeadersAdded => "response-headers-added",
            ChangeKind::ResponseHeadersRemoved => "response-headers-removed",
            ChangeKind::ResponseHeadersRenamed => "response-headers-renamed",
            ChangeKind::ResponseHeaderAdded => "response-header-added",
            ChangeKind::RequiredResponseHeaderAdded => "required-response-header-added",
            ChangeKind::ResponseHeaderRemoved => "response-header-removed",
            ChangeKind::ResponseHeaderTypeChanged => "response-header-type-changed",
            ChangeKind::ResponseHeaderRequirednessChanged => "response-header-requiredness-changed",
            ChangeKind::ServerAdded => "server-added",
            ChangeKind::ServerRemoved => "server-removed",
            ChangeKind::ServerVariableAdded => "server-variable-added",
            ChangeKind::ServerVariableRemoved => "server-variable-removed",
            ChangeKind::ServerVariableTypeChanged => "server-variable-type-changed",
            ChangeKind::AdditionalPropertiesAdded => "additional-properties-added",
            ChangeKind::AdditionalPropertiesRemoved => "additional-properties-removed",
            ChangeKind::AdditionalPropertiesTypeChanged => "additional-properties-type-changed",
        }
    }
}

impl serde::Serialize for ChangeKind {
    /// Serializes as the stable kebab-case [`code`](ChangeKind::code), not the Rust variant name:
    /// the code is the documented machine surface of `spargen diff`, and the variant name is an
    /// implementation detail. Hand-written rather than `rename_all = "kebab-case"` so `code` stays
    /// the single spelling table for both renderings.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

/// A single classified difference between two surfaces.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Change {
    /// What kind of change this is.
    pub kind: ChangeKind,
    /// The change's semver impact (`kind.impact()`, surfaced for convenience).
    pub impact: Impact,
    /// Where the change is, e.g. `GET /pets/{id}` or `Pet.name`.
    pub location: String,
    /// A one-line human description of the change.
    pub detail: String,
}

impl Change {
    fn new(kind: ChangeKind, location: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            impact: kind.impact(),
            location: location.into(),
            detail: detail.into(),
        }
    }
}

/// The result of diffing two surfaces: every classified change (deterministically ordered) and the
/// overall recommended semver bump (the max impact, or `Patch` when there are no changes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffReport {
    /// Every change, sorted most-severe-first then by location and code (deterministic).
    pub changes: Vec<Change>,
    /// The overall recommended bump: the maximum impact across `changes`, or `Patch` if empty.
    pub bump: Impact,
}

impl std::fmt::Display for Change {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:>5} [{}] {}: {}",
            self.impact.as_str(),
            self.kind.code(),
            self.location,
            self.detail
        )
    }
}

impl std::fmt::Display for DiffReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for change in &self.changes {
            writeln!(formatter, "{change}")?;
        }
        writeln!(formatter, "{}", self.summary())?;
        write!(formatter, "recommended bump: {}", self.bump.as_str())
    }
}

impl DiffReport {
    /// A one-line summary of the diff, e.g. `major: 3 breaking, 1 additive change(s)`.
    pub fn summary(&self) -> String {
        if self.changes.is_empty() {
            return "patch: no public API changes".to_owned();
        }
        let major = self
            .changes
            .iter()
            .filter(|change| change.impact == Impact::Major)
            .count();
        let minor = self
            .changes
            .iter()
            .filter(|change| change.impact == Impact::Minor)
            .count();
        format!(
            "{}: {major} breaking, {minor} additive change(s)",
            self.bump.as_str()
        )
    }
}

/// Build the consumer-visible [`Surface`] of the client generated from `api`, using `names` for the
/// generated identifiers a consumer actually sees.
pub(crate) fn build(api: &Api, names: &Names) -> Surface {
    let mut operations = BTreeMap::new();
    for operation in &api.operations {
        let key = format!("{} {}", method_label(&operation.method), operation.path.raw);
        let method_name = names
            .operations
            .get(&operation.id)
            .expect("operation name allocated")
            .as_str()
            .to_owned();

        let mut params = BTreeMap::new();
        for param in &operation.params {
            params.insert(
                (param.name.clone(), param.location.as_openapi_in()),
                ParamSurface {
                    ty: canon_ty(param.ty, api, names),
                    required: param.required,
                },
            );
        }

        let request_body = operation.request_body.as_ref().and_then(|body| {
            body.ty.map(|ty| BodySurface {
                ty: canon_ty(ty, api, names),
                required: body.required,
            })
        });

        operations.insert(
            key,
            OpSurface {
                method_name,
                params,
                request_body,
                success: success_sig(&operation.responses.success(), api, names),
                error: error_sig(&operation.responses.error(), api, names),
                error_body: error_body_sig(&operation.responses.error(), api, names),
                response_headers: response_headers(operation, api, names),
            },
        );
    }

    let mut types = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    // An elided type has a name but no item, so it is no part of the surface.
    for (id, def) in api.types.emitted() {
        let name = names
            .types
            .get(&id)
            .expect("type name allocated")
            .as_str()
            .to_owned();
        let surface = match &def.kind {
            TypeKind::Struct(object) => {
                let mut fields = BTreeMap::new();
                for field in &object.fields {
                    let field_name = names
                        .fields
                        .get(&(id, field.name.wire.clone()))
                        .expect("field name allocated")
                        .as_str()
                        .to_owned();
                    fields.insert(
                        field_name,
                        FieldSurface {
                            ty: canon_ty(field.ty, api, names),
                            required: field.required,
                        },
                    );
                }
                // `Deny` and `Allow` only change what a decode accepts; neither declares a field.
                let overflow = match &object.additional {
                    AdditionalProps::Typed(ty) => Some(OverflowSurface {
                        field: names
                            .struct_overflow
                            .get(&id)
                            .expect("overflow field name allocated")
                            .as_str()
                            .to_owned(),
                        ty: canon_ty(**ty, api, names),
                    }),
                    AdditionalProps::Allow | AdditionalProps::Deny => None,
                };
                TypeSurface::Struct(fields, overflow)
            }
            // Only string enums generate a real `pub enum` whose variants are surface. Integer /
            // boolean enums lower to a scalar alias and carry no variant surface — skip them here;
            // they render as their scalar at every use site via `canon_ty`.
            TypeKind::Enum(enumeration) if enumeration.repr == ScalarRepr::String => {
                let variants = enumeration
                    .variants
                    .iter()
                    .map(scalar_value_label)
                    .collect();
                let open = names
                    .open_variants
                    .get(&id)
                    .map(|ident| ident.as_str().to_owned());
                TypeSurface::Enum(variants, open)
            }
            TypeKind::Union(union) => {
                let mut variants = BTreeMap::new();
                for variant in &union.variants {
                    let variant_name = names
                        .variants
                        .get(&(id, variant.name_hint.clone()))
                        .expect("union variant name allocated")
                        .as_str()
                        .to_owned();
                    variants.insert(variant_name, canon_ty(variant.ty, api, names));
                }
                TypeSurface::Union(variants)
            }
            // A cycle-member array or tuple is a nominal newtype whose wrapped type is the public
            // `.0` field (#648); every use site renders it by name, so its structure is compared
            // here, one level deep, or a change to what it wraps would go unreported (#654).
            TypeKind::Array(_) | TypeKind::Tuple(_) if api.types.closes_alias_cycle(id) => {
                TypeSurface::Newtype(canon_container(&def.kind, api, names))
            }
            // Unreachable for the reason `canon_ty` states: only a checked `Api` is surfaced.
            // Skipping it like an alias would drop a type from the diff without anything noticing.
            TypeKind::Reserved => unreachable!(
                "a reservation reached the surface; `check_invariants` should have rejected it"
            ),
            // An uninhabited schema is emitted as an empty `pub enum`, not as an alias, and every
            // use site renders it by name, so it is listed here like any other nominal type (#661).
            TypeKind::Never => TypeSurface::Never,
            // Every other kind is emitted as `pub type X = …;`, as codegen's own fallback arm does.
            _ => {
                aliases.insert(name);
                continue;
            }
        };
        types.insert(name, surface);
    }

    Surface {
        operations,
        types,
        aliases,
        servers: servers(api, names),
    }
}

/// The server builders codegen's `emit_servers` declares, named as it names them.
fn servers(api: &Api, names: &Names) -> BTreeMap<String, ServerSurface> {
    let mut builders = BTreeMap::new();
    for (index, server) in api.servers.iter().enumerate() {
        let name = names
            .servers
            .get(index)
            .expect("server name allocated")
            .as_str()
            .to_owned();
        let variables = server
            .variables
            .iter()
            .map(|(variable, declared)| {
                let key = (index, variable.clone());
                let setter = names
                    .server_variable_fields
                    .get(&key)
                    .expect("server variable field allocated")
                    .as_str()
                    .to_owned();
                let surface = match names.server_variable_enums.get(&key) {
                    Some(enum_name) => ServerVariableSurface::Enum {
                        name: enum_name.as_str().to_owned(),
                        variants: declared
                            .enum_values
                            .iter()
                            .map(|value| {
                                names
                                    .server_variable_variants
                                    .get(&(index, variable.clone(), value.clone()))
                                    .expect("server variable variant allocated")
                                    .as_str()
                                    .to_owned()
                            })
                            .collect(),
                    },
                    None => ServerVariableSurface::Free,
                };
                (setter, surface)
            })
            .collect();
        builders.insert(name, ServerSurface { variables });
    }
    builders
}

/// The typed response-header structs codegen's `emit_response_headers` declares for `operation`:
/// one per documented status with at least one header, named and laid out as it names them.
fn response_headers(
    operation: &crate::ir::Operation,
    api: &Api,
    names: &Names,
) -> BTreeMap<String, HeadersSurface> {
    let mut structs = BTreeMap::new();
    for (status, response) in operation.responses.entries() {
        if response.headers.is_empty() {
            continue;
        }
        let label = crate::name::status_label(status);
        let name = names
            .response_header_structs
            .get(&(operation.id.clone(), label.clone()))
            .expect("response header struct name allocated")
            .as_str()
            .to_owned();
        let fields = response
            .headers
            .iter()
            .map(|header| {
                let field = names
                    .response_header_fields
                    .get(&(operation.id.clone(), label.clone(), header.name.clone()))
                    .expect("response header field allocated")
                    .as_str()
                    .to_owned();
                let surface = FieldSurface {
                    ty: canon_ty(header.ty, api, names),
                    required: header.required,
                };
                (field, surface)
            })
            .collect();
        structs.insert(status.display_label(), HeadersSurface { name, fields });
    }
    structs
}

/// Diff two surfaces into a classified, deterministically ordered [`DiffReport`].
pub(crate) fn diff(old: &Surface, new: &Surface) -> DiffReport {
    let mut changes = Vec::new();
    diff_operations(old, new, &mut changes);
    diff_types(old, new, &mut changes);
    diff_servers(old, new, &mut changes);

    // Deterministic order: most-severe first, then by location, then by machine code, then detail.
    changes.sort_by(|a, b| {
        (b.impact, &a.location, a.kind.code(), &a.detail).cmp(&(
            a.impact,
            &b.location,
            b.kind.code(),
            &b.detail,
        ))
    });

    let bump = changes
        .iter()
        .map(|change| change.impact)
        .max()
        .unwrap_or(Impact::Patch);
    DiffReport { changes, bump }
}

fn diff_operations(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    for key in keys(&old.operations, &new.operations) {
        match (old.operations.get(key), new.operations.get(key)) {
            (None, Some(_)) => {
                changes.push(Change::new(
                    ChangeKind::OperationAdded,
                    key,
                    "new operation",
                ));
            }
            (Some(_), None) => {
                changes.push(Change::new(
                    ChangeKind::OperationRemoved,
                    key,
                    "operation removed",
                ));
            }
            (Some(old_op), Some(new_op)) => diff_operation(key, old_op, new_op, changes),
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

fn diff_operation(key: &str, old: &OpSurface, new: &OpSurface, changes: &mut Vec<Change>) {
    if old.method_name != new.method_name {
        changes.push(Change::new(
            ChangeKind::MethodRenamed,
            key,
            format!(
                "method renamed `{}` -> `{}`",
                old.method_name, new.method_name
            ),
        ));
    }
    match (&old.request_body, &new.request_body) {
        (None, Some(body)) => changes.push(Change::new(
            ChangeKind::RequestBodyAdded,
            key,
            format!("request body added: `{}`", body.ty),
        )),
        (Some(body), None) => changes.push(Change::new(
            ChangeKind::RequestBodyRemoved,
            key,
            format!("request body removed (was `{}`)", body.ty),
        )),
        (Some(old_body), Some(new_body)) => {
            if old_body.ty != new_body.ty {
                changes.push(Change::new(
                    ChangeKind::RequestBodyTypeChanged,
                    key,
                    format!("request body type `{}` -> `{}`", old_body.ty, new_body.ty),
                ));
            }
            if old_body.required != new_body.required {
                changes.push(Change::new(
                    ChangeKind::RequestBodyRequirednessChanged,
                    key,
                    format!(
                        "request body now {}",
                        if new_body.required {
                            "required"
                        } else {
                            "optional"
                        }
                    ),
                ));
            }
        }
        (None, None) => {}
    }
    if old.success != new.success {
        changes.push(Change::new(
            ChangeKind::SuccessTypeChanged,
            key,
            format!("success type `{}` -> `{}`", old.success, new.success),
        ));
    }
    if old.error != new.error {
        changes.push(Change::new(
            ChangeKind::ErrorTypeChanged,
            key,
            format!("error type `{}` -> `{}`", old.error, new.error),
        ));
    }
    match (&old.error_body, &new.error_body) {
        (None, Some(body)) => changes.push(Change::new(
            ChangeKind::ApiErrorBodyAdded,
            key,
            format!("error type now implements `ApiErrorBody` (`Body = {body}`)"),
        )),
        (Some(body), None) => changes.push(Change::new(
            ChangeKind::ApiErrorBodyRemoved,
            key,
            format!("error type no longer implements `ApiErrorBody` (was `Body = {body}`)"),
        )),
        // A different `Body` on both sides always changes `error` too (the shared body is one of
        // the rendered status types, or the single body itself), so `ErrorTypeChanged` reports it.
        _ => {}
    }
    for param_key in keys(&old.params, &new.params) {
        let (name, loc) = param_key;
        let location = format!("{key} param `{name}` ({loc})");
        match (old.params.get(param_key), new.params.get(param_key)) {
            // The arrival side of a move; its departure side compares the pair.
            (None, Some(_)) if moved_param(param_key, &new.params, &old.params).is_some() => {}
            (Some(old_param), None) => match moved_param(param_key, &old.params, &new.params) {
                Some(to) => diff_param(
                    format!("{key} param `{name}` ({loc} -> {})", to.1),
                    old_param,
                    &new.params[to],
                    changes,
                ),
                None => changes.push(Change::new(
                    ChangeKind::ParamRemoved,
                    location,
                    format!("parameter removed (was `{}`)", old_param.ty),
                )),
            },
            (Some(old_param), Some(new_param)) => {
                diff_param(location, old_param, new_param, changes);
            }
            (None, Some(param)) => {
                let kind = if param.required {
                    ChangeKind::RequiredParamAdded
                } else {
                    ChangeKind::OptionalParamAdded
                };
                changes.push(Change::new(
                    kind,
                    location,
                    format!(
                        "{} parameter added: `{}`",
                        if param.required {
                            "required"
                        } else {
                            "optional"
                        },
                        param.ty
                    ),
                ));
            }
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
    diff_response_headers(key, &old.response_headers, &new.response_headers, changes);
}

/// The changes between two operations' typed response-header structs. A struct that appears is a
/// new type nothing yet names, so it is additive whatever it holds; inside a struct present on
/// both sides, a header is classified as a struct field is.
fn diff_response_headers(
    key: &str,
    old: &BTreeMap<String, HeadersSurface>,
    new: &BTreeMap<String, HeadersSurface>,
    changes: &mut Vec<Change>,
) {
    for status in keys(old, new) {
        let location = format!("{key} {status} headers");
        match (old.get(status), new.get(status)) {
            (None, Some(headers)) => changes.push(Change::new(
                ChangeKind::ResponseHeadersAdded,
                location,
                format!("response header struct `{}` added", headers.name),
            )),
            (Some(headers), None) => changes.push(Change::new(
                ChangeKind::ResponseHeadersRemoved,
                location,
                format!("response header struct `{}` removed", headers.name),
            )),
            (Some(old_headers), Some(new_headers)) => {
                if old_headers.name != new_headers.name {
                    changes.push(Change::new(
                        ChangeKind::ResponseHeadersRenamed,
                        location,
                        format!(
                            "response header struct renamed `{}` -> `{}`",
                            old_headers.name, new_headers.name
                        ),
                    ));
                }
                diff_header_fields(key, status, old_headers, new_headers, changes);
            }
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

/// The changes between the fields of one status's response-header struct on both sides.
fn diff_header_fields(
    key: &str,
    status: &str,
    old: &HeadersSurface,
    new: &HeadersSurface,
    changes: &mut Vec<Change>,
) {
    for field in keys(&old.fields, &new.fields) {
        let location = format!("{key} {status} header `{field}`");
        match (old.fields.get(field), new.fields.get(field)) {
            (None, Some(header)) => {
                let (kind, label) = if header.required {
                    (ChangeKind::RequiredResponseHeaderAdded, "required")
                } else {
                    (ChangeKind::ResponseHeaderAdded, "optional")
                };
                changes.push(Change::new(
                    kind,
                    location,
                    format!("{label} response header added: `{}`", header.ty),
                ));
            }
            (Some(header), None) => changes.push(Change::new(
                ChangeKind::ResponseHeaderRemoved,
                location,
                format!("response header removed (was `{}`)", header.ty),
            )),
            (Some(old_header), Some(new_header)) => {
                if old_header.ty != new_header.ty {
                    changes.push(Change::new(
                        ChangeKind::ResponseHeaderTypeChanged,
                        location.clone(),
                        format!(
                            "response header type `{}` -> `{}`",
                            old_header.ty, new_header.ty
                        ),
                    ));
                }
                if old_header.required != new_header.required {
                    changes.push(Change::new(
                        ChangeKind::ResponseHeaderRequirednessChanged,
                        location,
                        format!(
                            "response header now {}",
                            if new_header.required {
                                "required"
                            } else {
                                "optional"
                            }
                        ),
                    ));
                }
            }
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

/// The changes between two `servers` modules: builders by name, each builder's setters, and the
/// variants of each variable enum a setter takes on both sides.
fn diff_servers(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    for name in keys(&old.servers, &new.servers) {
        let location = format!("servers::{name}");
        match (old.servers.get(name), new.servers.get(name)) {
            (None, Some(_)) => changes.push(Change::new(
                ChangeKind::ServerAdded,
                location,
                "new server builder",
            )),
            (Some(_), None) => changes.push(Change::new(
                ChangeKind::ServerRemoved,
                location,
                "server builder removed",
            )),
            (Some(old_server), Some(new_server)) => {
                diff_server_variables(&location, old_server, new_server, changes);
            }
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

/// The changes between one server builder's setters on both sides.
fn diff_server_variables(
    builder: &str,
    old: &ServerSurface,
    new: &ServerSurface,
    changes: &mut Vec<Change>,
) {
    for setter in keys(&old.variables, &new.variables) {
        let location = format!("{builder}::{setter}");
        match (old.variables.get(setter), new.variables.get(setter)) {
            (None, Some(variable)) => changes.push(Change::new(
                ChangeKind::ServerVariableAdded,
                location,
                format!("server variable added: `{}`", variable.argument()),
            )),
            (Some(variable), None) => changes.push(Change::new(
                ChangeKind::ServerVariableRemoved,
                location,
                format!("server variable removed (was `{}`)", variable.argument()),
            )),
            // The same enum on both sides: the setter is unchanged, and the enum's variants are
            // compared as any enum's are.
            (
                Some(ServerVariableSurface::Enum {
                    name: old_name,
                    variants: old_variants,
                }),
                Some(ServerVariableSurface::Enum {
                    name: new_name,
                    variants: new_variants,
                }),
            ) if old_name == new_name => {
                diff_variant_set(
                    &format!("servers::{new_name}"),
                    old_variants,
                    new_variants,
                    changes,
                );
            }
            (Some(old_variable), Some(new_variable)) if old_variable != new_variable => {
                changes.push(Change::new(
                    ChangeKind::ServerVariableTypeChanged,
                    location,
                    format!(
                        "server variable type `{}` -> `{}`",
                        old_variable.argument(),
                        new_variable.argument()
                    ),
                ));
            }
            (Some(_), Some(_)) => {}
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

/// The key a parameter moved to: `key`'s wire name is declared exactly once in `here` and exactly
/// once in `there`, at a different location. The generated identifier is allocated from the wire
/// name (the location only ranks two names that escape to the same spelling), so such a move keeps
/// the argument or `…Params` field the consumer writes, and the pair is compared as one parameter. A name declared in more than one location on either side is ambiguous, and
/// its keys stay unpaired.
fn moved_param<'a, V>(
    key: &ParamKey,
    here: &BTreeMap<ParamKey, V>,
    there: &'a BTreeMap<ParamKey, V>,
) -> Option<&'a ParamKey> {
    let named = |(name, _): &&ParamKey| *name == key.0;
    let mut there_named = there.keys().filter(named);
    match (
        here.keys().filter(named).count(),
        there_named.next(),
        there_named.next(),
    ) {
        (1, Some(to), None) if to != key => Some(to),
        _ => None,
    }
}

/// The changes between two declarations of one parameter, reported at `location`.
fn diff_param(
    location: String,
    old_param: &ParamSurface,
    new_param: &ParamSurface,
    changes: &mut Vec<Change>,
) {
    if old_param.ty != new_param.ty {
        changes.push(Change::new(
            ChangeKind::ParamTypeChanged,
            location.clone(),
            format!("parameter type `{}` -> `{}`", old_param.ty, new_param.ty),
        ));
    }
    if old_param.required != new_param.required {
        changes.push(Change::new(
            ChangeKind::ParamRequirednessChanged,
            location,
            format!(
                "parameter now {}",
                if new_param.required {
                    "required"
                } else {
                    "optional"
                }
            ),
        ));
    }
}

fn diff_types(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    for name in keys(&old.types, &new.types) {
        match (old.types.get(name), new.types.get(name)) {
            // The name was public on both sides, so code naming the alias now names a nominal type
            // with another shape, and code relying on the alias's target (or on the nominal type's
            // constructor and fields, the other way round) stops compiling.
            (None, Some(new_ty)) if old.aliases.contains(name) => {
                push_kind_change(name, "alias", new_ty.kind_label(), changes);
            }
            (Some(old_ty), None) if new.aliases.contains(name) => {
                push_kind_change(name, old_ty.kind_label(), "alias", changes);
            }
            (None, Some(_)) => {
                changes.push(Change::new(ChangeKind::TypeAdded, name, "new public type"));
            }
            (Some(_), None) => {
                changes.push(Change::new(
                    ChangeKind::TypeRemoved,
                    name,
                    "public type removed",
                ));
            }
            (Some(old_ty), Some(new_ty)) => diff_type(name, old_ty, new_ty, changes),
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

fn diff_type(name: &str, old: &TypeSurface, new: &TypeSurface, changes: &mut Vec<Change>) {
    match (old, new) {
        (
            TypeSurface::Struct(old_fields, old_overflow),
            TypeSurface::Struct(new_fields, new_overflow),
        ) => {
            diff_struct(name, old_fields, new_fields, changes);
            diff_overflow(name, old_overflow.as_ref(), new_overflow.as_ref(), changes);
        }
        (TypeSurface::Enum(old_variants, old_open), TypeSurface::Enum(new_variants, new_open)) => {
            diff_variant_set(name, old_variants, new_variants, changes);
            diff_open_variant(name, old_open.as_deref(), new_open.as_deref(), changes);
        }
        (TypeSurface::Union(old_variants), TypeSurface::Union(new_variants)) => {
            diff_union(name, old_variants, new_variants, changes);
        }
        // The wrapped type is the newtype's public `.0` field, so a change to it is a field type
        // change: every `.0` read and every `X(…)` construction sees the new type.
        (TypeSurface::Newtype(old_ty), TypeSurface::Newtype(new_ty)) if old_ty != new_ty => {
            changes.push(Change::new(
                ChangeKind::FieldTypeChanged,
                format!("{name}.0"),
                format!("field type `{old_ty}` -> `{new_ty}`"),
            ));
        }
        (TypeSurface::Newtype(_), TypeSurface::Newtype(_)) => {}
        // An empty enum has no structure to differ in.
        (TypeSurface::Never, TypeSurface::Never) => {}
        (old_ty, new_ty) => {
            push_kind_change(name, old_ty.kind_label(), new_ty.kind_label(), changes);
        }
    }
}

/// Report that the public type `name` changed generation kind from `old` to `new`.
fn push_kind_change(name: &str, old: &str, new: &str, changes: &mut Vec<Change>) {
    changes.push(Change::new(
        ChangeKind::TypeKindChanged,
        name,
        format!("type kind `{old}` -> `{new}`"),
    ));
}

/// The changes to a struct's typed `additionalProperties` map. A map that moves to another field
/// name (a declared property taking its spelling) is the old field removed and a new one added.
fn diff_overflow(
    name: &str,
    old: Option<&OverflowSurface>,
    new: Option<&OverflowSurface>,
    changes: &mut Vec<Change>,
) {
    match (old, new) {
        (Some(old_map), Some(new_map)) if old_map.field == new_map.field => {
            if old_map.ty != new_map.ty {
                changes.push(Change::new(
                    ChangeKind::AdditionalPropertiesTypeChanged,
                    format!("{name}.{}", new_map.field),
                    format!(
                        "additional properties type `BTreeMap<String, {}>` -> `BTreeMap<String, {}>`",
                        old_map.ty, new_map.ty
                    ),
                ));
            }
        }
        _ => {
            if let Some(old_map) = old {
                changes.push(Change::new(
                    ChangeKind::AdditionalPropertiesRemoved,
                    format!("{name}.{}", old_map.field),
                    format!(
                        "additional properties map removed (was `BTreeMap<String, {}>`)",
                        old_map.ty
                    ),
                ));
            }
            if let Some(new_map) = new {
                changes.push(Change::new(
                    ChangeKind::AdditionalPropertiesAdded,
                    format!("{name}.{}", new_map.field),
                    format!(
                        "additional properties map added: `BTreeMap<String, {}>`",
                        new_map.ty
                    ),
                ));
            }
        }
    }
}

fn diff_struct(
    name: &str,
    old: &BTreeMap<String, FieldSurface>,
    new: &BTreeMap<String, FieldSurface>,
    changes: &mut Vec<Change>,
) {
    for field in keys(old, new) {
        let location = format!("{name}.{field}");
        match (old.get(field), new.get(field)) {
            (None, Some(field_surface)) => {
                let kind = if field_surface.required {
                    ChangeKind::RequiredFieldAdded
                } else {
                    ChangeKind::FieldAdded
                };
                changes.push(Change::new(
                    kind,
                    location,
                    format!(
                        "{} field added: `{}`",
                        if field_surface.required {
                            "required"
                        } else {
                            "optional"
                        },
                        field_surface.ty
                    ),
                ));
            }
            (Some(field_surface), None) => changes.push(Change::new(
                ChangeKind::FieldRemoved,
                location,
                format!("field removed (was `{}`)", field_surface.ty),
            )),
            (Some(old_field), Some(new_field)) => {
                if old_field.ty != new_field.ty {
                    changes.push(Change::new(
                        ChangeKind::FieldTypeChanged,
                        location.clone(),
                        format!("field type `{}` -> `{}`", old_field.ty, new_field.ty),
                    ));
                }
                if old_field.required != new_field.required {
                    changes.push(Change::new(
                        ChangeKind::FieldRequirednessChanged,
                        location,
                        format!(
                            "field now {}",
                            if new_field.required {
                                "required"
                            } else {
                                "optional"
                            }
                        ),
                    ));
                }
            }
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

fn diff_variant_set(
    name: &str,
    old: &BTreeSet<String>,
    new: &BTreeSet<String>,
    changes: &mut Vec<Change>,
) {
    for variant in new.difference(old) {
        changes.push(Change::new(
            ChangeKind::VariantAdded,
            name,
            format!("variant added: `{variant}`"),
        ));
    }
    for variant in old.difference(new) {
        changes.push(Change::new(
            ChangeKind::VariantRemoved,
            name,
            format!("variant removed: `{variant}`"),
        ));
    }
}

/// An open enum's catch-all is a variant like any other to a consumer that matches on the enum:
/// opening a closed enum adds one (`VariantAdded`, the additive rule), closing an open one removes
/// it, and renaming it — a listed value spelled `other` taking its name — is both.
fn diff_open_variant(name: &str, old: Option<&str>, new: Option<&str>, changes: &mut Vec<Change>) {
    if old == new {
        return;
    }
    if let Some(variant) = new {
        changes.push(Change::new(
            ChangeKind::VariantAdded,
            name,
            format!("variant added: `{variant}(String)`, holding any unlisted value"),
        ));
    }
    if let Some(variant) = old {
        changes.push(Change::new(
            ChangeKind::VariantRemoved,
            name,
            format!("variant removed: `{variant}(String)`, which held any unlisted value"),
        ));
    }
}

fn diff_union(
    name: &str,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
    changes: &mut Vec<Change>,
) {
    for variant in keys(old, new) {
        match (old.get(variant), new.get(variant)) {
            (None, Some(ty)) => changes.push(Change::new(
                ChangeKind::VariantAdded,
                name,
                format!("variant added: `{variant}` (`{ty}`)"),
            )),
            (Some(ty), None) => changes.push(Change::new(
                ChangeKind::VariantRemoved,
                name,
                format!("variant removed: `{variant}` (was `{ty}`)"),
            )),
            (Some(old_ty), Some(new_ty)) if old_ty != new_ty => changes.push(Change::new(
                ChangeKind::VariantTypeChanged,
                name,
                format!("variant `{variant}` type `{old_ty}` -> `{new_ty}`"),
            )),
            _ => {}
        }
    }
}

/// The sorted union of two maps' keys — the deterministic traversal spine of every diff.
fn keys<'a, K: Ord, V>(
    old: &'a BTreeMap<K, V>,
    new: &'a BTreeMap<K, V>,
) -> impl Iterator<Item = &'a K> {
    old.keys()
        .chain(new.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
}

/// Render a [`Ty`] to its stable structural string (see the module docs). Nominal types render by
/// their generated name, so recursion always terminates at a nominal boundary or a primitive.
fn canon_ty(ty: Ty, api: &Api, names: &Names) -> String {
    let base = match api.types.get(ty.id).map(|def| &def.kind) {
        Some(TypeKind::Primitive(prim)) => prim_label(*prim).to_owned(),
        Some(TypeKind::Struct(_) | TypeKind::Union(_)) => nominal_name(ty, names),
        Some(TypeKind::Enum(enumeration)) => match enumeration.repr {
            // A string enum is a real nominal `pub enum`; int/bool enums are scalar aliases.
            ScalarRepr::String => nominal_name(ty, names),
            ScalarRepr::Int => "i64".to_owned(),
            ScalarRepr::Bool => "bool".to_owned(),
        },
        // A cycle-member array or tuple is a nominal newtype in the generated client (#648), and
        // expanding its structure here would never terminate.
        Some(TypeKind::Array(_) | TypeKind::Tuple(_)) if api.types.closes_alias_cycle(ty.id) => {
            nominal_name(ty, names)
        }
        Some(kind @ (TypeKind::Array(_) | TypeKind::Tuple(_))) => canon_container(kind, api, names),
        Some(TypeKind::Bytes) => "Bytes".to_owned(),
        Some(TypeKind::Null) => "()".to_owned(),
        Some(TypeKind::Never) => nominal_name(ty, names),
        // A reservation has no structure to canonicalise. `spargen diff` compares two finished
        // surfaces, and `check_invariants` rejects a graph that still holds one, so this is
        // unreachable — and says so structurally, as `type_kind_tokens` in `codegen::models` does for
        // the same variant. A rendered placeholder would be a string nothing asserts and a later
        // hand could fold into the `Value` arm below without anything noticing, which is the
        // silent fall-through to `Any` this variant was introduced to prevent.
        Some(TypeKind::Reserved) => {
            unreachable!(
                "a reservation reached the surface; `check_invariants` should have rejected it"
            )
        }
        Some(TypeKind::Any) | None => "Value".to_owned(),
    };
    if ty.nullable {
        format!("Option<{base}>")
    } else {
        base
    }
}

/// Render an array's or tuple's own structure one level deep: `Vec<item>` or `(a, b, …)`, each
/// item through [`canon_ty`]. The structural form of an alias at a use site, and the wrapped type
/// of a cycle-member newtype in its own surface entry.
fn canon_container(kind: &TypeKind, api: &Api, names: &Names) -> String {
    match kind {
        TypeKind::Array(inner) => format!("Vec<{}>", canon_ty(**inner, api, names)),
        TypeKind::Tuple(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| canon_ty(*item, api, names))
                .collect();
            // A one-position tuple keeps its trailing comma, as codegen declares it: `(T)` would
            // name a parenthesized `T`, not the `(T,)` the generated client has.
            match rendered.as_slice() {
                [only] => format!("({only},)"),
                _ => format!("({})", rendered.join(", ")),
            }
        }
        TypeKind::Reserved => unreachable!(
            "a reservation reached the surface; `check_invariants` should have rejected it"
        ),
        _ => unreachable!("both callers pass only an array or tuple"),
    }
}

/// The generated type name for a nominal reference: the name `name` allocated, which codegen's
/// `ty_tokens` expects in the same way.
fn nominal_name(ty: Ty, names: &Names) -> String {
    names
        .types
        .get(&ty.id)
        .expect("type name allocated")
        .as_str()
        .to_owned()
}

/// Render an operation's success shape to a stable string.
fn success_sig(shape: &SuccessShape, api: &Api, names: &Names) -> String {
    match shape {
        SuccessShape::Unit => "()".to_owned(),
        SuccessShape::Plain(ty) => canon_ty(*ty, api, names),
        SuccessShape::Enum(entries) => status_enum_sig("Success", entries, api, names),
    }
}

/// Render an operation's error shape to a stable string.
fn error_sig(shape: &ErrorShape, api: &Api, names: &Names) -> String {
    match shape {
        ErrorShape::None => "None".to_owned(),
        ErrorShape::Single(ty) => canon_ty(*ty, api, names),
        ErrorShape::Enum(entries) => status_enum_sig("Error", entries, api, names),
    }
}

/// The canonical `<E as ApiErrorBody>::Body` of an operation's error type, or `None` when the type
/// does not implement `ApiErrorBody`. Recorded directly rather than derived from [`error_sig`]:
/// the rendered signature cannot see every change in whether the trait is implemented (a bodyless
/// status and a `null` body both render `()`, and a tuple item's `Box` is not rendered at all).
fn error_body_sig(shape: &ErrorShape, api: &Api, names: &Names) -> Option<String> {
    shape
        .api_error_body(&api.types)
        .map(|implementation| match implementation {
            ApiErrorBodyImpl::Uninhabited => "Infallible".to_owned(),
            ApiErrorBodyImpl::Body(ty) => canon_ty(ty, api, names),
        })
}

/// Render a multi-status success/error enum shape: the per-status body types in the IR's
/// pre-sorted decode/classification precedence, so it is deterministic and reflects the generated
/// enum's structure.
fn status_enum_sig(
    tag: &str,
    entries: &[(StatusSpec, Option<Ty>)],
    api: &Api,
    names: &Names,
) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .map(|(status, body)| {
            let ty = match body {
                Some(ty) => canon_ty(*ty, api, names),
                None => "()".to_owned(),
            };
            format!("{}:{ty}", status.display_label())
        })
        .collect();
    format!("{tag}{{{}}}", rendered.join(", "))
}

fn scalar_value_label(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(value) => value.to_string(),
        ScalarValue::Int(value) => value.to_string(),
        ScalarValue::String(value) => value.clone(),
    }
}

fn prim_label(prim: Prim) -> &'static str {
    match prim {
        Prim::Bool => "bool",
        Prim::String => "String",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::F64 => "f64",
        Prim::Uuid => "Uuid",
        Prim::DateTime => "DateTime",
        Prim::Date => "Date",
    }
}

fn method_label(method: &crate::ir::Method) -> String {
    method.as_str().to_uppercase()
}

#[cfg(test)]
mod tests;
