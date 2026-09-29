use serde::Serialize;

use super::{InterpId, Severity};

/// A stable diagnostic code — `E###` for errors, `W###` for warnings.
///
/// Codes are product surface: each has [`explain`](Code::explain) text, a docs entry, and at
/// least one fixture that triggers it. The set is closed and exhaustively
/// enumerable via [`all`](Code::all) so the docs/behavior exhaustiveness test can iterate it and
/// fail the build if code and docs diverge. `#[non_exhaustive]` keeps adding a code a non-breaking
/// change for external matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Code {
    /// The `openapi` field declares an unsupported version (e.g. 3.0.x); no conversion is
    /// offered.
    UnsupportedOpenApiVersion,
    /// `jsonSchemaDialect` is not the shared OAS 3.1/3.2 base dialect.
    UnsupportedDialect,
    /// A remote (`http`/`https`) `$ref` is not pinned in `spargen.lock` (or is an unfetchable
    /// absolute-URI scheme). Remote refs resolve only from vendored, hash-pinned copies.
    AbsoluteRefUnsupported,
    /// A `$ref` could not be resolved within the input bundle.
    UnresolvedRef,
    /// A vendored remote `$ref` document drifted from its `spargen.lock` pin (sha256 mismatch, or
    /// the vendored copy is missing) — the lock is the source of truth, so it is refused.
    VendoredRefDrift,
    /// A validation-only keyword (`pattern`, `minimum`, …) was ignored (W-class).
    ValidationKeywordIgnored,
    /// `patternProperties` cannot be represented as a typed overflow map — heterogeneous value
    /// types, or combined with `additionalProperties: false` (matrix: Schema shape → R).
    PatternPropertiesRejected,
    /// `$dynamicRef`/`$dynamicAnchor` are rejected (matrix: Schema shape → R).
    DynamicRefRejected,
    /// A `oneOf`/`anyOf` applicator combination could not be represented faithfully.
    NonDisjointUnion,
    /// A heterogeneous or structured `enum`/`const` value set is rejected.
    NonScalarEnum,
    /// A request body media type spargen does not support (XML, multipart, …).
    UnsupportedMediaType,
    /// An unsupported parameter style (`deepObject`, `spaceDelimited`, …) (matrix: Parameters → R).
    UnsupportedParameterStyle,
    /// `webhooks`/`callbacks`/`links` acknowledged; no code emitted (matrix: Document → W).
    ServerInitiatedFlowIgnored,
    /// A `security` requirement references a scheme that is not declared under
    /// `components.securitySchemes` (or is of an unsupported type) (matrix: Security).
    UnknownSecurityScheme,
    /// An intersecting composition — `allOf` members, or a `$ref` and its own shape-bearing sibling
    /// keywords — could not be reconciled into a single type: conflicting property types,
    /// conflicting `additionalProperties`, an object/scalar mix, incompatible scalars, or a `$ref`
    /// that closes a reference cycle back to the component enclosing it (matrix: Schema shape).
    AllOfIrreconcilable,
    /// The input could not be parsed or violates a required structural OpenAPI shape.
    InvalidInput,
    /// An object declares the same key twice; the duplicate makes the member ambiguous, so it is
    /// rejected rather than silently collapsed to one occurrence.
    DuplicateObjectKey,
    /// A compatibility omit rule did not match a source construct or attempted an invalid removal.
    InvalidOmitRule,
    /// A compatibility omit profile removed a construct.
    OmittedConstruct,
    /// A compatibility omit profile created an invalid remaining document.
    OmitCreatedInvalidDocument,
    /// A schema `default` value could not be applied as a deserialization default (it is not a
    /// scalar matching the field's type); it is documented in rustdoc but not wired (matrix: Schema
    /// shape → W).
    SchemaDefaultNotApplied,
    /// An unsupported XML representation hint (`xml.namespace`, `xml.prefix`, or `xml.wrapped`) was
    /// ignored; only `xml.name`/`xml.attribute` are honored (matrix: Media → W).
    XmlHintIgnored,
    /// OpenAPI 3.2 `itemSchema` appeared on non-sequential media, where it has no wire meaning.
    Oas32ConstructIgnored,
    /// A body or response offered several media types; one was generated and the alternatives were
    /// not (matrix: Media → W).
    AlternativeMediaIgnored,
    /// Schema composition nests deeper than spargen will lower (a very long `$ref` chain or a
    /// pathologically nested inline schema), so lowering is stopped before it could exhaust the
    /// stack. Rejected rather than risk a crash on adversarial or machine-generated input.
    SchemaNestingTooDeep,
    /// The consuming Cargo package does not declare the versions or features required by the
    /// generated runtime.
    RuntimeDependencyContract,
    /// A construct whose behavior the OpenAPI specification explicitly leaves *undefined*, so no
    /// generated client could be known to be correct.
    SpecUndefinedBehavior,
    /// `items` beside `prefixItems` describes a tuple with a typed variable-length rest, which no
    /// Rust type expresses. `items: false` — a pure fixed-length tuple — is supported.
    TupleRestNotRepresentable,
    /// A construct was declared that cannot change anything spargen generates or sends. It is
    /// acknowledged rather than dropped in silence, so the input never has an undocumented
    /// disposition.
    DeclarationHasNoEffect,
    /// Generation ran without a consumer Cargo manifest, so the generated runtime's dependency
    /// contract (`E023`) could not be audited.
    RuntimeAuditSkipped,
    /// Generation ran outside a Cargo build script, so no rebuild triggers were emitted and the
    /// dependency audit was skipped.
    CargoIntegrationDegraded,
    /// Cargo integration was required by the caller but is not available in this process.
    CargoIntegrationRequired,
}

