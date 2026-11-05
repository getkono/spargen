use super::*;

#[test]
fn package_workspace_names_the_workspace_root_directory() {
    // Cargo's `package.workspace` is a path to the root *directory*, not to its manifest. The
    // root here is deliberately not an ancestor of the member, so only that field can resolve
    // it and the assertion cannot pass through the ancestor walk by accident.
    let directory = Sandbox::new();
    let root_dir = directory.path().join("root");
    let member_dir = directory.path().join("outside");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::create_dir(&member_dir).unwrap();
    let root = Utf8PathBuf::from_path_buf(root_dir.join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = []\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../root\"\n\n\
             {CORE_INHERITED}"
        ),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    // The root is reported as the field spells it — `…/outside/../root/Cargo.toml`. `..` is
    // deliberately not folded away: doing that lexically changes which file a path names when
    // a component is a symlink, and Cargo accepts the unfolded form for `rerun-if-changed`
    // just the same.
    assert_eq!(result.manifests.len(), 2, "{:#?}", result.manifests);
    assert!(
        result
            .manifests
            .iter()
            .any(|path| path.canonicalize().ok() == root.canonicalize().ok()),
        "{:#?}",
        result.manifests
    );
}

#[test]
fn package_workspace_is_consulted_before_the_ancestor_walk() {
    // The first edge of the documented precedence, and the one no fixture put in a single
    // layout: a member that names a root with `package.workspace` *and* sits under an ancestor
    // that is a perfectly good workspace root. Cargo takes the field; so must the audit. If the
    // walk were consulted first the ancestor would resolve every inherited dependency and the
    // audit would fall silent about a root the member explicitly named and that does not exist.
    let directory = Sandbox::new();
    let ancestor = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &ancestor,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../elsewhere\"\n\n\
             {CORE_INHERITED}"
        ),
    )
    .unwrap();

    let diagnostics = directory
        .audit(&member, &RuntimeRequirements::default())
        .diagnostics;
    let message = messages(&diagnostics);
    assert!(!diagnostics.is_empty(), "the named root does not exist");
    // Which file the audit consulted is a path identity, so it is compared whole: a
    // `contains("elsewhere")` check was satisfied by the root *directory* and by a nested
    // manifest nobody named. `package.workspace` is joined lexically, so the expected spelling
    // keeps the `..`. Both the read failure and the inheritance message that defers to it
    // have to name exactly that file.
    let named = member
        .parent()
        .unwrap()
        .join("../elsewhere")
        .join("Cargo.toml");
    let blamed = diagnostics
        .iter()
        .find_map(|diagnostic| {
            manifest_named_after(&diagnostic.message, "failed to read workspace manifest `")
        })
        .unwrap_or_else(|| panic!("{message}"));
    assert_eq!(blamed, named, "{message}");
    let deferred = diagnostics
        .iter()
        .find_map(|diagnostic| {
            manifest_named_after(&diagnostic.message, "its workspace manifest `")
        })
        .unwrap_or_else(|| panic!("{message}"));
    assert_eq!(deferred, named, "{message}");
}

#[test]
fn a_self_declared_workspace_wins_over_package_workspace() {
    // The other edge: `[workspace]` in the consumer manifest is checked before
    // `package.workspace`, so a manifest carrying both resolves against its own table. Reversed,
    // the audit would chase a directory that is not there and report every inherited dependency
    // as unresolvable, about a table it had already parsed.
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nworkspace = \"../nowhere\"\n\n\
             [workspace]\n\n[workspace.dependencies]\n{}\n{CORE_INHERITED}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.manifests, vec![member]);
}

