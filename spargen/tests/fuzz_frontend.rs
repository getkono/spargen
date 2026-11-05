//! No-panic property harness for the `oas31` frontend (issue #32).
//!
//! The contract: `spargen::check` — which runs the whole frontend, `source` parse → `oas31`
//! parse/validate/audit → `ir` lower → `name` allocate — must ALWAYS return a [`Report`] for ANY
//! input, however malformed, random, or adversarial. It must never `panic!`, `unwrap` on bad input,
//! overflow the stack, or hang. This harness feeds a wide variety of inputs to `check` and asserts
//! that every case returns, so no case panicked or aborted. Beyond no-panic it holds these oracles:
//! no `check` or `generate` report carries an IR invariant or codegen failure
//! (`oracles::internal_failures`: lowering must produce a valid IR for any input), every
//! diagnostic names a real location (`oracles::location_violations`, #454), a document that is
//! valid JSON reports the same sorted codes through the JSON and the YAML parser, and `generate`
//! over the valid-skeleton and operation documents reaches the verdict `check` reaches with the
//! identical diagnostics (`oracles::parity_violations`), and writes a module that parses as Rust
//! whenever it reports `Generated`, whose unions' variants a value can tell apart unless a `W001`
//! at the union says why (`oracles::unexplained_variants`, #402), and whose every emitted type is a
//! component or named by another item (`oracles::unreferenced_types`, #928).
//!
//! Coverage (see the per-category tests below):
//!   * arbitrary raw bytes (invalid UTF-8, control bytes, truncated multibyte, …);
//!   * arbitrary UTF-8 strings (garbage that still decodes);
//!   * structurally-biased JSON/YAML documents whose keys are drawn from the OpenAPI/JSON-Schema
//!     keyword set (`openapi`/`paths`/`components`/`schemas`/`$ref`/`oneOf`/`allOf`/`type`/`enum`/
//!     `discriminator`/`properties`/`required`/…), so the fuzzer reaches deep into lowering rather
//!     than bouncing off the parser's reject path;
//!   * valid-skeleton documents declaring a supported version and wrapping random schemas, most of
//!     them well-formed Schema Objects (`discriminator`, `patternProperties`, `prefixItems` and
//!     `xml` among their keywords), so lowering runs to completion (at least half of the cases
//!     reach lowering, or the test fails);
//!   * operation documents whose one operation varies its parameters' locations, styles and
//!     `explode`, its request and response media, and its form and multipart `encoding`, over
//!     well-formed schemas, so parameter and body codegen runs to completion (at least half of the
//!     cases generate, or the test fails), through `generate` only;
//!   * deep `$ref` chains that exercise the recursion depth guard (the stack-overflow vector this
//!     issue found and fixed);
//!   * block-style YAML nested past the parsers' nesting cap, up to issue #706's million levels;
//!   * components whose root is a `$ref` beside random sibling keywords, over a random ref graph
//!     (the release-mode assertion abort of issue #148).
//!
//! Every document a `check_never_panics_*` test draws is fed through BOTH the JSON and the YAML
//! parser (by file extension; JSON is a subset of YAML), except the block-style YAML of
//! `check_never_panics_on_deeply_nested_yaml`, which is not JSON and goes through the YAML parser
//! only. The run is deterministic and bounded: a fixed-seed RNG and capped case counts / input
//! sizes keep `mise run test` fast and non-flaky. A panic anywhere inside `check` fails the test
//! with the (shrunk) offending input, and a stack overflow aborts the test process. A hang shows up
//! as a test that never finishes.

use std::cell::Cell;

