use super::*;

/// `tokio` declared at its floor, `fields` following the version inside the inline table.
fn tokio_at_floor(fields: &str) -> String {
    format!("tokio = {{ version = \"{}\"{fields} }}", TOKIO.floor)
}

/// The declaration the blocking client requires: `tokio` at its floor, with `rt`, optional.
fn tokio_declaration() -> String {
    tokio_at_floor(", features = [\"rt\"], optional = true")
}

/// The core manifest opted into `blocking`, followed by `tables` verbatim.
fn blocking_manifest(tables: &str) -> String {
    format!("{CORE_MANIFEST}\n[features]\nblocking = [\"dep:tokio\"]\n\n{tables}")
}

/// `[target.'<key>'.dependencies]` declaring `tokio` as `declaration`.
fn tokio_table(key: &str, declaration: &str) -> String {
    format!("[target.'{key}'.dependencies]\n{declaration}\n\n")
}

fn windows() -> TargetContext {
    build_target(
        "x86_64-pc-windows-msvc",
        &[
            ("TARGET_ARCH", "x86_64"),
            ("TARGET_OS", "windows"),
            ("TARGET_FAMILY", "windows"),
            ("WINDOWS", ""),
            ("TARGET_ENV", "msvc"),
            ("TARGET_VENDOR", "pc"),
            ("TARGET_POINTER_WIDTH", "64"),
            ("TARGET_ENDIAN", "little"),
            ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
            ("PANIC", "unwind"),
        ],
    )
}

fn wasm32() -> TargetContext {
    build_target(
        "wasm32-unknown-unknown",
        &[
            ("TARGET_ARCH", "wasm32"),
            ("TARGET_OS", "unknown"),
            ("TARGET_FAMILY", "wasm"),
            ("TARGET_VENDOR", "unknown"),
            ("TARGET_POINTER_WIDTH", "32"),
            ("TARGET_ENDIAN", "little"),
            ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
            ("PANIC", "abort"),
        ],
    )
}

#[test]
fn equivalent_native_cfg_spellings_satisfy_the_blocking_requirement() {
    // The three spellings #88 reported: each is a table Cargo applies on exactly the native
    // targets, and each used to be looked up by its literal key and reported missing.
    for target in [TargetContext::Unknown, linux(), windows()] {
        let failures = [
            r#"cfg(not(target_arch="wasm32"))"#,
            r#"cfg(not(target_family = "wasm"))"#,
            r#"cfg(all(not(target_arch = "wasm32")))"#,
        ]
        .into_iter()
        .filter_map(|key| {
            let manifest = blocking_manifest(&tokio_table(key, &tokio_declaration()));
            let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &target);
            (!diagnostics.is_empty()).then(|| format!("{key}: {diagnostics:#?}"))
        })
        .collect::<Vec<_>>();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

#[test]
fn without_a_build_target_tables_must_jointly_cover_every_native_target() {
    let covering = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_declaration()),
        tokio_table(
            r#"cfg(all(not(unix), not(target_family = "wasm")))"#,
            &tokio_declaration()
        )
    ));
    let diagnostics = audit_manifest(
        &covering,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    // Cargo builds this on every unix and windows host, but a proc-macro cannot tell which
    // target it is expanding for, and OS-less targets are covered by neither table.
    let gapped = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_declaration()),
        tokio_table("cfg(windows)", &tokio_declaration())
    ));
    let diagnostics = audit_manifest(
        &gapped,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let message = &diagnostics[0].message;
    assert!(message.contains("requires `tokio`"), "{message}");
    assert!(
        message.contains("proc-macro cannot see the build target"),
        "{message}"
    );
    assert!(message.contains("is not covered"), "{message}");
    assert!(
        message.contains("[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]")
            && message.contains("generate from `build.rs`"),
        "both remedies must be named: {message}"
    );
}

#[test]
fn a_tokio_table_that_applies_on_wasm_is_not_native_only() {
    let in_dependencies = format!(
        "{CORE_MANIFEST}{}\n\n[features]\nblocking = [\"dep:tokio\"]\n",
        tokio_declaration()
    );
    let wasm_applicable = blocking_manifest(&tokio_table(
        r#"cfg(any(unix, target_arch = "wasm32"))"#,
        &tokio_declaration(),
    ));
    for target in [TargetContext::Unknown, linux()] {
        for (manifest, table) in [
            (&in_dependencies, "`[dependencies]`"),
            (
                &wasm_applicable,
                "`[target.'cfg(any(unix, target_arch = \"wasm32\"))'.dependencies]`",
            ),
        ] {
            let diagnostics = audit_manifest(manifest, &RuntimeRequirements::default(), &target);
            let messages = messages(&diagnostics);
            assert!(messages.contains("requires `tokio`"), "{messages}");
            assert!(
                messages.contains(&format!(
                    "{table} applies on `wasm32-unknown-unknown`, where the blocking client \
                     is compiled out, so `tokio` must be native-only"
                )),
                "{messages}"
            );
        }
    }
}

