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
    /// `spargen lock` could not fetch a remote `$ref` it had to vendor: the request failed in
    /// transport (DNS, refused connection, TLS, timeout) or the server answered an error status.
    RemoteFetchFailed,
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
    /// A media type, or a use of one, that spargen cannot generate faithfully; `spargen explain
    /// E009` lists the media that are generated and the cases that reach this code.
    UnsupportedMediaType,
    /// A parameter serialization with no defined wire form spargen can generate; `spargen explain
    /// E010` lists the supported styles and the cases that reach this code (matrix: Parameters →
    /// R).
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
    /// The input could not be parsed or violates a required structural OpenAPI shape, or the
    /// generation pipeline itself failed (reading or writing a file, inputs that kept changing
    /// while read, or an internal invariant of the lowered model).
    InvalidInput,
    /// An object declares the same key twice; the duplicate makes the member ambiguous, so it is
    /// rejected rather than silently collapsed to one occurrence.
    DuplicateObjectKey,
    /// A compatibility omit rule did not match a source construct or attempted an invalid removal.
    InvalidOmitRule,
    /// An omit rule removed a construct: one written in a compatibility omit profile, or one
    /// auto-carve derived to cut away a rejected construct.
    OmittedConstruct,
    /// Applying the omit rules removed a required OpenAPI root field (`openapi`, `info`) or left
    /// none of `paths`, `components`, and `webhooks`.
    OmitCreatedInvalidDocument,
    /// A schema `default` value could not be applied as a deserialization default (it is not a
    /// scalar matching the field's type); it is documented in rustdoc but not wired (matrix: Schema
    /// shape → W).
    SchemaDefaultNotApplied,
    /// An XML representation hint was not applied, in one of two cases: `xml.name`/`xml.attribute`
    /// on a schema not used exclusively as an XML body, withheld because a serde rename would also
    /// rewrite the non-XML wire (on a schema an XML body also reaches, withholding it changes the
    /// XML wire); or an unsupported hint (`xml.namespace`, `xml.prefix`, `xml.wrapped`, or an
    /// unsupported `xml.nodeType`) on a type never serialized as XML, where it changes no wire. An
    /// unsupported hint on a type that *is* serialized as XML is `E009` instead (matrix: Media → W).
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
    /// Generation ran in a Cargo build script that located no consumer Cargo manifest, so the
    /// generated runtime's dependency contract (`E023`) could not be audited.
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
            Code::RemoteFetchFailed => "E025",
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
            Code::RemoteFetchFailed => "remote $ref fetch failed",
            Code::ValidationKeywordIgnored => "validation-only keyword ignored",
            Code::PatternPropertiesRejected => "patternProperties not representable as a typed map",
            Code::DynamicRefRejected => "dynamic reference unsupported",
            Code::NonDisjointUnion => "union cannot be represented as a generated enum",
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
            Code::XmlHintIgnored => "XML hint not applied",
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

    /// Extended documentation shown by `spargen explain E###`. The published errors index,
    /// `docs/errors.md`, lists each code's severity and title; this body is the explanation.
    ///
    /// Every body is published, user-facing text, but not every body is held to the code by a
    /// test, and a contributor editing one should know which kind they are editing:
    ///
    /// - A body that **enumerates the cases reaching its code** is listed in this module's
    ///   `ENUMERATED_CASES` test table. Each case names a phrase the body must contain, and every
    ///   emission site of the code carries a `// E### case: <case>` (or `// W### case: <case>`)
    ///   marker naming the cases it
    ///   reports, so an emission site outside the enumeration, a case with no emission site, or a
    ///   case deleted from the body fails `every_emission_site_falls_into_a_case_its_explain_text_lists`.
    ///   A case may also reserve a message wording, which its sites must use and no other site of
    ///   the code may. Adding an enumerating body means adding it to that table.
    /// - `E023`'s body is pinned byte-for-byte, clause by clause, by
    ///   `runtime_contract::tests::explain::the_e023_explain_text_states_the_inheritance_rules_this_module_enforces`.
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
                "`jsonSchemaDialect`, when present, must be the OAS base dialect (`https://spec.openapis.org/oas/3.1/dialect/base`). The OpenAPI 3.2 text deliberately retains that URI and defines no dialect identifier of its own, but 3.2's published document schema gives `https://spec.openapis.org/oas/3.2/dialect/2025-09-17` as the field's default, so a 3.2 document may use either spelling. Other dialects are permitted by OpenAPI but optional for tooling, and spargen rejects them because their keywords cannot be lowered under the compile-time-correctness contract. The same rule applies to a Schema Object's own `$schema`, which names a dialect outright: any value other than those two URIs is rejected, and because a Schema Object is read without the document's version, either URI is accepted there on a 3.1 or a 3.2 document alike."
            }
            Code::AbsoluteRefUnsupported => {
                "Remote (`http`/`https`) `$ref` resolution is hermetic: `generate` and `check` never touch the network. A remote ref is resolved only from a locally vendored copy whose bytes are hash-pinned in `spargen.lock`. This error fires when a remote ref is not yet pinned there (or names an unfetchable absolute-URI scheme such as `urn:`). A `$ref` is an absolute URI whenever it opens with an RFC 3986 scheme — a letter, then letters, digits, `+`, `-` or `.`, then `:` — so `git+https:` is one, and so is a one-letter scheme: a Windows drive-letter path such as `C:/specs/x.yaml` is a URI with scheme `c`, not a file path, and is rejected here. Reference a local file by a path relative to the referring document instead, and write a relative path whose first segment holds a `:` as `./a:b.yaml`. Run `spargen lock <spec>` to fetch, vendor under `.spargen/vendor/`, and pin it — then `generate`/`check` resolve it offline. Alternatively, vendor the document by hand and reference it with a relative file path."
            }
            Code::UnresolvedRef => {
                "Something spargen had to resolve — a reference, or the schema scope a reference would be resolved against — could not be followed to a usable target, so the construct that named it is rejected rather than generated without it. The target need not be a schema: Path Item, Parameter, Request Body, Response, Header, Media Type and Security Scheme references all report here. The cases that reach this code are: the target is absent from the loaded input bundle (check the file path and the JSON Pointer fragment); a local component reference names an entry the document does not declare, under `schemas`, `parameters`, `requestBodies`, `responses`, `headers`, `mediaTypes`, `securitySchemes` or `pathItems` — including one an omit profile removed, where auto-carve is the intended escape; a `discriminator.mapping` or `defaultMapping` value, a schema name or URI reference, names a schema the loaded description does not hold, whether the discriminator sits beside a union or on a schema with none, reported at that entry; a chain of reference hops closes into a cycle; a hop resolves but spargen declines to follow it — a Path Item `$ref` whose target is itself a Path Item `$ref`, a security scheme alias pointing at another alias, or a fragment form the resolver does not walk; or static `$id`/`$anchor` schema resource scopes, where scope-aware resolution is what is missing rather than a target. Each diagnostic's message is more specific than this list, with one exception: a reference reported as `unsupported or unresolved` is one spargen could not resolve *and* could not tell an absent target from a fragment form it declines to follow."
            }
            Code::VendoredRefDrift => {
                "A remote `$ref` is pinned in `spargen.lock`, but its vendored copy under `.spargen/vendor/` is missing or its bytes no longer match the pinned sha256. The lock is the source of truth, so the drifted content is refused rather than used silently. Re-run `spargen lock <spec>` to re-vendor and re-pin, or restore the vendored file to its pinned bytes."
            }
            Code::RemoteFetchFailed => {
                "`spargen lock` is the only step that touches the network, and this error means it could not fetch a remote (`http`/`https`) `$ref` it had to vendor: the request failed in transport (the name did not resolve, the connection was refused or timed out, a proxy or the TLS handshake rejected it), or the server answered with an error status. The message names the URL and carries the underlying error. It says nothing about the document itself — the `$ref` is well-formed and is what `spargen lock` exists to pin — so the fix is on the network side: check the URL and the status or transport error the message reports, then re-run `spargen lock <spec>`. The failed URL is not vendored again, and the run is rejected; the lock it still writes keeps the pin the earlier `spargen.lock` held for that URL, and for any document reachable only through it, beside the vendored copy that pin names, so an earlier working lock stays usable offline. If the document cannot be reached from where the lock step runs, vendor it by hand and reference it with a relative file path. `generate` and `check` never fetch, so they never report this code."
            }
            Code::ValidationKeywordIgnored => {
                "The keyword affects runtime validation but not the static Rust shape. Spargen records a warning and generates the shape. That includes OpenAPI 3.0's boolean `exclusiveMinimum`/`exclusiveMaximum` written in a 3.1 or 3.2 document, where JSON Schema 2020-12 expects a number: under either reading it constrains only validation. It is also reported for a `oneOf` decoded by trial matching that keeps a variant lowering to `serde_json::Value` (an untyped member such as `{}`) beside others: that variant accepts every value, so the other branches act only as validation, excluding the values they accept from it. They decode no value of their own, and serializing one fails the exactly-one rule; give the untyped branch a type, or use `anyOf`. It is reported, too, where union branches lower to the same generated type, or to identically structured ones, differing only in keywords that type does not carry — whether the branches are written that way or an intersection with keywords beside the union leaves them so: they become one type, or one variant of the generated enum, so which of them a value matches is not enforced; keep producer-side validation for the branch constraints. OpenAPI 3.2 `contentMediaType`/`contentSchema` are consumed without this warning only on the string `data` property of a sequential `text/event-stream` item envelope, where they define the JSON payload type."
            }
            Code::PatternPropertiesRejected => {
                "`patternProperties` is represented as a typed overflow map (`#[serde(flatten)]`) when every pattern value schema — and any typed `additionalProperties` value — lowers to the same emitted Rust type; the key regex itself is validation-only and reported as `W001`. Value schemas that are distinct definitions still share one map when they emit one Rust type: two `{type: string}` values are both `String`, and two integer `enum`s, or an integer `enum` beside `format: int64`, are all `i64`. A struct, string enum, union, uninhabited type (the `false` schema), or array or tuple that closes a `$ref` cycle is its own Rust item, so two distinct ones are two types even where their shapes agree. It is rejected only when a faithful map is impossible: value schemas that emit different Rust types (which one map cannot type), or a combination with `additionalProperties: false` (a flatten map cannot both capture pattern values and deny other unknown keys)."
            }
            Code::DynamicRefRejected => {
                "`$dynamicRef` and `$dynamicAnchor` require dynamic schema-scope evaluation and are rejected."
            }
            Code::NonDisjointUnion => {
                "`oneOf`/`anyOf` unions are lowered to typed Rust enums with custom `Deserialize`/`Serialize` — never `serde(untagged)` and never degraded to `serde_json::Value`. Fast paths dispatch by discriminator tag, a unique non-object JSON category, or a proven disjoint category/required key. Overlapping variants use typed trial matching over one buffered value: `oneOf` requires exactly one successful variant; `anyOf` deterministically selects the most specific successful variant (enum before broad scalar, integer before number, more-required object before broader object, recursive array specificity, then source order), and serialization revalidates the same rule. Shape constraints adjacent to the union are intersected into every branch — untyped object and array keywords only into the branches of their own category (see `E013`) — and a branch they exclude is dropped with `W011` while the rest of the enum stands. This error is reported for a union that cannot be turned into a generated enum, which happens in three ways. One is an applicator combination that is not yet representable, such as declaring both `oneOf` and `anyOf` on the same schema node, or a `discriminator.mapping` entry or OpenAPI 3.2 `discriminator.defaultMapping` naming a schema that exists but is not one of the union's `$ref` members, so the tag it describes has no variant to decode into and the fallback has no branch to fall back to — the specification requires every possible schema to be listed beside the discriminator — or a `$ref` object member that no discriminator value selects, because no `mapping` key names it and a key equal to its component name claims that value for another member, so the decoder has no arm for it and the tag it would serialize decodes as the other member (a member `defaultMapping` names is still reached by its fallback). The second is a union that resolves to *itself*: a member that is a `$ref` back to the very union being lowered, or a union that is a schema's whole body whose only non-null member is a `$ref` closing a cycle onto that same schema. Neither describes a value a decoder can terminate on — the generated `Deserialize` would re-enter itself on the same input with no base case — so they are refused rather than emitted. A *mutually* recursive nullable alias is not this and is supported: `B: {oneOf: [{$ref: A}, {type: \"null\"}]}` where `A` refers back to `B` is `Option<Box<A>>`, the same type the direct `{$ref: A}` spelling produces, whichever way the two are spelled or which files they live in. The third is sibling keywords that leave the union with no branch at all, because every branch — or the sole one, when the union has a single non-null member it collapses to — has an empty or unrepresentable intersection with the enclosing schema's own sibling keywords: `type: object` beside a lone `{type: string}` member, say. Split the applicators, make every discriminator branch explicit, break the self-reference, reconcile the sibling keywords with the members, or omit this API segment with `spargen::omit!`."
            }
            Code::NonScalarEnum => {
                "Enums and const values must be homogeneous scalar sets. A `null` member (or `\"null\"` in the schema's type array) is allowed: it is stripped and makes a remaining scalar enum nullable (`Option<Enum>`), while a value set of only `null` lowers to the exact JSON null type (`()`). The members that become enum variants are strings, integers, and booleans. On a schema whose `type` names `integer`, an integral floating-point member of magnitude below 2^53 is the integer it equals (`1.0` is `1`), as an integral float `default` on an `integer` field is. Rejected are a set that mixes distinct non-null scalar kinds (e.g. a string with an integer), an object or array member, any other floating-point member (`1.5`, `1.0` on a schema whose `type` does not name `integer`, or an integral float of magnitude 2^53 or more), which has no Rust enum discriminant, and an integer above `i64::MAX`, which no integer enum variant represents."
            }
            Code::UnsupportedMediaType => {
                "JSON (`application/json` and `application/*+json`), XML (`application/xml` and `text/xml`), raw binary (`application/octet-stream`, and CBOR — `application/cbor` and any `+cbor` structured-syntax suffix — as opaque octets under the same gate and rank, sent as its own `Content-Type` and, when every bodied success response is CBOR, asked for as its own `Accept`), raw UTF-8 text (`text/*` and GitHub's `application/octocat-stream`), form-urlencoded requests, and multipart requests are generated. Text is decoded through a JSON string value so string enums/formats remain typed; binary responses remain `bytes::Bytes`; single- and multi-status success/error dispatch use the selected response's codec. Raw text requires a string-like, binary, or untyped (`{}`) schema. Octet-stream requires a binary schema, or the OpenAPI 3.1 spelling of one: JSON Schema 2020-12 alignment removed `format: binary`, so an empty (always-true) Schema Object — or no `schema` at all — says \"any octets\" and lowers to `bytes::Bytes` like any other binary body. A nullable raw text *response* decodes through serde and is generated as `Option<..>`. Media *ranges* are permitted `content` keys and name a family rather than a type: `text/*` is raw UTF-8 and every other family (`video/*`, `audio/*`, `*/*`, …) is opaque octets, ranked below every codec spargen has, so a concrete sibling outranks a family range. The structured-suffix ranges `application/*+json` and `application/*+json-seq` are read instead through the codec their suffix names and rank level with it, so on a response `application/*+json` ties `application/json` and source order decides between them (both decode as JSON); no other suffix range gets a suffix codec (`text/*+json` is raw text like any `text/` key, and `application/*+xml` is unsupported). A request body withholds a suffix range that classifies from the choice whenever a sibling it could send instead exists (a key that classifies, is no range, and is not streaming media) and reports it as the alternative not generated (`W014`). A concrete member of the `image`, `audio`, or `video` family (`image/jpeg`, `audio/mpeg`, `video/mp4`) is opaque octets as well, under the same schema gate, and on a response is ranked below every other key spargen can classify, ranges included, so it is generated only when it is the sole classifiable key; a request body naming one is sent with that exact `Content-Type`, and is preferred there to a range, which a request cannot send. XML uses the feature-gated quick-xml codec. Sequential responses with `itemSchema` (`text/event-stream`, JSON Lines/NDJSON, and JSON Text Sequences) generate a typed standard `EventStream<T>` when the stream is the operation's single bodied success response; an SSE envelope's JSON `data.contentSchema` becomes `T` directly. Media Type Object `encoding` is implemented for `multipart/form-data` and `application/x-www-form-urlencoded`, with the specification's mode switch: any explicit `style`/`explode`/`allowReserved` selects RFC 6570 serialization (making `contentType` inert), otherwise the property is rendered by its `contentType`, whose first list element is sent as a multipart part's header and only picks the codec of a form field. The cases that reach this code are: a media type that names no codec spargen has — a concrete type outside `text/*`, CBOR, and the `image`/`audio`/`video` families (`application/pdf`, `application/sdp`, `font/woff2`), so `image/*` and `image/jpeg` are accepted where `application/pdf` is not, because the family is what says \"opaque octets\", or a family member whose subtype carries a structured-syntax suffix (`image/svg+xml`), whose suffix names a text syntax, so bytes would be the silently-wrong reading; a media range as a request body media type — a family range (`type/*` or `*/*`) when no concrete key beside it classifies, or a structured-suffix range when nothing beside it can be sent — since `Content-Type` requires a concrete type/subtype and a generated request sends its media key verbatim; a streaming request body; a `multipart/form-data` or `application/x-www-form-urlencoded` request body whose schema is not an object; a raw body whose schema its codec cannot carry — raw text with a schema that is not string-like, binary, or untyped (`{}`), or octet-stream (and every octet-classified family and CBOR type, so a schema'd CBOR document such as an object under `application/cbor`, which would need a CBOR codec) with a schema that is not binary; a raw body whose schema admits `null` — a `oneOf`/`anyOf` with `{ type: 'null' }`, or `type: [string, 'null']` — where it is sent or read verbatim: a request body under raw text, a `bytes::Bytes` request body under any media, and a `bytes::Bytes` response body, since raw content has no wire representation of `null` (a request body that may be omitted is `required: false`); a form-urlencoded or multipart response body; a bodied streaming response anywhere but the operation's single bodied success — an error status, a `default` response (which documents error statuses too, even when it is the only response), or beside a second bodied success status — since each of those positions decodes one whole body; an XML response body beside a second bodied status on the same side, whose response enum would decode more than one body (a lone XML body beside a bodyless status is still one body and is supported); a complete-sequence `schema` without `itemSchema` on OpenAPI 3.2 sequential media; a `content` parameter whose media type has no single-token serialization (anything but JSON or raw text); `prefixEncoding`/`itemEncoding` on multipart, which describe positional parts of an array-shaped multipart body, where spargen generates from an object schema; a nested `encoding`, `prefixEncoding`, or `itemEncoding` inside an Encoding Object, which describes a nested multipart part where spargen generates one flat level; the RFC 6570 combinations the specification leaves undefined — `spaceDelimited`/`pipeDelimited` with `explode: true`, and on multipart a `deepObject` style or RFC 6570 serialization of an object-valued property; a wildcard `encoding.contentType`, since a client sends one concrete media type; an `encoding.contentType` whose sent (first) element is not a media type at all under the RFC 6838 `type/subtype` rule `content` keys follow (`text/plain/extra`); an `encoding.contentType` whose sent element carries a parameter that is not RFC 9110 `name=value` with a token or quoted-string value (`text/plain; foo`), or, on multipart only, a quoted value the generated client's HTTP stack cannot send (empty, or holding a `\"` or a tab); a multipart `encoding.headers` entry whose pinned `const` or `default` is sent as a part header but whose name is not an RFC 9110 token (`Bad Name`, `X-A:B`) or whose value is not an RFC 9110 field value (a control character, DEL, or leading or trailing whitespace); an explicit `contentType` that is not JSON (`application/xml`, `text/csv`, or a type with no codec such as `application/yaml`) on a multipart property that is neither a scalar nor binary, or on a form-urlencoded property that is not a scalar — an object, array, tuple, union, or `{}` — because spargen serializes such a value only as JSON, so the part's bytes would contradict its header, or the field would not be in the syntax the document declares (RFC 6570 `style`/`explode` serialization is the form-urlencoded alternative); a binary property in a form-urlencoded body, including a string property made binary by a declared `contentType` of `application/octet-stream` or a concrete `image`/`audio`/`video` type, since a form field has no representation for raw bytes (on multipart a scalar or binary property keeps such a header as its part's `Content-Type`); and an unsupported XML hint on a type that is serialized as XML, where ignoring the hint would put structurally different XML on the wire (on a type never serialized as XML it has no effect, and is `W006`): `xml.namespace`, `xml.prefix`, `xml.wrapped`, or an `xml.nodeType` departing from the node's default, written on a property, on an array property's `items` (reported as `items.<hint>`), or on a schema's own root; an attribute node (`nodeType: attribute` or `attribute: true`) on `items` or a root, which has no element of its own there; and a root `xml.name` on a schema whose type is no struct, that does not name its own element (only a component schema or a Media Type Object's schema does), or that is a `$ref` with no shape-bearing sibling, whose type every use of the target shares; and any hint, an `xml.name` included, on the `xml` object of a composition member whose definition the merge consumes — an `allOf` member read by its keywords (a `$ref` member's siblings and a nested `allOf` among them) or a `oneOf`/`anyOf` branch merged into another type — which names no element of its own, judged by whether an XML body reaches the type the merge produced."
            }
            Code::UnsupportedParameterStyle => {
                "Path/header parameters support simple style, path parameters also `matrix` and `label`, and query/cookie parameters support form style, including the OpenAPI explode defaults and explicit explode overrides. OpenAPI 3.2 cookie style is emitted without percent encoding, and a single whole-query-string parameter supports JSON or application/x-www-form-urlencoded content. JSON and raw-text (`text/*`) content-typed parameters are generated. `deepObject`, `spaceDelimited`, and `pipeDelimited` query parameters and `allowReserved: true` are all supported. A style the parameter's location does not permit, and a `querystring` parameter without exactly one `content` entry, are rejected before lowering by the official document schema, as `E011`; a `content` key that names no codec spargen has is `E009`, in a parameter as anywhere else. What is rejected here is `spaceDelimited`/`pipeDelimited` with `explode: true` (which the specification's own serialization table leaves undefined), a nested array or object inside a `simple`/`form` value, an object property whose value no schema constrains (such as a `required` name no `properties` entry declares, on an object with no `additionalProperties` schema), a parameter schema that no value satisfies — the boolean schema `false`, or an `allOf` whose members conflict — or that has such an uninhabited `oneOf`/`anyOf` member, at the parameter's root or inside it, since serialization has no token for a value that cannot exist, a JSON or `text/*` `content` parameter whose entry has no schema, which leaves the argument no type, and a `querystring` parameter whose one content entry is neither JSON nor form-urlencoded or has no schema, or whose form-urlencoded schema is not an object — none of which could be serialized with defined wire semantics."
            }
            Code::ServerInitiatedFlowIgnored => {
                "Webhooks, callbacks, and links describe server-initiated or hypermedia behavior. They are acknowledged with a warning and no client code is emitted."
            }
            Code::InvalidInput => {
                "The input could not be read or parsed, or breaks a structural rule of OpenAPI, so feature auditing cannot proceed on it. The causes are: a file of the bundle, the lock file, or a vendored copy that cannot be read (or a vendored copy that is not UTF-8), or a `spargen.lock` that is not valid; malformed JSON or YAML, JSON or YAML nested more than 256 levels deep (a YAML alias counting at the depth its copy reaches), a YAML stream with more than one document, a YAML mapping key that is not a string, or YAML aliases whose expansion, counting the copy of each anchored node an alias names, grows the document past 100 nodes per parse event (nested anchors that compound); a violation of the vendored official OpenAPI document schema for the declared version (reported as an `OpenAPI structure violation`), which covers among others a parameter style its location does not permit, an `in: querystring` parameter without exactly one `content` entry, a Responses key that is not `default`, an extension, a status code, or a status range, and `allowReserved` outside a query parameter in 3.1; a required field that is missing, or a value that is not the object or string its position requires; a Schema Object keyword whose value has the wrong shape — a `type` that is not a JSON type name or an array of them, or names no JSON type (`strng`), a `required` that is not an array of strings, an `allOf`, `oneOf`, `anyOf`, or `prefixItems` that is not an array, or a `minLength`, `maxLength`, `minItems`, `maxItems`, `minProperties`, or `maxProperties` that is not a non-negative integer, a `minimum`, `maximum`, or `multipleOf` that is not a number, an `exclusiveMinimum` or `exclusiveMaximum` that is neither a number nor a boolean (OpenAPI 3.0's boolean form is reported as `W001` instead), a `$ref`, `$schema`, `pattern`, `format`, `contentEncoding`, `contentMediaType`, `title`, or `description` that is not a string, a `uniqueItems`, `deprecated`, `readOnly`, or `writeOnly` that is not a boolean, a `properties`, `patternProperties`, `$defs`, or `dependentSchemas` that is not an object, or an `enum` that is not an array — reported at the keyword or the array element holding it; and the semantic rules that schema cannot express: an `operationId` used twice, a parameter name/location pair declared twice on a path item or an operation, a path template whose placeholders do not match the declared path parameters, more than one `in: querystring` parameter or one combined with `in: query` parameters, a server URL that repeats a variable, uses one it does not declare, or has a `{` or `}` that is not one side of a non-empty `{name}` reference, a server variable `default` outside its `enum`, a tag hierarchy with a cycle or a missing parent, an `additionalOperations` key that restates a fixed-field method in any letter case, and `xml.attribute` or `xml.wrapped` beside `xml.nodeType`, which OpenAPI 3.2 forbids and which could disagree with it. The same code also reports a failure of the generation pipeline itself — writing the output, lock, or build cache, inputs that kept changing while they were read, or an internal invariant of the lowered model (`IR invariant failed`, a spargen bug worth reporting) — since none of those is a feature of the document to audit. Each diagnostic's message names its cause."
            }
            Code::DuplicateObjectKey => {
                "An object (JSON or YAML mapping) declares the same key more than once. Duplicate keys make the member ambiguous — a reader cannot tell which value wins, and downstream a duplicated `properties` name or schema keyword would resolve inconsistently — so spargen rejects the document at parse time and points at the second occurrence rather than silently keeping one. Remove or rename the duplicate key."
            }
            Code::UnknownSecurityScheme => {
                "Every scheme named in a `security` requirement must be declared under `components.securitySchemes` as a type spargen can satisfy, so credentials can be attached at the right location: `http` with the `bearer` or `basic` authentication scheme, matched in any letter case as RFC 9110 defines it (`Bearer` and `BASIC` are those same schemes), `apiKey`, `oauth2`, `openIdConnect`, or `mutualTLS`, which the client certificate of the injected `reqwest::Client` satisfies and which is acknowledged with `W011` because nothing is attached for it. This error is reported for a requirement naming a scheme that is not declared, or whose declaration did not lower, and for an `http` scheme whose authentication scheme is anything other than `bearer` or `basic` in some letter case (`digest`, `negotiate`, or a missing `scheme`), which needs a challenge/response exchange a statically attached credential cannot perform."
            }
            Code::AllOfIrreconcilable => {
                "Two constructs intersect schemas into one type and report this code when the result is empty or unrepresentable: the members of an `allOf`, and — because `$ref` is an applicator in JSON Schema 2020-12 rather than a replacement — a `$ref` together with its own shape-bearing sibling keywords, which are intersected with the referenced schema instead of being discarded. A schema's own keywords meet one another in the same way, and one more case reaches this code with no composition written: a schema with neither an `allOf` nor a `$ref` whose own `enum`, `const` or `anyOf` members leave only the `null` its `type` refuses (`{type: string, const: null}`, `{type: string, enum: [null]}`), which no value satisfies; add `null` to its `type`, or give the `enum`, `const` or `anyOf` a value the `type` accepts. An `allOf` beside a `oneOf`/`anyOf` on one schema meets that union in the same way: both apply, so the `allOf` composition is intersected with the union, its `discriminator` included, branch by branch, as a `$ref` is with a union target — a branch the composition excludes drops out, a union the composition leaves with no branch is rejected with this code (where the schema's own sibling keywords, not an `allOf`, leave a union no branch, the code is `E007`), and an `allOf` that constrains nothing leaves the union as it is. The schema's own keywords beside both are the union's siblings and not also part of the composition, and an `allOf` member of untyped object or array keywords alone refines the union as those sibling keywords do, so neither drops the union's branches of another category or its `null`; the composition of the other members admits `null` only where every one of them does. An `allOf` exactly one of whose members is a union — an inline `oneOf`/`anyOf`, or a `$ref` with no shape-bearing siblings to a component that is one — meets that union with its other members in the same way, so `allOf: [{$ref: Base}, {oneOf: [...]}]` is the `{$ref: Base, oneOf: [...]}` it spells and `allOf: [{$ref: Base}, {$ref: U}]` meets `U`'s branches with `Base` as `{$ref: U, <Base's keywords>}` does, and a union of one real branch beside `null` is met as that branch, beside another union member too. Object members are met with one union only, so an object composition with two or more unions to meet is rejected with this code: two `oneOf`/`anyOf` members, or one beside a `$ref` member to a union component, beside a union nested in an inner `allOf`, beside a union in its own `allOf`, or beside the union written next to the `allOf`, and a `$ref` to an object, or object sibling keywords, beside a union sibling and a union the target is or the siblings' `allOf` reaches, since meeting the second would nest one union's branches under the other's; a union of one real branch beside `null` is not counted, being that branch. A sibling bears a shape of its own through `type`, `properties`, `patternProperties`, `required`, `additionalProperties`, `items`, `prefixItems`, `enum`, `const`, `contentEncoding`, `format: binary`, `allOf`, `oneOf`, or `anyOf`. A union sibling is intersected with the target branch by branch, so the branches the target excludes are dropped, and a union none of whose branches meets the target (`{$ref: '#/components/schemas/Name', oneOf: [{type: integer}]}` where `Name` is a string) is rejected here. With no `type` beside them, the object keywords (`properties`, `patternProperties`, `required`, `additionalProperties`) establish an object and the array keywords (`items`, `prefixItems`) an array, and neither says anything about `null`, so the target's nullability stands: `required: [a]` beside a `$ref` to an object makes `a` required, and beside a `$ref` to a string it is an empty intersection — to a nullable string too, since sharing only `null` does not reconcile the categories. A sibling carrying both kinds with no `type` to choose between them is rejected, because no single type represents what it constrains. Beside a union — a `$ref` to one, or the `oneOf`/`anyOf` of the same schema — the untyped keywords instead refine only the branches of their own category and leave the others as they are, since an instance of another category satisfies them vacuously and establishing their category for the whole union would drop those branches and reject values it accepts: `required: [a]` beside `oneOf: [string, Obj]` requires `a` of `Obj` and keeps every string, and both kinds together each refine their own branches. Against a branch that states no category (`{}`, or `{required: [b]}` alone) they establish their own, as beside a `$ref` to it, so `properties` beside `anyOf: [{required: [a]}, {required: [b]}]` is an object in every branch. Such a branch is rejected when none can be established for it: both kinds together, or a multi-type `type` array beside them that admits another category too. A multi-type `type` array beside them still excludes the branches of the categories it omits. Where no branch has their category they constrain nothing the union accepts, and are acknowledged with `W011` rather than rejected. A `required` name no `properties` entry declares is still a requirement, carried as a required field typed by the object's `additionalProperties` schema, and unconstrained when there is none; `additionalProperties: false` closes the object to the fields the generated type declares, that one included. In a composition such a field is no declaration of the property: where another side declares it, that declaration supplies its type, as it would with the `required` absent, and otherwise every side's `additionalProperties` schema constrains it, so `{$ref: Labels, required: [a]}` with a string-valued `Labels` makes `a` a string. A `$ref` whose siblings bear no shape is simply its target. Either way, object members flatten into a single struct (union of properties; a property required by any member is required; repeated properties recursively retain their narrower compatible intersection; `additionalProperties` is intersected conservatively), while scalar members narrow compatible primitives, enums, arrays, objects, unions, and nullability. Examples include integer within number, enum within its scalar type, and a detailed object within a broader object; array items whose types share no value — disjoint JSON types, or scalar enums with no common member — become an uninhabited item type so the valid empty array remains representable, and in the same way a repeated property whose types share no value becomes an uninhabited field when no member requires it — whether the members are `allOf` entries or a `$ref` and its siblings — so the objects that omit it remain representable. Two sides that share values no single Rust type represents (a `uuid` string against a `contentEncoding: base64` one, both annotations on a string) are never typed uninhabited, collapsed to `null`, or dropped as a union branch: wherever they meet — array items, a property no member requires, a union branch, or a branch of a referenced union — they are rejected, because an uninhabited stand-in would refuse valid instances. It is rejected only when the overall intersection is empty or cannot be represented faithfully: scalar members with no common value or no single type, such as incompatible scalar categories (`{$ref: '#/components/schemas/Name', type: integer}` where `Name` is a string accepts no value at all) or two enums that share no member; conflicting constraints on a property some member requires, which no object satisfies, when `null` does not satisfy every member either (when it does, it is the one value left, and the composition is the null type `()` in every spelling); conflicting additional-value constraints; an object/scalar mix; an `allOf` member that is the boolean schema `false`, which admits no value; or a `$ref` that closes a reference cycle back to the schema enclosing it — an `allOf` member, a `$ref` carrying shape-bearing siblings, either of those written as a `oneOf`/`anyOf` member, or the schema of a property or of `additionalProperties` that two `allOf` members both constrain — whose target's own definition depends on the result being computed, so it can be composed neither against itself nor by discarding it. A union member beside a multi-category `type` array that is the union's only shape-bearing sibling, whose target's own `type`, or untyped object or array keywords, name only categories the array admits, meets that array in itself and stays itself. Met with an untyped schema (`{}`, or validation keywords alone), which every value satisfies, such a `$ref` needs nothing of its target and stays itself, wherever it meets one: array items, a tuple position, a property two `allOf` members both declare, or an `additionalProperties` value two members both constrain. For a `$ref` carrying shape-bearing siblings, and for a union member, that is a property of the document: the cycle is traced through every file, however each reference is spelled, so neither the order `components.schemas` is declared in nor the schema lowering happens to reach first can change the verdict. An `allOf` member, and a property or `additionalProperties` value two members both constrain, is still refused only when its target is mid-lowering; when lowering finds it so from another schema's body, that body is lowered first instead, so the order `components.schemas` declares them in does not decide the verdict, and a composition of schemas each composed of the next is refused in every order. Where it generates, the target was complete and the merge is exact. Restructure the composition — or make the `$ref` target and its siblings agree — or omit this API segment with `spargen::omit!`."
            }
            Code::InvalidOmitRule => {
                "A compatibility omit rule must match at least one source construct, and cannot omit the document root. An exact rule — a path, operation, component, pointer, or file-local pointer whose value carries no glob metacharacter — must name a construct the document holds. A glob rule — a `path`, operation `path`, component `name`, or `pointer` value carrying `*`, `**`, or `?` — removes every construct it matches and must match at least one. A file-local pointer rule must also name a file the input bundle loaded. Fix or remove the stale rule; a backslash (`\\*`, `\\?`) makes a metacharacter literal and keeps the rule exact."
            }
            Code::OmittedConstruct => {
                "An omit rule removed this construct before OpenAPI validation/lowering: either a rule written in a compatibility omit profile, or one auto-carve derived to cut away a construct spargen rejects. The source schema on disk was not modified. Once spargen supports the construct, remove a hand-written rule to generate it again; auto-carve stops omitting it on its own, because it carves only what is rejected."
            }
            Code::OmitCreatedInvalidDocument => {
                "After the omit rules were applied, what remains is no longer a valid OpenAPI root: a rule removed `openapi` or `info`, which every OpenAPI document requires, or the rules left none of `paths`, `components`, and `webhooks`, of which a document must carry at least one. A `$ref` an omission leaves dangling is reported as `E004`. Narrow the rules so they keep those root fields, or fix the source schema."
            }
            Code::SchemaDefaultNotApplied => {
                "A `default` is applied as a serde deserialization default only when it is a single scalar (bool/integer/number/string) that matches the field's own scalar type or one of its enum variants. Object, array, null, heterogeneous, or type-mismatched defaults cannot be lowered to a Rust literal, so the value is recorded in the field's rustdoc but not wired — deserialization of an absent field yields `None` rather than the default. The field's type is the one it ends lowering with: where an intersection (`allOf` members, or a `$ref` with sibling `properties`) narrows a property, a member's default is decided again against the narrowed type — a string default on a property narrowed to an enum becomes the enum's variant — and one that is no value of it is reported at that `default`, naming the type whose field drops it, while the member that wrote it keeps it on its own type. Where the sides of an intersection declare two different defaults for one property, the merged field keeps one — an applicable default before one that cannot be applied, then the lesser rustdoc note, the same one in either order — and the other is reported at its `default`, naming the kept one's location. A `default` with no field, parameter, or type of its own to carry it is reported and neither applied nor documented: on array `items`, a tuple `prefixItems` entry, an `additionalProperties` or `patternProperties` value, a request or response body root, or an SSE envelope's JSON `data` content schema; and beside a bare `$ref` — one with no other shape-bearing sibling — as a schema component, or as a parameter, header, or Media Type `schema` or `itemSchema`, where the reference resolves to its target and the default has nowhere to go."
            }
            Code::Oas32ConstructIgnored => {
                "OpenAPI 3.2 `itemSchema` describes one item of sequential media. On a non-sequential media type it does not define any wire behavior, so spargen acknowledges and ignores it while continuing to use the complete-body `schema`. Move the item schema to sequential media such as `application/x-ndjson`, `application/json-seq`, or `text/event-stream`, or use only `schema` for ordinary media."
            }
            Code::AlternativeMediaIgnored => {
                "A request body or response offered more than one media type. A generated method sends and decodes exactly one, so spargen picks the one it can represent best — JSON first, then XML, multipart, form-urlencoded, octet-stream and CBOR (`application/cbor`, `+cbor`) level with it, text, the sequential media, then the media *ranges* (`text/*`, then every other family such as `video/*` or `*/*`), and last of all a concrete `image`/`audio`/`video` type, which is ranked below every other key spargen can classify — breaking ties by source order. The structured-suffix ranges `application/*+json` and `application/*+json-seq` are the exception among ranges: each ranks level with the codec its suffix names, so on a response `application/*+json` ties `application/json` and source order decides. On a request body such a suffix range is withheld from the choice whenever a sibling it could send instead exists (a key that classifies, is no range, and is not streaming media), and is reported here as an alternative not generated, after the alternatives the ranking passed over. On a request body a family range (`type/*` or `*/*`) is considered only once no concrete key classifies, because a range cannot be sent as `Content-Type`, so there a concrete `image`/`audio`/`video` type is sent in preference to it. That is normally a real narrowing of the documented API surface: a server willing to accept XML as well as JSON will only ever be sent JSON by this client. It is reported rather than left silent so the choice is visible, and it is a warning rather than an error because the selected media type is genuinely supported and the generated client is correct for it. For the same reason it is reported only once the selected entry has passed every media-type check (`E009`) on that body or response's own content: a selection one of them rejects generates nothing, so there is no narrowing to disclose, and the rejection is reported alone. The checks that weigh a response against the operation's other responses — an XML body beside a second bodied status on its side, or a streaming body anywhere but the single bodied success — run after the narrowing is reported, so their `E009` can appear beside a `W014` for the same response. One narrow case is not reported, because nothing is given up: on a *response* whose selected media is octet-classified — `application/octet-stream`, a non-textual media range such as `video/*`, or a concrete `image`/`audio`/`video` member such as `image/png` — an alternative that is also octet-classified and describes nothing of its own is `bytes::Bytes` just as the selection is, so generating one of them narrows nothing. A selected CBOR type is not exempt, on any response, and its alternatives are always reported: where the request announces it as `Accept` (when every bodied success response is CBOR), every alternative is asked against at the wire; where it does not (an error response, or a success beside a non-CBOR success body), nothing narrows at the wire and the alternatives are still reported, because the selection is made one response at a time, before the operation's `Accept` is known. Everything about that exemption is load-bearing. It is confined to octet-stream because that is the only codec whose gate collapses every body it admits onto one type; elsewhere two entries that both look empty can still differ — a media type with no `schema` lowers to `()` while `schema: {}` lowers to `serde_json::Value`, and a sequential media's item type lives in `itemSchema`, outside the body schema entirely. It is confined to responses because a request narrows at the wire whatever its type: the selected media key becomes the `Content-Type` verbatim, so a server documented as accepting two media is only ever sent one. And \"describes nothing\" is proved from the alternative's own Media Type Object rather than assumed from its media type — a validation keyword such as `maxLength`, an `itemSchema`, or an `encoding` all count as saying something. To generate against a different one, remove the alternatives from the document, or omit this API segment with `spargen::omit!` and hand-write the call."
            }
            Code::SchemaNestingTooDeep => {
                "Lowering a schema into a Rust type is recursive: each nested object property, array item, `allOf`/`oneOf`/`anyOf` member, and `$ref` target descends one level. Spargen caps that descent so a pathologically deep composition — a very long chain of components that each `$ref` the next, or a deeply nested inline schema — is rejected with this error instead of being allowed to exhaust the call stack and abort the process. A genuine API surface never approaches the limit; hitting it almost always means the spec was machine-generated or adversarial. Flatten the offending chain, or omit that API segment with `spargen::omit!`."
            }
            Code::RuntimeDependencyContract => {
                "The generated module is freestanding, so its consuming Cargo package must declare the crates and dependency features referenced by that specific API. Spargen derives the exact requirement set after lowering and audits Cargo.toml during build.rs and proc-macro generation. Use the documented tested lower bounds (or a higher semver-compatible caret floor, or an exact `=x.y.z` requirement at or above the floor on the same semver-compatible line), keep reqwest default features disabled, and enable only the capabilities the diagnostic names. A dependency declared `workspace = true` is followed to the workspace root's `[workspace.dependencies]` — the consumer manifest itself when it declares `[workspace]`, otherwise the root `package.workspace` names, otherwise the nearest ancestor manifest that parses and declares `[workspace]` — taking the version from there, the union of both feature lists, and default features on when the root leaves them on or the member sets `default-features = true` (a member's `default-features = false` cannot turn off defaults the root leaves on, so disable them in `[workspace.dependencies]` and leave the member's `default-features` unset or `false`), while `optional` is read from the member, as Cargo does. Inheriting a required crate therefore satisfies the audit; when a root is found but cannot be read, or declares no such entry, the diagnostic says which of those happened rather than reporting the crate as missing, and when no root is found at all it says that no workspace manifest was found, then names the nearest ancestor manifest that failed to read, if there was one, only as a possible root together with the reason it could not be read — never as the workspace manifest, because an unrelated broken `Cargo.toml` above the project can be that file. A root reached through `package.workspace` is read and reported in its own right, so its inheritance message carries no reason of its own and a read-failure diagnostic naming the same file stands above it. Cargo resolves the declared range; Rust compilation then verifies the selected crates expose the APIs and traits used by the generated client."
            }
            Code::SpecUndefinedBehavior => {
                "The OpenAPI Specification marks some constructs' behavior as *undefined* rather than leaving them merely unsupported. Currently this fires for a Path Item `$ref` declared alongside structural fields (operations, `parameters`, `servers`): the specification says that when a field appears both in the referring Path Item and the referenced one, the behavior is undefined. Unlike a Reference Object, which requires adjacent properties to be ignored, there is no rule to follow — so either choice (the local fields winning, or the referenced ones) silently discards operations the author wrote, and produces a client that calls a different set of endpoints than the document describes. `summary` and `description` are exempt because they are documentation and cannot change the wire. Move the sibling fields into the referenced Path Item, or drop the `$ref` and declare the item inline."
            }
            Code::TupleRestNotRepresentable => {
                "In JSON Schema 2020-12 `prefixItems` fixes the leading positions of an array and `items` describes every position after them. Spargen lowers `prefixItems` to a Rust tuple, which is fixed-length, so a schema that also allows a typed remainder describes a value no single Rust type expresses: a tuple cannot grow, and a `Vec` cannot hold the distinct per-position types. `items: false` closes the array at the prefix and is fully supported — that is exactly a tuple. To send a variable-length remainder, drop `prefixItems` and describe the whole array with `items`, split the fixed head into its own object properties, or omit this API segment with `spargen::omit!`."
            }
            Code::DeclarationHasNoEffect => {
                "The document declared something the specification permits here, but which cannot change any byte spargen generates or sends, so it is acknowledged rather than dropped in silence. It fires for: an `in: header` parameter named `Accept`, `Content-Type`, or `Authorization`, which the specification says SHALL be ignored because the protocol layer owns those headers; `allowReserved` on a parameter that is never percent-encoded (an `in: header` parameter, or `style: cookie`, both of which the specification sends verbatim); `allowReserved` on a `multipart` Encoding Object, whose part values are never percent-encoded; `encoding`, `prefixEncoding`, or `itemEncoding` on a media type that is neither `multipart` nor `application/x-www-form-urlencoded`, where the specification says those fields SHALL be ignored; `prefixEncoding` or `itemEncoding` on `application/x-www-form-urlencoded`, which the specification scopes to `multipart` alone; an `encoding` entry naming a property the body schema does not declare; `encoding.headers` on a non-`multipart` media type; an `encoding.headers` Header Object that pins no `const`/`default` value, leaving a client nothing to send; `allowEmptyValue`, which is deprecated and cannot change what a typed client omits; a `mutualTLS` security scheme, which is satisfied by the transport's client certificate rather than by anything the client attaches; a response header named `Content-Type`, which the specification says SHALL be ignored; a response header whose `schema` has a shape `simple` serialization cannot express, or whose `content` media type spargen cannot decode, or whose textual `content` schema is not a single value, or which declares `content` with no schema at all, none of which yields a typed accessor; a `summary` or `description` on a Reference Object, which documents one use site of a component that every use site shares as one generated item, so the override has nowhere to land; a server variable that its server's `url` never uses; a `servers` entry past the first on a path item or operation, where the specification defines no client selection rule; a union branch that the enclosing schema's own constraints have already made unsatisfiable, whether they are keywords beside the union, the target of a `$ref` beside it, or the other members of an `allOf` the union is a member of (a branch that is the exact JSON `null` alone, beside constraints that refuse `null`, drops out unreported in every spelling, as a `{type: 'null'}` branch does); untyped object or array keywords beside a union, or beside a `$ref` to one, when no branch has their category, which they then constrain nothing of; a `discriminator` on a schema with no `oneOf` or `anyOf` of its own, which selects nothing because spargen dispatches by tag only across the members of the union beside it — the `allOf` polymorphism form, where child schemas reach the parent through `allOf`, is not generated, since no keyword of the parent lists its children — so the schema lowers by its other keywords, while each `mapping` and `defaultMapping` value is still resolved and one naming no schema is `E004`; a `discriminator` beside a union member that is no schema component — an inline member, or a `$ref` pointing into another schema — and that no `mapping` entry or `defaultMapping` names, because the specification gives such a member no implicit value, so no discriminator value selects it and spargen will not invent one: the members a tag does name keep their tag dispatch, and the untagged member is matched by its own schema only when the tag is absent or names no tagged member, or, where no member carries a tag at all, the union is decoded by its members' schemas, as one with no discriminator is; and a schema component declared inside a referenced sub-file whose name the root document also declares. That last one is a fact about the *document* rather than about one keyword: a JSON Pointer fragment addresses the file it is written in, so a sub-file's own `#/components/schemas/<name>` asks for that file's declaration — but spargen consults the root document's component map first, so where both declare the name the root's wins and the sub-file's is never read. The warning names both namespaces and fires once per reference site, because the site is what has to be found; address the file-local declaration explicitly with a relative-file reference, or rename one of the two, if the root's is not the one you meant. None of these is an error: the document is valid, and the construct simply has no reachable effect on this client."
            }
            Code::RuntimeAuditSkipped => {
                "Generated output is freestanding: the consuming package must itself declare the crates and dependency features that specific API needs. Spargen audits the consumer's `Cargo.toml` for that contract and reports any gap as `E023` — but only when it can locate that manifest. This generation ran in a Cargo build script, so its rebuild triggers were still emitted, but its environment named no consumer manifest: spargen reads `CARGO_MANIFEST_PATH`, or else `CARGO_MANIFEST_DIR`, which Cargo sets for a build script it runs. With nothing to audit, the contract is unverified, and a missing dependency surfaces later as a compile error in the generated module instead of a spargen diagnostic. Run `spargen deps <spec>` to print the exact `[dependencies]` block that spec requires and check the package against it, or run the build script where Cargo sets those variables so the audit runs. Set `CargoIntegration::Required` to make a missing manifest an error (`E024`), or `CargoIntegration::Off` to skip the audit without this warning; `Off` also stops the `cargo:rerun-if-changed` directives, so an edit to the spec no longer reruns the build script. Under `CargoIntegration::Auto`, generation outside a build script is reported as `W013` instead."
            }
            Code::CargoIntegrationDegraded => {
                "`generate` was called outside a Cargo build-script process, so two things Cargo would otherwise do did not happen: no `cargo:rerun-if-changed` directives were emitted, meaning an edited spec will NOT trigger a rebuild and the checked-in module can silently go stale; and the consumer manifest could not be located, so the runtime-dependency audit (`E023`) was skipped. Neither is an error — generating outside a build script is a legitimate thing to do — but both are silent by nature, which is why they are reported. Call `generate` from a `build.rs` to get both, or declare the intent with `CargoIntegration::Off` to accept them silently."
            }
            Code::CargoIntegrationRequired => {
                "The caller set `CargoIntegration::Required`, declaring that this generation must be wired into Cargo — rebuild triggers emitted, consumer manifest audited — and it is not: either the process is not a build script, or no consumer manifest could be found. This is an error rather than a warning purely because the caller asked for it: `Required` exists for builds where a missed rebuild trigger would ship a client generated from a stale spec. Move the call into a `build.rs`, or relax to `CargoIntegration::Auto` (degrade with `W013`/`W012`) or `CargoIntegration::Off` (degrade silently)."
            }
            Code::XmlHintIgnored => {
                "XML request/response bodies honor the `xml.name` (element/attribute rename) and `xml.attribute` (serialize as an XML attribute via quick-xml's `@name` convention, also spelled OpenAPI 3.2 `xml.nodeType: attribute`) hints on a property, and an `xml.name` on an array property's `items`, or else the root `xml.name` of the struct its items resolve to (naming each list element, while the array's own `xml.name` names nothing, as OpenAPI specifies for an unwrapped array), and, under OpenAPI 3.2, the element name of the struct a property that is a `$ref` (or an array property whose items are) resolves to — its root `xml.name`, or else its component name — since 3.2 gives a `$ref` the node type `none` (an explicit `xml.nodeType: element` on that `$ref` makes it an element named after the property), so the referenced schema's own element stands in its place (OpenAPI 3.1 has no node types and names that element after the property), but only on a schema used *exclusively* as an XML body. A root `xml.name` on a struct's own component schema or Media Type Object schema names the element the body serializes as, through a serde container rename no non-XML format reads, so it is applied whether or not the type is used exclusively as an XML body. Without a root `xml.name`, a struct lowered from a schema directly under `components.schemas` that an XML body reaches takes its component name as that element's name, as OpenAPI infers it. This warning reports a name that is not applied, in four cases. First, `xml.name`/`xml.attribute` on a schema that is not used exclusively as an XML body: the hint is withheld because a serde `rename` is format-agnostic and would also rewrite the non-XML wire name, so on a schema that is also reachable from a non-XML request or response body or from a parameter, or that no XML body reaches at all, the property keeps its normal wire name in every format, the XML body included. On a schema no XML body reaches, nothing on any wire moves; on one shared between an XML body and a non-XML use, withholding the hint does change the XML wire, which then carries the property's normal name rather than the hinted one. The message says which of the two it is. Second, a hint quick-xml serde has no faithful mapping for — `xml.namespace`, `xml.prefix`, `xml.wrapped`, or an `xml.nodeType` other than `attribute` that departs from the node's default (`element` on an array, which is `wrapped: true`; `none` on a node that does not default to it; `text`, `cdata`, or any other token), whether on a property, an array property's `items`, or a schema's own root, plus the `items`, root, and merged composition member hints `E009` lists — on a type that is never serialized as XML, where it has no effect on any wire. The same unsupported hint on a type that *is* serialized as XML is rejected with `E009` instead, because ignoring it would put structurally different XML on the wire. Third, a component name that is not an XML 1.1 name (a component key may begin with a digit, `-` or `.`, which no XML element name may), on a struct an XML body reaches with no root `xml.name`: quick-xml refuses to write such an element, so the inferred name is not applied and the body serializes under the struct's generated Rust type name instead; give the schema a root `xml.name` to choose the element name. Fourth, a name derived rather than written — the root `xml.name` an array property's items take, or a 3.2 `$ref` property's element name — that equals the element name of another field of the same struct: two fields read from one element name cannot be told apart, and quick-xml refuses the body as a duplicate field, so each such field keeps its property name as its element name. On a schema shared between an XML body and a non-XML one, the first case's message names a derived name as derived. Declaring `xml.attribute` or `xml.wrapped` beside `xml.nodeType` is `E011`."
            }
        }
    }

    /// The interpretation this code's behavior depends on, if any. [`InterpId`] states the
    /// reading behind each id this returns.
    pub fn interpretation(self) -> Option<InterpId> {
        match self {
            Code::UnsupportedOpenApiVersion => Some(InterpId(1)),
            Code::ValidationKeywordIgnored => Some(InterpId(2)),
            Code::NonDisjointUnion => Some(InterpId(3)),
            _ => None,
        }
    }

    /// Every code, in stable order — drives [`FromStr`](std::str::FromStr) and the tests that hold
    /// `docs/errors.md` and `docs/support-matrix.md` to the declared codes.
    pub fn all() -> &'static [Code] {
        const ALL: &[Code] = &[
            Code::UnsupportedOpenApiVersion,
            Code::UnsupportedDialect,
            Code::AbsoluteRefUnsupported,
            Code::UnresolvedRef,
            Code::VendoredRefDrift,
            Code::RemoteFetchFailed,
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

    /// Parse a stable string form (`"E001"`) back into a [`Code`].
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
mod tests;
