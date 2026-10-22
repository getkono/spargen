//! Building a [`Surface`] from a lowered [`Api`] and its allocated [`Names`], and the structural
//! canonicalisation ([`canon_ty`]) the module docs describe.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BodySurface, FieldSurface, HeadersSurface, OpSurface, OverflowSurface, ParamSurface,
    ServerSurface, ServerVariableSurface, Surface, TypeSurface,
};
use crate::ir::{
    AdditionalProps, Api, ApiErrorBodyImpl, ErrorShape, Prim, ScalarRepr, ScalarValue, StatusSpec,
    SuccessShape, Ty, TypeKind,
};
use crate::name::Names;

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

/// Render a [`Ty`] to its stable structural string (see the module docs). Nominal types render by
/// their generated name, so recursion always terminates at a nominal boundary or a primitive.
pub(super) fn canon_ty(ty: Ty, api: &Api, names: &Names) -> String {
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

pub(super) fn prim_label(prim: Prim) -> &'static str {
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
