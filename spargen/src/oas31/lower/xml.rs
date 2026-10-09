//! XML Object hints on fields, and the gate on XML renames the generated codec cannot honour.

use std::collections::HashMap;

use crate::diag::{Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{MediaType, Operation, TypeGraph, TypeId, TypeKind, XmlField};
use crate::oas31::{JsonType, SchemaOr};

use super::prune::reachable_types;
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower a property's OpenAPI `xml` hints into the field's [`XmlField`].
    ///
    /// `xml.name` and `xml.attribute` are represented (applied as a serde rename at emit time).
    /// The hints that change the XML wire without a faithful quick-xml mapping are recorded here
    /// and dispositioned in [`gate_xml_field_renames`], once it is known whether the owning type is
    /// ever serialized as XML at all. A `$ref` property carries no inline `xml` object here.
    pub(super) fn field_xml(&mut self, child: &SchemaOr) -> XmlField {
        let SchemaOr::Schema(schema) = child else {
            return XmlField::default();
        };
        let Some(hints) = &schema.xml else {
            return XmlField::default();
        };
        let mut unsupported: Vec<String> = Vec::new();
        if hints.namespace.is_some() {
            unsupported.push("namespace".to_owned());
        }
        if hints.prefix.is_some() {
            unsupported.push("prefix".to_owned());
        }
        if hints.wrapped {
            unsupported.push("wrapped".to_owned());
        }
        // OpenAPI 3.2 replaced the `attribute`/`wrapped` flags with `nodeType`, and gave it a
        // *defaulting table*: a `$ref` node and a `type: array` schema default to `none`, and
        // everything else to `element`. Reading the field as a plain string match misses that,
        // which is how the two spellings of one construct came to disagree — `wrapped: true` was
        // rejected while its exact 3.2 equivalent, `nodeType: element` on an array, was waved
        // through and put unwrapped XML on the wire.
        //
        // `none` on a node that defaults to `none` is the default restated: it is a genuine no-op
        // and takes no disposition. Anywhere else it deletes a node from the wire, so it joins
        // `text`/`cdata` and any token outside the enumeration (the document schema does not
        // validate Schema Objects, so unknown tokens do reach here).
        let is_array = schema.types.types.contains(&JsonType::Array);
        let defaults_to_none = schema.reference.is_some() || is_array;
        let effective =
            hints
                .node_type
                .as_deref()
                .unwrap_or(if defaults_to_none { "none" } else { "element" });
        let node_type_unsupported = match effective {
            "attribute" => false,
            // On an array this is precisely `wrapped: true` — it asks for an element wrapping the
            // list, which is the representation quick-xml does not give us. On a `$ref` it names
            // the element the referenced component already produces.
            "element" => is_array,
            "none" => !defaults_to_none,
            _ => true,
        };
        if node_type_unsupported {
            unsupported.push("nodeType".to_owned());
        }
        XmlField {
            name: hints.name.clone(),
            attribute: hints.attribute,
            unsupported,
        }
    }
}

