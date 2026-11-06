use super::*;

#[test]
fn a_missing_entry_names_the_absolutized_manifest_the_audit_read() {
    // #339: the report behind #71 said a required crate was missing and never said from which
    // `Cargo.toml`, so a macro that audited the workspace root instead of the member could not
    // be told apart from a member that really lacked the entry. Here the member lacks `bytes`
    // and its workspace root sits one directory up; the member is reached by a relative path,
    // the spelling a build driver may hand over, and the message names it as a path the reader
    // can open. The messages are read as rendered, not through `Sandbox::audit_in`'s strip.
    let directory = Sandbox::new();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    std::fs::create_dir(directory.path().join("member")).unwrap();
    let without_bytes = CORE_MANIFEST
        .lines()
        .filter(|line| !line.starts_with("bytes"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(directory.path().join("member/Cargo.toml"), without_bytes).unwrap();

    let _lock = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
    std::env::set_current_dir(directory.path()).unwrap();

    let result = directory.audit_unstripped(
        Utf8Path::new("member/Cargo.toml"),
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
    let member = directory.root.join("member/Cargo.toml");
    let messages = result
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        [format!(
            "{}; audited manifest: `{member}`",
            missing_message(BYTES)
        )],
        "the missing entry has to name the member it was looked for in"
    );
    // Named once, and never as the workspace root above it, which the audit did not read.
    assert_eq!(
        messages[0].matches(member.as_str()).count(),
        1,
        "{messages:#?}"
    );
    assert!(
        !messages[0].contains(&format!("`{}`", directory.root.join("Cargo.toml"))),
        "{messages:#?}"
    );
}

#[test]
fn a_self_rooted_manifest_names_an_absolute_path_when_an_entry_is_missing() {
    // The layout the relative-path fix exists for: a single-crate repository whose `[package]`
    // and `[workspace]` share one file, reached through a relative manifest path. The
    // diagnostic names the manifest it resolved against, and naming it
    // `./Cargo.toml` would tell the reader nothing about which file to open.
    let directory = Sandbox::new();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[workspace]\n\n\
             [workspace.dependencies]\n{}\n\n{CORE_INHERITED}",
            core_workspace_dependencies()
                .lines()
                .filter(|line| !line.starts_with("bytes"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();

    let _lock = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
    std::env::set_current_dir(directory.path()).unwrap();

    let result = directory.audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
    assert_eq!(result.diagnostics.len(), 1, "{:#?}", result.diagnostics);
    let message = &result.diagnostics[0].message;
    assert!(
        message.contains("`bytes` inherits") && message.contains("declares no `bytes` there"),
        "{message}"
    );
    // The point of the fix: a path the reader can open, not the caller's bare spelling.
    assert!(
        !message.contains("`Cargo.toml` declares") && !message.contains("`./Cargo.toml` declares"),
        "the diagnostic names the raw spelling rather than a path the reader can open: \
         {message}"
    );
}

#[test]
fn a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found() {
    // No `[workspace]` anywhere on the walk, and one ancestor that does not parse. Nothing
    // establishes that file is the workspace root — in a real layout it is as likely a sibling
    // crate, `$HOME/Cargo.toml`, or a stray scratch file — so the diagnostic must say *not
    // found*, and mention the file only as a possible cause. It once reported the file as "its
    // workspace manifest", sending the reader to repair something unrelated to their build,
    // after which the same `E023` recurred because the repaired file declares no `[workspace]`
    // either (#171).
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(&root, "[workspace\nthis is not toml\n").unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    // Every inherited crate is reported, each as not found and each carrying the hint — the
    // whole message pinned as one string from the verdict through the reason, so the verdict
    // cannot be dropped, the hint cannot be promoted back to the root, and the reason cannot
    // move away from the file it explains with this fixture still green. The reason has to
    // ride inline: the candidate is never audited as a manifest, so nothing else prints why
    // it failed.
    for dependency in [
        "bytes",
        "http",
        "http-body",
        "reqwest",
        "secrecy",
        "serde",
        "serde_json",
    ] {
        let expected = format!(
            "`{dependency}` inherits from `[workspace.dependencies]`, but no workspace \
             manifest was found above `{member}`; if the workspace root is `{root}`, it could \
             not be read: TOML parse error"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.starts_with(&expected)),
            "{expected}\n{:#?}",
            result.diagnostics
        );
    }
    // The file is never called what nothing established it to be.
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("its workspace manifest")),
        "{:#?}",
        result.diagnostics
    );
    // Nothing reads it as a manifest: no read-failure diagnostic of its own — which is what a
    // root named by `package.workspace` has instead — and no rebuild trigger on it.
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.starts_with("failed to ")),
        "nothing else reports this file, so the message must carry its reason: {:#?}",
        result.diagnostics
    );
    assert_eq!(result.manifests, vec![member], "{:#?}", result.manifests);
}

