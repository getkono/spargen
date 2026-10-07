use super::*;

#[test]
fn the_printed_tokio_table_is_the_generated_blocking_gate() {
    // `spargen deps` prints the table as a string, the audit evaluates `BLOCKING_GATE`, and
    // `e2e.rs` pins the gate generated code emits. This keeps the first two the same cfg.
    let requirements = Requirements::new(&RuntimeRequirements::default());
    let tokio = requirements
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "tokio")
        .expect("the blocking client's tokio is always in the table");
    assert_eq!(
        tokio.table,
        format!("target.'cfg({BLOCKING_GATE})'.dependencies")
    );
    assert!(Expression::parse(BLOCKING_GATE).is_ok());
    assert!(get_builtin_target_by_triple(WASM_ANCHOR).is_some());
}

#[test]
fn the_manifests_reported_in_issue_71_pass_as_written() {
    // The layout exactly as #71 reported it: the root spells `futures-core` as a plain string
    // and `uuid` as a table with its own features; the member inherits both beside the five
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
             futures-core = \"0.3.32\"\n\
             uuid = {{ version = \"1.26.0\", features = [\"v4\", \"serde\"] }}\n",
            core_workspace_dependencies()
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

/// The anti-drift property: the block `spargen deps` prints must be exactly a block the audit
/// accepts. If the two ever diverge — a feature demanded but not printed, or printed with the
/// wrong floor — this fails.
#[test]
fn the_printed_dependency_block_passes_the_audit_it_describes() {
    // Every capability on at once, so the table is exercised in full.
    let requirements = RuntimeRequirements {
        reqwest_json: true,
        reqwest_multipart: true,
        bytes_serde: true,
        streams: true,
        xml: true,
        uuid: true,
        time: true,
    };
    let block = Requirements::new(&requirements).manifest_block();
    // `deps` renders the blocking opt-in — its `[features]` entry and its dependency —
    // commented out under the feature that requires it; a consumer that opts in uncomments
    // every line below that header, and adds nothing else, which is what this reconstructs.
    let opted_in = block
        .lines()
        .map(|line| match line.strip_prefix("# ") {
            Some(rest) if !rest.starts_with("To opt in") => rest,
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        opted_in.contains("[features]\nblocking = [\"dep:tokio\"]\n"),
        "the printed opt-in carries the feature wiring the audit requires:\n{block}"
    );
    // A manifest that already has a `[features]` table (or a `blocking` key) cannot take a
    // second one, so the header must say the entries merge rather than claim a bare
    // uncomment always suffices.
    assert!(
        block.contains(
            "# To opt in to the `blocking` Cargo feature, uncomment the lines below, merging \
             each entry into a table (or `blocking` key) your manifest already declares:\n"
        ),
        "the opt-in header tells a consumer with existing tables to merge into them:\n{block}"
    );
    let manifest = format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{opted_in}");

    let diagnostics = audit_manifest(&manifest, requirements.clone());
    assert!(
        diagnostics.is_empty(),
        "the block `spargen deps` prints must satisfy the audit:\n{manifest}\n{diagnostics:#?}"
    );

    // And with the feature absent, the commented-out block is genuinely not required.
    let without_blocking =
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{block}");
    let diagnostics = audit_manifest(&without_blocking, requirements);
    assert!(
        diagnostics.is_empty(),
        "{without_blocking}\n{diagnostics:#?}"
    );
}
