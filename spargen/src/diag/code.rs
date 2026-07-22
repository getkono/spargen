use serde::Serialize;

use super::{InterpId, Severity};

/// A stable diagnostic code — `E###` for errors, `W###` for warnings.
///
/// Codes are product surface: each has [`explain`](Code::explain) text, a docs entry, and at
/// least one fixture that triggers it. The set is closed and exhaustively
/// enumerable via [`all`](Code::all) so the docs/behavior exhaustiveness test can iterate it and
/// fail the build if code and docs diverge. `#[non_exhaustive]` keeps adding a code a non-breaking
/// change for external matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub enum Code {
    /// The `openapi` field declares an unsupported version (e.g. 3.0.x); no conversion is
    /// offered.
    UnsupportedOpenApiVersion,
    /// `jsonSchemaDialect` is not the default OAS 3.1 dialect.
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
    /// A `oneOf`/`anyOf` union could not be represented: a discriminated variant is not an object,
    /// or an undiscriminated union is not provably disjoint (by JSON type category, or a unique
    /// required key across closed object variants) so a payload cannot be routed unambiguously.
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
    /// `generate --check` found checked-in output that drifted from (or is missing against) the
    /// spec.
    OutputDrifted,
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
    /// An OpenAPI 3.2-only construct (`$self`, `additionalOperations`, `in: querystring`) was
    /// acknowledged but not lowered into generated code (matrix: Version → W).
    Oas32ConstructIgnored,
    /// Schema composition nests deeper than spargen will lower (a very long `$ref` chain or a
    /// pathologically nested inline schema), so lowering is stopped before it could exhaust the
    /// stack. Rejected rather than risk a crash on adversarial or machine-generated input.
    SchemaNestingTooDeep,
}

