use std::process::Command;

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Code, Outcome, Spec};

/// A `cargo` invocation for the fixture crate at `crate_dir`, building into that crate's own
/// `target/`. Every nested `cargo` in this suite goes through here.
fn fixture_cargo(crate_dir: &std::path::Path) -> Command {
    isolated(Command::new("cargo"), crate_dir)
}

/// Run `command` in `crate_dir` with its build and target directories inside `crate_dir`,
/// overriding whatever this process inherited.
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
fn isolated(mut command: Command, crate_dir: &std::path::Path) -> Command {
    let target = crate_dir.join("target");
    command
        .current_dir(crate_dir)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_BUILD_DIR", &target);
    command
}

/// The helper is only a guard if nothing goes around it: a bare nested `cargo` inherits the
/// caller's target directory again.
#[test]
fn every_nested_cargo_goes_through_the_isolating_helper() {
    let bare = concat!("Command::new(", "\"cargo\")");
    let source = include_str!("e2e.rs");
    assert_eq!(
        source.matches(bare).count(),
        1,
        "every nested `cargo` must be built by `fixture_cargo`, which alone may spell `{bare}`"
    );
}

/// Two same-named fixture crates in different directories, both written before either is built,
/// under an inherited shared directory: the second one is judged fresh and runs the first one's
/// binary. The control proves each case reproduces that collision, so the isolated run printing its
/// own value, from its own `target/`, is evidence that `isolated` prevents it rather than a vacuous
/// pass. Each variable is inherited on its own, because each alone is enough to share artifacts.
#[test]
fn same_named_fixtures_never_share_an_inherited_target_directory() {
    let run = |mut command: Command| {
        let output = command.args(["run", "--quiet"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    for inherited in ["CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"] {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let fixture = |value: &str| {
            let root = temp.path().join(value);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"same_name\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n",
            )
            .unwrap();
            std::fs::write(
                root.join("src/main.rs"),
                format!("fn main() {{ print!(\"{value}\"); }}\n"),
            )
            .unwrap();
            root
        };
        let first = fixture("first");
        let control = fixture("control");
        let second = fixture("second");

        // What an unisolated nested `cargo` sees when the suite runs with this variable (or its
        // Cargo config key) pointing at a directory other runs share.
        let inheriting = |crate_dir: &std::path::Path| {
            let mut command = fixture_cargo(crate_dir);
            command
                .env_remove("CARGO_TARGET_DIR")
                .env_remove("CARGO_BUILD_BUILD_DIR")
                .env(inherited, &shared);
            command
        };

        assert_eq!(run(inheriting(&first)), "first", "{inherited}");
        assert_eq!(
            run(inheriting(&control)),
            "first",
            "under a shared {inherited}, the control must reproduce the collision, or this test \
             proves nothing about `isolated`"
        );
        assert_eq!(
            run(isolated(inheriting(&second), &second)),
            "second",
            "under a shared {inherited}, an isolated fixture must build its own sources, not reuse \
             a same-named crate's artifacts"
        );
        let binary = format!("target/debug/same_name{}", std::env::consts::EXE_SUFFIX);
        assert!(
            second.join(&binary).is_file(),
            "under a shared {inherited}, an isolated fixture's artifacts must land in its own \
             `target/`"
        );
    }
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
bytes = {{ version = "1.12.1", features = ["serde"] }}
futures-core = "0.3.32"
quick-xml = {{ version = "0.41.0", features = ["serialize"] }}
reqwest = {{ version = "0.12.28", default-features = false, features = ["json", "multipart", "stream"] }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"
uuid = {{ version = "1.24.0", features = ["serde"] }}
time = {{ version = "0.3.55", features = ["formatting", "parsing"] }}

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = {{ version = "1.53.1", features = ["rt"], optional = true }}
"#
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

/// One operation per runtime name that a `{Operation}Error` could shadow. Each documents an error
/// body, so each emits an error type into the same module as the runtime `pub use`.
const PRELUDE_COLLISION_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /request:
    get: { operationId: request, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /transport:
    get: { operationId: transport, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /header:
    get: { operationId: header, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /protocol:
    get: { operationId: protocol, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /redirect:
    get: { operationId: redirect, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /auth:
    get: { operationId: auth, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
  /stream:
    get: { operationId: stream, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }
"#;

/// An `operationId` that collides with a runtime re-export must still produce a module that
/// compiles. `operationId: request` otherwise emits `pub struct RequestError` beside
/// `pub use support::{… RequestError …}` in the same module, which is `E0255`.
#[test]
fn an_operation_named_after_a_runtime_type_still_compiles() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, PRELUDE_COLLISION_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "collide_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub struct RequestOperationError"),
        "a colliding error type must be widened, not shadowed:\n{generated}"
    );

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(
        status.success(),
        "an operationId matching a runtime re-export must still generate compiling code"
    );
}

/// Issue #356: the inline schema of the path parameter `date` is given the hint name `Date`, and
/// the response's `format: date-time` makes the `types` module import the runtime `Date` and
/// `DateTime`, so the alias otherwise lands beside `use super::{Date, DateTime};` (`E0255`). The
/// named components exercise every other spelling the `types` module brings into scope by `use`
/// or names bare from the prelude.
const TYPES_SCOPE_COLLISION_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /terms/{date}:
    parameters:
      - name: date
        in: path
        required: true
        schema: { type: string }
    get:
      operationId: getTerms
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  at: { type: string, format: date-time }
  /everything:
    get:
      operationId: getEverything
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  date_time: { $ref: '#/components/schemas/DateTime' }
                  serialize: { $ref: '#/components/schemas/Serialize' }
                  deserialize: { $ref: '#/components/schemas/Deserialize' }
                  b_tree_map: { $ref: '#/components/schemas/BTreeMap' }
                  string: { $ref: '#/components/schemas/String' }
                  option: { $ref: '#/components/schemas/Option' }
                  vec: { $ref: '#/components/schemas/Vec' }
                  box: { $ref: '#/components/schemas/Box' }
                  result: { $ref: '#/components/schemas/Result' }
                  day: { type: string, format: date }
                  tags: { type: array, items: { type: string } }
                  extra: { type: object, additionalProperties: { type: string } }
components:
  schemas:
    DateTime: { type: string }
    Serialize: { type: object, properties: { name: { type: string } } }
    Deserialize: { type: object, properties: { name: { type: string } } }
    BTreeMap: { type: object, properties: { name: { type: string } } }
    String: { type: object, properties: { name: { type: string } } }
    Option: { type: object, properties: { name: { type: string } } }
    Vec: { type: object, properties: { name: { type: string } } }
    Box: { type: object, properties: { next: { $ref: '#/components/schemas/Box' } } }
    Result: { type: object, properties: { name: { type: string } } }
"#;

/// A schema whose name the `types` module already uses must be disambiguated like any other clash
/// rather than emitted beside the import or prelude item of that name (issue #356).
#[test]
fn a_schema_named_after_a_name_the_types_module_uses_still_compiles() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, TYPES_SCOPE_COLLISION_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "types_scope_collide");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("use super::{Date, DateTime};"),
        "the fixture must make the `types` module import the runtime date types:\n{generated}"
    );
    for taken in [
        "Date",
        "DateTime",
        "Serialize",
        "Deserialize",
        "BTreeMap",
        "String",
        "Option",
        "Vec",
        "Box",
        "Result",
    ] {
        for item in ["pub type", "pub struct", "pub enum"] {
            assert!(
                !generated.contains(&format!("{item} {taken} ")),
                "a schema must not take `{taken}` from the `types` module's scope:\n{generated}"
            );
        }
    }

    let status = fixture_cargo(&out)
        .args([
            "clippy",
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
        "a schema named after a name the `types` module uses must still generate compiling code"
    );
}

/// One operation per fixed inherent method of `Client` and `BlockingClient` (issue #286). The spec
/// declares a server, so `with_default_server` is emitted too, and the fixture is checked with the
/// `blocking` feature on, so the `BlockingClient` methods (`inner` among them) are compiled.
const CLIENT_METHOD_COLLISION_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /new:
    get: { operationId: new, responses: { "204": { description: ok } } }
  /with-default-server:
    get: { operationId: withDefaultServer, responses: { "204": { description: ok } } }
  /with-client:
    get: { operationId: withClient, responses: { "204": { description: ok } } }
  /with-backend:
    get: { operationId: withBackend, responses: { "204": { description: ok } } }
  /core:
    get: { operationId: core, responses: { "204": { description: ok } } }
  /with-credential:
    get: { operationId: withCredential, responses: { "204": { description: ok } } }
  /without-credential:
    get: { operationId: withoutCredential, responses: { "204": { description: ok } } }
  /inner:
    get: { operationId: inner, responses: { "204": { description: ok } } }
"#;

/// An `operationId` spelling one of the client's own inherent methods must still produce a module
/// that compiles. Operation methods share `impl Client` (and `impl BlockingClient`) with the
/// constructors and accessors, so `operationId: withCredential` otherwise emits a second
/// `pub fn with_credential`, which is `E0592`.
#[test]
fn an_operation_named_after_a_client_method_still_compiles() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, CLIENT_METHOD_COLLISION_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "client_method_collide");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    for method in [
        "new",
        "with_default_server",
        "with_client",
        "with_backend",
        "core",
        "with_credential",
        "without_credential",
        "inner",
    ] {
        assert!(
            generated.contains(&format!("pub async fn {method}_")),
            "operation `{method}` must yield to the client method of that name:\n{generated}"
        );
    }

    let status = fixture_cargo(&out)
        .args([
            "clippy",
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
        "an operationId matching a fixed client method must still generate compiling code"
    );
}

/// A spec whose only binary payload is one documented error body. `ErrorShape::Single` over a
/// `TypeKind::Bytes` is the shape where generated code and the dependency contract can most easily
/// disagree: the body never travels through serde (it is classified by `classify_error_bytes`), so
/// the contract does not require `bytes/serde` — and codegen must not emit anything that does.
const BYTES_ERROR_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blob:
    get:
      operationId: getBlob
      responses:
        "200": { description: ok }
        "404":
          description: raw failure
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
"#;

/// Generated output must compile against exactly the `[dependencies]` block `spargen deps` prints
/// for the same spec — not against a superset. Building the manifest from `requirements()` rather
/// than a fixed fat manifest is the point: a fat manifest hides a missing feature.
#[test]
fn generated_output_compiles_against_exactly_the_dependencies_it_asks_for() {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, BYTES_ERROR_SPEC).unwrap();
    let spec = Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap());

    let requirements = spargen::requirements(&spec).expect("spec lowers");
    let out = temp.path().join("client");
    std::fs::create_dir_all(out.join("src")).unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        format!(
            "[package]\nname = \"deps_exact\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n{}",
            requirements.manifest_block()
        ),
    )
    .unwrap();

    let report = spargen::generate(
        &spec
            .clone()
            .build(Utf8PathBuf::from_path_buf(out.join("src/lib.rs")).unwrap())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(
        status.success(),
        "generated output must compile against the block `spargen deps` prints"
    );
}

/// Whether `name` is a TLS crate, by the same rule the `example` gate applies to each example
/// lockfile in `mise.toml` and `ci.yml`: it names `rustls`, `native-tls`, `openssl` or `webpki`,
/// or ends in `-tls`. The two share a rule so that "no TLS crate" there and "a TLS crate" here
/// mean the same set.
fn is_tls_crate(name: &str) -> bool {
    ["rustls", "native-tls", "openssl", "webpki"]
        .iter()
        .any(|family| name.contains(family))
        || name.ends_with("-tls")
}

/// The distinct package names in this workspace's resolved graph, from `Cargo.lock` as committed
/// (`--locked`, as the `deny` gate audits it), with `features` passed to `cargo tree`.
///
/// `cargo tree` is read rather than `cargo metadata` because it resolves features the way
/// cargo-deny does: `cargo metadata`'s package list keeps the target of a weak `dep?/feature`
/// that nothing enables (reqwest's `quinn`, which depends on `rustls`), so it could report a TLS
/// crate cargo-deny never audits (#187). Every edge kind on every target is read, the scope
/// cargo-deny audits by default.
fn workspace_graph_package_names(features: &[&str]) -> std::collections::BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let output = fixture_cargo(root)
        .args(["tree", "--workspace", "--locked", "--target", "all"])
        .args(["--edges", "normal,build,dev", "--prefix", "none"])
        .args(["--format", "{p}"])
        .args(features)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "`cargo tree {features:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// The `deny` gate audits under `--all-features` because that is what puts a TLS stack in the
/// audited graph, and TLS advisories (RUSTSEC-2026-0285) are found only through one that is
/// there. `the_deny_gate_states_the_feature_scope_it_audits` in `corpus_manifest.rs` pins the
/// flag; this pins the property the flag stands for. Restructuring `remote-fetch`, dropping
/// reqwest's `rustls-tls`, or removing the `rustls` floor would otherwise leave the flag in place
/// and the TLS advisory audit vacuous with every gate green (#227).
///
/// Any TLS crate satisfies it, not `rustls` by name, so a backend swap (to `native-tls`, say)
/// keeps it passing while the audit still sees a TLS stack. The default-features graph is the
/// control: it must contain none, or the matcher proves nothing and the gate comments saying
/// `--all-features` is what brings TLS in are false.
#[test]
fn the_all_features_workspace_graph_carries_a_tls_stack() {
    let tls = |features: &[&str]| -> Vec<String> {
        workspace_graph_package_names(features)
            .into_iter()
            .filter(|name| is_tls_crate(name))
            .collect()
    };

    let audited = tls(&["--all-features"]);
    assert!(
        !audited.is_empty(),
        "the `--all-features` workspace graph the `deny` gate audits carries no TLS crate, so no \
         TLS advisory can fail it"
    );

    let default = tls(&[]);
    assert!(
        default.is_empty(),
        "the default-features workspace graph carries TLS crates {default:?}: `--all-features` is \
         no longer what puts TLS in the audited graph, so this test's control and the `deny` \
         gate's stated rationale no longer hold"
    );
}

#[test]
fn cargo_build_rejects_a_runtime_requirement_below_the_supported_floor() {
    let temp = tempfile::tempdir().unwrap();
    let crate_dir = temp.path().join("consumer");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let spargen_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "unsupported-runtime-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
bytes = "1.12.0"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"

[build-dependencies]
spargen = {{ path = {:?}, default-features = false }}

[workspace]
"#,
            spargen_path
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("build.rs"),
        r#"fn main() {
    let build = spargen::Spec::new("openapi.yaml").build("src/generated.rs");
    let report = spargen::generate(&build);
    for diagnostic in report.diagnostics() {
        eprintln!("{}: {}", diagnostic.code.as_str(), diagnostic.message);
    }
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
}
"#,
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        "include!(\"generated.rs\");\n",
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("openapi.yaml"),
        r#"openapi: 3.1.0
info: { title: Minimal, version: 1.0.0 }
paths: {}
"#,
    )
    .unwrap();

    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        !output.status.success(),
        "unsupported floor unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E023"), "{stderr}");
    assert!(stderr.contains(">=1.12.1, <2.0.0"), "{stderr}");
    assert!(
        !crate_dir.join("src/generated.rs").exists(),
        "a rejected runtime contract must not write generated output"
    );
}

/// The blocking client's `tokio` table is evaluated for the target Cargo is building, read from the
/// `TARGET`/`CARGO_CFG_*` a real build script receives. This is the only test that proves that
/// mapping matches what Cargo actually sets.
#[test]
fn cargo_build_evaluates_the_tokio_table_for_the_target_being_built() {
    let temp = tempfile::tempdir().unwrap();
    let crate_dir = temp.path().join("consumer");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let spargen_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = |tokio_tables: &str| {
        format!(
            r#"[package]
name = "target-table-consumer"
version = "0.0.0"
edition = "2021"

[features]
blocking = ["dep:tokio"]

[dependencies]
bytes = "1.12.1"
reqwest = {{ version = "0.12.28", default-features = false }}
secrecy = "0.10.3"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"

{tokio_tables}
[build-dependencies]
spargen = {{ path = {spargen_path:?}, default-features = false }}

[workspace]
"#
        )
    };
    const TOKIO: &str = r#"tokio = { version = "1.53.1", features = ["rt"], optional = true }"#;
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&format!(
            "[target.'cfg(unix)'.dependencies]\n{TOKIO}\n\n\
             [target.'cfg(windows)'.dependencies]\n{TOKIO}\n"
        )),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("build.rs"),
        r#"fn main() {
    let build = spargen::Spec::new("openapi.yaml").build("src/generated.rs");
    let report = spargen::generate(&build);
    for diagnostic in report.diagnostics() {
        eprintln!("{}: {}", diagnostic.code.as_str(), diagnostic.message);
    }
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
}
"#,
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        "include!(\"generated.rs\");\n",
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("openapi.yaml"),
        r#"openapi: 3.1.0
info: { title: Minimal, version: 1.0.0 }
paths: {}
"#,
    )
    .unwrap();

    // One table per OS family: Cargo applies exactly one of them on any unix or windows host.
    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        output.status.success(),
        "a tokio table applying to the build target must pass the audit:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // A native-only table that applies to no host this test runs on.
    std::fs::remove_file(crate_dir.join("src/generated.rs")).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        manifest(&format!(
            "[target.'cfg(target_os = \"none\")'.dependencies]\n{TOKIO}\n"
        )),
    )
    .unwrap();
    let output = fixture_cargo(&crate_dir).arg("check").output().unwrap();
    assert!(
        !output.status.success(),
        "a tokio table that does not apply to the build target unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E023"), "{stderr}");
    assert!(stderr.contains("does not apply to"), "{stderr}");
    assert!(
        !crate_dir.join("src/generated.rs").exists(),
        "a rejected runtime contract must not write generated output"
    );
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
    for (name, expected) in [
        ("bytes", "1.12.1"),
        ("futures-core", "0.3.32"),
        ("quick-xml", "0.41.0"),
        ("reqwest", "0.12.28"),
        ("secrecy", "0.10.3"),
        ("serde", "1.0.229"),
        ("serde_json", "1.0.151"),
        ("time", "0.3.55"),
        ("tokio", "1.53.1"),
        ("uuid", "1.24.0"),
    ] {
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

#[test]
fn generated_module_compiles_in_basic_oas31_crate() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "basic_client");

    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics()
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(status.success());

    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--all-features",
            "--",
            "-D",
            "warnings",
            "-W",
            "clippy::expect-used",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    // The fixture manifest models the documented dependencies application developers provide.
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains(r#"blocking = ["dep:tokio"]"#),
        "fixture manifest must declare the blocking feature: {manifest}"
    );
    assert!(
        manifest.contains(r#"tokio = { version = "1.53.1", features = ["rt"], optional = true }"#),
        "tokio must be an optional dependency: {manifest}"
    );
    // `blocking` is opt-in and must never be a default feature. `uuid`/`time` are not consumer
    // features at all any more: generated code names them unconditionally, so the dependency audit
    // now requires them non-optional — which leaves this manifest with no `default` list.
    assert!(
        !manifest.contains("default = "),
        "the fixture manifest must declare no default features: {manifest}"
    );
    // The `BlockingClient` and every blocking method are emitted behind `#[cfg(feature = "blocking")]`
    // so a default build compiles them out entirely — there is no `BlockingClient` without the opt-in.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub struct BlockingClient"),
        "BlockingClient must be emitted"
    );
    // the `BlockingClient` is gated on the `blocking` feature AND `not(wasm32)` — its
    // current-thread tokio runtime cannot run on the single-threaded browser, so a wasm build never
    // compiles it (and never pulls tokio) even with the feature enabled.
    assert!(
        generated.contains("#[cfg(all(feature = \"blocking\", not(target_arch = \"wasm32\")))]"),
        "BlockingClient must be gated on the blocking feature and off wasm"
    );

    // A real round-trip driven by a blocking method against a std-thread mock server (the generated
    // crate is not inside an async runtime, so building a `BlockingClient` here is valid). Gated on
    // the `blocking` feature so the default `cargo test` compiles it to nothing.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/blocking.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

// Prove the BlockingClient performs an actual HTTP round-trip: a blocking method drives the async
// dispatch to completion on the owned current-thread runtime and returns the decoded, typed body.
#[test]
fn blocking_method_round_trips_against_a_mock() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let body = r#"{"ok":"yes"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let base = format!("http://{addr}");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_multi().expect("blocking get_multi round-trips");
    assert_eq!(response.status(), 200);
    match response.into_inner() {
        basic_client::GetMultiResponse::Status200(ok) => assert_eq!(ok.ok, "yes"),
        other => panic!("expected Status200, got {other:?}"),
    }

    // The constructors mirror the async client and the inner client is reachable.
    let _ = client.inner();
    let _ = client.core();

    server.join().unwrap();
}

// Selection never falls through past a chosen alternative whose credential fails, so the remedy
// is to unregister it: `without_credential` on the same client reaches the later, fully registered
// alternative over real HTTP, and the request carries that alternative's credential alone.
#[test]
fn without_credential_falls_through_to_a_later_alternative() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_ascii_lowercase();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });
    let failing_bearer = basic_client::Credential::Provider(std::sync::Arc::new(|| {
        Box::pin(async { Err(basic_client::AuthError::new("idp down")) })
            as basic_client::TokenFuture
    }));
    // `getConjunction` is `tenant + bearer`, or `apiKey`, or `mtls + tenant`: all three registered
    // selects the first, whose bearer provider fails before anything is sent — so the mock sees
    // no connection from this call.
    let client = basic_client::BlockingClient::new(&format!("http://{addr}"))
        .unwrap()
        .with_credential("tenant", basic_client::Credential::ApiKey(basic_client::SecretString::from("acme")))
        .with_credential("bearer", failing_bearer)
        .with_credential("apiKey", basic_client::Credential::ApiKey(basic_client::SecretString::from("k3y")));
    match client.get_conjunction() {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialProvider { scheme, .. },
        )) => assert_eq!(scheme, "bearer"),
        other => panic!("expected the selected alternative's provider failure, got {other:?}"),
    }

    let client = client.without_credential("bearer");
    let response = client.get_conjunction().expect("the apiKey alternative is selected and sent");
    assert_eq!(response.status(), 204);
    let request = server.join().unwrap();
    // The second alternative is `apiKey` alone: the still-registered `tenant` belongs only to
    // alternatives that were not selected, so it must not ride along, and neither may a bearer.
    assert!(request.contains("x-api-key: k3y\r\n"), "{request}");
    assert!(!request.contains("x-tenant"), "{request}");
    assert!(!request.contains("authorization"), "{request}");
}

/// A resolver whose lookup never completes. reqwest's `connect_timeout` bounds the whole connector
/// call, name resolution included, so a lookup that hangs is a connect that hangs: it reaches the
/// same `TimedOut` inside the same connect error a blackholed TCP handshake does, without needing a
/// non-routable address that some networks refuse instead of dropping.
struct NeverResolves;

impl reqwest::dns::Resolve for NeverResolves {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(std::future::pending())
    }
}

// A connect that exceeds the client's `connect_timeout` is `TimeoutKind::Connect` — the server never
// accepted a connection — and not `Total`, which says the whole request ran over its budget.
#[test]
fn an_elapsed_connect_timeout_is_classified_as_connect() {
    let http = reqwest::Client::builder()
        .dns_resolver(std::sync::Arc::new(NeverResolves))
        .connect_timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap();
    let client = basic_client::BlockingClient::with_client(http, "http://never-resolves.invalid").unwrap();
    match client.get_multi() {
        Err(basic_client::Error::Timeout(kind)) => {
            assert_eq!(kind, basic_client::TimeoutKind::Connect);
        }
        other => panic!("expected a connect timeout, got {other:?}"),
    }
}

