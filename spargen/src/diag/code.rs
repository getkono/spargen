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
    /// `allOf` members could not be reconciled into a single merged type — conflicting property
    /// types, conflicting `additionalProperties`, an object/scalar mix, incompatible scalars, or a
    /// direct recursive `$ref` member whose fields are not yet known (matrix: Schema shape).
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
            Code::AllOfIrreconcilable => "irreconcilable allOf composition",
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
                "A `$ref` target could not be found in the loaded input bundle. Check the file path and JSON Pointer fragment."
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
                "`oneOf`/`anyOf` unions are lowered to typed Rust enums with custom `Deserialize`/`Serialize` — never `serde(untagged)` and never degraded to `serde_json::Value`. Fast paths dispatch by discriminator tag, a unique non-object JSON category, or a proven disjoint category/required key. Overlapping variants use typed trial matching over one buffered value: `oneOf` requires exactly one successful variant; `anyOf` deterministically selects the most specific successful variant (enum before broad scalar, integer before number, more-required object before broader object, recursive array specificity, then source order), and serialization revalidates the same rule. Shape constraints adjacent to the union are intersected into every branch. This error is reserved for an applicator combination that is not yet representable, such as declaring both `oneOf` and `anyOf` on the same schema node, or OpenAPI 3.2 `discriminator.defaultMapping` without a generated fallback branch. Split the applicators, make every discriminator branch explicit, or omit this API segment with `spargen::omit!`."
            }
            Code::NonScalarEnum => {
                "Enums and const values must be homogeneous scalar sets. A `null` member (or `\"null\"` in the schema's type array) is allowed: it is stripped and makes a remaining scalar enum nullable (`Option<Enum>`), while a value set of only `null` lowers to the exact JSON null type (`()`). Sets that mix distinct non-null scalar kinds (e.g. a string with an integer) or that contain object/array members are rejected."
            }
            Code::UnsupportedMediaType => {
                "JSON (`application/json` and `application/*+json`), XML (`application/xml` and `text/xml`), raw binary (`application/octet-stream`), raw UTF-8 text (`text/*` and GitHub's `application/octocat-stream`), form-urlencoded requests, and multipart requests are generated. Text is decoded through a JSON string value so string enums/formats remain typed; binary responses remain `bytes::Bytes`; single- and multi-status success/error dispatch use the selected response's codec. Raw text requires a string-like/binary schema. Octet-stream requires a binary schema, or the OpenAPI 3.1 spelling of one: JSON Schema 2020-12 alignment removed `format: binary`, so an empty (always-true) Schema Object — or no `schema` at all — says \"any octets\" and lowers to `bytes::Bytes` like any other binary body. Media *ranges* are permitted `content` keys and name a family rather than a type: `text/*` is raw UTF-8 and every other family (`video/*`, `audio/*`, `*/*`, …) is opaque octets, ranked below every concrete media type so a concrete sibling always outranks a range. A range is rejected as a *request* body media type: `Content-Type` requires a concrete type/subtype, and a generated request sends its media key verbatim. XML uses the feature-gated quick-xml codec and is currently limited to an operation's single success or error body. Multipart requires an object request schema; form-urlencoded and multipart response bodies are rejected. Sequential responses with `itemSchema` (`text/event-stream`, JSON Lines/NDJSON, and JSON Text Sequences) generate a typed standard `EventStream<T>`; an SSE envelope's JSON `data.contentSchema` becomes `T` directly. Media Type Object `encoding` is implemented for `multipart/form-data` and `application/x-www-form-urlencoded`, with the specification's mode switch: any explicit `style`/`explode`/`allowReserved` selects RFC 6570 serialization (making `contentType` inert), otherwise the property is rendered by its `contentType`. Rejected are streaming requests, a complete-sequence `schema` without `itemSchema`, `prefixEncoding`/`itemEncoding` (which describe positional parts of an array-shaped multipart body, where spargen generates from an object schema), and the RFC 6570 combinations the specification leaves undefined — `deepObject` or an object-valued property on multipart, and `spaceDelimited`/`pipeDelimited` with `explode: true`."
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
                "`allOf` members are intersected into one type: object members flatten into a single struct (union of properties; a property required by any member is required; repeated properties recursively retain their narrower compatible intersection; `additionalProperties` is intersected conservatively), while scalar members narrow compatible primitives, enums, arrays, objects, unions, and nullability. Examples include integer within number, enum within its scalar type, and a detailed object within a broader object; an empty array-item intersection becomes an uninhabited item type so the valid empty array remains representable. It is rejected only when the overall intersection is empty or cannot be represented faithfully: incompatible scalar categories, conflicting property/additional-value constraints, an object/scalar mix, or a direct recursive `$ref` member whose fields are not yet known. Restructure the composition or omit this API segment with `spargen::omit!`."
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
                "A request body or response offered more than one media type. A generated method sends and decodes exactly one, so spargen picks the one it can represent best — JSON first, then XML, multipart, form-urlencoded, octet-stream, text, and finally the sequential media, with media *ranges* below all of them (`text/*`, then every other family such as `video/*` or `*/*`), so a concrete media type always outranks a range — breaking ties by source order. That is normally a real narrowing of the documented API surface: a server willing to accept XML as well as JSON will only ever be sent JSON by this client. It is reported rather than left silent so the choice is visible, and it is a warning rather than an error because the selected media type is genuinely supported and the generated client is correct for it. An alternative that decodes to exactly the same thing is not reported, because nothing is given up: that means it shares the selection's codec and both constrain nothing at all, so both lower to that codec's single representation — several opaque entries on a ranged response are all `bytes::Bytes`, and `text/plain` beside `text/csv` is `String` twice. Emptiness is proved from each schema rather than assumed from the media type, a validation keyword such as `maxLength` counts as saying something, and two different codecs are never treated as interchangeable even when they happen to lower alike. To generate against a different one, remove the alternatives from the document, or omit this API segment with `spargen::omit!` and hand-write the call."
            }
            Code::SchemaNestingTooDeep => {
                "Lowering a schema into a Rust type is recursive: each nested object property, array item, `allOf`/`oneOf`/`anyOf` member, and `$ref` target descends one level. Spargen caps that descent so a pathologically deep composition — a very long chain of components that each `$ref` the next, or a deeply nested inline schema — is rejected with this error instead of being allowed to exhaust the call stack and abort the process. A genuine API surface never approaches the limit; hitting it almost always means the spec was machine-generated or adversarial. Flatten the offending chain, or omit that API segment with `spargen::omit!`."
            }
            Code::RuntimeDependencyContract => {
                "The generated module is freestanding, so its consuming Cargo package must declare the crates and dependency features referenced by that specific API. Spargen derives the exact requirement set after lowering and audits Cargo.toml during build.rs and proc-macro generation. Use the documented tested lower bounds (or a higher semver-compatible caret floor), keep reqwest default features disabled, and enable only the reqwest/bytes/XML/UUID/time capabilities named by the diagnostic. A dependency declared `workspace = true` is followed to the workspace root's `[workspace.dependencies]` — the root named by `package.workspace`, the nearest ancestor manifest declaring `[workspace]`, or the consumer manifest itself when it is the root — taking the version from there and the union of both feature lists (`optional` is read from the member, as Cargo does), so inheriting a required crate satisfies the audit; when the root cannot be found, cannot be read, or declares no such entry, the diagnostic says which of those happened rather than reporting the crate as missing. Cargo resolves the declared range; Rust compilation then verifies the selected crates expose the APIs and traits used by the generated client."
            }
            Code::SpecUndefinedBehavior => {
                "The OpenAPI Specification marks some constructs' behavior as *undefined* rather than leaving them merely unsupported. Currently this fires for a Path Item `$ref` declared alongside structural fields (operations, `parameters`, `servers`): the specification says that when a field appears both in the referring Path Item and the referenced one, the behavior is undefined. Unlike a Reference Object, which requires adjacent properties to be ignored, there is no rule to follow — so either choice (the local fields winning, or the referenced ones) silently discards operations the author wrote, and produces a client that calls a different set of endpoints than the document describes. `summary` and `description` are exempt because they are documentation and cannot change the wire. Move the sibling fields into the referenced Path Item, or drop the `$ref` and declare the item inline."
            }
            Code::TupleRestNotRepresentable => {
                "In JSON Schema 2020-12 `prefixItems` fixes the leading positions of an array and `items` describes every position after them. Spargen lowers `prefixItems` to a Rust tuple, which is fixed-length, so a schema that also allows a typed remainder describes a value no single Rust type expresses: a tuple cannot grow, and a `Vec` cannot hold the distinct per-position types. `items: false` closes the array at the prefix and is fully supported — that is exactly a tuple. To send a variable-length remainder, drop `prefixItems` and describe the whole array with `items`, split the fixed head into its own object properties, or omit this API segment with `spargen::omit!`."
            }
            Code::DeclarationHasNoEffect => {
                "The document declared something the specification permits here, but which cannot change any byte spargen generates or sends, so it is acknowledged rather than dropped in silence. It fires for: `allowReserved` on a parameter that is never percent-encoded (an `in: header` parameter, or `style: cookie`, both of which the specification sends verbatim); `encoding`, `prefixEncoding`, or `itemEncoding` on a media type that is neither `multipart` nor `application/x-www-form-urlencoded`, where the specification says those fields SHALL be ignored; an `encoding` entry naming a property the body schema does not declare; `encoding.headers` on a non-`multipart` media type; an `encoding.headers` Header Object that pins no `const`/`default` value, leaving a client nothing to send; `allowEmptyValue`, which is deprecated and cannot change what a typed client omits; a `mutualTLS` security scheme, which is satisfied by the transport's client certificate rather than by anything the client attaches; a response header named `Content-Type`, which the specification says SHALL be ignored; a response header whose `content` media type spargen cannot decode, or whose textual `content` schema is not a single value, or which declares `content` with no schema at all, none of which yields a typed accessor; a `servers` entry past the first on a path item or operation, where the specification defines no client selection rule; and a union branch that the enclosing schema's own constraints have already made unsatisfiable. None of these is an error: the document is valid, and the construct simply has no reachable effect on this client."
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
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/errors.md");
        let Ok(index) = std::fs::read_to_string(path) else {
            // Absent when the crate is tested from a packaged `.crate`, which carries no docs
            // directory. In the repository — where the invariant matters — it is always there.
            eprintln!("skipping: {path} is not present");
            return;
        };
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
    #[test]
    fn the_support_documents_cite_the_codes_that_exist_where_they_belong() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        let matrix_path = format!("{root}/docs/support-matrix.md");
        let scope_path = format!("{root}/docs/openapi-3.2.md");
        let (Ok(matrix), Ok(scope)) = (
            std::fs::read_to_string(&matrix_path),
            std::fs::read_to_string(&scope_path),
        ) else {
            eprintln!("skipping: {matrix_path} is not present");
            return;
        };

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
}
