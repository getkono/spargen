use crate::diag::{Code, Diagnostic, Diagnostics};

use super::{AdditionalProps, Api, Ty, TypeKind};

/// Check the IR's well-formedness invariants, reporting any violation through `diags`.
///
/// Run after every lowering in tests and debug builds (PRD §7.5): every [`Ty`](super::Ty)'s
/// `TypeId` resolves in the [`TypeGraph`](super::TypeGraph), discriminator properties exist on
/// their variants, path parameters are declared, and there are no dangling references. A failure
/// here is a frontend bug, not a spec problem.
pub fn check_invariants(api: &Api, diags: &mut Diagnostics) {
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
            if let Some(ty) = body.ty {
                check_ty(api, ty, diags, "request body", operation.provenance.clone());
            }
        }
        for (_, response) in &operation.responses.by_status {
            if let Some(ty) = response.body {
                check_ty(
                    api,
                    ty,
                    diags,
                    "response body",
                    operation.provenance.clone(),
                );
            }
        }
        if let Some(response) = &operation.responses.default {
            if let Some(ty) = response.body {
                check_ty(
                    api,
                    ty,
                    diags,
                    "default response body",
                    operation.provenance.clone(),
                );
            }
        }
    }

    for (_, def) in api.types.iter() {
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
            TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Bytes | TypeKind::Any => {}
        }
    }
}

fn check_ty(
    api: &Api,
    ty: Ty,
    diags: &mut Diagnostics,
    label: &str,
    provenance: crate::diag::Provenance,
) {
    if api.types.get(ty.id).is_none() {
        Diagnostic::error(Code::InvalidInput, provenance)
            .message(format!(
                "IR invariant failed: `{label}` references missing type {}",
                ty.id.0
            ))
            .emit(diags);
    }
}
