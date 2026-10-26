//! Lowering a typed OpenAPI 3.1 / 3.2 [`Document`] into the version-agnostic [`Api`] IR.
//!
//! This module holds the pass driver ([`lower`], [`lower_pass`]) and the lowering context
//! [`LowerCtx`] with the types its passes share. Each child module adds the `LowerCtx` methods and
//! free helpers of one responsibility; a cross-module call is `pub(super)`, and everything else
//! stays private to the module that uses it.

mod all_of;
mod body;
mod collapse;
mod combine;
mod content;
mod cycle;
mod defaults;
mod discriminator;
mod encoding;
mod meet;
mod narrowing;
mod nullability;
mod object;
mod parameter;
mod prune;
mod reference;
mod refiner;
mod reject;
mod reserve;
mod response;
mod schema;
mod security;
mod server;
mod shape;
mod strategy;
mod union;
mod xml;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{
    Api, Docs, Info, JsonCategory, Method, Operation, OperationId, ParamLoc, PathSegment, SchemeId,
    SecuritySchemeDef, Ty, TypeGraph, TypeId,
};
use crate::name::synth_operation_id;

use super::{
    Document, JsonType, OperationObject, ParameterObject, PathItem, Resolver, ResponseObject,
    Schema,
};

use combine::{Gathering, RecordedContribution};
use defaults::retype_field_defaults;
use reference::resolve_path_item;
use security::{
    lower_security_requirement, lower_security_schemes, resolve_external_security_schemes,
};
use server::{lower_server, lower_server_override, parse_path_template};
use xml::{gate_xml_field_renames, RootXml, XmlSite};

/// Maximum schema-lowering recursion depth. Each nested object property, array item,
/// `allOf`/`oneOf`/`anyOf` member, and resolved `$ref` target descends one level through
/// [`LowerCtx::lower_schema`]. Inline nesting is already bounded by the parser's own depth cap, but
/// a chain of components (or remote refs) that each `$ref` the next is parsed shallowly and would
/// otherwise recurse without bound — a long enough chain overflows the stack. This cap stops that
/// descent and rejects with `E014` (`SchemaNestingTooDeep`) instead of crashing. It is far above any
/// real API's nesting depth; the whole frontend runs on a dedicated large-stack thread (see the
/// facade) so lowering this many levels deep is comfortably safe.
const MAX_SCHEMA_DEPTH: u32 = 128;

/// The identity of a schema the bundle resolver produced: the `file#pointer` it was parsed from,
/// read off the parsed schema's own provenance rather than off the `$ref` spelling that reached it.
///
/// This is what lets one target have one type however it is addressed — a sub-file's bare
/// `#/components/schemas/Inner` and the root's `./lib.yaml#/components/schemas/Inner` resolve to the
/// same file and the same pointer and therefore to the same key. `None` only when the target
/// carries no span, which no parser output does; callers treat that as "no identity" and fall back.
fn resolved_identity(provenance: &Provenance) -> Option<String> {
    let file = provenance.span?.file;
    Some(format!("{}#{}", file.0, provenance.pointer.as_str()))
}

/// `decide` of `target`, read once per [`resolved_identity`] in `memo` and replayed at every later
/// use. `provisional` is recorded before `decide` runs, so a walk that loops back to `target` reads
/// it there and ends. A target with no span has no identity to key on and is decided un-memoised;
/// the caller's depth bound still bounds it.
fn memoised_answer<T: Copy>(
    memo: &RefCell<HashMap<String, T>>,
    target: &Schema,
    provisional: T,
    decide: impl FnOnce() -> T,
) -> T {
    let Some(key) = resolved_identity(&target.provenance) else {
        return decide();
    };
    if let Some(&decides) = memo.borrow().get(&key) {
        return decides;
    }
    memo.borrow_mut().insert(key.clone(), provisional);
    let decides = decide();
    memo.borrow_mut().insert(key, decides);
    decides
}

/// The name hint a resolved target should carry: its own final pointer token, so the generated type
/// is named for the schema it came from rather than for whichever use site happened to reach it
/// first. Empty for a whole-file reference, which has no final token; the caller's hint stands then.
///
/// The token is unescaped (RFC 6901 `~1` → `/`, `~0` → `~`) before it becomes a hint. Component
/// *keys* are constrained by the official schema to `^[a-zA-Z0-9._-]+$` and could never carry an
/// escape, but this function exists partly to serve pointers that are not component keys — a
/// property name, a path template — and those are unconstrained. `name` sanitises and disambiguates
/// whatever it is given, so the consequence of leaving it escaped is cosmetic, but the result is a
/// public type name in the generated API and `~1` in one is a spelling nobody chose.
fn resolved_hint(provenance: &Provenance, fallback: &str) -> String {
    provenance
        .pointer
        .tokens()
        .pop()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Lower a typed OpenAPI 3.1 or 3.2 [`Document`] into the version-agnostic [`Api`] IR.
///
/// Lowering runs as one or more whole passes over the document, and only the last one's IR and
/// diagnostics are kept. A back-edge — a `$ref` taken while its target's body is still being
/// lowered — has to be typed before that body has decided whether the target is nullable, so it is
/// typed from a reserve-time guess ([`LowerCtx::null_admission`] of the body, which reads every
/// composition the body spells but cannot read the answer of a body still being lowered, so a `$ref`
/// loop decides nothing in it). When the body then decides otherwise,
/// every field that took the back-edge disagrees with every field that referenced the finished
/// component, and the one that took it cannot decode a value its schema admits (issue #222). A pass
/// that found such a disagreement is discarded and the document is lowered again with the body's
/// answer settled for that reservation, so the back-edge reads what a finished reference reads.
///
/// Each extra pass settles at least one reservation it had not settled before, and a settled value
/// is never revised, so the loop ends within one pass per reservation plus one. A document with no
/// such back-edge — every one without a recursive nullable component — is lowered exactly once.
pub(crate) fn lower(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
    options: LowerOptions,
) -> Result<Api, Aborted> {
    let mut settled = HashMap::new();
    loop {
        let mut pass = diags.clone();
        let (api, revisions) = lower_pass(document, resolver, &mut pass, &settled, options);
        let mut changed = false;
        for (reservation, nullable) in revisions {
            if let std::collections::hash_map::Entry::Vacant(entry) = settled.entry(reservation) {
                entry.insert(nullable);
                changed = true;
            }
        }
        if !changed {
            *diags = pass;
            return api;
        }
    }
}

/// One reservation's identity, in whichever of the three memos holds it: a root component by name,
/// a remote target by absolute `url#fragment`, a bundle target by resolved `file#pointer`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Reservation {
    Component(String),
    Remote(String),
    Resolved(String),
}

