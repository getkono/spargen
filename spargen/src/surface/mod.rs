//! # Subsystem: surface
//! layer-deps: ir, name
//!
//! The **consumer-visible API surface** of generated output, and semver-impact diffing between two
//! surfaces. This is a pure analysis subsystem: it reads a lowered [`Api`](crate::ir::Api) plus its
//! allocated [`Names`](crate::name::Names) and models exactly what a *consumer* of the generated
//! client sees — the client methods (operations) and the public model types — then classifies every
//! difference between an old and a new surface as a semver bump. It never emits code and has no
//! effect on generation or the runtime; it backs `spargen diff`.
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
//! * a **nominal** type — a `struct`, a string `enum`, or a union (the models a consumer names,
//!   constructs, and matches) → its generated type name;
//! * an array → `Vec<inner>`, a tuple → `(a, b, …)`, bytes → `Bytes`, an untyped node → `Value`;
//! * an integer/boolean scalar `enum`/`const` (which generates a `pub type X = i64`/`bool` alias,
//!   carrying no consumer-facing structure beyond its scalar) → that scalar;
//! * a nullable reference wraps the above in `Option<…>`.
//!
//! `boxed` is *not* rendered: boxing is an internal cycle-break artifact, not a surface distinction.
//! Because nominal types render by name (never expanded in place), rendering always terminates, and
//! each nominal type is compared once, as its own surface entry.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    Api, ErrorShape, Prim, ScalarRepr, ScalarValue, StatusSpec, SuccessShape, Ty, TypeKind,
};
use crate::name::Names;

/// The full consumer-visible surface of one generated client: its operations (client methods) and
/// its public model types, each keyed by a stable identity so two surfaces can be diffed.
pub(crate) struct Surface {
    /// Operations keyed by `"METHOD /path"` — the stable endpoint identity (independent of a
    /// possibly-synthesised `operationId`).
    operations: BTreeMap<String, OpSurface>,
    /// Public model types keyed by their generated type name.
    types: BTreeMap<String, TypeSurface>,
}

/// One client method's surface: the generated method name, its parameters, request body, and the
/// success/error return types — everything the method's signature exposes.
struct OpSurface {
    /// The generated Rust method identifier.
    method_name: String,
    /// Parameters keyed by wire name (path/query/header/cookie alike).
    params: BTreeMap<String, ParamSurface>,
    /// The canonical request-body type, or `None` for a bodyless operation. A present body is a
    /// required `&T` argument in the generated signature (the IR does not model an optional body).
    request_body: Option<String>,
    /// Canonical rendering of the success return type.
    success: String,
    /// Canonical rendering of the error type.
    error: String,
}

/// A single parameter's surface: its canonical type and whether it is required (required params are
/// positional method arguments; optional ones ride in the `…Params` struct).
struct ParamSurface {
    ty: String,
    required: bool,
}

/// A public model type's surface, by generation kind. Only nominal types with real distinct
/// structure are modelled: structs, string `enum`s, and unions. Integer/boolean enums and other
/// alias-only components carry no structure beyond a scalar the consumer already sees at every use
/// site, so they are compared structurally there rather than as standalone entries.
enum TypeSurface {
    /// A `struct`: fields keyed by generated field name.
    Struct(BTreeMap<String, FieldSurface>),
    /// A string `enum`: the set of variant wire values.
    Enum(BTreeSet<String>),
    /// A union enum: variants keyed by generated variant name → canonical payload type.
    Union(BTreeMap<String, String>),
}

