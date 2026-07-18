# Introduction

**spargen** is a compile-time-correct Rust client generator for OpenAPI **3.1.x**. Nothing else.

The name: a *spar* is the single load-bearing beam of an aircraft wing — sized on the drawing
board, carrying the entire span in flight with nothing propping it up. That is the product:
everything structural is decided at generation time; nothing is interpreted at runtime. Spec in,
spar out.

## The thesis

Three commitments define spargen and separate it from the existing generator ecosystem.

### Compile-time correctness

Spargen consumes an OpenAPI document at generation time and produces idiomatic, deterministic
Rust: typed models, a `Client`, one method per operation, and typed errors. **Generated code
compiles, or generation fails** — with a diagnostic that names the exact spec construct, its JSON
Pointer, and a remedy. Every spec construct has a disposition: it is supported, warned about, or
rejected. There is never a fourth, silent behavior, and a typed schema is never silently degraded
to `serde_json::Value`. The [feature support matrix](./support-matrix.md) and the
[diagnostic index](./errors.md) are the operational contract; `spargen explain E013` prints the
same text the docs carry.

### OpenAPI 3.1.x, natively

Most of the modern Rust server ecosystem emits OpenAPI **3.1** (utoipa, aide, poem-openapi —
everything downstream of JSON Schema 2020-12), but the ecosystem's client generators target
3.0.x. 3.1 is not a patch over 3.0: it replaces OpenAPI's bespoke schema dialect with real JSON
Schema 2020-12 (`nullable` → type arrays, numeric `exclusiveMinimum`, `$defs`, `prefixItems`,
`const`, …). The workaround in the wild — `sed`ing `openapi: 3.1.0` down to `3.0.0` before
generating — "works" only by accident and silently miscompiles any schema that uses 3.1
semantics.

Spargen speaks 3.1 natively and fails loudly and precisely on what it does not support. 3.0.x
input is **rejected** with a diagnostic (`E001`), never converted.

### A freestanding runtime

The runtime support code is embedded into the generated module; **no spargen crate ever appears
in a consumer's runtime dependency tree**. The default runtime dependencies are exactly
`reqwest` (no default features), `serde`, `serde_json`, `bytes`, and `secrecy`, plus opt-out
`uuid`/`time` for `format` mappings. Dependency hygiene is a first-class constraint, not an
afterthought — see [Runtime & Ergonomics](./runtime.md) for the opt-in transport, retry,
middleware, blocking, wasm, pagination, and streaming capabilities that keep that set intact.

## Design guarantees

- **Deterministic.** Same spargen version + spec + config ⇒ byte-identical output, enforced by
  test. Item ordering never depends on input map ordering.
- **No `serde(untagged)`.** First-match-wins deserialization can silently misparse;
  undiscriminated unions are rejected instead.
- **Safe by construction.** Unsafe-forbidding attributes ride on every generated item,
  `Debug`-redacted secrets (via [`secrecy`](https://docs.rs/secrecy)), and a configurable 64 KiB
  cap on error-body retention.
- **`include!`-friendly output.** Generated code carries no crate-level inner attributes, so it
  drops into a module or an `OUT_DIR` file consumed with `include!`.

## Where to next

- [Getting Started](./getting-started.md) — install, generate a client, and see the API shape.
- [CLI Reference](./cli.md) — every subcommand and flag.
- [Runtime & Ergonomics](./runtime.md) — the opt-in runtime capabilities.
- [Feature Support](./support-matrix.md) and [Diagnostics](./errors.md) — the operational
  contract.
