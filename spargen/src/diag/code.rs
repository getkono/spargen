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
    /// A `$ref` targets an absolute URL; only local relative-file refs are supported.
    AbsoluteRefUnsupported,
    /// A `$ref` could not be resolved within the input bundle.
    UnresolvedRef,
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
}

impl Code {
    /// The stable string form, e.g. `"E001"` or `"W004"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => "E001",
            Code::UnsupportedDialect => "E002",
            Code::AbsoluteRefUnsupported => "E003",
            Code::UnresolvedRef => "E004",
            Code::ValidationKeywordIgnored => "W001",
            Code::PatternPropertiesRejected => "E005",
            Code::DynamicRefRejected => "E006",
            Code::NonDisjointUnion => "E007",
            Code::NonScalarEnum => "E008",
            Code::UnsupportedMediaType => "E009",
            Code::UnsupportedParameterStyle => "E010",
            Code::ServerInitiatedFlowIgnored => "W002",
            Code::InvalidInput => "E011",
            Code::UnknownSecurityScheme => "E012",
            Code::AllOfIrreconcilable => "E013",
            Code::OutputDrifted => "W004",
            Code::OmittedConstruct => "W009",
            Code::InvalidOmitRule => "E019",
            Code::OmitCreatedInvalidDocument => "E020",
            Code::SchemaDefaultNotApplied => "W005",
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
            Code::AbsoluteRefUnsupported => "absolute $ref unsupported",
            Code::UnresolvedRef => "unresolved $ref",
            Code::ValidationKeywordIgnored => "validation-only keyword ignored",
            Code::PatternPropertiesRejected => "patternProperties not representable as a typed map",
            Code::DynamicRefRejected => "dynamic reference unsupported",
            Code::NonDisjointUnion => "union variants are not disjoint",
            Code::NonScalarEnum => "enum values are not homogeneous scalars",
            Code::UnsupportedMediaType => "unsupported media type",
            Code::UnsupportedParameterStyle => "unsupported parameter style",
            Code::ServerInitiatedFlowIgnored => "server-initiated flow ignored",
            Code::InvalidInput => "invalid input document",
            Code::UnknownSecurityScheme => "unknown security scheme",
            Code::AllOfIrreconcilable => "irreconcilable allOf composition",
            Code::OutputDrifted => "checked-in output drifted",
            Code::InvalidOmitRule => "invalid omit rule",
            Code::OmittedConstruct => "construct omitted",
            Code::OmitCreatedInvalidDocument => "omit profile created an invalid document",
            Code::SchemaDefaultNotApplied => "schema default not applied",
        }
    }

    /// Extended documentation shown by `spargen explain E###` and on the published errors index.
    pub fn explain(self) -> &'static str {
        match self {
            Code::UnsupportedOpenApiVersion => {
                "The root `openapi` field must declare `3.1.x`. OpenAPI 3.0.x uses a different schema dialect and is rejected rather than converted."
            }
            Code::UnsupportedDialect => {
                "`jsonSchemaDialect`, when present, must be the OAS 3.1 base dialect (`https://spec.openapis.org/oas/3.1/dialect/base`)."
            }
            Code::AbsoluteRefUnsupported => {
                "Remote or absolute-URL `$ref` targets are not fetched. Vendor them locally and reference them by relative file path."
            }
            Code::UnresolvedRef => {
                "A `$ref` target could not be found in the loaded input bundle. Check the file path and JSON Pointer fragment."
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
                "Enums and const values must be homogeneous scalar sets. A `null` member (or `\"null\"` in the schema's type array) is allowed: it is stripped and makes the generated type nullable (`Option<Enum>`), and a value set of only `null` lowers to a nullable untyped value. Sets that mix distinct scalar kinds (e.g. a string with an integer) or that contain object/array members are rejected."
            }
            Code::UnsupportedMediaType => {
                "Only application/json, application/x-www-form-urlencoded, application/octet-stream, text/plain, and multipart/form-data (request bodies) are currently generated. A multipart/form-data body must be an object schema — its properties become the form parts (a binary/bytes property is a file part, a scalar or composite becomes a text part) — so a non-object multipart body is rejected here."
            }
            Code::UnsupportedParameterStyle => {
                "Only simple/form styles and JSON content-typed parameters are generated. Deep object, pipe-delimited, and space-delimited styles are rejected."
            }
            Code::ServerInitiatedFlowIgnored => {
                "Webhooks, callbacks, and links describe server-initiated or hypermedia behavior. They are acknowledged with a warning and no client code is emitted."
            }
            Code::InvalidInput => {
                "The input is malformed JSON/YAML or is missing a required OpenAPI structure needed before feature auditing can continue."
            }
            Code::UnknownSecurityScheme => {
                "Every scheme named in a `security` requirement must be declared under `components.securitySchemes` as `http` bearer/basic, `apiKey`, `oauth2`, or `openIdConnect` so credentials can be attached at the right location."
            }
            Code::AllOfIrreconcilable => {
                "`allOf` members are merged into one type: object members flatten into a single struct (union of properties; a property required by any member is required; `additionalProperties` merged conservatively — a member that denies unknown keys wins), and all-scalar members that lower to the same primitive collapse to that primitive. It is rejected as irreconcilable only when the members cannot form one type: a property name appears with conflicting lowered types across members, `additionalProperties` conflict (e.g. two different typed value schemas), object and scalar members are mixed, distinct scalar members disagree, or a member is a direct recursive `$ref` to the component currently being lowered (its fields are not yet known). Restructure the composition or omit this API segment with `spargen::omit!`."
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
