//! End to end: generated modules compiled, linted, and driven inside application-owned fixture
//! crates.
//!
//! This file holds the fixture-crate harness, the shared specifications (the three large ones read
//! from `e2e/specs/`), the `BASIC_SPEC` golden files, and the nightly minimal-versions proof the
//! `runtime-dependencies` gate runs by its exact name. Every `insta::assert_snapshot!` sits in
//! this file, because insta names a snapshot after the module path and the source file of the
//! call: from here it is `snapshots/e2e__<name>.snap`. The tests sit in one module per area under
//! `e2e/`, each reaching the harness through `use super::*`.

use std::process::Command;

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Code, Outcome, Spec};

/// A `cargo` invocation that builds the fixture crate at `crate_dir` into [`fixture_target_dir`],
/// the one target directory every fixture of this run shares, so the dependency graph the fixtures
/// have in common (reqwest, tokio, serde, …) is compiled once per run rather than once per fixture.
/// Every nested `cargo` that builds goes through here.
fn fixture_cargo(crate_dir: &std::path::Path) -> Command {
    claim_fixture_name(crate_dir);
    isolated(query_cargo(crate_dir), crate_dir, fixture_target_dir())
}

/// A `cargo` invocation that only reads `crate_dir` (`metadata`, `tree`) and builds nothing. It
/// keeps `crate_dir`'s own `target/`, so its package name need not be unique in the run.
fn query_cargo(crate_dir: &std::path::Path) -> Command {
    isolated(Command::new("cargo"), crate_dir, &crate_dir.join("target"))
}

/// Run `command` in `crate_dir` with its build and target directories set to `target`, overriding
/// whatever this process inherited.
///
/// A nested `cargo` otherwise inherits the caller's `CARGO_TARGET_DIR` (or a `build.target-dir` /
/// `build.build-dir` from Cargo config), and then every fixture crate of every concurrent run shares
/// one artifact set: Cargo hashes a path package relative to its workspace root, so two fixtures
/// with the same package name in different temporary directories get the same `-C metadata`, the
/// same artifact paths, and the same fingerprint. A fixture whose sources were written before
/// another run's same-named build finished is then judged fresh, and runs *that* build: the
/// server-override fixture runs a binary with another run's port baked in, and the `W012` fixture
/// a build script that reads another run's (possibly deleted) spec. Environment variables outrank
/// Cargo config, so setting both here wins over either source.
///
/// [`fixture_cargo`] shares one `target` across the fixtures of one run without reopening that
/// collision: the directory belongs to this run alone ([`fixture_target_dir`]), and no two fixtures
/// of this run share a package name ([`claim_fixture_name`]). Nothing is shared *across* runs, because
/// Cargo's fingerprint cannot tell a same-named fixture of another run from this one's.
fn isolated(
    mut command: Command,
    crate_dir: &std::path::Path,
    target: &std::path::Path,
) -> Command {
    command
        .current_dir(crate_dir)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_BUILD_BUILD_DIR", target);
    command
}

