//! Remote references: a remote-`$ref` spec fixture referencing a single vendored schema, plus a
//! helper to lay it out in a tempdir with a hand-written lock + vendored file (no network) and run
//! `generate`/`check`.

use super::refs::two_shape_root;
use super::*;

// The exact bytes of the vendored remote document and their real SHA-256 (see the module test
// asserting spargen's own `sha256` matches this). A mismatch here is a pin-drift fixture.
const GIZMO_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n";
const GIZMO_SHA256: &str = "6d9d14b78ee36c68c62cfbde1e06186a7ded59991eb2f5b6aa8b4503209d8974";
const GIZMO_URL: &str = "https://api.example.com/schemas/gizmo.yaml";
const GIZMO_VENDOR_PATH: &str = "api.example.com/schemas/gizmo.yaml";

fn spec() -> String {
    format!(
        "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /gizmo:\n\
             \x20   get:\n\
             \x20     operationId: getGizmo\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{GIZMO_URL}\"\n"
    )
}

fn lock(sha256: &str) -> String {
    format!(
            "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{sha256}\"\npath = \"{GIZMO_VENDOR_PATH}\"\n"
        )
}

/// Write the spec and (optionally) a lock + vendored file into a fresh tempdir, then run the
/// pipeline. Returns the report and the generated module text (when generation ran).
fn run(
    with_lock: Option<String>,
    with_vendor: Option<&str>,
    check_only: bool,
) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), spec()).unwrap();
    if let Some(lock) = with_lock {
        std::fs::write(dir.join("spargen.lock"), lock).unwrap();
    }
    if let Some(vendor) = with_vendor {
        let path = dir.join(".spargen/vendor").join(GIZMO_VENDOR_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vendor).unwrap();
    }
    let out = dir.join("client.rs");
    let spec = Spec::new(dir.join("openapi.yaml"));
    let report = if check_only {
        run_check(&spec)
    } else {
        run_generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
    };
    (report, temp, out)
}

#[test]
fn unpinned_remote_ref_is_e003_with_remedy() {
    // No lock present ⇒ the remote ref is unpinned. This must be rejected with the *narrowed*
    // E003 and an actionable remedy pointing at `spargen lock`.
    let (report, _temp, _out) = run(None, None, false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let diag = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::AbsoluteRefUnsupported)
        .expect("E003 fires");
    let remedy = diag.remedy.as_deref().unwrap_or_default();
    assert!(
        remedy.contains("spargen lock"),
        "actionable remedy: {remedy}"
    );
    assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
}

/// The one diagnostic `report` carries, which must be `code`, with its pointer and the line its
/// span starts on, after the location oracle has found nothing to report about it.
fn located_at(report: &Report, root: &[u8], code: Code) -> (String, u32) {
    let reported: Vec<Code> = report.diagnostics().iter().map(|d| d.code).collect();
    assert_eq!(reported, [code], "{report:#?}");
    let violations = oracles::location_violations(report.diagnostics(), root);
    assert!(violations.is_empty(), "{violations:#?}");
    let diag = &report.diagnostics()[0];
    let span = diag.span.expect("located at a span");
    assert_eq!(span.file.0, 0, "{diag:?}");
    (diag.pointer.as_str().to_owned(), span.start.line)
}

/// `E003` and `E021` are raised at the `$ref` that names the remote document, with that
/// value's span, not at the referring document's root (#534), through
/// `check` and `generate` alike: unpinned, pinned with the vendored file missing, and pinned
/// with drifted vendored bytes.
#[test]
fn e003_and_e021_point_at_the_remote_ref() {
    let wrong_sha = "0".repeat(64);
    for (code, lock, vendored) in [
        (Code::AbsoluteRefUnsupported, None, None),
        (Code::VendoredRefDrift, Some(lock(GIZMO_SHA256)), None),
        (
            Code::VendoredRefDrift,
            Some(lock(&wrong_sha)),
            Some(GIZMO_YAML),
        ),
    ] {
        for check_only in [true, false] {
            let (report, temp, _) = run(lock.clone(), vendored, check_only);
            let root = std::fs::read(temp.path().join("openapi.yaml")).unwrap();
            assert_eq!(
                located_at(&report, &root, code),
                (
                    "/paths/~1gizmo/get/responses/200/content/application~1json/schema/$ref"
                        .to_owned(),
                    13
                ),
                "{code:?}, check only: {check_only}"
            );
        }
    }
}

