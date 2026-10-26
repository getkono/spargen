//! XML Object hints — on fields, on array items, and on a schema's root — and the gate on the ones
//! the generated codec cannot honour.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::ir::{Field, MediaType, Operation, TypeGraph, TypeId, TypeKind, XmlField};
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
    /// For a `$ref` whose siblings bear no shape, the type it resolved to: such a schema inserts
    /// no definition of its own, so its site names none, and the type is shared with every other
    /// use of the target.
    target: Option<TypeId>,
    /// The schema carrying the `xml` object, where its reports point.
    at: Provenance,
    /// For a composition member read by its keywords ([`LowerCtx::record_member_xml`]), the site
    /// of the composition it was gathered into, where its merged type is looked for first.
    holder: Option<XmlSite>,
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
        let holder = self
            .root_xml
            .get(&site)
            .and_then(|root| root.holder.clone());
        self.root_xml.insert(
            site,
            RootXml {
                name,
                unsupported,
                target: None,
                at: schema.provenance.clone(),
                holder,
            },
        );
    }

    /// Read the `xml` object of `schema`, a composition member gathered by its keywords into the
    /// composition [`Self::xml_holders`] names last. Such a member inserts no definition of its
    /// own, so [`gate_xml_field_renames`] reports each of its hints against the type its
    /// composition lowered to. A definition later lowered from the same schema reads it as a
    /// root instead ([`Self::record_root_xml`]), keeping the holder.
    pub(super) fn record_member_xml(&mut self, schema: &Schema) {
        let Some(hints) = &schema.xml else {
            return;
        };
        let (name, mut unsupported) = read_hints(schema, hints, XmlNode::Root);
        // A `$ref` defaults to the node type `none`, under which `read_hints` gives no name; a
        // name beside a member's `$ref` would still rename neither the target nor the merge.
        if schema.reference.is_some() && name.is_none() && hints.name.is_some() {
            unsupported.push("name".to_owned());
        }
        let site = self.xml_site(&schema.provenance);
        let holder = self.xml_holders.last().cloned();
        self.root_xml.entry(site).or_insert(RootXml {
            name,
            unsupported,
            target: None,
            at: schema.provenance.clone(),
            holder,
        });
    }

    /// Run `gather` with `schema` as the composition [`Self::record_member_xml`] attributes the
    /// members it gathers to.
    pub(super) fn with_xml_holder<T>(
        &mut self,
        schema: &Schema,
        gather: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let site = self.xml_site(&schema.provenance);
        self.xml_holders.push(site);
        let result = gather(self);
        self.xml_holders.pop();
        result
    }

    /// Note that the composition [`Self::xml_holders`] names last merges `ty`, the type a `$ref`
    /// member resolved to, so a member merged into `ty` is judged by that composition's type too.
    pub(super) fn record_xml_merger(&mut self, ty: TypeId) {
        let Some(holder) = self.xml_holders.last().cloned() else {
            return;
        };
        let mergers = self.xml_mergers.entry(ty).or_default();
        if !mergers.contains(&holder) {
            mergers.push(holder);
        }
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
                target: Some(target),
                at: schema.provenance.clone(),
                holder: None,
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
/// known whether the owning type is ever serialized as XML at all. Whether the property, or its
/// items, is a `$ref` is recorded too ([`XmlField::by_reference`]): under OpenAPI 3.2 that field
/// takes the referenced struct's element name, which is only known once the graph is final.
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
    // Only a `$ref` whose node type is `none` (its default) defers to the referenced element: an
    // explicit `nodeType: element` makes the reference itself an element, named after the property.
    let node = field_items(schema).unwrap_or(schema);
    let node_type = node
        .xml
        .as_ref()
        .and_then(|hints| hints.node_type.as_deref());
    field.by_reference =
        node.reference.is_some() && node_type.is_none_or(|node_type| node_type == "none");
    field.element_reference = node.reference.is_some() && node_type == Some("element");
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
        // list, which is the representation quick-xml does not give us. On a `$ref` it makes the
        // reference an element of its own, named after the property rather than after the
        // referenced component (`field_xml`), which a nested struct field already is.
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

/// The component name of a schema at `pointer` directly under `components.schemas`, unescaped: the
/// element name OpenAPI infers for it. A schema anywhere else has none.
fn component_name(pointer: &JsonPointer) -> Option<String> {
    match pointer.tokens().as_slice() {
        [components, schemas, name] if components == "components" && schemas == "schemas" => {
            Some(name.clone())
        }
        _ => None,
    }
}

/// The definitions a merged member's `xml` object at `site` is reported against: those at its
/// composition's site ([`RootXml::holder`]), else at the nearest pointer above it that has any, in
/// the same document; and, for each of those a `$ref` member of another composition merged
/// ([`LowerCtx::record_xml_merger`]), that composition's, transitively. None where no definition
/// encloses it.
///
/// Each site is looked up among the emitted definitions (`emitted_sites`) and among every
/// definition, elided ones included (`defined_sites`). A definition the output withholds is still
/// a link of the walk: a remote document's type that only an `allOf` member reaches is elided
/// ([`LowerCtx::elide_unreachable`]), and the composition that merged it is found only through it
/// (#1017). An elided owner decides nothing itself, because no XML body reaches a type the output
/// withholds.
fn merged_into(
    site: &XmlSite,
    root: &RootXml,
    emitted_sites: &HashMap<XmlSite, Vec<TypeId>>,
    defined_sites: &HashMap<XmlSite, Vec<TypeId>>,
    xml_mergers: &HashMap<TypeId, Vec<XmlSite>>,
) -> Vec<TypeId> {
    let enclosing = |document: &str, pointer: Option<&JsonPointer>| {
        let mut ids = enclosing_emitted(document, pointer, emitted_sites);
        for id in enclosing_emitted(document, pointer, defined_sites) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    };
    let mut owners = match &root.holder {
        Some((document, pointer)) => enclosing(document, Some(pointer)),
        None => enclosing(&site.0, site.1.parent().as_ref()),
    };
    let mut next = 0;
    while let Some(&id) = owners.get(next) {
        next += 1;
        for (document, pointer) in xml_mergers.get(&id).into_iter().flatten() {
            for merger in enclosing(document, Some(pointer)) {
                if !owners.contains(&merger) {
                    owners.push(merger);
                }
            }
        }
    }
    owners
}

/// The emitted definitions at `pointer` in `document`, else at the nearest pointer above it that
/// has any; none where no definition encloses it.
fn enclosing_emitted(
    document: &str,
    pointer: Option<&JsonPointer>,
    emitted_sites: &HashMap<XmlSite, Vec<TypeId>>,
) -> Vec<TypeId> {
    let mut pointer = pointer.cloned();
    while let Some(at) = pointer {
        if let Some(ids) = emitted_sites.get(&(document.to_owned(), at.clone())) {
            return ids.clone();
        }
        pointer = at.parent();
    }
    Vec::new()
}

/// Whether `name` is an XML 1.1 `Name` (<https://www.w3.org/TR/xml11/#NT-Name>), the rule quick-xml
/// holds every element name it writes to. A component key is not held to it: `[a-zA-Z0-9._-]+`
/// admits a leading digit, `-` or `.`.
fn is_xml_name(name: &str) -> bool {
    fn is_start(ch: char) -> bool {
        matches!(ch,
            ':' | 'A'..='Z' | '_' | 'a'..='z'
            | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    }
    fn is_char(ch: char) -> bool {
        is_start(ch)
            || matches!(ch,
                '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
    }
    let mut chars = name.chars();
    chars.next().is_some_and(is_start) && chars.all(is_char)
}

/// Settle every XML hint once the graph is final: suppress `xml.name`/`xml.attribute` renames on
/// any type that is not XML-dedicated (`W006`), apply root `xml.name`s, and report each hint the
/// codec cannot honour (`E009` on a type an XML body reaches, `W006` elsewhere).
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
/// (unless the items name their elements themselves); a component struct an XML body reaches with
/// no root `xml.name` takes its component name as its XML root name, or reports it (`W006`) where
/// it is no XML name; under OpenAPI 3.2 (`is_oas32`), a property that is a `$ref` to a struct, or
/// an array property whose items are, takes that struct's XML root name as its field name; and
/// every hint the codec cannot honour — a field's, an items', a root's, one beside a `$ref`
/// ([`LowerCtx::record_ref_xml`]), or any on a composition member the merge consumed
/// ([`LowerCtx::record_member_xml`], judged by the type the merge produced) — is `E009` on a type
/// an XML body reaches and `W006` elsewhere.
#[allow(clippy::too_many_arguments)]
pub(super) fn gate_xml_field_renames(
    graph: &mut TypeGraph,
    operations: &[Operation],
    meet_locations: &HashMap<TypeId, Provenance>,
    root_xml: &HashMap<XmlSite, RootXml>,
    field_xml_sites: &HashSet<XmlSite>,
    xml_mergers: &HashMap<TypeId, Vec<XmlSite>>,
    is_oas32: bool,
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
    let mut ref_sites: Vec<(&XmlSite, &RootXml, TypeId)> = root_xml
        .iter()
        .filter(|(site, _)| !field_xml_sites.contains(*site))
        .filter_map(|(site, root)| {
            let target = root.target?;
            (!root.unsupported.is_empty()).then_some((site, root, target))
        })
        .collect();
    ref_sites.sort_by(|(a, ..), (b, ..)| (&a.0, a.1.as_str()).cmp(&(&b.0, b.1.as_str())));
    // A schema whose `xml` object no emitted definition keeps: a composition member merged into
    // another type (an `allOf` member read by its keywords, a union branch merged away). It names
    // no element of its own, so each of its hints, a name included, is reported against the type
    // the merge produced: the nearest emitted definition at its composition's site or above it.
    let mut emitted_sites: HashMap<XmlSite, Vec<TypeId>> = HashMap::new();
    for (id, def) in graph.emitted() {
        let site = (def.document.clone(), def.provenance.pointer.clone());
        emitted_sites.entry(site).or_default().push(id);
    }
    let mut defined_sites: HashMap<XmlSite, Vec<TypeId>> = HashMap::new();
    for (id, def) in graph.iter() {
        let site = (def.document.clone(), def.provenance.pointer.clone());
        defined_sites.entry(site).or_default().push(id);
    }
    let mut merged_sites: Vec<(&XmlSite, &RootXml, Vec<TypeId>)> = root_xml
        .iter()
        .filter(|(site, root)| {
            root.target.is_none()
                && !field_xml_sites.contains(*site)
                && !emitted_sites.contains_key(*site)
                && (root.name.is_some() || !root.unsupported.is_empty())
        })
        .map(|(site, root)| {
            let owners = merged_into(site, root, &emitted_sites, &defined_sites, xml_mergers);
            (site, root, owners)
        })
        .collect();
    merged_sites.sort_by(|(a, ..), (b, ..)| (&a.0, a.1.as_str()).cmp(&(&b.0, b.1.as_str())));

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
    // Cheap guard: no hint to settle and no XML body whose root element a component names.
    if !any_hint
        && root_sites.is_empty()
        && ref_sites.is_empty()
        && merged_sites.is_empty()
        && xml_roots.is_empty()
    {
        return;
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
    for (_, root, owners) in merged_sites {
        let mut unsupported = root.unsupported.clone();
        if root.name.is_some() {
            unsupported.push("name".to_owned());
        }
        unsupported_reports.push((
            owners.iter().any(|id| xml_reachable.contains(id)),
            root.at.clone(),
            format!(
                "`{}` on the `xml` object of a composition member merged into another type, which \
                 names no element of its own",
                unsupported.join("`, `")
            ),
        ));
    }
    // A struct with an XML root name, reached as the items of an array property, is each element
    // of that list: OpenAPI 3.2 has a `$ref` item (node type `none`) produce the referenced
    // schema's own element. quick-xml writes a nested struct under the field's name and never
    // reads its container rename there, so the root name becomes the field's, unless the items
    // already gave one. Only an owner some XML body reaches takes it: on any other, the rename is
    // derived rather than written and moves no wire, so it is not set at all. On an owner a non-XML
    // body also reaches, it is a field rename like any other, suppressed below (`W006`). Under 3.2,
    // items that are a `$ref` declared `nodeType: element` are an element of their own, named after
    // the property, so they take no root name either.
    let view: &TypeGraph = graph;
    let item_names: Vec<(TypeId, usize, String)> = view
        .emitted()
        .filter(|(id, _)| xml_reachable.contains(id))
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
                .filter(|(_, field)| {
                    field.xml.name.is_none()
                        && !field.xml.attribute
                        && !(is_oas32 && field.xml.element_reference)
                })
                .filter_map(move |(index, field)| {
                    let TypeKind::Array(item) = &view.get(field.ty.id)?.kind else {
                        return None;
                    };
                    Some((id, index, view.xml_root_name(item.id)?.to_owned()))
                })
        })
        .collect();
    // Every field rename derived here rather than written by the author, by owner and field index:
    // checked for a clash with its siblings' element names below, and named as derived if the
    // shared-type suppression withholds it.
    let mut derived: HashSet<(TypeId, usize)> = HashSet::new();
    for (id, index, name) in item_names {
        if let Some(TypeKind::Struct(object)) = graph.get_mut(id).map(|def| &mut def.kind) {
            object.fields[index].xml.name = Some(name);
            derived.insert((id, index));
        }
    }
    // Without a root `xml.name`, OpenAPI infers a component schema's element name as the
    // component's own name ("XML Node Names"), where quick-xml would write the Rust type's name.
    // So a struct lowered from a schema directly under `components.schemas` takes that name as its
    // XML root name; codegen writes it only where it differs from the struct's identifier. Only a
    // struct some XML body reaches takes it: on any other, the name moves no wire. It is set after
    // the item names above, which follow an explicit root `xml.name` only. A component key that is
    // no XML name would make quick-xml refuse every serialization of the body at run time, so it
    // is reported (`W006`) and not applied, and the struct keeps its Rust name.
    let inferred: Vec<(TypeId, String, Provenance)> = graph
        .emitted()
        .filter(|(id, def)| {
            xml_reachable.contains(id)
                && matches!(def.kind, TypeKind::Struct(_))
                && graph.xml_root_name(*id).is_none()
        })
        .filter_map(|(id, def)| {
            let name = component_name(&def.provenance.pointer)?;
            let at = meet_locations.get(&id).unwrap_or(&def.provenance).clone();
            Some((id, name, at))
        })
        .collect();
    for (id, name, at) in inferred {
        if is_xml_name(&name) {
            graph.set_xml_root_name(id, name);
        } else {
            Diagnostic::warning(Code::XmlHintIgnored, at)
                .message(format!(
                    "component name `{name}` is not an XML name, so the element name OpenAPI \
                     infers from it is not applied: an XML body serializes this struct under its \
                     generated Rust type name instead"
                ))
                .remedy(
                    "give the schema a root `xml.name` that is an XML name (a letter, `_` or `:` \
                     first, then letters, digits, `-`, `.`, `_` or `:`), or rename the component",
                )
                .emit(diags);
        }
    }
    // OpenAPI 3.2 gives a `$ref` the node type `none`: it names no element, and the referenced
    // schema's own element stands in its place — named by its root `xml.name`, or else by its
    // component name. So a property that is a `$ref` to a struct, or an array property whose items
    // are, takes that struct's XML root name as its field name, now that the inferred names above
    // are set. OpenAPI 3.1 has no node types and names a property's element after the property, so
    // there the field keeps its name (an array's items still follow an explicit root `xml.name`,
    // above). A name the property or its items give themselves wins, and, as above, only an owner
    // some XML body reaches takes it; on one a non-XML body also reaches it is suppressed below.
    if is_oas32 {
        let view: &TypeGraph = graph;
        let ref_names: Vec<(TypeId, usize, String)> = view
            .emitted()
            .filter(|(id, _)| xml_reachable.contains(id))
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
                    .filter(|(_, field)| {
                        field.xml.by_reference && field.xml.name.is_none() && !field.xml.attribute
                    })
                    .filter_map(move |(index, field)| {
                        // An array's items are each its element; anything else is the element.
                        let element = if let TypeKind::Array(item) = &view.get(field.ty.id)?.kind {
                            item.id
                        } else {
                            field.ty.id
                        };
                        Some((id, index, view.xml_root_name(element)?.to_owned()))
                    })
            })
            .collect();
        for (id, index, name) in ref_names {
            if let Some(TypeKind::Struct(object)) = graph.get_mut(id).map(|def| &mut def.kind) {
                object.fields[index].xml.name = Some(name);
                derived.insert((id, index));
            }
        }
    }
    // A derived name can give two child elements of one struct the same name: two properties
    // referencing one component, or a reference whose element name is another property's. serde
    // would then read both fields from one element name, which quick-xml refuses as a duplicate
    // field, so each derived name that clashes with a sibling's element name is withdrawn and
    // reported (`W006`), and the field keeps its property name. Withdrawing one can expose another
    // clash with a property name, so this runs until no derived name clashes. An owner a non-XML
    // body also reaches is left to the suppression below, which withholds every rename on it.
    let mut owners: Vec<TypeId> = derived
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !non_xml_reachable.contains(id))
        .collect();
    owners.sort_unstable();
    owners.dedup();
    for id in owners {
        let mut withdrawn: Vec<(String, String)> = Vec::new();
        while let Some(TypeKind::Struct(object)) = graph.get_mut(id).map(|def| &mut def.kind) {
            let element = |field: &Field| {
                field
                    .xml
                    .name
                    .clone()
                    .unwrap_or_else(|| field.name.wire.clone())
            };
            let mut counts: HashMap<String, usize> = HashMap::new();
            for field in object.fields.iter().filter(|field| !field.xml.attribute) {
                *counts.entry(element(field)).or_default() += 1;
            }
            let clashing: Vec<usize> = (0..object.fields.len())
                .filter(|index| derived.contains(&(id, *index)))
                .filter(|index| {
                    let field = &object.fields[*index];
                    field.xml.name.is_some() && counts[&element(field)] > 1
                })
                .collect();
            if clashing.is_empty() {
                break;
            }
            for index in clashing {
                let field = &mut object.fields[index];
                if let Some(name) = field.xml.name.take() {
                    withdrawn.push((field.name.wire.clone(), name));
                }
                derived.remove(&(id, index));
            }
        }
        if withdrawn.is_empty() {
            continue;
        }
        let Some(def) = graph.get(id) else {
            continue;
        };
        let fields: Vec<String> = withdrawn
            .iter()
            .map(|(property, name)| format!("property `{property}` (`{name}`)"))
            .collect();
        Diagnostic::warning(
            Code::XmlHintIgnored,
            meet_locations.get(&id).unwrap_or(&def.provenance).clone(),
        )
        .message(format!(
            "the element name derived from the referenced schema is not applied to {}: it is \
             the name of another child element of this struct, and two fields read from one \
             element name cannot be told apart, so each keeps its property name as its element \
             name",
            fields.join(", ")
        ))
        .remedy(
            "give the properties distinct `xml.name`s, or give the referenced schemas distinct \
             root `xml.name`s",
        )
        .emit(diags);
    }
    for (_, root, target) in ref_sites {
        unsupported_reports.push((
            xml_reachable.contains(&target),
            root.at.clone(),
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
            // E009 case: xml-hint
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
    // suppressing its hint changes what the XML body puts on the wire. Both are one `W006` each,
    // so the message carries the distinction.
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
        // A name derived from a referenced schema is no hint the author wrote, so the message
        // names it as derived, and names the authored hints only where some field carries one.
        let mut authored = false;
        let mut derived_names: Vec<String> = Vec::new();
        if let TypeKind::Struct(object) = &mut def.kind {
            for (index, field) in object.fields.iter_mut().enumerate() {
                match &field.xml.name {
                    Some(name) if derived.contains(&(id, index)) => {
                        derived_names.push(format!("property `{}` (`{name}`)", field.name.wire))
                    }
                    Some(_) => authored = true,
                    None => authored |= field.xml.attribute,
                }
                field.xml = XmlField::default();
            }
        }
        let shared_remedy = "declare a separate schema for the XML body if the rename is \
                             required, so the two bodies stop sharing one generated type, or \
                             accept the property's normal wire name";
        let (message, remedy) = if !shared_with_xml {
            (
                "`xml.name`/`xml.attribute` not applied: this schema is never used as an XML body, \
                 so the hint cannot affect any wire format; the field keeps its normal wire name"
                    .to_owned(),
                "remove the `xml` hint, or use this schema as an XML body if the rename is \
                 required",
            )
        } else if derived_names.is_empty() {
            (
                "`xml.name`/`xml.attribute` not applied: this schema is shared between an XML body \
                 and a non-XML (e.g. JSON) body, and a serde rename applies to every format, so \
                 honoring the hint would rewrite the JSON wire name too. The field keeps its \
                 normal wire name — including in the XML body, whose element/attribute name is the \
                 property name rather than the hint"
                    .to_owned(),
                shared_remedy,
            )
        } else {
            let hints = if authored {
                "`xml.name`/`xml.attribute`, and "
            } else {
                ""
            };
            (
                format!(
                    "{hints}the element name derived from the referenced schema for {} not \
                     applied: this schema is shared between an XML body and a non-XML (e.g. JSON) \
                     body, and a serde rename applies to every format, so applying it would \
                     rewrite the JSON wire name too. The field keeps its normal wire name — \
                     including in the XML body, whose element name is the property name rather \
                     than the derived one",
                    derived_names.join(", ")
                ),
                shared_remedy,
            )
        };
        Diagnostic::warning(Code::XmlHintIgnored, provenance)
            .message(message)
            .remedy(remedy)
            .emit(diags);
    }
}
