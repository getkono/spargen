use super::*;

/// `format: date-time`/`date` lower to hand-written RFC 3339 newtypes, so `time` is required
/// with `formatting`/`parsing` and deliberately **without** `serde`: `time`'s own serde
/// representation is not RFC 3339 with or without `serde-human-readable`, and that mismatch
/// once shipped wrong bytes on the wire. Keeping the feature off is what makes the sequence
/// fallback unreachable, so the contract asserts it rather than only commenting on it.
#[test]
fn the_time_requirement_never_asks_for_serde() {
    let requirements = RuntimeRequirements {
        time: true,
        ..RuntimeRequirements::default()
    };
    let time = requirement_table(&requirements)
        .into_iter()
        .find(|requirement| requirement.dependency.name == "time")
        .expect("a time-using API requires the time crate");

    assert!(
        !time.features.contains(&"serde"),
        "time must not be required with serde: {:?}",
        time.features
    );
    assert!(time.features.contains(&"formatting"), "{:?}", time.features);
    assert!(time.features.contains(&"parsing"), "{:?}", time.features);
}

#[test]
fn exact_floors_and_higher_compatible_caret_requirements_are_supported() {
    for dependency in [
        BYTES,
        FUTURES_CORE,
        REQWEST,
        SECRECY,
        SERDE,
        SERDE_JSON,
        QUICK_XML,
        UUID,
        TIME,
        TOKIO,
    ] {
        assert!(supported_requirement(dependency.floor, dependency));
        let floor = dependency.floor_version();
        let higher = Version::new(floor.major, floor.minor, floor.patch + 1).to_string();
        assert!(supported_requirement(&higher, dependency));
        assert!(supported_requirement(&format!("^{higher}"), dependency));
        // The exact operator is part of the contract too, at the floor and above it on the same
        // line; below the floor, or at the next breaking release, it is refused like a caret.
        for exact in [dependency.floor, higher.as_str()] {
            assert!(
                supported_requirement(&format!("={exact}"), dependency),
                "`={exact}` for `{}`",
                dependency.name
            );
        }
        assert!(!supported_requirement(
            &format!("={}", version_below(&floor)),
            dependency
        ));
        assert!(!supported_requirement(
            &format!("={}", dependency.ceiling()),
            dependency
        ));
        // Other operators are refused even where they would admit the floor.
        assert!(!supported_requirement(
            &format!("~{}", dependency.floor),
            dependency
        ));
        assert!(!supported_requirement(
            &format!(">={}", dependency.floor),
            dependency
        ));
        assert!(!supported_requirement(
            &format!(">={}, <{}", dependency.floor, dependency.ceiling()),
            dependency
        ));
        assert!(!supported_requirement(
            &format!("^{}.0.0", dependency.ceiling_major),
            dependency
        ));
    }
}

/// The release just below `version` at its lowest non-zero component, with the components
/// after it zeroed: `1.12.1` gives `1.12.0`, `1.12.0` gives `1.11.0`, and `1.0.0` gives
/// `0.0.0`, so a floor that is an `x.y.0` or `x.0.0` release still has one.
fn version_below(version: &Version) -> Version {
    match (version.major, version.minor, version.patch) {
        (major, minor, patch @ 1..) => Version::new(major, minor, patch - 1),
        (major, minor @ 1.., 0) => Version::new(major, minor - 1, 0),
        (major @ 1.., 0, 0) => Version::new(major - 1, 0, 0),
        (0, 0, 0) => panic!("no release is below 0.0.0"),
    }
}

#[test]
fn version_below_steps_down_at_the_lowest_non_zero_component() {
    for (version, below) in [
        ("1.12.1", "1.12.0"),
        ("1.12.0", "1.11.0"),
        ("1.0.0", "0.0.0"),
        ("0.3.0", "0.2.0"),
    ] {
        assert_eq!(
            version_below(&Version::parse(version).unwrap()),
            Version::parse(below).unwrap(),
            "{version}"
        );
    }
}

