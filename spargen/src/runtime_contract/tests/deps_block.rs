use super::*;

/// The blocking opt-in every block ends with, whatever the API uses.
const BLOCKING_OPT_IN: &str = r#"
# To opt in to the `blocking` Cargo feature, uncomment the lines below, merging each entry into a table (or `blocking` key) your manifest already declares:
# [features]
# blocking = ["dep:tokio"]
# [target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# tokio = { version = "1.53.1", features = ["rt"], optional = true }
"#;

#[test]
fn the_default_dependency_block_is_printed_byte_for_byte() {
    // The bytes `spargen deps` prints for an API that uses no optional capability: a bare
    // `name = "floor"` where nothing else is required, the inline-table form otherwise, and the
    // blocking opt-in last. The audit test below proves the block is accepted; this one pins
    // what a consumer actually pastes.
    let block = Requirements::new(&RuntimeRequirements::default()).manifest_block();
    assert_eq!(
        block,
        format!(
            r#"[dependencies]
bytes = "1.12.1"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"
{BLOCKING_OPT_IN}"#
        )
    );
}

#[test]
fn the_all_capability_dependency_block_is_printed_byte_for_byte() {
    let requirements = RuntimeRequirements {
        reqwest_multipart: true,
        bytes_serde: true,
        streams: true,
        xml: true,
        uuid: true,
        time: true,
    };
    let block = Requirements::new(&requirements).manifest_block();
    assert_eq!(
        block,
        format!(
            r#"[dependencies]
bytes = {{ version = "1.12.1", features = ["serde"] }}
reqwest = {{ version = "0.12.28", default-features = false, features = ["multipart", "stream"] }}
futures-core = "0.3.32"
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"
quick-xml = {{ version = "0.41.0", features = ["serialize"] }}
uuid = {{ version = "1.24.0", features = ["serde"] }}
time = {{ version = "0.3.55", features = ["formatting", "parsing"] }}
{BLOCKING_OPT_IN}"#
        )
    );
    // `Display` is the same block without its trailing newline.
    assert_eq!(
        Requirements::new(&requirements).to_string(),
        block.trim_end()
    );
}

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

/// The anti-drift property: the block `spargen deps` prints must be exactly a block the audit
/// accepts. If the two ever diverge — a feature demanded but not printed, or printed with the
/// wrong floor — this fails.
#[test]
fn the_printed_dependency_block_passes_the_audit_it_describes() {
    // Every capability on at once, so the table is exercised in full.
    let requirements = RuntimeRequirements {
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

    let diagnostics = audit_manifest(&manifest, &requirements, &TargetContext::Unknown);
    assert!(
        diagnostics.is_empty(),
        "the block `spargen deps` prints must satisfy the audit:\n{manifest}\n{diagnostics:#?}"
    );

    // And with the feature absent, the commented-out block is genuinely not required.
    let without_blocking =
        format!("[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n\n{block}");
    let diagnostics = audit_manifest(&without_blocking, &requirements, &TargetContext::Unknown);
    assert!(
        diagnostics.is_empty(),
        "{without_blocking}\n{diagnostics:#?}"
    );
}
