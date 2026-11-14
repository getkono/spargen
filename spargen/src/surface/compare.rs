//! Diffing two [`Surface`]s into a classified [`DiffReport`]: one [`Change`] per difference, each
//! carrying the [`ChangeKind`] whose impact decides the bump.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    param_key_label, Bindings, Change, ChangeKind, DiffReport, HeadersSurface, Impact,
    MemberSurface, OpSurface, OverflowSurface, ParamKey, ServerSurface, ServerVariableSurface,
    Surface, TypeSurface, UrlArgSurface,
};

/// Diff two surfaces into a classified, deterministically ordered [`DiffReport`].
pub(crate) fn diff(old: &Surface, new: &Surface) -> DiffReport {
    let mut changes = Vec::new();
    diff_operations(old, new, &mut changes);
    diff_types(old, new, &mut changes);
    diff_servers(old, new, &mut changes);
    diff_credentials(old, new, &mut changes);
    diff_testing(old, new, &mut changes);

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

/// The changes between the typed credential setters on both sides, by security scheme: a scheme's
/// setters appearing, disappearing, or renamed or retyped as a set. A scheme renamed in the spec
/// is one removed and one added.
fn diff_credentials(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    let render = |setters: &BTreeMap<String, &'static str>| {
        setters
            .iter()
            .map(|(name, arguments)| format!("{name}({arguments})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for scheme in keys(&old.credentials, &new.credentials) {
        let location = format!("securitySchemes.{scheme}");
        match (old.credentials.get(scheme), new.credentials.get(scheme)) {
            (None, Some(setters)) => changes.push(Change::new(
                ChangeKind::CredentialSchemeAdded,
                location,
                format!("credential setters added: {}", render(setters)),
            )),
            (Some(setters), None) => changes.push(Change::new(
                ChangeKind::CredentialSchemeRemoved,
                location,
                format!("credential setters removed (were {})", render(setters)),
            )),
            (Some(old_setters), Some(new_setters)) if old_setters != new_setters => {
                changes.push(Change::new(
                    ChangeKind::CredentialSettersChanged,
                    location,
                    format!(
                        "credential setters {} -> {}",
                        render(old_setters),
                        render(new_setters)
                    ),
                ));
            }
            (Some(_), Some(_)) => {}
            (None, None) => unreachable!("key drawn from the union of both maps"),
        }
    }
}

/// The `testing` module appearing or disappearing.
fn diff_testing(old: &Surface, new: &Surface, changes: &mut Vec<Change>) {
    match (old.testing, new.testing) {
        (false, true) => changes.push(Change::new(
            ChangeKind::TestingModuleAdded,
            "testing",
            "`testing` module added",
        )),
        (true, false) => changes.push(Change::new(
            ChangeKind::TestingModuleRemoved,
            "testing",
            "`testing` module removed",
        )),
        (false, false) | (true, true) => {}
    }
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
    } else {
        // A companion was renamed although its method was not (see `OperationNames::url_accessor`
        // and the companions allocated after it): its callers break just as a method's would.
        let mut companions = vec![
            (
                "URL accessor",
                &old.url_accessor_name,
                &new.url_accessor_name,
            ),
            (
                "request builder",
                &old.request_builder_name,
                &new.request_builder_name,
            ),
            ("decoder", &old.decoder_name, &new.decoder_name),
        ];
        // The `_stream` twin and its companions, when both sides have a twin to rename.
        if let (Some(old_stream), Some(new_stream)) = (&old.byte_stream, &new.byte_stream) {
            companions.extend([
                ("stream twin", &old_stream.twin_name, &new_stream.twin_name),
                (
                    "stream request builder",
                    &old_stream.request_builder_name,
                    &new_stream.request_builder_name,
                ),
                (
                    "stream decoder",
                    &old_stream.decoder_name,
                    &new_stream.decoder_name,
                ),
            ]);
        }
        for (label, old_name, new_name) in companions {
            if old_name != new_name {
                changes.push(Change::new(
                    ChangeKind::MethodRenamed,
                    key,
                    format!("{label} renamed `{old_name}` -> `{new_name}`"),
                ));
            }
        }
    }
    if old.url_args != new.url_args {
        changes.push(Change::new(
            ChangeKind::UrlAccessorArgsChanged,
            key,
            format!(
                "URL accessor `{}` arguments [{}] -> [{}]",
                new.url_accessor_name,
                url_args_label(&old.url_args),
                url_args_label(&new.url_args)
            ),
        ));
    }
    // Only a reorder of the same parameters: one that appears, goes or turns positional is the
    // parameter's own change below.
    if old.positional_args != new.positional_args
        && sorted(&old.positional_args) == sorted(&new.positional_args)
    {
        changes.push(Change::new(
            ChangeKind::PositionalArgsReordered,
            key,
            format!(
                "positional arguments [{}] -> [{}]",
                params_label(&old.positional_args),
                params_label(&new.positional_args)
            ),
        ));
    }
    diff_member(
        &BODY,
        key,
        old.request_body.as_ref(),
        new.request_body.as_ref(),
        changes,
    );
    for setter in old.file_name_setters.union(&new.file_name_setters) {
        match (
            old.file_name_setters.contains(setter),
            new.file_name_setters.contains(setter),
        ) {
            (false, true) => changes.push(Change::new(
                ChangeKind::FileNameSetterAdded,
                key,
                format!("file name setter `{setter}` added"),
            )),
            (true, false) => changes.push(Change::new(
                ChangeKind::FileNameSetterRemoved,
                key,
                format!("file name setter `{setter}` removed"),
            )),
            _ => {}
        }
    }
    if old.success != new.success {
        changes.push(Change::new(
            ChangeKind::SuccessTypeChanged,
            key,
            format!("success type `{}` -> `{}`", old.success, new.success),
        ));
    }
    diff_byte_stream(key, old, new, changes);
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
                Some(to) => diff_member(
                    &PARAM,
                    format!("{key} param `{name}` ({loc} -> {})", to.1),
                    Some(old_param),
                    Some(&new.params[to]),
                    changes,
                ),
                None => diff_member(&PARAM, location, Some(old_param), None, changes),
            },
            (old_param, new_param) => diff_member(&PARAM, location, old_param, new_param, changes),
        }
    }
    diff_bindings(
        &old.param_fields,
        &new.param_fields,
        |field| format!("{key} params field `{field}`"),
        changes,
    );
    diff_response_headers(key, &old.response_headers, &new.response_headers, changes);
}

