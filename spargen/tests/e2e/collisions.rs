//! Names that collide or escape: operations, schemas, and parameters named after what the
//! generated module already uses, and Rust keywords, each still generating compiling code.

use super::*;

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

/// Pairs of distinct operations whose root type names derive alike (issue #693). `request`'s error
/// type is widened to `RequestOperationError` beside the runtime's `RequestError`, which is the
/// plain form of `requestOperation`'s. `getV2` and `get_v_2` allocate the distinct methods `get_v2`
/// and `get_v_2`, which both PascalCase to `GetV2`; each documents two success bodies and an error
/// body, so each emits a `…Response` enum and a `…Error` type. Each block is one path; the test
/// generates them in this order, which lists each losing operation before its winner, and reversed.
const OPERATION_TYPE_COLLISION_PATHS: [&str; 4] = [
    r#"
  /request:
    get: { operationId: request, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: string } } } } } }"#,
    r#"
  /request-operation:
    get: { operationId: requestOperation, responses: { "200": { description: ok }, "404": { description: nf, content: { application/json: { schema: { type: integer } } } } } }"#,
    r#"
  /get-v-2:
    get:
      operationId: get_v_2
      responses:
        "200": { description: ok, content: { application/json: { schema: { type: string } } } }
        "201": { description: created, content: { application/json: { schema: { type: integer } } } }
        "404": { description: nf, content: { application/json: { schema: { type: string } } } }"#,
    r#"
  /get-v2:
    get:
      operationId: getV2
      responses:
        "200": { description: ok, content: { application/json: { schema: { type: boolean } } } }
        "201": { description: created, content: { application/json: { schema: { type: number } } } }
        "404": { description: nf, content: { application/json: { schema: { type: integer } } } }"#,
];

/// The collision spec with its [`OPERATION_TYPE_COLLISION_PATHS`] in the given order.
fn operation_type_collision_spec<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
    let mut spec = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:".to_owned();
    spec.extend(paths);
    spec.push('\n');
    spec
}

/// The return type of the generated operation method `method`, whitespace removed.
fn operation_return_type(generated: &str, method: &str) -> String {
    let (_, rest) = generated
        .split_once(&format!("pub async fn {method}("))
        .unwrap_or_else(|| panic!("no operation method `{method}`:\n{generated}"));
    let (_, rest) = rest.split_once("->").unwrap();
    let (ret, _) = rest.split_once('{').unwrap();
    ret.split_whitespace().collect()
}

/// Which operation keeps each contested spelling: `requestOperation` keeps `RequestOperationError`
/// because a plain form beats `request`'s widened one, and `getV2` (method `get_v2`) keeps
/// `GetV2Response` and `GetV2Error` because `get_v2` sorts before `get_v_2`. The loser of each
/// takes a suffixed form of the name.
fn assert_contested_names_won_by_rank(generated: &str) {
    let request_operation = operation_return_type(generated, "request_operation");
    assert!(
        request_operation.contains("support::Error<RequestOperationError>"),
        "`requestOperation` keeps the plain `RequestOperationError`: {request_operation}"
    );
    let request = operation_return_type(generated, "request");
    assert!(
        request.contains("support::Error<RequestOperationError")
            && !request.contains("support::Error<RequestOperationError>"),
        "`request` takes a suffixed `RequestOperationError…`: {request}"
    );
    let get_v2 = operation_return_type(generated, "get_v2");
    assert!(
        get_v2.contains("ResponseValue<GetV2Response>")
            && get_v2.contains("support::Error<GetV2Error>"),
        "`getV2` keeps `GetV2Response` and `GetV2Error`: {get_v2}"
    );
    let get_v_2 = operation_return_type(generated, "get_v_2");
    assert!(
        get_v_2.contains("ResponseValue<GetV2Response")
            && !get_v_2.contains("ResponseValue<GetV2Response>")
            && get_v_2.contains("support::Error<GetV2Error")
            && !get_v_2.contains("support::Error<GetV2Error>"),
        "`get_v_2` takes suffixed `GetV2Response…` and `GetV2Error…`: {get_v_2}"
    );
}