#[test]
fn a_requirement_that_admits_a_version_below_the_floor_is_rejected() {
    // A version just below the contract's floor, in place of `CORE_MANIFEST`'s entry: both
    // are read rather than restated, so a floor bump in either place moves this fixture with
    // it, whether the new floor is a patch, minor, or major release.
    let (core_bytes, _) = core_entry("bytes");
    let floor = BYTES.floor_version();
    let below = version_below(&floor);
    assert!(below < floor, "{below} is not below {floor}");
    let manifest = replace_once(CORE_MANIFEST, core_bytes, &format!("bytes = \"{below}\""));
    let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, Code::RuntimeDependencyContract);
    assert!(diagnostics[0].message.contains("bytes"));
    assert!(diagnostics[0].message.contains(&format!("`{below}`")));
    assert!(diagnostics[0]
        .message
        .contains(&format!(">={}, <{}", BYTES.floor, BYTES.ceiling())));
}

#[test]
fn an_exact_requirement_at_the_floor_passes_the_audit() {
    // The audit-level counterpart of the `=floor` loop case: a manifest pinning a core crate
    // with `=` is accepted as written.
    let (core_bytes, floor) = core_entry("bytes");
    let manifest = replace_once(CORE_MANIFEST, core_bytes, &format!("bytes = \"={floor}\""));
    let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn a_runtime_crate_declared_without_a_version_requirement_is_rejected() {
    // A `path` or `git` declaration that states no `version` gives the audit nothing to hold to
    // the floor, so it says a requirement must be declared rather than calling the crate missing.
    let (core_bytes, _) = core_entry("bytes");
    for declaration in [
        "bytes = { path = \"../bytes\" }",
        "bytes = { git = \"https://github.com/tokio-rs/bytes\" }",
        "bytes = { features = [\"serde\"] }",
    ] {
        let manifest = replace_once(CORE_MANIFEST, core_bytes, declaration);
        let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
        assert_eq!(diagnostics.len(), 1, "{declaration}: {diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            format!(
                "`bytes` must declare a Cargo version requirement of `{}` or a higher compatible \
                 floor",
                BYTES.floor
            ),
            "{declaration}"
        );
    }
}

#[test]
fn a_runtime_crate_declared_only_as_a_dev_or_build_dependency_is_missing() {
    // Generated code is compiled as part of the package's library or binary, which neither
    // `[dev-dependencies]` nor `[build-dependencies]` reaches, so declaring a runtime crate only
    // there leaves it missing.
    let (core_bytes, _) = core_entry("bytes");
    for table in ["dev-dependencies", "build-dependencies"] {
        let manifest = format!(
            "{}\n[{table}]\n{core_bytes}\n",
            replace_once(CORE_MANIFEST, &format!("{core_bytes}\n"), "")
        );
        let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
        assert_eq!(diagnostics.len(), 1, "{table}: {diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            missing_message(BYTES),
            "{table}: {manifest}"
        );
    }
}

#[test]
fn conditional_dependencies_and_features_are_required_only_when_used() {
    assert!(audit_manifest(CORE_MANIFEST, RuntimeRequirements::default()).is_empty());

    let requirements = RuntimeRequirements {
        reqwest_json: true,
        reqwest_multipart: true,
        bytes_serde: true,
        streams: true,
        xml: true,
        uuid: true,
        time: true,
    };
    let diagnostics = audit_manifest(CORE_MANIFEST, requirements);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("feature `json` on `reqwest`"),
        "{messages}"
    );
    assert!(
        messages.contains("feature `multipart` on `reqwest`"),
        "{messages}"
    );
    assert!(
        messages.contains("feature `serde` on `bytes`"),
        "{messages}"
    );
    assert!(
        messages.contains("feature `stream` on `reqwest`"),
        "{messages}"
    );
    assert!(messages.contains("requires `futures-core`"), "{messages}");
    assert!(messages.contains("requires `quick-xml`"), "{messages}");
    assert!(messages.contains("requires `uuid`"), "{messages}");
    assert!(messages.contains("requires `time`"), "{messages}");
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract));
}

#[test]
fn reqwest_defaults_and_blocking_wiring_are_part_of_the_contract() {
    // The entry and its floor are read back out of `CORE_MANIFEST` rather than restated, so a
    // bump to the reqwest floor there changes this fixture with it.
    let (core_reqwest, floor) = core_entry("reqwest");
    let manifest = replace_once(
        CORE_MANIFEST,
        core_reqwest,
        &format!("reqwest = \"{floor}\"\n\n[features]\nblocking = []"),
    );
    let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("default-features = false"), "{messages}");
    assert!(messages.contains("requires `tokio`"), "{messages}");
    assert!(
        messages.contains("blocking = [\"dep:tokio\"]"),
        "{messages}"
    );
}