#[test]
fn a_relative_manifest_path_still_resolves_the_workspace_root() {
    // A build driver may name the manifest relatively (`CARGO_MANIFEST_DIR=.`), and
    // `generate_api!` passes it on as given. A one-component path has no ancestors to walk, so
    // the workspace root was never found and every inherited dependency reported as
    // unresolvable.
    let directory = Sandbox::new();
    let root = directory.path().join("Cargo.toml");
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    std::fs::write(
        member_dir.join("Cargo.toml"),
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let _lock = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
    std::env::set_current_dir(&member_dir).unwrap();

    let result = directory.audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    // The member as given plus the workspace root the walk reached. The root's textual form
    // depends on how the platform resolves the temporary directory, so only the count is
    // asserted.
    assert_eq!(result.manifests.len(), 2, "{:#?}", result.manifests);
}

#[test]
fn a_relative_manifest_path_with_no_root_names_the_absolute_path_searched_from() {
    // "No workspace manifest was found above `Cargo.toml`" names no directory at all. The
    // search runs from the absolutized path, and the message must name that one rather than
    // the caller's spelling (#202).
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        member_dir.join("Cargo.toml"),
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let _lock = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
    std::env::set_current_dir(&member_dir).unwrap();
    // How the platform spells the temporary directory is its own business (`/tmp` may be a
    // symlink), so the expected path is the working directory as the process reports it.
    let searched_from =
        Utf8PathBuf::from_path_buf(std::env::current_dir().unwrap().join("Cargo.toml")).unwrap();

    let result = directory.audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
    assert!(!result.diagnostics.is_empty());
    for diagnostic in &result.diagnostics {
        assert_eq!(
            manifest_named_after(
                &diagnostic.message,
                "no workspace manifest was found above `"
            ),
            Some(searched_from.as_str()),
            "{}",
            diagnostic.message
        );
    }
}

#[cfg(unix)]
#[test]
fn the_walk_climbs_a_symlinked_member_path_as_written() {
    // Absolutization is lexical: `std::path::absolute`, not `canonicalize` (#202). The two
    // disagree exactly here — a member reached through a symlinked directory. Cargo climbs the
    // path it was given, which is the path it hands the build as `CARGO_MANIFEST_DIR`, so the
    // root is the one above the link, not the one above where the link points.
    let directory = Sandbox::new();
    let lexical = directory.path().join("lexical");
    let physical = directory.path().join("physical");
    std::fs::create_dir_all(physical.join("client")).unwrap();
    std::fs::create_dir(&lexical).unwrap();
    std::os::unix::fs::symlink(physical.join("client"), lexical.join("client")).unwrap();
    let lexical_root = Utf8PathBuf::from_path_buf(lexical.join("Cargo.toml")).unwrap();
    let physical_root = Utf8PathBuf::from_path_buf(physical.join("Cargo.toml")).unwrap();
    std::fs::write(
        &lexical_root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    // A root above the link's target that declares nothing, so resolving against it cannot
    // pass for resolving against the lexical one.
    std::fs::write(
        &physical_root,
        "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n",
    )
    .unwrap();
    let member = Utf8PathBuf::from_path_buf(lexical.join("client").join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(
        result.manifests,
        vec![lexical_root, member],
        "the workspace root must be the one above the link as written"
    );
}

#[test]
fn an_unparseable_ancestor_manifest_is_not_an_error_on_its_own() {
    // The walk climbs to the filesystem root on every standalone crate, because a `cargo new`
    // manifest declares no `[workspace]`. Anything unreadable it passes on the way — a broken
    // `Cargo.toml`, one it lacks permission to read — must stay invisible to a consumer that
    // inherits nothing: it is not this crate's workspace root, and nothing here can tell
    // whether it is anyone's. Treating it as one turned an ordinary build into a hard `E023`.
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        "[workspace\nthis is not toml\n",
    )
    .unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    // Declares every runtime dependency directly — nothing inherits, so nothing needs a root.
    std::fs::write(&member, CORE_MANIFEST).unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    // And it is not recorded as a build input either: a `rerun-if-changed` on an unrelated
    // file would rebuild the consumer whenever it changed.
    assert_eq!(result.manifests, vec![member], "{:#?}", result.manifests);
}

#[test]
fn a_broken_manifest_below_the_real_root_does_not_stop_the_walk() {
    // Distinguishing "corrupt" from "missing" must not change *which* root resolves. An
    // unrelated manifest that happens not to parse can sit between a member and its real
    // workspace root — Cargo reaches the root regardless, and a spargen that stopped short
    // would report `E023` for a layout that builds perfectly well.
    let directory = Sandbox::new();
    let broken_dir = directory.path().join("group");
    let member_dir = broken_dir.join("client");
    std::fs::create_dir(&broken_dir).unwrap();
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"group/client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    std::fs::write(broken_dir.join("Cargo.toml"), "[package\nnot toml at all\n").unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
}

/// One choice in `workspace_root` is left unguarded. The walk starts at
/// `absolute.parent().and_then(Utf8Path::parent)`, skipping the consumer's own directory.
/// Starting at `.parent()` instead re-reads that directory's `Cargo.toml`, which is the audited
/// manifest, already known to parse and to declare no `[workspace]`, so the walk climbs on and
/// the two starts agree. That holds **only when the audited manifest is named `Cargo.toml`**:
/// audit `dir/Other.toml` beside a workspace root at `dir/Cargo.toml` and the original skips the
/// root where the other start adopts it. Cargo sets `CARGO_MANIFEST_PATH` to a `Cargo.toml`, and
/// `manifest_from_env` and `generate_api!` otherwise join `Cargo.toml` onto
/// `CARGO_MANIFEST_DIR`, so no reachable input tells the two apart and no fixture pins the
/// choice.
#[test]
fn the_walk_climbs_past_an_ancestor_that_parses_and_declares_no_workspace() {
    // "A manifest that parses but declares no `[workspace]` is an ordinary member or an
    // unrelated crate: keep climbing." Nested workspaces and vendored crates make that layout
    // ordinary, and treating the first *parseable* ancestor as the root silently resolves every
    // inherited dependency against a table that is not there. The existing walk fixtures all
    // put an ancestor that fails to *parse* in the way, which is a different arm.
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let middle_dir = directory.path().join("middle");
    let member_dir = middle_dir.join("client");
    std::fs::create_dir_all(&member_dir).unwrap();
    let middle = Utf8PathBuf::from_path_buf(middle_dir.join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = []\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    // Parses, and declares no `[workspace]`: the walk must step over it, not stop on it.
    std::fs::write(
        &middle,
        "[package]\nname = \"middle\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(result.manifests.contains(&root), "{:#?}", result.manifests);
    assert!(
        !result.manifests.contains(&middle),
        "{:#?}",
        result.manifests
    );
}

/// What `workspace_root` returned, spelled so an assertion can compare it.
fn walked(root: WorkspaceRoot) -> String {
    match root {
        WorkspaceRoot::SelfRooted(path) => format!("self-rooted {path}"),
        WorkspaceRoot::Separate(path) => format!("separate {path}"),
        WorkspaceRoot::NotFound {
            unreadable: Some((path, _)),
            ..
        } => format!("not found, unreadable {path}"),
        WorkspaceRoot::NotFound {
            unreadable: None, ..
        } => "not found".to_owned(),
    }
}

#[test]
fn the_walk_reads_nothing_above_its_ceiling() {
    // #214: the ceiling is what keeps a fixture's outcome to the files it wrote. Above it sit
    // a workspace root that would resolve and, further up, a manifest that does not parse —
    // host files a fixture must never read — and a walk bounded below both must neither adopt
    // the first nor name the second.
    let directory = Sandbox::new();
    let sandbox = Utf8PathBuf::from_path_buf(directory.path().to_path_buf()).unwrap();
    let outer = sandbox.join("outer");
    let inner = outer.join("inner");
    let member_dir = inner.join("client");
    std::fs::create_dir_all(&member_dir).unwrap();
    std::fs::write(sandbox.join("Cargo.toml"), "[workspace\nthis is not toml\n").unwrap();
    std::fs::write(outer.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let member = member_dir.join("Cargo.toml");
    let contents =
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}");
    std::fs::write(&member, &contents).unwrap();
    let manifest: toml::Value = toml::from_str(&contents).unwrap();
    let walk = |ceiling: Option<&Utf8PathBuf>| {
        walked(workspace_root(
            &member,
            &manifest,
            ceiling.map(Utf8PathBuf::as_path),
        ))
    };

    // The ceiling directory itself is read, so a root exactly at it still resolves.
    assert_eq!(
        walk(Some(&outer)),
        format!("separate {}", outer.join("Cargo.toml"))
    );
    // Below the root, the walk ends at the ceiling: nothing found, and nothing unreadable met.
    assert_eq!(walk(Some(&inner)), "not found");
    assert_eq!(walk(Some(&member_dir)), "not found");
    // A ceiling the manifest does not lie within bounds the walk before its first read.
    assert_eq!(walk(Some(&sandbox.join("elsewhere"))), "not found");

    // With the root removed, the unparseable manifest is met only where the ceiling admits it.
    std::fs::remove_file(outer.join("Cargo.toml")).unwrap();
    assert_eq!(walk(Some(&outer)), "not found");
    assert_eq!(
        walk(Some(&sandbox)),
        format!("not found, unreadable {}", sandbox.join("Cargo.toml"))
    );
}

#[test]
fn the_production_walk_has_no_ceiling() {
    // The other half of the ceiling: `audit`, which every real build reaches, passes none, so
    // it still climbs as Cargo does. A root above any directory a fixture could name as a
    // ceiling is found only by an unbounded walk, and this is the one fixture that runs one.
    let directory = Sandbox::new();
    let sandbox = Utf8Path::from_path(directory.path()).unwrap();
    let member_dir = sandbox.join("a").join("b").join("client");
    std::fs::create_dir_all(&member_dir).unwrap();
    std::fs::write(sandbox.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let member = member_dir.join("Cargo.toml");
    let contents = "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n";
    std::fs::write(&member, contents).unwrap();
    let manifest: toml::Value = toml::from_str(contents).unwrap();

    assert_eq!(
        walked(workspace_root(&member, &manifest, None)),
        format!("separate {}", sandbox.join("Cargo.toml"))
    );
    // And the entry point real builds call is the unbounded one: its source forwards `None` as
    // the last argument. Whitespace is dropped so the check survives rustfmt's wrapping.
    const SOURCE: &str = include_str!("../mod.rs");
    let entry: String = SOURCE
        .split_once("pub(crate) fn audit(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .expect("`audit` is defined in runtime_contract/mod.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        entry.ends_with("&TargetContext::from_env(),None,)")
            || entry.ends_with("&TargetContext::from_env(),None)"),
        "`audit` must not bound a real build's walk: {entry}"
    );
}

#[test]
fn the_nearest_workspace_root_wins_when_two_are_on_the_walk() {
    // "The **nearest** ancestor manifest that parses and declares `[workspace]`" — the clause
    // the explain text states and this module's explain test pins as *text*. Nothing pinned it
    // as behaviour: every other walk fixture has at most one valid root on the path, the
    // obstacles being unparseable or workspace-less, never a second workspace root. Nested
    // workspaces are ordinary — a vendored tree, or a crate inside someone else's checkout —
    // and returning the farthest root instead resolves inherited dependencies against a table
    // belonging to an unrelated project.
    let directory = Sandbox::new();
    let far = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let near_dir = directory.path().join("near");
    let member_dir = near_dir.join("client");
    std::fs::create_dir_all(&member_dir).unwrap();
    let near = Utf8PathBuf::from_path_buf(near_dir.join("Cargo.toml")).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    // The far root declares the core crates; the near one declares none, so whichever is
    // chosen is visible in the diagnostics rather than only in `manifests`.
    std::fs::write(
        &far,
        format!(
            "[workspace]\nmembers = []\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    std::fs::write(&near, "[workspace]\nmembers = [\"client\"]\n").unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    let message = messages(&result.diagnostics);
    assert!(
        message.contains(near.as_str()) && message.contains("declares no `bytes` there"),
        "the walk must stop at the nearest workspace root: {message}"
    );
    assert!(!message.contains(far.as_str()), "{message}");
    assert!(result.manifests.contains(&near), "{:#?}", result.manifests);
    assert!(!result.manifests.contains(&far), "{:#?}", result.manifests);
}

#[test]
fn a_self_rooted_manifest_reached_by_a_relative_path_is_recorded_once() {
    // A self-rooted manifest is the consumer manifest, already read and already recorded, so
    // resolution must not read it a second time or record it again. Reached by an absolute
    // path the duplicate is invisible — `manifests` is sorted and deduplicated — so the guard
    // has to come in through a relative manifest path (`CARGO_MANIFEST_DIR=.`), where the
    // second spelling is a different string and Cargo would receive two `rerun-if-changed`
    // directives for one file.
    let directory = Sandbox::new();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[workspace]\n\n\
             [workspace.dependencies]\n{}\n{CORE_INHERITED}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();

    let _lock = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
    std::env::set_current_dir(directory.path()).unwrap();

    let result = directory.audit(Utf8Path::new("Cargo.toml"), &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(
        result.manifests,
        vec![Utf8PathBuf::from("Cargo.toml")],
        "the consumer manifest must be recorded once, under the spelling it was given"
    );
}
