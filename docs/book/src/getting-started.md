# Getting Started

This walkthrough installs spargen, generates a client from a spec, and shows the shape of the
generated API. The complete, runnable version is
[`examples/petstore`](https://github.com/getkono/spargen/tree/master/examples/petstore) — it
drives every generated feature against a local mock server.

## Install

Spargen builds from source with a stable Rust toolchain. Two ways to run it:

- **CLI, checked-in output (recommended):** install the binary and commit the generated file.

  ```bash
  cargo install spargen --features cli
  spargen generate api/openapi.yaml --out src/api.rs
  ```

  Spargen appears nowhere in your `Cargo.toml`. `spargen generate --check` fails CI when the
  checked-in code drifts from the spec (`W004`).

- **`build.rs`:** add `spargen` as a `[build-dependencies]` entry, generate into `OUT_DIR`, and
  consume the result with `include!`. Nothing spargen-shaped is left in your runtime graph.

## Generate a client

### CLI mode

```bash
# Emit a module you check in and import with `mod api;`
spargen generate api/openapi.yaml --out src/api.rs

# Or a standalone, publishable crate
spargen generate api/openapi.yaml --out crates/api --as-crate
```

See the [CLI Reference](./cli.md) for every subcommand and flag (`check`, `lock`, `explain`,
`diff`, and generation flags like `--carve`, `--watch`, `--config`, and the `--omit-*` family).

### `build.rs` mode

```rust
// build.rs
fn main() {
    println!("cargo:rerun-if-changed=api/openapi.yaml");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let config = spargen::Config::new(
        "api/openapi.yaml",
        spargen::OutputTarget::Module(format!("{out_dir}/api.rs").into()),
    );
    let report = spargen::generate(&config);
    assert_eq!(report.outcome, spargen::Outcome::Generated, "{report:#?}");
}
```

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
- [Feature Support](./support-matrix.md) — exactly which 3.1 constructs are handled.
