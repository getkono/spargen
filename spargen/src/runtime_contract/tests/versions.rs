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