#[test]
fn unevaluable_or_unparseable_target_keys_explain_themselves() {
    for (tables, expected) in [
        (
            tokio_table(r#"cfg(not(feature = "x"))"#, &tokio_declaration()),
            "`[target.'cfg(not(feature = \"x\"))'.dependencies]` cannot be evaluated: \
             `feature = \"x\"` does not select target tables",
        ),
        (
            tokio_table("cfg(not(target_arch = ))", &tokio_declaration()),
            "`[target.'cfg(not(target_arch = ))'.dependencies]` does not parse:",
        ),
        (
            format!(
                "[target.x86_64-unknown-linux-gnu.dependencies]\n{}\n",
                tokio_declaration()
            ),
            "is not covered",
        ),
    ] {
        let diagnostics = audit_manifest(
            &blocking_manifest(&tables),
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        );
        let messages = messages(&diagnostics);
        assert!(messages.contains("requires `tokio`"), "{messages}");
        assert!(messages.contains(expected), "{expected}\n{messages}");
    }
}

#[test]
fn a_build_target_selects_the_tables_cargo_would_apply() {
    let manifest = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_declaration()),
        tokio_table("cfg(windows)", &tokio_declaration())
    ));
    for target in [linux(), windows()] {
        let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &target);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}

#[test]
fn a_table_that_misses_the_build_target_names_it() {
    let manifest = blocking_manifest(&tokio_table(
        r#"cfg(target_os = "none")"#,
        &tokio_declaration(),
    ));
    let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &linux());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let message = &diagnostics[0].message;
    assert!(message.contains("requires `tokio`"), "{message}");
    assert!(
        message.contains("evaluated for the build target `x86_64-unknown-linux-gnu`"),
        "{message}"
    );
    assert!(
        message.contains(
            "`[target.'cfg(target_os = \"none\")'.dependencies]` does not apply to \
             `x86_64-unknown-linux-gnu`"
        ),
        "{message}"
    );
}

#[test]
fn a_wasm32_build_needs_no_tokio_but_keeps_the_wiring_check() {
    let manifest = format!("{CORE_MANIFEST}\n[features]\nblocking = []\n");
    let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &wasm32());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        diagnostics[0]
            .message
            .contains("blocking = [\"dep:tokio\"]"),
        "{diagnostics:#?}"
    );

    // The same manifest on a native build still needs the table.
    let messages = messages(&audit_manifest(
        &manifest,
        &RuntimeRequirements::default(),
        &linux(),
    ));
    assert!(messages.contains("requires `tokio`"), "{messages}");
}

#[test]
fn build_flags_and_target_names_are_read_from_cargo() {
    let flagged = blocking_manifest(&tokio_table(
        "cfg(all(unix, not(my_flag)))",
        &tokio_declaration(),
    ));
    assert!(audit_manifest(&flagged, &RuntimeRequirements::default(), &linux()).is_empty());
    let with_flag = build_target(
        "x86_64-unknown-linux-gnu",
        &[LINUX_CFGS, &[("MY_FLAG", "")]].concat(),
    );
    let messages_with_flag = messages(&audit_manifest(
        &flagged,
        &RuntimeRequirements::default(),
        &with_flag,
    ));
    assert!(
        messages_with_flag.contains("does not apply to `x86_64-unknown-linux-gnu`"),
        "{messages_with_flag}"
    );

    let named = blocking_manifest(&format!(
        "[target.x86_64-unknown-linux-gnu.dependencies]\n{}\n",
        tokio_declaration()
    ));
    assert!(audit_manifest(&named, &RuntimeRequirements::default(), &linux()).is_empty());
    let musl = build_target("x86_64-unknown-linux-musl", LINUX_CFGS);
    let messages_on_musl = messages(&audit_manifest(
        &named,
        &RuntimeRequirements::default(),
        &musl,
    ));
    assert!(
        messages_on_musl.contains(
            "`[target.x86_64-unknown-linux-gnu.dependencies]` does not apply to \
             `x86_64-unknown-linux-musl`"
        ),
        "{messages_on_musl}"
    );
}

