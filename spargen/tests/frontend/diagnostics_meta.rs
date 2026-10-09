//! The harness's own guarantees: the oracles, outcome claims, check/generate parity, and relocation
//! of the parity fixtures.

use super::*;

/// [`assert_no_untyped_value`] can fail: a property with the empty schema, which admits any JSON
/// value, lowers to `serde_json::Value` by design, and the oracle must see it. Without this a
/// spelling that never matches (the token stream's `serde_json :: Value`) passes every fixture.
#[test]
fn the_untyped_value_oracle_sees_an_unconstrained_field() {
    let (report, code) = generate_with_code(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    S:\n      type: object\n      properties:\n        anything: {}\n",
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let failed = std::panic::catch_unwind(|| assert_no_untyped_value(&code)).is_err();
    assert!(
        failed,
        "the oracle passed a `serde_json::Value` field:\n{code}"
    );
}

/// [`oracles::location_violations`] can fail, on each of the three ways #454's diagnostics lost
/// their location, and passes what a real location looks like. Without this an oracle that read the
/// wrong field would pass every fixture.
#[test]
fn the_location_oracle_sees_each_lost_location() {
    let root = b"openapi: 3.1.0\ncomponents: {}\n";
    let reasons = |diagnostic: Diagnostic| -> Vec<String> {
        oracles::location_violations(&[diagnostic], root)
            .into_iter()
            .map(|violation| violation.reason)
            .collect()
    };
    let at = "/components/schemas/Pet";
    // The three #454 shapes: an empty pointer, an empty name, a span over the whole root.
    let empty_pointer = reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        0,
        (2, 15, 28),
        "m",
    ));
    assert!(
        empty_pointer[0].contains("the pointer is empty"),
        "{empty_pointer:?}"
    );
    let empty_name = reasons(located(
        Code::SchemaDefaultNotApplied,
        at,
        0,
        (2, 15, 28),
        "in ``",
    ));
    assert!(
        empty_name[0].contains("names something empty"),
        "{empty_name:?}"
    );
    let whole = reasons(located(
        Code::SchemaDefaultNotApplied,
        at,
        0,
        (1, 0, 29),
        "m",
    ));
    assert!(whole[0].contains("whole root document"), "{whole:?}");
    // A real location, and the root of a referenced file, which is a construct of its own.
    assert!(reasons(located(
        Code::SchemaDefaultNotApplied,
        at,
        0,
        (2, 15, 28),
        "`Pet`"
    ))
    .is_empty());
    assert!(reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        1,
        (1, 0, 29),
        "m"
    ))
    .is_empty());
    // A document-level diagnostic's location is the root.
    assert!(reasons(located(
        Code::UnsupportedOpenApiVersion,
        "",
        0,
        (1, 0, 29),
        "m"
    ))
    .is_empty());
    assert!(reasons(located(Code::InvalidInput, "", 0, (1, 0, 29), "m")).is_empty());
    // `E003` and `E021` are no longer known gaps (#534): each rule they break is a violation of
    // its own, the empty pointer and the whole-root span as much as an empty name.
    for code in [Code::AbsoluteRefUnsupported, Code::VendoredRefDrift] {
        let all: Vec<Option<u32>> =
            oracles::location_violations(&[located(code, "", 0, (1, 0, 29), "in ``")], root)
                .into_iter()
                .map(|violation| violation.known)
                .collect();
        assert_eq!(all, [None, None, None], "{code:?}: {all:?}");
    }
    // `E022` is no longer a known gap (#533): its empty pointer is a violation of its own.
    let duplicate = oracles::location_violations(
        &[located(Code::DuplicateObjectKey, "", 0, (2, 15, 28), "m")],
        root,
    );
    assert_eq!(duplicate[0].known, None, "{:?}", duplicate[0].reason);
    let unlocated = reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        0,
        (1, 0, 29),
        "m",
    ));
    assert_eq!(unlocated.len(), 2, "{unlocated:?}");
    assert!(
        unlocated[1].contains("whole root document"),
        "{unlocated:?}"
    );
}

