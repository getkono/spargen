# spargen-macro

The proc-macro front-end for [`spargen`](https://crates.io/crates/spargen): generate a typed,
compile-time-correct OpenAPI 3.1.x/3.2.x client **inline** — no `build.rs`, no CLI step.

```rust,ignore
mod api {
    // Resolved relative to your crate's Cargo.toml.
    spargen_macro::generate_api!("openapi.yaml");
}
```

Keyed form, with every argument the macro accepts besides the spec path. The `spargen.toml`
beside the spec is always discovered and applied first; `no_uuid`, `no_time`, `carve`,
`open_narrowing` and the two caps override it, and `omit { ... }` adds rules to its `[[omit]]`
rules:

```rust,ignore
spargen_macro::generate_api!(
    spec = "openapi.yaml",
    no_uuid,
    no_time,
    carve,
    open_narrowing,
    error_body_cap = 65536,
    batch_cap = 100,
    omit {
        operations { post "/legacy"; }
        paths { "/internal/**"; }
        components { schemas { "LegacyPet"; } }
        pointers { "/webhooks"; }
        file("shared.yaml") { pointers { "/Legacy"; } }
    }
);
```

## How it works

The macro is a thin shim over spargen's internal in-memory renderer. It resolves the spec,
renders the client, and parses the rendered source back into tokens. The generated API is the
same as the module written by
[`spargen::generate`](https://docs.rs/spargen/latest/spargen/fn.generate.html) from a `build.rs`.

## What you depend on

```toml
[dependencies]
spargen-macro = "0.5"
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
| `generate_api!` (this crate) | No (use `cargo expand`) | No build.rs |
| `build.rs` (`spargen::generate`) | Yes — in `OUT_DIR`, via `include!` | A few lines of build.rs |

Inline generation recompiles the whole generator (host-side) as part of your build. To vendor the
generated module, point the `build.rs` output directly at `src/api.rs` and commit it.

A generation failure is a `compile_error!` carrying spargen's diagnostics — no silent degradation.
Warnings are not surfaced through the macro (stable proc-macro APIs can't emit them); run
`spargen check <spec>` to see them.

### Optional `blocking` client

The generated code gates the synchronous `BlockingClient` behind a `blocking` feature (as it does
in every mode). When the client is inlined by the macro, that gate resolves against *your* crate's
features. A crate that does not declare it compiles the blocking client out cleanly, including
under `-D warnings`. To opt in, declare the feature as `blocking = ["dep:tokio"]` together with a
native-only optional `tokio`: `spargen deps <spec>` prints both, commented out, at the tested
floor, and
[`Requirements::manifest_block`](https://docs.rs/spargen/latest/spargen/struct.Requirements.html#method.manifest_block),
which renders that block, states how they are opted into.

Licensed under MIT OR Apache-2.0.