/// A remote reference that only a reference into a specification extension reaches is located
/// at its place inside the extension, and one a Security Requirement key spells is located at
/// that key (#534). A `$ref` at the root document's top level is located at its member, so
/// even that one is not reported at the root.
#[test]
fn e003_points_at_a_remote_ref_inside_an_extension_target_and_at_a_requirement_key() {
    let extension = format!(
        "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /gizmo:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: '#/x-defs/Gizmo' }}\n\
             x-defs:\n\
             \x20 Gizmo:\n\
             \x20   $ref: \"{GIZMO_URL}\"\n"
    );
    let requirement = "openapi: 3.1.0\n\
             info: { title: T, version: 1.0.0 }\n\
             security:\n\
             \x20 - https://api.example.com/schemes/key.yaml: []\n\
             paths: {}\n";
    let top_level = format!(
        "$ref: \"{GIZMO_URL}\"\n\
             openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths: {{}}\n"
    );
    for (spec, pointer, line) in [
        (extension.as_str(), "/x-defs/Gizmo/$ref", 14),
        (top_level.as_str(), "/$ref", 1),
        (
            requirement,
            "/security/0/https:~1~1api.example.com~1schemes~1key.yaml",
            4,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
        std::fs::write(&path, spec).unwrap();
        let report = run_check(&Spec::new(path));
        assert_eq!(
            located_at(&report, spec.as_bytes(), Code::AbsoluteRefUnsupported),
            (pointer.to_owned(), line),
            "{spec}"
        );
    }
}

/// A `$ref` is an absolute URI exactly when it opens with an RFC 3986 scheme
/// (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` then `:`), so a non-`http(s)` one is `E003`
/// at its `$ref`, through `check` and `generate` alike, never a file read that fails as `E011`
/// (#720). A scheme with a digit, `+`, `-` or `.` counts, and so does a one-letter one: a
/// Windows drive-letter path such as `C:/specs/x.yaml` is the URI with scheme `c`, which the
/// `E003` explain text states.
#[test]
fn a_ref_with_any_rfc_3986_scheme_is_e003_and_a_drive_letter_is_one() {
    for (reference, uri) in [
        (
            "git+https://example.com/s.yaml#/X",
            "git+https://example.com/s.yaml",
        ),
        ("coap+tcp://h/s.yaml", "coap+tcp://h/s.yaml"),
        ("x-1.a://h/s.yaml", "x-1.a://h/s.yaml"),
        ("urn:foo:bar", "urn:foo:bar"),
        ("C:/specs/x.yaml#/X", "C:/specs/x.yaml"),
        ("c:specs/x.yaml", "c:specs/x.yaml"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let root = spec().replace(GIZMO_URL, reference);
        std::fs::write(dir.join("openapi.yaml"), &root).unwrap();
        let spec = Spec::new(dir.join("openapi.yaml"));
        for check_only in [true, false] {
            let report = if check_only {
                run_check(&spec)
            } else {
                run_generate(
                    &spec
                        .clone()
                        .build(dir.join("client.rs"))
                        .cargo(CargoIntegration::Off),
                )
            };
            assert_eq!(
                located_at(&report, root.as_bytes(), Code::AbsoluteRefUnsupported),
                (
                    "/paths/~1gizmo/get/responses/200/content/application~1json/schema/$ref"
                        .to_owned(),
                    13
                ),
                "{reference}, check only: {check_only}"
            );
            let message = &report.diagnostics()[0].message;
            assert!(
                message.contains(&format!("`{uri}`")),
                "{reference}: {message}"
            );
        }
    }
    let explain = Code::AbsoluteRefUnsupported.explain();
    assert!(
        explain.contains("a Windows drive-letter path such as `C:/specs/x.yaml` is a URI"),
        "{explain}"
    );
}

/// A URL cited from several sites is reported once per site, each at its own `$ref` (#534):
/// the sites' locations differ, so `Diagnostics::emit` no longer merges them into one report
/// per referring file, as it did while every report sat at the root. Unpinned (`E003`) and
/// drifted (`E021`) alike, through `check` and `generate`.
#[test]
fn a_remote_url_cited_from_two_sites_is_reported_at_each() {
    let spec = format!(
        "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /a:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"{GIZMO_URL}\" }}\n\
             \x20 /b:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"{GIZMO_URL}\" }}\n"
    );
    let drifted = lock(&"0".repeat(64));
    for (code, lock, vendor) in [
        (Code::AbsoluteRefUnsupported, None, &[][..]),
        (
            Code::VendoredRefDrift,
            Some(drifted.as_str()),
            &[(GIZMO_VENDOR_PATH, GIZMO_YAML)][..],
        ),
    ] {
        for check_only in [true, false] {
            let (report, _temp, _out) = run_layout(&spec, lock, vendor, check_only);
            let located: Vec<(Code, &str, Option<u32>)> = report
                .diagnostics()
                .iter()
                .map(|d| (d.code, d.pointer.as_str(), d.span.map(|s| s.start.line)))
                .collect();
            assert_eq!(
                located,
                [
                    (
                        code,
                        "/paths/~1a/get/responses/200/content/application~1json/schema/$ref",
                        Some(11)
                    ),
                    (
                        code,
                        "/paths/~1b/get/responses/200/content/application~1json/schema/$ref",
                        Some(19)
                    ),
                ],
                "check only: {check_only}: {report:#?}"
            );
        }
    }
}

#[test]
fn pinned_remote_ref_resolves_hermetically_to_typed_schema() {
    // Lock pins the correct sha256 and the vendored bytes match ⇒ the remote ref resolves with
    // no network and lowers to a typed struct (never `serde_json::Value`).
    let (report, _temp, out) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), false);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::AbsoluteRefUnsupported),
        "{report:#?}"
    );
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    let generated = std::fs::read_to_string(&out).expect("module written");
    assert!(
        generated.contains("id"),
        "the vendored schema's field is emitted:\n{generated}"
    );

    // check/generate parity: `check` resolves the same remote ref, also without network.
    let (checked, _temp2, _out2) = run(Some(lock(GIZMO_SHA256)), Some(GIZMO_YAML), true);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::AbsoluteRefUnsupported),
        "{checked:#?}"
    );
}

