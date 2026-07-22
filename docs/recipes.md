# Framework round-trip recipes

Many Rust web servers describe themselves: they generate an OpenAPI document from the server code.
Spargen closes the loop by turning that document back into a typed Rust **client**. This is the
round-trip:

```
Rust server code  →  (framework)  →  OpenAPI document  →  (spargen)  →  typed Rust client
```

This page gives a concrete, runnable recipe for the three most common Rust OpenAPI-emitting
frameworks — [utoipa](#utoipa), [aide](#aide), and [poem-openapi](#poem-openapi) — plus the
[escape hatch](#carving-unsupported-idioms) for the rare construct spargen cannot represent.

Every recipe follows the same three steps:

1. **Export** the OpenAPI document from the framework.
2. **Generate** the client with spargen.
3. **Wire** the generate step into your build.

Two facts frame all of it:

- **Spargen requires OpenAPI 3.1.x (or 3.2.x).** A 3.0.x document is rejected with
  [`E001`](errors.md) — no silent downgrade. utoipa and aide emit 3.1.0; poem-openapi emits 3.0.0
  and must be upgraded (see its recipe).
- **Unsupported constructs never degrade silently.** Anything spargen cannot represent is
  supported, warned, or rejected with a stable code — see the [support matrix](support-matrix.md)
  and [diagnostic index](errors.md). When a real spec trips a rejection, the
  [`--carve` escape hatch](#carving-unsupported-idioms) drops just that island and generates the
  rest.

The specs used below are vendored under [`corpus/recipes/`](../corpus/recipes/README.md) and are
exercised by `spargen/tests/recipes.rs`, which asserts each framework's outcome so these recipes
stay honest.

## Wiring the generate step (applies to all frameworks)

Once you have an exported spec file, generation is one command. Emit a **module** to `include!`:

```bash
spargen generate openapi.json --out src/api.rs
```

or a standalone, publishable **crate**:

```bash
spargen generate openapi.json --out crates/api-client --as-crate
```

To keep the client in lockstep with the server, run generation from `build.rs` and commit the
result (spargen output is deterministic — same version + spec ⇒ byte-identical output):

```rust
// build.rs
fn main() {
    println!("cargo:rerun-if-changed=openapi.json");
    let config = spargen::Config::new(
        "openapi.json",
        spargen::OutputTarget::Module("src/api.rs".into()),
    );
    let report = spargen::generate(&config);
    println!("cargo:warning=spargen outcome: {:?}", report.outcome);
}
```

In CI, `spargen check openapi.json` is a fast pre-flight: it runs the full frontend (same support
audit as `generate`) without emitting code, so a spec that would reject fails the gate early. Add
`--format json` for machine-readable diagnostics.

---

## utoipa

[`juhaku/utoipa`](https://github.com/juhaku/utoipa) is code-first: `#[derive(ToSchema)]` on models
and `#[utoipa::path(...)]` on handlers, collected by a `#[derive(OpenApi)]` `ApiDoc`.

**OpenAPI version:** utoipa 5.x emits **OpenAPI 3.1.0** (the `OpenApiVersion` enum defaults to
`Version31`, serialized as `"3.1.0"`). No upgrade needed — spargen consumes it directly.

**Export.** The derived `ApiDoc` gives you the document; write it to a file:

```rust
use utoipa::OpenApi;

let json = ApiDoc::openapi()
    .to_pretty_json()
    .expect("serialize openapi");
std::fs::write("openapi.json", json).unwrap();
```

(`to_json()` is the compact form; `to_yaml()` exists behind utoipa's `yaml` feature. If you serve
the spec over HTTP via `utoipa-swagger-ui`/`utoipa-axum`, hitting that route and saving the body
works too.)

**Generate.**

```bash
spargen generate openapi.json --out src/api.rs
```

**Idioms spargen handles.** The vendored [`corpus/recipes/utoipa.json`](../corpus/recipes/utoipa.json)
mirrors a typical utoipa document and generates cleanly. It covers:

| utoipa idiom | OpenAPI shape | spargen result |
| --- | --- | --- |
| `Option<String>` field | `type: ["string", "null"]` | `Option<String>` |
| `Option<Category>` field | `oneOf: [{ "type": "null" }, { "$ref": … }]` | `Option<Category>` (the `null` member makes the lone remaining member optional) |
| flattened / composed model | `allOf: [{ "$ref": … }, { … }]` | one merged struct |
| `#[serde(tag = "…")]` enum | `oneOf` + `discriminator` | a Rust enum with custom (de)serialize dispatching on the tag |
| `#[utoipa::path(tag = "…")]` | `tags` on operations | grouped operations (no effect on typing) |
| multiple documented statuses | `200` body + `404` body | a per-operation response enum |
| `SecurityScheme::Http` bearer | `http` / `bearer` | bearer credential attached per operation |

**Caveat — `type: "null"` validity.** utoipa emits `oneOf`/`type` arrays containing
`{"type":"null"}` for nullable `$ref`s. This is valid OpenAPI 3.1 and spargen accepts it, though
some third-party 3.0-era validators flag it — that is a validator limitation, not a spargen one.

---

## aide

[`tamasfe/aide`](https://github.com/tamasfe/aide) documents an axum app: `ApiRouter` +
`api_route(...)`, with component schemas produced by [`schemars`](https://graham.cool/schemars/)
(JSON Schema 2020-12).

**OpenAPI version:** aide emits **OpenAPI 3.1.0** (its `OpenApi` document serializes the version as
`"3.1.0"`). No upgrade needed.

**Export.** aide builds an `OpenApi` value while assembling the router; serialize it with serde:

```rust
use aide::openapi::OpenApi;
use aide::axum::ApiRouter;

let mut api = OpenApi::default();
let router = ApiRouter::new()
    .api_route("/items", aide::axum::routing::get(list_items))
    // …
    .finish_api(&mut api); // populates `api`

std::fs::write("openapi.json", serde_json::to_string_pretty(&api).unwrap()).unwrap();
```

(Commonly you also serve `api` from a route via `aide::openapi::OpenApi` + `Json`; fetching that
route and saving the body is equivalent. `finish_api_with(&mut api, transform)` lets you set
titles/versions during assembly.)

**Generate.**

```bash
spargen generate openapi.json --out src/api.rs
```

**Idioms spargen handles.** The vendored [`corpus/recipes/aide.json`](../corpus/recipes/aide.json)
mirrors a schemars-backed aide document. It generates with only validation-only warnings
(`W001`, for the `minimum`/`format` hints schemars emits, which spargen faithfully ignores). It
covers:

| aide/schemars idiom | OpenAPI shape | spargen result |
| --- | --- | --- |
| `Option<String>` field | `anyOf: [{ "type": "string" }, { "type": "null" }]` | `Option<String>` |
| `Option<i32>` field | `type: ["integer", "null"]` | `Option<i32>` |
| externally-tagged enum with data | `oneOf` of closed (`additionalProperties: false`) objects with unique required keys | a Rust enum, dispatched by inspecting content (no `serde(untagged)`, no `Value`) |
| untagged scalar enum | `oneOf: [{ "type": "string" }, { "type": "integer" }]` | a Rust enum, dispatched by JSON type |
| `#[serde(flatten)]` | `allOf` composition | one merged struct |
| multiple documented statuses | `200` body + `400` body | a per-operation response enum |

> schemars validation keywords (`minimum`, `maxLength`, `pattern`, …) are not enforced by the
> generated types and surface as `W001`. That is expected, not a problem — the recipe test asserts
> `W001` is the only diagnostic class aide's document produces.

---

## poem-openapi

[`poem-web/poem`](https://github.com/poem-web/poem)'s `poem-openapi` is code-first over the `poem`
framework: `#[derive(Object)]`/`#[derive(ApiResponse)]` models and an `#[OpenApi]` impl block,
assembled into an `OpenApiService`.

**OpenAPI version — action required:** poem-openapi emits **OpenAPI 3.0.0**
(`const OPENAPI_VERSION: &str = "3.0.0"` in its serializer). **Spargen rejects 3.0.x with
[`E001`](errors.md).** You must upgrade the exported document to 3.1 before generating.

**Export.**

```rust
use poem_openapi::OpenApiService;

let service = OpenApiService::new(Api, "My API", "1.0");
std::fs::write("openapi-3.0.json", service.spec()).unwrap();     // JSON
// service.spec_yaml() gives YAML.
```

**Upgrade 3.0.0 → 3.1.** Convert the document with an off-the-shelf converter, then feed the 3.1
result to spargen. Any of these works:

- run the spec through a 3.0→3.1 upgrade tool (for example the LinkML `oas30-to-31` /
  `openapi-3-0-3-to-3-1` style converters, or an editor that round-trips to 3.1);
- if the document is simple, hand-edit: set `openapi: "3.1.0"`, and replace 3.0 `nullable: true`
  on a schema of type `T` with the 3.1 form `type: ["T", "null"]` (or, for a `$ref`,
  `oneOf: [{ "type": "null" }, { "$ref": … }]`).

Then:

```bash
spargen generate openapi-3.1.json --out src/api.rs
```

**Why the hard stop?** OpenAPI 3.0 and 3.1 differ in their schema model (3.1 adopts JSON Schema
2020-12; 3.0 uses its own dialect with `nullable`). Spargen targets the 3.1 dialect and refuses to
guess at a 3.0 document rather than mistranslate it — the rejection is the contract, per the
[support matrix](support-matrix.md). The vendored
[`corpus/recipes/poem-openapi.json`](../corpus/recipes/poem-openapi.json) is a 3.0.0 document; the
recipe test asserts it rejects with `E001`.

---

## Carving unsupported idioms

Real framework output occasionally contains a construct spargen cannot faithfully represent — for
example a JSON Schema `$dynamicRef`, which rejects with [`E006`](errors.md). By default one such
island rejects the **whole** document.

`--carve` is the escape hatch: it drops only the unrepresentable constructs (each reported once with
`W009`), then generates everything else. It reaches a fixpoint (cascading through any components that
referenced a carved schema) and stays deterministic.

```bash
# Rejects whole: one bad operation sinks the document.
spargen generate openapi.json --out src/api.rs
# => Rejected (E006)

# Generates the rest; the offending operation is dropped and reported as W009.
spargen generate openapi.json --out src/api.rs --carve
# => Generated (W009: get /measure omitted)
```

`spargen check --carve` audits the carved subset the same way. If you would rather remove specific
paths/operations/components by name (an exact or glob rule) instead of letting carve decide, use the
[compatibility omit mode](compatibility.md). The vendored
[`corpus/recipes/utoipa-untagged-overlap.json`](../corpus/recipes/utoipa-untagged-overlap.json)
demonstrates that overlapping `integer | number` unions no longer need this escape hatch: they
generate as typed trial-matching enums. The carve integration suite separately pins the
reject-then-carve flow for genuinely unsupported constructs.

---

## See also

- [Support matrix](support-matrix.md) — exactly what is supported, warned, or rejected.
- [Diagnostic index](errors.md) — every stable `E###`/`W###` code (`spargen explain E001`).
- [Compatibility omit mode](compatibility.md) — carve/omit unsupported segments by name.
- [`corpus/recipes/README.md`](../corpus/recipes/README.md) — provenance of the vendored specs and
  the verified per-framework version constants.