use camino::Utf8PathBuf;
use proptest::prelude::*;
use proptest::test_runner::{Config as PtConfig, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use serde_json::{Map, Value};
use spargen::{check, CargoIntegration, Code, Outcome, Report, Spec};
use tempfile::TempDir;

mod oracles;

use oracles::readers::codes;

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

/// Fail where the shared oracles find a defect in `report`, a run over the root document `bytes`,
/// or in `module`, the module a `Generated` run wrote ([`oracles::run_violations`]): an IR
/// invariant or codegen failure reported as an `E011`, a diagnostic with no real location, or a
/// module that does not parse, emits a union whose variants a value cannot tell apart and no
/// `W001` says why, or emits a type that is no component and that nothing names. `what` says which
/// run it was.
fn hold_report(
    report: &Report,
    bytes: &[u8],
    module: Option<&str>,
    what: &str,
) -> Result<(), TestCaseError> {
    let violations = oracles::run_violations(report, bytes, module);
    prop_assert!(
        violations.is_empty(),
        "{what} broke an oracle every run is held to: {violations:#?}"
    );
    Ok(())
}

/// Write `bytes` to `spec.<ext>` in `dir` and run `check`. Returning at all proves `check` did not
/// panic/abort; the report is also held to [`hold_report`].
fn exercise(dir: &TempDir, bytes: &[u8], ext: &str) -> Result<Report, TestCaseError> {
    let spec = Utf8PathBuf::from_path_buf(dir.path().join(format!("spec.{ext}"))).unwrap();
    std::fs::write(&spec, bytes).unwrap();
    let report = check(&Spec::new(spec));
    hold_report(&report, bytes, None, &format!("`check` over `.{ext}`"))?;
    Ok(report)
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

/// The JSON type names a Schema Object's `type` takes.
const TYPE_NAMES: &[&str] = &[
    "object", "array", "string", "integer", "number", "boolean", "null",
];

/// A Discriminator Object: a `propertyName` the generated schemas can declare, and a `mapping`
/// from tag values to components.
fn arb_discriminator() -> impl Strategy<Value = Value> {
    (
        "[a-c]",
        prop::collection::btree_map("[x-z]", 0usize..5, 0..3),
    )
        .prop_map(|(property, mapping)| {
            let mut discriminator = serde_json::json!({ "propertyName": property });
            if !mapping.is_empty() {
                let mapping: Map<String, Value> = mapping
                    .into_iter()
                    .map(|(tag, at)| (tag, Value::from(format!("#/components/schemas/S{at}"))))
                    .collect();
                discriminator["mapping"] = Value::Object(mapping);
            }
            discriminator
        })
}

/// An XML Object: an element or attribute name, and whether an array is wrapped.
fn arb_xml() -> impl Strategy<Value = Value> {
    (
        proptest::option::of("[a-c]"),
        proptest::option::of(any::<bool>()),
        proptest::option::of(any::<bool>()),
    )
        .prop_map(|(name, attribute, wrapped)| {
            let mut xml = Map::new();
            if let Some(name) = name {
                xml.insert("name".to_owned(), Value::from(name));
            }
            if let Some(attribute) = attribute {
                xml.insert("attribute".to_owned(), Value::from(attribute));
            }
            if let Some(wrapped) = wrapped {
                xml.insert("wrapped".to_owned(), Value::from(wrapped));
            }
            Value::Object(xml)
        })
}

/// A Schema Object whose every keyword holds a value of the shape that keyword requires: types
/// from [`TYPE_NAMES`], subschemas under `properties`/`items`/`prefixItems`/`patternProperties`/
/// `allOf`/`oneOf`/`anyOf`/`not`, string arrays under `required`, a component `$ref`, a
/// `discriminator` ([`arb_discriminator`]) and an `xml` ([`arb_xml`]), and arbitrary values only
/// where any value is valid (`enum`, `const`, `default`). The structural validator accepts it, so a
/// document built from it reaches lowering, where the combinations of keywords are what is under
/// test.
fn arb_schema() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        prop::sample::select(TYPE_NAMES).prop_map(|name| serde_json::json!({ "type": name })),
        (0usize..5)
            .prop_map(|at| serde_json::json!({ "$ref": format!("#/components/schemas/S{at}") })),
        Just(serde_json::json!({})),
        Just(Value::Bool(true)),
    ];
    leaf.prop_recursive(4, 32, 4, |inner| {
        let keyword = prop_oneof![
            prop::sample::select(TYPE_NAMES).prop_map(|name| ("type", Value::from(name))),
            prop::collection::vec(prop::sample::select(TYPE_NAMES), 1..3)
                .prop_map(|names| ("type", Value::from(names))),
            prop::collection::vec(("[a-c]", inner.clone()), 1..3)
                .prop_map(|fields| { ("properties", Value::Object(fields.into_iter().collect())) }),
            prop::collection::vec("[a-c]", 0..3).prop_map(|names| ("required", Value::from(names))),
            inner.clone().prop_map(|schema| ("items", schema)),
            inner.clone().prop_map(|schema| ("not", schema)),
            inner
                .clone()
                .prop_map(|schema| ("additionalProperties", schema)),
            prop::collection::vec(inner.clone(), 1..4).prop_map(|all| ("allOf", Value::from(all))),
            prop::collection::vec(inner.clone(), 1..4).prop_map(|one| ("oneOf", Value::from(one))),
            prop::collection::vec(inner.clone(), 1..4).prop_map(|any| ("anyOf", Value::from(any))),
            (0usize..5).prop_map(|at| ("$ref", Value::from(format!("#/components/schemas/S{at}")))),
            prop::collection::vec(arb_value(), 1..4)
                .prop_map(|values| ("enum", Value::from(values))),
            arb_value().prop_map(|value| ("const", value)),
            arb_value().prop_map(|value| ("default", value)),
            prop::sample::select(&["int32", "int64", "date-time", "uuid", "binary", "byte"][..])
                .prop_map(|format| ("format", Value::from(format))),
            (0u64..4).prop_map(|bound| ("minLength", Value::from(bound))),
            prop::collection::vec(inner.clone(), 1..3)
                .prop_map(|prefix| ("prefixItems", Value::from(prefix))),
            prop::collection::vec(("\\^[a-c]\\$?", inner.clone()), 1..3).prop_map(|patterns| {
                (
                    "patternProperties",
                    Value::Object(patterns.into_iter().collect()),
                )
            }),
            arb_discriminator().prop_map(|discriminator| ("discriminator", discriminator)),
            arb_xml().prop_map(|xml| ("xml", xml)),
        ];
        prop::collection::vec(keyword, 1..4).prop_map(|keywords| {
            Value::Object(
                keywords
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        })
    })
}

/// The `openapi` versions spargen implements, one of which every skeleton document declares.
const SUPPORTED_VERSIONS: &[&str] = &["3.1.0", "3.2.0"];

/// A document that always has the OpenAPI skeleton (`openapi`/`info`/`paths`/`components.schemas`)
/// under a version spargen implements, so it is never stopped at `E001`, but whose schemas are
/// arbitrary keyword-biased values. This is the strategy that exercises `oas31::lower` end to end;
/// `check_never_panics_on_skeleton_documents` counts the cases that reach lowering and fails if
/// too few do. An unsupported version is the keyword-biased documents' to draw.
fn arb_skeleton_doc() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(prop_oneof![3 => arb_schema(), 1 => arb_value()], 1..5),
        prop::sample::select(SUPPORTED_VERSIONS),
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

/// How many components an operation document declares, `S0` to `S2`, each a [`arb_typed_schema`].
const OPERATION_COMPONENTS: usize = 3;

/// A Schema Object a value of every JSON type can be described by, with no keyword combination
/// that rejects it by construction: scalars, a `format`, a string `enum`, a nullable scalar, a
/// `$ref` to an operation document's components, and arrays, objects and `oneOf`s of those.
/// [`arb_schema`] draws the keyword combinations; this draws the bodies and parameters an operation
/// carries, so most operation documents reach the codegen they are built to exercise.
fn arb_typed_schema() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        3 => prop::sample::select(&["string", "integer", "number", "boolean"][..])
            .prop_map(|name| serde_json::json!({ "type": name })),
        1 => prop::sample::select(&["date-time", "uuid", "int32", "binary"][..])
            .prop_map(|format| serde_json::json!({ "type": "string", "format": format })),
        1 => Just(serde_json::json!({ "type": "string", "enum": ["x", "y"] })),
        1 => Just(serde_json::json!({ "type": ["integer", "null"] })),
        1 => (0usize..OPERATION_COMPONENTS)
            .prop_map(|at| serde_json::json!({ "$ref": format!("#/components/schemas/S{at}") })),
    ];
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            inner
                .clone()
                .prop_map(|items| serde_json::json!({ "type": "array", "items": items })),
            arb_object_schema(inner.clone()),
            prop::collection::vec(inner, 2..4)
                .prop_map(|branches| serde_json::json!({ "oneOf": branches })),
        ]
    })
}