/// The target directory every fixture crate of this run builds into, and no other run's: a fresh
/// `run-<pid>` directory under `e2e-fixtures`, locked for the life of the process so that a later
/// run's [`sweep_finished_runs`] knows it is still in use. The sweep and this run's own directory
/// are made under the root's lock, so a starting run never deletes one that is being set up.
fn fixture_target_dir() -> &'static std::path::Path {
    static DIR: std::sync::LazyLock<(std::path::PathBuf, std::fs::File)> =
        std::sync::LazyLock::new(|| {
            let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e-fixtures");
            std::fs::create_dir_all(&root).unwrap();
            let root_lock = open_lock(&root.join(FIXTURE_RUN_LOCK));
            lock_exclusive(&root_lock);
            sweep_finished_runs(&root);
            let dir = root.join(format!("run-{}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            let lock = open_lock(&dir.join(FIXTURE_RUN_LOCK));
            lock_exclusive(&lock);
            (dir, lock)
        });
    &DIR.0
}

/// Delete every run directory under `root` whose lock can be taken, i.e. whose run has exited.
/// Each run deletes its predecessors' directories, so at most the last run's is left on disk.
///
/// Deletion is best-effort: a run killed outright can leave a nested `cargo` still writing into its
/// directory, and a leftover directory costs only disk, where a failed sweep would fail this run.
fn sweep_finished_runs(root: &std::path::Path) {
    for entry in std::fs::read_dir(root).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let finished = try_lock_exclusive(&open_lock(&dir.join(FIXTURE_RUN_LOCK)));
        if finished {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

const FIXTURE_RUN_LOCK: &str = ".e2e-run.lock";

fn open_lock(path: &std::path::Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap()
}

#[expect(
    clippy::incompatible_msrv,
    reason = "test-only: the msrv gate checks no test target"
)]
fn lock_exclusive(file: &std::fs::File) {
    file.lock().unwrap();
}

/// Take `file`'s exclusive lock if no other handle holds it; `false` when one does.
#[expect(
    clippy::incompatible_msrv,
    reason = "test-only: the msrv gate checks no test target"
)]
fn try_lock_exclusive(file: &std::fs::File) -> bool {
    match file.try_lock() {
        Ok(()) => true,
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(std::fs::TryLockError::Error(error)) => panic!("{error}"),
    }
}

/// Record that `crate_dir`'s package name is built into [`fixture_target_dir`] from `crate_dir`
/// alone. Two fixtures of one run with the same name would share artifacts there; this refuses the
/// second rather than letting it run the first one's build.
fn claim_fixture_name(crate_dir: &std::path::Path) {
    static CLAIMED: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>,
    > = std::sync::LazyLock::new(Default::default);
    let manifest: toml::Table = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let name = manifest["package"]["name"]
        .as_str()
        .expect("a fixture crate `fixture_cargo` builds is a package with a name")
        .to_owned();
    let mut claimed = CLAIMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let owner = claimed
        .entry(name.clone())
        .or_insert_with(|| crate_dir.to_path_buf());
    assert_eq!(
        owner.as_path(),
        crate_dir,
        "fixture package `{name}` is built from two directories; fixtures share one target \
         directory, so each needs a package name of its own"
    );
}

/// The helper is only a guard if nothing goes around it: a bare nested `cargo` inherits the
/// caller's target directory again. Every source file of this suite is read: this one and each
/// module under `e2e/`.
#[test]
fn every_nested_cargo_goes_through_the_isolating_helper() {
    let bare = concat!("Command::new(", "\"cargo\")");
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut sources = vec![tests.join("e2e.rs")];
    for entry in std::fs::read_dir(tests.join("e2e")).expect("tests/e2e/ is readable") {
        let path = entry.expect("readable directory entry").path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            sources.push(path);
        }
    }
    assert!(sources.len() > 1, "tests/e2e/ holds no module: {sources:?}");
    let count: usize = sources
        .iter()
        .map(|path| {
            std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
                .matches(bare)
                .count()
        })
        .sum();
    assert_eq!(
        count, 1,
        "every nested `cargo` must be built by `query_cargo`, which alone may spell `{bare}`"
    );
}

