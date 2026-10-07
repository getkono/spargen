use super::*;

/// `spargen explain E023` prints `Code::RuntimeDependencyContract`'s explain body verbatim, and
/// that body specifies workspace-dependency inheritance as an algorithm rather than describing
/// the audit in a paragraph. Nothing else holds it to anything —
/// `every_code_has_title_and_explain_text` asserts only that it is non-empty, and
/// `docs/errors.md` carries titles.
///
/// The rule, rather than a list: **every clause of that body which states what the resolver
/// does is asserted here, against the fixture in this module that makes it true, named in the
/// comment beside it.** Clauses that give advice, describe how the crate is built, give a
/// rationale, or restate what Cargo and rustc do afterwards are not asserted, because nothing
/// in this module observes them — but each is still *listed*, by `unasserted`, with the reason
/// it is exempt. No count of the body's propositions is offered: three counts have been
/// produced for this text and each was wrong.
///
/// The listing is what makes the rule total. The body is claimed byte by byte: every
/// `promises` and `unasserted` entry marks the one span it matches, and outside those spans
/// only punctuation and the connectives in `CONNECTIVES` may stand. So a sentence typed into
/// the body — and into the expected file beside it — fails here until its author either cites
/// the fixture that makes it true or states, in an `unasserted` entry, why nothing observes it.
/// Without that coverage check, a sentence added to both copies left every assertion here
/// passing, however little of it was true (#209). What the check cannot do is judge a
/// classification: listing a resolver claim as `unasserted` passes, and only review of that
/// entry catches it. Nor does it judge the connectives, which may be swapped for one another.
///
/// Why the body is pinned twice, and what each pin is worth. The equality assertion against
/// `runtime_contract_e023_explain.txt` **logically implies every clause assertion above it** —
/// the same string has the same substrings, the same counts and the same order — so the clause
/// assertions add no coverage. Their entire value is the failure message: equality says the
/// text changed, and they say **which promise broke** and which fixture was supposed to make it
/// true. That is worth keeping and is not worth claiming as coverage.
///
/// Equality constrains **drift between the two copies**, and nothing else. It is symmetric:
/// the same text typed into both copies passes it, whatever that text says, so it does not
/// stop a sentence being added that contradicts the algorithm, nor a line prefixed saying none
/// of the description is accurate. The coverage check is what stops those, by refusing any
/// word it has not been told how to classify.
///
/// The cost, stated rather than discovered. Requiring each clause **exactly once** is a
/// false-gate hazard: a short clause such as "taking the version from there" or "the union of
/// both feature lists" will red this test if a future edit legitimately uses the same words a
/// second time, even though nothing is wrong. That is the price of closing the
/// matched-the-wrong-occurrence class, and the fix in that case is to lengthen the assertion,
/// not to drop the rule. Equality is likewise deliberate friction, and the friction is the
/// mechanism: editing the explain text means re-typing it into a second file.
///
/// What the three therefore deliver, exactly: **no clause can change, and none can be added,
/// without someone re-typing it in the test and classifying it there.** They do not make the
/// body true. A clause that is false today stays false with every guard green — the promise
/// that a root which cannot be *found* is reported differently from one that cannot be *read*
/// was exactly that, pinned here and mirrored in the expected file while contradicting
/// `workspace_root`, until the resolver was made to keep it (#171) — and a maintainer who
/// re-types a change into the expected file has made this test agree with it, not verified
/// it. Holding the prose to the code is a wider question than this one code, and is **#137**.
#[test]
fn the_e023_explain_text_states_the_inheritance_rules_this_module_enforces() {
    let explain = Code::RuntimeDependencyContract.explain();
    // The expected body lives in its own file, not in this one, and that placement is the
    // guard. With the mirror inline, a single find-and-replace over the module's source
    // rewrote the mirror and every clause assertion in one stroke — negating "taking the
    // version from there" that way left the suite green. An edit to `code.rs` must now be
    // re-typed in a second file that no edit to this module can reach.
    // It sits beside `runtime_contract/`, under `src/`, so it ships in the published crate and
    // this test still compiles there. Being test-only, it is not needed to build the packaged
    // library, so `every_included_file_ships_in_the_published_crate` in `tests/layering.rs`
    // is what holds it in the package.
    // The file carries a trailing newline, as a text file should; the explain body does not.
    let expected = include_str!("../../runtime_contract_e023_explain.txt").trim_end_matches('\n');

    // The fixture each clause names as its pin is checked to resolve against the source of every
    // file of this test module (`TEST_SOURCES`).
    //
    // Be precise about what that is worth. The check catches a **rename or a typo** and nothing
    // else: it verifies the name belongs to a `#[test]` in this module, not that the test has
    // anything to do with the clause citing it. Repointing the version clause at
    // `the_time_requirement_never_asks_for_serde`, which asserts nothing about versions, passes.
    // Before round 6 it did not even require a test — `fn messages(`, `fn linux(` and any other
    // helper satisfied it. Turning a citation into *evidence* means deriving the text from the
    // behaviour, which is #137's subject and not this test's.
    //
    // A cited name must be a `#[test]` in this module: the attribute immediately precedes it,
    // with only whitespace between. The predicate is shared with `diag/code.rs`'s
    // `EXPLAIN_CLAUSES_OWNED_ELSEWHERE` check, so a fix to it reaches both.

    // Every span of the body some entry below claims, for the coverage check at the end.
    let mut claimed: Vec<&str> = Vec::new();
    // Exactly once, not merely present: an assertion whose text also occurs earlier or later
    // matches the wrong sentence and leaves the one it was written for unpinned.
    // `[workspace.dependencies]` appears twice in this body, and that is how the opening
    // clause below went unasserted while reading as though it were covered. The same holds
    // for an `unasserted` entry, which would otherwise exempt the wrong occurrence.
    let occurs_once = |clause: &str| {
        let occurrences = explain.matches(clause).count();
        // The body is deliberately not printed here, for the same reason the equality
        // assertion below avoids `assert_eq!`: handing a maintainer the new text beside a
        // failure is handing them the paste that makes the failure go away. Naming the clause
        // and its count is enough to find it.
        assert!(
            occurrences == 1,
            "`spargen explain E023` says {clause:?} {occurrences} times, expected exactly once"
        );
    };

    // Why a clause of the body is not asserted. A variant is required rather than a free-form
    // comment so that exempting a clause is a stated decision a reviewer can dispute.
    #[derive(Debug)]
    enum Unasserted {
        /// Tells the consumer what to do; nothing here observes whether they do it.
        Advice,
        /// Describes how the generated module is built, not what the audit does.
        Build,
        /// Says why a rule exists, not what the rule is.
        Rationale,
        /// Restates what Cargo or rustc does, which this module does not run.
        Toolchain,
    }
    let mut unasserted = |clause: &'static str, why: Unasserted| {
        occurs_once(clause);
        assert!(
            !clause.trim().is_empty(),
            "an empty `unasserted` entry ({why:?})"
        );
        claimed.push(clause);
    };

    unasserted("The generated module is freestanding", Unasserted::Build);
    unasserted(
        "enable only the capabilities the diagnostic names",
        Unasserted::Advice,
    );
    unasserted("as Cargo does", Unasserted::Toolchain);
    unasserted(
        "because an unrelated broken `Cargo.toml` above the project can be that file",
        Unasserted::Rationale,
    );
    unasserted(
        "Cargo resolves the declared range; Rust compilation then verifies the selected crates \
         expose the APIs and traits used by the generated client",
        Unasserted::Toolchain,
    );

    let mut promises = |clause: &'static str, pinned_by: &[&str]| {
        occurs_once(clause);
        claimed.push(clause);
        assert!(!pinned_by.is_empty(), "no fixture cited for {clause:?}");
        for fixture in pinned_by {
            assert!(
                is_test_fn(fixture),
                "the clause {clause:?} names `{fixture}` as the fixture that makes it true, \
                 and no `#[test]` of that name exists in this module"
            );
        }
    };

    // How the requirement set is arrived at, and where it is enforced. The proc-macro half of
    // "where it runs" is covered by `macro_manifest_audit_derives_only_capabilities_referenced_by_the_api`
    // in `tests/e2e.rs`, which is outside this file and so cannot be cited below.
    promises(
        "Spargen derives the exact requirement set after lowering and audits Cargo.toml during \
         build.rs and proc-macro generation",
        &[
            "conditional_dependencies_and_features_are_required_only_when_used",
            "the_time_requirement_never_asks_for_serde",
        ],
    );

    // What the requirement set is: exactly what the API references, so a construct the API
    // does not use asks for nothing, and one it does use asks for its crate and feature.
    promises(
        "its consuming Cargo package must declare the crates and dependency features \
         referenced by that specific API",
        &["conditional_dependencies_and_features_are_required_only_when_used"],
    );

    // The two version and feature rules the audit rejects a violation of. The first fixture
    // accepts each floor and a higher compatible caret and refuses a range reaching the next
    // incompatible release; the second rejects a requirement admitting a release below it.
    promises(
        "Use the documented tested lower bounds (or a higher semver-compatible caret floor)",
        &[
            "exact_floors_and_higher_compatible_caret_requirements_are_supported",
            "a_requirement_that_admits_a_version_below_the_floor_is_rejected",
        ],
    );
    promises(
        "keep reqwest default features disabled",
        &["reqwest_defaults_and_blocking_wiring_are_part_of_the_contract"],
    );

    // The sentence every clause below qualifies, and the one this test exists to pin: a member
    // declaring nothing but `workspace = true` resolves against the root's table.
    promises(
        "A dependency declared `workspace = true` is followed to the workspace root's \
         `[workspace.dependencies]`",
        &["workspace_inheritance_uses_the_workspace_version_and_features"],
    );

    // Which side decides default features, in both directions. Asserting the rule rather than
    // the bare words `default-features` is what makes a negation of it fail here.
    let both_directions: &[&str] = &[
        "a_member_default_features_false_cannot_turn_off_defaults_the_root_leaves_on",
        "a_member_default_features_true_turns_on_defaults_the_root_turned_off",
    ];
    promises(
        "default features on when the root leaves them on or the member sets \
         `default-features = true`",
        both_directions,
    );
    promises(
        "a member's `default-features = false` cannot turn off defaults the root leaves on",
        both_directions,
    );
    // The layout that rule leaves a consumer. The `unset` half is the core workspace layout
    // itself: its root disables `reqwest`'s defaults and its member declares only
    // `reqwest.workspace = true`, and it must audit clean.
    promises(
        "disable them in `[workspace.dependencies]` and leave the member's `default-features` \
         unset or `false`",
        &[
            "a_member_default_features_false_keeps_the_defaults_the_root_turned_off",
            "workspace_inheritance_uses_the_workspace_version_and_features",
        ],
    );

    // The outcomes a *found* root has, and the promise that the diagnostic tells them apart
    // instead of reporting the crate as missing. Each fixture below observes one of them, and
    // the clause now says only what the resolver does: `check_declaration` renders
    // `WorkspaceOrigin::Resolved` as "declares no `x` there" and `Unreadable` as "could not be
    // read", so a found root's two failures are distinguished.
    let unresolved_outcomes: &[&str] = &[
        "an_unresolvable_inheritance_says_where_the_lookup_went",
        "a_workspace_root_that_cannot_be_read_is_not_reported_as_missing",
        "a_package_workspace_naming_a_directory_without_a_manifest_is_a_workspace_read_failure",
        "a_self_rooted_manifest_names_an_absolute_path_when_an_entry_is_missing",
    ];
    promises(
        "when a root is found but cannot be read, or declares no such entry",
        unresolved_outcomes,
    );
    promises(
        "the diagnostic says which of those happened rather than reporting the crate as missing",
        unresolved_outcomes,
    );

    // The no-root-found outcome. `workspace_root` keeps the *first* unparseable candidate the
    // walk met, but nothing established it was the root — it may be a sibling crate or a stray
    // file far above the project — so `check_declaration` reports not-found first and renders
    // that candidate only as a conditional hint, never as "its workspace manifest" (#171).
    // Each clause below is observed by the fixtures it cites: not-found with no candidate at
    // all, not-found with one, and nearest-over-far with two.
    promises(
        "when no root is found at all it says that no workspace manifest was found",
        &[
            "an_unresolvable_inheritance_says_where_the_lookup_went",
            "a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found",
        ],
    );
    // "If there was one" is the no-candidate half: the orphaned member in the first fixture
    // meets no manifest on its walk, and its message carries no hint.
    promises(
        "names the nearest ancestor manifest that failed to read, if there was one",
        &[
            "an_unresolvable_inheritance_says_where_the_lookup_went",
            "a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found",
            "the_nearest_corrupt_ancestor_is_reported_although_no_workspace_root_was_found",
        ],
    );
    promises(
        "only as a possible root together with the reason it could not be read — never as the \
         workspace manifest",
        &["a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found"],
    );

    // The other limb, which *is* the workspace manifest: a root named by `package.workspace`
    // is read, so its failure is a diagnostic of its own and the inheritance message stays
    // bare. The fixtures below assert the exact strings — appending the reason, or renaming
    // the file to anything but "its workspace manifest", reds them. Each also asserts the read
    // failure of its own ("failed to parse" and "failed to read workspace manifest"), which
    // is the root being read and reported in its own right.
    promises(
        "A root reached through `package.workspace` is read and reported in its own right",
        &[
            "a_workspace_root_that_cannot_be_read_is_not_reported_as_missing",
            "a_package_workspace_naming_a_directory_without_a_manifest_is_a_workspace_read_failure",
        ],
    );
    promises(
        "its inheritance message carries no reason of its own and a read-failure diagnostic \
         naming the same file stands above it",
        &[
            "a_workspace_root_that_cannot_be_read_is_not_reported_as_missing",
            "a_package_workspace_naming_a_directory_without_a_manifest_is_a_workspace_read_failure",
        ],
    );

    // The three-way root search, each branch with the fixtures that exercise it. The precedence
    // between them is pinned behaviourally by
    // `package_workspace_is_consulted_before_the_ancestor_walk` and
    // `a_self_declared_workspace_wins_over_package_workspace`; the assertion below pins only
    // that the text states it in that order.
    let root_search = [
        (
            "the consumer manifest itself when it declares `[workspace]`",
            &[
                "a_root_package_inherits_its_own_workspace_dependencies",
                "a_self_declared_workspace_wins_over_package_workspace",
            ][..],
        ),
        (
            "otherwise the root `package.workspace` names",
            &[
                "package_workspace_names_the_workspace_root_directory",
                "a_relative_manifest_path_still_resolves_the_workspace_root",
                "package_workspace_is_consulted_before_the_ancestor_walk",
            ][..],
        ),
        (
            "otherwise the nearest ancestor manifest that parses and declares `[workspace]`",
            &[
                "a_broken_manifest_below_the_real_root_does_not_stop_the_walk",
                "an_unparseable_ancestor_manifest_is_not_an_error_on_its_own",
                "the_walk_climbs_past_an_ancestor_that_parses_and_declares_no_workspace",
            ][..],
        ),
    ];
    for (clause, pinned_by) in root_search {
        promises(clause, pinned_by);
    }
    // Precedence is a claim the three checks above do not make: all three would still pass with
    // the order reversed, and `workspace_root` tries them in exactly this order.
    let found = root_search.map(|(clause, _)| explain.find(clause));
    assert!(
        found[0] < found[1] && found[1] < found[2],
        "`spargen explain E023` states the root search out of the order `workspace_root` \
         performs it; the three branches appear at {found:?}"
    );

    // What is taken from the root once it is found: the first fixture's root carries every
    // version and `serde`'s `derive` while its member declares a bare `workspace = true`, and
    // the second adds `features = ["stream"]` to the member's inherited `reqwest`, which is the
    // member's half of the union.
    let from_the_root: &[&str] = &[
        "workspace_inheritance_uses_the_workspace_version_and_features",
        "the_manifests_reported_in_issue_71_pass_as_written",
    ];
    promises("taking the version from there", from_the_root);
    promises("the union of both feature lists", from_the_root);

    // The one field that stays with the member, in both directions: required-optional, and
    // forbidden-optional on a crate generated code names unconditionally.
    promises(
        "while `optional` is read from the member",
        &[
            "workspace_inherited_tokio_under_an_alternative_spelling_resolves",
            "an_inherited_member_cannot_make_an_unconditional_crate_optional",
        ],
    );

    // The clause that is the answer to #71 itself: an inherited declaration is accepted rather
    // than reported as missing. Both fixtures assert the audit emits nothing at all for five
    // crates the member only inherits.
    promises(
        "Inheriting a required crate therefore satisfies the audit",
        from_the_root,
    );

    // Coverage: the relation above made total. Every check so far is a substring test, so on
    // its own it constrains only the clauses it cites, and a sentence appended or prefixed to
    // the body — contradicting all of them — would leave it green. Here every byte of the body
    // must fall inside exactly one claimed span, or be punctuation, or spell one of these
    // connectives, which join claims without making one. A connective that could reverse a
    // claim ("not", "never", "unless") is deliberately absent.
    const CONNECTIVES: &[&str] = &["and", "so", "then"];
    let mut covered = vec![false; explain.len()];
    for clause in &claimed {
        let at = explain
            .find(clause)
            .expect("each claimed clause was asserted to occur");
        for byte in &mut covered[at..at + clause.len()] {
            assert!(
                !*byte,
                "{clause:?} overlaps a span another `promises` or `unasserted` entry already \
                 claims; each part of the body is classified exactly once"
            );
            *byte = true;
        }
    }
    // Claimed spans begin and end at clause boundaries, which are character boundaries, so
    // each unclaimed run is a valid `str` slice.
    let mut start = 0;
    while start < explain.len() {
        if covered[start] {
            start += 1;
            continue;
        }
        let end = covered[start..]
            .iter()
            .position(|byte| *byte)
            .map_or(explain.len(), |offset| start + offset);
        let run = &explain[start..end];
        let stray = run
            .split(|character: char| !character.is_alphanumeric())
            .find(|word| !word.is_empty() && !CONNECTIVES.contains(word));
        // The unclaimed run is printed: unlike the body, it is not a paste that makes the
        // failure go away, because the fix is a `promises` entry citing a fixture or an
        // `unasserted` entry stating why none exists, each a decision this message cannot take.
        assert!(
            stray.is_none(),
            "`spargen explain E023` says {run:?}, which no `promises` or `unasserted` entry \
             claims. Cite the `#[test]` that makes it true with `promises`, or, if nothing in \
             this module observes it, list it with `unasserted` and the reason."
        );
        start = end;
    }

    // And the body as a whole, against the expected file. This pins drift between the two
    // copies and nothing more: it is symmetric, so text typed into both passes it, and the
    // coverage check above is what refuses an unclassified addition.
    //
    // Deliberately not `assert_eq!`: its output prints the actual value in full, which beside
    // an instruction to update the expected file amounts to handing over the paste that makes
    // any change pass. The point of this assertion is that a reader has to decide the new text
    // is correct, so it reports where the two diverge and nothing more.
    assert!(
        explain == expected,
        "the `E023` explain body no longer matches \
         `spargen/src/runtime_contract_e023_explain.txt`; they first differ at byte {}. Read \
         the new text, satisfy yourself that every clause asserted above is still true of \
         `workspace_root` and `check_declaration`, and only then re-type the change into that \
         file.",
        explain
            .char_indices()
            .zip(expected.chars())
            .find(|((_, actual), expected)| actual != expected)
            .map_or_else(|| explain.len().min(expected.len()), |((at, _), _)| at)
    );
}