#[test]
fn contract_rules_still_apply_under_an_alternative_spelling() {
    let key = r#"cfg(not(target_family = "wasm"))"#;
    let not_optional =
        blocking_manifest(&tokio_table(key, &tokio_at_floor(", features = [\"rt\"]")));
    let without_rt = blocking_manifest(&tokio_table(key, &tokio_at_floor(", optional = true")));
    for target in [TargetContext::Unknown, linux()] {
        let messages_not_optional = messages(&audit_manifest(
            &not_optional,
            &RuntimeRequirements::default(),
            &target,
        ));
        assert!(
            messages_not_optional.contains("`tokio` must be optional"),
            "{messages_not_optional}"
        );
        let messages_without_rt = messages(&audit_manifest(
            &without_rt,
            &RuntimeRequirements::default(),
            &target,
        ));
        assert!(
            messages_without_rt.contains("generated client requires Cargo feature `rt` on `tokio`"),
            "{messages_without_rt}"
        );
    }

    // Cargo unifies features across the tables that apply, so `rt` has to reach every target
    // through one of them. Here the table covering non-unix targets leaves it out.
    let split = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_declaration()),
        tokio_table(
            r#"cfg(all(not(unix), not(target_family = "wasm")))"#,
            &tokio_at_floor(", optional = true")
        )
    ));
    let messages_split = messages(&audit_manifest(
        &split,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    ));
    assert!(
        messages_split.contains(
            "requires Cargo feature `rt` on `tokio` (not enabled by the tables that apply to"
        ),
        "{messages_split}"
    );
    // On a build only the tables that apply are unified: `cfg(windows)` carries `rt`, but it
    // does not apply on linux.
    let build_split = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_at_floor(", optional = true")),
        tokio_table("cfg(windows)", &tokio_declaration())
    ));
    let messages_build = messages(&audit_manifest(
        &build_split,
        &RuntimeRequirements::default(),
        &linux(),
    ));
    assert!(
        messages_build.contains("generated client requires Cargo feature `rt` on `tokio`"),
        "{messages_build}"
    );
    assert!(
        audit_manifest(&build_split, &RuntimeRequirements::default(), &windows())
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("feature `rt`"))
    );
}

#[test]
fn a_below_floor_tokio_in_one_of_two_counting_tables_names_that_table() {
    // Both tables count on either path: `cfg(unix)` applies to the unix builtins and to linux,
    // and `cfg(not(target_family = "wasm"))` covers everything else. Each counting declaration
    // meets the version floor on its own, and with more than one the message says which.
    let manifest = blocking_manifest(&format!(
        "{}{}",
        tokio_table(
            "cfg(unix)",
            "tokio = { version = \"1.53.0\", features = [\"rt\"], optional = true }"
        ),
        tokio_table(r#"cfg(not(target_family = "wasm"))"#, &tokio_declaration())
    ));
    for target in [TargetContext::Unknown, linux()] {
        let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &target);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            format!(
                "`tokio` version requirement `1.53.0` is outside the supported range >={floor}, \
                 <{ceiling}; use `{floor}` or a higher compatible caret requirement (in \
                 `[target.'cfg(unix)'.dependencies]`)",
                floor = TOKIO.floor,
                ceiling = TOKIO.ceiling(),
            )
        );
    }
}