#[test]
fn drifted_vendored_content_is_e021() {
    // The vendored bytes are fine, but the lock pins a different sha256 ⇒ the lock is the source
    // of truth, so the drift is refused (E021) rather than silently used.
    let wrong_sha = "0".repeat(64);
    let (report, _temp, _out) = run(Some(lock(&wrong_sha)), Some(GIZMO_YAML), false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
}

#[test]
fn missing_vendored_file_is_e021() {
    // Lock pins the ref but the vendored copy is absent ⇒ drift (nothing to hash against).
    let (report, _temp, _out) = run(Some(lock(GIZMO_SHA256)), None, false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::VendoredRefDrift), "{report:#?}");
}

/// A vendored document's `$self` with a scheme of its own (`urn:x:y`) is its identity as
/// written, wherever the document was retrieved from, as a local document's is: a reference
/// naming `urn:x:y` reaches the document the pinned URL loaded. Joined under the retrieval
/// URL instead (`https://api.example.com/schemas/urn:x:y`), it matches no reference, and the
/// second one is rejected as unpinned (`E003`) (#718).
#[test]
fn a_vendored_documents_opaque_self_is_an_identity_a_reference_reaches() {
    const PETS_URL: &str = "https://api.example.com/schemas/pets.yaml";
    const PETS_YAML: &str = "$self: 'urn:x:y'\n\
                                 components:\n  \
                                 schemas:\n    \
                                 Pet:\n      \
                                 type: object\n      \
                                 properties:\n        \
                                 name:\n          \
                                 type: string\n";
    const PETS_SHA: &str = "e945f51ca0c7b3ff66b5e27e4709f6227172613023e1f62bd06a91f9261daf25";

    // The pinned URL is named first, so the document is loaded before the opaque reference is
    // walked.
    let spec = format!(
        "openapi: 3.2.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /a:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"{PETS_URL}#/components/schemas/Pet\" }}\n\
             \x20 /b:\n\
             \x20   get:\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema: {{ $ref: \"urn:x:y#/components/schemas/Pet\" }}\n"
    );
    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{PETS_URL}\"\nsha256 = \"{PETS_SHA}\"\npath = \
             \"api.example.com/schemas/pets.yaml\"\n"
    );
    let vendor = [("api.example.com/schemas/pets.yaml", PETS_YAML)];

    for check_only in [true, false] {
        let (report, _temp, out) = run_layout(&spec, Some(&lock), &vendor, check_only);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "check only: {check_only}: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AbsoluteRefUnsupported),
            "check only: {check_only}: {report:#?}"
        );
        if !check_only {
            let generated = std::fs::read_to_string(&out).expect("module written");
            assert!(generated.contains("pub name"), "{generated}");
        }
    }
}

/// Lay out `spec` + `lock` + arbitrary vendored files `(vendor-relative path, bytes)` in a
/// fresh tempdir, then run the pipeline (no network). Returns the report and generated module
/// path.
pub(super) fn run_layout(
    spec: &str,
    lock: Option<&str>,
    vendor: &[(&str, &str)],
    check_only: bool,
) -> (Report, tempfile::TempDir, camino::Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), spec).unwrap();
    if let Some(lock) = lock {
        std::fs::write(dir.join("spargen.lock"), lock).unwrap();
    }
    for (rel, content) in vendor {
        let path = dir.join(".spargen/vendor").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
    let out = dir.join("client.rs");
    let spec = Spec::new(dir.join("openapi.yaml"));
    let report = if check_only {
        run_check(&spec)
    } else {
        run_generate(&spec.build(out.clone()).cargo(CargoIntegration::Off))
    };
    (report, temp, out)
}

/// The remote counterpart of the direct-recursive `allOf` member, reached through an **alias**,
/// which is the shape that needs the id-keyed guard rather than the spelling-keyed one.
///
/// `gather_member`'s remote arm has two checks. One keys on the reference *string* —
/// `remote_in_progress.contains_key(reference)` — and the other on the returned `Ty`'s id.
/// Only the second can see this case: `node.yaml` composes
/// `allOf: [alias.yaml]`, `alias.yaml` is a bare `$ref` back to `node.yaml`, so the member's own
/// spelling is never the in-progress key, and `ensure_remote` chains through the alias and hands
/// back a back-edge against `node.yaml`'s reservation.
///
/// **Removing the id-keyed check leaves every other test in the workspace green.** Without it
/// this document generates, with zero diagnostics, and emits
/// `pub type …child = serde_json::Value;` — the same silent degradation the component path
/// guards against, on a path no other test exercises.
#[test]
fn a_direct_recursive_remote_all_of_member_reached_through_an_alias_is_rejected() {
    const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
    const ALIAS_URL: &str = "https://api.example.com/schemas/alias.yaml";
    const NODE_YAML: &str = "type: object\nrequired: [label]\nproperties:\n  label: { type: string }\n  child:\n    allOf:\n      - { $ref: \"alias.yaml\" }\n";
    const ALIAS_YAML: &str = "$ref: \"node.yaml\"\n";
    const NODE_SHA: &str = "09216246cfa803064f874532df513b6458892853137616dd83c386ee3c4a49bd";
    const ALIAS_SHA: &str = "394e78d465e607843bb3b04078679cd2015aea58c67b79b9da1129591d8831a0";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n\n[[remote]]\nurl = \"{ALIAS_URL}\"\nsha256 = \
             \"{ALIAS_SHA}\"\npath = \"api.example.com/schemas/alias.yaml\"\n"
    );
    let vendor = [
        ("api.example.com/schemas/node.yaml", NODE_YAML),
        ("api.example.com/schemas/alias.yaml", ALIAS_YAML),
    ];

    let (generated, _temp, out) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let (checked, _temp2, _out2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        // The pins are live, so the document really reaches lowering rather than being
        // rejected for drift or for being unpinned.
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert!(
            !has_code(report, Code::AbsoluteRefUnsupported),
            "{entry}: {report:#?}"
        );
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: the member resolves to the schema being lowered, whose fields are not \
                 yet known: {report:#?}"
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::AllOfIrreconcilable
                    && d.message.contains("direct recursive")),
            "{entry}: {report:#?}"
        );
    }
    // The degradation itself, so the guard's removal fails on the emitted output and not only
    // on the verdict.
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