/// Report each identifier both sides generate that binds another wire name on each, at the
/// location `location` names for it. An identifier on one side only is an addition or a removal,
/// which the member's own comparison reports.
fn diff_bindings(
    old: &Bindings,
    new: &Bindings,
    location: impl Fn(&str) -> String,
    changes: &mut Vec<Change>,
) {
    for (ident, old_wire) in old {
        match new.get(ident) {
            Some(new_wire) if new_wire != old_wire => changes.push(Change::new(
                ChangeKind::IdentifierRebound,
                location(ident),
                format!("`{ident}` now binds `{new_wire}` (was `{old_wire}`)"),
            )),
            _ => {}
        }
    }
}

/// The changes to an operation's `{method}_stream` twin other than its companions' renames, which
/// `diff_operation` reports beside the method's own: a twin that appears or disappears, and a
/// twin whose success type changes, its enum's name included.
fn diff_byte_stream(key: &str, old: &OpSurface, new: &OpSurface, changes: &mut Vec<Change>) {
    match (&old.byte_stream, &new.byte_stream) {
        (None, Some(stream)) => changes.push(Change::new(
            ChangeKind::StreamTwinAdded,
            key,
            format!("stream twin `{}` added", stream.twin_name),
        )),
        (Some(stream), None) => changes.push(Change::new(
            ChangeKind::StreamTwinRemoved,
            key,
            format!("stream twin `{}` removed", stream.twin_name),
        )),
        (Some(old_stream), Some(new_stream)) if old_stream.success != new_stream.success => {
            changes.push(Change::new(
                ChangeKind::SuccessTypeChanged,
                key,
                format!(
                    "stream twin success type `{}` -> `{}`",
                    old_stream.success, new_stream.success
                ),
            ));
        }
        _ => {}
    }
}

