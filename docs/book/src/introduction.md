# Introduction

**spargen** is a compile-time-correct Rust client generator for OpenAPI **3.1.x and 3.2.x**.
Nothing older.

The name: a *spar* is the single load-bearing beam of an aircraft wing — sized on the drawing
board, carrying the entire span in flight with nothing propping it up. That is the product:
everything structural is decided at generation time; nothing is interpreted at runtime. Spec in,
spar out.

## Why

{{#include ../../../README.md:why}}

The [OpenAPI 3.2 scope](./openapi-3.2.md) summarizes the 3.2 delta; the 3.0.x rejection is
`E001`.

## What it guarantees

Generated code compiles, or generation fails with a diagnostic that names the construct, its JSON
Pointer, and a remedy. Every construct is supported, warned about, or rejected; the
[feature support matrix](./support-matrix.md) and the [diagnostic index](./errors.md) are that
operational contract. The rest of the design guarantees are listed once, in
[Design guarantees](./generated-api.md#design-guarantees).

## Where to next

- [Getting Started](./getting-started.md) — install, generate a client, and see the API shape.
- [CLI Reference](./cli.md) — every subcommand and flag.
- [Runtime & Ergonomics](./runtime.md) — the opt-in runtime capabilities.
- [Feature Support](./support-matrix.md) and [Diagnostics](./errors.md) — the operational
  contract.