impl Reservation {
    /// The key both of its frame's memos hold it under: the name, the URL, or the `file#pointer`.
    fn key(&self) -> &str {
        match self {
            Self::Component(key) | Self::Remote(key) | Self::Resolved(key) => key,
        }
    }

    /// What its frame calls the root it reserves, in the invariant messages.
    fn noun(&self) -> &'static str {
        match self {
            Self::Component(_) => "component",
            Self::Remote(_) => "remote",
            Self::Resolved(_) => "resolved",
        }
    }
}

/// One lowering pass. `settled` holds the nullability an earlier pass's bodies decided for the
/// reservations whose back-edges read a guess that turned out wrong; the second value returned is
/// every such reservation this pass found (see [`lower`]).
fn lower_pass(
    document: &Document,
    resolver: &Resolver,
    diags: &mut Diagnostics,
    settled: &HashMap<Reservation, bool>,
    options: LowerOptions,
) -> (Result<Api, Aborted>, Vec<(Reservation, bool)>) {
    let mut security_schemes = lower_security_schemes(document, diags);
    // OpenAPI 3.2 lets a security requirement name a Security Scheme Object by URI instead of by
    // component name. A component name always wins — the specification is explicit that name
    // lookup takes precedence, and flags the resulting hijack risk — so only names that match no
    // declared component are resolved as references. A leading `./` forces the URI reading for a
    // single-segment name that would otherwise collide.
    resolve_external_security_schemes(document, resolver, &mut security_schemes, diags);
    let mut ctx = LowerCtx {
        document,
        resolver,
        diags,
        graph: TypeGraph::default(),
        components: HashMap::new(),
        in_progress: HashMap::new(),
        component_alias_stack: HashSet::new(),
        remote_components: HashMap::new(),
        remote_in_progress: HashMap::new(),
        remote_alias_stack: HashSet::new(),
        resolved_components: HashMap::new(),
        resolved_in_progress: HashMap::new(),
        resolved_alias_stack: HashSet::new(),
        resolved_contributions: HashMap::new(),
        resolved_member_stack: Vec::new(),
        union_member_stack: Vec::new(),
        gatherings: Vec::new(),
        wraps_union_memo: RefCell::new(HashMap::new()),
        null_admission_memo: RefCell::new(HashMap::new()),
        settled,
        guessed: HashSet::new(),
        revisions: Vec::new(),
        depth: 0,
        open_narrowing: options.open_narrowing,
        narrowing_opens: false,
        open_candidates: HashSet::new(),
        meet_locations: HashMap::new(),
        root_xml: HashMap::new(),
        field_xml_sites: HashSet::new(),
        xml_holders: Vec::new(),
        xml_mergers: HashMap::new(),
        unmerged_union: None,
        unmerged_union_meets_null: false,
        held_untyped_warnings: None,
        scalar_meet_union: None,
        scalar_meet_unmerged: HashSet::new(),
        union_variant_members: HashMap::new(),
        variant_meets: Vec::new(),
        deferred_map_values: Vec::new(),
    };

    // These names come from `components.schemas` itself, so the lookup inside cannot miss and the
    // provenance is never used for a rejection; the document root is the only site there is.
    for name in document.components.schemas.keys() {
        let _ = ctx.ensure_component(name, None, &document.provenance);
    }

    let mut operations = Vec::new();
    let mut operation_ids = HashSet::new();
    for (path, item) in &document.paths.items {
        let Some(item) = resolve_path_item(item, resolver, ctx.diags) else {
            continue;
        };
        for (method, operation) in &item.operations {
            operations.push(lower_operation(
                &mut ctx,
                path,
                &item,
                method,
                operation,
                &security_schemes,
                &mut operation_ids,
            ));
        }
    }

    // Every reservation is filled now, so the map-value comparisons a reservation held back can
    // be decided.
    ctx.check_deferred_map_values();

    // Every schema is lowered, so what neither a component nor an operation reaches is a type
    // the output would carry with nothing naming it (#924). Withheld before the passes below,
    // which read and report on the emitted definitions only.
    ctx.elide_unreachable(&operations);

    // An intersection narrows a field's type after its `default` was checked against the type the
    // declaring member gave it, so the applied defaults are checked again against the final graph.
    retype_field_defaults(&mut ctx.graph, &ctx.meet_locations, ctx.diags);

    // A field's `xml.name`/`xml.attribute` become a format-agnostic serde `rename`, so they may only
    // be applied to a schema used *exclusively* as an XML body — otherwise the rename would corrupt
    // the JSON wire format. Suppress (and warn `W006` on) the rename for any shared/non-XML-reachable
    // type, apply each root `xml.name`, and disposition every hint the codec cannot honour.
    gate_xml_field_renames(
        &mut ctx.graph,
        &operations,
        &ctx.meet_locations,
        &ctx.root_xml,
        &ctx.field_xml_sites,
        &ctx.xml_mergers,
        document.is_oas32,
        ctx.diags,
    );

    let servers = document
        .servers
        .iter()
        .filter_map(|server| lower_server(server, ctx.diags))
        .collect();
    let revisions = std::mem::take(&mut ctx.revisions);
    let api = Api {
        info: Info {
            title: document.info.title.clone(),
            version: document.info.version.clone(),
            description: api_description(document),
        },
        servers,
        operations,
        types: ctx.graph,
        security_schemes,
    };
    (ctx.diags.result(api), revisions)
}

