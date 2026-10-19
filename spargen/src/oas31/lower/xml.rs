//! XML Object hints — on fields, on array items, and on a schema's root — and the gate on the ones
//! the generated codec cannot honour.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::ir::{MediaType, Operation, TypeGraph, TypeId, TypeKind, XmlField};
use crate::oas31::{JsonType, Schema, SchemaOr, XmlHints};

use super::prune::reachable_types;
use super::LowerCtx;

/// Where an `xml` object was written: the document key a definition's `document` holds, and the
/// pointer of the schema carrying it. A definition lowered from a schema keeps both, so the gate
/// finds a schema's root `xml` object from the definitions that schema produced.
pub(super) type XmlSite = (String, JsonPointer);

/// A schema's own `xml` object, read as the root of the definition the schema lowered to.
#[derive(Debug, Clone)]
pub(super) struct RootXml {
    /// `xml.name`, where the node type leaves the schema an element for it to name.
    name: Option<String>,
    /// The hints with no faithful quick-xml mapping, as [`XmlField::unsupported`] spells them.
    unsupported: Vec<String>,
    /// For a `$ref` whose siblings bear no shape, the type it resolved to and where the `$ref` was
    /// written: such a schema inserts no definition of its own, so its site names none, and the
    /// type is shared with every other use of the target.
    target: Option<(TypeId, Provenance)>,
}

/// The position an `xml` object is read in, which decides what its node type may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XmlNode {
    /// A property's own schema: a struct field, which may be an element or an attribute.
    Property,
    /// The `items` of an array property: each list element, emitted directly under the parent.
    Items,
    /// A schema a definition was lowered from: the element that definition serializes as.
    Root,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Read `schema`'s `xml` object as the root of the definition just inserted for it, keyed by
    /// the site the definition keeps. [`gate_xml_field_renames`] decides what it does once the
    /// graph is final; a site a field reads ([`Self::claim_field_xml`]) is skipped there.
    pub(super) fn record_root_xml(&mut self, schema: &Schema, hints: &XmlHints) {
        let (name, unsupported) = read_hints(schema, hints, XmlNode::Root);
        let site = self.xml_site(&schema.provenance);
        self.root_xml.insert(
            site,
            RootXml {
                name,
                unsupported,
                target: None,
            },
        );
    }

    /// Read the `xml` object of `schema`, a `$ref` whose siblings bear no shape and so resolved to
    /// `target` itself. A name there would rename the element of a type every use of the target
    /// shares (OpenAPI 3.1 reads it so; 3.2 ignores it under the `none` node type a `$ref`
    /// defaults to), so it is reported as unsupported rather than read by either rule.
    pub(super) fn record_ref_xml(&mut self, schema: &Schema, target: TypeId) {
        let Some(hints) = &schema.xml else {
            return;
        };
        let (_, mut unsupported) = read_hints(schema, hints, XmlNode::Root);
        if hints.name.is_some() {
            unsupported.push("name".to_owned());
        }
        let site = self.xml_site(&schema.provenance);
        self.root_xml.insert(
            site,
            RootXml {
                name: None,
                unsupported,
                target: Some((target, schema.provenance.clone())),
            },
        );
    }

    /// Mark the `xml` objects [`field_xml`] reads for property `child` — its own and its array
    /// items' — so the definitions lowered from those schemas do not read them a second time as
    /// roots.
    pub(super) fn claim_field_xml(&mut self, child: &SchemaOr) {
        let SchemaOr::Schema(schema) = child else {
            return;
        };
        for schema in std::iter::once(schema.as_ref()).chain(field_items(schema)) {
            if schema.xml.is_some() {
                let site = self.xml_site(&schema.provenance);
                self.field_xml_sites.insert(site);
            }
        }
    }

    /// The [`XmlSite`] a definition located at `provenance` keeps: the document key
    /// [`Self::insert_type`] computes, and the pointer.
    fn xml_site(&self, provenance: &Provenance) -> XmlSite {
        let document = provenance
            .span
            .map(|span| self.resolver.document_key(span.file))
            .unwrap_or_default();
        (document, provenance.pointer.clone())
    }
}