/// Parameters as a change's detail lists them, comma-separated.
fn params_label(params: &[ParamKey]) -> String {
    params
        .iter()
        .map(param_key_label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `params` in a fixed order, so two lists holding the same parameters compare equal.
fn sorted(params: &[ParamKey]) -> Vec<&ParamKey> {
    let mut sorted: Vec<&ParamKey> = params.iter().collect();
    sorted.sort();
    sorted
}

/// A URL accessor's arguments as a change's detail lists them, comma-separated.
fn url_args_label(args: &[UrlArgSurface]) -> String {
    args.iter()
        .map(UrlArgSurface::to_string)
        .collect::<Vec<_>>()
        .join(", ")
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
        diff_member(
            &HEADER,
            format!("{key} {status} header `{field}`"),
            old.fields.get(field),
            new.fields.get(field),
            changes,
        );
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

/// What a kind of typed member is called in a change's detail, and the [`ChangeKind`] each
/// difference in it takes.
struct Member {
    /// The member as a detail names it: `parameter`, `request body`, `field`, `response header`.
    noun: &'static str,
    /// The kind of an added member that is required, and of one that is optional.
    added: (ChangeKind, ChangeKind),
    /// Whether an added member's detail says which of the two it is (`required field added`); a
    /// request body's does not.
    added_says_required: bool,
    removed: ChangeKind,
    type_changed: ChangeKind,
    requiredness_changed: ChangeKind,
}

const PARAM: Member = Member {
    noun: "parameter",
    added: (
        ChangeKind::RequiredParamAdded,
        ChangeKind::OptionalParamAdded,
    ),
    added_says_required: true,
    removed: ChangeKind::ParamRemoved,
    type_changed: ChangeKind::ParamTypeChanged,
    requiredness_changed: ChangeKind::ParamRequirednessChanged,
};

const BODY: Member = Member {
    noun: "request body",
    added: (ChangeKind::RequestBodyAdded, ChangeKind::RequestBodyAdded),
    added_says_required: false,
    removed: ChangeKind::RequestBodyRemoved,
    type_changed: ChangeKind::RequestBodyTypeChanged,
    requiredness_changed: ChangeKind::RequestBodyRequirednessChanged,
};

const FIELD: Member = Member {
    noun: "field",
    added: (ChangeKind::RequiredFieldAdded, ChangeKind::FieldAdded),
    added_says_required: true,
    removed: ChangeKind::FieldRemoved,
    type_changed: ChangeKind::FieldTypeChanged,
    requiredness_changed: ChangeKind::FieldRequirednessChanged,
};

const HEADER: Member = Member {
    noun: "response header",
    added: (
        ChangeKind::RequiredResponseHeaderAdded,
        ChangeKind::ResponseHeaderAdded,
    ),
    added_says_required: true,
    removed: ChangeKind::ResponseHeaderRemoved,
    type_changed: ChangeKind::ResponseHeaderTypeChanged,
    requiredness_changed: ChangeKind::ResponseHeaderRequirednessChanged,
};

/// The changes between a typed `member`'s two sides, reported at `location`: added, removed, or,
/// present on both, its type and its requiredness each compared. Absent on both, nothing changed.
fn diff_member(
    member: &Member,
    location: impl Into<String>,
    old: Option<&MemberSurface>,
    new: Option<&MemberSurface>,
    changes: &mut Vec<Change>,
) {
    let location = location.into();
    let noun = member.noun;
    let label = |required: bool| if required { "required" } else { "optional" };
    match (old, new) {
        (None, Some(added)) => {
            let kind = if added.required {
                member.added.0
            } else {
                member.added.1
            };
            let detail = if member.added_says_required {
                format!("{} {noun} added: `{}`", label(added.required), added.ty)
            } else {
                format!("{noun} added: `{}`", added.ty)
            };
            changes.push(Change::new(kind, location, detail));
        }
        (Some(removed), None) => changes.push(Change::new(
            member.removed,
            location,
            format!("{noun} removed (was `{}`)", removed.ty),
        )),
        (Some(old), Some(new)) => {
            if old.ty != new.ty {
                changes.push(Change::new(
                    member.type_changed,
                    location.clone(),
                    format!("{noun} type `{}` -> `{}`", old.ty, new.ty),
                ));
            }
            if old.required != new.required {
                changes.push(Change::new(
                    member.requiredness_changed,
                    location,
                    format!("{noun} now {}", label(new.required)),
                ));
            }
        }
        (None, None) => {}
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
            TypeSurface::Struct(old_fields, old_overflow, old_bindings),
            TypeSurface::Struct(new_fields, new_overflow, new_bindings),
        ) => {
            diff_struct(name, old_fields, new_fields, changes);
            diff_overflow(name, old_overflow.as_ref(), new_overflow.as_ref(), changes);
            diff_bindings(
                old_bindings,
                new_bindings,
                |field| format!("{name}.{field}"),
                changes,
            );
        }
        (
            TypeSurface::Enum(old_variants, old_open, old_bindings),
            TypeSurface::Enum(new_variants, new_open, new_bindings),
        ) => {
            diff_variant_set(name, old_variants, new_variants, changes);
            diff_open_variant(name, old_open.as_deref(), new_open.as_deref(), changes);
            diff_bindings(
                old_bindings,
                new_bindings,
                |variant| format!("{name}::{variant}"),
                changes,
            );
        }
        (
            TypeSurface::Checked(old_scalar, old_values),
            TypeSurface::Checked(new_scalar, new_values),
        ) => {
            // `get` returns the scalar and `TryFrom` takes it, so a new width breaks both, as a
            // field type change breaks its reads.
            if old_scalar != new_scalar {
                changes.push(Change::new(
                    ChangeKind::FieldTypeChanged,
                    format!("{name}.get()"),
                    format!("checked scalar `{old_scalar}` -> `{new_scalar}`"),
                ));
            }
            diff_variant_set(name, old_values, new_values, changes);
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
    old: &BTreeMap<String, MemberSurface>,
    new: &BTreeMap<String, MemberSurface>,
    changes: &mut Vec<Change>,
) {
    for field in keys(old, new) {
        diff_member(
            &FIELD,
            format!("{name}.{field}"),
            old.get(field),
            new.get(field),
            changes,
        );
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