/// Lower the `operation` declared under `method` on the path item `item` of the template `path`:
/// its merged parameters, request body, responses, security requirements, documentation and
/// server override. Its id joins `operation_ids`, and a duplicate, a malformed parameter set, a
/// response it cannot decode where it stands, or a security requirement naming no lowered scheme
/// in `security_schemes` is reported; the operation is returned regardless, and the pass's
/// result carries the rejection.
fn lower_operation(
    ctx: &mut LowerCtx<'_, '_>,
    path: &str,
    item: &PathItem,
    method: &Method,
    operation: &OperationObject,
    security_schemes: &IndexMap<SchemeId, SecuritySchemeDef>,
    operation_ids: &mut HashSet<String>,
) -> Operation {
    let path_template = parse_path_template(path);
    let id = operation
        .operation_id
        .clone()
        .unwrap_or_else(|| synth_operation_id(method, &path_template));
    if !operation_ids.insert(id.clone()) {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(format!(
                "operationId `{id}` is not unique within the API description"
            ))
            .emit(ctx.diags);
    }

    let mut params = Vec::new();
    let mut merged_parameters: IndexMap<(String, String), ParameterObject> = IndexMap::new();
    for parameter in &item.parameters {
        if let Some(parameter) = ctx.resolve_parameter(parameter) {
            let key = (parameter.location.clone(), parameter.name.clone());
            if merged_parameters.insert(key, parameter).is_some() {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message("path-item parameters contain a duplicate name/location pair")
                    .emit(ctx.diags);
            }
        }
    }
    let mut operation_parameter_keys = HashSet::new();
    for parameter in &operation.parameters {
        if let Some(parameter) = ctx.resolve_parameter(parameter) {
            let key = (parameter.location.clone(), parameter.name.clone());
            if !operation_parameter_keys.insert(key.clone()) {
                Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
                    .message("operation parameters contain a duplicate name/location pair")
                    .emit(ctx.diags);
            }
            // Operation-level parameters override the matching path-item parameter.
            merged_parameters.insert(key, parameter);
        }
    }
    for parameter in merged_parameters.values() {
        if let Some(parameter) = ctx.lower_parameter(parameter) {
            params.push(parameter);
        }
    }
    let placeholders: HashSet<&str> = path_template
        .segments
        .iter()
        .filter_map(|segment| match segment {
            PathSegment::Param(name) => Some(name.as_str()),
            PathSegment::Literal(_) => None,
        })
        .collect();
    let path_parameters: HashSet<&str> = params
        .iter()
        .filter(|parameter| parameter.location == ParamLoc::Path)
        .map(|parameter| parameter.name.as_str())
        .collect();
    if placeholders != path_parameters {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(format!(
                "path template parameters {placeholders:?} do not match declared path \
                         parameters {path_parameters:?}"
            ))
            .emit(ctx.diags);
    }
    let querystrings = params
        .iter()
        .filter(|parameter| parameter.location == ParamLoc::QueryString)
        .count();
    let named_queries = params
        .iter()
        .any(|parameter| parameter.location == ParamLoc::Query);
    if querystrings > 1 || (querystrings == 1 && named_queries) {
        Diagnostic::error(Code::InvalidInput, operation.provenance.clone())
            .message(
                "an operation may declare at most one `in: querystring` parameter and may \
                         not combine it with `in: query` parameters",
            )
            .emit(ctx.diags);
    }

    let request_body = operation
        .request_body
        .as_ref()
        .and_then(|body| ctx.resolve_request_body(body))
        .and_then(|body| ctx.lower_request_body(&body));

    let responses = ctx.lower_responses(&operation.responses);
    // XML decode is scoped to the single-body success/error paths. An XML body beside a
    // second bodied status on the same side — a response enum that decodes two or more
    // bodies — is rejected cleanly (narrowed `E009`) rather than silently decoded as JSON.
    // A lone XML success body beside a bodyless status is still that one body.
    if responses.xml_in_multi_status() {
        // E009 case: xml-multi-body
        Diagnostic::error(Code::UnsupportedMediaType, operation.provenance.clone())
            .message(
                "an application/xml (or text/xml) response body is only supported as an \
                         operation's single bodied success or single bodied error response \
                         (bodyless statuses beside it are fine); it cannot share a response enum \
                         with a second bodied success or error status",
            )
            .remedy(
                "give the operation a single XML-bodied success/error response, use JSON \
                         for the other bodied responses, or omit this API segment with \
                         spargen::omit!",
            )
            .emit(ctx.diags);
    }
    // Streaming decode is scoped to the single bodied success (`EventStream<T>`). A stream
    // anywhere else — an error status, a `default` (which is always offered to the error
    // side), or a success enum beside a second bodied success — would be decoded as one
    // whole JSON body, so it is rejected (narrowed `E009`) rather than misread on the wire.
    if responses.stream_outside_single_success() {
        // E009 case: streaming-response-position
        Diagnostic::error(Code::UnsupportedMediaType, operation.provenance.clone())
                    .message(
                        "a streaming (text/event-stream, application/x-ndjson, or JSON Text \
                         Sequence) response body is only supported as an operation's single success body; on an error \
                         status, on a `default` response (which also documents error statuses), \
                         or beside a second bodied success status it would be decoded as one \
                         whole body",
                    )
                    .remedy(
                        "declare the stream under an explicit 2xx status as the operation's only \
                         bodied success, document error bodies with a whole-body media type, or \
                         omit this API segment with spargen::omit!",
                    )
                    .emit(ctx.diags);
    }

    let security: Vec<crate::ir::SecurityRequirement> = operation
        .security
        .as_ref()
        .unwrap_or(&ctx.document.security)
        .iter()
        .map(lower_security_requirement)
        .collect();
    // Codegen builds per-operation credential tables from the scheme map, so every
    // referenced scheme must have lowered; an undeclared or unsupported scheme would
    // otherwise silently generate an unauthenticated call.
    for requirement in &security {
        for (scheme, _) in &requirement.0 {
            if !security_schemes.contains_key(scheme) {
                Diagnostic::error(Code::UnknownSecurityScheme, operation.provenance.clone())
                    .message(format!(
                        "security requirement references undeclared or unsupported \
                             scheme `{}`",
                        scheme.0
                    ))
                    .remedy(
                        "declare the scheme under components.securitySchemes as http \
                             bearer/basic, apiKey, oauth2, openIdConnect, or mutualTLS",
                    )
                    .emit(ctx.diags);
            }
        }
    }

    let mut operation_description = operation.description.clone();
    // A Path Item's `summary`/`description` apply to every operation on the path. They are
    // additional context rather than a replacement, so the operation's own documentation
    // stays first and these follow it.
    for text in [item.summary.as_ref(), item.description.as_ref()]
        .into_iter()
        .flatten()
    {
        append_text(&mut operation_description, text.clone());
    }
    if !operation.tags.is_empty() {
        append_text(
            &mut operation_description,
            format!("Tags: {}.", operation.tags.join(", ")),
        );
    }
    for (status, response) in &operation.responses.by_status {
        if let Some(response) = ctx.resolve_response(response) {
            append_response_docs(&mut operation_description, status, &response);
        }
    }
    if let Some(response) = operation
        .responses
        .default
        .as_ref()
        .and_then(|response| ctx.resolve_response(response))
    {
        append_response_docs(&mut operation_description, "default", &response);
    }

    // Operation `servers` override the path item's, which override the document's. The
    // document's is the client's base URL, so "no override" is the common case.
    let server = if operation.servers.is_empty() {
        lower_server_override(&item.servers, ctx.diags)
    } else {
        lower_server_override(&operation.servers, ctx.diags)
    };

    Operation {
        id: OperationId(id),
        method: method.clone(),
        path: path_template,
        params,
        request_body,
        responses,
        security,
        deprecated: operation.deprecated,
        docs: Docs {
            title: None,
            summary: operation.summary.clone(),
            description: operation_description,
            deprecated: operation.deprecated,
        },
        server,
        provenance: operation.provenance.clone(),
    }
}

