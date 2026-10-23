//! # Subsystem: surface
//! layer-deps: ir, name
//!
//! The **consumer-visible API surface** of generated output, and semver-impact diffing between two
//! surfaces. This is a pure analysis subsystem: it reads a lowered [`crate::ir::Api`] plus its
//! allocated [`crate::name::Names`] and models these parts of what a *consumer* of the generated
//! client sees: the client methods (operations) with their typed response-header structs, the
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
//! structural string* by [`canon_ty`](extract::canon_ty):
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

mod compare;
mod extract;

pub(crate) use compare::diff;
pub(crate) use extract::build;

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

#[cfg(test)]
mod tests;
