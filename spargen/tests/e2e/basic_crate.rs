//! The fixture crate generated from `BASIC_SPEC`: compiled, linted, and driven over HTTP by the
//! tests it carries, plus the other spellings of that spec and the macro preview.

use super::*;

/// The variant names of the runtime enum `name`, read from the runtime source file `file` that the
/// generated module embeds.
fn runtime_enum_variants(file: &str, name: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/support/runtime")
        .join(file);
    let source = std::fs::read_to_string(&path).unwrap();
    let open = format!("pub enum {name} {{\n");
    let start = source
        .find(&open)
        .unwrap_or_else(|| panic!("{path:?} declares `{name}`"))
        + open.len();
    let body = &source[start..start + source[start..].find("\n}").unwrap()];
    let variants: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
        .map(|line| {
            line.split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(!variants.is_empty(), "`{name}` has no variants in {path:?}");
    variants
}

/// Every `support::AuthKind` and every `support::Framing` the runtime declares is emitted by
/// [`BASIC_SPEC`] or [`OAS32_SPEC`], the two specs whose fixture crates
/// `generated_module_compiles_in_basic_oas31_crate` and
/// `generated_module_compiles_in_oas32_crate_with_query_method` hold to `cargo check` and
/// `cargo clippy -D warnings`. A variant neither emits is generated code no gate compiles: extend
/// one of the specs until it emits it.
#[test]
fn the_compiled_specs_emit_every_auth_kind_and_stream_framing() {
    let emitted =
        basic_spec_module(|spec| spec) + &spec_module("openapi.yaml", OAS32_SPEC, |spec| spec);
    for (file, name) in [("auth.rs", "AuthKind"), ("stream.rs", "Framing")] {
        for variant in runtime_enum_variants(file, name) {
            let path = format!("support::{name}::{variant}");
            assert!(
                emitted.contains(&format!("{path},")) || emitted.contains(&format!("{path}(")),
                "no compiled e2e spec emits `{path}`"
            );
        }
    }
}

/// `value` as JSON text, keeping YAML's key order — `serde_json::Map` would sort the keys, and
/// document order is what orders the emitted items.
fn yaml_as_json(value: &yaml_rust2::Yaml, out: &mut String) {
    use yaml_rust2::Yaml;
    match value {
        Yaml::Null => out.push_str("null"),
        Yaml::Boolean(boolean) => out.push_str(&boolean.to_string()),
        Yaml::Integer(integer) => out.push_str(&integer.to_string()),
        Yaml::Real(real) => {
            let number: f64 = real.parse().unwrap();
            assert!(number.is_finite(), "no JSON spelling for {real}");
            out.push_str(real);
        }
        Yaml::String(string) => out.push_str(&serde_json::to_string(string).unwrap()),
        Yaml::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                yaml_as_json(item, out);
            }
            out.push(']');
        }
        Yaml::Hash(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let key = match key {
                    Yaml::String(key) => key.clone(),
                    Yaml::Integer(key) => key.to_string(),
                    Yaml::Boolean(key) => key.to_string(),
                    other => panic!("no JSON key for {other:?}"),
                };
                out.push_str(&serde_json::to_string(&key).unwrap());
                out.push(':');
                yaml_as_json(item, out);
            }
            out.push('}');
        }
        other => panic!("no JSON spelling for {other:?}"),
    }
}

/// A JSON root document is the same description as its YAML spelling, so it emits the same module
/// byte for byte — which also makes it compile wherever the pinned YAML module does.
#[test]
fn a_json_spelling_of_the_basic_spec_emits_the_same_module() {
    let documents = yaml_rust2::YamlLoader::load_from_str(BASIC_SPEC).unwrap();
    assert_eq!(documents.len(), 1);
    let mut json = String::new();
    yaml_as_json(&documents[0], &mut json);
    serde_json::from_str::<serde_json::Value>(&json).expect("the conversion is valid JSON");

    // The header names each spec by its file name, the one line the two spellings differ in.
    let from_json = spec_module("openapi.json", &json, |spec| spec);
    assert!(
        from_json.contains("\n// source: openapi.json\n"),
        "{from_json}"
    );
    assert_eq!(
        from_json.replacen(
            "\n// source: openapi.json\n",
            "\n// source: openapi.yaml\n",
            1
        ),
        basic_spec_module(|spec| spec),
        "the JSON spelling of BASIC_SPEC must emit the YAML spelling's module"
    );
}

/// A JSON root whose types live in a YAML sub-file, both reached through relative `$ref`s, one of
/// them from inside the sub-file to a sibling definition there: the multi-file, mixed-format input
/// a split description takes, compiled and linted in a consumer crate.
const MULTI_FILE_ROOT: &str = r##"{
  "openapi": "3.1.0",
  "info": { "title": "Split", "version": "1.0.0" },
  "servers": [ { "url": "https://example.com" } ],
  "paths": {
    "/pets/{id}": {
      "get": {
        "operationId": "getPet",
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "responses": {
          "200": {
            "description": "OK",
            "content": {
              "application/json": { "schema": { "$ref": "lib.yaml#/components/schemas/Pet" } }
            }
          },
          "404": {
            "description": "missing",
            "content": {
              "application/json": { "schema": { "$ref": "lib.yaml#/components/schemas/Problem" } }
            }
          }
        }
      }
    }
  }
}
"##;