/// Lower a property's OpenAPI `xml` hints, and those of its array items, into the field's
/// [`XmlField`].
///
/// `xml.name` and `xml.attribute` are represented (applied as a serde rename at emit time). An
/// array property's items are emitted directly under the parent, one element per item, so their
/// `xml.name` is the field's name, and the array's own `xml.name` names nothing: OpenAPI ignores it
/// on an unwrapped array, and a wrapped one is reported through its `wrapped`/`nodeType` hint. The
/// hints that change the XML wire without a faithful quick-xml mapping are recorded here — an
/// items' one under an `items.` prefix — and dispositioned in [`gate_xml_field_renames`], once it is
/// known whether the owning type is ever serialized as XML at all.
pub(super) fn field_xml(child: &SchemaOr) -> XmlField {
    let SchemaOr::Schema(schema) = child else {
        return XmlField::default();
    };
    let mut field = XmlField::default();
    if let Some(hints) = &schema.xml {
        let (name, unsupported) = read_hints(schema, hints, XmlNode::Property);
        field.name = name;
        field.attribute = hints.attribute;
        field.unsupported = unsupported;
    }
    if is_list(schema) {
        field.name = None;
    }
    if let Some(items) = field_items(schema) {
        if let Some(hints) = &items.xml {
            let (name, unsupported) = read_hints(items, hints, XmlNode::Items);
            field.name = name;
            field
                .unsupported
                .extend(unsupported.into_iter().map(|hint| format!("items.{hint}")));
        }
    }
    field
}

/// Whether a property schema is a list: `type: array`, or an untyped schema whose `items` makes it
/// one.
fn is_list(schema: &Schema) -> bool {
    schema.types.types.contains(&JsonType::Array)
        || (schema.types.types.is_empty() && schema.items.is_some())
}

/// The inline `items` schema of an array property, whose `xml` object the field reads.
fn field_items(schema: &Schema) -> Option<&Schema> {
    match schema.items.as_deref() {
        Some(SchemaOr::Schema(items)) if is_list(schema) => Some(items),
        _ => None,
    }
}

/// Read one `xml` object written on `schema` in position `node`: the name it gives, if the node
/// is one a name applies to, and the hints quick-xml has no faithful mapping for.
fn read_hints(schema: &Schema, hints: &XmlHints, node: XmlNode) -> (Option<String>, Vec<String>) {
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
    // everything else to `element`. Reading the field as a plain string match misses that, which
    // is how the two spellings of one construct came to disagree — `wrapped: true` was rejected
    // while its exact 3.2 equivalent, `nodeType: element` on an array, was waved through and put
    // unwrapped XML on the wire.
    //
    // `none` on a node that defaults to `none` is the default restated: it is a genuine no-op and
    // takes no disposition. Anywhere else it deletes a node from the wire, so it joins
    // `text`/`cdata` and any token outside the enumeration (the document schema does not validate
    // Schema Objects, so unknown tokens do reach here).
    let is_array = schema.types.types.contains(&JsonType::Array);
    let defaults_to_none = schema.reference.is_some() || is_array;
    let effective =
        hints
            .node_type
            .as_deref()
            .unwrap_or(if defaults_to_none { "none" } else { "element" });
    let represented = match effective {
        // A field can be an attribute. A list element or a body's root element cannot.
        "attribute" => node == XmlNode::Property,
        // On an array this is precisely `wrapped: true` — it asks for an element wrapping the
        // list, which is the representation quick-xml does not give us. On a `$ref` it names the
        // element the referenced component already produces.
        "element" => !is_array,
        "none" => defaults_to_none,
        _ => false,
    };
    if !represented {
        unsupported.push("nodeType".to_owned());
    }
    // The deprecated spelling of `nodeType: attribute`, which the parser folds into `attribute`
    // only (it is `E011` beside `nodeType`).
    if node != XmlNode::Property && hints.node_type.is_none() && hints.attribute {
        unsupported.push("attribute".to_owned());
    }
    let name = match node {
        XmlNode::Property | XmlNode::Items => hints.name.clone(),
        // `name` SHALL be ignored where the node type is `none`, `text` or `cdata`: only an
        // element root has a name to give, and only one quick-xml can write.
        XmlNode::Root => hints
            .name
            .clone()
            .filter(|_| effective == "element" && represented),
    };
    (name, unsupported)
}

