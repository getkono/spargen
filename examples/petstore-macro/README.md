# petstore-macro example

The **macro** consumption path: the client is generated inline by
`spargen_macro::generate_api!` — no `build.rs`, no `include!`, no CLI. Contrast with
[`examples/petstore`](../petstore), which uses the `build.rs` API for the same spec.

```bash
cargo run --manifest-path examples/petstore-macro/Cargo.toml
```

It spins up a local mock server and drives the generated client through a few typed calls
(list, create, fetch, a typed 404). The broader feature surface (auth failure modes, retry,
undocumented statuses, …) lives in the `petstore` example; this one exists to exercise the macro.
Its invocation exercises `no_uuid`, `no_time`, `carve`, `error_body_cap`, `batch_cap`, and an
`omit` block with a `components` rule; [spargen-macro's README](../../spargen-macro/README.md)
shows the rest. The expansion names, through `include_bytes!`, the spec, each file its `$ref`s
load, the manifests the runtime-dependency audit reads, and a `spargen.lock` and the applied
`spargen.toml` beside the spec when there are, so Cargo re-expands the macro when one of those
files changes; `schemas.yaml` is a relative-file `$ref` the spec reaches, there to exercise that.
A `spargen.toml` created after the last expansion is not yet on that list, so it takes effect only
once the macro is re-expanded for another reason.

## The runtime-graph invariant

`spargen-macro` is a normal `[dependencies]` entry, but a proc-macro crate — and everything it
reaches, including `spargen` — is compiled for the host and **never linked into the binary**. So no
spargen crate is in the runtime graph:

```bash
cargo tree -e no-proc-macro -i spargen --manifest-path examples/petstore-macro/Cargo.toml
# prints nothing: spargen is not a runtime dependency
```

CI (`mise run example`) asserts exactly this.