/// An object schema whose properties `a` to `c` are drawn from `property`, some of them required.
fn arb_object_schema(property: impl Strategy<Value = Value>) -> impl Strategy<Value = Value> {
    prop::collection::btree_map("[a-c]", (property, any::<bool>()), 1..4).prop_map(|props| {
        let required: Vec<&String> = props
            .iter()
            .filter(|(_, (_, required))| *required)
            .map(|(name, _)| name)
            .collect();
        let mut schema = serde_json::json!({ "type": "object", "required": required });
        let properties: Map<String, Value> = props
            .iter()
            .map(|(name, (schema, _))| (name.clone(), schema.clone()))
            .collect();
        schema["properties"] = Value::Object(properties);
        schema
    })
}

/// A parameter's schema: a scalar, an array of scalars, or an object of scalars.
fn arb_parameter_schema() -> impl Strategy<Value = Value> {
    let scalar = || {
        prop::sample::select(&["string", "integer", "number", "boolean"][..])
            .prop_map(|name| serde_json::json!({ "type": name }))
    };
    prop_oneof![
        2 => scalar(),
        1 => scalar().prop_map(|items| serde_json::json!({ "type": "array", "items": items })),
        1 => arb_object_schema(scalar()),
    ]
}

/// A Parameter Object outside the path: a location and a `style` that location takes, an optional
/// `explode`, whether it is required, and a schema ([`arb_parameter_schema`]). The caller names it.
fn arb_parameter() -> impl Strategy<Value = Value> {
    (
        prop_oneof![
            prop::sample::select(&["form", "spaceDelimited", "pipeDelimited", "deepObject"][..])
                .prop_map(|style| ("query", Some(style))),
            Just(("query", None)),
            Just(("header", Some("simple"))),
            Just(("header", None)),
            Just(("cookie", Some("form"))),
        ],
        proptest::option::of(any::<bool>()),
        any::<bool>(),
        arb_parameter_schema(),
    )
        .prop_map(|((location, style), explode, required, schema)| {
            let mut parameter = serde_json::json!({
                "in": location,
                "required": required,
                "schema": schema,
            });
            if let Some(style) = style {
                parameter["style"] = Value::from(style);
            }
            if let Some(explode) = explode {
                parameter["explode"] = Value::from(explode);
            }
            parameter
        })
}