/// Whether a schema at `pointer` names the element its definition serializes as: a component
/// schema (whose name OpenAPI infers as the element's) or a Media Type Object's schema (the body's
/// root). Anywhere else — an array's items, a composition member, a map value — the definition is
/// never written as an XML document's root, so a container rename names nothing.
fn names_its_own_element(pointer: &JsonPointer) -> bool {
    let tokens: Vec<&str> = pointer.as_str().split('/').skip(1).collect();
    matches!(
        tokens.as_slice(),
        ["components", "schemas", _] | [.., "content" | "mediaTypes", _, "schema"]
    )
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
///
/// It also settles each schema's root `xml` object (`root_xml`, less the sites a field reads): a
/// root `xml.name` on a struct that names its own element becomes the struct's XML root name,
/// whoever else uses the type, and the field name of an array property whose items are that struct
/// (unless the items name their elements themselves), and every hint the codec cannot honour — a field's, an items', or a
/// root's — is `E009` on a type an XML body reaches and `W006` elsewhere.
pub(super) fn gate_xml_field_renames(
    graph: &mut TypeGraph,
    operations: &[Operation],
    meet_locations: &HashMap<TypeId, Provenance>,
    root_xml: &HashMap<XmlSite, RootXml>,
    field_xml_sites: &HashSet<XmlSite>,
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
    // Each schema's root `xml` object, with the emitted definitions lowered from that schema, in
    // emission order so the reports are deterministic. One schema can produce more than one (a
    // nullable object's struct beside its `null`), and a site a field reads is the field's.
    let mut root_sites: IndexMap<XmlSite, Vec<TypeId>> = IndexMap::new();
    for (id, def) in graph.emitted() {
        let site = (def.document.clone(), def.provenance.pointer.clone());
        let read_as_root = root_xml
            .get(&site)
            .is_some_and(|root| root.target.is_none());
        if read_as_root && !field_xml_sites.contains(&site) {
            root_sites.entry(site).or_default().push(id);
        }
    }
    // A `$ref` with nothing but annotations beside it names its target's type, not a definition
    // of its own; ordered by site, which a hash map does not keep.
    let mut ref_sites: Vec<(&XmlSite, &RootXml, TypeId, &Provenance)> = root_xml
        .iter()
        .filter(|(site, _)| !field_xml_sites.contains(*site))
        .filter_map(|(site, root)| {
            let (target, at) = root.target.as_ref()?;
            (!root.unsupported.is_empty()).then_some((site, root, *target, at))
        })
        .collect();
    ref_sites.sort_by(|(a, ..), (b, ..)| (&a.0, a.1.as_str()).cmp(&(&b.0, b.1.as_str())));
    if !any_hint && root_sites.is_empty() && ref_sites.is_empty() {
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
        for (_, response) in operation.responses.entries() {
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
    // A root `xml.name` is a serde container rename on the struct, which quick-xml writes as the
    // root element's name and no other format reads, so unlike a field's it needs no exclusively
    // XML type. It has nowhere to go on a definition that is no struct, or on a schema that never
    // names its own element (an array's items, a composition member), where it is reported as
    // unsupported like any other hint the codec cannot honour.
    for (site, ids) in &root_sites {
        let root = &root_xml[site];
        let mut unsupported = root.unsupported.clone();
        if let Some(name) = &root.name {
            let structs: Vec<TypeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    graph
                        .get(*id)
                        .is_some_and(|def| matches!(def.kind, TypeKind::Struct(_)))
                })
                .collect();
            if structs.is_empty() || !names_its_own_element(&site.1) {
                unsupported.push("name".to_owned());
            } else {
                for id in structs {
                    graph.set_xml_root_name(id, name.clone());
                }
            }
        }
        let Some(def) = (!unsupported.is_empty())
            .then(|| graph.get(ids[0]))
            .flatten()
        else {
            continue;
        };
        unsupported_reports.push((
            ids.iter().any(|id| xml_reachable.contains(id)),
            meet_locations
                .get(&ids[0])
                .unwrap_or(&def.provenance)
                .clone(),
            format!(
                "`{}` on the schema's own `xml` object",
                unsupported.join("`, `")
            ),
        ));
    }
    // A struct with an XML root name, reached as the items of an array property, is each element
    // of that list: OpenAPI 3.2 has a `$ref` item (node type `none`) produce the referenced
    // schema's own element. quick-xml writes a nested struct under the field's name and never
    // reads its container rename there, so the root name becomes the field's, unless the items
    // already gave one. It is then a field rename like any other, suppressed below (`W006`) on an
    // owner that is not exclusively XML.
    let view: &TypeGraph = graph;
    let item_names: Vec<(TypeId, usize, String)> = view
        .emitted()
        .filter_map(|(id, def)| {
            let TypeKind::Struct(object) = &def.kind else {
                return None;
            };
            Some((id, object))
        })
        .flat_map(|(id, object)| {
            object
                .fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.xml.name.is_none() && !field.xml.attribute)
                .filter_map(move |(index, field)| {
                    let TypeKind::Array(item) = &view.get(field.ty.id)?.kind else {
                        return None;
                    };
                    Some((id, index, view.xml_root_name(item.id)?.to_owned()))
                })
        })
        .collect();
    for (id, index, name) in item_names {
        if let Some(TypeKind::Struct(object)) = graph.get_mut(id).map(|def| &mut def.kind) {
            object.fields[index].xml.name = Some(name);
        }
    }
    for (_, root, target, at) in ref_sites {
        unsupported_reports.push((
            xml_reachable.contains(&target),
            at.clone(),
            format!(
                "`{}` on the `xml` object beside a `$ref`, whose type every use of the target \
                 shares",
                root.unsupported.join("`, `")
            ),
        ));
    }
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