// The total-request budget elapsing is `TimeoutKind::Total`, including when the connection itself
// succeeded — and a connect timeout configured beside it does not change that.
#[test]
fn an_elapsed_total_timeout_is_classified_as_total() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Accept the connection and hold it open without answering, until the client has given up.
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        while stream.read(&mut buf).map(|read| read > 0).unwrap_or(false) {}
    });

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let client = basic_client::BlockingClient::with_client(http, &format!("http://{addr}")).unwrap();
    let outcome = client.get_multi();
    drop(client);
    match outcome {
        Err(basic_client::Error::Timeout(kind)) => {
            assert_eq!(kind, basic_client::TimeoutKind::Total);
        }
        other => panic!("expected a total timeout, got {other:?}"),
    }

    server.join().unwrap();
}

#[test]
fn typed_parameters_follow_openapi_wire_rules() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        let request_line = request.lines().next().unwrap();
        assert!(request_line.starts_with("GET /params/1,2?"), "{request}");
        assert!(request_line.contains("workflow_id=build.yml"), "{request}");
        assert!(request_line.contains("labels=bug&labels=api"), "{request}");
        // A non-exploded array joins with a literal `,`: the delimiter must stay distinguishable
        // from a comma inside a value, which `%2C` would not be.
        assert!(request_line.contains("compact=one,two"), "{request}");
        assert!(request.contains("x-flags: fast,safe\r\n"), "{request}");
        assert!(request.contains("cookie: session=a; session=b\r\n"), "{request}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let workflow_id: basic_client::types::WorkflowId =
        serde_json::from_str(r#""build.yml""#).unwrap();
    let params = basic_client::SerializeParamsParams::default()
        .labels(vec!["bug".to_owned(), "api".to_owned()])
        .compact(vec!["one".to_owned(), "two".to_owned()])
        .session(vec!["a".to_owned(), "b".to_owned()]);
    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .serialize_params(
            vec![1, 2],
            workflow_id,
            vec!["fast".to_owned(), "safe".to_owned()],
            Some(params),
        )
        .unwrap();

    server.join().unwrap();
}

/// The full RFC 6570 style table on one request line, plus path-value encoding.
///
/// The invariant: a style's delimiters are emitted literally and every data byte is percent-encoded,
/// so a joining `,` stays distinguishable from a `,` inside a value — and a path value can never
/// change the route it is spliced into.
#[test]
fn every_parameter_style_serializes_onto_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap().to_owned();
        let (target, _) = request_line
            .trim_start_matches("GET ")
            .split_once(" HTTP/1.1")
            .unwrap();
        let (path, query) = target.split_once('?').unwrap();

        // matrix, `explode: false`: `;name=v1,v2` — the `;`/`=`/`,` are structure, so the `,`
        // inside the value `a,b` must be `%2C` or the two are indistinguishable.
        // label, `explode: false`: a single `.` prefix and comma-joined members (`.x,y`);
        // `explode: true` would be `.x.y`.
        // The `raw` segment carries a `/`, `?`, `#`, and a stray `%`: all four must be encoded, or
        // the request would address a different route entirely.
        assert_eq!(
            path,
            "/styles/;matrix=one,a%2Cb/.x,y/a%2Fb%3Fc%23d%25e",
            "{request_line}"
        );

        let pairs: Vec<&str> = query.split('&').collect();
        // spaceDelimited/pipeDelimited join with a literal `%20`/`%7C`; RFC 6570 has no bare-space
        // or bare-pipe form, so the delimiter is the encoded triple and data bytes are encoded too.
        assert!(pairs.contains(&"space=one%20a%20b"), "{query}");
        assert!(pairs.contains(&"pipe=one%7Ca%7Cb"), "{query}");
        // deepObject: one `name[property]=value` pair per member, brackets literal.
        assert!(pairs.contains(&"deep%5Bkind%5D=wide"), "{query}");
        assert!(pairs.contains(&"deep%5Blimit%5D=3"), "{query}");
        // `allowReserved: true` is the one place a `/` in a query value survives unencoded.
        assert!(pairs.contains(&"reserved=a/b?c"), "{query}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let params = basic_client::SerializeStylesParams::default()
        .space(vec!["one".to_owned(), "a b".to_owned()])
        .pipe(vec!["one".to_owned(), "a|b".to_owned()])
        .deep(basic_client::types::DeepFilter {
            kind: "wide".to_owned(),
            limit: Some(3),
        })
        .reserved("a/b?c".to_owned());
    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .serialize_styles(
            vec!["one".to_owned(), "a,b".to_owned()],
            vec!["x".to_owned(), "y".to_owned()],
            "a/b?c#d%e".to_owned(),
            Some(params),
        )
        .unwrap();

    server.join().unwrap();
}

/// `content:`-typed path and header parameters on the wire.
///
/// A path value is rendered through its media codec and then percent-encoded as one opaque segment,
/// exactly as a schema-typed one is: neither the text value's `/` nor the JSON string member's
/// `/`, `?`, or `#` may leave the segment it is spliced into, so the request stays under the
/// client's base path. Header values are sent verbatim, as every header value is.
#[test]
fn content_typed_path_and_header_parameters_reach_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();

        assert_eq!(
            request_line,
            "GET /v1/content-params/x%2F..%2F..%2Fadmin/\
             %7B%22kind%22%3A%22a%2F..%2Fb%3Fc%23d%22%2C%22limit%22%3A3%7D HTTP/1.1",
            "{request}"
        );
        assert!(request.contains("x-text: x/../../admin\r\n"), "{request}");
        assert!(
            request.contains("x-json: {\"kind\":\"a/../b?c#d\",\"limit\":3}\r\n"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let filter = || basic_client::types::DeepFilter {
        kind: "a/../b?c#d".to_owned(),
        limit: Some(3),
    };
    let client = basic_client::BlockingClient::new(&format!("http://{addr}/v1")).unwrap();
    client
        .serialize_content_params(
            "x/../../admin".to_owned(),
            filter(),
            "x/../../admin".to_owned(),
            filter(),
        )
        .unwrap();

    server.join().unwrap();
}

/// The exact bytes of an `application/x-www-form-urlencoded` body built from an Encoding Object.
///
/// A property that declares `style` switches to RFC 6570 mode (its `contentType` becomes inert);
/// one that does not stays in media-type mode and is rendered by its declared content type.
#[test]
fn form_urlencoded_body_bytes_follow_the_encoding_object() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");

        assert!(
            request.contains("content-type: application/x-www-form-urlencoded\r\n"),
            "{request}"
        );
        // `name` is a plain text property; `tags` declared `pipeDelimited` so it joins with `%7C`;
        // `blob` declared `application/json` and is therefore a JSON document, form-encoded.
        // A space is `%20`, not `+`: the specification defers to query-parameter serialization
        // (RFC 6570), and every urlencoded parser decodes `%20` and `+` alike.
        assert_eq!(
            body,
            "name=Ada%20Lovelace&tags=a%7Cb&blob=%7B%22kind%22%3A%22wide%22%2C%22limit%22%3A3%7D",
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .submit_form(&basic_client::types::RequestBody75618f63 {
            name: "Ada Lovelace".to_owned(),
            tags: vec!["a".to_owned(), "b".to_owned()],
            blob: basic_client::types::DeepFilter {
                kind: "wide".to_owned(),
                limit: Some(3),
            },
        })
        .unwrap();

    server.join().unwrap();
}

/// Every multipart part carries a resolved `Content-Type`: the specification's defaulting table
/// picks `application/octet-stream` for a binary property, `text/plain` for a scalar, and
/// `application/json` for an object or array.
#[test]
fn multipart_parts_carry_their_resolved_content_types() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(
            request.contains("content-type: multipart/form-data; boundary="),
            "{request}"
        );
        for (part, content_type) in [
            ("file", "application/octet-stream"),
            ("caption", "text/plain"),
            ("tags", "application/json"),
        ] {
            let name = format!("name=\"{part}\"");
            let position = request.find(&name).unwrap_or_else(|| panic!("{request}"));
            let rest = &request[position..];
            let header = rest.split("\r\n\r\n").next().unwrap();
            assert!(
                header.contains(&format!("Content-Type: {content_type}")),
                "part `{part}` must declare {content_type}: {header}"
            );
        }

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .upload_file(&basic_client::types::RequestBodyE12d70b5 {
            file: bytes::Bytes::from_static(b"\x00\x01binary"),
            caption: "a caption".to_owned(),
            count: None,
            tags: Some(vec!["x".to_owned()]),
        })
        .unwrap();

    server.join().unwrap();
}

/// A raw byte request body is sent with the `Content-Type` its media key names: the octet gate
/// admits only `bytes::Bytes`, and a `Bytes` body is emitted with the header before the media
/// arms are consulted. This pins that wire fact directly, on the request the server actually reads,
/// for `application/octet-stream` and for a concrete binary family member (`image/png`), which
/// must go out under its own name rather than the generic one.
#[test]
fn a_byte_request_body_declares_its_content_type() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(request.starts_with("PUT /ranged HTTP/1.1"), "{request}");
        assert!(
            request.contains("content-type: application/octet-stream"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\n\
                  Content-Range: bytes 0-2/3\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .put_ranged(&bytes::Bytes::from_static(b"abc"))
        .unwrap();

    server.join().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert!(request.starts_with("PUT /artwork/poster HTTP/1.1"), "{request}");
        assert!(request.contains("content-type: image/png"), "{request}");

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .put_artwork("poster", &bytes::Bytes::from_static(b"\x89PNG"))
        .unwrap();

    server.join().unwrap();
}

#[test]
fn required_path_query_parameter_is_not_shadowed_by_codegen_local() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);

        assert_eq!(
            request.lines().next(),
            Some("GET /files?path=%2Ftmp%2Fexample.txt HTTP/1.1"),
            "{request}"
        );

        stream
            .write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    });

    let client = basic_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .read_file("/tmp/example.txt".to_owned())
        .expect("read_file sends the caller-provided path query value");

    server.join().unwrap();
}

fn serve_once(content_type: &str, status: &str, body: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let content_type = content_type.to_owned();
    let status = status.to_owned();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{addr}"), server)
}

#[test]
fn textual_vendor_and_binary_responses_use_raw_wire_codecs() {
    let (base, server) = serve_once("text/html", "200 OK", b"<p>Hello</p>");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.render_html().unwrap().into_inner(), "<p>Hello</p>");
    server.join().unwrap();

    let (base, server) = serve_once(
        "application/octocat-stream",
        "200 OK",
        b" /\\_/\\\n( o.o )",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.get_octocat().unwrap().into_inner(), " /\\_/\\\n( o.o )");
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "200 OK", b"\0raw\xff");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    assert_eq!(client.download_raw().unwrap().into_inner().as_ref(), b"\0raw\xff");
    server.join().unwrap();
}

#[test]
#[allow(deprecated)]
fn textual_documented_errors_decode_without_json_quotes() {
    let (base, server) = serve_once("text/plain", "400 Bad Request", b"plain failure");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_text_error().unwrap_err() {
        // `.0` unwraps the documented-error newtype; the same value is one `Deref` away, and
        // `String::from(..)` converts. The newtype is what makes `Error<E>` a `std::error::Error`.
        basic_client::Error::Api(response) => {
            assert_eq!(response.into_inner().0, "plain failure")
        }
        other => panic!("expected typed textual API error, got {other:?}"),
    }
    server.join().unwrap();
}

