//! No-panic property harness for the `oas31` frontend (issue #32).
//!
//! The contract: `spargen::check` — which runs the whole frontend, `source` parse → `oas31`
//! parse/validate/audit → `ir` lower → `name` allocate — must ALWAYS return a [`Report`] for ANY
//! input, however malformed, random, or adversarial. It must never `panic!`, `unwrap` on bad input,
//! overflow the stack, or hang. This harness feeds a wide variety of inputs to `check` and asserts
//! that every case returns, so no case panicked or aborted. Beyond no-panic it holds five oracles:
//! no report carries an `IR invariant failed` diagnostic (lowering must produce a valid IR for any
//! input), every diagnostic names a real location (`oracles::location_violations`, #454), a
//! document that is valid JSON reports the same sorted codes through the JSON and the YAML parser,
//! and `generate` over the valid-skeleton documents writes a module that parses as Rust whenever
//! it reports `Generated`, whose unions' variants a value can tell apart
//! (`oracles::indistinguishable_variants`, #402).
//!
//! Coverage (see the per-category tests below):
//!   * arbitrary raw bytes (invalid UTF-8, control bytes, truncated multibyte, …);
//!   * arbitrary UTF-8 strings (garbage that still decodes);
//!   * structurally-biased JSON/YAML documents whose keys are drawn from the OpenAPI/JSON-Schema
//!     keyword set (`openapi`/`paths`/`components`/`schemas`/`$ref`/`oneOf`/`allOf`/`type`/`enum`/
//!     `discriminator`/`properties`/`required`/…), so the fuzzer reaches deep into lowering rather
//!     than bouncing off the parser's reject path;
//!   * valid-skeleton documents wrapping random schemas, so lowering runs to completion;
//!   * deep `$ref` chains that exercise the recursion depth guard (the stack-overflow vector this
//!     issue found and fixed);
//!   * components whose root is a `$ref` beside random sibling keywords, over a random ref graph
//!     (the release-mode assertion abort of issue #148).
//!
//! Every generated document is fed through BOTH the JSON and the YAML parser (by file extension;
//! JSON is a subset of YAML). The run is deterministic and bounded: a fixed-seed RNG and capped
//! case counts / input sizes keep `mise run test` fast and non-flaky. A panic anywhere inside
//! `check` fails the test with the (shrunk) offending input; a stack overflow or hang aborts loudly.

use std::cell::Cell;

