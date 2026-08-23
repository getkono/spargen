# Migrating to spargen 0.3

0.3 changes the generator's public API and moves several constructs from *rejected* or *silently
dropped* to *supported*. Nothing about the shape of generated clients changed for constructs that
already worked; what changed is the API you call, and how much of a spec spargen accepts.

## Generation inputs: `Config` → `Spec` + `Build`

`Config` required an output path from every entry point, including the four that never write one.
It is replaced by two types: `Spec` decides **what** is generated, `Build` adds **where**.

```rust
// 0.2
let mut config = spargen::Config::new("api/openapi.yaml", "src/api.rs");
config.features.uuid = false;
config.carve = true;
let report = spargen::generate(&config);

// 0.3
let build = spargen::Spec::new("api/openapi.yaml")
    .uuid(false)
    .carve(true)
    .build("src/api.rs");
let report = spargen::generate(&build);
```

- `generate` takes a `&Build`. `check`, `diff`, `vendor`, and the new `requirements` take a `&Spec`
  and no longer ask for an output path they would not use.
- Fields are private; every knob is a chained setter, so future knobs are additive rather than
  breaking. `uuid`, `time`, `omit`/`omit_rule`, `error_body_cap`, `batch_cap`, `carve` on `Spec`;
  `cargo` on `Build`.
- **`Features` is deleted.** `features.uuid`/`features.time` become `Spec::uuid`/`Spec::time`. The
  struct never held what its name suggested: `xml` and `streams` are derived from the spec, and
  `blocking` is a Cargo feature of the *consuming* crate. In 0.3, "feature" means a Cargo feature
  and nothing else.

## Cargo integration is now a decision, not a silent degradation

`generate` outside a build script cannot emit `cargo:rerun-if-changed` directives (so an edited
spec will not trigger a rebuild) and cannot find a consumer manifest to audit (so `E023` goes
unchecked). 0.2 degraded silently. 0.3 asks:

```rust
use spargen::CargoIntegration;

Spec::new("openapi.yaml").build("src/api.rs").cargo(CargoIntegration::Required); // E024 if absent
Spec::new("openapi.yaml").build("src/api.rs").cargo(CargoIntegration::Off);      // silent, by request
```

`Auto` is the default and matches 0.2's behavior, plus a `W013` (or `W012`) saying what was
skipped. A build script wants `Required`; a test or code-gen script wants `Off`.

## Reports and outcomes

- `Outcome` gains `Cached`. A verified cache hit reported `Generated` in 0.2, which made
  `wrote_output` a lie. Code matching `Outcome::Generated` exactly should either accept both or use
  `Report::succeeded` / `Report::expect_success`.
- `Report` gains `errors()`, `warnings()`, `has_errors()`, `succeeded()`, `into_result()`,
  `emit_cargo_warnings()`, `expect_success()`, plus `Display`, `Serialize`, and `Error`. Folding
  over `diagnostics` to decide success is no longer necessary.
- `diff` returns `Result<DiffReport, DiffRejection>` instead of a tri-state `DiffOutcome`.
- `explain` returns `Result<&'static str, UnknownCode>` instead of `Option`.
- `Code` serializes as its stable string (`"W009"`), not its Rust variant name.

## `spargen.toml`

The keys under `[features]` moved to the top level, and `uuid`, `time`, and `error_body_cap`
joined them:

```toml
# 0.2                # 0.3
# [features]         uuid = true
# batch_cap = 100    time = true
# carve = false      carve = false
                     batch_cap = 100
                     error_body_cap = 65536
```

The old spelling is a clear migration error, not an "unknown field". The file is now parsed by the
library, so `build.rs` (`Spec::discover_config_file`) and `generate_api!` read the same schema the
CLI does.

## CLI

- New `spargen deps <spec>` prints the exact `[dependencies]` block generated output from that spec
  requires — the same table the `E023` audit reads, so what it prints is what the audit accepts.
- `check`, `deps`, `lock`, and `diff` share one options group: `--config`, `--carve`, `--no-uuid`,
  `--no-time`, `--error-body-cap`, `--batch-cap`, and the four `--omit-*` flags.

## Specs that behaved differently

These are wire-visible or acceptance changes; regenerate and review.

- **Query values are no longer over-encoded.** 0.2 percent-encoded whole query values, so a style's
  joining `,` and a `,` inside a value were both `%2C` and indistinguishable. Delimiters are now
  literal and data bytes are encoded.
- **Path values are now percent-encoded.** A path parameter containing `/` used to become extra
  path segments — a different route than the document describes.
- **Every parameter style is implemented**: `matrix`, `label`, `spaceDelimited`, `pipeDelimited`,
  `deepObject`, and `allowReserved: true` were `E010` and now generate.
- **Encoding Objects are implemented.** Multipart parts now carry a resolved `Content-Type`, and
  form-urlencoded bodies are built by spargen rather than by `serde_urlencoded` (which failed at
  *runtime* on any array or object property).
- **`requestBody.required: false`** now produces `Option<&T>` in the generated signature.
- **Path Item `$ref` is resolved**, so specs that previously generated an empty client now generate
  their operations. A `$ref` beside structural siblings is `E015` — the specification leaves that
  case undefined.
- **`xml.namespace`/`xml.prefix`/`xml.wrapped`** were a warning everywhere; on a type actually
  serialized as XML they are now `E009`, because ignoring them puts structurally different XML on
  the wire.
- **New diagnostics**: `E015`, `E016`, `E024`, `W011`, `W012`, `W013` — see
  [the diagnostic index](errors.md).
