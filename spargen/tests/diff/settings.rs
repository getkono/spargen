//! The generation settings that change the generated API without a change to the spec: the `uuid`
//! mapping, the `datetime` backend, and the `testing` module. Each side of a diff is resolved on
//! its own (the CLI reads a `spargen.toml` beside each spec), so a pair of identical specs whose
//! settings differ has a surface change to report.

use super::*;

/// One `get /pets` operation returning `Pet`, whose `id` is a `uuid`, `born` a `date-time` and
/// `day` a `date`, every one of them required.
fn formatted() -> String {
    full(
        &pets_get("listPets", "", PET_REF),
        "    Pet:
      type: object
      required: [id, born, day]
      properties:
        id: { type: string, format: uuid }
        born: { type: string, format: date-time }
        day: { type: string, format: date }
",
    )
}

/// The changes' `location: detail` lines, for asserting the labels a consumer reads.
fn details(report: &DiffReport) -> Vec<String> {
    report
        .changes
        .iter()
        .map(|change| format!("{}: {}", change.location, change.detail))
        .collect()
}

/// Mapping `format: uuid` to `String` instead of `uuid::Uuid` retypes every `uuid` member, which is
/// breaking in both directions; the same mapping on both sides is no change.
#[test]
fn a_changed_uuid_mapping_retypes_every_uuid_member() {
    let to_string = |spec: Spec| spec.with_uuid(spargen::UuidMapping::String);

    let report = diff_configured(&formatted(), &formatted(), |spec| spec, to_string);
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::FieldTypeChanged],
        "{report:?}"
    );
    assert_eq!(report.bump, Impact::Major);
    assert_eq!(
        details(&report),
        vec!["Pet.id: field type `Uuid` -> `String`"]
    );

    let back = diff_configured(&formatted(), &formatted(), to_string, |spec| spec);
    assert_eq!(kinds(&back), vec![ChangeKind::FieldTypeChanged], "{back:?}");
    assert_eq!(back.bump, Impact::Major);

    let same = diff_configured(&formatted(), &formatted(), to_string, to_string);
    assert!(same.changes.is_empty(), "{same:?}");
    assert_eq!(same.bump, Impact::Patch);
}

/// Each date-time backend carries its own `DateTime`/`Date` newtypes, over its own crate's types,
/// and `string` maps both formats to `String`: moving between any two retypes every `date-time`
/// and `date` member, which is breaking.
#[test]
fn a_changed_datetime_backend_retypes_every_date_member() {
    use spargen::DateTimeBackend;

    for (old, new) in [
        (DateTimeBackend::Time, DateTimeBackend::Jiff),
        (DateTimeBackend::Jiff, DateTimeBackend::Chrono),
        (DateTimeBackend::Time, DateTimeBackend::String),
        (DateTimeBackend::String, DateTimeBackend::Chrono),
    ] {
        let report = unsnapshotted_diff(
            &formatted(),
            &formatted(),
            |spec| spec.with_datetime(old),
            |spec| spec.with_datetime(new),
        );
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldTypeChanged, ChangeKind::FieldTypeChanged],
            "{old} -> {new}: {report:?}"
        );
        assert_eq!(report.bump, Impact::Major, "{old} -> {new}");
    }

    let report = diff_configured(
        &formatted(),
        &formatted(),
        |spec| spec,
        |spec| spec.with_datetime(DateTimeBackend::Jiff),
    );
    assert_eq!(
        details(&report),
        vec![
            "Pet.born: field type `DateTime<time>` -> `DateTime<jiff>`",
            "Pet.day: field type `Date<time>` -> `Date<jiff>`",
        ]
    );

    for backend in DateTimeBackend::ALL {
        let same = unsnapshotted_diff(
            &formatted(),
            &formatted(),
            |spec| spec.with_datetime(*backend),
            |spec| spec.with_datetime(*backend),
        );
        assert!(same.changes.is_empty(), "{backend}: {same:?}");
    }
}