use camino::Utf8PathBuf;
use proptest::prelude::*;
use proptest::test_runner::{Config as PtConfig, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use serde_json::{Map, Value};
use spargen::{check, CargoIntegration, Code, Outcome, Report, Spec};
use tempfile::TempDir;

mod oracles;

/// Keys the frontend interprets — biasing generated objects toward these drives the fuzzer past the
/// parser and into document assembly, resolution, and lowering.
const KEYWORDS: &[&str] = &[
    "openapi",
    "info",
    "title",
    "version",
    "paths",
    "get",
    "post",
    "put",
    "delete",
    "operationId",
    "parameters",
    "requestBody",
    "responses",
    "content",
    "application/json",
    "schema",
    "components",
    "schemas",
    "securitySchemes",
    "security",
    "$ref",
    "type",
    "properties",
    "required",
    "items",
    "prefixItems",
    "allOf",
    "oneOf",
    "anyOf",
    "enum",
    "const",
    "discriminator",
    "propertyName",
    "mapping",
    "additionalProperties",
    "patternProperties",
    "format",
    "nullable",
    "default",
    "$dynamicRef",
    "in",
    "name",
    "style",
];

/// Scalar strings the frontend gives meaning to (type names, formats, versions, ref targets).
const SCALARS: &[&str] = &[
    "object",
    "array",
    "string",
    "integer",
    "number",
    "boolean",
    "null",
    "3.1.0",
    "3.2.0",
    "3.0.0",
    "int32",
    "date-time",
    "uuid",
    "binary",
    "base64",
    "#/components/schemas/S0",
    "#/components/schemas/Missing",
    "http://example.com/x#/y",
    "query",
    "path",
    "deepObject",
    "1.0.0",
    "",
];

fn deterministic_runner(cases: u32) -> TestRunner {
    // A fixed algorithm + fixed (zero) seed ⇒ the same input sequence on every run: deterministic
    // and non-flaky. `failure_persistence: None` avoids writing a regression file into the repo.
    // `SPARGEN_FUZZ_CASES` lets a maintainer widen the search locally (e.g. after touching the
    // frontend) without changing the bounded default `mise run test` uses.
    let cases = std::env::var("SPARGEN_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(cases, |scale: u32| cases.saturating_mul(scale));
    TestRunner::new_with_rng(
        PtConfig {
            cases,
            failure_persistence: None,
            ..PtConfig::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    )
}

/// The prefix `ir::check_invariants` gives every diagnostic it emits. Lowering must produce an IR
/// that passes them for any input, so a report carrying one is a lowering bug, not a rejection.
const INVARIANT_FAILURE: &str = "IR invariant failed";

/// Write `bytes` to `spec.<ext>` in `dir` and run `check`. Returning at all proves `check` did not
/// panic/abort; the report must also carry no [`INVARIANT_FAILURE`], and every diagnostic it
/// carries must name a real location ([`oracles::location_violations`]).
fn exercise(dir: &TempDir, bytes: &[u8], ext: &str) -> Result<Report, TestCaseError> {
    let spec = Utf8PathBuf::from_path_buf(dir.path().join(format!("spec.{ext}"))).unwrap();
    std::fs::write(&spec, bytes).unwrap();
    let report = check(&Spec::new(spec));
    let failed: Vec<&str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .filter(|message| message.contains(INVARIANT_FAILURE))
        .collect();
    prop_assert!(
        failed.is_empty(),
        "lowering produced an IR that fails its invariants (`.{ext}`): {failed:#?}"
    );
    let unlocated = oracles::unknown(oracles::location_violations(report.diagnostics(), bytes));
    prop_assert!(
        unlocated.is_empty(),
        "diagnostics with no real location (`.{ext}`): {unlocated:#?}"
    );
    Ok(report)
}

/// The sorted diagnostic codes of a report.
fn codes(report: &Report) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    codes.sort_unstable();
    codes
}

/// Run one document through both the JSON and the YAML parser, so a single case covers both
/// frontends. The text need not be JSON, so the two may legitimately disagree; see
/// [`exercise_json_both`] for the inputs where they may not.
fn exercise_both(dir: &TempDir, text: &str) -> Result<(Report, Report), TestCaseError> {
    Ok((
        exercise(dir, text.as_bytes(), "json")?,
        exercise(dir, text.as_bytes(), "yaml")?,
    ))
}

/// [`exercise_both`] on a document that is valid JSON. JSON is a subset of YAML, so both parsers
/// read the same document, and the frontend must report the same codes for it either way.
fn exercise_json_both(dir: &TempDir, text: &str) -> Result<Report, TestCaseError> {
    let (json, yaml) = exercise_both(dir, text)?;
    prop_assert_eq!(
        codes(&json),
        codes(&yaml),
        "the JSON and the YAML parse of one document report different codes"
    );
    Ok(json)
}

// Strategies

/// A short string, biased toward frontend-meaningful scalars but including arbitrary noise.
fn arb_scalar_string() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => prop::sample::select(SCALARS).prop_map(str::to_owned),
        1 => "[a-zA-Z0-9_/#${}.-]{0,12}",
        1 => any::<String>().prop_map(|s| s.chars().take(16).collect()),
    ]
}

/// An object key, biased toward OpenAPI keywords but occasionally arbitrary.
fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop::sample::select(KEYWORDS).prop_map(str::to_owned),
        1 => "[a-zA-Z0-9_]{0,8}",
    ]
}