/// `E022` points at the duplicate key itself (#533), in YAML and in JSON, through `check` and
/// `generate` alike, so the location oracle finds nothing to report.
#[test]
fn e022_points_at_the_duplicate_key() {
    let yaml = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    Foo:\n      type: object\n      type: string\n";
    let json = r#"{"openapi": "3.1.0", "info": {"title": "T", "version": "1.0.0"}, "paths": {}, "components": {"schemas": {"Foo": {"type": "object", "type": "string"}}}}"#;
    // The parser is chosen by extension, and the shared helpers write `openapi.yaml`, so the JSON
    // document is written as `openapi.json` here to reach the JSON parser.
    for (name, spec) in [("openapi.yaml", yaml), ("openapi.json", json)] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join(name), spec).unwrap();
        let generated = run_generate(&build(dir.join(name), dir.join("client.rs")));
        let checked = run_check(&Spec::new(dir.join(name)));
        for report in [checked, generated] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
            let duplicate = report
                .diagnostics()
                .iter()
                .find(|d| d.code == Code::DuplicateObjectKey)
                .expect("duplicate-key diagnostic");
            assert_eq!(
                duplicate.pointer.as_str(),
                "/components/schemas/Foo/type",
                "{duplicate:#?}"
            );
            let violations = oracles::location_violations(report.diagnostics(), spec.as_bytes());
            assert!(violations.is_empty(), "{violations:?}");
        }
    }
}

/// [`oracles::indistinguishable_variants`] can fail: two variants whose payloads are differently
/// named structs with one shape, and a variant that is `serde_json::Value`, both in a `oneOf`. A
/// field of another type is another shape, and an `anyOf` is held to neither. Without this an
/// oracle that matched nothing would pass every fixture.
#[test]
fn the_distinguishable_variant_oracle_sees_equal_and_untyped_variants() {
    let code = "pub mod types {\n\
                pub enum U {\n    A(Box<A>),\n    B(Box<B>),\n    C(Box<C>),\n}\n\
                impl<'de> serde::Deserialize<'de> for U {\n\
                \"data must match exactly one typed variant of union U\"\n}\n\
                pub type Aa = String;\n\
                pub type Ba = String;\n\
                #[serde(deny_unknown_fields)]\n\
                pub struct A {\n    pub a: Aa,\n}\n\
                #[serde(deny_unknown_fields)]\n\
                pub struct B {\n    pub a: Ba,\n}\n\
                pub type C = serde_json::Value;\n\
                }\n";
    let reasons = |code: &str| -> Vec<String> {
        oracles::indistinguishable_variants(code)
            .into_iter()
            .map(|violation| violation.reason)
            .collect()
    };
    let found = reasons(code);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found[0].contains("`C` is `serde_json::Value`")
            && found[1].contains("variants `A` and `B` have one shape"),
        "{found:#?}"
    );
    let distinct = code.replace("pub type Ba = String;", "pub type Ba = i64;");
    assert_eq!(reasons(&distinct).len(), 1, "{:#?}", reasons(&distinct));
    // An `anyOf` selects its most specific match, so neither equal payloads nor a
    // `serde_json::Value` payload make another variant undecodable (#535).
    let any_of = code.replace("exactly one", "at least one");
    assert!(reasons(&any_of).is_empty(), "{:#?}", reasons(&any_of));
    // No issue tracks either defect, so neither is known.
    assert!(oracles::indistinguishable_variants(code)
        .iter()
        .all(|violation| violation.known.is_none()));
    // An enum with no `Deserialize` impl of its own is not a union.
    let not_a_union = code.replace("serde::Deserialize<'de> for U", "Other");
    assert!(
        reasons(&not_a_union).is_empty(),
        "{:#?}",
        reasons(&not_a_union)
    );
}

/// `check` must run the same lowering as `generate`, so an irreconcilable `allOf` rejects
/// identically through both entry points (check/generate parity).
#[test]
fn e013_check_generate_parity() {
    let report = check(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// `check` runs the same lowering as `generate`, so the W005 disposition fires identically.
#[test]
fn w005_check_generate_parity() {
    let report = check(W005_SPEC);
    assert_eq!(report.outcome(), Outcome::Clean, "{report:#?}");
    assert!(has_code(&report, Code::SchemaDefaultNotApplied));
}

/// [`W014_REJECTED_ELSEWHERE`] without the rejected `/doc`.
const W014_CLEAN: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string } }
            text/html: { schema: { type: string } }
"##;

/// `W014`'s message before #319. It claimed generation, which was false on every `check` run and
/// on every run rejected elsewhere (#174).
const W014_GENERATION_WORDING: &str =
    "`text/plain` is generated; the alternative media type(s) `text/html` are not";