impl Code {
    /// The stable string form, e.g. `"E001"` or `"W009"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => "E001",
            Code::UnsupportedDialect => "E002",
            Code::AbsoluteRefUnsupported => "E003",
            Code::UnresolvedRef => "E004",
            Code::VendoredRefDrift => "E021",
            Code::ValidationKeywordIgnored => "W001",
            Code::PatternPropertiesRejected => "E005",
            Code::DynamicRefRejected => "E006",
            Code::NonDisjointUnion => "E007",
            Code::NonScalarEnum => "E008",
            Code::UnsupportedMediaType => "E009",
            Code::UnsupportedParameterStyle => "E010",
            Code::ServerInitiatedFlowIgnored => "W002",
            Code::InvalidInput => "E011",
            Code::DuplicateObjectKey => "E022",
            Code::UnknownSecurityScheme => "E012",
            Code::AllOfIrreconcilable => "E013",
            Code::OmittedConstruct => "W009",
            Code::InvalidOmitRule => "E019",
            Code::OmitCreatedInvalidDocument => "E020",
            Code::SchemaDefaultNotApplied => "W005",
            Code::XmlHintIgnored => "W006",
            Code::Oas32ConstructIgnored => "W010",
            Code::AlternativeMediaIgnored => "W014",
            Code::SchemaNestingTooDeep => "E014",
            Code::RuntimeDependencyContract => "E023",
            Code::SpecUndefinedBehavior => "E016",
            Code::TupleRestNotRepresentable => "E015",
            Code::DeclarationHasNoEffect => "W011",
            Code::RuntimeAuditSkipped => "W012",
            Code::CargoIntegrationDegraded => "W013",
            Code::CargoIntegrationRequired => "E024",
        }
    }

    /// Whether this code is an error or a warning.
    pub fn severity(self) -> Severity {
        match self.as_str().as_bytes()[0] {
            b'E' => Severity::Error,
            b'W' => Severity::Warning,
            _ => unreachable!("diagnostic code prefixes are closed"),
        }
    }

    /// The one-line human title.
    pub fn title(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => "unsupported OpenAPI version",
            Code::UnsupportedDialect => "unsupported JSON Schema dialect",
            Code::AbsoluteRefUnsupported => "remote $ref not pinned",
            Code::UnresolvedRef => "unresolved $ref",
            Code::VendoredRefDrift => "vendored remote $ref drifted from lock",
            Code::ValidationKeywordIgnored => "validation-only keyword ignored",
            Code::PatternPropertiesRejected => "patternProperties not representable as a typed map",
            Code::DynamicRefRejected => "dynamic reference unsupported",
            Code::NonDisjointUnion => "union applicators cannot be represented",
            Code::NonScalarEnum => "enum values are not homogeneous scalars",
            Code::UnsupportedMediaType => "unsupported media type",
            Code::UnsupportedParameterStyle => "unsupported parameter style",
            Code::ServerInitiatedFlowIgnored => "server-initiated flow ignored",
            Code::InvalidInput => "invalid input document",
            Code::DuplicateObjectKey => "duplicate object key",
            Code::UnknownSecurityScheme => "unknown security scheme",
            Code::AllOfIrreconcilable => "irreconcilable composition",
            Code::InvalidOmitRule => "invalid omit rule",
            Code::OmittedConstruct => "construct omitted",
            Code::OmitCreatedInvalidDocument => "omit profile created an invalid document",
            Code::SchemaDefaultNotApplied => "schema default not applied",
            Code::XmlHintIgnored => "unsupported XML hint ignored",
            Code::Oas32ConstructIgnored => "non-sequential itemSchema ignored",
            Code::AlternativeMediaIgnored => "alternative media type not generated",
            Code::SchemaNestingTooDeep => "schema nesting is too deep to lower",
            Code::RuntimeDependencyContract => "invalid generated-runtime dependency contract",
            Code::SpecUndefinedBehavior => "specification-undefined construct",
            Code::TupleRestNotRepresentable => "variable-length tuple not representable",
            Code::DeclarationHasNoEffect => "declared construct has no effect",
            Code::RuntimeAuditSkipped => "runtime-dependency audit skipped",
            Code::CargoIntegrationDegraded => "cargo integration degraded",
            Code::CargoIntegrationRequired => "cargo integration required but unavailable",
        }
    }

    /// Extended documentation shown by `spargen explain E###` and on the published errors index.
    ///
    /// Every body is published, user-facing text, but not every body is held to the code by a
    /// test, and a contributor editing one should know which kind they are editing:
    ///
    /// - A body that **enumerates the cases reaching its code** is listed in this module's
    ///   `ENUMERATED_CASES` test table. Each case names a phrase the body must contain, and every
    ///   emission site of the code carries a `// E### case: <case>` marker naming the cases it
    ///   reports, so an emission site outside the enumeration, a case with no emission site, or a
    ///   case deleted from the body fails `every_emission_site_falls_into_a_case_its_explain_text_lists`.
    ///   A case may also reserve a message wording, which its sites must use and no other site of
    ///   the code may. Adding an enumerating body means adding it to that table.
    /// - `E023`'s body is pinned byte-for-byte, clause by clause, by
    ///   `runtime_contract::tests::the_e023_explain_text_states_the_inheritance_rules_this_module_enforces`.
    ///   Its consumer-obligation clauses, which that test does not cite fixtures for, are tied to
    ///   the fixtures that enforce them by this module's `EXPLAIN_CLAUSES_OWNED_ELSEWHERE` table.
    /// - Every other body is prose held only to being non-empty. A body that grows a list of the
    ///   cases reaching its code has become the first kind, and belongs in the table.
    pub fn explain(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => {
                "The root `openapi` field must declare `3.1.x` or `3.2.x`. OpenAPI 3.2 is a compatible superset of 3.1 (same JSON Schema 2020-12 semantics) and is accepted through the same frontend. OpenAPI 3.0.x uses a different schema dialect and is rejected rather than converted."
            }
            Code::UnsupportedDialect => {
                "`jsonSchemaDialect`, when present, must be the OAS base dialect (`https://spec.openapis.org/oas/3.1/dialect/base`). The OpenAPI 3.2 text deliberately retains that URI and defines no dialect identifier of its own, but 3.2's published document schema gives `https://spec.openapis.org/oas/3.2/dialect/2025-09-17` as the field's default, so a 3.2 document may use either spelling. Other dialects are permitted by OpenAPI but optional for tooling, and spargen rejects them because their keywords cannot be lowered under the compile-time-correctness contract."
            }
            Code::AbsoluteRefUnsupported => {
                "Remote (`http`/`https`) `$ref` resolution is hermetic: `generate` and `check` never touch the network. A remote ref is resolved only from a locally vendored copy whose bytes are hash-pinned in `spargen.lock`. This error fires when a remote ref is not yet pinned there (or names an unfetchable absolute-URI scheme such as `urn:`). Run `spargen lock <spec>` to fetch, vendor under `.spargen/vendor/`, and pin it — then `generate`/`check` resolve it offline. Alternatively, vendor the document by hand and reference it with a relative file path."
            }
            Code::UnresolvedRef => {
                "Something spargen had to resolve — a reference, or the schema scope a reference would be resolved against — could not be followed to a usable target, so the construct that named it is rejected rather than generated without it. The target need not be a schema: Path Item, Parameter, Request Body, Response, Header, Media Type and Security Scheme references all report here. The cases that reach this code are: the target is absent from the loaded input bundle (check the file path and the JSON Pointer fragment); a local component reference names an entry the document does not declare, under `schemas`, `parameters`, `requestBodies`, `responses`, `headers`, `mediaTypes`, `securitySchemes` or `pathItems` — including one an omit profile removed, where auto-carve is the intended escape; a chain of reference hops closes into a cycle; a hop resolves but spargen declines to follow it — a Path Item `$ref` whose target is itself a Path Item `$ref`, a security scheme alias pointing at another alias, or a fragment form the resolver does not walk; or static `$id`/`$anchor` schema resource scopes, where scope-aware resolution is what is missing rather than a target. Each diagnostic's message is more specific than this list, with one exception: a reference reported as `unsupported or unresolved` is one spargen could not resolve *and* could not tell an absent target from a fragment form it declines to follow."
            }
            Code::VendoredRefDrift => {
                "A remote `$ref` is pinned in `spargen.lock`, but its vendored copy under `.spargen/vendor/` is missing or its bytes no longer match the pinned sha256. The lock is the source of truth, so the drifted content is refused rather than used silently. Re-run `spargen lock <spec>` to re-vendor and re-pin, or restore the vendored file to its pinned bytes."
            }
            Code::ValidationKeywordIgnored => {
                "The keyword affects runtime validation but not the static Rust shape. Spargen records a warning and generates the shape. OpenAPI 3.2 `contentMediaType`/`contentSchema` are consumed without this warning only on the string `data` property of a sequential `text/event-stream` item envelope, where they define the JSON payload type."
            }
            Code::PatternPropertiesRejected => {
                "`patternProperties` is represented as a typed overflow map (`#[serde(flatten)]`) when every pattern value schema — and any typed `additionalProperties` value — lowers to the same type; the key regex itself is validation-only and reported as `W001`. It is rejected only when a faithful map is impossible: heterogeneous value types (which one map cannot type), or a combination with `additionalProperties: false` (a flatten map cannot both capture pattern values and deny other unknown keys)."
            }
            Code::DynamicRefRejected => {
                "`$dynamicRef` and `$dynamicAnchor` require dynamic schema-scope evaluation and are rejected."
            }
            Code::NonDisjointUnion => {
                "`oneOf`/`anyOf` unions are lowered to typed Rust enums with custom `Deserialize`/`Serialize` — never `serde(untagged)` and never degraded to `serde_json::Value`. Fast paths dispatch by discriminator tag, a unique non-object JSON category, or a proven disjoint category/required key. Overlapping variants use typed trial matching over one buffered value: `oneOf` requires exactly one successful variant; `anyOf` deterministically selects the most specific successful variant (enum before broad scalar, integer before number, more-required object before broader object, recursive array specificity, then source order), and serialization revalidates the same rule. Shape constraints adjacent to the union are intersected into every branch; a branch they exclude is dropped with `W011` while the rest of the enum stands. This error is reported for a union that cannot be turned into a generated enum, which happens in three ways. One is an applicator combination that is not yet representable, such as declaring both `oneOf` and `anyOf` on the same schema node, or OpenAPI 3.2 `discriminator.defaultMapping` without a generated fallback branch, or a `discriminator.mapping` value that names none of the union's members by component name — a pointer into a component written in the root document (`#/components/schemas/Envelope/properties/payload`; the same spelling inside a referenced sub-file names its member by the pointer text and matches it), a file reference (a slash-free one such as `cat.yaml` is also a legal component name and is read as one, as the specification recommends), or a component that is not a member — which no member can be matched to, so the tag it declares would not be the one on the wire. The other is a union that resolves to *itself*: a member that is a `$ref` back to the very union being lowered, or a union that is a schema's whole body whose only non-null member is a `$ref` closing a cycle onto that same schema. Neither describes a value a decoder can terminate on — the generated `Deserialize` would re-enter itself on the same input with no base case — so they are refused rather than emitted. A *mutually* recursive nullable alias is not this and is supported: `B: {oneOf: [{$ref: A}, {type: \"null\"}]}` where `A` refers back to `B` is `Option<Box<A>>`, the same type the direct `{$ref: A}` spelling produces, whichever way the two are spelled or which files they live in. The third is sibling keywords that leave the union with no branch at all, because every branch — or the sole one, when the union has a single non-null member it collapses to — has an empty or unrepresentable intersection with the enclosing schema's own sibling keywords: `type: object` beside a lone `{type: string}` member, say. Split the applicators, make every discriminator branch explicit, break the self-reference, reconcile the sibling keywords with the members, or omit this API segment with `spargen::omit!`."
            }
            Code::NonScalarEnum => {
                "Enums and const values must be homogeneous scalar sets. A `null` member (or `\"null\"` in the schema's type array) is allowed: it is stripped and makes a remaining scalar enum nullable (`Option<Enum>`), while a value set of only `null` lowers to the exact JSON null type (`()`). Sets that mix distinct non-null scalar kinds (e.g. a string with an integer) or that contain object/array members are rejected."
            }
            Code::UnsupportedMediaType => {
                "JSON (`application/json` and `application/*+json`), XML (`application/xml` and `text/xml`), raw binary (`application/octet-stream`), raw UTF-8 text (`text/*` and GitHub's `application/octocat-stream`), form-urlencoded requests, and multipart requests are generated. Text is decoded through a JSON string value so string enums/formats remain typed; binary responses remain `bytes::Bytes`; single- and multi-status success/error dispatch use the selected response's codec. Raw text requires a string-like/binary schema. Octet-stream requires a binary schema, or the OpenAPI 3.1 spelling of one: JSON Schema 2020-12 alignment removed `format: binary`, so an empty (always-true) Schema Object — or no `schema` at all — says \"any octets\" and lowers to `bytes::Bytes` like any other binary body. A raw body cannot admit `null` — a `oneOf`/`anyOf` with `{ type: 'null' }`, or `type: [string, 'null']` — where it is sent or read verbatim: a request body under raw text, a `bytes::Bytes` request body under any media, and a `bytes::Bytes` response body, since raw content has no wire representation of `null` (a request body that may be omitted is `required: false`). A nullable raw text *response* decodes through serde and is generated as `Option<..>`. Media *ranges* are permitted `content` keys and name a family rather than a type: `text/*` is raw UTF-8 and every other family (`video/*`, `audio/*`, `*/*`, …) is opaque octets, ranked below every codec spargen has, so a concrete sibling outranks a family range. The structured-suffix ranges `application/*+json` and `application/*+json-seq` are read instead through the codec their suffix names and rank level with it, so on a response `application/*+json` ties `application/json` and source order decides between them (both decode as JSON); no other suffix range gets a suffix codec (`text/*+json` is raw text like any `text/` key, and `application/*+xml` is unsupported). A request body withholds a suffix range that classifies from the choice whenever a sibling it could send instead exists (a key that classifies, is no range, and is not streaming media) and reports it as the alternative not generated (`W014`). A concrete member of the `image`, `audio`, or `video` family (`image/jpeg`, `audio/mpeg`, `video/mp4`) is opaque octets as well, under the same schema gate, and on a response is ranked below every other key spargen can classify, ranges included, so it is generated only when it is the sole classifiable key — the shape where it was previously rejected; a request body naming one is sent with that exact `Content-Type`, and is preferred there to a range, which a request cannot send. The family rule reaches Encoding Objects too: a form-urlencoded property declaring such a `contentType` is binary and is rejected, as `application/octet-stream` is there, while on multipart a scalar or binary property stays a part carrying that header (any other property is rendered as JSON, so there the declaration is rejected, as below). A concrete type outside those families and `text/*` — `application/pdf`, `application/sdp`, `font/woff2` — names no codec spargen has and is rejected: `image/*` and `image/jpeg` are accepted where `application/pdf` is not, because the family is what says \"opaque octets\". A family member whose subtype carries a structured-syntax suffix (`image/svg+xml`) is still rejected: the suffix names a text syntax, so bytes would be the silently-wrong reading. A family range (`type/*` or `*/*`) is rejected as a *request* body media type when no concrete key beside it classifies, and a structured-suffix range when nothing beside it can be sent: `Content-Type` requires a concrete type/subtype, and a generated request sends its media key verbatim. XML uses the feature-gated quick-xml codec and is currently limited to an operation's single success or error body. Multipart requires an object request schema; form-urlencoded and multipart response bodies are rejected. Sequential responses with `itemSchema` (`text/event-stream`, JSON Lines/NDJSON, and JSON Text Sequences) generate a typed standard `EventStream<T>` when the stream is the operation's single bodied success response; an SSE envelope's JSON `data.contentSchema` becomes `T` directly. A bodied stream anywhere else — an error status, a `default` response (which documents error statuses too, even when it is the only response), or beside a second bodied success status — is rejected, since each of those positions decodes one whole body. Media Type Object `encoding` is implemented for `multipart/form-data` and `application/x-www-form-urlencoded`, with the specification's mode switch: any explicit `style`/`explode`/`allowReserved` selects RFC 6570 serialization (making `contentType` inert), otherwise the property is rendered by its `contentType`. Rejected are streaming requests, an explicit non-JSON `contentType` (`application/xml`, `text/csv`, or a type with no codec such as `application/yaml`) on a multipart property that is neither a scalar nor binary — an object, array, tuple, union, or `{}` — because such a part is always rendered as JSON and its bytes would contradict its header, a complete-sequence `schema` without `itemSchema`, `prefixEncoding`/`itemEncoding` (which describe positional parts of an array-shaped multipart body, where spargen generates from an object schema), and the RFC 6570 combinations the specification leaves undefined — `deepObject` or an object-valued property on multipart, and `spaceDelimited`/`pipeDelimited` with `explode: true`."
            }
            Code::UnsupportedParameterStyle => {
                "Path/header parameters support simple style and query/cookie parameters support form style, including the OpenAPI explode defaults and explicit explode overrides. OpenAPI 3.2 cookie style is emitted without percent encoding, and a single whole-query-string parameter supports JSON or application/x-www-form-urlencoded content. JSON content-typed parameters are generated. `deepObject`, `spaceDelimited`, and `pipeDelimited` query parameters and `allowReserved: true` are all supported. What is rejected is a style the parameter's location does not permit, `spaceDelimited`/`pipeDelimited` with `explode: true` (which the specification's own serialization table leaves undefined), a nested array or object inside a `simple`/`form` value, and a `querystring` parameter without exactly one JSON or form-urlencoded content entry with a schema — none of which could be serialized with defined wire semantics."
            }
            Code::ServerInitiatedFlowIgnored => {
                "Webhooks, callbacks, and links describe server-initiated or hypermedia behavior. They are acknowledged with a warning and no client code is emitted."
            }
            Code::InvalidInput => {
                "The input is malformed JSON/YAML or is missing a required OpenAPI structure needed before feature auditing can continue."
            }
            Code::DuplicateObjectKey => {
                "An object (JSON or YAML mapping) declares the same key more than once. Duplicate keys make the member ambiguous — a reader cannot tell which value wins, and downstream a duplicated `properties` name or schema keyword would resolve inconsistently — so spargen rejects the document at parse time and points at the second occurrence rather than silently keeping one. Remove or rename the duplicate key."
            }
            Code::UnknownSecurityScheme => {
                "Every scheme named in a `security` requirement must be declared under `components.securitySchemes` as `http` bearer/basic, `apiKey`, `oauth2`, or `openIdConnect` so credentials can be attached at the right location."
            }
            Code::AllOfIrreconcilable => {
                "Two constructs intersect schemas into one type and report this code when the result is empty or unrepresentable: the members of an `allOf`, and — because `$ref` is an applicator in JSON Schema 2020-12 rather than a replacement — a `$ref` together with its own shape-bearing sibling keywords, which are intersected with the referenced schema instead of being discarded. A sibling bears a shape of its own through `type`, `properties`, `patternProperties`, `enum`, `const`, `contentEncoding`, `format: binary`, or `allOf`. `additionalProperties`, `items`, and `prefixItems` refine a shape rather than establish one, so they take part as soon as the sibling also carries the `type` that gives them one. `required` is narrower still: it is consumed per property the sibling itself declares, so it takes part only beside the sibling's own `properties` and a `type: object` next to it is not enough — a `$ref` whose extra keywords are `type: object` and `required` is exactly its target, and the requirement is not carried into the generated type. A `$ref` whose siblings bear no shape is simply its target. Either way, object members flatten into a single struct (union of properties; a property required by any member is required; repeated properties recursively retain their narrower compatible intersection; `additionalProperties` is intersected conservatively), while scalar members narrow compatible primitives, enums, arrays, objects, unions, and nullability. Examples include integer within number, enum within its scalar type, and a detailed object within a broader object; an empty array-item intersection becomes an uninhabited item type so the valid empty array remains representable, and in the same way a repeated property whose types cannot meet becomes an uninhabited field when no member requires it — whether the members are `allOf` entries or a `$ref` and its siblings — so the objects that omit it remain representable. It is rejected only when the overall intersection is empty or cannot be represented faithfully: incompatible scalar categories (`{$ref: '#/components/schemas/Name', type: integer}` where `Name` is a string accepts no value at all), conflicting constraints on a property some member requires, conflicting additional-value constraints, an object/scalar mix, or a `$ref` that closes a reference cycle back to the schema enclosing it — an `allOf` member, a `$ref` carrying shape-bearing siblings, or either of those written as a `oneOf`/`anyOf` member — whose target's own definition depends on the result being computed, so it can be composed neither against itself nor by discarding it. For a `$ref` carrying shape-bearing siblings, and for a union member, that is a property of the document: the cycle is traced through every file, however each reference is spelled, so neither the order `components.schemas` is declared in nor the schema lowering happens to reach first can change the verdict. An `allOf` member is still refused only when its target is mid-lowering, so under mutual recursion its verdict can follow which schema lowering reaches first; where it generates, the target was complete and the merge is exact. Restructure the composition — or make the `$ref` target and its siblings agree — or omit this API segment with `spargen::omit!`."
            }
            Code::InvalidOmitRule => {
                "A compatibility omit rule must match at least one exact path, operation, component, pointer, or file-local pointer and cannot omit the document root."
            }
            Code::OmittedConstruct => {
                "A compatibility omit profile removed this construct before OpenAPI validation/lowering. The source schema on disk was not modified."
            }
            Code::OmitCreatedInvalidDocument => {
                "After applying omit rules, the remaining document is structurally invalid. Omit dependent consumers too, or fix the source schema."
            }
            Code::SchemaDefaultNotApplied => {
                "A `default` is applied as a serde deserialization default only when it is a single scalar (bool/integer/number/string) that matches the field's own scalar type or one of its enum variants. Object, array, null, heterogeneous, or type-mismatched defaults cannot be lowered to a Rust literal, so the value is recorded in the field's rustdoc but not wired — deserialization of an absent field yields `None` rather than the default."
            }
            Code::Oas32ConstructIgnored => {
                "OpenAPI 3.2 `itemSchema` describes one item of sequential media. On a non-sequential media type it does not define any wire behavior, so spargen acknowledges and ignores it while continuing to use the complete-body `schema`. Move the item schema to sequential media such as `application/x-ndjson`, `application/json-seq`, or `text/event-stream`, or use only `schema` for ordinary media."
            }
            Code::AlternativeMediaIgnored => {
                "A request body or response offered more than one media type. A generated method sends and decodes exactly one, so spargen picks the one it can represent best — JSON first, then XML, multipart, form-urlencoded, octet-stream, text, the sequential media, then the media *ranges* (`text/*`, then every other family such as `video/*` or `*/*`), and last of all a concrete `image`/`audio`/`video` type, which is ranked below every other key spargen can classify — breaking ties by source order. On a request body a family range (`type/*` or `*/*`) is considered only once no concrete key classifies, because a range cannot be sent as `Content-Type`, so there a concrete `image`/`audio`/`video` type is sent in preference to it. That is normally a real narrowing of the documented API surface: a server willing to accept XML as well as JSON will only ever be sent JSON by this client. It is reported rather than left silent so the choice is visible, and it is a warning rather than an error because the selected media type is genuinely supported and the generated client is correct for it. For the same reason it is reported only once the selected entry has passed every media-type check (`E009`) on its body: a selection one of them rejects generates nothing, so there is no narrowing to disclose, and the rejection is reported alone. One narrow case is not reported, because nothing is given up: on a *response* whose selected media is octet-classified — `application/octet-stream`, a non-textual media range such as `video/*`, or a concrete `image`/`audio`/`video` member such as `image/png` — an alternative that is also octet-classified and describes nothing of its own is `bytes::Bytes` just as the selection is, so generating one of them narrows nothing. Everything about that exemption is load-bearing. It is confined to octet-stream because that is the only codec whose gate collapses every body it admits onto one type; elsewhere two entries that both look empty can still differ — a media type with no `schema` lowers to `()` while `schema: {}` lowers to `serde_json::Value`, and a sequential media's item type lives in `itemSchema`, outside the body schema entirely. It is confined to responses because a request narrows at the wire whatever its type: the selected media key becomes the `Content-Type` verbatim, so a server documented as accepting two media is only ever sent one. And \"describes nothing\" is proved from the alternative's own Media Type Object rather than assumed from its media type — a validation keyword such as `maxLength`, an `itemSchema`, or an `encoding` all count as saying something. To generate against a different one, remove the alternatives from the document, or omit this API segment with `spargen::omit!` and hand-write the call."
            }
            Code::SchemaNestingTooDeep => {
                "Lowering a schema into a Rust type is recursive: each nested object property, array item, `allOf`/`oneOf`/`anyOf` member, and `$ref` target descends one level. Spargen caps that descent so a pathologically deep composition — a very long chain of components that each `$ref` the next, or a deeply nested inline schema — is rejected with this error instead of being allowed to exhaust the call stack and abort the process. A genuine API surface never approaches the limit; hitting it almost always means the spec was machine-generated or adversarial. Flatten the offending chain, or omit that API segment with `spargen::omit!`."
            }
            Code::RuntimeDependencyContract => {
                "The generated module is freestanding, so its consuming Cargo package must declare the crates and dependency features referenced by that specific API. Spargen derives the exact requirement set after lowering and audits Cargo.toml during build.rs and proc-macro generation. Use the documented tested lower bounds (or a higher semver-compatible caret floor), keep reqwest default features disabled, and enable only the capabilities the diagnostic names. A dependency declared `workspace = true` is followed to the workspace root's `[workspace.dependencies]` — the consumer manifest itself when it declares `[workspace]`, otherwise the root `package.workspace` names, otherwise the nearest ancestor manifest that parses and declares `[workspace]` — taking the version from there, the union of both feature lists, and default features on when the root leaves them on or the member sets `default-features = true` (a member's `default-features = false` cannot turn off defaults the root leaves on, so disable them in `[workspace.dependencies]` and leave the member's `default-features` unset or `false`), while `optional` is read from the member, as Cargo does. Inheriting a required crate therefore satisfies the audit; when a root is found but cannot be read, or declares no such entry, the diagnostic says which of those happened rather than reporting the crate as missing, and when no root is found at all it says that no workspace manifest was found, then names the nearest ancestor manifest that failed to read, if there was one, only as a possible root together with the reason it could not be read — never as the workspace manifest, because an unrelated broken `Cargo.toml` above the project can be that file. A root reached through `package.workspace` is read and reported in its own right, so its inheritance message carries no reason of its own and a read-failure diagnostic naming the same file stands above it. Cargo resolves the declared range; Rust compilation then verifies the selected crates expose the APIs and traits used by the generated client."
            }
            Code::SpecUndefinedBehavior => {
                "The OpenAPI Specification marks some constructs' behavior as *undefined* rather than leaving them merely unsupported. Currently this fires for a Path Item `$ref` declared alongside structural fields (operations, `parameters`, `servers`): the specification says that when a field appears both in the referring Path Item and the referenced one, the behavior is undefined. Unlike a Reference Object, which requires adjacent properties to be ignored, there is no rule to follow — so either choice (the local fields winning, or the referenced ones) silently discards operations the author wrote, and produces a client that calls a different set of endpoints than the document describes. `summary` and `description` are exempt because they are documentation and cannot change the wire. Move the sibling fields into the referenced Path Item, or drop the `$ref` and declare the item inline."
            }
            Code::TupleRestNotRepresentable => {
                "In JSON Schema 2020-12 `prefixItems` fixes the leading positions of an array and `items` describes every position after them. Spargen lowers `prefixItems` to a Rust tuple, which is fixed-length, so a schema that also allows a typed remainder describes a value no single Rust type expresses: a tuple cannot grow, and a `Vec` cannot hold the distinct per-position types. `items: false` closes the array at the prefix and is fully supported — that is exactly a tuple. To send a variable-length remainder, drop `prefixItems` and describe the whole array with `items`, split the fixed head into its own object properties, or omit this API segment with `spargen::omit!`."
            }
            Code::DeclarationHasNoEffect => {
                "The document declared something the specification permits here, but which cannot change any byte spargen generates or sends, so it is acknowledged rather than dropped in silence. It fires for: `allowReserved` on a parameter that is never percent-encoded (an `in: header` parameter, or `style: cookie`, both of which the specification sends verbatim); `encoding`, `prefixEncoding`, or `itemEncoding` on a media type that is neither `multipart` nor `application/x-www-form-urlencoded`, where the specification says those fields SHALL be ignored; an `encoding` entry naming a property the body schema does not declare; `encoding.headers` on a non-`multipart` media type; an `encoding.headers` Header Object that pins no `const`/`default` value, leaving a client nothing to send; `allowEmptyValue`, which is deprecated and cannot change what a typed client omits; a `mutualTLS` security scheme, which is satisfied by the transport's client certificate rather than by anything the client attaches; a response header named `Content-Type`, which the specification says SHALL be ignored; a response header whose `content` media type spargen cannot decode, or whose textual `content` schema is not a single value, or which declares `content` with no schema at all, none of which yields a typed accessor; a `servers` entry past the first on a path item or operation, where the specification defines no client selection rule; a union branch that the enclosing schema's own constraints have already made unsatisfiable; and a schema component declared inside a referenced sub-file whose name the root document also declares. That last one is a fact about the *document* rather than about one keyword: a JSON Pointer fragment addresses the file it is written in, so a sub-file's own `#/components/schemas/<name>` asks for that file's declaration — but spargen consults the root document's component map first, so where both declare the name the root's wins and the sub-file's is never read. The warning names both namespaces and fires once per reference site, because the site is what has to be found; address the file-local declaration explicitly with a relative-file reference, or rename one of the two, if the root's is not the one you meant. None of these is an error: the document is valid, and the construct simply has no reachable effect on this client."
            }
            Code::RuntimeAuditSkipped => {
                "Generated output is freestanding: the consuming package must itself declare the crates and dependency features that specific API needs. Spargen audits the consumer's `Cargo.toml` for that contract and reports any gap as `E023` — but only when it can find the manifest, which in practice means a real `build.rs` process, where Cargo puts the package in the environment. Generating from a test, a wrapper binary, or a script leaves nothing to audit, so the contract is unverified and a missing dependency surfaces later as a compile error in the generated module instead of a spargen diagnostic. Run `spargen deps <spec>` to print the exact `[dependencies]` block that spec requires, or generate from a build script so the audit runs automatically. Set `CargoIntegration::Off` if this generation is deliberately not part of a Cargo build."
            }
            Code::CargoIntegrationDegraded => {
                "`generate` was called outside a Cargo build-script process, so two things Cargo would otherwise do did not happen: no `cargo:rerun-if-changed` directives were emitted, meaning an edited spec will NOT trigger a rebuild and the checked-in module can silently go stale; and the consumer manifest could not be located, so the runtime-dependency audit (`E023`) was skipped. Neither is an error — generating outside a build script is a legitimate thing to do — but both are silent by nature, which is why they are reported. Call `generate` from a `build.rs` to get both, or declare the intent with `CargoIntegration::Off` to accept them silently."
            }
            Code::CargoIntegrationRequired => {
                "The caller set `CargoIntegration::Required`, declaring that this generation must be wired into Cargo — rebuild triggers emitted, consumer manifest audited — and it is not: either the process is not a build script, or no consumer manifest could be found. This is an error rather than a warning purely because the caller asked for it: `Required` exists for builds where a missed rebuild trigger would ship a client generated from a stale spec. Move the call into a `build.rs`, or relax to `CargoIntegration::Auto` (degrade with `W013`/`W012`) or `CargoIntegration::Off` (degrade silently)."
            }
            Code::XmlHintIgnored => {
                "XML request/response bodies honor the `xml.name` (element/attribute rename) and `xml.attribute` (serialize as an XML attribute via quick-xml's `@name` convention) hints on a field, but only for a schema used *exclusively* as an XML body. A serde `rename` is format-agnostic — it would also rewrite the JSON wire names — so `xml.name`/`xml.attribute` are NOT applied to a schema that is also reachable from a JSON/form/multipart/text body, a response, or a parameter (or that is not used as an XML body at all); the field keeps its normal wire name and this warning fires, so JSON is never corrupted. The `xml.namespace`, `xml.prefix`, and `xml.wrapped` (wrapped arrays) hints are never represented — quick-xml serde has no faithful mapping for them — so they are always ignored with this warning rather than silently honored or rejected."
            }
        }
    }

    /// The interpretation this code's behavior depends on, if any.
    pub fn interpretation(self) -> Option<InterpId> {
        match self {
            Code::UnsupportedOpenApiVersion => Some(InterpId(1)),
            Code::ValidationKeywordIgnored => Some(InterpId(2)),
            Code::NonDisjointUnion => Some(InterpId(3)),
            _ => None,
        }
    }

    /// Every code, in stable order — drives the exhaustiveness test and docs generation.
    pub fn all() -> &'static [Code] {
        const ALL: &[Code] = &[
            Code::UnsupportedOpenApiVersion,
            Code::UnsupportedDialect,
            Code::AbsoluteRefUnsupported,
            Code::UnresolvedRef,
            Code::VendoredRefDrift,
            Code::DuplicateObjectKey,
            Code::PatternPropertiesRejected,
            Code::DynamicRefRejected,
            Code::NonDisjointUnion,
            Code::NonScalarEnum,
            Code::UnsupportedMediaType,
            Code::UnsupportedParameterStyle,
            Code::InvalidInput,
            Code::UnknownSecurityScheme,
            Code::AllOfIrreconcilable,
            Code::InvalidOmitRule,
            Code::OmitCreatedInvalidDocument,
            Code::ValidationKeywordIgnored,
            Code::ServerInitiatedFlowIgnored,
            Code::OmittedConstruct,
            Code::SchemaDefaultNotApplied,
            Code::XmlHintIgnored,
            Code::Oas32ConstructIgnored,
            Code::AlternativeMediaIgnored,
            Code::SchemaNestingTooDeep,
            Code::RuntimeDependencyContract,
            Code::SpecUndefinedBehavior,
            Code::TupleRestNotRepresentable,
            Code::DeclarationHasNoEffect,
            Code::RuntimeAuditSkipped,
            Code::CargoIntegrationDegraded,
            Code::CargoIntegrationRequired,
        ];
        ALL
    }
}