const MULTI_FILE_LIB: &str = r##"
components:
  schemas:
    Pet:
      type: object
      required: [id, owner]
      properties:
        id: { type: string }
        owner: { $ref: "#/components/schemas/Owner" }
    Owner:
      type: object
      required: [name]
      properties:
        name: { type: string }
    Problem:
      type: object
      required: [title]
      properties:
        title: { type: string }
"##;

#[test]
fn a_multi_file_json_root_compiles_in_a_consumer_crate() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.json", MULTI_FILE_ROOT);
    write_spec(temp.path(), "lib.yaml", MULTI_FILE_LIB);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "split_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(report.errors().count(), 0, "{report:#?}");

    // Every sub-file type is emitted under its own name, the one the sub-file's internal `$ref`
    // reaches included, and the operation's success and error both name them.
    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    for item in [
        "pub struct Pet ",
        "pub struct Owner ",
        "pub struct Problem ",
    ] {
        assert!(generated.contains(item), "{item} is emitted");
    }
    assert!(
        generated.contains("pub owner: Owner,"),
        "Pet's owner is typed"
    );
    assert!(
        generated.contains("ResponseValue<types::Pet>"),
        "getPet's success is the sub-file Pet"
    );

    let status = fixture_cargo(&out).arg("check").status().unwrap();
    assert!(status.success(), "the split client must compile");
    let status = fixture_cargo(&out)
        .args(["clippy", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the split client must pass clippy -D warnings"
    );
}

/// `generate_api!` renders through `preview_for_macro` and `build.rs` through `generate`. For one
/// configuration away from every default, the two emit the same module, and the macro's manifest
/// audit accepts the manifest the `build.rs` fixture crate declares.
#[test]
fn the_macro_preview_emits_the_module_generate_writes() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", BASIC_SPEC);
    let out = temp.path().join("client");
    let configure = |spec: Spec| {
        spec.with_uuid(spargen::UuidMapping::String)
            .with_datetime(spargen::DateTimeBackend::String)
            .with_open_narrowing(spargen::Narrowing::Open)
            .with_error_body_cap(4096)
            .with_batch_cap(7)
    };

    let report = generate_configured_fixture_crate(&spec, &out, "parity_client", "2021", configure);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let written = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();

    let preview = spargen::__private::preview_for_macro(
        &configure(Spec::new(spec)),
        out.join("Cargo.toml").to_str().unwrap(),
    );
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        preview.report
    );
    let contents = preview.contents.expect("a generated module");
    // The non-default cap reaches every client constructor that sets it: `with_config` (which
    // `new` builds through) as the `ClientConfig` field it starts from, and `with_client` and
    // `with_backend` through `config_mut`, so the parity below compares a module that carries it.
    assert_eq!(
        contents.matches("max_error_body: 4096usize,").count(),
        1,
        "the configured error_body_cap must reach `with_config`'s starting config"
    );
    assert_eq!(
        contents.matches("max_error_body = 4096usize;").count(),
        2,
        "the configured error_body_cap must reach `with_client` and `with_backend`"
    );
    assert!(!contents.contains("max_error_body = 65536usize"));
    assert!(!contents.contains("max_error_body: 65536usize"));
    // `generate` adds only the build cache's two digest lines above the rendered module.
    let mut lines = written.splitn(3, '\n');
    assert!(lines.next().unwrap().starts_with("// input-sha256: "));
    assert!(lines.next().unwrap().starts_with("// content-sha256: "));
    assert!(
        lines.next().unwrap() == contents,
        "the macro preview and `generate` emitted different modules for the same config"
    );
}

#[test]
fn generated_module_compiles_in_basic_oas31_crate() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", BASIC_SPEC);
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

    // What the generator itself requires of a consumer, not what this harness wrote into the
    // fixture manifest: `tokio` is the one opt-in dependency — optional, native-only, needed only
    // by the `blocking` feature — and every other dependency is unconditional. `uuid`/`time` are
    // not consumer features at all: generated code names them unconditionally.
    let (requirements, _) =
        spargen::requirements(&Spec::new(spec.clone())).expect("the basic spec lowers");
    let opt_in: Vec<_> = requirements
        .dependencies
        .iter()
        .filter(|dependency| dependency.optional || dependency.required_by_feature.is_some())
        .collect();
    assert_eq!(opt_in.len(), 1, "{requirements:#?}");
    let tokio = opt_in[0];
    assert_eq!(
        (
            tokio.name,
            tokio.optional,
            tokio.required_by_feature,
            tokio.features.as_slice(),
            tokio.table,
        ),
        (
            "tokio",
            true,
            Some("blocking"),
            &["rt"][..],
            "target.'cfg(not(target_arch = \"wasm32\"))'.dependencies",
        ),
        "{requirements:#?}"
    );
    // The block `spargen deps` prints keeps it opt-in: commented out under the feature that
    // enables it, so a consumer that pastes the block as printed never builds tokio.
    let block = requirements.manifest_block();
    assert!(
        block.contains("# blocking = [\"dep:tokio\"]\n")
            && !block.lines().any(|line| line.starts_with("tokio")),
        "{block}"
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
    write_fixture_test(&out, "blocking", basic_blocking::BLOCKING_TEST);

    // Prove the wired serde defaults actually deserialize: an absent optional field with a
    // representable scalar default fills in the default instead of `None`, while a required field
    // (default rustdoc-only) still comes from the payload.
    write_fixture_test(&out, "defaults", basic_defaults::DEFAULTS_TEST);
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
    write_fixture_test(&out, "errors", basic_errors::ERRORS_TEST);
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
