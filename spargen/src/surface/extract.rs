//! Building a [`Surface`] from a lowered [`Api`] and its allocated [`Names`], and the structural
//! canonicalisation ([`canon_ty`]) the module docs describe.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    ByteStreamSurface, HeadersSurface, MemberSurface, OpSurface, OverflowSurface, ServerSurface,
    ServerVariableSurface, Settings, Surface, TypeSurface, UrlArgSurface,
};
use crate::ir::{
    AdditionalProps, Api, ApiErrorBodyImpl, Emission, ErrorShape, Prim, ScalarRepr, ScalarValue,
    StatusSpec, SuccessShape, Ty, TypeKind,
};
use crate::name::Names;

/// Build the consumer-visible [`Surface`] of the client generated from `api` under `settings`, using
/// `names` for the generated identifiers a consumer actually sees.
pub(crate) fn build(api: &Api, names: &Names, settings: Settings) -> Surface {
    let mut operations = BTreeMap::new();
    for operation in &api.operations {
        let key = format!("{} {}", method_label(&operation.method), operation.path.raw);
        let operation_names = names.operation(&operation.id);
        let method_name = operation_names.method.as_str().to_owned();
        let url_accessor_name = operation_names.url_accessor.as_str().to_owned();

        let mut params = BTreeMap::new();
        // The accessor takes the method's required arguments that shape the URL, in the method's
        // order, then the bundle when an optional parameter shapes it. Codegen's
        // `OperationSig::url_args` reads the same two `ir` predicates.
        let mut url_args: Vec<UrlArgSurface> = operation
            .params
            .iter()
            .filter(|param| param.is_url_arg())
            .map(|param| UrlArgSurface::Param((param.name.clone(), param.location.as_openapi_in())))
            .collect();
        if operation.url_takes_params() {
            url_args.push(UrlArgSurface::Optional);
        }
        for param in &operation.params {
            // A param that sends an explicit `null` is set through `Nullable<T>`, so the same
            // nullable type behind another serialization is a different surface.
            let ty = if param.sends_explicit_null() {
                nullable_canon_ty(param.ty, api, names, settings)
            } else {
                canon_ty(param.ty, api, names, settings)
            };
            params.insert(
                (param.name.clone(), param.location.as_openapi_in()),
                MemberSurface {
                    ty,
                    required: param.required,
                },
            );
        }

        let request_body = operation.typed_body().map(|(body, ty)| MemberSurface {
            ty: canon_ty(ty, api, names, settings),
            required: body.required,
        });
        let error = operation.responses.error();

        operations.insert(
            key,
            OpSurface {
                method_name,
                url_accessor_name,
                request_builder_name: operation_names.request_builder.as_str().to_owned(),
                decoder_name: operation_names.decoder.as_str().to_owned(),
                byte_stream: operation_names
                    .byte_stream
                    .as_ref()
                    .map(|stream| ByteStreamSurface {
                        twin_name: stream.twin.as_str().to_owned(),
                        request_builder_name: stream.request_builder.as_str().to_owned(),
                        decoder_name: stream.decoder.as_str().to_owned(),
                        success: stream_success_sig(
                            &operation.responses.success(),
                            stream.success_enum.as_ref(),
                            api,
                            names,
                            settings,
                        ),
                    }),
                url_args,
                params,
                request_body,
                success: success_sig(&operation.responses.success(), api, names, settings),
                error: error_sig(&error, api, names, settings),
                error_body: error_body_sig(&error, api, names, settings),
                response_headers: response_headers(operation, api, names, settings),
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
        let surface = match api.types.emission(id, def) {
            Emission::Struct(object) => {
                let mut fields = BTreeMap::new();
                for field in &object.fields {
                    let field_name = names
                        .fields
                        .get(&(id, field.name.wire.clone()))
                        .expect("field name allocated")
                        .as_str()
                        .to_owned();
                    // An optional nullable field is a `Nullable<T>`, where a required one is the
                    // `Option<T>` `canon_ty` renders. A member is `required` here exactly when
                    // `new` takes it, so a property gaining `readOnly` leaves `new` as a required
                    // field turning optional does. A `writeOnly` one `new` still takes is an
                    // optional member read back, so its type is spelled with the `Option` (or
                    // `Nullable`) the struct declares.
                    let ty = if field.decodes_optional() && field.ty.nullable {
                        nullable_canon_ty(field.ty, api, names, settings)
                    } else {
                        canon_ty(field.ty, api, names, settings)
                    };
                    let ty =
                        if field.constructed() && field.decodes_optional() && !field.ty.nullable {
                            format!("Option<{ty}>")
                        } else {
                            ty
                        };
                    fields.insert(
                        field_name,
                        MemberSurface {
                            ty,
                            required: field.constructed(),
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
                        ty: canon_ty(**ty, api, names, settings),
                    }),
                    AdditionalProps::Allow | AdditionalProps::Deny => None,
                };
                TypeSurface::Struct(fields, overflow)
            }
            // Only string enums generate a real `pub enum` whose variants are surface. Integer /
            // boolean enums lower to a scalar alias and carry no variant surface; they render as
            // their scalar at every use site via `canon_ty`.
            Emission::StringEnum(enumeration) => {
                let variants = enumeration.variants.iter().map(ScalarValue::wire).collect();
                let open = names
                    .open_variants
                    .get(&id)
                    .map(|ident| ident.as_str().to_owned());
                TypeSurface::Enum(variants, open)
            }
            Emission::Union(union) => {
                let mut variants = BTreeMap::new();
                for variant in &union.variants {
                    let variant_name = names
                        .variants
                        .get(&(id, variant.name_hint.clone()))
                        .expect("union variant name allocated")
                        .as_str()
                        .to_owned();
                    variants.insert(variant_name, canon_ty(variant.ty, api, names, settings));
                }
                // An open union's `Unknown` variant is surface too: turning `open_unions` on adds
                // a variant every exhaustive `match` must name.
                if let Some(unknown) = names.open_variants.get(&id) {
                    variants.insert(
                        unknown.as_str().to_owned(),
                        "{ tag: String, value: serde_json::Value }".to_owned(),
                    );
                }
                TypeSurface::Union(variants)
            }
            // A cycle-member array or tuple is a nominal newtype whose wrapped type is the public
            // `.0` field (#648); every use site renders it by name, so its structure is compared
            // here, one level deep, or a change to what it wraps would go unreported (#654).
            Emission::Newtype => {
                TypeSurface::Newtype(canon_container(&def.kind, api, names, settings))
            }
            // An uninhabited schema is emitted as an empty `pub enum`, not as an alias, and every
            // use site renders it by name, so it is listed here like any other nominal type (#661).
            Emission::Never => TypeSurface::Never,
            // A base64 schema is a nominal newtype over `bytes::Bytes`, whose serde form is the
            // base64 string; its public `.0` field is the bytes.
            Emission::Base64 => TypeSurface::Newtype("Bytes".to_owned()),
            Emission::ScalarAlias(_) | Emission::Alias => {
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
        testing: settings.testing,
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
    settings: Settings,
) -> BTreeMap<String, HeadersSurface> {
    let mut structs = BTreeMap::new();
    for (status, response) in operation.responses.entries() {
        if response.headers.is_empty() {
            continue;
        }
        let label = crate::name::status_label(status);
        let header_names = names.operation(&operation.id).response_headers(&label);
        let name = header_names.ty.as_str().to_owned();
        let fields = response
            .headers
            .iter()
            .map(|header| {
                let field = header_names.field(&header.name).as_str().to_owned();
                let surface = MemberSurface {
                    ty: canon_ty(header.ty, api, names, settings),
                    required: header.required,
                };
                (field, surface)
            })
            .collect();
        structs.insert(status.display_label(), HeadersSurface { name, fields });
    }
    structs
}

/// Render a nullable [`Ty`] that a member carries as `Nullable<T>` (an optional nullable field, or a
/// parameter that sends an explicit `null`): the [`canon_ty`] of its non-nullable type, wrapped.
fn nullable_canon_ty(ty: Ty, api: &Api, names: &Names, settings: Settings) -> String {
    let inner = canon_ty(
        Ty {
            nullable: false,
            ..ty
        },
        api,
        names,
        settings,
    );
    format!("Nullable<{inner}>")
}

/// Render a [`Ty`] to its stable structural string (see the module docs). Nominal types render by
/// their generated name, so recursion always terminates at a nominal boundary or a primitive.
pub(super) fn canon_ty(ty: Ty, api: &Api, names: &Names, settings: Settings) -> String {
    let Some(def) = api.types.get(ty.id) else {
        return "Value".to_owned();
    };
    let base = match api.types.emission(ty.id, def) {
        // Every nominal item renders by name. A cycle-member array or tuple is one (#648), and
        // expanding its structure here would never terminate.
        Emission::Struct(_)
        | Emission::StringEnum(_)
        | Emission::Union(_)
        | Emission::Newtype
        | Emission::Never
        | Emission::Base64 => nominal_name(ty, names),
        // An integer or boolean enum is its scalar alias.
        Emission::ScalarAlias(ScalarRepr::Int) => "i64".to_owned(),
        Emission::ScalarAlias(ScalarRepr::Bool) => "bool".to_owned(),
        Emission::ScalarAlias(ScalarRepr::String) => {
            unreachable!("a string enum is emitted as a `pub enum`")
        }
        Emission::Alias => match &def.kind {
            TypeKind::Primitive(prim) => prim_label(*prim, settings),
            kind @ (TypeKind::Array(_) | TypeKind::Tuple(_)) => {
                canon_container(kind, api, names, settings)
            }
            TypeKind::Bytes(crate::ir::BytesEncoding::Raw) => "Bytes".to_owned(),
            TypeKind::Null => "()".to_owned(),
            TypeKind::Any => "Value".to_owned(),
            TypeKind::Struct(_)
            | TypeKind::Enum(_)
            | TypeKind::Union(_)
            | TypeKind::Never
            | TypeKind::Bytes(crate::ir::BytesEncoding::Base64)
            | TypeKind::Reserved => unreachable!("`TypeGraph::emission` never aliases {def:?}"),
        },
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
fn canon_container(kind: &TypeKind, api: &Api, names: &Names, settings: Settings) -> String {
    match kind {
        TypeKind::Array(inner) => format!("Vec<{}>", canon_ty(**inner, api, names, settings)),
        TypeKind::Tuple(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| canon_ty(*item, api, names, settings))
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
fn success_sig(shape: &SuccessShape, api: &Api, names: &Names, settings: Settings) -> String {
    match shape {
        SuccessShape::Unit => "()".to_owned(),
        SuccessShape::Plain(ty) => canon_ty(*ty, api, names, settings),
        SuccessShape::Enum(entries) => status_enum_sig("Success", entries, api, names, settings),
    }
}

/// Render the success type of an operation's `_stream` twin to a stable string: `ByteStream` for
/// a single binary body, or the twin's enum `enum_name` followed by its per-status bodies in the
/// IR's precedence order, each raw binary arm ([`Api::streams_binary_arm`]) a `ByteStream`. The
/// enum's name is part of it, since a consumer names the twin's enum to match on it.
fn stream_success_sig(
    shape: &SuccessShape,
    enum_name: Option<&crate::name::Ident>,
    api: &Api,
    names: &Names,
    settings: Settings,
) -> String {
    match (shape, enum_name) {
        (SuccessShape::Enum(entries), Some(enum_name)) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(status, body)| {
                    let ty = match body {
                        _ if api.streams_binary_arm(*body) => "ByteStream".to_owned(),
                        Some(ty) => canon_ty(*ty, api, names, settings),
                        None => "()".to_owned(),
                    };
                    format!("{}:{ty}", status.display_label())
                })
                .collect();
            format!("{}{{{}}}", enum_name.as_str(), rendered.join(", "))
        }
        _ => "ByteStream".to_owned(),
    }
}

/// Render an operation's error shape to a stable string.
fn error_sig(shape: &ErrorShape, api: &Api, names: &Names, settings: Settings) -> String {
    match shape {
        ErrorShape::None => "None".to_owned(),
        ErrorShape::Single(ty) => canon_ty(*ty, api, names, settings),
        ErrorShape::Enum(entries) => status_enum_sig("Error", entries, api, names, settings),
    }
}

/// The canonical `<E as ApiErrorBody>::Body` of an operation's error type, or `None` when the type
/// does not implement `ApiErrorBody`. Recorded directly rather than derived from [`error_sig`]:
/// the rendered signature cannot see every change in whether the trait is implemented (a tuple
/// item's `Box` is not rendered at all).
fn error_body_sig(
    shape: &ErrorShape,
    api: &Api,
    names: &Names,
    settings: Settings,
) -> Option<String> {
    shape
        .api_error_body(&api.types)
        .map(|implementation| match implementation {
            ApiErrorBodyImpl::Uninhabited => "Infallible".to_owned(),
            ApiErrorBodyImpl::Body(ty) => canon_ty(ty, api, names, settings),
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
    settings: Settings,
) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .map(|(status, body)| {
            let ty = match body {
                Some(ty) => canon_ty(*ty, api, names, settings),
                None => "()".to_owned(),
            };
            format!("{}:{ty}", status.display_label())
        })
        .collect();
    format!("{tag}{{{}}}", rendered.join(", "))
}

/// A primitive's canonical label under `settings`: the scalar it is, or, for the formats a setting
/// maps, what that mapping emits. `Uuid` or `String` by the `uuid` mapping; and by the `datetime`
/// backend, `DateTime<backend>`/`Date<backend>` (each backend's newtypes wrap its own crate's types,
/// so one backend's are not another's), or `String`.
pub(super) fn prim_label(prim: Prim, settings: Settings) -> String {
    let scalar = match prim {
        Prim::Bool => "bool",
        Prim::String => "String",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        Prim::F64 => "f64",
        Prim::Uuid if settings.uuid => "Uuid",
        Prim::DateTime if settings.datetime.is_typed() => {
            return format!("DateTime<{}>", settings.datetime.as_str());
        }
        Prim::Date if settings.datetime.is_typed() => {
            return format!("Date<{}>", settings.datetime.as_str());
        }
        Prim::Uuid | Prim::DateTime | Prim::Date => "String",
    };
    scalar.to_owned()
}

/// The method half of an operation's `"METHOD /path"` key. A fixed-field method is upper-cased, as
/// it is sent. An `additionalOperations` method keeps its declared case, which is the spelling it
/// is sent with, so `purge` and `PURGE` on one path stay two operations.
fn method_label(method: &crate::ir::Method) -> String {
    match method {
        crate::ir::Method::Custom(token) => token.clone(),
        fixed => fixed.as_str().to_uppercase(),
    }
}
