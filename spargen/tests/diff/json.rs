//! The JSON wire shape of a report.

use super::*;

/// `--format json` is the machine surface of `spargen diff`, and the CLI renders it straight from
/// `DiffReport`'s `Serialize` (`spargen/src/cli/run.rs`). The enum-level pinning lives in
/// `surface`'s own tests; this drives a real report through `serde_json` so the field names, the
/// nesting, and the spellings a script actually parses are all fixed at once.
#[test]
fn the_json_report_names_kinds_by_code_and_impacts_in_lowercase() {
    let report = diff(&base(), &spec("", "id", PET_PROPS, EXTRA_OP));
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).expect("a report serializes"))
            .unwrap();

    assert_eq!(json["bump"], "minor");
    let changes = json["changes"].as_array().expect("changes is an array");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0]["kind"], "operation-added");
    assert_eq!(changes[0]["impact"], "minor");
    assert_eq!(changes[0]["location"], "GET /owners");
    assert!(
        changes[0]["detail"].is_string(),
        "detail is a human string: {:?}",
        changes[0]
    );
}

/// The `major` spelling is the one `--exit-code` callers branch on, so pin it separately from the
/// additive case rather than assuming the enum renders uniformly.
#[test]
fn a_breaking_json_report_spells_the_bump_major() {
    let report = diff(&base(), &spec(PARAM_REQUIRED_INT, "id", PET_PROPS, ""));
    let rendered = serde_json::to_string(&report).expect("a report serializes");

    assert!(rendered.contains(r#""bump":"major""#), "{rendered}");
    assert!(
        rendered.contains(r#""kind":"required-param-added""#),
        "{rendered}"
    );
    // The Rust variant names are an implementation detail and must not reach the wire.
    assert!(!rendered.contains("RequiredParamAdded"), "{rendered}");
    assert!(!rendered.contains("Major"), "{rendered}");
}