/// The API's own documentation: `info`'s summary and description, its contact, license and
/// external documentation, and the document's tags.
fn api_description(document: &Document) -> Option<String> {
    let mut api_description = document.info.summary.clone();
    if let Some(description) = &document.info.description {
        append_text(&mut api_description, description.clone());
    }
    if let Some(contact) = &document.info.contact {
        append_text(&mut api_description, format!("Contact: {contact}."));
    }
    if let Some(license) = &document.info.license {
        append_text(&mut api_description, format!("License: {license}."));
    }
    if let Some(external_docs) = &document.info.external_docs {
        append_text(&mut api_description, format!("See also: {external_docs}."));
    }
    if !document.tags.is_empty() {
        let tags = document
            .tags
            .iter()
            .map(|tag| {
                let mut label = tag.name.clone();
                if let Some(summary) = &tag.summary {
                    label.push_str(": ");
                    label.push_str(summary);
                }
                if let Some(description) = &tag.description {
                    label.push_str(" — ");
                    label.push_str(description);
                }
                if let Some(parent) = &tag.parent {
                    label.push_str(&format!(" (parent: {parent})"));
                }
                if let Some(kind) = &tag.kind {
                    label.push_str(&format!(" [{kind}]"));
                }
                label
            })
            .collect::<Vec<_>>()
            .join("; ");
        append_text(&mut api_description, format!("Tags: {tags}."));
    }
    api_description
}

fn append_text(target: &mut Option<String>, text: String) {
    match target {
        Some(target) if !target.is_empty() => {
            target.push_str("\n\n");
            target.push_str(&text);
        }
        Some(target) => *target = text,
        None => *target = Some(text),
    }
}

fn append_response_docs(target: &mut Option<String>, status: &str, response: &ResponseObject) {
    let mut docs = response.summary.clone();
    if let Some(description) = &response.description {
        append_text(&mut docs, description.clone());
    }
    if let Some(docs) = docs {
        append_text(target, format!("Response `{status}`: {docs}"));
    }
}

/// A union's shape-bearing sibling: what each branch is met with, plus whether the sibling's own
/// keywords say anything about `null`.
///
/// The second field exists because the lowered type cannot answer it. A `properties`-only sibling
/// and a `type: object` + `properties` sibling both refine objects, yet only the second denies
/// `null` — the first is an object applicator, vacuously satisfied by every non-object. The
/// question has to be asked of the schema, and asked of the SIBLING rather than of the schema that
/// encloses it: those differ whenever a multi-type array is deleted for lowering, and whenever the
/// sibling speaks through `enum`/`const` instead of `type`.
#[derive(Clone, Copy)]
struct UnionSibling {
    refiner: Refiner,
    speaks_about_null: bool,
}

