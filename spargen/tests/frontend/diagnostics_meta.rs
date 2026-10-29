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
    };
    let at = "/components/schemas/Pet";
    let (root_file, lib_file) = bundle_file_ids();
    // The three #454 shapes: an empty pointer, an empty name, a span over the whole root.
    let empty_pointer = reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        root_file,
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
        root_file,
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
        root_file,
        (1, 0, 29),
        "m",
    ));
    assert!(whole[0].contains("whole root document"), "{whole:?}");
    // A real location, and the root of a referenced file, which is a construct of its own.
    assert!(reasons(located(
        Code::SchemaDefaultNotApplied,
        at,
        root_file,
        (2, 15, 28),
        "`Pet`"
    ))
    .is_empty());
    assert!(reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        lib_file,
        (1, 0, 29),
        "m"
    ))
    .is_empty());
    // A document-level diagnostic's location is the root.
    assert!(reasons(located(
        Code::UnsupportedOpenApiVersion,
        "",
        root_file,
        (1, 0, 29),
        "m"
    ))
    .is_empty());
    assert!(reasons(located(Code::InvalidInput, "", root_file, (1, 0, 29), "m")).is_empty());
    // So is an `E011` the root document could not be read for (no span), and a syntax error the
    // parser places only at a point of the root.
    let mut unread = located(Code::InvalidInput, "", root_file, (1, 0, 0), "m");
    unread.span = None;
    assert!(reasons(unread).is_empty());
    assert!(reasons(located(Code::InvalidInput, "", root_file, (2, 15, 15), "m")).is_empty());
    // An `E011` with an empty pointer whose span covers one construct below the root, such as a
    // mapping key that is not a string, is not about the document as a whole: that construct has
    // a pointer of its own.
    let key = reasons(located(Code::InvalidInput, "", root_file, (2, 15, 16), "m"));
    assert_eq!(key.len(), 1, "{key:?}");
    assert!(key[0].contains("the pointer is empty"), "{key:?}");
    // A non-string key is addressed by the mapping that holds it (#923): the empty pointer is its
    // location only where the root mapping itself holds such a key.
    let non_string = located(
        Code::InvalidInput,
        "",
        root_file,
        (2, 15, 16),
        "YAML object keys must be strings",
    );
    let nested = oracles::location_violations(
        std::slice::from_ref(&non_string),
        b"openapi: 3.1.0\nx: { 1: one }\n",
    );
    assert_eq!(nested.len(), 1, "{nested:?}");
    assert!(oracles::location_violations(
        std::slice::from_ref(&non_string),
        b"openapi: 3.1.0\n1: one\n"
    )
    .is_empty());
    // The validator's verdict on the root value is about the root, however short the YAML parser
    // spans a flow collection (`[]` to its closing bracket's start), and so is a stream that is
    // not one document.
    for message in [
        "OpenAPI structure violation: [] is not of type \"object\"",
        "YAML input must contain exactly one document",
    ] {
        assert!(
            reasons(located(
                Code::InvalidInput,
                "",
                root_file,
                (1, 0, 1),
                message
            ))
            .is_empty(),
            "{message}"
        );
    }
    // A message may quote an empty backtick pair the root document itself holds.
    let quoting = located(
        Code::SchemaDefaultNotApplied,
        at,
        root_file,
        (2, 15, 28),
        "`` is no type",
    );
    assert!(oracles::location_violations(
        std::slice::from_ref(&quoting),
        b"openapi: 3.1.0\nx: '``'\n"
    )
    .is_empty());
    assert_eq!(reasons(quoting).len(), 1);
    // Each rule an `E003` or `E021` breaks is a violation of its own (#534), the empty pointer and
    // the whole-root span as much as an empty name.
    for code in [Code::AbsoluteRefUnsupported, Code::VendoredRefDrift] {
        let all = oracles::location_violations(
            &[located(code, "", root_file, (1, 0, 29), "in ``")],
            root,
        );
        assert_eq!(all.len(), 3, "{code:?}: {all:?}");
    }
    // An `E022`'s empty pointer is a violation of its own (#533).
    let duplicate = oracles::location_violations(
        &[located(
            Code::DuplicateObjectKey,
            "",
            root_file,
            (2, 15, 28),
            "m",
        )],
        root,
    );
    assert_eq!(duplicate.len(), 1, "{duplicate:?}");
    let unlocated = reasons(located(
        Code::SchemaDefaultNotApplied,
        "",
        root_file,
        (1, 0, 29),
        "m",
    ));
    assert_eq!(unlocated.len(), 2, "{unlocated:?}");
    assert!(
        unlocated[1].contains("whole root document"),
        "{unlocated:?}"
    );
}

