//! Keyword-escaped identifiers.

use super::*;

/// A `Pet` whose properties include two Rust keywords, so the generated struct carries the raw
/// identifiers `r#type` and `r#gen` — `gen` being reserved only since edition 2024, which is why
/// spargen escapes against the union of every edition's reserved words.
const KEYWORD_PET_SCHEMA: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        type: { type: string }
        gen: { type: string }
";

/// The surface records identifiers exactly as `name` escapes them, `r#` prefix and all, so a
/// keyword-named operation or field is stored as `r#gen` rather than `gen`. That cannot desync the
/// two sides of a diff: `spargen::diff` lowers both specs in one process through one
/// `name::allocate` and one keyword table, and no `Surface` is ever serialized or persisted, so
/// there is no path by which one side is escaped and the other is not. Pinned here because the
/// asymmetry is the plausible-looking bug that this arrangement rules out.
#[test]
fn a_keyword_named_operation_and_field_are_not_a_false_rename() {
    let spec = full(&pets_get("gen", "", PET_REF), KEYWORD_PET_SCHEMA);
    let report = diff(&spec, &spec);

    assert!(
        report.changes.is_empty(),
        "a spec is identical to itself however its identifiers escape: {:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Patch);
}

/// The converse, so the test above proves symmetry rather than an inert detector: a real rename of
/// the same keyword-named operation and field is still classified, and still breaking.
#[test]
fn renaming_a_keyword_named_operation_and_field_is_still_major() {
    let renamed_field = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        kind: { type: string }
        gen: { type: string }
";
    let old = full(&pets_get("gen", "", PET_REF), KEYWORD_PET_SCHEMA);
    let new = full(&pets_get("generate", "", PET_REF), renamed_field);
    let report = diff(&old, &new);

    assert_eq!(
        kinds(&report),
        vec![
            ChangeKind::MethodRenamed,
            ChangeKind::FieldRemoved,
            ChangeKind::FieldAdded,
        ],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);

    // Not vacuous: the field changes are located by the escaped identifier, which is what makes
    // this pair a test of keyword-escaped names rather than of ordinary ones.
    let locations: Vec<&str> = report
        .changes
        .iter()
        .map(|change| change.location.as_str())
        .collect();
    assert!(locations.contains(&"Pet.r#type"), "{locations:?}");

    // The rename is reported in the spelling a consumer writes at the call site.
    let renamed = report
        .changes
        .iter()
        .find(|change| change.kind == ChangeKind::MethodRenamed)
        .expect("the method rename is reported");
    assert!(
        renamed.detail.contains("r#gen") && renamed.detail.contains("generate"),
        "{}",
        renamed.detail
    );
}