/// What a union's branches are met with: a union's own sibling keywords, or the sibling keywords
/// of a `$ref` whose target is a union.
#[derive(Clone, Copy)]
enum Refiner {
    /// A sibling that establishes a shape of its own (`type`, `enum`, `const`, `$ref`, `allOf`, a
    /// binary encoding): every branch is intersected with it.
    Whole(Ty),
    /// A sibling of untyped object or array applicators alone (see
    /// [`implied_applicator_category`]). In 2020-12 those are vacuously satisfied by an instance
    /// of another category, so each set refines only the branches of its own category (#282).
    ///
    /// [`implied_applicator_category`]: refiner::implied_applicator_category
    Scoped(ScopedRefiners),
}

/// The two halves of a [`Refiner::Scoped`] sibling, each lowered as its category with `null`
/// admitted where the sibling does not deny it.
#[derive(Clone, Copy)]
struct ScopedRefiners {
    /// The object applicators (`properties`, `patternProperties`, `required`,
    /// `additionalProperties`), lowered as an object: met with every object branch.
    object: Option<Ty>,
    /// The array applicators (`items`, `prefixItems`), lowered as an array: met with every array
    /// branch.
    array: Option<Ty>,
    /// Whether the sibling admits `null`. The applicators say nothing about it, so this is false
    /// only where a multi-type array deleted for lowering omitted `null`; a branch neither half
    /// reaches still loses its `null` then, as it would against the deleted array.
    admits_null: bool,
    /// The categories a multi-type array deleted for lowering admits, where there was one: a
    /// branch of any other category is excluded, as it would be against the array.
    allowed: Option<CategoryMask>,
}

/// A set of JSON categories, the non-null members of a `type` array. `integer` and `number` both
/// admit [`JsonCategory::Number`], the category a lowered numeric branch reports.
#[derive(Clone, Copy)]
struct CategoryMask(u8);

impl CategoryMask {
    fn of(types: &[JsonType]) -> Self {
        Self(types.iter().fold(0, |mask, kind| {
            mask | match kind {
                JsonType::Null => 0,
                JsonType::Boolean => Self::bit(JsonCategory::Boolean),
                JsonType::Object => Self::bit(JsonCategory::Object),
                JsonType::Array => Self::bit(JsonCategory::Array),
                JsonType::Number | JsonType::Integer => Self::bit(JsonCategory::Number),
                JsonType::String => Self::bit(JsonCategory::String),
            }
        }))
    }

    fn bit(category: JsonCategory) -> u8 {
        match category {
            JsonCategory::String => 1,
            JsonCategory::Number => 2,
            JsonCategory::Boolean => 4,
            JsonCategory::Array => 8,
            JsonCategory::Object => 16,
        }
    }

    fn admits(self, category: JsonCategory) -> bool {
        self.0 & Self::bit(category) != 0
    }

    /// Whether the set admits a category other than `category`.
    fn admits_besides(self, category: JsonCategory) -> bool {
        self.0 & !Self::bit(category) != 0
    }

    /// Whether the set names a category, and only categories `outer` admits too.
    fn is_within(self, outer: Self) -> bool {
        self.0 != 0 && self.0 & !outer.0 == 0
    }
}

/// What a [`Refiner::Scoped`] meeting found across the branches it visited: which halves reached
/// a branch of their category, and whether both halves met a branch that states no category, which
/// leaves no single category to establish for it.
#[derive(Default)]
struct ScopeReach {
    object: bool,
    array: bool,
    uncategorised: bool,
}

/// A `W001` an inline `oneOf` raised for its variants that lower to `serde_json::Value`
/// ([`LowerCtx::warn_untyped_one_of_variants`]), held back while an `allOf` gathers its members
/// ([`LowerCtx::held_untyped_warnings`]): where the union is a member the `allOf`'s scalar meet
/// replaces, the warning names only the variants the meet carries as `Value` (#660, #675). It is
/// reported as raised where the gathering or the meet fails, where the meet does not replace the
/// union (its lowering is still named, or the meet is a reservation), and for every warning a
/// union other than such a member raised.
#[derive(Clone, Debug)]
struct HeldUntypedWarning {
    /// The union the warning is raised at.
    provenance: Provenance,
    /// Each `Value` variant's hint, beside its member position for the message.
    untyped: Vec<(String, usize)>,
}

/// The three spellings of one conjunction a `oneOf`/`anyOf` takes part in, each met with the
/// union branch by branch and then collapsed by [`LowerCtx::collapse_met_union`]: a `$ref` with
/// the union as its sibling, an `allOf` with the union beside it on one schema (#419), and an
/// `allOf` with the union as one of its members (#463). The spelling decides three things:
///
/// * the wording of the diagnostics the meet reports ([`Self::subject`], and the messages
///   [`LowerCtx::reject_all_of_union_meet`] and [`LowerCtx::meet_union_with_all_of`] pick), with
///   the `$ref` remedy rather than the `allOf` one for [`MetUnion::RefSibling`];
/// * the hint the meet is inserted under, and so the names its generated types take
///   ([`Self::meet_suffix`]): `ReferenceIntersection` for a `$ref` sibling, `Intersection` for
///   either `allOf` spelling;
/// * which conjunct a branch the meet excludes is read against: the `allOf`'s other members for
///   [`MetUnion::AllOfMember`], the schema's own sibling keywords for the other two.
#[derive(Clone, Copy)]
enum MetUnion {
    RefSibling,
    BesideAllOf,
    AllOfMember,
}

