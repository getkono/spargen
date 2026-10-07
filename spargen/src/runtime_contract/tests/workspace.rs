use super::*;

#[test]
fn workspace_inheritance_uses_the_workspace_version_and_features() {
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            CORE_MANIFEST.split_once("[dependencies]\n").unwrap().1
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        r#"[package]
name = "consumer"
version = "0.0.0"

[dependencies]
bytes.workspace = true
reqwest.workspace = true
secrecy.workspace = true
serde.workspace = true
serde_json.workspace = true
"#,
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.manifests, vec![root, member]);
}

/// Whether the fixture's `[workspace.dependencies]` entry for `reqwest` leaves default features
/// on. That bit is the only thing these fixtures vary about the root, so it is the only thing
/// they state; the version floor comes from `CORE_MANIFEST` either way.
enum RootDefaults {
    On,
    Off,
}

/// Audits a root/member pair that differ from the core fixtures only in how `reqwest` is
/// declared, and returns every diagnostic message the audit produced.
///
/// The root entry is derived from `core_workspace_dependencies()` rather than written out at
/// the call sites. Passing the whole declaration in re-stated the `reqwest` floor three times
/// over, and a bump to that floor in `CORE_MANIFEST` would then stop the substitution matching
/// — the root would silently keep `default-features = false`, and the fixture that needs them
/// on would fail for a reason unrelated to what it names. Locating the entry by its key and
/// asserting it was found makes a rename of it fail loudly instead of quietly.
fn inherited_reqwest_default_feature_diagnostics(
    root_defaults: RootDefaults,
    member_reqwest: &str,
) -> Vec<String> {
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    let (core_reqwest, floor) = core_entry("reqwest");
    let root_dependencies = match root_defaults {
        // `CORE_MANIFEST` already disables them, so this is the core body unchanged.
        RootDefaults::Off => core_workspace_dependencies().to_owned(),
        RootDefaults::On => {
            core_workspace_dependencies().replace(core_reqwest, &format!("reqwest = \"{floor}\""))
        }
    };
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n\
             {root_dependencies}"
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{}",
            CORE_INHERITED.replace("reqwest.workspace = true", member_reqwest)
        ),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract),
        "{:#?}",
        result.diagnostics
    );
    result
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

#[test]
fn a_member_default_features_false_cannot_turn_off_defaults_the_root_leaves_on() {
    // Cargo's rule for inheritance: the workspace entry decides, and a member's `false` is
    // ignored when that entry leaves defaults on. The fix belongs in the root.
    let messages = inherited_reqwest_default_feature_diagnostics(
        RootDefaults::On,
        "reqwest = { workspace = true, default-features = false }",
    );
    assert_eq!(messages.len(), 1, "{messages:#?}");
    assert!(
        messages[0].contains("`reqwest` must set `default-features = false`"),
        "{messages:#?}"
    );
}

#[test]
fn a_member_default_features_true_turns_on_defaults_the_root_turned_off() {
    // The other direction: a member may re-enable defaults, so a root that already disables
    // them does not satisfy the audit on its own.
    let messages = inherited_reqwest_default_feature_diagnostics(
        RootDefaults::Off,
        "reqwest = { workspace = true, default-features = true }",
    );
    assert_eq!(messages.len(), 1, "{messages:#?}");
    assert!(
        messages[0].contains("`reqwest` must set `default-features = false`"),
        "{messages:#?}"
    );
}

#[test]
fn a_member_default_features_false_keeps_the_defaults_the_root_turned_off() {
    // The layout the E023 explain text advises: defaults disabled in the root, and the member
    // repeating `false`. Only a member `true` re-enables them, so any explicit member flag must
    // not count as one.
    let messages = inherited_reqwest_default_feature_diagnostics(
        RootDefaults::Off,
        "reqwest = { workspace = true, default-features = false }",
    );
    assert!(messages.is_empty(), "{messages:#?}");
}

#[test]
fn an_inherited_member_cannot_make_an_unconditional_crate_optional() {
    // The mirror of `workspace_inherited_tokio_under_an_alternative_spelling_resolves`:
    // `optional` is read from the member, so a member that adds `optional = true` to a crate
    // generated code names unconditionally must be rejected — the inheritance resolving is not
    // the same thing as the declaration being acceptable. Every other test that reaches this
    // rule declares its crate directly, so this is the one that holds it on the inheritance
    // path.
    let messages = inherited_reqwest_default_feature_diagnostics(
        RootDefaults::Off,
        "reqwest = { workspace = true, optional = true }",
    );
    assert_eq!(messages.len(), 1, "{messages:#?}");
    assert!(
        messages[0].contains("`reqwest` must not be optional"),
        "{messages:#?}"
    );
}