#[test]
fn multi_status_dispatch_uses_each_status_media_codec() {
    let (base, server) = serve_once("text/plain", "200 OK", b"plain success");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap().into_inner() {
        basic_client::GetRawMultiResponse::Status200(body) => {
            assert_eq!(body.as_str(), "plain success")
        }
        other => panic!("expected text success variant, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "201 Created", b"raw success");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap().into_inner() {
        basic_client::GetRawMultiResponse::Status201(body) => {
            assert_eq!(&body[..], b"raw success")
        }
        other => panic!("expected binary success variant, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/octet-stream", "409 Conflict", b"raw failure");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_raw_multi().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetRawMultiError::Status409(body) => {
                assert_eq!(&body[..], b"raw failure")
            }
            other => panic!("expected binary error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #115: with no success status declared, `default` documents a 2xx, so its body is decoded
// as the success value rather than discarded behind `Ok(())`; a non-2xx it covers is still the
// error enum's `Default` variant, and the declared `404` keeps its own.
#[test]
fn default_types_a_2xx_when_no_success_status_is_declared() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"kind":"ok","limit":3}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let body = client.get_no_success().unwrap().into_inner();
    assert_eq!(body.kind, "ok");
    assert_eq!(body.limit, Some(3));
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", br#"{"kind":"boom"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_no_success().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetNoSuccessError::Default(body) => assert_eq!(body.kind, "boom"),
            other => panic!("expected the default error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("text/plain", "404 Not Found", b"gone");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_no_success().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetNoSuccessError::Status404(body) => assert_eq!(body.as_str(), "gone"),
            other => panic!("expected the 404 error variant, got {other:?}"),
        },
        other => panic!("expected typed API error, got {other:?}"),
    }
    server.join().unwrap();
}

// `getMultiDefault` documents 200 and 201 with distinct bodies plus a bodied `default`. The
// matches below are exhaustive with no wildcard, so a `default` routed into the success enum
// fails to compile here; the undocumented 202 proves it is no success fallback at run time either.
#[test]
fn default_beside_multiple_bodied_successes_stays_on_the_error_side() {
    let (base, server) = serve_once("application/json", "201 Created", br#"{"id":7}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap().into_inner() {
        basic_client::GetMultiDefaultResponse::Status200(body) => {
            panic!("201 decoded as the 200 variant: {body:?}")
        }
        basic_client::GetMultiDefaultResponse::Status201(body) => assert_eq!(body.id, 7),
    }
    server.join().unwrap();

    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;
    let (base, server) = serve_once("application/json", "202 Accepted", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 202);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 2xx must not decode as `default`, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_multi_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetMultiDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #151: beside a declared success status, `default` satisfies no undeclared 2xx in any of
// the three success shapes. The enum shape is pinned above (`getMultiDefault`'s 202 is
// `UnexpectedStatus`); here the plain and unit shapes take an undeclared 201 as their one success,
// and a `default`-shaped 201 body is never decoded as `Problem`.
#[test]
fn an_undeclared_2xx_is_never_decoded_through_default_beside_a_declared_success() {
    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;

    // Plain: the 201 is the single success body, `MultiOk`.
    let (base, server) = serve_once("application/json", "201 Created", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_plain_default().expect("any 2xx is the plain success");
    assert_eq!(response.status(), 201);
    assert_eq!(response.into_inner().ok, "yes");
    server.join().unwrap();

    // A `Problem` body on that 201 is a malformed `MultiOk`, not a `default` decode.
    let (base, server) = serve_once("application/json", "201 Created", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_plain_default().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, .. } => {
            assert_eq!(status, 201);
            assert_eq!(headers.get("content-type").unwrap(), "application/json");
            assert_eq!(&body[..], problem);
        }
        other => panic!("a 2xx must decode as the declared success, got {other:?}"),
    }
    server.join().unwrap();

    // Unit: the 201 is `()`, its `Problem` body discarded rather than decoded.
    let (base, server) = serve_once("application/json", "201 Created", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_unit_default().expect("any 2xx is the unit success");
    assert_eq!(response.status(), 201);
    let () = response.into_inner();
    server.join().unwrap();

    // Both still classify a non-2xx through `default`.
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_plain_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetPlainDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_unit_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            let basic_client::GetUnitDefaultError(body) = response.into_inner();
            assert_eq!(body.title, "t");
        }
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();
}

// Issues #127 and #204: a bodyless error entry beside exactly one error body is its own unit
// variant — a bodyless `default`, `403`, or `304` alike — where the single-body newtype dropped it,
// so a documented status arrived as `Error::UnexpectedStatus` or, under a bodied `default`, had its
// empty body decoded as that model. Every match is exhaustive with no wildcard, so a variant
// appearing or vanishing fails to compile.
#[test]
fn a_bodyless_error_entry_beside_one_error_body_is_its_own_variant() {
    let problem: &'static [u8] = br#"{"title":"t","detail":"d"}"#;
    let (base, server) = serve_once("application/json", "404 Not Found", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_default().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 404);
            match response.into_inner() {
                basic_client::GetBodylessDefaultError::Status404(body) => {
                    assert_eq!(body.title, "t")
                }
                basic_client::GetBodylessDefaultError::Default => {
                    panic!("a 404 took the bodyless `default`")
                }
            }
        }
        other => panic!("expected the typed 404 error body, got {other:?}"),
    }
    server.join().unwrap();

    // A `500` is the documented bodyless `default`: `Api`, and its body is never parsed as the
    // `404`'s `Problem`.
    for body in [problem, b"".as_slice()] {
        let (base, server) = serve_once("application/json", "500 Internal Server Error", body);
        let client = basic_client::BlockingClient::new(&base).unwrap();
        match client.get_bodyless_default().unwrap_err() {
            basic_client::Error::Api(response) => {
                assert_eq!(response.status(), 500);
                match response.into_inner() {
                    basic_client::GetBodylessDefaultError::Default => {}
                    basic_client::GetBodylessDefaultError::Status404(body) => {
                        panic!("a 500 decoded as the documented 404: {body:?}")
                    }
                }
            }
            other => panic!("expected the unit `Default` error variant, got {other:?}"),
        }
        server.join().unwrap();
    }

    // The issue's repro: a documented bodyless `403` is `Api(Status403)`, not `UnexpectedStatus`.
    let (base, server) = serve_once("application/json", "403 Forbidden", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 403);
            match response.into_inner() {
                basic_client::GetBodylessSiblingError::Status403 => {}
                basic_client::GetBodylessSiblingError::Status404(body) => {
                    panic!("a 403 decoded as the 404: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Status403` error variant, got {other:?}"),
    }
    server.join().unwrap();
    // The bodied `404` still decodes, and an undocumented `500` is still unexpected.
    let (base, server) = serve_once("application/json", "404 Not Found", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetBodylessSiblingError::Status404(body) => assert_eq!(body.title, "t"),
            basic_client::GetBodylessSiblingError::Status403 => panic!("a 404 took the 403"),
        },
        other => panic!("expected the typed 404 error body, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_sibling().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 500 must stay unexpected, got {other:?}"),
    }
    server.join().unwrap();

    // The same shape with an XML error body: the `404` arm decodes XML, the `403` reads nothing.
    let (base, server) = serve_once(
        "application/xml",
        "404 Not Found",
        b"<XmlReceipt><ReceiptCode>A1</ReceiptCode></XmlReceipt>",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetXmlBodylessSiblingError::Status404(body) => {
                assert_eq!(body.code, "A1")
            }
            basic_client::GetXmlBodylessSiblingError::Status403 => panic!("a 404 took the 403"),
        },
        other => panic!("expected the typed XML 404 error body, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/xml", "403 Forbidden", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_bodyless_sibling().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetXmlBodylessSiblingError::Status403 => {}
            basic_client::GetXmlBodylessSiblingError::Status404(body) => {
                panic!("a 403 decoded as the 404: {body:?}")
            }
        },
        other => panic!("expected the unit `Status403` error variant, got {other:?}"),
    }
    server.join().unwrap();

    // A bodyless `304` beside a bodied `default`: its own variant, the empty body never decoded.
    let (base, server) = serve_once("application/json", "304 Not Modified", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_conditional().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 304);
            match response.into_inner() {
                basic_client::GetConditionalError::Status304 => {}
                basic_client::GetConditionalError::Default(body) => {
                    panic!("a 304 decoded as the `default` body: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Status304` error variant, got {other:?}"),
    }
    server.join().unwrap();
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_conditional().unwrap_err() {
        basic_client::Error::Api(response) => match response.into_inner() {
            basic_client::GetConditionalError::Default(body) => assert_eq!(body.title, "t"),
            basic_client::GetConditionalError::Status304 => panic!("a 500 took the 304"),
        },
        other => panic!("expected the typed `default` error body, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_bodyless_default_multi().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 500);
            match response.into_inner() {
                basic_client::GetBodylessDefaultMultiError::Default => {}
                basic_client::GetBodylessDefaultMultiError::Status404(body)
                | basic_client::GetBodylessDefaultMultiError::Status409(body) => {
                    panic!("a 500 decoded as a documented status: {body:?}")
                }
            }
        }
        other => panic!("expected the unit `Default` error variant, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #121: `getMaybeEmpty` documents a bodied 200 and a bodyless 204. The 204 is its own unit
// variant, read without parsing the empty body, where a plain `MultiOk` returned `Error::Decode`.
// The matches are exhaustive with no wildcard, so a variant appearing or vanishing fails to compile.
#[test]
fn a_bodyless_success_beside_one_body_is_its_own_variant() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_maybe_empty().unwrap().into_inner() {
        basic_client::GetMaybeEmptyResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        basic_client::GetMaybeEmptyResponse::Status204 => panic!("a 200 decoded as the 204"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "204 No Content", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_maybe_empty().expect("a documented 204 is a success");
    assert_eq!(response.status(), 204);
    match response.into_inner() {
        basic_client::GetMaybeEmptyResponse::Status204 => {}
        basic_client::GetMaybeEmptyResponse::Status200(body) => {
            panic!("a 204 decoded as the 200: {body:?}")
        }
    }
    server.join().unwrap();

    // An undocumented 2xx matches neither variant: preserved raw, never decoded as `MultiOk`.
    let (base, server) = serve_once("application/json", "202 Accepted", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_maybe_empty().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, .. } => assert_eq!(status, 202),
        other => panic!("expected an unexpected-status error, got {other:?}"),
    }
    server.join().unwrap();

    // The XML variant of the same shape decodes its one body through the XML codec.
    let (base, server) = serve_once(
        "application/xml",
        "200 OK",
        b"<XmlReceipt><ReceiptCode>A1</ReceiptCode></XmlReceipt>",
    );
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_maybe_empty().unwrap().into_inner() {
        basic_client::GetXmlMaybeEmptyResponse::Status200(body) => assert_eq!(body.code, "A1"),
        basic_client::GetXmlMaybeEmptyResponse::Status204 => panic!("a 200 decoded as the 204"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/xml", "204 No Content", b"");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_xml_maybe_empty().unwrap().into_inner() {
        basic_client::GetXmlMaybeEmptyResponse::Status204 => {}
        basic_client::GetXmlMaybeEmptyResponse::Status200(body) => {
            panic!("a 204 decoded as the 200: {body:?}")
        }
    }
    server.join().unwrap();
}

// `getRanged` documents an exact 200 and an overlapping 2XX range with different bodies. A 200
// matches both and must take the exact arm; a 202 matches only the range. Executed, not just
// ordered in the emitted text.
#[test]
fn success_dispatch_takes_the_exact_arm_before_an_overlapping_range() {
    let (base, server) = serve_once("application/json", "200 OK", br#"{"ok":"yes"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap().into_inner() {
        basic_client::GetRangedResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        basic_client::GetRangedResponse::Status2xx(body) => {
            panic!("a 200 took the range arm: {body:?}")
        }
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "202 Accepted", br#"{"id":7}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    let response = client.get_ranged().unwrap();
    assert_eq!(response.status(), 202);
    match response.into_inner() {
        basic_client::GetRangedResponse::Status2xx(body) => assert_eq!(body.id, 7),
        basic_client::GetRangedResponse::Status200(body) => {
            panic!("a 202 took the exact 200 arm: {body:?}")
        }
    }
    server.join().unwrap();

    // A body the matched arm cannot parse is `Error::Decode` at that status, with the headers
    // (#268) and the body kept.
    let (base, server) = serve_once("text/plain", "202 Accepted", b"not json");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_ranged().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, .. } => {
            assert_eq!(status, 202);
            assert_eq!(headers.get("content-type").unwrap(), "text/plain");
            assert_eq!(&body[..], b"not json");
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();
}

// Issue #128: the error-side counterpart of the test above, driven through the emitted dispatch
// rather than a hand-written stand-in. `getErrorRanged` documents an exact 409 and an overlapping
// 4XX range, declared range first. A 409 matches both and must take the exact arm; a 404 matches
// only the range; a 500 matches neither; and a body the matched arm cannot parse is `Decode`. The
// matches are exhaustive with no wildcard, so a variant appearing or vanishing fails to compile.
#[test]
fn error_dispatch_takes_the_exact_arm_before_an_overlapping_range() {
    let (base, server) = serve_once("application/json", "409 Conflict", br#"{"detail":"dup"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 409);
            match response.into_inner() {
                basic_client::GetErrorRangedError::Status409(body) => assert_eq!(body.detail, "dup"),
                basic_client::GetErrorRangedError::Status4xx(body) => {
                    panic!("a 409 took the range arm: {body:?}")
                }
            }
        }
        other => panic!("expected a typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let (base, server) = serve_once("application/json", "404 Not Found", br#"{"reason":"gone"}"#);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Api(response) => {
            assert_eq!(response.status(), 404);
            match response.into_inner() {
                basic_client::GetErrorRangedError::Status4xx(body) => assert_eq!(body.reason, "gone"),
                basic_client::GetErrorRangedError::Status409(body) => {
                    panic!("a 404 took the exact 409 arm: {body:?}")
                }
            }
        }
        other => panic!("expected a typed API error, got {other:?}"),
    }
    server.join().unwrap();

    let problem: &'static [u8] = br#"{"reason":"boom","detail":"boom"}"#;
    let (base, server) = serve_once("application/json", "500 Internal Server Error", problem);
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::UnexpectedStatus { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(&body[..], problem);
        }
        other => panic!("an undocumented 500 must not decode as a documented body, got {other:?}"),
    }
    server.join().unwrap();

    // Issue #268: a documented status whose body does not decode (here an HTML page, as a proxy
    // in front of the server would send) keeps its status and headers on `Decode`.
    let (base, server) = serve_once("text/html", "409 Conflict", b"not json");
    let client = basic_client::BlockingClient::new(&base).unwrap();
    match client.get_error_ranged().unwrap_err() {
        basic_client::Error::Decode { status, headers, body, truncated, .. } => {
            assert_eq!(status, 409);
            assert_eq!(headers.get("content-type").unwrap(), "text/html");
            assert_eq!(&body[..], b"not json");
            assert!(!truncated);
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    server.join().unwrap();
}
"##,
    )
    .unwrap();

    // Prove the wired serde defaults actually deserialize: an absent optional field with a
    // representable scalar default fills in the default instead of `None`, while a required field
    // (default rustdoc-only) still comes from the payload.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/defaults.rs"),
        r##"
#[test]
fn absent_optional_fields_use_schema_defaults() {
    let settings: basic_client::types::Settings =
        serde_json::from_str(r#"{"retries": 7}"#).unwrap();
    assert_eq!(settings.color.as_deref(), Some("red"));
    assert_eq!(settings.enabled, Some(true));
    assert_eq!(settings.ratio, Some(1.5));
    assert_eq!(settings.retries, 7);
    assert_eq!(settings.mode, Some(basic_client::types::Mode::Auto));
}

#[test]
fn an_uninhabited_optional_field_drops_its_members_default() {
    // Absent, the field stays absent: no default provider fabricates a value no type admits.
    let merged: basic_client::types::ConflictDefault = serde_json::from_str("{}").unwrap();
    assert!(merged.x.is_none());
    let sibling: basic_client::types::ConflictDefaultSibling = serde_json::from_str("{}").unwrap();
    assert!(sibling.x.is_none());
    // Present, no value decodes: neither member's type is the field's.
    assert!(serde_json::from_str::<basic_client::types::ConflictDefault>(r#"{"x": "a"}"#).is_err());
    assert!(serde_json::from_str::<basic_client::types::ConflictDefault>(r#"{"x": 1}"#).is_err());
    assert!(
        serde_json::from_str::<basic_client::types::ConflictDefaultSibling>(r#"{"x": "a"}"#)
            .is_err()
    );
}

#[test]
fn a_narrowing_meet_retypes_or_drops_its_members_default() {
    // Absent, `valid` takes the default as the enum's variant and `ratio` as an integer, on both
    // spellings of the meet; `bad` and `fraction` admit no default of the narrowed type.
    let merged: basic_client::types::NarrowDefault = serde_json::from_str("{}").unwrap();
    let sibling: basic_client::types::NarrowDefaultSibling = serde_json::from_str("{}").unwrap();
    for (valid, bad, ratio, fraction) in [
        (merged.valid, merged.bad, merged.ratio, merged.fraction),
        (sibling.valid, sibling.bad, sibling.ratio, sibling.fraction),
    ] {
        assert_eq!(serde_json::to_string(&valid).unwrap(), r#""a""#);
        assert!(bad.is_none());
        assert_eq!(ratio, Some(3_i64));
        assert!(fraction.is_none());
    }
    // The wider member keeps its own defaults as it wrote them.
    let base: basic_client::types::NarrowDefaultBase = serde_json::from_str("{}").unwrap();
    assert_eq!(base.valid.as_deref(), Some("a"));
    assert_eq!(base.bad.as_deref(), Some("zzz"));
    assert_eq!(base.fraction, Some(2.5));
}

// An optional uninhabited field is `Option<Never>`, and serde's `Option<T>` maps a JSON `null` to
// `None` without ever calling `T::deserialize` — so without a field-level deserializer the
// uninhabited type is never consulted and `{"x": null}`, which no schema here admits, decodes and
// re-serialises as `{}`. Every spelling that produces such a field is held to the same five rows:
// absent and unknown-only documents decode (and round-trip to `{}`), and any present value —
// `null` included — is rejected.
fn assert_only_absence_decodes<T>(type_name: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug,
{
    for valid in ["{}", r#"{"zz": 1}"#] {
        let value: T = serde_json::from_str(valid)
            .unwrap_or_else(|error| panic!("{type_name}: {valid} must decode: {error}"));
        assert_eq!(serde_json::to_string(&value).unwrap(), "{}", "{type_name}: {valid}");
    }
    for invalid in [r#"{"x": 1}"#, r#"{"x": "s"}"#, r#"{"x": null}"#] {
        let decoded = serde_json::from_str::<T>(invalid);
        assert!(
            decoded.is_err(),
            "{type_name}: {invalid} names a value no type admits, yet decoded as {decoded:?}"
        );
    }
}

#[test]
fn an_uninhabited_optional_field_admits_only_absence() {
    assert_only_absence_decodes::<basic_client::types::ConflictDefault>("ConflictDefault");
    assert_only_absence_decodes::<basic_client::types::ConflictDefaultSibling>(
        "ConflictDefaultSibling",
    );
    assert_only_absence_decodes::<basic_client::types::ForbiddenProperty>("ForbiddenProperty");
}

#[test]
fn a_nullable_uninhabited_field_still_admits_null() {
    // `null` is the one value a nullable `false` schema admits, so there the `Option` is the type.
    let present: basic_client::types::NullOnlyProperty =
        serde_json::from_str(r#"{"x": null}"#).unwrap();
    assert!(present.x.is_none());
    assert!(serde_json::from_str::<basic_client::types::NullOnlyProperty>(r#"{"x": 1}"#).is_err());
}

// serde's `Option<T>` maps a JSON `null` to `None` without calling `T::deserialize`, so an
// optional non-nullable field would decode `{"name": null}` as absent and re-serialise it as `{}`:
// a value the schema does not admit, accepted and then silently rewritten. A present value, `null`
// included, is decoded as the field's own type instead.
#[test]
fn an_optional_non_nullable_field_rejects_a_present_null() {
    use basic_client::types::OptionalFields;
    for field in ["name", "count", "flag", "tags", "mode", "nested", "choice", "colour"] {
        let document = format!(r#"{{"{field}": null}}"#);
        let decoded = serde_json::from_str::<OptionalFields>(&document);
        assert!(
            decoded.is_err(),
            "{document}: `{field}` is not nullable, yet decoded as {decoded:?}"
        );
    }
    // Absence is still `None` (or the schema default), and serialises back to absence.
    let absent: OptionalFields = serde_json::from_str("{}").unwrap();
    assert!(absent.name.is_none() && absent.count.is_none() && absent.choice.is_none());
    assert_eq!(absent.colour.as_deref(), Some("red"));
    // A present, well-typed value still decodes.
    let present: OptionalFields = serde_json::from_str(
        r#"{"name": "n", "count": 2, "flag": false, "tags": [], "mode": "manual", "nested": {},
            "choice": 3, "colour": "blue"}"#,
    )
    .unwrap();
    assert_eq!(present.name.as_deref(), Some("n"));
    assert_eq!(present.count, Some(2));
    assert_eq!(present.colour.as_deref(), Some("blue"));
    // An untyped property admits `null` as a value of its own, and keeps it present on the wire.
    let untyped: OptionalFields = serde_json::from_str(r#"{"anything": null}"#).unwrap();
    assert_eq!(untyped.anything, Some(serde_json::Value::Null));
    assert_eq!(
        serde_json::to_value(&untyped).unwrap(),
        serde_json::json!({"anything": null, "colour": "red"})
    );
    // A nullable optional property is where `null` and absence both mean `None`.
    let nullable: OptionalFields = serde_json::from_str(r#"{"maybe": null}"#).unwrap();
    assert!(nullable.maybe.is_none());
}

#[test]
fn pattern_properties_capture_into_typed_overflow_map() {
    // The declared `host` field is typed; every non-declared property is captured by the flatten
    // `BTreeMap<String, String>` overflow that `patternProperties` lowered to.
    let headers: basic_client::types::Headers =
        serde_json::from_str(r#"{"host": "h", "x-a": "1", "x-b": "2"}"#).unwrap();
    assert_eq!(headers.host.as_deref(), Some("h"));
    assert_eq!(headers.additional.get("x-a").map(String::as_str), Some("1"));
    assert_eq!(headers.additional.get("x-b").map(String::as_str), Some("2"));
}

#[test]
fn null_mixed_enum_field_is_option_of_enum() {
    // The null-mixed `Priority` enum lowered to a real Rust enum used behind `Option`: an absent
    // field and an explicit `null` both deserialize to `None`; a string value to the variant.
    let absent: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n"}"#).unwrap();
    assert_eq!(absent.priority, None);

    let explicit_null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": null}"#).unwrap();
    assert_eq!(explicit_null.priority, None);

    let set: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": "high"}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
}

#[test]
fn all_of_compatible_constraints_keep_the_narrow_typed_intersection() {
    let json = serde_json::json!({
        "run_id": 7,
        "status": "queued",
        "marker": null,
        "labels": ["linux", "x64"],
        "steps": [{"name": "build"}],
        "empty_only": [],
    });
    let refined: basic_client::types::Refined = serde_json::from_value(json.clone()).unwrap();

    // `number & integer` is emitted as an integer, and exact JSON null is Rust unit.
    assert_eq!(refined.run_id, 7_i64);
    assert_eq!(refined.marker, ());
    assert_eq!(serde_json::to_value(refined).unwrap(), json);

    let invalid = serde_json::json!({
        "run_id": 7,
        "status": "queued",
        "marker": null,
        "labels": ["linux"],
        "steps": [{"name": "build"}],
        "empty_only": [null],
    });
    assert!(serde_json::from_value::<basic_client::types::Refined>(invalid).is_err());
}

#[test]
fn overlapping_unions_enforce_one_of_and_canonicalize_any_of() {
    let string: basic_client::types::AnyString =
        serde_json::from_str(r#""special""#).unwrap();
    assert!(matches!(
        string,
        basic_client::types::AnyString::StringLiteral(_)
    ));

    let number: basic_client::types::AnyNumber = serde_json::from_str("7").unwrap();
    assert!(matches!(
        number,
        basic_client::types::AnyNumber::AnyNumberVariant1(_)
    ));

    let owner: basic_client::types::AnyOwner =
        serde_json::from_str(r#"{"id":7}"#).unwrap();
    assert!(matches!(
        owner,
        basic_client::types::AnyOwner::DetailedOwner(_)
    ));

    // Both branches accept `special`, so oneOf rejects it. A manually constructed broad branch is
    // revalidated during serialization and rejected for the same reason.
    assert!(serde_json::from_str::<basic_client::types::OneOverlap>(r#""special""#).is_err());
    let ambiguous = basic_client::types::OneOverlap::OneOverlapVariant0(Box::new(
        "special".to_owned(),
    ));
    assert!(serde_json::to_value(ambiguous).is_err());
    assert!(serde_json::from_str::<basic_client::types::OneOverlap>(r#""other""#).is_ok());
}

#[test]
fn mixed_discriminator_dispatches_arrays_by_category_and_objects_by_tag() {
    let directory: basic_client::types::MixedContent =
        serde_json::from_str(r#"["README.md"]"#).unwrap();
    assert!(matches!(
        directory,
        basic_client::types::MixedContent::MixedContentVariant0(_)
    ));

    let file: basic_client::types::MixedContent =
        serde_json::from_str(r#"{"type":"file","content":"hello"}"#).unwrap();
    assert!(matches!(
        file,
        basic_client::types::MixedContent::ContentFile(_)
    ));
    assert_eq!(
        serde_json::to_value(file).unwrap(),
        serde_json::json!({"type": "file", "content": "hello"})
    );
}

#[test]
fn component_nullability_propagates_through_ref() {
    // A REQUIRED field referencing the nullable `Priority` component is `Option<Priority>`: the key
    // must be present, but `null` deserializes to `None` and a string to the variant. This only
    // holds if the component's nullability propagated to the `$ref` use site.
    let null_priority: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": null, "history": []}"#).unwrap();
    assert_eq!(null_priority.priority, None);

    // An array of the nullable component is `Vec<Option<Priority>>`: a `null` element is accepted.
    let set: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": "high", "history": ["low", null]}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
    assert_eq!(
        set.history,
        vec![Some(basic_client::types::Priority::Low), None]
    );
}

#[test]
fn all_of_merged_struct_carries_every_member_field() {
    // `Account` merged a `$ref` base (id, required), an inline member (label, required) and a
    // sibling property (owner, optional). Required fields are plain, the optional is `Option`, and a
    // payload carrying all three deserializes into the single flattened struct.
    let account: basic_client::types::Account =
        serde_json::from_str(r#"{"id": "a1", "label": "L", "owner": "o"}"#).unwrap();
    assert_eq!(account.id, "a1");
    assert_eq!(account.label, "L");
    assert_eq!(account.owner.as_deref(), Some("o"));
}

#[test]
fn discriminated_union_round_trips_with_tag() {
    // Cat DECLARES `petType` as a required property — the shape that broke serde internal tagging
    // ("missing field petType"). The custom buffer-to-Value Deserialize hands the WHOLE value to the
    // variant, so Cat's own `pet_type` field is filled, and re-serialization keeps the tag.
    let pet: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "cat", "name": "Whiskers"}"#).unwrap();
    match &pet {
        basic_client::types::Pet::Cat(cat) => {
            assert_eq!(cat.name, "Whiskers");
            assert_eq!(cat.pet_type, "cat");
        }
        other => panic!("expected Cat variant, got {other:?}"),
    }
    let json = serde_json::to_value(&pet).unwrap();
    assert_eq!(json["petType"], "cat");
    assert_eq!(json["name"], "Whiskers");

    // Dog does NOT declare `petType`; the custom Serialize re-inserts the tag it would otherwise
    // lack, and deserialization still routes by the tag.
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "dog", "bark": true}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    let json = serde_json::to_value(&dog).unwrap();
    assert_eq!(json["petType"], "dog");
    assert_eq!(json["bark"], true);

    // Every value that names a member selects it (#263): the second mapping key `kitty`, and each
    // component name, which no mapping key claims. Cat keeps the tag it was decoded with in its own
    // field; Dog re-serializes with its first mapping key, `dog`.
    for tag in ["kitty", "Cat"] {
        let pet: basic_client::types::Pet = serde_json::from_value(
            serde_json::json!({"petType": tag, "name": "Whiskers"}),
        )
        .unwrap();
        match &pet {
            basic_client::types::Pet::Cat(cat) => assert_eq!(cat.pet_type, tag),
            other => panic!("{tag}: expected Cat variant, got {other:?}"),
        }
        assert_eq!(serde_json::to_value(&pet).unwrap()["petType"], tag);
    }
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "Dog", "bark": false}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    assert_eq!(serde_json::to_value(&dog).unwrap()["petType"], "dog");
    assert!(
        serde_json::from_str::<basic_client::types::Pet>(r#"{"petType": "cow", "bark": true}"#)
            .is_err()
    );
}

#[test]
fn an_untagged_discriminated_member_leaves_the_tagged_dispatch_in_place() {
    use basic_client::types::LooseAnimal;
    // `Cat` (two required fields) outranks `Dog` in a trial, and accepts this payload too; the tag
    // names `Dog`, so `Dog` it is.
    let dog: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Dog", "name": "Rex", "bark": true}"#).unwrap();
    assert!(matches!(dog, LooseAnimal::Dog(_)), "{dog:?}");
    // `Dog` declares no `petType`, so serialization re-inserts its tag.
    assert_eq!(serde_json::to_value(&dog).unwrap()["petType"], "Dog");
    let cat: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Cat", "name": "Tom"}"#).unwrap();
    assert!(matches!(cat, LooseAnimal::Cat(_)), "{cat:?}");
    // A tag naming a tagged member never falls through to the untagged one.
    assert!(
        serde_json::from_str::<LooseAnimal>(r#"{"petType": "Dog", "fins": 3}"#).is_err()
    );
    // An unrecognized or absent tag tries the untagged member by its schema, and it writes no tag
    // of its own beyond the field it holds.
    let fish: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Shark", "fins": 3}"#).unwrap();
    assert!(matches!(fish, LooseAnimal::LooseAnimalVariant2(_)), "{fish:?}");
    assert_eq!(
        serde_json::to_value(&fish).unwrap(),
        serde_json::json!({"petType": "Shark", "fins": 3})
    );
    assert!(serde_json::from_str::<LooseAnimal>(r#"{"bark": true}"#).is_err());
}

#[test]
fn nullable_variant_union_resolves_null_at_option() {
    // A `null` payload resolves at the outer `Option` (variant nullability hoisted to the union),
    // and non-null string/array content routes to the right disjoint variant and re-serializes as a
    // bare value.
    let null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": null}"#).unwrap();
    assert!(null.notes.is_none());

    let text: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": "hi"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&text.notes).unwrap(),
        serde_json::json!("hi")
    );

    let list: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": ["a", "b"]}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&list.notes).unwrap(),
        serde_json::json!(["a", "b"])
    );
}

#[test]
fn disjoint_union_round_trips_without_wrapper() {
    // A `string` payload deserializes to the string variant and re-serializes as a BARE string —
    // no tag, no wrapper (strategy B custom Serialize).
    let text: basic_client::types::StringOrList =
        serde_json::from_str(r#""hello""#).unwrap();
    assert_eq!(serde_json::to_string(&text).unwrap(), r#""hello""#);

    // An `array` payload deserializes to the array variant and re-serializes as a bare array.
    let list: basic_client::types::StringOrList =
        serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(serde_json::to_string(&list).unwrap(), r#"["a","b"]"#);
}

#[test]
fn multi_status_response_enums_carry_typed_variants() {
    // the two success statuses lowered to a `GetMultiResponse` enum and the two error
    // statuses to a `GetMultiError` enum, each variant carrying that status's typed body. The
    // variants deserialize their bodies (the same `serde_json::from_slice` the generated dispatch
    // runs after selecting by HTTP status), proving the types are real and payload-carrying — not
    // `serde_json::Value`.
    let ok: basic_client::types::MultiOk = serde_json::from_str(r#"{"ok":"yes"}"#).unwrap();
    match basic_client::GetMultiResponse::Status200(Box::new(ok)) {
        basic_client::GetMultiResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        other => panic!("expected Status200, got {other:?}"),
    }
    let created: basic_client::types::MultiCreated =
        serde_json::from_str(r#"{"id":7}"#).unwrap();
    match basic_client::GetMultiResponse::Status201(Box::new(created)) {
        basic_client::GetMultiResponse::Status201(body) => assert_eq!(body.id, 7),
        other => panic!("expected Status201, got {other:?}"),
    }
    // The documented bodyless 204 is a payload-free unit variant (carries no body).
    assert!(matches!(
        basic_client::GetMultiResponse::Status204,
        basic_client::GetMultiResponse::Status204
    ));

    let not_found: basic_client::types::NotFoundError =
        serde_json::from_str(r#"{"reason":"gone"}"#).unwrap();
    match basic_client::GetMultiError::Status404(Box::new(not_found)) {
        basic_client::GetMultiError::Status404(body) => assert_eq!(body.reason, "gone"),
        other => panic!("expected Status404, got {other:?}"),
    }
    let conflict: basic_client::types::ConflictError =
        serde_json::from_str(r#"{"detail":"dup"}"#).unwrap();
    match basic_client::GetMultiError::Status409(Box::new(conflict)) {
        basic_client::GetMultiError::Status409(body) => assert_eq!(body.detail, "dup"),
        other => panic!("expected Status409, got {other:?}"),
    }
}

#[test]
fn multipart_body_struct_has_typed_form_part_fields() {
    // the multipart/form-data body lowered to a typed struct whose fields are the form
    // parts. The binary `file` part is `bytes::Bytes` (its `serde` impls compile only because the
    // synthesized Cargo.toml enabled bytes' `serde` feature), `caption` a required `String`, and the
    // optional `count`/`tags` are `Option`. Constructing the value proves the field types; the
    // generated `upload_file` method (compiled here) builds the `reqwest::multipart::Form` from it,
    // which compiles only with reqwest's `multipart` feature enabled.
    let body = basic_client::types::RequestBodyE12d70b5 {
        file: bytes::Bytes::from_static(b"hello"),
        caption: "a caption".to_owned(),
        count: Some(3),
        tags: Some(vec!["x".to_owned(), "y".to_owned()]),
    };
    assert_eq!(&body.file[..], b"hello");
    assert_eq!(body.caption, "a caption");
    assert_eq!(body.count, Some(3));
    assert_eq!(body.tags.as_deref(), Some(&["x".to_owned(), "y".to_owned()][..]));
}

#[test]
fn streaming_op_item_type_is_typed_not_json_value() {
    // the SSE `/chat/stream` response schema lowered to a real `ChatChunk` type — the
    // streamed item of the `EventStream<ChatChunk>` the `stream_chat` method returns (that signature
    // and the embedded runtime `EventStream` are compile-verified by this crate's build). The item
    // type is a typed struct, never `serde_json::Value`; deserializing a frame the way the runtime's
    // `next` does proves it.
    let chunk: basic_client::types::ChatChunk =
        serde_json::from_str(r#"{"delta": "hi"}"#).unwrap();
    assert_eq!(chunk.delta, "hi");
}

#[test]
fn xml_body_types_carry_attribute_and_rename() {
    // the XML request/response bodies lowered to typed structs whose serde wire names
    // honor the `xml` hints — `XmlOrder.id` is an attribute (`xml.attribute` → serde `@id`), `sku` a
    // child element, and `XmlReceipt.code` is renamed via `xml.name` to `ReceiptCode`. The generated
    // crate depends on quick-xml (proving the conditional `xml` feature was enabled in its
    // Cargo.toml), so this exercises the same codec the `submit_order` method's `to_xml`/decode use.
    let order = basic_client::types::XmlOrder {
        id: 42,
        sku: "ABC".to_owned(),
    };
    let xml = quick_xml::se::to_string(&order).unwrap();
    assert!(xml.contains("id=\"42\""), "{xml}");
    assert!(xml.contains("<sku>ABC</sku>"), "{xml}");

    let receipt: basic_client::types::XmlReceipt =
        quick_xml::de::from_str("<XmlReceipt><ReceiptCode>OK</ReceiptCode></XmlReceipt>").unwrap();
    assert_eq!(receipt.code, "OK");
    assert_eq!(receipt.note, None);
    // A present optional element is decoded as the field's own type through its present-value
    // deserializer, not through quick-xml's `Option` handling.
    let noted: basic_client::types::XmlReceipt = quick_xml::de::from_str(
        "<XmlReceipt><ReceiptCode>OK</ReceiptCode><note>late</note></XmlReceipt>",
    )
    .unwrap();
    assert_eq!(noted.note.as_deref(), Some("late"));
}

#[test]
fn json_only_schema_with_xml_metadata_keeps_original_json_names() {
    // regression guard: `JsonMeta` carries `xml.attribute`/`xml.name` hints but is used
    // only by a JSON operation, so the format-agnostic serde rename is SUPPRESSED. JSON must use the
    // original `id`/`sku` names — deserializing a normal server payload succeeds and re-serializing
    // produces the same names (never `@id`/`ProductSku`), proving JSON is uncorrupted.
    let parsed: basic_client::types::JsonMeta =
        serde_json::from_str(r#"{"id": 5, "sku": "Z9"}"#).unwrap();
    assert_eq!(parsed.id, 5);
    assert_eq!(parsed.sku, "Z9");
    let back = serde_json::to_string(&parsed).unwrap();
    assert!(back.contains(r#""id":5"#), "{back}");
    assert!(back.contains(r#""sku":"Z9""#), "{back}");
    assert!(!back.contains("@id"), "{back}");
    assert!(!back.contains("ProductSku"), "{back}");
}

#[test]
fn optional_params_construct_via_fluent_setters() {
    // each optional param on a `…Params` struct gets a `#[must_use]` consuming setter
    // named after its field, taking the field's inner `T` (never `Option<T>`) and storing `Some`.
    // `getUser` has an ordinary optional query param (`page` → `Option<i64>`) and a NULLABLE optional
    // one (`filter`, `type: [integer, "null"]` → `Option<i64>`); the setter for the nullable param
    // must still take the bare `i64`. Building via `default().setter(x)` must compile and set fields.
    let params = basic_client::GetUserParams::default()
        .page(2)
        .filter(7);
    assert_eq!(params.page, Some(2));
    assert_eq!(params.filter, Some(7));

    // Back-compat: the struct still derives `Default` and keeps public fields, so the pre-existing
    // struct-literal form is unchanged.
    let literal = basic_client::GetUserParams {
        page: Some(2),
        ..Default::default()
    };
    assert_eq!(literal.filter, None);
}

#[test]
fn generated_support_module_exposes_link_paginator() {
    // the generic Link-header paginator is a runtime helper re-exported at the crate root
    // (`basic_client::LinkPaginator` / `basic_client::next_link`), so a generated client can drive
    // Link/RFC-8288 pagination with no per-operation codegen. Constructing one via
    // `client.core().paginate_links::<T>(url)` compiles under clippy -D warnings, proving the
    // embedded `support::paginate` module is present and wired.
    let client = basic_client::Client::new("https://api.example.com").unwrap();
    let first = reqwest::Url::parse("https://api.example.com/items?page=1").unwrap();
    let pages: basic_client::LinkPaginator<Vec<i64>> = client.core().paginate_links(first);
    assert!(pages.has_next());

    // The pure `rel="next"` header helper is exposed too: no `Link` header → no next page.
    let mut headers = reqwest::header::HeaderMap::new();
    assert!(basic_client::next_link(&headers).is_none());
    headers.insert(
        reqwest::header::LINK,
        r#"<https://api.example.com/items?page=2>; rel="next""#
            .parse()
            .unwrap(),
    );
    assert_eq!(
        basic_client::next_link(&headers).unwrap().as_str(),
        "https://api.example.com/items?page=2"
    );
}

#[test]
fn custom_http_backend_plugs_into_non_generic_client() {
    // the transport seam is re-exported at the crate root
    // (`basic_client::HttpBackend` / `ExecuteFuture` / `ReqwestBackend`), so a consumer can
    // implement their own transport and plug it via `Client::with_backend` WITHOUT `Client`
    // becoming generic. A trivial backend compiles (under clippy -D warnings) and constructs a
    // client. This test only exercises construction, so the transport is never polled — the runtime
    // crate's own tests prove that dispatch actually routes through the installed backend.
    #[derive(Debug)]
    struct TestBackend;
    impl basic_client::HttpBackend for TestBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TestBackend);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();

    // Back-compat: the pre-existing `new` / `with_client` constructors still work and install the
    // default reqwest-backed transport.
    let _default = basic_client::Client::new("https://api.example.com").unwrap();
    let _byo = basic_client::Client::with_client(
        reqwest::Client::new(),
        "https://api.example.com",
    )
    .unwrap();

    // The default backend type is nameable and usable as an `HttpBackend` too.
    let _reqwest_backend: std::sync::Arc<dyn basic_client::HttpBackend> =
        std::sync::Arc::new(basic_client::ReqwestBackend::new(reqwest::Client::new()));
}

#[test]
fn retry_backend_wraps_an_inner_backend() {
    // the retry adapter is re-exported at the crate root (`basic_client::RetryBackend`
    // / `RetryPolicy` / `RetryOutcome` / `exponential_backoff`). A consumer implements a policy
    // that decides retry AND supplies the wait (bring-your-own timing — no tokio in the runtime),
    // wraps their backend in a `RetryBackend`, and installs it via `Client::with_backend`, all
    // without `Client` becoming generic. This construction test compiles under clippy -D warnings;
    // the runtime crate's own tests prove the retry loop actually retries.
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    struct BackoffPolicy;
    impl basic_client::RetryPolicy for BackoffPolicy {
        fn retry<'a>(
            &'a self,
            attempt: u32,
            outcome: &basic_client::RetryOutcome<'_>,
        ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
            if attempt < 3 && outcome.is_transient() {
                // A real policy would await the caller's timer here (e.g. tokio::time::sleep); a
                // ready future keeps this construction test runtime-free.
                let _wait = basic_client::exponential_backoff(
                    attempt,
                    Duration::from_millis(50),
                    Duration::from_secs(2),
                );
                Some(Box::pin(std::future::ready(())))
            } else {
                None
            }
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let retry = basic_client::RetryBackend::new(inner, std::sync::Arc::new(BackoffPolicy));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(retry);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}

#[test]
fn middleware_backend_wraps_an_inner_backend() {
    // the interceptor middleware is re-exported at the crate root
    // (`basic_client::Middleware` / `Next` / `MiddlewareBackend`). A consumer implements a trivial
    // header-injecting middleware, layers it onto a `MiddlewareBackend`, and installs the whole
    // chain via `Client::with_backend` — all without `Client` becoming generic. This construction
    // test compiles under clippy -D warnings; the runtime crate's own tests prove the chain
    // actually observes/modifies/short-circuits and composes in order.
    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    // A middleware that inserts a header on the way in, then proceeds to the rest of the chain via
    // `Next::run`. Modifying the request before `run` and returning `run`'s future directly is the
    // simplest shape; the trait's `'a` ties the borrow of `self`, the `Next`, and the boxed future.
    #[derive(Debug)]
    struct InjectHeader;
    impl basic_client::Middleware for InjectHeader {
        fn handle<'a>(
            &'a self,
            mut request: reqwest::Request,
            next: basic_client::Next<'a>,
        ) -> basic_client::ExecuteFuture<'a> {
            request.headers_mut().insert(
                reqwest::header::HeaderName::from_static("x-generated-mw"),
                reqwest::header::HeaderValue::from_static("on"),
            );
            next.run(request)
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let middleware = basic_client::MiddlewareBackend::new(inner).layer(std::sync::Arc::new(InjectHeader));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(middleware);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}
"##,
    )
    .unwrap();
    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(status.success());

    // the same generated crate must also build and lint clean WITH the `blocking` feature,
    // proving the `BlockingClient` type and its blocking methods compile under clippy -D warnings.
    let status = fixture_cargo(&out)
        .args(["build", "--features", "blocking"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must build with --features blocking"
    );

    let status = fixture_cargo(&out)
        .args(["clippy", "--features", "blocking", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must pass clippy -D warnings with --features blocking"
    );

    // Every generated error type is a real `std::error::Error`, for both shapes the frontend
    // produces. This is the bound `Error<E>` has always carried and no generated `E` used to
    // satisfy, so `?` into `Box<dyn Error>` (the pattern the book's own snippet uses), `anyhow`,
    // `thiserror`'s `#[from]`, and `to_string()` were all unavailable on a client with a
    // documented error body.
    std::fs::write(
        out.join("tests/errors.rs"),
        r##"fn assert_error<E: std::error::Error + 'static>() {}

// Every runtime type a generated signature mentions must also be nameable, because the embedded
// `support` module is private and the root re-export list is the whole surface. Naming each one in
// a type position is the assertion.
#[test]
fn every_runtime_type_in_a_signature_is_nameable() {
    fn header_result() -> Result<(), basic_client::HeaderError> {
        Ok(())
    }
    fn core(client: &basic_client::Client) -> &basic_client::ClientCore {
        client.core()
    }
    fn timeout_kind(kind: basic_client::TimeoutKind) -> basic_client::TimeoutKind {
        kind
    }
    fn config(config: &basic_client::ClientConfig) -> usize {
        config.max_error_body
    }
    // `RetryWait` is what a caller's own `RetryPolicy` has to return; before it was re-exported the
    // only way to write this was to spell out `Pin<Box<dyn Future<Output = ()> + Send + 'a>>`.
    struct NeverRetry;
    impl basic_client::RetryPolicy for NeverRetry {
        fn retry<'a>(
            &'a self,
            _attempt: u32,
            _outcome: &basic_client::RetryOutcome<'_>,
        ) -> Option<basic_client::RetryWait<'a>> {
            None
        }
    }
    fn shape(shape: basic_client::HeaderShape) -> basic_client::HeaderShape {
        shape
    }
    // `ApiErrorBody` is the bound a caller writes to be generic over operations' error bodies.
    fn body_of<E: basic_client::ApiErrorBody>(error: &E) -> Option<&E::Body> {
        error.body()
    }
    // Naming each type above is the assertion; binding the items keeps them from reading as dead.
    let _ = (header_result, core, timeout_kind, config, shape);
    let _ = body_of::<basic_client::GetTextErrorError>;
    let _: &dyn basic_client::RetryPolicy = &NeverRetry;
}

// The call-site shapes `impl Into<..>` / `impl Into<Option<..>>` are meant to widen, never to
// narrow: everything that compiled against the concrete signatures still compiles, and the shorter
// spellings compile too.
#[test]
fn required_string_params_and_the_params_bundle_accept_conversions() {
    fn call(client: &basic_client::Client) {
        let owned = String::from("k");
        // Widened: a literal and a `&String` now work where only `String` did.
        let _ = client.read_file("k");
        let _ = client.read_file(&owned);
        // Still accepted, so no existing call site breaks.
        let _ = client.read_file(owned.clone());
        // The params bundle takes `None` and `Some(..)` as before, and now the bundle itself.
        let _ = client.get_user("1", None);
        let _ = client.get_user("1", basic_client::GetUserParams::default());
        let _ = client.get_user("1", Some(basic_client::GetUserParams::default()));
    }
    let _ = call;
}

#[test]
fn the_client_is_debug_and_clone() {
    fn assert_debug<T: std::fmt::Debug>() {}
    fn assert_clone<T: Clone>() {}
    assert_debug::<basic_client::Client>();
    assert_clone::<basic_client::Client>();
    // A `Debug` client must never render a registered secret.
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearerAuth",
            basic_client::Credential::Bearer(basic_client::SecretString::from("hunter2")),
        );
    let rendered = format!("{:?}", client.clone());
    assert!(!rendered.contains("hunter2"), "secret leaked: {rendered}");
}

#[test]
fn generated_error_types_are_std_errors() {
    // `GetMultiError` is the multi-status enum; `GetTextErrorError` the single-body newtype.
    assert_error::<basic_client::Error<basic_client::GetMultiError>>();
    assert_error::<basic_client::Error<basic_client::GetTextErrorError>>();
    // The no-documented-error shape stays the uninhabited alias.
    assert_error::<basic_client::Error<std::convert::Infallible>>();
}

#[test]
fn a_generated_error_boxes_and_renders() -> Result<(), Box<dyn std::error::Error>> {
    let error: basic_client::Error<basic_client::GetTextErrorError> =
        basic_client::Error::request_message("boom");
    let boxed: Box<dyn std::error::Error> = Box::new(error);
    assert!(!boxed.to_string().is_empty());
    Ok(())
}

// A missing credential is the one request-construction cause an application routes on, so it is
// a typed variant reachable from generated output — and it fails before anything is sent, so one
// poll with a no-op waker is enough: no server, no runtime.
#[test]
fn a_missing_credential_is_a_typed_request_construction_error() {
    use std::future::Future;
    // The opaque cause of every other request-construction failure is nameable too.
    let _: Option<basic_client::RequestCause> = None;
    let client = basic_client::Client::new("http://127.0.0.1:1").unwrap();
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a missing credential must fail before anything is sent");
    };
    match result {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::MissingCredential { alternatives },
        )) => assert_eq!(alternatives, [vec!["bearer"], vec!["apiKey"]]),
        other => panic!("expected MissingCredential, got {other:?}"),
    }
}

// `without_credential` on the async client reaches dispatch: a client derived from a registered
// one by unregistering its only credential is back to reporting that scheme as missing, and the
// client it was cloned from keeps its registration.
#[test]
fn without_credential_unregisters_a_scheme_on_a_derived_client() {
    use std::future::Future;
    let registered = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Bearer(basic_client::SecretString::from("t0k")),
        );
    let derived = registered.clone().without_credential("bearer").without_credential("never");
    assert!(registered.core().credential("bearer").is_some());
    assert!(derived.core().credential("bearer").is_none());
    let mut call = std::pin::pin!(derived.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a missing credential must fail before anything is sent");
    };
    match result {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::MissingCredential { alternatives },
        )) => assert_eq!(alternatives, [vec!["bearer"], vec!["apiKey"]]),
        other => panic!("expected MissingCredential, got {other:?}"),
    }
}

// The reason the payload is `Vec<Vec<&str>>` and not the flat `Vec<&str>` the issue proposed: an
// alternative is a conjunction, and flattening loses which schemes must be presented *together*.
// This is the only place a generated client is driven to produce that shape. It also pins the two
// properties the grouping exists for, which only a generated requirement can witness: spargen emits
// a conjunction into the requirement slice in declaration order, and a `mutualTLS` member is left
// out of the report entirely, because the transport satisfies it and the caller cannot register it.
#[test]
fn a_generated_conjunction_reports_each_alternative_grouped() {
    fn outstanding(client: &basic_client::Client) -> Vec<Vec<&'static str>> {
        use std::future::Future;
        let mut call = std::pin::pin!(client.get_conjunction());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
            panic!("a missing credential must fail before anything is sent");
        };
        match result {
            Err(basic_client::Error::RequestConstruction(
                basic_client::RequestError::MissingCredential { alternatives },
            )) => alternatives,
            other => panic!("expected MissingCredential, got {other:?}"),
        }
    }

    let bare = basic_client::Client::new("http://127.0.0.1:1").unwrap();
    assert_eq!(
        outstanding(&bare),
        [
            // A TWO-SCHEME CONJUNCTION, in declaration order. This inner list having length 2 is
            // the whole reason the payload is not a flat `Vec<&str>`: a caller told "tenant,
            // bearer, apiKey" cannot tell that the first two must be presented together.
            vec!["tenant", "bearer"],
            vec!["apiKey"],
            // `mtls` is absent. mutualTLS is satisfied by the transport's client certificate, so
            // it is never a credential the caller can register and must never be reported as one.
            vec!["tenant"],
        ]
    );
    // The grouping survives rendering: `+` joins a conjunction, `or` separates alternatives.
    let rendered = basic_client::Error::<std::convert::Infallible>::RequestConstruction(
        basic_client::RequestError::MissingCredential {
            alternatives: outstanding(&bare),
        },
    );
    let rendered = std::error::Error::source(&rendered).unwrap().to_string();
    assert!(
        rendered.ends_with("(missing: tenant + bearer or apiKey or tenant)"),
        "{rendered}"
    );

    // Registering one member of a conjunction does not satisfy the alternative it appears in: that
    // alternative goes on reporting the members still unregistered. `bearer` is in exactly one of
    // the three alternatives here, so only that one's report changes — and all three stay
    // outstanding.
    let partial = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Bearer(basic_client::SecretString::from("t0k")),
        );
    assert_eq!(
        outstanding(&partial),
        [vec!["tenant"], vec!["apiKey"], vec!["tenant"]]
    );
}

// The other credential state an application routes on: a credential *is* registered, but the
// provider behind it could not refresh it. That is still "unauthenticated", and it is still raised
// before anything is sent — so the same poll-once shape reaches it, with no server and no runtime.
#[test]
fn a_failed_token_provider_is_a_typed_request_construction_error() {
    use std::future::Future;
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Provider(std::sync::Arc::new(|| {
                Box::pin(async { Err(basic_client::AuthError::new("refresh rejected")) })
                    as basic_client::TokenFuture
            })),
        );
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a failed token provider must fail before anything is sent");
    };
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a failed token provider cannot produce a response"),
    };
    match &error {
        basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialProvider { scheme, source },
        ) => {
            assert_eq!(*scheme, "bearer");
            assert_eq!(source.to_string(), "refresh rejected");
        }
        other => panic!("expected CredentialProvider, got {other:?}"),
    }
    // Nothing was sent, so there is nothing to retry — and the provider's own error stays reachable
    // through the chain, which is how an application reports *why* the refresh failed.
    assert!(!error.is_transient());
    let cause = std::error::Error::source(&error).unwrap();
    // The request-level message is a fixed sentence naming the scheme, not the provider's text:
    // the provider's own message is one level further down, where it is downcast below.
    assert_eq!(
        cause.to_string(),
        "the token provider registered for security scheme `bearer` failed"
    );
    let provider = std::error::Error::source(cause).unwrap();
    assert!(provider.downcast_ref::<basic_client::AuthError>().is_some());
}

// The third credential state: a credential is registered and selects its alternative, but it is of
// a kind the scheme cannot carry. It is typed from generated output too, with nothing beneath it,
// so a consumer never has to match on its text.
#[test]
fn a_credential_of_the_wrong_kind_is_a_typed_request_construction_error() {
    use std::future::Future;
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Basic {
                username: "aladdin".to_owned(),
                password: basic_client::SecretString::from("open sesame"),
            },
        );
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a credential mismatch must fail before anything is sent");
    };
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a credential mismatch cannot produce a response"),
    };
    match &error {
        basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialMismatch { scheme, required, registered },
        ) => {
            assert_eq!(*scheme, "bearer");
            assert_eq!(*required, "bearer");
            assert_eq!(*registered, "Basic");
        }
        other => panic!("expected CredentialMismatch, got {other:?}"),
    }
    assert!(!error.is_transient());
    let cause = std::error::Error::source(&error).unwrap();
    assert!(std::error::Error::source(cause).is_none());
}

#[test]
fn the_single_error_body_stays_one_deref_away() {
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert_eq!(wrapped.len(), 4); // through `Deref` to `String`
    assert_eq!(wrapped.into_inner(), "nope");
    assert_eq!(String::from(basic_client::GetTextErrorError("x".to_owned())), "x");
}

#[test]
fn a_multi_status_error_names_its_status() {
    let error = basic_client::GetMultiError::Status404(Box::new(
        basic_client::types::NotFoundError { reason: "gone".to_owned() },
    ));
    assert!(error.to_string().contains("404"), "{error}");
}

#[test]
fn a_uniform_body_error_enum_hands_back_its_body_from_any_status() {
    let problem = basic_client::types::Problem { title: "nope".to_owned(), detail: "gone".to_owned() };
    let not_found = basic_client::GetSharedError::Status404(Box::new(problem.clone()));
    let conflict = basic_client::GetSharedError::Status409(Box::new(problem));
    assert_eq!(not_found.body().map(|p| p.detail.as_str()), Some("gone"));
    assert_eq!(conflict.body().map(|p| p.detail.as_str()), Some("gone"));
    // A documented bodyless status carries nothing to hand back.
    assert!(basic_client::GetSharedError::Status401.body().is_none());
}

#[test]
fn the_taxonomy_reaches_the_body_generically_over_the_operation() {
    // Generic over `E`: this is the "thirty distinct error types" case from the issue.
    fn detail<E>(error: &basic_client::Error<E>) -> Option<&str>
    where
        E: basic_client::ApiErrorBody<Body = basic_client::types::Problem>,
    {
        error.api_body().map(|problem| problem.detail.as_str())
    }
    let body = basic_client::GetSharedError::Status409(Box::new(basic_client::types::Problem {
        title: "conflict".to_owned(),
        detail: "dup".to_owned(),
    }));
    let error = basic_client::Error::Api(basic_client::ResponseValue::new(
        reqwest::StatusCode::CONFLICT,
        Default::default(),
        body,
    ));
    assert_eq!(detail(&error), Some("dup"));
    let transport: basic_client::Error<basic_client::GetSharedError> =
        basic_client::Error::request_message("boom");
    assert_eq!(detail(&transport), None);
}

#[test]
fn every_error_shape_implements_api_error_body() {
    use basic_client::ApiErrorBody;
    // The single-body newtype: `Body` is the inner type.
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert_eq!(wrapped.body().map(String::as_str), Some("nope"));
    // The no-documented-error shape: exists so generic code compiles, and is always `None`.
    fn never(error: &basic_client::Error<std::convert::Infallible>) -> bool {
        error.api_body().is_none()
    }
    assert!(never(&basic_client::Error::request_message("x")));
}

// `Error::problem` is generic over every operation, including an enum whose statuses carry
// DIFFERENT body types (`getMulti`), which has no `ApiErrorBody` and so no `api_body`.
#[test]
fn the_problem_reader_is_generic_over_every_error_shape() {
    fn detail<E: basic_client::ApiErrorProblem>(error: &basic_client::Error<E>) -> Option<String> {
        error.problem().and_then(|problem| problem.detail)
    }
    fn api<E>(status: reqwest::StatusCode, body: E) -> basic_client::Error<E> {
        basic_client::Error::Api(basic_client::ResponseValue::new(status, Default::default(), body))
    }
    // The uniform-body enum.
    let shared = basic_client::GetSharedError::Status409(Box::new(basic_client::types::Problem {
        title: "conflict".to_owned(),
        detail: "dup".to_owned(),
    }));
    assert_eq!(detail(&api(reqwest::StatusCode::CONFLICT, shared)), Some("dup".to_owned()));
    // The heterogeneous enum: a body with no `detail` member answers with the members it has.
    let multi = basic_client::GetMultiError::Status404(Box::new(
        basic_client::types::NotFoundError { reason: "gone".to_owned() },
    ));
    let problem = api(reqwest::StatusCode::NOT_FOUND, multi).problem().expect("an object body");
    assert_eq!(problem, basic_client::ProblemDetails::default());
    // A documented bodyless status, a textual body, and a raw-bytes body have no members.
    let unit = api(reqwest::StatusCode::UNAUTHORIZED, basic_client::GetSharedError::Status401);
    assert!(unit.problem().is_none());
    let text = basic_client::GetRawMultiError::Status400(Box::new("bad".to_owned()));
    assert!(api(reqwest::StatusCode::BAD_REQUEST, text).problem().is_none());
    let raw = basic_client::GetRawMultiError::Status409(Box::new(bytes::Bytes::from_static(
        br#"{"detail":"never read"}"#,
    )));
    assert!(api(reqwest::StatusCode::CONFLICT, raw).problem().is_none());
    // The single-body newtype and the uninhabited shape.
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert!(api(reqwest::StatusCode::BAD_REQUEST, wrapped).problem().is_none());
    let never: basic_client::Error<std::convert::Infallible> =
        basic_client::Error::request_message("x");
    assert!(never.problem().is_none());
}

#[test]
fn a_nullable_error_body_answers_none_for_null_on_both_shapes() {
    use basic_client::ApiErrorBody;
    let problem = basic_client::types::MaybeProblem { title: "nope".to_owned() };
    // The enum shape: a nullable payload is `Option<Box<T>>`, so `null` is `None`, a value is
    // `Some`, and the `default` response's variant is reached like any exact status.
    assert!(basic_client::GetMaybeError::Status404(None).body().is_none());
    let not_found = basic_client::GetMaybeError::Status404(Some(Box::new(problem.clone())));
    assert_eq!(not_found.body().map(|p| p.title.as_str()), Some("nope"));
    let fallback = basic_client::GetMaybeError::Default(Some(Box::new(problem.clone())));
    assert_eq!(fallback.body().map(|p| p.title.as_str()), Some("nope"));
    // The newtype shape over the same component: `Body` is the bare `MaybeProblem`, not the
    // `Option` the newtype wraps, and `null` answers `None` exactly as the enum does.
    assert!(basic_client::GetMaybeSingleError(None).body().is_none());
    let single = basic_client::GetMaybeSingleError(Some(problem.clone()));
    assert_eq!(single.body().map(|p| p.title.as_str()), Some("nope"));
    // One bound names the component and accepts both shapes.
    fn title<E>(error: &E) -> Option<&str>
    where
        E: basic_client::ApiErrorBody<Body = basic_client::types::MaybeProblem>,
    {
        error.body().map(|p| p.title.as_str())
    }
    let conflict = basic_client::GetMaybeError::Status409(Some(Box::new(problem.clone())));
    assert_eq!(title(&conflict), Some("nope"));
    assert_eq!(title(&basic_client::GetMaybeSingleError(Some(problem))), Some("nope"));
    assert_eq!(title(&basic_client::GetMaybeSingleError(None)), None);
}

#[test]
fn an_alias_equal_error_body_is_one_body_type() {
    // `PlainMessage` and the inline 409 schema are distinct schemas, but both generate `String`:
    // one body type, so the inherent accessor and the trait both exist.
    let not_found = basic_client::GetAliasSharedError::Status404(Box::new("gone".to_owned()));
    let conflict = basic_client::GetAliasSharedError::Status409(Box::new("dup".to_owned()));
    assert_eq!(not_found.body().map(String::as_str), Some("gone"));
    assert_eq!(conflict.body().map(String::as_str), Some("dup"));
    fn text<E: basic_client::ApiErrorBody<Body = String>>(error: &E) -> Option<&str> {
        error.body().map(String::as_str)
    }
    assert_eq!(text(&not_found), Some("gone"));
    assert_eq!(text(&conflict), Some("dup"));
}
"##,
    )
    .unwrap();
    let status = fixture_cargo(&out)
        .args(["test", "--test", "errors"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated error types must be usable as `std::error::Error`"
    );

    // Drive the blocking round-trip test under the feature (it is `#![cfg(feature = "blocking")]`, so
    // it only exists here). This exercises a real HTTP round-trip through a blocking method.
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "blocking"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the BlockingClient round-trip must pass with --features blocking"
    );
}

/// A response shape as the emitted client spells it, with the side's naming stripped: no body type
/// (`()` on the success side, the uninhabited alias on the error side), one body type alone, or an
/// enum listing, per variant, the index of the entry it stands for and its payload type (`None` for
/// a unit variant).
#[derive(Debug, PartialEq)]
enum EmittedShape {
    NoBody,
    OneBody(String),
    Enum(Vec<(u16, Option<String>)>),
}

/// The variants of the emitted `pub enum {name}`, each as its status minus `base` and its payload.
fn emitted_variants(code: &str, name: &str, base: u16) -> Vec<(u16, Option<String>)> {
    let head = format!("pub enum {name} {{\n");
    let start = code.find(&head).expect(&head) + head.len();
    let body = &code[start..start + code[start..].find("\n}").unwrap()];
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("Status"))
        .map(|variant| {
            let variant = variant.trim_end_matches(',');
            let (status, payload) = match variant.split_once('(') {
                Some((status, payload)) => (status, Some(payload.trim_end_matches(')').to_owned())),
                None => (variant, None),
            };
            (status.parse::<u16>().unwrap() - base, payload)
        })
        .collect()
}

/// `Responses::success` and `Responses::error` count bodies through one computation, and their doc
/// comments state the resulting rule once per side. They disagreed for two rounds of review with
/// every gate green (issue #210), because each side was pinned on its own. This drives every
/// pattern of one to three bodied/bodyless entries through `spargen::generate` twice — once as the
/// success statuses `200..` of one operation, once as the error statuses `400..` of another, each
/// entry `i` carrying the same body `Bi` on both sides — and requires the two emitted shapes to be
/// the same: both count bodies, both emit one variant per entry in status order, and both give a
/// bodyless entry beside any body its own unit variant. Only then is the shared rule itself checked.
/// The success side's streaming exception is the one deliberate asymmetry, and these bodies are
/// JSON, so it never applies.
#[test]
fn success_and_error_shapes_agree_on_the_shared_rule() {
    let patterns: Vec<Vec<bool>> = (1..=3u32)
        .flat_map(|len| {
            (0..1u32 << len).map(move |bits| (0..len).map(|i| bits & (1 << i) != 0).collect())
        })
        .collect();
    let entries = |base: u16, pattern: &[bool]| -> String {
        pattern
            .iter()
            .enumerate()
            .map(|(i, bodied)| {
                let content = if *bodied {
                    format!(
                        ", content: {{application/json: {{schema: {{$ref: '#/components/schemas/B{i}'}}}}}}"
                    )
                } else {
                    String::new()
                };
                format!("        '{}': {{description: d{content}}}\n", base + i as u16)
            })
            .collect()
    };
    let mut spec = String::from("openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths:\n");
    for (n, pattern) in patterns.iter().enumerate() {
        spec.push_str(&format!(
            "  /ok{n}:\n    get:\n      operationId: okSide{n}\n      responses:\n{}",
            entries(200, pattern)
        ));
        spec.push_str(&format!(
            "  /err{n}:\n    get:\n      operationId: errSide{n}\n      responses:\n        '200': {{description: ok}}\n{}",
            entries(400, pattern)
        ));
    }
    spec.push_str("components:\n  schemas:\n");
    for i in 0..3 {
        spec.push_str(&format!(
            "    B{i}: {{type: object, properties: {{field{i}: {{type: string}}}}}}\n"
        ));
    }

    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(&spec_path, &spec).unwrap();
    let out = temp.path().join("client.rs");
    let report = spargen::generate(
        &Spec::new(Utf8PathBuf::from_path_buf(spec_path).unwrap())
            .build(Utf8PathBuf::from_path_buf(out.clone()).unwrap())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}\n{spec}");
    let code = std::fs::read_to_string(out).unwrap();

    for (n, pattern) in patterns.iter().enumerate() {
        // The signature, whitespace removed, since the formatter wraps a long one across lines.
        let method = format!("pub async fn ok_side{n}(");
        let start = code.find(&method).expect(&method);
        let signature: String = code[start..start + code[start..].find('{').unwrap()]
            .split_whitespace()
            .collect();
        let head = "Result<support::ResponseValue<";
        let start = signature.find(head).expect(head) + head.len();
        let success_ty =
            &signature[start..start + signature[start..].find(">,support::Error<").unwrap()];
        let success = match success_ty {
            "()" => EmittedShape::NoBody,
            enum_ty if enum_ty == format!("OkSide{n}Response") => {
                EmittedShape::Enum(emitted_variants(&code, enum_ty, 200))
            }
            body => EmittedShape::OneBody(body.to_owned()),
        };

        let error_ty = format!("ErrSide{n}Error");
        let newtype = format!("pub struct {error_ty}(pub ");
        let error = if code.contains(&format!("pub type {error_ty} = std::convert::Infallible;")) {
            EmittedShape::NoBody
        } else if let Some(at) = code.find(&newtype) {
            let start = at + newtype.len();
            EmittedShape::OneBody(code[start..start + code[start..].find(");").unwrap()].to_owned())
        } else {
            EmittedShape::Enum(emitted_variants(&code, &error_ty, 400))
        };

        assert_eq!(
            success, error,
            "the success and error sides disagree on the entry pattern {pattern:?} (bodied?)"
        );

        let bodied: Vec<usize> = (0..pattern.len()).filter(|&i| pattern[i]).collect();
        let expected = match (bodied.as_slice(), pattern.len()) {
            ([], _) => EmittedShape::NoBody,
            ([only], 1) => EmittedShape::OneBody(format!("types::B{only}")),
            _ => EmittedShape::Enum(
                pattern
                    .iter()
                    .enumerate()
                    .map(|(i, bodied)| (i as u16, bodied.then(|| format!("Box<types::B{i}>"))))
                    .collect(),
            ),
        };
        assert_eq!(
            success, expected,
            "both sides departed from the shared rule on {pattern:?} (bodied?)"
        );
    }
}

#[test]
fn rejects_openapi_30_without_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        BASIC_SPEC.replace("openapi: 3.1.0", "openapi: 3.0.3"),
    )
    .unwrap();

    let report = spargen::check(&Spec::new(Utf8PathBuf::from_path_buf(spec).unwrap()));

    assert_eq!(report.outcome(), Outcome::Rejected);
    assert!(report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == Code::UnsupportedOpenApiVersion));
}

#[test]
fn generated_module_compiles_in_oas32_crate_with_query_method() {
    // OpenAPI 3.2 lowers through the same frontend. This spec exercises the new fixed `QUERY`
    // method (which must emit a real client method) alongside a plain `get`, and must produce a
    // module in an application-owned crate that passes `cargo check` + `cargo clippy -D warnings`.
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, OAS32_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "oas32_client");

    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics()
        .iter()
        .all(|diagnostic| diagnostic.severity != spargen::Severity::Error));

    let generated_path = out.join("src/lib.rs");
    let mut generated = std::fs::read_to_string(&generated_path).unwrap();
    generated.push_str(
        r#"
#[allow(dead_code)]
fn assert_standard_stream<S: futures_core::Stream<Item = Result<types::AdminEvent, StreamError>>>() {}
#[allow(dead_code)]
fn assert_generated_stream_surface() {
    assert_standard_stream::<EventStream<types::AdminEvent>>();
}
#[allow(dead_code)]
fn assert_reconnect_policy_is_public<P: ReconnectPolicy>() {}
"#,
    );
    std::fs::write(&generated_path, &generated).unwrap();

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(status.success());

    let status = fixture_cargo(&out)
        .args(["clippy", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(status.success());

    // The QUERY operation lowered to a real client method (compile-verified above); prove the
    // method exists in the emitted source so a regression that drops QUERY is caught.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("pub async fn search_records"),
        "QUERY operation should emit a client method"
    );
    assert!(
        generated.contains("reqwest::Method::from_bytes(b\"QUERY\")"),
        "QUERY method should be built from its token bytes"
    );
    // 3.2 `additionalOperations`: a custom method token also emits a real client method, built
    // from its exact bytes rather than mapped onto a fixed `reqwest::Method` constant.
    assert!(
        generated.contains("pub async fn purge_cache"),
        "an additionalOperations method should emit a client method"
    );
    assert!(
        generated.contains("reqwest::Method::from_bytes(b\"PURGE\")"),
        "a custom method should be built from its token bytes"
    );

    // The OpenAPI 3.2 streaming response recognizes the standard SSE envelope annotation and types
    // the stream as its JSON `data.contentSchema`, not as the envelope object.
    let flat: String = generated.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("pub async fn stream_events"),
        "streaming operation should emit a client method"
    );
    assert!(
        flat.contains("support :: EventStream < types :: AdminEvent >")
            || flat.contains("support::EventStream<types::AdminEvent>"),
        "SSE contentSchema must type EventStream as the JSON payload: {flat}"
    );

    // Wire-level 3.2 coverage: `in: querystring`, `style: cookie`, typed server variables, and a
    // typed response-header accessor, all against a real socket. Without this the 3.2 constructs
    // are only compile-verified, and a construct can compile while sending the wrong bytes.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/wire.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn oas32_constructs_reach_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();

        // `in: querystring` with `content: application/x-www-form-urlencoded` serializes the whole
        // object into the query string.
        assert!(request_line.starts_with("GET /records?term="), "{request}");
        assert!(request_line.contains("term=a%20b"), "{request}");
        // `style: cookie` sends the value verbatim — the one cookie style that never encodes.
        assert!(request.contains("cookie: session=a/b\r\n"), "{request}");

        let body = r#"[{"id":"r1"}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Total-Count: 42\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    // A templated server resolves with every variable at its declared default, and the typed enum
    // makes an out-of-enum region unconstructible.
    assert_eq!(oas32_client::servers::default_url(), "https://us.example.com/v1");
    assert_eq!(
        oas32_client::servers::Server0::new()
            .region(oas32_client::servers::Server0Region::Eu)
            .version("v2")
            .url(),
        "https://eu.example.com/v2"
    );

    let params = oas32_client::ListRecordsParams::default()
        .filter(oas32_client::types::Query { term: Some("a b".to_owned()) })
        .session("a/b".to_owned());
    let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    let response = client.list_records(Some(params)).unwrap();

    // Documented response headers are read through a typed accessor, as an explicit second step —
    // a malformed header can never turn a successful call into a failure.
    let headers = oas32_client::ListRecordsStatus200Headers::from_response(&response).unwrap();
    assert_eq!(headers.x_total_count, 42);
    assert_eq!(response.into_inner()[0].id, "r1");

    server.join().unwrap();
}

/// `in: querystring` with a JSON `content:` entry owns the whole query string: the serialized
/// value, percent-encoded as one token, so none of its `&`, `=`, or `#` splits or ends the query.
/// An absent optional value sends no query at all.
#[test]
fn a_json_querystring_parameter_is_one_encoded_query() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for expected in [
            "GET /lookup?%7B%22term%22%3A%22a%26b%3Dc%23d%22%7D HTTP/1.1",
            "POST /lookup?%7B%22term%22%3A%22x%20y%22%7D HTTP/1.1",
            "POST /lookup HTTP/1.1",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);
            let request_line = request.lines().next().unwrap();

            assert_eq!(request_line, expected, "{request}");

            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.flush().unwrap();
        }
    });

    let query = |term: &str| oas32_client::types::Query { term: Some(term.to_owned()) };
    let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client.lookup_records(query("a&b=c#d")).unwrap();
    client
        .lookup_records_maybe(Some(
            oas32_client::LookupRecordsMaybeParams::default().filter(query("x y")),
        ))
        .unwrap();
    client.lookup_records_maybe(None).unwrap();

    server.join().unwrap();
}

/// `additionalOperations` on the wire, plus the two constructs its operation is built from: a
/// `components.mediaTypes` reference supplying the request body, and a discriminated union whose
/// `defaultMapping` catches an unrecognized tag.
#[test]
fn oas32_custom_method_and_discriminator_fallback_reach_the_wire() {
    for (tag, expect_unknown) in [("purged", false), ("something-else", true)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);

            // The custom method token travels verbatim; nothing maps it onto a fixed method.
            assert!(
                request.starts_with("PURGE /records/cache "),
                "custom method must reach the wire unchanged: {request}"
            );
            // The body came from the reusable `components.mediaTypes` entry, typed as `Query`.
            assert!(request.contains(r#"{"term":"drop"}"#), "{request}");

            let body = if expect_unknown {
                format!(r#"{{"outcome":"{tag}","detail":"not recognized"}}"#)
            } else {
                r#"{"outcome":"purged","entries":7}"#.to_owned()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let client = oas32_client::BlockingClient::new(&format!("http://{addr}")).unwrap();
        let outcome = client
            .purge_cache(&oas32_client::types::Query { term: Some("drop".to_owned()) })
            .unwrap()
            .into_inner();

        match outcome {
            // An unmapped discriminator value decodes into the `defaultMapping` branch rather
            // than failing, which is the whole point of the 3.2 field.
            oas32_client::types::CacheOutcome::CacheUnknown(unknown) => {
                assert!(expect_unknown, "mapped tag must not fall back: {unknown:?}");
                assert_eq!(unknown.detail, "not recognized");
            }
            oas32_client::types::CacheOutcome::CachePurged(purged) => {
                assert!(!expect_unknown, "unmapped tag must fall back");
                assert_eq!(purged.entries, 7);
            }
        }

        server.join().unwrap();
    }
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "wire"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the OpenAPI 3.2 wire round-trip must pass with --features blocking"
    );
}

#[test]
fn omit_overlay_removes_unsupported_operation() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, SPEC_WITH_UNSUPPORTED_OPERATION).unwrap();
    let out = temp.path().join("client.rs");
    let config = Spec::new(Utf8PathBuf::from_path_buf(spec).unwrap())
        .omit(spargen::omit! {
            operations {
                post "/upload";
            }
        })
        .build(Utf8PathBuf::from_path_buf(out).unwrap())
        .cargo(CargoIntegration::Off);

    let report = spargen::generate(&config);

    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == Code::OmittedConstruct));
}

/// GENERATED-CODE property round-trip. Generate a module in a fixture crate carrying
/// the representative union/allOf types (a discriminated union, a structurally-disjoint
/// string-vs-array union, a required-key-disjoint closed-object union, a nullable-variant union, and
/// an `allOf`-merged struct), then add `proptest` as a dev-dependency OF THE HARNESS-SCAFFOLDED
/// CRATE ONLY and drive a generated `tests/roundtrip.rs` that round-trips MANY random values through
/// the real emitted serde code. The types derive `Serialize`/`Deserialize` but NOT `PartialEq`, so
/// stability is asserted via re-serialized `serde_json::Value` equality (serialize→deserialize→
/// serialize is a fixed point) — this catches a disjoint union misrouting a value to the wrong
/// variant (j2 would differ) and `allOf` field loss. A stronger per-variant assertion proves a value
/// built as variant K deserializes back onto variant K (not misrouted).
///
/// Confirms, before appending, that the fixture manifest carries no `proptest`: the dependency is
/// scoped strictly to this throwaway test crate.
#[test]
fn union_and_allof_roundtrip_under_proptest() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, ROUNDTRIP_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "roundtrip_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    // The property-test dependency is exclusively scaffolding for this test harness's crate.
    let mut manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("proptest"),
        "fixture manifest must not carry proptest: {manifest}"
    );
    manifest.push_str("\n[dev-dependencies]\nproptest = \"1\"\n");
    std::fs::write(out.join("Cargo.toml"), manifest).unwrap();

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/roundtrip.rs"), ROUNDTRIP_TEST).unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--test", "roundtrip"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the generated union/allOf types must survive the proptest JSON round-trip"
    );
}

/// The generated `tests/roundtrip.rs` for [`union_and_allof_roundtrip_under_proptest`]. Hand-written
/// proptest strategies span each type's value space; each type asserts the serialize→deserialize→
/// serialize `serde_json::Value` fixed point over 64 random cases, and the two disjoint unions plus
/// the discriminated union additionally assert a value built as variant K is not misrouted on decode.
const ROUNDTRIP_TEST: &str = r####"
use proptest::prelude::*;
use roundtrip_client::types;

/// serialize → deserialize → serialize is a fixed point on the JSON value (the types lack
/// `PartialEq`, so equality is asserted on the re-serialized `serde_json::Value`). A misrouted
/// disjoint-union value or a dropped `allOf` field would make the second serialization differ.
fn roundtrip_stable<T>(value: &T) -> Result<(), TestCaseError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let first = serde_json::to_value(value).expect("serialize");
    let back: T = serde_json::from_value(first.clone()).expect("deserialize");
    let second = serde_json::to_value(&back).expect("re-serialize");
    prop_assert_eq!(first, second);
    Ok(())
}

// Discriminated union (Cat DECLARES the `petType` tag, Dog does not). The tag value is fixed to the
// variant's mapping key so the payload routes back to the variant it was built as; `name`/`bark` are
// random.
fn pet_strategy() -> impl Strategy<Value = types::Pet> {
    prop_oneof![
        "[a-zA-Z0-9 ]{0,16}".prop_map(|name| types::Pet::Cat(Box::new(types::Cat {
            pet_type: "cat".to_owned(),
            name,
        }))),
        any::<bool>().prop_map(|bark| types::Pet::Dog(Box::new(types::Dog { bark }))),
    ]
}

// Structurally-disjoint union: a bare string vs an array of strings (distinct JSON categories).
fn string_or_list_strategy() -> impl Strategy<Value = types::StringOrList> {
    prop_oneof![
        "[a-zA-Z0-9 ]{0,16}".prop_map(|value| {
            types::StringOrList::StringOrListVariant0(Box::new(value))
        }),
        proptest::collection::vec("[a-zA-Z0-9 ]{0,8}", 0..5)
            .prop_map(|value| types::StringOrList::StringOrListVariant1(Box::new(value))),
    ]
}

// Nullable-variant union: the string member is nullable, hoisting nullability to the whole union, so
// the field is `Option<StringListOrNull>` — `None` is a bare JSON `null`.
fn notes_strategy() -> impl Strategy<Value = Option<types::StringListOrNull>> {
    prop_oneof![
        Just(None),
        "[a-zA-Z0-9 ]{0,16}"
            .prop_map(|s| Some(types::StringListOrNull::StringListOrNullVariant0(Box::new(s)))),
        proptest::collection::vec("[a-zA-Z0-9 ]{0,8}", 0..5)
            .prop_map(|v| Some(types::StringListOrNull::StringListOrNullVariant1(Box::new(v)))),
    ]
}

// Required-key-disjoint union: two CLOSED objects, each carrying a unique required key.
fn shape_strategy() -> impl Strategy<Value = types::Shape> {
    prop_oneof![
        (-1000.0f64..1000.0)
            .prop_map(|radius| types::Shape::Circle(Box::new(types::Circle { radius }))),
        (-1000.0f64..1000.0)
            .prop_map(|side| types::Shape::Square(Box::new(types::Square { side }))),
    ]
}

// `allOf`-merged struct: a required `$ref` base field, a required inline member field, and an
// optional sibling.
fn account_strategy() -> impl Strategy<Value = types::Account> {
    (
        "[a-zA-Z0-9]{0,12}",
        "[a-zA-Z0-9]{0,12}",
        proptest::option::of("[a-zA-Z0-9]{0,12}"),
    )
        .prop_map(|(id, label, owner)| types::Account { id, label, owner })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn pet_roundtrips(value in pet_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn string_or_list_roundtrips(value in string_or_list_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn notes_roundtrips(value in notes_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn shape_roundtrips(value in shape_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn account_roundtrips(value in account_strategy()) {
        roundtrip_stable(&value)?;
    }

    // Stronger property: a value built as variant K, serialized then deserialized, lands back on
    // variant K — the custom disjoint/discriminated Deserialize never misroutes.
    #[test]
    fn pet_variant_not_misrouted(value in pet_strategy()) {
        let was_cat = matches!(value, types::Pet::Cat(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::Pet = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(was_cat, matches!(back, types::Pet::Cat(_)));
    }

    #[test]
    fn string_or_list_variant_not_misrouted(value in string_or_list_strategy()) {
        let was_string = matches!(value, types::StringOrList::StringOrListVariant0(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::StringOrList = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(
            was_string,
            matches!(back, types::StringOrList::StringOrListVariant0(_))
        );
    }

    #[test]
    fn shape_variant_not_misrouted(value in shape_strategy()) {
        let was_circle = matches!(value, types::Shape::Circle(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::Shape = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(was_circle, matches!(back, types::Shape::Circle(_)));
    }
}
"####;

/// The spec generated for [`union_and_allof_roundtrip_under_proptest`]: a `User` object pulling in a
/// discriminated union (`Pet`), a string-vs-array disjoint union (`StringOrList`), a nullable-variant
/// union (`StringListOrNull`), a required-key-disjoint closed-object union (`Shape`), and an
/// `allOf`-merged struct (`Account`). One operation references `User` so every type is emitted.
const ROUNDTRIP_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Roundtrip, version: 1.0.0 }
servers:
  - url: https://example.com/api
paths:
  /user:
    get:
      operationId: getUser
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/User"
components:
  schemas:
    User:
      type: object
      required: [id]
      properties:
        id: { type: string }
        pet: { $ref: "#/components/schemas/Pet" }
        alias: { $ref: "#/components/schemas/StringOrList" }
        notes: { $ref: "#/components/schemas/StringListOrNull" }
        shape: { $ref: "#/components/schemas/Shape" }
        account: { $ref: "#/components/schemas/Account" }
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType: { type: string }
        name: { type: string }
    Dog:
      type: object
      required: [bark]
      properties:
        bark: { type: boolean }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
    StringOrList:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
    StringListOrNull:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
    Circle:
      type: object
      additionalProperties: false
      required: [radius]
      properties:
        radius: { type: number }
    Square:
      type: object
      additionalProperties: false
      required: [side]
      properties:
        side: { type: number }
    Shape:
      oneOf:
        - $ref: "#/components/schemas/Circle"
        - $ref: "#/components/schemas/Square"
    AccountBase:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Account:
      type: object
      properties:
        owner: { type: string }
      allOf:
        - $ref: "#/components/schemas/AccountBase"
        - type: object
          required: [label]
          properties:
            label: { type: string }
"##;

const BASIC_SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Basic
  version: 1.0.0
servers:
  - url: https://example.com/api
  # A templated server whose URL splits into single-character literal segments (`:` and `/`).
  # Rendering one of those with `push_str` trips `clippy::single_char_add_str`, and generated code
  # must pass `-D warnings` in the consuming crate — which the clippy gate below enforces.
  - url: https://{host}:{port}/{stage}
    variables:
      host: { default: api.example.com }
      port: { default: "443" }
      stage:
        default: v1
        enum: [v1, v2]
paths:
  /files:
    get:
      operationId: readFile
      parameters:
        - name: path
          in: query
          required: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
  # Required parameters reserve their natural identifiers before generator-owned bindings are
  # allocated. This compile-verifies collisions with every request-building local plus the fixed
  # optional-params and request-body arguments; `/files` above pins the wire behavior.
  /binding-collisions:
    get:
      operationId: bindingCollisions
      parameters:
        - name: query
          in: query
          required: true
          schema: { type: string }
        - name: url
          in: query
          required: true
          schema: { type: string }
        - name: request
          in: header
          required: true
          schema: { type: string }
        - name: cookies
          in: cookie
          required: true
          schema: { type: string }
        - name: optional
          in: query
          schema: { type: string }
      responses:
        "204": { description: No Content }
  /signature-binding-collisions:
    post:
      operationId: signatureBindingCollisions
      parameters:
        - name: body
          in: query
          required: true
          schema: { type: string }
        - name: params
          in: query
          required: true
          schema: { type: string }
        - name: optional
          in: query
          schema: { type: string }
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/CollisionPayload"
      responses:
        "204": { description: No Content }
  /params/{ids}:
    get:
      operationId: serializeParams
      parameters:
        - name: ids
          in: path
          required: true
          style: simple
          explode: false
          schema:
            type: array
            items: { type: integer }
        - name: workflow_id
          in: query
          required: true
          schema:
            $ref: "#/components/schemas/WorkflowId"
        - name: X-Flags
          in: header
          required: true
          schema:
            type: array
            items: { type: string }
        - name: labels
          in: query
          explode: true
          schema:
            type: array
            items: { type: string }
        - name: compact
          in: query
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: session
          in: cookie
          explode: true
          schema:
            type: array
            items: { type: string }
      responses:
        "204": { description: No Content }
  # The payload shape `MissingCredential` carries is `Vec<Vec<&str>>` rather than a flat list
  # because OpenAPI `security` is a disjunction of conjunctions: "A and B, or C" and "A, B, or C"
  # flatten to the same three names while demanding different credentials. This operation is the
  # only place in the repository where a *generated* client is asked to produce that shape — a
  # two-scheme conjunction, a single-scheme alternative, and an alternative whose other member is
  # `mutualTLS` and so must be omitted from the report.
  /conjunction:
    get:
      operationId: getConjunction
      security:
        - tenant: []
          bearer: []
        - apiKey: []
        - mtls: []
          tenant: []
      responses:
        "204": { description: No Content }
  /users/{id}:
    get:
      operationId: getUser
      security:
        - bearer: []
        - apiKey: []
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
        - name: page
          in: query
          schema:
            type: integer
            default: 1
        # Optional nullable query param: `type: [integer, "null"]` lowers to a nullable
        # `Ty`, which `ty_tokens` renders as `Option<i64>`. The params struct must NOT wrap it again
        # (`Option<Option<i64>>` would not serialize — `Option<i64>: !Display`).
        - name: filter
          in: query
          schema:
            type: [integer, "null"]
        # Rust-keyword-named param: must escape to `r#type` (field, arg, setter, wire name `type`),
        # not a bare `type` keyword token (which failed to parse -> a compile_error! safety net).
        - name: type
          in: query
          schema:
            type: string
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/User"
  # Multi-status responses: TWO success statuses (200/201) with different bodies lower
  # to a typed `GetMultiResponse` enum, and TWO error statuses (404/409) with different bodies to a
  # typed `GetMultiError` enum — no `serde_json::Value`, no `serde(untagged)`. Decode dispatches by
  # HTTP status. Here we compile-verify the enums and construct/deserialize their variants.
  /multi:
    get:
      operationId: getMulti
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        "201":
          description: Created
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiCreated"
        # A documented bodyless success alongside 2+ bodied successes → a payload-free unit variant
        # not silently dropped, decoded without reading a body.
        "204":
          description: No Content
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/NotFoundError"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ConflictError"
  # Two error statuses sharing ONE body schema plus a documented bodyless 401 → a `GetSharedError`
  # enum whose bodied variants agree on `types::Problem`, so it gets `body()` and implements
  # `ApiErrorBody` (the unit variant answers `None`). `getMulti` above is the heterogeneous
  # counterpart and gets neither.
  /shared:
    get:
      operationId: getShared
      responses:
        "200":
          description: OK
        "401":
          description: Unauthorized
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  # The same shape over a NULLABLE component, plus a bodied `default`: every bodied variant is
  # `Option<Box<types::MaybeProblem>>`, `body()` answers `None` for a `null` payload, and the
  # `Default` variant is reached like any other status.
  /maybe:
    get:
      operationId: getMaybe
      responses:
        "200":
          description: OK
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MaybeProblem"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MaybeProblem"
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MaybeProblem"
  # Two bodied successes AND a bodied `default`. `default` is never a success variant: the success
  # enum is exactly `Status200`/`Status201` (an exhaustive match in `tests/blocking.rs` fails to
  # compile if another appears), an undocumented 2xx is `Error::UnexpectedStatus` rather than a
  # `default` decode, and the `default` body is the sole error body, so the error type is the
  # single-body newtype `GetMultiDefaultError(types::Problem)`.
  /multi-default:
    get:
      operationId: getMultiDefault
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        "201":
          description: Created
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiCreated"
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  # The other two success shapes beside the same bodied `default` (issue #151): one bodied success
  # is the plain `MultiOk`, and one bodyless success is `()`. Neither shape names a status, so an
  # undeclared 2xx is that one success — decoded as `MultiOk`, or `()` with the body discarded —
  # never a `default` decode, exactly as the enum shape above never decodes one.
  /plain-default:
    get:
      operationId: getPlainDefault
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  /unit-default:
    get:
      operationId: getUnitDefault
      responses:
        "204":
          description: No Content
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  # A BODYLESS `default` beside one bodied error (issues #127, #204): two error entries, so the
  # error type is the enum `GetBodylessDefaultError { Status404(_), Default }`, and a `500` is the
  # unit `Default` variant — never a `Problem` decoded from whatever body it carried.
  /bodyless-default:
    get:
      operationId: getBodylessDefault
      responses:
        "200":
          description: OK
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
        default:
          description: Anything else
  # The same bodyless `default` beside two bodied errors: now an enum, in which the `default`
  # survives as the trailing unit variant `Default`, so a `500` is `Error::Api` with no body parse.
  /bodyless-default-multi:
    get:
      operationId: getBodylessDefaultMulti
      responses:
        "200":
          description: OK
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
        default:
          description: Anything else
  # Issue #204: a bodyless `403` beside one bodied `404` is its own unit variant of
  # `GetBodylessSiblingError`, where the single-body newtype dropped it and a real `403` arrived as
  # `Error::UnexpectedStatus`.
  /bodyless-sibling:
    get:
      operationId: getBodylessSibling
      responses:
        "200":
          description: OK
        "403":
          description: Forbidden
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  # The same shape with the one error body in XML: the enum arm decodes it through the XML codec
  # (`support::decode_xml_body`), since a lone XML error body is not the rejected two-XML-bodies
  # shape.
  /xml-bodyless-sibling:
    get:
      operationId: getXmlBodylessSibling
      responses:
        "200":
          description: OK
        "403":
          description: Forbidden
        "404":
          description: Not Found
          content:
            application/xml:
              schema:
                $ref: "#/components/schemas/XmlReceipt"
  # A bodyless `304` beside a bodied `default` and no success status: the `304` is its own unit
  # variant ahead of `Default`, where it once matched `default` and had its empty body decoded.
  /conditional:
    get:
      operationId: getConditional
      responses:
        "304":
          description: Not Modified
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Problem"
  # The single-body newtype over the same nullable component: `GetMaybeSingleError(Option<T>)`,
  # whose `ApiErrorBody::Body` is the bare `types::MaybeProblem` so one bound covers it and
  # `GetMaybeError` alike.
  /maybe-single:
    get:
      operationId: getMaybeSingle
      responses:
        "204":
          description: No Content
        "400":
          description: Bad Request
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MaybeProblem"
  # Two error statuses whose bodies are different schemas generating the SAME Rust type: a `$ref`
  # to a string component and an inline string. Their type ids differ, but both are `String`, so
  # `GetAliasSharedError` still gets `body()` and implements `ApiErrorBody`.
  /alias-shared:
    get:
      operationId: getAliasShared
      responses:
        "200":
          description: OK
        "404":
          description: Not Found
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/PlainMessage"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                type: string
  # multipart/form-data request body: the body is an object whose properties are the form
  # parts. `file` is `format: binary` → a `bytes::Bytes` file part; `caption` a required text part;
  # `count` an optional scalar text part; `tags` an optional array → a JSON-encoded text part. The
  # generated method builds a `reqwest::multipart::Form` (compile-verifies the multipart emit AND that
  # the synthesized Cargo.toml enabled reqwest's `multipart` feature and bytes' `serde` feature).
  /upload:
    post:
      operationId: uploadFile
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, caption]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
                count:
                  type: integer
                tags:
                  type: array
                  items:
                    type: string
      responses:
        "204":
          description: No Content
  # Binary in parameter / non-multipart body positions (regression guard): `format: binary`
  # on a param has no faithful byte rendering, so it is represented as `String` (remapped) and stays
  # renderable via `to_string()`; a `format: binary` text/plain body lowers to `bytes::Bytes` and is
  # sent as a raw byte body (`request.body(body.clone())`), never `.to_string()` (`Bytes: !Display`).
  # Compile-verified: without the fixes these positions generate with zero diagnostics yet fail to
  # compile (the forbidden silent non-compile).
  /blob/{token}:
    get:
      operationId: getBlob
      parameters:
        - name: token
          in: path
          required: true
          schema:
            type: string
            format: binary
        - name: cursor
          in: query
          schema:
            type: string
            format: binary
      responses:
        "204":
          description: No Content
  /raw:
    post:
      operationId: postRaw
      requestBody:
        required: true
        content:
          text/plain:
            schema:
              type: string
              format: binary
      responses:
        "204":
          description: No Content
  # A byte string that admits `null` (#104), in every position where `null` has a representation:
  # a JSON member, both ways, and a multipart part become `Option<bytes::Bytes>` whichever way the
  # `null` is spelled, and a query parameter an `Option<String>` (the binary parameter remap). The
  # `type: [string, 'null']` spelling used to lose its `null` here; a *raw* body admitting `null`
  # is rejected instead (`E009`, pinned in `frontend.rs`).
  /nullable-bytes:
    post:
      operationId: postNullableBytes
      parameters:
        - name: cursor
          in: query
          schema: { type: [string, 'null'], format: binary }
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [digest, previous]
              properties:
                digest: { type: [string, 'null'], contentEncoding: base64 }
                previous: { oneOf: [ { type: string, contentEncoding: base64 }, { type: 'null' } ] }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: object
                required: [digest]
                properties:
                  digest: { type: [string, 'null'], contentEncoding: base64 }
  /nullable-parts:
    post:
      operationId: postNullableParts
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties:
                file: { type: [string, 'null'], format: binary }
                thumb: { oneOf: [ { type: string, format: binary }, { type: 'null' } ] }
      responses:
        "204":
          description: No Content
  # Raw textual/vendor and binary response codecs: these bodies are not JSON documents. The
  # generated dispatch must decode UTF-8 text through a JSON string value (preserving typed string
  # schemas) and return binary bodies as bytes without attempting serde_json parsing.
  /render:
    get:
      operationId: renderHtml
      responses:
        "200":
          description: rendered HTML
          content:
            text/html:
              schema: { type: string }
  /octocat:
    get:
      operationId: getOctocat
      responses:
        "200":
          description: octocat art
          content:
            application/octocat-stream:
              schema: { type: string }
  /download:
    get:
      operationId: downloadRaw
      responses:
        "200":
          description: raw bytes
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
  # The OpenAPI 3.1 spelling of the same thing: `format: binary` is gone, so an empty (always-true)
  # Schema Object — and an absent one — say "any octets", as do media ranges naming a binary family.
  # All of them must reach `bytes::Bytes`, and the alternatives here decode identically so no `W014`
  # is reported. `Content-Range` is documented with `content:` rather than `schema:`, which is how a
  # ranged response routinely spells it and which must still produce a typed accessor.
  /ranged:
    put:
      operationId: putRanged
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
      responses:
        "206":
          description: partial content
          headers:
            Content-Range:
              content:
                text/plain: { schema: { type: string } }
          content:
            video/*: { schema: {} }
            audio/*: {}
            application/octet-stream: { schema: {} }
  # The shape from #82: an image proxy whose only honest key is the family, and an upload that
  # names its exact type. Both are `bytes::Bytes`; the upload goes out as `Content-Type: image/png`.
  /artwork/{id}:
    get:
      operationId: getArtwork
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: the image
          content:
            image/*: { schema: {} }
    put:
      operationId: putArtwork
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      requestBody:
        required: true
        content:
          image/png: { schema: {} }
      responses:
        "204": { description: stored }
  /text-error:
    get:
      operationId: getTextError
      deprecated: true
      responses:
        "204": { description: success }
        "400":
          description: textual failure
          content:
            text/plain:
              schema: { type: string }
  /raw-multi:
    get:
      operationId: getRawMulti
      responses:
        "200":
          description: text success
          content:
            text/plain:
              schema: { type: string }
        "201":
          description: binary success
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
        "400":
          description: text error
          content:
            text/plain:
              schema: { type: string }
        "409":
          description: binary error
          content:
            application/octet-stream:
              schema: { type: string, format: binary }
  # XML request + response bodies: both lower to typed structs and are
  # serialized/decoded through the embedded quick-xml codec — compile-verifies that the synthesized
  # Cargo.toml enabled quick-xml (the `xml` feature) and that the embedded `support::xml` helpers
  # (`to_xml`, `decode_success_xml`) compile. `id` carries `xml.attribute` (serde `@id`) and `code`
  # an `xml.name` rename; both are honored, an unsupported `xml.namespace` on `note` warns (W006).
  /xml/order:
    post:
      operationId: submitOrder
      requestBody:
        required: true
        content:
          application/xml:
            schema:
              $ref: "#/components/schemas/XmlOrder"
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema:
                $ref: "#/components/schemas/XmlReceipt"
  # JSON body carrying `xml` metadata (regression guard): the schema has `xml.attribute`
  # and `xml.name` hints but is used only by a JSON operation. The format-agnostic serde rename must
  # NOT be applied (it would corrupt JSON), so `JsonMeta` keeps its `id`/`sku` wire names — the
  # suppression is acknowledged as W006. Round-trip is compile+run verified below.
  /json/meta:
    post:
      operationId: postJsonMeta
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/JsonMeta"
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/JsonMeta"
  # Streaming response: a `text/event-stream` success response lowers to a streaming
  # operation whose method returns `support::EventStream<ChatChunk>` instead of `ResponseValue<T>`.
  # Compile-verifies both the streaming method signature and the embedded runtime `EventStream`
  # (framing + standard Stream plus inherent async `next`) and conditional stream dependencies.
  /chat/stream:
    get:
      operationId: streamChat
      responses:
        "200":
          description: OK
          content:
            text/event-stream:
              schema:
                $ref: "#/components/schemas/ChatChunk"
  # The complete RFC 6570 style table on ONE request, so the wire test below can assert every
  # style, `explode` setting, and `allowReserved` in a single request line. The invariant under
  # test: a style's delimiters are emitted literally while every data byte is percent-encoded, so a
  # joining `,` stays distinguishable from a `,` inside a value.
  /styles/{matrix}/{label}/{raw}:
    get:
      operationId: serializeStyles
      parameters:
        - name: matrix
          in: path
          required: true
          style: matrix
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: label
          in: path
          required: true
          style: label
          explode: false
          schema:
            type: array
            items: { type: string }
        # A path value carrying every character that would change the route if it were spliced in
        # raw: a segment separator, a query separator, a fragment separator, and a stray percent.
        - name: raw
          in: path
          required: true
          schema: { type: string }
        - name: space
          in: query
          style: spaceDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: pipe
          in: query
          style: pipeDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: deep
          in: query
          style: deepObject
          explode: true
          schema:
            $ref: "#/components/schemas/DeepFilter"
        # `allowReserved: true` means reserved characters pass through unencoded — the one place a
        # `/` in a query value is NOT `%2F`.
        - name: reserved
          in: query
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
  # `content:`-typed path and header parameters, one per codec. A path value is rendered through its
  # media codec and then percent-encoded as one opaque segment, so a `/`, `?`, or `#` inside it (a
  # JSON string member included) can never re-target the request; a header value is sent verbatim,
  # as every header value is.
  /content-params/{text}/{json}:
    get:
      operationId: serializeContentParams
      parameters:
        - name: text
          in: path
          required: true
          content:
            text/plain: { schema: { type: string } }
        - name: json
          in: path
          required: true
          content:
            application/json:
              schema: { $ref: "#/components/schemas/DeepFilter" }
        - name: X-Text
          in: header
          required: true
          content:
            text/plain: { schema: { type: string } }
        - name: X-Json
          in: header
          required: true
          content:
            application/json:
              schema: { $ref: "#/components/schemas/DeepFilter" }
      responses:
        "204": { description: No Content }
  # An `application/x-www-form-urlencoded` body with an Encoding Object per property. `tags` opts
  # into RFC 6570 mode (`style` present ⇒ `contentType` is inert); `blob` stays in media-type mode
  # and is JSON-encoded because its declared content type says so.
  /forms:
    post:
      operationId: submitForm
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              required: [name, tags, blob]
              properties:
                name: { type: string }
                tags:
                  type: array
                  items: { type: string }
                blob:
                  $ref: "#/components/schemas/DeepFilter"
            encoding:
              tags:
                style: pipeDelimited
                explode: false
              blob:
                contentType: application/json
      responses:
        "204": { description: No Content }
  # Documented response headers whose schemas are NAMED components. The generated header struct
  # lives beside `Client`, not inside `types`, so an unqualified type path here does not resolve —
  # a bug that only a named (non-primitive) header schema exposes, and only at compile time.
  /documented-headers:
    get:
      operationId: documentedHeaders
      responses:
        "204":
          description: No Content
          headers:
            X-Rate-Limit:
              required: true
              description: Requests remaining in the current window.
              schema:
                $ref: "#/components/schemas/WorkflowId"
            X-Trace-Ids:
              schema:
                type: array
                items:
                  $ref: "#/components/schemas/Mode"
  # No success status declared, only a `404` and a bodied `default` (issue #115): `default` is then
  # the only documentation of a 2xx, so it types the success side (`DeepFilter`) as well as the
  # error enum's catch-all, instead of the success side collapsing to `()` and dropping the body.
  /no-success:
    get:
      operationId: getNoSuccess
      responses:
        "404":
          description: Not Found
          content:
            text/plain:
              schema: { type: string }
        default:
          description: Anything else
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/DeepFilter"
  # One bodied success beside a documented bodyless `204` (issue #121): two outcomes, so the success
  # type is `GetMaybeEmptyResponse` with a `Status204` unit variant rather than a plain `MultiOk`
  # that would decode the `204`'s empty body as a malformed `MultiOk`.
  /maybe-empty:
    get:
      operationId: getMaybeEmpty
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        "204":
          description: No Content
  # The same shape over an XML body: the enum arm decodes it through the XML codec.
  /xml/maybe-empty:
    get:
      operationId: getXmlMaybeEmpty
      responses:
        "200":
          description: OK
          content:
            application/xml:
              schema:
                $ref: "#/components/schemas/XmlReceipt"
        "204":
          description: No Content
  # An exact `200` beside an overlapping `2XX` range, each with its own body: dispatch is exact
  # before range, so a `200` is `Status200` and any other 2xx is `Status2xx`.
  /exact-or-range:
    get:
      operationId: getRanged
      responses:
        "2XX":
          description: Any other success
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiCreated"
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
  # The error-side counterpart of `getRanged`: an exact `409` beside an overlapping `4XX` range,
  # each with its own body, declared range first so the emitted precedence cannot be declaration
  # order. A `409` is `Status409`, any other 4xx is `Status4xx`, and a 5xx is undocumented.
  /error-exact-or-range:
    get:
      operationId: getErrorRanged
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MultiOk"
        "4XX":
          description: Any other client error
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/NotFoundError"
        "409":
          description: Conflict
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ConflictError"
components:
  securitySchemes:
    bearer:
      type: http
      scheme: bearer
    apiKey:
      type: apiKey
      in: header
      name: X-Api-Key
    tenant:
      type: apiKey
      in: header
      name: X-Tenant
    mtls:
      type: mutualTLS
  schemas:
    # A flat object, so `deepObject` is defined for it (the specification leaves nested objects
    # and arrays inside a deepObject value undefined, and spargen rejects those).
    DeepFilter:
      type: object
      required: [kind]
      properties:
        kind: { type: string }
        limit: { type: integer }
    CollisionPayload:
      type: string
    BlankDocs:
      description: ""
      type: string
    MarkdownDocs:
      description: |-
        *   A list item
        continuation text

        > A quoted warning
        continuation text
      type: string
    WorkflowId:
      description: "Workflow identifier\taccepted as a numeric id or file name."
      oneOf:
        - type: integer
        - type: string
    Refined:
      allOf:
        - type: object
          required: [run_id, status, marker, labels, steps, empty_only]
          properties:
            run_id: { type: number }
            status: { type: string }
            marker: { type: [string, "null"] }
            labels:
              type: array
              items: { type: [string, "null"] }
            steps:
              type: array
              items: { type: [object, "null"] }
            empty_only:
              type: array
              items: { type: string }
        - type: object
          required: [run_id, status, marker, labels, steps, empty_only]
          properties:
            run_id: { type: integer }
            status: { type: string, enum: [queued, complete] }
            marker: { type: "null" }
            labels:
              type: array
              items: { type: string }
            steps:
              type: array
              items:
                type: object
                required: [name]
                properties:
                  name: { type: string }
            empty_only:
              type: array
              items: { type: "null" }
    # An optional property whose member types cannot meet is typed uninhabited, and the applied
    # `default` one member declares for it must not survive: a serde default provider returning
    # `Some("a")` for an uninhabited field does not compile. Both spellings of the conjunction —
    # inline `allOf` members, and a `$ref` with sibling `properties` — are held here.
    ConflictDefaultTarget:
      type: object
      properties:
        x: { type: string, default: a }
    ConflictDefault:
      allOf:
        - type: object
          properties:
            x: { type: string, default: a }
        - type: object
          properties:
            x: { type: integer }
    ConflictDefaultSibling:
      $ref: "#/components/schemas/ConflictDefaultTarget"
      properties:
        x: { type: integer }
    # Issue #404: a meet that narrows an optional property re-types the default a member wrote for
    # the wider type. `valid` becomes the enum's variant and `ratio` an integer; `bad` and
    # `fraction` are no value of the narrowed type and lose their default (`W005`). A provider
    # returning `"a".to_owned()` for an enum field, or `2.5` for an `i64`, does not compile.
    NarrowDefaultBase:
      type: object
      properties:
        valid: { type: string, default: a }
        bad: { type: string, default: zzz }
        ratio: { type: number, default: 3 }
        fraction: { type: number, default: 2.5 }
    NarrowDefaultEnum:
      type: object
      properties:
        valid: { enum: [a, b] }
        bad: { enum: [a, b] }
        ratio: { type: integer }
        fraction: { type: integer }
    NarrowDefault:
      allOf:
        - $ref: "#/components/schemas/NarrowDefaultBase"
        - $ref: "#/components/schemas/NarrowDefaultEnum"
    NarrowDefaultSibling:
      $ref: "#/components/schemas/NarrowDefaultBase"
      properties:
        valid: { enum: [a, b] }
        bad: { enum: [a, b] }
        ratio: { type: integer }
        fraction: { type: integer }
    # The direct spelling of an uninhabited optional property: a `false` subschema. Absence is the
    # only valid form, `null` included among the rejected values.
    ForbiddenProperty:
      type: object
      properties:
        x: false
    # A nullable union over `false`: `null` is the one value it admits, so here the `Option` itself
    # is the type and `{"x": null}` must still decode.
    NullOnlyProperty:
      type: object
      properties:
        x:
          anyOf:
            - false
            - type: "null"
    # Optional, non-nullable properties of every kind of type. A present `null` is a value none of
    # these schemas admits, so it is rejected wherever the property's own type rejects it rather
    # than decoding as absent; absence still decodes as `None`. The untyped `anything` admits
    # `null` and keeps it as a present value, and the nullable `maybe` keeps collapsing `null`.
    OptionalFields:
      type: object
      properties:
        name: { type: string }
        count: { type: integer }
        flag: { type: boolean }
        tags:
          type: array
          items: { type: string }
        mode: { $ref: "#/components/schemas/Mode" }
        nested: { $ref: "#/components/schemas/ForbiddenProperty" }
        choice:
          oneOf:
            - type: string
            - type: integer
        colour: { type: string, default: red }
        anything: {}
        maybe: { type: [string, "null"] }
    StringLiteral:
      type: string
      enum: [special]
    AnyString:
      anyOf:
        - type: string
        - $ref: "#/components/schemas/StringLiteral"
    AnyNumber:
      anyOf:
        - type: number
        - type: integer
    OneOverlap:
      oneOf:
        - type: string
        - $ref: "#/components/schemas/StringLiteral"
    BroadOwner:
      type: object
    DetailedOwner:
      type: object
      required: [id]
      properties:
        id: { type: integer }
    AnyOwner:
      anyOf:
        - $ref: "#/components/schemas/BroadOwner"
        - $ref: "#/components/schemas/DetailedOwner"
    ContentFile:
      type: object
      required: [type, content]
      properties:
        type: { type: string, enum: [file] }
        content: { type: string }
    ContentLink:
      type: object
      required: [type, target]
      properties:
        type: { type: string, enum: [symlink] }
        target: { type: string }
    MixedContent:
      oneOf:
        - type: array
          items: { type: string }
        - $ref: "#/components/schemas/ContentFile"
        - $ref: "#/components/schemas/ContentLink"
      discriminator:
        propertyName: type
        mapping:
          file: "#/components/schemas/ContentFile"
          symlink: "#/components/schemas/ContentLink"
    User:
      type: object
      required: [id, name]
      properties:
        id:
          type: string
        external_id:
          type: string
          format: uuid
        created_at:
          type: string
          format: date-time
        name:
          type: string
        tree:
          $ref: "#/components/schemas/TreeNode"
        category:
          $ref: "#/components/schemas/Category"
        dict:
          $ref: "#/components/schemas/Dict"
        alias_node:
          $ref: "#/components/schemas/AliasNode"
        priority:
          $ref: "#/components/schemas/Priority"
        # Discriminated union: an internally-tagged enum over object `$ref` variants.
        pet:
          $ref: "#/components/schemas/Pet"
        animal:
          $ref: "#/components/schemas/LooseAnimal"
        # Undiscriminated but provably-disjoint union (string vs array JSON category): an enum with a
        # content-inspecting custom Deserialize/Serialize — no wrapper on the wire.
        alias:
          $ref: "#/components/schemas/StringOrList"
        # Nullable union variant: the string variant is `{type: [string, null]}`;
        # its nullability is HOISTED to the union so this field is `Option<...>` and a `null` payload
        # resolves to `None` rather than erroring in the custom Deserialize.
        notes:
          $ref: "#/components/schemas/StringListOrNull"
        refined:
          $ref: "#/components/schemas/Refined"
        any_string:
          $ref: "#/components/schemas/AnyString"
        any_number:
          $ref: "#/components/schemas/AnyNumber"
        one_overlap:
          $ref: "#/components/schemas/OneOverlap"
        any_owner:
          $ref: "#/components/schemas/AnyOwner"
        mixed_content:
          $ref: "#/components/schemas/MixedContent"
    # Discriminated union: `petType` selects the object variant. Cat DECLARES `petType` as a required
    # property (the shape that broke serde internal tagging — "missing field petType"); the custom
    # buffer-to-Value Deserialize hands the WHOLE value to the variant, so Cat keeps its own tag.
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
    # Dog does NOT declare `petType`; on serialize the custom Serialize re-inserts the tag.
    Dog:
      type: object
      required: [bark]
      properties:
        bark:
          type: boolean
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        # Two keys name `Cat`; `cat`, the first, is the one serialization writes.
        mapping:
          cat: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
          kitty: Cat
    # An inline member no mapping entry names has no discriminator value (W011); Cat and Dog keep
    # their implicit tags, and the inline member is tried only when the tag names neither.
    LooseAnimal:
      anyOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
        - type: object
          required: [petType, fins]
          properties:
            petType: { type: string }
            fins: { type: integer }
      discriminator:
        propertyName: petType
    # Disjoint by JSON type category: a bare string or a list of strings. Serializes WITHOUT any tag
    # or wrapper — the active variant's inner value is emitted directly.
    StringOrList:
      oneOf:
        - type: string
        - type: array
          items:
            type: string
    # Nullable-variant union: the string member is nullable, hoisted to make the whole union nullable.
    StringListOrNull:
      oneOf:
        - type: [string, "null"]
        - type: array
          items:
            type: string
    # Self-recursive: `parent` is a direct back-edge (→ Option<Box<TreeNode>>) and `children`
    # recurses through an array (→ Vec<TreeNode>; the Vec supplies the indirection). Without
    # boxing the direct `parent` back-edge the
    # generated struct would have infinite size and fail to compile.
    TreeNode:
      type: object
      required: [value]
      properties:
        value:
          type: string
        parent:
          $ref: "#/components/schemas/TreeNode"
        children:
          type: array
          items:
            $ref: "#/components/schemas/TreeNode"
    # Mutually recursive: Category <-> Item. One of the two edges in the cycle is boxed.
    Category:
      type: object
      required: [name]
      properties:
        name:
          type: string
        item:
          $ref: "#/components/schemas/Item"
    Item:
      type: object
      required: [label]
      properties:
        label:
          type: string
        category:
          $ref: "#/components/schemas/Category"
    # Mutual recursion through a nullable **alias** component: `AliasNode.parent` refers to
    # `MaybeAliasNode`, whose whole body is "`AliasNode`, or null" and which therefore names no
    # shape of its own. The alias resolves to its target's still-open reservation, so the back-edge
    # must be BOXED — `Option<Box<AliasNode>>`. `Option<AliasNode>` is an infinitely sized type and
    # does not compile, and this suite is the only one in the repository that compiles generated
    # output: a `frontend.rs` string assertion over the emitted source cannot be relied on to notice
    # the difference, because the embedded runtime supplies `Option<Box<…>>` of its own. Two
    # separate mistakes in the alias's target selection each emitted that type with every other
    # suite green, which is why the shape lives here rather than only there.
    AliasNode:
      type: object
      required: [value]
      properties:
        value:
          type: string
        parent:
          $ref: "#/components/schemas/MaybeAliasNode"
    MaybeAliasNode:
      oneOf:
        - $ref: "#/components/schemas/AliasNode"
        - type: "null"
    # Self-recursive through additionalProperties (→ BTreeMap<String, Dict>; the map supplies
    # the indirection).
    Dict:
      type: object
      additionalProperties:
        $ref: "#/components/schemas/Dict"
    # Null-mixed enum: the `null` member is stripped and the remaining homogeneous string
    # scalars lower as a real Rust enum; the `"null"` in the type array makes every use nullable, so
    # a field of this type is emitted as `Option<Priority>`. An absent or `null` value deserializes
    # to `None`; a string value to the matching variant.
    Priority:
      type: [string, "null"]
      enum: [low, medium, high, null]
    # Propagation of component nullability through `$ref`: a REQUIRED field whose type is
    # the nullable `Priority` component must still be `Option<Priority>` (present, but may be `null`),
    # and an array of the component must be `Vec<Option<Priority>>` (a null element is accepted).
    # Before propagation these emitted `Priority` / `Vec<Priority>` and rejected a conforming `null`.
    Ticket:
      type: object
      required: [priority, history]
      properties:
        priority:
          $ref: "#/components/schemas/Priority"
        history:
          type: array
          items:
            $ref: "#/components/schemas/Priority"
    # `default` on the component schema itself → documented on the generated `Mode` type.
    Mode:
      type: string
      enum: [auto, manual]
      default: auto
    # Exercises schema `default`: representable scalar defaults on optional fields are wired via
    # generated serde providers; a required field's default is rustdoc-only.
    Settings:
      type: object
      required: [retries]
      properties:
        color:
          type: string
          default: red
        enabled:
          type: boolean
          default: true
        ratio:
          type: number
          default: 1.5
        retries:
          type: integer
          default: 3
        # Out-of-range for i32: must NOT be serde-wired (rustdoc-only, W005). If a regression wired
        # `Some(5000000000)` into `Option<i32>`, the generated crate's `cargo check` would fail.
        wide:
          type: integer
          format: int32
          default: 5000000000
        mode:
          $ref: "#/components/schemas/Mode"
          default: auto
    # `patternProperties` composed with an explicit property: the declared `host` field plus a typed
    # overflow map (`#[serde(flatten)] BTreeMap<String, String>`) for the pattern-matched keys. The
    # key regex is validation-only (W001) and not enforced by the map.
    Headers:
      type: object
      properties:
        host:
          type: string
      patternProperties:
        "^x-": { type: string }
    # Object-ness comes *only* from `patternProperties` (no `type`, no `properties`): still a struct
    # with empty fields and a typed overflow map, not an untyped `Any`.
    Tags:
      patternProperties:
        "^tag-": { type: string }
    # A declared property literally named `additional` alongside a typed overflow map: the synthetic
    # flatten field must be allocated in the field scope and disambiguated, or two `pub additional:`
    # fields would collide and the generated crate would fail to compile.
    Bag:
      type: object
      properties:
        additional:
          type: string
      patternProperties:
        "^x-": { type: integer }
    # allOf merge: `Account` flattens a `$ref` base (id, required), an inline member
    # (label, required) and the enclosing schema's own sibling property (owner, optional) into ONE
    # struct. All fields must be present and correctly typed in the generated `Account` type.
    AccountBase:
      type: object
      required: [id]
      properties:
        id:
          type: string
    Account:
      type: object
      properties:
        owner:
          type: string
      allOf:
        - $ref: "#/components/schemas/AccountBase"
        - type: object
          required: [label]
          properties:
            label:
              type: string
    # Distinct bodies for the multi-status `getMulti` operation.
    MultiOk:
      type: object
      required: [ok]
      properties:
        ok:
          type: string
    MultiCreated:
      type: object
      required: [id]
      properties:
        id:
          type: integer
    NotFoundError:
      type: object
      required: [reason]
      properties:
        reason:
          type: string
    ConflictError:
      type: object
      required: [detail]
      properties:
        detail:
          type: string
    # The one body shared by every documented error status of `getShared`.
    Problem:
      type: object
      required: [title, detail]
      properties:
        title:
          type: string
        detail:
          type: string
    # A nullable error body: `getMaybe` and `getMaybeSingle` reference it, so every use is
    # `Option<MaybeProblem>` and a `null` payload is a documented, body-less answer.
    MaybeProblem:
      type: [object, "null"]
      required: [title]
      properties:
        title:
          type: string
    # A string component `getAliasShared` pairs with an inline string body: two schemas, one
    # generated body type.
    PlainMessage:
      type: string
    # Streamed item type for the `/chat/stream` SSE operation.
    ChatChunk:
      type: object
      required: [delta]
      properties:
        delta:
          type: string
    # XML request body: `id` is an XML attribute (serde `@id`), `sku` a plain element.
    XmlOrder:
      type: object
      required: [id, sku]
      properties:
        id:
          type: integer
          xml: { attribute: true }
        sku:
          type: string
    # XML response body: `code` renamed via `xml.name`; `note` carries an unsupported
    # `xml.namespace` hint (→ W006, still generates).
    XmlReceipt:
      type: object
      required: [code]
      properties:
        code:
          type: string
          xml: { name: "ReceiptCode" }
        note:
          type: string
    # JSON-only schema carrying `xml` metadata (regression guard): the same hint shapes as
    # `XmlOrder`, but reachable only from a JSON body — the rename must be suppressed so JSON is
    # correct. Its `xml.namespace` is also the W006 case: on a type never serialized as XML the
    # hint genuinely has no effect, so it warns rather than rejecting.
    JsonMeta:
      type: object
      required: [id, sku]
      properties:
        id:
          type: integer
          xml: { attribute: true }
        sku:
          type: string
          xml: { name: "ProductSku" }
        note:
          type: string
          xml: { namespace: "urn:example:receipt" }
"##;

const SPEC_WITH_UNSUPPORTED_OPERATION: &str = r#"
openapi: 3.1.0
info:
  title: Upload
  version: 1.0.0
paths:
  /health:
    get:
      responses:
        "204":
          description: No Content
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
      responses:
        "204":
          description: No Content
"#;

const OAS32_SPEC: &str = r##"
openapi: 3.2.0
info:
  title: Records
  version: 1.0.0
# A templated server with an enumerated variable: the generated `servers` module must offer a typed
# builder whose default resolves with no arguments, and whose enum makes an illegal region
# unconstructible.
servers:
  - url: https://{region}.example.com/{version}
    variables:
      region:
        default: us
        enum: [us, eu]
      version:
        default: v1
paths:
  # `in: querystring` with a JSON `content:` entry: the whole query string is the serialized value,
  # percent-encoded as one opaque token. Required and optional set the raw query differently (an
  # initializer against a conditional assignment), so both are compile-verified.
  /lookup:
    get:
      operationId: lookupRecords
      parameters:
        - name: filter
          in: querystring
          required: true
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Query"
      responses:
        "204": { description: No Content }
    post:
      operationId: lookupRecordsMaybe
      parameters:
        - name: filter
          in: querystring
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Query"
      responses:
        "204": { description: No Content }
  /records:
    get:
      operationId: listRecords
      parameters:
        - name: filter
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                $ref: "#/components/schemas/Query"
        # `style: cookie` is 3.2-only: the cookie value is sent verbatim, never percent-encoded.
        - name: session
          in: cookie
          style: cookie
          schema: { type: string }
      responses:
        "200":
          description: ok
          headers:
            X-Total-Count:
              required: true
              schema: { type: integer }
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: "#/components/schemas/Record"
    query:
      operationId: searchRecords
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/Query"
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: "#/components/schemas/Record"
  /events:
    get:
      operationId: streamEvents
      responses:
        "200":
          description: ok
          content:
            text/event-stream:
              itemSchema:
                $ref: "#/components/schemas/SseEnvelope"
  # 3.2 `additionalOperations`: a custom method token generates a client method and must reach the
  # wire verbatim. The reusable `components.mediaTypes` reference supplies its request body, and the
  # response is a discriminated union with `defaultMapping` — three constructs that each emit code
  # and, until now, were only ever checked for diagnostics.
  /records/cache:
    additionalOperations:
      PURGE:
        operationId: purgeCache
        requestBody:
          required: true
          content:
            application/json:
              $ref: "#/components/mediaTypes/QueryBody"
        responses:
          "200":
            description: ok
            content:
              application/json:
                schema:
                  $ref: "#/components/schemas/CacheOutcome"
components:
  schemas:
    Record:
      type: object
      required: [id]
      properties:
        id: { type: string }
        name: { type: string }
    Query:
      type: object
      properties:
        term: { type: string }
    SseEnvelope:
      type: object
      required: [data]
      properties:
        data:
          type: string
          contentMediaType: application/json
          contentSchema:
            $ref: "#/components/schemas/AdminEvent"
        id: { type: [string, "null"] }
        retry: { type: [integer, "null"] }
    AdminEvent:
      type: object
      required: [kind]
      properties:
        kind: { type: string }
        resource: { type: [string, "null"] }
    CacheOutcome:
      oneOf:
        - $ref: "#/components/schemas/CachePurged"
        - $ref: "#/components/schemas/CacheUnknown"
      discriminator:
        propertyName: outcome
        mapping:
          purged: "#/components/schemas/CachePurged"
        # 3.2: an absent or unrecognized discriminator value decodes into this branch instead of
        # failing, and spargen generates exactly that fallback.
        defaultMapping: "#/components/schemas/CacheUnknown"
    CachePurged:
      type: object
      required: [outcome, entries]
      properties:
        outcome: { type: string }
        entries: { type: integer }
    CacheUnknown:
      type: object
      required: [detail]
      properties:
        detail: { type: string }
  mediaTypes:
    QueryBody:
      schema:
        $ref: "#/components/schemas/Query"
"##;

/// Whether the `wasm32-unknown-unknown` target's std is installed, so the wasm gate can run. When
/// `rustup` reports the installed targets and it is absent, the gate self-skips rather than failing
/// on a toolchain gap; without `rustup` we assume the target is present (CI installs it).
fn wasm32_target_installed() -> bool {
    match Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == "wasm32-unknown-unknown"),
        _ => true,
    }
}

/// THE gate: a generated client must compile for `wasm32-unknown-unknown` (the browser,
/// via reqwest's `fetch` backend), where reqwest's client/request/response and fetch futures are
/// `!Send`. The `BASIC_SPEC` exercises the wasm-sensitive surface — the transport seam, the auth
/// token provider, and a streaming `EventStream` operation — so `cargo check --target
/// wasm32-unknown-unknown` succeeding proves the conditional `MaybeSend`/`MaybeSync` bounds, the
/// `cfg`-gated boxed-future aliases, the wasm `EventStream` buffer path, and the target-gated
/// manifest (native-only tokio, wasm-gated `BlockingClient`) all hold together.
#[test]
fn generated_crate_compiles_for_wasm32_browser_target() {
    if !wasm32_target_installed() {
        eprintln!(
            "skipping wasm32 gate: target `wasm32-unknown-unknown` is not installed (rustup target add wasm32-unknown-unknown)"
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("wasm_client");

    let report = generate_fixture_crate(&spec, &out, "wasm_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    // Default features: the client, transport seam, middleware/retry helpers, auth token provider,
    // and streaming `EventStream` must all compile against reqwest's `!Send` wasm `fetch` backend.
    let status = fixture_cargo(&out)
        .args(["check", "--target", "wasm32-unknown-unknown"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated crate must `cargo check --target wasm32-unknown-unknown`"
    );

    // With the opt-in `blocking` feature enabled, a wasm build must STILL compile: the tokio-backed
    // `BlockingClient` and the tokio dependency are both gated off wasm, so the browser build never
    // pulls a runtime it cannot run.
    let status = fixture_cargo(&out)
        .args([
            "check",
            "--target",
            "wasm32-unknown-unknown",
            "--features",
            "blocking",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "wasm build must compile even with the `blocking` feature enabled (no tokio pulled)"
    );
}

/// The hidden proc-macro bridge renders deterministically without touching the output path.
#[test]
fn macro_preview_is_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap();

    let config = Spec::new(spec);
    let preview = spargen::__private::preview(&config);
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        preview.report
    );
    let contents = preview.contents.expect("generated module");
    assert!(
        !out.exists(),
        "macro preview must not write the output path"
    );
    let again = spargen::__private::preview(&config);
    assert_eq!(again.contents.as_deref(), Some(contents.as_str()));
}

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
    assert!(
        messages.contains("feature `json` on `reqwest`"),
        "{messages}"
    );
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
    // The requirement set really did include both crates: the module uses them. `mod stream` is
    // embedded only for a sequential response (the word `EventStream` alone rides in doc comments
    // every module carries), and `Uuid` appears only through the `format: uuid` mapping.
    let generated = preview.contents.expect("generated module");
    assert!(generated.contains("mod stream"), "{generated}");
    assert!(generated.contains("Uuid"), "{generated}");
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
        let output = fixture_cargo(member.parent().unwrap())
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
  /json:
    post:
      operationId: postJson
      requestBody:
        content:
          application/json:
            schema: { type: string }
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
        reqwest.no_default_features && reqwest.features.contains(&"json"),
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
                    r#"reqwest = { workspace = true, features = ["json"] }"#,
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
                r#"reqwest = { version = "{version}", path = "../stubs/reqwest", features = ["json"] }"#,
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
                r#"reqwest = { version = "{version}", path = "../stubs/reqwest", features = ["json"] }"#,
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

/// A spec that turns on every capability the requirement table knows: a JSON body (`reqwest/json`),
/// a multipart body (`reqwest/multipart`), a binary array inside JSON (`bytes/serde`), an XML body
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
    let metadata = fixture_cargo(manifest.parent().unwrap())
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
    for feature in ["json", "multipart", "stream"] {
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
    let opted_in = block
        .replace("# [target", "[target")
        .replace("# tokio", "tokio");
    assert_ne!(
        opted_in, block,
        "the blocking opt-in is rendered commented out:\n{block}"
    );
    let blocking = "[features]\nblocking = [\"dep:tokio\"]\n";

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
            format!(
                "{}\n{blocking}\n{opted_in}\n[workspace]\n",
                package("direct-blocking")
            ),
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
        let declared = spargen::RequiredDependency {
            optional: false,
            ..dependency.clone()
        };
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

/// A preview of a spec that uses an unsupported construct rejects loudly (matching `generate`) and
/// retains no files — the proc-macro relies on this to raise a `compile_error!` instead of emitting
/// half-generated code.
#[test]
fn preview_of_rejected_spec_has_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    // OpenAPI 3.0.x is rejected at the version gate (E001) — a reliable rejection with no codegen.
    std::fs::write(
        &spec,
        BASIC_SPEC.replace("openapi: 3.1.0", "openapi: 3.0.3"),
    )
    .unwrap();

    let preview = spargen::__private::preview(&Spec::new(spec));
    assert_eq!(preview.report.outcome(), Outcome::Rejected);
    assert!(
        preview.contents.is_none(),
        "a rejected preview retains no generated module"
    );
    assert!(preview
        .report
        .diagnostics()
        .iter()
        .any(|d| d.code == Code::UnsupportedOpenApiVersion));
}

/// `format: date-time` and `format: date` must reach the wire as RFC 3339 strings.
///
/// This is the test whose absence let a wire defect ship: every other date fixture only
/// *compile*-checks the mapping, and `time`'s own serde representation compiles perfectly well
/// while emitting a nine-element integer sequence (without its `serde-human-readable` feature) or a
/// space-separated `2023-11-14 22:13:20.0 +00:00:00` (with it). Neither is RFC 3339, which is what
/// JSON Schema 2020-12 — and therefore OpenAPI 3.1/3.2 — defines these formats to be. So the
/// assertions here are on the bytes, in a request body, a response body, and two query parameters.
#[test]
fn date_and_date_time_reach_the_wire_as_rfc3339() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, DATE_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "dates_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    // The model resolves to the embedded newtypes, not to `time`'s own types — naming those in a
    // model is exactly the defect, since they carry the non-RFC-3339 serde implementation. (The
    // newtype *definitions* name them, which is why this checks the aliases rather than the file.)
    assert!(
        generated.contains("pub type Eventat = DateTime;"),
        "a date-time property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        generated.contains("pub type Eventday = Date;"),
        "a date property must resolve to the RFC 3339 newtype: {generated}"
    );
    assert!(
        !generated.contains("pub type Eventat = time::")
            && !generated.contains("pub type Eventday = time::"),
        "no model alias may name time's own serde types"
    );
    assert!(
        generated.contains("pub struct DateTime(pub time::OffsetDateTime)"),
        "the RFC 3339 newtype module must be embedded for a spec that uses dates"
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/dates.rs"), DATE_WIRE_TEST).unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "dates"])
        .status()
        .unwrap();
    assert!(status.success(), "the date-time wire round-trip must pass");
}

/// A spec that puts both date formats in a request body, a response body, and query parameters —
/// the four positions a date value can reach the wire from.
const DATE_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Dates, version: 1.0.0 }
paths:
  /events:
    post:
      operationId: createEvent
      parameters:
        - name: since
          in: query
          schema: { type: string, format: date-time }
        - name: on
          in: query
          schema: { type: string, format: date }
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Event" }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Event" }
components:
  schemas:
    Event:
      type: object
      required: [at, day]
      properties:
        at: { type: string, format: date-time }
        day: { type: string, format: date }
"##;

const DATE_WIRE_TEST: &str = r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn dates_are_rfc3339_on_the_wire_in_both_directions() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]);
        let request_line = request.lines().next().unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");

        // Query parameters: RFC 3339 text, percent-encoded as query data (`:` -> `%3A`). A
        // sequence-serialized datetime could not appear here at all.
        assert!(
            request_line.contains("since=2023-11-14T22%3A13%3A20Z"),
            "date-time query parameter must be RFC 3339: {request_line}"
        );
        assert!(
            request_line.contains("on=2023-11-14"),
            "date query parameter must be a full-date: {request_line}"
        );

        // Request body: JSON strings, not the nine- and two-element integer arrays `time`'s own
        // `Serialize` produces without `serde-human-readable`.
        assert_eq!(
            body, r#"{"at":"2023-11-14T22:13:20Z","day":"2023-11-14"}"#,
            "date fields must serialize as RFC 3339 strings: {body}"
        );

        let payload = r#"{"at":"2024-02-29T01:02:03.5+05:30","day":"2024-02-29"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let base = format!("http://{addr}");
    let client = dates_client::BlockingClient::new(&base).unwrap();

    let at = dates_client::DateTime(
        time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
    );
    let day = dates_client::Date(
        time::Date::from_calendar_date(2023, time::Month::November, 14).unwrap(),
    );
    let event = dates_client::types::Event { at, day };

    let params = dates_client::CreateEventParams::default()
        .since(at)
        .on(day);
    let response = client
        .create_event(Some(params), &event)
        .expect("create_event round-trips");

    // Decoding: an offset and a subsecond survive the round trip as the server wrote them.
    let decoded = response.into_inner();
    assert_eq!(decoded.day.to_string(), "2024-02-29");
    assert_eq!(decoded.at.to_string(), "2024-02-29T01:02:03.5+05:30");
    // The newtype is transparent: `time`'s API is one deref away.
    assert_eq!(decoded.at.year(), 2024);

    server.join().unwrap();
}
"##;

/// A Path Item `servers` override must send that operation to a *different host*, while its
/// siblings keep the client's base URL.
///
/// This is the wire half of the fix: the runtime already had `build_url_on` for exactly this, with
/// its own unit test, but codegen never passed anything but `None` — so an override compiled fine
/// and silently went to the wrong server. Only a second listener can tell the two apart.
#[test]
fn a_server_override_sends_the_operation_to_another_host() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // Bound before generation: the override URL is baked into the generated code, so the port has
    // to be known first. This listener is served from *this* process while the generated crate's
    // test process drives the client against it.
    let override_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let override_addr = override_listener.local_addr().unwrap();
    let override_server = std::thread::spawn(move || {
        let (mut stream, _) = override_listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        format!(
            r##"
openapi: 3.1.0
info: {{ title: Servers, version: 1.0.0 }}
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "204": {{ description: No Content }}
  /upload:
    servers:
      - url: http://{override_addr}
    post:
      operationId: uploadItem
      responses:
        "204": {{ description: No Content }}
"##
        ),
    )
    .unwrap();
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "servers_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/servers.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

// The base-URL operation must reach the client's own base, proving the override is scoped to the
// path item that declares it rather than applied to the whole client.
#[test]
fn the_unoverridden_operation_still_uses_the_client_base_url() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let base_server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let base = format!("http://{addr}");
    let client = servers_client::BlockingClient::new(&base).unwrap();

    // Goes to the client's base URL.
    client.list_pets().expect("list_pets round-trips");
    let seen = base_server.join().unwrap();
    assert!(
        seen.starts_with("GET /pets "),
        "the base server should have served /pets: {seen}"
    );

    // Goes to the override host, which lives in the *parent* test process; reaching it at all is
    // the proof, since this process never bound that port.
    client.upload_item().expect("upload_item round-trips");
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "servers"])
        .status()
        .unwrap();
    assert!(status.success(), "the server-override round-trip must pass");

    let seen = override_server.join().unwrap();
    assert!(
        seen.starts_with("POST /upload "),
        "the override host should have served /upload: {seen}"
    );
    assert!(
        seen.contains(&format!("host: {override_addr}")),
        "the request must carry the override host, not the document server: {seen}"
    );
}

/// RFC 6570-mode `multipart/form-data` parts must carry literal delimiters, not percent-encoded
/// ones.
///
/// The specification is explicit that "when using RFC6570-style serialization for
/// `multipart/form-data`, URI percent-encoding MUST NOT be applied", but the part values were built
/// from the query-fragment builders, whose delimiters are pre-encoded `%20` / `%7C` triples. Only a
/// look at the raw body catches that; the existing multipart fixtures all use `contentType` mode.
#[test]
fn multipart_rfc6570_parts_carry_literal_delimiters() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        r##"
openapi: 3.1.0
info: { title: Multipart, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [tags, paths, names]
              properties:
                tags:
                  type: array
                  items: { type: string }
                paths:
                  type: array
                  items: { type: string }
                names:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: spaceDelimited
                explode: false
              paths:
                style: pipeDelimited
                explode: false
              names:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##,
    )
    .unwrap();
    let out = temp.path().join("client");
    let report = generate_fixture_crate(&spec, &out, "multipart_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/multipart.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn rfc6570_multipart_parts_are_not_percent_encoded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        request
    });

    let base = format!("http://{addr}");
    let client = multipart_client::BlockingClient::new(&base).unwrap();
    let body = multipart_client::types::RequestBody {
        tags: vec!["blue".into(), "black".into()],
        paths: vec!["a/b".into(), "c".into()],
        names: vec!["ada".into(), "grace".into()],
    };
    client.upload(&body).expect("upload round-trips");

    let request = server.join().unwrap();

    // `spaceDelimited` joins with a literal space, `pipeDelimited` with a literal `|`.
    assert!(
        request.contains("blue black"),
        "spaceDelimited must join with a literal space: {request}"
    );
    assert!(
        request.contains("a/b|c"),
        "pipeDelimited must join with a literal pipe: {request}"
    );
    // The encoded forms are exactly the defect.
    assert!(
        !request.contains("blue%20black"),
        "a part value must not be percent-encoded: {request}"
    );
    assert!(
        !request.contains("%7C"),
        "a part delimiter must not be percent-encoded: {request}"
    );
    // `form` + `explode` sends one part per item, under the same name (RFC 7578 s4.3).
    assert_eq!(
        request.matches(r#"name="names""#).count(),
        2,
        "an exploded array must send one part per item: {request}"
    );
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "multipart"])
        .status()
        .unwrap();
    assert!(status.success(), "the multipart wire round-trip must pass");
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
reqwest = { version = "0.12.28", default-features = false, features = ["json"] }
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

/// A response header and a server variable may be spelled with a Rust keyword. Both are allocated
/// with `IdentRole::Field`, so both escape to `r#type` — and both were then rebound through
/// `proc_macro2::Ident::new`, which *panics* on a raw identifier ("`r#type` is not a valid Ident").
/// A perfectly legal description crashed the generator rather than emitting anything. The
/// `cargo check` below is what pins the second half: the emitted `r#type` must also compile.
const KEYWORD_HEADER_AND_SERVER_VARIABLE_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{type}.{match}.example.com/{enum}"
    variables:
      type: { default: api }
      match: { default: eu, enum: [eu, us] }
      enum: { default: v1 }
paths:
  /h:
    get:
      operationId: h
      responses:
        "200":
          description: ok
          headers:
            type: { schema: { type: string } }
            ref: { schema: { type: string } }
            x-normal: { schema: { type: string } }
"#;

#[test]
fn a_keyword_header_or_server_variable_generates_compiling_code() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("api.yaml");
    std::fs::write(&spec, KEYWORD_HEADER_AND_SERVER_VARIABLE_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "keyword_ident_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    for raw in ["r#type", "r#match", "r#enum", "r#ref"] {
        assert!(
            generated.contains(raw),
            "a keyword header/server-variable name must survive as a raw identifier, missing \
             `{raw}`:\n{generated}"
        );
    }

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(
        status.success(),
        "a keyword-named header or server variable must generate compiling code"
    );
}

/// `gen` is reserved in edition 2024 but an ordinary identifier in 2021. Generated output is a
/// freestanding module compiled under the *consumer's* edition, so a spec that names anything `gen`
/// must still emit code an edition-2024 crate accepts. This fixture puts `gen` in every position a
/// spec can put it — operation, path and query parameter, schema property (including one with a
/// default, which also names a generated provider fn), response header, and server variable — and
/// compiles the result under edition 2024, which is the only oracle that can catch this: the
/// `name` proptests lex with `proc_macro2` under edition 2021, where `gen` is perfectly legal.
const GEN_KEYWORD_SPEC: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{gen}.example.com"
    variables:
      gen: { default: api }
paths:
  /gen/{gen}:
    get:
      operationId: gen
      parameters:
        - { name: gen, in: path, required: true, schema: { type: string } }
        - { name: gen2, in: query, required: false, schema: { type: string } }
      responses:
        "200":
          description: ok
          headers:
            gen: { schema: { type: string } }
          content:
            application/json: { schema: { $ref: "#/components/schemas/gen" } }
components:
  schemas:
    gen:
      type: object
      required: [gen]
      properties:
        gen: { type: string }
        gen_defaulted: { type: string, default: "d" }
"##;

#[test]
fn a_gen_named_spec_compiles_under_edition_2024() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("api.yaml");
    std::fs::write(&spec, GEN_KEYWORD_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate_in_edition(&spec, &out, "gen_client", "2024");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        generated.contains("r#gen"),
        "`gen` must be raw-escaped for an edition-2024 consumer:\n{generated}"
    );
    // The wire name is carried by an explicit `rename`, so escaping the Rust ident cannot move it.
    assert!(
        generated.contains(r#"rename = "gen""#),
        "escaping must not change the wire name:\n{generated}"
    );

    let output = fixture_cargo(&out).arg("check").output().unwrap();
    assert!(
        output.status.success(),
        "a spec naming things `gen` must compile for an edition-2024 consumer:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The RFC 9457 shape from #268: each documented error status narrows the shared `Problem`'s
/// `type` with a `const`, spelled as an `allOf` member (`404`) and as `$ref` siblings (`409`). The
/// positions `open_narrowing` leaves closed sit beside them: a union of narrowed problems (`400`),
/// a narrowing inside a component (`410`), a set narrowed against a `uuid` string in either member
/// order (`428`, `429`), and a request body.
const OPEN_NARROWING_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Problems, version: 1.0.0 }
paths:
  /problems:
    post:
      operationId: postProblems
      requestBody:
        required: true
        content:
          application/json:
            schema:
              allOf:
                - $ref: "#/components/schemas/Problem"
                - properties: { type: { const: "https://example.com/probs/request" } }
      responses:
        "200":
          description: a narrowed success body opens too
          content:
            application/json:
              schema:
                allOf:
                  - $ref: "#/components/schemas/Problem"
                  - properties: { type: { const: "https://example.com/probs/none" } }
        "400":
          description: a union of narrowed problems stays closed
          content:
            application/problem+json:
              schema:
                oneOf:
                  - allOf:
                      - $ref: "#/components/schemas/Problem"
                      - properties: { type: { const: "https://example.com/probs/a" } }
                  - allOf:
                      - $ref: "#/components/schemas/Problem"
                      - properties: { type: { const: "https://example.com/probs/b" } }
        "403":
          description: the narrowing value is itself a component, which stays closed
          content:
            application/problem+json:
              schema:
                allOf:
                  - $ref: "#/components/schemas/Problem"
                  - properties: { type: { $ref: "#/components/schemas/ForbiddenType" } }
        "404":
          description: not found
          content:
            application/problem+json:
              schema:
                allOf:
                  - $ref: "#/components/schemas/Problem"
                  - properties: { type: { const: "https://example.com/probs/not-found" } }
        "409":
          description: conflict, narrowed beside the `$ref`, with a value spelled `other`
          content:
            application/problem+json:
              schema:
                $ref: "#/components/schemas/Problem"
                properties: { type: { enum: ["https://example.com/probs/conflict", "other"] } }
        "410":
          description: a component's narrowing stays closed
          content:
            application/problem+json:
              schema: { $ref: "#/components/schemas/GoneProblem" }
        "422":
          description: a response's own set met by a union, the union last, keeps its variants closed
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - { type: string }
                      - { enum: [a, b, c] }
                      - oneOf: [{ const: a }, { const: b }]
        "423":
          description: the same set with the union first
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - oneOf: [{ const: a }, { const: b }]
                      - { enum: [a, b, c] }
                      - { type: string }
        "424":
          description: a union that narrows to its one string branch, the union last
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - { type: string }
                      - { enum: [a, b, c] }
                      - oneOf: [{ type: string }, { type: integer }]
        "425":
          description: the same union with the union first
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - oneOf: [{ type: string }, { type: integer }]
                      - { enum: [a, b, c] }
                      - { type: string }
        "426":
          description: a union keeping a plain string branch beside another, the union last
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - { type: string }
                      - { enum: [a, b, c] }
                      - oneOf: [{ const: a }, { type: string }]
        "427":
          description: the same union with the union first
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - oneOf: [{ const: a }, { type: string }]
                      - { enum: [a, b, c] }
                      - { type: string }
        "428":
          description: a set narrowed against a uuid string after a plain string opened it
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - { type: string }
                      - { enum: ["00000000-0000-0000-0000-000000000001"] }
                      - { type: string, format: uuid }
        "429":
          description: the same set with the uuid string first and the plain string last
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
                      - { type: string, format: uuid }
                      - { enum: ["00000000-0000-0000-0000-000000000001"] }
                      - { type: string }
components:
  schemas:
    Problem:
      type: object
      required: [type, title]
      properties:
        type: { type: string }
        title: { type: string }
        detail: { type: string }
    GoneProblem:
      allOf:
        - $ref: "#/components/schemas/Problem"
        - properties: { type: { const: "https://example.com/probs/gone" } }
    ForbiddenType:
      const: "https://example.com/probs/forbidden"
"##;

#[test]
fn open_narrowing_decodes_an_unlisted_problem_type_and_keeps_it_typed() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("problems.yaml");
    std::fs::write(&spec, OPEN_NARROWING_SPEC).unwrap();

    // The option off: the narrowing is exact, as before.
    let closed = temp.path().join("closed");
    let report = generate_fixture_crate(&spec, &closed, "closed_problems");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let generated = std::fs::read_to_string(closed.join("src/lib.rs")).unwrap();
    assert!(
        !generated.contains("Other(String)"),
        "without the option no set is open:\n{generated}"
    );

    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "open_problems", "2021", |spec| {
        spec.open_narrowing(true)
    });
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        report.diagnostics().is_empty(),
        "the option reports nothing: {report:#?}"
    );
    // Exactly the positions it applies to open: the `200`, `403`, `404`, and `409` bodies, and the
    // `424`/`425` `kind`, whose union narrows to one branch. The `{enum: [a, b, c]}` members of
    // `422`, `423`, `426`, and `427` open in place too, as intermediates no field uses: a union
    // that keeps two branches meets them into closed branch sets (`tests/open.rs` decodes both).
    // The `428`/`429` `kind`, narrowed against a `uuid` string, stays closed in either order.
    // Only an open enum emits `as_str`.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert_eq!(
        generated.matches("pub fn as_str(&self) -> &str").count(),
        10,
        "{generated}"
    );

    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the open enums must pass clippy -D warnings"
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/open.rs"),
        r##"use open_problems::{Error, PostProblemsError, ResponseValue};

/// Decode `json` as the body type the variant constructor `_variant` carries, without naming the
/// generated (hash-disambiguated) type.
fn decode<T: serde::de::DeserializeOwned, E>(
    _variant: fn(Box<T>) -> E,
    json: &str,
) -> Result<T, serde_json::Error> {
    serde_json::from_str(json)
}

#[test]
fn a_listed_value_is_its_own_variant_and_an_unlisted_one_is_kept() {
    let listed = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/not-found","title":"t"}"#,
    )
    .expect("a listed type decodes");
    assert_eq!(format!("{:?}", listed.r#type), "HttpsExampleComProbsNotFound");
    assert_eq!(listed.r#type.as_str(), "https://example.com/probs/not-found");

    // A problem type the description does not list: it decodes, and keeps its value.
    let unlisted = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/moved","title":"t","detail":"d"}"#,
    )
    .expect("an unlisted type still decodes");
    assert_eq!(
        format!("{:?}", unlisted.r#type),
        r#"Other("https://example.com/probs/moved")"#
    );
    assert_eq!(unlisted.r#type.to_string(), "https://example.com/probs/moved");
    // It is still a string on the wire, both ways.
    let wire = serde_json::to_value(&unlisted).unwrap();
    assert_eq!(wire["type"], "https://example.com/probs/moved");
    // A non-string is refused: the open set's domain is the `string` it narrowed.
    assert!(decode(PostProblemsError::Status404, r#"{"type":7,"title":"t"}"#).is_err());
}

#[test]
fn the_ref_sibling_spelling_opens_and_a_value_spelled_other_keeps_its_name() {
    let other = decode(PostProblemsError::Status409, r#"{"type":"other","title":"t"}"#).unwrap();
    // The listed `other` keeps `Other`; the catch-all takes the next name.
    assert_eq!(format!("{:?}", other.r#type), "Other");
    let unlisted =
        decode(PostProblemsError::Status409, r#"{"type":"elsewhere","title":"t"}"#).unwrap();
    assert!(
        format!("{:?}", unlisted.r#type).ends_with(r#"("elsewhere")"#),
        "{unlisted:?}"
    );
}

#[test]
fn a_union_and_a_component_stay_closed() {
    // Each union variant still refuses the other's type, so exactly one matches...
    assert!(decode(
        PostProblemsError::Status400,
        r#"{"type":"https://example.com/probs/b","title":"t"}"#,
    )
    .is_ok());
    // ...and a type neither lists matches neither.
    assert!(decode(
        PostProblemsError::Status400,
        r#"{"type":"https://example.com/probs/c","title":"t"}"#,
    )
    .is_err());
    assert!(decode(
        PostProblemsError::Status410,
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .is_err());
    // A narrowing through a `$ref`'d value opens a copy in the response and leaves the component
    // closed for its other uses.
    let copy = decode(
        PostProblemsError::Status403,
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .expect("the response's own copy is open");
    assert_eq!(copy.r#type.as_str(), "https://example.com/probs/moved");
    assert!(serde_json::from_str::<open_problems::types::ForbiddenType>(
        r#""https://example.com/probs/moved""#
    )
    .is_err());
    // A request body is never opened.
    assert!(serde_json::from_str::<open_problems::types::RequestBody>(
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .is_err());
}

#[test]
fn a_union_meeting_an_open_set_decodes_in_either_member_order() {
    // `422` writes the union last, after the set has opened; `423` writes it first. Either way
    // each variant stays closed, so a listed value matches exactly one of them.
    for (order, decoded) in [
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"a"}"#).map(|_| ())),
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"b"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"a"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"b"}"#).map(|_| ())),
    ] {
        decoded.unwrap_or_else(|error| panic!("{order}: a value one variant lists: {error}"));
    }
    // A value the set lists but no variant does, and one nothing lists, match no variant.
    for kind in ["c", "z"] {
        let json = format!(r#"{{"kind":"{kind}"}}"#);
        assert!(decode(PostProblemsError::Status422, &json).is_err(), "union last: {kind}");
        assert!(decode(PostProblemsError::Status423, &json).is_err(), "union first: {kind}");
    }
    // A union that narrows to one branch is no union: in either order the result is the response's
    // own open set, so an unlisted value is kept.
    for kind in ["a", "z"] {
        let json = format!(r#"{{"kind":"{kind}"}}"#);
        let last = decode(PostProblemsError::Status424, &json).expect("union last");
        let first = decode(PostProblemsError::Status425, &json).expect("union first");
        assert_eq!(last.kind.as_str(), kind);
        assert_eq!(first.kind.as_str(), kind);
    }
    // A plain `string` branch kept beside another stays closed in either order: `b` matches it
    // alone, `a` matches both branches (which `oneOf` forbids), and `z` matches neither.
    macro_rules! closed_beside_another {
        ($status:expr, $order:literal) => {
            decode($status, r#"{"kind":"b"}"#).unwrap_or_else(|error| panic!("{}: {error}", $order));
            assert!(decode($status, r#"{"kind":"a"}"#).is_err(), "{}: a", $order);
            assert!(decode($status, r#"{"kind":"z"}"#).is_err(), "{}: z", $order);
        };
    }
    closed_beside_another!(PostProblemsError::Status426, "union last");
    closed_beside_another!(PostProblemsError::Status427, "union first");
}

#[test]
fn a_set_narrowed_against_a_uuid_stays_closed_in_either_member_order() {
    // `428` opens the set before the `uuid` string meets it; `429` meets the `uuid` string first.
    // Either way the listed value decodes and an unlisted one, a uuid or not, is refused.
    macro_rules! closed_against_uuid {
        ($status:expr, $order:literal) => {
            decode($status, r#"{"kind":"00000000-0000-0000-0000-000000000001"}"#)
                .unwrap_or_else(|error| panic!("{}: listed: {error}", $order));
            assert!(
                decode($status, r#"{"kind":"00000000-0000-0000-0000-000000000002"}"#).is_err(),
                "{}: unlisted uuid",
                $order
            );
            assert!(decode($status, r#"{"kind":"z"}"#).is_err(), "{}: unlisted string", $order);
        };
    }
    closed_against_uuid!(PostProblemsError::Status428, "uuid last");
    closed_against_uuid!(PostProblemsError::Status429, "uuid first");
}

#[test]
fn the_problem_reader_reads_the_opened_type() {
    let body = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/moved","title":"t","detail":"d"}"#,
    )
    .unwrap();
    let error = Error::Api(ResponseValue::new(
        reqwest::StatusCode::NOT_FOUND,
        Default::default(),
        PostProblemsError::Status404(Box::new(body)),
    ));
    let problem = error.problem().expect("an object body");
    assert_eq!(problem.problem_type.as_deref(), Some("https://example.com/probs/moved"));
    assert_eq!(problem.detail.as_deref(), Some("d"));
}
"##,
    )
    .unwrap();
    let status = fixture_cargo(&out)
        .args(["test", "--test", "open"])
        .status()
        .unwrap();
    assert!(status.success(), "the open enums must decode as documented");
}