/// The `contentType`s an Encoding Object declares for a part or a field.
const PART_MEDIA_TYPES: &[&str] = &[
    "application/json",
    "application/vnd.api+json",
    "application/xml",
    "application/octet-stream",
    "text/plain",
    "image/png",
];

/// An Encoding Object: an optional part `contentType`, `style` and `explode`.
fn arb_encoding() -> impl Strategy<Value = Value> {
    (
        proptest::option::of(prop::sample::select(PART_MEDIA_TYPES)),
        proptest::option::of(prop::sample::select(
            &["form", "spaceDelimited", "pipeDelimited", "deepObject"][..],
        )),
        proptest::option::of(any::<bool>()),
    )
        .prop_map(|(content_type, style, explode)| {
            let mut encoding = Map::new();
            if let Some(content_type) = content_type {
                encoding.insert("contentType".to_owned(), Value::from(content_type));
            }
            if let Some(style) = style {
                encoding.insert("style".to_owned(), Value::from(style));
            }
            if let Some(explode) = explode {
                encoding.insert("explode".to_owned(), Value::from(explode));
            }
            Value::Object(encoding)
        })
}

/// One media type of a body's `content` and its Media Type Object: a JSON or XML document of any
/// [`arb_typed_schema`], text or octets of a string schema, and, on a request, a form or multipart
/// object whose fields are scalars, arrays, binary strings or objects, now and then with an
/// `encoding` ([`arb_encoding`]) for fields `a` to `c`.
fn arb_media(request: bool) -> BoxedStrategy<(&'static str, Value)> {
    let document = (
        prop::sample::select(
            &[
                "application/json",
                "application/vnd.api+json",
                "application/xml",
            ][..],
        ),
        arb_typed_schema(),
    )
        .prop_map(|(media, schema)| (media, serde_json::json!({ "schema": schema })));
    let text = prop_oneof![
        Just((
            "text/plain",
            serde_json::json!({ "schema": { "type": "string" } })
        )),
        Just((
            "application/octet-stream",
            serde_json::json!({ "schema": { "type": "string", "format": "binary" } })
        )),
        Just((
            "image/png",
            serde_json::json!({ "schema": { "type": "string", "contentEncoding": "base64" } })
        )),
    ];
    let field = prop_oneof![
        3 => prop::sample::select(&["string", "integer", "boolean"][..])
            .prop_map(|name| serde_json::json!({ "type": name })),
        1 => Just(serde_json::json!({ "type": "array", "items": { "type": "integer" } })),
        1 => Just(serde_json::json!({ "type": "string", "format": "binary" })),
        1 => Just(serde_json::json!({ "type": "object", "properties": { "x": { "type": "string" } } })),
    ];
    let form = (
        prop::sample::select(&["application/x-www-form-urlencoded", "multipart/form-data"][..]),
        arb_object_schema(field),
        proptest::option::of(prop::collection::btree_map("[a-c]", arb_encoding(), 1..3)),
    )
        .prop_map(|(media, schema, encoding)| {
            let mut object = serde_json::json!({ "schema": schema });
            if let Some(encoding) = encoding {
                object["encoding"] = Value::Object(encoding.into_iter().collect());
            }
            (media, object)
        });
    if request {
        prop_oneof![3 => document, 1 => text, 2 => form].boxed()
    } else {
        prop_oneof![3 => document, 1 => text].boxed()
    }
}

