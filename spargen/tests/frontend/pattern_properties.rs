//! `patternProperties` as a typed overflow map: which value schemas share one map, and the
//! `E005` cases that cannot.

use super::*;

#[test]
fn pattern_properties_lowers_to_typed_map_with_w001() {
    // A representable `patternProperties` now GENERATES a typed overflow map instead of being
    // rejected. Two inline `{type: string}` value schemas under different patterns collapse to one
    // `BTreeMap<String, String>`: both emit the one Rust type `String`. The key
    // regexes are validation-only and acknowledged as `W001`, never silently dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^x-": { type: string }
        "^y-": { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    // check/generate parity: the same disposition is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "{checked:#?}"
    );
}

#[test]
fn pattern_properties_cyclic_array_values_terminate() {
    // Mutually-recursive array value schemas (`A = [B]`, `B = [A]`) close an alias cycle, so each
    // is emitted as its own nominal newtype. The homogeneity comparison must terminate (return an
    // outcome, never abort with a stack overflow) and, since the two newtypes are distinct Rust
    // types one map cannot share, reject with `E005` (#733). The check/generate parity assertion
    // catches a regression that reintroduces the crash.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: array, items: { $ref: "#/components/schemas/B" } }
    B: { type: array, items: { $ref: "#/components/schemas/A" } }
    Thing:
      type: object
      patternProperties:
        "^a-": { $ref: "#/components/schemas/A" }
        "^b-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e005_pattern_properties_heterogeneous_rejected() {
    // Two pattern value schemas that lower to different types cannot share one typed map → E005.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: string }
    B: { type: integer }
    Thing:
      type: object
      patternProperties:
        "^s-": { $ref: "#/components/schemas/A" }
        "^i-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
    // check/generate parity: the rejection fires in `check` too.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::PatternPropertiesRejected));
}

/// `patternProperties` values whose integer `enum`s (or an integer `enum` beside `format: int64`)
/// all emit `i64` share one typed map: an integer enum is a transparent `i64` alias (#733).
#[test]
fn pattern_properties_integer_enum_values() {
    for (label, second) in [
        ("two integer enums", "{ type: integer, enum: [3, 4] }"),
        (
            "an integer enum beside int64",
            "{ type: integer, format: int64 }",
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths: {{}}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^a-": {{ type: integer, enum: [1, 2] }}
        "^b-": {second}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
        assert!(
            !has_code(&report, Code::PatternPropertiesRejected),
            "{label}: {report:#?}"
        );
        assert!(
            code.contains("BTreeMap<String, ThingValue>")
                && code.contains("pub type ThingValue = i64;"),
            "{label}: {code}"
        );
        let checked = check(&spec);
        assert_ne!(
            checked.outcome(),
            Outcome::Rejected,
            "{label}: {checked:#?}"
        );
    }
}

/// `patternProperties` values that are two distinct `false` schemas, each its own uninhabited
/// `Never` item, emit two Rust types, so one map cannot type them both: `E005` (#733).
#[test]
fn e005_pattern_properties_distinct_false_values_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^a-": false
        "^b-": false
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::PatternPropertiesRejected),
        "{checked:#?}"
    );
}

#[test]
fn e005_pattern_properties_with_deny_rejected() {
    // `patternProperties` + `additionalProperties: false` cannot be faithfully represented → E005.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      additionalProperties: false
      patternProperties:
        "^x-": { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
}