/// Two vendored remote documents that are each nothing but a `$ref` to the other. Neither has
/// a body to reserve a root for, so `ensure_remote` chains through them under its own alias
/// guard, and that guard is what reports the loop rather than recursing until the stack ends.
#[test]
fn two_vendored_remote_documents_aliasing_each_other_are_an_alias_cycle() {
    const A_URL: &str = "https://api.example.com/schemas/a.yaml";
    const B_URL: &str = "https://api.example.com/schemas/b.yaml";
    const A_YAML: &str = "$ref: \"b.yaml\"\n";
    const B_YAML: &str = "$ref: \"a.yaml\"\n";
    const A_SHA: &str = "984dfccb3155e113ff9f3e6a18f95eaffe372fdbdcdb4199164d4eaf50e84b43";
    const B_SHA: &str = "eb5e71cf27b9ae8ec27197ca8fe135d1da08e427a4e004e8c308dc9fb5d006ea";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{A_URL}\"\nsha256 = \"{A_SHA}\"\npath = \
             \"api.example.com/schemas/a.yaml\"\n\n[[remote]]\nurl = \"{B_URL}\"\nsha256 = \
             \"{B_SHA}\"\npath = \"api.example.com/schemas/b.yaml\"\n"
    );
    let vendor = [
        ("api.example.com/schemas/a.yaml", A_YAML),
        ("api.example.com/schemas/b.yaml", B_YAML),
    ];
    let (generated, _temp, _out) = run_layout(&responds_with(A_URL), Some(&lock), &vendor, false);
    let (checked, _temp2, _out2) = run_layout(&responds_with(A_URL), Some(&lock), &vendor, true);

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            messages_for(report, Code::UnresolvedRef)
                .iter()
                .any(|m| m.starts_with("remote $ref `") && m.ends_with("` forms an alias cycle")),
            "{entry}: {report:#?}"
        );
    }
}

/// Two vendored remote schemas in mutual recursion through a nullable alias — the remote
/// spelling of the commonest recursive idiom there is, and the one frame of the three whose
/// open reservations no guard could see.
///
/// `node.yaml` has a `parent` of `maybe.yaml`; `maybe.yaml` is nothing but "a `node.yaml`, or
/// null". Both are hash-pinned, so the document is legal and hermetic. The union collapse's
/// escape hatch asked `reservation_at`, which consults the root component map and
/// `resolved_in_progress` and **not** `remote_in_progress`, so the collapse handed
/// `ensure_remote` a foreign id and inserted no def — and `pop_last()` then popped a def that
/// was not this frame's root into an `assert_eq!` that is live in release builds too. The
/// failure was a **process abort**, not a diagnostic: inside a consumer's `build.rs` it is a
/// panicking build script with no code, no pointer and no `--carve` escape, which is the
/// prohibited fourth behaviour in its worst form. A panic is not an `Outcome`, so nothing in
/// this suite could have observed it; this fixture is what observes it.
#[test]
fn a_nullable_alias_between_two_vendored_remote_schemas_generates() {
    const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
    const MAYBE_URL: &str = "https://api.example.com/schemas/maybe.yaml";
    const NODE_YAML: &str =
        "type: object\nproperties:\n  name: { type: string }\n  parent: { $ref: 'maybe.yaml' }\n";
    const MAYBE_YAML: &str = "oneOf:\n  - $ref: 'node.yaml'\n  - type: 'null'\n";
    const NODE_SHA: &str = "b0b0741c3519d771b634a8cee182500b35efbfb9e20e84dd3b4dd62497174569";
    const MAYBE_SHA: &str = "33b37680c6082b1e81bb769383543219f1d1477bca2e9838f7da314631eac815";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n\n[[remote]]\nurl = \"{MAYBE_URL}\"\nsha256 = \
             \"{MAYBE_SHA}\"\npath = \"api.example.com/schemas/maybe.yaml\"\n"
    );
    let vendor = [
        ("api.example.com/schemas/node.yaml", NODE_YAML),
        ("api.example.com/schemas/maybe.yaml", MAYBE_YAML),
    ];

    let (generated, _temp, out) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let (checked, _temp2, _out2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        // The pins are live, so the document really reaches lowering.
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert!(
            !has_code(report, Code::AbsoluteRefUnsupported),
            "{entry}: {report:#?}"
        );
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: a hermetically pinned, legal description: {report:#?}"
        );
        // The reservation must not survive into the finished graph either.
        assert!(
            !has_code(report, Code::InvalidInput),
            "{entry}: {report:#?}"
        );
    }
    // The alias is the target, optional and boxed — what the direct `{$ref: T}` spelling of the
    // same construct already produces and what the support matrix promises for it. Named, and
    // read off `parent`'s own declaration: the embedded runtime supplies an `Option<Box<…>>` of
    // its own to every generated module, so a bare `contains("Option<Box<")` would hold even if
    // this reference had been emitted unboxed.
    assert_eq!(
        field_type(&code, "pub parent").as_deref(),
        Some("Option<Box<HttpsApiExampleComSchemasNodeYaml>>"),
        "{code}"
    );
    assert!(!code.contains("serde_json::Value>"), "{code}");
}

