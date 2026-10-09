//! Reading a Discriminator Object's `mapping` and `defaultMapping` values as schema targets,
//! shared by lowering and the audit so both resolve a value, and word a miss, the same way.

use crate::diag::{Code, Diagnostic, Diagnostics};

use super::Resolver;

/// Whether a Discriminator Object value is a schema *name* rather than a URI reference: a
/// non-empty string of the characters a Components Object key may hold (`^[a-zA-Z0-9.\-_]+$`).
/// The specification recommends reading a value that is both a valid name and a valid relative
/// reference (`Cat`, `pets.yaml`) as a name, and asks authors to write `./pets.yaml` to mean the
/// file — which the `/` here excludes.
pub(super) fn is_schema_component_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// How a diagnostic names one Discriminator Object target: the `mapping` entry with tag `tag`, or
/// `defaultMapping` for `None`.
pub(super) fn discriminator_entry(tag: Option<&String>) -> String {
    match tag {
        Some(tag) => format!("`discriminator.mapping` entry `{tag}`"),
        None => "`discriminator.defaultMapping`".to_owned(),
    }
}

/// The `file#pointer` of the schema one Discriminator Object `target` names, or `E004` at the
/// target when the loaded description holds no schema there. `entry` describes the target in
/// the message ([`discriminator_entry`]). Shared by lowering and the audit's walk of subschemas
/// lowering never reads, so a mapping value is read, and a miss worded, the same at both.
///
/// A value is a component name or a URI reference. The specification recommends reading a value
/// that could be either as a name, and a name is exactly a Components Object key, so a value
/// made only of key characters is `#/components/schemas/<value>` and anything else is a
/// reference, written relative to the file the discriminator sits in.
pub(super) fn discriminator_target_identity(
    resolver: &Resolver<'_>,
    diags: &mut Diagnostics,
    entry: &str,
    target: &super::schema::DiscriminatorTarget,
) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
    let value = &target.value;
    let reference = if is_schema_component_name(value) {
        format!("#/components/schemas/{value}")
    } else {
        value.clone()
    };
    let identity = resolver
        .schema_reference_identity(&reference, resolver.written_in(&target.provenance))
        .filter(|(file, pointer)| resolver.node_at(*file, pointer).is_some());
    if identity.is_none() {
        // E004 case: discriminator-target
        Diagnostic::error(Code::UnresolvedRef, target.provenance.clone())
            .message(format!(
                "{entry} names `{value}`, which is not a schema in the loaded description"
            ))
            .remedy("declare the schema, correct the name or reference, or remove the entry")
            .emit(diags);
    }
    identity
}