#[test]
fn a_root_package_inherits_its_own_workspace_dependencies() {
    // `[package]` and `[workspace]` in one file is the single-crate repository, and `workspace
    // = true` there resolves against the table directly below it. Resolution used to give up
    // the moment the consumer manifest declared `[workspace]` at all, so this layout reported
    // every inherited dependency as unresolvable — about a table spargen had already parsed.
    let directory = Sandbox::new();
    let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    std::fs::write(
        &manifest,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[workspace]\n\n\
             [workspace.dependencies]\n{}\n{CORE_INHERITED}",
            core_workspace_dependencies()
        ),
    )
    .unwrap();

    let result = directory.audit(&manifest, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    // Self-rooted: one manifest, not the same file reported twice.
    assert_eq!(result.manifests, vec![manifest]);
}

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

/// The working directory is process-global, so the one test that has to change it restores it
/// on the way out — including on a panic — and holds a lock so a second such test cannot race
/// it.
static WORKING_DIRECTORY: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct RestoreWorkingDirectory(std::path::PathBuf);

impl Drop for RestoreWorkingDirectory {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
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
    // search runs from the absolutized path, and the message must name that one; reporting
    // the caller's spelling instead left every test green (#202).
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
    // Absolutization is lexical: `std::path::absolute`, not `canonicalize`. Swapping one for
    // the other left every test green (#202), and they disagree exactly here — a member reached
    // through a symlinked directory. Cargo climbs the path it was given, which is the path it
    // hands the build as `CARGO_MANIFEST_DIR`, so the root is the one above the link, not the
    // one above where the link points.
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
    for dependency in ["bytes", "reqwest", "secrecy", "serde", "serde_json"] {
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

/// One choice in `workspace_root` is left unguarded, and the reason has a precondition that an
/// earlier version of this comment stated as though it were unconditional. The walk starts at
/// `absolute.parent().and_then(Utf8Path::parent)`, skipping the consumer's own directory.
/// Starting at `.parent()` instead re-reads the consumer's *directory*, whose `Cargo.toml` is
/// already known to parse and already known to declare no `[workspace]` — `workspace_root`
/// returns early when it does — so it falls through the `Ok(_) => {}` arm and climbs on.
///
/// That holds **only when the audited manifest is itself named `Cargo.toml`**. Point the audit
/// at `dir/Other.toml` with a real workspace root beside it at `dir/Cargo.toml` and the two
/// differ sharply: the original skips the sibling and reports every inherited dependency
/// unresolvable, the mutant adopts it and reports nothing. So the mutant is equivalent **under
/// that precondition** and distinguishable without it.
///
/// The precondition is not enforced anywhere: `manifest_from_env` and `generate_api!` both pass
/// `CARGO_MANIFEST_PATH` on verbatim with no filename check, and otherwise join `Cargo.toml`
/// onto `CARGO_MANIFEST_DIR`. It holds because Cargo sets that variable to a `Cargo.toml`,
/// which is why this is a comment rather than a fixture — there is no reachable input that
/// distinguishes the two.
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
    // the two host files that used to turn these fixtures red — and a walk bounded below both
    // must neither adopt the first nor name the second.
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

/// Audits a member that inherits the core crates from a workspace root whose `bytes` entry
/// has been rewritten to carry `package = "<package>"`, and returns the diagnostics.
///
/// Shared by the rename and identity fixtures below, which differ only in that one string;
/// `inherited_reqwest_default_feature_diagnostics` above collapses the same pattern.
fn workspace_root_bytes_package_diagnostics(package: &str) -> Vec<Diagnostic> {
    let core_bytes = core_workspace_dependencies()
        .lines()
        .find(|line| line.starts_with("bytes = "))
        .expect("CORE_MANIFEST declares bytes under that key");
    let floor = core_bytes
        .split('"')
        .nth(1)
        .expect("the bytes entry pins a quoted version");

    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies().replace(
                core_bytes,
                &format!("bytes = {{ package = \"{package}\", version = \"{floor}\" }}")
            )
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    directory
        .audit(&member, &RuntimeRequirements::default())
        .diagnostics
}

#[test]
fn a_runtime_crate_renamed_in_the_workspace_root_is_rejected() {
    // "A renamed runtime crate" is an advertised `E023` trigger, and for an inheriting member
    // the check reads `package` from the root, where Cargo reads it (a `package` on the
    // member's own `workspace = true` line is an unused key Cargo ignores, #317). Every other
    // rename fixture renames in the member's own table, so this is the root half.
    // `bytes-fork` is an actual rename: the key `bytes` would bind a different package.
    let diagnostics = workspace_root_bytes_package_diagnostics("bytes-fork");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        diagnostics[0].message.contains("`bytes` cannot be renamed"),
        "{diagnostics:#?}"
    );
}

#[test]
fn an_identity_package_key_in_the_workspace_root_is_accepted() {
    // `bytes = { package = "bytes", … }` renames nothing: Cargo's `package` field defaults to
    // the key, so this is the fully-qualified spelling of an ordinary dependency, and `cargo
    // check` compiles `bytes::Bytes` against it. The rule once tested the key's *presence*
    // rather than its value and refused this with a hard `E023` (#168).
    let diagnostics = workspace_root_bytes_package_diagnostics("bytes");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
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
fn a_root_declaring_optional_does_not_make_an_inherited_crate_optional() {
    // The `package` rule reads both the member's and the root's declaration. The `optional`
    // rule reads the member only, and this test is what holds it there: making it read the
    // root as well leaves every other test green.
    //
    // Like `a_self_declared_workspace_wins_over_package_workspace`, this pins spargen's answer
    // to a manifest **Cargo will not load** — `optional` is not an accepted key in
    // `[workspace.dependencies]`, which is *why* reading it from the root would be wrong. So it
    // guards against a silent change to a rule rather than describing a reachable layout, and
    // that is the whole of its value.
    let core_reqwest = core_workspace_dependencies()
        .lines()
        .find(|line| line.starts_with("reqwest = "))
        .expect("CORE_MANIFEST declares reqwest under that key");
    let optional_in_the_root = core_reqwest.replace(" }", ", optional = true }");
    assert_ne!(
        optional_in_the_root, core_reqwest,
        "the entry is an inline table"
    );

    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}",
            core_workspace_dependencies().replace(core_reqwest, &optional_in_the_root)
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{CORE_INHERITED}"),
    )
    .unwrap();

