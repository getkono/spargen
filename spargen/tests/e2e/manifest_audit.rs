//! The consumer manifest audit: what it derives from an API, how it follows workspace
//! inheritance in agreement with Cargo, and the build-script warnings it raises.

use super::*;

#[test]
fn macro_manifest_audit_derives_only_capabilities_referenced_by_the_api() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        r#"[package]
name = "audit-consumer"
version = "0.0.0"

[dependencies]
bytes = "1.12.1"
reqwest = { version = "0.12.28", default-features = false }
secrecy = "0.10.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"

[workspace]
"#,
    )
    .unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: { title: Core, version: 1.0.0 }\npaths: {}\n",
    )
    .unwrap();

    let core =
        spargen::__private::preview_for_macro(&Spec::new(spec.clone()), manifest.to_str().unwrap());
    assert_eq!(
        core.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        core.report
    );
    let core_output = core.contents.expect("generated core-only module");
    assert!(!core_output.contains("futures_core"), "{core_output}");
    assert!(!core_output.contains("mod stream"), "{core_output}");

    std::fs::write(
        &spec,
        r##"openapi: 3.1.0
info: { title: Conditional, version: 1.0.0 }
paths:
  /json:
    post:
      requestBody:
        content:
          application/json:
            schema: { type: string }
      responses:
        "204": { description: ok }
  /binary-array:
    get:
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: array
                items: { type: string, format: binary }
  /xml:
    get:
      responses:
        "200":
          description: ok
          content:
            application/xml:
              schema: { $ref: "#/components/schemas/XmlBody" }
  /events:
    get:
      responses:
        "200":
          description: events
          content:
            text/event-stream:
              schema: { type: string }
components:
  schemas:
    XmlBody:
      type: object
      properties:
        value: { type: string }
    Identifier: { type: string, format: uuid }
    Timestamp: { type: string, format: date-time }
"##,
    )
    .unwrap();

    let conditional =
        spargen::__private::preview_for_macro(&Spec::new(spec), manifest.to_str().unwrap());
    assert_eq!(conditional.report.outcome(), Outcome::Rejected);
    let messages = conditional
        .report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.code, Code::RuntimeDependencyContract);
            diagnostic.message.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n");
    // A JSON request body is serialized by `serde_json` and sent as bytes, so it needs no
    // reqwest feature.
    assert!(!messages.contains("`json`"), "{messages}");
    assert!(
        messages.contains("feature `serde` on `bytes`"),
        "{messages}"
    );
    assert!(messages.contains("requires `quick-xml`"), "{messages}");
    assert!(messages.contains("requires `futures-core`"), "{messages}");
    assert!(
        messages.contains("feature `stream` on `reqwest`"),
        "{messages}"
    );
    assert!(messages.contains("requires `uuid`"), "{messages}");
    assert!(messages.contains("requires `time`"), "{messages}");
    assert!(!messages.contains("multipart"), "{messages}");
    assert!(!messages.contains("tokio"), "{messages}");
}