/// `CORE_MANIFEST` plus the blocking client's native-only optional `tokio` at its floor with
/// `rt`, and a `[features]` table whose `blocking` entry is `blocking`.
fn blocking_manifest(blocking: &str) -> String {
    format!(
        "{CORE_MANIFEST}\n[target.'cfg({BLOCKING_GATE})'.dependencies]\n\
         tokio = {{ version = \"{}\", features = [\"rt\"], optional = true }}\n\n\
         [features]\nblocking = {blocking}\n",
        TOKIO.floor
    )
}

#[test]
fn blocking_may_enable_tokio_through_its_implicit_feature_or_dep_syntax() {
    // Cargo accepts both spellings for enabling an optional dependency from a feature: the
    // implicit feature named after it (`"tokio"`) and `"dep:tokio"`. Either wires the blocking
    // client, alone or beside other entries.
    for blocking in [
        "[\"dep:tokio\"]",
        "[\"tokio\"]",
        "[\"serde_json/std\", \"tokio\"]",
    ] {
        let diagnostics =
            audit_manifest(&blocking_manifest(blocking), RuntimeRequirements::default());
        assert!(diagnostics.is_empty(), "{blocking}: {diagnostics:#?}");
    }
}

#[test]
fn blocking_that_enables_anything_but_tokio_is_not_wired() {
    // Neither a feature *of* tokio (`tokio/rt`) nor a near-miss name enables the optional
    // dependency itself, so each is reported exactly as an empty entry is.
    for blocking in [
        "[]",
        "[\"tokio/rt\"]",
        "[\"dep:tokio-util\"]",
        "[\"tokio?/rt\"]",
    ] {
        let diagnostics =
            audit_manifest(&blocking_manifest(blocking), RuntimeRequirements::default());
        assert_eq!(
            messages(&diagnostics),
            "Cargo feature `blocking` must enable the native optional dependency with \
             `blocking = [\"dep:tokio\"]`",
            "{blocking}"
        );
    }
}

/// `CORE_MANIFEST` with its `reqwest` entry declared by `reqwest` (a whole line) and its package
/// at `edition`, or at Cargo's default edition (2015) when that is `None`.
fn reqwest_declared(edition: Option<&str>, reqwest: &str) -> String {
    let (core_reqwest, _) = core_entry("reqwest");
    let manifest = replace_once(CORE_MANIFEST, core_reqwest, reqwest);
    match edition {
        Some(edition) => replace_once(
            &manifest,
            "version = \"0.0.0\"\n",
            &format!("version = \"0.0.0\"\nedition = \"{edition}\"\n"),
        ),
        None => manifest,
    }
}

#[test]
fn the_underscore_default_features_spelling_turns_defaults_off_before_edition_2024() {
    // Cargo 1.98 accepts `default_features` (deprecated) in a 2015, 2018, or 2021 package and
    // resolves it exactly as `default-features`, so a consumer that turned defaults off with it
    // satisfies the contract.
    let (_, floor) = core_entry("reqwest");
    for edition in [None, Some("2018"), Some("2021")] {
        let manifest = reqwest_declared(
            edition,
            &format!("reqwest = {{ version = \"{floor}\", default_features = false }}"),
        );
        let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
        assert!(diagnostics.is_empty(), "{edition:?}: {diagnostics:#?}");
    }
}

#[test]
fn the_underscore_default_features_spelling_is_not_read_from_edition_2024() {
    // Cargo 1.98 refuses the manifest instead: "`default_features` is unsupported as of the 2024
    // edition; instead use `default-features`". The audit agrees by not reading it, so the
    // declaration states no `default-features` and keeps its defaults.
    let (_, floor) = core_entry("reqwest");
    let manifest = reqwest_declared(
        Some("2024"),
        &format!("reqwest = {{ version = \"{floor}\", default_features = false }}"),
    );
    let diagnostics = audit_manifest(&manifest, RuntimeRequirements::default());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        diagnostics[0]
            .message
            .contains("`reqwest` must set `default-features = false`"),
        "{diagnostics:#?}"
    );
}