/// Suppress `xml.name`/`xml.attribute` renames on any type that is not XML-dedicated, warning `W006`.
///
/// A serde `rename` applies to every serde format, so honoring an `xml.name`/`xml.attribute` hint on
/// a struct field also rewrites that field's JSON wire name. That is only safe when the owning type
/// is used *exclusively* as an XML body. This walks the type graph from each operation's bodies and
/// parameters, partitions types into XML-reachable and non-XML-reachable, and for any struct that
/// carries an appliable XML hint but is *not* (XML-reachable AND NOT non-XML-reachable), clears the
/// hint (restoring the property's normal wire name so JSON stays correct) and emits one `W006` — so
/// the ignored hint is never silent. XML-dedicated types keep their hints.
pub(super) fn gate_xml_field_renames(
    graph: &mut TypeGraph,
    operations: &[Operation],
    meet_locations: &HashMap<TypeId, Provenance>,
    diags: &mut Diagnostics,
) {
    // Cheap guard: nothing to gate (and nothing to warn) unless some field carries an XML hint.
    // Only an emitted type's hint is reported or suppressed: an elided meet intermediate is no
    // type of the output, so a hint it copied from a member has nothing to apply to.
    let any_hint = graph.emitted().any(|(_, def)| {
        matches!(&def.kind, TypeKind::Struct(object)
        if object.fields.iter().any(|field| {
            field.xml.name.is_some()
                || field.xml.attribute
                || !field.xml.unsupported.is_empty()
        }))
    });
    if !any_hint {
        return;
    }

    let mut xml_roots: Vec<TypeId> = Vec::new();
    let mut non_xml_roots: Vec<TypeId> = Vec::new();
    for operation in operations {
        if let Some(body) = &operation.request_body {
            if let Some(ty) = body.ty {
                if body.media == MediaType::Xml {
                    xml_roots.push(ty.id);
                } else {
                    non_xml_roots.push(ty.id);
                }
            }
        }
        let responses = operation
            .responses
            .by_status
            .iter()
            .map(|(_, response)| response)
            .chain(operation.responses.default.as_ref());
        for response in responses {
            if let Some(ty) = response.body {
                if response.media == Some(MediaType::Xml) {
                    xml_roots.push(ty.id);
                } else {
                    non_xml_roots.push(ty.id);
                }
            }
        }
        for param in &operation.params {
            non_xml_roots.push(param.ty.id);
        }
    }

    let xml_reachable = reachable_types(graph, &xml_roots);
    let non_xml_reachable = reachable_types(graph, &non_xml_roots);

    // A hint that changes the XML wire cannot be waved through on a type that is actually
    // serialized as XML: ignoring `wrapped`, a namespace, or a text/cdata node emits structurally
    // different XML while reporting success, which is exactly the silent fourth behavior the
    // contract forbids. On a type never serialized as XML the same hint genuinely has no effect,
    // so it stays a warning and the document is not refused for it.
    let mut unsupported_reports: Vec<(bool, Provenance, String)> = Vec::new();
    for (id, def) in graph.emitted() {
        let TypeKind::Struct(object) = &def.kind else {
            continue;
        };
        for field in &object.fields {
            if field.xml.unsupported.is_empty() {
                continue;
            }
            unsupported_reports.push((
                xml_reachable.contains(&id),
                meet_locations.get(&id).unwrap_or(&def.provenance).clone(),
                format!(
                    "`{}` on property `{}`",
                    field.xml.unsupported.join("`, `"),
                    field.name.wire
                ),
            ));
        }
    }
    for (serialized_as_xml, provenance, what) in unsupported_reports {
        if serialized_as_xml {
            Diagnostic::error(Code::UnsupportedMediaType, provenance)
                .message(format!(
                    "unsupported XML hint(s) {what}: this type is serialized as XML, and ignoring \
                     the hint would put structurally different XML on the wire"
                ))
                .remedy(
                    "remove the hint, model the wrapper element explicitly as a nested object, or \
                     omit this API segment with spargen::omit!",
                )
                .emit(diags);
        } else {
            Diagnostic::warning(Code::XmlHintIgnored, provenance)
                .message(format!(
                    "unsupported XML hint(s) {what} ignored; this type is never serialized as XML, \
                     so the hint has no effect"
                ))
                .emit(diags);
        }
    }

    // Two quite different situations reach the same suppression, and a consumer needs to tell them
    // apart. A type that is never reached from an XML body carries an inert hint: nothing on any
    // wire moves. A type reached from an XML body *and* a non-XML one is genuinely shared, and
    // suppressing its hint changes what the XML body puts on the wire. The second became reachable
    // for a sub-file schema only once one target started generating one type; before that the two
    // uses were two types and the XML one kept its rename. Same code, same count — so the message
    // has to carry the distinction or there is nothing to compare across an upgrade.
    let to_suppress: Vec<(TypeId, bool)> = graph
        .emitted()
        .filter_map(|(id, def)| {
            let TypeKind::Struct(object) = &def.kind else {
                return None;
            };
            let has_apply_hint = object
                .fields
                .iter()
                .any(|field| field.xml.name.is_some() || field.xml.attribute);
            let reached_from_xml = xml_reachable.contains(&id);
            let dedicated = reached_from_xml && !non_xml_reachable.contains(&id);
            (has_apply_hint && !dedicated).then_some((id, reached_from_xml))
        })
        .collect();

    for (id, shared_with_xml) in to_suppress {
        let Some(def) = graph.get_mut(id) else {
            continue;
        };
        let provenance = meet_locations.get(&id).unwrap_or(&def.provenance).clone();
        if let TypeKind::Struct(object) = &mut def.kind {
            for field in &mut object.fields {
                field.xml = XmlField::default();
            }
        }
        let (message, remedy) = if shared_with_xml {
            (
                "`xml.name`/`xml.attribute` not applied: this schema is shared between an XML body \
                 and a non-XML (e.g. JSON) body, and a serde rename applies to every format, so \
                 honoring the hint would rewrite the JSON wire name too. The field keeps its \
                 normal wire name — including in the XML body, whose element/attribute name is the \
                 property name rather than the hint",
                "declare a separate schema for the XML body if the rename is required, so the two \
                 bodies stop sharing one generated type, or accept the property's normal wire name",
            )
        } else {
            (
                "`xml.name`/`xml.attribute` not applied: this schema is never used as an XML body, \
                 so the hint cannot affect any wire format; the field keeps its normal wire name",
                "remove the `xml` hint, or use this schema as an XML body if the rename is \
                 required",
            )
        };
        Diagnostic::warning(Code::XmlHintIgnored, provenance)
            .message(message)
            .remedy(remedy)
            .emit(diags);
    }
}