#[test]
fn a_generation_claim_on_w014_is_contradicted_wherever_174_found_it_false() {
    // #174's four axes reduce to two outcomes for this message: `check`, which generates nothing
    // on any document, and a run rejected on another path. On each, a `W014` that declared the
    // generation its old wording asserted is refused by the claim check, and the old wording,
    // left undeclared, is caught by the prose backstop. Only a run that generates admits the
    // claim. So the check distinguishes runs; it does not refuse the claim outright.
    let refusing = [
        (check(W014_CLEAN), Outcome::Clean),
        (check(W014_REJECTED_ELSEWHERE), Outcome::Rejected),
        (generate(W014_REJECTED_ELSEWHERE), Outcome::Rejected),
    ];
    for (report, outcome) in &refusing {
        assert_eq!(report.outcome(), *outcome, "{report:#?}");
        let w014 = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == Code::AlternativeMediaIgnored)
            .unwrap_or_else(|| panic!("{report:#?}"));
        // As emitted: the selection, which every outcome admits.
        assert_eq!(w014.claim, OutcomeClaim::Independent, "{w014:#?}");
        assert!(claim_violations(*outcome, std::slice::from_ref(w014)).is_empty());
        // As #174 found it, with the claim declared: the outcome refuses it.
        let declared = Diagnostic {
            message: W014_GENERATION_WORDING.to_owned(),
            claim: OutcomeClaim::Generated,
            ..w014.clone()
        };
        // As #174 found it, undeclared: the message states a claim the field does not carry.
        let undeclared = Diagnostic {
            message: W014_GENERATION_WORDING.to_owned(),
            ..w014.clone()
        };
        for diagnostic in [declared, undeclared] {
            assert_eq!(
                claim_violations(*outcome, std::slice::from_ref(&diagnostic)).len(),
                1,
                "{outcome}: {diagnostic:#?}"
            );
        }
    }

    // A run that generates admits the declared claim, so the check tells runs apart rather than
    // refusing the claim outright.
    let generated = generate(W014_CLEAN);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    let w014 = generated
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == Code::AlternativeMediaIgnored)
        .unwrap_or_else(|| panic!("{generated:#?}"));
    let declared = Diagnostic {
        message: W014_GENERATION_WORDING.to_owned(),
        claim: OutcomeClaim::Generated,
        ..w014.clone()
    };
    assert!(claim_violations(Outcome::Generated, &[declared]).is_empty());
}

#[test]
fn every_outcome_admits_exactly_the_claims_true_of_it() {
    for outcome in [
        Outcome::Generated,
        Outcome::Cached,
        Outcome::Clean,
        Outcome::Rejected,
    ] {
        assert!(outcome.admits(OutcomeClaim::Independent), "{outcome}");
        assert_eq!(
            outcome.admits(OutcomeClaim::Rejected),
            outcome == Outcome::Rejected,
            "{outcome}"
        );
        assert_eq!(
            outcome.admits(OutcomeClaim::Generated),
            matches!(outcome, Outcome::Generated | Outcome::Cached),
            "{outcome}"
        );
    }
}

#[test]
fn the_prose_backstop_reads_only_an_unnegated_outcome_predicate() {
    for (message, stated) in [
        (W014_GENERATION_WORDING, Some(OutcomeClaim::Generated)),
        ("the body is rejected", Some(OutcomeClaim::Rejected)),
        ("these would be generated", Some(OutcomeClaim::Generated)),
        // Negated in its own clause: true on every run.
        (
            "webhooks describe server-initiated calls; no client code is generated for them",
            None,
        ),
        (
            "a header that is not a single value, so no typed accessor is generated",
            None,
        ),
        ("the polymorphism form is not generated", None),
        // A negation in an earlier clause does not reach a later one.
        (
            "nothing is selected; the rest is generated",
            Some(OutcomeClaim::Generated),
        ),
        // A quoted name supplies neither the predicate nor the negation.
        ("`is generated` names a schema", None),
        ("`no` is generated", Some(OutcomeClaim::Generated)),
        // An adjective is not a predicate.
        ("the generated item is shared across every use", None),
        ("`text/plain` is selected; the alternatives are not", None),
    ] {
        assert_eq!(stated_claim(message), stated, "{message}");
    }
}

