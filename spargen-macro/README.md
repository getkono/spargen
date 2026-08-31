# spargen-macro

The proc-macro front-end for [`spargen`](https://crates.io/crates/spargen): generate a typed,
compile-time-correct OpenAPI 3.1.x/3.2.x client **inline** — no `build.rs`, no CLI step.

```rust
mod api {
    // Resolved relative to your crate's Cargo.toml.
    spargen_macro::generate_api!("openapi.yaml");
}
```

Keyed form, with the same controls as spargen's `build.rs` surface:

```rust
spargen_macro::generate_api!(
    spec = "openapi.yaml",
    no_uuid,
    no_time,
    carve,
    error_body_cap = 65536,
    batch_cap = 100,
    omit { operations { post "/legacy"; } }
);
```

## What you depend on

```toml
[dependencies]
spargen-macro = "0.4"
# ...plus the audited crates this generated client uses at runtime. No spargen crate appears at
# runtime. See spargen's runtime dependency contract for the exact floors and conditional features.
```

`spargen-macro` and `spargen` are **host/build-time only** — a proc-macro crate is never linked
into your binary. `cargo tree -e no-proc-macro` shows no spargen crate.

## Choosing a mode

The macro and `build.rs` use the same generator and produce the same client API. Pick by what you
want to see:

| Mode | Generated code visible? | Setup |
| --- | --- | --- |
| `generate_api!` (this crate) | No (use `cargo expand`) | One dependency |
| `build.rs` (`spargen::generate`) | Yes — in `OUT_DIR`, via `include!` | A few lines of build.rs |

To vendor the generated module, point the `build.rs` output directly at `src/api.rs` and commit it.

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
tokio = { version = "1.53.1", features = ["rt"], optional = true }
```

Licensed under MIT OR Apache-2.0.