#[test]
fn a_workspace_root_that_failed_to_parse_explains_itself_once() {
    // `WorkspaceOrigin::Unreadable` carries no reason for a root named by `package.workspace`,
    // and its doc argues why: that root *is* audited, so `read_toml` already reported the parse
    // failure on its own line and "repeating it here would print it twice". Threading the real
    // reason through would do exactly that, and nothing noticed.
    let directory = Sandbox::new();
    let root_dir = directory.path().join("root");
    let member_dir = directory.path().join("outside");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(root_dir.join("Cargo.toml"), "[workspace\nbroken = ").unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
             {CORE_INHERITED}"
        ),
    )
    .unwrap();

    let diagnostics = directory
        .audit(&member, &RuntimeRequirements::default())
        .diagnostics;
    let message = messages(&diagnostics);
    // Reported once, by the audit of the root itself.
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("failed to parse workspace manifest"))
            .count(),
        1,
        "{message}"
    );
    // And not a second time on every inheritance that could not resolve: those say the file
    // could not be read, without restating why.
    assert!(message.contains("could not be read"), "{message}");
    assert!(!message.contains("could not be read: "), "{message}");
}

#[test]
fn the_nearest_corrupt_ancestor_is_reported_although_no_workspace_root_was_found() {
    // The walk remembers the *first* unparseable candidate it meets and never overwrites it,
    // so of the files a reader might be sent to open, it is the one closest to their crate.
    // With two broken manifests on one path and no `[workspace]` anywhere, only that choice is
    // observable, and no other fixture puts two of them on a single walk.
    //
    // It is not a second copy of
    // `a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found`, which pins the
    // wording of the hint: this one says only that the hint names the *near* broken manifest
    // and not the far one. No other fixture in this module puts two broken manifests on one
    // walk, so deleting it would leave the nearest-wins rule in `workspace_root` unguarded.
    let directory = Sandbox::new();
    let far = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let near_dir = directory.path().join("near");
    let member_dir = near_dir.join("client");
    std::fs::create_dir_all(&member_dir).unwrap();
    let near = Utf8PathBuf::from_path_buf(near_dir.join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(&far, "[workspace\nbroken = ").unwrap();
    std::fs::write(&near, "[workspace\nalso broken = ").unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let diagnostics = directory
        .audit(&member, &RuntimeRequirements::default())
        .diagnostics;
    let message = messages(&diagnostics);
    assert!(message.contains(near.as_str()), "{message}");
    assert!(!message.contains(far.as_str()), "{message}");
}

#[test]
fn an_unresolvable_inheritance_says_where_the_lookup_went() {
    // Two opposite situations get distinct messages: the workspace has no such entry (fix the
    // root), and no workspace was found at all (fix the layout, or spell the version out). One
    // message for both is the ambiguity behind #71.
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    // No workspace manifest anywhere above the member.
    let orphaned = directory.audit(&member, &RuntimeRequirements::default());
    for dependency in [
        "bytes",
        "http",
        "http-body",
        "reqwest",
        "secrecy",
        "serde",
        "serde_json",
    ] {
        assert!(
            orphaned.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains(&format!("`{dependency}` inherits"))
                    && diagnostic
                        .message
                        .contains("no workspace manifest was found above")
                    && diagnostic.message.contains(member.as_str())
            }),
            "{:#?}",
            orphaned.diagnostics
        );
    }
    // Nothing unreadable was met on the walk, so no possible root is named: the E023 explain
    // text's "if there was one".
    assert!(
        !orphaned
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("if the workspace root is")),
        "{:#?}",
        orphaned.diagnostics
    );

    // A workspace that resolves, but declares six of the seven.
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
                .lines()
                .filter(|line| !line.starts_with("bytes"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    let partial = directory.audit(&member, &RuntimeRequirements::default());
    assert_eq!(partial.diagnostics.len(), 1, "{:#?}", partial.diagnostics);
    let message = &partial.diagnostics[0].message;
    assert!(
        message.contains("`bytes` inherits")
            && message.contains(root.as_str())
            && message.contains("declares no `bytes` there"),
        "{message}"
    );
}

/// The reason `read_toml` appends to ``<prefix>`<path>` ``, after the closing backtick and
/// its `: ` separator, or `None` where the message does not continue that way.
///
/// The `package.workspace` limb of `E023` defers its whole account of the failure to this
/// line, so a renderer that dropped the reason would leave the failure unexplained anywhere
/// while every prefix and path assertion stayed green. Callers compare what this returns to
/// the error the same operation yields when the fixture repeats it, so the reason is pinned
/// exactly rather than merely required to be non-empty.
fn reason_after<'m>(message: &'m str, prefix: &str, path: &str) -> Option<&'m str> {
    message
        .strip_prefix(prefix)?
        .strip_prefix(path)?
        .strip_prefix("`: ")
}

