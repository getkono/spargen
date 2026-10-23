//! Diffing two [`Surface`]s into a classified [`DiffReport`]: one [`Change`] per difference, each
//! carrying the [`ChangeKind`] whose impact decides the bump.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Change, ChangeKind, DiffReport, FieldSurface, HeadersSurface, Impact, OpSurface,
    OverflowSurface, ParamKey, ParamSurface, ServerSurface, ServerVariableSurface, Surface,
    TypeSurface,
};

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
