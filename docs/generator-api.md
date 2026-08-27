# Generator API

Spargen supports exactly two ways to generate a client, both invoked by Rust compilation against a
vendored schema:

1. `spargen::generate(&Build)` from `build.rs`, writing one `include!`-safe module either under
   Cargo's `OUT_DIR` or at a source path the application chooses to commit.
2. `spargen_macro::generate_api!(...)` inside a Rust module.

There is no generator CLI, stdout renderer, watcher, drift checker, or standalone-crate scaffold.
A consumer owns its `Cargo.toml` and decides whether generated source is committed.

## `spargen` crate

The intentional public facade is:

| Area | Public items | Purpose |
| --- | --- | --- |
| Inputs | `Spec`, `Build`, `CargoIntegration`, `ConfigError` | Describe what to generate and where to write it. |
| Generation | `generate`, `Report`, `Outcome`, `Diagnostic` | Write one generated module during a build and inspect the result. |
| Omission | `Omit`, `OmitRule`, `OmitMethod`, `ComponentKind`, `UnknownOmitToken`, `omit!` | Express reviewed compatibility omissions or enable `Spec::carve`. |
| Diagnostics | `Code`, `UnknownCode`, `Diagnostic`, `Severity`, `JsonPointer`, `Span`, `FileId`, `Loc`, `InterpId`, `explain` | Inspect stable generator diagnostics and their source locations. |
| Support audit | `check` | Run the same frontend without code generation. |
| Dependencies | `requirements`, `Requirements`, `RequiredDependency` | Print the `[dependencies]` block generated output needs, before building. |
| API diff | `diff`, `DiffReport`, `DiffRejection`, `Side`, `Change`, `ChangeKind`, `Impact` | Classify generated public-API changes without writing output. |
| Remote vendoring | `vendor`, `VendorOutcome`, `VendorReport`, `VendoredRef` | With `remote-fetch`, fetch and pin remote refs before compilation. |

### `Spec` and `Build`

`Spec` decides **what** is generated; `Build` adds **where**:

```rust
let build = spargen::Spec::new("api/openapi.yaml")
    .uuid(false)                       // `format: uuid` → String
    .carve(true)                       // omit unsupported constructs instead of failing
    .build("src/api.rs")               // → Build
    .cargo(spargen::CargoIntegration::Required);
spargen::generate(&build).expect_success();
```

Only `generate` writes a file, so only `generate` takes a `Build`; `check`, `diff`, `requirements`,
and `vendor` take a `Spec` and never ask for an output path they would not use. Both types have
private fields and chained setters, so a new knob is additive rather than breaking.

`Spec`'s knobs are `uuid`, `time`, `omit`/`omit_rule`, `error_body_cap`, `batch_cap`, and `carve`,
and they can equally be read from a `spargen.toml` (`Spec::config_file`,
`Spec::discover_config_file`) — the same file the CLI and the macro read, parsed by the library so
the three cannot drift. The build path fingerprints all of those controls plus the generator
implementation, root document, every transitive relative or vendored ref, and `spargen.lock`. Cargo
dependency directives and a content-addressed cache under `OUT_DIR` avoid rewriting unchanged
output while detecting missing, stale, or manually edited modules; a verified cache hit reports
`Outcome::Cached`.

### `CargoIntegration`

Rebuild triggers and the consumer-manifest dependency audit (`E023`) both need a real build-script
process. `Build::cargo` decides what happens when there is not one:

| Value | Under a build script | Anywhere else |
| --- | --- | --- |
| `Auto` (default) | emit directives, run the audit | `W013` — no rebuild triggers, no audit |
| `Required` | emit directives, run the audit | `E024` — generation fails |
| `Off` | nothing, silently | nothing, silently |

`W012` covers the narrower case of a build script whose consuming manifest cannot be located, so
the audit is skipped even though directives were emitted.

The doc-hidden `spargen::__private` module is a cross-crate implementation bridge for
`spargen-macro`; it is not an application API or a third generation path.

## `spargen-macro` crate

The only public item is `generate_api!`. It accepts a positional schema path or `spec = "..."`,
plus `no_uuid`, `no_time`, `carve`, `error_body_cap = N`, `batch_cap = N`, and an `omit { ... }`
profile. Cargo/rustc tracks the root schema, all transitive source files, and `spargen.lock` through
the expansion.

## CLI tooling

The optional `spargen` binary exposes five non-generation methods:

- `lock` fetches, vendors, and hash-pins remote refs. It is the only networked operation.
- `check` audits support against the vendored input without generating code.
- `deps` prints the exact `[dependencies]` block generated output from that spec requires.
- `diff` compares the generated public API represented by two vendored specs.
- `explain` prints the documentation for a stable diagnostic code.

`check`, `deps`, `lock`, and `diff` share one flag group — `--config`, `--carve`, `--no-uuid`,
`--no-time`, `--error-body-cap`, `--batch-cap`, and the four `--omit-*` flags — which is exactly the
setter list on `Spec`.

Shell commands may prepare or audit schemas, but client generation stays in `build.rs` or Rust
macro expansion.