#[test]
fn a_workspace_root_that_cannot_be_read_is_not_reported_as_missing() {
    // Found-but-broken is a third state. Reporting it as "no workspace manifest was found"
    // contradicts the read failure reported beside it and points at the opposite remedy.
    let directory = Sandbox::new();
    let root_dir = directory.path().join("root");
    let member_dir = directory.path().join("outside");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::create_dir(&member_dir).unwrap();
    let root = Utf8PathBuf::from_path_buf(root_dir.join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    // `package.workspace` names the root explicitly, so the walk does not get to pre-parse and
    // silently skip it: the audit reads exactly this file, and it does not parse.
    std::fs::write(&root, "[workspace\nthis is not toml\n").unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
             {CORE_INHERITED}"
        ),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    // The read failure names the file for what it is. Calling the workspace root "the consumer
    // manifest" contradicted the inheritance diagnostic asserted just below, which calls the
    // same path a workspace manifest.
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("failed to parse workspace manifest")),
        "{:#?}",
        result.diagnostics
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("consumer manifest")),
        "the file that failed is the workspace root, not the consumer's own manifest: {:#?}",
        result.diagnostics
    );
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("`bytes` inherits")
                && diagnostic.message.contains("could not be read")
        }),
        "{:#?}",
        result.diagnostics
    );
    assert!(
        !result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("no workspace manifest was found")),
        "found-but-broken must not be reported as missing: {:#?}",
        result.diagnostics
    );
    // The mirror of the rule `a_corrupt_ancestor_is_only_a_hint_when_no_workspace_root_was_found`
    // pins from the other side, and the rule the `E023` explain text hands the reader: a root
    // the consumer's own `package.workspace` named is read in its own right, so its failure is
    // a diagnostic standing *above* the inheritance message, and the inheritance message
    // carries **no** `: {reason}` suffix. Both halves are asserted, because a renderer that
    // started appending the reason here — or that renamed the file to anything but "its
    // workspace manifest" — would falsify the text while every assertion above stayed green.
    let failure = result
        .diagnostics
        .iter()
        .position(|diagnostic| {
            diagnostic
                .message
                .starts_with("failed to parse workspace manifest `")
        })
        .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    let inherits = result
        .diagnostics
        .iter()
        .position(|diagnostic| {
            diagnostic.message.contains("`bytes` inherits")
                && diagnostic.message.contains("its workspace manifest `")
                && diagnostic.message.contains("` could not be read")
        })
        .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    assert!(
        failure < inherits,
        "the read failure is what the inheritance message defers to, so it has to be read \
         first: {:#?}",
        result.diagnostics
    );
    // "a read-failure diagnostic naming the same file stands above it" is two claims, and
    // matching each message by its own prefix pins only the ordering half. A renderer that put
    // the *consumer* manifest's path on the read-failure line would point the reader at a
    // different file with every assertion above still green. So take the path the inheritance
    // message names and require the read failure to name that same one. They are compared to
    // each other *and* to the path this fixture itself builds, never by `contains`, a substring
    // relation an empty path, a bare `Cargo.toml`, or the root *directory* satisfies. Agreement
    // alone is no better: two renderers degrading the same way still agree, and so does one
    // naming a manifest nothing ever opened. The expected
    // spelling is not `root`: `package.workspace` is joined, not normalised, so both messages
    // spell the root `<tmp>/outside/../root/Cargo.toml`, and camino joins lexically, so
    // building it the same way here is portable and needs no canonicalisation.
    let named = manifest_named_after(
        &result.diagnostics[inherits].message,
        "its workspace manifest `",
    )
    .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    let blamed = manifest_named_after(
        &result.diagnostics[failure].message,
        "failed to parse workspace manifest `",
    )
    .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    assert_eq!(
        named, blamed,
        "the read failure has to name the same file the inheritance message defers to: {:#?}",
        result.diagnostics
    );
    let read = member.parent().unwrap().join("../root").join("Cargo.toml");
    assert_eq!(
        named, read,
        "both messages have to name the manifest the audit actually read: {:#?}",
        result.diagnostics
    );
    // The line the inheritance message defers to has to actually carry the reason: it is the
    // only account of the failure this limb prints. The expected reason is the parse error
    // the same file yields when parsed again here, so the whole `: {error}` suffix is pinned,
    // less the trailing newline `diagnostic` trims before naming the audited manifest.
    let unparsed = toml::from_str::<toml::Value>(&std::fs::read_to_string(&root).unwrap())
        .unwrap_err()
        .to_string()
        .trim_end()
        .to_owned();
    assert_eq!(
        reason_after(
            &result.diagnostics[failure].message,
            "failed to parse workspace manifest `",
            read.as_str(),
        ),
        Some(unparsed.as_str()),
        "the read failure has to say why the root could not be parsed: {:#?}",
        result.diagnostics
    );
    // A root that failed to parse is still the file a consumer edits next, so it has to stay
    // a rebuild input: recording it only once it parsed would make repairing it a no-op.
    assert_eq!(
        result.manifests,
        vec![read, member],
        "{:#?}",
        result.manifests
    );
    // And the reason must not ride inline in *any* shape, not merely without a colon. The
    // explain text says this limb's inheritance message carries no reason of its own, so a
    // separator of any kind appended here falsifies it. Pinning the end of the message forbids
    // every separator, an em dash among them, where `!contains("could not be read:")` would
    // forbid one spelling. `location` is empty here: one counting table.
    assert!(
        result.diagnostics[inherits]
            .message
            .ends_with("could not be read"),
        "a root reported on its own line must not repeat its reason inline: {:#?}",
        result.diagnostics
    );
}