#[test]
fn a_renamed_tokio_in_a_target_table_follows_the_untargeted_rename_rule() {
    // Untargeted, the rule has two halves: the canonical name bound to another package is
    // rejected, and the crate declared under another name is not found at all. The identity
    // spelling `package = "<the key>"` renames nothing — Cargo's `package` defaults to the key —
    // so it is accepted (#168). The entry and its version are read out of `CORE_MANIFEST`, and
    // the missing-crate message names the contract's own floor, so neither is restated here.
    let (core_secrecy, version) = core_entry("secrecy");
    let untargeted_identity = replace_once(
        CORE_MANIFEST,
        core_secrecy,
        &format!("secrecy = {{ package = \"secrecy\", version = \"{version}\" }}"),
    );
    assert_eq!(
        messages(&audit_manifest(
            &untargeted_identity,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown
        )),
        ""
    );
    let untargeted_package = replace_once(
        CORE_MANIFEST,
        core_secrecy,
        &format!("secrecy = {{ package = \"secrecy-fork\", version = \"{version}\" }}"),
    );
    assert_eq!(
        messages(&audit_manifest(
            &untargeted_package,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown
        )),
        "`secrecy` cannot be renamed because generated code references that canonical crate \
         name"
    );
    // A non-string `package` is not a spelling Cargo accepts, so it is not read as the
    // identity: only `package = "<the key>"` is exempt, and `package = 1` stays a rename.
    let untargeted_non_string = replace_once(
        CORE_MANIFEST,
        core_secrecy,
        &format!("secrecy = {{ package = 1, version = \"{version}\" }}"),
    );
    assert_eq!(
        messages(&audit_manifest(
            &untargeted_non_string,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown
        )),
        "`secrecy` cannot be renamed because generated code references that canonical crate \
         name"
    );
    let untargeted_alias = replace_once(
        CORE_MANIFEST,
        core_secrecy,
        &format!("secret = {{ package = \"secrecy\", version = \"{version}\" }}"),
    );
    assert_eq!(
        messages(&audit_manifest(
            &untargeted_alias,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown
        )),
        format!(
            "generated client requires `secrecy`; add `secrecy` with version `{}`",
            SECRECY.floor
        )
    );

    // A target table applies the same two halves, and the same identity exemption, to `tokio`,
    // on both paths.
    let key = r#"cfg(not(target_family = "wasm"))"#;
    let floor = TOKIO.floor;
    let targeted_identity = blocking_manifest(&tokio_table(
        key,
        &format!(
            "tokio = {{ package = \"tokio\", version = \"{floor}\", features = [\"rt\"], \
             optional = true }}"
        ),
    ));
    let targeted_package = blocking_manifest(&tokio_table(
        key,
        &format!(
            "tokio = {{ package = \"tokio-fork\", version = \"{floor}\", features = [\"rt\"], \
             optional = true }}"
        ),
    ));
    let targeted_alias = blocking_manifest(&tokio_table(
        key,
        &format!(
            "tokio_rt = {{ package = \"tokio\", version = \"{floor}\", features = [\"rt\"], \
             optional = true }}"
        ),
    ));
    for target in [TargetContext::Unknown, linux()] {
        assert_eq!(
            messages(&audit_manifest(
                &targeted_identity,
                &RuntimeRequirements::default(),
                &target
            )),
            ""
        );
        assert_eq!(
            messages(&audit_manifest(
                &targeted_package,
                &RuntimeRequirements::default(),
                &target
            )),
            "`tokio` cannot be renamed because generated code references that canonical crate \
             name"
        );
        let alias_messages = messages(&audit_manifest(
            &targeted_alias,
            &RuntimeRequirements::default(),
            &target,
        ));
        assert!(
            alias_messages
                .starts_with("generated client requires `tokio`; add `tokio` with version")
                && !alias_messages.contains('\n'),
            "{alias_messages}"
        );
    }
}

#[test]
fn tokio_optional_in_one_counting_table_and_not_the_other_is_reported_on_that_table() {
    // Assumption A2: every counting declaration is held to `optional = true`, so a pair that
    // disagrees is reported on the declaration that is not optional rather than accepted.
    let manifest = blocking_manifest(&format!(
        "{}{}",
        tokio_table("cfg(unix)", &tokio_declaration()),
        tokio_table(
            r#"cfg(not(target_family = "wasm"))"#,
            &tokio_at_floor(", features = [\"rt\"]")
        )
    ));
    for target in [TargetContext::Unknown, linux()] {
        let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &target);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "`tokio` must be optional because it is enabled only by the generated `blocking` \
             feature (in `[target.'cfg(not(target_family = \"wasm\"))'.dependencies]`)"
        );
    }
}

