# Generator API

Spargen supports exactly two ways to generate a client, both invoked by Rust compilation against a
vendored schema:

1. `spargen::generate(&Config)` from `build.rs`, writing one `include!`-safe module either under
   Cargo's `OUT_DIR` or at a source path the application chooses to commit.
2. `spargen_macro::generate_api!(...)` inside a Rust module.

There is no generator CLI, stdout renderer, watcher, drift checker, or standalone-crate scaffold.
A consumer owns its `Cargo.toml` and decides whether generated source is committed.

## `spargen` crate

The intentional public facade is:

| Area | Public items | Purpose |
| --- | --- | --- |
| Generation | `Config`, `Features`, `generate`, `Report`, `Outcome` | Configure and write one generated module during a build. |
| Omission | `Omit`, `OmitRule`, `OmitMethod`, `ComponentKind`, `omit!` | Express reviewed compatibility omissions or enable `Config::carve`. |
| Diagnostics | `Code`, `UnknownCode`, `Diagnostic`, `Severity`, `JsonPointer`, `Span`, `FileId`, `Loc`, `InterpId`, `explain` | Inspect stable generator diagnostics and their source locations. |
| Support audit | `check` | Run the same frontend without code generation. |
| API diff | `diff`, `DiffOutcome`, `DiffReport`, `Change`, `ChangeKind`, `Impact` | Classify generated public-API changes without writing output. |
| Remote vendoring | `vendor`, `VendorOutcome`, `VendorReport`, `VendoredRef` | With `remote-fetch`, fetch and pin remote refs before compilation. |

`Config` is the complete generation configuration: `spec`, `output`, `features`, `omit`,
`error_body_cap`, `batch_cap`, and `carve`. The build path fingerprints all of those controls plus
the generator implementation, root document, every transitive relative or vendored ref, and
`spargen.lock`. Cargo dependency directives and a content-addressed cache under `OUT_DIR` avoid
rewriting unchanged output while detecting missing, stale, or manually edited modules.

The doc-hidden `spargen::__private` module is a cross-crate implementation bridge for
`spargen-macro`; it is not an application API or a third generation path.

## `spargen-macro` crate

The only public item is `generate_api!`. It accepts a positional schema path or `spec = "..."`,
plus `no_uuid`, `no_time`, `carve`, `error_body_cap = N`, `batch_cap = N`, and an `omit { ... }`
profile. Cargo/rustc tracks the root schema, all transitive source files, and `spargen.lock` through
the expansion.

## CLI tooling

The optional `spargen` binary exposes four non-generation methods:

- `lock` fetches, vendors, and hash-pins remote refs. It is the only networked operation.
- `check` audits support against the vendored input without generating code.
- `diff` compares the generated public API represented by two vendored specs.
- `explain` prints the documentation for a stable diagnostic code.

Shell commands may prepare or audit schemas, but client generation stays in `build.rs` or Rust
macro expansion.