/// Two operations whose error or success-enum type names derive alike still get distinct ones, so
/// the module compiles and lints clean instead of defining one type twice (`E0428`). The plain
/// form of a name wins it over a widened one, and of two plain forms the lexically smaller method
/// keeps it, whichever order `paths` lists them in.
#[test]
fn operations_whose_type_names_derive_alike_get_distinct_types() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec,
        operation_type_collision_spec(OPERATION_TYPE_COLLISION_PATHS),
    )
    .unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "operation_types_collide");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert_contested_names_won_by_rank(&generated);

    let reversed_spec = temp.path().join("reversed.yaml");
    std::fs::write(
        &reversed_spec,
        operation_type_collision_spec(OPERATION_TYPE_COLLISION_PATHS.into_iter().rev()),
    )
    .unwrap();
    let reversed_out = temp.path().join("reversed");
    let report = generate_fixture_crate(
        &reversed_spec,
        &reversed_out,
        "operation_types_collide_reversed",
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let reversed = std::fs::read_to_string(reversed_out.join("src/lib.rs")).unwrap();
    assert_contested_names_won_by_rank(&reversed);
    for method in ["request", "request_operation", "get_v2", "get_v_2"] {
        assert_eq!(
            operation_return_type(&generated, method),
            operation_return_type(&reversed, method),
            "`{method}`'s types do not depend on the order of `paths`"
        );
    }

    for (item, stem) in [
        ("struct", "RequestOperationError"),
        ("enum", "GetV2Response"),
        ("struct", "GetV2Error"),
    ] {
        assert_eq!(
            generated.matches(&format!("pub {item} {stem} ")).count()
                + generated.matches(&format!("pub {item} {stem}(")).count(),
            1,
            "`{stem}` is defined exactly once:\n{generated}"
        );
        assert!(
            generated.contains(&format!("pub {item} {stem}"))
                && generated
                    .split(&format!("pub {item} {stem}"))
                    .skip(1)
                    .any(|rest| rest.starts_with(|c: char| c.is_ascii_alphanumeric())),
            "the losing operation takes a suffixed `{stem}…`:\n{generated}"
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
        "operations whose type names derive alike must still generate compiling code"
    );
}

/// Parameters of one operation whose names escape to the same Rust identifier (issue #455). Each
/// operation lists the colliding pair with the later-ranked location first, so the bare spelling is
/// shown to follow the location rank and not the listing order. `search` also names its parameters
/// after the generated `query` and `params` bindings, which must yield to every parameter.
const PARAMETER_COLLISION_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items/{id}:
    get:
      operationId: getItem
      parameters:
        - { name: id, in: query, required: true, schema: { type: integer } }
        - { name: id, in: path, required: true, schema: { type: integer } }
        - { name: page_size, in: header, schema: { type: integer } }
        - { name: page-size, in: query, schema: { type: integer } }
      responses: { "204": { description: ok } }
  /search/{query}:
    get:
      operationId: search
      parameters:
        - { name: query, in: query, required: true, schema: { type: string } }
        - { name: query, in: path, required: true, schema: { type: string } }
        - { name: params, in: query, schema: { type: string } }
      responses: { "204": { description: ok } }
"#;

/// Two parameters escaping to one identifier get distinct ones, so the module compiles, lints
/// clean, and sends each parameter under its own wire name and location. Before the fix, `getItem`
/// emitted `id: i64, id: i64` (`E0415`) and two `pub page_size` fields (`E0124`).
#[test]
fn parameters_whose_names_escape_alike_get_distinct_identifiers() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, PARAMETER_COLLISION_SPEC).unwrap();
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "params_collide");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let generated = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    // The query `page-size` ranks before the header `page_size`, so it keeps the bare setter.
    assert_eq!(
        generated.matches("pub fn page_size(").count(),
        1,
        "{generated}"
    );
    let suffixed = generated
        .split("pub fn page_size_")
        .nth(1)
        .and_then(|rest| rest.split('(').next())
        .map(|suffix| format!("page_size_{suffix}"))
        .unwrap_or_else(|| panic!("the header parameter takes a suffixed setter:\n{generated}"));

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
        "colliding parameter names must still generate compiling, lint-clean code"
    );

    // Positional arguments follow the listing order: the query `id` first, then the path `id`.
    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/collide.rs"),
        r##"#![cfg(feature = "blocking")]

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn each_colliding_parameter_reaches_its_own_location() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            requests.push(String::from_utf8_lossy(&buf[..read]).into_owned());
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.flush().unwrap();
        }
        requests
    });

    let client = params_collide::BlockingClient::new(&format!("http://{addr}")).unwrap();
    client
        .get_item(
            8,
            7,
            params_collide::GetItemParams::default()
                .page_size(10)
                .HEADER_SETTER(20),
        )
        .expect("get_item round-trips");
    client
        .search(
            "b",
            "a",
            params_collide::SearchParams::default().params("c".to_owned()),
        )
        .expect("search round-trips");

    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].lines().next(),
        Some("GET /items/7?id=8&page-size=10 HTTP/1.1"),
        "{}",
        requests[0]
    );
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("\r\npage_size: 20\r\n"),
        "{}",
        requests[0]
    );
    assert_eq!(
        requests[1].lines().next(),
        Some("GET /search/a?query=b&params=c HTTP/1.1"),
        "{}",
        requests[1]
    );
}
"##
        .replace("HEADER_SETTER", &suffixed),
    )
    .unwrap();
    let status = fixture_cargo(&out)
        .args(["test", "--features", "blocking", "--test", "collide"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "each colliding parameter must reach the wire under its own name and location"
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
