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

### `build.rs` mode

```rust
// build.rs
fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let config = spargen::Config::new(
        "api/openapi.yaml",
        format!("{out_dir}/api.rs"),
    );
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, spargen::Outcome::Generated, "{report:#?}");
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

Your `[dependencies]` provide the runtime set the generated code needs — `reqwest` (with the
`json` feature), `serde` (with `derive`), `serde_json`, `bytes`, and `secrecy`. The
[petstore `Cargo.toml`](https://github.com/getkono/spargen/blob/master/examples/petstore/Cargo.toml)
is a copyable template.

### Macro mode

```rust
mod api {
    spargen_macro::generate_api!(spec = "api/openapi.yaml");
}
```

The macro supports the same feature, cap, carve, and omit controls as `Config`; see the
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

// A path parameter is positional. Errors are a closed taxonomy of `Error<E>`.
let pet = client.get_pet("1".to_owned()).await?.into_inner();
assert_eq!(pet.status, types::Status::Available);
# Ok(())
# }
```

Key points of the surface:

- `Client::new(base_url)` / `Client::with_client(reqwest::Client, base_url)`.
- One `async` method per operation returning `Result<ResponseValue<T>, Error<E>>`;
  `.into_inner()` unwraps the decoded body.
- `Client::with_credential(scheme, credential)` registers static secrets (via `secrecy`) or async
  token providers. Operation `security` requirements pick the first satisfiable alternative and
  attach bearer/basic/apiKey credentials; a missing required credential is a
  request-construction error, never a silent 401.
- A closed [error taxonomy](./errors.md), identical across all spargen output:
  request-construction, transport, timeout, protocol, redirect, documented API error (typed `E`),
  undocumented status (raw body preserved), decode failure, interrupted body.
  `Error::is_transient()` classifies retry-worthy failures — spargen ships no retry policy, but
  the runtime offers a bring-your-own [retry adapter](./runtime.md).
- Spec `title`/`summary`/`description` become rustdoc; `deprecated` becomes `#[deprecated]`.

## Next steps

- [Runtime & Ergonomics](./runtime.md) — retry, middleware, blocking, wasm, pagination, streaming.
- [Framework Recipes](./recipes.md) — generating from utoipa / aide / poem-openapi output.
- [OpenAPI 3.2 Scope](./openapi-3.2.md) — the focused delta from 3.1 and its disposition.
- [Feature Support](./support-matrix.md) — exactly which 3.1/3.2 constructs are handled.