/// `cargo run` the crate `command` is set up for, returning what it printed.
fn run_printed(mut command: Command) -> String {
    let output = command.args(["run", "--quiet"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A binary crate named `same_name` at `root`, printing `value`. Every one of these shares the
/// name, so any two built into one target directory collide unless Cargo rebuilds the second.
fn same_name_crate(root: &std::path::Path, value: &str) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"same_name\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        format!("fn main() {{ print!(\"{value}\"); }}\n"),
    )
    .unwrap();
}

/// Two same-named fixture crates in different directories, both written before either is built,
/// under an inherited shared directory: the second one is judged fresh and runs the first one's
/// binary. The control proves each case reproduces that collision, so the isolated run printing its
/// own value, from the target `isolated` names, is evidence that `isolated` prevents it rather than a
/// vacuous pass. Each variable is inherited on its own, because each alone is enough to share
/// artifacts.
#[test]
fn same_named_fixtures_never_share_an_inherited_target_directory() {
    for inherited in ["CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"] {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let own = temp.path().join("own");
        let first = temp.path().join("first");
        let control = temp.path().join("control");
        let second = temp.path().join("second");
        for (root, value) in [
            (&first, "first"),
            (&control, "control"),
            (&second, "second"),
        ] {
            same_name_crate(root, value);
        }

        // What an unisolated nested `cargo` sees when the suite runs with this variable (or its
        // Cargo config key) pointing at a directory other runs share.
        let inheriting = |crate_dir: &std::path::Path| {
            let mut command = query_cargo(crate_dir);
            command
                .env_remove("CARGO_TARGET_DIR")
                .env_remove("CARGO_BUILD_BUILD_DIR")
                .env(inherited, &shared);
            command
        };

        assert_eq!(run_printed(inheriting(&first)), "first", "{inherited}");
        assert_eq!(
            run_printed(inheriting(&control)),
            "first",
            "under a shared {inherited}, the control must reproduce the collision, or this test \
             proves nothing about `isolated`"
        );
        assert_eq!(
            run_printed(isolated(inheriting(&second), &second, &own)),
            "second",
            "under a shared {inherited}, an isolated fixture must build its own sources, not reuse \
             a same-named crate's artifacts"
        );
        let binary = format!("debug/same_name{}", std::env::consts::EXE_SUFFIX);
        assert!(
            own.join(&binary).is_file(),
            "under a shared {inherited}, an isolated fixture's artifacts must land in the target \
             `isolated` names"
        );
    }
}

/// Within one run, one package name is built from one directory, and a second directory claiming
/// it is refused before it can share the first one's artifacts.
#[test]
fn a_fixture_name_is_claimed_by_one_crate_directory() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    for root in [&first, &second] {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"claim_probe\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
    }

    claim_fixture_name(&first);
    claim_fixture_name(&first);
    let refused = std::panic::catch_unwind(|| claim_fixture_name(&second))
        .expect_err("a second directory claiming `claim_probe` must be refused");
    let message = refused
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("fixture package `claim_probe` is built from two directories"),
        "{message}"
    );
}

/// This run's fixture directory is its own, and stays locked for the whole run, so a run starting
/// alongside it cannot sweep it away.
#[test]
fn the_fixture_target_directory_is_this_runs_alone() {
    let dir = fixture_target_dir();
    assert_eq!(
        dir.file_name().unwrap(),
        format!("run-{}", std::process::id()).as_str()
    );
    assert!(
        !try_lock_exclusive(&open_lock(&dir.join(FIXTURE_RUN_LOCK))),
        "another handle must not take the run lock while this run holds it"
    );
}

/// A starting run deletes the directories of runs that have exited and keeps those of runs still
/// holding their lock.
#[test]
fn sweeping_removes_only_finished_runs() {
    let root = tempfile::tempdir().unwrap();
    let finished = root.path().join("run-finished");
    let live = root.path().join("run-live");
    for dir in [&finished, &live] {
        std::fs::create_dir_all(dir.join("debug")).unwrap();
    }
    drop(open_lock(&finished.join(FIXTURE_RUN_LOCK)));
    let live_lock = open_lock(&live.join(FIXTURE_RUN_LOCK));
    lock_exclusive(&live_lock);

    sweep_finished_runs(root.path());

    assert!(
        !finished.exists(),
        "a finished run's directory must be removed"
    );
    assert!(
        live.join("debug").is_dir(),
        "a live run's directory must be kept"
    );
}

/// Every runtime dependency's tested floor by crate name, as `spargen::requirements` states it for
/// [`EVERY_CAPABILITY_SPEC`], whose table names every dependency the requirement table knows.
///
/// The fixture crate's manifest and the minimal-versions proof both read their floors here, so a
/// floor bump in spargen moves them with it.
fn runtime_floors() -> &'static std::collections::BTreeMap<&'static str, &'static str> {
    static FLOORS: std::sync::OnceLock<std::collections::BTreeMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    FLOORS.get_or_init(|| {
        let temp = tempfile::tempdir().unwrap();
        let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
        std::fs::write(&spec, EVERY_CAPABILITY_SPEC).unwrap();
        spargen::requirements(&Spec::new(spec))
            .expect("spec lowers")
            .dependencies
            .into_iter()
            .map(|dependency| (dependency.name, dependency.version))
            .collect()
    })
}

