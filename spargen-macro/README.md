# spargen-macro

The proc-macro front-end for [`spargen`](https://crates.io/crates/spargen): generate a typed,
compile-time-correct OpenAPI 3.1.x client **inline** — no `build.rs`, no CLI step.

```rust
mod api {
    // Resolved relative to your crate's Cargo.toml.
    spargen_macro::generate_api!("openapi.yaml");
}
```

Keyed form, with the same toggles as spargen's `build.rs` / CLI surface:

```rust
spargen_macro::generate_api!(spec = "openapi.yaml", no_uuid, no_time, carve);
```

## What you depend on

```toml
[dependencies]
spargen-macro = "0.1"
# ...plus the crates the generated client uses at runtime (reqwest, serde, serde_json, bytes,
# secrecy, and any optional uuid/time). No spargen crate appears at runtime.
```

`spargen-macro` and `spargen` are **host/build-time only** — a proc-macro crate is never linked
into your binary. `cargo tree -e no-proc-macro` shows no spargen crate.

## Choosing a mode

The macro expands to the **same bytes** as `spargen generate` and the `build.rs` API, so all three
are interchangeable and deterministic. Pick by what you want to see:

| Mode | Generated code visible? | Setup |
| --- | --- | --- |
| `generate_api!` (this crate) | No (use `cargo expand` or `spargen generate --out -`) | One dependency |
| `build.rs` (`spargen::generate`) | Yes — in `OUT_DIR`, via `include!` | A few lines of build.rs |
| CLI (`spargen generate --out src/api.rs`) | Yes — checked in | Run the CLI |

A generation failure is a `compile_error!` carrying spargen's diagnostics — no silent degradation.
Warnings are not surfaced through the macro (stable proc-macro APIs can't emit them); run
`spargen check <spec>` to see them.

### Optional `blocking` client

The generated code gates the synchronous `BlockingClient` behind a `blocking` feature (as it does
in every mode). When the client is inlined by the macro, that gate resolves against *your* crate's
features. A crate that does not declare it compiles the blocking client out cleanly, including
under `-D warnings`. To opt in, declare it in your `Cargo.toml`:

```toml
[features]
blocking = ["dep:tokio"]

[dependencies]
tokio = { version = "1", features = ["rt"], optional = true }
```

Licensed under MIT OR Apache-2.0.
