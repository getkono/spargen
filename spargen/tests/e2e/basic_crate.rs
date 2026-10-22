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

    let from_json = spec_module("openapi.json", &json, |spec| spec);
    assert_eq!(
        from_json,
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
    let spec = temp.path().join("openapi.json");
    std::fs::write(&spec, MULTI_FILE_ROOT).unwrap();
    std::fs::write(temp.path().join("lib.yaml"), MULTI_FILE_LIB).unwrap();
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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, BASIC_SPEC).unwrap();
    let out = temp.path().join("client");
    let configure = |spec: Spec| {
        spec.uuid(false)
            .time(false)
            .open_narrowing(true)
            .error_body_cap(4096)
            .batch_cap(7)
    };

    let report = generate_configured_fixture_crate(&spec, &out, "parity_client", "2021", configure);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let written = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();

    let preview = spargen::__private::preview_for_macro(
        &configure(Spec::new(Utf8PathBuf::from_path_buf(spec).unwrap())),
        out.join("Cargo.toml").to_str().unwrap(),
    );
    assert_eq!(
        preview.report.outcome(),
        Outcome::Generated,
        "{:#?}",
        preview.report
    );
    let contents = preview.contents.expect("a generated module");
    // The non-default cap reaches both client constructors that set it (`with_client` and
    // `with_backend`), so the parity below compares a module that carries it.
    assert_eq!(
        contents.matches("max_error_body = 4096usize;").count(),
        2,
        "the configured error_body_cap must reach every constructor"
    );
    assert!(!contents.contains("max_error_body = 65536usize"));
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

    // What the generator itself requires of a consumer, not what this harness wrote into the
    // fixture manifest: `tokio` is the one opt-in dependency — optional, native-only, needed only
    // by the `blocking` feature — and every other dependency is unconditional. `uuid`/`time` are
    // not consumer features at all: generated code names them unconditionally.
    let requirements = spargen::requirements(&Spec::new(
        Utf8PathBuf::from_path_buf(spec.clone()).unwrap(),
    ))
    .expect("the basic spec lowers");
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
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/blocking.rs"), basic_blocking::BLOCKING_TEST).unwrap();

    // Prove the wired serde defaults actually deserialize: an absent optional field with a
    // representable scalar default fills in the default instead of `None`, while a required field
    // (default rustdoc-only) still comes from the payload.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/defaults.rs"), basic_defaults::DEFAULTS_TEST).unwrap();
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
    std::fs::write(out.join("tests/errors.rs"), basic_errors::ERRORS_TEST).unwrap();
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