impl Serialize for Code {
    /// Serializes as the stable `E###`/`W###` string, not the Rust variant name: the code string
    /// is the documented product surface, and the variant name is an implementation detail.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Code {
    type Err = UnknownCode;

    /// Parse a stable string form (`"E042"`) back into a [`Code`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Code::all()
            .iter()
            .copied()
            .find(|code| code.as_str() == s)
            .ok_or_else(|| UnknownCode(s.to_owned()))
    }
}

/// Error returned when a string does not name a known [`Code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCode(pub String);

impl std::fmt::Display for UnknownCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown diagnostic code: {}", self.0)
    }
}

impl std::error::Error for UnknownCode {}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::Code;

    /// Every code must appear in the published index, with the same title, and nothing may appear
    /// there that is not a real code. The index is product surface — `spargen explain` and
    /// `docs/errors.md` are the same contract — and without this the two drift silently.
    #[test]
    fn the_published_index_lists_exactly_the_declared_codes() {
        // A packaged `.crate` carries no docs directory, so the check is skipped there — gated on
        // the workspace marker, not on the file, so that inside the repository a missing or
        // renamed index fails instead of passing with a line on stderr nobody reads.
        let Some(root) = repo_root() else {
            eprintln!("skipping: not tested from the workspace");
            return;
        };
        let index = read_repo_document(&root, "docs/errors.md");
        let rows: Vec<(String, String)> = index
            .lines()
            .filter(|line| line.starts_with("| `E") || line.starts_with("| `W"))
            .map(|line| {
                let mut cells = line.split('|').map(str::trim);
                cells.next();
                let code = cells
                    .next()
                    .unwrap_or_default()
                    .trim_matches('`')
                    .to_owned();
                let _severity = cells.next();
                let title = cells.next().unwrap_or_default().to_owned();
                (code, title)
            })
            .collect();

        for code in Code::all() {
            let row = rows
                .iter()
                .find(|(listed, _)| listed == code.as_str())
                .unwrap_or_else(|| panic!("{} is missing from docs/errors.md", code.as_str()));
            // The index may elaborate ("… (3.1.x and 3.2.x are supported)") and may add markdown
            // code spans, but it must not describe a different thing than `spargen explain` does.
            assert!(
                row.1.replace('`', "").contains(code.title()),
                "docs/errors.md describes {} as `{}`, but its title is `{}`",
                code.as_str(),
                row.1,
                code.title()
            );
            assert!(
                !code.explain().is_empty(),
                "{} has no explain text",
                code.as_str()
            );
        }
        for (listed, _) in &rows {
            assert!(
                Code::all().iter().any(|code| code.as_str() == listed),
                "docs/errors.md lists `{listed}`, which is not a declared code"
            );
        }
    }

    /// Read every diagnostic code cited by a support document, paired with the 1-based index of
    /// the table column it sits in (`1` = Supported, `2` = Warned, `3` = Rejected).
    fn cited_codes(markdown: &str) -> Vec<(String, usize)> {
        let mut cited = Vec::new();
        for line in markdown.lines() {
            if !line.starts_with("| ") {
                continue;
            }
            // The leading `|` yields an empty first piece and the trailing one an empty last, so
            // the data cells are everything between.
            let cells: Vec<&str> = line.split('|').collect();
            for (index, cell) in cells.iter().enumerate().skip(1) {
                for token in cell.split('`') {
                    let token = token.trim();
                    let is_code = token.len() == 4
                        && matches!(token.as_bytes()[0], b'E' | b'W')
                        && token[1..].bytes().all(|byte| byte.is_ascii_digit());
                    if is_code {
                        cited.push((token.to_owned(), index));
                    }
                }
            }
        }
        cited
    }

    /// `docs/errors.md` is machine-checked above; the two documents that describe *what spargen
    /// does with a construct* were not, and drifted — a row claimed `prefixEncoding` was supported
    /// while the code rejected it, and `E014` was described nowhere at all. A prose claim is not
    /// checkable in general, but the code tokens in it are: every one must name a real code, every
    /// declared code must be placed somewhere in the matrix, and a code must not be filed under a
    /// column that disagrees with its own severity.
    ///
    /// That is all it holds. The prose of a cell is constrained by nothing here: a row rewritten
    /// to say the opposite of what the generator does keeps every assertion green, so the prose is
    /// reviewed by hand, as `docs/support-matrix.md` itself says. Where an `explain()` body is
    /// pinned (see its rustdoc), the matrix row defers to `spargen explain` rather than restating
    /// the body in words no test compares with it.
    #[test]
    fn the_support_documents_cite_the_codes_that_exist_where_they_belong() {
        // Skipped only from a packaged `.crate`, which carries no docs directory. In the
        // repository a missing or renamed support document fails rather than skipping the check.
        let Some(root) = repo_root() else {
            eprintln!("skipping: not tested from the workspace");
            return;
        };
        let matrix = read_repo_document(&root, "docs/support-matrix.md");
        let scope = read_repo_document(&root, "docs/openapi-3.2.md");

        let matrix_cited = cited_codes(&matrix);
        for (cited, _) in matrix_cited.iter().chain(cited_codes(&scope).iter()) {
            assert!(
                Code::from_str(cited).is_ok(),
                "a support document cites `{cited}`, which is not a declared code"
            );
        }

        // The matrix is the operational boundary: a construct spargen has an opinion about has a
        // cell describing it. Without this, adding a code and forgetting the matrix is invisible.
        for code in Code::all() {
            assert!(
                matrix_cited.iter().any(|(cited, _)| cited == code.as_str()),
                "{} is declared but appears nowhere in docs/support-matrix.md",
                code.as_str()
            );
        }

        // The matrix columns are Area | Supported | Warned | Rejected, so a code's home column is
        // 3 for a warning and 4 for a rejection. A code may *additionally* be named in the
        // Supported cell's prose — several are, explaining the boundary they sit on — but it must
        // appear in the column that matches its severity, or the table files it under an outcome
        // the generator does not produce.
        for code in Code::all() {
            let home = if code.as_str().starts_with('W') { 3 } else { 4 };
            let columns: Vec<usize> = matrix_cited
                .iter()
                .filter(|(cited, _)| cited == code.as_str())
                .map(|(_, column)| *column)
                .collect();
            assert!(
                columns.contains(&home),
                "docs/support-matrix.md cites {} only in column(s) {:?}, but a {} belongs in \
                 column {home} ({})",
                code.as_str(),
                columns,
                if home == 3 { "warning" } else { "rejection" },
                if home == 3 { "Warned" } else { "Rejected" }
            );
        }
    }

    #[test]
    fn all_codes_round_trip_from_stable_strings() {
        for code in Code::all() {
            assert_eq!(Code::from_str(code.as_str()).unwrap(), *code);
            match code.severity() {
                crate::diag::Severity::Error => assert!(code.as_str().starts_with('E')),
                crate::diag::Severity::Warning => assert!(code.as_str().starts_with('W')),
            }
        }
    }

    /// The Rust identifier of each variant. The match is exhaustive, so a variant added to `Code`
    /// does not compile until it is named here — which is what makes the two tests below closed
    /// over the whole enum rather than over whatever `all()` happens to list.
    fn variant_name(code: Code) -> &'static str {
        match code {
            Code::UnsupportedOpenApiVersion => "UnsupportedOpenApiVersion",
            Code::UnsupportedDialect => "UnsupportedDialect",
            Code::AbsoluteRefUnsupported => "AbsoluteRefUnsupported",
            Code::UnresolvedRef => "UnresolvedRef",
            Code::VendoredRefDrift => "VendoredRefDrift",
            Code::DuplicateObjectKey => "DuplicateObjectKey",
            Code::PatternPropertiesRejected => "PatternPropertiesRejected",
            Code::DynamicRefRejected => "DynamicRefRejected",
            Code::NonDisjointUnion => "NonDisjointUnion",
            Code::NonScalarEnum => "NonScalarEnum",
            Code::UnsupportedMediaType => "UnsupportedMediaType",
            Code::UnsupportedParameterStyle => "UnsupportedParameterStyle",
            Code::InvalidInput => "InvalidInput",
            Code::UnknownSecurityScheme => "UnknownSecurityScheme",
            Code::AllOfIrreconcilable => "AllOfIrreconcilable",
            Code::InvalidOmitRule => "InvalidOmitRule",
            Code::OmitCreatedInvalidDocument => "OmitCreatedInvalidDocument",
            Code::ValidationKeywordIgnored => "ValidationKeywordIgnored",
            Code::ServerInitiatedFlowIgnored => "ServerInitiatedFlowIgnored",
            Code::OmittedConstruct => "OmittedConstruct",
            Code::SchemaDefaultNotApplied => "SchemaDefaultNotApplied",
            Code::XmlHintIgnored => "XmlHintIgnored",
            Code::Oas32ConstructIgnored => "Oas32ConstructIgnored",
            Code::AlternativeMediaIgnored => "AlternativeMediaIgnored",
            Code::SchemaNestingTooDeep => "SchemaNestingTooDeep",
            Code::RuntimeDependencyContract => "RuntimeDependencyContract",
            Code::SpecUndefinedBehavior => "SpecUndefinedBehavior",
            Code::TupleRestNotRepresentable => "TupleRestNotRepresentable",
            Code::DeclarationHasNoEffect => "DeclarationHasNoEffect",
            Code::RuntimeAuditSkipped => "RuntimeAuditSkipped",
            Code::CargoIntegrationDegraded => "CargoIntegrationDegraded",
            Code::CargoIntegrationRequired => "CargoIntegrationRequired",
        }
    }

    /// Does `haystack` name the path `Code::<variant>` as a whole path segment? A plain
    /// `contains` would accept `Code::UnresolvedRefTypo` as evidence for `Code::UnresolvedRef`,
    /// so every occurrence must be followed by a non-identifier character.
    fn mentions_variant(haystack: &str, variant: &str) -> bool {
        let needle = format!("Code::{variant}");
        haystack.match_indices(&needle).any(|(at, _)| {
            haystack[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_alphanumeric() && next != '_')
        })
    }

    /// The workspace root, or `None` when this crate is tested from a packaged `.crate`, which
    /// carries neither the workspace manifest nor the test suites. Gating on the *workspace
    /// marker* rather than on the file under inspection is deliberate: inside the repository a
    /// missing `tests/` file must fail the test, not silently skip it.
    fn repo_root() -> Option<std::path::PathBuf> {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).to_path_buf();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
        manifest.contains("[workspace]").then_some(root)
    }

    /// Read a document the repository must carry, failing — never skipping — when it is absent.
    /// Only call this under [`repo_root`], which is what decides whether the file must exist.
    fn read_repo_document(root: &std::path::Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative)).unwrap_or_else(|error| {
            panic!("{relative} must exist in the repository, and reading it failed: {error}")
        })
    }

    /// `all()` is a hand-written `const ALL`, and every docs/behavior test iterates it — so a
    /// variant added to the enum but forgotten there would be invisible to all of them. Adding a
    /// variant first fails to compile in `variant_name`; once named, `DECLARED` no longer matches
    /// and this fails until `all()` lists it too.
    #[test]
    fn all_lists_every_declared_variant() {
        const DECLARED: usize = 32;

        assert_eq!(
            Code::all().len(),
            DECLARED,
            "Code::all() lists {} codes but {DECLARED} variants are declared — a variant reached \
             `Code` and `variant_name` without reaching `ALL`",
            Code::all().len()
        );

        let variants: std::collections::BTreeSet<&str> =
            Code::all().iter().map(|code| variant_name(*code)).collect();
        assert_eq!(
            variants.len(),
            DECLARED,
            "Code::all() lists the same variant twice"
        );

        let strings: std::collections::BTreeSet<&str> =
            Code::all().iter().map(|code| code.as_str()).collect();
        assert_eq!(
            strings.len(),
            DECLARED,
            "two variants share a code string, so `from_str` cannot round-trip both"
        );
    }

    /// Titles and explain text are product surface reached by `spargen explain`, independently of
    /// whether the docs tree is present. The assertions on them inside
    /// `the_published_index_lists_exactly_the_declared_codes` sit *after* its early return, so a
    /// packaged build skips them; these always run. The title check there is a `contains` against
    /// the docs row, which an empty title would satisfy trivially — this is what rules that out.
    #[test]
    fn every_code_has_title_and_explain_text() {
        for code in Code::all() {
            assert!(!code.title().is_empty(), "{} has no title", code.as_str());
            assert!(
                !code.explain().is_empty(),
                "{} has no explain text",
                code.as_str()
            );
        }
    }

    /// One case an enumerating explain body lists: the marker tag its emission sites carry, the
    /// phrase of the body that states it, and — where the case has one — the message wording that
    /// is reserved to it: every site of the case says it, and no other site of the code does.
    struct Case {
        tag: &'static str,
        stated_as: &'static str,
        reserved_wording: Option<&'static str>,
    }

    /// The codes whose explain body enumerates the cases that reach them. See [`Code::explain`].
    const ENUMERATED_CASES: &[(Code, &[Case])] = &[(
        Code::UnresolvedRef,
        &[
            Case {
                tag: "absent-target",
                stated_as: "the target is absent from the loaded input bundle",
                reserved_wording: Some("not found in the input bundle"),
            },
            Case {
                tag: "undeclared-component",
                stated_as: "a local component reference names an entry the document does not \
                            declare",
                reserved_wording: None,
            },
            Case {
                tag: "cycle",
                stated_as: "a chain of reference hops closes into a cycle",
                reserved_wording: Some("cycle"),
            },
            Case {
                tag: "declined-hop",
                stated_as: "a hop resolves but spargen declines to follow it",
                reserved_wording: None,
            },
            Case {
                tag: "resource-scope",
                stated_as: "static `$id`/`$anchor` schema resource scopes",
                reserved_wording: Some("`$id`/`$anchor`"),
            },
            Case {
                tag: "unsupported-or-unresolved",
                stated_as: "a reference reported as `unsupported or unresolved` is one spargen \
                            could not resolve *and* could not tell an absent target from a \
                            fragment form it declines to follow",
                reserved_wording: Some("unsupported or unresolved"),
            },
        ],
    )];

    /// How far above a `Code::<Variant>` line its case marker may sit: far enough for a
    /// `Diagnostic::error(` that rustfmt breaks before its first argument, near enough that a
    /// marker cannot drift onto an unrelated site.
    const MARKER_REACH: usize = 3;

    /// The `(code, tags)` a `// E### case: a, b` marker line declares, if it is one.
    fn case_marker(line: &str) -> Option<(&str, Vec<&str>)> {
        let rest = line.trim().strip_prefix("// ")?;
        let (code, tags) = rest.split_once(" case: ")?;
        let is_code = code.len() == 4
            && matches!(code.as_bytes()[0], b'E' | b'W')
            && code[1..].bytes().all(|byte| byte.is_ascii_digit());
        is_code.then(|| (code, tags.split(',').map(str::trim).collect()))
    }

    /// Every `.rs` file under `dir`, recursively, in a stable order.
    fn rust_sources(dir: &std::path::Path, into: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                rust_sources(&path, into);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                into.push(path);
            }
        }
    }

    /// `spargen explain` prints an explain body verbatim, and a body that lists the cases reaching
    /// its code makes a claim about the generator a test can check: that the list is exhaustive.
    /// `E004`'s once was not — a security scheme alias miss and a chained Path Item `$ref` reached
    /// the code while the body listed neither — and the only assertion on it was `!is_empty()`.
    ///
    /// So every occurrence of such a code in the crate's sources (outside `diag`, which declares
    /// it) must carry a case marker within [`MARKER_REACH`] lines above it, every tag a marker
    /// names must be a case of that code, every case must have a site and be stated in the body,
    /// and a case's reserved wording must appear in exactly its own sites — from the marker to the
    /// `.emit(` that ends the diagnostic. A new emission site fails until someone decides which
    /// listed case it is, or extends the list; a case deleted from the body fails too.
    #[test]
    fn every_emission_site_falls_into_a_case_its_explain_text_lists() {
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut files = Vec::new();
        rust_sources(src, &mut files);
        let diag = src.join("diag");

        let mut failures = Vec::new();
        let mut sites: std::collections::BTreeMap<(&str, &str), usize> = Default::default();
        for path in files.iter().filter(|path| !path.starts_with(&diag)) {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
            let lines: Vec<&str> = text.lines().collect();
            let shown = path.strip_prefix(src).unwrap_or(path).display().to_string();

            // Every marker must name an enumerating code and sit above a site of it.
            for (at, line) in lines.iter().enumerate() {
                let Some((code, tags)) = case_marker(line) else {
                    continue;
                };
                let Some((declared, cases)) = ENUMERATED_CASES
                    .iter()
                    .find(|(declared, _)| declared.as_str() == code)
                else {
                    failures.push(format!(
                        "src/{shown}:{}: a `{code} case:` marker, but {code} is not in \
                         ENUMERATED_CASES",
                        at + 1
                    ));
                    continue;
                };
                let variant = variant_name(*declared);
                let attached = lines[at + 1..]
                    .iter()
                    .take(MARKER_REACH)
                    .any(|below| mentions_variant(below, variant));
                if !attached {
                    failures.push(format!(
                        "src/{shown}:{}: a `{code} case:` marker with no `Code::{variant}` in the \
                         {MARKER_REACH} lines below it",
                        at + 1
                    ));
                }
                let region: String = lines[at..]
                    .iter()
                    .take_while({
                        let mut done = false;
                        move |line| !std::mem::replace(&mut done, line.contains(".emit("))
                    })
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                for tag in &tags {
                    match cases.iter().find(|case| case.tag == *tag) {
                        Some(case) => *sites.entry((declared.as_str(), case.tag)).or_default() += 1,
                        None => failures.push(format!(
                            "src/{shown}:{}: {code} has no case `{tag}`; its explain text lists \
                             {:?}",
                            at + 1,
                            cases.iter().map(|case| case.tag).collect::<Vec<_>>()
                        )),
                    }
                }
                for case in cases.iter() {
                    let Some(wording) = case.reserved_wording else {
                        continue;
                    };
                    let tagged = tags.contains(&case.tag);
                    let says = region.contains(wording);
                    if tagged != says {
                        failures.push(format!(
                            "src/{shown}:{}: a {code} site {} `{}` but {} {wording:?}, the wording \
                             reserved to that case",
                            at + 1,
                            if tagged { "tagged" } else { "not tagged" },
                            case.tag,
                            if says { "says" } else { "does not say" },
                        ));
                    }
                }
            }

            // Every site of an enumerating code must carry a marker for it.
            for (code, _) in ENUMERATED_CASES {
                let variant = variant_name(*code);
                for (at, line) in lines.iter().enumerate() {
                    if !mentions_variant(line, variant) {
                        continue;
                    }
                    let marked = lines[at.saturating_sub(MARKER_REACH)..at]
                        .iter()
                        .any(|above| case_marker(above).is_some_and(|(c, _)| c == code.as_str()));
                    if !marked {
                        failures.push(format!(
                            "src/{shown}:{}: `Code::{variant}` with no `// {code} case: <case>` \
                             marker in the {MARKER_REACH} lines above it — decide which case of \
                             `spargen explain {code}` this site is, or add one to the explain text \
                             and to ENUMERATED_CASES",
                            at + 1
                        ));
                    }
                }
            }
        }

        for (code, cases) in ENUMERATED_CASES {
            for case in cases.iter() {
                if !code.explain().contains(case.stated_as) {
                    failures.push(format!(
                        "`spargen explain {code}` no longer states case `{}`: {:?}",
                        case.tag, case.stated_as
                    ));
                }
                if !sites.contains_key(&(code.as_str(), case.tag)) {
                    failures.push(format!(
                        "{code} case `{}` has no emission site; drop it from the explain text and \
                         from ENUMERATED_CASES, or mark the site that reports it",
                        case.tag
                    ));
                }
            }
        }

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// CLAUDE.md: every code gets "a fixture in `spargen/tests/frontend.rs`", enforced by tests
    /// rather than convention. Frontend codes are asserted there; the seven the frontend cannot
    /// produce — the `compat` omit rules and the facade's own Cargo-integration and
    /// runtime-audit diagnostics — are asserted in the suite that *can* produce them, and each
    /// must say so here. A new code that lands in neither place fails, which is the point.
    #[test]
    fn every_code_is_asserted_by_the_suite_that_owns_it() {
        const OWNED_ELSEWHERE: &[(&str, &str)] = &[
            // `compat` rules: the frontend never sees an omit rule.
            ("InvalidOmitRule", "carve.rs"),
            ("OmitCreatedInvalidDocument", "carve.rs"),
            ("OmittedConstruct", "carve.rs"),
            // The runtime-dependency contract needs a real consumer manifest to audit.
            ("RuntimeDependencyContract", "e2e.rs"),
            ("RuntimeAuditSkipped", "e2e.rs"),
            // The Cargo-integration policy is a property of the build environment, not of the
            // spec; `frontend.rs` deliberately runs every fixture with the integration off.
            ("CargoIntegrationDegraded", "config.rs"),
            ("CargoIntegrationRequired", "config.rs"),
        ];

        let Some(root) = repo_root() else {
            return;
        };
        let tests = root.join("spargen/tests");
        let read = |name: &str| {
            std::fs::read_to_string(tests.join(name))
                .unwrap_or_else(|error| panic!("spargen/tests/{name} must be readable: {error}"))
        };

        let frontend = read("frontend.rs");
        for code in Code::all() {
            let variant = variant_name(*code);
            match OWNED_ELSEWHERE.iter().find(|(owned, _)| *owned == variant) {
                Some((_, suite)) => {
                    assert!(
                        mentions_variant(&read(suite), variant),
                        "{} is declared to be asserted in {suite}, but `Code::{variant}` appears \
                         nowhere in it",
                        code.as_str()
                    );
                    assert!(
                        !mentions_variant(&frontend, variant),
                        "{} is listed in OWNED_ELSEWHERE but frontend.rs now asserts it too — \
                         drop the entry so one suite owns it",
                        code.as_str()
                    );
                }
                None => assert!(
                    mentions_variant(&frontend, variant),
                    "{} has no fixture: `Code::{variant}` appears nowhere in \
                     spargen/tests/frontend.rs. Add one, or add the code to OWNED_ELSEWHERE \
                     naming the suite that asserts it.",
                    code.as_str()
                ),
            }
        }

        for (variant, _) in OWNED_ELSEWHERE {
            assert!(
                Code::all()
                    .iter()
                    .any(|code| variant_name(*code) == *variant),
                "OWNED_ELSEWHERE names `{variant}`, which is not a declared Code variant"
            );
        }
    }

    /// A clause of an `explain()` body whose **behaviour** is enforced by fixtures in another
    /// module, recorded so that the connection between the prose and those fixtures is written
    /// down and checkable, rather than re-derived by whoever notices the clause is unasserted.
    struct ExplainClauseOwner {
        code: Code,
        /// Verbatim text of the body; it must occur there exactly once.
        clause: &'static str,
        /// The module whose `#[test]`s enforce the clause, and its source.
        module: (&'static str, &'static str),
        /// Names of `#[test]` functions in `module` that fail when the behaviour breaks. Where they
        /// establish less than the clause says, a comment on the row states the gap.
        fixtures: &'static [&'static str],
    }

    const RUNTIME_CONTRACT: (&str, &str) = (
        "runtime_contract.rs",
        include_str!("../runtime_contract.rs"),
    );

    /// The sibling of `OWNED_ELSEWHERE` for explain **prose**. `OWNED_ELSEWHERE` records which
    /// suite asserts that a code is *emitted*; this records which fixtures enforce what a body
    /// *says*, for clauses no test asserts as text.
    ///
    /// `E023`'s body is pinned byte for byte by
    /// `runtime_contract::tests::the_e023_explain_text_states_the_inheritance_rules_this_module_enforces`,
    /// which additionally cites a fixture for each sentence stating what the resolver does, and by
    /// its own rule does not cite fixtures for sentences that give advice. The consumer-obligations
    /// clauses below fall outside that rule — most read as advice — yet each is enforced by an
    /// existing fixture, and one (reqwest's default features) is a named `E023` trigger in the
    /// support matrix's rejected column, not advice at all. This table is where that ownership is
    /// recorded, so the byte-for-byte test's rule need not change.
    ///
    /// What the check is worth, precisely: it catches a clause edited or deleted from the body and
    /// a fixture renamed or removed. It does not verify that a cited fixture has anything to do
    /// with the clause citing it; that is a reviewer's reading, and holding prose to behaviour in
    /// general is #137.
    const EXPLAIN_CLAUSES_OWNED_ELSEWHERE: &[ExplainClauseOwner] = &[
        ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            clause: "its consuming Cargo package must declare the crates and dependency features \
                     referenced by that specific API",
            module: RUNTIME_CONTRACT,
            fixtures: &[
                // Each missing crate or feature is reported, and only when the API uses it.
                "conditional_dependencies_and_features_are_required_only_when_used",
                "reqwest_defaults_and_blocking_wiring_are_part_of_the_contract",
            ],
        },
        ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            clause: "Use the documented tested lower bounds",
            module: RUNTIME_CONTRACT,
            fixtures: &["a_requirement_that_admits_a_version_below_the_floor_is_rejected"],
        },
        ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            clause: "(or a higher semver-compatible caret floor)",
            module: RUNTIME_CONTRACT,
            fixtures: &["exact_floors_and_higher_compatible_caret_requirements_are_supported"],
        },
        ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            clause: "keep reqwest default features disabled",
            module: RUNTIME_CONTRACT,
            fixtures: &["reqwest_defaults_and_blocking_wiring_are_part_of_the_contract"],
        },
        ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            // A rule, not a list: the body once enumerated "reqwest/bytes/XML/UUID/time" and so
            // omitted `futures-core` and tokio's `rt`, which the diagnostic also names (#172).
            // Whatever the requirement table demands, the diagnostic names, so this cannot go
            // stale as the table grows.
            clause: "enable only the capabilities the diagnostic names",
            module: RUNTIME_CONTRACT,
            // Not covered: these pin that the diagnostic names exactly the capabilities the API
            // uses, including `futures-core` for streams and tokio for `blocking` (and never
            // `serde` on `time`). The audit does not reject a capability enabled beyond that set,
            // so "only" is advice, and no fixture enforces it.
            fixtures: &[
                "conditional_dependencies_and_features_are_required_only_when_used",
                "reqwest_defaults_and_blocking_wiring_are_part_of_the_contract",
                "the_time_requirement_never_asks_for_serde",
            ],
        },
    ];

    /// Every way `owner` has gone stale: its clause no longer occurs exactly once in the body, it
    /// cites no fixture, or a fixture it cites is not a `#[test]` in its module.
    fn explain_clause_owner_failures(owner: &ExplainClauseOwner) -> Vec<String> {
        let mut failures = Vec::new();
        let code = owner.code;
        let occurrences = code.explain().matches(owner.clause).count();
        if occurrences != 1 {
            failures.push(format!(
                "`spargen explain {code}` says {:?} {occurrences} times, expected exactly once; \
                 bring EXPLAIN_CLAUSES_OWNED_ELSEWHERE in line with the body",
                owner.clause
            ));
        }
        if owner.fixtures.is_empty() {
            failures.push(format!("no fixture cited for {code}'s {:?}", owner.clause));
        }
        let (module, source) = owner.module;
        for fixture in owner.fixtures {
            if !crate::diag::is_test_fn(source, fixture) {
                failures.push(format!(
                    "{code}'s {:?} names `{fixture}` as a fixture that enforces it, and no \
                     `#[test]` of that name exists in {module}",
                    owner.clause
                ));
            }
        }
        failures
    }

    #[test]
    fn explain_clauses_owned_elsewhere_resolve_to_fixtures_that_exist() {
        let failures: Vec<String> = EXPLAIN_CLAUSES_OWNED_ELSEWHERE
            .iter()
            .flat_map(explain_clause_owner_failures)
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The check's own falsifiers: a row whose clause is not in the body, whose fixture is a
    /// helper rather than a `#[test]`, whose fixture does not exist, or that cites nothing, must
    /// each be refused.
    #[test]
    fn the_explain_ownership_check_refuses_a_stale_clause_or_fixture() {
        let row = |clause, fixtures| ExplainClauseOwner {
            code: Code::RuntimeDependencyContract,
            clause,
            module: RUNTIME_CONTRACT,
            fixtures,
        };
        const LIVE: &[&str] = &["reqwest_defaults_and_blocking_wiring_are_part_of_the_contract"];
        let clause = "keep reqwest default features disabled";

        assert!(explain_clause_owner_failures(&row(clause, LIVE)).is_empty());
        for stale in [
            row("keep reqwest default features enabled", LIVE),
            row(clause, &["replace_once"]),
            row(clause, &["no_such_fixture_exists"]),
            row(clause, &[]),
        ] {
            assert_eq!(
                explain_clause_owner_failures(&stale).len(),
                1,
                "{:?} citing {:?}",
                stale.clause,
                stale.fixtures
            );
        }
    }
}
