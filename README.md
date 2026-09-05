# spargen

> Status (Alpha): If it compiles, it should work. Currently being adopted by a few projects by author. Issues will be reviewed promptly.

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. Nothing older.

The name: a *spar* is the single load-bearing beam of an aircraft wing — sized on the drawing
board, carrying the entire span in flight with nothing propping it up. That is the product:
everything structural is decided at generation time; nothing is interpreted at runtime. Spec in,
spar out.

## Why

Most of the modern Rust server ecosystem emits OpenAPI **3.1** (utoipa, aide, poem-openapi —
everything downstream of JSON Schema 2020-12), but the ecosystem's client generators target
3.0.x. 3.1 is not a patch over 3.0: it replaces OpenAPI's bespoke schema dialect with real JSON
Schema 2020-12 (`nullable` → type arrays, numeric `exclusiveMinimum`, `$defs`, `prefixItems`,
`const`, …). The workaround in the wild — `sed`ing `openapi: 3.1.0` down to `3.0.0` before
generating — "works" only by accident and silently miscompiles any schema that uses 3.1
semantics.

Spargen speaks 3.1 and its focused 3.2 extension natively, fails loudly and precisely on what it
does not support, and treats dependency hygiene as a first-class constraint. 3.0.x input is
rejected with a diagnostic, never converted.

## What it does

Spargen consumes an OpenAPI 3.1.x or 3.2.x document (JSON or YAML, plus local relative-file
`$ref`s) at generation time and produces idiomatic, deterministic Rust: typed models, a `Client`,
one method per operation, and typed errors. Generated code compiles or generation fails — with a
diagnostic that names the exact spec construct, its JSON Pointer, and a remedy. Schemas must be
vendored into the source tree before compilation. There are exactly two supported generation paths:

| Mode | How | Generated code |
| --- | --- | --- |
| **`build.rs`** | `spargen` in `[build-dependencies]` | write to `OUT_DIR` or a committed module path |
| **Macro** | [`spargen-macro`](spargen-macro): `generate_api!("spec.yaml")` | inline (use `cargo expand` to inspect) |

Both modes run as part of Rust compilation. Spargen is host/build-time only and **never enters your
runtime dependency tree**. The CLI is tooling for `lock`, `check`, `deps`, `diff`, and
`explain`; it cannot generate code, stream generated source, watch files, or scaffold a crate.

```rust
// build.rs — spargen appears only in [build-dependencies].
let out_dir = std::env::var("OUT_DIR").unwrap();
let report = spargen::generate(
    &spargen::Spec::new("api/openapi.yaml").build(format!("{out_dir}/api.rs")),
);
// `Generated` on a cold build, `Cached` on a warm one — `expect_success` accepts both.
report.expect_success();
```

```rust
// Or generate inline, no build.rs — see examples/petstore-macro.
mod api {
    spargen_macro::generate_api!("openapi.yaml");
}
```

See [`examples/petstore`](examples/petstore) (build.rs) and
[`examples/petstore-macro`](examples/petstore-macro) (macro) for complete, runnable loops that drive
every generated feature against a local mock server. Point the `build.rs` output at `src/api.rs`
instead when the generated client should be reviewed and committed.

Generating a client from a spec that a Rust server framework emits (utoipa, aide, poem-openapi)?
The [framework round-trip recipes](docs/recipes.md) cover how each exports its OpenAPI document,
the version it emits, and the idioms spargen handles.

### Generated surface

- `Client::new(base_url)` / `Client::with_client(reqwest::Client, base_url)` — the injected
  client is the extension point for TLS choice, proxies, and timeouts. `Client` is `Debug` and
  `Clone`.
- One `async` method per operation: required parameters positional, optional parameters in a
  per-operation `…Params` struct deriving `Default`, `Result<ResponseValue<T>, Error<E>>` out.
  String parameters and the params bundle take `impl Into<…>`, so `client.get_pet("1")` and
  `client.list_pets(params)` need no `to_owned()` or `Some`.
