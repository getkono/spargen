# Getting Started

This walkthrough installs spargen, generates a client from a spec, and shows the shape of the
generated API. The complete, runnable version is
[`examples/petstore`](https://github.com/getkono/spargen/tree/master/examples/petstore) — it
drives every generated feature against a local mock server.

## Install

Vendor the OpenAPI document and every local `$ref` target in your source tree, then choose one of
two compilation-time paths:

- Add `spargen` under `[build-dependencies]` and call `spargen::generate` from `build.rs`.
- Add `spargen-macro` under `[dependencies]` and invoke `generate_api!` in a Rust module.

Both crates are host/build-time only. The optional CLI provides analysis and vendoring tools; it
does not generate clients.

## Generate a client

`Spec` decides **what** is generated; `Build` adds **where**. Only `generate` writes a file, so
only `generate` takes a `Build`; `check`, `diff`, `requirements`, and `vendor` take a `Spec` and
never ask for an output path they would not use. Both types have private fields and chained
setters, so a new knob is additive rather than breaking.

`Spec`'s knobs are `uuid`, `time`, `omit`/`omit_rule`, `error_body_cap`, `batch_cap`, and
`carve`. They can equally be read from a `spargen.toml` (`Spec::config_file`,
`Spec::discover_config_file`) — the same file the CLI and the macro read, parsed by the library
so the three cannot drift. Setters called afterwards still win, so build code stays
authoritative.

`Report` reads through accessors — `report.diagnostics()`, `report.outcome()`, and
`report.truncated()`. A run that reaches `batch_cap` stops collecting and drops the rest;
`truncated()` is `true` in exactly that case, and the rendered report says so on its last line.
Treat a truncated report as a partial view: fixing everything it lists may not be enough, so raise
`batch_cap` to see the remainder.

### `build.rs` mode

```rust
// build.rs
fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let build = spargen::Spec::new("api/openapi.yaml").build(format!("{out_dir}/api.rs"));
    let report = spargen::generate(&build);
    assert_eq!(report.outcome(), spargen::Outcome::Generated, "{report:#?}");
}
```

`generate` emits Cargo dependency directives for the root schema, every transitive `$ref`, the
lock file, and the output. It keeps a content-addressed cache under `OUT_DIR`, and regenerates if
any input changes or if the output is missing or manually edited. To vendor the generated client,
use `"src/api.rs"` as the output and commit that file; generation still happens during compilation.

The generated file carries no crate-level inner attributes, so it drops straight in:

```rust
mod api {
    include!(concat!(env!("OUT_DIR"), "/api.rs"));
}
```

Your `[dependencies]` provide the runtime set the generated code needs. The
[petstore `Cargo.toml`](https://github.com/getkono/spargen/blob/master/examples/petstore/Cargo.toml)
is a copyable template.

### Runtime dependency contract

Use these tested caret floors. You may choose a higher compatible floor; spargen rejects a
requirement that could resolve below these versions or beyond the next semver breaking line.

| Dependency | Required features | When required |
| --- | --- | --- |
| `bytes = "1.12.1"` | `serde` only when noted below | Always; `serde` only when a generated serialized aggregate contains bytes |
| `reqwest = "0.12.28"` | `default-features = false`; `json` for JSON requests; `multipart` for multipart requests; `stream` for sequential responses | Always; the three features are spec-derived |
| `secrecy = "0.10.3"` | - | Always |
| `serde = "1.0.229"` | `derive` | Always |
| `serde_json = "1.0.151"` | - | Always |
| `futures-core = "0.3.32"` | - | Only for an API with sequential responses |
| `quick-xml = "0.41.0"` | `serialize` | Only for an API with XML bodies |
| `uuid = "1.24.0"` | `serde` | Only when the enabled UUID mapping is actually emitted |
| `time = "0.3.55"` | `formatting`, `parsing` | Only when an enabled date/date-time mapping is actually emitted |
| `tokio = "1.53.1"` | `rt`; optional and native-only | Only when your package declares the generated `blocking` feature |

The audit happens during both `build.rs` and proc-macro expansion and fails compilation with
`E023` before generated output is accepted. Cargo cannot add spec-derived features after it has
resolved dependencies: build scripts and proc macros run later in the compilation. Spargen
therefore checks the manifest, Cargo performs resolution, and rustc provides the final proof that
the selected versions expose every API and trait the generated client uses. Extra application
dependencies and features are allowed.

#### Cargo integration

Rebuild triggers and the consumer-manifest dependency audit (`E023`) both need a real
build-script process. `Build::cargo` decides what happens when there is not one:

| Value | Under a build script | Anywhere else |
| --- | --- | --- |
| `Auto` (default) | emit directives, run the audit | `W013` — no rebuild triggers, no audit |
| `Required` | emit directives, run the audit | `E024` — generation fails |
| `Off` | nothing, silently | nothing, silently |

A build script wants `Required`; a test or code-generation script wants `Off`. `W012` covers
the narrower case of a build script whose consuming manifest cannot be located, so the audit is
skipped even though directives were emitted.

### Macro mode

```rust
mod api {
    spargen_macro::generate_api!(spec = "api/openapi.yaml");
}
```

It accepts a positional schema path or `spec = "..."`, plus the same controls as `Spec`:
`no_uuid`, `no_time`, `carve`, `error_body_cap = N`, `batch_cap = N`, and an `omit { ... }`
profile. Cargo and rustc track the root schema, every transitive source file, and
`spargen.lock` through the expansion. See the
[`spargen-macro` README](https://github.com/getkono/spargen/tree/master/spargen-macro).

## The generated client API

Every spec lowers to the same surface, so the shape below is stable across clients:

```rust
use api::{types, Client, Credential};
use secrecy::SecretString;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
// `new` validates the base URL; `with_client` injects a preconfigured reqwest::Client
// (TLS choice, proxies, middleware, timeouts).
let client = Client::new("https://api.example.com")?
    .with_credential("bearerAuth", Credential::Bearer(SecretString::from("token")));

// One async method per operation. Required parameters are positional; optional parameters
// live in a per-operation `…Params` struct that derives `Default`.
let pets = client
    .list_pets(api::ListPetsParams { limit: Some(20), ..Default::default() })
    .await?;
let pets: Vec<types::Pet> = pets.into_inner(); // ResponseValue<T> → T

// A path parameter is positional; a string one takes `impl Into<String>`. Errors are a closed
// taxonomy of `Error<E>`, and every generated error type is a `std::error::Error` — which is what
// lets the `?` above and here flow into this function's `Box<dyn std::error::Error>`.
let pet = client.get_pet("1").await?.into_inner();
assert_eq!(pet.status, types::Status::Available);
# Ok(())
# }
```

Key points of the surface:

- `Client::new(base_url)` / `Client::with_client(reqwest::Client, base_url)`.
- One `async` method per operation returning `Result<ResponseValue<T>, Error<E>>`;
  `.into_inner()` unwraps the decoded body. Required string parameters take `impl Into<String>`
  and the `…Params` bundle takes `impl Into<Option<…>>`, so `None`, `Some(params)`, and `params`
  all compile.
- `Client::with_credential(scheme, credential)` registers static secrets (via `secrecy`) or async
  token providers. Operation `security` requirements pick the first satisfiable alternative and
  attach bearer/basic/apiKey credentials; a missing required credential is a
  request-construction error — typed as `RequestError::MissingCredential`, naming the schemes
  the operation's security requirement declares — never a silent 401.
- A closed [error taxonomy](./errors.md), identical across all spargen output:
  request-construction, transport, timeout, protocol, redirect, documented API error (typed `E`),
  undocumented status (raw body preserved), decode failure, interrupted body.
  Every generated error type implements `Display` and `std::error::Error`, so `Error<E>` works
  with `?` into `Box<dyn Error>`, `anyhow`, and `thiserror`. `Error::is_transient()` classifies
  retry-worthy failures — spargen ships no retry policy, but the runtime offers a bring-your-own
  [retry adapter](./runtime.md).
- Spec `title`/`summary`/`description` become rustdoc; `deprecated` becomes `#[deprecated]`.

## Next steps

- [CLI Reference](./cli.md) — `check`, `deps`, `lock`, `diff`, and `explain`.
- [Runtime & Ergonomics](./runtime.md) — retry, middleware, blocking, wasm, pagination, streaming.
- [Framework Recipes](./recipes.md) — generating from utoipa / aide / poem-openapi output.
- [OpenAPI 3.2 Scope](./openapi-3.2.md) — the focused delta from 3.1 and its disposition.
- [Feature Support](./support-matrix.md) — exactly which 3.1/3.2 constructs are handled.
