use serde::Serialize;

use super::{InterpId, Severity};

/// A stable diagnostic code — `E###` for errors, `W###` for warnings (PRD FR6).
///
/// Codes are product surface: each has [`explain`](Code::explain) text, a docs entry, and at
/// least one fixture that triggers it (PRD §7.5). The set is closed and exhaustively
/// enumerable via [`all`](Code::all) so the docs/behavior exhaustiveness test can iterate it and
/// fail the build if code and docs diverge. `#[non_exhaustive]` keeps adding a code a non-breaking
/// change for external matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub enum Code {
    /// The `openapi` field declares an unsupported version (e.g. 3.0.x); no conversion is
    /// offered (PRD FR1, §3.2.1).
    UnsupportedOpenApiVersion,
    /// `jsonSchemaDialect` is not the default OAS 3.1 dialect (PRD FR1, §3.3).
    UnsupportedDialect,
    /// A `$ref` targets an absolute URL; only local relative-file refs are supported (§3.2.10).
    AbsoluteRefUnsupported,
    /// A `$ref` could not be resolved within the input bundle (PRD §3.3 prec 6).
    UnresolvedRef,
    /// A validation-only keyword (`pattern`, `minimum`, …) was ignored (PRD FR2, W-class).
    ValidationKeywordIgnored,
    /// `patternProperties` is not represented in generated types (matrix: Schema shape → R).
    PatternPropertiesRejected,
    /// `$dynamicRef`/`$dynamicAnchor` are rejected (matrix: Schema shape → R).
    DynamicRefRejected,
    /// A `oneOf`/`anyOf` with non-disjoint variant sets cannot be deserialized unambiguously;
    /// names the overlapping variants and suggests a `discriminator` (PRD D1).
    NonDisjointUnion,
    /// A heterogeneous or structured `enum`/`const` value set is rejected (PRD D6).
    NonScalarEnum,
    /// A request body media type spargen does not support (XML, multipart, …) (§3.1, §3.2.8).
    UnsupportedMediaType,
    /// An unsupported parameter style (`deepObject`, `spaceDelimited`, …) (matrix: Parameters → R).
    UnsupportedParameterStyle,
    /// `webhooks`/`callbacks`/`links` acknowledged; no code emitted (matrix: Document → W).
    ServerInitiatedFlowIgnored,
    /// A `security` requirement references a scheme that is not declared under
    /// `components.securitySchemes` (or is of an unsupported type) (matrix: Security).
    UnknownSecurityScheme,
    /// The input could not be parsed or violates a required structural OpenAPI shape.
    InvalidInput,
    /// A compatibility omit rule did not match a source construct or attempted an invalid removal.
    InvalidOmitRule,
    /// A compatibility omit profile removed a construct.
    OmittedConstruct,
    /// A compatibility omit profile created an invalid remaining document.
    OmitCreatedInvalidDocument,
}

impl Code {
    /// The stable string form, e.g. `"E001"` or `"W003"`.
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
            Code::OmittedConstruct => "W009",
            Code::InvalidOmitRule => "E019",
            Code::OmitCreatedInvalidDocument => "E020",
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
            Code::PatternPropertiesRejected => "patternProperties unsupported",
            Code::DynamicRefRejected => "dynamic reference unsupported",
            Code::NonDisjointUnion => "union variants are not disjoint",
            Code::NonScalarEnum => "enum values are not homogeneous scalars",
            Code::UnsupportedMediaType => "unsupported media type",
            Code::UnsupportedParameterStyle => "unsupported parameter style",
            Code::ServerInitiatedFlowIgnored => "server-initiated flow ignored",
            Code::InvalidInput => "invalid input document",
            Code::UnknownSecurityScheme => "unknown security scheme",
            Code::InvalidOmitRule => "invalid omit rule",
            Code::OmittedConstruct => "construct omitted",
            Code::OmitCreatedInvalidDocument => "omit profile created an invalid document",
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
                "`patternProperties` changes object shape in a way Spargen does not represent yet. Use explicit properties or omit this API segment."
            }
            Code::DynamicRefRejected => {
                "`$dynamicRef` and `$dynamicAnchor` require dynamic schema-scope evaluation and are rejected."
            }
            Code::NonDisjointUnion => {
                "A `oneOf`/`anyOf` without a discriminator must be statically disjoint. Add a discriminator or omit the unsupported operation/schema."
            }
            Code::NonScalarEnum => {
                "Enums and const values must be homogeneous scalar sets. Structured or mixed-type value sets are rejected."
            }
            Code::UnsupportedMediaType => {
                "Only application/json, application/x-www-form-urlencoded, application/octet-stream, and text/plain are currently generated."
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
            Code::InvalidOmitRule => {
                "A compatibility omit rule must match at least one exact path, operation, component, pointer, or file-local pointer and cannot omit the document root."
            }
            Code::OmittedConstruct => {
                "A compatibility omit profile removed this construct before OpenAPI validation/lowering. The source schema on disk was not modified."
            }
            Code::OmitCreatedInvalidDocument => {
                "After applying omit rules, the remaining document is structurally invalid. Omit dependent consumers too, or fix the source schema."
            }
        }
    }

    /// The interpretation this code's behavior depends on, if any (PRD §3.3).
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
            Code::InvalidOmitRule,
            Code::OmitCreatedInvalidDocument,
            Code::ValidationKeywordIgnored,
            Code::ServerInitiatedFlowIgnored,
            Code::OmittedConstruct,
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
