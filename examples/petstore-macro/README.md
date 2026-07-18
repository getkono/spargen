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

## The runtime-graph invariant

`spargen-macro` is a normal `[dependencies]` entry, but a proc-macro crate — and everything it
reaches, including `spargen` — is compiled for the host and **never linked into the binary**. So no
spargen crate is in the runtime graph:

```bash
cargo tree -e no-proc-macro -i spargen --manifest-path examples/petstore-macro/Cargo.toml
# prints nothing: spargen is not a runtime dependency
```

CI (`mise run example`) asserts exactly this.