/// The report behind #71, end to end: a workspace root declaring `futures-core` and `uuid` under
/// `[workspace.dependencies]`, a member inheriting them with `{ workspace = true }`, and a 3.2
/// document with one `text/event-stream` operation and a `format: uuid` field. The report said
/// both came back as `E023` "generated client requires …"; the layout resolves, and this pins the
/// whole chain — spec, derived requirement set, manifest audit — as passing.
#[test]
fn macro_manifest_audit_follows_workspace_inherited_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Cargo.toml");
    let member_dir = temp.path().join("client");
    std::fs::create_dir(&member_dir).unwrap();
    let manifest = member_dir.join("Cargo.toml");
    std::fs::write(
        &root,
        r#"[workspace]
members = ["client"]

[workspace.dependencies]
bytes = "1.12.1"
reqwest = { version = "0.12.28", default-features = false, features = ["stream"] }
secrecy = "0.10.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
futures-core = "0.3.32"
uuid = { version = "1.26.0", features = ["v4", "serde"] }
"#,
    )
    .unwrap();
    std::fs::write(
        &manifest,
        r#"[package]
name = "audit-consumer"
version = "0.0.0"

[dependencies]
bytes = { workspace = true }
reqwest = { workspace = true }
secrecy = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
futures-core = { workspace = true }
uuid = { workspace = true }
"#,
    )
    .unwrap();
    let spec = Utf8PathBuf::from_path_buf(member_dir.join("openapi.yaml")).unwrap();
    std::fs::write(
        &spec,
        r##"openapi: 3.2.0
info: { title: Inherited, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        "200":
          description: events
          content:
            text/event-stream:
              itemSchema: { $ref: "#/components/schemas/Event" }
components:
  schemas:
    Event:
      type: object
      required: [id]
      properties:
        id: { type: string, format: uuid }
"##,
    )
    .unwrap();

    let preview =
        spargen::__private::preview_for_macro(&Spec::new(spec), manifest.to_str().unwrap());
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        preview.report
    );
    assert!(
        !preview
            .report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract),
        "{:#?}",
        preview.report
    );
    // The requirement set really did include both crates: the module uses them. The `stream`
    // module is embedded only for a sequential response, and `Event`'s `id` field type is an alias
    // of the `format: uuid` mapping. Each assertion names the emitted item: the bare words
    // `stream`, `EventStream` and `uuid::Uuid` also ride in the runtime's doc comments, which
    // every module carries.
    let generated = preview.contents.expect("generated module");
    assert!(generated.contains("mod stream {"), "{generated}");
    assert!(
        generated.contains("pub type Eventid = uuid::Uuid;"),
        "{generated}"
    );
    // The root is part of the input set — an edit there changes what the audit sees.
    assert!(
        preview
            .source_files
            .iter()
            .any(|path| path.as_std_path() == root),
        "{:#?}",
        preview.source_files
    );
}

/// One objection the `E023` audit raises against a required dependency, by crate name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Objection {
    Missing(String),
    Feature { krate: String, feature: String },
    Defaults(String),
    Optional(String),
    Renamed(String),
}

/// Classify one `E023` message. A message this does not recognise panics, so an objection the
/// audit learns later cannot pass the Cargo oracle below unexamined.
fn objection(message: &str) -> Objection {
    let ticked = |text: &str, index: usize| text.split('`').nth(index).unwrap().to_owned();
    if message.starts_with("generated client requires Cargo feature `") {
        return Objection::Feature {
            krate: ticked(message, 3),
            feature: ticked(message, 1),
        };
    }
    if message.starts_with("generated client requires `") {
        return Objection::Missing(ticked(message, 1));
    }
    let krate = ticked(message, 1);
    let rest = message.splitn(3, '`').nth(2).unwrap_or_default();
    if rest.starts_with(" must set `default-features = false`") {
        Objection::Defaults(krate)
    } else if rest.starts_with(" must not be optional") || rest.starts_with(" must be optional") {
        Objection::Optional(krate)
    } else if rest.starts_with(" cannot be renamed") {
        Objection::Renamed(krate)
    } else {
        panic!("an E023 message the Cargo oracle cannot classify: {message}")
    }
}

/// What Cargo itself makes of one dependency the member declares.
#[derive(Debug)]
struct CargoDeclared {
    /// The package the declaration names (`package`, or else the key).
    package: String,
    /// Whether the crate is in the member's graph with every feature of the member off — the
    /// build in which an optional dependency is absent however its features are wired.
    unconditional: bool,
    /// The features Cargo activates on it, from that build where the crate is in it, otherwise
    /// with every member feature on.
    features: std::collections::BTreeSet<String>,
}