impl TypeSurface {
    /// A short label for the generation kind, used to report a kind change (`struct` → `enum`, …).
    fn kind_label(&self) -> &'static str {
        match self {
            TypeSurface::Struct(_) => "struct",
            TypeSurface::Enum(_) => "enum",
            TypeSurface::Union(_) => "union",
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

/// The semver impact of one change (and of a whole diff, taken as the max over its changes). The
/// ordering `Patch < Minor < Major` is load-bearing: the overall bump is the maximum impact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
pub enum ChangeKind {
    /// A new operation (client method) appeared. **Minor** — additive.
    OperationAdded,
    /// An operation was removed. **Major** — a method the consumer calls disappears.
    OperationRemoved,
    /// The generated method name for an endpoint changed. **Major** — the callable renames.
    MethodRenamed,
    /// A request body was added to an operation. **Major** — a new required `&T` argument.
    RequestBodyAdded,
    /// An operation's request body was removed. **Major** — an argument disappears.
    RequestBodyRemoved,
    /// An operation's request-body type changed. **Major**.
    RequestBodyTypeChanged,
    /// An operation's success return type changed. **Major**.
    SuccessTypeChanged,
    /// An operation's error type changed. **Major**.
    ErrorTypeChanged,
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
    /// A public type's generation kind changed (`struct` ↔ `enum` ↔ `union`). **Major**.
    TypeKindChanged,
    /// A new optional field was added to a struct. **Minor**.
    FieldAdded,
    /// A new required field was added to a struct. **Major** — every existing constructor breaks.
    RequiredFieldAdded,
    /// A struct field was removed. **Major**.
    FieldRemoved,
    /// A struct field's type changed. **Major**.
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
}

impl ChangeKind {
    /// This change kind's fixed semver impact — the classification policy.
    pub fn impact(self) -> Impact {
        match self {
            ChangeKind::OperationAdded
            | ChangeKind::OptionalParamAdded
            | ChangeKind::TypeAdded
            | ChangeKind::FieldAdded
            | ChangeKind::VariantAdded => Impact::Minor,
            ChangeKind::OperationRemoved
            | ChangeKind::MethodRenamed
            | ChangeKind::RequestBodyAdded
            | ChangeKind::RequestBodyRemoved
            | ChangeKind::RequestBodyTypeChanged
            | ChangeKind::SuccessTypeChanged
            | ChangeKind::ErrorTypeChanged
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
            | ChangeKind::VariantTypeChanged => Impact::Major,
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
            ChangeKind::SuccessTypeChanged => "success-type-changed",
            ChangeKind::ErrorTypeChanged => "error-type-changed",
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
        }
    }
}

/// A single classified difference between two surfaces.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct DiffReport {
    /// Every change, sorted most-severe-first then by location and code (deterministic).
    pub changes: Vec<Change>,
    /// The overall recommended bump: the maximum impact across `changes`, or `Patch` if empty.
    pub bump: Impact,
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
        let key = format!("{} {}", method_label(operation.method), operation.path.raw);
        let method_name = names
            .operations
            .get(&operation.id)
            .map(|ident| ident.as_str().to_owned())
            .unwrap_or_else(|| operation.id.0.clone());

        let mut params = BTreeMap::new();
        for param in &operation.params {
            params.insert(
                param.name.clone(),
                ParamSurface {
                    ty: canon_ty(param.ty, api, names),
                    required: param.required,
                },
            );
        }

        let request_body = operation
            .request_body
            .as_ref()
            .and_then(|body| body.ty)
            .map(|ty| canon_ty(ty, api, names));

        operations.insert(
            key,
            OpSurface {
                method_name,
                params,
                request_body,
                success: success_sig(&operation.responses.success(), api, names),
                error: error_sig(&operation.responses.error(), api, names),
            },
        );
    }

    let mut types = BTreeMap::new();
    for (id, def) in api.types.iter() {
        let name = match names.types.get(&id) {
            Some(ident) => ident.as_str().to_owned(),
            None => continue,
        };
        let surface = match &def.kind {
            TypeKind::Struct(object) => {
                let mut fields = BTreeMap::new();
                for field in &object.fields {
                    let field_name = names
                        .fields
                        .get(&(id, field.name.wire.clone()))
                        .map(|ident| ident.as_str().to_owned())
                        .unwrap_or_else(|| field.name.wire.clone());
                    fields.insert(
                        field_name,
                        FieldSurface {
                            ty: canon_ty(field.ty, api, names),
                            required: field.required,
                        },
                    );
                }
                TypeSurface::Struct(fields)
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
                TypeSurface::Enum(variants)
            }
            TypeKind::Union(union) => {
                let mut variants = BTreeMap::new();
                for variant in &union.variants {
                    let variant_name = names
                        .variants
                        .get(&(id, variant.name_hint.clone()))
                        .map(|ident| ident.as_str().to_owned())
                        .unwrap_or_else(|| variant.name_hint.clone());
                    variants.insert(variant_name, canon_ty(variant.ty, api, names));
                }
                TypeSurface::Union(variants)
            }
            _ => continue,
        };
        types.insert(name, surface);
    }