/// [`oracles::load_yaml`], the oracles' YAML reader, reads every document as `YamlLoader` does and
/// fails where it fails (#1008). It differs only in which anchored nodes it keeps a copy of, which
/// is visible only in memory. An alias still expands to its anchor's node, also one nested inside
/// an unaliased anchor. A duplicate key, which `YamlLoader` stops at without the parser failing,
/// is still a failure, as are a syntax error and an alias to an anchor its document does not
/// declare before it.
#[test]
fn the_oracle_yaml_reader_reads_what_yaml_loader_reads() {
    for text in [
        "a: 1\n",
        "a: &x [1, 2]\nb: *x\nc: *x\n",
        "outer: &o\n  inner: &i { k: v }\n  again: *i\nnone: &n [x]\n",
        "a: &s scalar\nb: [*s, &t 2, *t]\n",
        "--- &d [1]\n--- { e: &e 2, f: *e }\n",
        "&r [1, *r]\n",
        "? &k key\n: value\nother: *k\n",
        "",
    ] {
        let expected = yaml_rust2::YamlLoader::load_from_str(text).expect(text);
        assert_eq!(oracles::load_yaml(text), Some(expected), "{text:?}");
    }
    for text in [
        "a: 1\na: 2\n",
        "--- a\n--- { b: 1, b: 2 }\n",
        "a: [1\n",
        "a: *missing\n",
        "--- &d [1]\n--- [*d]\n",
    ] {
        assert!(
            yaml_rust2::YamlLoader::load_from_str(text).is_err(),
            "{text:?}"
        );
        assert_eq!(oracles::load_yaml(text), None, "{text:?}");
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
    let reasons = |code: &str| -> Vec<String> { oracles::indistinguishable_variants(code) };
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
    // An enum with no `Deserialize` impl of its own is not a union.
    let not_a_union = code.replace("serde::Deserialize<'de> for U", "Other");
    assert!(
        reasons(&not_a_union).is_empty(),
        "{:#?}",
        reasons(&not_a_union)
    );
}

/// [`oracles::unexplained_variants`] excuses a union only for a `W001` that explains it at the
/// union's own pointer, one union per warning: a warning at another component, a `W001` of another
/// kind, or one warning shared by two unions leaves the union unexplained.
#[test]
fn the_unexplained_variant_oracle_excuses_only_the_union_its_warning_is_at() {
    let doc = |schemas: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n{schemas}"
        )
    };
    let untyped_u = "    U: { oneOf: [{ type: string }, {}] }\n";
    let untyped_ub = "    Ub: { oneOf: [{ type: integer }, {}] }\n";
    let (report, code) = generate_with_code(&doc(&format!("{untyped_u}{untyped_ub}")));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        oracles::indistinguishable_variants(&code).len(),
        2,
        "{code}"
    );
    // Each union's own warning excuses it.
    let unexplained = oracles::unexplained_variants(&report, &code);
    assert!(unexplained.is_empty(), "{unexplained:#?}");
    // `U`'s warning alone excuses one union, not both.
    let (only_u, _) = generate_with_code(&doc(untyped_u));
    assert_eq!(
        oracles::unexplained_variants(&only_u, &code).len(),
        1,
        "{only_u:#?}"
    );
    // A warning at another component excuses neither.
    let elsewhere = generate(&doc("    X: { oneOf: [{ type: string }, {}] }\n"));
    assert_eq!(
        oracles::unexplained_variants(&elsewhere, &code).len(),
        2,
        "{elsewhere:#?}"
    );
    // Nor does a `W001` at the union that is not about its variants.
    let validation = generate(&doc(
        "    U: { type: string, pattern: '^a' }\n    Ub: { type: string, pattern: '^a' }\n",
    ));
    assert!(
        validation
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::ValidationKeywordIgnored),
        "{validation:#?}"
    );
    assert_eq!(
        oracles::unexplained_variants(&validation, &code).len(),
        2,
        "{validation:#?}"
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
        let mut declared = w014.clone();
        declared.message = W014_GENERATION_WORDING.to_owned();
        declared.claim = OutcomeClaim::Generated;
        // As #174 found it, undeclared: the message states a claim the field does not carry.
        let mut undeclared = w014.clone();
        undeclared.message = W014_GENERATION_WORDING.to_owned();
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
    let mut declared = w014.clone();
    declared.message = W014_GENERATION_WORDING.to_owned();
    declared.claim = OutcomeClaim::Generated;
    assert!(claim_violations(Outcome::Generated, &[declared]).is_empty());
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

/// [`oracles::unreferenced_types`] reports a type that is no component and that nothing names
/// (#928), and passes a component nothing names, a type a field names, and a type only the client
/// after the `types` module names. A name that merely contains another (`PetList` beside `Pet`)
/// names neither, and an item's own impls do not name it. A hash-suffixed copy of a component
/// (#926) and of a meet's `<Hint>Constraint` intermediate (#924) are reported like any other type,
/// and a component allocated a suffixed name because its own collides with a reserved one (`Box`)
/// passes as the component.
#[test]
fn the_unreferenced_type_oracle_sees_a_type_nothing_names() {
    let spec = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    S0: { type: object }\n    Box: { type: object }\n";
    let module = |items: &str, client: &str| format!("pub mod types {{\n{items}\n}}\n{client}\n");
    let unused = module(
        "    pub struct S0 {}\n    pub struct Orphan {}",
        "pub fn op() {}",
    );
    assert_eq!(
        oracles::unreferenced_types(&unused, spec),
        ["`Orphan` is emitted, but nothing names it and it is no component"],
    );
    let field = module(
        "    pub struct S0 {\n        pub inner: Inner,\n    }\n    pub struct Inner {}",
        "pub fn op() {}",
    );
    assert!(oracles::unreferenced_types(&field, spec).is_empty());
    let client = module(
        "    pub struct Request {}",
        "pub fn op(body: types::Request) {}",
    );
    assert!(oracles::unreferenced_types(&client, spec).is_empty());
    let contains = module(
        "    pub struct Pet {}\n    impl Pet {\n        pub fn new() -> Pet {\n            Pet {}\n        \
         }\n    }\n    pub struct PetList {}\n    pub type S0 = PetList;",
        "pub fn op() {}",
    );
    assert_eq!(
        oracles::unreferenced_types(&contains, spec),
        ["`Pet` is emitted, but nothing names it and it is no component"],
    );
    let copy = module(
        "    pub struct S0 {}\n    pub struct S071def3c4 {}\n    pub struct Box00ba9f5f {}",
        "pub fn op() {}",
    );
    assert_eq!(
        oracles::unreferenced_types(&copy, spec),
        ["`S071def3c4` is emitted, but nothing names it and it is no component"],
    );
    let named = module(
        "    pub struct S0 {}\n    pub struct S071def3c4 {}\n    pub type Alias = S071def3c4;",
        "pub fn op(alias: types::Alias) {}",
    );
    assert!(oracles::unreferenced_types(&named, spec).is_empty());
    let constraint = module(
        "    pub struct S0 {}\n    pub type S0ConstraintD3cb2251 = f64;",
        "pub fn op() {}",
    );
    assert_eq!(
        oracles::unreferenced_types(&constraint, spec),
        ["`S0ConstraintD3cb2251` is emitted, but nothing names it and it is no component"],
    );
}
