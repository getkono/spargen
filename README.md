# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x. Nothing else.

The name: a *spar* is the single load-bearing beam of an aircraft wing — sized on the drawing
board, carrying the entire span in flight with nothing propping it up. That is the product:
everything structural is decided at generation time; nothing is interpreted at runtime. Spec in,
spar out.

> Note: We intend to support OpenAPI 3.2+ in the near future as demands is required. We choose to omit 3.0.x and older due to divergence; for Rust, Progenitor's client generator may suffice.

## Why

Most of the modern Rust server ecosystem emits OpenAPI **3.1** (utoipa, aide, poem-openapi —
everything downstream of JSON Schema 2020-12), but the ecosystem's client generators target
3.0.x. 3.1 is not a patch over 3.0: it replaces OpenAPI's bespoke schema dialect with real JSON
Schema 2020-12 (`nullable` → type arrays, numeric `exclusiveMinimum`, `$defs`, `prefixItems`,
`const`, …). The workaround in the wild — `sed`ing `openapi: 3.1.0` down to `3.0.0` before
generating — "works" only by accident and silently miscompiles any schema that uses 3.1
semantics.

Spargen speaks 3.1 natively, fails loudly and precisely on what it does not support, and treats
dependency hygiene as a first-class constraint. 3.0.x input is rejected with a diagnostic, never
converted.

## What it does

Spargen consumes an OpenAPI 3.1.x document (JSON or YAML, plus local relative-file `$ref`s) at
generation time and produces idiomatic, deterministic Rust: typed models, a `Client`, one method
per operation, and typed errors. Generated code compiles or generation fails — with a diagnostic
that names the exact spec construct, its JSON Pointer, and a remedy. Two ways to run it:

1. **CLI, checked-in output (recommended):** `spargen generate spec.yaml --out src/api.rs`.
   Spargen appears nowhere in your `Cargo.toml`; `spargen generate --check` fails CI when
   checked-in code drifts from the spec (`W004`).
2. **`build.rs`:** `spargen` as a `[build-dependencies]` entry, generating into `OUT_DIR` and
   consumed with `include!`. See [`examples/petstore`](examples/petstore) for the complete,
   runnable loop — it drives every generated feature against a local mock server.

```rust
// build.rs
let report = spargen::generate(&spargen::Config::new(
    "api/openapi.yaml",
    spargen::OutputTarget::Module(format!("{out_dir}/api.rs").into()),
));
assert_eq!(report.outcome, spargen::Outcome::Generated);
```

Generating a client from a spec that a Rust server framework emits (utoipa, aide, poem-openapi)?
The [framework round-trip recipes](docs/recipes.md) cover how each exports its OpenAPI document,
the version it emits, and the idioms spargen handles.

### Generated surface

- `Client::new(base_url)` / `Client::with_client(reqwest::Client, base_url)` — the injected
  client is the extension point for TLS choice, proxies, middleware, and timeouts.
- One `async` method per operation: required parameters positional, optional parameters in a
  per-operation `…Params` struct deriving `Default`, `Result<ResponseValue<T>, Error<E>>` out.