/// The remote spelling of
/// `a_sub_file_nullable_union_component_stays_nullable_at_every_use`: a vendored schema whose
/// body is `oneOf: [null, string]`, used by two required fields. `ensure_remote` wrote the
/// then reserve-time guess's (`schema_is_nullable`'s) provisional `false` back over the union's
/// own `true` and cached it, so both fields were a bare type a `null` would fail to decode
/// into.
#[test]
fn a_vendored_nullable_union_stays_nullable_at_every_use() {
    const MAYBE_URL: &str = "https://api.example.com/schemas/maybe.yaml";
    const MAYBE_YAML: &str = "oneOf:\n  - type: 'null'\n  - type: string\n";
    const MAYBE_SHA: &str = "734c50f67b492acbbf4be1e9e09901db3f69ba14154ce2256cdd9bfe2e039fa7";

    let spec = format!(
        "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /it:\n\
             \x20   get:\n\
             \x20     operationId: getIt\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               type: object\n\
             \x20               required: [a, b]\n\
             \x20               properties:\n\
             \x20                 a: {{ $ref: \"{MAYBE_URL}\" }}\n\
             \x20                 b: {{ $ref: \"{MAYBE_URL}\" }}\n"
    );
    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{MAYBE_URL}\"\nsha256 = \"{MAYBE_SHA}\"\npath = \
             \"api.example.com/schemas/maybe.yaml\"\n"
    );
    let vendor = [("api.example.com/schemas/maybe.yaml", MAYBE_YAML)];

    let (generated, _temp, out) = run_layout(&spec, Some(&lock), &vendor, false);
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let (checked, _temp2, _out2) = run_layout(&spec, Some(&lock), &vendor, true);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        // The pin is live, so the document really reaches lowering.
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for field in ["pub a:", "pub b:"] {
        assert_eq!(
            field_type(&code, field).as_deref(),
            Some("Option<HttpsApiExampleComSchemasMaybeYaml>"),
            "{field}: {code}"
        );
    }
}

/// A vendored remote schema that refers to **itself** directly, with no alias in between: the
/// plainest recursion there is, and the one that reaches `ensure_remote`'s own in-progress arm.
///
/// The root-component and resolved-file frames each have a fixture that pins their back-edge is
/// boxed; the remote frame did not. Flipping that arm to `boxed: false` left the entire
/// workspace green while emitting `pub parent: Option<…NodeYaml>` inside the struct it names —
/// an infinitely sized type. The two remote fixtures beside this one both reach their back-edge
/// through the *alias* path, which boxes at a different site, so neither could see it.
#[test]
fn a_self_recursive_vendored_remote_schema_is_boxed() {
    const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
    const NODE_YAML: &str =
        "type: object\nproperties:\n  name: { type: string }\n  parent: { $ref: 'node.yaml' }\n";
    const NODE_SHA: &str = "119cdb35650ee7b837b855d28d5e3eef2f0b622aba52b5721817ae313aa19300";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \
             \"api.example.com/schemas/node.yaml\"\n"
    );
    let vendor = [("api.example.com/schemas/node.yaml", NODE_YAML)];

    let (generated, _temp, out) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let (checked, _temp2, _out2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::InvalidInput),
            "{entry}: {report:#?}"
        );
    }
    // Boxed, or the generated struct has no finite size and does not compile.
    assert_eq!(
        field_type(&code, "pub parent").as_deref(),
        Some("Option<Box<HttpsApiExampleComSchemasNodeYaml>>"),
        "{code}"
    );
}

/// The remote spelling of `Selfy = Selfy | null`: a vendored document whose **whole body** is a
/// union over a `$ref` back to itself plus `null`.
///
/// This is the one document that reaches `reservation_at`'s `remote_in_progress`
/// canonicalisation, and the only way to reach it. That loop exists because a remote frame is
/// keyed by the URL it was reached through while a provenance canonicalises to a
/// `file#pointer`, so a string comparison between the two can never match; the frame is only
/// *omitted* — rather than merely spelled differently — when the schema being asked about is a
/// remote document's root, which is exactly this shape and nothing else. Every other remote
/// recursion sits at a property or an `allOf` member, whose pointer is not the frame's.
///
/// Without the canonicalisation this document does not reject. The collapse hands
/// `ensure_remote` a foreign id and reserves nothing, and `TypeDefs::fill` is then called on an
/// id that was never reserved — `fill of an unreserved id`, a **process abort** from inside a
/// consumer's `build.rs` with no code, no pointer and no `--carve` escape. A panic is not an
/// `Outcome`, so nothing in this suite could have observed it. That assertion is a
/// `debug_assert!`, so a build with debug assertions off does not abort: it fills the
/// unreserved id and carries on, which is worse and not better.
///
/// The loop had zero iterations across the whole workspace before this fixture, while carrying
/// a paragraph crediting it with a fix the two-file remote fixture passes without. It is this
/// document that the paragraph is true of.
#[test]
fn a_vendored_remote_schema_that_is_a_union_over_itself_is_rejected() {
    const SELFY_URL: &str = "https://api.example.com/schemas/selfy.yaml";
    const SELFY_YAML: &str = "oneOf:\n  - $ref: 'selfy.yaml'\n  - type: 'null'\n";
    const SELFY_SHA: &str = "a85d6698c240a7a1bc9f53f18466c179a2f8fcc01492d822aa913b7a7f4e009a";

    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{SELFY_URL}\"\nsha256 = \"{SELFY_SHA}\"\npath = \
             \"api.example.com/schemas/selfy.yaml\"\n"
    );
    let vendor = [("api.example.com/schemas/selfy.yaml", SELFY_YAML)];

    let (generated, _temp, _out) =
        run_layout(&responds_with(SELFY_URL), Some(&lock), &vendor, false);
    let (checked, _temp2, _out2) =
        run_layout(&responds_with(SELFY_URL), Some(&lock), &vendor, true);

    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        // The pin is live, so the document really reaches lowering rather than stopping at the
        // lock.
        assert!(
            !has_code(report, Code::VendoredRefDrift),
            "{entry}: {report:#?}"
        );
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        // A spec-facing code, the same one the root-document spelling of this shape draws —
        // never the internal invariant's `E011`, and never a process abort.
        assert!(
            has_code(report, Code::NonDisjointUnion),
            "{entry}: {report:#?}"
        );
        assert!(
            !has_code(report, Code::InvalidInput),
            "{entry}: {report:#?}"
        );
    }
}