/// A recursive JSON value whose objects use OpenAPI-keyword keys: bounded depth/breadth so the
/// generated document stays small (and the test fast) while still nesting arbitrary composites of
/// maps, arrays, and scalars.
fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..1_000_000).prop_map(Value::from),
        arb_scalar_string().prop_map(Value::String),
    ];
    // depth 5, up to 48 total nodes, up to 6 children per collection.
    leaf.prop_recursive(5, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec((arb_key(), inner), 0..6).prop_map(|pairs| {
                let mut map = Map::new();
                for (k, v) in pairs {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
    })
}

/// A document that always has the OpenAPI skeleton (`openapi`/`info`/`paths`/`components.schemas`)
/// so it survives structural validation and reaches lowering, but whose schemas are arbitrary
/// keyword-biased values. This is the strategy that actually exercises `oas31::lower` end to end.
fn arb_skeleton_doc() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(arb_value(), 1..5),
        prop::sample::select(SCALARS),
    )
        .prop_map(|(schemas, version)| {
            let mut schema_map = Map::new();
            for (i, schema) in schemas.into_iter().enumerate() {
                schema_map.insert(format!("S{i}"), schema);
            }
            let doc = serde_json::json!({
                "openapi": version,
                "info": { "title": "t", "version": "1.0.0" },
                "paths": {
                    "/p": {
                        "get": {
                            "operationId": "op",
                            "responses": {
                                "200": {
                                    "description": "ok",
                                    "content": {
                                        "application/json": {
                                            "schema": { "$ref": "#/components/schemas/S0" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "components": { "schemas": Value::Object(schema_map) }
            });
            serde_json::to_string(&doc).unwrap()
        })
}

/// A chain of components `S0 -> S1 -> ... -> S{depth}` where each links to the next via a randomly
/// chosen composition (allOf / array items / object property / oneOf). Depths straddle the lowering
/// cap so both the accept path (below the cap) and the reject path (E014, above it) are hit — the
/// exact stack-overflow vector this issue found. `check_never_panics_on_deep_ref_chains` counts
/// both and fails if either was never reached.
fn arb_ref_chain() -> impl Strategy<Value = String> {
    (10usize..200, 0u8..4).prop_map(|(depth, kind)| {
        let mut schemas = String::new();
        for i in 0..depth {
            let next = format!("#/components/schemas/S{}", i + 1);
            let body = match kind {
                0 => format!("{{\"allOf\":[{{\"$ref\":\"{next}\"}}]}}"),
                1 => format!("{{\"type\":\"array\",\"items\":{{\"$ref\":\"{next}\"}}}}"),
                2 => format!(
                    "{{\"type\":\"object\",\"properties\":{{\"p\":{{\"$ref\":\"{next}\"}}}}}}"
                ),
                _ => format!("{{\"oneOf\":[{{\"$ref\":\"{next}\"}}]}}"),
            };
            schemas.push_str(&format!("\"S{i}\":{body},"));
        }
        schemas.push_str(&format!("\"S{depth}\":{{\"type\":\"string\"}}"));
        format!(
            "{{\"openapi\":\"3.1.0\",\"info\":{{\"title\":\"t\",\"version\":\"1.0.0\"}},\
             \"paths\":{{}},\"components\":{{\"schemas\":{{{schemas}}}}}}}"
        )
    })
}

/// Sibling keywords a `$ref` can carry, most of them bearing no shape (annotations, validation-only
/// keywords, a `default`) and a few that do, so both of the `$ref` arm's exits are reached.
const REF_SIBLINGS: &[(&str, &str)] = &[
    ("description", "\"d\""),
    ("title", "\"t\""),
    ("maxLength", "5"),
    ("pattern", "\"^a\""),
    ("readOnly", "true"),
    ("deprecated", "true"),
    ("default", "\"x\""),
    ("type", "\"string\""),
    ("properties", "{\"p\":{\"type\":\"integer\"}}"),
];

/// Components `S0..S{n}` where every `S{i}` past the first is a `$ref` to a random component —
/// earlier, later, or itself — beside a random subset of [`REF_SIBLINGS`], and the operation reaches
/// a random one of them first. A component whose ROOT is such a `$ref` is the shape `arb_value`
/// essentially never produces (neither `description` nor `maxLength` is in [`KEYWORDS`], and `S0` is
/// the only in-document target in [`SCALARS`]), and it aborted the process whenever the target had
/// been lowered first (issue #148). The ref graph's order is what decided it, so it is randomised.
fn arb_ref_sibling_components() -> impl Strategy<Value = String> {
    (2usize..6)
        .prop_flat_map(|count| {
            (
                Just(count),
                prop::collection::vec(
                    (
                        0..count,
                        prop::collection::vec(any::<bool>(), REF_SIBLINGS.len()),
                    ),
                    count - 1,
                ),
                0..count,
                prop::sample::select(&["{\"type\":\"string\"}", "{\"type\":\"object\"}", "{}"][..]),
            )
        })
        .prop_map(|(_, aliases, entry, root)| {
            let mut schemas = format!("\"S0\":{root}");
            for (i, (target, chosen)) in aliases.into_iter().enumerate() {
                let mut body = format!("\"$ref\":\"#/components/schemas/S{target}\"");
                for ((key, value), keep) in REF_SIBLINGS.iter().zip(chosen) {
                    if keep {
                        body.push_str(&format!(",\"{key}\":{value}"));
                    }
                }
                schemas.push_str(&format!(",\"S{}\":{{{body}}}", i + 1));
            }
            format!(
                "{{\"openapi\":\"3.1.0\",\"info\":{{\"title\":\"t\",\"version\":\"1.0.0\"}},\
                 \"paths\":{{\"/p\":{{\"get\":{{\"operationId\":\"op\",\"responses\":{{\"200\":\
                 {{\"description\":\"ok\",\"content\":{{\"application/json\":{{\"schema\":\
                 {{\"$ref\":\"#/components/schemas/S{entry}\"}}}}}}}}}}}}}}}},\
                 \"components\":{{\"schemas\":{{{schemas}}}}}}}"
            )
        })
}

// The no-panic properties

#[test]
fn check_never_panics_on_arbitrary_bytes() {
    let dir = TempDir::new().unwrap();
    deterministic_runner(256)
        .run(&prop::collection::vec(any::<u8>(), 0..1024), |bytes| {
            // Raw bytes: invalid UTF-8, embedded NULs, truncated multibyte, control chars.
            exercise(&dir, &bytes, "yaml")?;
            exercise(&dir, &bytes, "json")?;
            exercise(&dir, &bytes, "txt")?; // extension-sniff fallback path
            Ok(())
        })
        .unwrap();
}

#[test]
fn check_never_panics_on_arbitrary_utf8() {
    let dir = TempDir::new().unwrap();
    deterministic_runner(256)
        .run(&any::<String>(), |text| {
            exercise_both(&dir, &text)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn check_never_panics_on_keyword_biased_documents() {
    let dir = TempDir::new().unwrap();
    deterministic_runner(400)
        .run(&arb_value(), |value| {
            exercise_json_both(&dir, &serde_json::to_string(&value).unwrap())?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn check_never_panics_on_skeleton_documents() {
    let dir = TempDir::new().unwrap();
    deterministic_runner(400)
        .run(&arb_skeleton_doc(), |text| {
            exercise_json_both(&dir, &text)?;
            Ok(())
        })
        .unwrap();
}

/// `generate` over the documents that reach lowering: a `Generated` run must have written a module
/// that parses as Rust and is not the `compile_error!` stub codegen falls back to when its own
/// tokens fail to format, which keeps the outcome `Generated`.
#[test]
fn generate_writes_parseable_rust_for_skeleton_documents() {
    let dir = TempDir::new().unwrap();
    let spec = Utf8PathBuf::from_path_buf(dir.path().join("spec.json")).unwrap();
    let out = Utf8PathBuf::from_path_buf(dir.path().join("client.rs")).unwrap();
    let generated = Cell::new(0u32);
    // Fewer cases than `check`: each generating case renders and parses the whole module,
    // embedded runtime included.
    deterministic_runner(96)
        .run(&arb_skeleton_doc(), |text| {
            std::fs::write(&spec, &text).unwrap();
            let _ = std::fs::remove_file(&out);
            let report = spargen::generate(
                &Spec::new(spec.clone())
                    .build(out.clone())
                    .cargo(CargoIntegration::Off),
            );
            if report.outcome() == Outcome::Generated {
                generated.set(generated.get() + 1);
                let code = std::fs::read_to_string(&out).unwrap();
                if let Err(error) = syn::parse_file(&code) {
                    return Err(TestCaseError::fail(format!(
                        "a `Generated` run wrote unparseable Rust ({error})"
                    )));
                }
                prop_assert!(
                    !code.contains("compile_error!"),
                    "a `Generated` run wrote the codegen fallback stub"
                );
                let unexplained = oracles::unexplained_variants(&report, &code);
                prop_assert!(
                    unexplained.is_empty(),
                    "a union's variants cannot be told apart and no warning says why: \
                     {unexplained:#?}"
                );
            }
            let unlocated = oracles::unknown(oracles::location_violations(
                report.diagnostics(),
                text.as_bytes(),
            ));
            prop_assert!(
                unlocated.is_empty(),
                "diagnostics with no real location: {unlocated:#?}"
            );
            Ok(())
        })
        .unwrap();
    assert!(
        generated.get() > 0,
        "no skeleton document generated, so the parse oracle was never reached"
    );
}

#[test]
fn check_never_panics_on_component_root_refs_with_siblings() {
    let dir = TempDir::new().unwrap();
    deterministic_runner(256)
        .run(&arb_ref_sibling_components(), |text| {
            exercise_json_both(&dir, &text)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn check_never_panics_on_deep_ref_chains() {
    let dir = TempDir::new().unwrap();
    // How many chains lowered, and how many hit the depth cap: `arb_ref_chain` promises both.
    let accepted = Cell::new(0u32);
    let too_deep = Cell::new(0u32);
    // Fewer cases: each deep chain is comparatively heavy (it lowers up to the depth cap per link).
    deterministic_runner(48)
        .run(&arb_ref_chain(), |text| {
            let report = exercise_json_both(&dir, &text)?;
            if report.outcome() != Outcome::Rejected {
                accepted.set(accepted.get() + 1);
            } else if report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::SchemaNestingTooDeep)
            {
                too_deep.set(too_deep.get() + 1);
            }
            Ok(())
        })
        .unwrap();
    assert!(
        accepted.get() > 0,
        "no ref chain lowered, so the accept path below the depth cap was never reached"
    );
    assert!(
        too_deep.get() > 0,
        "no ref chain reported E014, so the depth-cap reject path was never reached"
    );
}
