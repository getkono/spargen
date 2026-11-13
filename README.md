# spargen

> **Alpha.** If it compiles, it should work. Issues are reviewed promptly.

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. Nothing older.

## Why

<!-- ANCHOR: why -->
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
<!-- ANCHOR_END: why -->

## Usage

Vendor the spec into your source tree, then generate from `build.rs` (`spargen` in
`[build-dependencies]`) or inline with the [`spargen-macro`](spargen-macro) proc macro. Either way,
spargen runs at build time only and never enters your runtime dependency tree.

```rust
// build.rs
let out_dir = std::env::var("OUT_DIR").unwrap();
spargen::generate(&spargen::Spec::new("api/openapi.yaml").build(format!("{out_dir}/api.rs")))
    .expect_success();
```

```rust
// or, with no build.rs
mod api {
    spargen_macro::generate_api!("openapi.yaml");
}
```

The [book](docs/book/src/SUMMARY.md) covers the generated API, the CLI, and the runtime;
[`docs/support-matrix.md`](docs/support-matrix.md) and [`docs/errors.md`](docs/errors.md) list what
is supported and every diagnostic. Runnable examples live in [`examples/`](examples).

## Development

Requires [Rust](https://rustup.rs) (the toolchain `rust-toolchain.toml` pins) and
[mise](https://github.com/jdx/mise).

```bash
mise install      # pinned tools
mise run hooks    # git hooks
mise run check    # build
mise run test     # tests
mise tasks        # every other task
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `docs:`, …); hooks and CI enforce it.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