    Surface { operations, types }
}

/// Diff two surfaces into a classified, deterministically ordered [`DiffReport`].
pub(crate) fn diff(old: &Surface, new: &Surface) -> DiffReport {
    let mut changes = Vec::new();
    diff_operations(old, new, &mut changes);
    diff_types(old, new, &mut changes);

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
        (None, Some(ty)) => changes.push(Change::new(
            ChangeKind::RequestBodyAdded,
            key,
            format!("request body added: `{ty}`"),
        )),
        (Some(ty), None) => changes.push(Change::new(
            ChangeKind::RequestBodyRemoved,
            key,
            format!("request body removed (was `{ty}`)"),
        )),
        (Some(old_ty), Some(new_ty)) if old_ty != new_ty => changes.push(Change::new(
            ChangeKind::RequestBodyTypeChanged,
            key,
            format!("request body type `{old_ty}` -> `{new_ty}`"),
        )),
        _ => {}
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
    for name in keys(&old.params, &new.params) {
        let location = format!("{key} param `{name}`");
        match (old.params.get(name), new.params.get(name)) {
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
            (Some(param), None) => changes.push(Change::new(
                ChangeKind::ParamRemoved,
                location,
                format!("parameter removed (was `{}`)", param.ty),
            )),
            (Some(old_param), Some(new_param)) => {
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
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

fn diff_types(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    for name in keys(&old.types, &new.types) {
        match (old.types.get(name), new.types.get(name)) {
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
        (TypeSurface::Struct(old_fields), TypeSurface::Struct(new_fields)) => {
            diff_struct(name, old_fields, new_fields, changes);
        }
        (TypeSurface::Enum(old_variants), TypeSurface::Enum(new_variants)) => {
            diff_variant_set(name, old_variants, new_variants, changes);
        }
        (TypeSurface::Union(old_variants), TypeSurface::Union(new_variants)) => {
            diff_union(name, old_variants, new_variants, changes);
        }
        (old_ty, new_ty) => changes.push(Change::new(
            ChangeKind::TypeKindChanged,
            name,
            format!(
                "type kind `{}` -> `{}`",
                old_ty.kind_label(),
                new_ty.kind_label()
            ),
        )),
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
fn keys<'a, V>(
    old: &'a BTreeMap<String, V>,
    new: &'a BTreeMap<String, V>,
) -> impl Iterator<Item = &'a String> {
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
        Some(TypeKind::Array(inner)) => format!("Vec<{}>", canon_ty(**inner, api, names)),
        Some(TypeKind::Tuple(items)) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| canon_ty(*item, api, names))
                .collect();
            format!("({})", rendered.join(", "))
        }
        Some(TypeKind::Bytes) => "Bytes".to_owned(),
        Some(TypeKind::Null) => "()".to_owned(),
        Some(TypeKind::Never) => nominal_name(ty, names),
        Some(TypeKind::Any) | None => "Value".to_owned(),
    };
    if ty.nullable {
        format!("Option<{base}>")
    } else {
        base
    }
}

/// The generated type name for a nominal reference, falling back to a structural placeholder if the
/// id somehow lacks an allocated name (never expected post-allocation).
fn nominal_name(ty: Ty, names: &Names) -> String {
    names
        .types
        .get(&ty.id)
        .map(|ident| ident.as_str().to_owned())
        .unwrap_or_else(|| "Unknown".to_owned())
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
            format!("{}:{ty}", status_label(*status))
        })
        .collect();
    format!("{tag}{{{}}}", rendered.join(", "))
}

fn status_label(status: StatusSpec) -> String {
    match status {
        StatusSpec::Exact(code) => code.to_string(),
        StatusSpec::Range(0) => "default".to_owned(),
        StatusSpec::Range(prefix) => format!("{prefix}XX"),
    }
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

fn method_label(method: crate::ir::Method) -> String {
    method.as_str().to_uppercase()
}