/// A body's `content`: one to two media types ([`arb_media`]), a request's or a response's.
fn arb_content(request: bool) -> impl Strategy<Value = Value> {
    prop::collection::vec(arb_media(request), 1..3).prop_map(|media| {
        Value::Object(
            media
                .into_iter()
                .map(|(key, object)| (key.to_owned(), object))
                .collect(),
        )
    })
}

/// An Operation Object on `/p/{id}`: the path parameter `id` in a path `style` with or without
/// `explode`, up to three more parameters ([`arb_parameter`]), an optional request body, and a
/// success response with or without content beside an optional error response.
fn arb_operation() -> impl Strategy<Value = Value> {
    (
        (
            proptest::option::of(prop::sample::select(&["simple", "label", "matrix"][..])),
            proptest::option::of(any::<bool>()),
            arb_parameter_schema(),
        ),
        prop::collection::vec(arb_parameter(), 0..4),
        proptest::option::of((any::<bool>(), arb_content(true))),
        proptest::option::of(arb_content(false)),
        proptest::option::of((
            prop::sample::select(&["404", "4XX", "default"][..]),
            arb_content(false),
        )),
    )
        .prop_map(
            |((style, explode, schema), mut parameters, body, success, error)| {
                for (at, parameter) in parameters.iter_mut().enumerate() {
                    parameter["name"] = Value::from(format!("p{at}"));
                }
                let mut id = serde_json::json!({
                    "name": "id",
                    "in": "path",
                    "required": true,
                    "schema": schema,
                });
                if let Some(style) = style {
                    id["style"] = Value::from(style);
                }
                if let Some(explode) = explode {
                    id["explode"] = Value::from(explode);
                }
                parameters.insert(0, id);
                let mut ok = serde_json::json!({ "description": "ok" });
                if let Some(content) = success {
                    ok["content"] = content;
                }
                let mut responses = serde_json::json!({ "200": ok });
                if let Some((status, content)) = error {
                    responses[status] =
                        serde_json::json!({ "description": "error", "content": content });
                }
                let mut operation = serde_json::json!({
                    "operationId": "op",
                    "parameters": parameters,
                    "responses": responses,
                });
                if let Some((required, content)) = body {
                    operation["requestBody"] =
                        serde_json::json!({ "required": required, "content": content });
                }
                operation
            },
        )
}

