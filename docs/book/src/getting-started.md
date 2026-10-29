# Getting Started

This walkthrough installs spargen, generates a client from a spec, and shows the shape of the
generated API. The complete, runnable version is
[`examples/petstore`](https://github.com/getkono/spargen/tree/master/examples/petstore) — it
drives the generated client against a local mock server: path, query, header and `deepObject`
parameters, JSON and multipart bodies, a multi-status response enum, typed servers and response
headers, bearer auth, typed and undocumented error statuses, RFC 9457 problems, and the retry
adapter. Its README lists exactly what each step exercises.

## Install

Vendor the OpenAPI document and every local `$ref` target in your source tree, then choose one of
two compilation-time paths:

- Add `spargen` under `[build-dependencies]` and call `spargen::generate` from `build.rs`.
- Add `spargen-macro` under `[dependencies]` and invoke `generate_api!` in a Rust module.

Both crates are host/build-time only. The optional [CLI](./cli.md) provides analysis and vendoring
tools; it has no `generate` subcommand.

## Generate a client

`Spec` decides **what** is generated; `Build` adds **where**. Only `generate` writes a file, so
only `generate` takes a `Build`; `check`, `diff`, `requirements`, and `vendor` take a `Spec` and
never ask for an output path they would not use. Both types have private fields and chained
setters, so a new knob is additive rather than breaking.

`Spec`'s knobs are `uuid`, `time`, `omit`/`omit_rule`, `error_body_cap`, `batch_cap`, `carve`,
and `open_narrowing` (see [problem details](./runtime.md#problem-details)). They can equally be read from a `spargen.toml` (`Spec::config_file`,
`Spec::discover_config_file`) — the same file the CLI and the macro read, parsed by the library
so the three cannot drift. Setters called afterwards still win, so build code stays
authoritative. `generate` names each applied file as a `cargo:rerun-if-changed` path, so editing
it regenerates the client; a `spargen.toml` created after the last build is picked up only once
the build reruns for another reason, such as touching the spec.

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
    // `Generated` on a cold build, `Cached` on a warm one — `expect_success` accepts both.
    spargen::generate(&build).expect_success();
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

`spargen deps <spec>` prints the exact `[dependencies]` block a spec's generated client needs:
each crate's tested caret floor and the features the compiled API uses, from the same table the
audit reads (see the [CLI reference](./cli.md#spargen-deps)). You may choose a higher compatible
floor; spargen rejects a requirement that could resolve below a floor or beyond the next semver
breaking line, and `reqwest` must have its default features off.

The blocking client's `tokio` may sit in any native-only `[target.…]` table, not only the spelling `spargen deps` prints: a
`build.rs` audit evaluates those tables for the target being built, as Cargo does (a wasm32 build
needs none), while `generate_api!`, which cannot see the target, requires them to jointly cover
every non-wasm target.

The audit happens during both `build.rs` and proc-macro expansion and fails compilation with
`E023` before generated output is accepted. Cargo cannot add spec-derived features after it has
resolved dependencies: build scripts and proc macros run later in the compilation. Spargen
therefore checks the manifest, Cargo performs resolution, and rustc provides the final proof that
the selected versions expose every API and trait the generated client uses. Extra application
dependencies and features are allowed.

#### Workspace inheritance and the full contract

A dependency may be declared `workspace = true` and inherited from `[workspace.dependencies]`;
the audit follows it to the workspace root the way Cargo does. The one trap is `reqwest`'s
default features: a member's `default-features = false` cannot turn off defaults the root leaves
on, so disable them in `[workspace.dependencies]` itself. The complete contract, including how the
root is found and what the diagnostic reports when it cannot be read, is the text
`spargen explain E023` prints, reproduced here verbatim:

> {{#include ../../../spargen/src/runtime_contract_e023_explain.txt}}

#### Cargo integration

Rebuild triggers and the consumer-manifest dependency audit (`E023`) both need a real
build-script process. `Build::cargo` decides what happens when there is not one:

| Value | Under a build script | Anywhere else |
| --- | --- | --- |
| `Auto` (default) | emit directives, run the audit | `W013` — no rebuild triggers, no audit |
| `Required` | emit directives, run the audit | `E024` — generation fails |
| `Off` | nothing, silently | nothing, silently |

A build script is served by `Auto`, the default; `Required` is the stricter opt-in for one where a
missing rebuild trigger would ship a stale client. A test or code-generation script wants `Off`. `W012` covers
the narrower case of a build script whose consuming manifest cannot be located, so the audit is
skipped even though directives were emitted.

### Macro mode

```rust
mod api {
    spargen_macro::generate_api!(spec = "api/openapi.yaml");
}
```

It accepts a positional schema path or `spec = "..."`, plus `no_uuid`, `no_time`, `carve`,
`open_narrowing`, `error_body_cap = N`, `batch_cap = N`, and an `omit { ... }` profile. The
`spargen.toml` beside the spec is always discovered and applied first, and those arguments
override it. Cargo and rustc track the root schema, every transitive source file,
`spargen.lock`, and the `spargen.toml` it applied through the expansion. See the
[`spargen-macro` README](https://github.com/getkono/spargen/tree/master/spargen-macro).

## The generated client API

Every spec lowers to the same surface, so the shape below is stable across clients:

```rust
use api::{types, Client, Credential};
use secrecy::SecretString;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
// `new` validates the base URL; `with_client` injects a preconfigured reqwest::Client
// (TLS choice, proxies, timeouts); `with_backend` installs a retry or middleware adapter.
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
  token providers, which operation `security` requirements attach; the README's
  [Generated surface](https://github.com/getkono/spargen#generated-surface) states how an
  alternative is selected and how each credential failure is typed.
- A closed error taxonomy, `Error<E>`, identical across all spargen output; every generated error
  type is a `std::error::Error`. The README's
  [Generated surface](https://github.com/getkono/spargen#generated-surface) lists every variant
  and the accessors over them; [problem details](./runtime.md#problem-details) and the
  [retry adapter](./runtime.md#retry) build on it.
- The README's [Generated surface](https://github.com/getkono/spargen#generated-surface) states how
  spec documentation metadata and `deprecated` reach rustdoc and `#[deprecated]`.

## Next steps

- [CLI Reference](./cli.md) — `check`, `deps`, `lock`, `diff`, and `explain`.
- [Runtime & Ergonomics](./runtime.md) — retry, middleware, blocking, wasm, pagination, streaming.
- [Framework Recipes](./recipes.md) — generating from utoipa / aide / poem-openapi output.
- [OpenAPI 3.2 Scope](./openapi-3.2.md) — the focused delta from 3.1 and its disposition.
- [Feature Support](./support-matrix.md) — exactly which 3.1/3.2 constructs are handled.