- `Client::with_credential(scheme, credential)` registers static secrets (via
  [`secrecy`](https://docs.rs/secrecy)) or async token providers; operation `security`
  requirements pick the first satisfiable alternative and attach bearer/basic/apiKey credentials.
  A missing required credential is a request-construction error, never a silent 401.
- A closed error taxonomy, identical across all spargen output: request-construction, transport,
  timeout, protocol, redirect, documented API error (typed `E`), undocumented status (raw body
  preserved), decode failure (serde path + capped body), interrupted body. `Error::is_transient()`
  classifies retry-worthy failures so any caller-side retry policy is trivial; spargen ships none.
- Spec `title`/`summary`/`description` become rustdoc; `deprecated` becomes `#[deprecated]`.

### Design guarantees

- **Freestanding output.** The runtime support code is embedded into the generated module; no
  spargen crate ever appears in a consumer's runtime dependency tree. Runtime dependencies are
  exactly `reqwest` (no default features), `serde`, `serde_json`, `bytes`, `secrecy`, plus
  opt-out `uuid`/`time` for `format` mappings.
- **Deterministic.** Same spargen version + spec + config ⇒ byte-identical output, enforced by
  test. Item ordering never depends on input map ordering.
- **Every construct has a disposition.** Supported, warned, or rejected — never a fourth, silent
  behavior; a typed schema is never silently degraded to `serde_json::Value`. The
  [support matrix](docs/support-matrix.md) and [diagnostic index](docs/errors.md) are the
  operational contract; `spargen explain E013` prints the same text the docs carry.
- **No `serde(untagged)`.** First-match-wins deserialization can silently misparse; undiscriminated
  unions are rejected instead.
- **`#![forbid(unsafe_code)]`-equivalent attributes on all generated items**, `Debug`-redacted
  secrets, and a 64 KiB (configurable) cap on error-body retention.

## Status

Implemented and verified today: the full pipeline for a substantial 3.1 subset — objects,
arrays, tuples, maps, scalar primitives and `format` mappings, homogeneous scalar enums,
`$ref`s (including self- and mutually-recursive schemas, whose cycle-closing references are
boxed), `allOf` merging, `oneOf`/`anyOf` unions, path/query/header/cookie parameters, JSON /
form-urlencoded / octet-stream / text bodies, per-status responses (including multi-status
success/error bodies lowered to typed per-operation response enums), auth attachment, and the
complete diagnostics surface (`check` / `generate` / `explain`, `--format json`, stable codes,
batch reporting).

Not yet implemented — all rejected or warned loudly, never silent: `multipart`/XML bodies
(`E009`). Diagnostics
carry file-level rather than line-precise spans for now. Large real-world specs (e.g. GitHub's)
exercise unsupported constructs and are expected to reject; the pinned [corpus](docs/corpus.md)
tracks exactly which, and the [compatibility omit mode](docs/compatibility.md) can carve
unsupported segments out of a vendored spec without editing it.

## Documentation

The full documentation site is an [mdBook](https://rust-lang.github.io/mdBook/) under
[`docs/book/`](docs/book) — an Introduction, Getting Started, and CLI/Runtime reference, wired
together with the [support matrix](docs/support-matrix.md), [diagnostic index](docs/errors.md),
[compatibility](docs/compatibility.md), [recipes](docs/recipes.md), [corpus](docs/corpus.md),
[benchmarks](docs/benchmarks.md), and [testing](docs/testing.md) docs (included, not duplicated).
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
| `mise run lint` | Clippy with warnings denied |
| `mise run test` | Full suite: unit, property, frontend-fixture, determinism, drift, and generated-code E2E tests |
| `mise run corpus-smoke` | Fast checks against pinned real-world specs |
| `mise run example` | Run the end-to-end petstore example |
| `mise run deny` | Supply-chain audit (licenses, advisories, bans) |

The validation strategy is documented per subsystem in [`docs/testing.md`](docs/testing.md).
Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/), enforced by
`convco` at commit time, pre-push, and in CI.

## Architecture

One published crate, internally partitioned into subsystems with a declared dependency DAG —
`diag`, `source`, `ir`, `oas31`, `name`, `support`, `codegen`, `emit`, `compat`, `cli`, and the
`lib.rs` facade. Everything that knows OpenAPI 3.1 syntax lives in the `oas31` frontend, which
lowers into a version-agnostic IR; codegen never sees a spec document. A future spec version
becomes a sibling frontend that lowers into the same IR and touches nothing downstream. The
emitted runtime is real, standalone-compilable source in the `support-runtime` workspace member
(`publish = false`), tested in its own right and embedded verbatim into output.

## Releases

Releases are automated via [release-plz](https://release-plz.dev): a standing pull request
tracks the next version bump; merging it tags the release and publishes to crates.io. Never bump
the version or tag manually. The semver surface is the public API of generated output: changes
that alter generated signatures, type shapes, or variant sets are major; output changes invisible
to that API are minor; generator-internal fixes are patch.

Publishing runs strictly in CI via crates.io [Trusted Publishing](https://crates.io/docs/trusted-publishing)
(OIDC) — no `CARGO_REGISTRY_TOKEN` secret. Bootstrap is one-time: the `0.1.0` release was published
manually to create the crate, after which a Trusted Publisher (`getkono/spargen`, workflow
`release-plz.yml`) was configured in the crate settings; every release since is automated.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