/// A document under a version spargen implements whose one operation ([`arb_operation`]) is a
/// `post` on `/p/{id}`, beside the components `S0` to `S2` its schemas refer to. This is the
/// strategy that exercises parameter serialization, body media and multipart encoding end to end.
fn arb_operation_doc() -> impl Strategy<Value = String> {
    (
        arb_operation(),
        prop::collection::vec(arb_typed_schema(), OPERATION_COMPONENTS),
        prop::sample::select(SUPPORTED_VERSIONS),
    )
        .prop_map(|(operation, schemas, version)| {
            let schemas: Map<String, Value> = schemas
                .into_iter()
                .enumerate()
                .map(|(i, schema)| (format!("S{i}"), schema))
                .collect();
            let doc = serde_json::json!({
                "openapi": version,
                "info": { "title": "t", "version": "1.0.0" },
                "paths": { "/p/{id}": { "post": operation } },
                "components": { "schemas": Value::Object(schemas) }
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

/// The deepest a value may sit in a parsed document, the root at depth 0, in either parser.
const MAX_NESTING_DEPTH: usize = 256;

/// A skeleton document whose `x-deep` holds `depth` nested block-style YAML containers (sequences
/// written `- - - x`, or mappings one indent apart) around the scalar `x`, which therefore sits at
/// depth `depth + 1`. Depths straddle [`MAX_NESTING_DEPTH`], and the million-level sequence is
/// issue #706's reproduction: block style is not bounded by the event parser, so it overflowed
/// the frontend stack before the builder capped nesting. Mappings stay shallow because their
/// indentation grows quadratically.
fn arb_deep_yaml() -> impl Strategy<Value = (usize, String)> {
    prop_oneof![
        7 => (1usize..600, any::<bool>()),
        1 => Just((1_000_000, false)),
    ]
    .prop_map(|(depth, mappings)| {
        let mut nest = String::new();
        if mappings {
            for level in 1..depth {
                nest.push_str(&format!("{}k:\n", " ".repeat(level)));
            }
            nest.push_str(&format!("{}k: x\n", " ".repeat(depth)));
        } else {
            nest.push_str(&format!("  {}x\n", "- ".repeat(depth)));
        }
        let text = format!(
            "openapi: 3.1.0\ninfo: {{ title: t, version: 1.0.0 }}\npaths: {{}}\nx-deep:\n{nest}"
        );
        (depth, text)
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

/// Whether `report` reached lowering: it carries no `E001` and no `E011`, the codes the version
/// gate, the parsers and the structural validator stop a document with before it is lowered.
fn reached_lowering(report: &Report) -> bool {
    !report.diagnostics().iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            Code::UnsupportedOpenApiVersion | Code::InvalidInput
        )
    })
}

#[test]
fn check_never_panics_on_skeleton_documents() {
    let dir = TempDir::new().unwrap();
    let cases = Cell::new(0u32);
    let reached = Cell::new(0u32);
    deterministic_runner(400)
        .run(&arb_skeleton_doc(), |text| {
            let report = exercise_json_both(&dir, &text)?;
            cases.set(cases.get() + 1);
            if reached_lowering(&report) {
                reached.set(reached.get() + 1);
            }
            Ok(())
        })
        .unwrap();
    assert!(
        reached.get() * 2 >= cases.get(),
        "only {} of {} skeleton documents reached lowering",
        reached.get(),
        cases.get()
    );
}

/// `generate` over the documents that reach lowering. Its report and the module a `Generated` run
/// wrote are held to [`hold_report`], and `check` over the same document, which runs the same
/// frontend without emitting, must reach the same verdict and report the identical diagnostics
/// ([`oracles::parity_violations`]).
#[test]
fn generate_writes_parseable_rust_for_skeleton_documents() {
    // Fewer cases than `check`: each generating case renders and parses the whole module,
    // embedded runtime included.
    let (generated, _) = generate_and_hold(96, arb_skeleton_doc());
    assert!(
        generated > 0,
        "no skeleton document generated, so the parse oracle was never reached"
    );
}

/// [`generate_writes_parseable_rust_for_skeleton_documents`] over documents whose one operation
/// varies ([`arb_operation_doc`]): parameter locations, styles and `explode`, request and response
/// media, and multipart and form `encoding`. At least half of them generate, so the parameter and
/// body codegen they vary is what the parse oracle reads.
#[test]
fn generate_writes_parseable_rust_for_operation_documents() {
    let (generated, cases) = generate_and_hold(96, arb_operation_doc());
    assert!(
        generated * 2 >= cases,
        "only {generated} of {cases} operation documents generated"
    );
}

/// Run `generate` over `cases` documents `documents` draws, holding each run to [`hold_report`]
/// and to parity with `check` ([`oracles::parity_violations`]). Returns how many of the runs
/// generated, and how many there were.
fn generate_and_hold(cases: u32, documents: impl Strategy<Value = String>) -> (u32, u32) {
    let dir = TempDir::new().unwrap();
    let spec = Utf8PathBuf::from_path_buf(dir.path().join("spec.json")).unwrap();
    let out = Utf8PathBuf::from_path_buf(dir.path().join("client.rs")).unwrap();
    let generated = Cell::new(0u32);
    let runs = Cell::new(0u32);
    deterministic_runner(cases)
        .run(&documents, |text| {
            runs.set(runs.get() + 1);
            std::fs::write(&spec, &text).unwrap();
            let _ = std::fs::remove_file(&out);
            let report = spargen::generate(
                &Spec::new(spec.clone())
                    .build(out.clone())
                    .cargo(CargoIntegration::Off),
            );
            let module = (report.outcome() == Outcome::Generated)
                .then(|| std::fs::read_to_string(&out).unwrap());
            if module.is_some() {
                generated.set(generated.get() + 1);
            }
            hold_report(&report, text.as_bytes(), module.as_deref(), "`generate`")?;
            let checked = check(&Spec::new(spec.clone()));
            let disagreements = oracles::parity_violations(&report, &checked);
            prop_assert!(
                disagreements.is_empty(),
                "`check` does not stand in for `generate`: {:#?}",
                disagreements
            );
            Ok(())
        })
        .unwrap();
    (generated.get(), runs.get())
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

#[test]
fn check_never_panics_on_deeply_nested_yaml() {
    let dir = TempDir::new().unwrap();
    // How many documents stayed within the nesting cap, and how many crossed it.
    let within = Cell::new(0u32);
    let past = Cell::new(0u32);
    deterministic_runner(32)
        .run(&arb_deep_yaml(), |(depth, text)| {
            let report = exercise(&dir, text.as_bytes(), "yaml")?;
            let too_deep = report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::InvalidInput
                    && diagnostic.message.contains("YAML nesting is too deep")
            });
            // The scalar sits at depth `depth + 1`, so the cap decides the outcome exactly.
            if depth < MAX_NESTING_DEPTH {
                prop_assert!(!too_deep, "depth {depth} is within the cap");
                prop_assert_ne!(report.outcome(), Outcome::Rejected, "depth {}", depth);
                within.set(within.get() + 1);
            } else {
                prop_assert!(too_deep, "depth {depth} is past the cap");
                past.set(past.get() + 1);
            }
            Ok(())
        })
        .unwrap();
    assert!(
        within.get() > 0,
        "no document stayed within the nesting cap"
    );
    assert!(past.get() > 0, "no document crossed the nesting cap");
}