impl MetUnion {
    /// The subject of the collapse warnings: what was intersected with what. `one_of_only` names
    /// the union `oneOf` alone, for the partial merge only a `oneOf` takes.
    fn subject(self, one_of_only: bool) -> &'static str {
        match (self, one_of_only) {
            (MetUnion::RefSibling, false) => "this `$ref` and its `oneOf`/`anyOf` sibling",
            (MetUnion::RefSibling, true) => "this `$ref` and its `oneOf` sibling",
            (MetUnion::BesideAllOf, false) => {
                "this schema's `allOf` and the `oneOf`/`anyOf` beside it"
            }
            (MetUnion::BesideAllOf, true) => "this schema's `allOf` and the `oneOf` beside it",
            (MetUnion::AllOfMember, false) => {
                "this `allOf`'s `oneOf`/`anyOf` member and its other members"
            }
            (MetUnion::AllOfMember, true) => "this `allOf`'s `oneOf` member and its other members",
        }
    }

    /// The suffix of the hint the meet with the union is inserted under, and so of the names its
    /// branches take.
    fn meet_suffix(self) -> &'static str {
        match self {
            MetUnion::RefSibling => "ReferenceIntersection",
            MetUnion::BesideAllOf | MetUnion::AllOfMember => "Intersection",
        }
    }
}

/// A union's Discriminator Object resolved against the union's own members: every `mapping`
/// entry's tag with the index (into the union's real, non-null members) of the member it names, in
/// document order, and the member `defaultMapping` names. Built by
/// [`LowerCtx::discriminator_members`], which rejects any entry naming a schema that does not
/// exist or is not a member, so every index here is a member.
struct DiscriminatorMembers {
    mapping: Vec<(String, usize)>,
    default: Option<usize>,
}