#[test]
fn a_consumer_manifest_that_cannot_be_read_or_parsed_is_named_as_the_consumer_manifest() {
    // The other half of the role noun: the consumer's own manifest is never called the
    // workspace manifest, on either failure. Swapping the two call-site nouns fails this test.
    let directory = Sandbox::new();

    let unparseable = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    std::fs::write(&unparseable, "[package\nnot toml at all\n").unwrap();
    let result = directory.audit(&unparseable, &RuntimeRequirements::default());
    assert_eq!(result.diagnostics.len(), 1, "{:#?}", result.diagnostics);
    let message = &result.diagnostics[0].message;
    assert!(
        message.contains("failed to parse consumer manifest"),
        "{message}"
    );
    assert!(!message.contains("workspace manifest"), "{message}");
    // The consumer's own failure is reported on this one line and nowhere else, so the line
    // has to carry its reason: exactly the error parsing the same file again yields, less the
    // trailing newline `diagnostic` trims before naming the audited manifest.
    let unparsed = toml::from_str::<toml::Value>(&std::fs::read_to_string(&unparseable).unwrap())
        .unwrap_err()
        .to_string()
        .trim_end()
        .to_owned();
    assert_eq!(
        reason_after(
            message,
            "failed to parse consumer manifest `",
            unparseable.as_str()
        ),
        Some(unparsed.as_str()),
        "{message}"
    );
    // Nothing past the consumer manifest was looked up, so nothing else is a rebuild input.
    assert_eq!(result.manifests, vec![unparseable]);

    let absent =
        Utf8PathBuf::from_path_buf(directory.path().join("absent").join("Cargo.toml")).unwrap();
    let result = directory.audit(&absent, &RuntimeRequirements::default());
    assert_eq!(result.diagnostics.len(), 1, "{:#?}", result.diagnostics);
    let message = &result.diagnostics[0].message;
    assert!(
        message.contains("failed to read consumer manifest"),
        "{message}"
    );
    assert!(!message.contains("workspace manifest"), "{message}");
    let unread = std::fs::read_to_string(&absent).unwrap_err().to_string();
    assert_eq!(
        reason_after(
            message,
            "failed to read consumer manifest `",
            absent.as_str()
        ),
        Some(unread.as_str()),
        "{message}"
    );
}

