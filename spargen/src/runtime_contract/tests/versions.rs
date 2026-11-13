use super::*;

/// `format: date-time`/`date` lower to hand-written RFC 3339 newtypes, so `time` is required
/// with `formatting`/`parsing` and deliberately **without** `serde`: `time`'s own serde
/// representation is not RFC 3339 with or without `serde-human-readable`, and that mismatch
/// once shipped wrong bytes on the wire. Keeping the feature off is what makes the sequence
/// fallback unreachable, so the contract asserts it rather than only commenting on it.
#[test]
fn the_time_requirement_never_asks_for_serde() {
    let requirements = RuntimeRequirements {
        datetime: Some(DateTimeBackend::Time),
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

/// Each backend requires exactly its own crate (`jiff` and `chrono` with no feature, since the
/// embedded codec writes the text) and no other date-time crate; `String` and an API without dates
/// require none.
#[test]
fn each_datetime_backend_requires_exactly_its_own_crate() {
    let crates = ["time", "jiff", "chrono"];
    let cases = [
        (Some(DateTimeBackend::Time), Some("time")),
        (Some(DateTimeBackend::Jiff), Some("jiff")),
        (Some(DateTimeBackend::Chrono), Some("chrono")),
        (Some(DateTimeBackend::String), None),
        (None, None),
    ];
    for (datetime, expected) in cases {
        let requirements = RuntimeRequirements {
            datetime,
            ..RuntimeRequirements::default()
        };
        let required: Vec<Requirement> = requirement_table(&requirements)
            .into_iter()
            .filter(|requirement| crates.contains(&requirement.dependency.name))
            .collect();
        assert_eq!(
            required
                .iter()
                .map(|requirement| requirement.dependency.name)
                .collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>(),
            "{datetime:?}"
        );
        for requirement in required
            .iter()
            .filter(|requirement| requirement.dependency.name != "time")
        {
            assert!(requirement.features.is_empty(), "{datetime:?}");
            assert!(!requirement.no_default_features, "{datetime:?}");
        }
    }
}

#[test]
fn exact_floors_and_higher_compatible_caret_requirements_are_supported() {
    for dependency in [
        BYTES,
        FUTURES_CORE,
        HTTP,
        HTTP_BODY,
        REQWEST,
        SECRECY,
        SERDE,
        SERDE_JSON,
        QUICK_XML,
        UUID,
        TIME,
        JIFF,
        CHRONO,
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
pub(super) fn version_below(version: &Version) -> Version {
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
    let diagnostics = audit_manifest(
        &manifest,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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
    let diagnostics = audit_manifest(
        &manifest,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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
        let diagnostics = audit_manifest(
            &manifest,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
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
        let diagnostics = audit_manifest(
            &manifest,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
        assert_eq!(diagnostics.len(), 1, "{table}: {diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            missing_message(BYTES),
            "{table}: {manifest}"
        );
    }
}

/// Issue #1305: an API whose only streaming construct is a binary `_stream` twin reads its
/// `ByteStream` chunk by chunk on `wasm32`, so it requires `futures-core` and reqwest's `stream`
/// feature, as an API with a sequential response does.
#[test]
fn a_binary_stream_twin_alone_requires_the_chunked_read_dependencies() {
    let sandbox = Sandbox::new();
    let path = sandbox.path().join("openapi.yaml");
    std::fs::write(
        &path,
        "openapi: 3.1.0\ninfo: { title: Blobs, version: 1.0.0 }\npaths:\n  /blob:\n    get:\n      \
         operationId: getBlob\n      responses:\n        \"200\":\n          description: OK\n          \
         content:\n            application/octet-stream: {}\n",
    )
    .unwrap();
    let spec = crate::Spec::new(camino::Utf8PathBuf::from_path_buf(path).unwrap());
    let (requirements, _) = crate::requirements(&spec).expect("the spec lowers");
    let dependency = |name: &str| {
        requirements
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
    };
    assert!(dependency("futures-core").is_some(), "{requirements}");
    assert_eq!(
        dependency("reqwest").map(|reqwest| reqwest.features.clone()),
        Some(vec!["stream"]),
        "{requirements}"
    );
}

#[test]
fn conditional_dependencies_and_features_are_required_only_when_used() {
    assert!(audit_manifest(
        CORE_MANIFEST,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown
    )
    .is_empty());

    let requirements = RuntimeRequirements {
        bytes_serde: true,
        streams: true,
        xml: true,
        uuid: true,
        datetime: Some(DateTimeBackend::Time),
    };
    let diagnostics = audit_manifest(CORE_MANIFEST, &requirements, &TargetContext::Unknown);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!messages.contains("feature `multipart`"), "{messages}");
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
    let diagnostics = audit_manifest(
        &manifest,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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
    // Cargo enables an optional dependency from a feature through three spellings: the implicit
    // feature named after it (`"tokio"`), `"dep:tokio"`, and a non-weak dependency feature
    // (`"tokio/rt"`, which enables tokio as well as its `rt`). Each wires the blocking client,
    // alone or beside other entries.
    for blocking in [
        "[\"dep:tokio\"]",
        "[\"tokio\"]",
        "[\"serde_json/std\", \"tokio\"]",
        "[\"tokio/rt\"]",
        "[\"serde_json/std\", \"tokio/rt-multi-thread\"]",
    ] {
        let diagnostics = audit_manifest(
            &blocking_manifest(blocking),
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
        assert!(diagnostics.is_empty(), "{blocking}: {diagnostics:#?}");
    }
}

#[test]
fn blocking_that_enables_anything_but_tokio_is_not_wired() {
    // Neither a weak dependency feature (`tokio?/rt`, which enables `rt` only if something else
    // already enables tokio) nor a near-miss name, as a dependency or as a dependency feature,
    // enables the optional dependency itself, so each is reported exactly as an empty entry is.
    for blocking in [
        "[]",
        "[\"dep:tokio-util\"]",
        "[\"tokio-util/rt\"]",
        "[\"tokio?/rt\"]",
    ] {
        let diagnostics = audit_manifest(
            &blocking_manifest(blocking),
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
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
        let diagnostics = audit_manifest(
            &manifest,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
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
    let diagnostics = audit_manifest(
        &manifest,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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
    let diagnostics = audit_manifest(
        &on,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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
    let diagnostics = audit_manifest(
        &off,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
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

/// `RuntimeRequirements::for_api` asks for reqwest's multipart support exactly when a request
/// body is `multipart/form-data`, and for `uuid` exactly when the mapping is on and the API emits
/// a `format: uuid` type.
#[test]
fn the_requirements_follow_the_multipart_bodies_and_uuid_types_the_api_carries() {
    const MULTIPART_AND_UUID: &str = "openapi: 3.1.0
info: {title: T, version: '1'}
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema: {type: object, properties: {name: {type: string}}}
      responses: {'204': {description: done}}
components:
  schemas:
    Id: {type: string, format: uuid}
";
    const JSON_ONLY: &str = "openapi: 3.1.0
info: {title: T, version: '1'}
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          application/json:
            schema: {type: object, properties: {name: {type: string}}}
      responses: {'204': {description: done}}
";
    let sandbox = Sandbox::new();
    let requirements = |source: &str, uuid: bool| {
        let path = sandbox.root.join("openapi.yaml");
        std::fs::write(&path, source).unwrap();
        let spec = crate::Spec::new(path).with_uuid(crate::UuidMapping::from_on(uuid));
        let mut diagnostics = crate::diag::Diagnostics::default();
        let (api, _) = crate::pipeline::lower_frontend(&spec, &mut diagnostics)
            .unwrap_or_else(|()| panic!("the spec lowers: {:?}", diagnostics.items()));
        RuntimeRequirements::for_api(&api, &spec)
    };
    for (case, source, uuid, expected) in [
        (
            "a multipart body and a uuid type, mapping on",
            MULTIPART_AND_UUID,
            true,
            true,
        ),
        (
            "a multipart body and a uuid type, mapping off",
            MULTIPART_AND_UUID,
            false,
            false,
        ),
        (
            "a JSON body and no uuid type, mapping on",
            JSON_ONLY,
            true,
            false,
        ),
    ] {
        let requirements = requirements(source, uuid);
        assert_eq!(requirements.uuid, expected, "{case}");
        // Issue #1287: the runtime encodes a multipart body itself, so it requires nothing a
        // JSON body does not.
        assert_eq!(
            RuntimeRequirements {
                uuid: false,
                ..requirements
            },
            RuntimeRequirements::default(),
            "{case}"
        );
    }
}

/// `CORE_MANIFEST` with `publish` set as `publish` gives (left unset for `None`), its `bytes`
/// entry declared as `bytes`, and, when `lock` is given, that `Cargo.lock` beside it: a package
/// that belongs to no workspace, which Cargo resolves with the lockfile in its own directory.
/// Returns the audit and the lockfile's path.
fn standalone_audit(
    publish: Option<&str>,
    bytes: &str,
    lock: Option<&str>,
) -> (Audit, Utf8PathBuf) {
    standalone_audit_under("", publish, bytes, lock)
}

/// [`standalone_audit`] with the package in `directory`, relative to its sandbox, whose Cargo
/// home directory is `cargo-home`.
fn standalone_audit_under(
    directory: &str,
    publish: Option<&str>,
    bytes: &str,
    lock: Option<&str>,
) -> (Audit, Utf8PathBuf) {
    standalone_audit_under_with(directory, publish, bytes, lock, |_| {})
}

/// [`standalone_audit_under`], with `prepare` run on the package directory before the audit, to
/// place whatever else sits beside the manifest.
fn standalone_audit_under_with(
    directory: &str,
    publish: Option<&str>,
    bytes: &str,
    lock: Option<&str>,
    prepare: impl FnOnce(&Utf8Path),
) -> (Audit, Utf8PathBuf) {
    let (core_bytes, _) = core_entry("bytes");
    let mut manifest = replace_once(CORE_MANIFEST, core_bytes, bytes);
    if let Some(publish) = publish {
        manifest = replace_once(
            &manifest,
            "version = \"0.0.0\"\n",
            &format!("version = \"0.0.0\"\npublish = {publish}\n"),
        );
    }
    let sandbox = Sandbox::new();
    let package = sandbox.root.join(directory);
    std::fs::create_dir_all(&package).unwrap();
    let path = package.join("Cargo.toml");
    std::fs::write(&path, manifest).unwrap();
    let lockfile = package.join("Cargo.lock");
    if let Some(lock) = lock {
        std::fs::write(&lockfile, lock).unwrap();
    }
    prepare(&package);
    let audit = sandbox.audit_in(
        &path,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
    (audit, lockfile)
}

/// The release just above `BYTES`'s floor on its line, a version a broad requirement may lock.
fn bytes_above_floor() -> String {
    let floor = BYTES.floor_version();
    Version::new(floor.major, floor.minor, floor.patch + 1).to_string()
}

#[test]
fn an_unpublished_package_is_held_to_the_version_its_lockfile_resolves() {
    // `bytes = "1"` admits releases below the floor, so the requirement alone is refused; the
    // lockfile resolves it above the floor, and that is what an unpublished package builds with.
    // Both spellings of "never publish" count, and the lockfile is named to Cargo for rebuilds.
    let above = bytes_above_floor();
    let lock = core_lock("consumer", &[("bytes", &above)], "");
    for publish in ["false", "[]"] {
        let (audit, lockfile) = standalone_audit(Some(publish), "bytes = \"1\"", Some(&lock));
        assert!(
            audit.diagnostics.is_empty(),
            "{publish}: {:#?}",
            audit.diagnostics
        );
        assert!(
            audit.manifests.contains(&lockfile),
            "{publish}: {:?}",
            audit.manifests
        );
    }
}

#[test]
fn a_locked_version_outside_the_supported_range_names_the_lockfile_and_the_floor() {
    let below = version_below(&BYTES.floor_version()).to_string();
    let above_ceiling = BYTES.ceiling().to_string();
    for (requirement, locked) in [("1", below.as_str()), (">=1", above_ceiling.as_str())] {
        let lock = core_lock("consumer", &[("bytes", locked)], "");
        let (audit, lockfile) = standalone_audit(
            Some("false"),
            &format!("bytes = \"{requirement}\""),
            Some(&lock),
        );
        assert_eq!(
            messages(&audit.diagnostics),
            format!(
                "`bytes` is locked at `{locked}` in `{lockfile}`, outside the supported range \
                 >={floor}, <{ceiling}; replace the requirement `{requirement}` with `{floor}` or \
                 a higher compatible caret requirement; the locked version was checked, not the \
                 requirement, because the package sets `publish = false` and `{lockfile}` \
                 resolves its dependencies",
                floor = BYTES.floor,
                ceiling = BYTES.ceiling(),
            )
        );
    }
}

#[test]
fn the_locked_version_checked_is_the_one_the_requirement_selects() {
    // With two versions of a crate locked, Cargo writes each dependency as `name version`, and
    // the one the package's requirement matches is the one it builds with: an older major
    // locked beside it is not the one the requirement selects.
    let above = bytes_above_floor();
    let lock = replace_once(
        &core_lock(
            "consumer",
            &[("bytes", &above)],
            "\n[[package]]\nname = \"bytes\"\nversion = \"0.4.12\"\nsource = \
             \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        ),
        " \"bytes\",\n",
        &format!(" \"bytes 0.4.12\",\n \"bytes {above}\",\n"),
    );
    let (audit, _) = standalone_audit(Some("false"), "bytes = \"1\"", Some(&lock));
    assert!(audit.diagnostics.is_empty(), "{:#?}", audit.diagnostics);
    // And the older one, selected, is the one held to the floor.
    let (audit, _) = standalone_audit(Some("false"), "bytes = \"0.4\"", Some(&lock));
    assert_eq!(audit.diagnostics.len(), 1, "{:#?}", audit.diagnostics);
    assert!(
        audit.diagnostics[0]
            .message
            .starts_with("`bytes` is locked at `0.4.12` in "),
        "{:#?}",
        audit.diagnostics
    );
}

#[test]
fn a_publishable_package_has_its_requirement_checked_beside_a_lockfile() {
    // The lockfile would pass it, but a published package's users never read that lockfile, so
    // it is neither checked nor named to Cargo.
    let lock = core_lock("consumer", &[("bytes", &bytes_above_floor())], "");
    for publish in [None, Some("true"), Some("[\"internal\"]")] {
        let (audit, lockfile) = standalone_audit(publish, "bytes = \"1\"", Some(&lock));
        assert_eq!(
            messages(&audit.diagnostics),
            format!(
                "`bytes` version requirement `1` is outside the supported range >={floor}, \
                 <{ceiling}; use `{floor}` or a higher compatible caret requirement; the \
                 requirement was checked, not a lockfile, because {PUBLISHABLE}",
                floor = BYTES.floor,
                ceiling = BYTES.ceiling(),
            ),
            "{publish:?}"
        );
        assert!(!audit.manifests.contains(&lockfile), "{publish:?}");
    }
}

#[test]
fn an_unpublished_package_without_a_usable_lock_has_its_requirement_checked() {
    // Each way the lockfile can fail to decide the version says which, after the floor message.
    let floor_message = format!(
        "`bytes` version requirement `1` is outside the supported range >={floor}, <{ceiling}; \
         use `{floor}` or a higher compatible caret requirement; the requirement was checked, \
         not a lockfile, because ",
        floor = BYTES.floor,
        ceiling = BYTES.ceiling(),
    );

    let (audit, lockfile) = standalone_audit(Some("false"), "bytes = \"1\"", None);
    assert_eq!(
        messages(&audit.diagnostics),
        format!("{floor_message}no `Cargo.lock` exists at `{lockfile}`")
    );
    assert!(!audit.manifests.contains(&lockfile));

    let other = core_lock("another", &[], "");
    let (audit, lockfile) = standalone_audit(Some("false"), "bytes = \"1\"", Some(&other));
    assert_eq!(
        messages(&audit.diagnostics),
        format!(
            "{floor_message}`{lockfile}` could not be used: it locks no workspace package named \
             `consumer`"
        )
    );
    assert!(audit.manifests.contains(&lockfile));

    let (audit, lockfile) = standalone_audit(
        Some("false"),
        "bytes = \"1\"",
        Some("version = [unterminated"),
    );
    let rendered = messages(&audit.diagnostics);
    assert!(
        rendered.starts_with(&format!("{floor_message}`{lockfile}` could not be used: ")),
        "{rendered}"
    );
    assert!(audit.manifests.contains(&lockfile));

    // A lockfile that locks the package, but no `bytes` the requirement matches.
    let unlocked = replace_once(&core_lock("consumer", &[], ""), " \"bytes\",\n", "");
    let (audit, lockfile) = standalone_audit(Some("false"), "bytes = \"1\"", Some(&unlocked));
    assert_eq!(
        messages(&audit.diagnostics),
        format!("{floor_message}`{lockfile}` locks no `bytes` matching `1` for this package")
    );
}

#[test]
fn an_unpublished_package_in_cargos_source_cache_has_its_requirement_checked() {
    // A `publish = false` package built as a git dependency of another project sits in a checkout
    // under Cargo's home, with its own repository's `Cargo.lock` beside it; Cargo resolves it
    // with the dependent's lockfile, so that one decides nothing and the requirement is checked.
    // The lock here would pass `bytes = "1"`, so only the requirement check refuses it.
    let lock = core_lock("consumer", &[("bytes", &bytes_above_floor())], "");
    for cache in ["git/checkouts", "registry/src"] {
        let (audit, lockfile) = standalone_audit_under(
            &format!("cargo-home/{cache}/consumer-0123456789abcdef/0a1b2c3"),
            Some("false"),
            "bytes = \"1\"",
            Some(&lock),
        );
        let cache = lockfile
            .ancestors()
            .nth(3)
            .expect("the lockfile sits three levels under the cache");
        assert_eq!(
            messages(&audit.diagnostics),
            format!(
                "`bytes` version requirement `1` is outside the supported range >={floor}, \
                 <{ceiling}; use `{floor}` or a higher compatible caret requirement; the \
                 requirement was checked, not a lockfile, because the package is built from \
                 Cargo's source cache `{cache}`, as a dependency of another workspace whose \
                 `Cargo.lock` resolves it",
                floor = BYTES.floor,
                ceiling = BYTES.ceiling(),
            ),
        );
        assert!(
            !audit.manifests.contains(&lockfile),
            "{:?}",
            audit.manifests
        );
    }
    // The cache is matched by whole path components: a directory whose name only starts with the
    // same spelling is an ordinary checkout, and its lockfile is read.
    let (audit, lockfile) = standalone_audit_under(
        "cargo-home/git/checkouts-old/consumer",
        Some("false"),
        "bytes = \"1\"",
        Some(&lock),
    );
    assert!(audit.diagnostics.is_empty(), "{:#?}", audit.diagnostics);
    assert!(audit.manifests.contains(&lockfile), "{:?}", audit.manifests);
}

#[test]
fn an_unpublished_package_in_a_directory_source_has_its_requirement_checked() {
    // #1242: a `publish = false` package vendored by `cargo vendor` (or in any other directory
    // source) lies outside Cargo's home, beside the `.cargo-checksum.json` Cargo requires of it,
    // and is built with the dependent workspace's lockfile. Its own `Cargo.lock` would pass
    // `bytes = "1"`, so only the requirement check refuses it.
    let lock = core_lock("consumer", &[("bytes", &bytes_above_floor())], "");
    let (audit, lockfile) = standalone_audit_under_with(
        "vendor/consumer",
        Some("false"),
        "bytes = \"1\"",
        Some(&lock),
        |package| {
            std::fs::write(
                package.join(".cargo-checksum.json"),
                "{\"files\":{},\"package\":null}",
            )
            .unwrap();
        },
    );
    let package = lockfile.parent().expect("the lockfile sits in the package");
    assert_eq!(
        messages(&audit.diagnostics),
        format!(
            "`bytes` version requirement `1` is outside the supported range >={floor}, \
             <{ceiling}; use `{floor}` or a higher compatible caret requirement; the \
             requirement was checked, not a lockfile, because the package is built from the \
             directory source `{package}` its `.cargo-checksum.json` marks, as a dependency of \
             another workspace whose `Cargo.lock` resolves it",
            floor = BYTES.floor,
            ceiling = BYTES.ceiling(),
        ),
    );
    assert!(
        !audit.manifests.contains(&lockfile),
        "{:?}",
        audit.manifests
    );

    // The marker is a file: a directory of that name marks nothing, and the lockfile is read.
    let (audit, lockfile) = standalone_audit_under_with(
        "vendor/consumer",
        Some("false"),
        "bytes = \"1\"",
        Some(&lock),
        |package| std::fs::create_dir(package.join(".cargo-checksum.json")).unwrap(),
    );
    assert!(audit.diagnostics.is_empty(), "{:#?}", audit.diagnostics);
    assert!(audit.manifests.contains(&lockfile), "{:?}", audit.manifests);
}