/// The tested floor of the runtime dependency `name`, from [`runtime_floors`].
fn runtime_floor(name: &str) -> &'static str {
    runtime_floors().get(name).unwrap_or_else(|| {
        panic!(
            "`{name}` is not a runtime dependency: {:#?}",
            runtime_floors()
        )
    })
}

fn generate_fixture_crate(
    spec: &std::path::Path,
    out: &std::path::Path,
    name: &str,
) -> spargen::Report {
    generate_fixture_crate_in_edition(spec, out, name, "2021")
}

/// Generated output is a freestanding module `include!`d into the consumer's crate, so it compiles
/// under the *consumer's* edition, not spargen's. Every fixture above pins edition 2021; this seam
/// exists so a fixture can pin a different one, because an identifier legal in 2021 is not
/// necessarily legal in 2024.
fn generate_fixture_crate_in_edition(
    spec: &std::path::Path,
    out: &std::path::Path,
    name: &str,
    edition: &str,
) -> spargen::Report {
    generate_configured_fixture_crate(spec, out, name, edition, |spec| spec)
}

/// [`generate_fixture_crate_in_edition`] with the `Spec` passed through `configure` first, for a
/// fixture that needs a generation option set.
fn generate_configured_fixture_crate(
    spec: &std::path::Path,
    out: &std::path::Path,
    name: &str,
    edition: &str,
    configure: impl FnOnce(Spec) -> Spec,
) -> spargen::Report {
    std::fs::create_dir_all(out.join("src")).unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.0.0"
edition = "{edition}"

[features]
blocking = ["dep:tokio"]

[dependencies]
bytes = {{ version = "{bytes}", features = ["serde"] }}
futures-core = "{futures_core}"
quick-xml = {{ version = "{quick_xml}", features = ["serialize"] }}
reqwest = {{ version = "{reqwest}", default-features = false, features = ["multipart", "stream"] }}
secrecy = "{secrecy}"
serde = {{ version = "{serde}", features = ["derive"] }}
serde_json = "{serde_json}"
uuid = {{ version = "{uuid}", features = ["serde"] }}
time = {{ version = "{time}", features = ["formatting", "parsing"] }}

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = {{ version = "{tokio}", features = ["rt"], optional = true }}
"#,
            bytes = runtime_floor("bytes"),
            futures_core = runtime_floor("futures-core"),
            quick_xml = runtime_floor("quick-xml"),
            reqwest = runtime_floor("reqwest"),
            secrecy = runtime_floor("secrecy"),
            serde = runtime_floor("serde"),
            serde_json = runtime_floor("serde_json"),
            uuid = runtime_floor("uuid"),
            time = runtime_floor("time"),
            tokio = runtime_floor("tokio"),
        ),
    )
    .unwrap();
    spargen::generate(
        &configure(Spec::new(
            Utf8PathBuf::from_path_buf(spec.to_path_buf()).unwrap(),
        ))
        .build(Utf8PathBuf::from_path_buf(out.join("src/lib.rs")).unwrap())
        // This test process is not a build script; the fixture crate below is compiled by a
        // real `cargo build`, which is where the manifest audit belongs.
        .cargo(CargoIntegration::Off),
    )
}

