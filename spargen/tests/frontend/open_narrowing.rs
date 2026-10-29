//! `open_narrowing`: which narrowed positions the option opens, and that it reports nothing of
//! its own. The spec is the one `e2e/open_narrowing.rs` compiles and drives over the wire, so the
//! positions pinned here are the ones that suite decodes.

use super::*;

/// The RFC 9457 problem spec from #268, shared with `e2e/open_narrowing.rs`, which documents its
/// statuses.
const OPEN_NARROWING_SPEC: &str = include_str!("../e2e/specs/open_narrowing.yaml");

/// `generate` over [`OPEN_NARROWING_SPEC`] with `open_narrowing` set to `open`, through the
/// harness's [`run_generate`] and [`run_check`], returning the report, the `(code, pointer)` of
/// each diagnostic, and the emitted module.
fn generate_narrowing(open: bool) -> (Report, Vec<(&'static str, String)>, String) {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("problems.yaml")).unwrap();
    std::fs::write(&spec_path, OPEN_NARROWING_SPEC).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let spec = Spec::new(spec_path).open_narrowing(open);
    let report = run_generate(&spec.clone().build(out.clone()).cargo(CargoIntegration::Off));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_no_codegen_failure(&report);
    assert_check_agrees(&report, &run_check(&spec));
    let code = std::fs::read_to_string(out).unwrap();
    assert_parseable(&code);
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
fn open_narrowing_reports_nothing_of_its_own() {
    // The `424`/`425` unions' `integer` branch, which the `type: string` member excludes, is
    // reported (`W011`) as the sibling spelling reports it (#671), with the option on or off.
    let expected = [424, 425].map(|status| {
        (
            "W011",
            format!(
                "/paths/~1problems/post/responses/{status}/content/application~1json/schema/\
                 properties/kind"
            ),
        )
    });
    for open in [false, true] {
        let (report, reported, _) = generate_narrowing(open);
        assert_eq!(
            reported, expected,
            "open_narrowing({open}) reports something of its own: {report:#?}"
        );
    }
}

#[test]
fn open_narrowing_opens_exactly_the_positions_it_applies_to() {
    // The option off: the narrowing is exact, and no set is open.
    let (_, _, closed) = generate_narrowing(false);
    assert!(
        !closed.contains("Other(String)"),
        "without the option no set is open:\n{closed}"
    );

    // Exactly the positions it applies to open: the `200`, `403`, `404`, and `409` bodies, and the
    // `424`/`425` `kind`, whose union narrows to one branch. The `{enum: [a, b, c]}` members of
    // `424` and `425` open in place too, as intermediates no field uses, which the output does not
    // carry (#924). The union members of `422`, `423`, `426`, and `427`, which keep two branches,
    // are met first in either order (#678), so those members meet them into closed branch sets as
    // they are written, opening nothing (the e2e fixture's `tests/open.rs` decodes both). The
    // `428`/`429` `kind`, narrowed against a `uuid` string, stays closed in either order. Only an
    // open enum emits `as_str`.
    let (_, _, open) = generate_narrowing(true);
    assert_eq!(
        open.matches("pub fn as_str(&self) -> &str").count(),
        6,
        "{open}"
    );
}