fn responds_with(url: &str) -> String {
    format!(
        "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n\
             \x20 /it:\n\
             \x20   get:\n\
             \x20     operationId: getIt\n\
             \x20     responses:\n\
             \x20       '200':\n\
             \x20         description: ok\n\
             \x20         content:\n\
             \x20           application/json:\n\
             \x20             schema:\n\
             \x20               $ref: \"{url}\"\n"
    )
}

/// The remote arm of issue #185: an `allOf` member that is a pinned remote `$ref` with
/// shape-bearing siblings contributes the target AND the siblings, exactly as the local arms
/// do (`an_all_of_member_ref_intersects_its_shape_bearing_siblings`). It used to contribute the
/// vendored `Gizmo` alone, deleting `dropped` and generating `id` as the target's `String`
/// over a member that required it to be an integer.
#[test]
fn a_remote_all_of_member_ref_intersects_its_shape_bearing_siblings() {
    let wrapping = |siblings: &str| {
        format!(
            "openapi: 3.1.0\n\
                 info: {{ title: T, version: 1.0.0 }}\n\
                 paths:\n\
                 \x20 /w:\n\
                 \x20   get:\n\
                 \x20     operationId: getW\n\
                 \x20     responses:\n\
                 \x20       '200':\n\
                 \x20         description: ok\n\
                 \x20         content:\n\
                 \x20           application/json:\n\
                 \x20             schema: {{ $ref: '#/components/schemas/Wrap' }}\n\
                 components:\n\
                 \x20 schemas:\n\
                 \x20   Wrap:\n\
                 \x20     allOf:\n\
                 \x20       - {{ $ref: '{GIZMO_URL}', {siblings} }}\n"
        )
    };
    let lock = lock(GIZMO_SHA256);
    let vendor = [(GIZMO_VENDOR_PATH, GIZMO_YAML)];

    let kept = wrapping("properties: { dropped: { type: integer } }, required: [dropped]");
    let (generated, _temp, out) = run_layout(&kept, Some(&lock), &vendor, false);
    let (checked, _temp2, _out2) = run_layout(&kept, Some(&lock), &vendor, true);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    assert_eq!(
        declared_fields(&code, "Wrap"),
        vec!["id", "dropped"],
        "{code}"
    );
    let dropped = field_type(&code, "pub dropped").unwrap_or_default();
    assert!(!dropped.starts_with("Option<"), "{code}");

    let contradicted = wrapping("properties: { id: { type: integer } }, required: [id]");
    for check_only in [false, true] {
        let (report, _temp, _out) = run_layout(&contradicted, Some(&lock), &vendor, check_only);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "check_only={check_only}: {report:#?}"
        );
        let pointers: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AllOfIrreconcilable)
            .map(|d| d.pointer.as_str())
            .collect();
        assert_eq!(
            pointers,
            vec!["/components/schemas/Wrap"],
            "check_only={check_only}: {report:#?}"
        );
    }
}

#[test]
fn self_recursive_remote_schema_generates_boxed_not_stack_overflow() {
    // A vendored remote schema that refers to ITSELF (a linked-list `next`) must terminate at
    // lowering with a boxed back-edge — ordinary OpenAPI — instead of recursing forever.
    const NODE_URL: &str = "https://api.example.com/schemas/node.yaml";
    const NODE_YAML: &str = "type: object\nproperties:\n  id:\n    type: string\n  next:\n    $ref: \"https://api.example.com/schemas/node.yaml\"\n";
    const NODE_SHA: &str = "926f0bc154b93b63208fb4895964ca3e7f67ae3bd7b5f6882156edcefb08fffb";
    let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \"api.example.com/schemas/node.yaml\"\n"
        );
    let vendor = [("api.example.com/schemas/node.yaml", NODE_YAML)];

    let (report, _temp, out) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("Box"),
        "recursion is closed with a boxed field:\n{generated}"
    );

    // check/generate parity: a regression that reintroduces the crash must fail here too, and
    // both must return an outcome rather than aborting.
    let (checked, _t2, _o2) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, true);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