#[test]
#[ignore = "nightly direct-minimal-versions proof; run by the runtime-dependencies CI job"]
fn runtime_dependency_floors_compile_with_direct_minimal_versions() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("minimum_runtime_client");
    let report = generate_fixture_crate(&spec, &out, "minimum_runtime_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out)
        .args([
            "+nightly",
            "generate-lockfile",
            "-Z",
            "direct-minimal-versions",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "direct-minimal lockfile generation failed"
    );

    let lock: toml::Value =
        toml::from_str(&std::fs::read_to_string(out.join("Cargo.lock")).unwrap()).unwrap();
    let packages = lock["package"].as_array().unwrap();
    // Every floor the requirement table states: the ten crates the fixture manifest declares, so
    // the proof cannot pass by the table having shrunk, and a crate added to it fails here until
    // the fixture declares it too.
    assert_eq!(runtime_floors().len(), 10, "{:#?}", runtime_floors());
    for (&name, &expected) in runtime_floors() {
        assert!(
            packages.iter().any(|package| {
                package["name"].as_str() == Some(name)
                    && package["version"].as_str() == Some(expected)
            }),
            "direct-minimal lock must select {name} {expected}"
        );
    }

    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--locked",
            "--all-features",
            "--",
            "-D",
            "warnings",
            "-W",
            "clippy::expect-used",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the declared runtime floors must compile natively"
    );

    if wasm32_target_installed() {
        let status = fixture_cargo(&out)
            .args([
                "check",
                "--locked",
                "--all-features",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "the declared runtime floors must compile for wasm"
        );
    }
}

/// The module `spargen::generate` writes for [`BASIC_SPEC`] with the `Spec` passed through
/// `configure`, with only the bytes that vary between runs and releases replaced by placeholders:
/// the two build-cache digest lines (the input digest covers the spargen version), and the
/// provenance header's spargen version and spec path (a temporary directory). Every byte below
/// the header is kept, the embedded `support` runtime included.
fn basic_spec_module(configure: impl FnOnce(Spec) -> Spec) -> String {
    spec_module("openapi.yaml", BASIC_SPEC, configure)
}

/// [`basic_spec_module`] over any root document, written as `file_name`.
fn spec_module(file_name: &str, contents: &str, configure: impl FnOnce(Spec) -> Spec) -> String {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join(file_name);
    std::fs::write(&spec, contents).unwrap();
    let out = temp.path().join("lib.rs");
    let spec_path = Utf8PathBuf::from_path_buf(spec).unwrap();
    let report = spargen::generate(
        &configure(Spec::new(spec_path.clone()))
            .build(Utf8PathBuf::from_path_buf(out.clone()).unwrap())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let written = std::fs::read_to_string(&out).unwrap();
    let mut lines = written.split_inclusive('\n');
    let mut expect = |prefix: &str, placeholder: &str| -> String {
        let line = lines.next().unwrap_or_default();
        assert!(
            line.starts_with(prefix),
            "the generated module's header changed shape: expected a line starting {prefix:?}, \
             got {line:?}; update `basic_spec_module`'s normalization"
        );
        format!("{prefix}{placeholder}\n")
    };
    let mut normalized = String::new();
    normalized += &expect("// input-sha256: ", "[input digest]");
    normalized += &expect("// content-sha256: ", "[content digest]");
    normalized += &expect("// @generated by spargen ", "[version]");
    let source = lines.next().unwrap_or_default();
    assert!(
        source.starts_with(&format!("// source: {spec_path}")),
        "the generated module's source line changed shape: {source:?}"
    );
    normalized += &source.replace(spec_path.as_str(), "[spec]");
    normalized.extend(lines);
    normalized
}

/// The complete emitted module for [`BASIC_SPEC`] under the default generation options is a golden
/// file, so a change to any emitted byte — a signature, a derive, a serde attribute, the embedded
/// runtime — is a reviewable diff rather than something only `cargo check` and clippy notice.
///
/// The defaults have both type-mapping features on (`uuid` and `time`). The fixture crate's own
/// cargo features need no golden file of their own: they select `cfg` branches *inside* this text
/// (`blocking`, `wasm32`), so one module covers every feature set it is compiled under.
#[test]
fn basic_spec_module_is_pinned() {
    insta::assert_snapshot!("basic_spec_module", basic_spec_module(|spec| spec));
}

/// The same golden file with both type-mapping features off: `format: uuid` and
/// `format: date-time`/`date` fall back to `String` and the embedded runtime drops its date
/// newtypes. `open_narrowing` is left at its default because no schema in [`BASIC_SPEC`] carries
/// the construct it changes, so enabling it emits the default module byte for byte.
#[test]
fn basic_spec_module_without_type_mapping_features_is_pinned() {
    let module = basic_spec_module(|spec| spec.uuid(false).time(false));
    assert_ne!(
        module,
        basic_spec_module(|spec| spec),
        "turning the type-mapping features off must change the emitted module, or this golden \
         file duplicates the default one"
    );
    insta::assert_snapshot!("basic_spec_module_no_features", module);
}

/// The broad OpenAPI 3.1 specification the basic fixture crate is generated from, compiled, linted
/// and driven over HTTP, and whose emitted module is the golden file above.
const BASIC_SPEC: &str = include_str!("e2e/specs/basic.yaml");

/// The OpenAPI 3.2 specification `generated_module_compiles_in_oas32_crate_with_query_method`
/// compiles and drives over HTTP.
const OAS32_SPEC: &str = include_str!("e2e/specs/oas32.yaml");

/// A spec that puts both date formats in a request body, a response body, and query parameters —
/// the four positions a date value can reach the wire from.
const DATE_SPEC: &str = include_str!("e2e/specs/date.yaml");

#[path = "e2e/basic_blocking.rs"]
mod basic_blocking;
#[path = "e2e/basic_crate.rs"]
mod basic_crate;
#[path = "e2e/basic_defaults.rs"]
mod basic_defaults;
#[path = "e2e/basic_errors.rs"]
mod basic_errors;
#[path = "e2e/collisions.rs"]
mod collisions;
#[path = "e2e/dependencies.rs"]
mod dependencies;
#[path = "e2e/manifest_audit.rs"]
mod manifest_audit;
#[path = "e2e/oas32.rs"]
mod oas32;
#[path = "e2e/open_narrowing.rs"]
mod open_narrowing;
#[path = "e2e/roundtrip.rs"]
mod roundtrip;
#[path = "e2e/shapes.rs"]
mod shapes;
#[path = "e2e/wire.rs"]
mod wire;

/// A spec that turns on every capability the requirement table knows: a multipart body
/// (`reqwest/multipart`), a binary array inside a JSON body (`bytes/serde`), an XML body
/// (`quick-xml`), an event stream (`futures-core` + `reqwest/stream`), `format: uuid` (`uuid`) and
/// `format: date-time` (`time`). The advice round trip below is only as strong as the set it
/// round-trips, so it asserts this spec really derives all of them.
const EVERY_CAPABILITY_SPEC: &str = r##"openapi: 3.1.0
info: { title: Everything, version: 1.0.0 }
paths:
  /json:
    post:
      operationId: postJson
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Stamped" }
      responses:
        "204": { description: ok }
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file: { type: string, format: binary }
      responses:
        "204": { description: ok }
  /binary-array:
    get:
      operationId: binaryArray
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
      operationId: getXml
      responses:
        "200":
          description: ok
          content:
            application/xml:
              schema: { $ref: "#/components/schemas/XmlBody" }
  /events:
    get:
      operationId: events
      responses:
        "200":
          description: events
          content:
            text/event-stream:
              schema: { type: string }
components:
  schemas:
    Stamped:
      type: object
      required: [id, at]
      properties:
        id: { type: string, format: uuid }
        at: { type: string, format: date-time }
    XmlBody:
      type: object
      properties:
        value: { type: string }
"##;

/// Whether the `wasm32-unknown-unknown` target's std is installed, so the wasm gate can run. When
/// `rustup` reports the installed targets and it is absent, the gate self-skips on a developer's
/// machine rather than failing on a toolchain gap; without `rustup` we assume the target is
/// present. Under CI (`CI` set, as GitHub Actions sets it) an absent target panics instead: every
/// job that runs a wasm gate installs the target, so a skip there would report a gate that never
/// ran as passed.
fn wasm32_target_installed() -> bool {
    let installed = match Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == "wasm32-unknown-unknown"),
        _ => true,
    };
    assert!(
        installed || std::env::var_os("CI").is_none(),
        "`CI` is set but the `wasm32-unknown-unknown` target is not installed, so the wasm gate \
         cannot run: add `targets: wasm32-unknown-unknown` to this job's toolchain step"
    );
    installed
}
