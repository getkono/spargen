use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, Provenance};
use crate::ir::{
    AdditionalProps, Api, ApiKeyLoc, BodyEncoding, DefaultValue, Delimiter, DisjointFeature, Docs,
    EncodingMode, Field, FieldDefault, HttpScheme, Info, JsonCategory, MediaType, Openness,
    Operation, OperationId, ParamLoc, ParamStyle, Parameter, PathSegment, PathTemplate, Prim,
    PropertyEncoding, PropertyName, RequestBody, Response, ResponseHeader, Responses, ScalarEnum,
    ScalarRepr, ScalarValue, SchemeId, SecurityScheme, SecuritySchemeDef, Server, StatusSpec,
    Struct, Ty, TypeDef, TypeGraph, TypeId, TypeKind, Union, UnionMode, UnionStrategy,
    UnionVariant, UrlSegment, XmlField,
};
use crate::name::synth_operation_id;
use crate::source::{is_remote_ref, Node, Number, SpannedValue};

use super::{
    Document, EncodingObject, JsonType, MediaTypeObject, ParameterObject, PathItem, RefOr,
    RequestBodyObject, Resolver, ResponseObject, Schema, SchemaOr, SecurityRequirement,
    ValidationKeywords,
};

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
        .as_str()
        .rsplit('/')
        .next()
        .filter(|token| !token.is_empty())
        .map_or_else(
            || fallback.to_owned(),
            // Order matters: `~1` first, then `~0`, or a literal `~01` would decode as `/`.
            |token| token.replace("~1", "/").replace("~0", "~"),
        )
}

/// Lower a typed OpenAPI 3.1 or 3.2 [`Document`] into the version-agnostic [`Api`] IR.
///
/// Lowering runs as one or more whole passes over the document, and only the last one's IR and
/// diagnostics are kept. A back-edge — a `$ref` taken while its target's body is still being
/// lowered — has to be typed before that body has decided whether the target is nullable, so it is
/// typed from a reserve-time guess ([`schema_is_nullable`], which cannot see a `null` that a union
/// member, an `allOf`, or a referenced component supplies). When the body then decides otherwise,
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
        settled,
        guessed: HashSet::new(),
        revisions: Vec::new(),
        depth: 0,
        open_narrowing: options.open_narrowing,
        narrowing_opens: false,
        open_candidates: HashSet::new(),
        meet_locations: HashMap::new(),
        unmerged_union: None,
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
        let item = &item;
        for (method, operation) in &item.operations {
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
            let mut merged_parameters: IndexMap<(String, String), ParameterObject> =
                IndexMap::new();
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
                .unwrap_or(&document.security)
                .iter()
                .map(lower_security_requirement)
                .collect();
            // Codegen builds per-operation credential tables from the scheme map, so every
            // referenced scheme must have lowered; an undeclared or unsupported scheme would
            // otherwise silently generate an unauthenticated call.
            for requirement in &security {
                for (scheme, _) in &requirement.0 {
                    if !security_schemes.contains_key(scheme) {
                        Diagnostic::error(
                            Code::UnknownSecurityScheme,
                            operation.provenance.clone(),
                        )
                        .message(format!(
                            "security requirement references undeclared or unsupported \
                             scheme `{}`",
                            scheme.0
                        ))
                        .remedy(
                            "declare the scheme under components.securitySchemes as http \
                             bearer/basic, apiKey, oauth2, or openIdConnect",
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

            operations.push(Operation {
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
            });
        }
    }

    // An intersection narrows a field's type after its `default` was checked against the type the
    // declaring member gave it, so the applied defaults are checked again against the final graph.
    retype_field_defaults(&mut ctx.graph, &ctx.meet_locations, ctx.diags);

    // `xml.name`/`xml.attribute` become a format-agnostic serde `rename`, so they may only be applied
    // to a schema used *exclusively* as an XML body — otherwise the rename would corrupt the JSON
    // wire format. Suppress (and warn `W006` on) the rename for any shared/non-XML-reachable type.
    gate_xml_field_renames(&mut ctx.graph, &operations, &ctx.meet_locations, ctx.diags);

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
            description: api_description,
        },
        servers,
        operations,
        types: ctx.graph,
        security_schemes,
    };
    (ctx.diags.result(api), revisions)
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

/// The three spellings of one conjunction a `oneOf`/`anyOf` takes part in, each met with the
/// union branch by branch and then collapsed by [`LowerCtx::collapse_met_union`]: a `$ref` with
/// the union as its sibling, an `allOf` with the union beside it on one schema (#419), and an
/// `allOf` with the union as one of its members (#463). Only the wording of the diagnostics they
/// report differs.
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

/// Whether a Discriminator Object value is a schema *name* rather than a URI reference: a
/// non-empty string of the characters a Components Object key may hold (`^[a-zA-Z0-9.\-_]+$`).
/// The specification recommends reading a value that is both a valid name and a valid relative
/// reference (`Cat`, `pets.yaml`) as a name, and asks authors to write `./pets.yaml` to mean the
/// file — which the `/` here excludes.
fn is_schema_component_name(value: &str) -> bool {
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

/// `E004` for a reference into `#/components/<kind>/` naming an entry the document does not
/// declare. Shared by lowering and the audit's walk of subschemas lowering never reads, so both
/// report the miss in one wording.
pub(super) fn reject_undeclared_component(
    diags: &mut Diagnostics,
    provenance: &Provenance,
    kind: &str,
    reference: &str,
) {
    // E004 case: undeclared-component
    Diagnostic::error(Code::UnresolvedRef, provenance.clone())
        .message(format!("unresolved {kind} reference `{reference}`"))
        .emit(diags);
}

/// The `file#pointer` a schema `$ref` written at `at` resolves to, answered the way lowering
/// resolves it: a `#/components/schemas/<name>` the root document declares is the root's
/// component wherever it is written — [`LowerCtx::ensure_component`] consults the root map first —
/// and every other reference is the bundle's own answer. Reads no schema and emits nothing.
pub(super) fn schema_reference_identity(
    document: &Document,
    resolver: &Resolver<'_>,
    reference: &str,
    at: &Provenance,
) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
    let root_component = reference
        .strip_prefix("#/components/schemas/")
        .is_some_and(|name| document.components.schemas.contains_key(name));
    if root_component {
        return resolver.reference_identity_from(reference, resolver.root_id());
    }
    resolver.reference_identity(reference, at)
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
    document: &Document,
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
    let identity = schema_reference_identity(document, resolver, &reference, &target.provenance)
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
    /// referencing a shared type, and without this memo it re-expanded the target at every use — a
    /// branching reuse graph cost work and generated types exponential in its depth. The target is
    /// expanded once and its contribution replayed at every later use, so the types its body lowers
    /// to are shared exactly as a root component member's are through [`Self::push_ref_member`].
    ///
    /// A contribution is recorded only once its expansion succeeds; one that failed re-expands, and
    /// reports again, at its next use, as it always did. What is replayed is a copy the enclosing
    /// merge consumes, so nothing one composition does to the merged fields reaches the next use.
    resolved_contributions: HashMap<String, Vec<Contribution>>,
    /// The bundle-`$ref` `allOf` member targets being expanded right now, outermost first, each
    /// with whether its body is a bare `$ref` alias. A target is flattened through its own `$ref`
    /// and `allOf` rather than lowered to a reserved type, so nothing else notices when that
    /// expansion reaches a target already on this stack; [`Self::gather_ref_target`] does, and
    /// rejects the loop instead of recursing through it. The stack belongs to the type being
    /// lowered: [`Self::lower_reserved_body`] empties it for each reserved body, so it never spans a
    /// reservation, and a target reached through one sits on the stack of the type it was
    /// reached from only.
    resolved_member_stack: Vec<(String, bool)>,
    /// The nullability earlier passes' bodies decided for reservations whose back-edges read a
    /// wrong reserve-time guess; consulted before [`schema_is_nullable`] when a reservation opens.
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
    /// The `oneOf` a `$ref`'s own sibling carries, while that sibling is lowered to be met with the
    /// `$ref`'s target — or the union an `allOf` is met with, beside it or as a member of it.
    /// [`Self::lower_union_closed`] leaves its indistinguishable variants unmerged (#402): the
    /// caller collapses them after the meet ([`Self::collapse_met_union`]), where the branches'
    /// `null` is still visible, and merging untyped branches first would hide it behind
    /// `serde_json::Value`.
    unmerged_union: Option<Provenance>,
}

/// The options that change what lowering produces.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LowerOptions {
    /// `Spec::open_narrowing`: lower a response body's own string narrowings as open sets.
    pub(crate) open_narrowing: bool,
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower `#/components/schemas/{name}` to its shared type, lowering it on first use and
    /// returning the cached type on every later one.
    ///
    /// `at` is the provenance of the `$ref` site asking for the component, not the component's own:
    /// when `name` is not declared at all there is no component to point at, so the diagnostic has
    /// to name the reference that could not be followed. That pointer is also what
    /// [`crate::compat`]'s auto-carve maps back to an enclosing operation, so a root-level
    /// provenance here would make the rejection un-carvable.
    ///
    /// Carvability holds for a `$ref` site in a referenced sub-file too, provided `at` carries that
    /// file's span: `compat::carve_rules` reads the pointer in the file the span lies in, and
    /// carves a sub-file construct as a file-scoped pointer rule.
    ///
    /// A name the root document does not declare is handed to [`Self::ensure_resolved`], because a
    /// `$ref` written inside a sub-file spells that file's own components the same way. That is a
    /// re-entry into the resolver from a function the resolver's own component path can call back
    /// into, so it owes a cycle-safety argument, and here it is: `ensure_resolved` reserves the
    /// target's id under its resolved `file#pointer` *before* lowering its body, so a re-entry on
    /// the same target — self-recursion, mutual recursion, an alias loop, or a diamond — finds the
    /// reservation and returns a boxed back-edge rather than descending again. Every cycle closes in
    /// one step, every target is lowered once, and only genuinely new targets consume depth. The two
    /// memos do not compete for one target: a resolved reference that lands on a root component
    /// comes straight back here by name, so `components` stays the single identity for those.
    fn ensure_component(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_component_closed(name, reference, at))
    }

    /// [`Self::ensure_component`]'s body, run with `open_narrowing` out of effect: a component is
    /// a `$ref` target, lowered once and shared by every use, whichever position first reached it.
    fn ensure_component_closed(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        self.warn_if_root_shadows_the_referring_file(name, reference, at);
        if let Some(&(id, nullable)) = self.components.get(name) {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        if let Some(&(id, nullable)) = self.in_progress.get(name) {
            // Re-entered while still lowering this component: a cycle-closing `$ref` back-edge.
            // Box the reference so the recursive type has a finite size instead of rejecting it;
            // the reserved id will hold the root def once the in-progress body finishes. The
            // nullability is provisional, so say it was read: the body checks it when it finishes.
            self.guessed.insert(Reservation::Component(name.to_owned()));
            return Some(Ty {
                id,
                nullable,
                boxed: true,
            });
        }
        // No such component. Report it against the referring site rather than dropping the
        // construct that named it: a silently-dropped `$ref` takes its request body, response, or
        // parameter with it, which is exactly the silent degradation the taxonomy forbids.
        let Some(component) = self.document.components.schemas.get(name) else {
            let reference = format!("#/components/schemas/{name}");
            // The name missed the ROOT document's component map — but a `$ref` written inside a
            // referenced sub-file spells that file's own components exactly the same way, and a
            // JSON Pointer fragment addresses the document it appears in. `Resolver::resolve`
            // already implements that: it keys on the provenance's file and shortcuts to the parsed
            // component map only for the root. `ensure_component` is reached by callers that strip
            // the `#/components/schemas/` prefix before any file is considered, so a sub-file's
            // sibling reference never got there. Hand it back to the resolver.
            //
            // Root first, file second: the root map was already consulted above, so a document that
            // resolves today keeps selecting the same component and only a name the root does not
            // declare reaches the sub-file reading. Which namespace *should* win when both declare
            // the name is a separate question; this deliberately does not change the answer.
            let from = at.span.map(|span| span.file);
            if from.is_some_and(|file| file != self.resolver.root_id()) {
                // The resolver reports its own failure, so a miss here is already diagnosed. Going
                // through `ensure_resolved` rather than straight to `resolve`/`lower_schema` is what
                // makes this re-entry safe *and* finite: see that method and the note above.
                return self.ensure_resolved(&reference, at, name);
            }
            // A raw `/` here is always a further pointer segment, never part of a component name: a
            // literal slash in a key is spelled `~1`. So when the segment it starts from is a
            // declared component, the fragment is a JSON Pointer into that component's body — a
            // *subschema* — and RFC 6901 gives it exactly the meaning the relative-file spelling
            // (`./lib.yaml#/components/schemas/Envelope/properties/payload`) already has. Both go
            // to the resolver, which walks the pointer and lowers its target once per resolved
            // `file#pointer` with its own reservation, so a subschema that refers back to itself
            // or to its enclosing component is boxed against that reservation rather than
            // re-entered. A pointer that walks off the declared body is the resolver's to report.
            //
            // Only when the leading segment is declared: otherwise the fault is the missing
            // component, not the fragment's shape, and that keeps the plain-name wording below.
            let into_a_declared_component = name
                .split_once('/')
                .is_some_and(|(root, _)| self.document.components.schemas.contains_key(root));
            if into_a_declared_component {
                return self.ensure_resolved(&reference, at, name);
            }
            // The wording matches the parameter/request-body/response component arms, which
            // already reject.
            return self.reject_component_alias(at, "schema", &reference);
        };
        let RefOr::Item(schema) = component else {
            let reference = match component {
                RefOr::Ref(reference) => reference.clone(),
                RefOr::Item(_) => return None,
            };
            return self.chain_component_alias(name, &reference.reference, &reference.provenance);
        };
        // A `$ref` whose siblings bear no shape — `description`, `title`, a validation keyword such
        // as `maxLength` — is the same alias spelled with annotations beside it: the parser keeps any
        // sibling key as an inline schema, but `lower_schema_inner`'s `$ref` arm returns the TARGET
        // for it without inserting anything. Through the reserve/pop machinery below that is two
        // faults, one per declaration order: a target lowered earlier leaves this frame's
        // reservation as the last insert and the invariant assertion aborts the process; a target
        // lowered inside this frame is the last insert, so its def is lifted into this reservation
        // and the target's own component entry is left naming an id that no longer holds it.
        // Chaining exactly as the bare spelling does gives both the one answer that spelling gives,
        // cycle check included. The siblings are still acknowledged where they always were: the
        // audit reports an ignored validation keyword (`W001`) independently of lowering. A
        // `default` is the one sibling this frame used to carry (as a doc note on the root's own
        // def); an alias has no def to carry it, so it is reported as `W005` in the parser's words
        // for the bare `$ref`+`default` spelling rather than dropped silently.
        if let Some(reference) = schema.reference.as_deref() {
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                // The alias never reaches `lower_schema_inner`, which reports this elsewhere.
                self.diagnose_standalone_discriminator(schema);
                if let Some(default) = &schema.default {
                    let at = crate::diag::Provenance::new(
                        schema.provenance.pointer.push("default"),
                        Some(default.span),
                    );
                    Diagnostic::warning(Code::SchemaDefaultNotApplied, at)
                        .message(
                            "a schema `default` declared alongside `$ref` is dropped when the \
                             reference resolves and is not applied",
                        )
                        .remedy(
                            "move the default onto the referenced schema, or set the value \
                             explicitly",
                        )
                        .emit(self.diags);
                }
                let reference = reference.to_owned();
                let provenance = schema.provenance.clone();
                return self.chain_component_alias(name, &reference, &provenance);
            }
        }
        // A component whose whole body is `oneOf`/`anyOf` over one `$ref` and one or more `null`
        // members names no shape of its own: it is a **nullable alias** for its target, the union
        // spelling of `B: {$ref: A}` with a null branch added. Recognised here, before anything is
        // reserved, and only while the target's own body is still being lowered.
        //
        // That is mutual recursion — `A.b: {$ref: B}` with `B: {oneOf: [{$ref: A}, {type: "null"}]}`
        // — one of the commonest recursive spellings there is. The union then collapses to the
        // target's reservation and has no def to hand back as this component's root: cloning the
        // reservation's kind inserts a second reservation nothing fills, and returning the
        // reservation itself breaks the last-insert invariant asserted below. Chaining to the
        // target, exactly as the bare-`$ref` alias arm above does, sidesteps both and yields the
        // `Option<Box<A>>` the direct spelling already yields.
        if let Some(alias) = self.nullable_alias_back_edge(schema) {
            return Some(alias);
        }
        // A PROVISIONAL answer, needed before the body finishes so a back-edge encountered mid-body
        // has something to carry. It is not the final one: `schema_is_nullable` is three disjuncts
        // over `types`, `enum_values` and `const_value` and never looks at `oneOf`/`anyOf`/`$ref`/
        // `allOf`, so for any composed body it is a guess. Writing it back over the lowered result
        // discarded every decision `lower_union` makes about null the moment a union was spelled as
        // a named component — the dominant spelling in real descriptions. A guess a back-edge read
        // and the body then contradicted is reported by `settle_reservation`, and the next pass
        // opens this reservation with the body's answer instead (see [`lower`]).
        let reservation = Reservation::Component(name.to_owned());
        let provisional_nullable = self.provisional_nullability(&reservation, schema);
        // Reserve the root id before lowering the body so any back-edge encountered mid-body can
        // box a reference to it. The root's def is inserted last (children first) and then lifted
        // into this reserved slot, which keeps ids dense and stable.
        let root_id = self.graph.reserve();
        self.in_progress
            .insert(name.to_owned(), (root_id, provisional_nullable));
        let lowered = self.lower_reserved_body(schema, name);
        self.in_progress.remove(name);
        self.settle_reservation(
            reservation,
            provisional_nullable,
            lowered.map(|ty| ty.nullable),
        );
        let mut ty = lowered?;
        let (popped_id, mut def) = self.pop_last_type().expect("component root def");
        // Hard invariant (release too): a component root's def is always the last graph insert
        // during its own body lowering (children insert first). If future lowering (allOf/union
        // wrappers) ever inserts a derived type *after* the root, this fails loudly here instead
        // of silently relocating the wrong def and dangling `components[name]`.
        assert_eq!(
            popped_id, ty.id,
            "component root was not the last inserted def"
        );
        // A `default` on the component schema itself has no field to carry it; document it on the
        // named type's rustdoc rather than dropping it. (A component that is a bare `$ref`+`default`
        // never reaches here — it parses to `RefOr::Ref` and is acknowledged as W005 at parse time
        // — so this only sees inline component schemas.) Pure pop-then-mutate: no graph insert
        // happens here, so the last-insert invariant asserted above still holds.
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        // The BODY's answer, not the provisional one: a composed body knows things
        // `schema_is_nullable` cannot see, and naming a schema must not change what it means. Cached
        // under the same value, so a direct return and a later cache hit still yield an identical
        // `Ty`.
        let nullable = ty.nullable;
        self.components.insert(name.to_owned(), (root_id, nullable));
        Some(ty)
    }

    /// The nullability a reservation opens with: an earlier pass's settled answer for it when there
    /// is one, [`schema_is_nullable`]'s guess otherwise.
    fn provisional_nullability(&self, reservation: &Reservation, schema: &Schema) -> bool {
        self.settled
            .get(reservation)
            .copied()
            .unwrap_or_else(|| schema_is_nullable(schema))
    }

    /// Close a reservation's nullability bookkeeping once its body is lowered: when a back-edge read
    /// the `provisional` value and the body decided otherwise, record the body's answer, which makes
    /// this pass stale (see [`lower`]). A body that failed to lower decides nothing.
    fn settle_reservation(
        &mut self,
        reservation: Reservation,
        provisional: bool,
        lowered: Option<bool>,
    ) {
        let read = self.guessed.remove(&reservation);
        if let Some(lowered) = lowered {
            if read && lowered != provisional {
                self.revisions.push((reservation, lowered));
            }
        }
    }

    /// Resolve the component `name`, whose root is an alias for `reference`, to the target's type and
    /// record it under `name`. An alias has no body of its own, so nothing is reserved for it; the
    /// alias stack is what makes a chain of aliases that loops back terminate, as `E004`.
    fn chain_component_alias(
        &mut self,
        name: &str,
        reference: &str,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        if !self.component_alias_stack.insert(name.to_owned()) {
            // E004 case: cycle
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!(
                    "schema component alias `{name}` forms a reference cycle"
                ))
                .emit(self.diags);
            return None;
        }
        let ty = if let Some(target) = reference.strip_prefix("#/components/schemas/") {
            self.ensure_component(target, Some(reference), at)
        } else if is_remote_ref(reference) {
            self.ensure_remote(reference)
        } else {
            self.ensure_resolved(reference, at, name)
        };
        self.component_alias_stack.remove(name);
        if let Some(ty) = ty {
            self.components
                .insert(name.to_owned(), (ty.id, ty.nullable));
        }
        ty
    }

    /// Acknowledge a sub-file's own component declaration that a same-named root declaration
    /// shadows.
    ///
    /// A JSON Pointer fragment addresses the document it appears in, so a `$ref` written inside
    /// `lib.yaml` as `#/components/schemas/Shared` asks for `lib.yaml`'s `Shared`. spargen consults
    /// the root document's component map first, so when the root declares the name too, the root's
    /// wins and the sub-file's declaration is never read.
    ///
    /// The precedence is kept — changing it would retype every split description that relies on it
    /// — but until now nothing said it. Before the sub-file branch existed the reference did not
    /// resolve at all, so only one of the two declarations was ever live and no choice had to be
    /// made; making the reference resolve makes the choice, and it is consequential: adding one
    /// unrelated component to the root document silently retargets a reference written in another
    /// file, and `spargen diff` across that pair reports a breaking change.
    ///
    /// `W011` is what the shadowed declaration is — a declaration with no effect — so no ordinal
    /// moves and the code keeps the matrix cell and `errors.md` row it already has. Emitted per
    /// reference site rather than once per name, because the site is what the reader has to find.
    ///
    /// `reference` is the `$ref` **as the site wrote it**, and only the bare-fragment spelling can
    /// be shadowed: `#/components/schemas/<name>` addresses the document it appears in, so writing
    /// it inside a sub-file that declares `<name>` asks for that file's declaration and is given
    /// the root's instead — which is the entire warning. A reference that names its own document
    /// (`./openapi.yaml#/components/schemas/<name>`) asked for one declaration and got that one:
    /// nothing is shadowed, the message would quote a spelling the site does not contain, and the
    /// remedy — "address the file-local one explicitly with a relative-file reference" — would tell
    /// the author to do what they have already done. `None` is the root's own pre-lowering pass,
    /// which walks declarations rather than references and has no spelling to judge.
    fn warn_if_root_shadows_the_referring_file(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) {
        if reference.and_then(|reference| reference.strip_prefix("#/components/schemas/"))
            != Some(name)
        {
            return;
        }
        let Some(file) = at.span.map(|span| span.file) else {
            return;
        };
        if file == self.resolver.root_id() || !self.document.components.schemas.contains_key(name) {
            return;
        }
        let Some(path) = self.resolver.declares_locally(file, name) else {
            return;
        };
        let message = format!(
            "`#/components/schemas/{name}` here reads the root document's `{name}`; the `{name}` \
             declared in `{path}` is shadowed by it and has no effect on this reference"
        );
        // W011 case: shadowed-component
        Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
            .message(message)
            .remedy(
                "rename one of the two declarations, or address the file-local one explicitly with \
                 a relative-file reference, if the root's is not the one you meant",
            )
            .emit(self.diags);
    }

    /// The cycle-closing back-edge a **nullable alias** component resolves to, when its target's
    /// body is still being lowered.
    ///
    /// `Some` only for a body that is a `oneOf`/`anyOf` over exactly one bare `$ref` plus any
    /// number of null-only members, carrying no shape, discriminator, sibling `$ref` or `default`
    /// of its own — and only when that reference resolves to a target whose body is currently being
    /// lowered, in any of the three frames. The question is asked of the resolved *target*, not of
    /// the reference's spelling: see [`Self::open_reservation_for_ref`].
    ///
    /// Both halves of that narrowness are load-bearing. Recognising an alias whose target is
    /// **finished** would change what is generated for a document that already generates: the
    /// ordinary path re-emits the target's kind under this component's own name, and that named
    /// type is part of the published API, so deleting it is a breaking change to output with
    /// nothing wrong with it. And the answer is only true while the target is open, which is why
    /// nothing is written to [`Self::components`]: a cached hit returns `boxed: false`, and a second
    /// reference taken during the same cycle would then emit an infinitely sized type. Each
    /// reference re-derives it; the memo stays the target's own name.
    fn nullable_alias_back_edge(&mut self, schema: &Schema) -> Option<Ty> {
        if schema.default.is_some() || schema.discriminator.is_some() {
            return None;
        }
        let members = match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
            (false, true) => &schema.one_of,
            (true, false) => &schema.any_of,
            // Neither, or both — the second is rejected by `lower_union` as an intersected
            // applicator and must reach it to be reported.
            _ => return None,
        };
        // Everything the component says apart from the union itself. A `type`, a `properties`, an
        // `enum`, a sibling `$ref`: anything at all makes it a constrained schema rather than
        // another name for its target.
        let mut without_union = schema.clone();
        without_union.one_of.clear();
        without_union.any_of.clear();
        if schema_has_shape_constraint(&without_union) {
            return None;
        }
        let mut real = members.iter().filter(|member| !member_is_null_only(member));
        let SchemaOr::Schema(only) = real.next()? else {
            return None;
        };
        if real.next().is_some() {
            return None;
        }
        let target = only.reference.as_deref()?;
        // A bare reference and nothing else: a member carrying its own keywords is a `$ref` with
        // siblings, which is an intersection and not an alias.
        let mut member_without_ref = only.clone();
        member_without_ref.reference = None;
        if schema_has_shape_constraint(&member_without_ref) {
            return None;
        }
        let (id, target_nullable) = self.open_reservation_for_ref(target, &only.provenance)?;
        Some(Ty {
            id,
            // The target's *own* nullability is as much a fact about this alias as a `"null"`
            // member is. `ensure_component` computes it at reserve time so that every `$ref`
            // consumer agrees on it without waiting for the body to finish, and reading only the
            // members disagrees: an alias with no `"null"` member whose target is nullable emitted
            // a non-`Option` field where the direct `{$ref: T}` spelling of that same target
            // emitted an optional one.
            nullable: target_nullable || members.iter().any(member_is_null_only),
            // The target is mid-lowering, so this is a cycle-closing reference and needs the box
            // for the recursive type to have a finite size.
            boxed: true,
        })
    }

    /// The still-open reservation `reference`, written at `at`, refers to — under any spelling.
    ///
    /// A `$ref` is identified by the target it resolves to, not by the characters used to write it.
    /// The three in-progress maps are each keyed by a different spelling of that identity, so this
    /// mirrors [`Self::lower_schema`]'s own `$ref` dispatch exactly: whichever `ensure_*` the
    /// reference would be lowered through is the map consulted for it. Keying on the literal
    /// `#/components/schemas/` prefix instead made a target's identity depend on the reference
    /// site's spelling, which is how one schema came to be both an alias and an unrepresentable
    /// shape in the same document.
    ///
    /// `None` for a target that is finished, absent, or was never a reservation — every one of
    /// which the ordinary lowering path handles and reports for itself. It resolves no node and
    /// lowers nothing, so asking costs the lowering that follows nothing; the one thing it does
    /// besides look up is raise the shadowed-component `W011` when it answers `Some` for a name
    /// a sub-file also declares, because answering `Some` is answering *instead of*
    /// [`Self::ensure_component`], which is where that warning otherwise lives.
    fn open_reservation_for_ref(
        &mut self,
        reference: &str,
        at: &Provenance,
    ) -> Option<(TypeId, bool)> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            // A root component the root document declares: `ensure_component`'s own key, and its
            // own precedence — root map first, and only a name the root does **not** declare is
            // handed to `ensure_resolved` against the referring file.
            //
            // The gate is the *declaration*, not the reservation. Falling through whenever the
            // name is merely not open resolves it against the referring file, which for a
            // reference written inside a sub-file finds that file's own declaration — the one the
            // root shadows, and one `lower_schema` would never have bound. That answered the same
            // reference string two ways in one document: the direct `{$ref: T}` spelling read the
            // root's component and raised `W011`, while the alias spelling silently read the
            // sub-file's and raised nothing at all.
            if self.document.components.schemas.contains_key(name) {
                // `None` when it is not currently open is the right answer and not a fall-through:
                // a finished or not-yet-started root component is exactly the case the ordinary
                // `ensure_component` path handles, and the case in which it must, because that is
                // where the shadowing warning is raised.
                let entry = self.in_progress.get(name).copied();
                if entry.is_some() {
                    // Answering here is answering *instead of* `ensure_component`, which is where
                    // the shadowing is acknowledged. Say it on the way past, or a reference the
                    // root wins silently retargets a sub-file's own declaration — supported as the
                    // matrix describes, but unreported, which the matrix also promises against.
                    self.warn_if_root_shadows_the_referring_file(name, Some(reference), at);
                    // And instead of `ensure_component`'s back-edge arm, which is where a read of
                    // the provisional nullability is otherwise recorded.
                    self.guessed.insert(Reservation::Component(name.to_owned()));
                }
                return entry;
            }
        } else if is_remote_ref(reference) {
            // `ensure_remote` keys on the absolute URL, and a reference inside a vendored document
            // has already been rewritten absolute, so the reference *is* the key.
            let entry = self.remote_in_progress.get(reference).copied();
            if entry.is_some() {
                self.guessed
                    .insert(Reservation::Remote(reference.to_owned()));
            }
            return entry;
        }
        let (file, pointer) = self.resolver.reference_identity(reference, at)?;
        // `ensure_resolved` routes a target inside the root's component map back to
        // `ensure_component`, whose identity is the name; ask the map that actually holds it.
        if file == self.resolver.root_id() {
            if let Some(name) = pointer
                .as_str()
                .strip_prefix("/components/schemas/")
                .filter(|name| !name.is_empty() && !name.contains('/'))
            {
                if self.document.components.schemas.contains_key(name) {
                    let entry = self.in_progress.get(name).copied();
                    if entry.is_some() {
                        self.guessed.insert(Reservation::Component(name.to_owned()));
                    }
                    return entry;
                }
            }
        }
        let key = format!("{}#{}", file.0, pointer);
        let entry = self.resolved_in_progress.get(&key).copied();
        if entry.is_some() {
            self.guessed.insert(Reservation::Resolved(key));
        }
        entry
    }

    /// Lower a remote (`http`/`https`) `$ref` to a shared, cycle-safe type — the remote analogue of
    /// [`Self::ensure_component`], keyed by the absolute `url#fragment`. Resolution is hermetic (the
    /// schema comes from the vendored, hash-pinned copy already in the bundle; no network). A remote
    /// ref re-entered while its own body is still lowering — a self- or mutually-recursive vendored
    /// schema — returns a boxed back-edge against the reserved root id, so recursion terminates and
    /// generates a finite (boxed) type instead of overflowing the stack.
    fn ensure_remote(&mut self, reference: &str) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_remote_closed(reference))
    }

    /// [`Self::ensure_remote`]'s body, run with `open_narrowing` out of effect, as for a component.
    fn ensure_remote_closed(&mut self, reference: &str) -> Option<Ty> {
        if let Some(&(id, nullable)) = self.remote_components.get(reference) {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        if let Some(&(id, nullable)) = self.remote_in_progress.get(reference) {
            self.guessed
                .insert(Reservation::Remote(reference.to_owned()));
            return Some(Ty {
                id,
                nullable,
                boxed: true,
            });
        }
        let resolved = self
            .resolver
            .resolve(reference, &self.document.provenance, self.diags)
            .ok()?;
        let schema = resolved.schema.into_owned();

        // A vendored document that is itself a bare `$ref` is an alias with no body to reserve a
        // root for. Chain to its target under a cycle guard (so an alias loop terminates) rather
        // than through the reserve/pop machinery, which assumes the body inserts a fresh root.
        if schema.reference.is_some() {
            if !self.remote_alias_stack.insert(reference.to_owned()) {
                // E004 case: cycle
                Diagnostic::error(Code::UnresolvedRef, self.document.provenance.clone())
                    .message(format!("remote $ref `{reference}` forms an alias cycle"))
                    .emit(self.diags);
                return None;
            }
            let ty = self.lower_schema(&schema, reference);
            self.remote_alias_stack.remove(reference);
            return ty;
        }
        // The union spelling of that same alias: a body that is `oneOf`/`anyOf` over one `$ref`
        // back into a frame still being lowered, plus `null`. It has no more shape of its own than
        // the bare `$ref` above does, and — exactly as above — no body to reserve a root for: the
        // collapse resolves it to the target's reservation, which is not this frame's, so the
        // `pop_last()` below would lift the wrong def and the `assert_eq!` after it would abort the
        // process. Recognised here, before anything is reserved, so the frame never opens and there
        // is nothing to pop. `ensure_component` has always done this; the other two frames had the
        // guard only downstream, where it could not see a remote frame's reservations.
        if let Some(alias) = self.nullable_alias_back_edge(&schema) {
            return Some(alias);
        }

        // Provisional, as in `ensure_component`: a back-edge met mid-body needs an answer before the
        // body has one, and `schema_is_nullable` cannot see a composed body's null.
        let reservation = Reservation::Remote(reference.to_owned());
        let provisional_nullable = self.provisional_nullability(&reservation, &schema);
        let root_id = self.graph.reserve();
        self.remote_in_progress
            .insert(reference.to_owned(), (root_id, provisional_nullable));
        let lowered = self.lower_reserved_body(&schema, reference);
        self.remote_in_progress.remove(reference);
        self.settle_reservation(
            reservation,
            provisional_nullable,
            lowered.map(|ty| ty.nullable),
        );
        let mut ty = lowered?;
        let (popped_id, mut def) = self.pop_last_type().expect("remote root def");
        // Same last-insert invariant as `ensure_component`: the remote type's root is the final
        // graph insert during its own body lowering (children insert first).
        assert_eq!(
            popped_id, ty.id,
            "remote root was not the last inserted def"
        );
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        // The body's answer, cached under the same value so a direct return and a later cache hit
        // yield an identical `Ty` — see `ensure_component`.
        self.remote_components
            .insert(reference.to_owned(), (root_id, ty.nullable));
        Some(ty)
    }

    /// Lower a `$ref` the bundle resolver has to follow — a relative-file reference, a whole-file
    /// reference, a non-component fragment, or a sub-file's own `#/components/schemas/<name>` — to a
    /// shared, cycle-safe type. The bundle analogue of [`Self::ensure_component`] and
    /// [`Self::ensure_remote`], and it exists for the reason `ensure_remote` states: `resolve` parses
    /// a *fresh owned schema* on every call, so a bundle reference has no `document`-level identity
    /// the way a root component does.
    ///
    /// Without one, each reference site re-resolved and re-lowered its target from scratch. That is
    /// three faults at once, not one: a shared component became one generated type per *use* instead
    /// of per *declaration* (two public Rust types for one schema, two items in the `surface` semver
    /// surface); a reuse graph cost 2^N lowerings rather than N, so a 40-line two-file description
    /// produced no output and no diagnostic; and a recursive schema had nothing to terminate
    /// against but [`MAX_SCHEMA_DEPTH`], rejecting with `E014` a document `docs/support-matrix.md`
    /// promises is supported.
    ///
    /// The identity is the resolved target's own `file#pointer` ([`resolved_identity`]), read from
    /// the parsed schema's provenance rather than from the `$ref` spelling, so every way of writing
    /// one target lands on one key: a sub-file's bare `#/components/schemas/Inner` and the root's
    /// `./lib.yaml#/components/schemas/Inner` resolve to the same file and pointer and share one
    /// type. Keying on the spelling — or on `(file, name)` — would give one target two identities,
    /// which is how these two paths came to behave differently in the first place. A target inside
    /// the root document's own component map is routed back to [`Self::ensure_component`] for the
    /// same reason: that map is already its identity, and a second one beside it would re-create the
    /// divergence in a new place.
    ///
    /// **Cycle safety.** The reservation is inserted *before* the body is lowered, so any re-entry
    /// on the same key — self-recursion, mutual recursion, or a diamond — finds it and returns a
    /// boxed back-edge instead of descending again. Every cycle therefore closes in one step. A
    /// chain of bare-`$ref` aliases has no body to reserve against and is guarded separately by
    /// [`Self::resolved_alias_stack`], exactly as `remote_alias_stack` guards the remote one. Only
    /// genuinely new targets descend, so the depth counter still bounds a real chain (`E014`) and
    /// nothing repeated can accumulate against it.
    ///
    /// **Semver.** spargen's semver surface is the public API of *generated output*, and this
    /// changes it for any description that reaches a target through more than one reference. Types
    /// that existed only because one schema was lowered once per use site are gone, and a type is
    /// now named for the schema it resolves from rather than for whichever site reached it first.
    /// `surface`'s own classifier calls a removed public item `ChangeKind::TypeRemoved`, which its
    /// impact policy grades **Major**, so regenerating against an unchanged description can stop
    /// compiling a consumer that named one of the removed types. That is the correct outcome — the
    /// removed types were artefacts of lowering the same schema repeatedly — but it is a breaking
    /// change to the generated API and is released as one.
    fn ensure_resolved(&mut self, reference: &str, at: &Provenance, hint: &str) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_resolved_closed(reference, at, hint))
    }

    /// [`Self::ensure_resolved`]'s body, run with `open_narrowing` out of effect, as for a
    /// component.
    fn ensure_resolved_closed(
        &mut self,
        reference: &str,
        at: &Provenance,
        hint: &str,
    ) -> Option<Ty> {
        let resolved = self.resolver.resolve(reference, at, self.diags).ok()?;
        let schema = resolved.schema.into_owned();
        let Some(key) = resolved_identity(&schema.provenance) else {
            // No span, so no identity to key on. Lower it un-deduplicated rather than share a type
            // under a key that does not identify it: a duplicated type is wrong, a wrongly shared
            // one is worse. Every schema the parser produces carries a span, so this is defensive.
            return self.lower_schema(&schema, hint);
        };
        // A resolved target that is a root component already has an identity — its name. The
        // `contains_key` alone decides it: a pointer deeper than a component (`Tree/properties/x`)
        // cannot equal a key, because structural validation rejects any root component key outside
        // `^[a-zA-Z0-9._-]+$` before lowering runs.
        if schema
            .provenance
            .span
            .is_some_and(|span| span.file == self.resolver.root_id())
        {
            if let Some(name) = schema
                .provenance
                .pointer
                .as_str()
                .strip_prefix("/components/schemas/")
            {
                if self.document.components.schemas.contains_key(name) {
                    return self.ensure_component(name, Some(reference), at);
                }
            }
        }
        if let Some(&(id, nullable)) = self.resolved_components.get(&key) {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        if let Some(&(id, nullable)) = self.resolved_in_progress.get(&key) {
            self.guessed.insert(Reservation::Resolved(key));
            return Some(Ty {
                id,
                nullable,
                boxed: true,
            });
        }
        // Name the type for the schema it came from, not for whichever use site reached it first:
        // once one type serves every site, a per-site hint would make the generated name depend on
        // lowering order. A whole-file reference has no final pointer token, so the caller's hint
        // stands there.
        let hint = resolved_hint(&schema.provenance, hint);

        // A target that is itself a bare `$ref` is an alias with no body to reserve a root for.
        // Chain to its target under a cycle guard rather than through the reserve/pop machinery,
        // which assumes the body inserts a fresh root.
        if schema.reference.is_some() {
            if !self.resolved_alias_stack.insert(key.clone()) {
                // E004 case: cycle
                Diagnostic::error(Code::UnresolvedRef, at.clone())
                    .message(format!(
                        "schema reference `{reference}` forms an alias cycle"
                    ))
                    .remedy(
                        "give one component in the cycle a schema body, or break the cycle at one \
                         of its references",
                    )
                    .emit(self.diags);
                return None;
            }
            let ty = self.lower_schema(&schema, &hint);
            self.resolved_alias_stack.remove(&key);
            return ty;
        }
        // The union spelling of that same alias — see the matching arm in [`Self::ensure_remote`].
        // This is the split-description case: a sub-file whose `MaybeNode` is nothing but "a
        // `Node`, or null", which is the namespace shape issue #107 exists to make resolve.
        if let Some(alias) = self.nullable_alias_back_edge(&schema) {
            return Some(alias);
        }

        // Provisional, as in `ensure_component`: a back-edge met mid-body needs an answer before the
        // body has one, and `schema_is_nullable` cannot see a composed body's null.
        let reservation = Reservation::Resolved(key.clone());
        let provisional_nullable = self.provisional_nullability(&reservation, &schema);
        let root_id = self.graph.reserve();
        self.resolved_in_progress
            .insert(key.clone(), (root_id, provisional_nullable));
        let lowered = self.lower_reserved_body(&schema, &hint);
        self.resolved_in_progress.remove(&key);
        self.settle_reservation(
            reservation,
            provisional_nullable,
            lowered.map(|ty| ty.nullable),
        );
        let mut ty = lowered?;
        let (popped_id, mut def) = self.pop_last_type().expect("resolved root def");
        // Same last-insert invariant as `ensure_component` and `ensure_remote`: the target's root is
        // the final graph insert during its own body lowering (children insert first).
        assert_eq!(
            popped_id, ty.id,
            "resolved root was not the last inserted def"
        );
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        // The body's answer, cached under the same value so a direct return and a later cache hit
        // yield an identical `Ty` — see `ensure_component`.
        self.resolved_components.insert(key, (root_id, ty.nullable));
        Some(ty)
    }

    fn lower_schema_or(&mut self, schema: &SchemaOr, hint: &str) -> Option<Ty> {
        match schema {
            SchemaOr::Bool(true) => {
                Some(self.insert_type(hint, TypeKind::Any, Docs::default(), None))
            }
            SchemaOr::Bool(false) => {
                Some(self.insert_type(hint, TypeKind::Never, Docs::default(), None))
            }
            SchemaOr::Schema(schema) => self.lower_schema(schema, hint),
        }
    }

    /// Depth-guarded entry to schema lowering. Bounds the `$ref`/allOf/array/object recursion to
    /// [`MAX_SCHEMA_DEPTH`] so a pathologically deep composition rejects with `E014` instead of
    /// exhausting the stack; the counter is decremented on every exit so sibling members (breadth)
    /// never accumulate against the cap.
    fn lower_schema(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        if self.depth >= MAX_SCHEMA_DEPTH {
            return self.reject_too_deep(&schema.provenance);
        }
        self.depth += 1;
        let result = self.lower_schema_inner(schema, hint);
        self.depth -= 1;
        result
    }

    /// Lower the body of a type whose root id `ensure_component`, `ensure_remote` or
    /// `ensure_resolved` has just reserved, with [`Self::resolved_member_stack`] empty for the
    /// duration and restored after.
    ///
    /// The stack answers "is this expansion inside itself", and a reservation starts a new type:
    /// a member target flattened by an enclosing expansion and met again inside this body is a
    /// recursive *field* of the new type, which the reservation boxes, not a loop of the enclosing
    /// expansion. Re-entering the reserved type itself is refused by its `*_in_progress` entry, so
    /// every loop that crosses this boundary is still caught, as the root document catches it.
    fn lower_reserved_body(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let enclosing = std::mem::take(&mut self.resolved_member_stack);
        let lowered = self.lower_schema(schema, hint);
        self.resolved_member_stack = enclosing;
        lowered
    }

    /// The `E014` rejection [`Self::lower_schema`] reports at [`MAX_SCHEMA_DEPTH`], shared with the
    /// one other recursion that does not pass through it: [`Self::gather_ref_target`]'s expansion
    /// of a bundle-`$ref` `allOf` member's target.
    fn reject_too_deep<T>(&mut self, provenance: &Provenance) -> Option<T> {
        Diagnostic::error(Code::SchemaNestingTooDeep, provenance.clone())
            .message(format!(
                "schema nesting exceeds the maximum lowering depth of {MAX_SCHEMA_DEPTH} \
                 (a very long `$ref` chain or a pathologically nested schema)"
            ))
            .remedy(
                "flatten the offending schema chain, or omit this API segment with \
                 spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    fn lower_schema_inner(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        // Before any arm can return: a discriminator beside no union is dropped by every one of
        // them (a `$ref` with no other sibling, `allOf`, a type array, a plain object).
        self.diagnose_standalone_discriminator(schema);
        if let Some(value) = schema.boolean {
            let kind = if value {
                TypeKind::Any
            } else {
                TypeKind::Never
            };
            return Some(self.insert_schema_type(schema, hint, kind));
        }

        if let Some(reference) = &schema.reference {
            let referenced = if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                self.ensure_component(name, Some(reference), &schema.provenance)?
                // Remote refs go through the cycle-safe, deduped remote path (keyed by
                // `url#fragment`), mirroring `ensure_component`; a bare relative/other ref falls
                // through to `resolve`, which reports it (E003/E004).
            } else if is_remote_ref(reference) {
                self.ensure_remote(reference)?
            } else {
                // Bundle refs go through the cycle-safe, deduped path too, keyed by the resolved
                // `file#pointer`. That key is why the ordinary spelling and the explicit
                // `./lib.yaml#/…` spelling of one target now share one type rather than two.
                self.ensure_resolved(reference, &schema.provenance, hint)?
            };

            // In JSON Schema 2020-12 `$ref` is an applicator, not a replacement for the containing
            // schema. Intersect every shape-bearing sibling instead of silently discarding it.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                // No shape to compose, so a cycle-closing reference here is the ordinary recursive
                // schema: it boxes and generates. Only the intersection below needs a real target.
                return Some(referenced);
            }
            // Whether this `$ref` closes a reference cycle back through a schema whose lowering
            // encloses it. A target inside the cycle cannot be composed with: its definition
            // depends on the very result being computed, so `intersect_non_null`'s `(Any, _)` arm
            // would return the sibling and silently discard the target.
            //
            // It is asked of the DOCUMENT, for every spelling alike. A lowering-order test — "is the
            // target mid-flight" — gives two byte-identical documents opposite verdicts when only
            // the order of two map entries differs (decision 23), and so does a document test that
            // only one spelling can reach: the sub-file spelling used to fall back to
            // `is_in_progress_root`, so with siblings on one edge of a two-schema cycle the verdict
            // followed which end lowering happened to enter first. `ref_closes_a_cycle` walks
            // resolved identities across every file instead.
            //
            // `is_in_progress_root` stays as a backstop and adds no rejection of its own: a target
            // still being lowered is one whose lowering reached this site, which is a cycle the walk
            // finds. Kept so that a walk which ever missed one reports the recursion, rather than
            // leaving `intersect_types`' fail-closed arm to report it as a failed intersection.
            let back_edge = self.ref_closes_a_cycle(reference, &schema.provenance)
                || self.is_in_progress_root(referenced.id);
            if back_edge {
                // The siblings have nothing yet to intersect with. The `allOf` spelling of the same
                // conjunction has always rejected this rather than composing against a placeholder,
                // and the alternative here is not "compose anyway" but "discard the target", which
                // produces a type accepting documents the description forbids — a recursive `Node`
                // flattened to a one-off struct, or to the sibling's own scalar.
                // The wording is chosen by what the reference RESOLVES TO, not by how it was
                // spelled. It used to branch on whether the string began `#/components/schemas/`,
                // which is a fact about the author's typing: the explicit `./lib.yaml#/…` spelling
                // of a sub-file component was therefore described to the reader as *remote*, which
                // it is not, while the bare spelling of the same target in the same file was
                // described correctly. One reference, two spellings, two different accounts of one
                // fact — the mistake decision 23 removed from the verdict, left standing in the
                // explanation.
                //
                // `is_remote_ref` is the same predicate that routes the lowering a few lines above,
                // so the message and the code path now agree by construction. A genuinely remote
                // target still says so; every local target, however addressed, says the same thing.
                // The local noun is "schema", not "component": a local target need not be a
                // component at all (`./lib.yaml#/bag/Tree`, or `#/bag/Tree` inside a sub-file), and
                // a two-way predicate cannot tell that case apart, so the wording must hold for it.
                return self.reject_ref_sibling_cycle(
                    schema,
                    if is_remote_ref(reference) {
                        "this remote `$ref` closes a reference cycle back to the schema that \
                         encloses it, so its shape-bearing siblings would have to be intersected \
                         with a target whose own definition depends on the result"
                    } else {
                        "this `$ref` closes a reference cycle back to the schema that encloses it, \
                         so its shape-bearing siblings would have to be intersected with a target \
                         whose own definition depends on the result"
                    },
                );
            }
            // A sibling carrying only object or only array applicators names no `type`, and
            // `lower_schema` reaches its object and array arms through `type`, so it would lower to
            // `TypeKind::Any` — which intersects as identity, discarding the keywords with no
            // diagnostic (#140). The applicators establish the category they apply to, as an
            // untyped `properties` already does, and say nothing about `null` (the same reading
            // `lower_union_sibling` takes), so the target's nullability survives the intersection.
            //
            // Against a union target that reading would drop every branch of another category in
            // silence — `oneOf: [string, Obj]` with a `required` sibling would become `Obj` alone
            // and reject the strings the target accepts. There the applicators refine the branches
            // of their own category, as they do beside an inline union (#282).
            let category = implied_applicator_category(&sibling);
            if category.is_some() {
                if let TypeKind::Union(union) = &self.graph.get(referenced.id)?.kind {
                    let union = union.clone();
                    return self.refine_union_target(schema, hint, referenced, &union, &sibling);
                }
            }
            let mut inferred_category = false;
            match category {
                Some(ImpliedCategory::Only(category)) => {
                    sibling.types.types = vec![category, JsonType::Null];
                    inferred_category = true;
                }
                Some(ImpliedCategory::Conflicting) => {
                    return self.reject_ref_sibling_category(
                        schema,
                        "this `$ref`'s untyped sibling keywords are both object keywords \
                         (`properties`, `patternProperties`, `required`, `additionalProperties`) \
                         and array keywords (`items`, `prefixItems`) with no `type` to choose \
                         between them, so no single Rust type represents what they constrain",
                    );
                }
                None => {}
            }
            let has_union_sibling = !schema.one_of.is_empty() || !schema.any_of.is_empty();
            if has_union_sibling {
                // Keywords carrying an `allOf` of their own stay on the one-schema path: lowered
                // as a conjunct, that nested `allOf` denies the target's `null` (#562), which
                // would turn a union that admits `null` into one that rejects it.
                let (keywords, union) = split_union_sibling(&sibling);
                if keywords.all_of.is_empty() && schema_has_shape_constraint(&keywords) {
                    return self
                        .meet_ref_union_sibling(schema, hint, referenced, &keywords, &union);
                }
            }
            let enclosing_unmerged = std::mem::replace(
                &mut self.unmerged_union,
                has_union_sibling.then(|| schema.provenance.clone()),
            );
            let sibling = self.lower_schema(&sibling, &format!("{hint}Constraint"));
            self.unmerged_union = enclosing_unmerged;
            let sibling = sibling?;
            let mark = self.graph_mark();
            let Ok(intersection) =
                self.intersect_types(referenced, sibling, &format!("{hint}ReferenceIntersection"))
            else {
                // `$ref` is an applicator: the value must satisfy the target AND these siblings.
                // `intersect_types` fails for two distinct conditions — the intersection is
                // empty, so no value satisfies both, or it is inhabited but has no single Rust type
                // — and this one message covers both, so it must not claim the first. Either way
                // it is reported rather than dropped: dropping would silently delete a body,
                // parameter or property from the generated client.
                return self.reject_ref_sibling_intersection(schema);
            };
            // Only a `$ref` whose own sibling is a `oneOf`/`anyOf` is collapsed. A `$ref` to a union
            // beside a non-union sibling is an intersection this check was never meant for, and it
            // keeps the shape it has always generated.
            let intersection = if has_union_sibling {
                let (collapsed, untyped_check) = self.collapse_met_union(
                    schema,
                    intersection,
                    !schema.one_of.is_empty(),
                    &format!("{hint}ReferenceIntersection"),
                    MetUnion::RefSibling,
                );
                // Nothing meets the union after the collapse here.
                if untyped_check {
                    self.warn_untyped_met_variants(schema, collapsed, MetUnion::RefSibling);
                }
                collapsed
            } else {
                intersection
            };
            let kind = self.graph.get(intersection.id)?.kind.clone();
            // The `null` the inferred category carries is there to leave the target's nullability
            // alone, not to satisfy the intersection on its own. Against a nullable target of
            // another category the two share only `null`, and typing that as the exact JSON null
            // would silently replace, say, a nullable string with `()`: the category contradiction
            // is the same empty intersection it is against the non-null target, and is reported
            // the same way. A target that is itself exactly `null` keeps its type.
            if inferred_category
                && matches!(kind, TypeKind::Null)
                && !matches!(self.graph.get(referenced.id)?.kind, TypeKind::Null)
            {
                return self.reject_ref_sibling_category(
                    schema,
                    "this `$ref`'s untyped sibling keywords establish a category its target does \
                     not have, so the only value both accept is `null`; the intersection is empty \
                     but for the target's nullability",
                );
            }
            self.discard_meet_intermediates(mark, &kind);
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = intersection.nullable;
            ty.boxed = intersection.boxed;
            return Some(ty);
        }

        if !schema.all_of.is_empty() {
            if schema_has_union(schema) {
                return self.lower_all_of_beside_union(schema, hint);
            }
            if let Some(index) = sole_union_member(schema) {
                return self.lower_all_of_with_union_member(schema, hint, index);
            }
            return self.lower_all_of(schema, hint);
        }

        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            return self.lower_union(schema, hint);
        }

        if let Some(enumeration) = &schema.enum_values {
            return self.lower_enum(enumeration, schema, hint);
        }
        if let Some(value) = &schema.const_value {
            return self.lower_enum(std::slice::from_ref(value), schema, hint);
        }

        let non_null_types: Vec<JsonType> = schema
            .types
            .types
            .iter()
            .copied()
            .filter(|ty| *ty != JsonType::Null)
            .collect();
        if non_null_types.len() > 1 {
            return self.lower_type_array(schema, hint, &non_null_types);
        }

        // A binary payload — `contentEncoding: base64` or `format: binary` (the OpenAPI file/upload
        // marker) — lowers to raw `bytes::Bytes` rather than a `String`, so a multipart file part
        // carries bytes and a byte body is not misdecoded as UTF-8. A `"null"` in the type array
        // (`type: [string, 'null']`) makes it nullable exactly as the `oneOf [.., null]` spelling
        // is, so both spellings reach the raw-body gates, and a JSON member becomes
        // `Option<bytes::Bytes>`, rather than the `null` being dropped here.
        if schema.content_encoding.as_deref() == Some("base64")
            || schema.format.as_deref() == Some("binary")
        {
            let mut ty = self.insert_schema_type(schema, hint, TypeKind::Bytes);
            ty.nullable = schema.types.types.contains(&JsonType::Null);
            return Some(ty);
        }

        let nullable = schema.types.types.contains(&JsonType::Null);
        let primary = schema
            .types
            .types
            .iter()
            .find(|ty| **ty != JsonType::Null)
            .copied();

        let mut ty = match primary {
            Some(JsonType::Boolean) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::Bool))
            }
            Some(JsonType::Integer) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("int32") => Prim::I32,
                    _ => Prim::I64,
                }),
            ),
            Some(JsonType::Number) => {
                self.insert_schema_type(schema, hint, TypeKind::Primitive(Prim::F64))
            }
            Some(JsonType::String) => self.insert_schema_type(
                schema,
                hint,
                TypeKind::Primitive(match schema.format.as_deref() {
                    Some("uuid") => Prim::Uuid,
                    Some("date-time") => Prim::DateTime,
                    Some("date") => Prim::Date,
                    _ => Prim::String,
                }),
            ),
            Some(JsonType::Array) => {
                if !schema.prefix_items.is_empty() {
                    // `items` beside `prefixItems` is the 2020-12 rest-element schema. A Rust tuple
                    // is fixed-length, so a typed remainder is not representable — except
                    // `items: false`, which closes the array at the prefix and *is* a tuple.
                    if let Some(rest) = &schema.items {
                        if !matches!(rest.as_ref(), SchemaOr::Bool(false)) {
                            Diagnostic::error(
                                Code::TupleRestNotRepresentable,
                                schema.provenance.clone(),
                            )
                            .message(
                                "`items` beside `prefixItems` allows a typed variable-length \
                                 remainder, which no single Rust type expresses",
                            )
                            .remedy(
                                "use `items: false` to close the tuple, describe the whole array \
                                 with `items`, or omit this API segment with spargen::omit!",
                            )
                            .emit(self.diags);
                            return None;
                        }
                    }
                    let mut items = Vec::new();
                    for (index, child) in schema.prefix_items.iter().enumerate() {
                        items.push(self.lower_schema_or(child, &format!("{hint}Item{index}"))?);
                        self.warn_structural_default_or(child, "a tuple `prefixItems` entry");
                    }
                    self.insert_schema_type(schema, hint, TypeKind::Tuple(items))
                } else {
                    let mut item = match &schema.items {
                        Some(items) => {
                            let item = self.lower_schema_or(items, &format!("{hint}Item"))?;
                            self.warn_structural_default_or(items, "array `items`");
                            item
                        }
                        None => self.insert_type(
                            &format!("{hint}Item"),
                            TypeKind::Any,
                            Docs::default(),
                            None,
                        ),
                    };
                    // A `Vec` already provides the heap indirection that breaks a `$ref` cycle, so a
                    // back-edge closing through an array never needs its own `Box`.
                    item.boxed = false;
                    self.insert_schema_type(schema, hint, TypeKind::Array(Box::new(item)))
                }
            }
            Some(JsonType::Object) | None
                if !schema.properties.is_empty() || !schema.pattern_properties.is_empty() =>
            {
                self.lower_object(schema, hint)?
            }
            Some(JsonType::Object) => self.lower_object(schema, hint)?,
            Some(JsonType::Null) => self.insert_schema_type(schema, hint, TypeKind::Null),
            None if schema.types.types.contains(&JsonType::Null) => {
                self.insert_schema_type(schema, hint, TypeKind::Null)
            }
            None => self.insert_schema_type(schema, hint, TypeKind::Any),
        };
        ty.nullable = nullable;
        Some(ty)
    }

    fn lower_type_array(
        &mut self,
        schema: &Schema,
        hint: &str,
        non_null_types: &[JsonType],
    ) -> Option<Ty> {
        let mut branches = Vec::with_capacity(non_null_types.len());
        for ty in non_null_types {
            let mut branch = schema.clone();
            branch.types.types = vec![*ty];
            branch.title = None;
            branch.description = None;
            branches.push(SchemaOr::Schema(Box::new(branch)));
        }

        let mut union = schema.clone();
        union.boolean = None;
        union.reference = None;
        union.types.types.retain(|ty| *ty == JsonType::Null);
        union.properties.clear();
        union.required.clear();
        union.additional_properties = None;
        union.pattern_properties.clear();
        union.items = None;
        union.prefix_items.clear();
        union.all_of.clear();
        union.one_of.clear();
        union.any_of = branches;
        union.discriminator = None;
        union.enum_values = None;
        union.const_value = None;
        union.format = None;
        union.content_encoding = None;
        union.content_media_type = None;
        union.content_schema = None;
        union.xml = None;
        union.validation = ValidationKeywords::default();
        self.lower_union(&union, hint)
    }

    fn lower_object(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (fields, additional) = self.object_body(schema, hint)?;
        Some(self.insert_schema_type(
            schema,
            hint,
            TypeKind::Struct(Struct { fields, additional }),
        ))
    }

    /// Lower a `oneOf`/`anyOf` union. `null` members are stripped and make the union `nullable`
    /// (`Option<Union>`), exactly like a `"null"` in a type array; a 2-member union whose other
    /// member is null collapses to `Option<TheOtherType>` with no enum. The remaining variants are
    /// represented WITHOUT `serde(untagged)` and without degrading to `serde_json::Value`:
    ///
    /// * a `discriminator` dispatches object variants by tag and uniquely categorized non-object
    ///   variants by JSON category;
    /// * statically disjoint variants dispatch by JSON category or unique required key;
    /// * overlapping variants use typed trial matching with exact-one (`oneOf`) or deterministic
    ///   most-specific (`anyOf`) semantics, including serialization revalidation.
    ///
    /// Every variant type inserts before the union def, so the [`TypeKind::Union`] is the final
    /// graph insert — preserving the [`Self::ensure_component`] last-insert invariant when the union
    /// is a component body.
    fn lower_union(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.lower_union_closed(schema, hint))
    }

    /// [`Self::lower_union`]'s body, run with `open_narrowing` out of effect. A union tells its
    /// variants apart by what each one refuses — a `Trial` `oneOf` requires exactly one to match —
    /// so an open set inside a variant could make two variants accept the same value and fail a
    /// value the closed union decodes.
    fn lower_union_closed(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let (members, mode): (Vec<&SchemaOr>, UnionMode) =
            match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
                (false, true) => (schema.one_of.iter().collect(), UnionMode::OneOf),
                (true, false) => (schema.any_of.iter().collect(), UnionMode::AnyOf),
                (false, false) => {
                    return self.reject_unrepresentable_union(
                    schema,
                    "a single schema node declares both `oneOf` and `anyOf`; their intersected \
                     applicator semantics are not representable as one generated union",
                );
                }
                (true, true) => unreachable!("lower_union is called only for a union schema"),
            };
        let sibling = self.lower_union_sibling(schema, hint)?;

        // Two DIFFERENT facts, tracked apart because only one of them can rescue an empty
        // intersection. A null-only MEMBER supplies a branch that `null` validates against. A
        // `"null"` in the enclosing `type` array only *permits* null: `oneOf` still demands exactly
        // one matching member and `anyOf` at least one, so with no null member there is nothing for
        // `null` to match and the schema admits nothing at all. Merging them made
        // `{type: [integer,'null'], oneOf: [{type: string}]}` — which nothing satisfies —
        // indistinguishable from the same document with a `{type: 'null'}` member, which only
        // `null` satisfies.
        let null_from_type_array = schema.types.types.contains(&JsonType::Null);
        let mut null_from_member = false;
        let mut real_members: Vec<&SchemaOr> = Vec::new();
        for member in members {
            if member_is_null_only(member) {
                null_from_member = true;
            } else {
                real_members.push(member);
            }
        }
        // The union's overall acceptance needs both; only the rescues below need them apart.
        let mut nullable = null_from_type_array || null_from_member;

        // Every schema the discriminator names is checked against the members before any path
        // below can return. The collapses do not build a discriminated dispatch at all, so a check
        // made only where one is built dropped a dangling or non-member mapping entry there
        // without looking at it. Resolution here reads identities, not schemas, so it lowers
        // nothing and cannot reorder what the members lower to.
        let discriminator_members = match &schema.discriminator {
            Some(discriminator) => Some(self.discriminator_members(discriminator, &real_members)?),
            None => None,
        };

        // Only null members remained: the exact JSON null type.
        if real_members.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        // The third spelling of the conjunction the `$ref`-sibling and `allOf` arms already guard.
        // A union member that closes a reference cycle resolves to the target's RESERVED id, whose
        // def is the `TypeKind::Reserved` placeholder, so intersecting a variant against it can
        // produce no true answer. Only reachable when there IS a sibling to intersect with: without
        // one, an ordinary recursive union boxes its back-edge and generates, which is what makes a
        // recursive `oneOf` usable at all.
        //
        // This is the DOCUMENT half of the test and it is answered before lowering, for every
        // spelling of the member's `$ref`. The reservation half is applied after each member is
        // lowered, at the two sites below where the member's `Ty` exists; between them the three
        // spellings of one conjunction give one verdict, which they did not before.
        if sibling.is_some() {
            for member in &real_members {
                // A member that is this union's OWN reservation is not an intersection problem:
                // the union is itself, which the collapse and variant paths below reject as `E007`
                // with or without siblings. Left to this guard it would draw `E013` only when
                // siblings are present, so one shape would get two codes.
                if self.member_closes_a_cycle(member, &schema.provenance)
                    && !self.member_is_this_union(member, &schema.provenance)
                {
                    return self.reject_ref_sibling_cycle(
                        schema,
                        "this union member's `$ref` closes a reference cycle back to the schema \
                         that encloses it, so the enclosing schema's own sibling keywords would \
                         have to be intersected with a target whose definition depends on the \
                         result",
                    );
                }
            }
        }

        // A single real member (the rest were null): `Option<ThatType>`, no enum needed. Re-emit the
        // member's kind as this position's own def so it is the final graph insert — mirroring the
        // allOf single-member collapse — which keeps the `ensure_component` last-insert invariant
        // when the union is a component body (a bare `$ref` member would otherwise return an existing
        // id and leave the popped root mismatched).
        if real_members.len() == 1 {
            let mut inner = self.lower_schema_or(real_members[0], hint)?;
            // Everything the sibling meet below inserts has an id at or above this mark; the
            // member and the sibling were lowered before it.
            let mark = self.graph_mark();
            // The member's OWN nullability, before the intersection overwrites `inner`. Needed
            // below when the sibling is not entitled to decide.
            let member_nullable = inner.nullable;
            // The sole member is the reservation *this* schema will occupy, so the union is the
            // whole of itself: `Selfy = Selfy | null` describes nothing a decoder can terminate on,
            // exactly as a direct recursive member does in a multi-member union. That path already
            // refuses it, and this is the same shape written with fewer members beside it, so it
            // gets the same code and the same wording rather than a second code chosen by member
            // count. Asked before the reservation guards below, which would otherwise answer the
            // narrower question first and hand one shape two codes again.
            //
            // The document-half guard above (`member_closes_a_cycle`) runs before this collapse
            // and would otherwise answer `E013` for this shape whenever the union carries sibling
            // keywords. It excludes a member that is this union's own reservation
            // (`member_is_this_union`), so the self-union reaches here and draws `E007` with or
            // without siblings, and with one member or several.
            // `a_union_whose_sole_member_is_its_own_reservation_is_rejected` in `tests/frontend.rs`
            // asserts the reported error codes are **exactly** `[E007]` on both its spellings.
            if self.reservation_at(&schema.provenance) == Some(inner.id) {
                return self.reject_self_referential_union(
                    schema,
                    "a union member is a direct recursive `$ref` to the union being lowered, so \
                     the member is the union itself and decoding it would never terminate",
                );
            }
            // The reservation half of the cycle test, on the sole real member. The document half
            // above answers only the `#/components/schemas/…` spelling; a sub-file or remote member
            // reference reaches here still pointing at a placeholder, and the intersection below
            // cannot compose with one. Reported with the same wording the other two spellings use,
            // because it is the same fact about the same document.
            if sibling.is_some() && self.is_in_progress_root(inner.id) {
                return self.reject_ref_sibling_cycle(
                    schema,
                    "this union member's `$ref` closes a reference cycle back to the schema that \
                     encloses it, so the enclosing schema's own sibling keywords would have to be \
                     intersected with a target whose definition depends on the result",
                );
            }
            // The member is some *other* type's still-open reservation — the cycle-closing
            // `$ref` of an ordinary recursive schema. Its kind may not be read: cloning a
            // `TypeKind::Reserved` inserts a second reservation nothing will ever `fill`, which
            // `check_invariants` reports as `E011` against a document that is not malformed, and
            // which at `2aa5ada` (where the placeholder was `TypeKind::Any`) cloned as
            // `serde_json::Value` instead — a typed schema silently degraded.
            //
            // A truthful answer exists and needs no def of its own: the member's own `Ty`, boxed so
            // the cycle has a finite size and optional because the `null` member is what collapsed
            // away. That is `Option<Box<T>>` — what `docs/support-matrix.md` promises for this
            // construct, and what the direct `{$ref: T}` spelling already produces.
            //
            // Inserting no def is safe exactly where this union is not itself the body of a type
            // whose root id was reserved before lowering began. Where it is, the caller pops the
            // last insert and lifts it into that reserved root, so returning a foreign id would
            // relocate the wrong def and dangle the component. `ensure_component` recognises that
            // shape as a nullable alias before it reserves anything, so the root-component spelling
            // never arrives here; a sub-file or remote body reaching it is refused rather than
            // mis-assembled.
            if self.is_reservation(inner.id) {
                if self.reservation_at(&schema.provenance).is_some() {
                    return self.reject_self_referential_union(
                        schema,
                        "this schema's whole body is a union whose only non-null member is a \
                         `$ref` that closes a reference cycle, so the schema names no shape of its \
                         own and cannot be given a generated type",
                    );
                }
                inner.nullable = inner.nullable || nullable;
                inner.boxed = true;
                return Some(inner);
            }
            if let Some(sibling) = sibling {
                // The null-only MEMBER's branch was stripped out above, BEFORE this intersection,
                // so `inner` carries `nullable: false` and `type_accepts_null` — which reads
                // `Ty::nullable` — cannot see it. Restore exactly that branch. Without it the
                // intersection reports empty for a schema `null` genuinely satisfies, and the
                // `$ref` spelling of the identical instance set, where the target's nullability
                // rides on its own `Ty`, generates while this one rejects.
                //
                // `null_from_type_array` is deliberately NOT restored here: it supplies no branch,
                // and it already reaches the intersection on the sibling side, where it belongs —
                // it can narrow what the result accepts, never create something to accept.
                inner.nullable = inner.nullable || null_from_member;
                let mut reach = ScopeReach::default();
                let met = self.meet_refiner(
                    inner,
                    sibling.refiner,
                    &mut reach,
                    &format!("{hint}Constrained"),
                );
                if met.is_err() && reach.uncategorised {
                    return self.reject_unscoped_union_sibling(
                        schema,
                        "the union's sole non-null member states no JSON category, and the \
                         enclosing schema's untyped sibling keywords settle none for it — they are \
                         both object keywords and array keywords, or its `type` array admits \
                         another category beside theirs — so no single Rust type represents what \
                         they constrain of it",
                    );
                }
                for keywords in unreached_halves(sibling.refiner, &reach) {
                    self.warn_unreached_union_sibling(schema, unreached_message(keywords));
                }
                let Ok(constrained) = met else {
                    // Neither side admits null and the non-null shapes do not meet, so nothing is
                    // left to collapse to. The terminal code matches the multi-variant path below,
                    // which rejects with `E007` once every variant has been excluded — but only the
                    // terminal code: that path also emits a per-variant `W011`, and this one
                    // deliberately does not, because `W011` describes a construct dropped from
                    // output that still exists, and here no enum is generated at all.
                    //
                    // The message says "empty or unrepresentable" for the same reason Site A's
                    // does: it covers both, and the sole non-null member is named because there
                    // is exactly one, so the author needs no index to find it.
                    return self.reject_branchless_union(
                        schema,
                        "the union's sole non-null member and the enclosing schema's own sibling \
                         keywords have an empty or unrepresentable intersection, leaving the union \
                         with no variant",
                    );
                };
                // The intersection owns the answer for nullability too — but ONLY where the sibling
                // is entitled to give one, which is a question about the SIBLING. Reading the
                // enclosing schema's `type` instead answered it wrongly in both directions: the two
                // disagree whenever a multi-type array is deleted for lowering, and whenever the
                // sibling speaks through `enum`/`const` rather than `type`. An object applicator
                // denies nothing and must not remove the union's own acceptance.
                nullable = if sibling.speaks_about_null {
                    constrained.nullable
                } else {
                    member_nullable || null_from_member
                };
                inner = constrained;
            }
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            // The meet's result is re-emitted under this schema's name, so the meet's own inserts
            // (`…Constrained`, and whatever it built on the way) are unused unless `kind` reaches
            // them (#462). Without a sibling nothing was inserted since `mark`.
            self.discard_meet_intermediates(mark, &kind);
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = inner.nullable || nullable;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        // Lower every real variant first (their defs — especially `$ref` components — insert before
        // the union def below), recording the `$ref` component name for tag/variant naming.
        let mut variants: Vec<UnionVariant> = Vec::new();
        let mut ref_names: Vec<Option<String>> = Vec::new();
        // The real member each variant came from: sibling keywords can exclude a member, so a
        // variant's position is not its member's.
        let mut variant_members: Vec<usize> = Vec::new();
        // Whether each variant accepted `null` before its nullability was hoisted to the union.
        let mut variant_nullable: Vec<bool> = Vec::new();
        let mut used_hints: HashSet<String> = HashSet::new();
        let mut reach = ScopeReach::default();
        // The ids each member's sibling meet inserted. They interleave with the members' own
        // lowered types, which stay, so a re-emit below elides the unused ones rather than popping.
        let mut meet_inserts: Vec<std::ops::Range<u32>> = Vec::new();
        for (index, member) in real_members.iter().enumerate() {
            let (mut ty, ref_name) =
                self.lower_union_variant(member, &format!("{hint}Variant{index}"))?;
            // A variant that is a back-edge to *this* union — the member's type is the very
            // reservation this schema will occupy — is the whole union, so it constrains nothing and
            // cannot be decoded: the emitted `Deserialize` opens by re-entering itself on the same
            // value, with no base case, and recurses until the stack is exhausted. It compiles, so
            // nothing downstream can catch it; refusing here is the only place it can be caught.
            //
            // The test is against *this* schema's own reservation, not against any open one. Asking
            // `is_in_progress_root(ty.id)` answers the strictly weaker "is the member any open
            // reservation", which is true of every ordinary recursive schema whose union sits in a
            // property: `Node.child: {oneOf: [{$ref: Node}, …]}` has a member pointing at `Node`'s
            // reservation while the union being built is `Nodechild`. That decodes perfectly well —
            // the member is a *different* type — and rejecting it refuses the most common recursive
            // construct there is, which `docs/support-matrix.md` lists as supported.
            if self.reservation_at(&schema.provenance) == Some(ty.id) {
                return self.reject_self_referential_union(
                    schema,
                    "a union member is a direct recursive `$ref` to the union being lowered, so \
                     the member is the union itself and decoding it would never terminate",
                );
            }
            // The reservation half of the cycle test again, on a multi-variant union. Same fact,
            // same wording, same place in the order: before anything tries to intersect against the
            // placeholder. Guarded on there being a sibling at all, so an ordinary recursive
            // `oneOf` still boxes its back-edge and generates.
            if sibling.is_some() && self.is_in_progress_root(ty.id) {
                return self.reject_ref_sibling_cycle(
                    schema,
                    "this union member's `$ref` closes a reference cycle back to the schema that \
                     encloses it, so the enclosing schema's own sibling keywords would have to be \
                     intersected with a target whose definition depends on the result",
                );
            }
            if let Some(sibling) = sibling {
                let mark = self.graph_mark();
                let met = self.meet_refiner(
                    ty,
                    sibling.refiner,
                    &mut reach,
                    &format!("{hint}Variant{index}Constrained"),
                );
                meet_inserts.push(mark..self.graph_mark());
                ty = match met {
                    Ok(intersection) => intersection,
                    // The sibling constraints make this branch impossible; JSON Schema simply
                    // removes it from the union's accepted set. Acknowledge it, because a variant
                    // vanishing from the generated enum is otherwise invisible.
                    Err(NoMeet::Empty) => {
                        // W011 case: excluded-union-branch
                        Diagnostic::warning(
                            Code::DeclarationHasNoEffect,
                            schema.provenance.clone(),
                        )
                        .message(format!(
                            "union member {index} cannot satisfy the enclosing schema's own \
                             constraints, so it is not a variant of the generated enum"
                        ))
                        .emit(self.diags);
                        continue;
                    }
                    // Object and array applicators together beside a branch that states no
                    // category (`{}`) have no single category to establish for it.
                    Err(NoMeet::Unrepresentable) if reach.uncategorised => {
                        let message = format!(
                            "union member {index} states no JSON category, and the enclosing \
                             schema's untyped sibling keywords settle none for it — they are both \
                             object keywords and array keywords, or its `type` array admits \
                             another category beside theirs — so no single Rust type represents \
                             what they constrain of it"
                        );
                        return self.reject_unscoped_union_sibling(schema, &message);
                    }
                    // The branch does admit values the siblings admit, but no Rust type holds
                    // them, so it can be neither kept nor dropped without refusing them.
                    Err(NoMeet::Unrepresentable) => {
                        let message = format!(
                            "union member {index} and the enclosing schema's own sibling keywords \
                             share values that no single Rust type represents, so the member can \
                             be neither generated nor dropped"
                        );
                        return self.reject_unrepresentable_meet(schema, &message);
                    }
                };
            }
            // Hoist a variant's own nullability up to the union: a `null` payload then resolves at the
            // outer `Option<Union>` (→ `None`), and the discriminated/disjoint dispatch below only
            // ever inspects non-null content — otherwise a variant like `{type: [string, null]}`
            // would be categorized `String` yet have no `null` arm in the custom `Deserialize`.
            nullable = nullable || ty.nullable;
            variant_nullable.push(ty.nullable);
            ty.nullable = false;
            let base_hint = ref_name
                .clone()
                .unwrap_or_else(|| format!("{hint}Variant{index}"));
            // Keep hints unique so `name` allocates one identifier per variant (the hint keys the
            // per-union variant table).
            let mut name_hint = base_hint.clone();
            let mut disambiguator = 2usize;
            while !used_hints.insert(name_hint.clone()) {
                name_hint = format!("{base_hint}{disambiguator}");
                disambiguator += 1;
            }
            variants.push(UnionVariant { name_hint, ty });
            ref_names.push(ref_name);
            variant_members.push(index);
        }

        if let Some(sibling) = sibling {
            for keywords in unreached_halves(sibling.refiner, &reach) {
                self.warn_unreached_union_sibling(schema, unreached_message(keywords));
            }
        }
        if variants.is_empty() {
            // The same blind spot as the sole-member site above, on the pre-existing path: every
            // REAL variant is impossible, but a null-only member's branch was stripped out before
            // any of them were intersected, so it is not among the variants that just vanished.
            // When the sibling admits null too, `null` still satisfies the whole schema and the
            // exact JSON null type is the answer — the same type the all-null-members branch above
            // returns for the same reason. Again only the MEMBER-derived flag can rescue: a
            // `"null"` in the enclosing `type` array leaves nothing for `null` to match.
            if null_from_member
                && sibling.is_none_or(|sibling| self.refiner_accepts_null(sibling.refiner))
            {
                return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
            }
            return self.reject_branchless_union(
                schema,
                "union sibling constraints make every variant impossible",
            );
        }
        // A `oneOf` needs exactly one branch to match, and its typed trial matching decides that
        // by which variants decode. Variants that lower to the same generated type decode the same
        // values, so every value one of them accepts fails exactly-one: branches of nothing but
        // `required` beside `type: object` (#402), or bare ones that each lower to
        // `serde_json::Value`. Two inline objects of one structure, or two string enums of one
        // value set, are distinct generated items that decode the same values too (#492). They
        // become one variant — the whole union is that type when every
        // variant shares it, as the `$ref`-sibling collapse answers for the same branches — and
        // the distinctions the generated type does not carry are reported, not dropped in silence.
        // A discriminator tells such variants apart by tag, so it keeps them all; an `anyOf`
        // decodes with any one match, so it does too. A `$ref`'s own `oneOf` sibling is collapsed
        // by the `$ref` arm after the meet instead ([`Self::unmerged_union`]).
        if mode == UnionMode::OneOf
            && schema.discriminator.is_none()
            && self.unmerged_union.as_ref() != Some(&schema.provenance)
        {
            self.merge_indistinguishable_variants(
                schema,
                &mut variants,
                &mut ref_names,
                &mut variant_members,
                &variant_nullable,
                &mut nullable,
            );
        }
        if variants.len() == 1 {
            let inner = variants[0].ty;
            let kind = self.graph.get(inner.id).map(|def| def.kind.clone())?;
            // The sole variant is re-emitted under this schema's name, so the meets' inserts —
            // the surviving variant's `…Constrained` def, and whatever an excluded member's meet
            // left — are unused unless `kind` reaches them (#462).
            let reached = reachable_types(&self.graph, &kind_edges(&kind));
            for id in meet_inserts.into_iter().flatten().map(TypeId) {
                if !reached.contains(&id) {
                    self.graph.elide(id);
                }
            }
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = inner.nullable || nullable;
            ty.boxed = inner.boxed;
            return Some(ty);
        }

        let strategy = if let (Some(discriminator), Some(resolved)) =
            (&schema.discriminator, &discriminator_members)
        {
            // `discriminator_members` already refused a `defaultMapping` naming a non-member. A
            // member the enclosing schema's sibling keywords excluded (`W011`) is the one way left
            // for it to have no variant, and a fallback to a branch the enum does not have cannot
            // be quietly downgraded to another dispatch strategy either.
            let default_variant = match resolved.default {
                None => None,
                Some(member) => {
                    let Some(variant) = variant_members.iter().position(|&m| m == member) else {
                        return self.reject_unrepresentable_union(
                            schema,
                            "`discriminator.defaultMapping` names a member that the enclosing \
                             schema's own sibling keywords exclude, so there is no branch to fall \
                             back to",
                        );
                    };
                    Some(variant)
                }
            };
            let mut discriminated = self.discriminated_strategy(
                &variants,
                &ref_names,
                &variant_members,
                &discriminator.property_name,
                resolved,
                default_variant,
                mode,
            );
            if let Some(UnionStrategy::Discriminated {
                tags,
                categories,
                default_variant,
                untagged,
                ..
            }) = &mut discriminated
            {
                let unselectable: Vec<usize> = (0..variants.len())
                    .filter(|&index| {
                        categories[index].is_none()
                            && tags[index].is_empty()
                            && *default_variant != Some(index)
                    })
                    .collect();
                let component = |index: usize| {
                    ref_names[index]
                        .as_deref()
                        .filter(|name| is_schema_component_name(name))
                };
                // A component member whose implicit value a mapping key claims for another
                // member: the document routes that member's own name elsewhere, which no
                // dispatch can honour.
                if let Some(&index) = unselectable
                    .iter()
                    .find(|&&index| component(index).is_some())
                {
                    let implicit = component(index).unwrap_or_default().to_owned();
                    return self.reject_unselectable_discriminated_variant(
                        schema,
                        discriminator,
                        variant_members[index],
                        &implicit,
                    );
                }
                // A member that is no component — inline, or a pointer into another schema — has
                // no implicit value, and with no mapping entry naming it no tag selects it;
                // inventing a tag for it would be one no server sends. Where no variant carries a
                // tag at all, the discriminator dispatches nothing, and the members' own schemas
                // decode the union, as for one with no discriminator. Otherwise the tagged
                // members keep their dispatch — a tag that names one selects it, exactly as the
                // document says — and the untagged ones are tried by their schemas only when the
                // tag is absent or names no tagged member. Dropping the dispatch for the whole
                // union instead would let an `anyOf` trial pick a tagged member the payload's own
                // tag does not name.
                if !unselectable.is_empty() {
                    let members: Vec<usize> = unselectable
                        .iter()
                        .map(|&index| variant_members[index])
                        .collect();
                    let dispatches = tags.iter().any(|accepted| !accepted.is_empty());
                    self.warn_untagged_discriminated_members(discriminator, &members, dispatches);
                    if dispatches {
                        for &index in &unselectable {
                            untagged[index] = Some(
                                self.type_specificity(variants[index].ty, &mut HashSet::new()),
                            );
                        }
                    } else {
                        discriminated = None;
                    }
                }
            }
            discriminated
                .or_else(|| self.disjoint_strategy(&variants))
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        } else {
            self.disjoint_strategy(&variants)
                .unwrap_or_else(|| self.trial_strategy(&variants, mode))
        };

        let union = Union { variants, strategy };
        // A `$ref`'s own `oneOf` sibling is reported by the `$ref` arm once the meet has made its
        // branches what they are ([`Self::collapse_met_union`]), as its merge is.
        if self.unmerged_union.as_ref() != Some(&schema.provenance) {
            self.warn_untyped_one_of_variants(
                &schema.provenance,
                &union,
                &variant_members,
                "this `oneOf`'s",
                "member",
            );
        }
        let mut ty = self.insert_schema_type(schema, hint, TypeKind::Union(union));
        ty.nullable = nullable;
        Some(ty)
    }

    /// Report, as `W001` at `provenance`, the variants of a trial-matched `oneOf` (one whose decode
    /// requires exactly one variant to match) that lower to `serde_json::Value` beside another
    /// variant (#535). Such a variant accepts every value, so every value another variant accepts
    /// matches two of them and fails the exactly-one rule. The union is faithful to the document,
    /// which admits only the values no other branch accepts, but the generated enum does not show
    /// that its other variants never decode a value and fail to serialize one, so it is said.
    /// `labels` gives each variant's position for the message, named `noun` after `prefix`.
    ///
    /// An `anyOf` is not reported: its most specific match picks a typed variant for every value
    /// one accepts, so a `serde_json::Value` variant, the faithful lowering of an untyped member,
    /// takes only the rest. Neither is a discriminated or disjoint union, which tells the variant
    /// apart by its tag or its JSON category.
    fn warn_untyped_one_of_variants(
        &mut self,
        provenance: &Provenance,
        union: &Union,
        labels: &[usize],
        prefix: &str,
        noun: &str,
    ) {
        if union.variants.len() < 2
            || !matches!(
                union.strategy,
                UnionStrategy::Trial {
                    mode: UnionMode::OneOf,
                    ..
                }
            )
        {
            return;
        }
        let untyped: Vec<String> = union
            .variants
            .iter()
            .zip(labels)
            .filter(|(variant, _)| {
                matches!(
                    self.graph.get(variant.ty.id).map(|def| &def.kind),
                    Some(TypeKind::Any)
                )
            })
            .map(|(_, label)| label.to_string())
            .collect();
        if untyped.is_empty() {
            return;
        }
        let (noun, verb) = if untyped.len() == 1 {
            (noun.to_owned(), "lowers")
        } else {
            (format!("{noun}s"), "lower")
        };
        Diagnostic::warning(Code::ValidationKeywordIgnored, provenance.clone())
            .message(format!(
                "{prefix} {noun} {} {verb} to `serde_json::Value`, which accepts every value, so \
                 a value any other variant accepts matches two variants and fails the exactly-one \
                 rule: the other variants never decode a value and fail to serialize one, and \
                 only a value no other variant accepts decodes, as `serde_json::Value`",
                untyped.join(", ")
            ))
            .remedy(
                "give the untyped branch the type its values have, or use `anyOf` where a value \
                 may match more than one branch",
            )
            .emit(self.diags);
    }

    /// Merge the `oneOf` variants that decode the same values — that lower to the same generated
    /// type, or to distinct structs or string enums of one structure (#492,
    /// [`TypeGraph::same_decoded_values`]) — into the first of them, keeping `ref_names` and
    /// `variant_members` aligned with `variants`, and report the merge as
    /// `W001` at the union. `variant_nullable` is whether each variant accepted `null` before it
    /// was hoisted to the union: two merged variants that both did put `null` in two branches,
    /// which fails exactly-one whatever else the union admits, so `null` is then invalid.
    fn merge_indistinguishable_variants(
        &mut self,
        schema: &Schema,
        variants: &mut Vec<UnionVariant>,
        ref_names: &mut Vec<Option<String>>,
        variant_members: &mut Vec<usize>,
        variant_nullable: &[bool],
        nullable: &mut bool,
    ) {
        // Each kept variant, with the members merged into it and whether one of them accepted
        // `null`.
        let mut groups: Vec<(usize, Vec<usize>, bool)> = Vec::new();
        let mut null_twice = false;
        for (index, variant) in variants.iter().enumerate() {
            let shared = groups.iter_mut().find(|(kept, _, _)| {
                self.graph
                    .same_decoded_values(variants[*kept].ty, variant.ty)
            });
            match shared {
                Some((_, members, accepts_null)) => {
                    members.push(variant_members[index]);
                    null_twice |= *accepts_null && variant_nullable[index];
                    *accepts_null |= variant_nullable[index];
                }
                None => groups.push((index, vec![variant_members[index]], variant_nullable[index])),
            }
        }
        if groups.len() == variants.len() {
            return;
        }
        let merged: Vec<String> = groups
            .iter()
            .filter(|(_, members, _)| members.len() > 1)
            .map(|(_, members, _)| {
                let members: Vec<String> = members.iter().map(usize::to_string).collect();
                format!("members {}", members.join(", "))
            })
            .collect();
        let consequence = if groups.len() == 1 {
            "the union is that one type"
        } else {
            "each such set is one variant of the generated enum"
        };
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(format!(
                "this `oneOf`'s {} lower to the same generated type or to identically structured \
                 ones, differing at most in keywords it does not carry, so a value matching one \
                 matches all of them and would fail the exactly-one rule: {consequence}, and which \
                 of them a value matches is not enforced",
                merged.join(" and ")
            ))
            .remedy("keep producer-side validation for the union's branch constraints")
            .emit(self.diags);
        fn keep<T>(items: &mut Vec<T>, kept: &HashSet<usize>) {
            *items = std::mem::take(items)
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| kept.contains(&index).then_some(item))
                .collect();
        }
        let kept: HashSet<usize> = groups.iter().map(|(kept, _, _)| *kept).collect();
        keep(variants, &kept);
        keep(ref_names, &kept);
        keep(variant_members, &kept);
        if null_twice {
            *nullable = false;
        }
    }

    /// Lower shape-bearing keywords adjacent to `oneOf`/`anyOf` so every branch is intersected with
    /// them. A multi-non-null `type` array is already expressed by the union members and is removed
    /// here (its `null` member is handled by the union's outer nullability).
    fn lower_union_sibling(&mut self, schema: &Schema, hint: &str) -> Option<Option<UnionSibling>> {
        let mut sibling = schema.clone();
        sibling.one_of.clear();
        sibling.any_of.clear();
        sibling.discriminator = None;

        // Asked of the sibling's OWN keywords, and asked BEFORE the type array is deleted below.
        // `type`, `enum`, `const`, `$ref` and `allOf` each state something about `null` — an `enum`
        // either lists it or does not, a `$ref`'s target carries its own nullability. The object and
        // array applicators (`properties`, `patternProperties`, `required`, `additionalProperties`,
        // `items`, `prefixItems`) state nothing: in 2020-12 they are vacuously satisfied by every
        // instance of another category, `null` included.
        let speaks_about_null = !sibling.types.types.is_empty()
            || sibling.enum_values.is_some()
            || sibling.const_value.is_some()
            || sibling.reference.is_some()
            || !sibling.all_of.is_empty();
        let declared_types_admit_null = sibling.types.types.contains(&JsonType::Null);

        let non_null_types = sibling
            .types
            .types
            .iter()
            .filter(|kind| **kind != JsonType::Null)
            .count();
        // More than one non-null type has no single lowered representation, so the array is dropped
        // for lowering. That is a lowering convenience, not a statement about the document.
        let types_deleted = non_null_types > 1;
        if types_deleted {
            sibling.types.types.clear();
        }
        if !schema_has_shape_constraint(&sibling) {
            return Some(None);
        }
        // Untyped object or array applicators alone name no `type`, so `lower_schema` would lower
        // them to `TypeKind::Any`, which intersects as identity and drops them from every branch
        // in silence (#282). Nor do they establish their category for the whole union, as they do
        // beside a `$ref` to a single schema: `required: [a]` beside `oneOf: [string, Obj]` is
        // vacuous for the strings, and reading it as an object would drop that branch. Each set
        // refines the branches of its own category instead.
        //
        // A multi-type array deleted above still says which categories the union admits, so it
        // goes with them: the branches of the categories it omits are excluded as they would be
        // against it.
        if implied_applicator_category(&sibling).is_some() {
            let admits_null = !types_deleted || declared_types_admit_null;
            let allowed = types_deleted.then(|| CategoryMask::of(&schema.types.types));
            let scoped = self.lower_scoped_refiners(&sibling, admits_null, allowed, hint)?;
            return Some(Some(UnionSibling {
                refiner: Refiner::Scoped(scoped),
                speaks_about_null,
            }));
        }
        let mut ty = self.lower_schema(&sibling, &format!("{hint}Constraint"))?;
        if types_deleted {
            // Restore the one piece of the deleted array that still has a faithful representation.
            // Without this the lowered sibling reads as null-rejecting for an array that admitted
            // null, and the union's acceptance is removed by a deletion the author never wrote.
            ty.nullable = declared_types_admit_null;
        }
        Some(Some(UnionSibling {
            refiner: Refiner::Whole(ty),
            speaks_about_null,
        }))
    }

    /// Lower a sibling of untyped object or array applicators (one [`implied_applicator_category`]
    /// answers for) into the [`ScopedRefiners`] its two halves refine. Each half is the sibling
    /// with the other half's keywords removed and its own category as `type`, with `null` admitted
    /// where `admits_null` says the sibling does not deny it. `allowed` is the categories of a
    /// multi-type array deleted for lowering, where there was one.
    fn lower_scoped_refiners(
        &mut self,
        sibling: &Schema,
        admits_null: bool,
        allowed: Option<CategoryMask>,
        hint: &str,
    ) -> Option<ScopedRefiners> {
        // `types` is empty here, so this is exactly the object applicators.
        let object_like = schema_is_object_like(sibling);
        let array_like = sibling.items.is_some() || !sibling.prefix_items.is_empty();
        let category = |kind: JsonType| {
            if admits_null {
                vec![kind, JsonType::Null]
            } else {
                vec![kind]
            }
        };
        let object = if object_like {
            let mut half = sibling.clone();
            half.items = None;
            half.prefix_items.clear();
            half.types.types = category(JsonType::Object);
            Some(self.lower_schema(&half, &format!("{hint}Constraint"))?)
        } else {
            None
        };
        let array = if array_like {
            let mut half = sibling.clone();
            half.properties.clear();
            half.pattern_properties.clear();
            half.required.clear();
            half.additional_properties = None;
            half.types.types = category(JsonType::Array);
            let name = if object_like {
                "ArrayConstraint"
            } else {
                "Constraint"
            };
            Some(self.lower_schema(&half, &format!("{hint}{name}"))?)
        } else {
            None
        };
        Some(ScopedRefiners {
            object,
            array,
            admits_null,
            allowed,
        })
    }

    /// Meet one union branch with `refiner`. A [`Refiner::Whole`] sibling is intersected with it.
    /// A [`Refiner::Scoped`] one meets an object branch with its object half and an array branch
    /// with its array half, recording in `reach` which half reached one; a branch of another
    /// category is left as it is, but for a `null` the sibling denies. A nested union is met
    /// branch by branch. A branch of a category [`ScopedRefiners::allowed`] omits is excluded. A
    /// branch that states no category (`{}`) takes the one the sibling establishes, as an untyped
    /// `$ref` target does; where the sibling carries both kinds, or `allowed` admits another
    /// category too, there is none to establish, and the meet is [`NoMeet::Unrepresentable`] with
    /// `reach.uncategorised` set.
    fn meet_refiner(
        &mut self,
        branch: Ty,
        refiner: Refiner,
        reach: &mut ScopeReach,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let scoped = match refiner {
            Refiner::Whole(ty) => return self.intersect_types(branch, ty, hint),
            Refiner::Scoped(scoped) => scoped,
        };
        let Some(kind) = self.graph.get(branch.id).map(|def| def.kind.clone()) else {
            return Err(NoMeet::Unrepresentable);
        };
        // A branch of a category the deleted `type` array omits is excluded, as against the array.
        if let (Some(allowed), Some(category)) = (scoped.allowed, value_category(&kind)) {
            if !allowed.admits(category) {
                return if branch.nullable && scoped.admits_null {
                    Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
                } else {
                    Err(NoMeet::Empty)
                };
            }
        }
        let half = match &kind {
            TypeKind::Union(union) => {
                return self.meet_scoped_refiner_with_union(branch, union, refiner, hint, reach);
            }
            TypeKind::Struct(_) => {
                reach.object |= scoped.object.is_some();
                scoped.object
            }
            TypeKind::Array(_) | TypeKind::Tuple(_) => {
                reach.array |= scoped.array.is_some();
                scoped.array
            }
            // A branch that states no category (`{}`, or `{required: [x]}` alone) is the untyped
            // target the `$ref` arm meets: there the applicators establish their category, so
            // `properties` beside `anyOf: [{required: [a]}, {required: [b]}]` is an object in
            // every branch. Both kinds at once have no single category to establish, and nor
            // does one kind beside a deleted `type` array that admits another category too.
            TypeKind::Any => {
                let (half, category) = match (scoped.object, scoped.array) {
                    (Some(object), None) => (object, JsonCategory::Object),
                    (None, Some(array)) => (array, JsonCategory::Array),
                    _ => {
                        reach.uncategorised = true;
                        return Err(NoMeet::Unrepresentable);
                    }
                };
                match scoped.allowed {
                    Some(allowed) if allowed.admits_besides(category) => {
                        reach.uncategorised = true;
                        return Err(NoMeet::Unrepresentable);
                    }
                    Some(allowed) if !allowed.admits(category) => {
                        return if branch.nullable && scoped.admits_null {
                            Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
                        } else {
                            Err(NoMeet::Empty)
                        };
                    }
                    _ => {}
                }
                if category == JsonCategory::Object {
                    reach.object = true;
                } else {
                    reach.array = true;
                }
                Some(half)
            }
            // A placeholder's body is not known yet, so nothing can be said of its category. The
            // callers refuse a reservation before they get here; this keeps the refusal for one
            // that does not.
            TypeKind::Reserved => return Err(NoMeet::Unrepresentable),
            TypeKind::Primitive(_)
            | TypeKind::Enum(_)
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never => None,
        };
        match half {
            Some(half) => self.intersect_types(branch, half, hint),
            None => {
                let mut ty = branch;
                ty.nullable = branch.nullable && scoped.admits_null;
                Ok(ty)
            }
        }
    }

    /// The `$ref` arm's answer for a `$ref` to the union `union` (its target, `referenced`) whose
    /// siblings are untyped object or array applicators alone: each set refines the target's
    /// branches of its own category, and the rest are kept as they are. Such siblings say nothing
    /// about `null`, so the target's nullability stands. A set that reaches no branch of its
    /// category is vacuous, and `W011`; a branch that states no category with no single one to
    /// establish for it, and a meet that leaves no branch, are `E013`.
    fn refine_union_target(
        &mut self,
        schema: &Schema,
        hint: &str,
        referenced: Ty,
        union: &Union,
        sibling: &Schema,
    ) -> Option<Ty> {
        let scoped = self.lower_scoped_refiners(sibling, true, None, hint)?;
        let refiner = Refiner::Scoped(scoped);
        let mut reach = ScopeReach::default();
        let mark = self.graph_mark();
        let met = self.meet_scoped_refiner_with_union(
            referenced,
            union,
            refiner,
            &format!("{hint}ReferenceIntersection"),
            &mut reach,
        );
        if met.is_err() && reach.uncategorised {
            return self.reject_ref_sibling_category(
                schema,
                "a branch of this `$ref`'s target union states no JSON category, and the untyped \
                 sibling keywords are both object keywords and array keywords with no `type` to \
                 choose between them, so no single Rust type represents what they constrain of it",
            );
        }
        for keywords in unreached_halves(refiner, &reach) {
            let message = format!(
                "this `$ref`'s untyped sibling {keywords} constrain only the instances of their \
                 own category, and no branch of its target union has that category, so they \
                 constrain no value the target accepts"
            );
            self.warn_unreached_union_sibling(schema, message);
        }
        let Ok(met) = met else {
            return self.reject_ref_sibling_intersection(schema);
        };
        let kind = self.graph.get(met.id)?.kind.clone();
        // The meet carries the target's nullability, but for a meet that left only `null`: that
        // is the exact null type, whose one value is `null` already, so it is not wrapped in an
        // `Option` as the `allOf` and inline spellings of the same union are not (#450).
        let nullable = referenced.nullable && !matches!(kind, TypeKind::Null);
        // The met union is re-emitted under this schema's name, so its own def
        // (`…ReferenceIntersection`) is unused; the branches it met stay where `kind` reaches
        // them (#462).
        self.discard_meet_intermediates(mark, &kind);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        ty.nullable = nullable;
        ty.boxed = met.boxed;
        Some(ty)
    }

    /// Meet `target` with the untyped applicators `refiner` carries: branch by branch where
    /// `target` is a union, keeping every branch of another category, and as that one branch
    /// otherwise. [`Self::refine_union_target`]'s meet, for a target that need not be a union.
    fn meet_scoped_refiner(
        &mut self,
        target: Ty,
        refiner: Refiner,
        hint: &str,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        match self.graph.get(target.id).map(|def| &def.kind) {
            Some(TypeKind::Union(union)) => {
                let union = union.clone();
                self.meet_scoped_refiner_with_union(target, &union, refiner, hint, reach)
            }
            // A placeholder's body is not known yet, so nothing can be said of its category,
            // exactly as `meet_refiner` answers for one.
            Some(TypeKind::Reserved) => Err(NoMeet::Unrepresentable),
            _ => self.closed_narrowing(|ctx| ctx.meet_refiner(target, refiner, reach, hint)),
        }
    }

    /// [`Self::meet_scoped_refiner`] for a `target` whose kind is `union`. The meet admits `null`
    /// exactly when `target` and `refiner` both do: [`Self::intersect_union`] builds its result
    /// from the non-null branches alone, and a union's `null` branch is its outer nullability,
    /// so it is carried across here rather than lost with the rebuilt union. For the same reason,
    /// a meet that excludes every non-null branch is not empty while both still admit `null`:
    /// `null` is then the only value left, and the meet is the exact JSON null type, as the inline
    /// sibling spelling of the same union answers (#450).
    fn meet_scoped_refiner_with_union(
        &mut self,
        target: Ty,
        union: &Union,
        refiner: Refiner,
        hint: &str,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        let enclosing = self.narrowing_opens;
        let accepts_null = target.nullable && self.refiner_accepts_null(refiner);
        match self.closed_narrowing(|ctx| {
            ctx.intersect_union(target, union, refiner, hint, enclosing, reach)
        }) {
            Ok(mut ty) => {
                ty.nullable = accepts_null;
                Ok(ty)
            }
            Err(NoMeet::Empty) if accepts_null => {
                Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
            }
            Err(no_meet) => Err(no_meet),
        }
    }

    /// Whether a union branch met with `refiner` may still be `null`, for a union whose every
    /// real branch was excluded: the sibling's own answer.
    fn refiner_accepts_null(&self, refiner: Refiner) -> bool {
        match refiner {
            Refiner::Whole(ty) => self.ty_accepts_null(ty),
            Refiner::Scoped(scoped) => scoped.admits_null,
        }
    }

    /// Lower one union member, returning its type and — when the member is a `$ref` to a component —
    /// that component's name (used to derive the variant name and implicit discriminator tag).
    ///
    /// The name is a fact about how the member is *written*; the type is what it *means*. A bare
    /// component `$ref` means its target, so it is that component's shared type. A `$ref` beside
    /// shape-bearing siblings means the intersection of the two — `$ref` is an applicator in
    /// 2020-12 — so it lowers through `lower_schema_or`, the same `$ref`-sibling intersection every
    /// other position takes (an empty or unrepresentable one is `E013` at the member), and keeps
    /// the component name only for naming. Returning the target here instead discarded the
    /// siblings in silence (#279).
    fn lower_union_variant(
        &mut self,
        member: &SchemaOr,
        hint: &str,
    ) -> Option<(Ty, Option<String>)> {
        let root = self.resolver.root_id();
        if let (Some(name), SchemaOr::Schema(schema)) =
            (member_component_name(member, root), member)
        {
            let mut sibling = schema.as_ref().clone();
            sibling.reference = None;
            let ty = if schema_has_shape_constraint(&sibling) {
                self.lower_schema_or(member, hint)?
            } else {
                // The member itself never reaches `lower_schema_inner`, which reports this.
                self.diagnose_standalone_discriminator(schema);
                self.ensure_component(name, schema.reference.as_deref(), &schema.provenance)?
            };
            return Some((ty, Some(name.to_owned())));
        }
        let ty = self.lower_schema_or(member, hint)?;
        Some((ty, None))
    }

    /// Resolve every schema a union's Discriminator Object names — each `mapping` value, then
    /// `defaultMapping` — to the union member it denotes.
    ///
    /// A value is a component name or a URI reference. The specification recommends reading a value
    /// that could be either as a name, and a name is exactly a Components Object key, so a value
    /// made only of key characters is `#/components/schemas/<value>` and anything else is a
    /// reference, written relative to the file the discriminator sits in. The two sides are compared
    /// by resolved `file#pointer` ([`Self::schema_reference_identity`]), never by spelling, so
    /// `Cat`, `#/components/schemas/Cat` and `./openapi.yaml#/components/schemas/Cat` all name one
    /// member. Only `$ref` members can be named: the specification excludes inline members from
    /// name mapping.
    ///
    /// Every entry is checked, and each failure is reported at the entry itself: one naming no
    /// schema the loaded description holds is `E004`, like any other reference that cannot be
    /// followed; one naming a schema the union does not list is `E007`, because the tag it describes
    /// has no variant to decode into and the specification requires every possible schema to be
    /// listed beside the discriminator.
    fn discriminator_members(
        &mut self,
        discriminator: &super::Discriminator,
        members: &[&SchemaOr],
    ) -> Option<DiscriminatorMembers> {
        let member_identities: Vec<_> = members
            .iter()
            .map(|member| match member {
                SchemaOr::Schema(schema) => schema.reference.as_deref().and_then(|reference| {
                    self.schema_reference_identity(reference, &schema.provenance)
                }),
                SchemaOr::Bool(_) => None,
            })
            .collect();
        let entries = discriminator
            .mapping
            .iter()
            .map(|(tag, target)| (Some(tag), target))
            .chain(
                discriminator
                    .default_mapping
                    .iter()
                    .map(|target| (None, target)),
            );
        let mut resolved = DiscriminatorMembers {
            mapping: Vec::new(),
            default: None,
        };
        let mut failed = false;
        for (tag, target) in entries {
            let entry = discriminator_entry(tag);
            let value = &target.value;
            let Some(identity) = self.discriminator_target_identity(&entry, target) else {
                failed = true;
                continue;
            };
            let Some(member) = member_identities
                .iter()
                .position(|member| member.as_ref() == Some(&identity))
            else {
                let consequence = match tag {
                    Some(tag) => format!("a payload tagged `{tag}` has no variant to decode into"),
                    None => "there is no branch to fall back to".to_owned(),
                };
                // E007 case: unrepresentable-applicators
                Diagnostic::error(Code::NonDisjointUnion, target.provenance.clone())
                    .message(format!(
                        "{entry} names `{value}`, which is not one of this union's `$ref` \
                         members, so {consequence}"
                    ))
                    .remedy(
                        "list the schema as a `$ref` member of the union beside the \
                         discriminator, or remove the entry",
                    )
                    .emit(self.diags);
                failed = true;
                continue;
            };
            match tag {
                Some(tag) => resolved.mapping.push((tag.clone(), member)),
                None => resolved.default = Some(member),
            }
        }
        (!failed).then_some(resolved)
    }

    /// [`discriminator_target_identity`] against this lowering's document and resolver.
    fn discriminator_target_identity(
        &mut self,
        entry: &str,
        target: &super::schema::DiscriminatorTarget,
    ) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
        discriminator_target_identity(self.document, self.resolver, self.diags, entry, target)
    }

    /// Give a `discriminator` on a schema with no `oneOf`/`anyOf` of its own a disposition (#264).
    ///
    /// spargen dispatches by discriminator only across the members of the union it sits beside,
    /// so here it selects nothing: the schema lowers by its other keywords, and the `allOf`
    /// polymorphism form — children reaching this schema through `allOf`, decoded by the tag into
    /// whichever child it names — is not generated, since no keyword of the parent lists its
    /// children. That is `W011` at the Discriminator Object. Every `mapping` and `defaultMapping`
    /// value is still a reference to a schema, so each is resolved as a union's would be, and one
    /// naming no schema is `E004` at the entry; with no union, there is no membership to check.
    /// Called wherever a schema is lowered or gathered as an `allOf` member by its keywords, and
    /// idempotent there: a repeated report at one site is the same diagnostic, which
    /// [`Diagnostics`] keeps once.
    fn diagnose_standalone_discriminator(&mut self, schema: &Schema) {
        let Some(discriminator) = &schema.discriminator else {
            return;
        };
        if !schema.one_of.is_empty() || !schema.any_of.is_empty() {
            return;
        }
        let entries = discriminator
            .mapping
            .iter()
            .map(|(tag, target)| (Some(tag), target))
            .chain(
                discriminator
                    .default_mapping
                    .iter()
                    .map(|target| (None, target)),
            );
        for (tag, target) in entries {
            self.discriminator_target_identity(&discriminator_entry(tag), target);
        }
        // W011 case: standalone-discriminator
        Diagnostic::warning(
            Code::DeclarationHasNoEffect,
            discriminator.provenance.clone(),
        )
        .message(
            "this `discriminator` has no `oneOf` or `anyOf` beside it, so it selects nothing: the \
             schema lowers by its other keywords alone, and the `allOf` polymorphism form, which \
             decodes a payload into whichever child schema its tag names, is not generated",
        )
        .remedy(
            "list every schema the tag can select in a `oneOf` beside the discriminator, or remove \
             the discriminator",
        )
        .emit(self.diags);
    }

    /// [`schema_reference_identity`] against this lowering's document and resolver.
    fn schema_reference_identity(
        &self,
        reference: &str,
        at: &Provenance,
    ) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
        schema_reference_identity(self.document, self.resolver, reference, at)
    }

    /// Build the discriminated fast path. Objects route by tag; a non-object variant routes by its
    /// unique JSON category. An object variant is selected by every `discriminator.mapping` key
    /// naming its member, in document order, and then by its own `$ref` component name unless a
    /// mapping key claims that value; the first is the tag serialization writes. Every variant
    /// starts with no `untagged` priority: the caller refuses a component variant left with no
    /// tag ([`Self::reject_unselectable_discriminated_variant`]) and decides how any other is
    /// reached.
    #[allow(clippy::too_many_arguments)]
    fn discriminated_strategy(
        &self,
        variants: &[UnionVariant],
        ref_names: &[Option<String>],
        variant_members: &[usize],
        tag_field: &str,
        discriminator: &DiscriminatorMembers,
        default_variant: Option<usize>,
        mode: UnionMode,
    ) -> Option<UnionStrategy> {
        let mut tags = Vec::new();
        let mut categories = Vec::new();
        for ((variant, ref_name), member) in variants.iter().zip(ref_names).zip(variant_members) {
            if !matches!(
                self.graph.get(variant.ty.id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            ) {
                let category = self.json_category(variant.ty)?;
                if category == JsonCategory::Object || categories.contains(&Some(category)) {
                    return None;
                }
                tags.push(Vec::new());
                categories.push(Some(category));
                continue;
            }
            // Every explicit mapping entry naming this variant's member — already resolved by
            // identity, so its spelling does not matter — selects it. So does its component name,
            // because the specification reads a value as a component name "unless a `mapping` is
            // present for that value": a key equal to it claims it, for this member or another.
            let mut accepted: Vec<String> = discriminator
                .mapping
                .iter()
                .filter(|(_, named)| named == member)
                .map(|(tag, _)| tag.clone())
                .collect();
            let component = ref_name
                .as_deref()
                .filter(|name| is_schema_component_name(name));
            // A member that is no component — inline, or a deeper pointer — has no implicit value
            // ("inline `oneOf` or `anyOf` subschemas are not considered"), so only a mapping key
            // selects it. With none it keeps no tag at all: anything else would be one spargen
            // made up and no server sends. The caller decides how such a member is reached.
            if let Some(name) = component {
                if !discriminator.mapping.iter().any(|(tag, _)| tag == name) {
                    accepted.push(name.to_owned());
                }
            }
            tags.push(accepted);
            categories.push(None);
        }
        Some(UnionStrategy::Discriminated {
            tag_field: tag_field.to_owned(),
            untagged: vec![None; tags.len()],
            tags,
            categories,
            default_variant,
            mode,
        })
    }

    /// Build the disjoint fast path for an undiscriminated union. Two proofs are attempted:
    ///
    /// 1. **JSON-type-disjoint**: every variant occupies a distinct JSON primitive category
    ///    (`number` and `integer` share one category, so they never separate).
    /// 2. **Required-key-disjoint**: every variant is a *closed* object (`additionalProperties:
    ///    false`) with at least one required property whose name appears in no other variant. Closed
    ///    is essential — an open object could carry another variant's unique key as an extra field
    ///    and be misrouted, so open-object required-key unions are never provably disjoint.
    fn disjoint_strategy(&self, variants: &[UnionVariant]) -> Option<UnionStrategy> {
        // Proof 1: pairwise-distinct JSON type categories.
        let categories: Option<Vec<JsonCategory>> =
            variants.iter().map(|v| self.json_category(v.ty)).collect();
        if let Some(categories) = categories {
            let all_distinct = categories.iter().enumerate().all(|(i, cat)| {
                categories
                    .iter()
                    .enumerate()
                    .all(|(j, other)| i == j || cat != other)
            });
            if all_distinct {
                return Some(UnionStrategy::Disjoint {
                    features: categories
                        .into_iter()
                        .map(DisjointFeature::JsonType)
                        .collect(),
                });
            }
        }

        // Proof 2: object variants each carrying a unique required key.
        if let Some(keys) = self.required_key_features(variants) {
            return Some(UnionStrategy::Disjoint {
                features: keys.into_iter().map(DisjointFeature::RequiredKey).collect(),
            });
        }

        None
    }

    fn trial_strategy(&self, variants: &[UnionVariant], mode: UnionMode) -> UnionStrategy {
        UnionStrategy::Trial {
            mode,
            priorities: variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, &mut HashSet::new()))
                .collect(),
        }
    }

    fn type_specificity(&self, ty: Ty, visiting: &mut HashSet<TypeId>) -> u32 {
        if !visiting.insert(ty.id) {
            return 0;
        }
        let priority = match self.graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Enum(enumeration)) => {
                2_000_u32.saturating_sub(enumeration.variants.len() as u32)
            }
            Some(TypeKind::Null) => 3_000,
            Some(TypeKind::Never) => 4_000,
            Some(TypeKind::Struct(object)) => {
                let required = object.fields.iter().filter(|field| field.required).count() as u32;
                1_000 + required * 20 + object.fields.len() as u32
            }
            Some(TypeKind::Tuple(items)) => 900 + items.len() as u32,
            Some(TypeKind::Array(item)) => 800 + self.type_specificity(**item, visiting) / 10,
            Some(TypeKind::Primitive(Prim::I32)) => 700,
            Some(TypeKind::Primitive(Prim::I64)) => 650,
            Some(TypeKind::Primitive(Prim::Uuid | Prim::DateTime | Prim::Date)) => 600,
            Some(TypeKind::Primitive(Prim::F64 | Prim::String | Prim::Bool) | TypeKind::Bytes) => {
                500
            }
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .map(|variant| self.type_specificity(variant.ty, visiting))
                .min()
                .unwrap_or(0),
            // A reservation has no body yet, so there is nothing to rank: least specific, alongside
            // `Any` and a missing definition.
            //
            // This arm is **live**, not defensive. An earlier comment here claimed a union holding a
            // reservation was rejected before ranking, and named a function that has never existed.
            // Neither half was true: `lower_union`'s guard tests the *direct* member's id, while this
            // function recurses through `Array` and `Union`, so an array-wrapped back edge —
            // `anyOf: [{type: array, items: {$ref: self}}, …]` — reaches here on a document that
            // generates cleanly, and the value returned is emitted into the client as the trial-match
            // order of an `anyOf`. Ranking it least specific is the answer that matches what is known
            // about it, which is nothing; it is pinned by
            // `an_array_wrapped_union_back_edge_ranks_least_specific`.
            Some(TypeKind::Reserved) => 0,
            Some(TypeKind::Any) | None => 0,
        };
        visiting.remove(&ty.id);
        priority
    }

    /// The JSON primitive category a lowered variant type serializes as, or `None` when it cannot be
    /// statically categorized (an untyped `Any`, raw `Bytes`, or a nested union).
    fn json_category(&self, ty: Ty) -> Option<JsonCategory> {
        Some(match &self.graph.get(ty.id)?.kind {
            TypeKind::Primitive(Prim::Bool) => JsonCategory::Boolean,
            TypeKind::Primitive(Prim::I32 | Prim::I64 | Prim::F64) => JsonCategory::Number,
            TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date) => {
                JsonCategory::String
            }
            TypeKind::Struct(_) => JsonCategory::Object,
            TypeKind::Array(_) | TypeKind::Tuple(_) => JsonCategory::Array,
            TypeKind::Enum(enumeration) => match enumeration.repr {
                ScalarRepr::String => JsonCategory::String,
                ScalarRepr::Int => JsonCategory::Number,
                ScalarRepr::Bool => JsonCategory::Boolean,
            },
            // A reservation cannot be categorised — its body has not been lowered, so nothing is
            // known about the JSON it serialises as. Uncategorisable, exactly like the others here.
            //
            // This arm is **live** on documents that generate cleanly. `lower_union` refuses a
            // member that is *this* union's own reservation, but not one that is another open
            // component's: `Tree: {type: array, items: {oneOf: [{$ref: Tree}, {type: string}]}}`
            // lowers the items union while `Tree` is still reserved, and both `disjoint_strategy`
            // and `discriminated_strategy` ask for the back edge's category. Guessing one (a
            // reservation is usually an object) would emit a disjoint `Deserialize` that routes the
            // back edge by `value.is_object()`, and a `Tree` — an array — would then match no
            // variant at runtime. `None` sends the union to trial matching, which decodes it.
            // Pinned by `a_union_back_edge_to_an_open_component_is_not_categorised`.
            TypeKind::Reserved
            | TypeKind::Bytes
            | TypeKind::Null
            | TypeKind::Never
            | TypeKind::Any
            | TypeKind::Union(_) => return None,
        })
    }

    /// If every variant lowers to a *closed* object (`additionalProperties: false`) with at least
    /// one required property whose name appears in no other variant, return that unique required key
    /// per variant (source order); else `None`. Closed is required for soundness: an open object
    /// could carry another variant's unique key as an extra field, misrouting the payload.
    fn required_key_features(&self, variants: &[UnionVariant]) -> Option<Vec<String>> {
        let structs: Option<Vec<&Struct>> = variants
            .iter()
            .map(|v| match &self.graph.get(v.ty.id)?.kind {
                // Only closed objects are sound discriminators by required-key presence.
                TypeKind::Struct(structure)
                    if matches!(structure.additional, AdditionalProps::Deny) =>
                {
                    Some(structure)
                }
                // A reservation's fields are not known yet, so no required key can be proven
                // unique to it: not a sound discriminator, like any non-closed variant.
                TypeKind::Reserved => None,
                _ => None,
            })
            .collect();
        let structs = structs?;
        let mut keys = Vec::new();
        for (index, structure) in structs.iter().enumerate() {
            let others: HashSet<&str> = structs
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .flat_map(|(_, s)| s.fields.iter().map(|f| f.name.wire.as_str()))
                .collect();
            let key = structure
                .fields
                .iter()
                .find(|field| field.required && !others.contains(field.name.wire.as_str()))?;
            keys.push(key.name.wire.clone());
        }
        Some(keys)
    }

    /// A union whose applicators or discriminator describe a combination no generated enum can
    /// carry: `oneOf` beside `anyOf`, or a `defaultMapping` whose member the enclosing schema's
    /// sibling keywords excluded, so the fallback has no branch. A `mapping`/`defaultMapping` entry
    /// naming a schema that is not a member is reported at the entry by
    /// [`Self::discriminator_members`].
    fn reject_unrepresentable_union<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        // E007 case: unrepresentable-applicators
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "split the applicators into separate schemas, make every discriminator mapping name \
                 a member of the union, or omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// A discriminated object member no discriminator value selects: no `mapping` key names it, and
    /// a key equal to its component name claims that value for another member, so the dispatch has
    /// no arm that decodes into it, and the tag it would serialize decodes as that other member.
    /// Reported at the entry that claims the name. A member `defaultMapping` names is still reached
    /// by the fallback and never comes here.
    fn reject_unselectable_discriminated_variant<T>(
        &mut self,
        schema: &Schema,
        discriminator: &super::Discriminator,
        member: usize,
        implicit: &str,
    ) -> Option<T> {
        let claim = discriminator.mapping.get_key_value(implicit);
        let (provenance, message) = match claim {
            Some((tag, target)) => (
                target.provenance.clone(),
                format!(
                    "`discriminator.mapping` entry `{tag}` claims the component name of union \
                     member {member} for `{}`, and no entry names member {member}, so no \
                     discriminator value selects it",
                    target.value
                ),
            ),
            None => (
                schema.provenance.clone(),
                format!("no discriminator value selects union member {member}"),
            ),
        };
        // E007 case: unrepresentable-applicators
        Diagnostic::error(Code::NonDisjointUnion, provenance)
            .message(message)
            .remedy(
                "add a `discriminator.mapping` entry naming the member, rename the entry that \
                 claims its component name, or remove the member from the union",
            )
            .emit(self.diags);
        None
    }

    /// Discriminated object members that are no schema component — inline, or a pointer into
    /// another schema — and that no `mapping` entry or `defaultMapping` names. The specification
    /// gives them no implicit value ("inline `oneOf` or `anyOf` subschemas are not considered"),
    /// so no discriminator value selects them. Where another variant carries a tag (`dispatches`),
    /// the caller keeps the tag dispatch for those and tries these by their schemas when the tag
    /// is absent or names no tagged member; otherwise the discriminator dispatches nothing and the
    /// union is decoded by its members' schemas, as one with no discriminator. Either way no tag
    /// is invented for them; this says so at the discriminator.
    fn warn_untagged_discriminated_members(
        &mut self,
        discriminator: &super::Discriminator,
        members: &[usize],
        dispatches: bool,
    ) {
        let list = members
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let (subject, pronoun) = if members.len() == 1 {
            (
                format!(
                    "union member {list} is no schema component — inline, or a pointer into \
                     another schema — so it has no implicit discriminator value"
                ),
                "it",
            )
        } else {
            (
                format!(
                    "union members {list} are no schema components — inline, or pointers into \
                     another schema — so they have no implicit discriminator value"
                ),
                "them",
            )
        };
        let consequence = if dispatches {
            let (verb, possessive) = if members.len() == 1 {
                ("it is", "its")
            } else {
                ("they are", "their")
            };
            format!(
                "so the tag dispatches only to the members a value names, and {verb} matched by \
                 {possessive} own schema when the tag is absent or names no such member"
            )
        } else {
            "so this `discriminator` dispatches nothing and the union is decoded by its members' \
             schemas"
                .to_owned()
        };
        // W011 case: untagged-discriminated-member
        Diagnostic::warning(
            Code::DeclarationHasNoEffect,
            discriminator.provenance.clone(),
        )
        .message(format!(
            "{subject}, and no `discriminator.mapping` entry names {pronoun}: no discriminator \
             value selects {pronoun}, {consequence}"
        ))
        .remedy(
            "add a `discriminator.mapping` entry naming each such member (a URI reference to it \
             works), move it to a schema component of its own, or remove the discriminator",
        )
        .emit(self.diags);
    }

    /// A union that resolves to itself, so its generated `Deserialize` would re-enter itself on the
    /// same input with no base case.
    fn reject_self_referential_union<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        // E007 case: resolves-to-itself
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "break the reference cycle where a member refers to the union it is written in, or \
                 omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// A union the enclosing schema's own sibling keywords leave with no branch at all.
    fn reject_branchless_union<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        // E007 case: no-branch-left
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "reconcile the enclosing schema's sibling keywords with the union's members, or \
                 omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// Lower an object schema's `properties`/`required`/`additionalProperties` into the pieces of a
    /// [`Struct`] *without* inserting the struct itself. Shared by [`Self::lower_object`] and the
    /// `allOf` merge, which collects field/additional pieces from several members before inserting a
    /// single merged struct as the final graph insert (the `ensure_component` last-insert invariant).
    fn object_body(
        &mut self,
        schema: &Schema,
        hint: &str,
    ) -> Option<(Vec<Field>, AdditionalProps)> {
        let required = schema.required.iter().cloned().collect::<HashSet<_>>();
        let mut fields = Vec::new();
        for (name, child) in &schema.properties {
            let ty = self.lower_schema_or(child, &format!("{hint}{name}"))?;
            let is_required = required.contains(name);
            let default = self.field_default(child, ty, is_required);
            let xml = self.field_xml(child);
            let (deprecated, read_only, write_only) = field_flags(child);
            fields.push(Field {
                name: PropertyName { wire: name.clone() },
                ty,
                required: is_required,
                deprecated,
                read_only,
                write_only,
                default,
                xml,
                undeclared: false,
            });
        }
        let additional = if schema.pattern_properties.is_empty() {
            match &schema.additional_properties {
                Some(schema) => match schema.as_ref() {
                    SchemaOr::Bool(false) => AdditionalProps::Deny,
                    SchemaOr::Bool(true) => AdditionalProps::Allow,
                    schema => {
                        let mut ty = self.lower_schema_or(schema, &format!("{hint}Additional"))?;
                        self.warn_structural_default_or(schema, "an `additionalProperties` value");
                        // A map value lives behind the map's own indirection; a cycle-closing ref
                        // here needs no `Box`.
                        ty.boxed = false;
                        AdditionalProps::Typed(Box::new(ty))
                    }
                },
                None => AdditionalProps::Allow,
            }
        } else {
            self.lower_pattern_additional(schema, hint)?
        };
        // A `required` name no `properties` entry declares is still required: the instance must
        // carry that key. Consuming `required` only as a per-property flag dropped such a name,
        // so the generated type accepted and could emit an object without it (#140). It becomes
        // a required field typed by what the object says of an undeclared key: the
        // `additionalProperties` schema when there is one, and nothing at all otherwise.
        // `patternProperties` cannot be matched against the name at generation time, so its
        // value type would be a guess. `additionalProperties: false` is read the way the rest of
        // lowering reads it — it closes the object to the fields the generated type declares, as
        // `deny_unknown_fields` — and this field is one of them; reading it strictly instead
        // (every undeclared key forbidden, so the object is uninhabited) would reject the common
        // `allOf: [{$ref: Base}, {additionalProperties: false, required: [id]}]`, which that same
        // reading generates when `Base` declares `id`.
        for name in undeclared_required(schema) {
            let ty = match (&additional, schema.additional_properties.as_deref()) {
                (AdditionalProps::Typed(ty), Some(SchemaOr::Schema(_))) => {
                    // The map value dropped its `Box` because the map already provides the
                    // indirection a cycle-closing reference needs. A plain field has none, so it
                    // is boxed again exactly when the value closes a cycle: its target is still
                    // being lowered, which is what makes the `ensure_*` paths box it.
                    let mut ty = **ty;
                    ty.boxed = self.is_in_progress_root(ty.id);
                    ty
                }
                _ => self.insert_type(
                    &format!("{hint}{name}"),
                    TypeKind::Any,
                    Docs::default(),
                    None,
                ),
            };
            fields.push(Field {
                name: PropertyName { wire: name },
                ty,
                required: true,
                deprecated: false,
                read_only: false,
                write_only: false,
                default: None,
                xml: XmlField::default(),
                undeclared: true,
            });
        }
        Some((fields, additional))
    }

    /// Lower a property's OpenAPI `xml` hints into the field's [`XmlField`].
    ///
    /// `xml.name` and `xml.attribute` are represented (applied as a serde rename at emit time).
    /// The hints that change the XML wire without a faithful quick-xml mapping are recorded here
    /// and dispositioned in [`gate_xml_field_renames`], once it is known whether the owning type is
    /// ever serialized as XML at all. A `$ref` property carries no inline `xml` object here.
    fn field_xml(&mut self, child: &SchemaOr) -> XmlField {
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

    /// Merge an `allOf` composition (plus the enclosing schema's own sibling
    /// `properties`/`required`/`additionalProperties`) into a single typed [`TypeKind`].
    ///
    /// Members are gathered in a deterministic order — every `allOf` entry in source order, then the
    /// enclosing schema's own object siblings — flattening `$ref` members by *copying* their fields
    /// (the referenced component still exists as its own named type) and recursing into nested
    /// `allOf`. The gathered members are then combined:
    ///
    /// * **all object members** → one flattened [`Struct`]: the union of properties in first-seen
    ///   order, recursive typed intersections for properties declared by several members (an empty
    ///   one types the field uninhabited unless some member requires it, which is `E013` — the rule
    ///   `intersect_structs` applies), the union of `required`, and a conservatively intersected
    ///   `additionalProperties` policy;
    /// * **all scalar members** → their typed intersection, including numeric narrowing, enum
    ///   narrowing, arrays/objects/unions, and exact nullability; an empty intersection → `E013`;
    /// * an **object/scalar mix** → `E013`.
    ///
    /// Every path inserts its result type as the *final* graph insert (all member/property/component
    /// types insert first), so an `allOf` used as a component body still satisfies the
    /// [`Self::ensure_component`] last-insert invariant.
    fn lower_all_of(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let mut contributions = Vec::new();
        self.gather_all_of(schema, hint, &mut contributions)?;
        self.combine_all_of(schema, hint, &contributions)
    }

    /// Lower a schema carrying `allOf` and `oneOf`/`anyOf` together (issue #419). Both apply to
    /// every instance, so the schema is their conjunction: the union (the schema without its
    /// `allOf`, so the schema's own keywords refine its branches as they do with no `allOf` beside
    /// it) met with the `allOf` members branch by branch. A branch a member excludes drops out,
    /// and a union left with no branch is `E013`. Dispatching to the `allOf` arm alone dropped the
    /// union and its discriminator with no diagnostic.
    ///
    /// The schema's own keywords are the union's siblings only. Folded into the composition as
    /// well, as [`Self::gather_all_of`] folds them beside a bare `allOf`, untyped ones would make
    /// it an object and drop every branch of another category, and every `null` branch.
    ///
    /// Each member of untyped object or array applicators alone (one
    /// [`implied_applicator_category`] answers for) refines the branches of its own category and
    /// leaves the rest, as the same keywords do as siblings of a `$ref` to a union
    /// ([`Self::refine_union_target`]). The other members are combined as an `allOf` and met
    /// with the union through [`Self::intersect_types`], the meet that `$ref` arm applies to a typed
    /// sibling, under the nullability [`Self::combine_all_of`] gives their merge, as the
    /// `allOf`-member spelling ([`Self::lower_all_of_with_union_member`]) takes it. Members that
    /// constrain nothing (`true`, `{}`, an annotation) take part in neither: with none of either
    /// kind left the union alone is the schema's type, under the schema's own hint, as it is with
    /// no `allOf` at all.
    fn lower_all_of_beside_union(&mut self, schema: &Schema, hint: &str) -> Option<Ty> {
        let mut composition = schema.clone();
        composition.one_of.clear();
        composition.any_of.clear();
        composition.discriminator = None;
        // Everything `gather_all_of` and `combine_all_of` read beside `all_of`: the fold of the
        // schema's own object keywords and its `null`.
        composition.types = super::TypeSet::default();
        composition.properties.clear();
        composition.pattern_properties.clear();
        composition.additional_properties = None;
        composition.required.clear();
        let (scoped, combined): (Vec<SchemaOr>, Vec<SchemaOr>) =
            schema.all_of.iter().cloned().partition(|member| {
                matches!(member, SchemaOr::Schema(member) if implied_applicator_category(member).is_some())
            });
        composition.all_of = combined;
        let mut union = schema.clone();
        union.all_of.clear();

        let composition_hint = format!("{hint}Composition");
        let mut contributions = Vec::new();
        self.gather_all_of(&composition, &composition_hint, &mut contributions)?;
        if contributions.is_empty() && scoped.is_empty() {
            return self.lower_union(&union, hint);
        }
        let composed = if contributions.is_empty() {
            None
        } else {
            // Under the nullability `combine_all_of` gives it, as the `allOf`-member spelling
            // takes it: an untyped object member admits `null` and decides nothing.
            Some(self.combine_all_of(&composition, &composition_hint, &contributions)?)
        };
        let refiners = self.lower_all_of_refiners(&scoped, hint)?;
        self.meet_union_with_all_of(
            schema,
            hint,
            composed,
            refiners,
            &union,
            &format!("{hint}Union"),
            MetUnion::BesideAllOf,
        )
    }

    /// Lower an `allOf` exactly one of whose members is an inline `oneOf`/`anyOf` (#463), on a
    /// schema with no union of its own. The member applies to every instance as the others do, so
    /// the schema is the union met branch by branch with the rest of the composition, as
    /// `{$ref: A, oneOf: […]}` meets its union with `A` and as the same union written beside the
    /// `allOf` is met ([`Self::lower_all_of_beside_union`]): a branch the composition excludes
    /// drops out, a union left with no branch is `E013`, and branches the meet leaves sharing one
    /// generated type collapse with `W001` ([`Self::collapse_met_union`]). A member whose union
    /// lowers to a single type (one real branch beside a `null` one) is met as that type. Read
    /// as an ordinary scalar member, the union made every object composition an object/scalar mix.
    ///
    /// The member's own keywords stay its union's siblings. The other members, and the
    /// schema's own keywords, are gathered as [`Self::gather_all_of`] gathers them, under the hints
    /// it gives them; members of untyped object or array applicators alone refine the union's
    /// branches of their own category, as beside the `allOf`. Where no member is an object and
    /// none refines one, the union is combined as the scalar member it always was.
    fn lower_all_of_with_union_member(
        &mut self,
        schema: &Schema,
        hint: &str,
        union_index: usize,
    ) -> Option<Ty> {
        let SchemaOr::Schema(union) = &schema.all_of[union_index] else {
            return self.lower_all_of(schema, hint);
        };
        let union_hint = format!("{hint}Member{union_index}");
        let mut scoped = Vec::new();
        let mut contributions = Vec::new();
        // Where the union's contribution goes when it is combined as a scalar member below.
        let mut union_slot = 0;
        for (index, member) in schema.all_of.iter().enumerate() {
            if index == union_index {
                union_slot = contributions.len();
                continue;
            }
            if matches!(member, SchemaOr::Schema(member) if implied_applicator_category(member).is_some())
            {
                scoped.push(member.clone());
                continue;
            }
            self.gather_member(member, &format!("{hint}Member{index}"), &mut contributions)?;
        }
        let mut composition = schema.clone();
        composition.all_of.clear();
        self.gather_all_of(&composition, hint, &mut contributions)?;
        // With no object member, the union meets the other members as the scalar it is, in its
        // place among them, exactly as an `allOf` of scalars always met one: the scalar meet
        // already intersects a union branch by branch, and it keeps the narrowing `open_narrowing`
        // gives each member where it is written.
        let has_object = contributions
            .iter()
            .any(|contribution| matches!(contribution, Contribution::Object { .. }));
        if !has_object && scoped.is_empty() {
            let ty = self.lower_schema(union, &union_hint)?;
            contributions.insert(union_slot, Contribution::Scalar(ty));
            return self.combine_all_of(schema, hint, &contributions);
        }
        let composed = if contributions.is_empty() {
            None
        } else {
            Some(self.combine_all_of(schema, &format!("{hint}Composition"), &contributions)?)
        };
        let refiners = self.lower_all_of_refiners(&scoped, hint)?;
        let mut ty = self.meet_union_with_all_of(
            schema,
            hint,
            composed,
            refiners,
            union,
            &union_hint,
            MetUnion::AllOfMember,
        )?;
        // As the other `allOf` arms apply it after their merge.
        ty = self.with_all_of_nullability(schema, ty);
        Some(ty)
    }

    /// Lower the `allOf` members of untyped object or array applicators alone, which refine a
    /// union's branches of their own category ([`Self::lower_all_of_beside_union`]).
    fn lower_all_of_refiners<'s>(
        &mut self,
        scoped: &'s [SchemaOr],
        hint: &str,
    ) -> Option<Vec<(&'s Schema, Refiner)>> {
        let mut refiners = Vec::new();
        for (index, member) in scoped.iter().enumerate() {
            let SchemaOr::Schema(member) = member else {
                continue;
            };
            let scoped =
                self.lower_scoped_refiners(member, true, None, &format!("{hint}Member{index}"))?;
            refiners.push((member.as_ref(), Refiner::Scoped(scoped)));
        }
        Some(refiners)
    }

    /// Lower `{$ref: T, <keywords>, oneOf|anyOf: […]}`, a `$ref` whose siblings are a union and
    /// shape-bearing `keywords` beside it ([`split_union_sibling`]), as the two `allOf` spellings of
    /// the same conjunction are lowered (#538): `keywords` meet the target, the union is met with
    /// that composition, and what the meet leaves sharing one generated type collapses with `W001`
    /// ([`Self::meet_union_with_all_of`]). Untyped object or array `keywords` refine the union's
    /// branches of their own category after the collapse, as such an `allOf` member does.
    ///
    /// Lowered as one schema, the sibling met `keywords` with each branch before the target, and
    /// the union hoisted every branch's `null` to itself, so the collapse could no longer count
    /// how many branches accept `null`: a `oneOf` that `null` matches twice kept it.
    fn meet_ref_union_sibling(
        &mut self,
        schema: &Schema,
        hint: &str,
        referenced: Ty,
        keywords: &Schema,
        union: &Schema,
    ) -> Option<Ty> {
        let keywords_hint = format!("{hint}Constraint");
        let (composed, refiners) = if implied_applicator_category(keywords).is_some() {
            let scoped = self.lower_scoped_refiners(keywords, true, None, &keywords_hint)?;
            (referenced, vec![(keywords, Refiner::Scoped(scoped))])
        } else {
            let keywords = self.lower_schema(keywords, &keywords_hint)?;
            let Ok(composed) =
                self.intersect_types(referenced, keywords, &format!("{hint}ReferenceComposition"))
            else {
                return self.reject_ref_sibling_intersection(schema);
            };
            (composed, Vec::new())
        };
        self.meet_union_with_all_of(
            schema,
            hint,
            Some(composed),
            refiners,
            union,
            &format!("{hint}Union"),
            MetUnion::RefSibling,
        )
    }

    /// Lower `union`, with its own merge held back, meet it with an `allOf`'s `composed` members,
    /// collapse what that meet leaves sharing one generated type, and then meet the result with
    /// each of its `refiners`; the result is inserted as `schema`'s type under `hint`. The meet
    /// shared by [`Self::lower_all_of_beside_union`], [`Self::lower_all_of_with_union_member`] and
    /// [`Self::meet_ref_union_sibling`], whose `composed` is the `$ref` target met with the
    /// keywords beside the union.
    #[allow(clippy::too_many_arguments)]
    fn meet_union_with_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        composed: Option<Ty>,
        refiners: Vec<(&Schema, Refiner)>,
        union: &Schema,
        union_hint: &str,
        spelling: MetUnion,
    ) -> Option<Ty> {
        // How the refiner diagnostics name the union and the keywords that refine it.
        let (union_is, beside, refining) = match spelling {
            MetUnion::AllOfMember => (
                "the union member of this `allOf`",
                "the union member",
                "this `allOf` member's",
            ),
            MetUnion::BesideAllOf => (
                "the union beside this `allOf`",
                "the union beside the `allOf`",
                "this `allOf` member's",
            ),
            MetUnion::RefSibling => (
                "the union beside this `$ref`",
                "the union beside the `$ref`",
                "this `$ref`'s sibling",
            ),
        };
        let meet_hint = format!("{hint}{}", spelling.meet_suffix());
        let enclosing_unmerged = self.unmerged_union.replace(union.provenance.clone());
        let lowered = self.lower_schema(union, union_hint);
        self.unmerged_union = enclosing_unmerged;
        let lowered = lowered?;
        let mark = self.graph_mark();
        let mut meet = lowered;
        if let Some(composed) = composed {
            let Ok(met) = self.intersect_types(composed, meet, &meet_hint) else {
                return self.reject_all_of_union_meet(schema, spelling);
            };
            meet = met;
        }
        // Collapsed before the refiners, which meet each branch apart and so would give branches
        // the composition left as one type distinct definitions of the same shape: the refiners
        // constrain every branch of their category alike, so refining the collapsed type admits
        // the same values.
        let (collapsed, untyped_check) =
            self.collapse_met_union(schema, meet, !union.one_of.is_empty(), &meet_hint, spelling);
        meet = collapsed;
        for (index, (member, refiner)) in refiners.into_iter().enumerate() {
            let mut reach = ScopeReach::default();
            let met = self.meet_scoped_refiner(
                meet,
                refiner,
                &format!("{hint}Refined{index}"),
                &mut reach,
            );
            if met.is_err() && reach.uncategorised {
                let message = format!(
                    "a branch of {union_is} states no JSON category, and {refining} untyped \
                     keywords are both object keywords and array keywords with no `type` to \
                     choose between them, so no single Rust type represents what they constrain \
                     of it"
                );
                return self.reject_unscoped_union_sibling(member, &message);
            }
            for keywords in unreached_halves(refiner, &reach) {
                let message = format!(
                    "{refining} untyped {keywords} constrain only the instances of \
                     their own category, and no branch of {beside} has that category, so they \
                     apply to no value the union accepts"
                );
                self.warn_unreached_union_sibling(member, message);
            }
            let Ok(met) = met else {
                return self.reject_all_of_union_meet(schema, spelling);
            };
            meet = met;
        }
        // After the refiners, which give an untyped branch of their category a type (#535).
        if untyped_check {
            self.warn_untyped_met_variants(schema, meet, spelling);
        }
        let kind = self.graph.get(meet.id)?.kind.clone();
        self.discard_meet_intermediates(mark, &kind);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        ty.nullable = meet.nullable;
        ty.boxed = meet.boxed;
        Some(ty)
    }

    /// Combine the gathered members of an `allOf` into its type; see [`Self::lower_all_of`].
    fn combine_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        contributions: &[Contribution],
    ) -> Option<Ty> {
        let has_object = contributions
            .iter()
            .any(|c| matches!(c, Contribution::Object { .. }));
        let scalars: Vec<Ty> = contributions
            .iter()
            .filter_map(|c| match c {
                Contribution::Scalar(ty) => Some(*ty),
                Contribution::Object { .. } => None,
            })
            .collect();

        // Object-vs-scalar mix has no single representable type.
        if has_object && !scalars.is_empty() {
            return self.reject_all_of_object_scalar_mix(schema);
        }

        // All-scalar allOf: recursively intersect compatible members (for example integer with
        // number, an enum with its underlying scalar, or arrays whose item constraints narrow).
        if !has_object {
            let Some(mut intersection) = scalars.first().copied() else {
                // Only no-constraint members (`true`/`{}`) remained: a faithful open object.
                let ty = self.insert_schema_type(
                    schema,
                    hint,
                    TypeKind::Struct(Struct {
                        fields: Vec::new(),
                        additional: AdditionalProps::Allow,
                    }),
                );
                return Some(self.with_all_of_nullability(schema, ty));
            };
            let mark = self.graph_mark();
            for (index, member) in scalars.iter().copied().enumerate().skip(1) {
                let Ok(merged) = self.intersect_types(
                    intersection,
                    member,
                    &format!("{hint}Intersection{index}"),
                ) else {
                    return self.reject_all_of_scalars(schema);
                };
                intersection = merged;
            }
            // Re-emit the intersection as the final graph insert so the invariant holds even when
            // the allOf is a component body (the per-member scalar inserts above are left dead —
            // `#[allow(dead_code)]` on the models module — rather than threading a reserved id).
            // The meets' own inserts are discarded unless the re-emitted kind reaches them.
            let kind = self
                .graph
                .get(intersection.id)
                .map(|def| def.kind.clone())?;
            self.discard_meet_intermediates(mark, &kind);
            let mut ty = self.insert_schema_type(schema, hint, kind);
            ty.nullable = intersection.nullable;
            return Some(self.with_all_of_nullability(schema, ty));
        }

        // All object members: flatten into one struct. Property union preserves first-seen order.
        let mut fields: IndexMap<String, Field> = IndexMap::new();
        let mut required: Vec<String> = Vec::new();
        let mut additional = AdditionalProps::Allow;
        // Repeated properties whose types have no common value, in first-seen order.
        let mut uninhabited: IndexSet<String> = IndexSet::new();
        // Every member is lowered already, so what the merge inserts from here on is its meets'.
        let mark = self.graph_mark();
        for contribution in contributions {
            let Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: member_required,
                ..
            } = contribution
            else {
                continue;
            };
            for name in member_required {
                if !required.contains(name) {
                    required.push(name.clone());
                }
            }
            match self.merge_additional(
                &additional,
                member_additional,
                &format!("{hint}Additional"),
            ) {
                Some(merged) => additional = merged,
                None => {
                    // `merge_additional` can only decline by failing to intersect the two value
                    // types, and that has two causes the author has to tell apart. A genuine
                    // conflict is one sentence; a value schema that is a `$ref` back to the type
                    // being lowered is another, and calling it "conflicting" sends the reader
                    // looking for a disagreement that is not in the document — nothing conflicts,
                    // the target's body simply has not been computed yet.
                    let unlowered = [&additional, member_additional].into_iter().any(|policy| {
                        matches!(policy, AdditionalProps::Typed(ty) if self.is_reservation(ty.id))
                    });
                    if unlowered {
                        return self.reject_all_of_cycle(
                            schema.provenance.clone(),
                            "an `allOf` member's `additionalProperties` value schema is a `$ref` \
                             that closes a reference cycle back to the schema being lowered, whose \
                             body is not yet known, so the merged overflow map has no computable \
                             value type",
                        );
                    }
                    return self.reject_all_of_additional(schema);
                }
            }
            for field in member_fields {
                match fields.get_mut(&field.name.wire) {
                    Some(existing) => {
                        // A field one side carries only because it requires the name is not a
                        // declaration of the property, so it does not intersect with one: the
                        // declaring member supplies the type and the metadata, and the requirement
                        // survives (see `take_declaration`).
                        if take_declaration(existing, field) {
                            continue;
                        }
                        // Either member's `default` is a default of the merged field, whichever
                        // member came first (see `merge_field_default`).
                        merge_field_default(
                            &mut existing.default,
                            field.default.as_ref(),
                            &field.name.wire,
                            self.diags,
                        );
                        // A repeated property is an intersection, not an equality assertion: retain
                        // the narrower compatible type.
                        let field_hint = format!("{hint}{}Intersection", field.name.wire);
                        let intersection = self.intersect_types(existing.ty, field.ty, &field_hint);
                        existing.required = existing.required || field.required;
                        existing.ty = match intersection {
                            Ok(ty) => ty,
                            // A reservation's body is not known yet, so the failure here says
                            // nothing about whether the property's types meet; typing the field
                            // uninhabited would be a guess. Refuse it, naming the cycle rather
                            // than a conflict nobody wrote.
                            Err(_)
                                if self.is_reservation(existing.ty.id)
                                    || self.is_reservation(field.ty.id) =>
                            {
                                let message = format!(
                                    "property `{}` repeated across `allOf` members is typed by a \
                                     `$ref` that closes a reference cycle back to the schema \
                                     being lowered, so its intersection cannot be computed",
                                    field.name.wire
                                );
                                return self
                                    .reject_all_of_cycle(schema.provenance.clone(), &message);
                            }
                            // The same rule `intersect_structs` applies to a `$ref` and its
                            // siblings, so the four equivalent spellings of one conjunction agree:
                            // the types cannot meet, but that empties the object only if some
                            // instance must carry the property. Whether one must is not known
                            // until every member's `required` has been read — a later member may
                            // require it without declaring it — so the field takes an uninhabited
                            // type now and the requirement is settled after the loop. A member's
                            // applied `default` is left for `retype_field_defaults`, which finds
                            // it no value of the uninhabited type and reports it (`W005`) where it
                            // was written, documenting it as not applied (#453).
                            Err(NoMeet::Empty) => {
                                uninhabited.insert(field.name.wire.clone());
                                self.insert_type(
                                    &field_hint,
                                    TypeKind::Never,
                                    Docs::default(),
                                    None,
                                )
                            }
                            // Only an empty meet is uninhabited: these two types share values, and
                            // an uninhabited field would refuse every object carrying one.
                            Err(NoMeet::Unrepresentable) => {
                                let message = format!(
                                    "property `{}` repeated across `allOf` members has types that \
                                     share values no single Rust type represents",
                                    field.name.wire
                                );
                                return self.reject_unrepresentable_meet(schema, &message);
                            }
                        };
                    }
                    None => {
                        fields.insert(field.name.wire.clone(), field.clone());
                    }
                }
            }
        }

        // A field no member declares is an undeclared key of every member, so each member's
        // `additionalProperties` value schema constrains it, not only the requiring member's own:
        // `allOf: [{$ref: Labels}, {required: [a]}]` with string-valued `Labels` makes `a` a
        // string, not an unconstrained value. The requiring member already applied its own.
        for contribution in contributions {
            let Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                ..
            } = contribution
            else {
                continue;
            };
            for field in fields.values_mut() {
                if !field.undeclared
                    || member_fields
                        .iter()
                        .any(|member| member.name.wire == field.name.wire)
                {
                    continue;
                }
                let field_hint = format!("{hint}{}Intersection", field.name.wire);
                match self.narrow_undeclared(field.ty, member_additional, &field_hint) {
                    Ok(ty) => field.ty = ty,
                    Err(_)
                        if self.is_reservation(field.ty.id)
                            || matches!(member_additional, AdditionalProps::Typed(value) if self.is_reservation(value.id)) =>
                    {
                        let message = format!(
                            "required property `{}`, which no `allOf` member declares, is typed by \
                             an `additionalProperties` value schema that is a `$ref` closing a \
                             reference cycle back to the schema being lowered, so its \
                             intersection cannot be computed",
                            field.name.wire
                        );
                        return self.reject_all_of_cycle(schema.provenance.clone(), &message);
                    }
                    // The field is required, so a value no type admits empties the composition.
                    Err(NoMeet::Empty) => {
                        return self.reject_all_of_undeclared_required(schema, &field.name.wire);
                    }
                    Err(NoMeet::Unrepresentable) => {
                        let message = format!(
                            "required property `{}`, which no `allOf` member declares, is typed by \
                             `additionalProperties` value schemas that share values no single Rust \
                             type represents",
                            field.name.wire
                        );
                        return self.reject_unrepresentable_meet(schema, &message);
                    }
                }
            }
        }

        // An uninhabited property that any member requires obliges every instance to carry a value
        // no type admits: the composition is empty, and that is the document error.
        if let Some(name) = uninhabited.iter().find(|name| {
            required.contains(name) || fields.get(*name).is_some_and(|field| field.required)
        }) {
            return self.reject_all_of_required_property(schema, name);
        }

        // Apply the required union, then keep required fields consistent: a serde default only fires
        // for an absent optional field, so a field promoted to required by another member drops its
        // applied default (it stays documented in rustdoc).
        let mut fields: Vec<Field> = fields.into_values().collect();
        for field in &mut fields {
            if required.contains(&field.name.wire) {
                field.required = true;
            }
            if field.required {
                if let Some(default) = &mut field.default {
                    default.applied = None;
                }
            }
        }

        // A property repeated by three or more members is met pair by pair, and each meet replaces
        // the field's type, so the struct refers to the last meet and not to the ones before it.
        let kind = TypeKind::Struct(Struct { fields, additional });
        self.elide_meet_intermediates(mark, &kind);
        let mut ty = self.insert_schema_type(schema, hint, kind);
        // As the all-scalar branch takes its meet's nullability: `null` satisfies the merge when
        // it satisfies every member.
        ty.nullable = object_all_of_admits_null(contributions);
        Some(self.with_all_of_nullability(schema, ty))
    }

    /// Gather every member of `schema.all_of` (source order) plus the enclosing schema's own object
    /// siblings (last), pushing a [`Contribution`] per constraining member.
    fn gather_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        for (index, member) in schema.all_of.iter().enumerate() {
            self.gather_member(member, &format!("{hint}Member{index}"), out)?;
        }
        // The enclosing schema may carry its own object keywords beside `allOf`; fold them in last.
        if schema_is_object_like(schema) {
            let (member_fields, member_additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields: member_fields,
                additional: member_additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
            });
        }
        Some(())
    }

    fn gather_member(
        &mut self,
        member: &SchemaOr,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        let schema = match member {
            // A `true`/`{}` member imposes no constraint.
            SchemaOr::Bool(true) => return Some(()),
            SchemaOr::Bool(false) => {
                return self.reject_all_of_false_member(member_provenance(member));
            }
            SchemaOr::Schema(schema) => schema.as_ref(),
        };
        // An inline member, or a non-component target expanded in place, is read by its keywords
        // here rather than lowered through `lower_schema_inner`, which would report this.
        self.diagnose_standalone_discriminator(schema);

        if let Some(reference) = &schema.reference {
            self.gather_ref_target(schema, reference, hint, out)?;
            // `$ref` is an applicator, not a replacement for the member that holds it: the member
            // is the target AND its own shape-bearing siblings, so those are further conjuncts of
            // this same merge, gathered exactly as a separate member carrying them would be. Every
            // arm above used to return once the target was pushed, which silently deleted the
            // siblings' properties and `required`, and let a sibling contradicting its target
            // generate as the target alone. The gate is the one `lower_schema_inner` asks of a
            // `$ref`'s siblings, so the two positions agree on what counts as a shape.
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                return Some(());
            }
            return self.gather_member(
                &SchemaOr::Schema(Box::new(sibling)),
                &format!("{hint}Constraint"),
                out,
            );
        }

        if !schema.all_of.is_empty() && !schema_has_union(schema) {
            // Nested allOf: flatten its members (and its own siblings) into the same accumulator.
            // One with a union beside it is that composition met with the union, which only
            // lowering computes, so `gather_inline` lowers it as the scalar it then is.
            return self.gather_all_of(schema, hint, out);
        }

        self.gather_inline(schema, hint, out)
    }

    /// Push the contribution of an `allOf` member's `$ref` target — and only the target: the
    /// member's own siblings are [`Self::gather_member`]'s to gather, after this returns.
    fn gather_ref_target(
        &mut self,
        schema: &Schema,
        reference: &str,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            // A `$ref` to a component still being lowered is a direct recursive allOf member
            // whose fields are not yet known — irreconcilable (distinct from a member with
            // recursive *fields*, which lowers fine).
            if self.in_progress.contains_key(name) {
                return self.reject_all_of_cycle(
                    schema.provenance.clone(),
                    "an `allOf` member is a direct recursive `$ref` to the component being \
                     lowered",
                );
            }
            let ty = self.ensure_component(name, Some(reference), &schema.provenance)?;
            // The pre-check above sees root components only. A name the root does not declare
            // is a *sub-file* component, and it reaches its own reservation through
            // `ensure_resolved`, so a direct recursive member there arrives here as a back-edge
            // rather than being caught above; `push_ref_member` refuses to read it.
            return self.push_ref_member(
                ty,
                &schema.provenance,
                "an `allOf` member is a direct recursive `$ref` to the component being lowered",
                out,
            );
        }
        // A remote `$ref` member goes through the cycle-safe remote path, exactly like a
        // component member: a member still being lowered is a direct recursive ref whose fields
        // are not yet known (irreconcilable), otherwise its shared type contributes its fields.
        if is_remote_ref(reference) {
            if self.remote_in_progress.contains_key(reference) {
                return self.reject_all_of_cycle(
                    schema.provenance.clone(),
                    "an `allOf` member is a direct recursive remote `$ref` to the schema being \
                     lowered",
                );
            }
            let ty = self.ensure_remote(reference)?;
            return self.push_ref_member(
                ty,
                &schema.provenance,
                "an `allOf` member is a direct recursive remote `$ref` to the schema being \
                 lowered",
                out,
            );
        }
        // Non-component refs resolve (or error) exactly as `lower_schema` does; the target is then
        // gathered as an inline member would be (see `gather_resolved_target`).
        let resolved = self
            .resolver
            .resolve(reference, &schema.provenance, self.diags)
            .ok()?;
        let target = resolved.schema.into_owned();
        // This arm inlines rather than referencing a shared type, so there is no `Ty` to test —
        // test the target instead. Without this, a member that is the very schema being lowered
        // descends into its own body again and stops only at `MAX_SCHEMA_DEPTH`, reporting a
        // chain length for what is a cycle of length one. The component and remote arms above
        // refuse to read an in-progress member; this one now does too.
        if self.resolved_target_in_progress(&target.provenance) {
            return self.reject_all_of_cycle(
                schema.provenance.clone(),
                "an `allOf` member is a direct recursive `$ref` to the schema being lowered",
            );
        }
        // Expand the target once per resolved `file#pointer` and replay its contribution at
        // every later use: see `resolved_contributions`. The in-progress test above runs
        // first on every use, so a replay never stands in for a refusal. Only the target is
        // memoised: the member's siblings belong to this use, and `gather_member` adds them.
        let Some(key) = resolved_identity(&target.provenance) else {
            // No span, so no identity to key on — expand un-memoised, as `ensure_resolved`
            // lowers un-deduplicated in the same case. The depth cap still bounds it.
            return self.gather_resolved_target(target, hint, out);
        };
        if let Some(recorded) = self.resolved_contributions.get(&key) {
            out.extend(recorded.iter().cloned());
            return Some(());
        }
        // The target's expansion follows its own `$ref` and `allOf` members, and nothing on that
        // path reserves a type a re-entry could be boxed against, so a target this expansion is
        // already inside is a loop: reject it rather than recurse to the depth cap. A loop made
        // only of bare aliases is the alias cycle `ensure_resolved` reports; one that passes
        // through a body is a member recursive through its own composition, as the root document
        // reports it.
        if let Some(start) = self
            .resolved_member_stack
            .iter()
            .position(|(open, _)| *open == key)
        {
            if self.resolved_member_stack[start..]
                .iter()
                .all(|&(_, alias)| alias)
            {
                // E004 case: cycle
                Diagnostic::error(Code::UnresolvedRef, schema.provenance.clone())
                    .message(format!(
                        "schema reference `{reference}` forms an alias cycle"
                    ))
                    .remedy(
                        "give one component in the cycle a schema body, or break the cycle at one \
                         of its references",
                    )
                    .emit(self.diags);
                return None;
            }
            return self.reject_all_of_cycle(
                schema.provenance.clone(),
                "an `allOf` member is a recursive `$ref` that reaches itself through its target's \
                 own `$ref` and `allOf` members",
            );
        }
        // Name what the body lowers to for the schema it came from, not for whichever use
        // reached it first — once one expansion serves every use, a per-use hint would make
        // the generated names depend on lowering order. The `Member` suffix keeps it off the
        // hint `ensure_resolved` gives the same target when it is also a direct `$ref`: that
        // lowers a second copy of the body, and two copies on one hint would leave the bare
        // name (`Basemeta`, or a scalar target's own `Code`) to whichever lowering ran first.
        let hint = format!("{}Member", resolved_hint(&target.provenance, hint));
        let mut contributed = Vec::new();
        self.resolved_member_stack
            .push((key.clone(), target.reference.is_some()));
        let expanded = self.gather_resolved_target(target, &hint, &mut contributed);
        self.resolved_member_stack.pop();
        expanded?;
        self.resolved_contributions.insert(key, contributed.clone());
        out.extend(contributed);
        Some(())
    }

    /// Expand a bundle-`$ref` `allOf` member's resolved target as [`Self::gather_member`] expands
    /// any member: a target that is itself a `$ref` chains to *its* target (and gathers its own
    /// siblings), one that is an `allOf` flattens its members, and only a plain body is read for
    /// object or scalar keywords. Reading every target as a plain body took an `allOf` or alias
    /// target, which carries neither kind of keyword, for a pure annotation, and silently dropped
    /// everything it constrains (issue #306).
    ///
    /// This recursion does not pass through [`Self::lower_schema`], so it counts against
    /// [`Self::depth`] itself: a long acyclic chain of such targets rejects with `E014` rather than
    /// exhausting the stack. Loops are the caller's to refuse, before this is entered.
    fn gather_resolved_target(
        &mut self,
        target: Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // The target's contribution is memoised and replayed at every later use of it, so what it
        // lowers must not depend on the position that first reached it: it is a `$ref` target,
        // and `open_narrowing` is out of effect there. The merge of its fields with the enclosing
        // members' still happens at the use site.
        self.closed_narrowing(|ctx| ctx.gather_resolved_target_closed(target, hint, out))
    }

    /// [`Self::gather_resolved_target`]'s body, run with `open_narrowing` out of effect.
    fn gather_resolved_target_closed(
        &mut self,
        target: Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        if self.depth >= MAX_SCHEMA_DEPTH {
            return self.reject_too_deep(&target.provenance);
        }
        self.depth += 1;
        let result = self.gather_member(&SchemaOr::Schema(Box::new(target)), hint, out);
        self.depth -= 1;
        result
    }

    /// Turn a resolved `$ref` member's already-lowered type into a contribution: an object component
    /// contributes a *copy* of its fields/`additionalProperties`; any other lowered kind is a
    /// scalar member.
    ///
    /// A member whose body is still being lowered is refused here, with `recursive` as the
    /// message, rather than by each caller: a reservation's kind says nothing about the schema's
    /// shape, and reading it as "not a struct" is exactly how a recursive member once became a
    /// silent scalar. A caller cannot forget the guard because it no longer holds it.
    fn push_ref_member(
        &mut self,
        ty: Ty,
        provenance: &Provenance,
        recursive: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // Every id `is_in_progress_root` accepts is still a `Reserved` placeholder — each
        // in-progress map is entered with a fresh `reserve` and left before its `fill` — so this
        // arm is the whole guard the callers used to hold.
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Reserved) => {
                return self.reject_all_of_cycle(provenance.clone(), recursive)
            }
            Some(TypeKind::Struct(structure)) => {
                let fields = structure.fields.clone();
                let required = fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.name.wire.clone())
                    .collect();
                let additional = structure.additional.clone();
                // A copied field keeps its `undeclared` mark, so one the component carries only
                // for its own `required` still gives way to a later member's declaration.
                out.push(Contribution::Object {
                    fields,
                    additional,
                    required,
                    nullable: Some(ty.nullable),
                });
            }
            _ => out.push(Contribution::Scalar(ty)),
        }
        Some(())
    }

    fn gather_inline(
        &mut self,
        schema: &Schema,
        hint: &str,
        out: &mut Vec<Contribution>,
    ) -> Option<()> {
        // A member carrying its own `oneOf`/`anyOf` is that union, its object keywords refining the
        // branches as `lower_union` refines them; read as an object by its keywords, the union was
        // dropped with no diagnostic (issue #419).
        if schema_is_object_like(schema) && !schema_has_union(schema) {
            let (fields, additional) = self.object_body(schema, hint)?;
            out.push(Contribution::Object {
                fields,
                additional,
                required: schema.required.clone(),
                nullable: stated_nullability(schema),
            });
        } else if schema_imposes_scalar(schema) {
            let ty = self.lower_schema(schema, hint)?;
            out.push(Contribution::Scalar(ty));
        }
        // Otherwise the member is a pure annotation (`{description: ...}`): no constraint.
        Some(())
    }

    /// Merge two `additionalProperties` policies for an `allOf` intersection. `Deny` dominates (a
    /// value must satisfy every member, so any member denying unknown keys forbids them outright);
    /// two typed value schemas must lower to the same type. Returns `None` when irreconcilable.
    fn merge_additional(
        &mut self,
        acc: &AdditionalProps,
        next: &AdditionalProps,
        hint: &str,
    ) -> Option<AdditionalProps> {
        Some(match (acc, next) {
            (AdditionalProps::Deny, _) | (_, AdditionalProps::Deny) => AdditionalProps::Deny,
            (AdditionalProps::Typed(x), AdditionalProps::Typed(y)) => {
                let intersection = self.intersect_types(**x, **y, hint).ok()?;
                AdditionalProps::Typed(Box::new(intersection))
            }
            (AdditionalProps::Typed(x), AdditionalProps::Allow)
            | (AdditionalProps::Allow, AdditionalProps::Typed(x)) => {
                AdditionalProps::Typed(x.clone())
            }
            (AdditionalProps::Allow, AdditionalProps::Allow) => AdditionalProps::Allow,
        })
    }

    /// The type of a [`Field::undeclared`] field `field` once it is also a key another object does
    /// not declare, whose overflow policy is `additional`: a typed value schema there constrains
    /// the key as well, and `true`, `false` and an absent one leave it as it was (`false` closes
    /// the object to the fields the merged type declares, this one included, as in
    /// [`Self::object_body`]).
    fn narrow_undeclared(
        &mut self,
        field: Ty,
        additional: &AdditionalProps,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let AdditionalProps::Typed(value) = additional else {
            return Ok(field);
        };
        // An unconstrained field simply takes the value type. A map value dropped its `Box`
        // because the map is the indirection a cycle-closing reference needs; a plain field has
        // none, so it is boxed again exactly when the value's target is still being lowered.
        if matches!(
            self.graph.get(field.id).map(|def| &def.kind),
            Some(TypeKind::Any)
        ) {
            let mut ty = **value;
            ty.boxed = self.is_in_progress_root(ty.id);
            return Ok(ty);
        }
        let mut ty = self.intersect_types(field, **value, hint)?;
        ty.boxed = field.boxed || self.is_in_progress_root(ty.id);
        Ok(ty)
    }

    /// Apply the enclosing `allOf` schema's own nullability (a `"null"` in its type array) to the
    /// merged type. Set after the final insert — a pure mutate that preserves the last-insert
    /// invariant.
    fn with_all_of_nullability(&self, schema: &Schema, mut ty: Ty) -> Ty {
        if schema.types.types.contains(&JsonType::Null) {
            ty.nullable = true;
        }
        ty
    }

    /// An `allOf` that mixes object and scalar members, which no single type can be.
    fn reject_all_of_object_scalar_mix<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: object-scalar-mix
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message("an `allOf` mixes object and scalar members, which cannot form one type")
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// An all-scalar `allOf` whose members have no common value or no single representable type.
    fn reject_all_of_scalars<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: scalar-members
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message("`allOf` scalar members have an empty or unrepresentable intersection")
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// `allOf` members whose `additionalProperties` value schemas have no common type.
    fn reject_all_of_additional<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: additional-values
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message("`allOf` members declare conflicting `additionalProperties`")
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// A property repeated across `allOf` members with types that cannot meet, which a member
    /// requires, so every instance must carry a value no type admits.
    fn reject_all_of_required_property<T>(&mut self, schema: &Schema, name: &str) -> Option<T> {
        // E013 case: required-property
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(format!(
                "property `{name}` appears in multiple `allOf` members with conflicting types, and \
                 a member requires it"
            ))
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// A required property no `allOf` member declares, whose members' `additionalProperties` value
    /// schemas share no value, so no instance can carry it.
    fn reject_all_of_undeclared_required<T>(&mut self, schema: &Schema, name: &str) -> Option<T> {
        // E013 case: required-property
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(format!(
                "property `{name}` is required but no `allOf` member declares it, and the members' \
                 `additionalProperties` value schemas it must satisfy share no value"
            ))
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// An `allOf` member that is the boolean schema `false`, which admits no value, so neither
    /// does the composition.
    fn reject_all_of_false_member<T>(&mut self, provenance: crate::diag::Provenance) -> Option<T> {
        // E013 case: false-member
        Diagnostic::error(Code::AllOfIrreconcilable, provenance)
            .message("an `allOf` member is `false`")
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// An `allOf` whose merge would have to read a `$ref` target still being lowered: a member that
    /// is a direct recursive reference, or a property or `additionalProperties` value two members
    /// both constrain that is typed by one. Its body is not known yet, so the composition can be
    /// computed neither against it nor by discarding it.
    fn reject_all_of_cycle<T>(
        &mut self,
        provenance: crate::diag::Provenance,
        message: &str,
    ) -> Option<T> {
        // E013 case: cycle
        Diagnostic::error(Code::AllOfIrreconcilable, provenance)
            .message(message.to_owned())
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// Two sides that share values no single Rust type represents ([`NoMeet::Unrepresentable`]),
    /// met where an empty meet would have been typed uninhabited or dropped: a union branch against
    /// the enclosing schema's siblings, or a property repeated across `allOf` members. Either
    /// stand-in would refuse the values the two sides share, so the composition is refused instead.
    fn reject_unrepresentable_meet<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        // E013 case: unrepresentable-meet
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(ALL_OF_REMEDY)
            .emit(self.diags);
        None
    }

    /// Report that a `$ref` target and its own sibling keywords have no single typed intersection.
    /// `$ref` is a 2020-12 applicator, so this is the same class of irreconcilable composition an
    /// `allOf` reports — `E013` covers both spellings — but the remedy names the construct the
    /// author actually wrote. Its callers report an empty intersection and an inhabited but
    /// unrepresentable one alike, for any of the reasons an `allOf` merge has, so the message
    /// distinguishes no further than that.
    fn reject_ref_sibling_intersection(&mut self, schema: &Schema) -> Option<Ty> {
        // E013 case: scalar-members, required-property, additional-values, object-scalar-mix, unrepresentable-meet
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(
                "the `$ref` target and this schema's own sibling keywords have an empty or \
                 unrepresentable intersection",
            )
            .remedy(REF_SIBLING_REMEDY)
            .emit(self.diags);
        None
    }

    /// Report that a schema's `allOf` composition and the `oneOf`/`anyOf` beside it, or among its
    /// members, have no single typed intersection (see [`Self::meet_union_with_all_of`]): no branch
    /// meets the composition, or one does in a way no single Rust type represents.
    fn reject_all_of_union_meet(&mut self, schema: &Schema, spelling: MetUnion) -> Option<Ty> {
        let (message, remedy) = match spelling {
            MetUnion::AllOfMember => (
                "this `allOf`'s `oneOf`/`anyOf` member and its other members all apply, and their \
                 intersection is empty or unrepresentable",
                ALL_OF_REMEDY,
            ),
            MetUnion::BesideAllOf => (
                "this schema's `allOf` and the `oneOf`/`anyOf` beside it both apply, and their \
                 intersection is empty or unrepresentable",
                ALL_OF_REMEDY,
            ),
            MetUnion::RefSibling => (
                "the `$ref` target, this schema's own sibling keywords and the `oneOf`/`anyOf` \
                 beside them all apply, and their intersection is empty or unrepresentable",
                REF_SIBLING_REMEDY,
            ),
        };
        // E013 case: scalar-members, required-property, additional-values, object-scalar-mix, unrepresentable-meet
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(message)
            .remedy(remedy)
            .emit(self.diags);
        None
    }

    /// Report that the category a `$ref`'s untyped sibling keywords establish (see
    /// [`implied_applicator_category`]) cannot be intersected with the target without either an
    /// empty result or a dropped target branch. `message` says which.
    fn reject_ref_sibling_category(&mut self, schema: &Schema, message: &str) -> Option<Ty> {
        // E013 case: inferred-category
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(REF_SIBLING_REMEDY)
            .emit(self.diags);
        None
    }

    /// Acknowledge that a union's untyped object or array sibling keywords (a [`Refiner::Scoped`]
    /// sibling, beside the union or beside a `$ref` to it) reach no branch of their category. In
    /// 2020-12 they are then vacuously satisfied by every value the union accepts, so the union
    /// generates as it is, and the keywords are reported rather than dropped in silence.
    fn warn_unreached_union_sibling(&mut self, schema: &Schema, message: String) {
        // W011 case: unreached-union-sibling
        Diagnostic::warning(Code::DeclarationHasNoEffect, schema.provenance.clone())
            .message(message)
            .emit(self.diags);
    }

    /// Report that a union's untyped object or array sibling keywords (a [`Refiner::Scoped`]
    /// sibling) settle no category for a branch that states none: they are both kinds, or a
    /// deleted multi-type array admits another category beside theirs.
    fn reject_unscoped_union_sibling<T>(&mut self, schema: &Schema, message: &str) -> Option<T> {
        // E013 case: inferred-category
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(UNION_SIBLING_REMEDY)
            .emit(self.diags);
        None
    }

    /// Report that a `$ref` carrying shape-bearing siblings — or a union member, when the union
    /// has siblings of its own — closes a reference cycle back to the schema enclosing it, so the
    /// siblings would have to be intersected with a target whose definition depends on the result.
    fn reject_ref_sibling_cycle(&mut self, schema: &Schema, message: &str) -> Option<Ty> {
        // E013 case: cycle
        Diagnostic::error(Code::AllOfIrreconcilable, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(REF_SIBLING_REMEDY)
            .emit(self.diags);
        None
    }

    /// Lower the overflow policy for an object that declares `patternProperties`. The generated
    /// struct captures every non-declared property into a single `#[serde(flatten)]` typed map, so
    /// every `patternProperties` value schema — together with a typed `additionalProperties` value,
    /// if any — must lower to the *same emitted Rust type*; otherwise a single map cannot type them.
    ///
    /// Homogeneity is decided by [`Self::same_map_value_type`], a bounded structural equivalence:
    /// same `TypeId` (a shared `$ref`, or the single-entry case) is homogeneous, and distinct inline
    /// leaf shapes (primitives, `Bytes`, `Any`, or arrays thereof) that emit the identical Rust type
    /// collapse to one map — so `{type:string}` under two patterns yields one `BTreeMap<String,
    /// String>`. Distinct inline composites (`Struct`/`Enum`/`Tuple`) stay heterogeneous and are
    /// rejected (`E005`), since two different object shapes cannot share one map value type. The
    /// first collected value type is used as the map's value type. Deterministic (graph lookups by
    /// `TypeId`, source-order collection) and bounded (recurses only through `Array` elements).
    fn lower_pattern_additional(&mut self, schema: &Schema, hint: &str) -> Option<AdditionalProps> {
        // `additionalProperties: false` denies unknown keys, but the flatten map must capture the
        // pattern-matched keys (which are themselves "unknown" to the named fields). Serde cannot do
        // both, so this combination has no faithful representation.
        if matches!(
            schema.additional_properties.as_deref(),
            Some(SchemaOr::Bool(false))
        ) {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties` combined with `additionalProperties: false` cannot be \
                     represented: a flatten map captures pattern values but cannot also deny other \
                     unknown keys",
                )
                .remedy(
                    "drop `additionalProperties: false`, or omit this API segment with \
                     spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        // Collect the value types in deterministic source order: patternProperties entries first
        // (IndexMap preserves source order), then a typed `additionalProperties` value if present.
        let mut value_types: Vec<Ty> = Vec::new();
        for (_pattern, child) in &schema.pattern_properties {
            let ty = self.lower_schema_or(child, &format!("{hint}Value"))?;
            self.warn_structural_default_or(child, "a `patternProperties` value");
            value_types.push(ty);
        }
        if let Some(additional) = schema.additional_properties.as_deref() {
            // `true`/absent leave unknown non-pattern keys unconstrained; the typed map still stands
            // in for the overflow. Only a schema value adds another type that must agree.
            if !matches!(additional, SchemaOr::Bool(_)) {
                let ty = self.lower_schema_or(additional, &format!("{hint}Additional"))?;
                self.warn_structural_default_or(additional, "an `additionalProperties` value");
                value_types.push(ty);
            }
        }

        let first = value_types[0];
        if value_types
            .iter()
            .any(|ty| !self.same_map_value_type(first, *ty))
        {
            Diagnostic::error(Code::PatternPropertiesRejected, schema.provenance.clone())
                .message(
                    "`patternProperties`/`additionalProperties` value schemas lower to different \
                     types; a single typed overflow map cannot represent them all",
                )
                .remedy(
                    "make every pattern/additional value the same type (e.g. a shared `$ref` or the \
                     same primitive), or omit this API segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }

        let mut ty = first;
        // A map value lives behind the map's own indirection; a cycle-closing ref needs no `Box`.
        ty.boxed = false;
        Some(AdditionalProps::Typed(Box::new(ty)))
    }

    /// Compute a typed intersection for two already-lowered schemas. Nullability is intersected
    /// independently from the non-null shape; an intersection containing only JSON `null` becomes
    /// [`TypeKind::Null`]. Derived arrays, objects, enums, and narrowed unions are inserted into the
    /// graph so codegen still sees an ordinary, fully typed IR node.
    ///
    /// No typed intersection is one of two answers, and [`NoMeet`] says which: an empty one may be
    /// typed uninhabited where an empty value remains (an array's items, a property no side
    /// requires) or collapse to `null` where both sides admit it, while an unrepresentable one is
    /// never narrowed that way — it reaches a caller that reports it.
    fn intersect_types(&mut self, a: Ty, b: Ty, hint: &str) -> Result<Ty, NoMeet> {
        let (Some(a_def), Some(b_def)) = (self.graph.get(a.id), self.graph.get(b.id)) else {
            return Err(NoMeet::Unrepresentable);
        };
        let a_kind = a_def.kind.clone();
        let b_kind = b_def.kind.clone();

        // Fail closed on a reservation, BEFORE nullability is consulted. A `TypeKind::Reserved`
        // operand is a placeholder whose body is still being lowered, so no true statement can be
        // made about the intersection — `is_in_progress_root`'s own documentation says the only
        // safe thing to do with one is refuse to read it. The callers above guard their own paths,
        // but a guard that asks about the *spelling* of a reference rather than its resolved
        // identity lets one through, and the rescue below then converted that unanswerable
        // intersection into a confident wrong answer: `intersect_non_null` found no meet for it
        // (it now refuses by a `Reserved` arm of its own), and the null rescue typed the
        // position as the exact JSON null type. The result was `pub type X = ();` — a client that
        // decodes only `null` for a schema that accepts objects — emitted with no diagnostic,
        // which is the standing invariant's fourth, silent behaviour.
        //
        // Refusing it as `NoMeet::Unrepresentable` hands the refusal to the caller, and that answer
        // is never typed uninhabited or collapsed to `null`: the `Never` fallbacks (an array's
        // items, a property no side requires) take only `NoMeet::Empty`, and so does the null
        // rescue below. So a reservation that slips past a caller-side guard is still rejected.
        //
        // A reservation intersected with ITSELF is exempt: `X ∩ X = X` needs no knowledge of the
        // body, and it is how every ordinary recursive schema composes when two `allOf` members
        // repeat one construct. Refusing it rejected those documents with a false "conflicting
        // types" message. `intersect_non_null` answers it by its identity short-circuit.
        if a.id != b.id
            && (matches!(a_kind, TypeKind::Reserved) || matches!(b_kind, TypeKind::Reserved))
        {
            return Err(NoMeet::Unrepresentable);
        }

        let accepts_null = type_accepts_null(a, &a_kind) && type_accepts_null(b, &b_kind);

        let non_null = if matches!(a_kind, TypeKind::Null) || matches!(b_kind, TypeKind::Null) {
            Err(NoMeet::Empty)
        } else {
            self.intersect_non_null(a, &a_kind, b, &b_kind, hint)
        };

        match non_null {
            Ok(mut ty) => {
                ty.nullable = accepts_null;
                Ok(ty)
            }
            // Only an EMPTY non-null meet leaves exactly `null`. An unrepresentable one still holds
            // the non-null values the two sides share, and `()` would refuse every one of them.
            Err(NoMeet::Empty) if accepts_null => {
                Ok(self.insert_type(hint, TypeKind::Null, Docs::default(), None))
            }
            Err(no_meet) => Err(no_meet),
        }
    }

    /// Whether `reference`, written at `at`, closes a reference cycle back through a schema whose
    /// lowering encloses `at`.
    ///
    /// This is a property of the DOCUMENT, not of the lowering: it asks whether the target reaches,
    /// through `$ref`s, a schema that contains `at` along the keywords lowering descends into. That
    /// is the same answer however any map is ordered and whichever end of a cycle lowering entered
    /// first. The predicate it replaced — membership of an in-progress map — was a property of
    /// *when* lowering happened, so mutual recursion rejected or generated according to which entry
    /// the document happened to declare, or which operation happened to reach it, first. Re-ordering
    /// a YAML map is a no-op in OpenAPI.
    ///
    /// Every schema is named by its resolved `(file, pointer)`, for every spelling alike, which is
    /// what makes the answer spelling-independent. An earlier form walked the root document's
    /// `components.schemas` by name, so it could not see a sub-file or remote target at all, and it
    /// matched a sub-file component's name against the root's map — a sub-file `Item` sharing its
    /// name with a root `Item` in a cycle was reported as closing that cycle.
    fn ref_closes_a_cycle(&self, reference: &str, at: &Provenance) -> bool {
        let site_file = at
            .span
            .map_or_else(|| self.resolver.root_id(), |span| span.file);
        let Some(start) = self.schema_ref_identity(reference, site_file) else {
            // Not a target this bundle knows; the lowering reports it in its own words.
            return false;
        };
        let mut seen: HashSet<(crate::diag::FileId, crate::diag::JsonPointer)> = HashSet::new();
        let mut stack = vec![start];
        while let Some((file, pointer)) = stack.pop() {
            if file == site_file && lowering_encloses(&pointer, &at.pointer) {
                return true;
            }
            if !seen.insert((file, pointer.clone())) {
                continue;
            }
            let Some(node) = self.resolver.node_at(file, &pointer) else {
                continue;
            };
            let mut references = Vec::new();
            collect_node_refs(node, &mut references);
            stack.extend(
                references
                    .into_iter()
                    .filter_map(|reference| self.schema_ref_identity(reference, file)),
            );
        }
        false
    }

    /// The `(file, pointer)` a schema `$ref` written in `from` lowers to, with the lowering's own
    /// precedence: `#/components/schemas/<name>` is the ROOT document's component whenever the root
    /// declares `name`, from whichever file it is written in (see [`Self::ensure_component`]), and
    /// every other reference resolves against the file it is written in.
    fn schema_ref_identity(
        &self,
        reference: &str,
        from: crate::diag::FileId,
    ) -> Option<(crate::diag::FileId, crate::diag::JsonPointer)> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            if self.document.components.schemas.contains_key(name) {
                return Some((
                    self.resolver.root_id(),
                    crate::diag::JsonPointer::from(format!("/components/schemas/{name}")),
                ));
            }
        }
        self.resolver.reference_identity_from(reference, from)
    }

    /// Whether a union member is a `$ref` that closes a reference cycle back through the component
    /// enclosing the union. Only a member that IS a reference counts: a member with recursive
    /// *fields* lowers fine, exactly as it does on the `allOf` path.
    ///
    /// Every spelling is asked. Asking only `#/components/schemas/…` left the explicit
    /// `./lib.yaml#/…` spelling to the reservation checks after lowering, so with siblings on one
    /// edge of a two-schema cycle it generated when lowering entered at one end and rejected when
    /// it entered at the other.
    fn member_closes_a_cycle(&self, member: &SchemaOr, at: &crate::diag::Provenance) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        member
            .reference
            .as_deref()
            .is_some_and(|reference| self.ref_closes_a_cycle(reference, at))
    }

    /// Whether a union member is a `$ref` to the very schema the union at `at` is. Such a union
    /// resolves to itself, which is `E007` rather than a sibling-intersection question.
    ///
    /// Answered by resolved identity, for every spelling, and also by the reservation the schema at
    /// `at` occupies, which is how the `#/components/schemas/…` spelling was answered before the
    /// document half of the union guard was asked of every spelling.
    fn member_is_this_union(&self, member: &SchemaOr, at: &crate::diag::Provenance) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        let Some(reference) = member.reference.as_deref() else {
            return false;
        };
        let site_file = at
            .span
            .map_or_else(|| self.resolver.root_id(), |span| span.file);
        if self
            .schema_ref_identity(reference, site_file)
            .is_some_and(|(file, pointer)| file == site_file && pointer == at.pointer)
        {
            return true;
        }
        let Some(own) = self.reservation_at(at) else {
            return false;
        };
        reference
            .strip_prefix("#/components/schemas/")
            .and_then(|name| self.in_progress.get(name))
            .is_some_and(|&(id, _)| id == own)
    }

    /// Whether an already-lowered type admits JSON `null`, resolving its kind out of the graph.
    /// [`type_accepts_null`] needs the kind beside the [`Ty`]; callers outside the intersection
    /// machinery hold only the [`Ty`].
    fn ty_accepts_null(&self, ty: Ty) -> bool {
        self.graph
            .get(ty.id)
            .is_some_and(|def| type_accepts_null(ty, &def.kind))
    }

    fn intersect_non_null(
        &mut self,
        a: Ty,
        a_kind: &TypeKind,
        b: Ty,
        b_kind: &TypeKind,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        if a.id == b.id {
            let mut ty = a;
            ty.nullable = false;
            ty.boxed = a.boxed || b.boxed;
            return Ok(ty);
        }

        match (a_kind, b_kind) {
            // Nothing true can be said about intersecting an unlowered body with anything else
            // (the identical reservation answered above by id). `intersect_types` refuses this
            // before calling here; stating it again means a new caller inherits the refusal rather
            // than reaching the `Any` arms below, which would answer with the placeholder itself.
            (TypeKind::Reserved, _) | (_, TypeKind::Reserved) => Err(NoMeet::Unrepresentable),
            (TypeKind::Any, _) => Ok(non_nullable(b)),
            (_, TypeKind::Any) => Ok(non_nullable(a)),
            (TypeKind::Primitive(left), TypeKind::Primitive(right)) => {
                let Some(primitive) = intersect_primitives(*left, *right) else {
                    return Err(no_meet(a_kind, b_kind));
                };
                if primitive == *left {
                    Ok(non_nullable(a))
                } else if primitive == *right {
                    Ok(non_nullable(b))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Primitive(primitive),
                        Docs::default(),
                        None,
                    ))
                }
            }
            // A set's variants are the values the description lists, open or not: `open` says only
            // that the lowering also holds an unlisted string, because a plain `string` was met
            // (`narrowed_string`). So two sets meet in the values both list, open when either is.
            // Both parts are order-independent (an intersection and a disjunction), so an `allOf`
            // lowers to the same set whichever order its members are written in; keeping the
            // closed side whole instead would admit values the open side's description forbids.
            // Where `open_narrowing` is out of effect (inside a union, which `intersect_union`
            // reaches with a set the response already opened) the meet is closed: two variants
            // that each held an unlisted string would both match it, and the trial union would
            // refuse every value. A locked set (one narrowed against a `uuid` or date string) locks
            // the meet, open side or not, which is again order-independent: the format's domain
            // holds no unlisted string for the open side to keep.
            (TypeKind::Enum(left), TypeKind::Enum(right)) if left.repr == right.repr => {
                let variants: Vec<ScalarValue> = left
                    .variants
                    .iter()
                    .filter(|value| right.variants.contains(value))
                    .cloned()
                    .collect();
                let openness =
                    if left.openness == Openness::Locked || right.openness == Openness::Locked {
                        Openness::Locked
                    } else if self.narrowing_opens && (left.is_open() || right.is_open()) {
                        Openness::Open
                    } else {
                        Openness::Closed
                    };
                if variants.is_empty() {
                    // Both value sets are finite and listed in full, so sharing no value is proof.
                    Err(NoMeet::Empty)
                } else if variants == left.variants && openness == left.openness {
                    Ok(non_nullable(a))
                } else if variants == right.variants && openness == right.openness {
                    Ok(non_nullable(b))
                } else if variants == left.variants {
                    Ok(self.reopened_set(a, left, openness, hint))
                } else if variants == right.variants {
                    Ok(self.reopened_set(b, right, openness, hint))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Enum(ScalarEnum {
                            repr: left.repr,
                            variants,
                            openness,
                        }),
                        Docs::default(),
                        None,
                    ))
                }
            }
            (TypeKind::Enum(enumeration), TypeKind::Primitive(primitive))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Ok(self.narrowed_string(a, enumeration, *primitive, hint))
            }
            (TypeKind::Primitive(primitive), TypeKind::Enum(enumeration))
                if enum_matches_primitive(enumeration.repr, *primitive) =>
            {
                Ok(self.narrowed_string(b, enumeration, *primitive, hint))
            }
            (TypeKind::Array(left), TypeKind::Array(right)) => {
                let item_hint = format!("{hint}Item");
                let item = match self.intersect_types(**left, **right, &item_hint) {
                    Ok(item) => item,
                    // No item satisfies both, so exactly the empty array satisfies both arrays:
                    // `Vec<Never>` is faithful.
                    Err(NoMeet::Empty) => {
                        self.insert_type(&item_hint, TypeKind::Never, Docs::default(), None)
                    }
                    // Items both sides admit exist, and `Vec<Never>` would refuse every array that
                    // holds one.
                    Err(NoMeet::Unrepresentable) => return Err(NoMeet::Unrepresentable),
                };
                if same_ty(item, **left) {
                    Ok(non_nullable(a))
                } else if same_ty(item, **right) {
                    Ok(non_nullable(b))
                } else {
                    Ok(self.insert_type(
                        hint,
                        TypeKind::Array(Box::new(item)),
                        Docs::default(),
                        None,
                    ))
                }
            }
            // A position with no intersection is unrepresentable rather than empty, whichever way
            // it fails: `prefixItems` does not require the array to reach that position, so an
            // array shorter than it still satisfies both tuples.
            (TypeKind::Tuple(left), TypeKind::Tuple(right)) if left.len() == right.len() => {
                let items = left
                    .iter()
                    .zip(right)
                    .enumerate()
                    .map(|(index, (left, right))| {
                        self.intersect_types(*left, *right, &format!("{hint}Item{index}"))
                            .ok()
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(NoMeet::Unrepresentable)?;
                Ok(self.insert_type(hint, TypeKind::Tuple(items), Docs::default(), None))
            }
            // A homogeneous array against a tuple: every tuple position must also satisfy the
            // array's item schema, and the length is the tuple's. So the intersection is the tuple
            // with each position narrowed by the item — `{$ref: Coord, type: array}` over a
            // `prefixItems` `Coord` is `Coord`. A position with no intersection leaves no tuple,
            // and, as for two tuples, that is unrepresentable rather than empty.
            (TypeKind::Array(item), TypeKind::Tuple(positions)) => {
                self.intersect_array_tuple(**item, positions, b, hint)
            }
            (TypeKind::Tuple(positions), TypeKind::Array(item)) => {
                self.intersect_array_tuple(**item, positions, a, hint)
            }
            (TypeKind::Struct(left), TypeKind::Struct(right)) => {
                let location = self
                    .authored_location(a)
                    .or_else(|| self.authored_location(b));
                self.intersect_structs(left, right, hint, location)
            }
            // A union's variants stay closed for the reason `lower_union_closed` gives; a union
            // that narrows to one branch is no union, and `intersect_union` meets that branch where
            // the enclosing position's answer holds.
            (TypeKind::Union(union), _) => {
                let enclosing = self.narrowing_opens;
                self.closed_narrowing(|ctx| {
                    let reach = &mut ScopeReach::default();
                    ctx.intersect_union(a, union, Refiner::Whole(b), hint, enclosing, reach)
                })
            }
            (_, TypeKind::Union(union)) => {
                let enclosing = self.narrowing_opens;
                self.closed_narrowing(|ctx| {
                    let reach = &mut ScopeReach::default();
                    ctx.intersect_union(b, union, Refiner::Whole(a), hint, enclosing, reach)
                })
            }
            (TypeKind::Bytes, TypeKind::Bytes) => Ok(non_nullable(a)),
            // Binary content (`format: binary` / `contentEncoding: base64`) is a string, so a plain
            // string conjoined with it is the binary content: `{$ref: Data, format: binary}` over a
            // string `Data` lowers exactly as the inline `{type: string, format: binary}` does. Only
            // the unformatted string: `uuid` and the date formats carry a decoded representation of
            // their own that `Bytes` cannot also be, so that pair is unrepresentable (`no_meet`).
            (TypeKind::Bytes, TypeKind::Primitive(Prim::String)) => Ok(non_nullable(a)),
            (TypeKind::Primitive(Prim::String), TypeKind::Bytes) => Ok(non_nullable(b)),
            _ => Err(no_meet(a_kind, b_kind)),
        }
    }

    /// The meet of the scalar set `set` (whose type is `enum_ty`) with the primitive `primitive` it
    /// is a set of: the set itself, or — for a closed string set narrowing a plain `string` where
    /// [`Self::narrowing_opens`] holds — an open set listing the same values, whose domain is the
    /// `string` it narrowed. That is the set itself, opened, when it is one of
    /// [`Self::open_candidates`], and a new open copy otherwise (a set that came from a `$ref`
    /// target, or from another intersection, may be reached from where it must stay closed).
    ///
    /// Only a plain `string` widens it: `uuid` and the date formats have a decoded representation
    /// of their own that an arbitrary string is not. Under `open_narrowing` (in a response body's
    /// own schema or not), a string set meeting one of them is [`Openness::Locked`] instead, so no
    /// plain `string` met before or after opens it: the set an `allOf` lowers to does not depend
    /// on where its formatted member sits.
    ///
    /// A set the response already opened, met where [`Self::narrowing_opens`] does not hold (a
    /// union variant `intersect_union` meets it with), is a new closed copy under `hint`, for the
    /// reason the enum-meet arm of [`Self::intersect_non_null`] gives.
    fn narrowed_string(
        &mut self,
        enum_ty: Ty,
        set: &ScalarEnum,
        primitive: Prim,
        hint: &str,
    ) -> Ty {
        let openness = match (set.openness, primitive) {
            _ if set.repr != ScalarRepr::String => set.openness,
            (_, Prim::Uuid | Prim::Date | Prim::DateTime) if self.open_narrowing => {
                Openness::Locked
            }
            (Openness::Closed, Prim::String) if self.narrowing_opens => Openness::Open,
            (Openness::Open, _) if !self.narrowing_opens => Openness::Closed,
            (openness, _) => openness,
        };
        self.reopened_set(enum_ty, set, openness, hint)
    }

    /// The set `set` (whose type is `enum_ty`) with `openness`: the set itself when it already has
    /// it, the set opened as [`Self::opened_set`] does, and otherwise a closed or locked one. That
    /// is the set itself, changed in place, when it is one of [`Self::open_candidates`] met where
    /// [`Self::narrowing_opens`] holds, and a new copy under `hint` otherwise, since a set reached
    /// from anywhere else may be reached from where it must keep its own openness.
    fn reopened_set(
        &mut self,
        enum_ty: Ty,
        set: &ScalarEnum,
        openness: Openness,
        hint: &str,
    ) -> Ty {
        if openness == set.openness {
            return non_nullable(enum_ty);
        }
        if openness == Openness::Open {
            return self.opened_set(enum_ty, set);
        }
        if self.narrowing_opens && self.reopen_in_place(enum_ty, openness) {
            return non_nullable(enum_ty);
        }
        self.insert_type(
            hint,
            TypeKind::Enum(ScalarEnum {
                openness,
                ..set.clone()
            }),
            Docs::default(),
            None,
        )
    }

    /// Give the set `enum_ty` `openness` in place, when it is one of [`Self::open_candidates`]:
    /// whether it was.
    fn reopen_in_place(&mut self, enum_ty: Ty, openness: Openness) -> bool {
        if !self.open_candidates.contains(&enum_ty.id) {
            return false;
        }
        if let Some(TypeKind::Enum(own)) = self.graph.get_mut(enum_ty.id).map(|def| &mut def.kind) {
            own.openness = openness;
            return true;
        }
        false
    }

    /// The closed set `set` (whose type is `enum_ty`), opened: in place when it is one of
    /// [`Self::open_candidates`], and as a new open copy otherwise, as [`Self::narrowed_string`]
    /// describes.
    fn opened_set(&mut self, enum_ty: Ty, set: &ScalarEnum) -> Ty {
        if self.reopen_in_place(enum_ty, Openness::Open) {
            return non_nullable(enum_ty);
        }
        // Named for the closed set it opens, which stays in the graph where it came from.
        let (name_hint, docs, provenance) = match self.graph.get(enum_ty.id) {
            Some(def) => (
                format!("{}Open", def.name_hint),
                def.docs.clone(),
                Some(def.provenance.clone()),
            ),
            None => return non_nullable(enum_ty),
        };
        self.insert_type(
            &name_hint,
            TypeKind::Enum(ScalarEnum {
                repr: ScalarRepr::String,
                variants: set.variants.clone(),
                openness: Openness::Open,
            }),
            docs,
            provenance,
        )
    }

    /// The id the next graph insert takes: every type inserted from here on has an id at or above
    /// it. [`Self::discard_meet_intermediates`] takes it back.
    fn graph_mark(&self) -> u32 {
        self.graph.last_id().map_or(0, |id| id.0 + 1)
    }

    /// [`TypeGraph::pop_last`], also forgetting the location [`Self::meet_locations`] recorded for
    /// the popped id. The next insert reuses that id for a type of its own, which would otherwise
    /// inherit the popped meet's location. Every pop in lowering goes through here.
    fn pop_last_type(&mut self) -> Option<(TypeId, TypeDef)> {
        let popped = self.graph.pop_last();
        if let Some((id, _)) = &popped {
            self.meet_locations.remove(id);
        }
        popped
    }

    /// Discard every type inserted since `mark` that `kind` does not refer to, directly or
    /// transitively. The caller has just met two or more types inserted before `mark` and is about
    /// to re-emit the meet's result as a new definition of `kind`, so the meets' own inserts are
    /// unused unless that definition reaches them. Each would otherwise be emitted as a public type
    /// nothing refers to: the open or locked copy [`Self::reopened_set`] makes of a `$ref`'d set
    /// (#401), and every intermediate a later meet superseded.
    ///
    /// When `kind` reaches none of them, all are removed (#401). Otherwise only the most recent
    /// could be, since ids are dense, so the unused ones are elided instead, as
    /// [`Self::elide_meet_intermediates`] does.
    fn discard_meet_intermediates(&mut self, mark: u32, kind: &TypeKind) {
        let reached = reachable_types(&self.graph, &kind_edges(kind));
        if reached.iter().any(|id| id.0 >= mark) {
            self.elide_unreached(mark, &reached);
            return;
        }
        while self.graph.last_id().is_some_and(|id| id.0 >= mark) {
            self.pop_last_type();
        }
    }

    /// [Elide](TypeGraph::elide) every type inserted since `mark` that `kind` does not refer to,
    /// directly or transitively. The caller has just met the properties its members repeat, each
    /// meet replacing the field's type, and is about to emit `kind`, the struct that refers to the
    /// last meet of each property and not to the ones a later member superseded: the open copy
    /// [`Self::reopened_set`] makes of a `$ref`'d set, or the struct an earlier pair of members met
    /// a repeated object property in (#428). Those are interleaved with the inserts the struct
    /// uses, so they cannot be popped; eliding keeps each one's id and name, so no type the output
    /// carries is renamed or reordered.
    ///
    /// Sound because intersecting only reads the graph and inserts into it: it lowers no schema and
    /// fills no memo, so nothing outside the inserts since `mark` refers to them, and an in-place
    /// change of an earlier set's openness ([`Self::reopen_in_place`]) is kept.
    fn elide_meet_intermediates(&mut self, mark: u32, kind: &TypeKind) {
        let reached = reachable_types(&self.graph, &kind_edges(kind));
        self.elide_unreached(mark, &reached);
    }

    /// Elide every type inserted since `mark` that is not in `reached`.
    fn elide_unreached(&mut self, mark: u32, reached: &HashSet<TypeId>) {
        let Some(last) = self.graph.last_id() else {
            return;
        };
        for id in (mark..=last.0).map(TypeId) {
            if !reached.contains(&id) {
                self.graph.elide(id);
            }
        }
    }

    /// Run `lower` with `open_narrowing` out of effect, restoring the enclosing position's answer
    /// afterwards. Every `$ref` target and every union is lowered through this.
    fn closed_narrowing<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let enclosing = std::mem::replace(&mut self.narrowing_opens, false);
        let lowered = lower(self);
        self.narrowing_opens = enclosing;
        lowered
    }

    /// Run `lower` over a response body's own schema: with `open_narrowing` in effect when the
    /// option is on, restoring the enclosing answer afterwards.
    fn response_narrowing<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let enclosing = std::mem::replace(&mut self.narrowing_opens, self.open_narrowing);
        let lowered = lower(self);
        self.narrowing_opens = enclosing;
        lowered
    }

    /// The intersection of a homogeneous array whose items are `item` with the tuple `tuple`, whose
    /// positions are `positions`: the tuple, each position intersected with `item`. Returns the
    /// tuple itself when no position narrowed, and [`NoMeet::Unrepresentable`] when any position
    /// has no intersection.
    fn intersect_array_tuple(
        &mut self,
        item: Ty,
        positions: &[Ty],
        tuple: Ty,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let items = positions
            .iter()
            .enumerate()
            .map(|(index, position)| {
                self.intersect_types(*position, item, &format!("{hint}Item{index}"))
                    .ok()
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(NoMeet::Unrepresentable)?;
        if items
            .iter()
            .zip(positions)
            .all(|(narrowed, position)| same_ty(*narrowed, *position))
        {
            Ok(non_nullable(tuple))
        } else {
            Ok(self.insert_type(hint, TypeKind::Tuple(items), Docs::default(), None))
        }
    }

    fn intersect_structs(
        &mut self,
        left: &Struct,
        right: &Struct,
        hint: &str,
        location: Option<Provenance>,
    ) -> Result<Ty, NoMeet> {
        let mut fields: IndexMap<String, Field> = left
            .fields
            .iter()
            .cloned()
            .map(|field| (field.name.wire.clone(), field))
            .collect();
        // An unrepresentable property is remembered rather than returned at once: a later
        // required property whose types are disjoint still proves the whole object empty.
        let mut unrepresentable = false;
        for field in &right.fields {
            match fields.get_mut(&field.name.wire) {
                Some(existing) => {
                    // A field one side carries only for its `required` gives way to the other
                    // side's declaration of the property (see `take_declaration`).
                    if take_declaration(existing, field) {
                        continue;
                    }
                    // Either side's `default` is a default of the merged field, whichever side is
                    // the `$ref` (see `merge_field_default`).
                    merge_field_default(
                        &mut existing.default,
                        field.default.as_ref(),
                        &field.name.wire,
                        self.diags,
                    );
                    let field_hint = format!("{hint}{}", field.name.wire);
                    let intersection = self.intersect_types(existing.ty, field.ty, &field_hint);
                    let required = existing.required || field.required;
                    existing.ty = match intersection {
                        Ok(ty) => ty,
                        // Mirrors the array arm above, and for the same reason `E013`'s explain
                        // gives for it: a property NEITHER side requires does not empty the
                        // object when its two types cannot meet, because every instance that
                        // omits it still satisfies both sides. The field takes an uninhabited
                        // type, so the instances that remain representable are exactly the valid
                        // ones. Propagating the failure would reject a document that `{}`
                        // satisfies. An applied `default` is no value of the uninhabited type, and
                        // is left for `retype_field_defaults` to report (`W005`) where it was
                        // written and document as not applied (#453).
                        Err(NoMeet::Empty) if !required => {
                            self.insert_type(&field_hint, TypeKind::Never, Docs::default(), None)
                        }
                        // Required on one side or the other: every instance must carry a value no
                        // type admits, so the composition really is empty.
                        Err(NoMeet::Empty) => return Err(NoMeet::Empty),
                        // Values both sides admit exist, so an uninhabited field would refuse every
                        // object carrying one, required or not.
                        Err(NoMeet::Unrepresentable) => {
                            unrepresentable = true;
                            continue;
                        }
                    };
                    existing.required = required;
                    if existing.required {
                        if let Some(default) = &mut existing.default {
                            default.applied = None;
                        }
                    }
                }
                None => {
                    fields.insert(field.name.wire.clone(), field.clone());
                }
            }
        }
        // A field neither side declares is an undeclared key of the side that does not carry it
        // too, so that side's `additionalProperties` value schema constrains it:
        // `{$ref: Labels, required: [a]}` with string-valued `Labels` makes `a` a string, not an
        // unconstrained value. The field is required, so a value no type admits empties the object.
        for field in fields.values_mut() {
            if !field.undeclared {
                continue;
            }
            let other = if left
                .fields
                .iter()
                .any(|carried| carried.name.wire == field.name.wire)
            {
                if right
                    .fields
                    .iter()
                    .any(|carried| carried.name.wire == field.name.wire)
                {
                    continue;
                }
                &right.additional
            } else {
                &left.additional
            };
            let field_hint = format!("{hint}{}", field.name.wire);
            match self.narrow_undeclared(field.ty, other, &field_hint) {
                Ok(ty) => field.ty = ty,
                Err(NoMeet::Empty) => return Err(NoMeet::Empty),
                Err(NoMeet::Unrepresentable) => unrepresentable = true,
            }
        }
        if unrepresentable {
            return Err(NoMeet::Unrepresentable);
        }
        // Two additional-value types that do not meet leave the object inhabited (one with no
        // additional key satisfies both), so that failure is never an empty object.
        let additional = self
            .merge_additional(
                &left.additional,
                &right.additional,
                &format!("{hint}Additional"),
            )
            .ok_or(NoMeet::Unrepresentable)?;
        let meet = self.insert_type(
            hint,
            TypeKind::Struct(Struct {
                fields: fields.into_values().collect(),
                additional,
            }),
            Docs::default(),
            None,
        );
        if let Some(location) = location {
            self.meet_locations.insert(meet.id, location);
        }
        Ok(meet)
    }

    /// Where the schema `ty` lowers from was authored: the location [`Self::meet_locations`]
    /// recorded for a meet struct, else the type's own provenance, unless that is the document root
    /// every synthesized type falls back to.
    fn authored_location(&self, ty: Ty) -> Option<Provenance> {
        if let Some(location) = self.meet_locations.get(&ty.id) {
            return Some(location.clone());
        }
        let def = self.graph.get(ty.id)?;
        (!def.provenance.pointer.as_str().is_empty()).then(|| def.provenance.clone())
    }

    /// The one type every variant of `ty` shares, when `ty` is a union of two or more variants that
    /// are all the same type. No value can tell such variants apart, so the union adds nothing to
    /// its common type — and a `oneOf` of them rejects every value its common type accepts.
    /// `structural` also counts variants that are distinct definitions decoding the same values —
    /// one Rust type, or distinct items of one structure (#492) — as the same
    /// ([`Self::same_type_apart_from_null`]), unless a discriminator tells them apart by
    /// tag. Returned with the number of variants that accept `null`: after the meet with a `$ref`
    /// target each variant keeps its own nullability, so structurally shared variants need not
    /// agree on it.
    fn indistinguishable_union_variant(&self, ty: Ty, structural: bool) -> Option<(Ty, usize)> {
        let Some(TypeKind::Union(union)) = self.graph.get(ty.id).map(|def| &def.kind) else {
            return None;
        };
        let structural =
            structural && !matches!(union.strategy, UnionStrategy::Discriminated { .. });
        let (first, rest) = union.variants.split_first()?;
        let nullable = union
            .variants
            .iter()
            .filter(|variant| variant.ty.nullable)
            .count();
        (!rest.is_empty()
            && rest.iter().all(|variant| {
                same_ty(variant.ty, first.ty)
                    || (structural && self.same_type_apart_from_null(variant.ty, first.ty))
            }))
        .then_some((first.ty, nullable))
    }

    /// Whether two `oneOf` variants decode the same values — one Rust type, or two of one
    /// structure ([`TypeGraph::same_decoded_values`]) — once each one's own `null` is set aside.
    /// No non-null value tells them apart, so they are one variant whatever their nullability; the
    /// callers decide `null` separately, by how many of the merged branches accept it.
    fn same_type_apart_from_null(&self, a: Ty, b: Ty) -> bool {
        self.graph
            .same_decoded_values(non_nullable(a), non_nullable(b))
    }

    /// Collapse `met`, the meet of a `oneOf` (`one_of`) or `anyOf` with what the union is
    /// conjoined with, as each of the [`MetUnion`] spellings must: the union is lowered with its
    /// own merge held back ([`Self::unmerged_union`]), because its branches are compared once the
    /// meet has made them what they are. Returns `met` itself where nothing collapses, beside
    /// whether the result is a `oneOf` whose `serde_json::Value` variants the caller reports
    /// ([`Self::warn_untyped_met_variants`]) once nothing else meets it.
    fn collapse_met_union(
        &mut self,
        schema: &Schema,
        met: Ty,
        one_of: bool,
        hint: &str,
        spelling: MetUnion,
    ) -> (Ty, bool) {
        // A `oneOf`'s branches are compared by generated type, as the inline merge compares them
        // (#402): two distinct `i64`-alias enums are one Rust type, so no value tells them apart,
        // and two structs or string enums of one structure decode the same values (#492).
        // An `anyOf` keeps the identity comparison it has always had.
        match self.indistinguishable_union_variant(met, one_of) {
            // Every branch of the intersected union is one and the same type: the branches differ
            // only in keywords the lowered shape does not carry, such as a branch of nothing but
            // `required` (#140). Emitting them as a union gives a `oneOf` whose exactly-one check
            // fails on every value, so the position takes that one type and the ignored branch
            // distinctions are reported, not dropped in silence.
            Some((mut common, branches_accepting_null)) => {
                // `branches_accepting_null` counts the branches that accept `null`, and
                // `met.nullable` says whether the union's own `null` member survived the meet. An
                // `anyOf` needs one match, so `null` is valid when any of them admits it. A
                // `oneOf` needs exactly one: its branches are grouped apart from their own `null`,
                // so they need not agree on it, and `null` is valid only when exactly one of the
                // branches and that member accepts it — two put it in two branches, which fails
                // exactly-one.
                common.nullable = if one_of {
                    usize::from(met.nullable) + branches_accepting_null == 1
                } else {
                    met.nullable || branches_accepting_null > 0
                };
                Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
                    .message(format!(
                        "{} intersect to a union whose branches differ only in keywords the \
                         generated type does not carry, so which branch a value matches is not \
                         enforced",
                        spelling.subject(false)
                    ))
                    .remedy("keep producer-side validation for the union's branch constraints")
                    .emit(self.diags);
                (common, false)
            }
            // Only some branches share a generated type: they become one variant, as the inline
            // merge makes them, and the others stand. A `serde_json::Value` variant left among them
            // is the caller's to report ([`Self::warn_untyped_met_variants`]), once whatever still
            // meets the union after the collapse has made its branches final (#535).
            None if one_of => {
                let collapsed = self
                    .merge_intersected_one_of(schema, met, hint, spelling)
                    .unwrap_or(met);
                (collapsed, true)
            }
            None => (met, false),
        }
    }

    /// Report, as the inline union reports them ([`Self::warn_untyped_one_of_variants`]), the
    /// `serde_json::Value` variants of `met`, a `oneOf` meet that [`Self::collapse_met_union`]
    /// left a union (#535). Called on the union as it is generated: after the `allOf` refiners,
    /// whose untyped object or array keywords give a branch of no category a type of their own, so
    /// a branch they refine is not reported as accepting every value.
    fn warn_untyped_met_variants(&mut self, schema: &Schema, met: Ty, spelling: MetUnion) {
        if let Some(TypeKind::Union(union)) = self.graph.get(met.id).map(|def| def.kind.clone()) {
            let positions: Vec<usize> = (0..union.variants.len()).collect();
            self.warn_untyped_one_of_variants(
                &schema.provenance,
                &union,
                &positions,
                &format!("{} intersect to a `oneOf` whose", spelling.subject(true)),
                "variant",
            );
        }
    }

    /// The `oneOf` union `ty` a `$ref` met its own sibling to, with the variants that lower to one
    /// generated type merged into the first of them, or `None` when no two do — the post-meet
    /// counterpart of [`Self::merge_indistinguishable_variants`] (#402). Called once
    /// [`Self::indistinguishable_union_variant`] has found that not every variant shares one type,
    /// so two or more variants remain. Variants are grouped apart from their own `null`
    /// ([`Self::same_type_apart_from_null`]). `null` is counted across the union's own `null`
    /// member and every branch of every set: where exactly one accepts it, that one keeps it; where
    /// two or more do, `null` is in two branches, which fails exactly-one, so it is then invalid
    /// everywhere in the union.
    fn merge_intersected_one_of(
        &mut self,
        schema: &Schema,
        ty: Ty,
        hint: &str,
        spelling: MetUnion,
    ) -> Option<Ty> {
        let TypeKind::Union(union) = &self.graph.get(ty.id)?.kind else {
            return None;
        };
        if matches!(union.strategy, UnionStrategy::Discriminated { .. }) {
            return None;
        }
        let union = union.clone();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            let shared = groups.iter_mut().find(|group| {
                self.same_type_apart_from_null(union.variants[group[0]].ty, variant.ty)
            });
            match shared {
                Some(group) => group.push(index),
                None => groups.push(vec![index]),
            }
        }
        if groups.len() == union.variants.len() {
            return None;
        }
        // The members of a set need not agree on `null`, and neither need the sets nor the union's
        // own `null` member (`ty.nullable`). `null` is counted across all of them, as the
        // all-collapse path in [`Self::collapse_met_union`] counts it: it stays valid, where it
        // was, only when exactly one source accepts it; two or more put `null` in two branches,
        // which fails exactly-one everywhere in the union.
        let accepting_null = |group: &Vec<usize>| {
            group
                .iter()
                .filter(|index| union.variants[**index].ty.nullable)
                .count()
        };
        let null_sources =
            usize::from(ty.nullable) + groups.iter().map(accepting_null).sum::<usize>();
        let null_twice = null_sources > 1;
        let retained: Vec<usize> = groups.iter().map(|group| group[0]).collect();
        let variants = groups
            .iter()
            .map(|group| {
                let mut variant = union.variants[group[0]].clone();
                variant.ty.nullable = accepting_null(group) == 1 && !null_twice;
                variant
            })
            .collect();
        Diagnostic::warning(Code::ValidationKeywordIgnored, schema.provenance.clone())
            .message(format!(
                "{} intersect to a union some of whose branches lower to the same generated type \
                 or to identically structured ones, differing only in keywords it does not carry, \
                 so a value matching one matches all \
                 of them and would fail the exactly-one rule: each such set is one variant of the \
                 generated enum, and which of them a value matches is not enforced",
                spelling.subject(true)
            ))
            .remedy("keep producer-side validation for the union's branch constraints")
            .emit(self.diags);
        let strategy = retain_strategy(&union.strategy, &retained);
        let mut merged = self.insert_type(
            hint,
            TypeKind::Union(Union { variants, strategy }),
            Docs::default(),
            None,
        );
        merged.nullable = ty.nullable && !null_twice;
        merged.boxed = ty.boxed;
        Some(merged)
    }

    /// The meet of the union `union` (whose type is `union_ty`) with `other`, branch by branch.
    /// Called with `open_narrowing` out of effect, so every retained branch is closed.
    /// `enclosing_opens` is [`Self::narrowing_opens`] where the meet was asked for: when exactly one
    /// branch survives, the result is no union, and that branch is met again under that answer, so
    /// the meet is the same set whichever order the `allOf` writes the union and the `string` in
    /// (written first, the union narrows to that branch before the `string` opens it).
    ///
    /// Each branch is met through [`Self::meet_refiner`], so a [`Refiner::Scoped`] `other` leaves
    /// the branches of another category as they are and records what it reached in `reach`.
    fn intersect_union(
        &mut self,
        union_ty: Ty,
        union: &Union,
        other: Refiner,
        hint: &str,
        enclosing_opens: bool,
        reach: &mut ScopeReach,
    ) -> Result<Ty, NoMeet> {
        let mut variants = Vec::new();
        let mut retained = Vec::new();
        for (index, variant) in union.variants.iter().enumerate() {
            match self.meet_refiner(variant.ty, other, reach, &format!("{hint}Variant{index}")) {
                Ok(ty) => {
                    variants.push(UnionVariant {
                        name_hint: variant.name_hint.clone(),
                        ty,
                    });
                    retained.push(index);
                }
                // A branch no value of `other` satisfies contributes nothing to the intersection,
                // so dropping it loses no value.
                Err(NoMeet::Empty) => {}
                // A branch that shares values with `other` but has no type for them cannot be
                // dropped without refusing those values, whatever the other branches do.
                Err(NoMeet::Unrepresentable) => return Err(NoMeet::Unrepresentable),
            }
        }
        if variants.len() == 1 {
            if enclosing_opens {
                let branch = union.variants[retained[0]].ty;
                return self.response_narrowing(|ctx| ctx.meet_refiner(branch, other, reach, hint));
            }
            return Ok(variants.remove(0).ty);
        }
        if variants.is_empty() {
            return Err(NoMeet::Empty);
        }
        if variants.len() == union.variants.len()
            && variants
                .iter()
                .zip(&union.variants)
                .all(|(left, right)| same_ty(left.ty, right.ty))
        {
            return Ok(non_nullable(union_ty));
        }
        let strategy = retain_strategy(&union.strategy, &retained);
        Ok(self.insert_type(
            hint,
            TypeKind::Union(Union { variants, strategy }),
            Docs::default(),
            None,
        ))
    }

    /// Whether two lowered value types would emit the *same* Rust type as a shared map value, so
    /// multiple `patternProperties`/`additionalProperties` values can collapse into one typed
    /// overflow map. A bounded structural equivalence:
    ///
    /// * equal `TypeId` (with equal `nullable`) — a shared `$ref` or the single-entry case;
    /// * otherwise, for distinct ids with equal `nullable`, compare the def kinds structurally but
    ///   only for *leaf* shapes that have no per-inline-schema identity: `Primitive` (same `Prim`),
    ///   `Bytes`, `Any`, and `Array` (recursing on the element). Composite kinds
    ///   (`Struct`/`Enum`/`Tuple`) generate a distinct named Rust type per inline schema, so two
    ///   such inline shapes are treated as heterogeneous (→ `E005`) rather than silently merged.
    ///
    /// `boxed` is deliberately ignored: it is a use-site indirection modifier, not part of the map
    /// value's emitted type (the map value is never boxed).
    ///
    /// The `Array` recursion is *not* structurally bounded — array element types can form `$ref`
    /// cycles (`A = [B]`, `B = [A]`) — so a visited-pair guard makes it terminate: an `(a.id, b.id)`
    /// pair already on the comparison stack is a co-recursive back-edge and compares equal (the two
    /// types are being compared identically along the cycle, so they are structurally equal there).
    fn same_map_value_type(&self, a: Ty, b: Ty) -> bool {
        self.same_map_value_type_guarded(a, b, &mut Vec::new())
    }

    fn same_map_value_type_guarded(
        &self,
        a: Ty,
        b: Ty,
        visiting: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        if a.nullable != b.nullable {
            return false;
        }
        if a.id == b.id {
            return true;
        }
        let pair = (a.id, b.id);
        if visiting.contains(&pair) {
            // Co-recursive back-edge: the same pair is already being compared further up the stack.
            // Along a cycle the two types are compared identically, so they are structurally equal.
            return true;
        }
        visiting.push(pair);
        let result = match (self.graph.get(a.id), self.graph.get(b.id)) {
            (Some(a_def), Some(b_def)) => match (&a_def.kind, &b_def.kind) {
                (TypeKind::Primitive(x), TypeKind::Primitive(y)) => x == y,
                (TypeKind::Bytes, TypeKind::Bytes) => true,
                (TypeKind::Null, TypeKind::Null) | (TypeKind::Never, TypeKind::Never) => true,
                (TypeKind::Any, TypeKind::Any) => true,
                (TypeKind::Array(x), TypeKind::Array(y)) => {
                    self.same_map_value_type_guarded(**x, **y, visiting)
                }
                // An unlowered body cannot be proven the same value type as anything else, so the
                // pair is heterogeneous and the map is rejected with `E005` rather than merged on
                // a guess. The same reservation on both sides already answered `true` by id.
                (TypeKind::Reserved, _) | (_, TypeKind::Reserved) => false,
                _ => false,
            },
            _ => false,
        };
        visiting.pop();
        result
    }

    /// Give a property's `default` its single explicit disposition. Returns `None` when the
    /// property declared no `default`; otherwise a [`FieldDefault`] whose `applied` is set only for
    /// a representable scalar on a plain optional field. A non-representable default emits `W005`.
    fn field_default(&mut self, child: &SchemaOr, ty: Ty, required: bool) -> Option<FieldDefault> {
        let SchemaOr::Schema(schema) = child else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let classified = classify_default(raw);
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        let provenance = Provenance::new(schema.provenance.pointer.push("default"), Some(raw.span));
        match representable_default(&classified, kind) {
            Some(value) => {
                let display = default_display(&value);
                // A serde default only fires for an absent field on deserialization, so it is wired
                // only for a plain optional (non-required, non-nullable) scalar. A required field is
                // always present, and a nullable field already carries `Option`; both are documented
                // in rustdoc instead of silently ignored.
                let applied = (!required && !ty.nullable).then_some(value);
                Some(FieldDefault {
                    doc_note: format!("Default: `{display}`."),
                    applied,
                    provenance,
                    also_written: Vec::new(),
                })
            }
            None => {
                Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                    .message(
                        "schema `default` is not a scalar matching the field type; it is \
                         documented in rustdoc but not applied as a deserialization default",
                    )
                    .remedy(
                        "use a scalar default matching the field's own type, or set the value \
                         explicitly at each call site",
                    )
                    .emit(self.diags);
                Some(FieldDefault {
                    doc_note: format!("Default (not applied): `{}`.", raw_display(raw)),
                    applied: None,
                    provenance,
                    also_written: Vec::new(),
                })
            }
        }
    }

    /// Render the rustdoc `Default:` note for a parameter's schema `default`, if it declared one.
    /// Parameter defaults are documented but never serde-wired.
    fn param_default_display(&self, schema: Option<&RefOr<Schema>>, ty: Ty) -> Option<String> {
        let RefOr::Item(schema) = schema? else {
            return None;
        };
        let raw = schema.default.as_ref()?;
        let kind = self.graph.get(ty.id).map(|def| &def.kind);
        Some(default_display_for(raw, kind))
    }

    /// A `default` in a structural position with no field/parameter/type home of its own —
    /// array `items`, tuple `prefixItems`, `additionalProperties` value, or a request/response body
    /// root — cannot be applied or documented against a named item, so it is reported as `W005`
    /// rather than dropped silently.
    fn warn_structural_default_or(&mut self, schema: &SchemaOr, position: &str) {
        if let SchemaOr::Schema(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    fn warn_structural_default_ref(&mut self, schema: &RefOr<Schema>, position: &str) {
        if let RefOr::Item(schema) = schema {
            self.warn_structural_default(schema, position);
        }
    }

    fn warn_structural_default(&mut self, schema: &Schema, position: &str) {
        if schema.default.is_some() {
            Diagnostic::warning(Code::SchemaDefaultNotApplied, schema.provenance.clone())
                .message(format!(
                    "schema `default` on {position} has no field to carry it and is not applied"
                ))
                .remedy("move the default onto a named property, or set the value explicitly")
                .emit(self.diags);
        }
    }

    fn lower_enum(&mut self, values: &[SpannedValue], schema: &Schema, hint: &str) -> Option<Ty> {
        // A `null` member — or `"null"` in the schema's own type array — makes the enum/const
        // nullable: strip the nulls, lower the remaining scalars as the enum, and wrap the result
        // in `Option`. The enum/const branch returns before `lower_schema` computes `nullable`, so
        // the nullability has to be decided here from both sources.
        let has_null = schema.types.types.contains(&JsonType::Null)
            || values.iter().any(|value| matches!(value.node, Node::Null));
        // Declared order is preserved (minus nulls) so double generation stays byte-identical.
        let remainder: Vec<&SpannedValue> = values
            .iter()
            .filter(|value| !matches!(value.node, Node::Null))
            .collect();

        // Only `null` members remained (`enum: [null]` / `const: null`): emit the exact JSON null
        // type (`()`), not a nullable unconstrained value that would also accept non-null content.
        if remainder.is_empty() {
            return Some(self.insert_schema_type(schema, hint, TypeKind::Null));
        }

        let mut variants = Vec::new();
        let mut repr = None;
        for value in remainder {
            let scalar = match scalar_value(value) {
                Some(value) => value,
                None => {
                    Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                        .message(non_scalar_enum_message(value))
                        .emit(self.diags);
                    return None;
                }
            };
            let scalar_repr = match scalar {
                ScalarValue::Bool(_) => ScalarRepr::Bool,
                ScalarValue::Int(_) => ScalarRepr::Int,
                ScalarValue::String(_) => ScalarRepr::String,
            };
            if repr
                .replace(scalar_repr)
                .is_some_and(|previous| previous != scalar_repr)
            {
                Diagnostic::error(Code::NonScalarEnum, schema.provenance.clone())
                    .message("enum/const values must all share the same scalar kind")
                    .emit(self.diags);
                return None;
            }
            variants.push(scalar);
        }
        // The enum def is the last graph insert; setting `nullable` afterward is a pure mutate that
        // preserves the component-root last-insert invariant asserted in `ensure_component`.
        let repr = repr.unwrap_or(ScalarRepr::String);
        // A string set whose own schema names a `uuid` or date format is narrowed against that
        // format exactly as an `allOf` member declaring it would narrow it (`narrowed_string`).
        let formatted = matches!(
            schema.format.as_deref(),
            Some("uuid" | "date" | "date-time")
        );
        let openness = if self.open_narrowing && repr == ScalarRepr::String && formatted {
            Openness::Locked
        } else {
            Openness::Closed
        };
        let mut ty = self.insert_schema_type(
            schema,
            hint,
            TypeKind::Enum(ScalarEnum {
                repr,
                variants,
                openness,
            }),
        );
        if self.narrowing_opens && repr == ScalarRepr::String {
            self.open_candidates.insert(ty.id);
        }
        ty.nullable = has_null;
        Some(ty)
    }

    /// A parameter is always rendered to a wire string — path/query/header/cookie interpolation or a
    /// serialized content value — and `bytes::Bytes` (from `format: binary` / `contentEncoding:
    /// base64`) is not `Display` and has no faithful string rendering. `format: binary` on a
    /// parameter is conventionally just an opaque string, so a parameter whose type lowered to raw
    /// bytes is represented as a plain `String` instead — keeping the parameter renderable and
    /// matching the pre-`Bytes` behavior. Body/multipart binary lowering is unaffected.
    fn remap_binary_param(&mut self, ty: Ty, hint: &str) -> Ty {
        if matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Bytes)
        ) {
            let mut remapped = self.insert_type(
                hint,
                TypeKind::Primitive(Prim::String),
                Docs::default(),
                None,
            );
            remapped.nullable = ty.nullable;
            remapped
        } else {
            ty
        }
    }

    fn lower_parameter(&mut self, parameter: &ParameterObject) -> Option<Parameter> {
        let location = match parameter.location.as_str() {
            "path" => ParamLoc::Path,
            "query" => ParamLoc::Query,
            "header" => ParamLoc::Header,
            "cookie" => ParamLoc::Cookie,
            "querystring" => ParamLoc::QueryString,
            _ => {
                Diagnostic::error(Code::InvalidInput, parameter.provenance.clone())
                    .message(format!(
                        "unsupported parameter location `{}`",
                        parameter.location
                    ))
                    .emit(self.diags);
                return None;
            }
        };
        // `Accept`, `Content-Type`, and `Authorization` header parameters "SHALL be ignored": the
        // protocol layer owns those, and emitting a client argument for one would let a caller
        // silently fight the codec or the auth attachment.
        if location == ParamLoc::Header
            && matches!(
                parameter.name.to_ascii_lowercase().as_str(),
                "accept" | "content-type" | "authorization"
            )
        {
            // W011 case: reserved-header-parameter
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(format!(
                    "header parameter `{}` is ignored: the specification reserves `Accept`, \
                     `Content-Type`, and `Authorization` to the protocol layer",
                    parameter.name
                ))
                .remedy(
                    "remove the parameter; content types follow the operation's media types and \
                     credentials are registered with `Client::with_credential`",
                )
                .emit(self.diags);
            return None;
        }
        if location == ParamLoc::QueryString {
            let Some((media_name, object)) = parameter.content.iter().next() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("`in: querystring` requires one content media type with a schema")
                .emit(self.diags);
                return None;
            };
            let object = self.resolve_media_object(object, media_name)?;
            let media = lower_media_type(media_name, &parameter.provenance, self.diags)?;
            if !matches!(media, MediaType::Json | MediaType::FormUrlEncoded) {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "querystring media type `{media_name}` is not supported; use JSON or \
                         application/x-www-form-urlencoded"
                ))
                .emit(self.diags);
                return None;
            }
            let Some(schema) = object.schema.as_ref() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("querystring content requires a schema for a typed client argument")
                .emit(self.diags);
                return None;
            };
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            if media == MediaType::FormUrlEncoded
                && !matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Struct(_))
                )
            {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("form-urlencoded querystring parameters require an object schema")
                .emit(self.diags);
                return None;
            }
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required,
                style: ParamStyle::Content(media),
                allow_reserved: false,
                explode: true,
                deprecated: parameter.deprecated,
                default_display: self.param_default_display(object.schema.as_ref(), ty),
            });
        }
        let style_name = parameter.style.as_deref().unwrap_or(match location {
            ParamLoc::Path | ParamLoc::Header => "simple",
            ParamLoc::Query | ParamLoc::Cookie => "form",
            ParamLoc::QueryString => unreachable!("querystring returned above"),
        });
        // The legal `(style, in)` pairs are enforced by the official document schema before
        // lowering (`E011`), so an unknown pairing here is a generator bug rather than user input.
        // The arm is kept so a future schema relaxation cannot silently mis-serialize.
        let style = match (location, style_name) {
            (ParamLoc::Path | ParamLoc::Header, "simple") => ParamStyle::Simple,
            (ParamLoc::Path, "matrix") => ParamStyle::Matrix,
            (ParamLoc::Path, "label") => ParamStyle::Label,
            (ParamLoc::Query | ParamLoc::Cookie, "form") => ParamStyle::Form,
            (ParamLoc::Query, "spaceDelimited") => ParamStyle::Delimited(Delimiter::Space),
            (ParamLoc::Query, "pipeDelimited") => ParamStyle::Delimited(Delimiter::Pipe),
            (ParamLoc::Query, "deepObject") => ParamStyle::DeepObject,
            (ParamLoc::Cookie, "cookie") => ParamStyle::Cookie,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "parameter style `{style_name}` is not permitted for `{}` parameters",
                    parameter.location
                ))
                .emit(self.diags);
                return None;
            }
        };
        // `deepObject` ignores `explode` entirely; every other style defaults per the
        // specification (true only for `form` and 3.2's `cookie`).
        let explode = parameter
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form | ParamStyle::Cookie));
        if matches!(style, ParamStyle::Delimited(_)) && parameter.explode == Some(true) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(format!(
                "`style: {style_name}` with `explode: true` has no defined serialization"
            ))
            .remedy("set `explode: false`, which is the default for this style")
            .emit(self.diags);
            return None;
        }
        // Deprecated in 3.2, and inert for a typed client: an absent optional parameter is simply
        // not sent, so there is never a case where the client would send an empty string instead.
        if parameter.allow_empty_value {
            // W011 case: allow-empty-value
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowEmptyValue` has no effect: an optional parameter the caller omits is \
                     not sent at all",
                )
                .remedy("remove `allowEmptyValue`; it is deprecated in OpenAPI 3.2")
                .emit(self.diags);
        }
        // `allowReserved` only means anything where the location percent-encodes at all.
        if parameter.allow_reserved
            && (location == ParamLoc::Header || matches!(style, ParamStyle::Cookie))
        {
            // W011 case: allow-reserved-parameter
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowReserved` has no effect here: this parameter is sent without \
                     percent-encoding",
                )
                .remedy("remove `allowReserved`, or use `style: form` if encoding is wanted")
                .emit(self.diags);
        }
        let ty = if let Some(schema) = &parameter.schema {
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            self.remap_binary_param(ty, &parameter.name)
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let object = self.resolve_media_object(object, media)?;
            let media_name = media.clone();
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
            // A `content` parameter is rendered by its media codec. Only JSON and raw text have a
            // codec that produces a single parameter token; anything else would fall through to
            // `simple` serialization and be sent in the wrong format.
            if !matches!(media, MediaType::Json | MediaType::Text) {
                Diagnostic::error(Code::UnsupportedMediaType, parameter.provenance.clone())
                    .message(format!(
                        "`content` parameter media type `{media_name}` has no single-token \
                         serialization"
                    ))
                    .remedy(
                        "use `application/json` or a `text/*` media type, or describe the \
                         parameter with `schema` and a serialization style",
                    )
                    .emit(self.diags);
                return None;
            }
            let ty = object
                .schema
                .as_ref()
                .and_then(|schema| self.lower_schema_ref(schema, &parameter.name))?;
            let ty = self.remap_binary_param(ty, &parameter.name);
            let default_display = self.param_default_display(object.schema.as_ref(), ty);
            return Some(Parameter {
                name: parameter.name.clone(),
                location,
                ty,
                required: parameter.required || location == ParamLoc::Path,
                style: ParamStyle::Content(media),
                allow_reserved: false,
                explode: false,
                deprecated: parameter.deprecated,
                default_display,
            });
        } else {
            self.insert_type(
                &parameter.name,
                TypeKind::Any,
                Docs::default(),
                Some(parameter.provenance.clone()),
            )
        };
        if let Some((path, kind)) = uninhabited_parameter_part(&self.graph, ty) {
            // `false`, or an `allOf` whose members meet empty, admits no value at all (#407).
            // Nothing is nested, so "nested arrays or objects" would describe nothing the author
            // wrote; name the schema that admits nothing instead. A union with such a member
            // still admits its other members' values, so only the member is called uninhabited.
            let at = format!("{}{path}", parameter.name);
            let (message, remedy) = match kind {
                Uninhabited::Whole => (
                    format!(
                        "parameter schema `{at}` is uninhabited: no value satisfies it (`false`, \
                         or an `allOf` whose members conflict), so simple/form/deepObject \
                         serialization has no token for it"
                    ),
                    format!("give `{at}` a schema some value satisfies, or remove it"),
                ),
                Uninhabited::Member => (
                    format!(
                        "parameter schema `{at}` has a `oneOf`/`anyOf` member that is \
                         uninhabited: no value satisfies that member (`false`, or an `allOf` \
                         whose members conflict), so simple/form/deepObject serialization has no \
                         token for it"
                    ),
                    format!(
                        "remove the uninhabited member from `{at}`, or give it a schema some \
                         value satisfies"
                    ),
                ),
            };
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(message)
            .remedy(remedy)
            .emit(self.diags);
            return None;
        }
        if let Some(property) = unconstrained_parameter_property(&self.graph, ty) {
            // Most often a `required` name no `properties` entry declares, which is a required
            // field typed by `additionalProperties` and unconstrained without one (#140). An
            // arbitrary JSON value has no `key=value` token, and "nested arrays or objects"
            // would describe nothing the author wrote.
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(format!(
                "parameter property `{property}` is unconstrained: no schema constrains its \
                 value, so simple/form/deepObject serialization has no scalar token for it"
            ))
            .remedy(format!(
                "declare `{property}` under `properties` with a scalar schema, or give the object \
                 a scalar `additionalProperties` schema"
            ))
            .emit(self.diags);
            return None;
        }
        if !parameter_shape_supported(&self.graph, ty) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(
                "simple/form parameter serialization does not support nested arrays or objects",
            )
            .emit(self.diags);
            return None;
        }
        let default_display = self.param_default_display(parameter.schema.as_ref(), ty);
        Some(Parameter {
            name: parameter.name.clone(),
            location,
            ty,
            required: parameter.required || location == ParamLoc::Path,
            style,
            allow_reserved: parameter.allow_reserved,
            explode,
            deprecated: parameter.deprecated,
            default_display,
        })
    }

    fn lower_request_body(&mut self, body: &RequestBodyObject) -> Option<RequestBody> {
        // A structured-suffix range such as `application/*+json` ranks with the concrete types its
        // suffix covers, so it could win a tie or a rank and then be refused below as a range. While
        // a sibling can be sent, it is withheld from the choice and reported as not selected.
        let (candidates, withheld) = request_media_candidates(&body.content);
        let ChosenMedia {
            media: media_name,
            value: object,
            narrowing,
        } = choose_media(
            &candidates,
            &body.provenance,
            self.diags,
            BodyPosition::Request,
            |object: &&super::MediaTypeObject| media_object_is_opaque(object),
        )?;
        let lowered = self.lower_chosen_request_body(body, media_name, object)?;
        // Both `W014`s are emitted only now that every gate above has accepted the selection: a
        // refused one is reported by its `E009` alone, since no method narrows to a body that is
        // not lowered. First the alternatives `choose_media` passed over, then the withheld suffix
        // ranges — always in this order.
        let withheld = alternative_media_ignored(media_name, &withheld, &body.provenance);
        for warning in narrowing.into_iter().chain(withheld) {
            self.diags.emit(warning);
        }
        Some(lowered)
    }

    /// Lower the request body entry [`Self::lower_request_body`] selected, or `None` when one of
    /// the request-body gates (each an `E009`) refuses it.
    fn lower_chosen_request_body(
        &mut self,
        body: &RequestBodyObject,
        media_name: &str,
        object: &MediaTypeObject,
    ) -> Option<RequestBody> {
        let object = self.resolve_media_object(object, media_name)?;
        let media = lower_media_type(media_name, &body.provenance, self.diags)?;
        // A media *range* describes what a server may return, not what a client sends: `Content-Type`
        // requires a concrete type/subtype (RFC 9110 § 8.3), and a generated request puts its media
        // key on the wire verbatim. Emitting `Content-Type: video/*` would be an undispatchable
        // header, and picking a concrete member of the family would be spargen inventing what the
        // document declined to say — so it is rejected rather than guessed at. `choose_media`
        // selects a range for a request only once no concrete key beside it classifies, so this
        // fires only when the document offers nothing else spargen can send.
        if classify_media_range(media_essence(media_name)).is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is a media range, which describes a family rather \
                     than the concrete `Content-Type` a request must send"
                ))
                .remedy(
                    "name the concrete media type the request body is sent as, or omit this API \
                     segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // A structured-suffix range such as `application/*+json` is a range for the same reason,
        // even though the suffix arms classify it as the codec its suffix names.
        if media_essence_is_suffix_range(media_essence(media_name)) {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is a media range, which describes a family rather \
                     than the concrete `Content-Type` a request must send"
                ))
                .remedy(
                    "name the concrete media type the request body is sent as, or omit this API \
                     segment with spargen::omit!",
                )
                .emit(self.diags);
            return None;
        }
        // Streaming media is a response-only construct: a `text/event-stream` / `application/x-ndjson`
        // *request* body has no representation here, so it stays rejected (narrowed `E009`) rather
        // than silently degrade. (`choose_media` only picks it when no whole-body alternative exists.)
        if media.stream_framing().is_some() {
            Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                .message(format!(
                    "media type `{media_name}` is only supported for streaming response bodies, \
                     not request bodies"
                ))
                .remedy("send a non-streaming request body, or omit this API segment with spargen::omit!")
                .emit(self.diags);
            return None;
        }
        // A streaming request body is already rejected above, so any `itemSchema` reaching here sits
        // on a non-streaming media where it is meaningless; acknowledge it with `W010` rather than
        // dropping it silently.
        if object.item_schema.is_some() {
            Diagnostic::warning(Code::Oas32ConstructIgnored, body.provenance.clone())
                .message(
                    "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; on this \
                     request body it is not used",
                )
                .emit(self.diags);
        }
        let ty = object
            .schema
            .as_ref()
            .and_then(|schema| self.lower_schema_ref(schema, "RequestBody"));
        if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a request body schema");
        }
        // A `multipart/form-data` body is emitted as a `reqwest::multipart::Form` whose parts are the
        // fields of an object schema. A concrete non-object type (or a multipart body with no schema
        // at all) has no fields to enumerate as parts, so it stays unsupported (`E009`, narrowed)
        // rather than silently degrade. A schema that *failed* to lower for its own reason (`ty` is
        // `None` though a schema was declared) has already emitted that diagnostic — don't pile a
        // misleading "must be an object" E009 on top of it.
        if media == MediaType::Multipart {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "a `multipart/form-data` request body must be an object schema; its \
                         properties are the form parts, so a non-object multipart body is not \
                         representable",
                    )
                    .remedy(
                        "give the multipart body an object schema with a property per form part, \
                         or omit this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets("RequestBody", ty, object.schema.is_some(), &body.provenance)
        } else {
            ty
        };
        if let Some(ty) = ty {
            let compatible = match media {
                MediaType::Text => raw_text_type_supported(&self.graph, ty),
                MediaType::OctetStream => matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Bytes)
                ),
                _ => true,
            };
            if !compatible {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "media type `{media_name}` requires a string-like or binary schema that can be sent as a raw body"
                    ))
                    .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
            // A raw request body — `bytes::Bytes` under any media, which the emitter sends
            // verbatim, or anything under the raw text codec — is the literal content of the
            // request, so a schema admitting `null` would ask the caller for an `Option` whose
            // `None` the wire cannot carry. An absent body is `required: false`, a different
            // construct, so the `null` is refused rather than reinterpreted as one.
            if ty.nullable && (media == MediaType::Text || self.is_bytes(ty)) {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(format!(
                        "this `{media_name}` request body is sent as raw content, which has no \
                         wire representation of `null`, but its schema admits `null`"
                    ))
                    .remedy(
                        "remove `null` from the body schema (use `required: false` for a body \
                         that may be omitted), or omit this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        // A form-urlencoded body is rendered property by property, so it needs properties. Without
        // this gate a non-object body compiled and then failed at runtime inside the form encoder.
        if media == MediaType::FormUrlEncoded {
            let is_struct = matches!(
                ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            );
            let schema_failed_to_lower = object.schema.is_some() && ty.is_none();
            if !is_struct && !schema_failed_to_lower {
                Diagnostic::error(Code::UnsupportedMediaType, body.provenance.clone())
                    .message(
                        "an `application/x-www-form-urlencoded` request body must be an object \
                         schema; its properties are the form fields",
                    )
                    .remedy(
                        "give the body an object schema with a property per form field, or omit \
                         this API segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let encoding = self.lower_body_encoding(media, media_name, ty, &object)?;
        Some(RequestBody {
            media,
            content_type: media_essence(media_name).to_owned(),
            ty,
            required: body.required,
            encoding,
        })
    }

    /// Resolve the Encoding Objects of a form or multipart request body into a fully-populated
    /// [`BodyEncoding`] — one entry per body property, so the emitted code never has to infer a
    /// default at runtime.
    ///
    /// Returns `None` only when the body is unrepresentable; an encoding that simply has no effect
    /// here is reported as `W011` and dropped.
    fn lower_body_encoding(
        &mut self,
        media: MediaType,
        media_name: &str,
        ty: Option<Ty>,
        object: &MediaTypeObject,
    ) -> Option<BodyEncoding> {
        // Encoding diagnostics point at the Media Type Object that declares them, not at the whole
        // request body.
        let at = &object.provenance;
        // Media that is neither form nor multipart is dispositioned once, in
        // `resolve_media_object`, which every Media Type Object passes through.
        if !matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
            return Some(BodyEncoding::default());
        }
        // `prefixEncoding`/`itemEncoding` describe positional parts of an array-shaped body, and
        // the specification scopes both to `multipart`. On multipart spargen generates from an
        // object schema, so there are no positions to encode and the declaration is rejected; on
        // form-urlencoded the specification itself says they do not apply, so they are inert.
        let positional = object
            .prefix_encoding
            .first()
            .map(|(_, at)| ("prefixEncoding", at.clone()))
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = positional {
            if media == MediaType::FormUrlEncoded {
                // W011 case: positional-encoding-form
                Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                    .message(format!(
                        "`{field}` has no effect on `{media_name}`: the specification scopes it to \
                         `multipart` content"
                    ))
                    .emit(self.diags);
            } else {
                Diagnostic::error(Code::UnsupportedMediaType, at)
                    .message(format!(
                        "`{field}` describes positional parts of an array-shaped multipart body; \
                         spargen generates `multipart/form-data` from an object schema, which has \
                         no positions"
                    ))
                    .remedy(
                        "use `encoding` keyed by property name, or omit this API segment with \
                         spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        let Some(TypeKind::Struct(structure)) =
            ty.and_then(|ty| self.graph.get(ty.id)).map(|def| &def.kind)
        else {
            // The body already failed its own shape gate above; don't pile on.
            return Some(BodyEncoding::default());
        };
        let fields: Vec<(String, Ty)> = structure
            .fields
            .iter()
            .map(|field| (field.name.wire.clone(), field.ty))
            .collect();
        // Nested encoding describes nested multipart parts (`multipart/mixed` inside a part).
        // Spargen generates one flat level, so a nested field is rejected rather than dropped.
        for (name, encoding) in &object.encoding {
            if let Some((field, at)) = encoding.nested.first() {
                Diagnostic::error(Code::UnsupportedMediaType, at.clone())
                    .message(format!(
                        "`encoding.{name}.{field}` describes a nested multipart part, which \
                         spargen does not generate"
                    ))
                    .remedy("flatten the body, or omit this API segment with spargen::omit!")
                    .emit(self.diags);
                return None;
            }
        }
        // An `encoding` key naming no property has nothing to apply to.
        for name in object.encoding.keys() {
            if !fields.iter().any(|(wire, _)| wire == name) {
                // W011 case: encoding-unknown-property
                Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
                    .message(format!(
                        "`encoding` entry `{name}` names no property of the body schema, so it is \
                         ignored"
                    ))
                    .emit(self.diags);
            }
        }
        let mut properties = Vec::with_capacity(fields.len());
        for (name, field_ty) in fields {
            let declared = object.encoding.get(&name);
            let mode = self.encoding_mode(declared, field_ty, media, &name, at)?;
            let headers = self.encoding_headers(declared, media, &name, at);
            properties.push(PropertyEncoding {
                name,
                mode,
                headers,
            });
        }
        Some(BodyEncoding { properties })
    }

    /// Apply the Encoding Object's mode switch for one property.
    ///
    /// In media mode a part's bytes come from the property's lowered type and its declared
    /// `contentType` rides on it as the header. A declaration is refused (`E009`) where the two
    /// cannot agree: a property rendered as JSON (anything but a scalar or bytes) whose declared
    /// type is not JSON, on multipart (the part's header) and on form-urlencoded (the field's
    /// serialization syntax) alike.
    fn encoding_mode(
        &mut self,
        declared: Option<&EncodingObject>,
        field_ty: Ty,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Option<EncodingMode> {
        // Presence of any RFC 6570 field selects query-style serialization outright, and makes
        // `contentType` inert — the specification is explicit that it is then ignored.
        if let Some(encoding) = declared {
            if encoding.style.is_some()
                || encoding.explode.is_some()
                || encoding.allow_reserved.is_some()
            {
                let style_name = encoding.style.as_deref().unwrap_or("form");
                let style = match style_name {
                    "form" => ParamStyle::Form,
                    "spaceDelimited" => ParamStyle::Delimited(Delimiter::Space),
                    "pipeDelimited" => ParamStyle::Delimited(Delimiter::Pipe),
                    "deepObject" => ParamStyle::DeepObject,
                    // The document schema enumerates these four, so this is unreachable for a
                    // validated document.
                    _ => {
                        Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                            .message(format!(
                                "`encoding.{name}.style: {style_name}` is not a form style"
                            ))
                            .emit(self.diags);
                        return None;
                    }
                };
                let explode = encoding
                    .explode
                    .unwrap_or(matches!(style, ParamStyle::Form));
                // The specification's own serialization table marks the delimited styles with
                // `explode: true` as *n/a* — undefined. The identical parameter-side construct is
                // already `E010`; without this an Encoding Object could declare it and have the
                // `explode` silently ignored.
                if explode && matches!(style, ParamStyle::Delimited(_)) {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.style: {style_name}` with `explode: true` is \
                             undefined: the specification's serialization table gives no value for \
                             that combination"
                        ))
                        .remedy("set `explode: false`, or use `style: form`")
                        .emit(self.diags);
                    return None;
                }
                // `deepObject` builds `name[key]=value` query fragments. A multipart part carries
                // its name in `Content-Disposition` and its value alone, so there is nowhere for
                // that syntax to go and no defined representation to fall back on.
                if media == MediaType::Multipart && style == ParamStyle::DeepObject {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.style: deepObject` is defined only for `in: query`; \
                             it has no `multipart/form-data` part representation"
                        ))
                        .remedy(
                            "use `style: form`, give the property a `contentType` such as \
                             `application/json`, or omit this API segment with spargen::omit!",
                        )
                        .emit(self.diags);
                    return None;
                }
                // An object property under RFC 6570 serialization: the specification says the
                // Encoding Object applies to the *entire value* for a non-array property, but
                // defines no part representation for an object, so there is nothing to generate.
                if media == MediaType::Multipart
                    && matches!(
                        self.graph.get(field_ty.id).map(|def| &def.kind),
                        Some(TypeKind::Struct(_))
                    )
                {
                    Diagnostic::error(Code::UnsupportedMediaType, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}` selects RFC 6570 serialization for an object \
                             property, which has no defined `multipart/form-data` part \
                             representation"
                        ))
                        .remedy(
                            "give the property a `contentType` such as `application/json` instead \
                             of `style`/`explode`/`allowReserved`, or omit this API segment with \
                             spargen::omit!",
                        )
                        .emit(self.diags);
                    return None;
                }
                // Multipart part values are never percent-encoded, so `allowReserved` is inert.
                let allow_reserved = encoding.allow_reserved.unwrap_or(false);
                if allow_reserved && media == MediaType::Multipart {
                    // W011 case: allow-reserved-multipart
                    Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                        .message(
                            "`allowReserved` has no effect on `multipart/form-data`: part values \
                             are not percent-encoded",
                        )
                        .emit(self.diags);
                }
                return Some(EncodingMode::Style {
                    style,
                    explode,
                    allow_reserved: allow_reserved && media != MediaType::Multipart,
                });
            }
        }
        let explicit = declared.and_then(|encoding| encoding.content_type.as_deref());
        let content_type = match explicit {
            // `contentType` is a comma-separated list of acceptable types, but a client sends
            // exactly one, so the first element wins. A comma inside a quoted parameter value is
            // part of that element, not a list separator.
            Some(list) => {
                let first = first_list_element(list).to_owned();
                // Only the essence can be a range; a `*` in a parameter value is an ordinary
                // `tchar`.
                if media_essence(&first).contains('*') {
                    Diagnostic::error(
                        Code::UnsupportedMediaType,
                        declared
                            .map(|encoding| encoding.provenance.clone())
                            .unwrap_or_else(|| at.clone()),
                    )
                    .message(format!(
                        "`encoding.{name}.contentType: {first}` is a wildcard; a client must send \
                         one concrete media type"
                    ))
                    .remedy("name a concrete media type such as `image/png`")
                    .emit(self.diags);
                    return None;
                }
                // The value is sent verbatim as the part's `Content-Type`, so it is held to the
                // rule a `content` key is: a string that is not a media type at all would
                // otherwise fall through to the natural codec below with nothing reported, and
                // fail only when a request is built.
                if !media_type_is_well_formed(media_essence(&first)) {
                    Diagnostic::error(
                        Code::UnsupportedMediaType,
                        declared
                            .map(|encoding| encoding.provenance.clone())
                            .unwrap_or_else(|| at.clone()),
                    )
                    .message(format!(
                        "`encoding.{name}.contentType: {first}` is not a media type"
                    ))
                    .remedy("name a media type such as `text/plain`, as `type/subtype`")
                    .emit(self.diags);
                    return None;
                }
                // Its parameters are sent too, so they are held to RFC 9110 § 5.6.6: a
                // parameter without `=` (`text/plain; foo`) would otherwise generate with nothing
                // reported and fail only when the part's `Content-Type` is parsed at request time.
                // Only a multipart part sends it (`mime_str`); a form-urlencoded field's
                // `contentType` only picks the codec, so a value the transport could not carry is
                // no reason to refuse it there. A malformed list is refused under either, as the
                // essence rule above is: it is not a media type.
                match media_type_with_parameters(&first) {
                    Ok(canonical) => canonical,
                    Err(ParameterFault::Unsendable(canonical)) if media != MediaType::Multipart => {
                        canonical
                    }
                    Err(fault) => {
                        let (message, remedy) = match fault {
                            ParameterFault::Malformed => (
                                format!(
                                    "`encoding.{name}.contentType: {first}` has a parameter \
                                     that is not `name=value` under RFC 9110 § 5.6.6"
                                ),
                                "write each parameter as `name=value`, with a token name and a \
                                 token or quoted-string value, such as `text/plain; charset=utf-8`",
                            ),
                            ParameterFault::Unsendable(_) => (
                                format!(
                                    "`encoding.{name}.contentType: {first}` has a quoted \
                                     parameter value the generated client cannot send: an empty \
                                     value, or one holding a `\"` or a tab"
                                ),
                                "quote a non-empty value without `\"` or a tab, or drop the \
                                 parameter",
                            ),
                        };
                        Diagnostic::error(
                            Code::UnsupportedMediaType,
                            declared
                                .map(|encoding| encoding.provenance.clone())
                                .unwrap_or_else(|| at.clone()),
                        )
                        .message(message)
                        .remedy(remedy)
                        .emit(self.diags);
                        return None;
                    }
                }
            }
            None => self.default_content_type(field_ty),
        };
        // The declared `contentType` is a wire *header*; how the value is rendered into bytes is
        // decided by the property's own lowered type. That is what lets a part declare
        // `application/sdp` (which spargen has no codec for) over a string property and still be
        // sent correctly, with the declared header attached. Media types are case-insensitive
        // (RFC 9110 § 8.3.1) and the classifier's arms are spelled in lowercase, so the essence is
        // classified lowercased: `Application/JSON` selects the codec `application/json` does,
        // here and in the refusal below.
        let classified = classify_media(&media_essence(&content_type).to_ascii_lowercase())
            .map(|(codec, _)| codec);
        let codec = match classified {
            Some(codec @ (MediaType::Json | MediaType::Text | MediaType::OctetStream)) => codec,
            _ => self.natural_codec(field_ty),
        };
        // That header-rides rule holds only where the header and the bytes agree. A scalar part
        // is the text the document described and a bytes part is whatever the caller supplies, so
        // either carries any well-formed declaration. Every other property is rendered as JSON
        // whatever it declares, so a declaration that is not JSON (`application/xml` over an
        // object, `text/csv` over an array, or a type with no codec at all) would put JSON under
        // a header naming another syntax — bytes no reader of the document predicts. Refused.
        // `classified` is the lowercased classification, so `Application/JSON` is judged as the
        // JSON it is.
        //
        // A form-urlencoded field has no header, so the argument there is not the same one, but it
        // reaches the same rule: its `contentType` is the only statement of the syntax the field's
        // value is serialized in before percent-encoding (the specification's form examples), so
        // it is what a server decodes the field by. Spargen serializes a non-scalar field only as
        // JSON (`FormMode::Json`); XML or any other syntax has no field codec, and `text/plain`
        // over an object or array has no defined rendering (the runtime's `FormMode::Text` refuses
        // a nested value, failing every call). Either way the declaration cannot be honoured, so
        // it is refused rather than sent as JSON or generated to fail.
        if matches!(media, MediaType::Multipart | MediaType::FormUrlEncoded)
            && explicit.is_some()
            && self.natural_codec(field_ty) == MediaType::Json
            && classified != Some(MediaType::Json)
        {
            Diagnostic::error(
                Code::UnsupportedMediaType,
                declared
                    .map(|encoding| encoding.provenance.clone())
                    .unwrap_or_else(|| at.clone()),
            )
            .message(if media == MediaType::Multipart {
                format!(
                    "property `{name}` declares `contentType: {content_type}`, but it is not a \
                     scalar or binary value, so spargen can send it only as JSON; the part's bytes \
                     would contradict its header"
                )
            } else {
                format!(
                    "property `{name}` declares `contentType: {content_type}`, but it is not a \
                     scalar value, so spargen can serialize it into a form field only as JSON; the \
                     field would not be in the syntax the document declares"
                )
            })
            .remedy(if media == MediaType::Multipart {
                "declare `application/json` (or a `+json` type), make the property a string or \
                 binary value, or omit this API segment with spargen::omit!"
            } else {
                "declare `application/json` (or a `+json` type), select RFC 6570 serialization \
                 with `style`/`explode`, make the property a scalar, or omit this API segment with \
                 spargen::omit!"
            })
            .emit(self.diags);
            return None;
        }
        // A form field is a single URL-encoded string; raw bytes have no representation there.
        if media == MediaType::FormUrlEncoded && codec == MediaType::OctetStream {
            Diagnostic::error(
                Code::UnsupportedMediaType,
                declared
                    .map(|encoding| encoding.provenance.clone())
                    .unwrap_or_else(|| at.clone()),
            )
            // Name what the document wrote: a declared `contentType` is the reason a string
            // property is binary here, while a property whose own schema is binary defaulted to
            // octet-stream and never mentioned a `contentType` at all.
            .message(if explicit.is_some() {
                format!(
                    "property `{name}` declares `contentType: {content_type}`, which is binary; a \
                     form-urlencoded body cannot carry a binary part"
                )
            } else {
                format!(
                    "property `{name}` is binary, which has no \
                     `application/x-www-form-urlencoded` representation"
                )
            })
            .remedy("send the body as `multipart/form-data`, or encode the value as text")
            .emit(self.diags);
            return None;
        }
        Some(EncodingMode::Media {
            content_type,
            codec,
        })
    }

    /// How a property's value is rendered into bytes, from its lowered type alone.
    fn natural_codec(&self, ty: Ty) -> MediaType {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => MediaType::OctetStream,
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_)) => MediaType::Text,
            // Encodings are lowered per operation, after every component the body reaches is
            // filled, so this is not expected. Were it reached, JSON is the codec that renders any
            // value faithfully, and it agrees with `default_content_type`'s answer for the same
            // placeholder, so the part's header and its bytes cannot disagree.
            Some(TypeKind::Reserved) => MediaType::Json,
            _ => MediaType::Json,
        }
    }

    /// The Encoding Object's default `contentType` for a property, from its lowered type.
    fn default_content_type(&self, ty: Ty) -> String {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Bytes) => "application/octet-stream".to_owned(),
            Some(TypeKind::Struct(_)) | Some(TypeKind::Union(_)) | Some(TypeKind::Any) => {
                "application/json".to_owned()
            }
            // In 3.1 an array's default follows its item type; 3.2 simplified this to JSON. Both
            // agree that an array of objects is JSON, and spargen sends any array as JSON, which
            // is the 3.2 rule and the only self-consistent reading for a nested array.
            Some(TypeKind::Array(_)) | Some(TypeKind::Tuple(_)) => "application/json".to_owned(),
            Some(TypeKind::Primitive(_)) | Some(TypeKind::Enum(_)) => "text/plain".to_owned(),
            // Not expected, for the reason `natural_codec` states. Were it reached, JSON is what
            // `natural_codec` renders a placeholder as, so it is the header that tells the truth
            // about those bytes; the octet-stream fallback below would not.
            Some(TypeKind::Reserved) => "application/json".to_owned(),
            _ => "application/octet-stream".to_owned(),
        }
    }

    /// The literal extra part headers of one multipart property.
    ///
    /// A Header Object *describes* a header; it carries no value. Only a schema that pins one —
    /// through `const`, or `default` in its absence — gives a client something to send.
    fn encoding_headers(
        &mut self,
        declared: Option<&EncodingObject>,
        media: MediaType,
        name: &str,
        at: &Provenance,
    ) -> Vec<(String, String)> {
        let Some(encoding) = declared else {
            return Vec::new();
        };
        if encoding.headers.is_empty() {
            return Vec::new();
        }
        if media != MediaType::Multipart {
            // W011 case: encoding-headers-non-multipart
            Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                .message(format!(
                    "`encoding.{name}.headers` applies only to `multipart` content"
                ))
                .emit(self.diags);
            return Vec::new();
        }
        let _ = at;
        let mut headers = Vec::new();
        for (header_name, header) in &encoding.headers {
            // `Content-Type` is described by `contentType`, not here.
            if header_name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            // A `$ref` here is resolved rather than treated as pinning nothing: the target may
            // well declare the `const` that gives the client something to send, and reporting
            // "pins no value" without looking would name the wrong reason. An unresolvable
            // reference is `E004` from `resolve_header`, not a warning.
            let literal = match header {
                RefOr::Item(header) => header
                    .schema
                    .as_ref()
                    .and_then(|schema| self.literal_header_value(schema)),
                RefOr::Ref(_) => match self.resolve_header(header) {
                    Some(resolved) => resolved
                        .schema
                        .as_ref()
                        .and_then(|schema| self.literal_header_value(schema)),
                    // `resolve_header` already reported the unresolvable reference; adding
                    // "pins no value" would name a second, wrong reason for one defect.
                    None => continue,
                },
            };
            match literal {
                Some(value) => headers.push((header_name.clone(), value)),
                None => {
                    // W011 case: encoding-header-no-value
                    Diagnostic::warning(Code::DeclarationHasNoEffect, encoding.provenance.clone())
                        .message(format!(
                            "`encoding.{name}.headers.{header_name}` pins no value, so there is \
                             nothing for the client to send"
                        ))
                        .remedy("give the header schema a `const` (or a `default`) value")
                        .emit(self.diags);
                }
            }
        }
        headers
    }

    /// The literal value a header schema pins, if any.
    fn literal_header_value(&self, schema: &RefOr<Schema>) -> Option<String> {
        let RefOr::Item(schema) = schema else {
            return None;
        };
        let value = schema.const_value.as_ref().or(schema.default.as_ref())?;
        match &value.node {
            crate::source::Node::String(text) => Some(text.clone()),
            crate::source::Node::Bool(value) => Some(value.to_string()),
            crate::source::Node::Number(number) => Some(match number {
                crate::source::Number::Int(value) => value.to_string(),
                crate::source::Number::UInt(value) => value.to_string(),
                crate::source::Number::Float(value) => value.to_string(),
            }),
            _ => None,
        }
    }

    fn lower_responses(&mut self, responses: &super::ResponsesObject) -> Responses {
        let mut by_status = Vec::new();
        for (status, response) in &responses.by_status {
            if let Some(status) = parse_status(status) {
                if let Some(response) = self
                    .resolve_response(response)
                    .and_then(|r| self.lower_response(&r))
                {
                    by_status.push((status, response));
                }
            }
        }
        let default = responses
            .default
            .as_ref()
            .and_then(|response| self.resolve_response(response))
            .and_then(|response| self.lower_response(&response));
        Responses { by_status, default }
    }

    fn lower_response(&mut self, response: &ResponseObject) -> Option<Response> {
        let body = choose_media(
            &response.content,
            &response.provenance,
            self.diags,
            BodyPosition::Response,
            media_object_is_opaque,
        )
        .and_then(
            |ChosenMedia {
                 media: media_name,
                 value: object,
                 narrowing,
             }| {
                let lowered = self.response_narrowing(|ctx| {
                    ctx.lower_chosen_response_body(response, media_name, object)
                })?;
                // `W014` is emitted only once the gates in `lower_chosen_response_body` have
                // accepted the selection; a refused one is reported by its `E009` alone.
                if let Some(narrowing) = narrowing {
                    self.diags.emit(narrowing);
                }
                Some(lowered)
            },
        );
        // A streaming response media (`text/event-stream` / `application/x-ndjson`) records its
        // framing; the body is then the streamed item type `T`. A whole-body response has no
        // framing. Framing is recorded in every response position; streaming only takes effect when
        // this is the operation's single success body (see `Responses::stream_success`), and a
        // bodied stream anywhere else rejects the operation (`Responses::stream_outside_single_success`).
        let headers = self.lower_response_headers(response);
        Some(Response {
            media: body.map(|(media, _, _)| media),
            body: body.and_then(|(_, ty, _)| ty),
            stream: body.and_then(|(_, _, stream)| stream),
            headers,
        })
    }

    /// Lower the response body entry [`Self::lower_response`] selected into its media, type, and
    /// framing, or `None` when one of the response-body gates (each an `E009`) refuses it.
    fn lower_chosen_response_body(
        &mut self,
        response: &ResponseObject,
        media_name: &str,
        object: &MediaTypeObject,
    ) -> Option<(MediaType, Option<Ty>, Option<crate::ir::Framing>)> {
        let object = self.resolve_media_object(object, media_name)?;
        let media = lower_media_type(media_name, &response.provenance, self.diags)?;
        // For a sequential/streaming media (`text/event-stream` / `application/x-ndjson`),
        // OpenAPI 3.2 gives the PER-ITEM type in `itemSchema`; a whole-body `schema` does not
        // apply to a stream, so `itemSchema` is preferred (falling back to `schema` for the
        // pre-3.2 form where the item type was written as `schema`). On a non-streaming media
        // `itemSchema` is meaningless: acknowledge it with `W010` and use `schema`.
        let (ty, stream) = if let Some(framing) = media.stream_framing() {
            if let Some(item_schema) = object.item_schema.as_ref() {
                if media == MediaType::EventStream && self.document.is_oas32 {
                    if let Some(json) =
                        super::sse::json_data_schema(item_schema, self.resolver, self.diags)
                    {
                        let ty = self.lower_schema_or(&json.schema, "ResponseBody");
                        self.warn_structural_default_or(
                            &json.schema,
                            "an SSE JSON data content schema",
                        );
                        (ty, Some(crate::ir::Framing::SseJsonData))
                    } else {
                        (
                            self.lower_schema_ref(item_schema, "ResponseBody"),
                            Some(crate::ir::Framing::SseEvent),
                        )
                    }
                } else {
                    (
                        self.lower_schema_ref(item_schema, "ResponseBody"),
                        Some(framing),
                    )
                }
            } else if self.document.is_oas32 && object.schema.is_some() {
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(
                                "in OpenAPI 3.2, `schema` on sequential media describes the \
                                 complete sequence; use `itemSchema` for a streaming client result",
                            )
                            .remedy("replace `schema` with `itemSchema`, or choose a non-sequential response media type")
                            .emit(self.diags);
                return None;
            } else {
                (
                    object
                        .schema
                        .as_ref()
                        .and_then(|schema| self.lower_schema_ref(schema, "ResponseBody")),
                    Some(framing),
                )
            }
        } else {
            if object.item_schema.is_some() {
                Diagnostic::warning(Code::Oas32ConstructIgnored, response.provenance.clone())
                    .message(
                        "`itemSchema` (OpenAPI 3.2) applies only to sequential/streaming media; \
                             on this non-streaming media it is not used",
                    )
                    .emit(self.diags);
            }
            (
                object
                    .schema
                    .as_ref()
                    .and_then(|schema| self.lower_schema_ref(schema, "ResponseBody")),
                None,
            )
        };
        if let Some(schema) = object.item_schema.as_ref().filter(|_| stream.is_some()) {
            self.warn_structural_default_ref(schema, "a response body schema");
        } else if let Some(schema) = object.schema.as_ref() {
            self.warn_structural_default_ref(schema, "a response body schema");
        }
        if matches!(media, MediaType::FormUrlEncoded | MediaType::Multipart) {
            Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                        .message(format!(
                            "media type `{media_name}` is supported for request bodies, not response bodies"
                        ))
                        .remedy("document a JSON, XML, textual, binary, or streaming response, or omit this API segment with spargen::omit!")
                        .emit(self.diags);
            return None;
        }
        let ty = if media == MediaType::OctetStream {
            self.opaque_octets(
                "ResponseBody",
                ty,
                object.schema.is_some(),
                &response.provenance,
            )
        } else {
            ty
        };
        if let Some(ty) = ty {
            let compatible = match media {
                MediaType::Text => raw_text_type_supported(&self.graph, ty),
                MediaType::OctetStream => matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Bytes)
                ),
                _ => true,
            };
            if !compatible {
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                            .message(format!(
                                "media type `{media_name}` requires a string-like or binary response schema"
                            ))
                            .remedy("use a string/binary schema, choose a structured media type, or omit this API segment with spargen::omit!")
                            .emit(self.diags);
                return None;
            }
            // A `bytes::Bytes` response is decoded as the raw octets of the body under any
            // media, so `null` is never what arrives, and the byte decoder has no `Option`
            // to build. The raw *text* codec decodes through serde and builds
            // `Option<String>` soundly, so it is not refused here, and neither is a
            // streamed item, which is framed and decoded element by element.
            if stream.is_none() && ty.nullable && self.is_bytes(ty) {
                Diagnostic::error(Code::UnsupportedMediaType, response.provenance.clone())
                    .message(format!(
                        "this `{media_name}` response body is read as raw bytes, whose \
                                 content has no wire representation of `null`, but its schema \
                                 admits `null`"
                    ))
                    .remedy(
                        "remove `null` from the response body schema, or omit this API \
                                 segment with spargen::omit!",
                    )
                    .emit(self.diags);
                return None;
            }
        }
        Some((media, ty, stream))
    }

    /// Lower a response's documented headers into typed accessors.
    ///
    /// A header that cannot be represented is skipped with a diagnostic rather than failing the
    /// whole operation: the body is what the call returns, and refusing an otherwise-generatable
    /// operation over an unreadable header would be a poor trade.
    fn lower_response_headers(&mut self, response: &ResponseObject) -> Vec<ResponseHeader> {
        let mut headers = Vec::new();
        for (name, header) in &response.headers {
            // The specification says a documented `Content-Type` header SHALL be ignored: the
            // media type is already the operation's, and a second source would only disagree.
            if name.eq_ignore_ascii_case("content-type") {
                // W011 case: response-content-type
                Diagnostic::warning(Code::DeclarationHasNoEffect, response.provenance.clone())
                    .message(
                        "a documented `Content-Type` response header is ignored; the operation's \
                         media type already determines it",
                    )
                    .emit(self.diags);
                continue;
            }
            let Some(header) = self.resolve_header(header) else {
                continue;
            };
            let header = &header;
            // A Header Object may only use `simple`; the document schema already enforces that.
            let (ty, shape) = if let Some(schema) = &header.schema {
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                let Some(shape) = self.header_shape(ty) else {
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` has a shape `simple` serialization cannot \
                             express, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                };
                (ty, shape)
            } else if let Some((media, object)) = header.content.iter().next() {
                // Resolve first: a header's content may itself be a Reference Object, and reading
                // `schema` off the unresolved shell would find `None` and drop the typed accessor
                // with nothing said.
                let Some(object) = self.resolve_media_object(object, media) else {
                    continue;
                };
                let Some(media) = lower_media_type(media, &header.provenance, self.diags) else {
                    continue;
                };
                // A textual `content` entry describes the field value itself, so it decodes exactly
                // like the `schema:` spelling — the shape gate below is what decides. This is not a
                // rare form: `Content-Range` on a ranged response is routinely documented this way,
                // and refusing it cost a typed accessor for no reason.
                if !matches!(media, MediaType::Json | MediaType::Text) {
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` uses a `content` media type spargen cannot \
                             decode, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                    continue;
                }
                if object.item_schema.is_some() {
                    Diagnostic::warning(Code::Oas32ConstructIgnored, header.provenance.clone())
                        .message(format!(
                            "`itemSchema` has no effect on response header `{name}`: a header \
                             field value is not a sequential media"
                        ))
                        .emit(self.diags);
                }
                let Some(schema) = object.schema.as_ref() else {
                    // W011 case: response-header-untyped
                    Diagnostic::warning(Code::DeclarationHasNoEffect, header.provenance.clone())
                        .message(format!(
                            "response header `{name}` declares `content` without a schema, so no \
                             typed accessor is generated"
                        ))
                        .remedy("give the content entry a `schema`")
                        .emit(self.diags);
                    continue;
                };
                let Some(ty) = self.lower_schema_ref(schema, &format!("Header{name}")) else {
                    continue;
                };
                if media == MediaType::Json {
                    (ty, crate::ir::HeaderShape::Json)
                } else {
                    // Textual content carries the field value verbatim, so only a scalar schema is
                    // representable: a list or an object under `text/plain` says nothing about how
                    // the value is framed, and `simple` is not that framing.
                    let Some(shape @ crate::ir::HeaderShape::Scalar) = self.header_shape(ty) else {
                        // W011 case: response-header-untyped
                        Diagnostic::warning(
                            Code::DeclarationHasNoEffect,
                            header.provenance.clone(),
                        )
                        .message(format!(
                            "response header `{name}` declares a textual `content` schema that \
                                 is not a single value, so no typed accessor is generated"
                        ))
                        .emit(self.diags);
                        continue;
                    };
                    (ty, shape)
                }
            } else {
                continue;
            };
            // `Set-Cookie` is the one field RFC 9110 §5.3 exempts from the comma-joined field-list
            // rule, and 3.2 gives it a section of its own saying each value must be kept on its own
            // line. The declared schema therefore describes ONE cookie, and the accessor is a list
            // of them — a schema that is already a list is taken to be that list.
            let (ty, shape) = if name.eq_ignore_ascii_case("set-cookie") {
                let already_list = match self.graph.get(ty.id).map(|def| &def.kind) {
                    Some(TypeKind::Array(_)) => true,
                    // `header_shape` refused a reservation above, so none reaches here; were one
                    // to, it is not known to be a list, and the declared schema is one cookie.
                    Some(TypeKind::Reserved) => false,
                    _ => false,
                };
                let list = if already_list {
                    ty
                } else {
                    self.insert_type(
                        &format!("Header{name}"),
                        TypeKind::Array(Box::new(ty)),
                        Docs::default(),
                        Some(header.provenance.clone()),
                    )
                };
                (list, crate::ir::HeaderShape::SetCookie)
            } else {
                (ty, shape)
            };
            headers.push(ResponseHeader {
                name: name.clone(),
                ty,
                required: header.required,
                explode: header.explode.unwrap_or(false),
                shape,
                deprecated: header.deprecated,
                docs: Docs {
                    description: header.description.clone(),
                    ..Docs::default()
                },
            });
        }
        headers
    }

    /// Resolve a Header Object that may be a `$ref` into `#/components/headers/`.
    fn resolve_header(
        &mut self,
        header: &RefOr<super::HeaderObject>,
    ) -> Option<super::HeaderObject> {
        let mut current = header.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(header) => return Some(header),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "header");
                    }
                    let alias = self
                        .root_component_name(&reference, "#/components/headers/")
                        .map(|name| self.document.components.headers.get(name).cloned());
                    match alias {
                        Some(Some(target)) => current = target,
                        Some(None) => {
                            return self.reject_component_alias(
                                &reference.provenance,
                                "header",
                                &reference.reference,
                            );
                        }
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle exactly as a Parameter or Response Object reference already
                        // does — and may itself be a Reference, followed from the file it is
                        // written in.
                        None => {
                            current = self.follow_bundle_reference(
                                &reference,
                                "header",
                                super::deserialize::parse_header_object,
                            )?;
                        }
                    }
                }
            }
        }
    }

    /// The `simple` wire shape of a lowered header type, or `None` when it has none.
    fn header_shape(&self, ty: Ty) -> Option<crate::ir::HeaderShape> {
        match self.graph.get(ty.id).map(|def| &def.kind) {
            Some(TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Null) => {
                Some(crate::ir::HeaderShape::Scalar)
            }
            Some(TypeKind::Array(_)) => Some(crate::ir::HeaderShape::Array),
            Some(TypeKind::Struct(_)) => Some(crate::ir::HeaderShape::Object),
            // Headers are lowered per operation, after every component they reach is filled, so
            // this is not expected. Were it reached, an unknown body has no provable `simple`
            // shape: `None` sends the header to the callers' `W011` (no accessor, warned) instead
            // of guessing one.
            Some(TypeKind::Reserved) => None,
            _ => None,
        }
    }

    fn resolve_parameter(&mut self, parameter: &RefOr<ParameterObject>) -> Option<ParameterObject> {
        let mut current = parameter.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(parameter) => return Some(parameter),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "parameter");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/parameters/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "parameter",
                            super::deserialize::parse_parameter,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.parameters.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "parameter",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
        }
    }

    fn resolve_request_body(
        &mut self,
        body: &RefOr<RequestBodyObject>,
    ) -> Option<RequestBodyObject> {
        let mut current = body.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(body) => return Some(body),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "request body");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/requestBodies/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "request body",
                            super::deserialize::parse_request_body,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.request_bodies.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "request body",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
        }
    }

    fn resolve_response(&mut self, response: &RefOr<ResponseObject>) -> Option<ResponseObject> {
        let mut current = response.clone();
        let mut seen = HashSet::new();
        loop {
            match current {
                RefOr::Item(response) => return Some(response),
                RefOr::Ref(reference) => {
                    self.note_reference_docs(&reference);
                    if !seen.insert(self.hop_identity(&reference)) {
                        return self.reject_alias_cycle(&reference.provenance, "response");
                    }
                    let Some(name) =
                        self.root_component_name(&reference, "#/components/responses/")
                    else {
                        // Not a root component alias: a multi-file description may reference a
                        // whole file, or a sub-file's own components, which resolve through the
                        // input bundle like a schema `$ref` — and may itself be a Reference,
                        // followed from the file it is written in.
                        current = self.follow_bundle_reference(
                            &reference,
                            "response",
                            super::deserialize::parse_response,
                        )?;
                        continue;
                    };
                    let Some(target) = self.document.components.responses.get(name) else {
                        return self.reject_component_alias(
                            &reference.provenance,
                            "response",
                            &reference.reference,
                        );
                    };
                    current = target.clone();
                }
            }
        }
    }

    /// One hop of a Parameter, Request Body, Response or Header chain through the input bundle:
    /// the target as written, which is either the object or the next Reference to follow. A miss
    /// is reported here, in the words of the way it failed.
    fn follow_bundle_reference<T>(
        &mut self,
        reference: &super::Reference,
        kind: &str,
        parse: fn(&SpannedValue, &crate::diag::JsonPointer, &mut Diagnostics) -> Option<T>,
    ) -> Option<RefOr<T>> {
        let from = reference
            .provenance
            .span
            .map_or_else(|| self.resolver.root_id(), |span| span.file);
        match self
            .resolver
            .resolve_component_or_ref(&reference.reference, from, parse, self.diags)
        {
            Ok(target) => Some(target),
            Err(miss) => self.reject_unfollowable_reference(
                &reference.provenance,
                kind,
                &reference.reference,
                miss,
            ),
        }
    }

    /// What a reference hop names, for a chain's cycle check: the `(file, pointer)` it resolves to
    /// wherever the bundle can place it, so one relative spelling written in two files is two
    /// targets and two spellings of one target are one; the reference as written otherwise.
    fn hop_identity(
        &self,
        reference: &super::Reference,
    ) -> Result<(crate::diag::FileId, crate::diag::JsonPointer), String> {
        self.resolver
            .reference_identity(&reference.reference, &reference.provenance)
            .ok_or_else(|| reference.reference.clone())
    }

    /// The `<name>` of a `#/components/<kind>/<name>` reference (`prefix` is
    /// `#/components/<kind>/`) that addresses the **root** document's component map — one written
    /// in the root document, or with no span to say otherwise.
    ///
    /// A JSON Pointer fragment addresses the document it appears in, so the same spelling written
    /// inside a referenced file names that file's components (#397). Reading the root's map for it
    /// rejected the reference when the root declared no such name and silently substituted the
    /// root's declaration when it did; `None` sends it to the bundle, which resolves it from the
    /// file it is written in.
    fn root_component_name<'r>(
        &self,
        reference: &'r super::Reference,
        prefix: &str,
    ) -> Option<&'r str> {
        let root = self.resolver.root_id();
        let written_in = reference.provenance.span.map_or(root, |span| span.file);
        if written_in != root {
            return None;
        }
        reference.reference.strip_prefix(prefix)
    }

    /// Acknowledge a Reference Object `summary`/`description`.
    ///
    /// These document the *reference site*, not the target. Spargen emits one shared item per
    /// component, so a per-site documentation override has nowhere to land without making two use
    /// sites of the same component disagree. Reported rather than dropped.
    fn note_reference_docs(&mut self, reference: &super::Reference) {
        if reference.summary.is_none() && reference.description.is_none() {
            return;
        }
        // W011 case: reference-docs
        Diagnostic::warning(Code::DeclarationHasNoEffect, reference.provenance.clone())
            .message(format!(
                "the `summary`/`description` on the reference to `{}` documents this use site, \
                 but the generated item is shared across every use, so the override is not applied",
                reference.reference
            ))
            .remedy("document the referenced component itself")
            .emit(self.diags);
    }

    /// A reference into `#/components/<kind>/` naming an entry the document does not declare.
    fn reject_component_alias<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
        reference: &str,
    ) -> Option<T> {
        reject_undeclared_component(self.diags, provenance, kind, reference);
        None
    }

    /// A chain of `{kind}` reference hops that returns to a reference it already followed.
    fn reject_alias_cycle<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
    ) -> Option<T> {
        // E004 case: cycle
        Diagnostic::error(Code::UnresolvedRef, provenance.clone())
            .message(format!("{kind} reference cycle cannot be resolved"))
            .emit(self.diags);
        None
    }

    /// A reference outside `#/components/<kind>/` that the input bundle could not follow, reported
    /// in the words of the way it failed: the resolver separates a reference it cannot place from
    /// a pointer with nothing at it, and a target that exists but does not parse has already been
    /// rejected by its parser, at the target, so it gets no second diagnostic here — the same
    /// disposition a malformed Path Item or schema target gets.
    fn reject_unfollowable_reference<T>(
        &mut self,
        provenance: &crate::diag::Provenance,
        kind: &str,
        reference: &str,
        miss: super::resolve::ComponentMiss,
    ) -> Option<T> {
        use super::resolve::ComponentMiss;
        match miss {
            // The bundle cannot tell a file it does not hold from a fragment form it declines to
            // walk, so the message says both, in the one wording `E004` reserves for that.
            ComponentMiss::Unclassifiable => {
                // E004 case: unsupported-or-unresolved
                Diagnostic::error(Code::UnresolvedRef, provenance.clone())
                    .message(format!(
                        "unsupported or unresolved {kind} reference `{reference}`"
                    ))
                    .emit(self.diags);
            }
            ComponentMiss::AbsentTarget => {
                // E004 case: absent-target
                let mut diagnostic = Diagnostic::error(Code::UnresolvedRef, provenance.clone())
                    .message(format!(
                        "{kind} reference target `{reference}` was not found in the input bundle"
                    ));
                // A bare fragment written in a referenced file addresses that file (#397); when
                // the root declares what it names, that is almost certainly what was meant.
                if let Some(remedy) = self.root_declares_the_fragment(provenance, reference) {
                    diagnostic = diagnostic.remedy(remedy);
                }
                diagnostic.emit(self.diags);
            }
            ComponentMiss::Unparsable => {}
        }
        None
    }

    /// The remedy for a bare `#…` fragment written in a referenced file whose own document holds
    /// nothing at it while the root document does: the fragment addresses the file it is written
    /// in, so the root's declaration is reached only by naming the root document.
    fn root_declares_the_fragment(
        &self,
        provenance: &crate::diag::Provenance,
        reference: &str,
    ) -> Option<String> {
        let root = self.resolver.root_id();
        let written_in = provenance.span.map(|span| span.file)?;
        if written_in == root || !reference.starts_with('#') {
            return None;
        }
        let (file, pointer) = self.resolver.reference_identity_from(reference, root)?;
        self.resolver.node_at(file, &pointer)?;
        Some(format!(
            "the root document declares `{reference}`, but a `#` fragment addresses the file it \
             is written in; name the root document before the fragment to reference the root's \
             declaration, or declare it in this file"
        ))
    }

    /// Resolve a Media Type Object through any `$ref` hops and give every position-independent
    /// field it can carry a disposition.
    ///
    /// This is the single seam every Media Type Object passes through — request bodies, responses,
    /// parameter content, whole-query-string content, and response header content alike — so a
    /// field that only *sometimes* has an effect is reported once, wherever it appears, instead of
    /// being dispositioned on the request-body path and silently dropped everywhere else.
    fn resolve_media_object(
        &mut self,
        object: &super::MediaTypeObject,
        media_name: &str,
    ) -> Option<super::MediaTypeObject> {
        let mut current = object.clone();
        let mut seen = HashSet::new();
        while let Some(reference) = current.reference.clone() {
            // A Reference Object's own `summary`/`description` documents this use site, which one
            // generated item shared across every use cannot express — the same disposition the
            // Parameter, Response, and Request Body paths already give it.
            self.note_reference_docs(&reference);
            // Keyed on the target each hop resolves to, as the Parameter, Response, Request Body
            // and Header chains are: `#/components/mediaTypes/A` written in two files is two hops.
            if !seen.insert(self.hop_identity(&reference)) {
                return self.reject_alias_cycle(&reference.provenance, "media type");
            }
            let Some(name) = self.root_component_name(&reference, "#/components/mediaTypes/")
            else {
                // Not a root component alias: a multi-file description may reference a whole
                // file, or a sub-file's own components, which resolve through the input bundle
                // exactly as a Parameter or Response Object reference already does.
                let from = reference
                    .provenance
                    .span
                    .map_or_else(|| self.resolver.root_id(), |span| span.file);
                let resolved = self.resolver.resolve_component(
                    &reference.reference,
                    from,
                    |value, pointer, diags| {
                        Some(super::deserialize::parse_media_type(value, pointer, diags))
                    },
                    self.diags,
                );
                match resolved {
                    Ok(resolved) => {
                        current = resolved;
                        continue;
                    }
                    Err(miss) => {
                        return self.reject_unfollowable_reference(
                            &reference.provenance,
                            "Media Type Object",
                            &reference.reference,
                            miss,
                        );
                    }
                }
            };
            let Some(target) = self.document.components.media_types.get(name) else {
                return self.reject_component_alias(
                    &reference.provenance,
                    "Media Type Object",
                    &reference.reference,
                );
            };
            current = target.clone();
        }
        // Encoding is scoped to form and multipart content. The specification says it is simply
        // ignored elsewhere, so rejecting would refuse valid documents — but ignoring it silently
        // would be the fourth behavior this generator does not have. Acknowledge it instead. The
        // form/multipart case carries on to `lower_body_encoding`, which knows the schema.
        if !matches!(
            media_essence(media_name),
            "multipart/form-data" | "application/x-www-form-urlencoded"
        ) {
            self.note_inert_encoding(&current, media_name);
        }
        Some(current)
    }

    /// Report the encoding fields of a Media Type Object that cannot take effect in this position.
    fn note_inert_encoding(&mut self, object: &super::MediaTypeObject, media_name: &str) {
        let declared = object
            .encoding
            .first()
            .map(|(_, encoding)| ("encoding", encoding.provenance.clone()))
            .or_else(|| {
                object
                    .prefix_encoding
                    .first()
                    .map(|(_, at)| ("prefixEncoding", at.clone()))
            })
            .or_else(|| {
                object
                    .item_encoding
                    .as_ref()
                    .map(|(_, at)| ("itemEncoding", at.clone()))
            });
        if let Some((field, at)) = declared {
            // W011 case: encoding-on-other-media
            Diagnostic::warning(Code::DeclarationHasNoEffect, at)
                .message(format!(
                    "`{field}` has no effect on `{media_name}`: it applies only to `multipart` \
                     and `application/x-www-form-urlencoded` content"
                ))
                .emit(self.diags);
        }
    }

    /// Lower a possibly-`$ref` schema. Component refs go through [`Self::ensure_component`] so
    /// every use site shares one generated type instead of lowering a duplicate.
    fn lower_schema_ref(&mut self, schema: &RefOr<Schema>, hint: &str) -> Option<Ty> {
        match schema {
            RefOr::Item(schema) => self.lower_schema(schema, hint),
            RefOr::Ref(reference) => {
                if let Some(name) = reference.reference.strip_prefix("#/components/schemas/") {
                    self.ensure_component(name, Some(&reference.reference), &reference.provenance)
                } else if is_remote_ref(&reference.reference) {
                    self.ensure_remote(&reference.reference)
                } else {
                    self.ensure_resolved(&reference.reference, &reference.provenance, hint)
                }
            }
        }
    }

    /// Whether this definition is a named component root — reachable by name from anywhere else in
    /// the document, rather than owned by the single use site that produced it.
    fn is_component_root(&self, id: TypeId) -> bool {
        self.components
            .values()
            .chain(self.in_progress.values())
            .chain(self.remote_components.values())
            .chain(self.remote_in_progress.values())
            .chain(self.resolved_components.values())
            .chain(self.resolved_in_progress.values())
            .any(|&(root, _)| root == id)
    }

    /// Whether `id` is a reservation whose body is still being lowered, so its definition is the
    /// placeholder [`TypeGraph::reserve`] inserted rather than the schema's own shape.
    ///
    /// This matters because [`Self::push_ref_member`] classifies an `allOf` member by reading
    /// `graph.get(id).kind`. That kind is now [`TypeKind::Reserved`] — it was `TypeKind::Any` until
    /// the dedicated variant landed, which is why reading one answered "scalar" for a type that is
    /// not a scalar and the member silently became `serde_json::Value`. `push_ref_member` now names
    /// `Reserved` in its own `match` and refuses it, so the refusal lives in the function rather
    /// than in each caller. Every id in the three in-progress maps is such a placeholder, and the
    /// only safe thing to do with one is refuse to read it.
    ///
    /// This asks "is `id` **any** open reservation". A caller that needs "is `id` the reservation
    /// belonging to the schema at *this* provenance" wants [`Self::reservation_at`] instead; the two
    /// coincide only when the construct being lowered is the component's whole body, and confusing
    /// them rejects every recursive schema whose reference sits inside a property.
    fn is_in_progress_root(&self, id: TypeId) -> bool {
        self.in_progress
            .values()
            .chain(self.remote_in_progress.values())
            .chain(self.resolved_in_progress.values())
            .any(|&(root, _)| root == id)
    }

    /// Whether the graph currently holds `id` as a [`TypeKind::Reserved`] placeholder.
    ///
    /// A third question, narrower than [`Self::is_in_progress_root`] in one way and wider in
    /// another: it asks what the graph *holds* rather than which maps are open, so it answers for a
    /// reservation taken by any of the three in-progress maps without having to name them, and it
    /// answers `false` for an id whose body has since been filled. It exists so a caller can refuse
    /// to **clone** a placeholder's kind: a clone inserts a second reservation that nothing will
    /// ever `fill`, and `Api::check_invariants` reports that as `E011` against a document that is
    /// not malformed.
    fn is_reservation(&self, id: TypeId) -> bool {
        matches!(
            self.graph.get(id).map(|def| &def.kind),
            Some(TypeKind::Reserved)
        )
    }

    /// Whether a schema the bundle resolver just produced is the very schema whose body is being
    /// lowered. The inlining arm of [`Self::gather_member`] has no shared `Ty` to test against
    /// [`Self::is_in_progress_root`], so it tests the resolved target's identity instead.
    fn resolved_target_in_progress(&self, provenance: &Provenance) -> bool {
        self.reservation_at(provenance).is_some()
    }

    /// The reserved id of the schema *at* `provenance`, when that schema is one whose body is
    /// currently being lowered.
    ///
    /// This answers a strictly narrower question than [`Self::is_in_progress_root`], and the
    /// difference matters. `is_in_progress_root` answers "is this id **any** open reservation";
    /// this answers "is the schema written **here** the one that reservation belongs to". They
    /// coincide only when the construct being lowered *is* the component's whole body, which is why
    /// a guard that needs the second and asks the first over-rejects every case where a recursive
    /// reference is nested inside a property rather than being the component itself.
    fn reservation_at(&self, provenance: &Provenance) -> Option<TypeId> {
        if let Some(key) = resolved_identity(provenance) {
            if let Some(&(id, _)) = self.resolved_in_progress.get(&key) {
                return Some(id);
            }
            // A remote frame keys on the URL it was reached by rather than on its target's
            // `file#pointer`, so a string comparison against this provenance can never match one.
            // Canonicalise its keys to the same identity instead of reconstructing a URL from a
            // file id: a URL is only one of the spellings that reaches a vendored document, and
            // the map holds at most one entry per open recursion frame. Omitting this frame is what
            // let a remote body whose union collapses onto an open remote reservation past the
            // guard below, to be aborted by `TypeDefs::fill`'s `fill of an unreserved id` instead
            // of reported.
            //
            // That abort is a `debug_assert!`, so it is an enforcement point that degrades: it
            // holds under `cargo test` and not in a consumer's release `build.rs`. Both halves are
            // measured with this loop deleted. Debug assertions on: the process aborts at
            // `TypeDefs::fill`. Debug assertions off: it neither aborts nor emits — `fill` writes
            // the unreserved id, the reservation survives, and `check_invariants` rejects with
            // `E011`, "type `` is still a reservation, so its body was never lowered", naming no
            // type and carrying no pointer. So the release outcome is a poor diagnostic rather
            // than silent wrong output, and the second net is `check_invariants`, not `fill`.
            //
            // Only one shape reaches here: a vendored document whose **whole body** is the union,
            // because only then does the provenance canonicalise to a frame's own `file#pointer`.
            // `remote::a_vendored_remote_schema_that_is_a_union_over_itself_is_rejected` is that
            // document and the only thing in the suite that executes this loop; deleting the loop
            // turns it red. Every other remote recursion sits at a property or an `allOf` member,
            // whose pointer is not the frame's, so it reaches here and matches nothing.
            for (reference, &(id, _)) in &self.remote_in_progress {
                let matches = self
                    .resolver
                    .reference_identity(reference, &self.document.provenance)
                    .is_some_and(|(file, pointer)| key == format!("{}#{}", file.0, pointer));
                if matches {
                    return Some(id);
                }
            }
        }
        // A target inside the root document's component map has its identity there instead —
        // `ensure_resolved` routes such a reference back to `ensure_component` — so consult that
        // map too, or a root component addressed by file reference escapes the check.
        if !provenance
            .span
            .is_some_and(|span| span.file == self.resolver.root_id())
        {
            return None;
        }
        provenance
            .pointer
            .as_str()
            .strip_prefix("/components/schemas/")
            .and_then(|name| self.in_progress.get(name))
            .map(|&(id, _)| id)
    }

    /// Read an untyped body on a binary media type as raw octets.
    ///
    /// OpenAPI 3.1 aligned Schema Objects with JSON Schema 2020-12 and removed `format: binary`, so
    /// an empty (always-true) Schema Object — or no `schema` at all — is now how a document says
    /// *any octets*: the media type already carries the meaning, and `type: string` would be the
    /// 3.0 spelling the release deliberately retired. Both lower to `Any`, which on
    /// `application/octet-stream` would emit `serde_json::Value` for a byte stream, so the use site
    /// is retyped to `Bytes`.
    ///
    /// `declared_but_unlowerable` is a schema that was written and failed to lower for its own
    /// reason: it has already reported that, and must not be silently rewritten into bytes.
    ///
    /// `provenance` is the body's own, never the document root's: `Scope::alloc` disambiguates
    /// colliding name hints by pointer precisely so that reordering paths renames nothing, and a
    /// root pointer would collapse every `RequestBody`/`ResponseBody` here into arrival order.
    fn opaque_octets(
        &mut self,
        hint: &str,
        ty: Option<Ty>,
        declared: bool,
        provenance: &crate::diag::Provenance,
    ) -> Option<Ty> {
        let Some(ty) = ty else {
            let declared_but_unlowerable = declared;
            return (!declared_but_unlowerable).then(|| {
                self.insert_type(
                    hint,
                    TypeKind::Bytes,
                    Docs::default(),
                    Some(provenance.clone()),
                )
            });
        };
        if !matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Any)
        ) {
            return Some(ty);
        }
        // An inline `{}` is the definition just inserted, and nothing can reference it yet, so it
        // is replaced in place — left behind it would emit a second `pub type … =
        // serde_json::Value` alias and take the name this body wants.
        //
        // Being the last definition is not enough to prove that, though: a *childless* component
        // (`Opaque: {}`) is lifted into its reserved id, which is then the last id as well, and
        // rewriting that would retype the component for every other reference in the document. A
        // named root is therefore left exactly as declared and the use site gets its own type.
        if self.graph.last_id() == Some(ty.id) && !self.is_component_root(ty.id) {
            let (_, definition) = self
                .pop_last_type()
                .expect("a definition was just observed");
            let id = self.graph.insert(TypeDef {
                kind: TypeKind::Bytes,
                ..definition
            });
            debug_assert_eq!(id, ty.id, "popping and reinserting reuses the dense id");
            return Some(Ty { id, ..ty });
        }
        Some(self.insert_type(
            hint,
            TypeKind::Bytes,
            Docs::default(),
            Some(provenance.clone()),
        ))
    }

    /// Whether `ty`'s definition is raw `bytes::Bytes`, which the emitter sends and decodes
    /// verbatim whatever the media.
    fn is_bytes(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|definition| &definition.kind),
            Some(TypeKind::Bytes)
        )
    }

    fn insert_schema_type(&mut self, schema: &Schema, hint: &str, kind: TypeKind) -> Ty {
        self.insert_type(
            hint,
            kind,
            Docs {
                title: schema.title.clone(),
                description: schema.description.clone(),
                deprecated: schema.deprecated,
                ..Docs::default()
            },
            Some(schema.provenance.clone()),
        )
    }

    fn insert_type(
        &mut self,
        hint: &str,
        kind: TypeKind,
        docs: Docs,
        provenance: Option<crate::diag::Provenance>,
    ) -> Ty {
        let provenance = provenance.unwrap_or_else(|| self.document.provenance.clone());
        let document = provenance
            .span
            .map(|span| self.resolver.document_key(span.file))
            .unwrap_or_default();
        let id = self.graph.insert(TypeDef {
            name_hint: hint.to_owned(),
            kind,
            docs,
            provenance,
            document,
        });
        Ty {
            id,
            nullable: false,
            boxed: false,
        }
    }
}

/// The component name a union member is written as — `$ref: '#/components/schemas/<name>'` — or
/// `None`. It names the member's variant and implicit discriminator tag; an explicit
/// `discriminator.mapping` value is matched by resolved target instead
/// ([`LowerCtx::discriminator_members`]). Written in the root document, a name with a
/// raw `/` is a pointer *into* a component, not a component name: it has none to derive a variant
/// or a tag from, exactly as the same pointer written against a relative file has none, and neither
/// has any file reference.
///
/// Written in a sub-file, the same spelling keeps the variant name it has always had. That route
/// resolved through the resolver before same-file deep pointers did in the root, and its members
/// were named from the pointer text (`Envelope/properties/cat` → variant `EnvelopePropertiesCat`).
/// Dropping the name there would rename those variants in documents that generate today; the
/// root-only filter confines the change to what previously rejected. The name is no component
/// name, so it supplies no implicit discriminator tag ([`is_schema_component_name`]).
fn member_component_name(member: &SchemaOr, root: crate::diag::FileId) -> Option<&str> {
    let SchemaOr::Schema(schema) = member else {
        return None;
    };
    let in_sub_file = schema.provenance.span.is_some_and(|span| span.file != root);
    schema
        .reference
        .as_deref()?
        .strip_prefix("#/components/schemas/")
        .filter(|name| in_sub_file || !name.contains('/'))
}

/// The wire name of the first field of an object parameter whose value is unconstrained
/// ([`TypeKind::Any`]), which [`parameter_shape_supported`] refuses because an arbitrary JSON value
/// has no single serialized token. A `oneOf`/`anyOf` parameter schema serializes as whichever
/// member the value is, so the fields of each object member are searched too, through nested
/// unions, as [`uninhabited_parameter_part`] searches them (#435). `None` for a parameter that is
/// neither an object nor a union, or has no such field.
fn unconstrained_parameter_property(graph: &TypeGraph, ty: Ty) -> Option<String> {
    unconstrained_parameter_property_inner(graph, ty, &mut HashSet::new())
}

fn unconstrained_parameter_property_inner(
    graph: &TypeGraph,
    ty: Ty,
    members: &mut HashSet<TypeId>,
) -> Option<String> {
    match &graph.get(ty.id)?.kind {
        TypeKind::Struct(object) => object
            .fields
            .iter()
            .find(|field| matches!(graph.get(field.ty.id).map(|d| &d.kind), Some(TypeKind::Any)))
            .map(|field| field.name.wire.clone()),
        // `members` stops a union that reaches itself through a member from being walked again.
        TypeKind::Union(union) if members.insert(ty.id) => {
            let found = union.variants.iter().find_map(|variant| {
                unconstrained_parameter_property_inner(graph, variant.ty, members)
            });
            members.remove(&ty.id);
            found
        }
        // A reservation's shape is unknown, so it has no field to name; it is left to
        // `parameter_shape_supported`, which refuses it.
        TypeKind::Reserved => None,
        _ => None,
    }
}

/// How a parameter position fails to be inhabited, as [`uninhabited_parameter_part`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Uninhabited {
    /// The schema at the position admits no value: [`TypeKind::Never`], or a union every one of
    /// whose members admits none.
    Whole,
    /// The schema is a union that admits some value, but at least one of its members admits none
    /// (`oneOf: [{type: string}, false]`). The position itself is inhabited, so only the member
    /// may be called uninhabited.
    Member,
}

/// The path, relative to the parameter, of the first schema at a position simple/form/deepObject
/// serialization would otherwise accept that is uninhabited, or is a union holding an uninhabited
/// ([`TypeKind::Never`]) member, together with which of the two it is. [`parameter_shape_supported`]
/// refuses both, because no value of the uninhabited schema has a token. The path is `""` for the
/// parameter itself, `.name` for an object property, `.*` for its `additionalProperties`, `[]` for
/// an array's items, and `[i]` for a tuple's. A `oneOf`/`anyOf` parameter schema serializes as
/// whichever member the value is, so the parts of each member are searched too, with the same
/// paths (a member adds nothing to it): `f.a` is the property `a` of an object member of `f`
/// (#435). Only those positions are searched, so an uninhabited schema below a nested array or
/// object stays reported as the nesting. `None` when there is no such schema.
fn uninhabited_parameter_part(graph: &TypeGraph, ty: Ty) -> Option<(String, Uninhabited)> {
    uninhabited_parameter_part_inner(graph, ty, &mut HashSet::new())
}

fn uninhabited_parameter_part_inner(
    graph: &TypeGraph,
    ty: Ty,
    members: &mut HashSet<TypeId>,
) -> Option<(String, Uninhabited)> {
    fn classify(graph: &TypeGraph, ty: Ty, visiting: &mut HashSet<TypeId>) -> Option<Uninhabited> {
        if !visiting.insert(ty.id) {
            return None;
        }
        let found = match graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Never) => Some(Uninhabited::Whole),
            Some(TypeKind::Union(union)) => {
                let members: Vec<_> = union
                    .variants
                    .iter()
                    .map(|variant| classify(graph, variant.ty, visiting))
                    .collect();
                if !members.is_empty()
                    && members
                        .iter()
                        .all(|member| *member == Some(Uninhabited::Whole))
                {
                    Some(Uninhabited::Whole)
                } else if members.iter().any(Option::is_some) {
                    Some(Uninhabited::Member)
                } else {
                    None
                }
            }
            // A reservation's shape is unknown, so nothing proves it admits no value; it is left
            // to `parameter_shape_supported`, which refuses it.
            Some(TypeKind::Reserved) => None,
            _ => None,
        };
        visiting.remove(&ty.id);
        found
    }
    let mut visiting = HashSet::new();
    if let Some(kind) = classify(graph, ty, &mut visiting) {
        return Some((String::new(), kind));
    }
    let mut at = |path: String, ty: Ty| classify(graph, ty, &mut visiting).map(|kind| (path, kind));
    match &graph.get(ty.id)?.kind {
        TypeKind::Array(item) => at("[]".to_owned(), **item),
        TypeKind::Tuple(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, item)| at(format!("[{index}]"), *item)),
        TypeKind::Struct(object) => object
            .fields
            .iter()
            .find_map(|field| at(format!(".{}", field.name.wire), field.ty))
            .or_else(|| match &object.additional {
                AdditionalProps::Typed(value) => at(".*".to_owned(), **value),
                AdditionalProps::Deny | AdditionalProps::Allow => None,
            }),
        // `parameter_shape_supported` gives each member the position the union holds, so a
        // member's parts are parts of the parameter. `members` stops a union that reaches itself
        // through a member from being walked again.
        TypeKind::Union(union) if members.insert(ty.id) => {
            let found = union
                .variants
                .iter()
                .find_map(|variant| uninhabited_parameter_part_inner(graph, variant.ty, members));
            members.remove(&ty.id);
            found
        }
        // The parameter itself was classified above; a reservation has no parts to search.
        TypeKind::Reserved => None,
        _ => None,
    }
}

fn parameter_shape_supported(graph: &TypeGraph, ty: Ty) -> bool {
    parameter_shape_supported_inner(graph, ty, false, &mut HashSet::new())
}

fn parameter_shape_supported_inner(
    graph: &TypeGraph,
    ty: Ty,
    scalar_only: bool,
    visiting: &mut HashSet<TypeId>,
) -> bool {
    if !visiting.insert(ty.id) {
        return false;
    }
    let Some(definition) = graph.get(ty.id) else {
        visiting.remove(&ty.id);
        return false;
    };
    let supported = match &definition.kind {
        TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Bytes | TypeKind::Null => true,
        TypeKind::Array(item) if !scalar_only => {
            parameter_shape_supported_inner(graph, **item, true, visiting)
        }
        TypeKind::Tuple(items) if !scalar_only => items
            .iter()
            .all(|item| parameter_shape_supported_inner(graph, *item, true, visiting)),
        TypeKind::Struct(object) if !scalar_only => {
            object
                .fields
                .iter()
                .all(|field| parameter_shape_supported_inner(graph, field.ty, true, visiting))
                && match &object.additional {
                    AdditionalProps::Deny | AdditionalProps::Allow => true,
                    AdditionalProps::Typed(value) => {
                        parameter_shape_supported_inner(graph, **value, true, visiting)
                    }
                }
        }
        TypeKind::Union(union) => union.variants.iter().all(|variant| {
            parameter_shape_supported_inner(graph, variant.ty, scalar_only, visiting)
        }),
        // A reservation's shape is unknown, so it cannot be *proved* serialisable as a parameter.
        // This function answers "is this supported", and an unknown must answer no: saying yes
        // would let a recursive schema through as a parameter on the strength of nothing.
        //
        // Parameters are lowered only after every component, and each lazily resolved target
        // fills its reservation before returning, so no reservation is open here. One still
        // survives: a component whose lowering *failed* never fills its reservation, and a
        // component that closed a cycle through it before the failure is cached complete, holding
        // that dangling id. `A: {properties: {bs: {$ref: B}, x: {$ref: Missing}}}` with
        // `B: {type: array, items: {$ref: A}}` leaves `B`'s items `Reserved` for good, and a
        // parameter referencing `B` reaches this arm. The document is already rejected by the
        // failure (`E004` there); answering no adds `E010` for the parameter rather than accepting
        // a shape nobody knows. Pinned by
        // `a_parameter_reaching_a_failed_components_reservation_is_refused`.
        TypeKind::Reserved
        | TypeKind::Struct(_)
        | TypeKind::Array(_)
        | TypeKind::Tuple(_)
        | TypeKind::Never
        | TypeKind::Any => false,
    };
    visiting.remove(&ty.id);
    supported
}

/// Whether lowering the schema at `frame` descends into the schema at `site`, both pointers into
/// one file: `site` is `frame` itself, or lies below it along only the keywords
/// [`collect_node_refs`] walks.
///
/// Lexical containment alone is not enough. A whole-file target (`./lib.yaml`, pointer `""`)
/// contains every pointer in that file, but lowering it as a schema never enters its `components`,
/// so a `$ref` there is not inside that frame and cannot be handed its placeholder.
fn lowering_encloses(frame: &crate::diag::JsonPointer, site: &crate::diag::JsonPointer) -> bool {
    let Some(rest) = site.as_str().strip_prefix(frame.as_str()) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(rest) = rest.strip_prefix('/') else {
        // `/components/schemas/Ab` is not below `/components/schemas/A`.
        return false;
    };
    let mut tokens = rest.split('/');
    while let Some(keyword) = tokens.next() {
        match keyword {
            // Each of these is followed by a member name or an index.
            "properties" | "patternProperties" | "prefixItems" | "allOf" | "oneOf" | "anyOf" => {
                if tokens.next().is_none() {
                    return false;
                }
            }
            "additionalProperties" | "items" | "contentSchema" => {}
            _ => return false,
        }
    }
    true
}

/// Push every `$ref` string this raw schema subtree carries onto `out`, its own included.
///
/// Every spelling counts — a root component, a sub-file pointer, a whole file, a remote URL — since
/// the cycle predicate resolves each to its `(file, pointer)` identity before comparing anything.
///
/// The keywords walked here are exactly the ones `lower_schema_inner` descends into. `$defs` and
/// the validation-only applicators — `not`, `if`/`then`/`else`, `contains`, `propertyNames`,
/// `unevaluated*`, `dependentSchemas` — are deliberately NOT walked: lowering never enters them, so
/// a `$ref` reachable only that way can never put a component mid-flight and can never yield the
/// placeholder this predicate exists to detect. Counting them made an unreferenced `$defs` entry —
/// zero emitted bytes, not one instance added or removed — flip a document into a hard rejection
/// whose message asserted a dependence that does not exist. [`lowering_encloses`] accepts exactly
/// the same keywords, so the two halves of the predicate agree on what "inside" means.
fn collect_node_refs<'v>(node: &'v SpannedValue, out: &mut Vec<&'v str>) {
    // A boolean schema, or anything that is not a schema object, carries no reference.
    let Some(object) = node.as_object() else {
        return;
    };
    if let Some(reference) = object.get("$ref").and_then(SpannedValue::as_str) {
        out.push(reference);
    }
    for keyword in ["properties", "patternProperties"] {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_object) {
            for (_, child) in members.iter() {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in ["prefixItems", "allOf", "oneOf", "anyOf"] {
        if let Some(members) = object.get(keyword).and_then(SpannedValue::as_array) {
            for child in members {
                collect_node_refs(child, out);
            }
        }
    }
    for keyword in ["additionalProperties", "items", "contentSchema"] {
        if let Some(child) = object.get(keyword) {
            collect_node_refs(child, out);
        }
    }
}

fn type_accepts_null(ty: Ty, kind: &TypeKind) -> bool {
    ty.nullable || matches!(kind, TypeKind::Null | TypeKind::Any)
}

/// Why two lowered types have no typed intersection. The two answers call for different handling,
/// so an intersection never reports one where it may be the other.
///
/// Only [`NoMeet::Empty`] may be typed uninhabited ([`TypeKind::Never`]) or collapsed to the exact
/// JSON `null`: those stand in for the intersection only when no value satisfies both sides.
/// [`NoMeet::Unrepresentable`] is a set of values the generated client would silently refuse, so
/// every caller reports it (`E013`) instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoMeet {
    /// No JSON value satisfies both sides: their value categories are disjoint (a string and an
    /// integer), or their scalar `enum` sets share no value.
    Empty,
    /// The sides may share values, but no single Rust type represents the ones they share — `uuid`
    /// and `contentEncoding: base64` are both annotations on a string, so every string satisfies
    /// both — or nothing can be known yet, because one side is a reservation.
    Unrepresentable,
}

/// The JSON category every instance of a non-null lowered kind falls in, for a kind confined to
/// one. Unlike [`LowerCtx::json_category`], which picks a union's dispatch and so leaves raw bytes
/// uncategorised, this answers what an instance of a *schema* can be: binary content in a schema is
/// a (base64) JSON string.
fn value_category(kind: &TypeKind) -> Option<JsonCategory> {
    match kind {
        TypeKind::Primitive(Prim::Bool) => Some(JsonCategory::Boolean),
        TypeKind::Primitive(Prim::I32 | Prim::I64 | Prim::F64) => Some(JsonCategory::Number),
        TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date)
        | TypeKind::Bytes => Some(JsonCategory::String),
        TypeKind::Enum(enumeration) => Some(match enumeration.repr {
            ScalarRepr::String => JsonCategory::String,
            ScalarRepr::Int => JsonCategory::Number,
            ScalarRepr::Bool => JsonCategory::Boolean,
        }),
        TypeKind::Array(_) | TypeKind::Tuple(_) => Some(JsonCategory::Array),
        TypeKind::Struct(_) => Some(JsonCategory::Object),
        // `null` is intersected before any non-null kind is compared, `Never` has no instance, a
        // union and `Any` span several categories, and a reservation's body is not known yet.
        TypeKind::Null
        | TypeKind::Never
        | TypeKind::Union(_)
        | TypeKind::Any
        | TypeKind::Reserved => None,
    }
}

/// Why two non-null kinds that no intersection rule meets do not meet: empty when one side is
/// uninhabited or the two sit in disjoint JSON categories, and otherwise unrepresentable — two
/// strings of different formats, tuples of different lengths — because nothing here proves that
/// no value satisfies both.
fn no_meet(left: &TypeKind, right: &TypeKind) -> NoMeet {
    if matches!(left, TypeKind::Never) || matches!(right, TypeKind::Never) {
        return NoMeet::Empty;
    }
    match (value_category(left), value_category(right)) {
        (Some(left), Some(right)) if left != right => NoMeet::Empty,
        _ => NoMeet::Unrepresentable,
    }
}

fn non_nullable(mut ty: Ty) -> Ty {
    ty.nullable = false;
    ty
}

/// `strategy` restricted to the variants at `retained` (ascending positions into the union it
/// described), in that order.
fn retain_strategy(strategy: &UnionStrategy, retained: &[usize]) -> UnionStrategy {
    match strategy {
        UnionStrategy::Discriminated {
            tag_field,
            tags,
            categories,
            default_variant,
            untagged,
            mode,
        } => UnionStrategy::Discriminated {
            tag_field: tag_field.clone(),
            tags: retained.iter().map(|index| tags[*index].clone()).collect(),
            categories: retained.iter().map(|index| categories[*index]).collect(),
            untagged: retained.iter().map(|index| untagged[*index]).collect(),
            mode: *mode,
            // The fallback variant's index moves with the retained set; if the fallback itself
            // was dropped, the union simply has no fallback any more.
            default_variant: default_variant
                .and_then(|target| retained.iter().position(|index| *index == target)),
        },
        UnionStrategy::Disjoint { features } => UnionStrategy::Disjoint {
            features: retained
                .iter()
                .map(|index| features[*index].clone())
                .collect(),
        },
        UnionStrategy::Trial { mode, priorities } => UnionStrategy::Trial {
            mode: *mode,
            priorities: retained.iter().map(|index| priorities[*index]).collect(),
        },
    }
}

fn same_ty(left: Ty, right: Ty) -> bool {
    left.id == right.id && left.nullable == right.nullable && left.boxed == right.boxed
}

/// The remedy every `allOf` rejection (`E013`) gives.
const ALL_OF_REMEDY: &str =
    "restructure the composition so members agree, or omit this API segment with spargen::omit!";

/// The remedy every `$ref`-sibling intersection rejection (`E013`) gives, naming the construct the
/// author wrote rather than an `allOf` they did not.
const REF_SIBLING_REMEDY: &str = "restructure the schema so the `$ref` target and its sibling \
                                  keywords describe one representable type, or omit this API \
                                  segment with spargen::omit!";

/// The remedy for a union's untyped sibling keywords that cannot be applied to its branches.
const UNION_SIBLING_REMEDY: &str = "give the sibling keywords a `type`, move them into the \
                                    branches they constrain, or omit this API segment with \
                                    spargen::omit!";

/// The keyword set of every half of a [`Refiner::Scoped`] sibling that reached no branch of its
/// category, object half first; empty where every half the sibling carries reached one (or the
/// sibling is not scoped). Each entry is reported with a `W011` of its own.
fn unreached_halves(refiner: Refiner, reach: &ScopeReach) -> Vec<&'static str> {
    let Refiner::Scoped(scoped) = refiner else {
        return Vec::new();
    };
    let mut halves = Vec::new();
    if scoped.object.is_some() && !reach.object {
        halves.push(
            "object keywords (`properties`, `patternProperties`, `required`, \
             `additionalProperties`)",
        );
    }
    if scoped.array.is_some() && !reach.array {
        halves.push("array keywords (`items`, `prefixItems`)");
    }
    halves
}

/// The message for a scoped sibling half [`unreached_halves`] names.
fn unreached_message(keywords: &str) -> String {
    format!(
        "this schema's untyped {keywords} constrain only the instances of their own category, \
         and no branch of its union has that category, so they apply to no value the union \
         accepts"
    )
}

fn intersect_primitives(left: Prim, right: Prim) -> Option<Prim> {
    use Prim::{Bool, Date, DateTime, String, Uuid, F64, I32, I64};
    Some(match (left, right) {
        (Bool, Bool) => Bool,
        (I32, I32 | I64 | F64) | (I64 | F64, I32) => I32,
        (I64, I64 | F64) | (F64, I64) => I64,
        (F64, F64) => F64,
        (String, String) => String,
        (String, formatted @ (Uuid | DateTime | Date))
        | (formatted @ (Uuid | DateTime | Date), String) => formatted,
        (Uuid, Uuid) => Uuid,
        (DateTime, DateTime) => DateTime,
        (Date, Date) => Date,
        _ => return None,
    })
}

fn enum_matches_primitive(repr: ScalarRepr, primitive: Prim) -> bool {
    match repr {
        ScalarRepr::String => matches!(
            primitive,
            Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date
        ),
        ScalarRepr::Int => matches!(primitive, Prim::I32 | Prim::I64 | Prim::F64),
        ScalarRepr::Bool => primitive == Prim::Bool,
    }
}

fn lower_security_requirement(requirement: &SecurityRequirement) -> crate::ir::SecurityRequirement {
    crate::ir::SecurityRequirement(
        requirement
            .0
            .iter()
            .map(|(name, scopes)| (SchemeId(name.clone()), scopes.clone()))
            .collect(),
    )
}

/// Lower one Server Object, parsing its URL template and validating its variables.
///
/// A Server Variable `default` is unlike a Schema Object `default`: the specification says it is
/// actually sent when the caller supplies no alternative, so it changes the wire and must be
/// modeled rather than documented.
/// Resolve the base-URL override an Operation or Path Item Object declares, rendered with every
/// server variable at its declared default.
///
/// The specification defines no way for a client to *choose* among several `servers` entries in
/// this position, so the first is used and the rest are acknowledged as having no effect (`W011`).
/// Variables are substituted with their declared defaults — what the specification says is sent
/// when nothing selects another value. Unlike the document's `servers`, a per-operation override
/// gets no typed builder: there is no constructor to hand a selection to, since the choice is made
/// per call rather than per client.
fn lower_server_override(servers: &[super::Server], diags: &mut Diagnostics) -> Option<String> {
    let (first, rest) = servers.split_first()?;
    for extra in rest {
        // W011 case: extra-servers
        Diagnostic::warning(Code::DeclarationHasNoEffect, extra.provenance.clone())
            .message(format!(
                "`servers` entry `{}` past the first has no effect here: the specification \
                 defines no rule for selecting among per-operation servers, so the first is used",
                extra.url
            ))
            .emit(diags);
    }
    lower_server(first, diags).map(|server| render_server_url(&server))
}

/// Render a lowered server URL template with each variable at its declared default.
///
/// `lower_server` has already rejected a template naming an undeclared variable, so a missing
/// entry here can only occur on a document that is already failing.
fn render_server_url(server: &Server) -> String {
    let mut url = String::with_capacity(server.url.len());
    for segment in &server.segments {
        match segment {
            UrlSegment::Literal(text) => url.push_str(text),
            UrlSegment::Variable(name) => {
                if let Some(variable) = server.variables.get(name) {
                    url.push_str(&variable.default);
                }
            }
        }
    }
    url
}

fn lower_server(server: &super::Server, diags: &mut Diagnostics) -> Option<Server> {
    let segments = parse_url_template(&server.url);
    let mut seen: HashSet<&str> = HashSet::new();
    for segment in &segments {
        let UrlSegment::Variable(name) = segment else {
            continue;
        };
        if !seen.insert(name.as_str()) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` appears more than once in `{}`",
                    server.url
                ))
                .emit(diags);
            return None;
        }
        if !server.variables.contains_key(name) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server URL `{}` references undeclared variable `{name}`",
                    server.url
                ))
                .remedy("declare it under the server's `variables`")
                .emit(diags);
            return None;
        }
    }
    for (name, variable) in &server.variables {
        // A default outside its own `enum` would make the no-argument path send an illegal value.
        if !variable.enum_values.is_empty() && !variable.enum_values.contains(&variable.default) {
            Diagnostic::error(Code::InvalidInput, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` has default `{}`, which is not one of its declared \
                     `enum` values",
                    variable.default
                ))
                .emit(diags);
            return None;
        }
        if !seen.contains(name.as_str()) {
            // W011 case: unused-server-variable
            Diagnostic::warning(Code::DeclarationHasNoEffect, server.provenance.clone())
                .message(format!(
                    "server variable `{name}` is declared but does not appear in `{}`",
                    server.url
                ))
                .emit(diags);
        }
    }
    let mut docs = server.name.as_ref().map(|name| format!("Server `{name}`."));
    if let Some(description) = &server.description {
        append_text(&mut docs, description.clone());
    }
    Some(Server {
        name: server.name.clone(),
        url: server.url.clone(),
        segments,
        variables: server
            .variables
            .iter()
            .map(|(name, variable)| {
                (
                    name.clone(),
                    crate::ir::ServerVariable {
                        default: variable.default.clone(),
                        enum_values: variable.enum_values.clone(),
                        description: variable.description.clone(),
                    },
                )
            })
            .collect(),
        description: docs,
    })
}

/// Split a server URL template into literals and `{variable}` references.
///
/// An unmatched `{` is kept as literal text: the document schema constrains the template shape, so
/// there is nothing useful to diagnose here that it has not already refused.
fn parse_url_template(url: &str) -> Vec<UrlSegment> {
    let mut segments = Vec::new();
    let mut rest = url;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|at| open + at) else {
            break;
        };
        if open > 0 {
            segments.push(UrlSegment::Literal(rest[..open].to_owned()));
        }
        segments.push(UrlSegment::Variable(rest[open + 1..close].to_owned()));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(UrlSegment::Literal(rest.to_owned()));
    }
    segments
}

/// Resolve a Path Item `$ref`.
///
/// Unlike a Reference Object, the specification leaves the behavior of fields declared *alongside*
/// a Path Item `$ref` undefined. Guessing either way ships a client that calls a different set of
/// endpoints than the document describes, so a structural sibling is rejected; `summary` and
/// `description` are documentation and cannot change the wire, so they are applied.
///
/// Applying them is what makes them "allowed" rather than silently dropped. A Path Item resolves
/// to exactly one generated construct per path, so unlike a Reference Object — whose target is a
/// component shared across every use site, and whose per-site docs therefore have nowhere to land
/// (`W011`) — a Path Item reference site has a unique home for its documentation.
fn resolve_path_item(
    item: &PathItem,
    resolver: &Resolver,
    diags: &mut Diagnostics,
) -> Option<PathItem> {
    let Some(reference) = &item.reference else {
        return Some(item.clone());
    };
    if let Some(sibling) = item.reference_siblings.first() {
        // E016 case: path-item-ref-siblings
        Diagnostic::error(Code::SpecUndefinedBehavior, reference.provenance.clone())
            .message(format!(
                "a Path Item `$ref` declared alongside `{sibling}` has undefined behavior, so \
                 there is no correct client to generate"
            ))
            .remedy("move the sibling fields into the referenced Path Item, or drop the `$ref`")
            .emit(diags);
        return None;
    }
    // Relative refs inside the referenced item resolve against the file that declared the `$ref`.
    let from = reference
        .provenance
        .span
        .map(|span| span.file)
        .unwrap_or(crate::diag::FileId(0));
    let mut target =
        resolver.resolve_path_item(&reference.reference, from, &reference.provenance, diags)?;
    // One level of indirection is what the specification requires implementations to support, and
    // a chain would need its own cycle guard.
    if target.reference.is_some() {
        // E004 case: declined-hop
        Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
            .message(format!(
                "Path Item `$ref` `{}` resolves to another Path Item `$ref`; chained Path Item \
                 references are not resolved",
                reference.reference
            ))
            .emit(diags);
        return None;
    }
    // The reference site documents *this* path, so its `summary`/`description` override the
    // referenced item's own. Each is overridden independently: declaring only one at the reference
    // site keeps the other from the target rather than blanking it.
    if reference.summary.is_some() {
        target.summary = reference.summary.clone();
    }
    if reference.description.is_some() {
        target.description = reference.description.clone();
    }
    Some(target)
}

/// Resolve security requirement names that are URIs rather than declared component names.
fn resolve_external_security_schemes(
    document: &Document,
    resolver: &Resolver,
    schemes: &mut IndexMap<SchemeId, SecuritySchemeDef>,
    diags: &mut Diagnostics,
) {
    let mut wanted: Vec<(String, crate::diag::Provenance)> = Vec::new();
    let mut collect = |requirements: &[SecurityRequirement], at: &crate::diag::Provenance| {
        for requirement in requirements {
            for name in requirement.0.keys() {
                wanted.push((name.clone(), at.clone()));
            }
        }
    };
    collect(&document.security, &document.provenance);
    for item in document.paths.items.values() {
        for operation in item.operations.values() {
            if let Some(security) = &operation.security {
                collect(security, &operation.provenance);
            }
        }
    }
    for (name, at) in wanted {
        if schemes.contains_key(&SchemeId(name.clone())) {
            continue;
        }
        // Only a name that looks like a reference is worth resolving; a plain unknown name is an
        // ordinary undeclared-scheme error, reported at the requirement site.
        let reference = match name.strip_prefix("./") {
            Some(rest) => rest.to_owned(),
            None if name.contains('/') || name.contains('#') || name.contains(':') => name.clone(),
            None => continue,
        };
        let from = at
            .span
            .map(|span| span.file)
            .unwrap_or(crate::diag::FileId(0));
        let Ok(object) = resolver.resolve_component(
            &reference,
            from,
            super::deserialize::parse_security_scheme,
            diags,
        ) else {
            continue;
        };
        let mut resolved = IndexMap::new();
        resolved.insert(name.clone(), RefOr::Item(object));
        let mut document = document.clone();
        document.components.security_schemes = resolved;
        for (id, scheme) in lower_security_schemes(&document, diags) {
            schemes.insert(id, scheme);
        }
    }
}

/// Lower `components.securitySchemes`.
///
/// Every declared scheme gets a disposition here rather than only when something references it: a
/// scheme that silently vanished used to surface — if at all — as a confusing `E012` at the
/// requirement site, naming a scheme the document plainly declares.
fn lower_security_schemes(
    document: &Document,
    diags: &mut Diagnostics,
) -> IndexMap<SchemeId, SecuritySchemeDef> {
    let mut schemes = IndexMap::new();
    for (name, scheme) in &document.components.security_schemes {
        let scheme = match scheme {
            RefOr::Item(scheme) => scheme,
            // A `$ref` to another scheme component of this document resolves, one hop only. Each
            // way it can fail gets its own message: calling a declared-but-aliased scheme
            // "unresolved" would send the reader looking for a declaration that is right there.
            RefOr::Ref(reference) => {
                let resolved = match reference
                    .reference
                    .strip_prefix("#/components/securitySchemes/")
                {
                    None => Err(format!(
                        "security scheme `$ref` `{}` does not point into this document's \
                         `#/components/securitySchemes/`; only a reference to a scheme the \
                         same document declares is resolved",
                        reference.reference
                    )),
                    Some(target) => match document.components.security_schemes.get(target) {
                        Some(RefOr::Item(target)) => Ok(target),
                        None => Err(format!(
                            "unresolved security scheme reference `{}`: no scheme named \
                             `{target}` is declared under `#/components/securitySchemes/`",
                            reference.reference
                        )),
                        // One level of indirection is what the specification requires, and a
                        // chain would need its own cycle guard (an alias to itself is one).
                        Some(RefOr::Ref(_)) => Err(format!(
                            "security scheme `$ref` `{}` resolves to another security scheme \
                             `$ref`; chained security scheme references are not resolved",
                            reference.reference
                        )),
                    },
                };
                match resolved {
                    Ok(target) => target,
                    Err(message) => {
                        // E004 case: undeclared-component, declined-hop
                        Diagnostic::error(Code::UnresolvedRef, reference.provenance.clone())
                            .message(message)
                            .remedy(
                                "reference a scheme declared directly, not as another `$ref`, \
                                 under this document's `#/components/securitySchemes/`",
                            )
                            .emit(diags);
                        continue;
                    }
                }
            }
        };
        let lowered = match scheme.scheme_type.as_str() {
            "http" => match scheme.scheme.as_deref() {
                Some("bearer") => SecurityScheme::Http(HttpScheme::Bearer),
                Some("basic") => SecurityScheme::Http(HttpScheme::Basic),
                other => {
                    // `digest`, `negotiate`, and friends need a challenge/response exchange that a
                    // statically-attached credential cannot perform.
                    Diagnostic::error(Code::UnknownSecurityScheme, scheme.provenance.clone())
                        .message(format!(
                            "`http` security scheme `{}` uses authentication scheme `{}`, which \
                             spargen cannot attach",
                            name,
                            other.unwrap_or("<missing>")
                        ))
                        .remedy(
                            "use `bearer` or `basic`, or omit this API segment with \
                             spargen::omit!",
                        )
                        .emit(diags);
                    continue;
                }
            },
            "apiKey" => {
                let location = match scheme.location.as_deref() {
                    Some("header") => ApiKeyLoc::Header,
                    Some("query") => ApiKeyLoc::Query,
                    Some("cookie") => ApiKeyLoc::Cookie,
                    // The document schema requires a valid `in` for `apiKey`.
                    _ => continue,
                };
                SecurityScheme::ApiKey {
                    location,
                    name: scheme.name.clone().unwrap_or_else(|| name.clone()),
                }
            }
            "oauth2" => SecurityScheme::OAuth2,
            "openIdConnect" => SecurityScheme::OpenIdConnect,
            "mutualTLS" => {
                // W011 case: mutual-tls
                Diagnostic::warning(Code::DeclarationHasNoEffect, scheme.provenance.clone())
                    .message(format!(
                        "`mutualTLS` scheme `{name}` is satisfied by the client certificate on the \
                         injected `reqwest::Client`, so no credential is registered for it"
                    ))
                    .remedy(
                        "configure the certificate on the client passed to `Client::with_client`",
                    )
                    .emit(diags);
                SecurityScheme::MutualTls
            }
            // The document schema closes the `type` enum.
            _ => continue,
        };
        schemes.insert(
            SchemeId(name.clone()),
            SecuritySchemeDef {
                kind: lowered,
                docs: security_scheme_docs(name, scheme),
            },
        );
    }
    schemes
}

/// Render the documentation a Security Scheme Object carries into rustdoc lines.
///
/// A caller of `Client::with_credential` needs exactly this to know what to register: the token
/// format, where a token is obtained, and whether the scheme is on its way out. None of it changes
/// a byte on the wire, which is why it is documentation rather than lowered structure.
fn security_scheme_docs(name: &str, scheme: &super::SecuritySchemeObject) -> Vec<String> {
    let mut docs = Vec::new();
    let kind = match scheme.scheme_type.as_str() {
        "http" => match scheme.scheme.as_deref() {
            Some(inner) => format!("`http` (`{inner}`)"),
            None => "`http`".to_owned(),
        },
        other => format!("`{other}`"),
    };
    docs.push(format!("- `{name}` — {kind}."));
    if scheme.deprecated {
        docs.push("  - **Deprecated.**".to_owned());
    }
    if let Some(description) = &scheme.description {
        docs.push(format!("  - {}", description.replace('\n', " ")));
    }
    if let Some(format) = &scheme.bearer_format {
        docs.push(format!("  - Bearer format: `{format}`."));
    }
    if let Some(url) = &scheme.open_id_connect_url {
        docs.push(format!("  - OpenID Connect discovery: <{url}>"));
    }
    if let Some(url) = &scheme.oauth2_metadata_url {
        docs.push(format!("  - OAuth 2 metadata: <{url}>"));
    }
    for flow in &scheme.flows {
        docs.push(format!("  - Flow `{}`:", flow.name));
        for (label, url) in [
            ("authorization", &flow.authorization_url),
            ("token", &flow.token_url),
            ("refresh", &flow.refresh_url),
            ("device authorization", &flow.device_authorization_url),
        ] {
            if let Some(url) = url {
                docs.push(format!("    - {label}: <{url}>"));
            }
        }
        for (scope, description) in &flow.scopes {
            let description = description.replace('\n', " ");
            if description.is_empty() {
                docs.push(format!("    - scope `{scope}`"));
            } else {
                docs.push(format!("    - scope `{scope}` — {description}"));
            }
        }
    }
    docs
}

/// Re-type every applied field `default` against the type its field ends lowering with (#404).
///
/// [`LowerCtx::field_default`] decides a default against the type the declaring property lowers
/// to, but an intersection — `allOf` members repeating the property, or a `$ref` whose sibling
/// `properties` repeat it — then narrows that type: a `string` met with `enum: [a, b]` is the enum,
/// a `number` met with `integer` is the integer. A default the narrowed type still admits becomes a
/// value of it (the enum variant, the integer); one it does not admit is no value of the field, so
/// it is documented as not applied and reported (`W005`) at the `default` that wrote it, naming
/// the type whose field drops it. Running once over the finished graph reaches every meet, and
/// only the types that are emitted: a meet's discarded intermediates are gone or elided by now.
fn retype_field_defaults(
    graph: &mut TypeGraph,
    meet_locations: &HashMap<TypeId, Provenance>,
    diags: &mut Diagnostics,
) {
    let mut retyped: Vec<(TypeId, usize, Option<DefaultValue>)> = Vec::new();
    for (id, def) in graph.emitted() {
        let TypeKind::Struct(object) = &def.kind else {
            continue;
        };
        for (index, field) in object.fields.iter().enumerate() {
            let Some(applied) = field.default.as_ref().and_then(|d| d.applied.as_ref()) else {
                continue;
            };
            let kind = graph.get(field.ty.id).map(|target| &target.kind);
            let value = representable_default(&reclassify_default(applied), kind);
            if value.as_ref() != Some(applied) {
                retyped.push((id, index, value));
            }
        }
    }
    for (id, index, value) in retyped {
        let Some(def) = graph.get_mut(id) else {
            continue;
        };
        let located = meet_locations
            .get(&id)
            .unwrap_or(&def.provenance)
            .pointer
            .clone();
        let TypeKind::Struct(object) = &mut def.kind else {
            continue;
        };
        let field = &mut object.fields[index];
        let Some(default) = field.default.as_mut() else {
            continue;
        };
        if value.is_none() {
            let written = default
                .applied
                .as_ref()
                .map(written_default_display)
                .unwrap_or_default();
            // Each `default` that wrote this value is dropped with it, so each is reported (#543).
            for at in std::iter::once(&default.provenance).chain(&default.also_written) {
                Diagnostic::warning(Code::SchemaDefaultNotApplied, at.clone())
                    .message(format!(
                        "schema `default` `{written}` of property `{}` is not a value of the type \
                         an intersection narrows the property to in `{}`; it is documented in \
                         rustdoc there but not applied as a deserialization default",
                        field.name.wire, located
                    ))
                    .remedy(
                        "use a default every intersected schema of the property admits, or set \
                         the value explicitly at each call site",
                    )
                    .emit(diags);
            }
            default.doc_note = format!("Default (not applied): `{written}`.");
        }
        default.applied = value;
    }
}

/// Recover the JSON value a representable default was decided from, so it can be decided again
/// against another type. An integral float is classified as the integer JSON Schema says it is: a
/// `number` field's `3` is carried as `3.0`, and the `integer` it narrows to admits it.
fn reclassify_default(value: &DefaultValue) -> RawDefault {
    match value {
        DefaultValue::Bool(value) => RawDefault::Bool(*value),
        DefaultValue::Int(value) => RawDefault::Int(*value),
        DefaultValue::Float(value)
            if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value < i64::MAX as f64 =>
        {
            RawDefault::Int(*value as i64)
        }
        DefaultValue::Float(value) => RawDefault::Float(*value),
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => {
            RawDefault::Str(value.clone())
        }
    }
}

/// Render a representable default as [`raw_display`] renders the JSON it came from.
fn written_default_display(value: &DefaultValue) -> String {
    match value {
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => format!("{value:?}"),
        other => default_display(other),
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
fn gate_xml_field_renames(
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

/// The set of type ids transitively reachable from `roots` through the type graph's structural
/// edges (struct fields and typed `additionalProperties`, array/tuple elements, union variants).
/// A visited set makes recursive (`$ref`-cycle) types terminate.
fn reachable_types(graph: &TypeGraph, roots: &[TypeId]) -> HashSet<TypeId> {
    let mut visited = HashSet::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(def) = graph.get(id) else {
            continue;
        };
        stack.extend(kind_edges(&def.kind));
    }
    visited
}

/// The type ids a definition of `kind` refers to directly: its struct fields and typed
/// `additionalProperties`, array/tuple elements, and union variants.
fn kind_edges(kind: &TypeKind) -> Vec<TypeId> {
    match kind {
        TypeKind::Struct(object) => {
            let mut edges: Vec<TypeId> = object.fields.iter().map(|field| field.ty.id).collect();
            if let AdditionalProps::Typed(ty) = &object.additional {
                edges.push(ty.id);
            }
            edges
        }
        TypeKind::Array(ty) => vec![ty.id],
        TypeKind::Tuple(items) => items.iter().map(|ty| ty.id).collect(),
        TypeKind::Union(union) => union.variants.iter().map(|variant| variant.ty.id).collect(),
        // A reservation has no structural edges yet: its body is still being lowered.
        TypeKind::Reserved
        | TypeKind::Primitive(_)
        | TypeKind::Enum(_)
        | TypeKind::Bytes
        | TypeKind::Null
        | TypeKind::Never
        | TypeKind::Any => Vec::new(),
    }
}

fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    let essence = media_essence(media);
    match classify_media(essence) {
        Some((media, _)) => Some(media),
        None => {
            Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
                .message(format!("media type `{essence}` is not supported"))
                .emit(diags);
            None
        }
    }
}

/// Where a body sits, which decides whether an alternative that decodes identically may go
/// unreported. A response narrows only at the type; a request narrows at the wire as well.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyPosition {
    Request,
    Response,
}

/// The `content` entry [`choose_media`] selected, and the `W014` disclosing what it passed over.
struct ChosenMedia<'a, T> {
    media: &'a str,
    value: &'a T,
    /// Built but not emitted. The narrowing `W014` discloses is only real once the caller's own
    /// gates accept the selection, so the caller emits this when — and only when — the selected
    /// entry lowers. A selection those gates then reject is reported by its `E009` alone.
    narrowing: Option<Diagnostic>,
}

/// `opaque` answers, without lowering anything, whether an entry's body constrains nothing — the
/// proof that an ignored alternative would decode exactly like the selection.
fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
    position: BodyPosition,
    opaque: impl Fn(&T) -> bool,
) -> Option<ChosenMedia<'a, T>> {
    if content.is_empty() {
        return None;
    }
    let mut selected: Option<(bool, u8, usize, &str, &T, MediaType)> = None;
    for (source_index, (media, value)) in content.iter().enumerate() {
        let Some((classified, rank)) = classify_media(media_essence(media)) else {
            continue;
        };
        // A request sends its media key as `Content-Type`, which must be concrete (RFC 9110 § 8.3),
        // so a range is only a candidate there once no concrete key classifies — and then it is
        // selected and rejected by `lower_request_body`, never silently skipped.
        let unsendable = position == BodyPosition::Request
            && classify_media_range(media_essence(media)).is_some();
        let candidate = (
            unsendable,
            rank,
            source_index,
            media.as_str(),
            value,
            classified,
        );
        if selected.as_ref().is_none_or(|current| {
            (unsendable, rank, source_index) < (current.0, current.1, current.2)
        }) {
            selected = Some(candidate);
        }
    }
    if let Some((_, _, _, media, value, classified)) = selected {
        // A generated method sends and decodes exactly one media type, so the alternatives are not
        // generated. That narrows the documented surface — a server that also accepts XML will only
        // ever be sent JSON — so it is reported rather than dropped in silence.
        //
        // An alternative that decodes to the very same thing narrows nothing, though. An
        // opaque-octets body that constrains nothing is `bytes::Bytes`, so a ranged media response
        // offering `video/*`, `image/png` and `application/octet-stream` gives up nothing by
        // generating one of them, and saying otherwise is noise on a common shape.
        //
        // The rule is deliberately confined to octet-stream, and the confinement is load-bearing
        // rather than conservatism waiting to be relaxed. Octet-stream is the one codec whose gate
        // collapses every body it admits onto a single type: `opaque_octets` maps an absent or
        // empty schema to `Bytes`, `format: binary` and `contentEncoding: base64` are `Bytes`, and
        // anything else is rejected outright. No other codec has that property, and three things
        // go wrong the moment the rule is widened on the strength of "both sides constrain
        // nothing":
        //
        // - *Constrains nothing* has two spellings that do not agree outside this gate. A media
        //   type with no `schema` at all lowers to `()`; one with `schema: {}` lowers to `Any`,
        //   i.e. `serde_json::Value`. Suppressing between them makes the *order* of two content
        //   keys decide the response type, silently.
        // - `itemSchema` lives outside the body schema entirely. Two sequential entries can both
        //   constrain nothing and still stream different item types.
        // A *request* narrows at the wire whatever the types do, so nothing is suppressed there at
        // all. The chosen media key becomes the `Content-Type` verbatim, and a server documented as
        // accepting `application/octet-stream` and `video/*` is only ever sent the first — a real
        // narrowing even though both decode to `Bytes`. It is tempting to think ranges cannot reach
        // a request anyway, since one is rejected as a request `Content-Type` below; that rejection
        // fires on the media actually *selected*, and a suppressed alternative is never selected.
        //
        // "Constrains nothing" also has to be proved, not assumed from the media type: an
        // octet-classified alternative carrying an object schema would be *rejected* by the octet
        // gate, not turned into bytes, so suppressing it would be the silent fourth behavior
        // nothing is allowed.
        let suppressible =
            position == BodyPosition::Response && classified == MediaType::OctetStream;
        let ignored: Vec<&str> = content
            .iter()
            .filter(|(candidate, _)| candidate.as_str() != media)
            .filter(|(candidate, candidate_value)| {
                !suppressible
                    || !opaque(candidate_value)
                    || classify_media(media_essence(candidate)).map(|(media, _)| media)
                        != Some(MediaType::OctetStream)
            })
            .map(|(candidate, _)| candidate.as_str())
            .collect();
        return Some(ChosenMedia {
            media,
            value,
            narrowing: alternative_media_ignored(media, &ignored, provenance),
        });
    }
    let (media, _) = content.first()?;
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
    None
}

/// The `W014` saying `media` is selected and `ignored` is not, or `None` when nothing was ignored.
/// Built rather than emitted: see [`ChosenMedia::narrowing`].
///
/// The message asserts only what is decided here — which entry was selected — and never that it
/// "is generated": whether anything is generated depends on the rest of the document and on the
/// entry point (`check` generates nothing), neither of which this site can see (#174).
fn alternative_media_ignored(
    media: &str,
    ignored: &[&str],
    provenance: &crate::diag::Provenance,
) -> Option<Diagnostic> {
    if ignored.is_empty() {
        return None;
    }
    Some(
        Diagnostic::warning(Code::AlternativeMediaIgnored, provenance.clone())
            .message(format!(
                "`{media}` is selected; the alternative media type(s) `{}` are not",
                ignored.join("`, `")
            ))
            .remedy(
                "remove the alternatives, or omit this API segment with spargen::omit! and \
                 hand-write the call",
            )
            .build(),
    )
}

/// Whether a Media Type Object constrains nothing about the body it describes.
///
/// Only ever asked of an octet-classified entry, where "constrains nothing" and every other body
/// the gate admits mean the same type, `bytes::Bytes`. It is deliberately not a general answer to
/// "do these two entries decode alike" — see the caller for why that question needs more than
/// this one does.
///
/// A `$ref` is never taken as opaque — proving it would mean resolving it here, and answering
/// "unknown" as "not opaque" only costs a warning that was already being reported. That holds for
/// both places a reference can appear: a `schema: {$ref: …}`, and a 3.2 Media Type Object that is
/// *itself* a Reference Object, which parses with `schema: None` and would otherwise take the
/// no-schema arm and be called opaque on the strength of a field the `$ref` spelling never sets.
fn media_object_is_opaque(object: &MediaTypeObject) -> bool {
    // Destructured exhaustively, like the schema predicates it delegates to. A Media Type Object
    // carries four fields besides `schema` that can describe the body, and reading only `schema`
    // is how `itemSchema` — which is where a sequential media's item type actually lives — slipped
    // past this question entirely. A field added here must be classified, not silently ignored.
    let MediaTypeObject {
        reference,
        schema,
        item_schema,
        encoding,
        prefix_encoding,
        item_encoding,
        provenance: _,
    } = object;
    if reference.is_some()
        || item_schema.is_some()
        || !encoding.is_empty()
        || !prefix_encoding.is_empty()
        || item_encoding.is_some()
    {
        return false;
    }
    match schema {
        None => true,
        Some(RefOr::Item(schema)) => schema.constrains_nothing(),
        Some(RefOr::Ref(_)) => false,
    }
}

/// Whether a media type essence is a media type or range at all: exactly one `/` between two
/// RFC 6838 § 4.2 `restricted-name`s, with `*` allowed only as the whole key (`*/*`), as the whole
/// subtype (`type/*`), or in front of a structured syntax suffix (`application/*+json`, the range
/// over every subtype carrying that suffix).
///
/// [`classify_media`] asks this first, so no arm can accept a key on the strength of a prefix or a
/// suffix alone: not `text/plain/extra`, not `application/vnd.a/b+json`, and not the range `a/b/*`.
/// Parameters are already gone, because every caller passes [`media_essence`] output. A key that
/// fails is not a media type, so it classifies as nothing and takes the existing unsupported path:
/// `E009` when it is the only candidate, or an ignored alternative under `W014` otherwise. An
/// Encoding Object's `contentType` is asked this directly and is `E009` when it fails, since it is
/// sent verbatim even when it names no codec spargen has; its parameters are then held to
/// [`media_type_with_parameters`].
fn media_type_is_well_formed(essence: &str) -> bool {
    /// `restricted-name = restricted-name-first *126restricted-name-chars` (RFC 6838 § 4.2). ASCII
    /// letters of either case are accepted; case sensitivity is left to the arms that match names.
    fn restricted_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        matches!(bytes.first(), Some(first) if first.is_ascii_alphanumeric())
            && bytes.len() <= 127
            && bytes.iter().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
                    )
            })
    }
    let Some((kind, subtype)) = essence.split_once('/') else {
        return false;
    };
    match (kind, subtype) {
        ("*", "*") => true,
        (_, "*") => restricted_name(kind),
        _ => {
            restricted_name(kind)
                && match subtype.strip_prefix("*+") {
                    Some(suffix) => restricted_name(suffix),
                    None => restricted_name(subtype),
                }
        }
    }
}

/// Whether a well-formed essence is a structured-suffix media **range** such as
/// `application/*+json`, the range over every subtype carrying that suffix.
///
/// Unlike `type/*` and `*/*`, which [`classify_media_range`] gives their own family codec, a suffix
/// range needs no codec of its own: the suffix arms of [`classify_media`] already read it the way
/// the suffix says. It is still a range, though, and so it can no more be a request's
/// `Content-Type` than `video/*` can.
fn media_essence_is_suffix_range(essence: &str) -> bool {
    essence
        .split_once('/')
        .is_some_and(|(_, subtype)| subtype.starts_with("*+"))
}

/// The request body `content` entries [`choose_media`] may select from, plus the structured-suffix
/// ranges withheld from that choice.
///
/// A suffix range is withheld only while another entry is *sendable*: it classifies, it is neither
/// kind of range, and it is not streaming media. Then the range, which a request cannot send,
/// never outranks something it could. With no sendable sibling, nothing is withheld, the range is
/// selected as before, and the request range check refuses it. Keys keep their document order.
fn request_media_candidates<T>(content: &IndexMap<String, T>) -> (IndexMap<String, &T>, Vec<&str>) {
    let suffix_range =
        |essence: &str| media_essence_is_suffix_range(essence) && classify_media(essence).is_some();
    let sendable = content.keys().any(|media| {
        let essence = media_essence(media);
        !suffix_range(essence)
            && classify_media_range(essence).is_none()
            && classify_media(essence)
                .is_some_and(|(classified, _)| classified.stream_framing().is_none())
    });
    let mut candidates = IndexMap::new();
    let mut withheld = Vec::new();
    for (media, value) in content {
        if sendable && suffix_range(media_essence(media)) {
            withheld.push(media.as_str());
        } else {
            candidates.insert(media.clone(), value);
        }
    }
    (candidates, withheld)
}

fn media_essence(media: &str) -> &str {
    media.split(';').next().unwrap_or(media).trim()
}

/// The element of a comma-separated media type list a client sends: the first, ended by the first
/// comma outside an RFC 9110 § 5.6.4 quoted-string, so `text/plain; name="a, b"` is one element.
/// An unterminated quoted-string runs to the end of the list, where [`media_type_with_parameters`]
/// rejects it.
fn first_list_element(list: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in list.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b',' if !quoted => return list[..index].trim(),
            _ => {}
        }
    }
    list.trim()
}

/// Why a media type's parameter list cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParameterFault {
    /// Not `parameters = *( OWS ";" OWS [ parameter ] )` with
    /// `parameter = token "=" ( token / quoted-string )` (RFC 9110 §§ 5.6.6, 5.6.4).
    Malformed,
    /// Well-formed, but a quoted value the multipart transport's parser (`mime` 0.3, behind
    /// reqwest's `Part::mime_str`) refuses: empty, or holding a `"` (as a quoted-pair) or a tab.
    /// Carries the canonical form, for a caller that never sends the value.
    Unsendable(String),
}

/// A media type whose essence is already well-formed, with its parameter list checked against
/// RFC 9110 § 5.6.6 and re-serialized as `type/subtype; name=value; …`.
///
/// Names and values keep their spelling, quoted-strings included. What changes is only what the
/// grammar leaves free: whitespace around `;` and empty parameters (`text/plain;;a=b`) are
/// dropped, since the multipart transport's parser refuses whitespace before a `;` and an empty
/// parameter, both of which RFC 9110 admits.
fn media_type_with_parameters(media: &str) -> Result<String, ParameterFault> {
    fn tchar(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
    }
    fn ows(bytes: &[u8], mut at: usize) -> usize {
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        at
    }
    fn token(bytes: &[u8], from: usize) -> Result<usize, ParameterFault> {
        let end = from
            + bytes[from..]
                .iter()
                .take_while(|byte| tchar(**byte))
                .count();
        if end == from {
            Err(ParameterFault::Malformed)
        } else {
            Ok(end)
        }
    }
    /// The end of the quoted-string opening at `from`, and whether the transport can send it.
    fn quoted_string(bytes: &[u8], from: usize) -> Result<(usize, bool), ParameterFault> {
        let mut sendable = true;
        let mut at = from + 1;
        loop {
            match bytes.get(at).copied() {
                None => return Err(ParameterFault::Malformed),
                Some(b'"') => return Ok((at + 1, sendable && at > from + 1)),
                Some(b'\\') => {
                    // quoted-pair = "\" ( HTAB / SP / VCHAR / obs-text )
                    match bytes.get(at + 1).copied() {
                        Some(b'\t' | b'"') => sendable = false,
                        Some(b' ' | 0x21..=0x7e | 0x80..=0xff) => {}
                        _ => return Err(ParameterFault::Malformed),
                    }
                    at += 2;
                }
                // qdtext = HTAB / SP / %x21 / %x23-5B / %x5D-7E / obs-text
                Some(b'\t') => {
                    sendable = false;
                    at += 1;
                }
                Some(b' ' | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff) => at += 1,
                Some(_) => return Err(ParameterFault::Malformed),
            }
        }
    }

    let essence = media_essence(media);
    let Some((_, parameters)) = media.split_once(';') else {
        return Ok(essence.to_owned());
    };
    let bytes = parameters.as_bytes();
    let mut canonical = essence.to_owned();
    let mut unsendable = false;
    let mut at = 0;
    loop {
        at = ows(bytes, at);
        match bytes.get(at) {
            None => break,
            Some(b';') => {
                at += 1;
                continue;
            }
            Some(_) => {}
        }
        let name_end = token(bytes, at)?;
        if bytes.get(name_end) != Some(&b'=') {
            return Err(ParameterFault::Malformed);
        }
        let value_start = name_end + 1;
        let value_end = if bytes.get(value_start) == Some(&b'"') {
            let (end, sendable) = quoted_string(bytes, value_start)?;
            unsendable |= !sendable;
            end
        } else {
            token(bytes, value_start)?
        };
        canonical.push_str("; ");
        canonical.push_str(&parameters[at..value_end]);
        at = ows(bytes, value_end);
        match bytes.get(at) {
            None => break,
            Some(b';') => at += 1,
            Some(_) => return Err(ParameterFault::Malformed),
        }
    }
    if unsendable {
        Err(ParameterFault::Unsendable(canonical))
    } else {
        Ok(canonical)
    }
}

/// Classify a content type into its wire codec and deterministic preference rank. Structured JSON
/// suffixes use the JSON codec; textual types use raw UTF-8 except for the two streaming framings.
/// GitHub's documented octocat representation is a textual vendor media type. Concrete members of
/// the `image`, `audio`, and `video` families are opaque octets, like the ranges naming them.
fn classify_media(essence: &str) -> Option<(MediaType, u8)> {
    if !media_type_is_well_formed(essence) {
        return None;
    }
    if let Some(range) = classify_media_range(essence) {
        return Some(range);
    }
    let classified = match essence {
        "application/json" => (MediaType::Json, 0),
        media if media.starts_with("application/") && media.ends_with("+json") => {
            (MediaType::Json, 0)
        }
        "application/xml" | "text/xml" => (MediaType::Xml, 1),
        "multipart/form-data" => (MediaType::Multipart, 2),
        "application/x-www-form-urlencoded" => (MediaType::FormUrlEncoded, 3),
        "application/octet-stream" => (MediaType::OctetStream, 4),
        // The sequential kinds are matched before the `text/` prefix arm so `text/event-stream` is
        // a stream, not text; their rank (6) still sits below text (5).
        "text/event-stream" => (MediaType::EventStream, 6),
        "application/x-ndjson" | "application/jsonl" => (MediaType::Ndjson, 6),
        "application/json-seq" => (MediaType::JsonSequence, 6),
        media if media.starts_with("application/") && media.ends_with("+json-seq") => {
            (MediaType::JsonSequence, 6)
        }
        "application/octocat-stream" => (MediaType::Text, 5),
        media if media.starts_with("text/") => (MediaType::Text, 5),
        // Rank 9, the end of the ladder, is a concrete member of a binary family
        // (`classify_binary_family`): below every codec listed above and below both range ranks
        // (7 and 8), so on a response a family key is generated only when it is the sole key that
        // classifies; a request additionally prefers it to a range, which it cannot send
        // (`choose_media`).
        _ => return classify_binary_family(essence),
    };
    Some(classified)
}

/// Classify a media **range** — `type/*`, or `*/*` — which the specification permits as a `content`
/// key and which describes a whole family rather than one type.
///
/// `text/*` is the family read as raw UTF-8; every other family, `*/*` included, is opaque octets,
/// which is the only honest reading of "whatever this server detected". A range ranks below every
/// codec spargen has (`text/*` at 7, every other family at 8), so a concrete sibling outranks it —
/// with one deliberate exception on responses: a concrete `image`/`audio`/`video` member (9,
/// `classify_binary_family`) sits *below* both, because a response listing a range beside
/// `image/png` generated from the range before that family classified and must keep doing so. A
/// request never selects a range while a concrete key classifies (`choose_media`), since a range
/// is not a `Content-Type`.
///
/// The type before the slash must be present — `/*` names no family and stays unsupported — and is
/// matched case-insensitively, because media types are (RFC 9110 § 8.3.1) and reading `TEXT/*` as
/// binary would be silently wrong rather than loudly unsupported.
fn classify_media_range(essence: &str) -> Option<(MediaType, u8)> {
    let family = essence
        .strip_suffix("/*")
        .filter(|family| !family.is_empty())?;
    Some(if family.eq_ignore_ascii_case("text") {
        (MediaType::Text, 7)
    } else {
        (MediaType::OctetStream, 8)
    })
}

/// Classify a concrete member of a family whose every subtype is an opaque payload — `image/jpeg`,
/// `audio/mpeg`, `video/mp4`. RFC 6838 registers `image`, `audio`, and `video` as top-level types
/// for non-textual data, so bytes is the only faithful reading of any member, exactly as it is for
/// the family's range (`image/*`); the octet gate still demands a schema that collapses to
/// `bytes::Bytes`. It sits at the very end of the ladder, below every other key spargen can
/// classify — octet-stream, text, the sequential kinds, and the ranges, `*/*` included — so on a
/// response a family key is generated only when it is the sole key that classifies, which is
/// exactly the shape #82 reports (`image/jpeg` as the only content key), and no response that
/// generated before the family rule existed changes its selection or body type. A request body is
/// the one place a family key outranks a range: a range cannot be sent as `Content-Type`, and
/// every such request was rejected before, so preferring the concrete key only turns a rejection
/// into a client.
///
/// `application/*` is deliberately not a family here: it mixes binary (`application/pdf`) with
/// textual (`application/sdp`, `application/sql`) subtypes, and reading SDP as bytes would be
/// silently wrong rather than loudly unsupported. For the same reason a subtype carrying an RFC
/// 6838 structured-syntax suffix (`image/svg+xml`, or any `+suffix`) is not claimed: the suffix
/// says the payload is a text syntax, so reading SVG as bytes would be silently wrong, and it stays
/// unsupported until a codec for the suffix exists in this position. The family is matched
/// case-insensitively for the same reason the range is (RFC 9110 § 8.3.1): `IMAGE/*` and
/// `IMAGE/JPEG` must agree.
fn classify_binary_family(essence: &str) -> Option<(MediaType, u8)> {
    let (family, subtype) = essence.split_once('/')?;
    if subtype.is_empty() || subtype.contains('*') || subtype.contains('+') {
        return None;
    }
    ["image", "audio", "video"]
        .iter()
        .any(|binary| family.eq_ignore_ascii_case(binary))
        .then_some((MediaType::OctetStream, 9))
}

fn raw_text_type_supported(graph: &TypeGraph, ty: Ty) -> bool {
    fn visit(graph: &TypeGraph, ty: Ty, seen: &mut HashSet<TypeId>) -> bool {
        if !seen.insert(ty.id) {
            return true;
        }
        let supported = match graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Primitive(Prim::String | Prim::Uuid | Prim::DateTime | Prim::Date))
            | Some(TypeKind::Bytes)
            | Some(TypeKind::Any) => true,
            Some(TypeKind::Enum(enumeration)) => enumeration.repr == ScalarRepr::String,
            Some(TypeKind::Union(union)) => union
                .variants
                .iter()
                .all(|variant| visit(graph, variant.ty, seen)),
            // An unlowered body cannot be proved string-like, and this answers "is it proved":
            // no, so the raw text body is refused rather than admitted on the strength of nothing.
            Some(TypeKind::Reserved) => false,
            _ => false,
        };
        seen.remove(&ty.id);
        supported
    }

    visit(graph, ty, &mut HashSet::new())
}

fn parse_status(status: &str) -> Option<StatusSpec> {
    if let Some(prefix) = status.strip_suffix("XX") {
        return Some(StatusSpec::Range(prefix.parse().ok()?));
    }
    Some(StatusSpec::Exact(status.parse().ok()?))
}

fn parse_path_template(path: &str) -> PathTemplate {
    let mut segments = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let (literal, after_literal) = rest.split_at(open);
        if !literal.is_empty() {
            segments.push(PathSegment::Literal(literal.to_owned()));
        }
        if let Some(close) = after_literal.find('}') {
            let name = &after_literal[1..close];
            segments.push(PathSegment::Param(name.to_owned()));
            rest = &after_literal[close + 1..];
        } else {
            rest = after_literal;
            break;
        }
    }
    if !rest.is_empty() {
        segments.push(PathSegment::Literal(rest.to_owned()));
    }
    PathTemplate {
        raw: path.to_owned(),
        segments,
    }
}

/// A `default` value classified into the scalar kinds that can back a Rust literal, or `Other` for
/// anything (object/array/null) that cannot.
#[derive(PartialEq)]
enum RawDefault {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Other,
}

fn classify_default(value: &SpannedValue) -> RawDefault {
    match &value.node {
        Node::Bool(value) => RawDefault::Bool(*value),
        Node::Number(Number::Int(value)) => RawDefault::Int(*value),
        Node::Number(Number::UInt(value)) => {
            i64::try_from(*value).map_or(RawDefault::Float(*value as f64), RawDefault::Int)
        }
        Node::Number(Number::Float(value)) => RawDefault::Float(*value),
        Node::String(value) => RawDefault::Str(value.clone()),
        Node::Null | Node::Array(_) | Node::Object(_) => RawDefault::Other,
    }
}

/// Decide whether a classified `default` is representable against the field's lowered type: a
/// `Primitive` of the matching scalar kind, or a `ScalarEnum` value that is one of its variants.
/// The `deprecated`/`readOnly`/`writeOnly` annotations of one *property* subschema.
///
/// These are per-property annotations. Reading them from the enclosing object would both ignore a
/// property's own `deprecated: true` and mark every field of a deprecated object as deprecated;
/// an object-level annotation belongs on the type, where it already is.
fn field_flags(child: &SchemaOr) -> (bool, bool, bool) {
    match child {
        // A boolean schema carries no annotations.
        SchemaOr::Bool(_) => (false, false, false),
        SchemaOr::Schema(schema) => (schema.deprecated, schema.read_only, schema.write_only),
    }
}

fn representable_default(raw: &RawDefault, kind: Option<&TypeKind>) -> Option<DefaultValue> {
    let kind = kind?;
    match (raw, kind) {
        (RawDefault::Bool(value), TypeKind::Primitive(Prim::Bool)) => {
            Some(DefaultValue::Bool(*value))
        }
        // Width-check the literal so an out-of-range `int32` default is treated as
        // non-representable (→ W005, rustdoc-only) rather than rendered into code that fails to
        // compile. `i64` fields always fit.
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I32))
            if i32::try_from(*value).is_ok() =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Int(value), TypeKind::Primitive(Prim::I64)) => Some(DefaultValue::Int(*value)),
        (RawDefault::Int(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value as f64))
        }
        (RawDefault::Float(value), TypeKind::Primitive(Prim::F64)) => {
            Some(DefaultValue::Float(*value))
        }
        (RawDefault::Str(value), TypeKind::Primitive(Prim::String)) => {
            Some(DefaultValue::Str(value.clone()))
        }
        (RawDefault::Str(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::String
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::String(v) if v == value)) =>
        {
            Some(DefaultValue::EnumVariant(value.clone()))
        }
        (RawDefault::Int(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Int
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Int(v) if v == value)) =>
        {
            Some(DefaultValue::Int(*value))
        }
        (RawDefault::Bool(value), TypeKind::Enum(enumeration))
            if enumeration.repr == ScalarRepr::Bool
                && enumeration
                    .variants
                    .iter()
                    .any(|variant| matches!(variant, ScalarValue::Bool(v) if v == value)) =>
        {
            Some(DefaultValue::Bool(*value))
        }
        // A property whose type is a cycle-closing `$ref` to a component still being lowered sees
        // its placeholder here. No literal can be proved to fit an unknown body, so the default is
        // not representable: it is documented and reported (`W005`) rather than wired. It is also
        // the answer the filled body would get — a cycle closes only through a schema that holds a
        // reference (an object, array, tuple, or union), and none of those takes a literal here.
        (_, TypeKind::Reserved) => None,
        _ => None,
    }
}

/// Render any `default` for a rustdoc note — nicely when it is representable against `kind`, else
/// as compact JSON. Used by the document-only positions (parameters, component roots) that never
/// serde-wire a default but must still surface it.
fn default_display_for(raw: &SpannedValue, kind: Option<&TypeKind>) -> String {
    match representable_default(&classify_default(raw), kind) {
        Some(value) => default_display(&value),
        None => raw_display(raw),
    }
}

/// Render a representable default for its rustdoc `Default:` note.
fn default_display(value: &DefaultValue) -> String {
    match value {
        DefaultValue::Bool(value) => value.to_string(),
        DefaultValue::Int(value) => value.to_string(),
        DefaultValue::Float(value) => value.to_string(),
        DefaultValue::Str(value) | DefaultValue::EnumVariant(value) => value.clone(),
    }
}

/// Render an arbitrary default value as compact JSON-ish text for the rustdoc note of a
/// non-representable (`W005`) default.
fn raw_display(value: &SpannedValue) -> String {
    match &value.node {
        Node::Null => "null".to_owned(),
        Node::Bool(value) => value.to_string(),
        Node::Number(Number::Int(value)) => value.to_string(),
        Node::Number(Number::UInt(value)) => value.to_string(),
        Node::Number(Number::Float(value)) => value.to_string(),
        Node::String(value) => format!("{value:?}"),
        Node::Array(items) => {
            let items = items.iter().map(raw_display).collect::<Vec<_>>().join(", ");
            format!("[{items}]")
        }
        Node::Object(map) => {
            let entries = map
                .iter()
                .map(|(key, value)| format!("{:?}: {}", key.name, raw_display(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{entries}}}")
        }
    }
}

/// Append a note as a trailing rustdoc paragraph on a type's [`Docs`], used to surface a
/// component-root `default` on the generated named type.
fn append_doc_note(docs: &mut Docs, note: String) {
    match &mut docs.description {
        Some(description) => {
            description.push_str("\n\n");
            description.push_str(&note);
        }
        None => docs.description = Some(note),
    }
}

/// Whether a schema accepts `null`: a `"null"` member of its type array, or a `null` `enum` member
/// or `const`. Computed at component reserve time so `$ref` consumers wrap the type in `Option`,
/// and it agrees with the `nullable` that [`LowerCtx::lower_schema`]/[`LowerCtx::lower_enum`]
/// compute from the same schema.
/// One `allOf` member's contribution to the merged type: either a set of object fields (with its
/// `additionalProperties` policy and its own `required` names) to flatten, or a scalar/leaf type.
/// `Clone` so a bundle-`$ref` member's contribution can be recorded once and replayed at every use
/// (see `LowerCtx::resolved_contributions`).
#[derive(Clone)]
enum Contribution {
    Object {
        fields: Vec<Field>,
        additional: AdditionalProps,
        required: Vec<String>,
        /// Whether the member admits `null`, where the member decides it: `Some` for a member
        /// that states a `type` (whether it lists `"null"`) or is a `$ref` target (its lowered
        /// nullability), `None` for an untyped one ([`stated_nullability`]). An untyped member's
        /// object keywords constrain only objects, so it admits `null` without deciding the
        /// merge's nullability, as an untyped `$ref` sibling leaves its target's alone.
        nullable: Option<bool>,
    },
    Scalar(Ty),
}

/// Whether an object `allOf` merge admits `null` (issue #425): every member that decides its
/// nullability admits it, and at least one decides. An untyped member admits `null` and decides
/// nothing, so a merge of untyped members alone keeps the non-null struct an untyped object
/// schema lowers to on its own; one nullable `$ref` member beside them makes it nullable, as the
/// `$ref`-sibling spelling of the same conjunction does. Only object contributions reach here.
fn object_all_of_admits_null(contributions: &[Contribution]) -> bool {
    let mut decided = false;
    for contribution in contributions {
        if let Contribution::Object {
            nullable: Some(admits),
            ..
        } = contribution
        {
            if !admits {
                return false;
            }
            decided = true;
        }
    }
    decided
}

/// Whether a schema's own `type` admits `null`: `None` for an untyped schema, which states no
/// category and so decides nothing about `null` in an `allOf` merge.
fn stated_nullability(schema: &Schema) -> Option<bool> {
    (!schema.types.types.is_empty()).then(|| schema.types.types.contains(&JsonType::Null))
}

/// Whether a schema constrains object shape — declared/pattern properties, an `additionalProperties`
/// policy, a `required` set, or an explicit `object` type — and so contributes fields to an `allOf`
/// merge rather than a scalar.
fn schema_is_object_like(schema: &Schema) -> bool {
    !schema.properties.is_empty()
        || !schema.pattern_properties.is_empty()
        || schema.additional_properties.is_some()
        || !schema.required.is_empty()
        || schema.types.types.contains(&JsonType::Object)
}

/// Whether a schema carries a `oneOf` or an `anyOf` of its own.
fn schema_has_union(schema: &Schema) -> bool {
    !schema.one_of.is_empty() || !schema.any_of.is_empty()
}

/// The index of the one `allOf` member that is an inline `oneOf`/`anyOf`, where exactly one is
/// ([`LowerCtx::lower_all_of_with_union_member`]). A member that is a `$ref` is its target first,
/// and an `allOf` with several union members is an ordinary `allOf`: its unions meet as scalar
/// members, and beside object members they are `E013`.
fn sole_union_member(schema: &Schema) -> Option<usize> {
    let mut unions = schema.all_of.iter().enumerate().filter(|(_, member)| {
        matches!(member, SchemaOr::Schema(member) if member.reference.is_none() && schema_has_union(member))
    });
    let (index, _) = unions.next()?;
    unions.next().is_none().then_some(index)
}

/// The `required` names a schema's own `properties` do not declare, deduplicated, in source order.
/// [`LowerCtx::object_body`] carries each as a required field of its own, marked
/// [`Field::undeclared`].
fn undeclared_required(schema: &Schema) -> Vec<String> {
    let mut seen: HashSet<&str> = schema.properties.keys().map(String::as_str).collect();
    schema
        .required
        .iter()
        .filter(|name| seen.insert(name.as_str()))
        .cloned()
        .collect()
}

/// Settle a property two sides of an intersection both carry when exactly one side declares it,
/// and report whether it did. A field marked [`Field::undeclared`] is no declaration: it stands
/// for a key its object requires and types it by that object's `additionalProperties` schema,
/// which by the rule every merge here applies (a merged object's `additionalProperties` constrains
/// only the keys no side declares) does not reach a property the other side declares. So the
/// declared field is kept whole — its type, default, flags and `xml` hints — and the requirement
/// is added to it. Two declarations, or two undeclared fields, are left to the caller to intersect.
fn take_declaration(existing: &mut Field, other: &Field) -> bool {
    if existing.undeclared == other.undeclared {
        return false;
    }
    let required = existing.required || other.required;
    if existing.undeclared {
        *existing = other.clone();
    }
    existing.required = required;
    if required {
        if let Some(default) = &mut existing.default {
            default.applied = None;
        }
    }
    true
}

/// Merge the `default` the other side of an intersection declares for a repeated property into
/// the field kept for it (#432). `allOf` is commutative, and so is this merge: a default either
/// side declares survives whichever side came first, and two sides that declare different
/// defaults keep the same one in either order — an applicable default before one that cannot be
/// applied, then the lesser rustdoc note, then the lesser `default` location — while the other is
/// reported (`W005`) at the `default` that wrote it, since the field cannot carry it. Two defaults
/// of one value (`3` and `3.0` alike) are one default for choosing what to keep, but not for
/// accounting: the kept one carries the other's pointers in [`FieldDefault::also_written`], and
/// whichever drop reports it reports every pointer that wrote the value (#543). The kept default
/// is then decided against the merged field as every other is: a requirement drops its
/// application here, and [`retype_field_defaults`] re-types it against the narrowed type, which
/// for an empty meet leaves it unapplied and reports it as `W005` (#453).
fn merge_field_default(
    kept: &mut Option<FieldDefault>,
    other: Option<&FieldDefault>,
    property: &str,
    diags: &mut Diagnostics,
) {
    let Some(other) = other else {
        return;
    };
    let Some(current) = kept.as_ref() else {
        *kept = Some(other.clone());
        return;
    };
    let rank = |default: &FieldDefault| {
        (
            default.applied.is_none(),
            default.doc_note.clone(),
            provenance_rank(&default.provenance),
        )
    };
    let other_first = rank(other) < rank(current);
    let (winner, loser) = if other_first {
        (other, current)
    } else {
        (current, other)
    };
    let same_value = match (&winner.applied, &loser.applied) {
        (Some(left), Some(right)) => reclassify_default(left) == reclassify_default(right),
        _ => winner.doc_note == loser.doc_note,
    };
    if same_value {
        // The equal value the loser wrote is merged, not forgotten: its pointers ride on the kept
        // default, so a later drop reports each of them (#543).
        let mut merged = winner.clone();
        merged.also_written.push(loser.provenance.clone());
        merged
            .also_written
            .extend(loser.also_written.iter().cloned());
        merged.also_written.sort_by_key(provenance_rank);
        *kept = Some(merged);
        return;
    }
    // Every pointer that wrote the dropped value is reported, not only the one the merge reached
    // first, so which `default`s are reported does not depend on the members' order (#543).
    for at in std::iter::once(&loser.provenance).chain(&loser.also_written) {
        Diagnostic::warning(Code::SchemaDefaultNotApplied, at.clone())
            .message(format!(
                "schema `default` of property `{property}` differs from the `default` another \
                 intersected schema declares for it at `{}`, which the merged field keeps; this \
                 one is neither applied nor documented there",
                winner.provenance.pointer
            ))
            .remedy(
                "declare one default for the property, or the same default on every intersected \
                 schema that declares it",
            )
            .emit(diags);
    }
    if other_first {
        *kept = Some(other.clone());
    }
}

/// The total order [`merge_field_default`] breaks ties by and keeps [`FieldDefault::also_written`]
/// in: the `default`'s pointer, then its source span.
fn provenance_rank(provenance: &Provenance) -> (String, Option<(u32, usize, usize)>) {
    (
        provenance.pointer.to_string(),
        provenance
            .span
            .map(|span| (span.file.0, span.start.offset, span.end.offset)),
    )
}

/// The category a schema's object or array applicators imply, for a schema that establishes none
/// of its own. See [`implied_applicator_category`].
enum ImpliedCategory {
    /// Only object applicators, or only array applicators: the category they apply to.
    Only(JsonType),
    /// Both kinds, and nothing to choose between them.
    Conflicting,
}

/// The category a `$ref` sibling's applicators establish when the sibling names no `type` and
/// carries no other keyword that lowers to a shape of its own (`enum`, `const`, a composition, a
/// binary encoding, a `$ref`).
///
/// The object applicators are `properties`, `patternProperties`, `required` and
/// `additionalProperties`; the array applicators are `items` and `prefixItems`. `None` when the
/// schema carries neither kind, or already names or implies its shape some other way.
fn implied_applicator_category(schema: &Schema) -> Option<ImpliedCategory> {
    let establishes_elsewhere = !schema.types.types.is_empty()
        || schema.reference.is_some()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || !schema.all_of.is_empty()
        || !schema.one_of.is_empty()
        || !schema.any_of.is_empty()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary");
    if establishes_elsewhere {
        return None;
    }
    // `types` is empty here, so this is exactly the object applicators.
    let object = schema_is_object_like(schema);
    let array = schema.items.is_some() || !schema.prefix_items.is_empty();
    match (object, array) {
        (true, false) => Some(ImpliedCategory::Only(JsonType::Object)),
        (false, true) => Some(ImpliedCategory::Only(JsonType::Array)),
        (true, true) => Some(ImpliedCategory::Conflicting),
        (false, false) => None,
    }
}

/// Split the siblings of a `$ref` (`sibling`, the reference already stripped) that carry a
/// `oneOf`/`anyOf` into the keywords beside the union and the union itself, as separate conjuncts
/// ([`LowerCtx::meet_ref_union_sibling`]). The union keeps its `discriminator` and the sibling's
/// provenance, and nothing else: every other keyword, annotations included, stays with the
/// keywords, so each is lowered, and reported, once. The struct literal is exhaustive, so a field
/// added to [`Schema`] has to be placed here.
fn split_union_sibling(sibling: &Schema) -> (Schema, Schema) {
    let mut keywords = sibling.clone();
    let one_of = std::mem::take(&mut keywords.one_of);
    let any_of = std::mem::take(&mut keywords.any_of);
    let discriminator = keywords.discriminator.take();
    let union = Schema {
        boolean: None,
        types: super::TypeSet::default(),
        reference: None,
        properties: IndexMap::new(),
        required: Vec::new(),
        additional_properties: None,
        pattern_properties: IndexMap::new(),
        items: None,
        prefix_items: Vec::new(),
        all_of: Vec::new(),
        one_of,
        any_of,
        discriminator,
        defs: IndexMap::new(),
        validation_children: Vec::new(),
        enum_values: None,
        const_value: None,
        default: None,
        format: None,
        content_encoding: None,
        content_media_type: None,
        content_schema: None,
        xml: None,
        validation: ValidationKeywords::default(),
        deprecated: false,
        read_only: false,
        write_only: false,
        title: None,
        description: None,
        provenance: sibling.provenance.clone(),
    };
    (keywords, union)
}

/// Whether a non-object schema still imposes a scalar/leaf constraint (a non-null primitive type,
/// an `enum`/`const`, or `contentEncoding`) — as opposed to a pure annotation member (`{}` /
/// `{description: ...}`) that constrains nothing.
fn schema_imposes_scalar(schema: &Schema) -> bool {
    !schema.types.types.is_empty()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary")
        || !schema.one_of.is_empty()
        || !schema.any_of.is_empty()
}

/// One row of [`SHAPE_KEYWORDS`]: a keyword's published spelling, and whether a schema carries it.
type ShapeKeyword = (&'static str, fn(&Schema) -> bool);

/// Every keyword [`schema_has_shape_constraint`] reads, as the published spelling beside the test
/// that recognises it. The gate is exactly "any row matches", so this table IS the gate: a keyword
/// enters or leaves it here and nowhere else.
///
/// `E013`'s explain publishes this set, less `$ref` (a `$ref`'s siblings are this gate's input with
/// the reference already stripped, so `$ref` is never one of them); it is split there into the
/// keywords that establish a shape, the ones that refine one, and `required`. The in-module tests
/// hold that text equal to this table in both directions, and hold the gate to reading nothing the
/// table does not name.
const SHAPE_KEYWORDS: &[ShapeKeyword] = &[
    ("type", |schema| !schema.types.types.is_empty()),
    ("properties", |schema| !schema.properties.is_empty()),
    ("patternProperties", |schema| {
        !schema.pattern_properties.is_empty()
    }),
    ("additionalProperties", |schema| {
        schema.additional_properties.is_some()
    }),
    ("required", |schema| !schema.required.is_empty()),
    ("items", |schema| schema.items.is_some()),
    ("prefixItems", |schema| !schema.prefix_items.is_empty()),
    ("enum", |schema| schema.enum_values.is_some()),
    ("const", |schema| schema.const_value.is_some()),
    ("contentEncoding", |schema| {
        schema.content_encoding.is_some()
    }),
    ("format: binary", |schema| {
        schema.format.as_deref() == Some("binary")
    }),
    ("$ref", |schema| schema.reference.is_some()),
    ("allOf", |schema| !schema.all_of.is_empty()),
    // `oneOf`/`anyOf` count exactly as `allOf` does: in 2020-12 each is an applicator constraining
    // the instance, so a union beside a `$ref` narrows the target like a `type` beside it would,
    // and `schema_imposes_scalar` already treats them so. Leaving them out made a `$ref` whose only
    // sibling was a union take the bare-reference exit, discarding the union with no diagnostic.
    ("oneOf", |schema| !schema.one_of.is_empty()),
    ("anyOf", |schema| !schema.any_of.is_empty()),
];

/// Whether a schema carries any keyword that gives it a shape of its own, which decides whether a
/// `$ref`'s siblings are intersected with its target or the `$ref` is simply its target. Defined
/// by [`SHAPE_KEYWORDS`] alone; add a keyword there, never as another clause here.
fn schema_has_shape_constraint(schema: &Schema) -> bool {
    SHAPE_KEYWORDS.iter().any(|(_, bears)| bears(schema))
}

/// The provenance of an `allOf` member for diagnostics — the schema's own provenance, or the
/// document root for a bare boolean member that carries none.
fn member_provenance(member: &SchemaOr) -> crate::diag::Provenance {
    match member {
        SchemaOr::Schema(schema) => schema.provenance.clone(),
        SchemaOr::Bool(_) => crate::diag::Provenance::new(crate::diag::JsonPointer::root(), None),
    }
}

/// Whether a union member is a null-only schema (`{type: "null"}`) — stripped from the union and
/// folded into its nullability, exactly like a `"null"` in a type array. A bare `$ref` member is
/// never null-only here (it names a component with its own shape); only an inline `type: null`
/// node with no other constraints counts.
fn member_is_null_only(member: &SchemaOr) -> bool {
    let SchemaOr::Schema(schema) = member else {
        return false;
    };
    schema.reference.is_none()
        && schema.types.types == [JsonType::Null]
        && schema.one_of.is_empty()
        && schema.any_of.is_empty()
        && schema.all_of.is_empty()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.properties.is_empty()
}

fn schema_is_nullable(schema: &Schema) -> bool {
    schema.types.types.contains(&JsonType::Null)
        || schema
            .enum_values
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| matches!(value.node, Node::Null)))
        || schema
            .const_value
            .as_ref()
            .is_some_and(|value| matches!(value.node, Node::Null))
}

fn scalar_value(value: &SpannedValue) -> Option<ScalarValue> {
    match &value.node {
        Node::Bool(value) => Some(ScalarValue::Bool(*value)),
        Node::Number(Number::Int(value)) => Some(ScalarValue::Int(*value)),
        Node::Number(Number::UInt(value)) => i64::try_from(*value).ok().map(ScalarValue::Int),
        Node::String(value) => Some(ScalarValue::String(value.clone())),
        _ => None,
    }
}

/// The `E008` message for an enum/const member `scalar_value` rejected, naming the reason that
/// member is not representable: an object/array member, a float, and an integer above `i64::MAX`
/// fail for different reasons, and a message blaming the wrong one misdirects the fix.
fn non_scalar_enum_message(value: &SpannedValue) -> String {
    match &value.node {
        Node::Number(Number::Float(float)) => format!(
            // `Debug`, so `1.0` reads as the float it is rather than `Display`'s `1`.
            "enum/const value {float:?} is a floating-point number, which has no Rust enum \
             discriminant (only string, integer, and boolean members are representable as enum \
             variants)"
        ),
        Node::Number(Number::UInt(uint)) => format!(
            "enum/const value {uint} exceeds i64::MAX, so it is not representable as an integer \
             enum variant"
        ),
        _ => "enum/const values must be scalars (object/array members are not representable as \
              enum variants)"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{schema_has_shape_constraint, Schema, SHAPE_KEYWORDS};
    use crate::diag::{Code, Diagnostics, FileId, JsonPointer};

    fn schema(yaml: &str) -> Schema {
        let mut diags = Diagnostics::default();
        let value = crate::source::parse_yaml(FileId(0), yaml, &mut diags)
            .unwrap_or_else(|_| panic!("probe does not parse: {yaml}"));
        super::super::deserialize::parse_schema(&value, &JsonPointer::root(), &mut diags)
            .unwrap_or_else(|| panic!("probe is not a schema: {yaml}"))
    }

    /// The backticked spans of the one sentence of `explain` that `lead` begins, `lead` included.
    fn backticked_in_sentence(explain: &str, lead: &str) -> Vec<String> {
        let start = explain
            .find(lead)
            .unwrap_or_else(|| panic!("E013's explain no longer says {lead:?}: {explain}"));
        let rest = &explain[start..];
        let sentence = &rest[..rest
            .find(". ")
            .unwrap_or_else(|| panic!("E013's sentence {lead:?} never ends"))];
        sentence
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    /// The sibling keywords `E013`'s explain publishes as taking part in a `$ref`-sibling
    /// intersection: the one sentence that lists every keyword bearing a shape of its own.
    fn published_sibling_keywords() -> Vec<String> {
        // Named by its code string: a `Code::<Variant>` mention of an enumerating code is read as
        // an emission site by `diag`'s case-marker test, and this reads the text, emitting nothing.
        let explain = "E013".parse::<Code>().expect("E013 is a code").explain();
        backticked_in_sentence(explain, "A sibling bears a shape of its own ")
    }

    /// Issue #155: the explain's keyword list and the gate that decides are one set. Read in both
    /// directions — a keyword the gate reads that the text omits, and one the text names that the
    /// gate never reads — so neither can move alone. `$ref` is the one row the text does not name,
    /// because the gate sees a `$ref`'s siblings with the reference already stripped.
    #[test]
    fn e013_explain_names_exactly_the_keywords_the_gate_reads() {
        let listed = published_sibling_keywords();
        let mut published = BTreeSet::new();
        for keyword in &listed {
            assert!(
                published.insert(keyword.as_str()),
                "E013's explain names `{keyword}` twice"
            );
        }
        let gate: BTreeSet<&str> = SHAPE_KEYWORDS
            .iter()
            .map(|(keyword, _)| *keyword)
            .filter(|keyword| *keyword != "$ref")
            .collect();
        assert_eq!(
            published, gate,
            "E013's explain and `SHAPE_KEYWORDS` disagree about which sibling keywords take part in \
             a `$ref` intersection (explain lists: {listed:?})"
        );
        assert_eq!(
            SHAPE_KEYWORDS.len(),
            gate.len() + 1,
            "`SHAPE_KEYWORDS` repeats a keyword, or lost `$ref`"
        );
    }

    /// Each row's name is the keyword its test recognises: a schema carrying that keyword alone
    /// clears the gate through that row and no other. A table with a predicate filed under the
    /// wrong name would pass the explain comparison above while publishing the wrong rule.
    #[test]
    fn every_shape_keyword_row_recognises_the_keyword_it_names() {
        let probe = |keyword: &str| match keyword {
            "type" => "type: string",
            "properties" => "properties: { a: { type: string } }",
            "patternProperties" => "patternProperties: { '^a': { type: string } }",
            "additionalProperties" => "additionalProperties: false",
            "required" => "required: [a]",
            "items" => "items: { type: string }",
            "prefixItems" => "prefixItems: [{ type: string }]",
            "enum" => "enum: [a]",
            "const" => "const: a",
            "contentEncoding" => "contentEncoding: base64",
            "format: binary" => "format: binary",
            "$ref" => "$ref: '#/components/schemas/A'",
            "allOf" => "allOf: [{ type: string }]",
            "oneOf" => "oneOf: [{ type: string }]",
            "anyOf" => "anyOf: [{ type: string }]",
            other => panic!("`SHAPE_KEYWORDS` row `{other}` has no probe here; add one"),
        };
        for (keyword, _) in SHAPE_KEYWORDS {
            let schema = schema(probe(keyword));
            let matched: Vec<&str> = SHAPE_KEYWORDS
                .iter()
                .filter(|(_, bears)| bears(&schema))
                .map(|(name, _)| *name)
                .collect();
            assert_eq!(
                matched,
                [*keyword],
                "a schema carrying only `{keyword}` should clear exactly its own row"
            );
        }
    }

    /// The gate reads nothing the table does not name. A schema carrying every other keyword the
    /// parser keeps — validation, annotations, content, `$defs` — does not clear it, so a
    /// clause added to `schema_has_shape_constraint` beside the table (the `maxLength` mutation
    /// #155 measured surviving) fails here rather than widening the published rule unseen.
    #[test]
    fn the_gate_reads_only_the_keywords_its_table_names() {
        let everything_else = schema(
            "discriminator: { propertyName: kind }\n\
             $defs: { A: { type: string } }\n\
             not: { type: string }\n\
             if: { type: string }\n\
             then: { type: string }\n\
             else: { type: string }\n\
             format: date-time\n\
             contentMediaType: application/json\n\
             contentSchema: { type: string }\n\
             xml: { name: a }\n\
             pattern: '^a'\n\
             minimum: 1\n\
             maximum: 2\n\
             exclusiveMinimum: 0\n\
             exclusiveMaximum: 3\n\
             multipleOf: 1\n\
             minLength: 1\n\
             maxLength: 2\n\
             minItems: 1\n\
             maxItems: 2\n\
             uniqueItems: true\n\
             minProperties: 1\n\
             maxProperties: 2\n\
             default: a\n\
             deprecated: true\n\
             readOnly: true\n\
             writeOnly: true\n\
             title: t\n\
             description: d\n",
        );
        assert!(
            !schema_has_shape_constraint(&everything_else),
            "the gate cleared a schema that carries none of `SHAPE_KEYWORDS`: it reads a keyword the \
             table (and so E013's explain) does not name"
        );
    }
}
