//! Omit and keep rules across a diff pair (#1428). The CLI applies one set of rules to both specs,
//! and a rule that names a construct only one of them declares is the common case: a new endpoint
//! the user omits. Such a rule is satisfied on the side that lacks its construct; only a rule that
//! matches neither spec is an `E019`, reported on each side that carries it.

use spargen::{OmitMethod, OmitRule, Outcome};

use super::*;

/// The codes of `report`'s diagnostics, in emission order.
fn codes(report: &spargen::Report) -> Vec<&'static str> {
    report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect()
}

/// The new spec adds `/owners`; one shared rule omits it. The old side, which never declared it,
/// lowers clean with no `E019`, the new side omits it with a `W009`, and the two surfaces are
/// equal. Under carve the same pair diffs the same way, since carve applies the same rules.
#[test]
fn an_omit_rule_matching_only_one_spec_is_satisfied_on_the_other() {
    let old = base();
    let new = spec("", "id", PET_PROPS, EXTRA_OP);
    let omit = |spec: Spec| spec.add_omit_rule(OmitRule::path("/owners"));

    for carve in [spargen::Carve::Off, spargen::Carve::Auto] {
        let configure = |spec: Spec| omit(spec).with_carve(carve);
        let (report, sides) = diff_with_sides(&old, &new, configure, configure);
        assert!(report.changes.is_empty(), "{carve:?}: {report:?}");
        assert_eq!(report.bump, Impact::Patch);
        assert!(codes(sides.old_report()).is_empty(), "{carve:?}: {sides}");
        assert_eq!(codes(sides.new_report()), ["W009"], "{carve:?}: {sides}");
    }

    // Reversed, the rule is satisfied on the new side instead.
    let (report, sides) = diff_with_sides(&new, &old, omit, omit);
    assert!(report.changes.is_empty(), "{report:?}");
    assert_eq!(codes(sides.old_report()), ["W009"], "{sides}");
    assert!(codes(sides.new_report()).is_empty(), "{sides}");
}

/// A rule only one side's profile carries is judged against both specs too: it matches the other
/// spec, so it is satisfied where it names nothing.
#[test]
fn a_rule_only_one_side_carries_is_satisfied_when_the_other_spec_matches_it() {
    let old = base();
    let new = spec("", "id", PET_PROPS, EXTRA_OP);
    let omit = |spec: Spec| spec.add_omit_rule(OmitRule::path("/owners"));

    let (report, sides) = diff_with_sides(&old, &new, omit, |spec| spec);
    assert_eq!(kinds(&report), [ChangeKind::OperationAdded], "{report:?}");
    assert!(codes(sides.old_report()).is_empty(), "{sides}");
    assert!(codes(sides.new_report()).is_empty(), "{sides}");
}

/// A keep rule naming an operation only the new spec declares keeps it there, and on the old side
/// it is satisfied and keeps nothing: the old side narrows to no operation, so the kept one is an
/// addition.
#[test]
fn a_keep_rule_matching_only_one_spec_is_satisfied_on_the_other() {
    let old = base();
    let new = spec("", "id", PET_PROPS, EXTRA_OP);
    let keep = |spec: Spec| spec.add_keep_operation(OmitMethod::Get, "/owners");

    let (report, sides) = diff_with_sides(&old, &new, keep, keep);
    assert_eq!(kinds(&report), [ChangeKind::OperationAdded], "{report:?}");
    assert_eq!(report.changes[0].location, "GET /owners", "{report:?}");
    for (side, side_report) in sides.reports() {
        assert_eq!(side_report.outcome(), Outcome::Clean, "{side}: {sides}");
        assert!(side_report.errors().next().is_none(), "{side}: {sides}");
    }
}

/// A rule that matches neither spec is an `E019` on each side that carries it, as it would be
/// for either spec alone, whether it is an omit or a keep rule.
#[test]
fn a_rule_matching_neither_spec_is_e019_on_each_side_that_carries_it() {
    let old = base();
    let new = spec("", "id", PET_PROPS, EXTRA_OP);
    let temp = tempfile::tempdir().unwrap();
    let old_path = write_spec(temp.path(), "old.yaml", &old);
    let new_path = write_spec(temp.path(), "new.yaml", &new);

    let rules: [fn(Spec) -> Spec; 2] = [
        |spec| spec.add_omit_rule(OmitRule::path("/nowhere")),
        |spec| spec.add_keep_operation(OmitMethod::Get, "/nowhere"),
    ];
    for configure in rules {
        let rejection = spargen::diff(
            &configure(Spec::new(old_path.clone())),
            &configure(Spec::new(new_path.clone())),
        )
        .expect_err("a rule naming nothing in either spec rejects both sides");
        for side in [rejection.old_report(), rejection.new_report()] {
            let side = side.expect("each side carries the rule, so each is rejected");
            assert_eq!(codes(side), ["E019"], "{rejection}");
            assert!(side.errors().next().unwrap().message.contains("/nowhere"));
        }

        // Carried by the old side alone, it rejects that side alone.
        let rejection = spargen::diff(
            &configure(Spec::new(old_path.clone())),
            &Spec::new(new_path.clone()),
        )
        .expect_err("the old side carries a rule naming nothing in either spec");
        assert_eq!(
            codes(rejection.old_report().expect("the old side is rejected")),
            ["E019"]
        );
        assert!(rejection.new_report().is_none(), "{rejection}");
    }
}