/// A run that reaches `batch_cap` drops every diagnostic past it. Presenting that shortened list
/// as if it were the whole one is the silent behavior the disposition invariant forbids, so the
/// report says it is partial and `Display` carries the marker.
#[test]
fn a_report_that_hit_the_batch_cap_says_it_is_truncated() {
    // Each of these paths carries its own unsupported media type, so the run has far more
    // diagnostics to emit than the cap allows.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for index in 0..12 {
        spec.push_str(&format!(
            "  /item{index}:\n    get:\n      responses:\n        '200':\n          description: ok\n          content:\n            application/vnd.unsupported: {{ schema: {{ type: string }} }}\n"
        ));
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("openapi.yaml");
    std::fs::write(&path, &spec).unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(path).unwrap();

    let uncapped = run_check(&Spec::new(spec_path.clone()).batch_cap(100));
    assert!(!uncapped.truncated(), "{uncapped:#?}");
    let all = uncapped.diagnostics().len();
    assert!(all > 3, "need more than the cap to prove truncation: {all}");

    let capped = run_check(&Spec::new(spec_path).batch_cap(3));
    assert!(capped.truncated(), "{capped:#?}");
    assert_eq!(capped.diagnostics().len(), 3, "{capped:#?}");
    assert!(
        capped.to_string().contains("truncated at batch_cap"),
        "{capped}"
    );
}

/// `check` must reach the same verdict as `generate`: it accepts exactly what `generate` accepts,
/// and reports exactly the same diagnostics. The two outcomes are deliberately *not* compared for
/// equality — a successful `check` is `Clean` and a successful `generate` is `Generated`, because
/// only one of them wrote a module. What must agree is the accept/reject decision.
///
/// `check` skips codegen and emit, so any diagnostic the two disagree on is one that only a
/// generating run reports — which is exactly what would stop `check` standing in for it.
fn assert_parity(name: &str, spec: &str) {
    let generated = generate(spec);
    let checked = check(spec);

    assert_eq!(
        checked.outcome() == Outcome::Rejected,
        generated.outcome() == Outcome::Rejected,
        "`{name}`: check says {:?} but generate says {:?}",
        checked.outcome(),
        generated.outcome()
    );
    assert_eq!(
        codes(&checked),
        codes(&generated),
        "`{name}`: check and generate report different diagnostics"
    );

    // A fixture whose name begins with a code must actually report it. Without this the parity
    // suite is satisfied by both entry points being equally wrong: the `E004 unresolvable ref`
    // fixture reported `clean` for as long as the bug it was named for existed, and passed, because
    // parity compares the two reports to each other and the span test only *counts* verdicts.
    if let Some(labelled) = parity_label(name) {
        assert!(
            codes(&checked).contains(&labelled),
            "`{name}`: the fixture is named for {labelled} but reports {:?}",
            codes(&checked)
        );
    }
}