/// Ask Cargo, not spargen, how `member` resolves: every dependency it declares, keyed by the name
/// the member's code sees it under. `Err` carries Cargo's refusal when it will not load the layout.
///
/// Each stub crate is a path dependency outside the workspace root, so it is never a member and
/// its activated features are exactly what the member's declarations ask for; `default` is one of
/// them only when Cargo kept default features on.
fn cargo_view(
    member: &std::path::Path,
    package: &str,
) -> Result<std::collections::BTreeMap<String, CargoDeclared>, String> {
    let metadata = |features: &str| {
        let output = query_cargo(member.parent().unwrap())
            .args(["metadata", "--offline", "--format-version", "1", features])
            .arg("--manifest-path")
            .arg(member)
            .output()
            .unwrap();
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap())
    };
    let minimal = metadata("--no-default-features")?;
    let full = metadata("--all-features")?;

    // package name -> the features activated on it, over the member's direct dependencies.
    let activated = |metadata: &serde_json::Value| {
        let name_of = |id: &serde_json::Value| {
            metadata["packages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|candidate| candidate["id"] == *id)
                .unwrap()["name"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let nodes = metadata["resolve"]["nodes"].as_array().unwrap();
        let features_of = |id: &serde_json::Value| {
            nodes.iter().find(|node| node["id"] == *id).unwrap()["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|feature| feature.as_str().unwrap().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let member = nodes
            .iter()
            .find(|node| name_of(&node["id"]) == package)
            .unwrap();
        member["deps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|dep| (name_of(&dep["pkg"]), features_of(&dep["pkg"])))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let (minimal_activated, full_activated) = (activated(&minimal), activated(&full));

    let declarations = full["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["name"] == package)
        .unwrap()["dependencies"]
        .as_array()
        .unwrap();
    Ok(declarations
        .iter()
        .map(|declared| {
            let name = declared["name"].as_str().unwrap().to_owned();
            let key = declared["rename"].as_str().unwrap_or(&name).to_owned();
            let unconditional = minimal_activated.get(&name);
            let features = unconditional
                .or_else(|| full_activated.get(&name))
                .cloned()
                .unwrap_or_default();
            let view = CargoDeclared {
                unconditional: unconditional.is_some(),
                package: name,
                features,
            };
            (key, view)
        })
        .collect())
}

/// What the audit must say about a layout Cargo accepts, derived from Cargo's own resolution of it
/// and the requirement set alone — never from spargen's model of inheritance.
fn objections_cargo_implies(
    requirements: &spargen::Requirements,
    cargo: &std::collections::BTreeMap<String, CargoDeclared>,
) -> std::collections::BTreeSet<Objection> {
    let mut objections = std::collections::BTreeSet::new();
    for required in requirements
        .dependencies
        .iter()
        .filter(|dependency| dependency.required_by_feature.is_none())
    {
        let name = required.name.to_owned();
        let Some(declared) = cargo.get(required.name) else {
            objections.insert(Objection::Missing(name));
            continue;
        };
        if declared.package != required.name {
            objections.insert(Objection::Renamed(name.clone()));
        }
        for feature in &required.features {
            if !declared.features.contains(*feature) {
                objections.insert(Objection::Feature {
                    krate: name.clone(),
                    feature: (*feature).to_owned(),
                });
            }
        }
        if required.no_default_features && declared.features.contains("default") {
            objections.insert(Objection::Defaults(name.clone()));
        }
        if declared.unconditional == required.optional {
            objections.insert(Objection::Optional(name));
        }
    }
    objections
}

/// #173: `E023`'s explain text says it follows `workspace = true` "as Cargo does" — the feature
/// union, the `default-features` rule, `optional` read from the member, and what counts as a
/// rename. Every other inheritance fixture pins spargen's model of that against spargen's own
/// expectations, so a divergence from Cargo would leave the suite green and surface as a rustc
/// error inside a consumer's generated code. Here Cargo is the oracle: each layout is resolved by
/// `cargo metadata` over local stub crates (offline, no registry), the objections Cargo's
/// resolution implies are derived from that and the requirement set alone, and the audit must
/// raise exactly those. A layout Cargo refuses never reaches the audit — neither `build.rs` nor
/// the macro runs for a manifest Cargo will not load — so it pins only the refusal the explain
/// text's advice rests on.
#[test]
fn workspace_inheritance_audit_agrees_with_cargo() {
    let temp = tempfile::tempdir().unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(
        &spec,
        r##"openapi: 3.1.0
info: { title: Inherited, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                name: { type: string }
      responses:
        "204": { description: ok }
"##,
    )
    .unwrap();
    let requirements = spargen::requirements(&Spec::new(spec.clone())).expect("spec lowers");
    let required = requirements
        .dependencies
        .iter()
        .filter(|dependency| dependency.required_by_feature.is_none())
        .collect::<Vec<_>>();
    let reqwest = required
        .iter()
        .find(|dependency| dependency.name == "reqwest")
        .unwrap();
    assert!(
        reqwest.no_default_features && reqwest.features.contains(&"multipart"),
        "the cases below need a required feature and a required `default-features = false`: \
         {requirements:#?}"
    );

    // One stub per required crate at its floor version, declaring `default` and every feature the
    // requirement names, plus a `bytes-fork` to rename to.
    let stub = |dir: &str, name: &str, version: &str, features: &[&str]| {
        let root = temp.path().join("stubs").join(dir);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        let features = features
            .iter()
            .map(|feature| format!("{feature} = []\n"))
            .collect::<String>();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n\
                 [features]\ndefault = []\n{features}"
            ),
        )
        .unwrap();
    };
    for dependency in &required {
        stub(
            dependency.name,
            dependency.name,
            dependency.version,
            &dependency.features,
        );
    }
    let bytes = required
        .iter()
        .find(|dependency| dependency.name == "bytes")
        .unwrap();
    stub("bytes-fork", "bytes-fork", bytes.version, &bytes.features);

    // The declaration `spargen deps` asks for, as a `[workspace.dependencies]` line on a stub.
    let root_line = |dependency: &spargen::RequiredDependency| {
        let mut parts = vec![
            format!("version = \"{}\"", dependency.version),
            format!("path = \"../stubs/{}\"", dependency.name),
        ];
        if dependency.no_default_features {
            parts.push("default-features = false".to_owned());
        }
        if !dependency.features.is_empty() {
            let features = dependency
                .features
                .iter()
                .map(|feature| format!("\"{feature}\""))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("features = [{features}]"));
        }
        format!("{} = {{ {} }}", dependency.name, parts.join(", "))
    };

    struct Case {
        name: &'static str,
        /// `[workspace.dependencies]` lines replacing the advised one, by crate.
        root: &'static [(&'static str, &'static str)],
        /// Member lines replacing `name = { workspace = true }`, by crate; empty drops it.
        member: &'static [(&'static str, &'static str)],
        /// Extra `[package]` keys, then extra member tables.
        package: &'static str,
        tables: &'static str,
        /// `Some(reason)` where Cargo must refuse the layout with that reason.
        refused: Option<&'static str>,
    }
    const PLAIN: Case = Case {
        name: "",
        root: &[],
        member: &[],
        package: "",
        tables: "",
        refused: None,
    };
    let cases = [
        Case {
            name: "every requirement inherited as advised",
            ..PLAIN
        },
        Case {
            name: "features split between root and member (the union)",
            root: &[
                (
                    "reqwest",
                    r#"reqwest = { version = "{version}", path = "../stubs/reqwest", default-features = false }"#,
                ),
                (
                    "serde",
                    r#"serde = { version = "{version}", path = "../stubs/serde" }"#,
                ),
            ],
            member: &[
                (
                    "reqwest",
                    r#"reqwest = { workspace = true, features = ["multipart"] }"#,
                ),
                (
                    "serde",
                    r#"serde = { workspace = true, features = ["derive"] }"#,
                ),
            ],
            ..PLAIN
        },
        Case {
            name: "a required feature declared on neither side",
            root: &[(
                "reqwest",
                r#"reqwest = { version = "{version}", path = "../stubs/reqwest", default-features = false }"#,
            )],
            ..PLAIN
        },
        Case {
            name:
                "the member's default-features = false cannot turn off defaults the root leaves on",
            root: &[(
                "reqwest",
                r#"reqwest = { version = "{version}", path = "../stubs/reqwest", features = ["multipart"] }"#,
            )],
            member: &[(
                "reqwest",
                r#"reqwest = { workspace = true, default-features = false }"#,
            )],
            ..PLAIN
        },
        Case {
            name: "the same override, refused outright on edition 2024",
            root: &[(
                "reqwest",
                r#"reqwest = { version = "{version}", path = "../stubs/reqwest", features = ["multipart"] }"#,
            )],
            member: &[(
                "reqwest",
                r#"reqwest = { workspace = true, default-features = false }"#,
            )],
            package: "edition = \"2024\"\n",
            refused: Some(
                "`default-features = false` cannot override workspace's `default-features`",
            ),
            ..PLAIN
        },
        Case {
            name: "the member's default-features = true turns the root's disabled defaults back on",
            member: &[(
                "reqwest",
                r#"reqwest = { workspace = true, default-features = true }"#,
            )],
            ..PLAIN
        },
        Case {
            name: "optional on the member, even wired into default",
            member: &[(
                "serde_json",
                r#"serde_json = { workspace = true, optional = true }"#,
            )],
            tables: "[features]\ndefault = [\"dep:serde_json\"]\n",
            ..PLAIN
        },
        Case {
            name: "optional in the root",
            root: &[(
                "serde_json",
                r#"serde_json = { version = "{version}", path = "../stubs/serde_json", optional = true }"#,
            )],
            refused: Some("workspace dependencies cannot be optional"),
            ..PLAIN
        },
        Case {
            name: "a root `package` naming the key itself renames nothing (#168)",
            root: &[(
                "bytes",
                r#"bytes = { package = "bytes", version = "{version}", path = "../stubs/bytes" }"#,
            )],
            ..PLAIN
        },
        Case {
            name: "a root `package` naming another crate renames it",
            root: &[(
                "bytes",
                r#"bytes = { package = "bytes-fork", version = "{version}", path = "../stubs/bytes-fork" }"#,
            )],
            ..PLAIN
        },
        // #317: an inheriting line takes only `workspace`, `features`, `default-features` and
        // `optional` (plus `public`); Cargo warns about any other key and ignores it. Only the
        // root's `package` renames an inherited dependency, and only the root's `version` bounds it.
        Case {
            name: "a member `package` beside `workspace = true` is ignored by Cargo",
            member: &[(
                "bytes",
                r#"bytes = { workspace = true, package = "bytes-fork" }"#,
            )],
            ..PLAIN
        },
        Case {
            name: "the same ignored member `package`, on edition 2024",
            member: &[(
                "bytes",
                r#"bytes = { workspace = true, package = "bytes-fork" }"#,
            )],
            package: "edition = \"2024\"\n",
            ..PLAIN
        },
        Case {
            name: "a member `version` beside `workspace = true` is ignored by Cargo",
            member: &[(
                "bytes",
                r#"bytes = { workspace = true, version = "0.0.1" }"#,
            )],
            ..PLAIN
        },
        Case {
            name: "a required crate the member does not inherit",
            member: &[("secrecy", "")],
            ..PLAIN
        },
    ];

    let mut raised = std::collections::BTreeSet::new();
    let mut clean = 0;
    for (index, case) in cases.iter().enumerate() {
        let lookup = |overrides: &[(&str, &'static str)], name: &str| {
            overrides
                .iter()
                .find(|(krate, _)| *krate == name)
                .map(|(_, line)| *line)
        };
        let mut root =
            String::from("[workspace]\nmembers = [\"client\"]\n\n[workspace.dependencies]\n");
        let mut member = format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n{}\n{}\n[dependencies]\n",
            case.package, case.tables
        );
        for dependency in &required {
            match lookup(case.root, dependency.name) {
                Some(line) => root.push_str(&line.replace("{version}", dependency.version)),
                None => root.push_str(&root_line(dependency)),
            }
            root.push('\n');
            match lookup(case.member, dependency.name) {
                Some(line) => member.push_str(line),
                None => member.push_str(&format!("{} = {{ workspace = true }}", dependency.name)),
            }
            member.push('\n');
        }
        let dir = temp.path().join(format!("case-{index}"));
        std::fs::create_dir_all(dir.join("client/src")).unwrap();
        std::fs::write(dir.join("client/src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), &root).unwrap();
        let manifest = dir.join("client/Cargo.toml");
        std::fs::write(&manifest, &member).unwrap();
        let layout = format!("{}:\n{root}\n{member}", case.name);

        let cargo = cargo_view(&manifest, "consumer");
        if let Some(reason) = case.refused {
            let refusal = cargo.expect_err(&format!("Cargo must refuse {layout}"));
            assert!(refusal.contains(reason), "{layout}\n{refusal}");
            continue;
        }
        let cargo = cargo.unwrap_or_else(|refusal| panic!("Cargo must accept {layout}\n{refusal}"));
        let expected = objections_cargo_implies(&requirements, &cargo);

        let preview = spargen::__private::preview_for_macro(
            &Spec::new(spec.clone()),
            manifest.to_str().unwrap(),
        );
        let audited = preview
            .report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract)
            .map(|diagnostic| objection(&diagnostic.message))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            audited, expected,
            "the audit (left) disagrees with what Cargo's resolution implies (right) for {layout}\
             \nCargo resolved: {cargo:#?}"
        );
        if expected.is_empty() {
            clean += 1;
        }
        raised.extend(expected);
    }

    // The table exercises both verdicts and every objection the oracle can derive, so no rule can
    // agree with Cargo merely because no case reaches it.
    assert!(clean >= 3, "{clean} clean layouts");
    let kinds = raised
        .iter()
        .map(std::mem::discriminant)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(kinds.len(), 5, "{raised:#?}");
}

/// Write `manifests` (paths relative to `root`) plus an empty `src/lib.rs` beside each, prove Cargo
/// itself accepts the layout — so the audit is never judged clean on a manifest Cargo would refuse —
/// and return what the `E023` audit says about `member` for `spec`.
fn audit_materialized_layout(
    root: &std::path::Path,
    manifests: &[(&str, String)],
    member: &str,
    spec: &Utf8PathBuf,
) -> spargen::__private::MacroPreview {
    for (path, contents) in manifests {
        let path = root.join(path);
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    let manifest = root.join(member);
    let metadata = query_cargo(manifest.parent().unwrap())
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "Cargo must accept the materialized layout before the audit's verdict on it means \
         anything:\n{}\n{manifests:#?}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    spargen::__private::preview_for_macro(&Spec::new(spec.clone()), manifest.to_str().unwrap())
}

fn assert_audit_clean(preview: &spargen::__private::MacroPreview, layout: &str) {
    let contract = preview
        .report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::RuntimeDependencyContract)
        .collect::<Vec<_>>();
    assert!(
        contract.is_empty(),
        "following `spargen deps` ({layout}) must satisfy the E023 audit:\n{contract:#?}"
    );
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{layout}: {:#?}",
        preview.report
    );
}

/// #158: `spargen deps` is the advice and the `E023` audit is the check, and a consumer who does
/// exactly what the first says must pass the second — in both shapes the advice admits. The
/// requirement set is derived from a real spec through `spargen::requirements` (what backs
/// `spargen deps`), not written by hand, and each layout is audited through the same entry point a
/// build uses on a real manifest on disk, with Cargo confirming the layout is one it accepts.
///
/// - **Direct:** the printed block pasted verbatim into the member, which is what the advice most
///   literally says; then with the blocking opt-in taken as the block describes it (declare the
///   feature, uncomment the dependency).
/// - **Workspace-inherited:** every requirement moved into the root's `[workspace.dependencies]`
///   and the member inheriting each with `workspace = true` in the table the advice names. The one
///   thing that cannot move is `optional = true`: Cargo refuses it in `[workspace.dependencies]`,
///   so it stays on the member's inheriting line, as it must in any real workspace.
#[test]
fn following_spargen_deps_satisfies_the_audit_directly_and_through_workspace_inheritance() {
    let temp = tempfile::tempdir().unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec, EVERY_CAPABILITY_SPEC).unwrap();
    let requirements = spargen::requirements(&Spec::new(spec.clone())).expect("spec lowers");

    // The set is non-trivial: every conditional crate and feature is in it, so neither layout can
    // pass by the table having shrunk to the five core crates.
    let features_of = |name: &str| {
        requirements
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap_or_else(|| panic!("`{name}` missing from {requirements:#?}"))
            .features
            .clone()
    };
    for feature in ["multipart", "stream"] {
        assert!(
            features_of("reqwest").contains(&feature),
            "{requirements:#?}"
        );
    }
    assert!(features_of("bytes").contains(&"serde"), "{requirements:#?}");
    for crate_name in ["futures-core", "quick-xml", "uuid", "time", "tokio"] {
        features_of(crate_name);
    }

    let package = |name: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\n");
    let block = requirements.manifest_block();
    // Opting in is uncommenting every line under the opt-in header: the `[features]` entry and
    // the dependency both come from the printed block, nothing is added by hand.
    let opted_in = block
        .lines()
        .map(|line| match line.strip_prefix("# ") {
            Some(rest) if !rest.starts_with("To opt in") => rest,
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(
        opted_in, block,
        "the blocking opt-in is rendered commented out:\n{block}"
    );
    assert!(
        opted_in.contains("[features]\nblocking = [\"dep:tokio\"]\n"),
        "the printed opt-in carries the blocking feature wiring:\n{block}"
    );
    // The workspace-inherited member below assembles its own dependency lines, so it takes the
    // feature entry `deps` prints rather than restating it.
    let blocking = format!(
        "{}\n",
        opted_in
            .lines()
            .skip_while(|line| *line != "[features]")
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Direct, verbatim. `[workspace]` keeps each fixture its own root, so the walk upward never
    // leaves the temporary directory.
    let direct = temp.path().join("direct");
    let preview = audit_materialized_layout(
        &direct,
        &[(
            "Cargo.toml",
            format!("{}\n{block}\n[workspace]\n", package("direct")),
        )],
        "Cargo.toml",
        &spec,
    );
    assert_audit_clean(&preview, "direct, verbatim");

    // Direct, with the blocking opt-in.
    let direct_blocking = temp.path().join("direct-blocking");
    let preview = audit_materialized_layout(
        &direct_blocking,
        &[(
            "Cargo.toml",
            format!("{}\n{opted_in}\n[workspace]\n", package("direct-blocking")),
        )],
        "Cargo.toml",
        &spec,
    );
    assert_audit_clean(&preview, "direct, blocking opted in");

    // Workspace-inherited, from the same structured requirements `spargen deps --format json`
    // serializes. The root carries every declaration minus `optional`; the member carries only
    // `workspace = true` (plus `optional = true` where required), each under the advice's table.
    let mut root_dependencies = String::from("[workspace.dependencies]\n");
    for dependency in &requirements.dependencies {
        let mut declared = dependency.clone();
        declared.optional = false;
        root_dependencies.push_str(&declared.manifest_line());
        root_dependencies.push('\n');
    }
    let inheriting = |include_opt_in: bool| {
        let mut member = String::new();
        let mut table = None;
        for dependency in requirements
            .dependencies
            .iter()
            .filter(|dependency| include_opt_in || dependency.required_by_feature.is_none())
        {
            if table != Some(dependency.table) {
                member.push_str(&format!("\n[{}]\n", dependency.table));
                table = Some(dependency.table);
            }
            let optional = if dependency.optional {
                ", optional = true"
            } else {
                ""
            };
            member.push_str(&format!(
                "{} = {{ workspace = true{optional} }}\n",
                dependency.name
            ));
        }
        member
    };
    let root = format!("[workspace]\nmembers = [\"client\"]\n\n{root_dependencies}");

    let inherited = temp.path().join("inherited");
    let preview = audit_materialized_layout(
        &inherited,
        &[
            ("Cargo.toml", root.clone()),
            (
                "client/Cargo.toml",
                format!("{}{}", package("inherited"), inheriting(false)),
            ),
        ],
        "client/Cargo.toml",
        &spec,
    );
    assert_audit_clean(&preview, "workspace-inherited");
    assert!(
        preview
            .source_files
            .iter()
            .any(|path| path.as_std_path() == inherited.join("Cargo.toml")),
        "the audit must have resolved the inheritance through the root: {:#?}",
        preview.source_files
    );

    let inherited_blocking = temp.path().join("inherited-blocking");
    let preview = audit_materialized_layout(
        &inherited_blocking,
        &[
            ("Cargo.toml", root),
            (
                "client/Cargo.toml",
                format!(
                    "{}\n{blocking}{}",
                    package("inherited-blocking"),
                    inheriting(true)
                ),
            ),
        ],
        "client/Cargo.toml",
        &spec,
    );
    assert_audit_clean(&preview, "workspace-inherited, blocking opted in");
}

/// A crate that declares `uuid`/`time` as *optional* must be rejected.
///
/// Generated code names `uuid::Uuid` and the date newtypes unconditionally — there is no `cfg` to
/// hide behind — so an optional declaration leaves a feature resolution
/// (`--no-default-features`, or a dependent turning defaults off) in which the generated module
/// references a crate that is not in the graph. The audit checked only the opposite direction, so
/// this shape passed and failed later as a rustc error inside generated code. Both this repo's
/// examples and this test's own fixture shipped it.
#[test]
fn the_manifest_audit_rejects_an_optional_unconditional_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        r#"[package]
name = "optional-consumer"
version = "0.0.0"

[features]
default = ["uuid"]
uuid = ["dep:uuid"]

[dependencies]
bytes = "1.12.1"
reqwest = { version = "0.12.28", default-features = false }
secrecy = "0.10.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
uuid = { version = "1.24.0", features = ["serde"], optional = true }

[workspace]
"#,
    )
    .unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(
        &spec,
        r##"openapi: 3.1.0
info: { title: Ids, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string, format: uuid }
"##,
    )
    .unwrap();

    let preview =
        spargen::__private::preview_for_macro(&Spec::new(spec), manifest.to_str().unwrap());
    assert_eq!(
        preview.report.outcome(),
        Outcome::Rejected,
        "{:#?}",
        preview.report
    );
    let messages = preview
        .report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.code, Code::RuntimeDependencyContract);
            diagnostic.message.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("must not be optional"), "{messages}");
}

/// `W012` fires only where `under_build_script()` is true and no consumer manifest is discoverable
/// — a combination no plain test process can reach, because Cargo always sets
/// `CARGO_MANIFEST_DIR`. A real build script that clears it exercises the branch honestly rather
/// than through a stubbed environment, and proves the run still generates: an un-audited build is
/// degraded, not failed.
#[test]
fn w012_a_build_script_with_no_discoverable_manifest_warns_and_still_generates() {
    let temp = tempfile::tempdir().unwrap();
    let crate_dir = temp.path().join("consumer");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let spargen_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "unauditable-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
bytes = "1.12.1"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"

[build-dependencies]
spargen = {{ path = {spargen_path:?}, default-features = false }}

[workspace]
"#
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("build.rs"),
        r#"fn main() {
    // Cargo always sets these for a build script, which is exactly why the "no manifest" branch
    // is otherwise unreachable. Clearing them here is the honest way to reach it.
    std::env::remove_var("CARGO_MANIFEST_PATH");
    std::env::remove_var("CARGO_MANIFEST_DIR");

    // Resolved at compile time, so clearing the runtime variable above cannot affect it.
    let build = spargen::Spec::new(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.yaml"))
        .build(concat!(env!("OUT_DIR"), "/generated.rs"));
    let report = spargen::generate(&build);
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == spargen::Code::RuntimeAuditSkipped),
        "expected W012: {report:#?}"
    );
    assert!(report.outcome().is_success(), "{report:#?}");
}
"#,
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        crate_dir.join("openapi.yaml"),
        r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /ping:
    get: { operationId: ping, responses: { "204": { description: OK } } }
"#,
    )
    .unwrap();

    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        output.status.success(),
        "build script must warn and still generate:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
