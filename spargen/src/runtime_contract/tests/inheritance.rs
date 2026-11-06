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
http.workspace = true
http-body.workspace = true
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

#[test]
fn a_member_inheriting_plain_and_table_dependencies_with_an_added_feature_passes() {
    // The layout exactly as #71 reported it: the root spells `futures-core` as a plain string
    // and `uuid` as a table with its own features; the member inherits both beside the seven
    // core crates. The report said both came back as "generated client requires …". The
    // member adds `stream` to the inherited `reqwest`, which is the feature union a stream
    // needs and the half of it no other fixture exercises.
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}\
             futures-core = \"{}\"\n\
             uuid = {{ version = \"1.26.0\", features = [\"v4\", \"serde\"] }}\n",
            core_workspace_dependencies(),
            FUTURES_CORE.floor
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{}\
             futures-core = {{ workspace = true }}\nuuid = {{ workspace = true }}\n",
            CORE_INHERITED.replace(
                "reqwest.workspace = true",
                "reqwest = { workspace = true, features = [\"stream\"] }"
            )
        ),
    )
    .unwrap();

    let requirements = RuntimeRequirements {
        streams: true,
        uuid: true,
        ..RuntimeRequirements::default()
    };
    let result = directory.audit(&member, &requirements);
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.manifests, vec![root, member]);
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
    // = true` there resolves against the table directly below it, so a consumer manifest that
    // declares `[workspace]` itself resolves its inherited dependencies rather than reporting
    // them unresolvable.
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
    // directions, and that fixture holds only the forbidden one; this one holds the required
    // side, where a root `optional = true` must not excuse a member that leaves it out (#202).
    // Cargo rejects `optional` in `[workspace.dependencies]`,
    // so this, too, pins a rule rather than a layout Cargo loads.
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}\
             tokio = {{ version = \"{}\", features = [\"rt\"], optional = true }}\n",
            core_workspace_dependencies(),
            TOKIO.floor
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
