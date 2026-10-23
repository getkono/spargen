use serde::Serialize;

/// Identifies a numbered interpretation: a chosen reading where the normative sources are
/// ambiguous or in tension. A diagnostic carries the id of the interpretation its code depends
/// on; `Display` renders it as `INT-###`, and JSON output and the build cache carry the bare
/// number.
///
/// The ids [`Code::interpretation`](crate::Code::interpretation) assigns:
///
/// - `INT-001` (`E001`): OpenAPI 3.2 is read as a compatible superset of 3.1 and accepted through
///   the same frontend, while 3.0.x, whose schema dialect differs, is rejected rather than
///   converted.
/// - `INT-002` (`W001`): a validation-only keyword shapes no generated type, so the type is
///   generated and the keyword is reported rather than enforced.
/// - `INT-003` (`E007`): a `oneOf`/`anyOf` union becomes a typed enum decoded by discriminator,
///   disjoint category, or typed trial matching, and a union that none of these can represent is
///   rejected rather than degraded to `serde_json::Value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct InterpId(pub u16);

impl std::fmt::Display for InterpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "INT-{:03}", self.0)
    }
}