#[test]
fn build_flag_predicates_have_no_value_on_a_builtin_target() {
    // A flag, a key-value cfg or a target feature has no value on a builtin target, where
    // `RUSTFLAGS` could set anything. Without a build target every native table is judged on
    // builtins, so a table that needs one to apply cannot be evaluated, and says so.
    let first_native = ALL_BUILTINS
        .iter()
        .find(|info| !info.families.iter().any(|family| family.as_str() == "wasm"))
        .expect("cfg-expr knows native targets")
        .triple
        .as_str();
    for (predicate, spelled) in [
        ("my_flag", "`my_flag`"),
        (r#"my_key = "on""#, "`my_key = \"on\"`"),
        (
            r#"target_feature = "crt-static""#,
            "`target_feature = \"crt-static\"`",
        ),
    ] {
        let key = format!(r#"cfg(all(not(target_arch = "wasm32"), {predicate}))"#);
        let manifest = blocking_manifest(&tokio_table(&key, &tokio_declaration()));
        let messages = messages(&audit_manifest(
            &manifest,
            &RuntimeRequirements::default(),
            &TargetContext::Unknown,
        ));
        let expected = format!(
            "`[target.'{key}'.dependencies]` cannot be evaluated: {spelled} depends on the \
             build's flags, which are not known for `{first_native}`"
        );
        assert!(
            messages.contains("requires `tokio`") && messages.contains(&expected),
            "{expected}\n{messages}"
        );
    }

    // The native-only check runs on the builtin wasm anchor on both paths, so from a build
    // script too a table whose only native evidence is a flag never counts, even when the flag
    // is set for the target being built.
    let flag_only = blocking_manifest(&tokio_table("cfg(my_flag)", &tokio_declaration()));
    let flagged_linux = build_target(
        "x86_64-unknown-linux-gnu",
        &[LINUX_CFGS, &[("MY_FLAG", "")]].concat(),
    );
    let messages = messages(&audit_manifest(
        &flag_only,
        &RuntimeRequirements::default(),
        &flagged_linux,
    ));
    assert!(
        messages.contains(
            "`[target.'cfg(my_flag)'.dependencies]` cannot be evaluated: `my_flag` depends on \
             the build's flags, which are not known for `wasm32-unknown-unknown`"
        ),
        "{messages}"
    );
}

#[test]
fn a_target_triple_spargen_does_not_know_is_named_as_unknown() {
    // Cargo accepts any triple as a table key, including a custom target's. Without a build
    // target spargen can only compare it against the builtin list, so a triple missing from
    // that list is reported as unknown, not as a table that applies to no native target.
    let custom = blocking_manifest(&format!(
        "[target.x86_64-custom-none.dependencies]\n{}\n",
        tokio_declaration()
    ));
    let macro_messages = messages(&audit_manifest(
        &custom,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    ));
    assert!(
        macro_messages.contains(
            "`[target.x86_64-custom-none.dependencies]` names `x86_64-custom-none`, which is \
             not a target spargen knows, so without the build target it cannot be shown to \
             apply to the one being built"
        ),
        "{macro_messages}"
    );
    assert!(
        !macro_messages.contains("applies to no"),
        "{macro_messages}"
    );

    // A build script building that custom target applies the table exactly as Cargo does.
    let custom_build = build_target(
        "x86_64-custom-none",
        &[
            ("TARGET_ARCH", "x86_64"),
            ("TARGET_OS", "none"),
            ("TARGET_VENDOR", "unknown"),
            ("TARGET_POINTER_WIDTH", "64"),
            ("TARGET_ENDIAN", "little"),
            ("PANIC", "abort"),
        ],
    );
    let diagnostics = audit_manifest(&custom, &RuntimeRequirements::default(), &custom_build);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    // A cfg that genuinely matches no known native target keeps saying so.
    let contradictory = blocking_manifest(&tokio_table(
        "cfg(all(unix, windows))",
        &tokio_declaration(),
    ));
    let messages = messages(&audit_manifest(
        &contradictory,
        &RuntimeRequirements::default(),
        &TargetContext::Unknown,
    ));
    assert!(
        messages.contains(
            "`[target.'cfg(all(unix, windows))'.dependencies]` applies to no known non-wasm \
             target"
        ),
        "{messages}"
    );
}

#[test]
fn predicates_that_never_select_target_tables_are_explained_on_both_paths() {
    // Pins existing behaviour. `feature`, `test`, `debug_assertions` and `proc_macro` have no
    // value for any target: Cargo does not select target tables by them. The key rules wasm
    // out first, so it passes the native-only check and the unknown predicate is what the build
    // target, or every native builtin, has to answer.
    for (predicate, reason) in [
        (
            r#"feature = "x""#,
            "`feature = \"x\"` does not select target tables",
        ),
        ("test", "`test` does not select target tables"),
        (
            "debug_assertions",
            "`debug_assertions` does not select target tables",
        ),
        ("proc_macro", "`proc_macro` does not select target tables"),
    ] {
        let key = format!(r#"cfg(all(not(target_arch = "wasm32"), not({predicate})))"#);
        let manifest = blocking_manifest(&tokio_table(&key, &tokio_declaration()));
        let clause = format!("; `[target.'{key}'.dependencies]` cannot be evaluated: {reason}");
        for (target, rule) in [
            (
                linux(),
                "(evaluated for the build target `x86_64-unknown-linux-gnu`)",
            ),
            (
                TargetContext::Unknown,
                "(a proc-macro cannot see the build target",
            ),
        ] {
            let diagnostics = audit_manifest(&manifest, &RuntimeRequirements::default(), &target);
            assert_eq!(diagnostics.len(), 1, "{key}: {diagnostics:#?}");
            let message = &diagnostics[0].message;
            assert!(
                message.starts_with("generated client requires `tokio`")
                    && message.contains(rule)
                    && message.ends_with(&clause),
                "{rule}\n{clause}\n{message}"
            );
        }
    }

    // Without `not(target_arch = "wasm32")` the same predicate already fails the native-only
    // check on the wasm anchor, from a build script as well.
    let feature_only = blocking_manifest(&tokio_table(
        r#"cfg(not(feature = "x"))"#,
        &tokio_declaration(),
    ));
    let messages = messages(&audit_manifest(
        &feature_only,
        &RuntimeRequirements::default(),
        &linux(),
    ));
    assert!(
        messages.contains("(evaluated for the build target `x86_64-unknown-linux-gnu`)")
            && messages.ends_with(
                "; `[target.'cfg(not(feature = \"x\"))'.dependencies]` cannot be evaluated: \
                 `feature = \"x\"` does not select target tables"
            ),
        "{messages}"
    );
}

#[test]
fn workspace_inherited_tokio_under_an_alternative_spelling_resolves() {
    let directory = Sandbox::new();
    let member_dir = directory.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n{}\
             {}\n",
            core_workspace_dependencies(),
            tokio_at_floor(", features = [\"rt\"]")
        ),
    )
    .unwrap();
    let member = Utf8PathBuf::from_path_buf(member_dir.join("Cargo.toml")).unwrap();
    let inherited_optional = format!(
        "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n[features]\n\
         blocking = [\"dep:tokio\"]\n\n{CORE_INHERITED}\n\
         [target.'cfg(not(target_arch=\"wasm32\"))'.dependencies]\n\
         tokio = {{ workspace = true, optional = true }}\n"
    );
    let inherited_required = replace_once(&inherited_optional, ", optional = true", "");
    for target in [TargetContext::Unknown, linux()] {
        // Cargo does not inherit `optional`: the root declares version and features, and the
        // member adds `optional = true` beside `workspace = true`.
        std::fs::write(&member, &inherited_optional).unwrap();
        let result = directory.audit_in(&member, &RuntimeRequirements::default(), &target);
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

        // So a member that leaves it out is held to the optional rule although the entry it
        // inherits resolves: exactly that rule fires, and nothing about the inheritance. Only
        // this half notices the rule being skipped for inherited declarations.
        std::fs::write(&member, &inherited_required).unwrap();
        let result = directory.audit_in(&member, &RuntimeRequirements::default(), &target);
        assert_eq!(
            messages(&result.diagnostics),
            "`tokio` must be optional because it is enabled only by the generated `blocking` \
             feature"
        );
    }
}

#[test]
fn every_target_predicate_reads_the_variable_cargo_sets() {
    // The build-script mapping must agree with cfg-expr's own database for the same triple, for
    // every kind of target predicate, both ways round.
    let TargetContext::Build(build) = linux() else {
        unreachable!("linux() is a build target")
    };
    let builtin = get_builtin_target_by_triple("x86_64-unknown-linux-gnu").unwrap();
    for predicate in [
        r#"target_arch = "x86_64""#,
        r#"target_arch = "aarch64""#,
        r#"target_os = "linux""#,
        r#"target_os = "none""#,
        r#"target_family = "unix""#,
        r#"target_family = "windows""#,
        "unix",
        "windows",
        r#"target_env = "gnu""#,
        r#"target_env = "musl""#,
        r#"target_env = """#,
        r#"target_abi = """#,
        r#"target_abi = "eabihf""#,
        r#"target_vendor = "unknown""#,
        r#"target_vendor = "apple""#,
        r#"target_pointer_width = "64""#,
        r#"target_pointer_width = "32""#,
        r#"target_endian = "little""#,
        r#"target_endian = "big""#,
        r#"target_has_atomic = "ptr""#,
        r#"target_has_atomic = "64""#,
        r#"panic = "unwind""#,
        r#"panic = "abort""#,
    ] {
        let spelled = format!("cfg({predicate})");
        let key = TableKey::parse(&spelled);
        assert_eq!(
            key.evaluate(Subject::Build(&build)),
            key.evaluate(Subject::Builtin(builtin)),
            "{predicate}"
        );
    }
}
