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
}

impl Code {
    /// The stable string form, e.g. `"E001"` or `"W003"`.
    pub fn as_str(self) -> &'static str {
        todo!()
    }

    /// Whether this code is an error or a warning.
    pub fn severity(self) -> Severity {
        todo!()
    }

    /// The one-line human title.
    pub fn title(self) -> &'static str {
        todo!()
    }

    /// Extended documentation shown by `spargen explain E###` and on the published errors index.
    pub fn explain(self) -> &'static str {
        todo!()
    }

    /// The interpretation this code's behavior depends on, if any (PRD §3.3).
    pub fn interpretation(self) -> Option<InterpId> {
        todo!()
    }

    /// Every code, in stable order — drives the exhaustiveness test and docs generation.
    pub fn all() -> &'static [Code] {
        todo!()
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
        todo!()
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
