//! Open string enums: every string enum is open by default, wherever it sits, `closed_enums`
//! closes every one, and neither switch nor `open_narrowing` reports anything of its own. The spec
//! is the one `e2e/open_enums.rs` compiles and drives over the wire, so the positions pinned here
//! are the ones that suite decodes.

use super::*;

/// The RFC 9457 problem spec from #268, shared with `e2e/open_enums.rs`, which documents its
/// statuses.
const OPEN_ENUMS_SPEC: &str = include_str!("../e2e/specs/open_enums.yaml");

/// `generate` over [`OPEN_ENUMS_SPEC`] configured by `configure`, through the harness's
/// [`generate_spec_with_code`], returning the report, the `(code, pointer)` of each diagnostic,
/// and the emitted module.
fn generate_problems(
    configure: impl FnOnce(Spec) -> Spec,
) -> (Report, Vec<(&'static str, String)>, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = write_spec(temp.path(), "problems.yaml", OPEN_ENUMS_SPEC);
    let (report, code) = generate_spec_with_code(configure(Spec::new(spec_path)));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let reported = report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.as_str(),
                diagnostic.pointer.as_str().to_owned(),
            )
        })
        .collect();
    (report, reported, code)
}

#[test]
fn neither_switch_reports_anything_of_its_own() {
    // The `424`/`425` unions' `integer` branch, which the `type: string` member excludes, is
    // reported (`W011`) as the sibling spelling reports it (#671), whatever the switches say.
    let expected = [424, 425].map(|status| {
        (
            "W011",
            format!(
                "/paths/~1problems/post/responses/{status}/content/application~1json/schema/\
                 properties/kind"
            ),
        )
    });
    let configurations: [(&str, fn(Spec) -> Spec); 3] = [
        ("default", |spec| spec),
        ("open_narrowing", |spec| {
            spec.with_open_narrowing(spargen::Narrowing::Open)
        }),
        ("closed_enums", |spec| {
            spec.with_closed_enums(spargen::StringEnums::Closed)
        }),
    ];
    for (name, configure) in configurations {
        let (report, reported, _) = generate_problems(configure);
        assert_eq!(
            reported, expected,
            "{name} reports something of its own: {report:#?}"
        );
    }
}

#[test]
fn every_string_enum_is_open_unless_closed_enums_closes_them_all() {
    // By default every string enum is open, in a response, a union, a component or a request
    // body: none is emitted closed, and `open_narrowing`, which governs integer and boolean sets,
    // changes nothing here.
    let (_, _, open) = generate_problems(|spec| spec);
    assert!(
        open.contains("Unknown(super::support::UnknownValue<"),
        "an open enum holds an unlisted value:\n{open}"
    );
    assert!(
        !open.contains("type Err = super::support::UnknownVariant;"),
        "no string enum is closed by default:\n{open}"
    );
    let (_, _, narrowing) =
        generate_problems(|spec| spec.with_open_narrowing(spargen::Narrowing::Open));
    // The first line fingerprints the configuration, which differs; the module does not.
    let module = |code: &str| code.split_once('\n').unwrap().1.to_owned();
    assert_eq!(
        module(&narrowing),
        module(&open),
        "open_narrowing changed a string enum"
    );

    // `closed_enums` closes every one.
    let (_, _, closed) =
        generate_problems(|spec| spec.with_closed_enums(spargen::StringEnums::Closed));
    assert!(
        !closed.contains("Unknown(super::support::UnknownValue<"),
        "closed_enums leaves no enum open:\n{closed}"
    );
    let opened = open.matches("impl super::support::OpenEnum for ").count();
    assert!(opened > 0, "{open}");
    assert_eq!(
        opened,
        closed
            .matches("type Err = super::support::UnknownVariant;")
            .count(),
        "each enum is open by default and closed under closed_enums"
    );
}