impl Code {
    /// The stable string form, e.g. `"E001"` or `"W004"`.
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
            Code::OutputDrifted => "W004",
            Code::OmittedConstruct => "W009",
            Code::InvalidOmitRule => "E019",
            Code::OmitCreatedInvalidDocument => "E020",
            Code::SchemaDefaultNotApplied => "W005",
            Code::XmlHintIgnored => "W006",
            Code::Oas32ConstructIgnored => "W010",
            Code::SchemaNestingTooDeep => "E014",
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
            Code::NonDisjointUnion => "union variants are not disjoint",
            Code::NonScalarEnum => "enum values are not homogeneous scalars",
            Code::UnsupportedMediaType => "unsupported media type",
            Code::UnsupportedParameterStyle => "unsupported parameter style",
            Code::ServerInitiatedFlowIgnored => "server-initiated flow ignored",
            Code::InvalidInput => "invalid input document",
            Code::DuplicateObjectKey => "duplicate object key",
            Code::UnknownSecurityScheme => "unknown security scheme",
            Code::AllOfIrreconcilable => "irreconcilable allOf composition",
            Code::OutputDrifted => "checked-in output drifted",
            Code::InvalidOmitRule => "invalid omit rule",
            Code::OmittedConstruct => "construct omitted",
            Code::OmitCreatedInvalidDocument => "omit profile created an invalid document",
            Code::SchemaDefaultNotApplied => "schema default not applied",
            Code::XmlHintIgnored => "unsupported XML hint ignored",
            Code::Oas32ConstructIgnored => "OpenAPI 3.2 construct ignored",
            Code::SchemaNestingTooDeep => "schema nesting is too deep to lower",
        }
    }

    /// Extended documentation shown by `spargen explain E###` and on the published errors index.
    pub fn explain(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => {
                "The root `openapi` field must declare `3.1.x` or `3.2.x`. OpenAPI 3.2 is a compatible superset of 3.1 (same JSON Schema 2020-12 semantics) and is accepted through the same frontend. OpenAPI 3.0.x uses a different schema dialect and is rejected rather than converted."
            }
            Code::UnsupportedDialect => {
                "`jsonSchemaDialect`, when present, must be the OAS 3.1 base dialect (`https://spec.openapis.org/oas/3.1/dialect/base`) or the OAS 3.2 base dialect (`https://spec.openapis.org/oas/3.2/dialect/base`); both are the JSON Schema 2020-12-based OAS dialect."
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
                "The keyword affects runtime validation but not the static Rust shape. Spargen records a warning and generates the shape."
            }
            Code::PatternPropertiesRejected => {
                "`patternProperties` is represented as a typed overflow map (`#[serde(flatten)]`) when every pattern value schema — and any typed `additionalProperties` value — lowers to the same type; the key regex itself is validation-only and reported as `W001`. It is rejected only when a faithful map is impossible: heterogeneous value types (which one map cannot type), or a combination with `additionalProperties: false` (a flatten map cannot both capture pattern values and deny other unknown keys)."
            }
            Code::DynamicRefRejected => {
                "`$dynamicRef` and `$dynamicAnchor` require dynamic schema-scope evaluation and are rejected."
            }
            Code::NonDisjointUnion => {
                "`oneOf`/`anyOf` unions are lowered to Rust enums with a custom `Deserialize`/`Serialize` — never `serde(untagged)` and never degraded to `serde_json::Value`. A union with a `discriminator` reads/writes the tag field on a buffered value, so every variant must be an object (a `$ref` to an object component or an inline object); a primitive/array/untyped variant is rejected. A union without a discriminator is emitted only when it is statically disjoint: either every variant occupies a distinct JSON type category (integer and number share one category and never separate), or every variant is a *closed* object (`additionalProperties: false`) with at least one required property whose name appears in no other variant — closed is required because an open object could carry another variant's unique key as an extra field and be misrouted. It is rejected only when neither proof holds — overlapping JSON types, open or non-uniquely-keyed object variants, an untyped variant, or a variant that is itself a union. Add or fix the discriminator, make the object variants closed with disjoint required keys, or omit this API segment with `spargen::omit!`."
            }
            Code::NonScalarEnum => {
                "Enums and const values must be homogeneous scalar sets. A `null` member (or `\"null\"` in the schema's type array) is allowed: it is stripped and makes a remaining scalar enum nullable (`Option<Enum>`), while a value set of only `null` lowers to the exact JSON null type (`()`). Sets that mix distinct non-null scalar kinds (e.g. a string with an integer) or that contain object/array members are rejected."
            }
            Code::UnsupportedMediaType => {
                "Only application/json, application/xml (and text/xml), application/x-www-form-urlencoded, application/octet-stream, text/plain, and multipart/form-data (request bodies) are currently generated; a streaming response media (text/event-stream or application/x-ndjson) generates a typed async EventStream<T>. An application/xml or text/xml body lowers to the same typed struct as JSON and is serialized/decoded through the runtime's feature-gated quick-xml codec (JSON still wins when both are offered); it is supported only as an operation's single success or single error body, so an XML body in a multi-status response enum is rejected. A multipart/form-data body must be an object schema — its properties become the form parts (a binary/bytes property is a file part, a scalar or composite becomes a text part) — so a non-object multipart body is rejected here. text/event-stream and application/x-ndjson are supported only for response bodies, so a streaming request body is rejected."
            }
            Code::UnsupportedParameterStyle => {
                "Path/header parameters support simple style and query/cookie parameters support form style, including the OpenAPI explode defaults and explicit explode overrides. JSON content-typed parameters are generated. Deep object, pipe-delimited, space-delimited, invalid location/style combinations, and allowReserved: true are rejected rather than serialized with incorrect wire semantics."
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
            Code::OutputDrifted => {
                "The checked-in generated code no longer matches what this spec and spargen version produce. Re-run `spargen generate` and commit the result."
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
                "OpenAPI 3.2 is accepted through the 3.1 frontend, but a handful of 3.2-only constructs describe behavior spargen does not generate a client for, and are acknowledged with this warning rather than silently dropped. `$self` sets the document's base URI for reference resolution and does not change locally-generated code. `additionalOperations` declares custom/extension HTTP methods on a path item, for which no client method is emitted. An `in: querystring` parameter treats the entire URL query string as a single value; that parameter is skipped and the rest of the operation still generates. The new fixed `QUERY` method is fully supported and does NOT trigger this warning."
            }
            Code::SchemaNestingTooDeep => {
                "Lowering a schema into a Rust type is recursive: each nested object property, array item, `allOf`/`oneOf`/`anyOf` member, and `$ref` target descends one level. Spargen caps that descent so a pathologically deep composition — a very long chain of components that each `$ref` the next, or a deeply nested inline schema — is rejected with this error instead of being allowed to exhaust the call stack and abort the process. A genuine API surface never approaches the limit; hitting it almost always means the spec was machine-generated or adversarial. Flatten the offending chain, or omit that API segment with `spargen::omit!`."
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
            Code::OutputDrifted,
            Code::OmittedConstruct,
            Code::SchemaDefaultNotApplied,
            Code::XmlHintIgnored,
            Code::Oas32ConstructIgnored,
            Code::SchemaNestingTooDeep,
        ];
        ALL
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
}