/// The `E###`/`W###` code a [`PARITY_FIXTURES`] name is labelled with, if it is labelled at all.
/// Shared by [`assert_parity`], which holds a labelled fixture to its label, and by
/// [`every_parity_fixture_that_reports_is_labelled`], which stops the label convention from
/// quietly becoming optional.
fn parity_label(name: &str) -> Option<&str> {
    name.split_whitespace().next().filter(|token| {
        token.len() == 4
            && matches!(token.as_bytes()[0], b'E' | b'W')
            && token[1..].bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[test]
fn check_and_generate_agree_on_every_parity_fixture() {
    for (name, spec) in PARITY_FIXTURES {
        assert_parity(name, spec);
    }
}

#[test]
fn the_parity_fixtures_span_both_verdicts() {
    // A parity suite made only of clean specs would pass trivially. This keeps it honest: it has to
    // contain rejections, warnings, and specs that succeed.
    let mut rejected = 0;
    let mut warned = 0;
    let mut succeeded = 0;
    for (_, spec) in PARITY_FIXTURES {
        let report = check(spec);
        if report.outcome() == Outcome::Rejected {
            rejected += 1;
        } else {
            succeeded += 1;
        }
        if codes(&report).iter().any(|code| code.starts_with('W')) {
            warned += 1;
        }
    }
    assert!(rejected >= 4, "only {rejected} fixtures reject");
    assert!(warned >= 3, "only {warned} fixtures warn");
    assert!(succeeded >= 2, "only {succeeded} fixtures succeed");
}

/// The label check in [`assert_parity`] is opt-in by naming convention, so on its own it can be
/// disarmed rather than satisfied: renaming `"E004 unresolvable ref"` to `"unresolvable ref"`, or
/// widening [`parity_label`] until it matches nothing, makes the whole suite pass again — the same
/// silent escape the label check was added to close.
///
/// This requires the convention instead of hoping for it: a fixture that reports any diagnostic at
/// all must be named for one. Only the deliberately clean fixture reports nothing, and it is the
/// only one allowed to go unlabelled.
#[test]
fn every_parity_fixture_that_reports_is_labelled() {
    let mut labelled = 0;
    for (name, spec) in PARITY_FIXTURES {
        let report = check(spec);
        match parity_label(name) {
            Some(_) => labelled += 1,
            None => assert!(
                report.diagnostics().is_empty(),
                "`{name}` reports {:?} but is not named for a code",
                codes(&report)
            ),
        }
    }
    // Belt and braces: if `parity_label` itself stopped matching, every fixture would fall into the
    // arm above and this floor is what notices.
    assert!(
        labelled >= 7,
        "only {labelled} parity fixtures are labelled; the convention has been disarmed"
    );
}

/// Hold `spec` to its relocation twin ([`oracles::relocate`]): the same document with its
/// `components.schemas` moved into a referenced `lib.yaml`. Both entry points must reach the same
/// verdict with the same code multiset (so a `W001` the audit raises in the root and not in a
/// referenced file, #446, shows as a count), and a `Generated` run must give each schema the same
/// [`oracles::shape`]. Returns whether `spec` had schemas to move.
fn assert_relocation_changes_nothing(name: &str, spec: &str) -> bool {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let Some(root) = oracles::write_relocated(&dir, spec) else {
        return false;
    };
    let (inline, inline_code) = generate_with_code(spec);
    let inline_checked = check(spec);
    let out = dir.join("client.rs");
    let moved = run_generate(&build(root.clone(), out.clone()));
    let moved_code = std::fs::read_to_string(&out).unwrap_or_default();
    let moved_checked = run_check(&Spec::new(root));
    assert_eq!(
        moved.outcome(),
        inline.outcome(),
        "`{name}`: relocated, generate says {:?}: {moved:#?}",
        moved.outcome()
    );
    assert_eq!(
        codes(&moved),
        codes(&inline),
        "`{name}`: relocated, generate reports other codes"
    );
    assert_eq!(
        moved_checked.outcome(),
        inline_checked.outcome(),
        "`{name}`: relocated, check says {:?}",
        moved_checked.outcome()
    );
    assert_eq!(
        codes(&moved_checked),
        codes(&inline_checked),
        "`{name}`: relocated, check reports other codes"
    );
    if inline.outcome() == Outcome::Generated {
        for schema in oracles::schema_names(spec) {
            assert_eq!(
                oracles::shape(&moved_code, &schema),
                oracles::shape(&inline_code, &schema),
                "`{name}`: relocated, `{schema}` lowers to another shape"
            );
        }
    }
    true
}

/// Moving a document's schemas into a referenced file changes no verdict, code or shape (#446):
/// every [`PARITY_FIXTURES`] spec that declares schemas is held to its relocation twin. The
/// floor keeps the property from passing because nothing was moved.
#[test]
fn moving_the_parity_fixtures_schemas_into_a_referenced_file_changes_nothing() {
    let moved = PARITY_FIXTURES
        .iter()
        .filter(|(name, spec)| assert_relocation_changes_nothing(name, spec))
        .count();
    assert!(
        moved >= 4,
        "only {moved} parity fixtures declare schemas to move"
    );
}

/// [`oracles::relocate`] moves what it says: the root keeps one `$ref` per schema, into the
/// sub-file, which declares the schemas with their references between them spelled against
/// itself.
#[test]
fn relocation_moves_every_schema_behind_a_reference() {
    let spec = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    A: { type: string }\n    'b/c': { $ref: '#/components/schemas/A' }\n";
    let (root, lib) = oracles::relocate(spec).unwrap();
    let root: serde_json::Value = serde_json::from_str(&root).unwrap();
    let lib: serde_json::Value = serde_json::from_str(&lib).unwrap();
    assert_eq!(
        root["components"]["schemas"],
        serde_json::json!({
            "A": { "$ref": "./lib.yaml#/components/schemas/A" },
            "b/c": { "$ref": "./lib.yaml#/components/schemas/b~1c" },
        })
    );
    assert_eq!(
        lib["components"]["schemas"],
        serde_json::json!({
            "A": { "type": "string" },
            "b/c": { "$ref": "./lib.yaml#/components/schemas/A" },
        })
    );
    assert_eq!(root["openapi"], "3.1.0");
    let bare = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\n";
    assert!(oracles::relocate(bare).is_none());
}