struct LowerCtx<'a, 'doc> {
    document: &'doc Document,
    resolver: &'a Resolver<'doc>,
    diags: &'a mut Diagnostics,
    graph: TypeGraph,
    /// Lowered components, mapped to their root id and nullability. Nullability is carried so a
    /// `$ref` consumer wraps the type in `Option` when the component itself is nullable (a
    /// `"null"` in its type array, or a `null` enum/const member) — otherwise a null-mixed enum
    /// used via `$ref` would emit a non-`Option` field that rejects a conforming `null` payload.
    components: HashMap<String, (TypeId, bool)>,
    /// Components currently being lowered, mapped to the id reserved for their root and their
    /// provisional nullability (an earlier pass's settled answer, else a reserve-time guess from
    /// the schema). A `$ref` that re-enters a name still in this map is a cycle-closing back-edge
    /// and is boxed against the reserved id; a guess it read that the body contradicts is recorded
    /// in [`Self::revisions`], and [`lower`] lowers again until each back-edge carries the same
    /// nullability a completed lowering does.
    in_progress: HashMap<String, (TypeId, bool)>,
    /// Guards chains of component aliases (`A -> B -> A`) that do not have a concrete schema body
    /// to enter the normal reserve/box recursion path.
    component_alias_stack: HashSet<String>,
    /// The remote-`$ref` analogue of [`Self::components`], keyed by the absolute `url#fragment`. A
    /// remote ref resolves to a fresh owned schema each call, so — unlike local components — it has
    /// no `document`-level identity; this map gives it one, so repeated remote uses share one
    /// generated type and, together with [`Self::remote_in_progress`], recursion terminates.
    remote_components: HashMap<String, (TypeId, bool)>,
    /// Remote refs currently being lowered (same role as [`Self::in_progress`] for components): a
    /// re-entered `url#fragment` is a cycle-closing back-edge and is boxed against its reserved id.
    remote_in_progress: HashMap<String, (TypeId, bool)>,
    /// Guards a chain of bare-`$ref` (alias) remote documents so an alias cycle terminates instead
    /// of recursing forever; a real (object/enum/…) remote schema uses the reserve/box machinery.
    remote_alias_stack: HashSet<String>,
    /// The bundle-`$ref` analogue of [`Self::components`], keyed by the resolved target's own
    /// `file#pointer` (see [`resolved_identity`]). `Resolver::resolve` parses a fresh owned schema
    /// on every call, so — exactly as for a remote ref — a relative-file reference and a sub-file's
    /// own `#/components/schemas/<name>` have no `document`-level identity of their own. This map
    /// gives them one, so repeated uses of one target share one generated type instead of producing
    /// a fresh type per reference site.
    resolved_components: HashMap<String, (TypeId, bool)>,
    /// Bundle refs currently being lowered (same role as [`Self::in_progress`]): a re-entered
    /// `file#pointer` is a cycle-closing back-edge and is boxed against its reserved id, so a
    /// recursive sub-file schema terminates and generates rather than walking to
    /// [`MAX_SCHEMA_DEPTH`] and rejecting.
    resolved_in_progress: HashMap<String, (TypeId, bool)>,
    /// Guards a chain of bare-`$ref` (alias) bundle targets, which have no body to reserve a root
    /// against; the counterpart of [`Self::remote_alias_stack`].
    resolved_alias_stack: HashSet<String>,
    /// What a bundle-`$ref` `allOf` member contributes, keyed by the resolved target's own
    /// `file#pointer` (see [`resolved_identity`]): the [`Self::resolved_components`] analogue for the
    /// one resolution site that does not lower its target to a type. `allOf` is an applicator, so
    /// [`Self::gather_member`] flattens such a target's fields into the enclosing object instead of
    /// referencing a shared type, and re-expanding the target at every use would cost a branching
    /// reuse graph work and generated types exponential in its depth. The target is
    /// expanded once and its contribution replayed at every later use, so the types its body lowers
    /// to are shared exactly as a root component member's are through [`Self::push_ref_member`].
    ///
    /// A contribution is recorded only once its expansion succeeds; one that failed re-expands, and
    /// reports again, at its next use. What is replayed is a copy the enclosing
    /// merge consumes, so nothing one composition does to the merged fields reaches the next use.
    ///
    /// Each recorded contribution carries the nested bundle target it came from (`None`: the
    /// target's own), so a replay adds each target's contribution to a composition once, however
    /// many paths reach it ([`Self::gatherings`]). Replaying every copy would double a target's
    /// record at each level of a branching graph, leaving it exponential despite the memo (#616).
    resolved_contributions: HashMap<String, Vec<RecordedContribution>>,
    /// The bundle-`$ref` `allOf` member targets being expanded right now, outermost first, each
    /// with whether its body is a bare `$ref` alias. A target is flattened through its own `$ref`
    /// and `allOf` rather than lowered to a reserved type, so nothing else notices when that
    /// expansion reaches a target already on this stack; [`Self::gather_ref_target`] does, and
    /// rejects the loop instead of recursing through it. The stack belongs to the type being
    /// lowered: [`Self::lower_reserved_body`] empties it for each reserved body, so it never spans a
    /// reservation, and a target reached through one sits on the stack of the type it was
    /// reached from only.
    resolved_member_stack: Vec<(String, bool)>,
    /// The root components whose union bodies are being lowered inline right now as a `$ref`
    /// `allOf` member's union ([`Self::lower_all_of_with_union_member`]), outermost first. The
    /// expansion reserves nothing, so [`Self::sole_union_member`] reads this stack to leave a
    /// `$ref` to one of them to the ordinary gather, which lowers the component through its
    /// reservation and so meets the recursion there. Emptied for each reserved body, as
    /// [`Self::resolved_member_stack`] is.
    union_member_stack: Vec<String>,
    /// One [`Gathering`] per composition being gathered right now, innermost last: each `allOf`
    /// lowering opens one, and so does each bundle-`$ref` target expansion
    /// [`Self::gather_ref_target`] records, so the record does not depend on the use that first
    /// reached it. `allOf` is idempotent under a repeated conjunct, so a bundle target whose
    /// contribution the innermost gathering already holds adds nothing more to it.
    gatherings: Vec<Gathering>,
    /// [`Self::all_of_wraps_union`]'s answer for each `$ref` member's target body, keyed by the
    /// body's own `file#pointer` ([`resolved_identity`]): a reuse graph of wrappers that branches
    /// (`W<i>: allOf [$ref W<i-1>, $ref W<i-1>]`) is otherwise re-read once per path, in time
    /// exponential in its depth. A body being read records `false` (wraps no union) before its
    /// members are ([`memoised_answer`]), so a loop through it ends there, as the depth bound would
    /// end it. The document is fixed for the pass, so an answer never goes stale.
    wraps_union_memo: RefCell<HashMap<String, bool>>,
    /// [`Self::null_admission`]'s answer for each `$ref` target body it has read, keyed as
    /// [`Self::wraps_union_memo`] is, so a reuse graph that branches is read once per target. A
    /// body being read records [`NullAdmission::Undecided`] before its keywords are
    /// ([`memoised_answer`]), so a loop through it ends there.
    ///
    /// [`NullAdmission::Undecided`]: nullability::NullAdmission::Undecided
    null_admission_memo: RefCell<HashMap<String, nullability::NullAdmission>>,
    /// The nullability earlier passes' bodies decided for reservations whose back-edges read a
    /// wrong reserve-time guess; consulted before [`Self::null_admission`] when a reservation
    /// opens.
    settled: &'a HashMap<Reservation, bool>,
    /// Open reservations whose provisional nullability a back-edge has read during this pass.
    guessed: HashSet<Reservation>,
    /// Reservations whose body decided a nullability other than the provisional one a back-edge
    /// read, with the body's answer: the pass is stale, and [`lower`] runs another.
    revisions: Vec<(Reservation, bool)>,
    /// Current schema-lowering recursion depth, incremented on entry to [`Self::lower_schema`] and
    /// decremented on exit. A `$ref`/allOf/array/object chain that pushes this past
    /// [`MAX_SCHEMA_DEPTH`] is rejected (`E014`) rather than allowed to overflow the stack.
    depth: u32,
    /// The `open_narrowing` option: whether a string `enum`/`const` narrowing a plain `string`
    /// is lowered as an open set where [`Self::narrowing_opens`] allows it.
    open_narrowing: bool,
    /// Whether the schema being lowered right now is one `open_narrowing` applies to: set by
    /// [`Self::lower_chosen_response_body`] for a response body's own schema, and cleared by
    /// [`Self::closed_narrowing`] for every `$ref` target and every `oneOf`/`anyOf` lowered inside
    /// it. Always `false` while the option is off.
    narrowing_opens: bool,
    /// The string sets [`Self::lower_enum`] lowered while [`Self::narrowing_opens`] held: each is
    /// the type of one inline `enum`/`const` in a position `open_narrowing` applies to, and no
    /// `$ref` target, memo, or union reaches it, so [`Self::narrowed_string`] opens it in place
    /// rather than leaving it beside an open copy as an unused public type.
    open_candidates: HashSet<TypeId>,
    /// The authored schema each meet struct [`Self::intersect_structs`] built is located at (the
    /// left side's definition, else the right side's). The struct itself keeps the document root's
    /// provenance, which ranks it after every declared schema when type names collide; this is
    /// where `W005` and `W006` raised against it point instead of that root (#454).
    meet_locations: HashMap<TypeId, Provenance>,
    /// The `xml` object of every schema a definition was lowered from, read as that schema's root
    /// ([`Self::record_root_xml`]), by the site it was written at. The definitions keep the site,
    /// so [`gate_xml_field_renames`] finds each one from the types it produced once ids are final.
    root_xml: HashMap<XmlSite, RootXml>,
    /// The sites of the `xml` objects a field reads instead ([`Self::claim_field_xml`]): a
    /// property's own and its array items'. Their [`Self::root_xml`] entries are skipped.
    field_xml_sites: HashSet<XmlSite>,
    /// The sites of the compositions being gathered, innermost last
    /// ([`Self::with_xml_holder`]): a member [`Self::record_member_xml`] reads is attributed to
    /// the last.
    xml_holders: Vec<XmlSite>,
    /// For each type a `$ref` member contributed to an `allOf` composition, or a `$ref` target was
    /// intersected with its shape-bearing siblings as ([`Self::record_ref_sibling_merger`]), the
    /// sites of the compositions that merged it ([`Self::record_xml_merger`]), that `$ref` schema
    /// itself counting as one: a member merged into that type is merged into theirs too.
    xml_mergers: HashMap<TypeId, Vec<XmlSite>>,
    /// The `oneOf` a `$ref`'s own sibling carries, while that sibling is lowered to be met with the
    /// `$ref`'s target — or the union an `allOf` is met with, beside it or as a member of it.
    /// [`Self::lower_union_closed`] leaves its indistinguishable variants unmerged (#402): the
    /// caller collapses them after the meet ([`Self::collapse_met_union`]), where the branches'
    /// `null` is still visible, and merging untyped branches first would hide it behind
    /// `serde_json::Value`.
    unmerged_union: Option<Provenance>,
    /// Whether a conjunct [`Self::unmerged_union`] is met with admits `null`: the `$ref` target,
    /// or the composition beside it. Only then does an untyped object branch of that union take
    /// `null` from the meet (#567); where every conjunct is untyped, nothing admits it, and every
    /// spelling keeps the non-null struct an untyped object lowers to.
    unmerged_union_meets_null: bool,
    /// The `W001`s inline `oneOf`s raise for their `serde_json::Value` variants while an `allOf`
    /// gathers its members, held back where `Some` ([`HeldUntypedWarning`]). A union member the
    /// `allOf`'s scalar meet meets lowers on its own first, where a branch that states nothing
    /// (`{}`, `true`) is `Value`; the meet then gives it the other members' type, so the warning
    /// is raised for the variants the meet leaves `Value` alone (#660).
    held_untyped_warnings: Option<Vec<HeldUntypedWarning>>,
    /// The union member of an `allOf` being lowered as an input to the `allOf`'s scalar meet
    /// ([`LowerCtx::lower_scalar_meet_union`]). [`Self::lower_union_closed`] leaves its
    /// indistinguishable variants unmerged, as it leaves [`Self::unmerged_union`]'s, so the meet
    /// still tells apart the branches it excludes (`W011`), and records the union in
    /// [`Self::scalar_meet_unmerged`] for [`Self::collapse_met_one_ofs`] to merge after the meet
    /// (#678). Unlike [`Self::unmerged_union`] it changes nothing else about the union's lowering:
    /// its `null` and its `W001` for `serde_json::Value` variants are the scalar meet's to settle,
    /// as they are for a union lowered there on its own.
    scalar_meet_union: Option<Provenance>,
    /// The unions lowered as [`Self::scalar_meet_union`], whose indistinguishable variants
    /// [`Self::collapse_met_one_ofs`] merges, with `W001`, wherever a scalar meet reaches them,
    /// though the meet's own inserts begin after them.
    scalar_meet_unmerged: HashSet<TypeId>,
    /// For each union [`Self::lower_union_closed`] lowered to an enum of two or more variants, the
    /// members each variant came from (more than one where indistinguishable branches merged), by
    /// their positions among the union's real members (the positions a `W011` names). Kept aligned
    /// when a variant is dropped in place ([`Self::settle_null_admission`]), and moved with the
    /// union's def into a reservation.
    union_variant_members: HashMap<TypeId, Vec<Vec<usize>>>,
    /// Every branch [`Self::intersect_union`] met, in order: the union's id, the variant's
    /// position, and whether the meet kept it. An `allOf`'s scalar meet reads the records its own
    /// meet made to report, as `W011`, a union member's branch it kept nowhere
    /// ([`Self::warn_excluded_union_member_branches`]).
    variant_meets: Vec<(TypeId, usize, bool)>,
    /// The `patternProperties`/`additionalProperties` value types whose one-map homogeneity
    /// [`Self::lower_pattern_additional`] could not decide because a value reached a reservation,
    /// with the object's provenance, in source order; [`Self::check_deferred_map_values`] decides
    /// them once lowering has filled every reservation.
    deferred_map_values: Vec<(Provenance, Vec<Ty>)>,
}

/// The options that change what lowering produces.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LowerOptions {
    /// `Spec::open_narrowing`: lower a response body's own string narrowings as open sets.
    pub(crate) open_narrowing: bool,
}

/// Append a note as a trailing rustdoc paragraph on a type's [`Docs`], used to surface a
/// component-root `default` on the generated named type. An empty description counts as absent,
/// as in [`append_text`].
fn append_doc_note(docs: &mut Docs, note: String) {
    append_text(&mut docs.description, note);
}

/// The provenance of entry `index` of the `allOf` of the schema at `holder`, for a member that
/// carries no provenance of its own (a boolean schema): the entry's pointer, in the file `holder`
/// is written in, with the span of the node written there when the file holds one.
fn all_of_entry_provenance(
    resolver: &Resolver<'_>,
    holder: &Provenance,
    index: usize,
) -> Provenance {
    let pointer = holder.pointer.push("allOf").index(index);
    let span = resolver
        .node_at(resolver.written_in(holder), &pointer)
        .map(|node| node.span);
    Provenance::new(pointer, span)
}