/// The cycle guard's two message arms, pinned SEPARATELY.
///
/// Both said "direct recursive", and that was the only thing asserted, so swapping the local
/// and remote strings was green — which left the LOCAL arm's wording unpinned too: the local
/// case could have told the reader its reference was remote and nothing would have noticed.
/// Each arm now asserts the noun it must use and the noun it must not.
#[test]
fn the_cycle_guard_names_the_right_kind_of_reference_in_each_arm() {
    const NODE_URL: &str = "https://api.example.com/schemas/recursive.yaml";
    // A remote document whose own schema references itself with shape-bearing siblings.
    const NODE_YAML: &str = "type: object\nproperties:\n  next:\n    $ref: \"https://api.example.com/schemas/recursive.yaml\"\n    type: object\n    properties:\n      x:\n        type: string\n";
    const NODE_SHA: &str = "1c720abdd7ea1b336ad3d49b6a0c5a80430c55d28746fd7540d37e4ae0f091d3";
    let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{NODE_URL}\"\nsha256 = \"{NODE_SHA}\"\npath = \"api.example.com/schemas/recursive.yaml\"\n"
        );
    let vendor = [("api.example.com/schemas/recursive.yaml", NODE_YAML)];

    let (report, _t, _o) = run_layout(&responds_with(NODE_URL), Some(&lock), &vendor, false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let remote_messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(remote_messages.len(), 1, "{report:#?}");
    assert!(
        remote_messages[0].contains("remote `$ref`"),
        "the remote arm must say the reference is remote: {:?}",
        remote_messages[0]
    );
    assert!(
        remote_messages[0].contains("closes a reference cycle"),
        "{:?}",
        remote_messages[0]
    );

    // The local arm, against the same guard. Asserting what it must NOT say is what closes the
    // swap: with the two strings exchanged, this message would call a local component's
    // reference remote.
    let local = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    Node:\n      type: object\n      properties:\n        next:\n          $ref: '#/components/schemas/Node'\n          type: object\n          properties: { x: { type: string } }\n";
    let report = generate(local);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let local_messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(local_messages.len(), 1, "{report:#?}");
    assert!(
        !local_messages[0].contains("remote"),
        "a local component reference must not be described as remote: {:?}",
        local_messages[0]
    );
    assert!(
        local_messages[0].contains("back to the schema that encloses it"),
        "the local arm must name the enclosing schema: {:?}",
        local_messages[0]
    );

    // And the two arms must not be the same string: a single shared message would satisfy every
    // assertion above only by accident of wording, and this states the requirement directly.
    assert_ne!(
        local_messages[0], remote_messages[0],
        "the two arms report the same message, so neither is pinned to its own case"
    );
}

#[test]
fn mutually_recursive_remote_docs_generate_boxed() {
    // a.yaml ↔ b.yaml reference each other across two vendored documents; the cross-doc cycle
    // must terminate (boxed) rather than overflow.
    const A_URL: &str = "https://api.example.com/schemas/a.yaml";
    const A_YAML: &str =
        "type: object\nproperties:\n  b:\n    $ref: \"https://api.example.com/schemas/b.yaml\"\n";
    const A_SHA: &str = "bb995ec038973f6ca10fd6674a76a516dc29962fcdf061e1ad49717b2f6e2544";
    const B_URL: &str = "https://api.example.com/schemas/b.yaml";
    const B_YAML: &str =
        "type: object\nproperties:\n  a:\n    $ref: \"https://api.example.com/schemas/a.yaml\"\n";
    const B_SHA: &str = "62d1762eb79467f3a7204c626a7a268647910a218ac4b344a878a6421c300674";
    let lock = format!(
            "version = 1\n\n[[remote]]\nurl = \"{A_URL}\"\nsha256 = \"{A_SHA}\"\npath = \"api.example.com/schemas/a.yaml\"\n\n[[remote]]\nurl = \"{B_URL}\"\nsha256 = \"{B_SHA}\"\npath = \"api.example.com/schemas/b.yaml\"\n"
        );
    let vendor = [
        ("api.example.com/schemas/a.yaml", A_YAML),
        ("api.example.com/schemas/b.yaml", B_YAML),
    ];
    let (report, _temp, out) = run_layout(&responds_with(A_URL), Some(&lock), &vendor, false);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(
        generated.contains("Box"),
        "cross-doc cycle is boxed:\n{generated}"
    );
}

/// Lay out the gizmo spec beside a `spargen.lock` holding `lock`'s bytes and, when given, a
/// vendored gizmo holding `vendored`'s, then run both entry points. Each run's `E011`s are
/// returned as their pointers and messages, with the tempdir's path spelled `<dir>`.
fn invalid_input_with_bytes(lock: &[u8], vendored: Option<&[u8]>) -> Vec<(String, String)> {
    let temp = tempfile::tempdir().unwrap();
    let dir = camino::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(dir.join("openapi.yaml"), spec()).unwrap();
    std::fs::write(dir.join("spargen.lock"), lock).unwrap();
    if let Some(vendored) = vendored {
        let path = dir.join(".spargen/vendor").join(GIZMO_VENDOR_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vendored).unwrap();
    }
    let spec = Spec::new(dir.join("openapi.yaml"));
    let generated = run_generate(&build(dir.join("openapi.yaml"), dir.join("client.rs")));
    let checked = run_check(&spec);
    assert_check_agrees(&generated, &checked);
    let mut found = Vec::new();
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        found.extend(
            report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::InvalidInput)
                .map(|d| {
                    (
                        d.pointer.as_str().to_owned(),
                        d.message.replace(dir.as_str(), "<dir>"),
                    )
                }),
        );
    }
    found
}