- `Client::with_credential(scheme, credential)` registers static secrets (via
  [`secrecy`](https://docs.rs/secrecy)) or async token providers; operation `security`
  requirements pick the first satisfiable alternative and attach bearer/basic/apiKey credentials.
  A missing required credential is a request-construction error, never a silent 401.
- A closed error taxonomy, identical across all spargen output: request-construction, transport,
  timeout, protocol, redirect, documented API error (typed `E`), undocumented status (raw body
  preserved), decode failure (serde path + body, capped except on the two paths the emitted
  `ClientConfig::max_error_body` doc names), interrupted body. Every generated error
  type is `Display` + `std::error::Error`, so `Error<E>` drops straight into `?`, `anyhow`, or
  `thiserror`. `Error::is_transient()` classifies retry-worthy failures. Every generated error
  type implements `ApiErrorBody`: an error enum whose documented statuses share one body type
  gets `body()`, and `Error::api_body()` hands that body back whichever status carried it (the
  status itself is `ResponseValue::status()` on `Error::Api`); an enum mixing body types is
  matched by variant instead.
- Beyond the request/response path, the embedded runtime carries a swappable transport seam
  (`HttpBackend`) with composable retry and middleware adapters, `Link:`-header pagination, typed
  SSE/NDJSON/JSON-sequence streams, an opt-in `blocking` client, and `wasm32-unknown-unknown`
  support. Each is opt-in and adds no dependency; spargen ships no retry *policy* and no async
  timer — the caller supplies both. See the
  [runtime reference](docs/book/src/runtime.md).
- Spec `title`/`summary`/`description` become rustdoc; `deprecated` becomes `#[deprecated]`.

### Design guarantees

- **Freestanding output.** The runtime support code is embedded into the generated module; no
  spargen crate ever appears in a consumer's runtime dependency tree. Runtime dependencies are
  exactly `reqwest` (no default features), `serde`, `serde_json`, `bytes`, and `secrecy`, plus only
  the capabilities the compiled API uses: `futures-core` and reqwest's `stream` feature for
  sequential responses, `quick-xml` for XML bodies, `time` for date formats, `uuid` for UUIDs, and
  an opt-in `tokio` only when the consumer declares its own `blocking` feature. Generation audits
  the consumer manifest against the
  tested floors and feature set before Cargo/rustc compile the emitted client; see the
  [runtime dependency contract](docs/book/src/getting-started.md#runtime-dependency-contract).
- **Deterministic.** Same spargen version + spec + config ⇒ byte-identical output, enforced by
  test. Item ordering never depends on input map ordering.
- **Edition-independent output.** The generated module is `include!`d into the consumer's crate and
  compiles under *that* crate's edition, so identifiers are escaped against the union of Rust's
  reserved words across editions 2015-2024: a spec that names a field `type` or `gen` emits
  `r#type` / `r#gen` rather than code only some editions accept. Wire names are unaffected — they
  ride on an explicit `serde(rename)`.
- **Every construct has a disposition.** Supported, warned, or rejected — never a fourth, silent
  behavior; a typed schema is never silently degraded to `serde_json::Value`. The
  [support matrix](docs/support-matrix.md) and [diagnostic index](docs/errors.md) are the
  operational contract; `spargen explain E013` prints the same text the docs carry.
- **No `serde(untagged)`.** First-match-wins deserialization can silently misparse; undiscriminated
  unions are rejected instead.
- **`#![forbid(unsafe_code)]`-equivalent attributes on all generated items**, `Debug`-redacted
  secrets, and a 64 KiB (configurable) cap on error-body retention — bounding reading too, for
  bodies read as errors on native targets. Two paths are not yet capped; the emitted
  `ClientConfig::max_error_body` doc names them.

## Status

Implemented and verified today: the full pipeline for substantial 3.1 and 3.2 surfaces — boolean
schemas, objects, arrays, tuples, maps, scalar primitives and `format` mappings, homogeneous scalar enums,
`$ref`s (including self- and mutually-recursive schemas, whose cycle-closing references are
boxed), `allOf` merging, `oneOf`/`anyOf` unions, path/query/header/cookie parameters, JSON /
form-urlencoded / octet-stream / text bodies, per-status responses (including multi-status
success/error bodies lowered to typed per-operation response enums), auth attachment, and the
complete diagnostics surface (`check` / `explain`, `--format json`, stable codes,
batch reporting). OpenAPI 3.2 adds canonical `$self` reference identity, `QUERY` and custom HTTP
methods, whole-query-string and cookie-style parameters, reusable media types, typed SSE/JSON
sequence streams (including JSON typed by SSE `data.contentSchema`), and expanded documentation
metadata. See the concise
[OpenAPI 3.2 scope](docs/openapi-3.2.md) for the exact delta from 3.1.

Anything outside the documented surface is rejected or warned loudly, never silent; see the
[support matrix](docs/support-matrix.md) for the exact boundary. Diagnostics carry file-level
rather than line-precise spans for now. The pinned GitHub OpenAPI 3.1 description generates and its
full emitted crate is compile-checked in CI; the remaining [corpus](corpus/README.md) pins both
generating and intentionally rejecting real-world cases.

## Documentation

The full documentation site is an [mdBook](https://rust-lang.github.io/mdBook/) under
[`docs/book/`](docs/book) — an Introduction, Getting Started, and CLI/runtime references, wired
together with the [OpenAPI 3.2 scope](docs/openapi-3.2.md), [support matrix](docs/support-matrix.md),
[diagnostic index](docs/errors.md), [compatibility](docs/compatibility.md), and
[recipes](docs/recipes.md) docs (included, not duplicated).
Build it locally:

```bash
cargo install mdbook        # one-time
mise run docs               # or: mdbook build docs/book
```

The rendered HTML lands in the git-ignored `docs/book/book/`; open `index.html` from there. CI
builds the book on every push so doc-site breakage is caught.

## Prerequisites

- [Rust](https://rustup.rs) (toolchain pinned by `rust-toolchain.toml`)
- [mise](https://github.com/jdx/mise) — dev tool provisioning and task runner
- [hk](https://hk.jdx.dev) — git hooks (installed via mise)

## Development

```bash
mise install          # provision hk, convco, cargo-deny
mise run hooks        # install git hooks
```

| Command | Description |
| --- | --- |
| `mise run check` | Type-check the workspace |
| `mise run fmt` | Format the workspace |
| `mise run fmt-check` | Verify the workspace is formatted |
| `mise run lint` | Clippy with warnings denied |
| `mise run test` | Full suite: unit, property, frontend-fixture, cache, determinism, and generated-code E2E tests |
| `mise run powerset` | Every feature combination via cargo-hack, not just `--all-features` |
| `mise run corpus-smoke` | Fast checks against pinned real-world specs |
| `mise run bench` | Criterion benchmarks over the generation pipeline |
| `mise run github-api` | Generate and compile the full pinned GitHub API client (native strict Clippy + wasm) |
| `mise run example` | Run the end-to-end petstore example |
| `mise run deny` | Supply-chain audit (licenses, advisories, bans) |
| `mise run docs` | Build the mdBook site (fails on broken links or includes) |
| `mise run doc-links` | Rustdoc over the workspace, warnings denied, private items included |

The validation strategy is documented per subsystem in
[`AGENTS.md`](AGENTS.md#testing-strategy-by-subsystem).
Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/), enforced by
`convco` at commit time, pre-push, and in CI.

## Architecture

The primary published crate is internally partitioned into subsystems with a declared dependency DAG —
`diag`, `source`, `ir`, `oas31`, `name`, `support`, `codegen`, `emit`, `compat`, `surface`, `cli`,
and the `lib.rs` facade. Everything that knows OpenAPI 3.1/3.2 syntax lives in the `oas31` frontend, which
lowers into a version-agnostic IR; codegen never sees a spec document. A future incompatible spec
version can become a sibling frontend that lowers into the same IR and touches nothing downstream. The
emitted runtime is real, standalone-compilable source in the `support-runtime` workspace member
(`publish = false`), tested in its own right and embedded verbatim into output.

## Releases

Releases are automated via [release-plz](https://release-plz.dev): a standing pull request
tracks the next version bump; merging it tags the release and publishes to crates.io. Never bump
the version or tag manually. The semver surface is the public API of generated output: changes
that alter generated signatures, type shapes, or variant sets are major; output changes invisible
to that API are minor; generator-internal fixes are patch.

Publishing runs strictly in CI via crates.io [Trusted Publishing](https://crates.io/docs/trusted-publishing)
(OIDC) — no `CARGO_REGISTRY_TOKEN` secret. Each crate was bootstrapped once: `spargen 0.1.0` and
`spargen-macro 0.2.0` were published manually to create their crates.io entries. Both now trust
`getkono/spargen`'s `release-plz.yml` workflow, which publishes `spargen` before its dependent
`spargen-macro`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