#[test]
fn a_package_workspace_naming_a_directory_without_a_manifest_is_a_workspace_read_failure() {
    // `package.workspace` is taken at its word, so a root directory holding no `Cargo.toml` is
    // a workspace manifest that could not be *read* — not a parse failure, and not a missing
    // root.
    let directory = Sandbox::new();
    let root_dir = directory.path().join("root");
    let member_dir = directory.path().join("outside");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
             {CORE_INHERITED}"
        ),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    let any = |needle: &str| {
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(needle))
    };
    assert!(
        any("failed to read workspace manifest"),
        "{:#?}",
        result.diagnostics
    );
    assert!(!any("failed to parse"), "{:#?}", result.diagnostics);
    assert!(!any("consumer manifest"), "{:#?}", result.diagnostics);
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("`bytes` inherits")
                && diagnostic.message.contains("could not be read")
        }),
        "{:#?}",
        result.diagnostics
    );
    assert!(
        !any("no workspace manifest was found"),
        "{:#?}",
        result.diagnostics
    );
    // The whole identification rule as in
    // `a_workspace_root_that_cannot_be_read_is_not_reported_as_missing`, over the other failure
    // a declared root can have: the read failure stands above the inheritance message, names
    // the same manifest, and the reason stays on its own line so the inheritance message ends
    // where the noun phrase ends. A read failure and a parse failure are rendered by the same
    // arm, so both limbs have to hold all three — and both hold the last against an appended
    // reason of any shape, not just one introduced by a colon. Asserting all three here rather
    // than only the suffix is what stops the shipped clause resting on one fixture.
    let failure = result
        .diagnostics
        .iter()
        .position(|diagnostic| {
            diagnostic
                .message
                .starts_with("failed to read workspace manifest `")
        })
        .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    let inherits = result
        .diagnostics
        .iter()
        .position(|diagnostic| {
            diagnostic.message.contains("`bytes` inherits")
                && diagnostic.message.contains("its workspace manifest `")
                && diagnostic.message.contains("` could not be read")
        })
        .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    assert!(
        failure < inherits,
        "the read failure is what the inheritance message defers to, so it has to be read \
         first: {:#?}",
        result.diagnostics
    );
    assert!(
        result.diagnostics[inherits]
            .message
            .ends_with("could not be read"),
        "a root reported on its own line must not repeat its reason inline: {:#?}",
        result.diagnostics
    );
    let named = manifest_named_after(
        &result.diagnostics[inherits].message,
        "its workspace manifest `",
    )
    .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    let blamed = manifest_named_after(
        &result.diagnostics[failure].message,
        "failed to read workspace manifest `",
    )
    .unwrap_or_else(|| panic!("{:#?}", result.diagnostics));
    assert_eq!(
        named, blamed,
        "the read failure has to name the same file the inheritance message defers to: {:#?}",
        result.diagnostics
    );
    let read = member.parent().unwrap().join("../root").join("Cargo.toml");
    assert_eq!(
        named, read,
        "both messages have to name the manifest the audit actually read: {:#?}",
        result.diagnostics
    );
    // The same two obligations as the parse limb: the line the inheritance message defers to
    // carries the reason (exactly the error reading that path again yields), and the root
    // that could not be read is still a rebuild input, since creating it is the repair.
    let unread = std::fs::read_to_string(&read).unwrap_err().to_string();
    assert_eq!(
        reason_after(
            &result.diagnostics[failure].message,
            "failed to read workspace manifest `",
            read.as_str(),
        ),
        Some(unread.as_str()),
        "the read failure has to say why the root could not be read: {:#?}",
        result.diagnostics
    );
    assert_eq!(
        result.manifests,
        vec![read, member],
        "{:#?}",
        result.manifests
    );
}