    let result = directory.audit(&member, &RuntimeRequirements::default());
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
}

#[test]
fn a_root_declaring_optional_does_not_satisfy_the_rule_that_tokio_be_optional() {
    // The other half of the fixture above. `optional` is read from the member in both
    // directions, and that fixture holds only the forbidden one: reading the root as well on
    // the required side — so a root `optional = true` excuses a member that leaves it out —
    // left every test green (#202). Cargo rejects `optional` in `[workspace.dependencies]`,
    // so this, too, pins a rule rather than a layout Cargo loads.
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}\
             tokio = {{ version = \"1.53.1\", features = [\"rt\"], optional = true }}\n",
            core_workspace_dependencies()
        ),
    )
    .unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[features]\n\
             blocking = [\"dep:tokio\"]\n\n{CORE_INHERITED}\n\
             [target.'cfg(not(target_arch=\"wasm32\"))'.dependencies]\n\
             tokio = {{ workspace = true }}\n"
        ),
    )
    .unwrap();
    for target in [TargetContext::Unknown, linux()] {
        let result = directory.audit_in(&member, &RuntimeRequirements::default(), &target);
        assert_eq!(
            messages(&result.diagnostics),
            "`tokio` must be optional because it is enabled only by the generated `blocking` \
             feature"
        );
    }
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

#[test]
fn an_unresolvable_inheritance_says_where_the_lookup_went() {
    // One message used to cover two opposite situations: the workspace has no such entry (fix
    // the root), and no workspace was found at all (fix the layout, or spell the version out).
    // The report behind #71 landed in exactly that ambiguity.
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
    for dependency in ["bytes", "reqwest", "secrecy", "serde", "serde_json"] {
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

    // A workspace that resolves, but declares four of the five.
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

/// The path a diagnostic spells in backticks directly after `prefix`.
///
/// The messages these fixtures read render the manifest as ``<noun> `<path>` ``, so lifting
/// the path out of each is what lets it be compared to the file the fixture itself built.
/// Asking whether one message `contains` the other's path cannot pin that: `contains` is a
/// substring relation and every string contains the empty one, so a renderer that emitted no
/// path at all would satisfy it.
fn manifest_named_after<'m>(message: &'m str, prefix: &str) -> Option<&'m str> {
    message
        .split_once(prefix)
        .and_then(|(_, rest)| rest.split_once('`'))
        .map(|(path, _)| path)
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
    // each other *and* to the path this fixture itself builds. Requiring the read failure to
    // merely `contain` the inheritance message's path left the claim open: `contains` is a
    // substring relation, so an arm rendering an empty path, a bare `Cargo.toml`, or the root
    // *directory* satisfied it. Agreement alone is no better: two renderers degrading the same
    // way still agree, and so does one naming a manifest nothing ever opened. The expected
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
    // separator of any kind appended here falsifies it; `!contains("could not be read:")` forbade one
    // spelling and left the rest — an em dash among them — free. Pinning the end of the
    // message forbids all of them. `location` is empty here: one counting table.
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