#[test]
fn the_hyphenated_default_features_spelling_wins_over_the_underscore_one() {
    // With both spellings in one declaration, Cargo 1.98 resolves the hyphenated key and ignores
    // the underscore one, in either direction.
    let (_, floor) = core_entry("reqwest");
    let on = reqwest_declared(
        Some("2021"),
        &format!(
            "reqwest = {{ version = \"{floor}\", default_features = false, default-features = true }}"
        ),
    );
    let diagnostics = audit_manifest(&on, RuntimeRequirements::default());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        diagnostics[0]
            .message
            .contains("`reqwest` must set `default-features = false`"),
        "{diagnostics:#?}"
    );

    let off = reqwest_declared(
        Some("2021"),
        &format!(
            "reqwest = {{ version = \"{floor}\", default_features = true, default-features = false }}"
        ),
    );
    let diagnostics = audit_manifest(&off, RuntimeRequirements::default());
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// Audits a root whose `[workspace.dependencies]` turns `reqwest`'s defaults off with the
/// underscore spelling `default_features = false`, beside a member whose `[package]` carries
/// `member_edition` (whole lines, possibly none) and which declares `reqwest` as
/// `member_reqwest`. `workspace_package` is the root's `[workspace.package]` body, or empty for
/// none. Returns every diagnostic message.
fn underscore_root_reqwest_diagnostics(
    workspace_package: &str,
    member_edition: &str,
    member_reqwest: &str,
) -> Vec<String> {
    let directory = Sandbox::new();
    let root = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    let (core_reqwest, floor) = core_entry("reqwest");
    let root_dependencies = replace_once(
        core_workspace_dependencies(),
        core_reqwest,
        &format!("reqwest = {{ version = \"{floor}\", default_features = false }}"),
    );
    let workspace_package = if workspace_package.is_empty() {
        String::new()
    } else {
        format!("[workspace.package]\n{workspace_package}\n")
    };
    std::fs::write(
        &root,
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n{workspace_package}\
             [workspace.dependencies]\n{root_dependencies}"
        ),
    )
    .unwrap();
    std::fs::write(
        &member,
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n{member_edition}\n{}",
            replace_once(CORE_INHERITED, "reqwest.workspace = true", member_reqwest)
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
fn a_root_underscore_default_features_false_turns_defaults_off_for_a_pre_2024_member() {
    // Cargo 1.98 resolves a `[workspace.dependencies]` entry's `default_features` as
    // `default-features` for a member below edition 2024, whether that member states its
    // edition, inherits it, or takes the 2015 default.
    for (workspace_package, member_edition) in [
        ("", ""),
        ("", "edition = \"2021\""),
        ("edition = \"2021\"", "edition.workspace = true"),
    ] {
        let messages = underscore_root_reqwest_diagnostics(
            workspace_package,
            member_edition,
            "reqwest.workspace = true",
        );
        assert!(messages.is_empty(), "{member_edition:?}: {messages:#?}");
    }
}

#[test]
fn a_root_underscore_default_features_is_not_read_for_an_edition_2024_member() {
    // The edition that decides is the inheriting member's: Cargo 1.98 refuses that member's
    // manifest ("`default_features` is unsupported as of the 2024 edition"), naming the
    // inherited dependency, even when the root is a virtual manifest with no edition of its own.
    for (workspace_package, member_edition) in [
        ("", "edition = \"2024\""),
        ("edition = \"2024\"", "edition.workspace = true"),
    ] {
        let messages = underscore_root_reqwest_diagnostics(
            workspace_package,
            member_edition,
            "reqwest.workspace = true",
        );
        assert_eq!(messages.len(), 1, "{member_edition:?}: {messages:#?}");
        assert!(
            messages[0].contains("`reqwest` must set `default-features = false`"),
            "{messages:#?}"
        );
    }
}

#[test]
fn a_member_underscore_default_features_true_turns_on_defaults_the_root_turned_off() {
    // The member's own flag is read under either spelling too, so a member re-enabling defaults
    // with `default_features = true` is caught as `default-features = true` is.
    let messages = underscore_root_reqwest_diagnostics(
        "",
        "edition = \"2021\"",
        "reqwest = { workspace = true, default_features = true }",
    );
    assert_eq!(messages.len(), 1, "{messages:#?}");
    assert!(
        messages[0].contains("`reqwest` must set `default-features = false`"),
        "{messages:#?}"
    );
}