/// A vendored file whose bytes match the pin but are not UTF-8 is `E011` naming the URL it
/// was vendored for, not drift: the hash already agrees. It is located at the `$ref` that
/// names the URL, as `E003` and `E021` are, not at the root (#711).
#[test]
fn a_vendored_file_that_is_not_utf8_is_e011() {
    use sha2::{Digest, Sha256};
    let bytes = b"type: object\ndescription: \xff\n";
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let message = format!("vendored file for `{GIZMO_URL}` is not valid UTF-8");
    let pointer = "/paths/~1gizmo/get/responses/200/content/application~1json/schema/$ref";
    assert_eq!(
        invalid_input_with_bytes(lock(&sha256).as_bytes(), Some(bytes)),
        [
            (pointer.to_owned(), message.clone()),
            (pointer.to_owned(), message)
        ]
    );
}

/// A lock that exists but cannot be read as text, and one whose text is not a lock, are each
/// `E011` naming the lock, before any remote reference is resolved against it.
#[test]
fn an_unreadable_or_invalid_lock_is_e011() {
    let unreadable = "failed to read `<dir>/spargen.lock`: stream did not contain valid UTF-8";
    assert_eq!(
        invalid_input_with_bytes(b"version = 1\n\xff\n", None),
        [
            (String::new(), unreadable.to_owned()),
            (String::new(), unreadable.to_owned())
        ]
    );
    let invalid = "invalid spargen.lock: spargen.lock line 2: expected `key = value`";
    assert_eq!(
        invalid_input_with_bytes(b"version = 1\nnot a lock\n", None),
        [
            (String::new(), invalid.to_owned()),
            (String::new(), invalid.to_owned())
        ]
    );
}

#[test]
fn traversal_vendor_path_in_lock_is_rejected_without_reading() {
    // A hand-edited lock whose `path` escapes the vendor dir must be rejected at lock-parse
    // time — before any file is opened — rather than reading an arbitrary file.
    for bad_path in ["../../etc/passwd", "/etc/passwd"] {
        let lock = format!(
                "version = 1\n\n[[remote]]\nurl = \"{GIZMO_URL}\"\nsha256 = \"{GIZMO_SHA256}\"\npath = \"{bad_path}\"\n"
            );
        let (report, _temp, _out) = run(Some(lock), Some(GIZMO_YAML), false);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{bad_path}: {report:#?}"
        );
        assert!(
            has_code(&report, Code::InvalidInput),
            "{bad_path} rejected at parse: {report:#?}"
        );
        // It never reached resolution, so no drift/unpinned diagnostic fires.
        assert!(!has_code(&report, Code::VendoredRefDrift), "{report:#?}");
    }
}

/// A vendored remote document is ranked by its retrieval URL, not by where its copy is
/// vendored. Every type a remote document yields is hinted by its whole reference URL, so two
/// remote documents contest a name only when their URLs spell the same identifier:
/// `a-example.com` and `a.example.com` both become `HttpsAExampleComShapeYaml`. Both are whole
/// documents at the empty pointer, so only the document key tells them apart, and `-` sorts
/// before `.`. The lock vendors the copies in the opposite order (`z/` and `a/`), so ranking by
/// the vendored path would hand the bare name to `a.example.com`'s schema, and ranking by
/// discovery order would move it when `paths` is reordered. Both orders must give it to
/// `a-example.com`'s.
#[test]
fn a_contested_type_name_ranks_a_vendored_remote_document_by_its_url() {
    const DASH_URL: &str = "https://a-example.com/shape.yaml";
    const DOT_URL: &str = "https://a.example.com/shape.yaml";
    const ALPHA_YAML: &str =
        "type: object\nrequired: [alpha]\nproperties: { alpha: { type: string } }\n";
    const BETA_YAML: &str =
        "type: object\nrequired: [beta]\nproperties: { beta: { type: integer } }\n";
    const ALPHA_SHA: &str = "ddc1311e15bb569ce61ed7e650a34cdcbc0a1c123025149aede8df402b86e9b8";
    const BETA_SHA: &str = "ae8c71c6ff85f26973d98c92f545011b417f902b20990dcbf631997fc1de047a";
    const NAME: &str = "HttpsAExampleComShapeYaml";
    let lock = format!(
        "version = 1\n\n[[remote]]\nurl = \"{DASH_URL}\"\nsha256 = \"{ALPHA_SHA}\"\npath = \
             \"z/shape.yaml\"\n\n[[remote]]\nurl = \"{DOT_URL}\"\nsha256 = \"{BETA_SHA}\"\npath = \
             \"a/shape.yaml\"\n"
    );
    let vendor = [("z/shape.yaml", ALPHA_YAML), ("a/shape.yaml", BETA_YAML)];
    for swap in [false, true] {
        let spec = two_shape_root([("a", DASH_URL), ("b", DOT_URL)], swap);
        let (report, _temp, out) = run_layout(&spec, Some(&lock), &vendor, false);
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let code = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            field_owner(&code, "pub alpha:").as_deref(),
            Some(NAME),
            "swap={swap}: the lower retrieval URL keeps the bare name: {code}"
        );
        // The loser's suffix is the hash of its pointer, which is empty for a whole document.
        assert_eq!(
            field_owner(&code, "pub beta:").as_deref(),
            Some("HttpsAexampleComShapeYaml84222325"),
            "swap={swap}: the other document carries the disambiguator: {code}"
        );
    }
}
